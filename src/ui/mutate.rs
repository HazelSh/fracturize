//! Mutate window: which dice profile, how strongly, and the grid — the scene
//! in the middle and its mutations around it, in the order the decision is
//! made. The profile says *what* may change (`colour-only` and `no-colour`
//! keep half the flame), the strength *how much*, and both are the profile's
//! (see `crate::dice`): the slider is the same number as Random settings →
//! Mutation step → strength.
//!
//! The grid is look-and-choose: click a tile and it becomes the scene, as one
//! edit, and a new ring grows around it. Ctrl+Z brings back the old scene and
//! the same tiles around it, so a sibling can be picked instead. The model is
//! `crate::mutation_grid`; the thumbnails are `gpu::thumbs`, as in the Random
//! flames list, and behave the same way: only the tile under the pointer
//! flies its camera. The Mutate button beside the grid does what U does — one
//! mutation, unseen — and both are the same edit as picking a tile.

use crate::app::App;
use crate::dice::STRENGTH_LIMITS;
use crate::gpu::thumbs::{ThumbKey, ThumbState};
use crate::mutation_grid::{self, MutationGrid, Tile};

use super::hints::hinted;
use super::icons;
use super::num;
use super::random_settings;

/// Space between tiles, in points.
const GAP: f32 = 4.0;

/// Thumbnail widths, in pixels, that a tile is drawn from: the smallest one at
/// least as wide as the tile. So resizing the window by a few pixels draws the
/// same thumbnails a little smaller instead of rendering all 25 again.
const LADDER: [u32; 6] = [128, 192, 256, 384, 512, 768];

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.mutate_open;
    if !open {
        return;
    }
    let mut picked = None;
    let mut adopt = None;
    let mut hovered = None;
    let mut reroll = false;
    // The grid's tiles are framed at the main view's shape, so a tile reads
    // the way the scene would if you picked it.
    let screen = ctx.content_rect();
    let aspect = (screen.width() / screen.height().max(1.0)).clamp(1.0, 2.4);

    app.poll_mutation_grid();
    super::window(ctx, app, super::WindowKey::Mutate, "Mutate")
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Panel::top("fracturize_mutate_bar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Profile");
                    picked = random_settings::profile_picker(ui, app, 150.0);
                });
                strength(ui, app);
                ui.horizontal(|ui| {
                    size_radio(ui, app);
                    ui.separator();
                    reroll = buttons(ui, app);
                });
                status_line(ui, app);
                ui.add_space(2.0);
            });
            let (a, h) = grid(ui, app, aspect);
            adopt = a;
            hovered = h;
        });
    super::remember(ctx, app, super::WindowKey::Mutate);

    if let Some(entry) = picked {
        app.load_dice_profile(entry);
    }
    if reroll {
        app.reroll_mutation_grid();
    }
    if let Some(id) = hovered {
        app.thumbs.hover(ThumbKey::Mutation(id));
    }
    if let Some(k) = adopt {
        app.adopt_mutation(k);
    }
    app.ui_state.panels.mutate_open = open;
}

fn strength(ui: &mut egui::Ui, app: &mut App) {
    let mut strength = app.mutate_strength();
    let resp = ui.add(
        egui::Slider::new(&mut strength, STRENGTH_LIMITS[0]..=STRENGTH_LIMITS[1])
            .text("strength")
            .clamping(egui::SliderClamping::Always),
    );
    if resp.changed() {
        app.set_mutate_strength(strength);
    }
    // Persist once the drag ends rather than on every changed-frame,
    // so a single drag writes prefs.toml once, not per pixel.
    if resp.drag_stopped() {
        app.save_prefs();
    }
    hinted(
        resp,
        &mut app.ui_state,
        "How much each mutation changes the scene: the tiles next to it, the Mutate \
         button and U. A 5×5's outer ring is made at twice this. Part of the dice \
         profile. It halves on its own when a scene keeps rejecting strong changes.",
        "drag: mutation strength",
    );
}

fn size_radio(ui: &mut egui::Ui, app: &mut App) {
    let size = app.mutation_grid_size();
    let chosen = super::radio::radio(&mut app.ui_state, "mutation_grid_size", size)
        .option(
            3,
            "3×3",
            "Eight mutations around the scene",
            "click: a grid of eight mutations",
        )
        .option(
            5,
            "5×5",
            "Twenty-four: the eight of the 3×3 next to the scene, and sixteen more \
             around them at twice the strength, so one grid shows near and far",
            "click: a grid of twenty-four mutations",
        )
        .show(ui);
    if let Some(n) = chosen {
        app.set_mutation_grid_size(n);
    }
}

/// New mutations, Mutate, Undo, Settings. Returns whether New was clicked.
fn buttons(ui: &mut egui::Ui, app: &mut App) -> bool {
    let resp = ui.button((icons::REFRESH, "New"));
    let reroll = hinted(
        resp,
        &mut app.ui_state,
        "Throw these mutations away and make new ones of the same scene",
        "click: new mutations around the scene",
    )
    .clicked();

    let resp = ui.button((icons::SHUFFLE, "Mutate"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Mutate the scene once without looking first (U). Ctrl+Z takes it back.",
        "click: mutate the scene",
    );
    if resp.clicked() {
        app.mutate_scene();
    }

    let resp = ui.add_enabled(app.history.undo_len() > 0, egui::Button::new((icons::UNDO, "Undo")));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Undo the last edit (Ctrl+Z). After picking a tile, this brings back the scene \
         before it and the same tiles around it.",
        "click: undo the last edit",
    );
    if resp.clicked() {
        app.undo();
    }

    let resp = ui.button((icons::SLIDERS, "Settings…"));
    let resp = hinted(
        resp,
        &mut app.ui_state,
        "Which parts of the flame may change, and how often: the dice profile, \
         in Random settings",
        "click: open the mutation settings",
    );
    if resp.clicked() {
        app.ui_state.panels.random_settings_open = true;
    }
    reroll
}

/// The command that renders this grid as a sheet, for the scene as saved.
fn command(app: &App, grid: &MutationGrid) -> String {
    let scene = app.scene_path.as_deref().unwrap_or("<scene.toml>");
    format!(
        "fracturize -s {scene} -r renders/grid.png --mutations {} --seed {}{}",
        mutation_grid::children(grid.size()),
        grid.base_seed,
        app.dice_config_flag(),
    )
}

fn status_line(ui: &mut egui::Ui, app: &mut App) {
    let grid = &app.mutation_grid;
    let n = grid.tiles.len();
    let ready = grid.tiles.iter().filter(|t| matches!(t, Tile::Ready(_))).count();
    let failed = grid.tiles.iter().filter(|t| matches!(t, Tile::Failed(_))).count();
    let text = if grid.parent().is_none() {
        "Mutating…".to_string()
    } else if grid.working() {
        format!("Mutating… {ready} of {n}")
    } else if failed > 0 {
        format!("{ready} of {n} · {failed} found nothing · seed {}", grid.base_seed)
    } else {
        format!("{n} mutations · seed {}", grid.base_seed)
    };
    let tip = format!(
        "Each tile is checked like a random roll, against the scene in the middle, so none \
         is worse than it. A tile that found nothing in the profile's tries is left empty.\n\n\
         This grid as a sheet, for the scene as saved:\n  {}",
        command(app, grid),
    );
    let resp = ui.add(egui::Label::new(egui::RichText::new(text).small().weak()).sense(egui::Sense::hover()));
    hinted(resp, &mut app.ui_state, tip, "the grid's seed; hover for the command that renders it");
}

/// The tiles. Returns the tile clicked and the thumbnail under the pointer.
fn grid(ui: &mut egui::Ui, app: &mut App, aspect: f32) -> (Option<u32>, Option<u64>) {
    let ppp = ui.ctx().pixels_per_point();
    let config = app.dice_config_flag();
    let (grid, thumbs, ui_state) = app.mutation_grid_parts();
    let n = grid.size();
    let Some(parent) = grid.parent() else { return (None, None) };
    if grid.tiles.len() as u32 != mutation_grid::children(n) {
        return (None, None);
    }

    // A few points short of the bottom, or the last row's edge is clipped.
    let avail = ui.available_size() - egui::vec2(0.0, 6.0);
    let span = GAP * (n - 1) as f32;
    let w = ((avail.x - span) / n as f32).min((avail.y - span) / n as f32 * aspect).max(24.0);
    let tile = egui::vec2(w, w / aspect);
    let px_w = LADDER.into_iter().find(|&p| p as f32 >= w * ppp).unwrap_or(LADDER[LADDER.len() - 1]);
    let px = (px_w, ((px_w as f32 / aspect / 2.0).round() as u32 * 2).max(2));

    let total = egui::vec2(tile.x * n as f32 + span, tile.y * n as f32 + span);
    let (area, _) = ui.allocate_exact_size(egui::vec2(avail.x.max(total.x), total.y), egui::Sense::hover());
    let origin = egui::pos2(area.center().x - total.x / 2.0, area.top());
    let visuals = ui.visuals().clone();

    let mut adopt = None;
    let mut hovered = None;
    for k in 0..=mutation_grid::children(n) {
        let (col, row) = mutation_grid::cell(n, k);
        let rect = egui::Rect::from_min_size(
            origin + egui::vec2(col as f32 * (tile.x + GAP), row as f32 * (tile.y + GAP)),
            tile,
        );
        let resp = ui.interact(rect, ui.id().with(("mutation_tile", k)), egui::Sense::click());
        let painter = ui.painter_at(rect.expand(2.0));

        let (key, scene, tip, hint) = if k == 0 {
            (
                Some(grid.parent_id),
                Some(parent),
                "The scene as it is now. Every tile around it is a mutation of it, framed \
                 the way you're looking at it."
                    .to_string(),
                "the scene as it is: the mutations around it are of this",
            )
        } else {
            match &grid.tiles[k as usize - 1] {
                Tile::Ready(c) => (
                    Some(c.id),
                    Some(&c.scene),
                    child_tip(k, n, c, grid.base_seed, &config),
                    "click: make this the scene (Ctrl+Z brings this grid back) · hover flies its camera",
                ),
                Tile::Pending => (None, None, "Being made…".to_string(), "a mutation on its way"),
                Tile::Failed(why) => (
                    None,
                    None,
                    format!("Nothing passed for this tile: {why}"),
                    "no mutation met the profile here",
                ),
            }
        };

        match key.zip(scene) {
            Some((id, scene)) => match thumbs.get(&ThumbKey::Mutation(id), None, px, scene) {
                ThumbState::Ready(tex) => {
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    painter.image(tex, rect, uv, egui::Color32::WHITE);
                }
                ThumbState::Pending => {
                    let [r, g, b] = crate::palette::to_srgb8(scene.background);
                    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(r, g, b));
                }
                ThumbState::Failed(_) => blank(&painter, rect, &visuals, "no preview"),
            },
            None => {
                let [r, g, b] = crate::palette::to_srgb8(parent.background);
                painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(r, g, b).gamma_multiply(0.5));
                let text = if matches!(grid.tiles.get(k as usize - 1), Some(Tile::Failed(_))) {
                    "nothing passed"
                } else {
                    "…"
                };
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(11.0),
                    visuals.weak_text_color(),
                );
            }
        }

        if k == 0 {
            painter.rect_stroke(rect, 0.0, egui::Stroke::new(2.0, visuals.selection.stroke.color), egui::StrokeKind::Outside);
        } else if resp.hovered() && grid.child(k).is_some() {
            painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.5, visuals.widgets.hovered.fg_stroke.color), egui::StrokeKind::Outside);
        }

        let resp = hinted(resp, ui_state, tip, hint);
        if resp.hovered() {
            hovered = key;
        }
        if k > 0 && resp.clicked() && grid.child(k).is_some() {
            adopt = Some(k);
        }
    }
    (adopt, hovered)
}

fn blank(painter: &egui::Painter, rect: egui::Rect, visuals: &egui::Visuals, text: &str) {
    painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(11.0),
        visuals.weak_text_color(),
    );
}

fn child_tip(k: u32, n: u32, c: &mutation_grid::Child, base: u64, config: &str) -> String {
    let changes = c.log.iter().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n");
    let strength = if c.strength < c.asked {
        format!("strength {} (backed off from {})", num::fixed(c.strength, 2), num::fixed(c.asked, 2))
    } else {
        format!("strength {}", num::fixed(c.strength, 2))
    };
    format!(
        "Tile {k}, seed {seed} · {strength} · {tries} tr{ies}\n{changes}\n\n\
         Click to make it the scene. Ctrl+Z brings back the scene before it, and this grid.\n\n\
         It is tile {k} of: --mutations {count} --seed {base}{config}",
        seed = c.seed,
        tries = c.attempts,
        ies = if c.attempts == 1 { "y" } else { "ies" },
        count = mutation_grid::children(n),
    )
}
