//! egui UI layer.
//!
//! Phase 1 got an `EguiLayer` wired into the winit event loop and the wgpu
//! render pass, keeping every existing overlay (HUD, help panel, browser,
//! gizmo labels) alive via a temporary "legacy text shim". Phase 2 adds the
//! real UI around it: a top icon toolbar, four `egui::Window` skeletons
//! (Explore and Render wired to live controls; Transforms/Camera are
//! placeholders until Phases 4/5), a bottom status bar with hover hints and
//! an FPS/p99 tracker, vendored Phosphor icons, and a best-effort Envy Code R
//! load. None of this runs on the offline (`--render`) path, which never
//! constructs an `EguiLayer`.

use std::sync::Arc;

use winit::window::Window;

use crate::prefs::PanelPrefs;

pub mod camera_panel;
pub mod explore;
pub mod hints;
pub mod icons;
pub mod render_panel;
pub mod status_bar;
pub mod toolbar;
pub mod transforms;

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
/// those own the egui machinery itself).
pub struct UiState {
    /// This frame's legacy `TextEntry` list, rebuilt in `App::update()` from
    /// `build_text_entries` and painted every frame by `draw_legacy_text`.
    pub legacy_entries: Vec<TextEntry>,
    /// Open/closed state of the four Phase 2 panel windows. Mirrors
    /// `Prefs::panels`; `App::panel_prefs_changed` writes back and persists
    /// the moment it differs (toolbar toggle and a window's own close
    /// button both mutate this same value, so they can't go out of sync).
    pub panels: PanelPrefs,
    /// This frame's status-bar left-hand hint, set by `hints::hinted` when a
    /// widget is hovered; cleared at the start of every frame and read (then
    /// taken) by `status_bar::draw`, which falls back to gizmo-hover /
    /// viewport-default hints when nothing set it.
    pub status_hint: Option<String>,
    /// Transforms window inspector: cached decomposed TRS fields (or raw
    /// matrix grid) for the selected transform, keyed by
    /// `(transform_index, matrix_generation)` — the jitter guard from the
    /// plan. `None` forces a fresh decompose next draw.
    pub trs_cache: Option<transforms::TrsCache>,
    /// Transforms window list: the row currently being renamed inline (via
    /// its right-click context menu), and its in-progress text buffer. Only
    /// one row can be mid-rename at a time.
    pub renaming_transform: Option<(usize, String)>,
}

impl UiState {
    fn new(panels: PanelPrefs) -> Self {
        Self {
            legacy_entries: Vec::new(),
            panels,
            status_hint: None,
            trs_cache: None,
            renaming_transform: None,
        }
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self::new(PanelPrefs::default())
    }
}

impl UiState {
    /// Seed panel open/closed state from persisted prefs (called once from
    /// `App::new`).
    pub fn from_prefs(prefs: &crate::prefs::Prefs) -> Self {
        Self::new(prefs.panels)
    }
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

/// Build this frame's egui UI: toolbar, panel windows, status bar, and the
/// legacy-text shim (HUD/help/browser/gizmo labels), in that order — the
/// status bar must draw after the toolbar/windows so it can read any
/// `hinted()` hover hint they set this frame.
///
/// `ui` is the top-level root `Ui` handed to `ctx.run_ui`'s closure: egui
/// 0.35 replaced `TopBottomPanel`/`SidePanel` with a unified `Panel` type
/// whose `show()` always takes a `&mut Ui` rather than a bare `&Context`
/// (see the module doc on `egui::containers::panel`), so the toolbar and
/// status bar need the root `Ui`. `egui::Window::show` is unchanged and
/// still just wants a `&Context`, obtained here via `ui.ctx()`.
pub fn draw(ui: &mut egui::Ui, app: &mut crate::app::App) {
    app.ui_state.status_hint = None;
    let ctx = ui.ctx().clone();

    toolbar::draw(ui, app);
    explore::draw(&ctx, app);
    render_panel::draw(&ctx, app);
    transforms::draw(&ctx, app);
    camera_panel::draw(&ctx, app);
    status_bar::draw(ui, app);

    draw_legacy_text(&ctx, &app.ui_state.legacy_entries, ctx.pixels_per_point());

    // Persist panel open/closed state the instant it changes (toolbar
    // toggle or a window's close button) — same pattern as invert_pitch.
    app.panel_prefs_changed(app.ui_state.panels);
}

/// Register fonts: the vendored Phosphor icon set (always) as a fallback on
/// both built-in families, plus a best-effort Envy Code R as the *primary*
/// monospace family when fontconfig resolves it. Never a hard dependency —
/// falls back to egui's built-in monospace when it can't be found (see
/// AGENTS.md / the GUI upgrade plan, risk #5). Only called from the
/// interactive path (`EguiLayer::new`); the offline `--render` path never
/// constructs an `EguiLayer` and so never runs this.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "Phosphor".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/Phosphor.ttf"
        ))),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("Phosphor".to_owned());
    }

    match load_envy_code_r() {
        Some(bytes) => {
            fonts.font_data.insert(
                "EnvyCodeR".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "EnvyCodeR".to_owned());
            log::info!("Envy Code R resolved via fontconfig; using it as the primary monospace font");
        }
        None => {
            log::info!("Envy Code R not found via fontconfig; using egui's built-in monospace font");
        }
    }

    ctx.set_fonts(fonts);
}

/// Ask fontconfig for the Envy Code R font file and read it. Returns `None`
/// on any failure (no `fc-match` binary, no match, unreadable file) — this
/// must never be a hard dependency.
fn load_envy_code_r() -> Option<Vec<u8>> {
    let output = std::process::Command::new("fc-match")
        .args(["--format=%{file}", "Envy Code R"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    // fc-match always returns *some* match (falling back to a default sans
    // font) rather than failing outright, so only trust a result that
    // actually looks like Envy Code R.
    if !path.to_lowercase().contains("envy") {
        log::info!("fc-match fell back to '{}' — Envy Code R isn't installed here", path);
        return None;
    }
    std::fs::read(path).ok()
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
        install_fonts(&ctx);
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
