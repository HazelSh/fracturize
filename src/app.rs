use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{Mat4, Vec3};
use winit::window::Window;

use crate::camera::{world_to_screen, OrbitCamera};
use crate::gpu::lines::LineVertex;
use crate::gpu::{CameraUniforms, GizmoRenderer, GpuContext, LineRenderer, PointCompute, PointRenderer, SplatRenderer, DEPTH_FORMAT};
use crate::scene::{Scene, TransformSpec};
use crate::ui::{TextEntry, UiState};
use crate::view::View;

/// Seconds since the epoch, for unique output filenames
fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// RGB (0-1) to HSV (h in degrees, s/v in 0-1)
fn rgb_to_hsv(c: Vec3) -> (f32, f32, f32) {
    let max = c.x.max(c.y).max(c.z);
    let min = c.x.min(c.y).min(c.z);
    let delta = max - min;
    let h = if delta < 1e-6 {
        0.0
    } else if max == c.x {
        60.0 * (((c.y - c.z) / delta).rem_euclid(6.0))
    } else if max == c.y {
        60.0 * ((c.z - c.x) / delta + 2.0)
    } else {
        60.0 * ((c.x - c.y) / delta + 4.0)
    };
    let s = if max < 1e-6 { 0.0 } else { delta / max };
    (h, s, max)
}

/// HSV (h in degrees, s/v in 0-1) to RGB (0-1)
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Vec3 {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Vec3::new(r, g, b) + Vec3::splat(v - c)
}

/// Clear color: dark blue-black
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.02,
    g: 0.02,
    b: 0.05,
    a: 1.0,
};

/// Screenshot dimensions
const SCREENSHOT_WIDTH: u32 = 1280;
const SCREENSHOT_HEIGHT: u32 = 720;

/// Create a depth texture
fn create_depth_texture(device: &wgpu::Device, width: u32, height: u32, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

/// FPS tracking
pub struct FpsTracker {
    last_log_time: Instant,
    frames_since_log: u32,
    last_frame_time: Instant,
    frame_times: Vec<Duration>,
    pub current_fps: f32,
    pub current_frametime_ms: f32,
    pub should_update_display: bool,
}

impl FpsTracker {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            last_log_time: now,
            frames_since_log: 0,
            last_frame_time: now,
            frame_times: Vec::with_capacity(120),
            current_fps: 0.0,
            current_frametime_ms: 0.0,
            should_update_display: false,
        }
    }

    fn frame(&mut self) -> bool {
        let now = Instant::now();
        let frame_time = now.duration_since(self.last_frame_time);
        self.last_frame_time = now;
        self.frames_since_log += 1;
        self.should_update_display = false;

        self.frame_times.push(frame_time);
        if self.frame_times.len() > 60 {
            self.frame_times.remove(0);
        }

        if !self.frame_times.is_empty() {
            let total: Duration = self.frame_times.iter().sum();
            self.current_frametime_ms = total.as_secs_f32() * 1000.0 / self.frame_times.len() as f32;
            self.current_fps = self.frame_times.len() as f32 / total.as_secs_f32();
        }

        let elapsed = now.duration_since(self.last_log_time);
        if elapsed >= Duration::from_secs(1) {
            self.should_update_display = true;
            self.last_log_time = now;
            self.frames_since_log = 0;
        }

        self.should_update_display
    }
}

/// How a grabbed gizmo responds to cursor movement. All modes work from the
/// matrix captured at grab time, so a drag is absolute rather than a chain of
/// incremental (drift-prone) updates.
#[derive(Clone, Copy, Debug)]
enum GizmoDragMode {
    /// Dragging the origin dot: translate in the plane through the grab point
    /// facing the camera
    TranslateView { normal: Vec3, grab_offset: Vec3 },
    /// Dragging an origin->axis edge: slide along that world-space axis line
    TranslateAxis { axis: Vec3, s0: f32 },
    /// Dragging an outer edge: rotate around the transform's local axis
    /// (world direction `axis`, through the transform origin)
    Rotate { axis: Vec3, center: (f32, f32), start_angle: f32 },
    /// Ctrl-drag anywhere on the gizmo: uniform scale, drag up = grow
    Scale { start_y: f32 },
}

/// Actions dispatchable by clicking a help-panel row
#[derive(Clone, Copy, Debug)]
enum HelpAction {
    ToggleHelp,
    Reset,
    Zoom,
    ToggleSelected,
    ToggleText,
    ToggleGizmos,
    ToggleOrbit,
    SaveView,
    Screenshot,
    SaveScene,
    AddTransform,
    DeleteTransform,
    Weight,
    Hue,
    Sat,
    Val,
    CycleVariation,
    VariationWeight,
    PointSize,
    ColorFalloff,
    ColorContrast,
    FogIntensity,
    FogNear,
    FogFar,
    Mutate,
    Browse,
    HqRender,
    Traces,
    InvertPitch,
    RenderMode,
    Exposure,
    PathPlay,
    PathKey,
}

/// Which point renderer draws the fractal. Points is the classic opaque
/// depth-tested renderer; Splat is additive log-density accumulation
/// (flame-style, no occlusion). R toggles at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    Points,
    Splat,
}

/// Outcome of `App::render`'s attempt to acquire and present a frame.
/// wgpu 29 replaced `Surface::get_current_texture`'s `Result<_, SurfaceError>`
/// with the `CurrentSurfaceTexture` enum (and dropped the `OutOfMemory`
/// variant — device loss is now reported via `Device::set_device_lost_callback`
/// rather than through frame acquisition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// Frame presented normally.
    Presented,
    /// Transient (timeout/occluded/validation) — skip, try again next frame.
    Skip,
    /// Surface needs reconfiguring (lost/outdated).
    Reconfigure,
}

/// What a mouse drag is currently doing
#[derive(Clone, Copy, Debug)]
enum Drag {
    None,
    /// Left-drag on empty space: orbit the camera
    Orbit,
    /// Middle-drag or shift+left-drag: pan the focus in the view plane
    Pan,
    /// Left-drag on a gizmo: edit that transform live
    Gizmo {
        transform: usize,
        mode: GizmoDragMode,
        start_matrix: Mat4,
    },
}

pub struct App {
    pub gpu: GpuContext,
    pub window: Arc<Window>,
    pub frame_count: u32,
    /// Wall-clock timestamp of the previous update (for time-based motion)
    last_update: Instant,
    pub orbit_paused: bool,
    pub camera: OrbitCamera,
    /// Camera-path flythrough preview: normalized path time when playing
    path_play_t: Option<f32>,
    pub point_size: f32,

    /// Persistent user preferences (I toggles pitch inversion)
    prefs: crate::prefs::Prefs,
    /// Duration of the last frame, for time-based motion and churn
    frame_dt: f32,

    // Mouse state
    cursor: (f32, f32),
    drag: Drag,
    /// Gizmo part under the cursor (when not dragging)
    hovered: Option<crate::pick::GizmoHit>,
    pub shift_held: bool,
    pub ctrl_held: bool,

    pub show_gizmos: bool,
    pub show_text: bool,
    pub show_help: bool,

    // Fog parameters
    pub fog_near: f32,
    pub fog_far: f32,
    pub fog_brightness: f32,
    pub fog_saturation: f32,

    // Color accumulation / rendering parameters
    /// Scale-aware color accumulation exponent (0 = classic fixed-rate EMA)
    pub color_falloff: f32,
    /// Render-time cyclic contrast stretch of the colormap index
    pub color_contrast: f32,
    fps_tracker: FpsTracker,

    // Simple point rendering pipeline
    point_compute: PointCompute,
    point_renderer: PointRenderer,
    splat_renderer: SplatRenderer,
    gizmo_renderer: GizmoRenderer,
    line_renderer: LineRenderer,

    /// Plain-data egui UI state (open panels, drag caches, ... — grows in
    /// later phases). Phase 1 only uses `legacy_entries`, the TextEntry shim
    /// stash rebuilt every `update()`.
    pub ui_state: UiState,

    /// Which renderer draws the fractal (R toggles)
    pub render_mode: RenderMode,
    /// Splat-renderer exposure multiplier (W / Shift+W)
    pub exposure: f32,

    /// Chaos-game trace overlay (X): show walker paths as line segments
    pub show_traces: bool,

    /// The scene, mutable: gizmo drags and editing keys change it in place,
    /// Ctrl+S writes it back to disk
    pub scene: Scene,
    /// Scene file path (save target; also recorded in saved view files)
    scene_path: Option<String>,
    /// Point buffer capacity for HUD
    buffer_capacity: u32,

    /// Selected transform index (Some when text overlay visible)
    selected_transform: Option<usize>,
    /// Per-transform enabled state
    transform_enabled: Vec<bool>,
    /// Variation slot targeted by the variation-editing keys
    selected_variation: usize,

    // Scene browser overlay (B)
    pub show_browser: bool,
    browser_files: Vec<std::path::PathBuf>,
    browser_selected: usize,

    /// A background high-quality render is running (P)
    hq_render_in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,

    /// Pre-mutation snapshots for Shift+U undo (newest last, bounded)
    undo_stack: Vec<Scene>,

    depth_texture: wgpu::Texture,

    screenshot_texture: wgpu::Texture,
    screenshot_depth: wgpu::Texture,
    screenshot_buffer: wgpu::Buffer,
    pub pending_screenshot: bool,
}

impl App {
    /// Create a new App
    pub async fn new(
        window: Arc<Window>,
        scene: Scene,
        fog_enabled: bool,
        vsync: bool,
        scene_path: Option<String>,
        view: Option<View>,
        splat: bool,
        exposure: Option<f32>,
    ) -> Self {
        let gpu = GpuContext::new(window.clone(), vsync).await;

        log::info!("Loaded scene: {} by {}", scene.name, scene.author);

        let point_size = scene.point_size;
        log::info!("Point size from scene: {}", point_size);

        let buffer_capacity = scene.point_count as u32;
        log::info!("Point buffer capacity: {}", buffer_capacity);

        // Create point compute pipeline
        let point_compute = PointCompute::new(
            &gpu.device,
            &scene.transforms,
            &scene.colormap,
            buffer_capacity,
        );

        // Create point renderer
        let point_renderer = PointRenderer::new(
            &gpu.device,
            gpu.format,
            &point_compute.point_buffer,
            &point_compute.colormap_buffer,
        );

        // Splat renderer shares the same point + colormap buffers
        let splat_renderer = SplatRenderer::new(
            &gpu.device,
            gpu.format,
            &point_compute.point_buffer,
            &point_compute.colormap_buffer,
        );

        // --splat / a splat-captured view starts in splat mode; an explicit
        // --exposure wins over the view's saved exposure
        let render_mode = if splat || view.as_ref().is_some_and(|v| v.is_splat()) {
            RenderMode::Splat
        } else {
            RenderMode::Points
        };
        let exposure = exposure
            .or(view.as_ref().and_then(|v| v.exposure))
            .unwrap_or(1.0);

        // Create gizmo renderer
        let gizmo_renderer = GizmoRenderer::new(
            &gpu.device,
            gpu.format,
            &scene.transforms,
        );

        // Create depth texture
        let (width, height) = gpu.size();
        let depth_texture = create_depth_texture(&gpu.device, width, height, "main_depth");

        // Screenshot resources
        let screenshot_texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot_texture"),
            size: wgpu::Extent3d {
                width: SCREENSHOT_WIDTH,
                height: SCREENSHOT_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let screenshot_depth = create_depth_texture(&gpu.device, SCREENSHOT_WIDTH, SCREENSHOT_HEIGHT, "screenshot_depth");

        let bytes_per_row = SCREENSHOT_WIDTH * 4;
        let padded_bytes_per_row = (bytes_per_row + 255) & !255;
        let screenshot_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot_buffer"),
            size: (padded_bytes_per_row * SCREENSHOT_HEIGHT) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Trace line renderer (empty until X is pressed)
        let line_renderer = LineRenderer::new(&gpu.device, gpu.format);

        let num_transforms = scene.transforms.len();

        // Fog settings
        let (fog_brightness, fog_saturation) = if fog_enabled {
            (0.4, 0.3)
        } else {
            (1.0, 1.0)
        };

        let mut app = Self {
            gpu,
            window,
            frame_count: 0,
            last_update: Instant::now(),
            orbit_paused: false,
            camera: OrbitCamera {
                yaw: scene.camera_yaw,
                pitch: scene.camera_pitch,
                distance: scene.camera_distance,
                focus: scene.camera_focus,
            },
            path_play_t: None,
            point_size,
            prefs: crate::prefs::Prefs::load(),
            frame_dt: 1.0 / 60.0,
            cursor: (0.0, 0.0),
            drag: Drag::None,
            hovered: None,
            shift_held: false,
            ctrl_held: false,
            show_gizmos: true,
            show_text: true,
            // Env override lets automated captures verify the help overlay
            show_help: std::env::var("FRACTURIZE_SHOW_HELP").is_ok(),
            fog_near: 3.0,
            fog_far: 4.5,
            fog_brightness,
            fog_saturation,
            color_falloff: scene.color_falloff,
            color_contrast: scene.color_contrast,
            fps_tracker: FpsTracker::new(),
            point_compute,
            point_renderer,
            splat_renderer,
            gizmo_renderer,
            line_renderer,
            ui_state: UiState::default(),
            render_mode,
            exposure,
            show_traces: false,
            scene,
            scene_path,
            buffer_capacity,
            selected_transform: Some(0),
            transform_enabled: vec![true; num_transforms],
            selected_variation: 0,
            show_browser: false,
            browser_files: Vec::new(),
            browser_selected: 0,
            undo_stack: Vec::new(),
            hq_render_in_flight: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            depth_texture,
            screenshot_texture,
            screenshot_depth,
            screenshot_buffer,
            pending_screenshot: false,
        };

        // Apply a saved view, if given. Pause the orbit so the loaded
        // framing holds exactly (press O to resume).
        if let Some(v) = view {
            // Fold any legacy eye offset into the on-sphere camera
            app.camera = OrbitCamera::from_legacy(
                Vec3::from(v.focus),
                Vec3::from(v.offset),
                v.distance,
                v.rotation,
                v.pitch,
            );
            app.point_size = v.point_size;
            app.fog_near = v.fog_near;
            app.fog_far = v.fog_far;
            app.fog_brightness = v.fog_brightness;
            app.fog_saturation = v.fog_saturation;
            if let Some(c) = v.color_contrast {
                app.color_contrast = c;
            }
            if let Some(f) = v.color_falloff {
                app.color_falloff = f;
                app.refresh_color_speeds();
            }
            app.orbit_paused = true;
            log::info!("Loaded view (orbit paused; press O to resume)");
        }

        app
    }

    pub fn toggle_orbit(&mut self) {
        self.orbit_paused = !self.orbit_paused;
        log::info!("Camera orbit: {}", if self.orbit_paused { "paused" } else { "running" });
    }

    /// Play / stop the camera-path flythrough preview (Z)
    pub fn toggle_path_play(&mut self) {
        if self.path_play_t.is_some() {
            self.path_play_t = None;
            log::info!("Camera path: stopped");
            return;
        }
        match &self.scene.camera_path {
            Some(p) if p.keys.len() >= 2 => {
                self.path_play_t = Some(0.0);
                self.orbit_paused = true;
                log::info!(
                    "Camera path: playing {} keys over {:.1}s",
                    p.keys.len(),
                    p.duration()
                );
            }
            _ => log::warn!("No camera path to play — press Y to add keypoints"),
        }
    }

    /// Append the current camera framing as a path keypoint (Y)
    pub fn add_path_key(&mut self) {
        let key = crate::path::PathKey::from_camera(&self.camera);
        let path = self.scene.camera_path.get_or_insert_with(|| crate::path::CameraPath {
            keys: Vec::new(),
            closed: false,
            ease: None,
            seconds: None,
        });
        path.keys.push(key);
        log::info!(
            "Camera path keypoint {} added (Z plays, Ctrl+S saves with the scene)",
            path.keys.len()
        );
    }

    /// Remove the last path keypoint (Shift+Y)
    pub fn remove_path_key(&mut self) {
        let Some(path) = &mut self.scene.camera_path else {
            log::warn!("No camera path");
            return;
        };
        path.keys.pop();
        let n = path.keys.len();
        if n == 0 {
            self.scene.camera_path = None;
            self.path_play_t = None;
            log::info!("Camera path removed");
        } else {
            log::info!("Camera path keypoint removed ({} left)", n);
        }
    }

    /// Toggle whether the path loops back to its first key (Ctrl+Y)
    pub fn toggle_path_closed(&mut self) {
        if let Some(path) = &mut self.scene.camera_path {
            path.closed = !path.closed;
            log::info!("Camera path: {}", if path.closed { "closed loop" } else { "open" });
        } else {
            log::warn!("No camera path");
        }
    }

    /// Filesystem-safe slug of the scene name
    fn scene_slug(&self) -> String {
        let slug: String = self
            .scene
            .name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        slug.trim_matches('-').to_string()
    }

    /// The view currently on screen, for saving (V) and HQ renders (P)
    fn current_view(&self) -> View {
        View {
            scene: self.scene_path.clone(),
            rotation: self.camera.yaw,
            pitch: self.camera.pitch,
            distance: self.camera.distance,
            focus: self.camera.focus.to_array(),
            offset: [0.0; 3],
            point_size: self.point_size,
            fog_near: self.fog_near,
            fog_far: self.fog_far,
            fog_brightness: self.fog_brightness,
            fog_saturation: self.fog_saturation,
            color_falloff: Some(self.color_falloff),
            color_contrast: Some(self.color_contrast),
            renderer: match self.render_mode {
                RenderMode::Points => None,
                RenderMode::Splat => Some("splat".to_string()),
            },
            exposure: match self.render_mode {
                RenderMode::Points => None,
                RenderMode::Splat => Some(self.exposure),
            },
        }
    }

    /// Save the current view parameters to views/<scene>-<timestamp>.toml
    pub fn save_view(&self) {
        let view = self.current_view();

        let path = format!("views/{}-{}.toml", self.scene_slug(), unix_timestamp());

        match view.save(&path) {
            Ok(()) => log::info!("View saved to {}", path),
            Err(e) => log::error!("{}", e),
        }
    }

    /// Write the scene (with any interactive edits) back to its TOML file.
    /// Camera framing and render params adjusted in-app become scene defaults.
    pub fn save_scene(&mut self) {
        self.scene.camera_focus = self.camera.focus;
        self.scene.camera_distance = self.camera.distance;
        self.scene.camera_yaw = self.camera.yaw;
        self.scene.camera_pitch = self.camera.pitch;
        self.scene.point_size = self.point_size;
        self.scene.color_falloff = self.color_falloff;
        self.scene.color_contrast = self.color_contrast;

        let path = self
            .scene_path
            .clone()
            .unwrap_or_else(|| format!("scenes/untitled-{}.toml", unix_timestamp()));

        match self.scene.save(&path) {
            Ok(()) => {
                log::info!("Scene saved to {}", path);
                self.scene_path = Some(path);
            }
            Err(e) => log::error!("{}", e),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        if width > 0 && height > 0 {
            self.depth_texture = create_depth_texture(&self.gpu.device, width, height, "main_depth");
        }
    }

    pub fn reset(&mut self) {
        self.point_compute.reset(&self.gpu.queue);
        self.frame_count = 0;
    }

    /// Whether a mouse drag (orbit/pan/gizmo) is currently in progress —
    /// used by the egui event-gating rules in main.rs: a mouse-release
    /// always reaches the app while a drag is active, and gizmo hover is
    /// never suppressed mid-drag, even if the pointer strays over a panel.
    pub fn has_active_drag(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }

    /// Current FPS, for the egui test window (Phase 1) and future status bar.
    pub fn current_fps(&self) -> f32 {
        self.fps_tracker.current_fps
    }

    pub fn zoom_in(&mut self) {
        self.camera.zoom(1.0);
    }

    pub fn zoom_out(&mut self) {
        self.camera.zoom(-1.0);
    }

    /// Mouse wheel: zoom — unless a gizmo is hovered, in which case it
    /// adjusts that transform's chaos weight (its selection probability),
    /// the lever that emphasizes an element without changing structure
    pub fn on_scroll(&mut self, steps: f32) {
        if let Some(hit) = self.hovered {
            self.selected_transform = Some(hit.transform);
            let w = &mut self.scene.transforms[hit.transform].weight;
            *w = (*w * 1.08f32.powf(steps)).clamp(0.01, 100.0);
            log::info!("T{} weight: {:.2} (scroll)", hit.transform, *w);
            self.sync_transforms_to_gpu();
            return;
        }
        self.camera.zoom(steps);
    }

    /// Toggle flightsim-style pitch inversion (persisted across sessions)
    pub fn toggle_invert_pitch(&mut self) {
        self.prefs.invert_pitch = !self.prefs.invert_pitch;
        log::info!(
            "Pitch inversion: {}",
            if self.prefs.invert_pitch { "inverted" } else { "normal" }
        );
        self.prefs.save();
    }

    /// `suppress_hover`: true when the pointer is over an egui area and no
    /// drag is active — gizmo hover-picking is skipped so egui panels don't
    /// fight the 3D gizmo highlight/cursor-icon underneath them. Active
    /// drags (orbit/pan/gizmo) always keep receiving motion regardless.
    pub fn on_cursor_moved(&mut self, x: f32, y: f32, suppress_hover: bool) {
        let (dx, dy) = (x - self.cursor.0, y - self.cursor.1);
        self.cursor = (x, y);

        match self.drag {
            Drag::None => {
                if suppress_hover {
                    if self.hovered.is_some() {
                        self.gizmo_renderer.set_highlight(&self.gpu.queue, None);
                        self.window.set_cursor(winit::window::CursorIcon::Default);
                        self.hovered = None;
                    }
                } else {
                    self.update_hover();
                }
            }
            Drag::Orbit => {
                let dy = if self.prefs.invert_pitch { -dy } else { dy };
                self.camera.orbit(dx, dy);
            }
            Drag::Pan => {
                let (_, h) = self.gpu.size();
                self.camera.pan(dx, dy, h as f32);
            }
            Drag::Gizmo { transform, mode, start_matrix } => {
                self.update_gizmo_drag(transform, mode, start_matrix);
            }
        }
    }

    /// Re-pick the gizmo part under the cursor; update the glow highlight and
    /// the cursor icon so grabbable things announce themselves
    fn update_hover(&mut self) {
        use winit::window::CursorIcon;

        let hit = if self.show_gizmos {
            let (w, h) = self.gpu.size();
            let matrices: Vec<Mat4> = self.scene.transforms.iter().map(|t| t.matrix).collect();
            crate::pick::pick_gizmo(
                &matrices,
                self.current_view_proj(),
                self.cursor,
                w as f32,
                h as f32,
            )
        } else {
            None
        };

        let changed = match (&self.hovered, &hit) {
            (None, None) => false,
            (Some(a), Some(b)) => a.transform != b.transform || a.part != b.part,
            _ => true,
        };
        if changed {
            self.gizmo_renderer.set_highlight(
                &self.gpu.queue,
                hit.map(|h| (h.transform, h.part)),
            );
            self.window.set_cursor(if hit.is_some() {
                CursorIcon::Grab
            } else {
                CursorIcon::Default
            });
            self.hovered = hit;
        }
    }

    /// Current view-projection matrix for the window surface
    fn current_view_proj(&self) -> Mat4 {
        let (w, h) = self.gpu.size();
        self.camera.view_proj(w as f32 / h as f32)
    }

    pub fn on_mouse_press(&mut self, button: winit::event::MouseButton) {
        use winit::event::MouseButton;
        match button {
            MouseButton::Left => {
                // Overlay panels swallow clicks when open
                if self.try_click_browser() {
                    return;
                }
                if self.try_click_help() {
                    return;
                }
                if let Some(drag) = self.try_grab_gizmo() {
                    self.drag = drag;
                    self.window.set_cursor(winit::window::CursorIcon::Grabbing);
                    return;
                }
                self.drag = if self.shift_held { Drag::Pan } else { Drag::Orbit };
                // Manual orbiting shouldn't fight the auto-orbit or a
                // playing path flythrough
                self.orbit_paused = true;
                self.path_play_t = None;
            }
            MouseButton::Middle => {
                self.drag = Drag::Pan;
                self.orbit_paused = true;
                self.path_play_t = None;
            }
            _ => {}
        }
    }

    pub fn on_mouse_release(&mut self, button: winit::event::MouseButton) {
        use winit::event::MouseButton;
        if matches!(button, MouseButton::Left | MouseButton::Middle) {
            if let Drag::Gizmo { transform, .. } = self.drag {
                let spec = &self.scene.transforms[transform];
                let t = spec.matrix.w_axis.truncate();
                log::info!(
                    "T{}: p=({:.3},{:.3},{:.3}) s={:.3}",
                    transform, t.x, t.y, t.z, spec.matrix.x_axis.truncate().length(),
                );
            }
            self.drag = Drag::None;
            self.update_hover();
        }
    }

    /// Try to start a gizmo drag at the current cursor position. Also selects
    /// the grabbed transform.
    fn try_grab_gizmo(&mut self) -> Option<Drag> {
        use crate::pick::{pick_gizmo, GizmoPart};

        if !self.show_gizmos {
            return None;
        }
        let (w, h) = self.gpu.size();
        let (w, h) = (w as f32, h as f32);
        let view_proj = self.current_view_proj();

        let matrices: Vec<Mat4> = self.scene.transforms.iter().map(|t| t.matrix).collect();
        let hit = pick_gizmo(&matrices, view_proj, self.cursor, w, h)?;

        self.selected_transform = Some(hit.transform);
        let m = matrices[hit.transform];
        let origin = m.w_axis.truncate();
        let inv_vp = view_proj.inverse();
        let (ray_o, ray_d) = crate::camera::cursor_ray(inv_vp, self.cursor.0, self.cursor.1, w, h);

        // Ctrl turns any grab into a uniform scale
        let mode = if self.ctrl_held {
            GizmoDragMode::Scale { start_y: self.cursor.1 }
        } else {
            match hit.part {
                GizmoPart::Origin => {
                    let normal = self.camera.forward();
                    let grab = crate::pick::ray_plane(ray_o, ray_d, origin, normal)?;
                    GizmoDragMode::TranslateView { normal, grab_offset: origin - grab }
                }
                GizmoPart::Axis(k) => {
                    let axis = m.col(k).truncate().normalize_or(Vec3::X);
                    let s0 = crate::pick::line_param_closest_to_ray(origin, axis, ray_o, ray_d);
                    GizmoDragMode::TranslateAxis { axis, s0 }
                }
                GizmoPart::RotEdge(k) => {
                    let mut axis = m.col(k).truncate().normalize_or(Vec3::Y);
                    // Screen-CCW should mean world-CCW around the axis as
                    // seen by the camera; flip when the axis points away
                    if axis.dot(self.camera.eye() - origin) < 0.0 {
                        axis = -axis;
                    }
                    let center = crate::camera::world_to_screen(origin, view_proj, w, h)?;
                    GizmoDragMode::Rotate {
                        axis,
                        center,
                        start_angle: crate::pick::screen_angle(center, self.cursor),
                    }
                }
            }
        };

        Some(Drag::Gizmo {
            transform: hit.transform,
            mode,
            start_matrix: m,
        })
    }

    /// Recompute the dragged transform's matrix from the grab-time state and
    /// the current cursor, then push it live to the GPU
    fn update_gizmo_drag(&mut self, transform: usize, mode: GizmoDragMode, start: Mat4) {
        let (w, h) = self.gpu.size();
        let (w, h) = (w as f32, h as f32);
        let view_proj = self.current_view_proj();
        let inv_vp = view_proj.inverse();
        let (ray_o, ray_d) = crate::camera::cursor_ray(inv_vp, self.cursor.0, self.cursor.1, w, h);
        let start_origin = start.w_axis.truncate();

        let new_matrix = match mode {
            GizmoDragMode::TranslateView { normal, grab_offset } => {
                let Some(hit) = crate::pick::ray_plane(ray_o, ray_d, start_origin, normal) else {
                    return;
                };
                let mut m = start;
                m.w_axis = (hit + grab_offset).extend(1.0);
                m
            }
            GizmoDragMode::TranslateAxis { axis, s0 } => {
                let s = crate::pick::line_param_closest_to_ray(start_origin, axis, ray_o, ray_d);
                let mut m = start;
                m.w_axis = (start_origin + axis * (s - s0)).extend(1.0);
                m
            }
            GizmoDragMode::Rotate { axis, center, start_angle } => {
                let angle = crate::pick::screen_angle(center, self.cursor);
                let delta = crate::pick::wrap_angle(angle - start_angle);
                let rot = Mat4::from_quat(glam::Quat::from_axis_angle(axis, delta));
                // Rotate the linear part around the transform's own origin
                let mut m = rot * Mat4::from_cols(
                    start.x_axis,
                    start.y_axis,
                    start.z_axis,
                    glam::Vec4::W,
                );
                m.w_axis = start.w_axis;
                m
            }
            GizmoDragMode::Scale { start_y } => {
                let factor = ((start_y - self.cursor.1) * 0.005).exp().clamp(0.02, 50.0);
                let mut m = start;
                m.x_axis *= factor;
                m.y_axis *= factor;
                m.z_axis *= factor;
                m
            }
        };

        self.scene.transforms[transform].matrix = new_matrix;
        self.sync_transforms_to_gpu();
    }

    // === Scene browser (B) ===

    /// Open/close the scene browser overlay. Opening rescans scenes/ (plus
    /// the current scene's directory, if different).
    pub fn toggle_browser(&mut self) {
        if self.show_browser {
            self.show_browser = false;
            return;
        }

        let mut dirs = vec![std::path::PathBuf::from("scenes")];
        if let Some(dir) = self
            .scene_path
            .as_ref()
            .and_then(|p| Path::new(p).parent())
            .filter(|d| !d.as_os_str().is_empty() && *d != Path::new("scenes"))
        {
            dirs.push(dir.to_path_buf());
        }

        let mut files: Vec<std::path::PathBuf> = dirs
            .iter()
            .filter_map(|d| fs::read_dir(d).ok())
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();
        files.dedup();

        if files.is_empty() {
            log::warn!("No scene files found in scenes/");
            return;
        }
        // Start on the current scene when it's in the list
        self.browser_selected = self
            .scene_path
            .as_ref()
            .and_then(|p| files.iter().position(|f| f == Path::new(p)))
            .unwrap_or(0);
        self.browser_files = files;
        self.show_browser = true;
    }

    pub fn browser_move(&mut self, down: bool) {
        let n = self.browser_files.len();
        if n == 0 {
            return;
        }
        self.browser_selected = if down {
            (self.browser_selected + 1) % n
        } else {
            (self.browser_selected + n - 1) % n
        };
    }

    pub fn browser_load_selected(&mut self) {
        if let Some(path) = self.browser_files.get(self.browser_selected).cloned() {
            self.load_scene_file(&path);
        }
        self.show_browser = false;
    }

    /// Browser panel geometry: (x, y of first row, line height)
    fn browser_panel_geometry(&self, height: f32) -> (f32, f32, f32) {
        let line_height = 16.0f32;
        let panel_lines = self.browser_files.len() + 2;
        let y = (height - panel_lines as f32 * line_height) * 0.5;
        (6.0, y, line_height)
    }

    /// Click a browser row to load that scene. Returns true if the click was
    /// consumed by the panel.
    fn try_click_browser(&mut self) -> bool {
        if !self.show_browser {
            return false;
        }
        let (_, h) = self.gpu.size();
        let (px, py, lh) = self.browser_panel_geometry(h as f32);
        let (cx, cy) = self.cursor;
        let panel_h = (self.browser_files.len() + 2) as f32 * lh;
        if cx < px || cx > px + 380.0 || cy < py || cy > py + panel_h {
            // Clicking outside closes the browser
            self.show_browser = false;
            return true;
        }
        let row = ((cy - py - lh * 0.5) / lh).floor() as i32 - 1;
        if row >= 0 && (row as usize) < self.browser_files.len() {
            self.browser_selected = row as usize;
            self.browser_load_selected();
        }
        true
    }

    /// Replace the current scene with one loaded from disk, rebuilding the
    /// GPU pipelines and resetting camera/selection to the scene's defaults
    pub fn load_scene_file(&mut self, path: &Path) {
        let scene = match Scene::load(path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("Failed to load {}: {}", path.display(), e);
                return;
            }
        };
        log::info!("Loading scene: {} ({})", scene.name, path.display());

        self.camera = OrbitCamera {
            yaw: scene.camera_yaw,
            pitch: scene.camera_pitch,
            distance: scene.camera_distance,
            focus: scene.camera_focus,
        };
        self.point_size = scene.point_size;
        self.color_falloff = scene.color_falloff;
        self.color_contrast = scene.color_contrast;
        self.buffer_capacity = scene.point_count as u32;
        self.transform_enabled = vec![true; scene.transforms.len()];
        self.selected_transform = Some(0);
        self.selected_variation = 0;
        self.drag = Drag::None;
        self.hovered = None;
        self.scene = scene;
        self.scene_path = Some(path.to_string_lossy().into_owned());
        self.rebuild_pipelines();
    }

    /// Text overlay for the scene browser
    fn build_browser_entries(&self, height: f32) -> Vec<TextEntry> {
        let (px, py, lh) = self.browser_panel_geometry(height);
        let font_size = 13.0;

        let mut lines = vec!["Scenes  (Enter/click = load, Esc/B = close)".to_string()];
        for (i, f) in self.browser_files.iter().enumerate() {
            let stem = f.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
            let sel = if i == self.browser_selected { ">" } else { " " };
            let cur = if self.scene_path.as_deref() == Some(&f.to_string_lossy() as &str) {
                "*"
            } else {
                " "
            };
            lines.push(format!("{}{} {}", sel, cur, stem));
        }
        let text = lines.join("\n");

        let bg_width = 4 + text.lines().map(|l| l.len()).max().unwrap_or(0);
        let bg: String =
            vec!["\u{2588}".repeat(bg_width); self.browser_files.len() + 2].join("\n");

        vec![
            TextEntry { text: bg, x: px, y: py, color: [10, 10, 20, 225], font_size },
            TextEntry {
                text,
                x: px + font_size,
                y: py + lh * 0.5,
                color: [235, 235, 245, 255],
                font_size,
            },
        ]
    }

    // === High-quality background render (P) ===

    /// Kick off an offline render of the current framing on a background
    /// thread (own wgpu device, so the realtime view keeps running). Pauses
    /// the auto-orbit so the rendered framing is the one on screen.
    pub fn start_hq_render(&mut self) {
        use std::sync::atomic::Ordering;

        if self.hq_render_in_flight.load(Ordering::SeqCst) {
            log::warn!("HQ render already running");
            return;
        }
        self.orbit_paused = true;

        let mut scene = self.scene.clone();
        // Scale the point budget up from the interactive count
        scene.point_count = (scene.point_count * 4).clamp(8_000_000, 40_000_000);

        let view = self.current_view();

        let splat = self.render_mode == RenderMode::Splat;
        let exposure = self.exposure;

        let out = format!("renders/{}-{}.png", self.scene_slug(), unix_timestamp());
        let flag = self.hq_render_in_flight.clone();
        flag.store(true, Ordering::SeqCst);
        log::info!(
            "HQ render started: {} ({} points, 2560x1440) — timing prints when done",
            out, scene.point_count,
        );

        std::thread::spawn(move || {
            let result = crate::offline::render(crate::offline::OfflineParams {
                scene,
                view: Some(view),
                width: 2560,
                height: 1440,
                out_path: Path::new(&out),
                accumulate: 96,
                fog_enabled: false, // view carries the real fog settings
                grid: crate::offline::GridMode::Single,
                splat,
                exposure,
            });
            match result {
                Ok(()) => log::info!("HQ render finished: {}", out),
                Err(e) => log::error!("HQ render failed: {}", e),
            }
            flag.store(false, Ordering::SeqCst);
        });
    }

    // === Chaos-game traces (X) ===

    /// X: show traces (re-rolling new random walkers each press);
    /// Shift+X hides them.
    pub fn toggle_traces(&mut self, hide: bool) {
        if hide {
            self.show_traces = false;
            log::info!("Traces: off");
            return;
        }
        self.show_traces = true;
        self.regenerate_traces();
    }

    /// Run fresh CPU walkers through the current IFS and rebuild the trace
    /// line geometry. Head of each trace is brightest; the tail fades out.
    fn regenerate_traces(&mut self) {
        const TRACES: usize = 24;
        const STEPS: usize = 50;

        let traces = crate::trace::generate_traces(
            &self.scene.transforms,
            &self.transform_enabled,
            TRACES,
            STEPS,
            &mut rand::thread_rng(),
        );

        let mut verts = Vec::with_capacity(traces.iter().map(|t| t.len() * 2).sum());
        for trace in &traces {
            for (i, pair) in trace.windows(2).enumerate() {
                // Age fade along the trace: oldest segments nearly invisible
                let age = (i + 1) as f32 / (trace.len() - 1) as f32;
                let alpha = 0.08 + 0.72 * age * age;
                for step in pair {
                    let cm = self.scene.colormap
                        [(step.color_val * 255.0).clamp(0.0, 255.0) as usize];
                    verts.push(LineVertex {
                        position: step.pos.to_array(),
                        color: [cm[0], cm[1], cm[2], alpha],
                    });
                }
            }
        }
        let segments = verts.len() / 2;
        self.line_renderer.set_lines(&self.gpu.device, &verts);
        log::info!("Traces: {} walkers × {} steps ({} segments)", TRACES, STEPS, segments);
    }

    // === Evolutionary exploration (U / Shift+U) ===

    /// Apply a random mutation to the scene (undoable with Shift+U).
    /// Repeated presses walk the scene through mutation space; Ctrl+S keeps
    /// a variant you like.
    pub fn mutate_scene(&mut self) {
        self.undo_stack.push(self.scene.clone());
        if self.undo_stack.len() > 32 {
            self.undo_stack.remove(0);
        }

        let log = crate::mutate::mutate(&mut self.scene, &mut rand::thread_rng(), 1.0);
        log::info!("Mutation: {}", log.join("; "));
        self.after_scene_shape_change();
    }

    /// Revert the most recent mutation
    pub fn undo_mutation(&mut self) {
        let Some(prev) = self.undo_stack.pop() else {
            log::warn!("Nothing to undo");
            return;
        };
        self.scene = prev;
        log::info!("Mutation undone ({} left on stack)", self.undo_stack.len());
        self.after_scene_shape_change();
    }

    /// Reconcile app state after the scene changed under us (mutation/undo
    /// may add or remove transforms) and rebuild the GPU pipelines
    fn after_scene_shape_change(&mut self) {
        let n = self.scene.transforms.len();
        self.transform_enabled.resize(n, true);
        if let Some(sel) = self.selected_transform {
            self.selected_transform = Some(sel.min(n.saturating_sub(1)));
        }
        self.drag = Drag::None;
        self.hovered = None;
        self.rebuild_pipelines();
    }

    // === Transform editing (see also gizmo drags above) ===

    /// The transform editing keys act on: the explicit selection, or T0
    fn selection(&self) -> Option<usize> {
        let n = self.scene.transforms.len();
        if n == 0 {
            return None;
        }
        Some(self.selected_transform.unwrap_or(0).min(n - 1))
    }

    /// Add a transform: a nudged copy of the selection, or (fresh) a small
    /// default one. Rebuilds the GPU pipelines for the new transform count.
    pub fn add_transform(&mut self, fresh: bool) {
        let (spec, name, color) = if fresh || self.selection().is_none() {
            (
                TransformSpec {
                    matrix: Mat4::from_scale_rotation_translation(
                        Vec3::splat(0.5),
                        glam::Quat::IDENTITY,
                        Vec3::new(0.3, 0.3, 0.0),
                    ),
                    color_value: 0.5,
                    weight: 1.0,
                    color_speed: self.scene.color_speed,
                    explicit_color_speed: None,
                    variations: TransformSpec::linear_variations(),
                },
                None,
                Vec3::splat(0.9),
            )
        } else {
            let idx = self.selection().unwrap();
            let mut spec = self.scene.transforms[idx].clone();
            // Nudge so the copy's gizmo doesn't sit exactly on the original
            spec.matrix.w_axis += glam::Vec4::new(0.15, 0.0, 0.0, 0.0);
            let name = self.scene.transform_names[idx]
                .as_ref()
                .map(|n| format!("{} copy", n));
            (spec, name, self.scene.colors[idx])
        };

        self.scene.transforms.push(spec);
        self.scene.transform_names.push(name);
        self.scene.colors.push(color);
        self.transform_enabled.push(true);
        self.selected_transform = Some(self.scene.transforms.len() - 1);
        self.scene.regenerate_colormap();
        self.rebuild_pipelines();
        log::info!("Added transform T{}", self.scene.transforms.len() - 1);
    }

    /// Delete the selected transform (keeps at least one)
    pub fn delete_selected_transform(&mut self) {
        let Some(idx) = self.selection() else { return };
        if self.scene.transforms.len() <= 1 {
            log::warn!("Cannot delete the last transform");
            return;
        }
        self.scene.transforms.remove(idx);
        self.scene.transform_names.remove(idx);
        self.scene.colors.remove(idx);
        self.transform_enabled.remove(idx);
        self.selected_transform = Some(idx.min(self.scene.transforms.len() - 1));
        self.drag = Drag::None;
        self.scene.regenerate_colormap();
        self.rebuild_pipelines();
        log::info!("Deleted transform T{}", idx);
    }

    /// Multiply the selected transform's chaos-game weight
    pub fn adjust_weight(&mut self, increase: bool) {
        let Some(idx) = self.selection() else { return };
        let factor = if increase { 1.15 } else { 1.0 / 1.15 };
        let w = &mut self.scene.transforms[idx].weight;
        *w = (*w * factor).clamp(0.01, 100.0);
        log::info!("T{} weight: {:.2}", idx, *w);
        self.sync_transforms_to_gpu();
    }

    /// Nudge the selected transform's gradient color in HSV space
    /// (channel: 0 = hue, 1 = saturation, 2 = value)
    pub fn adjust_color(&mut self, channel: usize, increase: bool) {
        let Some(idx) = self.selection() else { return };
        let (mut h, mut s, mut v) = rgb_to_hsv(self.scene.colors[idx]);
        let dir = if increase { 1.0 } else { -1.0 };
        match channel {
            0 => h = (h + dir * 15.0).rem_euclid(360.0),
            1 => s = (s + dir * 0.08).clamp(0.0, 1.0),
            2 => v = (v + dir * 0.08).clamp(0.05, 1.0),
            _ => return,
        }
        self.scene.colors[idx] = hsv_to_rgb(h, s, v);
        let c = self.scene.colors[idx];
        log::info!("T{} color: h={:.0} s={:.2} v={:.2} rgb=({:.2},{:.2},{:.2})", idx, h, s, v, c.x, c.y, c.z);
        self.scene.regenerate_colormap();
        self.point_compute.update_colormap(&self.gpu.queue, &self.scene.colormap);
    }

    /// Step the variation slot targeted by the weight keys
    pub fn cycle_variation(&mut self, forward: bool) {
        let n = crate::scene::NUM_VARIATIONS;
        self.selected_variation = if forward {
            (self.selected_variation + 1) % n
        } else {
            (self.selected_variation + n - 1) % n
        };
        let idx = self.selection();
        let w = idx.map_or(0.0, |i| self.scene.transforms[i].variations[self.selected_variation]);
        log::info!(
            "Variation slot: {} (weight {:.2} on selected transform)",
            crate::scene::VARIATION_NAMES[self.selected_variation], w,
        );
    }

    /// Adjust the selected transform's weight for the targeted variation
    pub fn adjust_variation_weight(&mut self, increase: bool) {
        let Some(idx) = self.selection() else { return };
        let step = if increase { 0.05 } else { -0.05 };
        let w = &mut self.scene.transforms[idx].variations[self.selected_variation];
        // Snap through zero so slots can be cleanly removed
        *w = ((*w + step) * 100.0).round() / 100.0;
        *w = w.clamp(-4.0, 4.0);
        log::info!(
            "T{} {} = {:.2}",
            idx, crate::scene::VARIATION_NAMES[self.selected_variation], *w,
        );
        self.sync_transforms_to_gpu();
    }

    /// Recreate the GPU pipelines after the transform count changed.
    /// The chaos game restarts from warmup.
    fn rebuild_pipelines(&mut self) {
        crate::scene::resolve_color_speeds(
            &mut self.scene.transforms,
            self.scene.color_speed,
            self.color_falloff,
        );
        self.point_compute = PointCompute::new(
            &self.gpu.device,
            &self.scene.transforms,
            &self.scene.colormap,
            self.buffer_capacity,
        );
        self.point_renderer = PointRenderer::new(
            &self.gpu.device,
            self.gpu.format,
            &self.point_compute.point_buffer,
            &self.point_compute.colormap_buffer,
        );
        self.splat_renderer = SplatRenderer::new(
            &self.gpu.device,
            self.gpu.format,
            &self.point_compute.point_buffer,
            &self.point_compute.colormap_buffer,
        );
        self.gizmo_renderer = GizmoRenderer::new(
            &self.gpu.device,
            self.gpu.format,
            &self.scene.transforms,
        );
        self.point_compute.update_weights(
            &self.gpu.queue,
            &self.scene.transforms,
            &self.transform_enabled,
        );
        self.gizmo_renderer.update_alpha(&self.gpu.queue, &self.transform_enabled);
        self.frame_count = 0;
        if self.show_traces {
            self.regenerate_traces();
        }
    }

    /// Push edited transforms to the chaos-game and gizmo pipelines and
    /// restart point accumulation so the fractal re-forms live
    fn sync_transforms_to_gpu(&mut self) {
        // Contraction feeds falloff-derived color speeds, so re-resolve
        crate::scene::resolve_color_speeds(
            &mut self.scene.transforms,
            self.scene.color_speed,
            self.color_falloff,
        );
        self.point_compute.update_weights(
            &self.gpu.queue,
            &self.scene.transforms,
            &self.transform_enabled,
        );
        self.gizmo_renderer.update_transforms(&self.gpu.queue, &self.scene.transforms);
        self.reset();
        if self.show_traces {
            self.regenerate_traces();
        }
    }

    pub fn toggle_render_mode(&mut self) {
        self.render_mode = match self.render_mode {
            RenderMode::Points => RenderMode::Splat,
            RenderMode::Splat => RenderMode::Points,
        };
        log::info!(
            "Renderer: {}",
            match self.render_mode {
                RenderMode::Points => "points",
                RenderMode::Splat => "splat (additive log-density)",
            }
        );
    }

    pub fn adjust_exposure(&mut self, increase: bool) {
        let factor = if increase { 1.25 } else { 0.8 };
        self.exposure = (self.exposure * factor).clamp(0.01, 100.0);
        log::info!("Splat exposure: {:.2}", self.exposure);
    }

    pub fn adjust_point_size(&mut self, increase: bool) {
        let factor = if increase { 1.1 } else { 0.909 };
        self.point_size = (self.point_size * factor).clamp(0.0001, 0.1);
        log::info!("Point size: {:.5}", self.point_size);
    }

    pub fn adjust_fog_intensity(&mut self, more_fog: bool) {
        let factor = if more_fog { 0.7 } else { 1.4 };
        let old_b = self.fog_brightness;
        let old_s = self.fog_saturation;
        self.fog_brightness = (self.fog_brightness * factor).clamp(0.05, 1.0);
        self.fog_saturation = (self.fog_saturation * factor).clamp(0.05, 1.0);
        log::info!("Fog intensity: brightness {:.2}->{:.2}, saturation {:.2}->{:.2}",
            old_b, self.fog_brightness, old_s, self.fog_saturation);
    }

    pub fn adjust_fog_near(&mut self, closer: bool) {
        let delta = if closer { -0.5 } else { 0.5 };
        self.fog_near = (self.fog_near + delta).clamp(0.1, self.fog_far - 1.0);
        log::info!("Fog near: {:.1}", self.fog_near);
    }

    pub fn adjust_fog_far(&mut self, closer: bool) {
        let delta = if closer { -1.0 } else { 1.0 };
        self.fog_far = (self.fog_far + delta).clamp(self.fog_near + 1.0, 30.0);
        log::info!("Fog far: {:.1}", self.fog_far);
    }

    /// Re-resolve per-transform color speeds from the current falloff and
    /// push them to the GPU, then reset so the point buffer refills quickly
    fn refresh_color_speeds(&mut self) {
        crate::scene::resolve_color_speeds(
            &mut self.scene.transforms,
            self.scene.color_speed,
            self.color_falloff,
        );
        self.point_compute.update_weights(
            &self.gpu.queue,
            &self.scene.transforms,
            &self.transform_enabled,
        );
        self.reset();
    }

    /// Adjust the scale-aware color falloff exponent. Turning it up from 0
    /// enters scale-aware mode at the neutral value 1.0.
    pub fn adjust_color_falloff(&mut self, finer: bool) {
        let old = self.color_falloff;
        self.color_falloff = if old == 0.0 {
            1.0
        } else {
            let factor = if finer { 1.0 / 1.15 } else { 1.15 };
            (old * factor).clamp(0.05, 4.0)
        };
        log::info!("Color falloff: {:.2} -> {:.2} (lower = finer color detail)", old, self.color_falloff);
        self.refresh_color_speeds();
    }

    pub fn adjust_color_contrast(&mut self, increase: bool) {
        let factor = if increase { 1.15 } else { 1.0 / 1.15 };
        self.color_contrast = (self.color_contrast * factor).clamp(0.25, 16.0);
        log::info!("Color contrast: {:.2}", self.color_contrast);
    }

    pub fn toggle_gizmos(&mut self) {
        self.show_gizmos = !self.show_gizmos;
        log::info!("Gizmos: {}", if self.show_gizmos { "on" } else { "off" });
    }

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    pub fn toggle_text_overlay(&mut self) {
        self.show_text = !self.show_text;
        self.selected_transform = if self.show_text { Some(0) } else { None };
        log::info!("Text overlay: {}", if self.show_text { "on" } else { "off" });
    }

    pub fn select_prev_transform(&mut self) {
        if let Some(idx) = self.selected_transform {
            let n = self.scene.transforms.len();
            self.selected_transform = Some(if idx == 0 { n - 1 } else { idx - 1 });
        }
    }

    pub fn select_next_transform(&mut self) {
        if let Some(idx) = self.selected_transform {
            let n = self.scene.transforms.len();
            self.selected_transform = Some((idx + 1) % n);
        }
    }

    pub fn toggle_selected_transform(&mut self) {
        let Some(idx) = self.selected_transform else { return };
        if idx >= self.transform_enabled.len() { return }

        // Guard: don't disable the last enabled transform
        let enabled_count = self.transform_enabled.iter().filter(|&&e| e).count();
        if self.transform_enabled[idx] && enabled_count <= 1 {
            log::warn!("Cannot disable last remaining transform");
            return;
        }

        self.transform_enabled[idx] = !self.transform_enabled[idx];
        log::info!(
            "Transform {} {}", idx,
            if self.transform_enabled[idx] { "enabled" } else { "disabled" },
        );

        self.point_compute.update_weights(
            &self.gpu.queue,
            &self.scene.transforms,
            &self.transform_enabled,
        );
        self.gizmo_renderer.update_alpha(&self.gpu.queue, &self.transform_enabled);
        self.reset();
    }

    pub fn request_screenshot(&mut self) {
        self.pending_screenshot = true;
    }

    /// Use native 1px point primitives (~3x faster) when the projected point
    /// size at the orbit distance would be subpixel anyway
    fn use_point_primitives(&self, screen_height: f32) -> bool {
        self.point_size * screen_height / self.camera.distance <= 1.5
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_update).as_secs_f32().min(0.1);
        self.last_update = now;
        self.frame_dt = dt;

        let should_log = self.fps_tracker.frame();
        self.frame_count += 1;
        if let Some(t) = self.path_play_t {
            // Path flythrough preview: fly the spline in real time
            match &self.scene.camera_path {
                Some(path) if path.keys.len() >= 2 => {
                    let t = t + dt / path.duration();
                    if !path.closed && t >= 1.0 {
                        self.camera = path.sample(1.0);
                        self.path_play_t = None;
                        log::info!("Camera path: finished");
                    } else {
                        let t = if path.closed { t.rem_euclid(1.0) } else { t };
                        self.camera = path.sample(t);
                        self.path_play_t = Some(t);
                    }
                }
                _ => self.path_play_t = None,
            }
        } else if !self.orbit_paused {
            // Time-based so the orbit speed is refresh-rate independent
            // (0.003 rad/frame at the old 60 FPS baseline)
            self.camera.yaw += 0.18 * dt;
        }

        if should_log {
            let point_count = self.point_compute.valid_point_count();
            let warmup_done = self.point_compute.current_frame >= self.point_compute.warmup_frames;
            let title = format!(
                "Fracturize | {:.0} FPS | {:.1}ms | {}k points{}",
                self.fps_tracker.current_fps,
                self.fps_tracker.current_frametime_ms,
                point_count / 1000,
                if warmup_done { "" } else { " (warming up)" },
            );
            self.window.set_title(&title);
            log::info!(
                "FPS: {:.1} | Frametime: {:.2}ms | Points: {}",
                self.fps_tracker.current_fps,
                self.fps_tracker.current_frametime_ms,
                point_count,
            );
        }

        // Rebuild the legacy TextEntry shim (HUD, help panel, browser,
        // gizmo labels) for this frame; ui::draw_legacy_text paints it via
        // an egui foreground layer once RedrawRequested runs ctx.run_ui.
        let (width, height) = self.gpu.size();
        let aspect = width as f32 / height as f32;
        let view_proj = self.camera.view_proj(aspect);
        self.ui_state.legacy_entries =
            self.build_text_entries(view_proj, width as f32, height as f32);
    }

    pub fn take_screenshot(&mut self) {
        let point_count = self.point_compute.valid_point_count();
        if point_count == 0 {
            log::warn!("No points to screenshot");
            return;
        }

        let aspect = SCREENSHOT_WIDTH as f32 / SCREENSHOT_HEIGHT as f32;
        let mvp = self.camera.view_proj(aspect);

        let camera = CameraUniforms::new(
            mvp,
            SCREENSHOT_HEIGHT as f32,
            self.point_size,
            aspect,
            1.0,
            self.fog_near,
            self.fog_far,
            self.fog_brightness,
            self.fog_saturation,
            self.color_contrast,
        );

        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("screenshot_encoder"),
        });

        let color_view = self.screenshot_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = self.screenshot_depth.create_view(&wgpu::TextureViewDescriptor::default());
        match self.render_mode {
            RenderMode::Points => {
                self.point_renderer.upload_camera(&self.gpu.queue, &camera);
                let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("screenshot_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });

                self.point_renderer.draw(
                    &mut render_pass,
                    point_count,
                    self.use_point_primitives(SCREENSHOT_HEIGHT as f32),
                );
            }
            RenderMode::Splat => {
                // The accum texture resizes to the screenshot dimensions
                // here and back to the window's on the next frame
                self.splat_renderer.upload_camera(&self.gpu.queue, &camera);
                self.splat_renderer.upload_params(
                    &self.gpu.queue,
                    self.exposure,
                    self.buffer_capacity,
                    SCREENSHOT_HEIGHT as f32,
                    CLEAR_COLOR,
                );
                self.splat_renderer.render(
                    &self.gpu.device,
                    &mut encoder,
                    &color_view,
                    &depth_view,
                    SCREENSHOT_WIDTH,
                    SCREENSHOT_HEIGHT,
                    point_count,
                    self.use_point_primitives(SCREENSHOT_HEIGHT as f32),
                );
            }
        }

        let bytes_per_row = SCREENSHOT_WIDTH * 4;
        let padded_bytes_per_row = (bytes_per_row + 255) & !255;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.screenshot_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.screenshot_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(SCREENSHOT_HEIGHT),
                },
            },
            wgpu::Extent3d {
                width: SCREENSHOT_WIDTH,
                height: SCREENSHOT_HEIGHT,
                depth_or_array_layers: 1,
            },
        );

        self.gpu.queue.submit(std::iter::once(encoder.finish()));

        let buffer_slice = self.screenshot_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });

        {
            let data = buffer_slice.get_mapped_range();

            let mut pixels = vec![0u8; (SCREENSHOT_WIDTH * SCREENSHOT_HEIGHT * 4) as usize];
            for y in 0..SCREENSHOT_HEIGHT as usize {
                let src_start = y * padded_bytes_per_row as usize;
                let src_end = src_start + bytes_per_row as usize;
                let dst_start = y * bytes_per_row as usize;
                let dst_end = dst_start + bytes_per_row as usize;
                pixels[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
            }

            for chunk in pixels.chunks_mut(4) {
                chunk.swap(0, 2);
            }

            let screenshot_dir = Path::new("screenshots");
            if !screenshot_dir.exists() {
                fs::create_dir_all(screenshot_dir).expect("Failed to create screenshots directory");
            }

            // Never overwrite: slug + timestamp, with a counter for
            // multiple captures in the same second
            let base = format!("{}-{}", self.scene_slug(), unix_timestamp());
            let mut path = screenshot_dir.join(format!("{}.png", base));
            let mut n = 1;
            while path.exists() {
                path = screenshot_dir.join(format!("{}-{}.png", base, n));
                n += 1;
            }
            image::save_buffer(
                &path,
                &pixels,
                SCREENSHOT_WIDTH,
                SCREENSHOT_HEIGHT,
                image::ColorType::Rgba8,
            ).expect("Failed to save screenshot");
            log::info!("Screenshot saved to {}", path.display());
        }

        self.screenshot_buffer.unmap();
        self.pending_screenshot = false;
    }

    /// Rows of the help panel: key label, description, and what a click on
    /// the row does (None = informational only). Click = the first-listed
    /// binding, shift+click = the second.
    const HELP_ROWS: &'static [(&'static str, &'static str, Option<HelpAction>)] = &[
        ("H / ?", "toggle this help", Some(HelpAction::ToggleHelp)),
        ("Esc", "quit", None),
        ("drag", "orbit camera (pauses auto-orbit)", None),
        ("shift+drag", "pan focus (middle-drag works too)", None),
        ("scroll", "zoom", None),
        ("", "", None),
        ("drag gizmo dot", "select + move in view plane", None),
        ("drag O-axis edge", "move along that axis", None),
        ("drag outer edge", "rotate around third axis", None),
        ("ctrl+drag gizmo", "uniform scale", None),
        ("A / Shift+A", "duplicate selected / add new transform", Some(HelpAction::AddTransform)),
        ("Del", "delete selected transform", Some(HelpAction::DeleteTransform)),
        (". / ,", "selected weight up / down", Some(HelpAction::Weight)),
        ("J / Shift+J", "color hue up / down", Some(HelpAction::Hue)),
        ("K / Shift+K", "color saturation up / down", Some(HelpAction::Sat)),
        ("L / Shift+L", "color value up / down", Some(HelpAction::Val)),
        ("E / Shift+E", "next / prev variation slot", Some(HelpAction::CycleVariation)),
        ("= / -", "variation weight up / down", Some(HelpAction::VariationWeight)),
        ("Ctrl+S", "save scene TOML", Some(HelpAction::SaveScene)),
        ("U / Shift+U", "random mutation / undo it", Some(HelpAction::Mutate)),
        ("X / Shift+X", "chaos traces: show+re-roll / hide", Some(HelpAction::Traces)),
        ("I", "invert mouse pitch (saved to prefs)", Some(HelpAction::InvertPitch)),
        ("scroll on gizmo", "adjust that transform's weight", None),
        ("B", "browse + load scenes", Some(HelpAction::Browse)),
        ("P", "HQ render current view (background)", Some(HelpAction::HqRender)),
        ("", "", None),
        ("Space", "re-seed points (reset)", Some(HelpAction::Reset)),
        ("Up / Down", "zoom in / out", Some(HelpAction::Zoom)),
        ("", "(select transform when overlay is on)", None),
        ("Enter", "enable/disable selected transform", Some(HelpAction::ToggleSelected)),
        ("T", "toggle info overlay", Some(HelpAction::ToggleText)),
        ("G", "toggle transform gizmos", Some(HelpAction::ToggleGizmos)),
        ("O", "pause / resume camera orbit", Some(HelpAction::ToggleOrbit)),
        ("Z", "play / stop camera path flythrough", Some(HelpAction::PathPlay)),
        ("Y / Shift+Y", "add / remove camera path keypoint", Some(HelpAction::PathKey)),
        ("Ctrl+Y", "toggle camera path loop", None),
        ("V", "save current view to views/", Some(HelpAction::SaveView)),
        ("S", "save screenshot to screenshots/", Some(HelpAction::Screenshot)),
        ("R", "renderer: points / splat (log-density)", Some(HelpAction::RenderMode)),
        ("W / Shift+W", "splat exposure up / down", Some(HelpAction::Exposure)),
        ("] / [", "grow / shrink point size", Some(HelpAction::PointSize)),
        ("D / Shift+D", "finer / coarser color detail (falloff)", Some(HelpAction::ColorFalloff)),
        ("C / Shift+C", "less / more color contrast", Some(HelpAction::ColorContrast)),
        ("F / Shift+F", "more / less fog", Some(HelpAction::FogIntensity)),
        ("N / Shift+N", "fog start closer / farther", Some(HelpAction::FogNear)),
        ("M / Shift+M", "fog end closer / farther", Some(HelpAction::FogFar)),
    ];

    const HELP_FONT_SIZE: f32 = 13.0;
    const HELP_LINE_HEIGHT: f32 = Self::HELP_FONT_SIZE * 1.2;
    const HELP_X: f32 = 6.0;

    /// Top of the help panel background for the given window height
    fn help_panel_y(&self, height: f32) -> f32 {
        // +3 lines: title, subtitle, trailing spacer
        let panel_lines = Self::HELP_ROWS.len() + 3;
        (height - panel_lines as f32 * Self::HELP_LINE_HEIGHT) * 0.5
    }

    /// If the cursor sits on a clickable help row, run its action.
    /// Returns true when the click hit the panel (even a dead row).
    fn try_click_help(&mut self) -> bool {
        if !self.show_help {
            return false;
        }
        let (w, h) = self.gpu.size();
        let (_w, h) = (w as f32, h as f32);
        let panel_y = self.help_panel_y(h);
        // Panel text is ~68 monospace-ish chars; be generous on width
        let panel_w = 420.0;
        let panel_h = (Self::HELP_ROWS.len() + 3) as f32 * Self::HELP_LINE_HEIGHT;
        let (cx, cy) = self.cursor;
        if cx < Self::HELP_X || cx > Self::HELP_X + panel_w || cy < panel_y || cy > panel_y + panel_h {
            return false;
        }
        // Text starts half a line into the panel; rows follow title + subtitle
        let row = ((cy - panel_y - Self::HELP_LINE_HEIGHT * 0.5) / Self::HELP_LINE_HEIGHT).floor() as i32 - 2;
        if row >= 0 {
            if let Some((_, _, Some(action))) = Self::HELP_ROWS.get(row as usize) {
                self.run_help_action(*action, self.shift_held);
            }
        }
        true
    }

    /// Dispatch a clicked help row. `shift` selects the second-listed binding.
    fn run_help_action(&mut self, action: HelpAction, shift: bool) {
        match action {
            HelpAction::ToggleHelp => self.toggle_help(),
            HelpAction::Reset => self.reset(),
            HelpAction::Zoom => if shift { self.zoom_out() } else { self.zoom_in() },
            HelpAction::ToggleSelected => self.toggle_selected_transform(),
            HelpAction::ToggleText => self.toggle_text_overlay(),
            HelpAction::ToggleGizmos => self.toggle_gizmos(),
            HelpAction::ToggleOrbit => self.toggle_orbit(),
            HelpAction::PathPlay => self.toggle_path_play(),
            HelpAction::PathKey => {
                if shift {
                    self.remove_path_key()
                } else {
                    self.add_path_key()
                }
            }
            HelpAction::SaveView => self.save_view(),
            HelpAction::Screenshot => self.request_screenshot(),
            HelpAction::SaveScene => self.save_scene(),
            HelpAction::AddTransform => self.add_transform(shift),
            HelpAction::DeleteTransform => self.delete_selected_transform(),
            HelpAction::Weight => self.adjust_weight(!shift),
            HelpAction::Hue => self.adjust_color(0, !shift),
            HelpAction::Sat => self.adjust_color(1, !shift),
            HelpAction::Val => self.adjust_color(2, !shift),
            HelpAction::CycleVariation => self.cycle_variation(!shift),
            HelpAction::VariationWeight => self.adjust_variation_weight(!shift),
            HelpAction::PointSize => self.adjust_point_size(!shift),
            HelpAction::ColorFalloff => self.adjust_color_falloff(!shift),
            HelpAction::ColorContrast => self.adjust_color_contrast(shift),
            HelpAction::FogIntensity => self.adjust_fog_intensity(!shift),
            HelpAction::FogNear => self.adjust_fog_near(!shift),
            HelpAction::FogFar => self.adjust_fog_far(!shift),
            HelpAction::Mutate => {
                if shift {
                    self.undo_mutation()
                } else {
                    self.mutate_scene()
                }
            }
            HelpAction::Browse => self.toggle_browser(),
            HelpAction::HqRender => self.start_hq_render(),
            HelpAction::Traces => self.toggle_traces(shift),
            HelpAction::InvertPitch => self.toggle_invert_pitch(),
            HelpAction::RenderMode => self.toggle_render_mode(),
            HelpAction::Exposure => self.adjust_exposure(!shift),
        }
    }

    /// Keybind help panel, shown when `show_help` is on
    fn build_help_entries(&self, height: f32) -> Vec<TextEntry> {
        let font_size = Self::HELP_FONT_SIZE;
        let line_height = Self::HELP_LINE_HEIGHT;
        let panel_lines = Self::HELP_ROWS.len() + 3;
        let panel_y = self.help_panel_y(height);

        let text: String = [
            "Keybinds".to_string(),
            "(rows are clickable; shift+click = second binding)".to_string(),
        ]
        .into_iter()
        .chain(
            Self::HELP_ROWS
                .iter()
                .map(|(key, desc, _)| format!("{:<13} {}", key, desc)),
        )
        .collect::<Vec<_>>()
        .join("\n");

        // Dark block-character backdrop so the panel reads over a bright fractal
        let bg_width = 4 + text.lines().map(|l| l.len()).max().unwrap_or(0);
        let bg: String = vec!["\u{2588}".repeat(bg_width); panel_lines].join("\n");

        vec![
            TextEntry {
                text: bg,
                x: Self::HELP_X,
                y: panel_y,
                color: [10, 10, 20, 215],
                font_size,
            },
            TextEntry {
                text,
                x: Self::HELP_X + font_size, // ~2 block chars of left padding
                y: panel_y + line_height * 0.5,
                color: [235, 235, 245, 255],
                font_size,
            },
        ]
    }

    fn build_text_entries(&self, view_proj: Mat4, width: f32, height: f32) -> Vec<TextEntry> {
        let mut entries = Vec::new();

        if self.show_browser {
            entries.extend(self.build_browser_entries(height));
        } else if self.show_help {
            entries.extend(self.build_help_entries(height));
        } else {
            // Discoverability hint, bottom-left
            entries.push(TextEntry {
                text: "[H] keybinds".to_string(),
                x: 10.0,
                y: height - 24.0,
                color: [160, 160, 170, 180],
                font_size: 12.0,
            });
        }

        if !self.show_text {
            return entries;
        }

        let white = [255, 255, 255, 255];
        let grey = [180, 180, 180, 220];

        // === Top-left HUD ===
        let point_count = self.point_compute.valid_point_count();

        // Scene name + author
        entries.push(TextEntry {
            text: format!("{} — {}", self.scene.name, self.scene.author),
            x: 10.0,
            y: 10.0,
            color: grey,
            font_size: 12.0,
        });

        // Performance + point stats
        entries.push(TextEntry {
            text: format!(
                "{:.0} FPS | {:.1}ms | {}k / {}k points",
                self.fps_tracker.current_fps,
                self.fps_tracker.current_frametime_ms,
                point_count / 1000,
                self.buffer_capacity / 1000,
            ),
            x: 10.0,
            y: 26.0,
            color: white,
            font_size: 14.0,
        });

        // Camera params
        let mut param_y = 48.0;
        entries.push(TextEntry {
            text: format!(
                "cam: d={:.1} yaw={:.2} pitch={:.2} focus=({:.1},{:.1},{:.1})",
                self.camera.distance, self.camera.yaw, self.camera.pitch,
                self.camera.focus.x, self.camera.focus.y, self.camera.focus.z,
            ),
            x: 10.0,
            y: param_y,
            color: grey,
            font_size: 12.0,
        });
        param_y += 16.0;

        // Camera path status (only when a path exists)
        if let Some(path) = &self.scene.camera_path {
            let status = match self.path_play_t {
                Some(t) => format!("playing {:.0}%", t * 100.0),
                None => "Z plays".to_string(),
            };
            entries.push(TextEntry {
                text: format!(
                    "path: {} keys, {}, {:.1}s | {}",
                    path.keys.len(),
                    if path.closed { "loop" } else { "open" },
                    path.duration(),
                    status,
                ),
                x: 10.0,
                y: param_y,
                color: grey,
                font_size: 12.0,
            });
            param_y += 16.0;
        }

        // Point size + variation editing target
        let var_weight = self
            .selection()
            .map_or(0.0, |i| self.scene.transforms[i].variations[self.selected_variation]);
        let splat_info = match self.render_mode {
            RenderMode::Points => String::new(),
            RenderMode::Splat => format!(" | splat [R] exp={:.2}", self.exposure),
        };
        entries.push(TextEntry {
            text: format!(
                "pt size={:.4} | var slot [E]: {} {:+.2}{}",
                self.point_size,
                crate::scene::VARIATION_NAMES[self.selected_variation],
                var_weight,
                splat_info,
            ),
            x: 10.0,
            y: param_y,
            color: grey,
            font_size: 12.0,
        });
        param_y += 16.0;

        // Color params (only if not default)
        if self.color_falloff > 0.0 || self.color_contrast != 1.0 {
            entries.push(TextEntry {
                text: format!(
                    "color: falloff={:.2} contrast={:.2}",
                    self.color_falloff, self.color_contrast,
                ),
                x: 10.0,
                y: param_y,
                color: grey,
                font_size: 12.0,
            });
            param_y += 16.0;
        }

        // Fog params (only if not default)
        if self.fog_brightness < 1.0 || self.fog_saturation < 1.0 {
            entries.push(TextEntry {
                text: format!(
                    "fog: near={:.1} far={:.1} b={:.2} s={:.2}",
                    self.fog_near, self.fog_far, self.fog_brightness, self.fog_saturation,
                ),
                x: 10.0,
                y: param_y,
                color: grey,
                font_size: 12.0,
            });
        }

        // === Right-side transform list ===
        let right_x = width - 250.0;
        let mut y = 10.0;
        entries.push(TextEntry {
            text: "Transforms".to_string(),
            x: right_x,
            y,
            color: white,
            font_size: 14.0,
        });
        y += 20.0;

        for (i, spec) in self.scene.transforms.iter().enumerate() {
            let name = self.scene.transform_names
                .get(i)
                .and_then(|n| n.as_deref())
                .unwrap_or("");
            let label = if name.is_empty() {
                format!("T{}", i)
            } else {
                format!("T{}: {}", i, name)
            };

            let translation = spec.matrix.w_axis.truncate();
            let scale = spec.matrix.x_axis.length();

            let is_selected = self.selected_transform == Some(i);
            let is_enabled = self.transform_enabled.get(i).copied().unwrap_or(true);

            let sel = if is_selected { ">" } else { " " };
            let on = if is_enabled { " " } else { "×" };

            let summary = spec.variation_summary();
            let var_info = if summary == "linear" {
                String::new()
            } else {
                format!(" [{}]", summary)
            };
            let line = format!(
                "{}{} {} p=({:.2},{:.2},{:.2}) s={:.2} w={:.1}{}",
                sel, on, label, translation.x, translation.y, translation.z, scale, spec.weight, var_info,
            );

            // Use colormap color for this transform, dim if disabled
            let cm_idx = (spec.color_value * 255.0).clamp(0.0, 255.0) as usize;
            let cm = self.scene.colormap[cm_idx];
            let alpha: u8 = if is_enabled { 220 } else { 80 };
            let color = [
                (cm[0] * 255.0) as u8,
                (cm[1] * 255.0) as u8,
                (cm[2] * 255.0) as u8,
                alpha,
            ];

            entries.push(TextEntry {
                text: line,
                x: right_x,
                y,
                color,
                font_size: 12.0,
            });
            y += 16.0;
        }

        // === World-space gizmo labels ===
        for (i, spec) in self.scene.transforms.iter().enumerate() {
            let is_enabled = self.transform_enabled.get(i).copied().unwrap_or(true);
            let is_selected = self.selected_transform == Some(i);
            let origin = spec.matrix.w_axis.truncate();
            if let Some((sx, sy)) = world_to_screen(origin, view_proj, width, height) {
                let name = self.scene.transform_names
                    .get(i)
                    .and_then(|n| n.as_deref())
                    .unwrap_or("");
                let label = if name.is_empty() {
                    format!("T{}", i)
                } else {
                    name.to_string()
                };

                if is_selected {
                    // Draw background highlight for selected label
                    let bg_chars: String = "\u{2588}".repeat(label.len() + 2);
                    entries.push(TextEntry {
                        text: bg_chars,
                        x: sx + 4.0,
                        y: sy - 7.0,
                        color: [255, 255, 255, 200],
                        font_size: 13.0,
                    });
                    entries.push(TextEntry {
                        text: format!(" {} ", label),
                        x: sx + 4.0,
                        y: sy - 6.0,
                        color: [0, 0, 0, 255],
                        font_size: 13.0,
                    });
                } else {
                    let alpha: u8 = if is_enabled { 200 } else { 80 };
                    entries.push(TextEntry {
                        text: label,
                        x: sx + 8.0,
                        y: sy - 6.0,
                        color: [255, 255, 255, alpha],
                        font_size: 13.0,
                    });
                }
            }
        }

        entries
    }

    /// Render one frame, including the egui pass (`egui_renderer`/`paint_jobs`/
    /// `textures_delta`/`pixels_per_point` come from the caller's
    /// `ctx.run_ui` + `tessellate`, per the frame flow in main.rs).
    pub fn render(
        &mut self,
        egui_renderer: &mut egui_wgpu::Renderer,
        paint_jobs: &[egui::ClippedPrimitive],
        textures_delta: &egui::TexturesDelta,
        pixels_per_point: f32,
    ) -> FrameOutcome {
        if self.pending_screenshot {
            self.take_screenshot();
        }

        let (width, height) = self.gpu.size();
        let aspect = width as f32 / height as f32;
        let view_proj = self.camera.view_proj(aspect);

        // Advance circular buffer and get valid point count
        let point_count = self.point_compute.advance_frame(&self.gpu.queue, self.frame_dt);

        // wgpu 29: `get_current_texture` returns `CurrentSurfaceTexture`
        // directly (no more `Result<_, SurfaceError>`, and no `OutOfMemory`
        // variant — device loss is now reported via
        // `Device::set_device_lost_callback` instead).
        let surface_texture = match self.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return FrameOutcome::Skip;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                return FrameOutcome::Reconfigure;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::warn!("Surface validation error acquiring frame");
                return FrameOutcome::Skip;
            }
        };
        let color_view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = self.depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame_encoder"),
        });

        // === STEP 1: RUN CHAOS GAME ===
        self.point_compute.dispatch(&mut encoder);

        // === STEP 2: RENDER POINTS ===
        let camera = CameraUniforms::new(
            view_proj,
            height as f32,
            self.point_size,
            aspect,
            1.0,
            self.fog_near,
            self.fog_far,
            self.fog_brightness,
            self.fog_saturation,
            self.color_contrast,
        );
        self.gizmo_renderer.upload_camera(&self.gpu.queue, &camera);
        if self.show_traces {
            self.line_renderer.upload_camera(&self.gpu.queue, &camera);
        }

        match self.render_mode {
            RenderMode::Points => {
                self.point_renderer.upload_camera(&self.gpu.queue, &camera);
                let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("point_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });

                if point_count > 0 {
                    self.point_renderer.draw(
                        &mut render_pass,
                        point_count,
                        self.use_point_primitives(height as f32),
                    );
                }
            }
            RenderMode::Splat => {
                self.splat_renderer.upload_camera(&self.gpu.queue, &camera);
                self.splat_renderer.upload_params(
                    &self.gpu.queue,
                    self.exposure,
                    self.buffer_capacity,
                    height as f32,
                    CLEAR_COLOR,
                );
                self.splat_renderer.render(
                    &self.gpu.device,
                    &mut encoder,
                    &color_view,
                    &depth_view,
                    width,
                    height,
                    point_count,
                    self.use_point_primitives(height as f32),
                );
            }
        }

        // === STEP 2.5: RENDER TRACES ===
        if self.show_traces {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trace_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.line_renderer.draw(&mut render_pass);
        }

        // === STEP 3: RENDER GIZMOS ===
        if self.show_gizmos {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gizmo_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.gizmo_renderer.draw(&mut render_pass);
        }

        // === STEP 4: RENDER EGUI OVERLAY (replaces the old text-overlay pass) ===
        {
            for (id, image_delta) in &textures_delta.set {
                egui_renderer.update_texture(&self.gpu.device, &self.gpu.queue, *id, image_delta);
            }

            let screen_descriptor = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [width, height],
                pixels_per_point,
            };
            let user_cmd_bufs = egui_renderer.update_buffers(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                paint_jobs,
                &screen_descriptor,
            );

            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // egui-wgpu's `render` takes a `'static` pass so it can keep
            // referenced resources (textures/buffers) alive as long as
            // needed; the only consequence is that further use of `encoder`
            // after this point would be a runtime rather than compile-time
            // error, and we don't touch it again this frame.
            let mut render_pass = render_pass.forget_lifetime();
            egui_renderer.render(&mut render_pass, paint_jobs, &screen_descriptor);
            drop(render_pass);

            for id in &textures_delta.free {
                egui_renderer.free_texture(id);
            }

            self.gpu.queue.submit(user_cmd_bufs.into_iter().chain(std::iter::once(encoder.finish())));
        }

        surface_texture.present();

        FrameOutcome::Presented
    }
}
