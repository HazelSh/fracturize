//! Mutation grid: the scene in the centre, a ring of its mutations round it —
//! look, then choose, rather than roll, judge and undo.
//!
//! The model is `mutation_grid` (made on background threads, through the same
//! gate as `U`) and the thumbnails are `gpu::thumbs`. Unlike the browser and
//! the Random flames list, **every tile draws from the main view's camera**:
//! orbit the scene, play its path, zoom, and all of them turn with it, so any
//! mutation can be judged from any angle and the tiles always compare like
//! with like. A drag on a tile turns that shared camera, and the wheel over
//! one zooms it; a click — which is a press that didn't become a drag, so it
//! works while the camera is playing — picks the tile.
//!
//! Picking a tile is an edit, named for what the mutation did. The ring then
//! grows round it, and Ctrl+Z brings back the previous centre with the ring
//! it was picked from, so a sibling can be picked instead. Clicking the
//! centre deals a new ring round the same scene.

use crate::app::App;
use crate::gpu::thumbs::{ThumbKey, ThumbState};
use crate::mutation_grid::{cells, GridSize, TileState};
use crate::scene::Scene;

use super::hints::hinted;
use super::icons;
use super::num;

/// Space between tiles, in points.
const GAP: f32 = 4.0;

/// Tile pixel sizes snap to this, so resizing the window doesn't rebuild
/// every tile on every frame of the drag.
const SNAP_PX: u32 = 16;

/// What a press on a tile turned out to be.
enum Act {
    Adopt(usize),
    Reroll,
}

pub fn draw(ctx: &egui::Context, app: &mut App) {
    if !app.ui_state.panels.mutation_grid_open {
        return;
    }
    let mut open = true;
    let mut picked = None;
    let mut size = None;
    let mut reroll = false;
    let mut act = None;
    let mut orbit = egui::Vec2::ZERO;
    let mut zoom = 0.0;

    super::window(ctx, app, super::WindowKey::MutationGrid, "Mutation grid")
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Panel::top("fracturize_mutation_grid_bar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    let current = app.mutation_grid_size();
                    size = super::radio::radio(&mut app.ui_state, "mutation_grid_size", current)
                        .option(GridSize::Three, "3×3", "Eight mutations round the scene", "click: a 3×3 grid")
                        .option(
                            GridSize::Five,
                            "5×5",
                            "Twenty-four: the inner ring at the profile's strength, the outer ring at \
                             twice it, so one grid shows near and far",
                            "click: a 5×5 grid",
                        )
                        .show(ui);
                    let resp = ui.button((icons::REFRESH, "New ring"));
                    let resp = hinted(
                        resp,
                        &mut app.ui_state,
                        "Deal a new ring of mutations round the same scene (or click the centre)",
                        "click: new mutations of this scene",
                    );
                    reroll |= resp.clicked();
                });
                ui.horizontal(|ui| {
                    ui.label("Profile");
                    picked = super::random_settings::profile_picker(ui, app, 150.0);
                    let resp = ui.toggle_value(
                        &mut app.ui_state.panels.random_settings_open,
                        (icons::SLIDERS, "Settings…"),
                    );
                    hinted(
                        resp,
                        &mut app.ui_state,
                        "Which parts of the flame a mutation may change, and how hard. The ring \
                         starts over when they change.",
                        "toggle the Random settings window",
                    );
                });
                status_line(ui, app);
                ui.add_space(2.0);
            });
            (act, orbit, zoom) = grid(ui, app);
        });
    super::remember(ctx, app, super::WindowKey::MutationGrid);

    if let Some(entry) = picked {
        app.load_dice_profile(entry);
    }
    if let Some(size) = size {
        app.set_mutation_grid_size(size);
    }
    if orbit != egui::Vec2::ZERO {
        app.orbit_view_by(orbit.x, orbit.y);
    }
    if zoom != 0.0 {
        app.zoom_view_by(zoom);
    }
    match act {
        Some(Act::Adopt(n)) => app.adopt_grid_tile(n),
        Some(Act::Reroll) => app.reroll_mutation_grid(),
        None if reroll => app.reroll_mutation_grid(),
        None => {}
    }
    app.poll_mutation_grid();
    app.ui_state.panels.mutation_grid_open = open;
}

/// Seed, progress, and the strengths the rings are at.
fn status_line(ui: &mut egui::Ui, app: &mut App) {
    let grid = &app.mutation_grid;
    let text = match (grid.seed(), grid.settings()) {
        (Some((seed, size)), Some(settings)) => {
            let made = grid.tiles().len() - grid.pending();
            let empty = grid.tiles().iter().filter(|t| matches!(t.state, TileState::Empty(_))).count();
            let strength = settings.mutation.strength;
            format!(
                "seed {seed} · {}{}{} · strength {}{}",
                if grid.pending() > 0 { format!("{made} of {} made", size.children()) } else { "made".into() },
                if empty > 0 { format!(", {empty} empty") } else { String::new() },
                if grid.settling() { " · updating…" } else { "" },
                num::fixed(strength, 2),
                if size == GridSize::Five { format!(", outer {}", num::fixed(2.0 * strength, 2)) } else { String::new() },
            )
        }
        _ => "Mutating…".to_string(),
    };
    let resp = ui.add(egui::Label::new(egui::RichText::new(text).small().weak()).sense(egui::Sense::hover()));
    let tip = match grid.seed() {
        Some((seed, size)) => format!(
            "Tile k is mutation seed {seed}+k, numbered row by row round the centre. The same \
             grid from the command line, with this scene saved as scene.toml:\n  \
             fracturize -s scene.toml -r grid.png --mutations {} --seed {seed}{}",
            size.children(),
            app.dice_config_flag(),
        ),
        None => "The first ring is on its way.".to_string(),
    };
    hinted(resp, &mut app.ui_state, tip, "the grid's seed, and the command that draws it again");
}

/// The tiles, as large as the window allows at the main view's aspect.
/// Returns what was clicked, how far a tile was dragged (in pixels) and
/// how far the wheel turned over one.
fn grid(ui: &mut egui::Ui, app: &mut App) -> (Option<Act>, egui::Vec2, f32) {
    let ppp = ui.ctx().pixels_per_point();
    let (w, h) = app.gpu.size();
    let aspect = (w as f32 / h.max(1) as f32).clamp(0.25, 4.0);
    let wanted = app.mutation_grid_size();
    let side = wanted.side();
    let avail = ui.available_size();
    let n = side as f32;
    let tile_w = ((avail.x - GAP * (n - 1.0)) / n).min((avail.y - GAP * (n - 1.0)) / n * aspect).max(16.0);
    let tile = egui::vec2(tile_w, tile_w / aspect);
    let snap = |v: f32| (((v * ppp) as u32).div_ceil(SNAP_PX) * SNAP_PX).max(SNAP_PX);
    let px = (snap(tile.x), snap(tile.y));
    let origin = ui.cursor().min;

    let config = app.dice_config_flag();
    let grid = &app.mutation_grid;
    let thumbs = &mut app.thumbs;
    let ui_state = &mut app.ui_state;
    let visuals = ui.visuals().clone();
    // Shown as the grid it is, not as one that's out of date: a size change
    // keeps the old ring until the new one is dealt.
    let shown = grid.seed().map_or(wanted, |(_, s)| s);
    if shown.side() != side {
        ui.allocate_space(avail);
        return (None, egui::Vec2::ZERO, 0.0);
    }

    let mut act = None;
    let mut orbit = egui::Vec2::ZERO;
    let mut zoom = 0.0;
    for cell in cells(shown) {
        let min = origin + egui::vec2(cell.col as f32 * (tile.x + GAP), cell.row as f32 * (tile.y + GAP));
        let rect = egui::Rect::from_min_size(min, tile);
        let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect.expand(2.0));

        let (scene, key, caption, tip): (Option<&Scene>, Option<ThumbKey>, Option<String>, String) = if cell.tile == 0 {
            let parent = grid.parent();
            (
                parent.map(|(s, _)| s),
                parent.map(|(_, id)| ThumbKey::Grid(id)),
                None,
                "The scene as it is. Click for a new ring of mutations round it.".to_string(),
            )
        } else {
            match grid.tile(cell.tile).map(|t| (t, &t.state)) {
                Some((t, TileState::Ready(c))) => (
                    Some(&c.scene),
                    Some(ThumbKey::Grid(c.id)),
                    Some(c.log.join("; ")),
                    format!(
                        "{}{}\n\nTile {} (seed {}{}). Click to make it the scene — one Ctrl+Z \
                         brings back this grid.{}",
                        c.log.join("\n"),
                        c.note.as_ref().map_or(String::new(), |n| format!("\n({n})")),
                        cell.tile,
                        t.seed,
                        if cell.ring == 2 { ", twice the strength" } else { "" },
                        if config.is_empty() { String::new() } else { format!("\nProfile:{config}") },
                    ),
                ),
                Some((_, TileState::Empty(why))) => {
                    (None, None, None, format!("Nothing passed for this tile: {why}."))
                }
                _ => (None, None, None, "Mutating…".to_string()),
            }
        };

        match (scene, &key) {
            (Some(scene), Some(key)) => match thumbs.get(key, None, px, scene) {
                ThumbState::Ready(id) => {
                    thumbs.follow(key.clone());
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    painter.image(id, rect, uv, egui::Color32::WHITE);
                }
                ThumbState::Pending => {
                    let [r, g, b] = crate::palette::to_srgb8(scene.background);
                    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(r, g, b));
                }
                ThumbState::Failed(_) => {
                    painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
                    centred(&painter, rect, "no preview", &visuals);
                }
            },
            _ => {
                painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
                let text = if tip.starts_with("Nothing") { "none passed" } else { "…" };
                centred(&painter, rect, text, &visuals);
            }
        }

        // The centre wears the selection colour: it's the scene. A hovered
        // tile gets a plain rim and its operators along its foot.
        if cell.tile == 0 {
            painter.rect_stroke(rect, 0.0, visuals.selection.stroke, egui::StrokeKind::Outside);
        } else if resp.hovered() || resp.dragged() {
            painter.rect_stroke(rect, 0.0, visuals.widgets.hovered.fg_stroke, egui::StrokeKind::Outside);
            if let Some(caption) = caption {
                foot(&painter, rect, caption, ui);
            }
        }

        let resp = hinted(
            resp,
            ui_state,
            tip,
            if cell.tile == 0 {
                "click: new mutations · drag: turn the view · wheel: zoom"
            } else {
                "click: make this the scene · drag: turn the view · wheel: zoom"
            },
        );
        if resp.dragged() {
            orbit += resp.drag_delta() * ppp;
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if resp.hovered() {
            zoom += wheel_steps(ui);
        }
        if resp.clicked() {
            act = match cell.tile {
                0 => Some(Act::Reroll),
                n if matches!(grid.tile(n).map(|t| &t.state), Some(TileState::Ready(_))) => Some(Act::Adopt(n)),
                _ => None,
            };
        }
    }
    (act, orbit, zoom)
}

/// The wheel this frame, in the notches `App::on_scroll` takes — read from
/// the raw events, because egui's own scroll delta is smoothed over frames.
fn wheel_steps(ui: &egui::Ui) -> f32 {
    ui.input(|i| {
        i.events
            .iter()
            .map(|e| match e {
                egui::Event::MouseWheel { unit, delta, .. } => match unit {
                    egui::MouseWheelUnit::Line => delta.y,
                    egui::MouseWheelUnit::Page => delta.y * 10.0,
                    // As main.rs converts a touchpad's pixels.
                    egui::MouseWheelUnit::Point => delta.y * i.pixels_per_point / 60.0,
                },
                _ => 0.0,
            })
            .sum()
    })
}

fn centred(painter: &egui::Painter, rect: egui::Rect, text: &str, visuals: &egui::Visuals) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(11.0),
        visuals.weak_text_color(),
    );
}

/// The operators, on a dark strip along the tile's foot, cut off to fit.
fn foot(painter: &egui::Painter, rect: egui::Rect, text: String, ui: &egui::Ui) {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let strip = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - font.size - 6.0), rect.max);
    painter.rect_filled(strip, 0.0, egui::Color32::from_black_alpha(170));
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, egui::Color32::WHITE);
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.width() - 8.0).max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.galley(strip.min + egui::vec2(4.0, 3.0), painter.layout_job(job), egui::Color32::WHITE);
}
