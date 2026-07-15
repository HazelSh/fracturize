//! egui scaffold (Phase 1 of the GUI upgrade).
//!
//! Owns nothing about *what* the interactive panels look like yet — that's
//! later phases. Phase 1 just gets an `EguiLayer` wired into the winit event
//! loop and the wgpu render pass, and keeps every existing overlay (HUD,
//! help panel, browser, gizmo labels) alive via a temporary "legacy text
//! shim" now that the old text renderer is deleted.

use std::sync::Arc;

use winit::window::Window;

/// A single line of legacy overlay text (HUD, help panel, browser rows,
/// world-space gizmo labels). Positions/sizes are in *physical* pixels, as
/// they always were; `draw_legacy_text` converts to egui's logical-point
/// space by dividing by `pixels_per_point`. Existing app-side click
/// hit-tests (help rows, browser rows) operate on these same physical
/// coordinates and are unaffected.
///
/// Deleted along with the shim in Phase 6, once real egui panels/windows
/// replace every one of its callers.
pub struct TextEntry {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub color: [u8; 4],
    pub font_size: f32,
}

/// Plain-data UI state living on `App` (not on `AppWrapper`/`EguiLayer` —
/// those own the egui machinery itself). Grows in later phases (open-panel
/// flags, drag caches, TRS field cache, ...); Phase 1 only needs the shim's
/// stashed output.
#[derive(Default)]
pub struct UiState {
    /// This frame's legacy `TextEntry` list, rebuilt in `App::update()` from
    /// `build_text_entries` and painted every frame by `draw_legacy_text`.
    pub legacy_entries: Vec<TextEntry>,
}

/// Paint every legacy `TextEntry` via an egui foreground-layer painter, so
/// the HUD/help/browser/gizmo-label overlays keep rendering with no
/// dedicated text renderer of their own. `pixels_per_point` bridges
/// physical (entry) to logical (egui) space; this and the gizmo world-space
/// labels are the only such bridges (see AGENTS.md / plan doc).
pub fn draw_legacy_text(ctx: &egui::Context, entries: &[TextEntry], pixels_per_point: f32) {
    if entries.is_empty() {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("fracturize_legacy_text_shim"),
    ));
    for entry in entries {
        let pos = egui::pos2(entry.x / pixels_per_point, entry.y / pixels_per_point);
        let font = egui::FontId::monospace(entry.font_size / pixels_per_point);
        let color = egui::Color32::from_rgba_unmultiplied(
            entry.color[0],
            entry.color[1],
            entry.color[2],
            entry.color[3],
        );
        painter.text(pos, egui::Align2::LEFT_TOP, &entry.text, font, color);
    }
}

/// Build this frame's egui UI: the legacy-text shim, plus one trivial
/// floating window proving egui is actually wired up and interactive.
/// Later phases replace/extend this with the toolbar/panels/status bar.
pub fn draw(ctx: &egui::Context, app: &mut crate::app::App) {
    draw_legacy_text(ctx, &app.ui_state.legacy_entries, ctx.pixels_per_point());

    // Default position picked to clear the legacy HUD's top-left text block
    // so this is visibly a real, independently-draggable egui window rather
    // than something fighting the shim for the same corner.
    egui::Window::new("egui (Phase 1 scaffold)")
        .default_pos(egui::pos2(420.0, 20.0))
        .show(ctx, |ui| {
            ui.label(format!("FPS: {:.1}", app.current_fps()));
        });
}

/// The egui context/state/renderer bundle. Lives on `AppWrapper` (main.rs),
/// *not* on `App`: `ui::draw` needs `&mut App` while the frame is being
/// built, so `AppWrapper` must be able to split-borrow `self.app` and
/// `self.egui` at the same time — impossible if `App` owned this itself.
pub struct EguiLayer {
    pub ctx: egui::Context,
    pub state: egui_winit::State,
    pub renderer: egui_wgpu::Renderer,
}

impl EguiLayer {
    pub fn new(window: &Arc<Window>, device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let ctx = egui::Context::default();
        let viewport_id = egui::ViewportId::ROOT;
        let state = egui_winit::State::new(
            ctx.clone(),
            viewport_id,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        let renderer = egui_wgpu::Renderer::new(
            device,
            surface_format,
            egui_wgpu::RendererOptions::default(),
        );
        Self { ctx, state, renderer }
    }
}
