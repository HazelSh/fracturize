//! Mutate window: how strongly a mutation perturbs the scene, and the button
//! that does one. The same `App::mutate_scene` U runs, so a mutation from here
//! and one from the key are the same edit and undo the same way.

use crate::app::App;

use super::hints::hinted;
use super::icons;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.mutate_open;
    if !open {
        return;
    }

    super::window(ctx, app, super::WindowKey::Mutate, "Mutate")
        .open(&mut open)
        .show(ctx, |ui| {
            let mut strength = app.mutate_strength();
            let resp = ui.add(
                egui::Slider::new(&mut strength, 0.1..=3.0)
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
                "How much each mutation changes the scene, from this button or U",
                "drag: change mutation strength",
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
            });
        });

    super::remember(ctx, app, super::WindowKey::Mutate);
    app.ui_state.panels.mutate_open = open;
}
