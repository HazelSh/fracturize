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
pub mod confirm;
pub mod explore;
pub mod gradient;
pub mod hints;
pub mod icons;
pub mod labels;
pub mod num;
pub mod radio;
pub mod render_job;
pub mod render_panel;
pub mod save_as;
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
    /// A transform's context menu, opened by right-clicking its *gizmo* in the
    /// viewport: the transform index, and where to draw the menu (filled in on
    /// the first frame from egui's pointer position, then held so the menu
    /// doesn't chase the cursor). The menu itself is the same one the Transforms
    /// window's rows use — see `transforms::context_menu`.
    pub transform_menu: Option<(usize, Option<egui::Pos2>)>,
    /// Variation slots the inspector keeps showing for the transform in
    /// `.0` even at weight 0 — dragging a weight to zero must not make its
    /// row vanish mid-gesture (and blocking the drag there would put
    /// Apophysis-style negative weights out of mouse reach). Rows leave the
    /// list only via their explicit remove button. Reset when the selected
    /// transform changes.
    pub variation_rows: (usize, Vec<usize>),
    /// Which browser row the list has already scrolled into view. Scrolling
    /// to the selection on *every* frame makes the list impossible to scroll
    /// by hand — it snaps back the instant the selection leaves the viewport
    /// — so it only happens on the frame the selection actually moves.
    pub browser_scrolled_to: Option<usize>,
    /// "Save scene as…" dialog: open state and in-progress filename.
    pub save_as: save_as::SaveAsState,
    /// Render-job dialog: the in-progress form. Kept whole (rather than
    /// rebuilt from `JobParams`) so switching output modes doesn't discard
    /// the settings of the mode you switched away from.
    pub render_job: render_job::RenderJobForm,
    /// Which palette control point the gradient editor has selected, if any.
    /// Held across frames so the colour picker and position field have
    /// something to act on, and updated by `App::set_palette_stop_at` when a
    /// drag reorders the stops out from under the index.
    pub palette_stop: Option<usize>,
    /// Click-wait-click state for the unsaved-changes dialog's Discard button
    /// (see `confirm::Arm`). Lives here rather than in the dialog because the
    /// dialog is redrawn from scratch every frame and the arm has to outlive
    /// that; reset whenever the dialog isn't up.
    pub discard_arm: confirm::Arm,
    /// The control point currently being dragged, followed through reorders.
    ///
    /// Stops are kept sorted, so dragging one past its neighbour swaps their
    /// indices — but egui keys a drag by *widget id*, which is built from the
    /// index. Without this the drag silently transfers to whichever stop
    /// inherited the index and starts hauling that one along too, and two
    /// control points end up stacked on the cursor. So the id says only "a
    /// drag is in progress"; this says what it is dragging.
    pub palette_drag: Option<usize>,
}

impl UiState {
    fn new(panels: PanelPrefs) -> Self {
        Self {
            panels,
            status_hint: None,
            trs_cache: None,
            renaming_transform: None,
            transform_menu: None,
            variation_rows: (usize::MAX, Vec::new()),
            browser_scrolled_to: None,
            save_as: save_as::SaveAsState::default(),
            render_job: render_job::RenderJobForm::default(),
            palette_stop: None,
            palette_drag: None,
            discard_arm: confirm::Arm::default(),
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

/// Stable identifier for a panel window: the egui `Id` salt, the prefs key
/// its geometry is stored under, and what `default_layout` keys off. Adding a
/// panel means adding a variant here and a case to `default_layout`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WindowKey {
    Render,
    Explore,
    Scenes,
    Keybinds,
    Transforms,
    Camera,
}

impl WindowKey {
    pub fn name(self) -> &'static str {
        match self {
            Self::Render => "render",
            Self::Explore => "explore",
            Self::Scenes => "scenes",
            Self::Keybinds => "keybinds",
            Self::Transforms => "transforms",
            Self::Camera => "camera",
        }
    }

    fn id(self) -> egui::Id {
        egui::Id::new(("fracturize_window", self.name()))
    }

    /// The smallest this window may be dragged to.
    ///
    /// Not cosmetic. A window whose fixed furniture doesn't fit hands its
    /// flexible middle a *negative* height, and the thing in that middle draws
    /// anyway — on top of the rows pinned below it. Worse, it draws on top
    /// *interactively*: egui gives the pointer to whichever widget was
    /// registered later, and a `Panel::bottom`'s contents are registered
    /// before the body that follows them. So the buried controls still paint,
    /// still highlight on hover and still show their tooltips, but their
    /// clicks go to the thing lying over them — a control that looks alive and
    /// isn't, which is a genuinely nasty thing to debug.
    ///
    /// The Camera window is the one that has to say so: it stacks a framing
    /// block, saved views, a scrolling keypoint list, the transport and loop
    /// rows, and the output buttons, and its own furniture wants ~290px before
    /// the list gets a single pixel. The width is the widest fixed row (the
    /// four output buttons), which merely clips rather than colliding — so it
    /// is a legibility floor, where the height is a correctness one.
    fn min_size(self) -> egui::Vec2 {
        match self {
            Self::Camera => egui::vec2(390.0, 310.0),
            _ => egui::vec2(180.0, 100.0),
        }
    }
}

/// Default position and size for a panel, as a function of the viewport so
/// the right-hand column tracks the window edge instead of sitting at a fixed
/// x that only works at one size.
///
/// The layout is Hazel's: Render top-left with Explore beneath it, then Scenes
/// and Keybinds filling the middle, and Transforms top-right with Camera
/// beneath it against the right edge. Persisted geometry always wins over
/// this — it's only what you get before you've moved anything.
fn default_layout(key: WindowKey, screen: egui::Rect) -> (egui::Pos2, egui::Vec2) {
    let top = 60.0;
    // The Transforms window is the widest by some way: it holds a tab rail
    // *and* a detail pane side by side. The right column is sized to it so
    // Camera lines up underneath.
    let right_col_w = 450.0;
    let right_x = (screen.right() - right_col_w - 20.0).max(20.0);
    match key {
        WindowKey::Render => (egui::pos2(20.0, top), egui::vec2(280.0, 290.0)),
        WindowKey::Explore => (egui::pos2(20.0, top + 300.0), egui::vec2(280.0, 220.0)),
        WindowKey::Scenes => (egui::pos2(320.0, top), egui::vec2(280.0, 420.0)),
        WindowKey::Keybinds => (egui::pos2(620.0, top), egui::vec2(360.0, 420.0)),
        WindowKey::Transforms => (egui::pos2(right_x, top), egui::vec2(right_col_w, 430.0)),
        WindowKey::Camera => (
            egui::pos2(right_x, top + 460.0),
            egui::vec2(right_col_w, 320.0),
        ),
    }
}

/// Start a panel window at its persisted geometry, or the default layout.
pub fn window(ctx: &egui::Context, app: &crate::app::App, key: WindowKey, title: &str) -> egui::Window<'static> {
    let (default_pos, default_size) = default_layout(key, ctx.content_rect());
    let stored = app.window_geometry(key.name());
    let min = key.min_size();
    let (pos, size) = match stored {
        // Geometry saved before this window grew a minimum — or by a build
        // that had a smaller one — comes back clamped rather than reinstating
        // a size the content can't live in.
        Some([x, y, w, h]) => (egui::pos2(x, y), egui::vec2(w.max(min.x), h.max(min.y))),
        None => (default_pos, default_size),
    };
    egui::Window::new(title.to_owned())
        .id(key.id())
        .default_pos(pos)
        .default_size(size)
        .min_size(min)
        .resizable(true)
}

/// Read a panel's current rect back out of egui and remember it, so the
/// arrangement survives a restart. Called right after the window is shown.
pub fn remember(ctx: &egui::Context, app: &mut crate::app::App, key: WindowKey) {
    if let Some(rect) = ctx.memory(|m| m.area_rect(key.id())) {
        app.set_window_geometry(
            key.name(),
            [rect.min.x, rect.min.y, rect.width(), rect.height()],
        );
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

    // Env lookup once, not per frame.
    static PROFILE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let profile = *PROFILE.get_or_init(|| std::env::var_os("FRACTURIZE_UI_PROFILE").is_some());
    let mut timings: Vec<(&str, f32)> = Vec::new();
    let step = |name: &'static str, t: std::time::Instant, timings: &mut Vec<(&str, f32)>| {
        if profile {
            timings.push((name, t.elapsed().as_secs_f32() * 1000.0));
        }
    };

    let t = std::time::Instant::now();
    toolbar::draw(ui, app);
    step("toolbar", t, &mut timings);
    let t = std::time::Instant::now();
    explore::draw(&ctx, app);
    step("explore", t, &mut timings);
    let t = std::time::Instant::now();
    render_panel::draw(&ctx, app);
    step("render", t, &mut timings);
    let t = std::time::Instant::now();
    transforms::draw(&ctx, app);
    step("transforms", t, &mut timings);
    let t = std::time::Instant::now();
    camera_panel::draw(&ctx, app);
    step("camera", t, &mut timings);
    let t = std::time::Instant::now();
    browser::draw(&ctx, app);
    step("browser", t, &mut timings);
    let t = std::time::Instant::now();
    shortcuts::draw(&ctx, app);
    step("shortcuts", t, &mut timings);
    let t = std::time::Instant::now();
    save_as::draw(&ctx, app);
    step("save_as", t, &mut timings);
    let t = std::time::Instant::now();
    render_job::draw(&ctx, app);
    step("render_job", t, &mut timings);
    let t = std::time::Instant::now();
    // Last of the dialogs, and modal: it draws over everything else, which is
    // the point — it stands between the person and work they can't get back.
    confirm::draw(&ctx, app);
    step("confirm", t, &mut timings);
    let t = std::time::Instant::now();
    status_bar::draw(ui, app);
    step("status_bar", t, &mut timings);
    let t = std::time::Instant::now();
    labels::draw(&ctx, app);
    step("labels", t, &mut timings);
    let t = std::time::Instant::now();
    draw_transform_menu(&ctx, app);
    step("transform_menu", t, &mut timings);

    if profile && app.frame_count % 120 == 0 {
        let parts: Vec<String> = timings
            .iter()
            .filter(|(_, ms)| *ms > 0.02)
            .map(|(n, ms)| format!("{} {:.2}", n, ms))
            .collect();
        log::info!("ui panels (ms): {}", parts.join(", "));
    }

    // Persist panel open/closed state the instant it changes (toolbar
    // toggle or a window's close button) — same pattern as invert_pitch.
    app.panel_prefs_changed(app.ui_state.panels);
}

/// The context menu for a transform right-clicked in the *viewport*, on its
/// gizmo (`App::on_mouse_press` opens it; `transforms::context_menu` is the
/// body, shared with the Transforms window's rows).
///
/// Hand-rolled as an `Area` rather than egui's `Response::context_menu`,
/// because there is no egui widget under the pointer to hang it off — the
/// thing that was clicked is a tetrahedron in a 3D scene, picked by
/// `src/pick.rs`. Everything else about it behaves like a menu: it appears
/// where you clicked, and it goes away on Escape or a click outside.
fn draw_transform_menu(ctx: &egui::Context, app: &mut crate::app::App) {
    let Some((idx, stored_pos)) = app.ui_state.transform_menu else {
        return;
    };
    // Pinned on the first frame: a menu that tracked the cursor would run away
    // from the pointer coming to click it.
    let pos = match stored_pos {
        Some(p) => p,
        None => {
            let p = ctx.pointer_latest_pos().unwrap_or_else(|| ctx.content_rect().center());
            app.ui_state.transform_menu = Some((idx, Some(p)));
            p
        }
    };

    let mut close = false;
    let area = egui::Area::new(egui::Id::new("fracturize_transform_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_max_width(150.0);
                let name = app
                    .scene
                    .transform_names
                    .get(idx)
                    .and_then(|n| n.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("T{}", idx));
                ui.label(egui::RichText::new(name).strong().small());
                ui.separator();
                close |= transforms::context_menu(ui, app, idx);
            });
        });

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    // Any click that isn't on the menu dismisses it, the way a real menu does.
    // Not on the frame it opened: the right-click that *asked* for the menu is
    // still in this frame's input, and the menu isn't laid out yet to say the
    // pointer is over it.
    let just_opened = stored_pos.is_none();
    if !just_opened
        && ctx.input(|i| i.pointer.any_pressed())
        && !area.response.contains_pointer()
    {
        close = true;
    }
    if close {
        app.ui_state.transform_menu = None;
    }
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
