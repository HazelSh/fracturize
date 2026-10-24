//! Mutate window: which dice profile, how strongly, and the button that does
//! one — in the order the decision is made. The profile says *what* may
//! change (`colour-only` and `no-colour` keep half the flame), the strength
//! *how much*, and both are the profile's (see `crate::dice`): the slider is
//! the same number as Random settings → Each mutation → strength. The button runs the same
//! `App::mutate_scene` U does, so a mutation from here and one from the key
//! are the same edit and undo the same way.

use crate::app::App;
use crate::dice::STRENGTH_LIMITS;

use super::hints::hinted;
use super::icons;
use super::random_settings;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.mutate_open;
    if !open {
        return;
    }
    let mut picked = None;

    super::window(ctx, app, super::WindowKey::Mutate, "Mutate")
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Profile");
                picked = random_settings::profile_picker(ui, app, 150.0);
            });

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
                "How much each mutation changes the scene, from this button or U. Part of the \
                 dice profile. It halves on its own when a scene keeps rejecting strong changes.",
                "drag: mutation strength",
            );

            ui.horizontal(|ui| {
                let resp = ui.button((icons::SHUFFLE, "Mutate"));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Change the scene a little, at random (U). Ctrl+Z takes it back.",
                    "click: mutate the scene",
                );
                if resp.clicked() {
                    app.mutate_scene();
                }

                let resp = ui.add_enabled(app.history.undo_len() > 0, egui::Button::new((icons::UNDO, "Undo")));
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Undo the last edit, mutation or not (Ctrl+Z, Shift+U)",
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

                let resp = ui.toggle_value(&mut app.ui_state.panels.mutation_grid_open, (icons::GRID, "Grid…"));
                hinted(
                    resp,
                    &mut app.ui_state,
                    "See a ring of mutations round the scene and pick one, rather than rolling one \
                     at a time",
                    "toggle the Mutation grid window",
                );
            });
        });

    super::remember(ctx, app, super::WindowKey::Mutate);
    if let Some(entry) = picked {
        app.load_dice_profile(entry);
    }
    app.ui_state.panels.mutate_open = open;
}
