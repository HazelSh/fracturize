//! Random flames: a scrolling list of rolls down the left of the screen, each
//! a live thumbnail, click one to open it.
//!
//! The model is `random_list` (rolled on background threads, under the
//! person's random settings) and the thumbnails are `gpu::thumbs`, shared
//! with the scene browser — so a row here behaves like a row there: the one
//! under the pointer flies its camera, and the rest are stills. The list grows
//! as it is scrolled; there is no end to reach.
//!
//! Opening a row is an *edit*, like the dice: one Ctrl+Z brings back what was
//! on screen. So browsing here is safe — click through as many as you like,
//! and the History window holds every one you looked at.

use egui::text::{LayoutJob, TextWrapping};

use crate::app::App;
use crate::gpu::thumbs::{ThumbKey, ThumbState};
use crate::random_list::RandomEntry;

use super::hints::hinted;
use super::icons;
use super::num;

/// Thumbnail size, in points, at its widest. A narrower window draws it
/// smaller rather than asking for a new one, so resizing the window never
/// re-renders the list.
const THUMB: egui::Vec2 = egui::vec2(256.0, 144.0);
const PAD: f32 = 4.0;
/// Two caption lines under the thumbnail.
const CAPTION_H: f32 = 34.0;
/// Rows rolled ahead of the last one on screen, so scrolling meets rows that
/// are already there rather than ones being rolled.
const LOOKAHEAD: usize = 8;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    if !app.ui_state.panels.random_open {
        return;
    }
    let mut open = true;
    let mut adopt = None;
    let mut hovered = None;
    let mut want = LOOKAHEAD;
    let mut reroll = false;
    let mut persist = false;

    super::window(ctx, app, super::WindowKey::Random, "Random flames")
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Panel::top("fracturize_random_bar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    let resp = ui.button((icons::REFRESH, "New rolls"));
                    let resp = hinted(
                        resp,
                        &mut app.ui_state,
                        "Throw this list away and roll a fresh one under the same settings",
                        "click: roll a new list",
                    );
                    reroll |= resp.clicked();

                    let resp = ui.toggle_value(
                        &mut app.ui_state.panels.random_settings_open,
                        (icons::SLIDERS, "Settings"),
                    );
                    hinted(
                        resp,
                        &mut app.ui_state,
                        "What the dice may roll: variations and their weights, how many maps, \
                         and bounds on dimension and lacunarity. The list starts over when \
                         they change.",
                        "toggle the Random settings window",
                    );
                });
                status_line(ui, app);
                ui.add_space(2.0);
            });
            let (end, a, h, p) = list(ui, app);
            want = end + LOOKAHEAD;
            adopt = a;
            hovered = h;
            persist = p;
        });
    super::remember(ctx, app, super::WindowKey::Random);

    if reroll {
        app.reroll_random_list();
    }
    if persist {
        app.random_list.persist();
    }
    app.poll_random_list(want);
    if let Some(id) = hovered {
        app.thumbs.hover(ThumbKey::Random(id));
    }
    if let Some(i) = adopt {
        app.adopt_random(i);
    }
    app.ui_state.panels.random_open = open;
}

/// What the list has found so far under these settings.
fn status_line(ui: &mut egui::Ui, app: &mut App) {
    let list = &app.random_list;
    let rows = list.entries.len();
    let text = match (rows, list.tries_per_flame()) {
        (0, _) if list.rolling() => "Rolling…".to_string(),
        (0, _) => "Nothing yet.".to_string(),
        (n, Some(t)) => format!(
            "{n} flame{} · {} tries each{}",
            if n == 1 { "" } else { "s" },
            num::fixed(t, 1),
            if list.failures > 0 { format!(" · {} came up empty", list.failures) } else { String::new() },
        ),
        (n, None) => format!("{n} flames"),
    };
    let resp = ui.add(
        egui::Label::new(egui::RichText::new(text).small().weak()).sense(egui::Sense::hover()),
    );
    let rejected = list.rejected;
    let tip = if rejected.total() == 0 {
        "How many candidates each flame took to find. One means the settings ask for \
         nothing unusual."
            .to_string()
    } else {
        format!(
            "How many candidates each flame took to find. Thrown away so far: {}.\n\n\
             A roll that tries {} candidates and finds nothing comes up empty. Shape \
             rejections are the quality gate (collapsed, diverged, a line or a lump); the \
             others are your bounds.",
            rejected.describe(),
            list.settings().attempts,
        )
    };
    hinted(resp, &mut app.ui_state, tip, "how hard the dice are working for these settings");
}

/// The rows. Returns the last row index on screen, the row clicked, the
/// roll under the pointer, and whether "keep trying" was pressed.
fn list(ui: &mut egui::Ui, app: &mut App) -> (usize, Option<usize>, Option<u64>, bool) {
    let ppp = ui.ctx().pixels_per_point();
    let px = ((THUMB.x * ppp).round() as u32, (THUMB.y * ppp).round() as u32);
    let open_name = app.scene.name.clone();
    let custom = *app.random_settings() != Default::default();

    let (list, thumbs, ui_state) = app.random_list_parts();
    let width = (ui.available_width() - 2.0 * PAD).clamp(64.0, THUMB.x);
    let thumb_size = egui::vec2(width, width * THUMB.y / THUMB.x);
    let row_h = thumb_size.y + CAPTION_H + 2.0 * PAD;

    let mut end = 0;
    let mut adopt = None;
    let mut hovered = None;
    let mut persist = false;
    // One more row than there are flames: the foot, saying what's coming.
    let rows = list.entries.len() + 1;
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .id_salt("fracturize_random_list")
        .auto_shrink([false, false])
        .show_rows(ui, row_h, rows, |ui, range| {
            end = range.end;
            for i in range {
                let Some(entry) = list.entries.get(i) else {
                    persist = foot(ui, list, ui_state, row_h);
                    continue;
                };
                let is_open = entry.scene.name == open_name;
                let resp = row(ui, entry, is_open, thumb_size, row_h, px, thumbs);
                let resp = hinted(resp, ui_state, tooltip(entry, custom), "click: open this flame (undoable) · the row under the pointer flies its camera");
                if resp.hovered() {
                    hovered = Some(entry.id);
                }
                if resp.clicked() {
                    adopt = Some(i);
                }
            }
        });
    (end, adopt, hovered, persist)
}

fn tooltip(entry: &RandomEntry, custom: bool) -> String {
    format!(
        "Seed {seed}. Click to open it — an edit, so one Ctrl+Z brings back what you had.\n\n\
         From the command line:\n  fracturize --random --seed {seed}{config}",
        seed = entry.seed,
        config = if custom { " --random-config ~/.config/fracturize/prefs.toml" } else { "" },
    )
}

/// The row after the last flame: rolling, or stuck and why.
fn foot(
    ui: &mut egui::Ui,
    list: &crate::random_list::RandomList,
    ui_state: &mut super::UiState,
    row_h: f32,
) -> bool {
    let mut persist = false;
    ui.allocate_ui(egui::vec2(ui.available_width(), row_h), |ui| {
        ui.add_space(8.0);
        if list.stalled() {
            ui.label(
                egui::RichText::new(format!(
                    "The last {} rolls found nothing within the settings in {} tries each \
                     (rejected for {}).",
                    list.empty_run(),
                    list.settings().attempts,
                    list.rejected.describe(),
                ))
                .small(),
            );
            let resp = ui.button("Keep trying");
            let resp = hinted(
                resp,
                ui_state,
                "Roll on anyway. Loosening a bound, or allowing more tries in the \
                 settings, is usually the better fix.",
                "click: keep rolling under these settings",
            );
            persist = resp.clicked();
        } else {
            ui.label(egui::RichText::new("Rolling…").small().weak());
        }
    });
    persist
}

fn row(
    ui: &mut egui::Ui,
    entry: &RandomEntry,
    is_open: bool,
    thumb_size: egui::Vec2,
    row_h: f32,
    px: (u32, u32),
    thumbs: &mut crate::gpu::thumbs::ThumbCache,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::click());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals().clone();
    if is_open {
        painter.rect_filled(rect, 3.0, visuals.selection.bg_fill.gamma_multiply(0.6));
    } else if resp.hovered() {
        painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
    }

    let thumb = egui::Rect::from_min_size(rect.min + egui::vec2(PAD, PAD), thumb_size);
    match thumbs.get(&ThumbKey::Random(entry.id), None, px, &entry.scene) {
        ThumbState::Ready(id) => {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(id, thumb, uv, egui::Color32::WHITE);
        }
        // The flame's own ground until its picture lands a frame later.
        ThumbState::Pending => {
            let [r, g, b] = crate::palette::to_srgb8(entry.scene.background);
            painter.rect_filled(thumb, 0.0, egui::Color32::from_rgb(r, g, b));
        }
        ThumbState::Failed(_) => {
            painter.rect_filled(thumb, 0.0, visuals.extreme_bg_color);
            painter.text(
                thumb.center(),
                egui::Align2::CENTER_CENTER,
                "no preview",
                egui::FontId::proportional(11.0),
                visuals.weak_text_color(),
            );
        }
    }

    let x = thumb.left();
    let width = thumb.width();
    let top = thumb.bottom() + 3.0;
    let n = entry.scene.transforms.len();
    let kinds = if entry.kinds.is_empty() { "affine".to_string() } else { entry.kinds.join(" + ") };
    let title = format!(
        "{}{n} maps · {}{kinds}",
        if is_open { "(open) " } else { "" },
        entry.symmetry.as_ref().map_or(String::new(), |g| format!("{g} · ")),
    );
    let body = egui::TextStyle::Body.resolve(ui.style());
    line(&painter, egui::pos2(x, top), title, body, visuals.strong_text_color(), width);

    let measure = |v: Option<f32>, d: usize| v.map_or("–".to_string(), |v| num::fixed(v, d));
    let meta = format!(
        "d {} · Λ {} · seed {}",
        measure(entry.dimension, 2),
        measure(entry.lacunarity, 1),
        entry.seed,
    );
    let small = egui::TextStyle::Small.resolve(ui.style());
    line(&painter, egui::pos2(x, top + 17.0), meta, small, visuals.weak_text_color(), width);
    resp
}

/// One line of text, cut off with an ellipsis at `width`.
fn line(
    painter: &egui::Painter,
    pos: egui::Pos2,
    text: String,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) {
    let mut job = LayoutJob::simple_singleline(text, font, color);
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.galley(pos, painter.layout_job(job), color);
}
