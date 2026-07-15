//! Render window: Phase 2 wires exposure + point size directly to their
//! existing `App` fields (same values the W/Shift+W and [/] keybinds adjust).
//! Splat mode, point count, fog, and color falloff/contrast land in Phase 5.

use crate::app::{App, RenderMode};

use super::hints::hinted;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.render_open;
    if !open {
        return;
    }

    egui::Window::new("Render")
        .id(egui::Id::new("fracturize_render_window"))
        .open(&mut open)
        .default_pos(egui::pos2(20.0, 220.0))
        .default_width(240.0)
        .show(ctx, |ui| {
            ui.label(format!(
                "Renderer: {}",
                match app.render_mode {
                    RenderMode::Points => "points",
                    RenderMode::Splat => "splat (log-density)",
                }
            ));

            let splat = app.render_mode == RenderMode::Splat;
            ui.add_enabled_ui(splat, |ui| {
                let resp = ui.add(
                    egui::Slider::new(&mut app.exposure, 0.01..=20.0)
                        .logarithmic(true)
                        .text("exposure"),
                );
                hinted(
                    resp,
                    &mut app.ui_state,
                    "Splat exposure — additive log-density brightness (W/Shift+W)",
                    "drag: adjust splat exposure",
                );
            });

            let resp = ui.add(
                egui::Slider::new(&mut app.point_size, 0.0001..=0.02)
                    .logarithmic(true)
                    .text("point size"),
            );
            hinted(
                resp,
                &mut app.ui_state,
                "World-space point size ([ / ])",
                "drag: adjust point size",
            );
        });

    app.ui_state.panels.render_open = open;
}
