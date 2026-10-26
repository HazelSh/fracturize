//! Mutate window: which dice profile, how strongly, how big a grid, and the
//! buttons — in the order the decision is made. The profile says *what* may
//! change (`colour-only` and `no-colour` keep half the flame), the strength
//! *how much*, and both are the profile's (see `crate::dice`): the slider is
//! the same number as Random settings → Mutation step → strength.
//!
//! The grid itself is the view, not this window: Ctrl+U, or the toggle here,
//! fills the viewport with the scene and its mutations around it, every pane
//! seen through the one camera (`ui::grid_view`). The Mutate button does what
//! U does — one mutation, unseen — and is the same edit as picking a tile.

use crate::app::App;
use crate::dice::STRENGTH_LIMITS;
use crate::mutation_grid::{self, MutationGrid, Tile};

use super::hints::hinted;
use super::icons;
use super::random_settings;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.mutate_open;
    if !open {
        return;
    }
    let mut picked = None;
    let mut reroll = false;

    super::window(ctx, app, super::WindowKey::Mutate, "Mutate")
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Profile");
                picked = random_settings::profile_picker(ui, app, 150.0);
            });
            strength(ui, app);
            ui.horizontal(|ui| {
                let mut on = app.grid_view();
                let resp = ui.toggle_value(&mut on, (icons::SHUFFLE, "Grid in the view"));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Fill the view with the scene and its mutations around it, all seen through \
                     one camera (Ctrl+U). Orbit to judge them from every side, click one to take \
                     it, click the middle for new ones.",
                    "toggle: the mutation grid in the view",
                );
                if resp.clicked() {
                    app.toggle_grid_view();
                }
                size_radio(ui, app);
            });
            ui.horizontal(|ui| {
                reroll = buttons(ui, app);
            });
            if app.grid_view() {
                status_line(ui, app);
            }
        });
    super::remember(ctx, app, super::WindowKey::Mutate);

    if let Some(entry) = picked {
        app.load_dice_profile(entry);
    }
    if reroll {
        app.reroll_mutation_grid();
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
        "New mutations of the same scene in the grid (or click the grid's middle)",
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
