use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{Mat4, Vec3};
use winit::window::Window;

use crate::camera::OrbitCamera;
use crate::gpu::lines::LineVertex;
use crate::gpu::{CameraUniforms, GizmoRenderer, GpuContext, LineRenderer, OverlayTargets, PointCompute, PointRenderer, SplatRenderer, DEPTH_FORMAT};
use crate::history::{EditSnapshot, History};
use crate::path::CameraPath;
use crate::scene::{Scene, TransformSpec};
use crate::ui::{hints, UiState};
use crate::view::View;

/// Seconds since the epoch, for unique output filenames
pub fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A running render job, as the dialog needs to see it: what was asked for,
/// where it has got to, and the two switches that stop it.
///
/// The job itself lives on another thread with its own wgpu device; this is
/// the near side of a channel, refreshed once per frame by `App::poll_job`.
pub struct JobHandle {
    pub params: crate::render_job::JobParams,
    events: std::sync::mpsc::Receiver<crate::render_job::JobEvent>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    pause: Arc<std::sync::atomic::AtomicBool>,
    pub started: Instant,
    pub phase: &'static str,
    pub done: u32,
    pub total: u32,
    pub log: Vec<String>,
    /// When the first click of the two-step cancel landed. A long render is
    /// expensive to lose to a misclick, so the button arms rather than fires,
    /// and disarms itself if it isn't confirmed.
    pub cancel_armed_at: Option<Instant>,
    /// When the current pause began, and how long previous pauses lasted.
    ///
    /// Kept so the time estimate can run on *working* time. Without it,
    /// wall-clock elapsed keeps climbing while progress is frozen and the
    /// projected remaining time climbs with it — a countdown that goes up,
    /// which is worse than no countdown at all.
    paused_at: Option<Instant>,
    paused_total: Duration,
}

/// How long an armed cancel stays armed before it goes back to being a button
/// that does nothing dangerous.
pub const CANCEL_ARM_WINDOW: Duration = Duration::from_secs(4);

impl JobHandle {
    pub fn paused(&self) -> bool {
        self.pause.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_paused(&mut self, paused: bool) {
        if paused {
            self.paused_at.get_or_insert_with(Instant::now);
        } else if let Some(at) = self.paused_at.take() {
            self.paused_total += at.elapsed();
        }
        self.pause.store(paused, std::sync::atomic::Ordering::Relaxed);
    }

    /// Time the job has actually spent working — wall clock minus every pause,
    /// including the one in progress.
    pub fn working_elapsed(&self) -> f32 {
        let paused_now = self.paused_at.map_or(Duration::ZERO, |at| at.elapsed());
        (self.started.elapsed().saturating_sub(self.paused_total + paused_now)).as_secs_f32()
    }

    pub fn cancelling(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether the cancel button is currently armed (first click landed,
    /// still inside the window).
    pub fn cancel_armed(&self) -> bool {
        self.cancel_armed_at
            .is_some_and(|t| t.elapsed() < CANCEL_ARM_WINDOW)
    }

    /// First click arms, second confirms. A cancelled job also un-pauses, or
    /// it would sit in the pause loop never noticing.
    pub fn click_cancel(&mut self) {
        if self.cancel_armed() {
            self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            self.set_paused(false);
        } else {
            self.cancel_armed_at = Some(Instant::now());
        }
    }

    /// Fraction complete within the current phase, when it reports one.
    pub fn fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| (self.done as f32 / self.total as f32).clamp(0.0, 1.0))
    }

    /// Seconds remaining, extrapolated from observed progress. `None` until
    /// there is enough progress for the extrapolation to mean anything —
    /// a countdown from one sample is a random number.
    pub fn remaining_secs(&self) -> Option<f32> {
        let f = self.fraction()?;
        if f < 0.05 {
            return None;
        }
        let working = self.working_elapsed();
        Some((working / f - working).max(0.0))
    }
}

/// Cheap change-detector for a camera path's drawn geometry: everything
/// `indicators::build_path` reads, folded into one number.
///
/// A generation counter bumped by each path-writing method would be faster
/// but has to be maintained at every one of them, and a missed bump is a
/// stale drawing nobody notices for weeks. Folding the values themselves
/// cannot go stale, and even a 40k-key path is a few thousand integer ops
/// against a GPU buffer allocation.
fn path_fingerprint(path: &CameraPath) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a offset basis
    let mut mix = |v: u32| {
        h ^= v as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    };
    mix(path.keys.len() as u32);
    mix(path.closed as u32);
    // The zoom loop changes where the closing segment goes, so the drawn
    // polyline has to be rebuilt when it changes or when its map is dragged
    mix(path.zoom_loop.map_or(0, |z| z.periods.wrapping_add(1)));
    mix(path.zoom_loop.map_or(0, |z| z.scale.to_bits()));
    mix(path.ease.map_or(2, |e| e as u32));
    mix(path.seconds.unwrap_or(f32::NAN).to_bits());
    for k in &path.keys {
        for v in [k.yaw, k.pitch, k.distance, k.roll, k.focus.x, k.focus.y, k.focus.z] {
            mix(v.to_bits());
        }
    }
    h
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
        // TEXTURE_BINDING as well as RENDER_ATTACHMENT: the overlay pass
        // samples this to seed its own multisampled depth, so the gizmos stay
        // occluded by the point cloud (see src/gpu/overlay.rs).
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// Ring buffer capacity: ~8.5s of history at 60 FPS (plan: `[f32; 512]`).
const FRAMETIME_RING_SIZE: usize = 512;
/// Sparkline shows the most recent slice of the ring.
const SPARKLINE_SAMPLES: usize = 120;

/// FPS tracking: a fixed ring of per-frame times feeds a continuously
/// updated avg/FPS and a p99 recomputed at the existing 1Hz display tick
/// (`select_nth_unstable_by` on a copy — cheap enough at 1Hz on 512 floats).
pub struct FpsTracker {
    last_log_time: Instant,
    last_frame_time: Instant,
    ring: [f32; FRAMETIME_RING_SIZE],
    /// Next slot to write (i.e. one past the most recently written sample)
    ring_pos: usize,
    /// How many slots hold valid data so far (saturates at ring size)
    ring_len: usize,
    pub current_fps: f32,
    pub current_frametime_ms: f32,
    pub p99_frametime_ms: f32,
    pub should_update_display: bool,
}

impl FpsTracker {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            last_log_time: now,
            last_frame_time: now,
            ring: [0.0; FRAMETIME_RING_SIZE],
            ring_pos: 0,
            ring_len: 0,
            current_fps: 0.0,
            current_frametime_ms: 0.0,
            p99_frametime_ms: 0.0,
            should_update_display: false,
        }
    }

    fn frame(&mut self) -> bool {
        let now = Instant::now();
        let frame_ms = now.duration_since(self.last_frame_time).as_secs_f32() * 1000.0;
        self.last_frame_time = now;
        self.should_update_display = false;

        self.ring[self.ring_pos] = frame_ms;
        self.ring_pos = (self.ring_pos + 1) % FRAMETIME_RING_SIZE;
        self.ring_len = (self.ring_len + 1).min(FRAMETIME_RING_SIZE);

        // While the ring hasn't wrapped, valid samples are exactly
        // `0..ring_len`; once full, every slot is valid regardless of
        // order, so `.take(ring_len)` covers both cases correctly (sums and
        // percentiles don't care about ordering).
        let valid = &self.ring[..self.ring_len];
        let sum: f32 = valid.iter().sum();
        self.current_frametime_ms = sum / self.ring_len as f32;
        self.current_fps = if self.current_frametime_ms > 0.0 {
            1000.0 / self.current_frametime_ms
        } else {
            0.0
        };

        let elapsed = now.duration_since(self.last_log_time);
        if elapsed >= Duration::from_secs(1) {
            self.should_update_display = true;
            self.last_log_time = now;
            self.recompute_p99();
        }

        self.should_update_display
    }

    fn recompute_p99(&mut self) {
        if self.ring_len == 0 {
            self.p99_frametime_ms = 0.0;
            return;
        }
        let mut copy: Vec<f32> = self.ring[..self.ring_len].to_vec();
        let idx = (((self.ring_len as f32) * 0.99) as usize).min(self.ring_len - 1);
        copy.select_nth_unstable_by(idx, |a, b| a.partial_cmp(b).unwrap());
        self.p99_frametime_ms = copy[idx];
    }

    /// Up to the last 120 frametime samples (milliseconds), oldest first —
    /// for the status-bar sparkline.
    fn sparkline_samples(&self) -> Vec<f32> {
        let n = self.ring_len.min(SPARKLINE_SAMPLES);
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            // ring_pos is the next write slot, so ring_pos - 1 is the most
            // recently written sample; walk backward from there.
            let idx = (self.ring_pos + FRAMETIME_RING_SIZE - 1 - i) % FRAMETIME_RING_SIZE;
            out.push(self.ring[idx]);
        }
        out.reverse();
        out
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

/// Actions dispatchable by clicking a row in the Keybinds window
/// (`src/ui/shortcuts.rs`).
#[derive(Clone, Copy, Debug)]
pub enum HelpAction {
    ToggleHelp,
    Reset,
    Zoom,
    ToggleSelected,
    ToggleGizmos,
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
    HazeIntensity,
    Mutate,
    Undo,
    Browse,
    Traces,
    InvertPitch,
    RenderMode,
    Exposure,
    /// Start / stop the camera flying its path (O, Z) — one action, because
    /// there's one motion.
    CameraMotion,
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
    /// Right-drag on empty space: roll the camera about its view axis
    Roll,
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
    pub camera: OrbitCamera,
    /// How many zoom periods the camera has travelled since the scene loaded,
    /// under infinite zoom. Purely for the person watching: the picture is the
    /// same at every level, so this is the only thing that says how deep in
    /// they are. Positive = inward.
    pub zoom_level: i32,
    /// Why infinite zoom is off, when the scene asked for it and it couldn't
    /// be built. Shown in the status bar rather than swallowed.
    pub zoom_error: Option<String>,
    /// Playhead along [`App::camera_path`] in 0..1 while the camera is flying
    /// it; `None` when the camera is being positioned by hand.
    ///
    /// One flag, because there's one motion. The turntable and the camera-path
    /// flythrough used to be separate mechanisms with a flag each (`orbit_
    /// paused` and `path_play_t`) that had to be kept from fighting; the
    /// turntable is now just the path a scene gets when it authors none, so
    /// "is the camera flying the path" is the only question left.
    path_t: Option<f32>,
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
    pub show_help: bool,

    // Haze. One control; see `src/haze.rs` for why the shader's four
    // parameters are not the user's four parameters.
    /// Depth-cue strength, 0 (off) to 1. Scene data — saved by Ctrl+S and
    /// undoable, like the other look parameters the scene file carries.
    pub haze_amount: f32,
    /// Write an alpha channel in captured stills (S) and render jobs, so
    /// they can be composited. Session state, not scene data: it says what
    /// leaves the app, not what the artwork is. The live window is always
    /// opaque — its swapchain has nothing to composite through.
    pub transparent_render: bool,
    /// Pinned haze band in world units, or `None` to auto-range off the camera
    /// distance. Only the Render window's advanced disclosure and legacy view
    /// files set this; auto is the default and the case worth optimising for.
    pub haze_band: Option<(f32, f32)>,

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
    /// Second line buffer for the selected transform's offset/rotation
    /// indicators (see src/indicators.rs). Separate from `line_renderer`
    /// because traces are rebuilt on demand and these follow the selection.
    indicator_renderer: LineRenderer,
    /// `(selected transform, matrix generation)` the indicator buffer was
    /// built for; rebuilt only when that changes.
    indicator_key: Option<(usize, u64)>,
    /// Third line buffer: the camera path's route through space
    /// (see `indicators::build_path`).
    path_renderer: LineRenderer,
    /// Fingerprint of the path the buffer was last built for, or `None` when
    /// nothing is drawn. See `refresh_path_lines`.
    path_lines_key: Option<u64>,
    /// Multisampled target the in-world UI is drawn into and composited from,
    /// so the gizmos and paths are anti-aliased and the point cloud isn't.
    overlay: OverlayTargets,

    /// Where the chaos game lands, measured on the CPU — the input to the
    /// drawn-point budget (see `drawn_points`). `None` until first measured,
    /// or when no walker can be built at all.
    attractor: Option<crate::trace::AttractorStats>,
    /// What `attractor` was measured for (see `attractor_fingerprint`), and
    /// when, so re-measuring is rate-limited during a drag.
    attractor_key: u64,
    attractor_measured_at: Instant,

    /// The running render job, if any (see `start_job`). One at a time.
    job: Option<JobHandle>,
    /// The last finished job's outcome, kept so the dialog can report it
    /// after the handle is gone.
    job_done: Option<Result<std::path::PathBuf, String>>,
    /// Why the last launch was refused (memory limit, bad size) — set instead
    /// of starting, so the dialog can say why rather than doing nothing.
    job_error: Option<String>,

    /// The path a scene gets when it authors none: a full orbit around the
    /// current framing. See [`App::camera_path`] — this isn't a second system
    /// beside the camera-path system, it's that system's default value, and
    /// it's kept out of `scene.camera_path` only so a scene doesn't grow a
    /// path it never asked for the first time it's saved.
    default_path: CameraPath,
    /// Set when a manual camera move made `default_path` stale;
    /// `refresh_default_path` rebuilds it at the top of the next frame, so
    /// `camera_path()` can stay a plain `&self` read.
    default_path_stale: bool,

    /// Plain-data egui UI state: which panels are open, the inspector's TRS
    /// field cache, this frame's status hint, and similar. The egui machinery
    /// itself (`EguiLayer`) lives on `AppWrapper` in main.rs instead, so the
    /// UI closure can hold `&mut App`.
    pub ui_state: UiState,
    /// Smoothed cost of building the egui frame, in ms (see record_ui_time)
    ui_build_ms: f32,
    /// Smoothed time blocked in `get_current_texture`, in ms — the frame's
    /// idle-waiting-for-vsync share (see `present_wait_ms`)
    present_wait_ms: f32,
    /// When prefs last changed without being written yet (window drags churn
    /// geometry every frame; see flush_dirty_prefs)
    prefs_dirty_since: Option<std::time::Instant>,
    /// Cached `views/` scan for the Camera panel (see saved_views)
    saved_views_cache: Option<Vec<(String, std::path::PathBuf)>>,
    /// Point capacity the Render slider is asking for but that hasn't been
    /// applied yet, and when the last reallocation happened (see
    /// request_point_capacity / apply_pending_capacity)
    pending_capacity: Option<u32>,
    last_capacity_change: Instant,

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
    pub scene_path: Option<String>,
    /// Point buffer capacity for HUD
    buffer_capacity: u32,

    /// Selected transform index (Some when text overlay visible)
    selected_transform: Option<usize>,
    /// Per-transform enabled state
    transform_enabled: Vec<bool>,
    /// Variation slot targeted by the variation-editing keys
    selected_variation: usize,
    /// Bumped by every matrix-writing path (gizmo drags, mutation, undo/
    /// redo/apply_snapshot, add/duplicate/delete, inspector edits). The
    /// Transforms window's inspector caches its decomposed TRS fields keyed
    /// by (transform_index, matrix_generation) so live typing/dragging in
    /// an inspector field isn't clobbered by a same-frame re-decompose.
    matrix_generation: u64,

    // Scene browser overlay (B)
    pub show_browser: bool,
    browser_files: Vec<std::path::PathBuf>,
    browser_selected: usize,

    /// Unified undo/redo history (see src/history.rs): every scene-mutating
    /// edit path commits through `commit_edit`. Cleared on scene load.
    pub history: History,
    /// Snapshot taken at gizmo-grab time, consumed (and committed) at
    /// release — a drag is a single history entry, not one per frame.
    gizmo_drag_before: Option<EditSnapshot>,

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
        haze_enabled: bool,
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

        // Everything drawn into the overlay pass shares its sample count.
        let overlay_samples = OverlayTargets::choose_samples(&gpu.surface_sample_counts);

        // Create gizmo renderer
        let gizmo_renderer = GizmoRenderer::new(
            &gpu.device,
            gpu.format,
            overlay_samples,
            &scene.transforms,
        );

        // Create depth texture
        let (width, height) = gpu.size();
        let depth_texture = create_depth_texture(&gpu.device, width, height, "main_depth");
        let overlay = OverlayTargets::new(
            &gpu.device,
            gpu.format,
            overlay_samples,
            width,
            height,
            &depth_texture.create_view(&Default::default()),
        );

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
        let line_renderer = LineRenderer::new(&gpu.device, gpu.format, overlay_samples);
        let indicator_renderer = LineRenderer::new(&gpu.device, gpu.format, overlay_samples);
        let path_renderer = LineRenderer::new(&gpu.device, gpu.format, overlay_samples);

        let num_transforms = scene.transforms.len();

        // `--fog` is a legacy on-switch: it predates haze being scene data, so
        // it means "turn it on at the old default strength" and the scene's
        // own value wins whenever it has one.
        let haze_amount = if scene.haze > 0.0 {
            scene.haze
        } else if haze_enabled {
            crate::haze::amount_from_brightness(0.4)
        } else {
            0.0
        };

        let prefs = crate::prefs::Prefs::load();
        let ui_state = UiState::from_prefs(&prefs);

        let camera = OrbitCamera {
            yaw: scene.camera_yaw,
            pitch: scene.camera_pitch,
            distance: scene.camera_distance,
            focus: scene.camera_focus,
            roll: scene.camera_roll,
        };

        let mut app = Self {
            gpu,
            window,
            frame_count: 0,
            last_update: Instant::now(),
            default_path: CameraPath::full_orbit(&camera),
            default_path_stale: false,
            // The camera starts moving, as it always has. When the scene
            // authored no path that's the default orbit, whose t=0 is exactly
            // the framing it was just built from — so nothing jumps.
            path_t: Some(0.0),
            camera,
            point_size,
            prefs,
            frame_dt: 1.0 / 60.0,
            cursor: (0.0, 0.0),
            drag: Drag::None,
            hovered: None,
            shift_held: false,
            ctrl_held: false,
            show_gizmos: true,
            // Env override lets automated captures verify the help overlay
            show_help: std::env::var("FRACTURIZE_SHOW_HELP").is_ok(),
            haze_amount,
            haze_band: None,
            transparent_render: false,
            color_falloff: scene.color_falloff,
            color_contrast: scene.color_contrast,
            fps_tracker: FpsTracker::new(),
            zoom_level: 0,
            zoom_error: None,
            point_compute,
            point_renderer,
            splat_renderer,
            gizmo_renderer,
            line_renderer,
            indicator_renderer,
            indicator_key: None,
            path_renderer,
            path_lines_key: None,
            overlay,
            attractor: None,
            // Zero is not a fingerprint any real scene produces, so the first
            // frame always measures.
            attractor_key: 0,
            attractor_measured_at: Instant::now(),
            job: None,
            job_done: None,
            job_error: None,
            ui_state,
            ui_build_ms: 0.0,
            present_wait_ms: 0.0,
            prefs_dirty_since: None,
            saved_views_cache: None,
            pending_capacity: None,
            last_capacity_change: Instant::now(),
            render_mode,
            exposure,
            show_traces: false,
            scene,
            scene_path,
            buffer_capacity,
            selected_transform: Some(0),
            transform_enabled: vec![true; num_transforms],
            selected_variation: 0,
            matrix_generation: 0,
            show_browser: false,
            browser_files: Vec::new(),
            browser_selected: 0,
            history: History::new(),
            gizmo_drag_before: None,
            depth_texture,
            screenshot_texture,
            screenshot_depth,
            screenshot_buffer,
            pending_screenshot: false,
        };
        app.refresh_zoom();

        // Apply a saved view, if given. Pause the orbit so the loaded
        // framing holds exactly (press O to resume).
        if let Some(v) = view {
            // Startup already honoured `--splat` / `--exposure` over the
            // view's own renderer fields, so don't let apply_view re-apply
            // them here.
            app.apply_view(&v, false);
        }

        app
    }

    /// Restore a saved view: camera framing, point size, haze, and color
    /// falloff/contrast. Shared by the `--view` startup path and the Camera
    /// window's saved-view list. Pauses the orbit so the loaded framing holds
    /// exactly.
    ///
    /// `with_renderer` also restores the view's renderer mode and exposure —
    /// wanted when loading a view interactively, but not at startup, where
    /// `--splat`/`--exposure` have already had their say.
    pub fn apply_view(&mut self, v: &View, with_renderer: bool) {
        // Fold any legacy eye offset into the on-sphere camera
        self.camera = OrbitCamera::from_legacy(
            Vec3::from(v.focus),
            Vec3::from(v.offset),
            v.distance,
            v.rotation,
            v.pitch,
            v.roll,
        );
        self.point_size = v.point_size;
        // Views written before haze became one control carry only the four raw
        // shader values; recover an equivalent amount from them and treat
        // their band as pinned, since it was chosen by hand.
        match v.haze {
            Some(amount) => {
                self.haze_amount = amount.clamp(0.0, 1.0);
                self.haze_band = v.haze_band_pinned.then_some((v.haze_near, v.haze_far));
            }
            None => {
                self.haze_amount = crate::haze::amount_from_brightness(v.haze_transmittance);
                self.haze_band = Some((v.haze_near, v.haze_far));
            }
        }
        if let Some(c) = v.color_contrast {
            self.color_contrast = c;
        }
        if let Some(f) = v.color_falloff {
            self.color_falloff = f;
            self.refresh_color_speeds();
        }
        if with_renderer {
            self.render_mode = if v.is_splat() {
                RenderMode::Splat
            } else {
                RenderMode::Points
            };
            if let Some(e) = v.exposure {
                self.exposure = e;
            }
        }
        self.path_t = None;
        self.invalidate_default_path();
        log::info!("Loaded view (camera stopped; press O to fly the path)");
    }

    // === The camera path (one system, with a default) ===

    /// The camera path. There is always one.
    ///
    /// A scene that authors `[[camera.path]]` keypoints flies those; one that
    /// doesn't flies a full orbit around its current framing. The default is
    /// not a separate turntable mechanism — it's this system's default value,
    /// synthesized rather than authored but identical in every other respect:
    /// it draws, it plays, it renders, and editing it is what turns it into
    /// scene data. `src/offline.rs` has always resolved `--render x.avif` this
    /// exact way; now the app agrees with it.
    ///
    /// "Authored" means two or more keys, since fewer can't be flown. So the
    /// first `Y` stores a key while you're still orbiting, and the second is
    /// where your own path takes over — and deleting your way back down to one
    /// key hands the default back rather than leaving the camera stranded.
    pub fn camera_path(&self) -> &CameraPath {
        crate::path::resolve(self.scene.camera_path.as_ref(), &self.default_path)
    }

    /// Whether [`camera_path`](Self::camera_path) is currently the synthesized
    /// default rather than the scene's own keys.
    pub fn path_is_default(&self) -> bool {
        std::ptr::eq(self.camera_path(), &self.default_path)
    }

    /// Give the path properties (loop, duration) somewhere to be written by
    /// turning the default into real scene data. Editing the default is what
    /// authors it — there's no separate "adopt the orbit" step, because there's
    /// no separate system to adopt it from.
    ///
    /// An existing `Some` is left alone even when it's too short to fly, so a
    /// half-built path never loses the key it has.
    fn author_path(&mut self) -> &mut CameraPath {
        if self.scene.camera_path.is_none() {
            self.refresh_default_path();
            self.scene.camera_path = Some(self.default_path.clone());
            log::info!("Camera path: the default orbit is now the scene's own, and editable");
        }
        self.scene.camera_path.as_mut().expect("just authored")
    }

    /// Mark the default orbit stale after a manual camera move, so it orbits
    /// what you're actually looking at rather than snapping back.
    fn invalidate_default_path(&mut self) {
        self.default_path_stale = true;
    }

    /// Rebuild the default orbit around the current framing if a manual camera
    /// move made it stale. Called at the top of `update`, which is what lets
    /// `camera_path()` be a plain `&self` read.
    fn refresh_default_path(&mut self) {
        if !self.default_path_stale {
            return;
        }
        self.default_path_stale = false;
        let was_default = self.path_is_default();
        self.default_path = CameraPath::full_orbit(&self.camera);
        // t=0 on the new path *is* the framing it was just built from, so
        // continuing from the old playhead would jump; continuing from 0 is
        // seamless. Only when the default is what's flying, of course — an
        // authored path must not restart because someone scrolled.
        if was_default && self.path_t.is_some() {
            self.path_t = Some(0.0);
        }
    }

    /// Set the camera's view-axis roll (the Camera window's field and its
    /// "level" button; right-drag goes through `OrbitCamera::roll_by`).
    pub fn set_camera_roll(&mut self, roll: f32) {
        self.camera.roll = roll;
        self.invalidate_default_path();
    }

    /// The camera path to draw, if it should be drawn at all.
    ///
    /// Nothing while it's playing. During playback the camera is standing *on*
    /// the line, so drawing it puts a permanent smear across the view that
    /// tells you nothing and spoils the shot. It appears the moment you take
    /// the camera back by hand — which is exactly when the route matters,
    /// because that's when you're positioning against it.
    pub fn visible_path(&self) -> Option<&CameraPath> {
        if self.path_t.is_some() {
            return None;
        }
        let path = self.camera_path();
        path.playable().then_some(path)
    }

    /// Is the camera flying the path right now?
    pub fn camera_moving(&self) -> bool {
        self.path_t.is_some()
    }

    /// Take the camera off the path, because the framing is being set by hand.
    /// The viewport drags do this inline; the Camera window's numeric fields
    /// call it, since typing a distance is the same intent as dragging one.
    pub fn stop_camera_motion(&mut self) {
        self.path_t = None;
    }

    /// Start / stop the camera flying the path (O, Z, and the toolbar's
    /// transport button). The one motion control there is.
    pub fn toggle_camera_motion(&mut self) {
        if self.path_t.is_some() {
            self.path_t = None;
            log::info!("Camera: stopped");
            return;
        }
        let path = self.camera_path();
        // `playable` is the rule, not a keypoint count: a zoom loop flies on a
        // single key, because its closing segment runs to that key's own image
        // under the symmetry. Counting to two here refused to *start* one —
        // though a loop already in flight kept going as you deleted down to
        // one key, which is what made it look like the spline was at fault.
        if !path.playable() {
            log::warn!("Camera path has too few keypoints to fly — press Y to add some");
            return;
        }
        log::info!(
            "Camera: flying {} keys over {:.1}s{}",
            path.keys.len(),
            path.duration(),
            if self.path_is_default() { " (default orbit)" } else { "" },
        );
        self.path_t = Some(0.0);
    }

    /// Append the current camera framing as a keypoint of this scene's own path
    /// (Y). Not an edit to the default orbit — the default is what you get
    /// *until* the scene has two keypoints, and this is how it gets them.
    pub fn add_path_key(&mut self) {
        let was_default = self.path_is_default();
        let key = crate::path::PathKey::from_camera(&self.camera);
        let path = self.scene.camera_path.get_or_insert_with(|| crate::path::CameraPath {
            keys: Vec::new(),
            closed: false,
            zoom_loop: None,
            ease: None,
            seconds: None,
        });
        path.keys.push(key);
        log::info!(
            "Camera path keypoint {} added ({}; Ctrl+S saves it with the scene)",
            path.keys.len(),
            if path.keys.len() >= 2 {
                "Z flies it"
            } else {
                "one more and it takes over from the default orbit"
            },
        );
        self.after_path_edit(was_default);
    }

    /// Remove the last path keypoint (Shift+Y)
    pub fn remove_path_key(&mut self) {
        let was_default = self.path_is_default();
        let Some(path) = &mut self.scene.camera_path else {
            log::warn!("Nothing to remove — this scene is on the default orbit");
            return;
        };
        path.keys.pop();
        self.after_path_edit(was_default);
    }

    /// Toggle whether the path loops back to its first key (Ctrl+Y).
    ///
    /// A path that closes under the zoom symmetry is already a loop, and a
    /// different one: returning to the first key would undo the descent that
    /// makes it endless. Say so rather than silently doing the other thing,
    /// and leave the scene's choice alone.
    pub fn toggle_path_closed(&mut self) {
        let path = self.author_path();
        if let Some(z) = path.zoom_loop {
            log::info!(
                "Camera path already loops under the zoom symmetry ({} period(s) per \
                 loop). Turn that off first if you want a key-to-key loop instead.",
                z.periods
            );
            return;
        }
        path.closed = !path.closed;
        log::info!("Camera path: {}", if path.closed { "closed loop" } else { "open" });
    }

    /// The path's zoom loop, if it has one
    pub fn path_zoom_loop(&self) -> Option<crate::path::ZoomLoop> {
        self.camera_path().zoom_loop
    }

    /// Turn the path's zoom loop on for `periods` periods per loop, or off.
    ///
    /// Closing under the scale symmetry and closing back to the first key are
    /// two different loops, so this clears the other one rather than leaving
    /// the path claiming both.
    pub fn set_path_zoom_loop(&mut self, periods: Option<u32>) {
        let was_default = self.path_is_default();
        let Some(loop_) = periods.map(|n| n.clamp(1, 64)) else {
            self.author_path().zoom_loop = None;
            log::info!("Camera path: zoom loop off");
            self.after_path_edit(was_default);
            return;
        };
        // The similarity comes from the live renormalizing map, so a scene
        // without one can't have this — and shouldn't silently get a path
        // that claims to loop and doesn't.
        let Some(zoom) = self.point_compute.zoom else {
            log::warn!(
                "A zoom loop closes under the scene's scale symmetry, and this scene \
                 has none. Select a transform in the Transforms window and press \
                 \"Zoom about this\" first."
            );
            return;
        };
        let path = self.author_path();
        path.zoom_loop = Some(zoom.loop_similarity(loop_));
        path.closed = false;
        log::info!("Camera path: zoom loop, {} period(s) per loop", loop_);
        self.after_path_edit(was_default);
    }

    /// Remove one path keypoint by index (Camera window row ✕). The keyboard
    /// path (Shift+Y) only ever pops the last one.
    pub fn remove_path_key_at(&mut self, idx: usize) {
        let was_default = self.path_is_default();
        let Some(path) = &mut self.scene.camera_path else { return };
        if idx >= path.keys.len() {
            return;
        }
        path.keys.remove(idx);
        log::info!("Camera path keypoint {} removed", idx);
        self.after_path_edit(was_default);
    }

    /// Housekeeping after the scene's own keypoints change. `was_default` is
    /// [`path_is_default`](Self::path_is_default) from *before* the edit.
    ///
    /// An empty key list isn't a path, so it goes back to `None`: nothing
    /// writes an empty `[[camera.path]]`, and the default orbit is what the
    /// scene flies again. And when the edit changed *which* path flies — the
    /// second key taking over, or a deletion handing back — the playhead means
    /// nothing on the new route, so the camera stops instead of teleporting
    /// partway along a path it was never on.
    fn after_path_edit(&mut self, was_default: bool) {
        if self.scene.camera_path.as_ref().is_some_and(|p| p.keys.is_empty()) {
            self.scene.camera_path = None;
            log::info!("Camera path cleared — back to the default orbit");
        }
        if self.path_is_default() != was_default {
            self.path_t = None;
        }
    }

    /// Set the path's playback duration in seconds; `None` restores the
    /// default of 3s per segment.
    pub fn set_path_seconds(&mut self, seconds: Option<f32>) {
        self.author_path().seconds = seconds.map(|s| s.max(0.1));
    }

    /// Discard the scene's own keypoints and go back to the default orbit —
    /// the counterpart to the first edit that authored them.
    pub fn reset_path_to_default(&mut self) {
        if self.scene.camera_path.is_none() {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.camera_path = None;
        self.path_t = None;
        self.invalidate_default_path();
        self.commit_edit("Reset camera path", None, before);
        log::info!("Camera path reset to the default orbit");
    }

    /// Saved views for the current scene: `views/<slug>-*.toml`, newest
    /// first. Returns (display name, path). Missing directory = empty list.
    pub fn saved_views(&mut self) -> &[(String, std::path::PathBuf)] {
        // Cached: the Camera panel asks for this every frame, and a
        // directory scan per frame is a syscall storm for a list that only
        // changes when we save a view or load a different scene.
        if self.saved_views_cache.is_none() {
            self.saved_views_cache = Some(self.scan_saved_views());
        }
        self.saved_views_cache.as_deref().unwrap_or(&[])
    }

    /// Drop the `saved_views` cache — call after anything that changes the
    /// contents of `views/` or the scene slug we filter by.
    pub fn invalidate_saved_views(&mut self) {
        self.saved_views_cache = None;
    }

    fn scan_saved_views(&self) -> Vec<(String, std::path::PathBuf)> {
        let prefix = format!("{}-", self.scene_slug());
        let Ok(entries) = std::fs::read_dir("views") else {
            return Vec::new();
        };
        let mut out: Vec<(String, std::path::PathBuf)> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .filter_map(|p| {
                let stem = p.file_stem()?.to_str()?.to_string();
                stem.starts_with(&prefix).then_some((stem, p))
            })
            .collect();
        // Names end in a unix timestamp, so a reverse lexical sort is
        // newest-first for as long as timestamps keep their digit count.
        out.sort_by(|a, b| b.0.cmp(&a.0));
        out
    }

    /// Load one of `saved_views()` back onto the current scene.
    pub fn load_saved_view(&mut self, path: &std::path::Path) {
        match View::load(path) {
            Ok(v) => self.apply_view(&v, true),
            Err(e) => log::error!("{}", e),
        }
    }

    /// Filesystem-safe slug of the scene name
    pub fn scene_slug(&self) -> String {
        let slug: String = self
            .scene
            .name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        slug.trim_matches('-').to_string()
    }

    /// The view currently on screen, for saving (V) and for render jobs
    fn current_view(&self) -> View {
        View {
            scene: self.scene_path.clone(),
            rotation: self.camera.yaw,
            pitch: self.camera.pitch,
            roll: self.camera.roll,
            distance: self.camera.distance,
            focus: self.camera.focus.to_array(),
            offset: [0.0; 3],
            point_size: self.point_size,
            // The band is resolved rather than stored raw, so a view of an
            // auto-ranged scene renders offline at the framing it was saved
            // at even though the offline path knows nothing about auto-ranging.
            haze_near: self.haze_range().0,
            haze_far: self.haze_range().1,
            haze_transmittance: self.haze_falloff().0,
            haze_saturation: self.haze_falloff().1,
            haze: Some(self.haze_amount),
            haze_band_pinned: self.haze_band.is_some(),
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
    pub fn save_view(&mut self) {
        let view = self.current_view();

        let path = format!("views/{}-{}.toml", self.scene_slug(), unix_timestamp());

        match view.save(&path) {
            Ok(()) => log::info!("View saved to {}", path),
            Err(e) => log::error!("{}", e),
        }
        self.invalidate_saved_views();
    }

    /// Write the scene (with any interactive edits) back to its TOML file.
    /// Camera framing and render params adjusted in-app become scene defaults.
    /// Fold the live values that live on `App` rather than on `Scene` back
    /// into the scene, so what gets written is what's on screen.
    fn sync_scene_for_save(&mut self) {
        self.scene.camera_focus = self.camera.focus;
        self.scene.camera_distance = self.camera.distance;
        self.scene.camera_yaw = self.camera.yaw;
        self.scene.camera_pitch = self.camera.pitch;
        self.scene.camera_roll = self.camera.roll;
        self.scene.point_size = self.point_size;
        self.scene.color_falloff = self.color_falloff;
        self.scene.color_contrast = self.color_contrast;
        self.scene.haze = self.haze_amount;
    }

    pub fn save_scene(&mut self) {
        self.sync_scene_for_save();
        let path = self
            .scene_path
            .clone()
            .unwrap_or_else(|| format!("scenes/untitled-{}.toml", unix_timestamp()));
        self.write_scene_to(&path);
    }

    /// Save under a new name and continue working on *that* file — the fork
    /// point for "I like this, but I want to keep the original too".
    ///
    /// `name` renames the scene itself. A fork that kept the original's name
    /// would leave the toolbar, the render filenames and the `views/` lookups
    /// all still saying "Koru" while you work on `koru-v2.toml` — two
    /// identities for one thing, and the wrong one visible.
    ///
    /// Deliberately refuses to overwrite: the dialog checks first and says so,
    /// and this is the backstop for the case where something appeared in
    /// between. Clobbering another scene is not recoverable from in-app —
    /// history only covers the scene you have open.
    pub fn save_scene_as(&mut self, path: &str, name: &str, overwrite: bool) -> Result<(), String> {
        if !overwrite && Path::new(path).exists() {
            return Err(format!("{} already exists", path));
        }
        let name = name.trim();
        if !name.is_empty() {
            self.scene.name = name.to_string();
        }
        self.sync_scene_for_save();
        self.scene.save(path).map_err(|e| e.to_string())?;
        log::info!("Scene saved as {} ({})", path, self.scene.name);
        self.scene_path = Some(path.to_string());
        // The slug feeds views/ lookups and render filenames, and it comes
        // from the scene name, not the path — but the *saved views* cache is
        // keyed on it, so a rename has to drop it.
        self.invalidate_saved_views();
        Ok(())
    }

    /// The path a "save as" should suggest: the current file with a `-copy`
    /// suffix, or a name derived from the scene for an unsaved one.
    pub fn suggested_fork_path(&self) -> String {
        match &self.scene_path {
            Some(p) => {
                let path = Path::new(p);
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("scene");
                let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
                let name = format!("{}-copy.toml", stem);
                match dir {
                    Some(d) => d.join(name).to_string_lossy().into_owned(),
                    None => name,
                }
            }
            None => format!("scenes/{}.toml", self.scene_slug()),
        }
    }

    /// Set the scene's display name — the toolbar's label, the `views/` and
    /// render-filename slug, and what Ctrl+S writes.
    pub fn set_scene_name(&mut self, name: &str) {
        if self.scene.name == name {
            return;
        }
        self.scene.name = name.to_string();
        // The slug is derived from the name, and the views cache is keyed on
        // the slug.
        self.invalidate_saved_views();
    }

    /// Set the scene's author, and remember it as the default for scenes made
    /// in-app from here on — nobody wants to retype their own name.
    pub fn set_scene_author(&mut self, author: &str) {
        if self.scene.author == author {
            return;
        }
        self.scene.author = author.to_string();
        let trimmed = author.trim();
        self.prefs.author = (!trimmed.is_empty()).then(|| trimmed.to_string());
        self.prefs_dirty_since = Some(std::time::Instant::now());
    }

    /// The remembered author, for a scene that hasn't got one.
    fn default_author(&self) -> String {
        self.prefs.author.clone().unwrap_or_default()
    }

    fn write_scene_to(&mut self, path: &str) {
        match self.scene.save(path) {
            Ok(()) => {
                log::info!("Scene saved to {}", path);
                self.scene_path = Some(path.to_string());
            }
            Err(e) => log::error!("{}", e),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        if width > 0 && height > 0 {
            self.depth_texture = create_depth_texture(&self.gpu.device, width, height, "main_depth");
            // The overlay's depth blit reads the texture we just replaced, so
            // its bind group has to be rebuilt with it.
            self.overlay.resize(
                &self.gpu.device,
                width,
                height,
                &self.depth_texture.create_view(&Default::default()),
            );
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

    // === Unified undo/redo history (src/history.rs) ===

    /// Snapshot of every piece of state history tracks. Callers take one of
    /// these *before* mutating, then hand it to `commit_edit` after.
    pub fn edit_snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            scene: self.scene.clone(),
            transform_enabled: self.transform_enabled.clone(),
            point_size: self.point_size,
            color_falloff: self.color_falloff,
            color_contrast: self.color_contrast,
            haze_amount: self.haze_amount,
        }
    }

    /// The single choke point for history-tracked edits. `coalesce_key`
    /// lets rapid same-key commits (held weight/color keys, drag-scroll
    /// bursts) merge into one entry instead of flooding the stack.
    pub fn commit_edit(&mut self, label: impl Into<String>, coalesce_key: Option<&str>, before: EditSnapshot) {
        self.history.commit(label, coalesce_key, before, Instant::now());
    }

    /// Restore a snapshot and rebuild the GPU pipelines — the same path
    /// add/delete transform uses, since undo/redo can change transform count.
    fn apply_snapshot(&mut self, snap: EditSnapshot) {
        self.scene = snap.scene;
        self.transform_enabled = snap.transform_enabled;
        self.point_size = snap.point_size;
        self.color_falloff = snap.color_falloff;
        self.color_contrast = snap.color_contrast;
        self.haze_amount = snap.haze_amount;
        self.bump_matrix_generation();
        self.after_scene_shape_change();
    }

    /// Bump the matrix-generation counter — called from every path that can
    /// write a transform's matrix (gizmo drags, mutation, undo/redo, add/
    /// duplicate/delete, inspector edits) so the Transforms window's TRS
    /// field cache (`UiState`) knows to re-decompose.
    fn bump_matrix_generation(&mut self) {
        self.matrix_generation = self.matrix_generation.wrapping_add(1);
    }

    /// Current matrix generation, for the Transforms window's TRS field
    /// cache key (see `bump_matrix_generation`).
    pub fn matrix_generation(&self) -> u64 {
        self.matrix_generation
    }

    /// Selected transform index — two-way synced with the Transforms
    /// window's list (row click) and gizmo-click selection / Up-Down keys.
    pub fn selected_transform(&self) -> Option<usize> {
        self.selected_transform
    }

    /// Select a transform from the Transforms window's list (or clear the
    /// selection). Gizmo clicks and Up/Down go through `selected_transform`
    /// directly; this is the same field, so both stay in sync automatically.
    pub fn select_transform(&mut self, idx: Option<usize>) {
        self.selected_transform = idx;
    }

    /// Whether a transform is enabled, for the Transforms window's list and
    /// inspector (defaults to enabled for an out-of-range index, matching
    /// the HUD's historical behavior).
    pub fn is_transform_enabled(&self, idx: usize) -> bool {
        self.transform_enabled.get(idx).copied().unwrap_or(true)
    }

    /// Variation slot targeted by the -/= keys and E/Shift+E cycling — also
    /// settable from the Transforms window's variation editor so clicking a
    /// row there keeps keyboard cycling in sync (per the plan).
    pub fn selected_variation(&self) -> usize {
        self.selected_variation
    }

    pub fn set_selected_variation(&mut self, slot: usize) {
        self.selected_variation = slot.min(crate::scene::NUM_VARIATIONS - 1);
    }

    /// Undo the most recent history entry (Ctrl+Z, Shift+U, the Explore
    /// window's Undo button).
    pub fn undo(&mut self) {
        let current = self.edit_snapshot();
        match self.history.undo(current) {
            Some((label, restore)) => {
                self.apply_snapshot(restore);
                log::info!("Undo: {}", label);
            }
            None => log::warn!("Nothing to undo"),
        }
    }

    /// Redo the most recently undone entry (Ctrl+Shift+Z, the Explore
    /// window's Redo button).
    pub fn redo(&mut self) {
        let current = self.edit_snapshot();
        match self.history.redo(current) {
            Some((label, restore)) => {
                self.apply_snapshot(restore);
                log::info!("Redo: {}", label);
            }
            None => log::warn!("Nothing to redo"),
        }
    }

    /// Undo `steps` entries in one go (clicking N deep into the Explore
    /// window's history list) as a single pipeline rebuild.
    pub fn jump_undo(&mut self, steps: usize) {
        if steps == 0 {
            return;
        }
        let current = self.edit_snapshot();
        match self.history.jump_undo(steps, current) {
            Some(restore) => {
                self.apply_snapshot(restore);
                log::info!("Undid {} step(s)", steps);
            }
            None => log::warn!("Not enough undo history for {} step(s)", steps),
        }
    }

    /// Symmetric opposite of `jump_undo`.
    pub fn jump_redo(&mut self, steps: usize) {
        if steps == 0 {
            return;
        }
        let current = self.edit_snapshot();
        match self.history.jump_redo(steps, current) {
            Some(restore) => {
                self.apply_snapshot(restore);
                log::info!("Redid {} step(s)", steps);
            }
            None => log::warn!("Not enough redo history for {} step(s)", steps),
        }
    }

    /// Persisted geometry for a panel window, if it has ever been moved.
    pub fn window_geometry(&self, key: &str) -> Option<[f32; 4]> {
        self.prefs.window_geometry.get(key).copied()
    }

    /// Remember where a panel window is now. Called every frame for every
    /// open panel, so it only marks prefs dirty on a real change and defers
    /// the disk write to `flush_dirty_prefs` — otherwise dragging a window
    /// would rewrite prefs.toml once per frame.
    pub fn set_window_geometry(&mut self, key: &str, rect: [f32; 4]) {
        let changed = match self.prefs.window_geometry.get(key) {
            Some(old) => old
                .iter()
                .zip(rect.iter())
                .any(|(a, b)| (a - b).abs() > 0.5),
            None => true,
        };
        if changed {
            self.prefs.window_geometry.insert(key.to_string(), rect);
            self.prefs_dirty_since = Some(std::time::Instant::now());
        }
    }

    /// Write deferred prefs changes once they've been quiet for a moment.
    /// Called from `update()`.
    fn flush_dirty_prefs(&mut self) {
        const QUIET: std::time::Duration = std::time::Duration::from_millis(800);
        if self.prefs_dirty_since.is_some_and(|t| t.elapsed() >= QUIET) {
            self.prefs_dirty_since = None;
            self.prefs.save();
        }
    }

    /// Record how long building this frame's UI took (egui `run_ui` +
    /// `tessellate`), smoothed. Kept separate from the frametime tracker so
    /// the status bar can say whether an FPS drop is the panels' fault or the
    /// chaos game's.
    pub fn record_ui_time(&mut self, ms: f32) {
        self.ui_build_ms = self.ui_build_ms * 0.9 + ms * 0.1;
    }

    /// Smoothed UI build time in milliseconds.
    pub fn ui_ms(&self) -> f32 {
        self.ui_build_ms
    }

    /// Record how long `get_current_texture` blocked, smoothed the same way
    /// as `record_ui_time`.
    pub fn record_present_wait(&mut self, ms: f32) {
        self.present_wait_ms = self.present_wait_ms * 0.9 + ms * 0.1;
    }

    /// Smoothed time per frame spent blocked acquiring the swapchain image.
    ///
    /// This is the number that settles the recurring "are the panels costing
    /// me frames?" question. With vsync on, a healthy frame spends most of its
    /// budget parked *here*, doing nothing: high wait means the display is
    /// pacing us and there is headroom to spare. A frame time that has gone up
    /// while this has gone *down* means the work genuinely got slower. A frame
    /// time that has gone up while this stays high means something outside our
    /// control (compositor, driver, another GPU client) is holding the
    /// swapchain, and no amount of UI optimisation will help.
    pub fn present_wait_ms(&self) -> f32 {
        self.present_wait_ms
    }

    /// (fps, avg frametime ms, p99 frametime ms) for the status bar.
    pub fn fps_stats(&self) -> (f32, f32, f32) {
        (
            self.fps_tracker.current_fps,
            self.fps_tracker.current_frametime_ms,
            self.fps_tracker.p99_frametime_ms,
        )
    }

    /// Up to the last 120 frametime samples (ms), oldest first, for the
    /// status-bar sparkline.
    pub fn frametime_sparkline(&self) -> Vec<f32> {
        self.fps_tracker.sparkline_samples()
    }

    /// (valid points, buffer capacity, still warming up) for the status bar.
    pub fn point_stats(&self) -> (u32, u32, bool) {
        let valid = self.point_compute.valid_point_count();
        let warming = self.point_compute.current_frame < self.point_compute.warmup_frames;
        (valid, self.buffer_capacity, warming)
    }

    /// How many points the drawn-point budget is holding back, if any — the
    /// status bar says so rather than leaving a suddenly-fast frame rate and a
    /// point count that disagree (see `drawn_points`).
    pub fn withheld_points(&self, screen_height: f32) -> Option<(u32, u32)> {
        let valid = self.point_compute.valid_point_count();
        let drawn = self.drawn_points(screen_height);
        (drawn < valid).then_some((drawn, valid))
    }

    /// Hint for the gizmo part currently hovered (`None` when nothing's
    /// hovered, e.g. gizmos are off or hover was suppressed because the
    /// pointer is over egui) — status bar tier 2, per the plan.
    pub fn hovered_hint(&self) -> Option<&'static str> {
        use crate::pick::GizmoPart;
        self.hovered.map(|hit| match hit.part {
            GizmoPart::Origin => hints::HINT_ORIGIN,
            GizmoPart::Axis(_) => hints::HINT_AXIS,
            GizmoPart::RotEdge(_) => hints::HINT_ROT_EDGE,
        })
    }

    /// Strength multiplier for U's random mutation (Explore window slider;
    /// persisted in prefs).
    pub fn mutate_strength(&self) -> f32 {
        self.prefs.mutate_strength
    }

    pub fn set_mutate_strength(&mut self, strength: f32) {
        self.prefs.mutate_strength = strength.clamp(0.1, 3.0);
    }

    /// Write prefs to disk now (callers debounce — e.g. on slider
    /// drag-stop — so a drag doesn't hammer the file every frame).
    pub fn save_prefs(&mut self) {
        self.prefs.save();
    }

    /// Persist panel open/closed state the moment it differs from what's on
    /// disk — same pattern as `toggle_invert_pitch`. Called every frame from
    /// `ui::draw`; cheap no-op when nothing changed.
    pub fn panel_prefs_changed(&mut self, panels: crate::prefs::PanelPrefs) {
        if self.prefs.panels != panels {
            self.prefs.panels = panels;
            self.prefs.save();
        }
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
            let before = self.edit_snapshot();
            let w = &mut self.scene.transforms[hit.transform].weight;
            *w = (*w * 1.08f32.powf(steps)).clamp(0.01, 100.0);
            log::info!("T{} weight: {:.2} (scroll)", hit.transform, *w);
            self.sync_transforms_to_gpu();
            self.commit_edit(
                format!("Weight T{}", hit.transform),
                Some(&format!("weight:T{}", hit.transform)),
                before,
            );
            return;
        }
        self.camera.zoom(steps);
        self.invalidate_default_path();
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
                self.invalidate_default_path();
            }
            Drag::Pan => {
                let (_, h) = self.gpu.size();
                self.camera.pan(dx, dy, h as f32);
                self.invalidate_default_path();
            }
            Drag::Roll => {
                self.camera.roll_by(dx);
                self.invalidate_default_path();
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
    pub fn current_view_proj(&self) -> Mat4 {
        let (w, h) = self.gpu.size();
        self.camera.view_proj(w as f32 / h as f32)
    }

    /// Surface size in physical pixels — `ui::labels` needs it to project
    /// transform origins to screen space.
    pub fn surface_size(&self) -> (u32, u32) {
        self.gpu.size()
    }

    pub fn on_mouse_press(&mut self, button: winit::event::MouseButton) {
        use winit::event::MouseButton;
        match button {
            MouseButton::Left => {
                // The keybinds and scene-browser overlays used to hand-roll
                // click hit-testing here. Both are real egui windows now
                // (src/ui/shortcuts.rs, src/ui/browser.rs), so egui's own
                // pointer gating in main.rs keeps their clicks away from us.
                if let Some(drag) = self.try_grab_gizmo() {
                    // A gizmo grab always yields Drag::Gizmo; snapshot now so
                    // the whole drag commits as one history entry at release.
                    self.gizmo_drag_before = Some(self.edit_snapshot());
                    self.drag = drag;
                    self.window.set_cursor(winit::window::CursorIcon::Grabbing);
                    return;
                }
                self.drag = if self.shift_held { Drag::Pan } else { Drag::Orbit };
                // Taking the camera by hand stops it flying the path — they'd
                // fight over the same framing otherwise.
                self.path_t = None;
            }
            MouseButton::Middle => {
                self.drag = Drag::Pan;
                self.path_t = None;
            }
            MouseButton::Right => {
                // Over a gizmo, right-click is that transform's context menu —
                // the same menu its row in the Transforms window has, opened on
                // the thing itself. Rolling the whole camera off a click aimed
                // at one transform would be a surprise, so rolling only starts
                // on empty space.
                if let Some(hit) = self.hovered {
                    self.selected_transform = Some(hit.transform);
                    // Position is filled in by `ui::draw` from egui's own
                    // pointer, which is already in logical points.
                    self.ui_state.transform_menu = Some((hit.transform, None));
                    return;
                }
                self.drag = Drag::Roll;
                self.path_t = None;
            }
            _ => {}
        }
    }

    pub fn on_mouse_release(&mut self, button: winit::event::MouseButton) {
        use winit::event::MouseButton;
        if matches!(button, MouseButton::Left | MouseButton::Middle | MouseButton::Right) {
            if let Drag::Gizmo { transform, mode, start_matrix } = self.drag {
                let spec = &self.scene.transforms[transform];
                let t = spec.matrix.w_axis.truncate();
                log::info!(
                    "T{}: p=({:.3},{:.3},{:.3}) s={:.3}",
                    transform, t.x, t.y, t.z, spec.matrix.x_axis.truncate().length(),
                );
                let matrix_changed = spec.matrix != start_matrix;
                if matrix_changed {
                    if let Some(before) = self.gizmo_drag_before.take() {
                        let label = self.gizmo_drag_label(transform, mode);
                        self.commit_edit(label, None, before);
                    }
                } else {
                    // No net motion (a click that didn't drag): no history entry.
                    self.gizmo_drag_before = None;
                }
            }
            self.drag = Drag::None;
            self.update_hover();
        }
    }

    /// History label for a completed gizmo drag, e.g. "Move whorl" /
    /// "Rotate T2" / "Scale core".
    fn gizmo_drag_label(&self, transform: usize, mode: GizmoDragMode) -> String {
        let name = self
            .scene
            .transform_names
            .get(transform)
            .and_then(|n| n.as_deref())
            .map(str::to_string)
            .unwrap_or_else(|| format!("T{}", transform));
        let verb = match mode {
            GizmoDragMode::TranslateView { .. } | GizmoDragMode::TranslateAxis { .. } => "Move",
            GizmoDragMode::Rotate { .. } => "Rotate",
            GizmoDragMode::Scale { .. } => "Scale",
        };
        format!("{} {}", verb, name)
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
        self.bump_matrix_generation();
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

    /// Scenes found by the last `toggle_browser` scan, for the browser
    /// window's list.
    pub fn browser_files(&self) -> &[std::path::PathBuf] {
        &self.browser_files
    }

    /// Index the Up/Down keys are currently on.
    pub fn browser_selected(&self) -> usize {
        self.browser_selected
    }

    /// Move the keyboard selection to a clicked row, so the two stay in sync.
    pub fn set_browser_selected(&mut self, idx: usize) {
        if idx < self.browser_files.len() {
            self.browser_selected = idx;
        }
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
            roll: scene.camera_roll,
        };
        self.invalidate_default_path();
        self.point_size = scene.point_size;
        self.color_falloff = scene.color_falloff;
        self.color_contrast = scene.color_contrast;
        // Same precedence as startup: a point count the person chose in the
        // Render window follows them across scene loads; otherwise take the
        // scene file's own.
        self.buffer_capacity = self
            .prefs
            .point_count
            .map(|n| n as u32)
            .unwrap_or(scene.point_count as u32);
        self.transform_enabled = vec![true; scene.transforms.len()];
        self.selected_transform = Some(0);
        self.selected_variation = 0;
        self.drag = Drag::None;
        self.hovered = None;
        self.scene = scene;
        self.scene_path = Some(path.to_string_lossy().into_owned());
        self.invalidate_saved_views();
        self.history.clear();
        self.rebuild_pipelines();
    }

    // === Render jobs ===

    /// Launch a render job on its own thread and its own wgpu device, so the
    /// realtime view keeps running — camera included.
    ///
    /// One at a time. A queue was explicitly deferred in the previous plan and
    /// stays deferred: two jobs sharing a GPU makes both slower and the
    /// estimates meaningless, and the interesting question ("did my render
    /// finish?") is answerable without one.
    pub fn start_job(&mut self, params: crate::render_job::JobParams) {
        use crate::render_job::{JobEvent, JobControl, JobKind};

        if self.job.is_some() {
            log::warn!("A render job is already running");
            return;
        }
        if let Some(reason) = params.rejection(self.max_point_capacity() as u64 * 16) {
            self.job_error = Some(reason);
            return;
        }
        self.job_error = None;
        // The camera keeps flying. The job takes a *snapshot* — `current_view`
        // below, and a clone of the scene — and everything after that runs on
        // its own thread and device, so there is no moving target to protect
        // against: stopping the flyby changed nothing about what got rendered,
        // it just interrupted what you were watching while you waited.

        // A view descriptor renders nothing — it's the "note down this
        // framing, render it later" case — so it completes inline rather than
        // spinning up a device and a thread to write one small file.
        if matches!(params.kind, JobKind::ViewDescriptor) {
            let view = self.current_view();
            let result = view
                .save(&params.out_path)
                .map(|()| params.out_path.clone())
                .map_err(|e| e.to_string());
            match &result {
                Ok(p) => log::info!("View descriptor written: {}", p.display()),
                Err(e) => log::error!("View descriptor failed: {}", e),
            }
            self.job_done = Some(result);
            return;
        }

        let mut scene = self.scene.clone();
        // Job-scoped: the interactive buffer keeps whatever the Render window
        // set, so exploring stays comfortable while a big render runs.
        scene.point_count = params.points;
        let view = self.current_view();

        let (tx, rx) = std::sync::mpsc::channel();
        let control = JobControl {
            events: tx,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pause: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let handle = JobHandle {
            params: params.clone(),
            events: rx,
            cancel: control.cancel.clone(),
            pause: control.pause.clone(),
            started: Instant::now(),
            phase: "starting",
            done: 0,
            total: 0,
            log: Vec::new(),
            cancel_armed_at: None,
            paused_at: None,
            paused_total: Duration::ZERO,
        };

        let out = params.out_path.clone();
        let kind = params.kind;
        let (splat, exposure, transparent, accumulate) =
            (params.splat, params.exposure, params.transparent, params.accumulate);

        log::info!(
            "Render job started: {} ({}, {} points)",
            out.display(),
            kind.label(),
            params.points,
        );

        std::thread::spawn(move || {
            control.phase("setting up");
            let base = crate::offline::OfflineParams {
                scene,
                view: Some(view),
                width: kind.size().0,
                height: kind.size().1,
                out_path: &out,
                accumulate,
                haze_enabled: false, // the view carries the real haze settings
                grid: crate::offline::GridMode::Single,
                splat,
                exposure,
                transparent,
                control: Some(control.clone()),
                // The dialog's framing is the view above; there are no flags
                // in here to override it with.
                camera: Default::default(),
            };
            let result = match kind {
                JobKind::Still { .. } => crate::offline::render(base),
                JobKind::Animation { fps, seconds, quality, format, .. } => {
                    crate::offline::render_animation(
                        base,
                        crate::offline::AnimParams {
                            fps,
                            seconds: Some(seconds),
                            quality,
                            format,
                        },
                    )
                }
                JobKind::ViewDescriptor => Ok(()), // handled inline above
            };
            let _ = control.events.send(JobEvent::Done(
                result.map(|()| out.clone()).map_err(|e| e.to_string()),
            ));
        });

        self.job = Some(handle);
    }

    /// Drain the running job's event queue into the handle the dialog reads.
    /// Called once per frame from `update`.
    fn poll_job(&mut self) {
        use crate::render_job::{JobEvent, CANCELLED};
        let Some(job) = &mut self.job else { return };
        let mut finished = None;
        while let Ok(event) = job.events.try_recv() {
            match event {
                JobEvent::Phase(p) => {
                    job.phase = p;
                    job.done = 0;
                    job.total = 0;
                    job.log.push(format!("— {}", p));
                }
                JobEvent::Progress { done, total } => {
                    job.done = done;
                    job.total = total;
                }
                JobEvent::Log(msg) => job.log.push(msg),
                JobEvent::Done(result) => finished = Some(result),
            }
        }
        // Keep the log bounded: a long animation logs per phase, but a future
        // chattier job shouldn't be able to grow this without limit.
        if job.log.len() > 200 {
            let excess = job.log.len() - 200;
            job.log.drain(0..excess);
        }
        if let Some(result) = finished {
            match &result {
                Ok(p) => log::info!("Render job finished: {}", p.display()),
                Err(e) if e == CANCELLED => log::info!("Render job cancelled"),
                Err(e) => log::error!("Render job failed: {}", e),
            }
            self.job = None;
            self.job_done = Some(result);
        }
    }

    /// The running job, for the dialog to display.
    pub fn job(&self) -> Option<&JobHandle> {
        self.job.as_ref()
    }

    pub fn job_mut(&mut self) -> Option<&mut JobHandle> {
        self.job.as_mut()
    }

    /// The last finished job's outcome, shown until the next one starts.
    pub fn job_done(&self) -> Option<&Result<std::path::PathBuf, String>> {
        self.job_done.as_ref()
    }

    /// Why the last launch attempt was refused, if it was.
    pub fn job_error(&self) -> Option<&str> {
        self.job_error.as_deref()
    }

    /// Measured chaos-game throughput in points per second, for the dialog's
    /// time estimate. The only real measurement of this machine available
    /// without running the job first.
    ///
    /// Crucially it's frame time *minus the present wait*. With vsync on, most
    /// of a frame is spent parked in `get_current_texture` doing nothing (see
    /// `present_wait_ms`), so the raw frame time says this GPU is about six
    /// times slower than it is — and an estimate built on that would quote
    /// minutes for a job that takes seconds. What's left after the wait is
    /// still an over-estimate of the chaos game alone, since it includes the
    /// render pass and the UI, which is the direction to be wrong in.
    pub fn measured_throughput(&self) -> Option<f32> {
        let working_ms = self.fps_tracker.current_frametime_ms - self.present_wait_ms;
        if working_ms <= 0.05 || self.buffer_capacity == 0 {
            return None;
        }
        Some(self.buffer_capacity as f32 / (working_ms / 1000.0))
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

        let mut traces = crate::trace::generate_traces(
            &self.scene.transforms,
            &self.transform_enabled,
            TRACES,
            STEPS,
            &mut rand::thread_rng(),
        );
        // The walkers run on the plain attractor; under infinite zoom the
        // points on screen don't, so bring each trace along with them or the
        // overlay draws a walk through something nobody is looking at.
        if let Some(zoom) = self.point_compute.zoom {
            for trace in &mut traces {
                let mut pos: Vec<glam::Vec3> = trace.iter().map(|s| s.pos).collect();
                zoom.renormalize_trace(&mut pos);
                for (step, p) in trace.iter_mut().zip(pos) {
                    step.pos = p;
                }
            }
        }

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

    /// Rebuild the selected transform's offset/rotation indicator lines when
    /// the selection or its matrix changes. Cheap (a few dozen segments) but
    /// pointless to redo every frame.
    fn refresh_indicators(&mut self) {
        let key = self.selected_transform.map(|i| (i, self.matrix_generation));
        if key == self.indicator_key {
            return;
        }
        self.indicator_key = key;
        let verts = match key {
            Some((i, _)) if i < self.scene.transforms.len() => {
                crate::indicators::build(self.scene.transforms[i].matrix)
            }
            _ => Vec::new(),
        };
        self.indicator_renderer.set_lines(&self.gpu.device, &verts);
    }

    /// Rebuild the camera-path polyline, but only when it would differ.
    ///
    /// `LineRenderer::set_lines` allocates a fresh GPU buffer every call, so
    /// this can't just run every frame the way the geometry is cheap enough to
    /// suggest. A fingerprint over the keys is what decides. Now that a playing
    /// path isn't drawn at all, the only thing that rebuilds per frame is
    /// dragging the camera around a *default* orbit, which re-synthesizes the
    /// path as you go — and that's the one case where seeing it move is the
    /// point.
    fn refresh_path_lines(&mut self) {
        let key = match (self.show_gizmos, self.visible_path()) {
            (true, Some(path)) => Some(path_fingerprint(path)),
            _ => None,
        };
        if key == self.path_lines_key {
            return;
        }
        self.path_lines_key = key;
        let verts = match (self.show_gizmos, self.visible_path()) {
            (true, Some(path)) => crate::indicators::build_path(path),
            _ => Vec::new(),
        };
        self.path_renderer.set_lines(&self.gpu.device, &verts);
    }

    /// World-space offset of the selected transform from the origin, for the
    /// label `ui::labels` paints on the offset vector.
    pub fn selected_offset(&self) -> Option<(Vec3, f32)> {
        let i = self.selected_transform?;
        let t = self.scene.transforms.get(i)?.matrix.w_axis.truncate();
        (t.length() > 1e-4).then(|| (t, t.length()))
    }

    // === Evolutionary exploration (U / Shift+U) ===

    /// Replace the whole scene with a freshly generated random flame
    /// (`src/randomize.rs`) — see `adopt_scene` for why that's an edit rather
    /// than a load.
    pub fn random_flame(&mut self) {
        let scene = crate::randomize::random_flame(&mut rand::thread_rng());
        log::info!(
            "Random flame: {} transforms, camera distance {:.2}",
            scene.transforms.len(),
            scene.camera_distance
        );
        self.adopt_scene(scene, "Random flame");
    }

    /// Start over on an empty canvas (`Scene::blank`) — the build-from-nothing
    /// workflow's entry point. An *edit*, like a random flame: one Ctrl+Z
    /// brings back whatever was on screen, so it's safe to reach for.
    pub fn new_blank_scene(&mut self) {
        log::info!("New blank scene");
        self.adopt_scene(Scene::blank(), "New blank scene");
    }

    /// Replace the whole scene with one generated in-app rather than loaded
    /// from disk (a random flame, a blank canvas). Unlike `load_scene_file`
    /// this is an *edit*: it goes on the history stack, so one Ctrl+Z brings
    /// back what was on screen.
    fn adopt_scene(&mut self, mut scene: Scene, label: &str) {
        let before = self.edit_snapshot();
        // A scene made in-app has nobody's name on it yet; put the person's
        // there if they've told us one (see `set_scene_author`).
        if scene.author.trim().is_empty() {
            scene.author = self.default_author();
        }
        self.camera = OrbitCamera {
            yaw: scene.camera_yaw,
            pitch: scene.camera_pitch,
            distance: scene.camera_distance,
            focus: scene.camera_focus,
            roll: scene.camera_roll,
        };
        self.invalidate_default_path();
        self.point_size = scene.point_size;
        self.color_falloff = scene.color_falloff;
        self.color_contrast = scene.color_contrast;
        self.haze_amount = scene.haze;
        self.transform_enabled = vec![true; scene.transforms.len()];
        self.selected_transform = Some(0);
        self.selected_variation = 0;
        self.drag = Drag::None;
        self.hovered = None;
        self.scene = scene;
        // A scene made in-app has no file behind it: Ctrl+S should write a new
        // scenes/untitled-*.toml rather than overwrite whatever was loaded.
        self.scene_path = None;
        self.invalidate_saved_views();
        self.bump_matrix_generation();
        self.after_scene_shape_change();
        self.commit_edit(label, None, before);
    }

    pub fn mutate_scene(&mut self) {
        let before = self.edit_snapshot();

        let log = crate::mutate::mutate(&mut self.scene, &mut rand::thread_rng(), self.prefs.mutate_strength);
        log::info!("Mutation: {}", log.join("; "));
        self.bump_matrix_generation();
        self.after_scene_shape_change();

        // Label from the op log, truncated so the history list stays tidy
        // (the op log can contain multi-byte glyphs like '°', so truncate at
        // the nearest char boundary rather than a raw byte index)
        let mut label = log.join("; ");
        if label.len() > 60 {
            let mut cut = 57;
            while cut > 0 && !label.is_char_boundary(cut) {
                cut -= 1;
            }
            label.truncate(cut);
            label.push_str("...");
        }
        self.commit_edit(label, None, before);
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
        let before = self.edit_snapshot();
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
        self.bump_matrix_generation();
        self.scene.regenerate_colormap();
        self.rebuild_pipelines();
        log::info!("Added transform T{}", self.scene.transforms.len() - 1);
        let label = if fresh { "Add transform" } else { "Duplicate transform" };
        self.commit_edit(label, None, before);
    }

    /// Delete the selected transform (keeps at least one)
    pub fn delete_selected_transform(&mut self) {
        let Some(idx) = self.selection() else { return };
        if self.scene.transforms.len() <= 1 {
            log::warn!("Cannot delete the last transform");
            return;
        }
        let before = self.edit_snapshot();
        self.scene.transforms.remove(idx);
        self.scene.transform_names.remove(idx);
        self.scene.colors.remove(idx);
        self.transform_enabled.remove(idx);
        self.selected_transform = Some(idx.min(self.scene.transforms.len() - 1));
        self.drag = Drag::None;
        self.bump_matrix_generation();
        self.scene.regenerate_colormap();
        self.rebuild_pipelines();
        log::info!("Deleted transform T{}", idx);
        self.commit_edit("Delete transform", None, before);
    }

    /// Multiply the selected transform's chaos-game weight
    pub fn adjust_weight(&mut self, increase: bool) {
        let Some(idx) = self.selection() else { return };
        let before = self.edit_snapshot();
        let factor = if increase { 1.15 } else { 1.0 / 1.15 };
        let w = &mut self.scene.transforms[idx].weight;
        *w = (*w * factor).clamp(0.01, 100.0);
        log::info!("T{} weight: {:.2}", idx, *w);
        self.sync_transforms_to_gpu();
        self.commit_edit(format!("Weight T{}", idx), Some(&format!("weight:T{}", idx)), before);
    }

    /// Nudge the selected transform's gradient color in HSV space
    /// (channel: 0 = hue, 1 = saturation, 2 = value)
    pub fn adjust_color(&mut self, channel: usize, increase: bool) {
        let Some(idx) = self.selection() else { return };
        if channel > 2 {
            return;
        }
        let before = self.edit_snapshot();
        let (mut h, mut s, mut v) = rgb_to_hsv(self.scene.colors[idx]);
        let dir = if increase { 1.0 } else { -1.0 };
        match channel {
            0 => h = (h + dir * 15.0).rem_euclid(360.0),
            1 => s = (s + dir * 0.08).clamp(0.0, 1.0),
            2 => v = (v + dir * 0.08).clamp(0.05, 1.0),
            _ => unreachable!("channel > 2 returned above"),
        }
        self.scene.colors[idx] = hsv_to_rgb(h, s, v);
        let c = self.scene.colors[idx];
        log::info!("T{} color: h={:.0} s={:.2} v={:.2} rgb=({:.2},{:.2},{:.2})", idx, h, s, v, c.x, c.y, c.z);
        self.scene.regenerate_colormap();
        self.point_compute.update_colormap(&self.gpu.queue, &self.scene.colormap);
        const CHANNEL_NAMES: [&str; 3] = ["Hue", "Saturation", "Value"];
        self.commit_edit(
            format!("{} T{}", CHANNEL_NAMES[channel], idx),
            Some(&format!("color:{}:T{}", channel, idx)),
            before,
        );
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
        let before = self.edit_snapshot();
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
        let vname = crate::scene::VARIATION_NAMES[self.selected_variation];
        self.commit_edit(
            format!("{} T{}", vname, idx),
            Some(&format!("varweight:T{}:{}", idx, self.selected_variation)),
            before,
        );
    }

    // === Transforms window inspector (Phase 4) ===
    //
    // These mirror the shape of `adjust_weight`/`adjust_color` above but act
    // on an explicit index (the inspector always edits the selected
    // transform, but list-row actions like Duplicate/Delete/eye-toggle can
    // target any row) and share the same coalesce-key naming scheme
    // (`insp:t{idx}:{field}`) the Phase 3 outcome notes call for.

    /// Replace a transform's matrix wholesale — the inspector's TRS fields
    /// (recomposed from position/rotation/scale), the non-TRS matrix grid,
    /// and "Orthogonalize -> TRS" all funnel through this.
    pub fn set_transform_matrix(
        &mut self,
        idx: usize,
        matrix: Mat4,
        label: impl Into<String>,
        coalesce_key: Option<String>,
    ) {
        if idx >= self.scene.transforms.len() {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.transforms[idx].matrix = matrix;
        self.bump_matrix_generation();
        self.sync_transforms_to_gpu();
        self.commit_edit(label, coalesce_key.as_deref(), before);
    }

    /// Discard any shear/mirroring in a transform's matrix by rebuilding it
    /// from its own (possibly approximate) scale/rotation/translation
    /// decomposition — the inspector's "Orthogonalize -> TRS" button. Always
    /// its own history entry (no coalescing), since it's an explicit,
    /// infrequent action.
    pub fn orthogonalize_transform(&mut self, idx: usize) {
        let Some(spec) = self.scene.transforms.get(idx) else { return };
        let (scale, rotation, translation) = spec.matrix.to_scale_rotation_translation();
        let orthogonalized = Mat4::from_scale_rotation_translation(scale, rotation, translation);
        self.set_transform_matrix(idx, orthogonalized, format!("Orthogonalize T{}", idx), None);
    }

    /// Rename a transform (inspector name field / list row's inline rename).
    pub fn rename_transform(&mut self, idx: usize, name: Option<String>) {
        if idx >= self.scene.transform_names.len() {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.transform_names[idx] = name;
        self.commit_edit(
            format!("Rename T{}", idx),
            Some(&format!("insp:t{}:name", idx)),
            before,
        );
    }

    /// Duplicate an arbitrary transform (list row context menu), regardless
    /// of what's currently selected. Reuses `add_transform`'s nudge-and-copy
    /// path and label, and leaves the new copy selected.
    pub fn duplicate_transform_at(&mut self, idx: usize) {
        if idx >= self.scene.transforms.len() {
            return;
        }
        self.selected_transform = Some(idx);
        self.add_transform(false);
    }

    /// Delete an arbitrary transform (list row context menu).
    pub fn delete_transform_at(&mut self, idx: usize) {
        if idx >= self.scene.transforms.len() {
            return;
        }
        self.selected_transform = Some(idx);
        self.delete_selected_transform();
    }

    /// Set a transform's chaos-game weight directly (inspector/list
    /// DragValue) — same coalesce key as `adjust_weight`/scroll-weight so
    /// they merge into one history entry regardless of which UI drove it.
    pub fn set_transform_weight(&mut self, idx: usize, weight: f32) {
        if idx >= self.scene.transforms.len() {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.transforms[idx].weight = weight.clamp(0.01, 100.0);
        self.sync_transforms_to_gpu();
        self.commit_edit(
            format!("Weight T{}", idx),
            Some(&format!("weight:T{}", idx)),
            before,
        );
    }

    /// Set a transform's gradient color directly (inspector color picker).
    pub fn set_transform_color(&mut self, idx: usize, color: Vec3) {
        if idx >= self.scene.colors.len() {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.colors[idx] = color;
        self.scene.regenerate_colormap();
        self.point_compute.update_colormap(&self.gpu.queue, &self.scene.colormap);
        self.commit_edit(
            format!("Recolor T{}", idx),
            Some(&format!("insp:t{}:color", idx)),
            before,
        );
    }

    /// Set a transform's explicit colormap index (inspector color_value
    /// slider).
    pub fn set_transform_color_value(&mut self, idx: usize, value: f32) {
        if idx >= self.scene.transforms.len() {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.transforms[idx].color_value = value.clamp(0.0, 1.0);
        self.sync_transforms_to_gpu();
        self.commit_edit(
            format!("Color value T{}", idx),
            Some(&format!("insp:t{}:color_value", idx)),
            before,
        );
    }

    /// Set (or clear) a transform's explicit color_speed override (inspector
    /// checkbox + slider).
    pub fn set_transform_explicit_color_speed(&mut self, idx: usize, speed: Option<f32>) {
        if idx >= self.scene.transforms.len() {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.transforms[idx].explicit_color_speed = speed;
        self.refresh_color_speeds();
        self.commit_edit(
            format!("Color speed T{}", idx),
            Some(&format!("insp:t{}:color_speed", idx)),
            before,
        );
    }

    /// Set a transform's weight for one variation slot (inspector variation
    /// editor row / add-variation combo box).
    pub fn set_transform_variation(&mut self, idx: usize, slot: usize, weight: f32) {
        if idx >= self.scene.transforms.len() || slot >= crate::scene::NUM_VARIATIONS {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.transforms[idx].variations[slot] = weight.clamp(-4.0, 4.0);
        self.sync_transforms_to_gpu();
        let vname = crate::scene::VARIATION_NAMES[slot];
        self.commit_edit(
            format!("{} T{}", vname, idx),
            Some(&format!("insp:t{}:var{}", idx, slot)),
            before,
        );
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
            self.overlay.samples(),
            &self.scene.transforms,
        );
        self.point_compute.update_weights(
            &self.gpu.queue,
            &self.scene.transforms,
            &self.transform_enabled,
        );
        self.gizmo_renderer.update_alpha(&self.gpu.queue, &self.transform_enabled);
        self.refresh_zoom();
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
        // The renormalizing map may itself have just been dragged
        self.refresh_zoom();
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
        let before = self.edit_snapshot();
        let factor = if increase { 1.1 } else { 0.909 };
        self.point_size = (self.point_size * factor).clamp(0.0001, 0.1);
        log::info!("Point size: {:.5}", self.point_size);
        self.commit_edit("Point size", Some("point_size"), before);
    }

    /// F / Shift+F. The near/far keys that used to sit beside this (N and M)
    /// are gone: the band auto-ranges off the camera now, so nudging it by
    /// half a world unit at a time no longer means anything.
    pub fn adjust_haze_intensity(&mut self, more: bool) {
        let delta = if more { 0.1 } else { -0.1 };
        let old = self.haze_amount;
        self.set_haze_amount(self.haze_amount + delta);
        log::info!("Haze: {:.2} -> {:.2}", old, self.haze_amount);
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
        let before = self.edit_snapshot();
        let old = self.color_falloff;
        self.color_falloff = if old == 0.0 {
            1.0
        } else {
            let factor = if finer { 1.0 / 1.15 } else { 1.15 };
            (old * factor).clamp(0.05, 4.0)
        };
        log::info!("Color falloff: {:.2} -> {:.2} (lower = finer color detail)", old, self.color_falloff);
        self.refresh_color_speeds();
        self.commit_edit("Color falloff", Some("color_falloff"), before);
    }

    pub fn adjust_color_contrast(&mut self, increase: bool) {
        let before = self.edit_snapshot();
        let factor = if increase { 1.15 } else { 1.0 / 1.15 };
        self.color_contrast = (self.color_contrast * factor).clamp(0.25, 16.0);
        log::info!("Color contrast: {:.2}", self.color_contrast);
        self.commit_edit("Color contrast", Some("color_contrast"), before);
    }

    // === Render window (Phase 5) ===
    //
    // Absolute-value setters for the controls that own a slider, alongside
    // the relative `adjust_*` nudges the keybinds use. Which of these commit
    // history is deliberate and follows Phase 3: parameters that get written
    // back into the scene TOML by Ctrl+S (point size, color falloff, color
    // contrast) are edits; view-only knobs (renderer mode, exposure) are
    // not, matching the keyboard paths exactly.

    pub fn set_render_mode(&mut self, mode: RenderMode) {
        if self.render_mode != mode {
            self.render_mode = mode;
            log::info!(
                "Renderer: {}",
                match mode {
                    RenderMode::Points => "points",
                    RenderMode::Splat => "splat (additive log-density)",
                }
            );
        }
    }

    pub fn set_point_size(&mut self, size: f32) {
        let before = self.edit_snapshot();
        self.point_size = size.clamp(0.0001, 0.1);
        self.commit_edit("Point size", Some("point_size"), before);
    }

    pub fn set_color_falloff(&mut self, falloff: f32) {
        let before = self.edit_snapshot();
        self.color_falloff = falloff.clamp(0.0, 4.0);
        self.refresh_color_speeds();
        self.commit_edit("Color falloff", Some("color_falloff"), before);
    }

    /// Haze band in world units: the pinned one, or auto-ranged off the
    /// current camera distance so it tracks the framing.
    pub fn haze_range(&self) -> (f32, f32) {
        self.haze_band
            .unwrap_or_else(|| crate::haze::auto_band(self.camera.distance))
    }

    /// `(brightness, saturation)` survival at the far plane, from the amount.
    pub fn haze_falloff(&self) -> (f32, f32) {
        crate::haze::falloff(self.haze_amount)
    }

    /// The four values the shader actually wants, in `CameraUniforms` order.
    pub fn haze_uniforms(&self) -> (f32, f32, f32, f32) {
        let (near, far) = self.haze_range();
        let (brightness, saturation) = self.haze_falloff();
        (near, far, brightness, saturation)
    }

    /// Background colour behind the fractal (linear RGB). Scene data, so it
    /// is an undoable edit like the other look parameters Ctrl+S writes.
    pub fn set_background(&mut self, rgb: Vec3) {
        let before = self.edit_snapshot();
        self.scene.background = rgb.clamp(Vec3::ZERO, Vec3::ONE);
        self.commit_edit("Background", Some("background"), before);
    }

    pub fn set_haze_amount(&mut self, amount: f32) {
        let before = self.edit_snapshot();
        self.haze_amount = amount.clamp(0.0, 1.0);
        self.commit_edit("Haze", Some("haze"), before);
    }

    pub fn set_color_contrast(&mut self, contrast: f32) {
        let before = self.edit_snapshot();
        self.color_contrast = contrast.clamp(0.25, 16.0);
        self.commit_edit("Color contrast", Some("color_contrast"), before);
    }

    /// Points the chaos game keeps in flight. Raising this costs GPU memory
    /// and a warmup, so the panel applies it explicitly rather than live.
    pub fn point_capacity(&self) -> u32 {
        self.buffer_capacity
    }

    /// Largest capacity this device can bind: the point buffer is a storage
    /// buffer of 16-byte `Point`s (vec3 position + packed color index), so
    /// the binding-size limit divided by 16 is the hard ceiling.
    pub fn max_point_capacity(&self) -> u32 {
        let limit = self.gpu.device.limits().max_storage_buffer_binding_size;
        ((limit / 16) as u32).max(100_000)
    }

    /// Ask for a new point-buffer capacity. The Render window calls this
    /// while you drag, and `update()` applies it at most a few times a
    /// second (see `apply_pending_capacity`).
    ///
    /// The old design was a value box plus an Apply button, which is exactly
    /// the wrong shape for the one control that decides whether the machine
    /// stays responsive: you commit blind to a number, and if it's too big
    /// you're already in trouble before you can react. Applying live means
    /// the frame counter and the fan tell you you've gone too far while
    /// you're still holding the mouse and can drag back.
    pub fn request_point_capacity(&mut self, count: u32) {
        let count = count.clamp(100_000, self.max_point_capacity());
        // Ignore jitter: a log slider emits a slightly different value every
        // pixel, and reallocating for a 1% change is pure churn.
        let ratio = count as f32 / self.buffer_capacity.max(1) as f32;
        if (0.98..=1.02).contains(&ratio) {
            self.pending_capacity = None;
            return;
        }
        self.pending_capacity = Some(count);
    }

    /// The capacity the slider is asking for, if it hasn't landed yet.
    pub fn pending_point_capacity(&self) -> Option<u32> {
        self.pending_capacity
    }

    /// Apply a requested capacity change, no more often than
    /// `CAPACITY_RATE_LIMIT`. Reallocating the point buffer is a real stall
    /// (it rebuilds the compute pipeline and restarts warmup), so a drag that
    /// sweeps two orders of magnitude must not try to do it every frame — but
    /// it should still land often enough that the drag feels connected.
    fn apply_pending_capacity(&mut self) {
        const CAPACITY_RATE_LIMIT: std::time::Duration = std::time::Duration::from_millis(250);
        let Some(count) = self.pending_capacity else { return };
        if self.last_capacity_change.elapsed() < CAPACITY_RATE_LIMIT {
            return;
        }
        self.pending_capacity = None;
        self.last_capacity_change = Instant::now();
        self.set_point_capacity(count);
    }

    /// Reallocate the point buffer. Not a history entry (it's a performance
    /// setting, not part of the artwork) but it *is* persisted to prefs, so
    /// the count follows the person across scenes and restarts.
    pub fn set_point_capacity(&mut self, count: u32) {
        let count = count.clamp(100_000, self.max_point_capacity());
        if count == self.buffer_capacity {
            return;
        }
        // Deliberately does *not* write `scene.point_count`: after startup
        // `buffer_capacity` is the single source of truth. `scene` rides
        // inside every history snapshot, so keeping a second copy there would
        // let an undo across a capacity change silently desync the two.
        self.buffer_capacity = count;
        log::info!("Point buffer capacity: {} (restarting warmup)", count);
        self.rebuild_pipelines();
        self.prefs.point_count = Some(count as usize);
        self.prefs_dirty_since = Some(std::time::Instant::now());
    }

    /// Whether the pitch-inversion preference is on (Camera window checkbox
    /// mirrors the I keybind).
    pub fn invert_pitch(&self) -> bool {
        self.prefs.invert_pitch
    }

    /// Toggle the transform gizmos *and* their name labels (G). These used to
    /// be two bindings; a separate toggle for "the gizmo's caption" was a
    /// distinction without a difference once the panels took over every other
    /// overlay readout.
    pub fn toggle_gizmos(&mut self) {
        self.show_gizmos = !self.show_gizmos;
        log::info!("Gizmos: {}", if self.show_gizmos { "on" } else { "off" });
    }

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
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
        self.toggle_transform_enabled(idx);
    }

    /// Enable/disable a transform by index. Shared by the Enter-key path
    /// (acts on the selection) and the Transforms window's eye toggle (acts
    /// on whichever row was clicked) — the plan requires these to behave
    /// identically, including the guard below.
    pub fn toggle_transform_enabled(&mut self, idx: usize) {
        if idx >= self.transform_enabled.len() { return }

        // Guard: don't disable the last enabled transform
        let enabled_count = self.transform_enabled.iter().filter(|&&e| e).count();
        if self.transform_enabled[idx] && enabled_count <= 1 {
            log::warn!("Cannot disable last remaining transform");
            return;
        }

        let before = self.edit_snapshot();
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
        let label = if self.transform_enabled[idx] {
            format!("Enable T{}", idx)
        } else {
            format!("Disable T{}", idx)
        };
        self.commit_edit(label, None, before);
    }

    pub fn request_screenshot(&mut self) {
        self.pending_screenshot = true;
    }

    /// Use native 1px point primitives (~3x faster) when the projected point
    /// size at the orbit distance would be subpixel anyway
    fn use_point_primitives(&self, screen_height: f32) -> bool {
        self.point_size * screen_height / self.camera.distance <= 1.5
    }

    // === Drawing fewer points than we have, when they'd land on top of
    //     each other ===

    /// Points per pixel of the attractor's screen footprint that are worth
    /// drawing. Generous — this is a safety valve, not a quality setting, and
    /// it must not touch a scene that's merely dense.
    const POINTS_PER_PIXEL: f32 = 2_000.0;

    /// Never draw fewer than this, however collapsed the attractor is. Well
    /// past the point where more points change the image.
    const MIN_DRAWN_POINTS: u32 = 400_000;

    /// How many of the buffer's points to actually draw this frame.
    ///
    /// Normally all of them. The exception is an attractor that has collapsed
    /// to a speck — a single enabled transform has exactly one fixed point, so
    /// every point in the buffer lands on the same pixel — and there the cost
    /// is brutal in a way that isn't obvious: blending is serialized per
    /// sample, so six million fragments on one texel is six million operations
    /// the GPU cannot overlap. Measured on the reference desktop: 654 FPS for a
    /// normal scene at 6M points, **46 FPS** for a one-transform scene at the
    /// same count, scaling linearly with the count. That's the state you're in
    /// for the first few seconds of building a scene from nothing, which is
    /// exactly when the app should feel light.
    ///
    /// Drawing fewer costs nothing visually, because the points are stacked:
    /// the image is the same dot either way. The splat renderer's exposure is
    /// normalized by the drawn count (see `upload_params`), so brightness
    /// doesn't move either.
    ///
    /// The live window only. `--render` and screenshots draw everything: they
    /// are one-off and must stay reproducible from the parameters alone.
    fn drawn_points(&self, screen_height: f32) -> u32 {
        let valid = self.point_compute.valid_point_count();
        // The budget is measured on the plain attractor by CPU walkers; under
        // infinite zoom the points get renormalized somewhere else entirely,
        // so the measurement doesn't describe what's on screen. There is also
        // nothing to protect against — a scale-invariant set never collapses
        // to a speck, which is the whole reason the budget exists.
        if self.point_compute.zoom.is_some() {
            return valid;
        }
        let Some(stats) = self.attractor else { return valid };
        let depth = (stats.center - self.camera.eye()).length();
        let r_px = stats.radius * OrbitCamera::pixels_per_world_unit(depth, screen_height);
        // A disc, not the dust that's actually there — an over-estimate of the
        // covered area, which errs toward not intervening.
        let covered = (std::f32::consts::PI * r_px * r_px).max(1.0);
        let cap = (covered * Self::POINTS_PER_PIXEL).min(u32::MAX as f32) as u32;
        valid.min(cap.max(Self::MIN_DRAWN_POINTS))
    }

    /// Rebuild the infinite-zoom renormalization from the scene, after
    /// anything that could have changed a transform matrix. Cheap (a 3x3
    /// inverse and a power iteration), so it just runs rather than being
    /// tracked by a dirty flag.
    ///
    /// A scene that asks for zoom it can't have keeps rendering — a mid-drag
    /// map that stopped contracting shouldn't blank the window — and says so
    /// in the status bar instead.
    pub fn refresh_zoom(&mut self) {
        let Some(spec) = self.scene.zoom.clone() else {
            self.point_compute.zoom = None;
            self.zoom_error = None;
            return;
        };
        match crate::renorm::Renorm::build(&spec, &self.scene.transforms, self.scene.camera_distance)
        {
            Ok(r) => {
                // A zoom loop is derived from this map, so re-derive it too —
                // otherwise dragging the map leaves the loop closing under the
                // similarity it used to have, and the seam quietly opens.
                if let Some(path) = self.scene.camera_path.as_mut() {
                    if let Some(z) = path.zoom_loop {
                        path.zoom_loop = Some(r.loop_similarity(z.periods));
                    }
                }
                self.point_compute.zoom = Some(r);
                self.zoom_error = None;
            }
            Err(e) => {
                self.point_compute.zoom = None;
                self.zoom_error = Some(e);
            }
        }
    }

    /// The live renormalization, if the scene has a usable one
    pub fn zoom(&self) -> Option<&crate::renorm::Renorm> {
        self.point_compute.zoom.as_ref()
    }

    /// Which transform is currently the scale symmetry, if any. Read by the
    /// Transforms context menu so its checkmark tracks the scene.
    pub fn zoom_map(&self) -> Option<usize> {
        self.scene.zoom.as_ref().map(|z| z.map)
    }

    /// Turn infinite zoom on for `map`, or off with `None`, and re-form the
    /// point cloud (every point moves, so the buffer has to refill).
    pub fn set_zoom_map(&mut self, map: Option<usize>) {
        self.scene.zoom = map.map(|map| crate::renorm::ZoomSpec {
            map,
            ..self.scene.zoom.clone().unwrap_or_default()
        });
        self.refresh_zoom();
        self.zoom_level = 0;
        self.point_compute.reset(&self.gpu.queue);
        self.frame_count = 0;
    }

    /// Keep the eye inside one zoom period; see `renorm::Renorm::wrap`
    fn wrap_zoom(&mut self) {
        let Some(zoom) = self.point_compute.zoom else { return };
        self.zoom_level += zoom.wrap(&mut self.camera);
    }

    /// What the drawn-point budget is keyed on: any change to the maps, to
    /// which of them are enabled, or to how many there are.
    fn attractor_fingerprint(&self) -> u64 {
        let mut h = self.matrix_generation
            .wrapping_mul(0x100000001b3)
            ^ (self.scene.transforms.len() as u64);
        for (i, on) in self.transform_enabled.iter().enumerate() {
            if *on {
                h ^= (i as u64).wrapping_add(1).wrapping_mul(0x9E3779B97F4A7C15);
            }
        }
        h
    }

    /// Re-measure the attractor when the IFS changed, at most a few times a
    /// second. Rate-limited because a gizmo drag bumps `matrix_generation`
    /// every frame and this runs a few thousand chaos steps — and because a
    /// slightly stale *budget* costs nothing.
    fn refresh_attractor(&mut self) {
        let key = self.attractor_fingerprint();
        if key == self.attractor_key {
            return;
        }
        if self.attractor.is_some()
            && self.attractor_measured_at.elapsed() < Duration::from_millis(200)
        {
            return;
        }
        self.attractor_key = key;
        self.attractor_measured_at = Instant::now();
        self.attractor = crate::trace::measure(&self.scene.transforms, &self.transform_enabled);
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_update).as_secs_f32().min(0.1);
        self.last_update = now;
        self.frame_dt = dt;
        self.flush_dirty_prefs();
        self.apply_pending_capacity();
        // Before the path lines are built, so what's drawn is what will fly.
        self.refresh_default_path();
        self.refresh_attractor();
        self.refresh_indicators();
        self.refresh_path_lines();
        self.poll_job();

        let should_log = self.fps_tracker.frame();
        self.frame_count += 1;
        // One motion: the camera flies the path. Which path — the scene's own
        // keys or the default orbit — is `camera_path`'s business, not this
        // loop's, so the turntable and a hand-authored flythrough are the same
        // three lines of code and behave identically.
        if let Some(t) = self.path_t {
            let (duration, loops, playable) = {
                let p = self.camera_path();
                (p.duration(), p.wraps(), p.playable())
            };
            if !playable {
                self.path_t = None;
            } else {
                let t = t + dt / duration;
                // A zoom loop wraps like a closed one: it ends on the frame it
                // began, so there is nothing to stop for.
                let ended = !loops && t >= 1.0;
                let t = if ended {
                    1.0
                } else if loops {
                    t.rem_euclid(1.0)
                } else {
                    t
                };
                let cam = self.camera_path().sample(t);
                self.camera = cam;
                // A path key's distance is unwrapped — that's the point, it's
                // how a path descends nine zoom periods in one straight
                // interpolation — so the sample arrives outside the band every
                // frame and `wrap_zoom` puts it back. Its return is then the
                // absolute depth of this sample, not a step, so the counter
                // has to be reset rather than added to. Miss this and the
                // reading runs away by nine per frame.
                self.zoom_level = 0;
                self.path_t = if ended { None } else { Some(t) };
                if ended {
                    log::info!("Camera path: finished");
                }
            }
        }

        // Keep the eye inside one zoom period. Last, so it catches the camera
        // however it moved this frame — dragged, scrolled, or flown along the
        // path — and the picture it lands on is identical to the one it left.
        self.wrap_zoom();

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

    }

    pub fn take_screenshot(&mut self) {
        let point_count = self.point_compute.valid_point_count();
        if point_count == 0 {
            log::warn!("No points to screenshot");
            return;
        }

        let aspect = SCREENSHOT_WIDTH as f32 / SCREENSHOT_HEIGHT as f32;
        let mvp = self.camera.view_proj(aspect);
        // Unlike the window pass, this one writes to a file that has an alpha
        // channel worth filling in.
        let alpha = if self.transparent_render { 0.0 } else { 1.0 };

        let haze = self.haze_uniforms();
        let camera = CameraUniforms::new(
            mvp,
            SCREENSHOT_HEIGHT as f32,
            self.point_size,
            aspect,
            1.0,
            haze.0,
            haze.1,
            haze.2,
            haze.3,
            self.color_contrast,
            self.scene.background.to_array(),
            // A screenshot is a file with an alpha channel worth filling in.
            self.transparent_render,
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
                            load: wgpu::LoadOp::Clear(crate::scene::clear_color(self.scene.background, alpha)),
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
                    crate::scene::clear_color(self.scene.background, alpha),
                    self.transparent_render,
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

    /// Dispatch a clicked keybind row. `shift` selects the second-listed
    /// binding (so a row reading "J / Shift+J" runs the Shift variant).
    pub fn run_help_action(&mut self, action: HelpAction, shift: bool) {
        match action {
            HelpAction::ToggleHelp => self.toggle_help(),
            HelpAction::Reset => self.reset(),
            HelpAction::Zoom => if shift { self.zoom_out() } else { self.zoom_in() },
            HelpAction::ToggleSelected => self.toggle_selected_transform(),
            HelpAction::ToggleGizmos => self.toggle_gizmos(),
            HelpAction::CameraMotion => self.toggle_camera_motion(),
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
            HelpAction::HazeIntensity => self.adjust_haze_intensity(!shift),
            HelpAction::Mutate => {
                if shift {
                    self.undo()
                } else {
                    self.mutate_scene()
                }
            }
            HelpAction::Undo => {
                if shift {
                    self.redo()
                } else {
                    self.undo()
                }
            }
            HelpAction::Browse => self.toggle_browser(),
            HelpAction::Traces => self.toggle_traces(shift),
            HelpAction::InvertPitch => self.toggle_invert_pitch(),
            HelpAction::RenderMode => self.toggle_render_mode(),
            HelpAction::Exposure => self.adjust_exposure(!shift),
        }
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

        // Advance circular buffer and get valid point count. What we *draw*
        // can be less — see `drawn_points` for the collapsed-attractor case.
        self.point_compute.advance_frame(&self.gpu.queue, self.frame_dt);
        let point_count = self.drawn_points(height as f32);

        // wgpu 29: `get_current_texture` returns `CurrentSurfaceTexture`
        // directly (no more `Result<_, SurfaceError>`, and no `OutOfMemory`
        // variant — device loss is now reported via
        // `Device::set_device_lost_callback` instead).
        // Acquiring the swapchain image is where a vsync-paced frame parks:
        // under FIFO the driver blocks here until a buffer frees up. Timing it
        // separately is what makes "we're idle waiting for the display" and
        // "we're actually too slow" distinguishable in the status bar — see
        // `record_present_wait`.
        let acquire_start = Instant::now();
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
        self.record_present_wait(acquire_start.elapsed().as_secs_f32() * 1000.0);
        let color_view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = self.depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame_encoder"),
        });

        // === STEP 1: RUN CHAOS GAME ===
        self.point_compute.dispatch(&mut encoder);

        // === STEP 2: RENDER POINTS ===
        let haze = self.haze_uniforms();
        let camera = CameraUniforms::new(
            view_proj,
            height as f32,
            self.point_size,
            aspect,
            1.0,
            haze.0,
            haze.1,
            haze.2,
            haze.3,
            self.color_contrast,
            self.scene.background.to_array(),
            // The window's swapchain has nothing behind it to composite
            // through, so the live pass is always opaque.
            false,
        );
        self.gizmo_renderer.upload_camera(&self.gpu.queue, &camera);
        if self.show_traces {
            self.line_renderer.upload_camera(&self.gpu.queue, &camera);
        }
        if self.show_gizmos {
            self.indicator_renderer.upload_camera(&self.gpu.queue, &camera);
            self.path_renderer.upload_camera(&self.gpu.queue, &camera);
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
                            load: wgpu::LoadOp::Clear(crate::scene::clear_color(self.scene.background, 1.0)),
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
                    // The *drawn* count, not the buffer's: exposure is
                    // normalized by it, so capping the draw (see
                    // `drawn_points`) doesn't change how bright the result is.
                    // During warmup this is below capacity too, which is the
                    // brightness ramp that has always been there.
                    self.buffer_capacity.min(point_count.max(1)),
                    height as f32,
                    crate::scene::clear_color(self.scene.background, 1.0),
                    false,
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

        // === STEP 2.5/2.6/3: THE IN-WORLD UI, MULTISAMPLED ===
        //
        // Traces, the selected transform's indicators, the camera path and the
        // gizmos all go into one multisampled target and come back as a single
        // composite (see src/gpu/overlay.rs). They used to be three passes
        // straight onto the swapchain, sharing the main depth buffer and its
        // aliasing; the pass gets its own copy of that depth so the point
        // cloud still occludes them, and its own sample count so they are the
        // only thing anti-aliased.
        let draw_overlay = self.show_traces || self.show_gizmos;
        if draw_overlay {
            {
                let mut render_pass = self.overlay.begin(&mut encoder);
                if self.show_traces {
                    self.line_renderer.draw(&mut render_pass);
                }
                if self.show_gizmos {
                    self.indicator_renderer.draw(&mut render_pass);
                    self.path_renderer.draw(&mut render_pass);
                    self.gizmo_renderer.draw(&mut render_pass);
                }
            }
            self.overlay.composite(&mut encoder, &color_view);
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
