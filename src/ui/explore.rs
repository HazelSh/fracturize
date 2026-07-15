//! Explore window: the first real panel with wired controls, proving the
//! pattern later phases (browser, random flames, history list) build on.
//! Phase 2 scope: mutate strength slider + Mutate/Undo buttons (the U /
//! Shift+U keybinds keep working unchanged — both paths share
//! `App::mutate_scene` / `App::undo_mutation`).

use crate::app::App;

use super::hints::hinted;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.explore_open;
    if !open {
        return;
    }

    egui::Window::new("Explore")
        .id(egui::Id::new("fracturize_explore_window"))
        .open(&mut open)
        .default_pos(egui::pos2(20.0, 60.0))
        .default_width(220.0)
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
                "How strongly U perturbs the scene (persisted)",
                "drag: change mutation strength",
            );

            ui.horizontal(|ui| {
                let resp = ui.button("Mutate");
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Apply a random mutation (U)",
                    "click: mutate the scene",
                );
                if resp.clicked() {
                    app.mutate_scene();
                }

                let resp = ui.button("Undo");
                let resp = hinted(
                    resp,
                    &mut app.ui_state,
                    "Revert the last mutation (Shift+U)",
                    "click: undo the last mutation",
                );
                if resp.clicked() {
                    app.undo_mutation();
                }
            });

            ui.label(
                egui::RichText::new("History list + random flames arrive in later phases.")
                    .small()
                    .weak(),
            );
        });

    app.ui_state.panels.explore_open = open;
}
