//! Transforms window: list + Mat4↔TRS inspector. Placeholder in Phase 2 —
//! the full panel (per-transform rows, gizmo two-way selection, non-TRS
//! fallback, color picker, variation editor) lands in Phase 4. This stub
//! only needs to open/close and persist that state; the legacy transform
//! list HUD (right-hand `T` overlay) keeps working until then.

use crate::app::App;

pub fn draw(ctx: &egui::Context, app: &mut App) {
    let mut open = app.ui_state.panels.transforms_open;
    if !open {
        return;
    }

    egui::Window::new("Transforms")
        .id(egui::Id::new("fracturize_transforms_window"))
        .open(&mut open)
        .default_pos(egui::pos2(260.0, 60.0))
        .default_width(220.0)
        .show(ctx, |ui| {
            ui.label(format!("{} transforms in the current scene.", app.scene.transforms.len()));
            ui.label(
                egui::RichText::new("Transform list + inspector coming in Phase 4.")
                    .small()
                    .weak(),
            );
        });

    app.ui_state.panels.transforms_open = open;
}
