//! Top toolbar: a thin strip of icon toggle buttons for the four Phase 2+
//! window skeletons, plus Shortcuts/Browser buttons that drive the legacy
//! overlays (same code paths as the H/B keys) until Phase 6 gives them real
//! windows of their own.

use crate::app::App;

use super::hints::hinted;
use super::icons;

pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    egui::Panel::top("fracturize_toolbar").show(ui, |ui| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);

            let resp = ui.toggle_value(&mut app.ui_state.panels.transforms_open, icons::LIST);
            hinted(
                resp,
                &mut app.ui_state,
                "Transforms — list + inspector",
                "toggle the Transforms window",
            );

            let resp = ui.toggle_value(&mut app.ui_state.panels.explore_open, icons::FLASK);
            hinted(
                resp,
                &mut app.ui_state,
                "Explore — mutate, undo, strength",
                "toggle the Explore window",
            );

            let resp = ui.toggle_value(&mut app.ui_state.panels.camera_open, icons::VIDEO_CAMERA);
            hinted(
                resp,
                &mut app.ui_state,
                "Camera — framing, saved views, paths, render output",
                "toggle the Camera window",
            );

            let resp = ui.toggle_value(&mut app.ui_state.panels.render_open, icons::SLIDERS);
            hinted(
                resp,
                &mut app.ui_state,
                "Render — renderer, point size + count, color, fog",
                "toggle the Render window",
            );

            ui.separator();

            let resp = ui.selectable_label(app.show_help, icons::KEYBOARD);
            let resp = hinted(resp, &mut app.ui_state, "Keybind reference (H)", "toggle the keybind help panel");
            if resp.clicked() {
                app.toggle_help();
            }

            let resp = ui.selectable_label(app.show_browser, icons::FOLDER_OPEN);
            let resp = hinted(resp, &mut app.ui_state, "Scene browser (B)", "toggle the scene browser");
            if resp.clicked() {
                app.toggle_browser();
            }

            // Scene identity, right-aligned — this used to be the HUD's
            // first line, which sat underneath this very panel.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                let resp = ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("{} — {}", app.scene.name, app.scene.author)).weak(),
                    )
                    .truncate()
                    .selectable(false),
                );
                let path = app.scene_path.clone().unwrap_or_else(|| "(unsaved scene)".to_string());
                hinted(resp, &mut app.ui_state, &path, "the scene currently loaded");
            });
        });
        ui.add_space(2.0);
    });
}
