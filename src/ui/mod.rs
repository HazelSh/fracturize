//! egui UI layer: a top icon toolbar, floating panel windows (Transforms,
//! Explore, Render, Camera, Scenes, Keybinds), a bottom status bar with
//! hover hints and an FPS/p99 tracker, and world-anchored gizmo labels —
//! over vendored Phosphor icons and a best-effort Envy Code R.
//!
//! The viewport itself stays the full surface; nothing here shrinks it. None
//! of this runs on the offline (`--render`) path, which never constructs an
//! `EguiLayer`.
//!
//! The `TextEntry` "legacy text shim" that carried the old hand-rolled
//! glyphon overlays through Phases 1-5 is gone: every one of its callers
//! (HUD readouts, keybind panel, scene browser, gizmo labels) now has real
//! widgets, in the panels and in `labels.rs`.

use std::sync::Arc;

use winit::window::Window;

use crate::prefs::PanelPrefs;

pub mod browser;
pub mod camera_panel;
pub mod explore;
pub mod hints;
pub mod icons;
pub mod labels;
pub mod render_panel;
pub mod shortcuts;
pub mod status_bar;
pub mod toolbar;
pub mod transforms;

/// Plain-data UI state living on `App` (not on `AppWrapper`/`EguiLayer` —
/// those own the egui machinery itself).
pub struct UiState {
    /// Open/closed state of the four toolbar-toggled panel windows. Mirrors
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
    /// Variation slots the inspector keeps showing for the transform in
    /// `.0` even at weight 0 — dragging a weight to zero must not make its
    /// row vanish mid-gesture (and blocking the drag there would put
    /// Apophysis-style negative weights out of mouse reach). Rows leave the
    /// list only via their explicit remove button. Reset when the selected
    /// transform changes.
    pub variation_rows: (usize, Vec<usize>),
    /// Render window: the point count (in millions) currently typed into the
    /// DragValue but not yet applied. Held separately from the live capacity
    /// because applying it reallocates the point buffer and restarts warmup,
    /// so it happens on the Apply button rather than on every drag frame.
    pub pending_point_count: Option<f32>,
}

impl UiState {
    fn new(panels: PanelPrefs) -> Self {
        Self {
            panels,
            status_hint: None,
            trs_cache: None,
            renaming_transform: None,
            variation_rows: (usize::MAX, Vec::new()),
            pending_point_count: None,
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

/// Build this frame's egui UI: toolbar, panel windows, status bar, gizmo
/// labels — in that order. The status bar must draw after the toolbar and
/// windows so it can read any `hinted()` hover hint they set this frame.
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
    browser::draw(&ctx, app);
    shortcuts::draw(&ctx, app);
    status_bar::draw(ui, app);

    labels::draw(&ctx, app);

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
        // This is a tool UI, not a document: labels are captions on controls,
        // not prose to copy out. egui's cross-widget text selection is on by
        // default, and left enabled a drag starting on any panel label paints
        // a selection band across every row it crosses — worse, it competes
        // with the gestures that matter here (drag a transform row to select
        // it, drag a DragValue to change it). Set once, not per frame:
        // `all_styles_mut` clones the shared `Style` behind an `Arc`.
        ctx.all_styles_mut(|s| s.interaction.selectable_labels = false);
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
