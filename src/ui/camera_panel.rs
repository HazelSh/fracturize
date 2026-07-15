//! Camera window: orbit/path/view controls. Placeholder in Phase 2 — the
//! full panel (orbit pause, invert-pitch, path-key list, saved views, HQ
//! render) lands in Phase 5. This stub only needs to open/close and persist
//! that state; the O/Z/Y/V/P keybinds keep working unchanged.

use crate::app::App;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.camera_open;
    if !open {
        return;
    }

    egui::Window::new("Camera")
        .id(egui::Id::new("fracturize_camera_window"))
        .open(&mut open)
        .default_pos(egui::pos2(260.0, 220.0))
        .default_width(220.0)
        .show(ctx, |ui| {
            ui.label(format!(
                "orbit {} · d={:.2} yaw={:.2} pitch={:.2}",
                if app.orbit_paused { "paused" } else { "running" },
                app.camera.distance,
                app.camera.yaw,
                app.camera.pitch,
            ));
            ui.label(
                egui::RichText::new("Orbit/views/paths/HQ render coming in Phase 5.")
                    .small()
                    .weak(),
            );
        });

    app.ui_state.panels.camera_open = open;
}
