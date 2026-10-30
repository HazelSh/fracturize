use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{Mat4, Vec3};
use winit::window::Window;

use crate::camera::{OrbitCamera, OrbitStyle};
use crate::gpu::lines::LineVertex;
use crate::gpu::{CameraUniforms, EmphasisParams, EmphasisRenderer, GizmoRenderer, GpuContext, LineRenderer, OverlayTargets, PointCompute, PointRenderer, SplatRenderer, DEPTH_FORMAT};
use crate::history::{EditSnapshot, History};
use crate::path::{CameraPath, LoopKind};
use crate::scene::{Scene, TransformSpec};
use crate::ui::{hints, UiState};
use crate::view::View;

#[path = "app_collab.rs"]
mod collab;
#[path = "app_agent.rs"]
pub(crate) mod agent;

/// Seconds since the epoch, for unique output filenames
pub fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A saved view's name as the Camera panel should show it.
///
/// Views are born called `<slug>-<unix timestamp>`, so until somebody renames
/// one its name is ten digits — which says *when* and reads as nothing. A bare
/// timestamp becomes `2026-08-16 14:08` (UTC); anything else is a name a
/// person chose and is shown exactly as they wrote it.
///
/// UTC rather than local time, and deliberately: the offset rules are a
/// database, this app has no date dependency, and guessing wrong would put a
/// confidently incorrect hour on the label. The date and the ordering are
/// what the label is for.
/// Floor on the point size the controls will set — the `[` / `]` keys, the
/// panel slider's low end, and `set_point_size`'s clamp, all from here.
///
/// It was 0.0001, which was also the point-size slider's lower bound and was
/// fine while the number meant one thing. Under an infinite zoom it means a
/// size *at the scene's own framing*, and saving re-expresses it against the
/// framing being written (see `sync_scene_for_save`) — a save at the deep end
/// of a band multiplies it by the zoom's scale, so a scene authored at the old
/// floor legitimately ends up below it. A floor a real scene sits under is a
/// control that lies about its own value, and clamping to it would put back
/// exactly the jump that change removed.
pub const MIN_POINT_SIZE: f32 = 1e-6;

pub fn view_display_name(name: &str) -> String {
    let Some(secs) = name.parse::<i64>().ok().filter(|s| *s > 0) else {
        return name.to_string();
    };
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, rem / 3600, (rem % 3600) / 60)
}

/// Days since the Unix epoch to a civil (proleptic Gregorian) date.
///
/// Howard Hinnant's `civil_from_days`, the standard branch-free form — chosen
/// over a leap-year loop because it is the version that is *known* correct,
/// and over a date crate because one label does not earn a dependency.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
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
    pub phase: crate::render_job::Phase,
    /// Estimated cost of each phase, so one bar can span all of them. Built at
    /// launch from the same constants the pre-flight estimate quotes.
    pub ledger: crate::render_job::Ledger,
    pub done: u32,
    pub total: u32,
    pub log: Vec<String>,
    /// The two-step cancel's arm state. A long render is expensive to lose to
    /// a misclick, so the button arms rather than fires — and, crucially,
    /// refuses the second click until a wall-clock second has passed, so a
    /// double-click can't span the guard. See `ui::confirm::Arm`.
    pub cancel_arm: crate::ui::confirm::Arm,
    /// When the current pause began, and how long previous pauses lasted.
    ///
    /// Kept so the time estimate can run on *working* time. Without it,
    /// wall-clock elapsed keeps climbing while progress is frozen and the
    /// projected remaining time climbs with it — a countdown that goes up,
    /// which is worse than no countdown at all.
    paused_at: Option<Instant>,
    paused_total: Duration,
    /// The countdown actually shown, and the working-time it was last moved at.
    ///
    /// Not the raw extrapolation: that is `working / f - working` off an
    /// `overall()` which only moves when a frame, lap or tile reports, so it
    /// sawtooths — creeping up between reports and dropping at each one, by
    /// tens of seconds on a job whose reports are seconds apart. A countdown
    /// that visibly jitters is one you can't read a number off. See
    /// [`tick_estimate`](Self::tick_estimate).
    eta: Option<f32>,
    eta_at: f32,
}

/// Time constant of the ETA filter, in seconds of working time: how long the
/// shown countdown takes to slew most of the way to a changed estimate.
///
/// Long enough to absorb the report-to-report sawtooth, short enough that a
/// job which genuinely slows down (a tile grid hitting the small tiles at the
/// edge) says so within a few seconds rather than a minute.
const ETA_TAU: f32 = 6.0;

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

    /// Stop the job for good. Guarded by `ui::confirm::danger_button`, which
    /// owns the arm-and-confirm interaction; by the time this is called the
    /// person has confirmed.
    ///
    /// A cancelled job also un-pauses, or it would sit in the pause loop never
    /// noticing it had been cancelled.
    pub fn cancel_now(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.set_paused(false);
    }

    /// Fraction complete within the current phase, when it reports one.
    pub fn fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| (self.done as f32 / self.total as f32).clamp(0.0, 1.0))
    }

    /// Fraction of the **whole job** complete, phases weighted by their
    /// estimated cost.
    ///
    /// This is what a person watching wants, and it is not the same as
    /// [`fraction`](Self::fraction): the phases are wildly unequal — an A2
    /// poster spends 67s accumulating and 5.7s saving — so a bar showing
    /// progress within the current phase races to 100% and starts again, which
    /// reads as either finished or broken. Weighted, it only moves forward.
    pub fn overall(&self) -> f32 {
        self.ledger.fraction(self.phase, self.fraction())
    }

    /// Seconds remaining, extrapolated from observed progress and filtered.
    /// `None` until there is enough progress for the extrapolation to mean
    /// anything — a countdown from one sample is a random number.
    ///
    /// Extrapolates from the *overall* fraction, so the estimate does not lurch
    /// at every phase boundary the way a within-phase one does.
    pub fn remaining_secs(&self) -> Option<f32> {
        self.eta
    }

    /// The unfiltered extrapolation [`tick_estimate`](Self::tick_estimate)
    /// steers towards. Split out so the filter can be tested against it.
    fn raw_remaining_secs(&self) -> Option<f32> {
        let f = self.overall();
        if f < 0.05 {
            return None;
        }
        let working = self.working_elapsed();
        Some((working / f - working).max(0.0))
    }

    /// Move the shown countdown. Called once per frame, from `poll_job`.
    ///
    /// Two things happen here, and they are the two things a countdown has to
    /// do. It **counts down in real time**: between progress reports the number
    /// falls by the seconds that actually passed, because that is what those
    /// seconds mean. And it **slews** towards the current extrapolation with a
    /// time constant rather than snapping to it, so a report that revises the
    /// estimate by a minute arrives as a few seconds of drift instead of a
    /// jump. Filtering the *rate* instead would be the textbook answer and is
    /// worse here: the rate can read zero across a long silent phase, and
    /// `1 / 0` is a countdown to the heat death of the universe.
    ///
    /// Paused time is excluded, same as everywhere else on this handle: the
    /// countdown holds while paused instead of running out.
    pub fn tick_estimate(&mut self) {
        if self.paused() {
            return;
        }
        let working = self.working_elapsed();
        let dt = (working - self.eta_at).max(0.0);
        self.eta_at = working;
        let Some(raw) = self.raw_remaining_secs() else {
            return;
        };
        self.eta = Some(match self.eta {
            None => raw,
            Some(shown) => {
                let counted_down = (shown - dt).max(0.0);
                let alpha = 1.0 - (-dt / ETA_TAU).exp();
                counted_down + alpha * (raw - counted_down)
            }
        });
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
    mix(path.loops.kind() as u32);
    // The zoom loop changes where the closing segment goes, so the drawn
    // polyline has to be rebuilt when it changes or when its map is dragged
    mix(path.loops.zoom().map_or(0, |z| z.periods.wrapping_add(1)));
    mix(path.loops.zoom().map_or(0, |z| z.scale.to_bits()));
    mix(path.ease.map_or(2, |e| e as u32));
    mix(path.period.unwrap_or(f32::NAN).to_bits());
    for t in path.key_times() {
        mix(t.to_bits());
    }
    // Routes are part of the shape: two paths through the same keys but
    // different routes draw different polylines.
    for r in &path.routes {
        match r {
            crate::path::Route::Turns(w) => mix(*w as u32),
            crate::path::Route::Exact(t) => {
                for v in t.as_rotation_vector().to_array() {
                    mix(v.to_bits());
                }
            }
        }
    }
    for k in &path.keys {
        let q = k.orientation.as_quat();
        for v in [q.x, q.y, q.z, q.w, k.distance, k.focus.x, k.focus.y, k.focus.z] {
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
    /// Dragging an axis endpoint: stretch that one axis, leaving the other two
    /// alone.
    ///
    /// `dir` is the axis's world direction captured at grab and held fixed for
    /// the drag, so the column only ever changes length. That keeps the three
    /// columns mutually perpendicular, which is what keeps the matrix an honest
    /// scale-rotate-translate and so keeps it saveable — the file format has no
    /// way to write a sheared matrix (see GIZMO-PLAN.md §1.1).
    ///
    /// `len0` is the column's length at grab and `s0` the pointer's parameter
    /// along the axis line, so grabbing slightly off the end doesn't snap.
    /// Length is signed: dragging back through the origin flips the axis, which
    /// mirrors the map, and that is a continuous move rather than a special
    /// case.
    ///
    /// `dash_pitch` is the world-space period of the extension line drawn while
    /// this is held, and `dash_reach` the shortest it may run from the origin.
    /// Both are fixed here at grab time so the dashes don't crawl as the axis
    /// changes length (see `indicators::build_axis_extension`), and both come
    /// from a screen measurement, so a very short axis still draws a line long
    /// enough to see and aim along.
    ScaleAxis { k: usize, dir: Vec3, len0: f32, s0: f32, dash_pitch: f32, dash_reach: f32 },
    /// Dragging an outer edge: rotate around the transform's local axis
    /// (world direction `axis`, through the transform origin)
    Rotate { axis: Vec3, center: (f32, f32), start_angle: crate::rot::Angle },
    /// Dragging the ring: roll about the camera's own view axis, through the
    /// transform's origin.
    ///
    /// Carries the same three values as [`GizmoDragMode::Rotate`] and is driven
    /// by exactly the same math — the only difference is where the axis came
    /// from, and that the undo entry says "Roll". It is a separate variant
    /// rather than a flag so that difference is visible at the call site.
    Roll { axis: Vec3, center: (f32, f32), start_angle: crate::rot::Angle },
    /// Ctrl-drag anywhere on the gizmo: uniform scale, drag up = grow
    Scale { start_y: f32 },
}

/// Smallest a scaled axis may get. A zero-length column is a singular matrix,
/// and everything downstream divides by the column length — the inspector's
/// decomposition, the save path, the contraction measure.
const MIN_AXIS_LEN: f32 = 1e-4;

/// Signed length for an axis-scale drag: where the pointer is along the axis,
/// offset so that grabbing slightly off the endpoint doesn't snap it.
///
/// Signed on purpose. Dragging back past the origin takes the length negative,
/// which reverses that column and mirrors the map through the other two axes.
/// It is one continuous move with no branch in it, which is the whole reason it
/// is safe to leave ungated.
///
/// `snap` is alt: tenths, because the tetrahedron is the unit shape, so a
/// column's length *is* that axis's scale factor and 0.1 steps are the round
/// numbers scenes get written in.
fn axis_scale_length(len0: f32, s: f32, s0: f32, snap: bool) -> f32 {
    let mut len = len0 + (s - s0);
    if snap {
        const SNAP: f32 = 0.1;
        len = (len / SNAP).round() * SNAP;
    }
    if len.abs() < MIN_AXIS_LEN {
        // Keep whichever side of zero the drag asked for; clamping to a fixed
        // sign would make the mirror stick on the way through.
        len = if len < 0.0 { -MIN_AXIS_LEN } else { MIN_AXIS_LEN };
    }
    len
}

/// Replace one column of a matrix's linear part with `dir * len`, leaving the
/// other two columns and the translation exactly as they were.
///
/// Written as a column store rather than a decompose-and-recompose, and that is
/// the point: `dir` is fixed at grab time, so the column changes length without
/// changing direction, the three columns stay mutually perpendicular, and the
/// matrix stays a faithful scale-rotate-translate. A round trip through
/// `to_scale_rotation_translation` here would quietly destroy any shear the
/// matrix already carried, and the scene format has nowhere to write shear
/// back out (GIZMO-PLAN.md §1.1).
fn with_scaled_column(start: Mat4, k: usize, dir: Vec3, len: f32) -> Mat4 {
    let mut m = start;
    let col = (dir * len).extend(0.0);
    match k {
        0 => m.x_axis = col,
        1 => m.y_axis = col,
        _ => m.z_axis = col,
    }
    m
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
    ToggleEmphasis,
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
    /// Left / Right: a frame back or on; with shift, to the key before or after
    StepFrame,
    PathKey,
    NewScene,
    Quit,
    FrameSelected,
}

/// Which point renderer draws the fractal. Points is the classic opaque
/// depth-tested renderer; Splat is additive log-density accumulation
/// (flame-style, no occlusion); Volume is splat with absorption, so near
/// material hides what is behind it by the opacity setting. R cycles them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    Points,
    Splat,
    Volume,
}

impl RenderMode {
    /// Both the splat family: they share the accumulation and tonemap, so
    /// exposure, the grade and the emphasis glow all treat them alike.
    pub fn is_splat_family(self) -> bool {
        matches!(self, RenderMode::Splat | RenderMode::Volume)
    }

    pub fn label(self) -> &'static str {
        match self {
            RenderMode::Points => "points",
            RenderMode::Splat => "splat (additive log-density)",
            RenderMode::Volume => "volume (splat with absorption)",
        }
    }
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

/// How long the zoom magnifier stays after the last wheel step, so the pauses
/// inside one gesture don't drop it.
const ZOOM_CURSOR_HOLD: Duration = Duration::from_millis(450);
/// How far the zoom-direction accumulator can run either way. Three steps to
/// cross from settled to reversed: enough that a stray step doesn't flip the
/// icon, few enough that a deliberate reversal is followed promptly.
const ZOOM_BIAS_CAP: f32 = 3.0;

/// How long a discrete camera move takes. Roughly Blender's default, and about
/// the shortest interval that still reads as motion rather than a cut.
const CAMERA_GLIDE: Duration = Duration::from_millis(200);

/// A camera glide in flight: where it left, where it's going, and when it
/// started. See [`App::glide_camera_to`].
struct CameraGlide {
    from: OrbitCamera,
    to: OrbitCamera,
    started: Instant,
}

/// Interpolate a framing.
///
/// Distance is interpolated *geometrically*, not linearly: distance is a zoom,
/// zoom is multiplicative, and a linear ramp from 100 to 1 spends nine tenths
/// of its time already arrived. Orientation goes the short way round as one
/// turn about one axis, which is what the eye reads as a single movement rather
/// than a yaw followed by a pitch.
fn lerp_camera(a: &OrbitCamera, b: &OrbitCamera, s: f32) -> OrbitCamera {
    let turn = a.orientation.shortest_turn_to(b.orientation);
    OrbitCamera {
        orientation: a
            .orientation
            .then_body(crate::rot::Turn::from_rotation_vector(turn.as_rotation_vector() * s)),
        distance: a.distance * (b.distance / a.distance).powf(s),
        focus: a.focus.lerp(b.focus, s),
        // A fly-to moves the camera, not the lens. Both ends are the live
        // camera's FoV in every caller; taking `a`'s rather than lerping keeps
        // that true even if one day they aren't.
        fov: a.fov,
    }
}

/// How far the pointer must travel, in physical pixels, before a button-down
/// counts as a drag rather than a click. Three to four pixels is what every
/// other drag-capable surface uses, and it is enough to absorb the twitch a
/// hand makes while clicking a mouse button.
const DRAG_THRESHOLD_PX: f32 = 4.0;

/// How long the pointer must sit still over the artwork, in view mode, before
/// it is hidden (`App::update_cursor_visibility`).
///
/// Two seconds: long enough that it never goes while you are still deciding
/// where to click, short enough that it is gone by the time you have settled
/// into watching. Video players land in the same place for the same reason.
const CURSOR_IDLE_HIDE: Duration = Duration::from_secs(2);

/// Should the pointer be hidden right now? Split out from
/// `App::update_cursor_visibility` because the decision is the whole of the
/// feature and the rest is one winit call — and because every clause here is a
/// case somebody has to be able to check without a mouse and a stopwatch.
fn cursor_should_hide(view_mode: bool, over_ui: bool, dragging: bool, idle: Duration) -> bool {
    view_mode && !over_ui && !dragging && idle >= CURSOR_IDLE_HIDE
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
    /// The press landed on an unselected transform's origin dot and was spent
    /// selecting it. Nothing follows: no transform edit, and no camera orbit
    /// either, so the view doesn't swing off a click that was aimed at a dot.
    Consumed,
    /// Left-drag on a gizmo: edit that transform live
    Gizmo {
        /// Index into `App::gizmo_specs`: a map, or 0 for a subscene.
        transform: usize,
        /// The subscene being moved as a whole, when it is one.
        subscene: Option<usize>,
        mode: GizmoDragMode,
        /// In scene space, like everything `gizmo_specs` hands out.
        start_matrix: Mat4,
    },
}

/// One layer's xaos chain, for the panels: see [`App::xaos_summary`].
pub struct XaosSummary {
    /// The layer's maps as scene indices, in layer order — what the chain's
    /// positions mean.
    pub idx: Vec<usize>,
    pub chain: crate::xaos::Chain,
    /// Each map's share of the walk, by layer position.
    pub shares: Vec<f64>,
    /// Whether any row in the layer says anything.
    pub has_xaos: bool,
}

impl XaosSummary {
    /// Layer position of scene index `i`.
    pub fn pos(&self, i: usize) -> Option<usize> {
        self.idx.iter().position(|&x| x == i)
    }
}

/// The scene's tracks, built for the live window (see `App::refresh_animation`).
struct LiveAnimation {
    /// The document it was built from: history position and scene generation.
    key: (Option<u64>, u64),
    /// `None` when every track is muted, or the scene's tracks won't apply
    animator: Option<crate::track::Animator>,
    /// The playhead it last showed, so a paused timeline costs nothing
    shown_at: Option<f64>,
    /// Which targets are tracked, among the values the window keeps live on
    /// `App` rather than on the scene
    view_targets: ViewTargets,
}

#[derive(Default, Clone, Copy)]
struct ViewTargets {
    point_size: bool,
    exposure: bool,
    haze: bool,
    contrast: bool,
    falloff: bool,
    background: bool,
}

/// View-class values a track holds at the playhead (`App::anim_view`).
#[derive(Default, Clone, Copy)]
struct AnimView {
    point_size: Option<f32>,
    exposure: Option<f32>,
    haze: Option<f32>,
    contrast: Option<f32>,
    background: Option<Vec3>,
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
    /// The same count, but *monotone*: it keeps climbing across a zoom loop's
    /// seam instead of returning to zero with the playhead.
    ///
    /// [`zoom_level`](Self::zoom_level) can't serve. A looping path resamples
    /// an unwrapped spline every frame, so what `Renorm::wrap` hands back is
    /// the absolute depth of *this* sample and the counter is reset rather
    /// than added to (see [`App::update`]) — which is right for a readout that
    /// says how deep you are within one pass, and wrong for anything that has
    /// to stay continuous while the pass rolls over.
    ///
    /// The one thing that has to is [`zoom_frame`](Self::zoom_frame): a wrap
    /// turns the camera by the map's rotation, so anything drawn in world axes
    /// spins by that much in one frame unless it is carried along.
    pub zoom_turns: i32,
    /// Whole loops of a zoom-looping path completed since the scene loaded,
    /// in periods. The base [`zoom_turns`](Self::zoom_turns) counts up from
    /// while a path is flying.
    zoom_turns_base: i32,
    /// The turn the *point buffer* has been carried to. Chases
    /// [`zoom_turns`](Self::zoom_turns); the difference is what the next
    /// `PointCompute::rewrap` has to apply.
    zoom_turns_drawn: i32,
    /// Periods the buffer still owes the camera, waiting for the frame's own
    /// encoder. `wrap_zoom` runs in `update`, which has no encoder, so the
    /// pass used to take a submit of its own.
    ///
    /// Recorded into the frame instead because that is where it belongs — it
    /// is that frame's work, and it has to land before the chaos dispatch
    /// either way. Not because it is faster: over 129 wraps it measured no
    /// different (3.9% of wrap frames missed a 120Hz vsync against 2.7% of
    /// ordinary ones, and the same reading the other way round), and the
    /// 0.93ms the pass costs is well under this renderer's ordinary frame
    /// jitter.
    pending_rewrap: i32,
    /// Why infinite zoom is off, when the scene asked for it and it couldn't
    /// be built. Shown in the status bar rather than swallowed.
    pub zoom_error: Option<String>,
    /// The timeline's playhead, in seconds: the one clock the camera path and
    /// every `[[track]]` are sampled on (see `crate::track`).
    ///
    /// It used to be `path_t`, a 0..1 position along the camera path, because
    /// the camera was the only thing that moved. Now tracks move too, and they
    /// are in seconds, so the clock is; the camera samples it at
    /// `(time − camera_epoch) / duration`.
    time: f64,
    /// Whether the clock is running.
    playing: bool,
    /// Whether the camera is flying the path, rather than being positioned by
    /// hand. Taking the camera stops this and, in a scene with tracks, leaves
    /// the clock running — you can orbit an animation while it plays. In a
    /// scene without them the two are one motion, as they always were.
    camera_follows: bool,
    /// Where on the clock the camera path's own t = 0 sits. Zero for a path
    /// the scene authored; for the default orbit, the moment it was last
    /// rebuilt around the current framing, so the orbit always starts from
    /// where you are.
    camera_epoch: f64,
    /// Which pass of a looping path the camera is on, so a zoom loop can count
    /// the periods it has descended.
    camera_lap: i64,
    /// Whether playback runs back to 0 at the end of the timeline (a session
    /// setting, toggled in the Timeline window).
    pub loop_playback: bool,
    /// The loop region, in seconds, start before end: with looping on,
    /// playback goes round inside it instead of over the whole timeline. Set
    /// on the Timeline's ruler. Session state, like `loop_playback`.
    pub loop_region: Option<(f64, f64)>,
    /// Record is armed: an edit to a value keys it at the playhead
    /// (`autokey`), in the same undo step, instead of editing the value the
    /// scene authors. Session state; it never arms itself.
    pub record_armed: bool,
    /// The playhead, history position and scene `sync_record_base` last put
    /// the authored scene at.
    record_synced: Option<(u64, Option<u64>, u64)>,
    /// The scene's tracks, ready to evaluate, and what they last showed.
    live_anim: Option<LiveAnimation>,
    /// View-class values a track holds at the playhead, swapped in around the
    /// frame's GPU work only (`App::render`) so that nothing which edits or
    /// saves the scene ever sees them.
    anim_view: AnimView,
    /// Bumped whenever a whole scene arrives, so the live animator knows its
    /// history position names a different document.
    scene_generation: u64,
    /// Why the tracks aren't playing, when a track won't apply.
    track_error: Option<String>,
    /// The track whose value the scene can't use at the playhead, while that
    /// lasts: what makes the error once per stretch rather than per frame.
    track_invalid: Option<String>,
    /// `param_catalogue`'s cache, keyed like the live animator.
    param_cache: Option<((Option<u64>, u64), std::rc::Rc<Vec<crate::params::Param>>)>,
    pub point_size: f32,

    /// Persistent user preferences (I toggles pitch inversion)
    prefs: crate::prefs::Prefs,
    /// Duration of the last frame, for time-based motion and churn
    frame_dt: f32,

    // Mouse state
    cursor: (f32, f32),
    drag: Drag,
    /// Where the pointer was when the button went down, and whether it has
    /// since travelled far enough to count as a drag rather than a click.
    ///
    /// This is what tells a click from a drag at release, which is what
    /// click-to-deselect needs and what keeps a click on a gizmo from
    /// committing a one-pixel edit.
    ///
    /// **It does not gate camera motion.** The obvious reading of "add a drag
    /// threshold" is to withhold movement until the pointer has travelled far
    /// enough, and that is wrong here: orbiting is the gesture you use
    /// continuously, and a dead zone on it is felt as stiction at the start of
    /// every single drag — a constant cost, paid to prevent a two-pixel camera
    /// nudge that is imperceptible and that nothing records. So the camera
    /// tracks the pointer from the first pixel and this flag only *observes*.
    /// Gizmo drags are the other way round (see `on_cursor_moved`).
    drag_origin: (f32, f32),
    drag_moved: bool,
    /// Shift-for-fine during a gizmo drag: the virtual-cursor position and the
    /// real cursor position at the moment the modifier last changed, plus the
    /// state currently in force.
    ///
    /// Held as an anchor pair rather than a plain gain so pressing or releasing
    /// Shift *mid-drag* doesn't teleport the thing you're dragging — the new
    /// gain applies from where the pointer is now, which is what every tool
    /// that has this does and what makes it usable as a mid-gesture correction
    /// rather than a decision you make before you start.
    fine_anchor: (f32, f32),
    fine_from: (f32, f32),
    fine_active: bool,
    /// A discrete camera move in flight (see `glide_camera_to`).
    camera_glide: Option<CameraGlide>,
    /// The magnifier shown while scroll-zooming: which way it points, and when
    /// it stops being shown. See `show_zoom_cursor`.
    zoom_cursor: Option<(bool, Instant)>,
    /// Accumulated scroll direction, for the hysteresis on that swap.
    zoom_bias: f32,
    /// Gizmo part under the cursor (when not dragging)
    hovered: Option<crate::pick::GizmoHit>,
    pub shift_held: bool,
    pub ctrl_held: bool,
    /// Alt: the modifier that turns scroll from navigation into an edit (see
    /// `on_scroll`), and rotation snapping on a gizmo drag.
    pub alt_held: bool,

    pub show_gizmos: bool,
    pub show_help: bool,

    // Haze. One control; see `src/haze.rs` for why the shader's four
    // parameters are not the user's four parameters.
    /// Depth-cue strength, 0 (off) to 1. Scene data — saved by Ctrl+S and
    /// undoable, like the other look parameters the scene file carries.
    pub haze_amount: f32,
    /// Write an alpha channel in captured stills (S) and render jobs, so
    /// they can be composited. Session state, not scene data: it says what
    /// leaves the app, not what the artwork is. The live window is opaque
    /// unless `transparent_window` says otherwise.
    pub transparent_render: bool,
    /// `--transparent-window`: the live view clears to alpha 0 and writes
    /// premultiplied coverage, so the desktop shows through everywhere the
    /// fractal isn't. Fixed for the session — the window's visual is chosen
    /// when it is created.
    transparent_window: bool,
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
    /// The glow over the material the selected transform moves. Drawn over
    /// whichever renderer produced the frame; see `src/gpu/points/emphasis.rs`.
    emphasis_renderer: EmphasisRenderer,
    /// Whether that glow is wanted at all. It only ever appears in edit mode
    /// with something selected, so this is the switch for people who would
    /// rather edit against the picture they are going to keep.
    pub show_emphasis: bool,
    emphasis_params: EmphasisParams,
    /// Mute/solo: which maps' points are drawn (`gpu::points::mask`). A view,
    /// like orbiting — session state, never in the file or the history — and
    /// cleared when maps come or go, since it is held by position.
    pub map_view: crate::gpu::points::mask::MuteSolo,
    map_view_count: usize,
    gizmo_renderer: GizmoRenderer,
    line_renderer: LineRenderer,
    /// Second line buffer for the selected transform's offset/rotation
    /// indicators (see src/indicators.rs). Separate from `line_renderer`
    /// because traces are rebuilt on demand and these follow the selection.
    indicator_renderer: LineRenderer,
    /// `(selected transform, matrix generation)` the indicator buffer was
    /// built for; rebuilt only when that changes.
    /// What the indicator lines were last built for: the selected transform and
    /// its matrix generation, plus the axis handle being held (with its dash
    /// pitch, as raw bits so the key stays comparable).
    indicator_key: (Option<(usize, u64)>, Option<(usize, u32, u32)>),
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

    /// Cached lacunarity summary — computed on a slower cadence since it's
    /// more expensive than dimension (which is O(N) while this is O(N * steps)).
    /// Updated once per second or when the scene changes.
    lacunarity: Option<f32>,
    lacunarity_key: u64,
    lacunarity_measured_at: Instant,

    /// The running render job, if any (see `start_job`). One at a time.
    job: Option<JobHandle>,
    /// The last finished job, kept so the dialog can report it after the
    /// handle is gone. Its result carries [`crate::render_job::Outcome`]
    /// rather than a bare path, which is what stops a render stopped at 40%
    /// being reported as a finished one — every display site has to say which.
    job_done: Option<crate::render_job::FinishedJob>,
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
    /// Volume-mode opacity. **Session state**, deliberately: nothing writes it
    /// to the scene, a view or the preferences yet (that is the next slice, and
    /// it will be scene data). 0 is plain splat.
    pub opacity: f32,
    /// The bounding sphere volume mode frames, for a composite, whose
    /// attractor `refresh_attractor` does not describe. Keyed on
    /// `attractor_fingerprint`.
    volume_bounds: Option<(Vec3, f32)>,
    volume_bounds_key: u64,

    /// The tonemap grade — gamma, its threshold, and vibrancy.
    ///
    /// Live, and exactly live: the grade is a pure function of the accumulated
    /// density, so the curve this applies in the window is the same arithmetic
    /// an offline render applies. The viewport's input is noisier, but it is
    /// not an *approximation* of the render's grade, which is what makes this
    /// worth being a slider you drag rather than a number you commit to in the
    /// render dialog and find out about minutes later.
    ///
    /// **View** data, not scene data — see `View`'s own note on why. That is
    /// also why there is no `set_grade` committing to the undo history the way
    /// `set_exposure` does: undo is for edits Ctrl+S will write to the scene
    /// TOML, and this is never written there.
    pub grade: crate::gpu::points::splat::Grade,

    /// Chaos-game trace overlay (X): show walker paths as line segments
    pub show_traces: bool,

    /// The scene, mutable: gizmo drags and editing keys change it in place,
    /// Ctrl+S writes it back to disk
    pub scene: Scene,
    /// Scene file path (save target; also recorded in saved view files)
    pub scene_path: Option<String>,
    /// The render this scene was opened from, when it came out of a PNG's
    /// embedded `fracturize:scene` chunk. `scene_path` is `None` then: there
    /// is no `.toml` behind it, and Ctrl+S writes a new one rather than
    /// touching the picture. Cleared by anything that gives the scene a file.
    pub scene_png: Option<std::path::PathBuf>,
    /// A message about the last attempt to open a scene — why it failed, or
    /// what was ignored — shown in the status bar until dismissed or until the
    /// next open. See `set_notice`.
    notice: Option<Notice>,
    /// A file being dragged over the window, for the status bar's hint.
    pub drop_hover: Option<std::path::PathBuf>,
    /// Files dropped since the last frame. winit delivers a multi-file drop as
    /// one event per file, so they're gathered here and handled together.
    dropped: Vec<std::path::PathBuf>,
    /// Point buffer capacity for HUD
    buffer_capacity: u32,

    /// Selected transform index (Some when text overlay visible)
    selected_transform: Option<usize>,
    /// A whole subscene selected as one object (a layer, `1..=n`), exclusive
    /// with `selected_transform`. While set, the gizmo layer shows that
    /// subscene's placement and nothing else — see `gizmo_specs`.
    selected_subscene: Option<usize>,
    /// The layer the Transforms window lists and new maps join. Follows the
    /// selection; 0 (the scene's own maps) for an ordinary scene, always.
    current_layer: usize,
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
    /// One layer's xaos chain — walk shares, probabilities, dead ends — keyed
    /// by a hash of what it was built from, so the Xaos block, the rail's
    /// walk-share marks and the matrix window share one computation per edit
    /// rather than one per frame per widget. See [`App::xaos_summary`].
    xaos_cache: Option<(u64, std::rc::Rc<XaosSummary>)>,

    // Scene browser (B, Ctrl+O)
    pub show_browser: bool,
    /// The folder the browser is showing, read when it opens or navigates;
    /// `Err` when that folder couldn't be read.
    browser: Option<Result<crate::scene_browser::Folder, String>>,
    browser_selected: usize,
    /// Scene thumbnails for the browser's rows. See `gpu::thumbs`.
    pub thumbs: crate::gpu::thumbs::ThumbCache,
    /// The Random flames list: rolls under the person's random settings,
    /// made on background threads. See `random_list`.
    pub random_list: crate::random_list::RandomList,
    /// The Mutate window's grid: the scene's mutations around it, made on
    /// background threads. See `mutation_grid`.
    pub mutation_grid: crate::mutation_grid::MutationGrid,
    /// Scenes opened from disk so far. With the history's position it tells
    /// the mutation grid when the scene may have changed under it: opening a
    /// file clears the history, so the position alone can't.
    scene_loads: u64,
    /// The dice profile the random settings came from: its name, and what it
    /// said, so the Random settings window can mark them changed.
    dice_profile: crate::dice::profile::Selected,
    /// The chaos game's and the window's splat pipelines, compiled once. See
    /// `ChaosPipelines`.
    chaos_pipelines: crate::gpu::points::compute::ChaosPipelines,
    splat_pipelines: crate::gpu::points::splat::SplatPipelines,

    /// Unified undo/redo history (see src/history.rs): every scene-mutating
    /// edit path commits through `commit_edit`. Cleared on scene load.
    pub history: History,
    /// Snapshot taken at gizmo-grab time, consumed (and committed) at
    /// release — a drag is a single history entry, not one per frame.
    gizmo_drag_before: Option<EditSnapshot>,

    /// When the pointer last did anything — moved, clicked, scrolled. Read by
    /// `update_cursor_visibility` for the idle auto-hide.
    cursor_last_moved: Instant,
    /// Whether we currently have the pointer hidden. Tracked so the winit call
    /// is made on the frame the state changes and not on every frame.
    cursor_hidden: bool,
    /// The document state that was last written to disk, as a history serial
    /// (see `History::top_serial`). `None` means "nothing has been edited
    /// since this scene was opened", which is also what a freshly-loaded
    /// scene's history reads.
    ///
    /// A *position*, not a flag. `is_dirty` compares it against where the
    /// history is now, so undoing back past the last save reports the scene as
    /// clean — which it is, byte for byte — and redoing forward past it
    /// reports dirty again. The boolean this replaced could only ever be set
    /// by an edit and cleared by a save, so a scene you had edited and then
    /// completely undone still claimed to be modified, and closing it still
    /// demanded an answer about work that no longer existed.
    saved_serial: Option<u64>,
    /// Why the last attempt to write the scene failed, if it did (see
    /// `write_scene_to`).
    last_save_error: Option<String>,
    /// Last string handed to `Window::set_title`, so the title is only pushed
    /// to the window manager when it actually changes (see
    /// `refresh_window_title`).
    window_title: String,
    /// A destructive action waiting on the unsaved-changes prompt
    /// (`ui::confirm`). `Some` means the modal is up and the action happens
    /// only if the person says so.
    pub pending_action: Option<crate::ui::confirm::Pending>,
    /// Set when the app should leave. The event loop owns the actual exit, so
    /// quitting from inside a UI frame — the Save/Discard buttons of the
    /// unsaved-changes prompt — has to ask rather than do.
    pub exit_requested: bool,

    depth_texture: wgpu::Texture,

    screenshot_texture: wgpu::Texture,
    screenshot_depth: wgpu::Texture,
    screenshot_buffer: wgpu::Buffer,
    pub pending_screenshot: bool,
    /// Where the next screenshot goes instead of `screenshots/`: a render a
    /// thread message is waiting for (`app_collab`).
    screenshot_to: Option<std::path::PathBuf>,
    /// Threads with the agent: the Collaborate window's state. See
    /// `app_collab`.
    pub(crate) collab: collab::Collab,
    /// The agent connection: the socket a session's bridge reaches the
    /// window on, and the backends the window drives itself. See
    /// `app_agent`.
    pub(crate) agent: agent::AgentState,
    /// Whether the window has keyboard focus, so a new message for the person
    /// can be announced to the desktop only when they aren't looking.
    pub focused: bool,
}

/// A status-bar message about opening a scene (see `App::set_notice`).
pub struct Notice {
    pub text: String,
    pub tone: Tone,
}

/// What a notice is saying, which decides its colour (`ui::theme`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Worth knowing: a drop that ignored files, say.
    Note,
    /// Something asked for was done: sent, kept, withdrawn.
    Success,
    /// A failure.
    Error,
}

/// Whether a dropped file is one the window can open, by its extension: a
/// scene, or a render with a scene in it. Content is checked when it loads.
pub fn droppable(path: &Path) -> Result<(), String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    match ext.as_deref() {
        Some("toml") | Some("png") => Ok(()),
        _ => Err(format!(
            "Can't open {}: drop a .toml scene or a .png render",
            path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        )),
    }
}

/// A mutation's history label: what changed, cut short so the History window
/// stays tidy (at a character boundary — the log has glyphs like `°` in it).
fn mutation_label(prefix: &str, log: &[String]) -> String {
    let mut label = format!("{prefix}{}", log.join("; "));
    if label.len() > 60 {
        let mut cut = 57;
        while cut > 0 && !label.is_char_boundary(cut) {
            cut -= 1;
        }
        label.truncate(cut);
        label.push_str("...");
    }
    label
}

impl App {
    /// Create a new App
    pub async fn new(
        window: Arc<Window>,
        scene: Scene,
        haze_enabled: bool,
        vsync: bool,
        // `--transparent-window`: the window was created transparent, so
        // render the live view with the desktop showing through it.
        transparent_window: bool,
        scene_path: Option<String>,
        view: Option<View>,
        // `--fov`. Wins over the view, the scene and the preference alike,
        // which is what makes it usable as an override for a settings file
        // you don't want to edit.
        fov_override: Option<crate::camera::Fov>,
        splat: bool,
        // `--volume`: start in volume mode. The opacity is `--opacity`, else the
        // default; either way it is session state and nothing saves it.
        volume: bool,
        opacity: Option<f32>,
        exposure: Option<f32>,
        // `grade` arrives already resolved against the view and the CLI flags
        // — see `resolve_grade` in `main.rs`, which the headless path shares.
        grade: crate::gpu::points::splat::Grade,
    ) -> Self {
        let gpu = GpuContext::new(window.clone(), vsync, transparent_window).await;

        log::info!("Loaded scene: {} by {}", scene.name, scene.author);

        let point_size = scene.point_size;
        log::info!("Point size from scene: {}", point_size);

        let buffer_capacity = scene.point_count as u32;
        log::info!("Point buffer capacity: {}", buffer_capacity);

        // Compiled once for the life of the app: rebuilding the chaos game on
        // a transform add, and every thumbnail in the scene browser, share them.
        let chaos_pipelines = crate::gpu::points::compute::ChaosPipelines::new(&gpu.device);
        // fp16 emission accumulation, as the window has always had; see
        // `SplatPipelines::with_precision`. Volume mode's moments are fp32
        // whatever this says.
        let splat_pipelines =
            crate::gpu::points::splat::SplatPipelines::with_precision(&gpu.device, gpu.format, false);
        let thumbs = crate::gpu::thumbs::ThumbCache::new(&gpu.device, &chaos_pipelines);

        // Create point compute pipeline
        let point_compute = PointCompute::with_pipelines(
            &gpu.device,
            &chaos_pipelines,
            &crate::gpu::points::compute::LayerTable::build(&scene, &[], scene.color_contrast),
            buffer_capacity,
            crate::gpu::points::compute::DEFAULT_SEED,
            crate::gpu::points::compute::DEFAULT_WORKGROUPS,
        );

        // Create point renderer
        let point_renderer = PointRenderer::new(
            &gpu.device,
            gpu.format,
            &point_compute.point_buffer,
            &point_compute.colormap_buffer,
        );

        // Splat renderer shares the same point + colormap buffers
        let splat_renderer = SplatRenderer::with_pipelines(
            &gpu.device,
            &splat_pipelines,
            &point_compute.point_buffer,
            &point_compute.colormap_buffer,
        );

        let emphasis_renderer = EmphasisRenderer::new(
            &gpu.device,
            gpu.format,
            &point_compute.point_buffer,
            &point_compute.sensitivity_buffer,
            &point_compute.stats_buffer,
        );

        // --splat / a splat-captured view starts in splat mode; an explicit
        // --exposure wins over the view's saved exposure
        let volume_ok = splat_pipelines.volume_available();
        if volume && !volume_ok {
            eprintln!(
                "--volume needs fp32 blending (FLOAT32_BLENDABLE), which this GPU does not \
                 offer. The moments it accumulates are useless at fp16 precision, so there is \
                 no fallback."
            );
            std::process::exit(1);
        }
        let view_volume = view.as_ref().is_some_and(|v| v.is_volume());
        let render_mode = if volume || (view_volume && volume_ok) {
            RenderMode::Volume
        } else if splat || view_volume || view.as_ref().is_some_and(|v| v.is_splat()) {
            // A view saved in volume mode on a GPU that cannot do it opens as
            // the splat it is a refinement of, rather than refusing to open.
            RenderMode::Splat
        } else {
            RenderMode::Points
        };
        // Same precedence as everything else the scene now carries: the flag
        // beats the view, the view beats the scene file, the scene file beats
        // the default. Before `exposure` was scene data this bottomed out at a
        // hardcoded 1.0, and a scene tuned for 1.8 came back looking wrong.
        let exposure = exposure
            .or(view.as_ref().and_then(|v| v.exposure))
            .unwrap_or(scene.exposure);

        // Everything drawn into the overlay pass shares its sample count.
        let overlay_samples = OverlayTargets::choose_samples(&gpu.surface_sample_counts);

        // Create gizmo renderer
        let gizmo_renderer = GizmoRenderer::new(
            &gpu.device,
            gpu.format,
            overlay_samples,
            &scene.world_transforms(),
        );
        let first_layer = scene.first_layer_with_maps();

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
        let random_list = crate::random_list::RandomList::new(prefs.random.clone());
        let dice_profile = crate::dice::profile::Selected::remembered(
            prefs.random_profile.as_deref(),
            prefs.random_profile_builtin.as_deref(),
        );
        let ui_state = UiState::from_prefs(&prefs);
        // Read before `prefs` is moved into the struct below.
        let (help_open, browser_open) = (prefs.panels.help_open, prefs.panels.browser_open);

        // The framing is the scene's; the lens is the scene's only if it
        // states one. Otherwise it is the person's, from prefs — the windowed
        // app is the only place prefs may decide this, because a headless
        // render has to stay reproducible from flags plus scene alone (see
        // the `threads` note in `main.rs`).
        let camera = {
            let mut c = scene.camera();
            if !scene.camera_fov_authored {
                if let Some(d) = prefs.fov {
                    c.fov = crate::camera::Fov::from_degrees(d);
                }
            }
            c
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
            time: 0.0,
            playing: true,
            camera_follows: true,
            camera_epoch: 0.0,
            camera_lap: 0,
            loop_playback: true,
            loop_region: None,
            record_armed: false,
            record_synced: None,
            live_anim: None,
            anim_view: AnimView::default(),
            scene_generation: 0,
            track_error: None,
            track_invalid: None,
            param_cache: None,
            camera,
            point_size,
            prefs,
            frame_dt: 1.0 / 60.0,
            cursor: (0.0, 0.0),
            drag: Drag::None,
            drag_origin: (0.0, 0.0),
            drag_moved: false,
            fine_anchor: (0.0, 0.0),
            fine_from: (0.0, 0.0),
            fine_active: false,
            camera_glide: None,
            zoom_cursor: None,
            zoom_bias: 0.0,
            hovered: None,
            shift_held: false,
            ctrl_held: false,
            alt_held: false,
            show_gizmos: true,
            // Persisted like the other five panels; the env override lets
            // automated captures verify the help overlay regardless.
            show_help: help_open || std::env::var("FRACTURIZE_SHOW_HELP").is_ok(),
            haze_amount,
            haze_band: None,
            transparent_render: false,
            transparent_window,
            color_falloff: scene.color_falloff,
            color_contrast: scene.color_contrast,
            fps_tracker: FpsTracker::new(),
            zoom_level: 0,
            zoom_turns: 0,
            zoom_turns_base: 0,
            zoom_turns_drawn: 0,
            pending_rewrap: 0,
            zoom_error: None,
            point_compute,
            point_renderer,
            splat_renderer,
            emphasis_renderer,
            show_emphasis: true,
            map_view: Default::default(),
            map_view_count: 0,
            emphasis_params: EmphasisParams::default(),
            gizmo_renderer,
            line_renderer,
            indicator_renderer,
            indicator_key: (None, None),
            path_renderer,
            path_lines_key: None,
            overlay,
            attractor: None,
            // Zero is not a fingerprint any real scene produces, so the first
            // frame always measures.
            attractor_key: 0,
            attractor_measured_at: Instant::now(),
            lacunarity: None,
            lacunarity_key: 0,
            lacunarity_measured_at: Instant::now(),
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
            opacity: opacity
                .unwrap_or(crate::gpu::points::volume::DEFAULT_OPACITY)
                .clamp(0.0, crate::gpu::points::volume::MAX_OPACITY),
            volume_bounds: None,
            volume_bounds_key: 0,
            grade: grade.clamped(),
            show_traces: false,
            scene,
            // A render passed as `-s` is opened, never saved back to.
            scene_png: scene_path
                .as_deref()
                .filter(|p| crate::scene_source::is_png(p))
                .map(std::path::PathBuf::from),
            scene_path: scene_path.filter(|p| !crate::scene_source::is_png(p)),
            notice: None,
            drop_hover: None,
            dropped: Vec::new(),
            buffer_capacity,
            // Nothing selected until you pick something. A scene arrives as a
            // picture, not as an open inspector: selecting transform 0 on load
            // put a gizmo and (since the emphasis) a lit region on screen that
            // nobody had asked for, on every scene, before anyone had looked
            // at it.
            selected_transform: None,
            selected_subscene: None,
            current_layer: first_layer,
            transform_enabled: vec![true; num_transforms],
            selected_variation: 0,
            matrix_generation: 0,
            xaos_cache: None,
            // Reopened below once the app exists — the browser has a directory
            // scan behind it, so it can't just be a bool set here.
            show_browser: false,
            browser: None,
            browser_selected: 0,
            thumbs,
            random_list,
            mutation_grid: crate::mutation_grid::MutationGrid::new(),
            scene_loads: 0,
            dice_profile,
            chaos_pipelines,
            splat_pipelines,
            history: History::new(),
            gizmo_drag_before: None,
            cursor_last_moved: Instant::now(),
            cursor_hidden: false,
            saved_serial: None,
            last_save_error: None,
            window_title: String::new(),
            pending_action: None,
            exit_requested: false,
            depth_texture,
            screenshot_texture,
            screenshot_depth,
            screenshot_buffer,
            pending_screenshot: false,
            screenshot_to: None,
            collab: Default::default(),
            agent: Default::default(),
            focused: true,
        };
        app.refresh_zoom();

        // The browser is a bool with a directory scan behind it, so restoring
        // it means actually opening it — and if `scenes/` has since gone away,
        // `toggle_browser` declines and the toolbar reads honestly.
        if browser_open {
            app.toggle_browser();
        }

        // Apply a saved view, if given. Pause the orbit so the loaded
        // framing holds exactly (press O to resume).
        if let Some(v) = view {
            // Startup already honoured `--splat` / `--exposure` over the
            // view's own renderer fields, so don't let apply_view re-apply
            // them here.
            app.apply_view(&v, false);
        }

        // Last, so it beats the view file as well as the scene and the
        // preference. `glide_camera_to` in `apply_view` sets a target rather
        // than the camera, so the lens has to be put on both.
        if let Some(f) = fov_override {
            app.set_camera_fov(f);
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
        let target = OrbitCamera::from_legacy(
            Vec3::from(v.focus),
            Vec3::from(v.offset),
            v.distance,
            v.rotation,
            v.pitch,
            v.roll,
        );
        // The framing glides; everything else the view carries lands at once.
        // Point size and haze aren't *places*, so easing them would only make
        // the view look like it arrived twice.
        self.glide_camera_to(target);
        // A view written before the lens was settable says nothing about it,
        // and keeping the current one is the honest reading of that: it is
        // what the person is already looking through.
        if let Some(d) = v.fov {
            self.set_camera_fov(crate::camera::Fov::from_degrees(d));
        }
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
            self.render_mode = if v.is_volume() && self.splat_renderer.volume_available() {
                RenderMode::Volume
            } else if v.is_splat() || v.is_volume() {
                RenderMode::Splat
            } else {
                RenderMode::Points
            };
            if let Some(e) = v.exposure {
                self.exposure = e;
            }
            // Per field, via `View::grade` — a hand-written view that sets only
            // `gamma` gets the neutral threshold and vibrancy rather than being
            // treated as carrying no grade at all.
            self.grade = v.grade();
        }
        self.stop_following();
        self.invalidate_default_path();
        log::info!("Loaded view (camera stopped; press O to fly the path)");
    }

    // === The camera path (one system, with a default) ===

    /// The camera path. There is always one.
    ///
    /// A scene with a camera track (`target = "camera"`) flies it; one that
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
        // the orbit restarts its own clock here; continuing from the old one
        // would jump. Only the default's — an authored path must not restart
        // because someone scrolled, and the timeline's clock never does.
        if was_default {
            self.camera_epoch = self.time;
        }
    }

    /// Set the camera's view-axis roll (the Camera window's field and its
    /// "level" button; right-drag goes through `OrbitCamera::roll_by`).
    /// Put the horizon back level (the Camera window's "level" button).
    ///
    /// A no-op looking straight up or down, where every roll is level and the
    /// request has no answer — which is a framing you can now reach.
    pub fn level_camera(&mut self) {
        let mut target = self.camera;
        target.level();
        self.glide_camera_to(target);
        self.invalidate_default_path();
    }

    /// Put the camera on the selected transform's fixed point (Home).
    ///
    /// Every 3D tool has "frame selected" — Blender's numpad-`.`, everyone
    /// else's Home — and this app had nothing. "Frame all" is ill-defined for an
    /// object with no largest feature, but *this* is exactly defined: an affine
    /// contraction has one point it doesn't move, `(I − A)p = b`, and that point
    /// is the centre of everything the transform generates. It is also the most
    /// useful place to be at depth, where the rest of the attractor is
    /// elsewhere and you cannot see it to navigate by.
    ///
    /// Moves the focus and leaves the distance alone: this answers "take me to
    /// it", not "and how close".
    pub fn frame_selected_transform(&mut self) {
        let Some(idx) = self.selected_transform else {
            log::warn!("Nothing selected — click a transform's gizmo or a row in the Transforms window");
            return;
        };
        if idx >= self.scene.transforms.len() {
            return;
        }
        // The map's fixed point in its own subscene's space, carried into the
        // scene's — which is where the camera is.
        let m = self.scene.transforms[idx].matrix;
        let placement = self.scene.layer_placement(self.scene.transforms[idx].layer as usize);
        let a = glam::Mat3::from_cols(
            m.x_axis.truncate(),
            m.y_axis.truncate(),
            m.z_axis.truncate(),
        );
        let b = m.w_axis.truncate();
        let p = placement.transform_point3((glam::Mat3::IDENTITY - a).inverse().mul_vec3(b));
        if !p.is_finite() {
            // A map with an eigenvalue at 1 — a pure translation, say — has no
            // finite fixed point. Say so rather than flying the camera to NaN.
            log::warn!("T{} has no finite fixed point (its linear part fixes a direction)", idx);
            return;
        }
        log::info!("Framing T{}'s fixed point ({:.3}, {:.3}, {:.3})", idx, p.x, p.y, p.z);
        let target = OrbitCamera { focus: p, ..self.camera };
        self.glide_camera_to(target);
        self.stop_camera_motion();
    }

    /// Move the camera to `target` over [`CAMERA_GLIDE`] rather than teleporting.
    ///
    /// Loading a saved view and pressing `level` both used to cut, and a cut
    /// gives you no idea *where* you went — a 3D scene has no landmarks except
    /// the ones you were just looking at, so an instant reframe reads as a
    /// different scene rather than a different angle. Blender, Fusion and
    /// everything after them smooth view changes over roughly this long by
    /// default, and it is the single most-noticed "this feels expensive"
    /// detail in a 3D application. Cheap to add; large return.
    ///
    /// Only for *discrete* framing changes. Drags and scrolls are continuous
    /// and already track the hand, so smoothing them would just add lag —
    /// and any of them cancels a glide in flight (see `cancel_camera_glide`).
    pub fn glide_camera_to(&mut self, mut target: OrbitCamera) {
        // A glide moves the camera, never the lens: several callers build
        // their target with `OrbitCamera::from_legacy` or a literal, both of
        // which carry the *default* FoV, and letting that ride in would snap
        // the field of view back to 45 at the end of every fly-to. Whoever
        // wants the lens changed says so separately (`set_camera_fov`).
        target.fov = self.camera.fov;
        // Already there: no glide, or a 200ms pause where nothing happens.
        if target.distance == self.camera.distance
            && target.focus == self.camera.focus
            && self.camera.orientation.angle_to(target.orientation) < 1e-6
        {
            self.camera = target;
            return;
        }
        self.camera_glide = Some(CameraGlide {
            from: self.camera,
            to: target,
            started: Instant::now(),
        });
    }

    /// Drop any glide in flight, leaving the camera exactly where it is. Called
    /// by every gesture that takes the camera by hand: a glide fighting a drag
    /// for the same framing is the worst of both.
    pub fn cancel_camera_glide(&mut self) {
        self.camera_glide = None;
    }

    fn advance_camera_glide(&mut self) {
        let Some(glide) = &self.camera_glide else { return };
        let s = (glide.started.elapsed().as_secs_f32() / CAMERA_GLIDE.as_secs_f32()).min(1.0);
        // Smoothstep: eased at both ends, so it neither jerks off the mark nor
        // slams into it.
        let e = s * s * (3.0 - 2.0 * s);
        self.camera = lerp_camera(&glide.from, &glide.to, e);
        if s >= 1.0 {
            self.camera = glide.to;
            self.camera_glide = None;
        }
        self.invalidate_default_path();
    }

    pub fn set_camera_roll(&mut self, roll: f32) {
        // Same guard as `level()`: within a whisker of straight up or down,
        // yaw and roll are the same control and the chart's split of them is
        // noise. Writing that noise back would scramble the framing — the
        // panel disables the field there, and this catches everyone else.
        if !self.camera.orientation.chart_is_faithful() {
            return;
        }
        let c = self.camera.chart();
        self.camera.orientation = crate::rot::Orientation::from_yaw_pitch_roll(
            c.yaw,
            c.pitch,
            crate::rot::Angle::from_radians(roll),
        );
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
        if self.camera_moving() {
            return None;
        }
        let path = self.camera_path();
        path.playable().then_some(path)
    }

    /// Is the camera flying the path right now?
    pub fn camera_moving(&self) -> bool {
        self.camera_follows && self.playing
    }

    /// Whether the camera is attached to the timeline: following its path on
    /// the clock, whether or not the clock is running. Detached, the camera is
    /// yours — playing and scrubbing leave it where you put it.
    pub fn camera_attached(&self) -> bool {
        self.camera_follows
    }

    /// Attach the camera to the timeline or detach it (the Timeline's camera
    /// row). Not an edit: how you are watching, like orbiting. Attaching the
    /// default orbit starts it from where the camera is now, as playing does.
    pub fn set_camera_attached(&mut self, attached: bool) {
        if attached == self.camera_follows {
            return;
        }
        if attached {
            if !self.camera_path().playable() {
                return;
            }
            if self.path_is_default() {
                self.invalidate_default_path();
                self.refresh_default_path();
                self.camera_epoch = self.time;
            }
            self.camera_lap = self.camera_local().floor() as i64;
        }
        self.camera_follows = attached;
        log::info!("Camera: {}", if attached { "attached to the timeline" } else { "detached" });
    }

    /// Take the camera off the path, because the framing is being set by hand.
    /// The viewport drags do this inline; the Camera window's numeric fields
    /// call it, since typing a distance is the same intent as dragging one.
    pub fn stop_camera_motion(&mut self) {
        self.stop_following();
    }

    /// The camera leaves the path. In a scene with tracks the clock keeps
    /// running, so an animation can be orbited while it plays; in one without,
    /// the clock was only ever the camera's, and stops with it.
    fn stop_following(&mut self) {
        self.camera_follows = false;
        if self.scene.tracks.is_empty() {
            self.playing = false;
        }
    }

    /// Send the playhead back to the start of the path, and the zoom depth
    /// counters with it — what a *new document* should get.
    ///
    /// Called by both ways a whole scene arrives (`load_scene_file`,
    /// `adopt_scene`). Neither used to touch the playhead, so opening a scene
    /// while the camera was flying dropped you two thirds of the way into the
    /// new scene's path, and on a zoom loop at whatever depth the *previous*
    /// scene's counter had reached.
    ///
    /// Playing-or-stopped is deliberately kept: that's a transport state the
    /// person set, and a scene that stopped the camera dead on load would be
    /// its own annoyance. Only the position is reset.
    fn rewind_camera_playhead(&mut self) {
        self.time = 0.0;
        self.camera_epoch = 0.0;
        self.camera_lap = 0;
        self.scene_generation += 1;
        self.zoom_level = 0;
        self.zoom_turns = 0;
        self.zoom_turns_base = 0;
        self.zoom_turns_drawn = 0;
        self.pending_rewrap = 0;
    }

    /// Start / stop the camera flying the path (O, Z, and the toolbar's
    /// transport button). The one motion control there is.
    pub fn toggle_camera_motion(&mut self) {
        if self.camera_moving() {
            self.camera_follows = false;
            self.playing = false;
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
        // Play resumes from the playhead — the timeline's clock is a place,
        // not a stopwatch — except from the very end of a one-shot, which
        // starts over. The default orbit always starts from where you are.
        if self.timeline_end().is_some_and(|end| self.time >= end - 1e-6) {
            self.time = 0.0;
            self.camera_epoch = 0.0;
        }
        self.enter_loop_region();
        if self.path_is_default() {
            self.camera_epoch = self.time;
        }
        self.camera_lap = self.camera_local().floor() as i64;
        self.camera_follows = true;
        self.playing = true;
    }

    /// Where the camera is in its own laps: the clock, from the camera's
    /// epoch and its first key, in laps. The whole part counts laps; the
    /// fraction is what `CameraPath::sample` takes.
    fn camera_local(&self) -> f64 {
        let p = self.camera_path();
        (self.time - self.camera_epoch - p.start() as f64) / p.duration() as f64
    }

    /// Append the current camera framing as a key of this scene's own camera
    /// track (Y), [`KEY_SPACING`] after the last one, and put the playhead on
    /// it. Not an edit to the default orbit — the default is what you get
    /// *until* the scene has two keys, and this is how it gets them. The
    /// Timeline's "Key the camera at the playhead" is the other way in, for a
    /// key at a chosen moment.
    ///
    /// [`KEY_SPACING`]: crate::path::KEY_SPACING
    pub fn add_path_key(&mut self) {
        let was_default = self.path_is_default();
        let before = self.edit_snapshot();
        let key = crate::path::PathKey::from_camera(&self.camera);
        let path = self
            .scene
            .camera_path
            .get_or_insert_with(|| {
                crate::path::CameraPath::new(Vec::new(), crate::path::Loop::Once)
            });
        let t = path.times_or_spacing_after_last();
        path.key_at(t, key);
        self.time = t as f64;
        let path = self.scene.camera_path.as_ref().expect("just keyed");
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
        self.commit_edit("Add path keypoint", None, before);
    }

    /// Key the camera at the playhead (the Timeline): this framing — or a
    /// saved view's, with `view` — becomes the camera's view at this moment,
    /// replacing a key already there. A view at a keyframe point.
    pub fn key_camera_at_playhead(&mut self, view: Option<&std::path::Path>) {
        self.key_camera_at(self.time, view);
    }

    /// Key the camera at `t` seconds: the framing on screen, or a saved
    /// view's. The Timeline's right-click on the camera row.
    pub fn key_camera_at(&mut self, t: f64, view: Option<&std::path::Path>) {
        let framing = match view {
            None => crate::path::PathKey::from_camera(&self.camera),
            Some(path) => match View::load(path) {
                Ok(v) => crate::path::PathKey::from_camera(&OrbitCamera::from_legacy(
                    Vec3::from(v.focus),
                    Vec3::from(v.offset),
                    v.distance,
                    v.rotation,
                    v.pitch,
                    v.roll,
                )),
                Err(e) => {
                    self.set_notice(format!("Couldn't key that view: {e}"), Tone::Error);
                    return;
                }
            },
        };
        let was_default = self.path_is_default();
        let before = self.edit_snapshot();
        let t = t.max(0.0) as f32;
        let path = self
            .scene
            .camera_path
            .get_or_insert_with(|| crate::path::CameraPath::new(Vec::new(), crate::path::Loop::Once));
        let i = path.key_at(t, framing);
        self.ui_state.timeline_camera_key = Some(i);
        self.after_path_edit(was_default);
        self.commit_edit(format!("Key the camera at {t:.2}s"), None, before);
    }

    /// Move camera key `k` to `t` seconds (dragging its diamond). Coalesced
    /// like any drag; never reorders the keys.
    pub fn set_camera_key_time(&mut self, k: usize, t: f64) {
        let Some(p) = self.scene.camera_path.as_ref() else { return };
        if k >= p.keys.len() || (p.time(k) as f64 - t).abs() < 1e-4 {
            return;
        }
        let before = self.edit_snapshot();
        if let Some(p) = self.scene.camera_path.as_mut() {
            p.set_key_time(k, t as f32);
        }
        self.commit_edit("Retime camera key", Some(&format!("camera_key_time_{k}")), before);
    }

    /// Put camera key `k`'s view back to the current framing, at its time.
    pub fn rekey_camera(&mut self, k: usize) {
        let Some(p) = self.scene.camera_path.as_ref() else { return };
        if k >= p.keys.len() {
            return;
        }
        let before = self.edit_snapshot();
        let framing = crate::path::PathKey::from_camera(&self.camera);
        if let Some(p) = self.scene.camera_path.as_mut() {
            p.keys[k] = framing;
        }
        self.commit_edit(format!("Re-key camera key {}", k + 1), None, before);
    }

    /// A looping camera's lap, in seconds (the Timeline): how long after its
    /// first key it is back there.
    pub fn set_camera_period(&mut self, seconds: f32) {
        let Some(p) = self.scene.camera_path.as_ref() else { return };
        let span = p.time(p.keys.len().saturating_sub(1)) - p.start();
        let seconds = seconds.max(span + 0.05);
        if !p.loops.closes() || (p.lap() - seconds).abs() < 1e-4 {
            return;
        }
        let before = self.edit_snapshot();
        if let Some(p) = self.scene.camera_path.as_mut() {
            p.period = Some(seconds);
        }
        self.commit_edit("Camera loop period", Some("camera_period"), before);
    }

    /// Remove the last path keypoint (Shift+Y)
    pub fn remove_path_key(&mut self) {
        if self.scene.camera_path.is_none() {
            log::warn!("Nothing to remove — this scene is on the default orbit");
            return;
        }
        let was_default = self.path_is_default();
        let before = self.edit_snapshot();
        if let Some(path) = &mut self.scene.camera_path {
            path.remove_key(path.keys.len().saturating_sub(1));
        }
        self.after_path_edit(was_default);
        self.commit_edit("Remove path keypoint", None, before);
    }

    // A `toggle_path_closed` used to live here, on Ctrl+Y. It's gone: Ctrl+Y is
    // Redo on Windows and so in an Apophysis refugee's fingers, and a keystroke
    // that could only reach one of the four loop kinds — and couldn't be undone
    // — was the worst possible thing for a mistaken redo to land on. The Camera
    // window's four-way radio does the whole job, visibly.

    /// How the path loops, as the Camera window's radio names it.
    pub fn path_loop(&self) -> crate::path::Loop {
        self.camera_path().loops
    }

    /// The path's zoom loop, if it's on one
    pub fn path_zoom_loop(&self) -> Option<crate::path::ZoomLoop> {
        self.camera_path().loops.zoom()
    }

    /// Put the path on one of the four loops.
    ///
    /// One setter for all four, because they are one choice: it is no longer
    /// possible to ask for two loops at once, or to turn one off and leave the
    /// path on nothing. Choosing `Zoom` keeps whatever period count the path
    /// already had, so flipping away and back doesn't silently reset it.
    pub fn set_path_loop(&mut self, kind: LoopKind) {
        let periods = self.path_zoom_loop().map_or(1, |z| z.periods);
        let loops = match kind {
            LoopKind::Once => crate::path::Loop::Once,
            LoopKind::PingPong => crate::path::Loop::PingPong,
            LoopKind::Closed => crate::path::Loop::Closed,
            LoopKind::Zoom => {
                // The similarity comes from the live renormalizing map, so a
                // scene without one can't have this — and shouldn't silently
                // get a path that claims to loop and doesn't.
                let Some(zoom) = self.point_compute.zoom else {
                    log::warn!(
                        "A zoom loop closes under the scene's scale symmetry, and this scene \
                         has none. The Render window's infinite-zoom section lists every \
                         transform and says which of them could carry one."
                    );
                    return;
                };
                crate::path::Loop::Zoom(zoom.loop_similarity(periods))
            }
        };
        if self.camera_path().loops == loops {
            return;
        }
        // An `ease` that only says what the loop it's leaving wanted anyway is
        // not a choice, and carrying it across is wrong in both directions.
        // The default orbit pins `ease = false` because a closed loop must not
        // stall at its seam; switch that to a ping-pong and the same `false`
        // takes away the deceleration its turnarounds need, which is the
        // judder `Loop::eases_by_default` exists to prevent. A deliberate ease
        // — one that already differs from the old loop's default — is kept.
        let old = self.camera_path().loops;
        let inherited = self.camera_path().ease == Some(old.eases_by_default());
        let was_default = self.path_is_default();
        let before = self.edit_snapshot();
        let path = self.author_path();
        path.loops = loops;
        if inherited {
            path.ease = None;
        }
        log::info!("Camera path: {}", kind.label());
        self.after_path_edit(was_default);
        self.commit_edit(format!("Path loop: {}", kind.label()), None, before);
    }

    /// Zoom periods descended per loop. Only meaningful on a zoom loop, and
    /// silently ignored off one — the panel greys the control there.
    pub fn set_path_zoom_periods(&mut self, periods: u32) {
        let periods = periods.clamp(1, 64);
        let Some(zoom) = self.point_compute.zoom else { return };
        if self.camera_path().loops.zoom().is_none() {
            return;
        }
        let was_default = self.path_is_default();
        let before = self.edit_snapshot();
        self.author_path().loops = crate::path::Loop::Zoom(zoom.loop_similarity(periods));
        log::info!("Camera path: zoom loop, {} period(s) per loop", periods);
        self.after_path_edit(was_default);
        // Draggable, so it coalesces the way the inspector's fields do — one
        // entry for the gesture, not one per pixel.
        self.commit_edit("Zoom periods per loop", Some("path_zoom_periods"), before);
    }

    /// Remove one path keypoint by index (Camera window row ✕). The keyboard
    /// path (Shift+Y) only ever pops the last one.
    pub fn remove_path_key_at(&mut self, idx: usize) {
        if !self.scene.camera_path.as_ref().is_some_and(|p| idx < p.keys.len()) {
            return;
        }
        let was_default = self.path_is_default();
        let before = self.edit_snapshot();
        if let Some(path) = &mut self.scene.camera_path {
            path.remove_key(idx);
        }
        log::info!("Camera path keypoint {} removed", idx);
        self.after_path_edit(was_default);
        self.commit_edit(format!("Remove path keypoint {}", idx + 1), None, before);
    }

    /// Housekeeping after the scene's own keypoints change. `was_default` is
    /// [`path_is_default`](Self::path_is_default) from *before* the edit.
    ///
    /// An empty key list isn't a path, so it goes back to `None`: nothing
    /// writes an empty camera track, and the default orbit is what the
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
            self.stop_following();
        }
    }

    /// Set the path's playback duration in seconds, stretching every key's
    /// time (and a loop's period) with it.
    pub fn set_path_seconds(&mut self, seconds: f32) {
        let seconds = seconds.max(0.1);
        if (self.camera_path().duration() - seconds).abs() < 1e-4 {
            return;
        }
        let before = self.edit_snapshot();
        self.author_path().retime_to(seconds);
        // The draggy one of the six: coalesced, so a drag across the field is
        // one undo step rather than sixty.
        self.commit_edit("Path duration", Some("path_seconds"), before);
    }

    /// Discard the scene's own keypoints and go back to the default orbit —
    /// the counterpart to the first edit that authored them.
    pub fn reset_path_to_default(&mut self) {
        if self.scene.camera_path.is_none() {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.camera_path = None;
        self.stop_following();
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
        let mut out: Vec<(f64, String, std::path::PathBuf)> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .filter_map(|p| {
                let stem = p.file_stem()?.to_str()?.to_string();
                if !stem.starts_with(&prefix) {
                    return None;
                }
                // The scene's own slug is on every name in this list, in a
                // panel that belongs to that scene — so what's shown is the
                // part that differs. The full path is a hover away.
                let name = stem[prefix.len()..].to_string();
                let age = std::fs::metadata(&p)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0.0, |d| d.as_secs_f64());
                Some((age, name, p))
            })
            .collect();
        // Newest first, by the file's own mtime. This used to be a reverse
        // lexical sort on the name, which was newest-first only because the
        // names were unix timestamps — `rename_saved_view` means they aren't
        // any more, and a renamed view would otherwise sort itself to wherever
        // its first letter fell.
        out.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        out.into_iter().map(|(_, name, p)| (name, p)).collect()
    }

    /// Give a saved view a name of its own. `name` is the part after the
    /// scene's slug — what the Camera panel shows and edits.
    ///
    /// Returns an error string fit to show, and changes nothing when it does:
    /// a rename that half-happens loses somebody's framing.
    pub fn rename_saved_view(
        &mut self,
        path: &std::path::Path,
        name: &str,
    ) -> Result<(), String> {
        // Same slug rule as `scene_slug`: this becomes a filename, and it has
        // to keep the `<slug>-` prefix that `scan_saved_views` filters on or
        // the view vanishes out of its own scene's list.
        let clean: String = name
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        let clean = clean.trim_matches('-').to_string();
        if clean.is_empty() {
            return Err("A view needs a name".to_string());
        }
        let target =
            std::path::PathBuf::from(format!("views/{}-{}.toml", self.scene_slug(), clean));
        if target == path {
            return Ok(());
        }
        if target.exists() {
            return Err(format!("{} already exists", target.display()));
        }
        std::fs::rename(path, &target).map_err(|e| format!("{}: {}", path.display(), e))?;
        log::info!("View renamed: {} -> {}", path.display(), target.display());
        self.invalidate_saved_views();
        Ok(())
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
    /// The grade a saved view should carry, or `None` for "don't write one".
    ///
    /// Two ways to be nothing worth saving: the points renderer, which has no
    /// tonemap for a grade to act on, and a neutral grade, which is the
    /// tonemap this app had before grading existed. Both resolve back to
    /// neutral on load, so omitting them loses nothing and keeps a view file
    /// down to what someone actually chose.
    fn saved_grade(&self) -> Option<crate::gpu::points::splat::Grade> {
        grade_for_view(self.render_mode, self.grade)
    }

    fn current_view(&self) -> View {
        View {
            scene: self.scene_path.clone(),
            rotation: self.camera.chart().yaw.radians(),
            pitch: self.camera.chart().pitch.radians(),
            roll: self.camera.chart().roll.radians(),
            distance: self.camera.distance,
            focus: self.camera.focus.to_array(),
            offset: [0.0; 3],
            point_size: self.point_size,
            // A view reproduces a frame, and the lens is part of the frame.
            // Always written, so `--view` renders what was on screen even on
            // a machine whose owner prefers a different one.
            fov: Some(self.camera.fov.degrees()),
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
                // The mode is saved; the opacity is session state and is not.
                RenderMode::Volume => Some("volume".to_string()),
            },
            exposure: match self.render_mode {
                RenderMode::Points => None,
                RenderMode::Splat | RenderMode::Volume => Some(self.exposure),
            },
            // Only under splat, and only when the grade actually says
            // something. The points renderer has no tonemap to grade, and
            // writing a neutral grade explicitly would put three lines in every
            // view file to describe the absence of one — `View::grade` already
            // resolves a missing field to neutral.
            gamma: self.saved_grade().map(|g| g.gamma),
            gamma_threshold: self.saved_grade().map(|g| g.gamma_threshold),
            vibrancy: self.saved_grade().map(|g| g.vibrancy),
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
        // **Read the drawn size before the framing moves.** Under an infinite
        // zoom the authored `point_size` is a size *at the scene's own
        // framing* — `rendered_point_size` scales it by how far the eye has
        // travelled from there — and the next line makes that framing this
        // one. Copying the authored number across would leave a scene that
        // draws dots at a different size than the one you just saved: Ctrl+S
        // deep in a band visibly doubled them, because the reference halved
        // and the size didn't move with it.
        //
        // Without a zoom this is exactly `self.point_size` and the whole thing
        // is a no-op, which is why it isn't guarded: `rendered_point_size`
        // already knows when the question doesn't apply.
        let drawn = self.rendered_point_size();
        self.scene.set_camera(&self.camera);
        // Deliberately not through `set_point_size`: that clamps to the
        // slider's range, and a save at the deep end of a band legitimately
        // lands under it (the factor bottoms out at the zoom's own scale).
        // Clamping here would put the jump back, one period further down.
        self.point_size = drawn;
        self.scene.point_size = drawn;
        self.scene.color_falloff = self.color_falloff;
        self.scene.color_contrast = self.color_contrast;
        self.scene.haze = self.haze_amount;
        self.scene.exposure = self.exposure;
    }

    pub fn save_scene(&mut self) {
        self.sync_scene_for_save();
        let path = self.scene_path.clone().unwrap_or_else(|| match self.png_stem() {
            // Opened from a render: named after it, since that's the name the
            // person knows it by — but never over a scene that already exists.
            Some(stem) => {
                let named = format!("scenes/{}.toml", stem);
                if Path::new(&named).exists() {
                    format!("scenes/{}-{}.toml", stem, unix_timestamp())
                } else {
                    named
                }
            }
            None => format!("scenes/untitled-{}.toml", unix_timestamp()),
        });
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
        self.scene_png = None;
        self.mark_saved();
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
            None => format!("scenes/{}.toml", self.png_stem().unwrap_or_else(|| self.scene_slug())),
        }
    }

    /// The file stem of the render this scene was opened from, if it was.
    fn png_stem(&self) -> Option<String> {
        self.scene_png
            .as_deref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
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
        if self.previewing().is_some() {
            // It would write the option over the scene's file. Keep, in the
            // Collaborate window, is how an option is saved.
            let e = "previewing an option: leave the preview (Esc) to save the scene, or Keep the option";
            self.set_notice(format!("Not saved: {e}"), Tone::Error);
            self.last_save_error = Some(e.to_string());
            return;
        }
        match self.scene.save(path) {
            Ok(()) => {
                log::info!("Scene saved to {}", path);
                self.scene_path = Some(path.to_string());
                self.scene_png = None;
                self.mark_saved();
                self.last_save_error = None;
            }
            Err(e) => {
                log::error!("{}", e);
                // Kept, not just logged. A save can fail for ordinary reasons —
                // a read-only file, a full disk, a directory that went away
                // with a removable drive — and the unsaved-changes prompt has
                // to be able to say so rather than quitting on the assumption
                // that Save worked. See `ui::confirm::draw`.
                self.last_save_error = Some(e.to_string());
            }
        }
    }

    /// Why the last `save_scene` failed, if it did. Cleared by a save that
    /// works.
    pub fn last_save_error(&self) -> Option<&str> {
        self.last_save_error.as_deref()
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
            exposure: self.exposure,
        }
    }

    /// The single choke point for history-tracked edits. `coalesce_key`
    /// lets rapid same-key commits (held weight/color keys, drag-scroll
    /// bursts) merge into one entry instead of flooding the stack.
    pub fn commit_edit(&mut self, label: impl Into<String>, coalesce_key: Option<&str>, before: EditSnapshot) {
        // Record keys what the edit changed, before it is committed, so the
        // edit and its keys are one undo step.
        if self.record_armed {
            self.autokey(&before);
        }
        // No dirty flag to raise: committing moves the history to a state the
        // save point doesn't name, which is what `is_dirty` asks about.
        self.history.commit(label, coalesce_key, before, Instant::now());
    }

    /// Record: key every value an edit changed at the playhead, making the
    /// track where there isn't one (Blender's auto-key, a DAW's armed
    /// automation lane). An edit to the timeline itself keys nothing.
    ///
    /// The changes are `scene_diff`'s, so they are `-S` paths, which is what
    /// a track names. A vector (a position, a scale, a colour) keys all three
    /// of its parts whichever moved, so a map picked up and moved is pinned
    /// there on every axis; a rotation keys the whole pose. A value moved by a
    /// rate or a cycle is left to move: its authored value is what was edited.
    fn autokey(&mut self, before: &EditSnapshot) {
        use crate::track::{KeyValue, Source};
        let mut was = before.scene.clone();
        was.point_size = before.point_size;
        was.haze = before.haze_amount;
        was.exposure = before.exposure;
        was.color_contrast = before.color_contrast;
        was.color_falloff = before.color_falloff;
        let now = self.scene_with_live_values();
        if was.tracks != now.tracks {
            return;
        }
        let Some(keys) = autokey_targets(&was, &now, &self.scene.tracks) else { return };
        let t = self.time;
        for (target, value) in keys {
            match self.scene.tracks.iter().position(|tr| tr.target == target) {
                Some(i) => {
                    let Source::Keys { keys, integral, .. } = &mut self.scene.tracks[i].source else { continue };
                    let value = match value {
                        KeyValue::Number(x) if *integral => KeyValue::Number(x.round()),
                        v => v,
                    };
                    if keys.iter().any(|k| std::mem::discriminant(&k.value) != std::mem::discriminant(&value)) {
                        continue;
                    }
                    let key = crate::track::Key { t, value };
                    match keys.iter().position(|k| (k.t - t).abs() < 1e-3) {
                        Some(j) => keys[j] = key,
                        None => {
                            let j = keys.partition_point(|k| k.t < t);
                            keys.insert(j, key);
                        }
                    }
                }
                None => {
                    let start = match &value {
                        KeyValue::Number(x) => toml::Value::Float(*x),
                        KeyValue::Rotation(_) => toml::Value::Array(vec![toml::Value::Float(0.0); 3]),
                        KeyValue::Other(v) => v.clone(),
                    };
                    // Only paths a track can take: the same check `-S` makes.
                    let Ok(mut track) = crate::track::one_key(&target, t, start) else { continue };
                    if crate::set::apply_to_doc(
                        &mut match now.to_toml_string().ok().and_then(|s| s.parse::<toml_edit::DocumentMut>().ok()) {
                            Some(d) => d,
                            None => continue,
                        },
                        &track.spec_at(t, None),
                    )
                    .is_err()
                    {
                        continue;
                    }
                    if let (KeyValue::Rotation(_), Source::Keys { keys, .. }) = (&value, &mut track.source) {
                        keys[0].value = value.clone();
                    }
                    self.scene.tracks.push(track);
                }
            }
        }
    }

    /// While record is armed and the clock is still, what the inspector and
    /// the gizmos edit is the scene *at the playhead*: every keyed value is
    /// put into the authored scene as the frame has it, so an edit starts
    /// from what is on screen and the key it makes is the value you set.
    ///
    /// Not an edit and not in the history: a keyed value's authored value is
    /// never seen (the keys override it), so the picture doesn't change. It
    /// is written by Ctrl+S, which only changes what a muted or deleted track
    /// would leave behind. Values the window keeps live (point size, haze,
    /// exposure) are left alone; they don't go through the scene.
    fn sync_record_base(&mut self) {
        if !self.record_armed || self.playing || self.has_active_drag() {
            return;
        }
        let at = (self.time.to_bits(), self.history.top_serial(), self.scene_generation);
        if self.record_synced == Some(at) {
            return;
        }
        self.record_synced = Some(at);
        let specs: Vec<String> = self
            .scene
            .tracks
            .iter()
            .filter(|tr| !tr.mute && matches!(tr.source, crate::track::Source::Keys { .. }))
            .filter(|tr| tr.class() != crate::track::Class::View)
            .map(|tr| tr.spec_at(self.time, None))
            .collect();
        if specs.is_empty() {
            return;
        }
        let Ok(text) = self.scene.to_toml_string() else { return };
        let Ok(mut doc) = text.parse::<toml_edit::DocumentMut>() else { return };
        let mut changed = false;
        for spec in &specs {
            if let Ok(applied) = crate::set::apply_to_doc(&mut doc, spec) {
                let new = spec.split_once('=').map(|(_, v)| v.trim());
                changed |= applied.from.as_deref().map(str::trim) != new;
            }
        }
        if !changed {
            return;
        }
        let dir = self.scene_path.as_deref().map(Path::new).and_then(Path::parent).unwrap_or(Path::new("."));
        let Ok(mut synced) = Scene::from_toml_str(&doc.to_string(), dir) else { return };
        synced.point_count = self.scene.point_count;
        synced.point_count_defaulted = self.scene.point_count_defaulted;
        if synced.transforms.len() != self.scene.transforms.len() {
            return;
        }
        self.scene = synced;
        self.bump_matrix_generation();
    }

    /// Record that the scene as it stands right now is what's on disk. Called
    /// by every successful write, and by opening a scene (where both sides are
    /// `None`: a fresh history at a fresh file).
    fn mark_saved(&mut self) {
        self.saved_serial = self.history.top_serial();
    }

    /// Whether the scene has edits not yet on disk. Drives the title bar's
    /// dirty marker and every "are you sure" prompt.
    ///
    /// Undo and redo need no special case here, and deliberately don't have
    /// one: they move the history, and this reads where the history is.
    pub fn is_dirty(&self) -> bool {
        // Previewing an option sets the live scene's history aside; the
        // question is still about the live scene.
        self.live_dirty().unwrap_or(self.history.top_serial() != self.saved_serial)
    }

    // === Losing work: the prompts that stand in the way (see ui::confirm) ===

    /// Ask to leave. Prompts first if there is unsaved work; otherwise the
    /// event loop is told to exit on its next pass.
    pub fn request_quit(&mut self) {
        if self.is_dirty() {
            self.pending_action = Some(crate::ui::confirm::Pending::Quit);
        } else {
            self.exit_requested = true;
        }
    }

    /// Ask to open a scene. Same shape as `request_quit`: opening replaces the
    /// scene *and* clears the undo stack, so it is exactly as destructive.
    pub fn request_load_scene(&mut self, path: &Path) {
        // Asking whether to keep your edits, and then failing to open the
        // file anyway, is a question about nothing. Find out first.
        if self.is_dirty() {
            if let Err(e) = Scene::load(path) {
                self.set_notice(format!("Couldn't open {}: {}", path.display(), e), Tone::Error);
                return;
            }
        }
        if self.is_dirty() {
            self.pending_action = Some(crate::ui::confirm::Pending::Load(path.to_path_buf()));
        } else {
            self.load_scene_file(path);
        }
    }

    /// Carry out whatever the unsaved-changes prompt was standing in front of.
    /// Called once the person has chosen Save or Discard.
    pub fn proceed_with_pending(&mut self) {
        match self.pending_action.take() {
            Some(crate::ui::confirm::Pending::Quit) => self.exit_requested = true,
            Some(crate::ui::confirm::Pending::Load(path)) => self.load_scene_file(&path),
            None => {}
        }
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
        self.exposure = snap.exposure;
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

    /// Whether a roll drag is in progress — `ui::gizmo_ring` draws the ring
    /// solid while it is.
    pub fn rolling(&self) -> bool {
        matches!(self.drag, Drag::Gizmo { mode: GizmoDragMode::Roll { .. }, .. })
    }

    /// Whether the pointer is over the roll ring. The ring is painted in screen
    /// space rather than built as gizmo geometry, so it can't light up through
    /// the highlight uniform the way the tetrahedron's parts do.
    pub fn hovering_roll(&self) -> bool {
        matches!(self.hovered, Some(hit) if hit.part == crate::pick::GizmoPart::Roll)
    }

    /// Which transform the pointer is over, if any — whichever part of its
    /// gizmo is under the cursor. `src/ui/labels.rs` uses it to promote that
    /// transform's name to a readable backdrop.
    pub fn hovered_transform(&self) -> Option<usize> {
        self.hovered.map(|hit| hit.transform)
    }

    /// Select a transform from the Transforms window's list (or clear the
    /// selection). Gizmo clicks and Up/Down go through `selected_transform`
    /// directly; this is the same field, so both stay in sync automatically.
    pub fn select_transform(&mut self, idx: Option<usize>) {
        self.selected_transform = idx;
        if let Some(i) = idx {
            if let Some(t) = self.scene.transforms.get(i) {
                self.current_layer = t.layer as usize;
            }
            self.leave_subscene_gizmo();
        }
    }

    // === Subscenes (see src/subscene.rs) ===

    /// The subscene selected as a whole, if any.
    pub fn selected_subscene(&self) -> Option<usize> {
        self.selected_subscene.filter(|&k| k >= 1 && k <= self.scene.subscenes.len())
    }

    /// The layer the Transforms window is listing.
    pub fn current_layer(&self) -> usize {
        self.current_layer.min(self.scene.layer_count() - 1)
    }

    /// Make a layer current, and — for a subscene — select it as a whole, so
    /// its gizmo moves the entire attractor. Layer 0 has no placement (it is
    /// the scene's own frame), so choosing it just lists its maps.
    pub fn select_layer(&mut self, k: usize) {
        if k >= self.scene.layer_count() {
            return;
        }
        self.current_layer = k;
        self.selected_transform = None;
        let want = (k >= 1).then_some(k);
        if self.selected_subscene != want {
            self.selected_subscene = want;
            self.rebuild_gizmos();
        }
    }

    /// Keep the current layer but stop selecting it as a whole.
    pub fn leave_subscene_selection(&mut self) {
        self.leave_subscene_gizmo();
    }

    /// Back to the maps' gizmos, if a subscene's was showing.
    fn leave_subscene_gizmo(&mut self) {
        if self.selected_subscene.take().is_some() {
            self.rebuild_gizmos();
        }
    }

    /// Every gizmo the viewport draws and picks, in scene space: each map's
    /// matrix carried by its subscene's placement — or, with a subscene
    /// selected as a whole, that subscene's placement alone. Indices into this
    /// list are what `pick`, the highlight and a `Drag::Gizmo` talk about.
    pub fn gizmo_specs(&self) -> Vec<TransformSpec> {
        if let Some(k) = self.selected_subscene() {
            let mut spec = self.scene.transforms[0].clone();
            spec.matrix = self.scene.layer_placement(k);
            spec.post_affine = Mat4::IDENTITY;
            spec.symmetry = None;
            return vec![spec];
        }
        self.scene.world_transforms()
    }

    fn gizmo_matrices(&self) -> Vec<Mat4> {
        self.gizmo_specs().into_iter().map(|t| t.matrix).collect()
    }

    /// Which gizmo is selected: the one subscene gizmo, or the selected map.
    pub fn gizmo_selection(&self) -> Option<usize> {
        match self.selected_subscene() {
            Some(_) => Some(0),
            None => self.selected_transform,
        }
    }

    /// Per-gizmo enabled state, parallel to `gizmo_specs`.
    fn gizmo_enabled(&self) -> Vec<bool> {
        match self.selected_subscene() {
            Some(_) => vec![true],
            None => self.transform_enabled.clone(),
        }
    }

    /// Name labels for the gizmos: `(scene-space matrix, name, enabled)`.
    pub fn gizmo_labels(&self) -> Vec<(Mat4, String, bool)> {
        if let Some(k) = self.selected_subscene() {
            return vec![(self.scene.layer_placement(k), self.scene.layer_name(k).to_string(), true)];
        }
        self.gizmo_specs()
            .into_iter()
            .enumerate()
            .map(|(i, spec)| {
                let name = self
                    .scene
                    .transform_names
                    .get(i)
                    .and_then(|n| n.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("T{}", i));
                (spec.matrix, name, self.is_transform_enabled(i))
            })
            .collect()
    }

    /// Rebuild the gizmo renderer for the current gizmo list — its instance
    /// count is fixed at construction, and a subscene's gizmo is one instance
    /// where the maps are many.
    fn rebuild_gizmos(&mut self) {
        self.gizmo_renderer = GizmoRenderer::new(
            &self.gpu.device,
            self.gpu.format,
            self.overlay.samples(),
            &self.gizmo_specs(),
        );
        self.gizmo_renderer.update_alpha(&self.gpu.queue, &self.gizmo_enabled());
        self.hovered = None;
        self.indicator_key = (None, None);
    }

    /// Show or hide a layer. The last visible layer can't be hidden — a scene
    /// with nothing to draw is a scene that looks broken.
    pub fn set_layer_visible(&mut self, k: usize, visible: bool) {
        if k >= self.scene.layer_count() || self.scene.layer_visible(k) == visible {
            return;
        }
        if !visible
            && (0..self.scene.layer_count())
                .filter(|&j| j != k && self.scene.layer_visible(j) && self.scene.layer_sizes()[j] > 0)
                .count()
                == 0
        {
            log::warn!("{} is the only visible layer", self.scene.layer_name(k));
            return;
        }
        let before = self.edit_snapshot();
        match k {
            0 => self.scene.root_visible = visible,
            k => self.scene.subscenes[k - 1].visible = visible,
        }
        self.sync_transforms_to_gpu();
        let verb = if visible { "Show" } else { "Hide" };
        self.commit_edit(format!("{} {}", verb, self.scene.layer_name(k)), None, before);
    }

    /// Whether hiding layer `k` is allowed (it isn't the last visible one).
    pub fn can_hide_layer(&self, k: usize) -> bool {
        (0..self.scene.layer_count())
            .any(|j| j != k && self.scene.layer_visible(j) && self.scene.layer_sizes()[j] > 0)
    }

    /// Solo a layer: hide every other. Alt-clicking the eye again brings
    /// everything back, as it does for a transform.
    pub fn solo_layer(&mut self, k: usize) {
        if k >= self.scene.layer_count() {
            return;
        }
        let before = self.edit_snapshot();
        let already = (0..self.scene.layer_count()).all(|j| self.scene.layer_visible(j) == (j == k));
        let show = |j: usize| already || j == k;
        self.scene.root_visible = show(0);
        for (i, sub) in self.scene.subscenes.iter_mut().enumerate() {
            sub.visible = show(i + 1);
        }
        self.sync_transforms_to_gpu();
        let label = if already { "Show all".to_string() } else { format!("Show only {}", self.scene.layer_name(k)) };
        self.commit_edit(label, None, before);
    }

    /// Set a layer's share of the point budget. Coalesces like a weight drag.
    pub fn set_layer_share(&mut self, k: usize, share: f32) {
        if k >= self.scene.layer_count() {
            return;
        }
        let before = self.edit_snapshot();
        let share = share.clamp(0.0, 100.0);
        match k {
            0 => self.scene.root_share = share,
            k => self.scene.subscenes[k - 1].share = share,
        }
        self.sync_transforms_to_gpu();
        self.commit_edit(
            format!("Share {}", self.scene.layer_name(k)),
            Some(&format!("share:L{}", k)),
            before,
        );
    }

    /// Move, turn or resize a subscene as a whole.
    pub fn set_subscene_placement(&mut self, k: usize, placement: crate::subscene::Placement) {
        if k == 0 || k > self.scene.subscenes.len() {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.subscenes[k - 1].placement = placement;
        self.bump_matrix_generation();
        self.sync_transforms_to_gpu();
        self.commit_edit(
            format!("Place {}", self.scene.layer_name(k)),
            Some(&format!("place:L{}", k)),
            before,
        );
    }

    pub fn rename_subscene(&mut self, k: usize, name: &str) {
        let name = name.trim();
        if k == 0 || k > self.scene.subscenes.len() || name.is_empty() {
            return;
        }
        if (1..=self.scene.subscenes.len()).any(|j| j != k && self.scene.layer_name(j) == name) {
            log::warn!("a subscene is already called '{}'", name);
            return;
        }
        let before = self.edit_snapshot();
        self.scene.subscenes[k - 1].name = name.to_string();
        self.commit_edit("Rename subscene", Some(&format!("rename:L{}", k)), before);
    }

    /// Set a subscene's colour source (its own `transforms | palette | mix`).
    pub fn set_subscene_color_mode(&mut self, k: usize, mode: crate::scene::ColorMode) {
        if k == 0 || k > self.scene.subscenes.len() {
            return;
        }
        let before = self.edit_snapshot();
        let colors = self.scene.layer_colors(k);
        let c = &mut self.scene.subscenes[k - 1].colour;
        if mode == crate::scene::ColorMode::Palette && c.palette.is_none() {
            let mut p = crate::palette::Palette::from_transform_colors(&colors);
            p.name = None;
            c.palette = Some(p);
        }
        c.color_mode = mode;
        self.push_colormap();
        self.commit_edit("Subscene colour", None, before);
    }

    /// Import another scene as a subscene: its maps and colour, placed at the
    /// origin, with an equal share. Recorded as a `file =` reference, so an
    /// unedited import saves as four lines and never copies the artwork.
    pub fn import_subscene(&mut self, path: &Path) -> Result<(), String> {
        if self.scene.zoom.is_some() {
            return Err("Subscenes don't work with infinite zoom yet. Turn infinite zoom off first".into());
        }
        if self.scene.layer_count() >= crate::subscene::MAX_LAYERS {
            return Err(format!("A scene can hold at most {} subscenes", crate::subscene::MAX_LAYERS - 1));
        }
        let sub = Scene::load(path)?;
        if sub.is_composite() {
            return Err(format!("{} has subscenes of its own, and subscenes can't be nested yet", path.display()));
        }
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        // Relative to where this scene lives, which is what `file =` means.
        let here = self
            .scene_path
            .as_deref()
            .and_then(|p| Path::new(p).parent().map(Path::to_path_buf))
            .unwrap_or_else(|| std::path::PathBuf::from("scenes"));
        let file = crate::subscene::relative_path(&abs, &here);
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or("subscene".into());
        let mut name = stem.clone();
        let mut n = 2;
        while (0..self.scene.layer_count()).any(|k| self.scene.layer_name(k) == name) {
            name = format!("{}-{}", stem, n);
            n += 1;
        }
        let before = self.edit_snapshot();
        let layer = self.scene.layer_count() as u16;
        let base = crate::subscene::FileBase {
            maps_toml: crate::scene::maps_toml(&sub.transforms, &sub.transform_names, &sub.colors),
            color_mode: sub.color_mode,
            color_speed: sub.color_speed,
            color_falloff: sub.color_falloff,
            color_contrast: sub.color_contrast,
            palette_toml: sub.palette.as_ref().map(|p| {
                crate::palette::spec::PaletteDef::from_palette(p, sub.color_mode == crate::scene::ColorMode::Palette)
                    .to_table()
                    .to_string()
            }),
        };
        self.scene.subscenes.push(crate::subscene::Subscene {
            name: name.clone(),
            author: Some(sub.author.clone()).filter(|a| a != "Unknown" && !a.is_empty()),
            file: Some(file),
            file_abs: Some(abs),
            base: Some(base),
            source: None,
            visible: true,
            share: 1.0,
            placement: crate::subscene::Placement::IDENTITY,
            colour: crate::subscene::LayerColour {
                color_mode: sub.color_mode,
                palette: sub.palette.clone(),
                color_speed: sub.color_speed,
                color_falloff: sub.color_falloff,
                color_contrast: sub.color_contrast,
                colormap: sub.colormap,
            },
        });
        for t in &sub.transforms {
            let mut t = t.clone();
            t.layer = layer;
            self.scene.transforms.push(t);
        }
        self.scene.transform_names.extend(sub.transform_names.iter().cloned());
        self.scene.colors.extend(sub.colors.iter().copied());
        self.transform_enabled.resize(self.scene.transforms.len(), true);
        self.scene.regenerate_colormap();
        self.bump_matrix_generation();
        self.selected_transform = None;
        self.selected_subscene = Some(layer as usize);
        self.current_layer = layer as usize;
        self.after_scene_shape_change();
        log::info!("Imported {} as subscene '{}'", path.display(), name);
        self.commit_edit(format!("Import {}", name), None, before);
        Ok(())
    }

    /// Scene files that could be imported as a subscene: `scenes/` and the
    /// current scene's own directory, minus the current scene itself.
    pub fn importable_scenes(&self) -> Vec<std::path::PathBuf> {
        let mut dirs = vec![std::path::PathBuf::from("scenes")];
        if let Some(dir) = self.scene_path.as_ref().and_then(|p| Path::new(p).parent()) {
            if !dir.as_os_str().is_empty() {
                dirs.push(dir.to_path_buf());
            }
        }
        let current = self.scene_path.as_ref().and_then(|p| std::fs::canonicalize(p).ok());
        let mut files: Vec<std::path::PathBuf> = dirs
            .iter()
            .filter_map(|d| fs::read_dir(d).ok())
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .filter(|p| std::fs::canonicalize(p).ok() != current)
            .collect();
        files.sort();
        files.dedup_by(|a, b| std::fs::canonicalize(&*a).ok() == std::fs::canonicalize(&*b).ok());
        files
    }

    /// Remove a subscene and its maps. Undoable, like deleting a transform.
    pub fn remove_subscene(&mut self, k: usize) {
        if k == 0 || k > self.scene.subscenes.len() {
            return;
        }
        if !self.can_hide_layer(k) && self.scene.layer_visible(k) {
            log::warn!("{} is the only visible layer", self.scene.layer_name(k));
            return;
        }
        let before = self.edit_snapshot();
        let name = self.scene.layer_name(k).to_string();
        fn keep_where<T>(v: &mut Vec<T>, keep: &[bool]) {
            let mut i = 0;
            v.retain(|_| {
                i += 1;
                keep.get(i - 1).copied().unwrap_or(true)
            });
        }
        let keep: Vec<bool> = self.scene.transforms.iter().map(|t| t.layer as usize != k).collect();
        keep_where(&mut self.scene.transforms, &keep);
        keep_where(&mut self.scene.transform_names, &keep);
        keep_where(&mut self.scene.colors, &keep);
        keep_where(&mut self.transform_enabled, &keep);
        for t in &mut self.scene.transforms {
            if t.layer as usize > k {
                t.layer -= 1;
            }
        }
        self.scene.subscenes.remove(k - 1);
        self.scene.xaos_matrix_layers.retain(|&l| l as usize != k);
        for l in &mut self.scene.xaos_matrix_layers {
            if *l as usize > k {
                *l -= 1;
            }
        }
        self.selected_subscene = None;
        self.selected_transform = None;
        self.current_layer = 0;
        self.scene.regenerate_colormap();
        self.bump_matrix_generation();
        self.after_scene_shape_change();
        self.commit_edit(format!("Remove {}", name), None, before);
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
    /// menu, the toolbar's Undo button).
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
    /// menu, the toolbar's Redo button).
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

    /// Undo `steps` entries in one go (clicking N deep into the History
    /// window's list) as a single pipeline rebuild.
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
        if self.selected_subscene().is_some() {
            return self.hovered.map(|hit| match hit.part {
                GizmoPart::RotEdge(_) => hints::HINT_ROT_EDGE,
                GizmoPart::Roll => hints::HINT_ROLL,
                _ => hints::HINT_SUBSCENE,
            });
        }
        self.hovered.map(|hit| match hit.part {
            // On an unselected transform the dot only selects, so promising a
            // drag would be a lie — and it's the one part of one that's on
            // offer at all.
            GizmoPart::Origin if self.selected_transform != Some(hit.transform) => {
                hints::HINT_SELECT
            }
            GizmoPart::Origin => hints::HINT_ORIGIN,
            GizmoPart::Tip(_) => hints::HINT_TIP,
            GizmoPart::Axis(_) => hints::HINT_AXIS,
            GizmoPart::RotEdge(_) => hints::HINT_ROT_EDGE,
            GizmoPart::Roll => hints::HINT_ROLL,
        })
    }

    /// Strength multiplier for U's random mutation (Mutate window slider).
    /// Part of the dice profile, so it is saved with it.
    pub fn mutate_strength(&self) -> f32 {
        self.prefs.random.mutation.strength
    }

    pub fn set_mutate_strength(&mut self, strength: f32) {
        let mut s = self.prefs.random.clone();
        s.mutation.strength = strength;
        self.set_random_settings(s);
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

    /// One notch of the wheel, or one press of Up/Down: move the camera in by
    /// `steps` (negative is out).
    ///
    /// Under an infinite zoom this is a step along the symmetry itself — see
    /// [`crate::renorm::Renorm::scroll`], which also has the account of what
    /// the plain radial version got wrong. Without one there is no symmetry to
    /// follow and the ordinary dolly is exactly right.
    ///
    /// Everything that steps the framing comes through here, so the two
    /// notions of "deeper" cannot drift apart.
    fn zoom_steps(&mut self, steps: f32) {
        match self.zoom().copied() {
            Some(z) => z.scroll(&mut self.camera, steps),
            None => self.camera.zoom(steps, crate::camera::MIN_ORBIT_DISTANCE),
        }
    }

    pub fn zoom_in(&mut self) {
        self.zoom_steps(1.0);
    }

    pub fn zoom_out(&mut self) {
        self.zoom_steps(-1.0);
    }

    /// Mouse wheel: zoom, always. **Alt**+wheel over a hovered gizmo adjusts
    /// that transform's chaos weight (its selection probability) — the lever
    /// that emphasizes an element without changing structure.
    ///
    /// The weight lever used to be on plain scroll, and it had to move. Scroll
    /// is *the* navigation gesture: people use it continuously and without
    /// looking, and the fractal fully occludes gizmos while they keep taking
    /// input — so zooming through a scene silently edited whatever happened to
    /// pass under the pointer, which is exactly what `todo.txt` records as
    /// "zooming breaks horribly as things move under the scrollwheel without
    /// visibility". A navigation gesture must not be able to change the
    /// artwork. Alt costs one finger and the hint line is right there.
    pub fn on_scroll(&mut self, steps: f32) {
        self.note_pointer_activity();
        // Over a whole subscene's gizmo, the same lever weighs the subscene:
        // its share of the points is to it what a weight is to a map.
        if let (Some(k), Some(_)) = (self.selected_subscene(), self.hovered.filter(|_| self.alt_held)) {
            let share = self.scene.layer_share(k) * 1.08f32.powf(steps);
            self.set_layer_share(k, share.max(0.01));
            return;
        }
        // Alt+scroll over an xaos edge scales it: the weight lever, one level
        // down. A gizmo under the pointer wins, as it does for clicks.
        if self.alt_held && self.hovered.is_none() {
            if let Some((a, b)) = self.xaos_edge_under_cursor() {
                let pos = self.scene.layer_position(b);
                let x = self.scene.transforms[a].xaos.get(pos);
                let base = if x == 0.0 { 0.05 } else { x };
                self.set_xaos_edge(a, b, (base * 1.1f32.powf(steps)).clamp(0.0, 100.0));
                return;
            }
        }
        if let Some(hit) = self.hovered.filter(|_| self.alt_held) {
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
        self.cancel_camera_glide();
        self.show_zoom_cursor(steps);
        self.zoom_steps(steps);
        self.invalidate_default_path();
    }

    /// Point the cursor at what the wheel is doing, with hysteresis on the swap.
    ///
    /// A wheel is noisy: a burst of zooming in often contains a stray step the
    /// other way, and a cursor that flipped on each one would strobe between
    /// two icons through every gesture — worse than no cursor at all. So the
    /// direction shown is the sign of an *accumulator*, capped a few steps
    /// either side: one stray step against a settled direction doesn't reach
    /// the boundary, while a genuine reversal crosses it in three.
    ///
    /// The icon then holds briefly after the last step, so the gaps inside one
    /// gesture don't drop it either. `update` puts the pointer back.
    fn show_zoom_cursor(&mut self, steps: f32) {
        // A gizmo grab or a camera drag owns the pointer; don't fight it.
        if !matches!(self.drag, Drag::None) {
            return;
        }
        // A fresh gesture starts from neutral rather than from wherever the
        // last one left off.
        if self.zoom_cursor.is_none() {
            self.zoom_bias = 0.0;
        }
        self.zoom_bias = (self.zoom_bias + steps).clamp(-ZOOM_BIAS_CAP, ZOOM_BIAS_CAP);
        let zoom_in = self.zoom_bias >= 0.0;
        let changed = self.zoom_cursor.map(|(dir, _)| dir) != Some(zoom_in);
        self.zoom_cursor = Some((zoom_in, Instant::now() + ZOOM_CURSOR_HOLD));
        if changed {
            self.window.set_cursor(if zoom_in {
                winit::window::CursorIcon::ZoomIn
            } else {
                winit::window::CursorIcon::ZoomOut
            });
        }
    }

    /// Drop the zoom cursor once the gesture has been over for a moment.
    fn expire_zoom_cursor(&mut self) {
        let Some((_, until)) = self.zoom_cursor else { return };
        if Instant::now() < until {
            return;
        }
        self.zoom_cursor = None;
        self.zoom_bias = 0.0;
        self.window.set_cursor(if self.hovered.is_some() {
            winit::window::CursorIcon::Grab
        } else {
            winit::window::CursorIcon::Default
        });
    }

    /// Toggle flightsim-style pitch inversion (persisted across sessions)
    /// The camera a scene should be framed with as it opens.
    ///
    /// The framing is always the scene's. The lens is the scene's **only if it
    /// states one** — otherwise the person keeps looking through whatever they
    /// were already looking through, which is their preference at startup,
    /// `--fov` if they passed one, and the settings slider after that.
    ///
    /// Without this, opening any of the shipped scenes (none of which state a
    /// lens) would snap the FoV back to 45 and there would be no way to keep a
    /// preference for longer than one scene.
    fn camera_for(&self, scene: &Scene) -> OrbitCamera {
        let mut cam = scene.camera();
        if !scene.camera_fov_authored {
            cam.fov = self.camera.fov;
        }
        cam
    }

    /// The lens the live camera looks through. Does **not** touch prefs — this
    /// is the one a view file or `--fov` sets, and neither is a statement
    /// about what the person wants by default.
    pub fn set_camera_fov(&mut self, fov: crate::camera::Fov) {
        self.camera.fov = fov;
        // A glide in flight carries its own copy of both ends.
        if let Some(g) = &mut self.camera_glide {
            g.from.fov = fov;
            g.to.fov = fov;
        }
    }

    /// The settings-window control: change the lens *and* remember it, so it
    /// follows the person to the next scene and the next launch.
    ///
    /// Note what it does not do: it doesn't touch the scene. A FoV reaches a
    /// scene file only through `--set camera.fov` or a text editor, so that
    /// working at 90 degrees for an afternoon doesn't quietly rewrite the lens
    /// of every scene saved along the way.
    pub fn set_fov_preference(&mut self, fov: crate::camera::Fov) {
        if self.camera.fov == fov && self.prefs.fov == Some(fov.degrees()) {
            return;
        }
        self.set_camera_fov(fov);
        self.prefs.fov = Some(fov.degrees());
        self.prefs.save();
    }

    pub fn camera_fov(&self) -> crate::camera::Fov {
        self.camera.fov
    }

    pub fn toggle_invert_pitch(&mut self) {
        self.prefs.invert_pitch = !self.prefs.invert_pitch;
        log::info!(
            "Pitch inversion: {}",
            if self.prefs.invert_pitch { "inverted" } else { "normal" }
        );
        self.prefs.save();
    }

    /// Panel scale multiplier (persisted across sessions).
    pub fn ui_scale(&self) -> f32 {
        self.prefs.ui_scale
    }

    /// Set the panel scale. Deferred-write like the window geometry: this is a
    /// slider, so it changes sixty times a second while being dragged.
    pub fn set_ui_scale(&mut self, scale: f32) {
        let scale = scale.clamp(0.6, 3.0);
        if self.prefs.ui_scale == scale {
            return;
        }
        self.prefs.ui_scale = scale;
        self.prefs_dirty_since = Some(std::time::Instant::now());
    }

    /// Which axis orbit drags yaw about (persisted across sessions)
    pub fn orbit_style(&self) -> OrbitStyle {
        self.prefs.orbit_style
    }

    /// Choose the orbit geometry. A control preference, not scene data: it
    /// changes how the next drag is interpreted and leaves the framing, the
    /// camera path and the scene exactly where they are.
    pub fn set_orbit_style(&mut self, style: OrbitStyle) {
        if self.prefs.orbit_style == style {
            return;
        }
        self.prefs.orbit_style = style;
        log::info!("Orbit style: {:?}", style);
        self.prefs.save();
    }

    /// `suppress_hover`: true when the pointer is over an egui area and no
    /// drag is active — gizmo hover-picking is skipped so egui panels don't
    /// fight the 3D gizmo highlight/cursor-icon underneath them. Active
    /// drags (orbit/pan/gizmo) always keep receiving motion regardless.
    /// The pointer did something. Restarts the idle clock behind the cursor
    /// auto-hide, and brings the cursor straight back if it had gone.
    ///
    /// Called from every pointer entry point — motion, press, release, scroll
    /// — rather than motion alone, so a wheel-zoom or a click that the hand
    /// makes without moving the mouse still counts as "someone is using this".
    fn note_pointer_activity(&mut self) {
        self.cursor_last_moved = Instant::now();
        if self.cursor_hidden {
            self.cursor_hidden = false;
            self.window.set_cursor_visible(true);
        }
    }

    /// Hide the pointer once it has sat still over the artwork, and bring it
    /// back the moment anything happens.
    ///
    /// The pointer is a black arrow parked in the middle of a picture, and
    /// this is a program for looking at pictures. Nothing else fades, and
    /// nothing here contradicts the zero-animation rule — the cursor is the
    /// operating system's furniture, not this app's chrome, and it doesn't
    /// fade: it is drawn on one frame and not on the next.
    ///
    /// **View mode only.** In edit mode every gizmo is a thing you aim at,
    /// hover-highlight and grab, so a cursor that disappears while you line
    /// one up disappears at exactly the moment it is being used. `over_ui`
    /// keeps it while the pointer is parked in a panel, where the same
    /// argument applies to every widget.
    pub fn update_cursor_visibility(&mut self, over_ui: bool) {
        let hide = cursor_should_hide(
            !self.show_gizmos,
            over_ui,
            !matches!(self.drag, Drag::None),
            self.cursor_last_moved.elapsed(),
        );
        if hide != self.cursor_hidden {
            self.cursor_hidden = hide;
            self.window.set_cursor_visible(!hide);
        }
    }

    pub fn on_cursor_moved(&mut self, x: f32, y: f32, suppress_hover: bool) {
        let (dx, dy) = (x - self.cursor.0, y - self.cursor.1);
        self.cursor = (x, y);
        if let Some(stroke) = self.ui_state.xaos_knife.as_mut() {
            stroke.push((x, y));
        }
        self.note_pointer_activity();

        // Travel from the press, tracked for every drag — but it *gates*
        // almost nothing. See `drag_moved` for why the camera doesn't wait.
        if !self.drag_moved && !matches!(self.drag, Drag::None) {
            let (ox, oy) = (x - self.drag_origin.0, y - self.drag_origin.1);
            if ox * ox + oy * oy >= DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX {
                self.drag_moved = true;
                self.set_drag_cursor();
            }
        }

        match self.drag {
            Drag::None => {
                if suppress_hover {
                    if self.hovered.is_some() {
                        self.gizmo_renderer.set_highlight(&self.gpu.queue, None, false);
                        self.window.set_cursor(winit::window::CursorIcon::Default);
                        self.hovered = None;
                    }
                } else {
                    self.update_hover();
                }
            }
            // The press already did its whole job (it selected something).
            // Moving the pointer with the button still down must not start
            // anything, or "click to select" would become "click to select and
            // then drag whatever I happen to be over".
            Drag::Consumed => {}
            Drag::Orbit => {
                let dy = if self.prefs.invert_pitch { -dy } else { dy };
                self.camera.orbit(dx, dy, self.prefs.orbit_style);
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
            Drag::Gizmo { transform, subscene, mode, start_matrix } => {
                // The dead zone applies *here* and nowhere else. A gizmo drag
                // writes to the artwork and lands a history entry, so a twitch
                // during a click is a real edit you then have to undo; the
                // camera has neither problem. The gesture is also a careful,
                // deliberate one where a few pixels of settling costs nothing —
                // unlike orbiting, which you do constantly.
                if self.drag_moved {
                    self.update_gizmo_drag(transform, subscene, mode, start_matrix);
                }
            }
        }
    }

    /// The cursor position a gizmo drag should be read from: the real one, or
    /// a slowed-down virtual one while Shift is held.
    ///
    /// Shift-during-drag means *precision* in every drawing and modelling tool
    /// in the world, and it was free here — the gizmo path never consulted it.
    /// It's worth more in this app than in most, because these drags run the
    /// chaos game live and are therefore very high gain: a few pixels of hand
    /// travel can be the difference between a structure and a smear.
    ///
    /// See `fine_anchor` for why this is anchored rather than a bare multiply.
    fn fine_cursor(&mut self) -> (f32, f32) {
        if self.shift_held != self.fine_active {
            // Re-anchor at the current virtual position, so the gain changes
            // without the thing being dragged jumping.
            self.fine_anchor = self.fine_cursor_with(self.fine_active);
            self.fine_from = self.cursor;
            self.fine_active = self.shift_held;
        }
        self.fine_cursor_with(self.fine_active)
    }

    fn fine_cursor_with(&self, fine: bool) -> (f32, f32) {
        /// A fifth of normal travel: slow enough to be a different instrument,
        /// fast enough that you can still cross a gizmo with one hand movement.
        const FINE_GAIN: f32 = 0.2;
        let gain = if fine { FINE_GAIN } else { 1.0 };
        (
            self.fine_anchor.0 + (self.cursor.0 - self.fine_from.0) * gain,
            self.fine_anchor.1 + (self.cursor.1 - self.fine_from.1) * gain,
        )
    }

    /// Say which gesture is in flight, with the pointer.
    ///
    /// The viewport is one surface that does four different things depending
    /// on which button you held and whether Shift was down, and until now the
    /// only one that changed the cursor was a gizmo grab — so orbit, pan and
    /// roll all left it an arrow and the viewport never confirmed what it had
    /// decided you meant.
    ///
    /// Set on crossing the drag threshold rather than on press: below the dead
    /// zone the gesture is still a click, and a cursor that flickered on every
    /// click would be worse than none.
    fn set_drag_cursor(&mut self) {
        use winit::window::CursorIcon;
        let icon = match self.drag {
            // Nothing is being dragged in either case, so the pointer keeps
            // whatever hover icon it already had.
            Drag::None | Drag::Consumed => return,
            Drag::Orbit | Drag::Gizmo { .. } => CursorIcon::Grabbing,
            Drag::Pan => CursorIcon::Move,
            // `winit` has no trackball or rotate cursor. Roll is driven purely
            // by horizontal travel, so the horizontal-resize arrows are at
            // least honest about which axis of the mouse is being read; the
            // status bar carries the word itself.
            Drag::Roll => CursorIcon::EwResize,
        };
        self.window.set_cursor(icon);
    }

    /// Re-pick the gizmo part under the cursor; update the glow highlight and
    /// the cursor icon so grabbable things announce themselves
    fn update_hover(&mut self) {
        use winit::window::CursorIcon;

        let hit = if self.show_gizmos {
            let (w, h) = self.gpu.size();
            let matrices = self.gizmo_matrices();
            crate::pick::pick_gizmo(
                &matrices,
                self.gizmo_selection(),
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
            // Hover, never held: `update_hover` doesn't run during a drag, for
            // the reason set out on `set_highlight`.
            self.gizmo_renderer.set_highlight(
                &self.gpu.queue,
                hit.map(|h| (h.transform, h.part)),
                false,
            );
            // Not while the zoom magnifier is up: scrolling moves the scene
            // under a still pointer, so the hovered gizmo changes constantly
            // mid-gesture and this would strobe against it. `expire_zoom_cursor`
            // restores the right icon when the gesture ends.
            if self.zoom_cursor.is_none() {
                self.window.set_cursor(if hit.is_some() {
                    CursorIcon::Grab
                } else {
                    CursorIcon::Default
                });
            }
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
        self.note_pointer_activity();
        use winit::event::MouseButton;
        self.drag_origin = self.cursor;
        self.drag_moved = false;
        // Taking the camera by hand beats any glide still in flight, and ends
        // the zoom gesture the magnifier was showing.
        self.cancel_camera_glide();
        self.zoom_cursor = None;
        self.zoom_bias = 0.0;
        // Fine-drag starts from wherever this gesture starts, at whatever the
        // modifier is right now.
        self.fine_anchor = self.cursor;
        self.fine_from = self.cursor;
        self.fine_active = self.shift_held;
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
                // An xaos edge under the pointer opens its popover, and the
                // press does nothing else — like selecting a gizmo.
                if let Some(edge) = self.xaos_edge_under_cursor() {
                    self.ui_state.xaos_edge = Some(edge);
                    self.drag = Drag::Consumed;
                    return;
                }
                self.ui_state.xaos_edge = None;
                // Nothing under the pointer. If this turns out to be a click
                // rather than a drag, `on_mouse_release` reads that as "I meant
                // *nothing*" and drops the selection — the universal editor
                // convention, and the only exit from a selection this app used
                // to have no way out of.
                self.drag = if self.shift_held { Drag::Pan } else { Drag::Orbit };
                // Taking the camera by hand stops it flying the path — they'd
                // fight over the same framing otherwise.
                self.stop_following();
            }
            MouseButton::Middle => {
                self.drag = Drag::Pan;
                self.stop_following();
            }
            MouseButton::Right => {
                // Over a gizmo, right-click is that transform's context menu —
                // the same menu its row in the Transforms window has, opened on
                // the thing itself. Rolling the whole camera off a click aimed
                // at one transform would be a surprise, so rolling only starts
                // on empty space.
                if let Some(hit) = self.hovered.filter(|_| self.selected_subscene().is_none()) {
                    self.selected_transform = Some(hit.transform);
                    // Position is filled in by `ui::draw` from egui's own
                    // pointer, which is already in logical points.
                    self.ui_state.transform_menu = Some((hit.transform, None));
                    return;
                }
                if crate::ui::xaos::overlay_active(self) {
                    // Ctrl+right-drag is the knife: every edge the stroke
                    // crosses is cut, as one edit, when the button comes up.
                    if self.ctrl_held {
                        self.ui_state.xaos_knife = Some(vec![self.cursor]);
                        self.drag = Drag::Consumed;
                        return;
                    }
                    if let Some(edge) = self.xaos_edge_under_cursor() {
                        self.ui_state.xaos_edge_menu = Some((edge, None));
                        return;
                    }
                }
                self.drag = Drag::Roll;
                self.stop_following();
            }
            _ => {}
        }
    }

    pub fn on_mouse_release(&mut self, button: winit::event::MouseButton) {
        self.note_pointer_activity();
        use winit::event::MouseButton;
        if matches!(button, MouseButton::Right) {
            if let Some(stroke) = self.ui_state.xaos_knife.take() {
                let (edges, _) = crate::ui::xaos::overlay_edges(self);
                let cuts: Vec<(usize, usize, f32)> =
                    crate::ui::xaos::knife_cuts(&edges, &stroke).into_iter().map(|(a, b)| (a, b, 0.0)).collect();
                if !cuts.is_empty() {
                    let label = format!("Xaos: knife, {} edge{}", cuts.len(), if cuts.len() == 1 { "" } else { "s" });
                    self.set_xaos_edges(&cuts, Some(label));
                }
            }
        }
        if matches!(button, MouseButton::Left | MouseButton::Middle | MouseButton::Right) {
            // A left-click on empty viewport space that never became a drag is
            // a deselect. Only the plain orbit grab: a shift-click is a
            // modified gesture and a right-click is the roll/menu button, and
            // neither reads as "select nothing".
            if matches!(button, MouseButton::Left)
                && matches!(self.drag, Drag::Orbit)
                && !self.drag_moved
                && self.gizmo_selection().is_some()
            {
                self.select_transform(None);
                self.leave_subscene_gizmo();
                log::info!("Selection cleared");
            }
            if let Drag::Gizmo { transform, subscene, mode, start_matrix } = self.drag {
                let now = self.gizmo_matrices().get(transform).copied().unwrap_or(start_matrix);
                let t = now.w_axis.truncate();
                log::info!(
                    "{}{}: p=({:.3},{:.3},{:.3}) s={:.3}",
                    if subscene.is_some() { "subscene " } else { "T" },
                    subscene.unwrap_or(transform), t.x, t.y, t.z, now.x_axis.truncate().length(),
                );
                let matrix_changed = now != start_matrix;
                if matrix_changed {
                    if let Some(before) = self.gizmo_drag_before.take() {
                        let label = match subscene {
                            Some(k) => {
                                let verb = self.gizmo_drag_label(transform, mode);
                                let verb = verb.split(' ').next().unwrap_or("Move").to_string();
                                format!("{} {}", verb, self.scene.layer_name(k))
                            }
                            None => self.gizmo_drag_label(transform, mode),
                        };
                        self.commit_edit(label, None, before);
                    }
                } else {
                    // No net motion (a click that didn't drag): no history entry.
                    self.gizmo_drag_before = None;
                }
            }
            self.drag = Drag::None;
            self.drag_moved = false;
            self.update_hover();
            // `update_hover` only touches the cursor when the *hit* changed,
            // and after an orbit over empty space it hasn't (None before, None
            // after) — so the drag cursor would stay on the pointer for good.
            // Put it back explicitly.
            self.window.set_cursor(if self.hovered.is_some() {
                winit::window::CursorIcon::Grab
            } else {
                winit::window::CursorIcon::Default
            });
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
            GizmoDragMode::TranslateView { .. } | GizmoDragMode::TranslateAxis { .. } => {
                "Move".to_string()
            }
            GizmoDragMode::Rotate { .. } => "Rotate".to_string(),
            // Its own verb, matching the camera's vocabulary for the same idea.
            GizmoDragMode::Roll { .. } => "Roll".to_string(),
            GizmoDragMode::Scale { .. } => "Scale".to_string(),
            // Which axis, because "Scale" three times in the undo list doesn't
            // say what you'd be undoing.
            GizmoDragMode::ScaleAxis { k, .. } => {
                format!("Scale {}", ["X", "Y", "Z"][k.min(2)])
            }
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

        let matrices = self.gizmo_matrices();
        let was_selected = self.gizmo_selection();
        let subscene = self.selected_subscene();
        let hit = pick_gizmo(&matrices, was_selected, view_proj, self.cursor, w, h)?;

        // Grabbing an *unselected* transform selects it and stops there. The
        // gesture is spent on choosing; it starts no drag, and deliberately not
        // a camera orbit either — swinging the view off a press that was aimed
        // at a dot would be its own small betrayal. Press to choose, then drag.
        if was_selected != Some(hit.transform) {
            self.select_transform(Some(hit.transform));
            return Some(Drag::Consumed);
        }

        // Say so on the gizmo itself, now, rather than leaving the hover glow
        // to stand in for a grab. `update_hover` won't run again until the drag
        // ends, so without this the press produces no feedback at all.
        self.hovered = Some(hit);
        self.gizmo_renderer
            .set_highlight(&self.gpu.queue, Some((hit.transform, hit.part)), true);

        if subscene.is_none() {
            self.selected_transform = Some(hit.transform);
        }
        let m = matrices[hit.transform];
        let origin = m.w_axis.truncate();
        let inv_vp = view_proj.inverse();
        let (ray_o, ray_d) = crate::camera::cursor_ray(inv_vp, self.cursor.0, self.cursor.1, w, h);

        // Ctrl turns any grab into a uniform scale — except on the two rotation
        // controls, where it is the snap modifier instead.
        //
        // Alt was the snap, and on this desktop alt+drag never reaches the app
        // at all: XFCE claims it for move-window. A modifier the window manager
        // eats is not a modifier. Ctrl was free to take over here because
        // "ctrl = uniform scale" on a *rotation* control was the one place that
        // meaning was redundant — uniform scale is still reachable by ctrl on
        // the origin dot, on any axis shaft, and on any tip. Alt is kept as an
        // alias for anyone whose desktop leaves it alone.
        //
        // The rule this settles on: **ctrl snaps wherever the grab has
        // something to snap to** — a tip's scale, a rotate edge's angle, the
        // ring's — and means uniform scale everywhere else. Uniform scale loses
        // only the entry points where it was redundant.
        let snappable = matches!(
            hit.part,
            GizmoPart::Tip(_) | GizmoPart::RotEdge(_) | GizmoPart::Roll
        );
        // A subscene's scale is uniform (see `subscene::Placement`), so its
        // tip handles resize the whole thing rather than one axis of it.
        let uniform_tip = subscene.is_some() && matches!(hit.part, GizmoPart::Tip(_));
        let mode = if (self.ctrl_held && !snappable) || uniform_tip {
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
                GizmoPart::Tip(k) => {
                    let col = m.col(k).truncate();
                    let len0 = col.length();
                    let dir = col.normalize_or(Vec3::X);
                    let s0 = crate::pick::line_param_closest_to_ray(origin, dir, ray_o, ray_d);
                    // How much world distance one screen pixel is worth along
                    // this axis, measured once at the grab. Everything about
                    // the dashed line is sized from it: a period of ~16px
                    // (8 on, 8 off), and a minimum run of a fifth of the
                    // smaller screen dimension so a nearly-flat axis still
                    // draws a line you can see and aim along. Measured once
                    // rather than per frame — see the note on
                    // `indicators::build_axis_extension` for why recomputing it
                    // would make the dashes crawl.
                    let world_per_px = crate::camera::world_to_screen(origin + col, view_proj, w, h)
                        .zip(crate::camera::world_to_screen(origin, view_proj, w, h))
                        .map(|(tip_s, origin_s)| {
                            let px = ((tip_s.0 - origin_s.0).powi(2)
                                + (tip_s.1 - origin_s.1).powi(2))
                            .sqrt();
                            if px > 1.0 { len0 / px } else { len0 * 0.02 }
                        })
                        .unwrap_or(len0 * 0.02);
                    let dash_pitch = 16.0 * world_per_px;
                    let dash_reach = 0.2 * w.min(h) * world_per_px;
                    GizmoDragMode::ScaleAxis { k, dir, len0, s0, dash_pitch, dash_reach }
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
                GizmoPart::Roll => {
                    // The view axis, pointing back at the eye, so a clockwise
                    // drag rolls the map clockwise on screen. Captured once:
                    // the camera can't move mid-gizmo-drag.
                    let axis = -self.camera.forward();
                    let center = crate::camera::world_to_screen(origin, view_proj, w, h)?;
                    GizmoDragMode::Roll {
                        axis,
                        center,
                        start_angle: crate::pick::screen_angle(center, self.cursor),
                    }
                }
            }
        };

        Some(Drag::Gizmo {
            transform: hit.transform,
            subscene,
            mode,
            start_matrix: m,
        })
    }

    /// Recompute the dragged transform's matrix from the grab-time state and
    /// the current cursor, then push it live to the GPU
    fn update_gizmo_drag(
        &mut self,
        transform: usize,
        subscene: Option<usize>,
        mode: GizmoDragMode,
        start: Mat4,
    ) {
        let (w, h) = self.gpu.size();
        let (w, h) = (w as f32, h as f32);
        let view_proj = self.current_view_proj();
        let inv_vp = view_proj.inverse();
        // Every drag mode below is a pure function of *a* cursor position, so
        // Shift-for-fine is one substitution here rather than four separate
        // gain factors.
        let cursor = self.fine_cursor();
        let (ray_o, ray_d) = crate::camera::cursor_ray(inv_vp, cursor.0, cursor.1, w, h);
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
            // One body for both: a roll is a rotation whose axis happens to be
            // the camera's. Sharing the arm is what keeps the snap, the
            // shortest-way-round and the group-step lookup from drifting
            // between the two controls.
            GizmoDragMode::Rotate { axis, center, start_angle }
            | GizmoDragMode::Roll { axis, center, start_angle } => {
                let angle = crate::pick::screen_angle(center, cursor);
                // Shortest way round is right for a drag: nobody swings the
                // pointer more than half a turn between two frames.
                let mut delta = start_angle.shortest_to(angle);
                // Ctrl snaps to 15°, with alt kept as an alias. Worth more here
                // than in a general 3D tool:
                // IFS aesthetics live on clean rotational symmetry, and a
                // fifteenth of a degree off exact is visible as a smear in the
                // attractor. Ctrl doesn't mean uniform scale on a rotation grab
                // (see `try_grab_gizmo`), which is what frees it to mean this.
                //
                // When the map is a motif of a symmetry group, the *group's*
                // own step is the clean increment — 72° under C5 — so the
                // constant becomes a lookup. A group is a statement about which
                // rotations are exact in this scene, and this is the control
                // that was already trying to guess that.
                if self.ctrl_held || self.alt_held {
                    const SNAP: f32 = std::f32::consts::FRAC_PI_2 / 6.0; // 15°
                    let snap = self
                        .selected_transform
                        .and_then(|i| self.scene.transforms.get(i))
                        .and_then(|t| t.symmetry.as_ref())
                        .map_or(SNAP, |s| s.kind().snap_degrees().to_radians());
                    delta = crate::rot::Turn1D::from_radians(
                        (delta.radians() / snap).round() * snap,
                    );
                }
                let rot = Mat4::from_quat(
                    crate::rot::Turn::about(axis, delta.radians()).exp().as_quat(),
                );
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
                let factor = ((start_y - cursor.1) * 0.005).exp().clamp(0.02, 50.0);
                let mut m = start;
                m.x_axis *= factor;
                m.y_axis *= factor;
                m.z_axis *= factor;
                m
            }
            GizmoDragMode::ScaleAxis { k, dir, len0, s0, .. } => {
                let s = crate::pick::line_param_closest_to_ray(start_origin, dir, ray_o, ray_d);
                let len = axis_scale_length(len0, s, s0, self.ctrl_held || self.alt_held);
                with_scaled_column(start, k, dir, len)
            }
        };

        // Hold `d` across any drag that can change the map's contraction — both
        // scale modes can. Translation and rotation leave the determinant
        // alone, and `hold_dimension_through` returns immediately for them, so
        // this gate saves the work rather than guarding correctness.
        // A whole subscene: the drag moves its placement, which is a
        // similarity, so whatever the gizmo produced is read back as one.
        if let Some(k) = subscene {
            if let Some(sub) = self.scene.subscenes.get_mut(k - 1) {
                sub.placement = crate::subscene::Placement::from_matrix(new_matrix);
            }
            self.bump_matrix_generation();
            self.sync_transforms_to_gpu();
            return;
        }

        // The gizmo works in scene space; the map lives in its subscene's.
        let placement = self.scene.layer_placement(self.scene.transforms[transform].layer as usize);
        let local = if placement == Mat4::IDENTITY { new_matrix } else { placement.inverse() * new_matrix };
        if matches!(
            mode,
            GizmoDragMode::Scale { .. } | GizmoDragMode::ScaleAxis { .. }
        ) {
            self.hold_dimension_through(transform, local);
        }

        self.scene.transforms[transform].matrix = local;
        self.bump_matrix_generation();
        self.sync_transforms_to_gpu();
    }

    // === Scene browser (B) ===

    /// Open/close the scene browser. Opening reads the folder it was last
    /// showing (else the current scene's, else `scenes/`), fresh, so a scene
    /// that landed while it was closed is there.
    pub fn toggle_browser(&mut self) {
        if self.show_browser {
            self.show_browser = false;
            return;
        }
        let current_dir = self
            .scene_path
            .as_ref()
            .and_then(|p| Path::new(p).parent())
            .filter(|d| !d.as_os_str().is_empty())
            .map(Path::to_path_buf);
        let folder = self
            .prefs
            .browser_folder
            .clone()
            .filter(|d| d.is_dir())
            .or(current_dir)
            .unwrap_or_else(|| std::path::PathBuf::from("scenes"));
        self.browser_open_folder(&folder);
        self.show_browser = true;
    }

    /// Show `path` in the browser, and remember it for next time.
    pub fn browser_open_folder(&mut self, path: &Path) {
        let folder = crate::scene_browser::Folder::read(path, self.prefs.browser_sort);
        // Start on the current scene when it's in this folder.
        self.browser_selected = folder
            .as_ref()
            .ok()
            .and_then(|f| {
                let current = self.scene_path.as_deref().map(Path::new).or(self.scene_png.as_deref())?;
                f.entries.iter().position(|e| crate::scene_browser::same_path(&e.path, current))
            })
            .unwrap_or(0);
        self.browser = Some(folder);
        self.ui_state.browser_arrived = path.canonicalize().ok();
        if self.prefs.browser_folder.as_deref() != Some(path) {
            self.prefs.browser_folder = Some(path.to_path_buf());
            self.prefs_dirty_since = Some(std::time::Instant::now());
        }
    }

    /// Read the shown folder again.
    pub fn browser_refresh(&mut self) {
        self.ui_state.browser_tree.clear();
        if let Some(path) = self.browser_folder_path().map(Path::to_path_buf) {
            self.browser_open_folder(&path);
        }
    }

    /// The folder the browser is showing, whether or not it could be read.
    pub fn browser_folder_path(&self) -> Option<&Path> {
        self.prefs.browser_folder.as_deref().filter(|_| self.browser.is_some())
    }

    /// The browser's folder, the thumbnail cache and the UI state, borrowed
    /// together so a row can ask for its thumbnail while it draws.
    pub fn browser_parts(
        &mut self,
    ) -> (
        Option<&Result<crate::scene_browser::Folder, String>>,
        &mut crate::gpu::thumbs::ThumbCache,
        &mut crate::ui::UiState,
    ) {
        (self.browser.as_ref(), &mut self.thumbs, &mut self.ui_state)
    }

    pub fn browser_sort(&self) -> crate::scene_browser::Sort {
        self.prefs.browser_sort
    }

    pub fn set_browser_sort(&mut self, sort: crate::scene_browser::Sort) {
        if self.prefs.browser_sort == sort {
            return;
        }
        self.prefs.browser_sort = sort;
        self.prefs_dirty_since = Some(std::time::Instant::now());
        // Keep the keyboard selection on the same scene through the re-sort.
        if let Some(Ok(folder)) = self.browser.as_mut() {
            let selected = folder.entries.get(self.browser_selected).map(|e| e.path.clone());
            folder.sort(sort);
            self.browser_selected = selected
                .and_then(|p| folder.entries.iter().position(|e| e.path == p))
                .unwrap_or(0);
        }
    }

    /// Built-in places, then the ones added by hand.
    pub fn browser_places(&self) -> Vec<crate::scene_browser::Place> {
        crate::scene_browser::places(&self.prefs.browser_places)
    }

    pub fn add_browser_place(&mut self, path: &Path) {
        if self.browser_places().iter().any(|p| crate::scene_browser::same_path(&p.path, path)) {
            return;
        }
        self.prefs.browser_places.push(path.to_path_buf());
        self.prefs_dirty_since = Some(std::time::Instant::now());
    }

    pub fn remove_browser_place(&mut self, path: &Path) {
        self.prefs.browser_places.retain(|p| p != path);
        self.prefs_dirty_since = Some(std::time::Instant::now());
    }

    fn browser_len(&self) -> usize {
        match &self.browser {
            Some(Ok(f)) => f.entries.len(),
            _ => 0,
        }
    }

    /// Index the Up/Down keys are currently on.
    pub fn browser_selected(&self) -> usize {
        self.browser_selected
    }

    /// Move the keyboard selection to a clicked row, so the two stay in sync.
    pub fn set_browser_selected(&mut self, idx: usize) {
        if idx < self.browser_len() {
            self.browser_selected = idx;
        }
    }

    pub fn browser_move(&mut self, down: bool) {
        let n = self.browser_len();
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
        let path = match &self.browser {
            Some(Ok(f)) => f.entries.get(self.browser_selected).map(|e| e.path.clone()),
            _ => None,
        };
        if let Some(path) = path {
            self.request_load_scene(&path);
        }
        self.show_browser = false;
    }

    /// Replace the current scene with one loaded from disk, rebuilding the
    /// GPU pipelines and resetting camera/selection to the scene's defaults
    pub fn load_scene_file(&mut self, path: &Path) {
        // Back to the live scene first, so what's set aside isn't lost under
        // the one being opened.
        self.leave_preview();
        let scene = match Scene::load(path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("Failed to load {}: {}", path.display(), e);
                // The scene on screen is untouched; say why nothing happened.
                self.set_notice(format!("Couldn't open {}: {}", path.display(), e), Tone::Error);
                return;
            }
        };
        log::info!("Loading scene: {} ({})", scene.name, path.display());
        self.notice = None;

        self.camera = self.camera_for(&scene);
        self.invalidate_default_path();
        self.rewind_camera_playhead();
        self.point_size = scene.point_size;
        self.color_falloff = scene.color_falloff;
        self.color_contrast = scene.color_contrast;
        // `haze` and `exposure` were missing from this list, so opening a scene
        // kept whatever the *previous* scene had — `adopt_scene` (random flame,
        // blank canvas) has always taken both. Every value the scene file
        // carries is restored here, or the file isn't really the document.
        self.haze_amount = scene.haze;
        self.exposure = scene.exposure;
        // Same precedence as startup: a point count the person chose in the
        // Render window follows them across scene loads; otherwise take the
        // scene file's own.
        self.buffer_capacity = self
            .prefs
            .point_count
            .map(|n| n as u32)
            .unwrap_or(scene.point_count as u32);
        self.transform_enabled = vec![true; scene.transforms.len()];
        // A loaded scene is something to look at first; nothing is selected
        // until you select it. See the note in `App::new`.
        self.selected_transform = None;
        self.selected_subscene = None;
        self.current_layer = scene.first_layer_with_maps();
        self.selected_variation = 0;
        self.drag = Drag::None;
        self.hovered = None;
        self.scene = scene;
        // A render has no `.toml` behind it: the document is untitled, and
        // remembers the picture only to name itself and its first save.
        if crate::scene_source::is_png(path) {
            self.scene_path = None;
            self.scene_png = Some(path.to_path_buf());
        } else {
            self.scene_path = Some(path.to_string_lossy().into_owned());
            self.scene_png = None;
        }
        self.invalidate_saved_views();
        // Opening a document gives you a fresh undo stack, as it does in every
        // editor ever written. The surprise was never the clear — it was that
        // it happened without asking, which is what `request_load_scene` fixes.
        self.history.clear();
        self.scene_loads += 1;
        self.mark_saved();
        self.rebuild_pipelines();
    }

    // === Opening by drag and drop ===

    /// A file was dropped on the window. Gathered rather than opened at once,
    /// because a multi-file drop arrives as one event per file.
    pub fn file_dropped(&mut self, path: std::path::PathBuf) {
        self.drop_hover = None;
        self.dropped.push(path);
    }

    /// Open what was dropped since the last frame: the first file, through
    /// the same door as the Scenes browser — unsaved-changes prompt and all.
    pub fn open_dropped(&mut self) {
        if self.dropped.is_empty() {
            return;
        }
        let files = std::mem::take(&mut self.dropped);
        let first = &files[0];
        if let Err(e) = droppable(first) {
            self.set_notice(e, Tone::Error);
            return;
        }
        self.request_load_scene(first);
        let ignored = files.len() - 1;
        let failed = self.notice.as_ref().is_some_and(|n| n.tone == Tone::Error);
        if ignored > 0 && !failed {
            let name = first.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // Still asking about unsaved edits, it hasn't opened yet.
            let verb = if self.pending_action.is_some() { "Opening" } else { "Opened" };
            self.set_notice(
                format!(
                    "{} {} and ignored the other {} — one scene at a time",
                    verb,
                    name,
                    if ignored == 1 { "file".to_string() } else { format!("{} files", ignored) },
                ),
                Tone::Note,
            );
        }
    }

    /// Show a message in the status bar until it's clicked away or the next
    /// scene opens. Not on a timer: a message that leaves on its own can leave
    /// before it's been read.
    pub fn set_notice(&mut self, text: String, tone: Tone) {
        if tone == Tone::Error {
            log::error!("{}", text);
        }
        self.notice = Some(Notice { text, tone });
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn dismiss_notice(&mut self) {
        self.notice = None;
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
        fn kind_is_still(kind: crate::render_job::JobKind) -> bool {
            matches!(kind, crate::render_job::JobKind::Still { .. })
        }
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
        self.job_done = None;
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
                // Nothing was rendered, so nothing could have been cut short.
                .map(|()| (params.out_path.clone(), crate::render_job::Outcome::Complete))
                .map_err(|e| e.to_string());
            match &result {
                Ok((p, _)) => log::info!("View descriptor written: {}", p.display()),
                Err(e) => log::error!("View descriptor failed: {}", e),
            }
            self.job_done = Some(crate::render_job::FinishedJob {
                result,
                params,
                secs: 0.0,
                log: Vec::new(),
                seen: false,
            });
            return;
        }

        // A still is the scene at the playhead — what is on screen — and an
        // animation is the whole timeline, so it takes the tracks with it.
        let mut scene = match (&kind_is_still(params.kind), self.live_anim.as_ref().and_then(|a| a.animator.as_ref())) {
            (true, Some(anim)) => anim.at(self.time).unwrap_or_else(|e| {
                log::warn!("Render job: tracks won't apply ({e}); rendering the scene as authored");
                self.scene.clone()
            }),
            _ => self.scene.clone(),
        };
        // Job-scoped: the interactive buffer keeps whatever the Render window
        // set, so exploring stays comfortable while a big render runs.
        scene.point_count = params.points;
        // The view carries the window's live values; at the playhead, a track's
        // value is the live one.
        let held = self.swap_anim_view(self.anim_view);
        let view = self.current_view();
        self.swap_anim_view(held);

        // Built here rather than on the job thread because the *measured*
        // throughput lives on the app: the job renders on its own device and
        // has no idea how fast this machine is.
        let ledger = crate::render_job::Ledger::for_job(
            &params,
            self.measured_throughput(),
            self.max_point_capacity() as u64 * crate::render_job::BYTES_PER_POINT,
        );

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
            phase: crate::render_job::Phase::Setup,
            ledger,
            done: 0,
            total: 0,
            log: Vec::new(),
            cancel_arm: Default::default(),
            paused_at: None,
            paused_total: Duration::ZERO,
            eta: None,
            eta_at: 0.0,
        };

        let out = params.out_path.clone();
        let kind = params.kind;
        let (splat, exposure, transparent) = (params.splat, params.exposure, params.transparent);
        // Two fields out of one choice: `--spp` is what makes the accumulating
        // path run, and `accumulate` is the ring path's extra-frames dial that
        // it ignores. `Samples` is the single place that decides, so the two
        // can't be set to disagree here.
        let (spp, accumulate) = (params.samples.spp(), params.samples.accumulate());
        let density_estimation = params.density_estimation;
        let gpu_share = params.gpu_share;
        // What the *window* is holding on the same card while the job runs.
        //
        // The job gets its own wgpu device, which makes it easy to forget that
        // it does not get its own GPU. The interactive point buffer alone is
        // hundreds of megabytes and the viewport's splat targets are more, so a
        // job that plans as if the card were empty will happily size tiles it
        // cannot allocate — which is exactly what happened, and why the same
        // settings rendered fine from a terminal.
        //
        // Deliberately an over-estimate. Planning one tile too many costs a
        // little wall clock; planning one too few costs the whole render.
        let gpu_reserve = {
            let points = self.point_capacity() as u64 * crate::render_job::BYTES_PER_POINT;
            let (vw, vh) = self.gpu.size();
            let (w, h) = (vw as u64, vh as u64);
            // Splat accumulation, resolve, surface and depth — call it six
            // full-size targets at 8 bytes, which is generous and cheap to be
            // wrong about in this direction.
            points + w * h * 8 * 6
        };
        // Read off `self` rather than `params`: the grade is not a job field,
        // it is the window's live look, and the job renders what you saw.
        let grade = self.grade;
        let threads = params.threads;
        let (supersample, filter, filter_radius, bit_depth) =
            (params.supersample, params.filter, params.filter_radius, params.bit_depth);
        // The scene as it is *in the app*, which is what the job renders — the
        // path is only where it came from, and it may well have been edited
        // since. The record embeds the scene itself, so the two can't disagree.
        let scene_path = self.scene_path.clone().map(std::path::PathBuf::from);

        log::info!(
            "Render job started: {} ({}, {} points)",
            out.display(),
            kind.label(),
            params.points,
        );

        std::thread::spawn(move || {
            control.phase(crate::render_job::Phase::Setup);
            let base = crate::offline::OfflineParams {
                gpu_reserve,
                gpu_share,
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
                // A render job produces one image or one animation.
                labels: false,
                supersample,
                filter,
                filter_radius,
                bit_depth,
                scene_path,
                threads,
                // A dialog render is not an investigation.
                gpu_timing: false,
                // The dialog has no control for an independent stream yet;
                // there is nothing here that would use one until accumulation
                // lands, and a knob with no effect is worse than none.
                chaos_seed: crate::gpu::points::compute::DEFAULT_SEED,
                isolate: None,
                spp,
                // Inherited from the Render window, like `exposure` above,
                // rather than duplicated as three more sliders in the dialog:
                // the grade is the one thing here you can already see live, so
                // the job's job is to match what you were looking at.
                grade,
                // No re-grading from the dialog yet; slice 5a is CLI-side.
                grade_out: None,
                checkpoint_out: None,
                resume_from: None,
                density_estimation,
                // The dialog has no volume render yet (it accumulates by
                // default, which volume mode cannot); see the form's note.
                opacity: None,
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
                // Handled inline above, and it writes a file with no render
                // in it, so there is nothing to have been cut short.
                JobKind::ViewDescriptor => Ok(crate::render_job::Outcome::Complete),
            };
            let _ = control.events.send(JobEvent::Done(
                result.map(|outcome| (out.clone(), outcome)).map_err(|e| e.to_string()),
            ));
        });

        self.job = Some(handle);
    }

    /// Drain the running job's event queue into the handle the dialog reads.
    /// Called once per frame from `update`.
    fn poll_job(&mut self) {
        use crate::render_job::{JobEvent, Outcome, CANCELLED};
        let Some(job) = &mut self.job else { return };
        // Before draining, not after: the countdown is a filter over time, and
        // it wants the estimate as it stood over the frame that just passed.
        job.tick_estimate();
        let mut finished = None;
        loop {
            let event = match job.events.try_recv() {
                Ok(e) => e,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                // The sender is gone without having sent `Done`, which means
                // the render thread died — a panic inside wgpu, most likely.
                //
                // Before this, that left the dialog waiting on a message that
                // was never coming: the bar frozen at its last value, no error,
                // no way to tell a hung render from a dead one, and the only
                // clue in a terminal the window may not even have. A job that
                // cannot finish must still *end*.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // Only when the job did *not* report a result. A finished
                    // job drops its sender the moment after sending `Done`, so
                    // both arrive in the same drain and treating that as a
                    // failure would turn every successful render into an error.
                    if finished.is_some() {
                        break;
                    }
                    finished = Some(Err(String::from(
                        "the render thread stopped unexpectedly — if it ran out of GPU memory                          the details are on stderr. Try fewer points, less supersampling, or a                          smaller output.",
                    )));
                    break;
                }
            };
            match event {
                JobEvent::Phase(p) => {
                    // A new phase resets the within-phase counter but never the
                    // overall bar: the ledger derives that from which phase
                    // this is, so it keeps climbing across the boundary.
                    job.phase = p;
                    job.done = 0;
                    job.total = 0;
                    match p.position() {
                        Some(pos) => job.log.push(format!("— {} ({})", p.label(), pos)),
                        None => job.log.push(format!("— {}", p.label())),
                    }
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
                Ok((p, Outcome::Complete)) => log::info!("Render job finished: {}", p.display()),
                Ok((p, Outcome::Partial)) => {
                    log::info!("Render job stopped early; partial result written: {}", p.display())
                }
                Err(e) if e == CANCELLED => log::info!("Render job cancelled, nothing written"),
                Err(e) => log::error!("Render job failed: {}", e),
            }
            if let Some(job) = self.job.take() {
                self.job_done = Some(crate::render_job::FinishedJob {
                    result,
                    secs: job.working_elapsed(),
                    params: job.params,
                    log: job.log,
                    seen: false,
                });
            }
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
    pub fn job_done(&self) -> Option<&crate::render_job::FinishedJob> {
        self.job_done.as_ref()
    }

    /// The dialog has shown the finished job, so the status bar stops
    /// announcing it.
    pub fn mark_job_seen(&mut self) {
        if let Some(done) = self.job_done.as_mut() {
            done.seen = true;
        }
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

        let (mut traces, trace_layers) = crate::trace::generate_layer_traces(
            &self.scene,
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
        for (trace, &layer) in traces.iter().zip(&trace_layers) {
            let colormap = self.scene.layer_colour(layer).2;
            for (i, pair) in trace.windows(2).enumerate() {
                // Age fade along the trace: oldest segments nearly invisible
                let age = (i + 1) as f32 / (trace.len() - 1) as f32;
                let alpha = 0.08 + 0.72 * age * age;
                for step in pair {
                    let cm = colormap
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
        // The held axis is part of the key, not just of the drawing. A drag
        // bumps `matrix_generation` every frame so the dash keeps up on its
        // own, but *releasing* changes nothing else — without the axis here the
        // last frame's key would still match and the dashes would stay on
        // screen after the grab ended.
        let held_axis = match self.drag {
            Drag::Gizmo {
                mode: GizmoDragMode::ScaleAxis { k, dash_pitch, dash_reach, .. },
                ..
            } => Some((k, dash_pitch.to_bits(), dash_reach.to_bits())),
            _ => None,
        };
        // Keyed on the gizmo list's own index, with the subscene folded into
        // the generation so that switching between a subscene's gizmo and
        // its maps' (both index 0) still rebuilds.
        let key = (
            self.gizmo_selection().map(|i| {
                (i, self.matrix_generation ^ ((self.selected_subscene().unwrap_or(0) as u64) << 56))
            }),
            held_axis,
        );
        if key == self.indicator_key {
            return;
        }
        self.indicator_key = key;
        let key = key.0;
        let world = self.gizmo_matrices();
        let mut verts = match key {
            Some((i, _)) if i < world.len() => crate::indicators::build_rotation(world[i]),
            _ => Vec::new(),
        };

        // The axis being scaled, extended past both ends of itself.
        if let (Some((i, _)), Some((k, pitch, reach))) = (key, held_axis) {
            if let Some(m) = world.get(i) {
                verts.extend(crate::indicators::build_axis_extension(
                    *m,
                    k,
                    f32::from_bits(pitch),
                    f32::from_bits(reach),
                ));
            }
        }

        // The selected map's symmetry, drawn into the scene: its axis and fold
        // count, or the polyhedral group's cage. Sized to the attractor rather
        // than to the map, because the group acts on the whole form.
        //
        // Only for the selected transform, and only alongside its own
        // indicators — drawing every group in the scene at once would be a
        // thicket, which is the same reason `build` is selection-scoped.
        let mut ghosts: Vec<Mat4> = Vec::new();
        if let Some((i, _)) = key.filter(|_| self.selected_subscene().is_none()) {
            if let Some(spec) = self.scene.transforms.get(i) {
                if let Some(sym) = spec.symmetry.as_ref() {
                    let radius = self.attractor.map_or(1.0, |a| a.radius).max(0.05);
                    verts.extend(crate::indicators::build_symmetry(sym, radius));
                    // Element 0 is the identity, and that copy is already drawn
                    // solid as the selected transform's own gizmo. The orbit
                    // is the subscene's own, so it is placed like the map is.
                    let placement = self.scene.layer_placement(spec.layer as usize);
                    ghosts = sym.orbit(spec.matrix).skip(1).map(|g| placement * g).collect();
                }
            }
        }
        self.gizmo_renderer.set_ghosts(&self.gpu.queue, &ghosts);
        // The selected gizmo also gets drawn ignoring depth, so a dense
        // attractor can't hide the thing you're editing while `pick.rs` — which
        // has no depth awareness at all — goes on offering it to the cursor.
        let xray = key.map(|(i, _)| i).and_then(|i| world.get(i).map(|m| (i, *m)));
        self.gizmo_renderer.set_xray(&self.gpu.queue, xray);
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

    // === Evolutionary exploration (U / Shift+U) ===

    /// The `--random-config` that gets the dice's current settings back from
    /// the command line, with its leading space, or nothing for the defaults:
    /// the profile's name while it is unchanged, else prefs.toml, which holds
    /// the settings as they are.
    pub fn dice_config_flag(&self) -> String {
        let p = &self.dice_profile;
        match (&p.entry.path, p.modified(&self.prefs.random)) {
            (None, false) if p.entry.name == crate::dice::profile::BUILTIN => String::new(),
            (_, false) => format!(" --random-config {}", p.entry.name),
            (_, true) => " --random-config ~/.config/fracturize/prefs.toml".to_string(),
        }
    }

    /// Replace the whole scene with a freshly generated random flame
    /// (`src/dice/`), rolled under the random settings — see
    /// `adopt_scene` for why that's an edit rather than a load.
    ///
    /// When nothing meets the settings the scene stays as it was and the
    /// status bar says why. The generator's old habit of handing back its last
    /// candidate anyway was right for the defaults, which nearly always pass;
    /// with bounds set, a flame outside them is a flame the person asked not
    /// to see.
    pub fn random_flame(&mut self) {
        let seed = crate::random_list::fresh_seed();
        match crate::dice::roll_seeded(&self.prefs.random, &self.scene, seed) {
            Ok(r) => {
                log::info!(
                    "Random flame seed {seed}: {} transforms, {} tries (reproduce with --random --seed {seed}{})",
                    r.scene.transforms.len(),
                    r.attempts,
                    self.dice_config_flag(),
                );
                self.adopt_scene(r.scene, &format!("Random flame (seed {seed})"));
            }
            Err(f) => self.set_notice(format!("Random flame: {}. Kept the scene you had.", f.message()), Tone::Error),
        }
    }

    // === Random flames list and settings ===

    pub fn random_settings(&self) -> &crate::dice::RandomSettings {
        &self.prefs.random
    }

    /// Change what the dice roll. Persisted; the Random flames list starts
    /// over once the settings stop moving.
    pub fn set_random_settings(&mut self, settings: crate::dice::RandomSettings) {
        let settings = settings.normalized();
        if settings == self.prefs.random {
            return;
        }
        self.prefs.random = settings;
        self.prefs_dirty_since = Some(std::time::Instant::now());
        self.random_list.settings_changed(&self.prefs.random);
    }

    // === Dice profiles: the settings, saved and named ===

    /// The profile the settings came from.
    pub fn dice_profile(&self) -> &crate::dice::profile::Selected {
        &self.dice_profile
    }

    /// Replace the settings with a profile's. Not undoable — these are
    /// preferences, not the scene — but Revert gets the profile back, and the
    /// settings it replaced can be had again by loading their own profile.
    pub fn load_dice_profile(&mut self, entry: crate::dice::profile::Entry) {
        match crate::dice::profile::load(&entry) {
            Ok(loaded) => {
                if !loaded.warnings.is_empty() {
                    self.set_notice(format!("Profile {}: {}", entry.name, loaded.warnings.join("; ")), Tone::Error);
                }
                self.prefs.random_profile = entry.path.clone();
                self.prefs.random_profile_builtin = entry.is_builtin().then(|| entry.name.clone());
                self.dice_profile = crate::dice::profile::Selected {
                    entry,
                    description: loaded.meta.description.clone(),
                    baseline: Some(loaded.settings.clone()),
                };
                self.set_random_settings(loaded.settings);
                self.prefs_dirty_since = Some(std::time::Instant::now());
            }
            Err(e) => self.set_notice(format!("Couldn't load the profile: {e}"), Tone::Error),
        }
    }

    /// Back to what the profile says.
    pub fn revert_dice_profile(&mut self) {
        let entry = self.dice_profile.entry.clone();
        self.load_dice_profile(entry);
    }

    /// Write the settings back into the profile they came from. Only the
    /// person's own profiles are written in place; anything else is a Save as.
    pub fn save_dice_profile(&mut self) -> Result<(), String> {
        if !self.dice_profile.writable() {
            return Err(format!("'{}' can't be saved over — use Save as", self.dice_profile.entry.name));
        }
        let entry = self.dice_profile.entry.clone();
        let path = entry.path.clone().unwrap_or_default();
        // Keep what the file says about itself; the merge keeps its comments.
        let meta = crate::dice::profile::load(&entry).map(|l| l.meta).unwrap_or_else(|_| {
            crate::dice::profile::Meta { name: entry.name.clone(), ..Default::default() }
        });
        crate::dice::profile::save(&path, &self.prefs.random, &meta)?;
        self.dice_profile.baseline = Some(self.prefs.random.clone());
        log::info!("Dice profile saved to {}", path.display());
        Ok(())
    }

    /// Save the settings as a new profile of the person's own, and make it
    /// the current one. Refuses to replace a different existing profile: Save
    /// as is how a profile is forked, and a fork that silently overwrote
    /// another one would lose it.
    pub fn save_dice_profile_as(&mut self, name: &str) -> Result<(), String> {
        let name = crate::dice::profile::check_name(name)?;
        let path = crate::dice::profile::user_path(&name).ok_or("no config folder to save profiles in")?;
        if path.exists() && self.dice_profile.entry.path.as_deref() != Some(path.as_path()) {
            return Err(format!("there's already a profile called '{name}' — load it to change it"));
        }
        let meta = crate::dice::profile::Meta {
            name: name.clone(),
            description: String::new(),
            author: self.prefs.author.clone().unwrap_or_default(),
        };
        crate::dice::profile::save(&path, &self.prefs.random, &meta)?;
        self.prefs.random_profile = Some(path.clone());
        self.prefs.random_profile_builtin = None;
        self.dice_profile = crate::dice::profile::Selected {
            entry: crate::dice::profile::entry_for(&path),
            description: String::new(),
            baseline: Some(self.prefs.random.clone()),
        };
        self.prefs_dirty_since = Some(std::time::Instant::now());
        log::info!("Dice profile saved as {}", path.display());
        Ok(())
    }

    /// Throw the list away and roll a fresh one.
    pub fn reroll_random_list(&mut self) {
        self.random_list.reroll(&self.prefs.random, &self.scene);
    }

    /// Collect finished rolls and keep `want` rows rolled or on their way.
    pub fn poll_random_list(&mut self, want: usize) {
        self.random_list.poll(&self.prefs.random, &self.scene, want);
    }

    /// Open row `index` of the Random flames list: an edit, like the dice.
    pub fn adopt_random(&mut self, index: usize) {
        let Some(e) = self.random_list.entries.get(index) else { return };
        let (scene, seed) = (e.scene.clone(), e.seed);
        log::info!("Random flame seed {seed}, from the list");
        self.adopt_scene(scene, &format!("Random flame (seed {seed})"));
    }

    /// The list, the thumbnail cache and the UI state, borrowed together so a
    /// row can ask for its thumbnail while it draws.
    pub fn random_list_parts(
        &mut self,
    ) -> (&crate::random_list::RandomList, &mut crate::gpu::thumbs::ThumbCache, &mut crate::ui::UiState) {
        (&self.random_list, &mut self.thumbs, &mut self.ui_state)
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
        self.camera = self.camera_for(&scene);
        self.invalidate_default_path();
        self.rewind_camera_playhead();
        self.point_size = scene.point_size;
        self.color_falloff = scene.color_falloff;
        self.color_contrast = scene.color_contrast;
        self.haze_amount = scene.haze;
        self.exposure = scene.exposure;
        self.transform_enabled = vec![true; scene.transforms.len()];
        // As for a loaded scene, and doubly so for a blank one, which has no
        // transform 0 to select. See the note in `App::new`.
        self.selected_transform = None;
        self.selected_subscene = None;
        self.current_layer = scene.first_layer_with_maps();
        self.selected_variation = 0;
        self.drag = Drag::None;
        self.hovered = None;
        self.scene = scene;
        // A scene made in-app has no file behind it: Ctrl+S should write a new
        // scenes/untitled-*.toml rather than overwrite whatever was loaded.
        self.scene_path = None;
        self.scene_png = None;
        self.invalidate_saved_views();
        self.bump_matrix_generation();
        self.after_scene_shape_change();
        self.commit_edit(label, None, before);
    }

    /// Mutate the scene under the dice profile (U, the Mutate window). The
    /// child goes through the same gate a random flame does, judged against
    /// this scene, so a mutation that blows the attractor up or collapses it
    /// is never shown; if nothing passes in the profile's budget, the scene
    /// stays as it was and the status bar says why.
    pub fn mutate_scene(&mut self) {
        let seed = crate::random_list::fresh_seed();
        let r = match crate::dice::mutate_seeded(&self.prefs.random, &self.scene, seed) {
            Ok(r) => r,
            Err(f) => {
                self.set_notice(format!("Mutate: {}. Kept the scene you had.", f.message()), Tone::Error);
                return;
            }
        };
        log::info!(
            "Mutation seed {seed}: {} ({} tries{}; reproduce with --mutations 1 --seed {}{})",
            r.log.join("; "),
            r.attempts,
            r.strength.filter(|&s| s < self.prefs.random.mutation.strength).map_or(String::new(), |s| format!(
                ", strength backed off to {s:.2}"
            )),
            seed.wrapping_sub(1),
            self.dice_config_flag(),
        );
        let label = mutation_label("", &r.log);
        self.apply_mutation(r.scene, label);
    }

    /// Make tile `k` of the mutation grid the scene: the same edit U makes,
    /// labelled with what changed. The grid then starts over around it, at
    /// once; Ctrl+Z brings back the old parent, and its grid with it.
    pub fn adopt_mutation(&mut self, k: u32) {
        let grid = &self.mutation_grid;
        let Some(child) = grid.child(k) else { return };
        let mut scene = child.scene.clone();
        log::info!(
            "Mutation grid tile {k}, seed {}: {} (reproduce the grid with --mutations {} --seed {}{})",
            child.seed,
            child.log.join("; "),
            crate::mutation_grid::children(grid.size()),
            grid.base_seed,
            self.dice_config_flag(),
        );
        let label = mutation_label(&format!("Grid {k}: "), &child.log);
        // The tiles were framed by the live camera; the scene's own framing
        // is the person's, and picking a tile isn't a reason to move it.
        scene.set_camera(&self.scene.camera());
        self.apply_mutation(scene, label);
        self.mutation_grid.expire();
    }

    /// A mutated scene in place of this one, as one edit.
    fn apply_mutation(&mut self, scene: Scene, label: String) {
        let before = self.edit_snapshot();
        self.scene = scene;
        self.bump_matrix_generation();
        self.after_scene_shape_change();
        self.commit_edit(label, None, before);
    }

    /// Bring the mutation grid up to date with the scene on screen and
    /// collect its finished tiles. Called while the Mutate window is open.
    pub fn poll_mutation_grid(&mut self) {
        self.mutation_grid.poll(
            &self.prefs.random,
            &self.scene,
            &self.camera,
            self.history.top_serial(),
            self.scene_loads,
            self.prefs.mutation_grid,
        );
    }

    /// New mutations of the same scene.
    pub fn reroll_mutation_grid(&mut self) {
        self.mutation_grid.reroll(&self.prefs.random, &self.scene, &self.camera, self.prefs.mutation_grid);
    }

    /// Tiles a side of the mutation grid: 3 or 5.
    pub fn mutation_grid_size(&self) -> u32 {
        self.prefs.mutation_grid
    }

    /// 3 or 5 tiles a side.
    pub fn set_mutation_grid_size(&mut self, n: u32) {
        if crate::mutation_grid::SIZES.contains(&n) && n != self.prefs.mutation_grid {
            self.prefs.mutation_grid = n;
            self.prefs_dirty_since = Some(std::time::Instant::now());
        }
    }

    /// The grid, the thumbnail cache and the UI state, borrowed together so a
    /// tile can ask for its thumbnail while it draws.
    pub fn mutation_grid_parts(
        &mut self,
    ) -> (&crate::mutation_grid::MutationGrid, &mut crate::gpu::thumbs::ThumbCache, &mut crate::ui::UiState) {
        (&self.mutation_grid, &mut self.thumbs, &mut self.ui_state)
    }

    /// Reconcile app state after the scene changed under us (mutation/undo
    /// may add or remove transforms) and rebuild the GPU pipelines
    fn after_scene_shape_change(&mut self) {
        let n = self.scene.transforms.len();
        self.transform_enabled.resize(n, true);
        if let Some(sel) = self.selected_transform {
            self.selected_transform = Some(sel.min(n.saturating_sub(1)));
        }
        // Undo can take away the subscene that was selected or listed.
        if self.selected_subscene.is_some_and(|k| k > self.scene.subscenes.len()) {
            self.selected_subscene = None;
        }
        if self.current_layer >= self.scene.layer_count() {
            self.current_layer = self.scene.first_layer_with_maps();
        }
        self.drag = Drag::None;
        self.hovered = None;
        self.rebuild_pipelines();
    }

    // === Transform editing (see also gizmo drags above) ===

    /// The transform editing keys act on: the explicit selection, or T0
    pub fn selection(&self) -> Option<usize> {
        let n = self.scene.transforms.len();
        // With a whole subscene selected the keys have no map to act on;
        // falling back to T0 would edit something nobody is looking at.
        if n == 0 || self.selected_subscene().is_some() {
            return None;
        }
        Some(self.selected_transform.unwrap_or(0).min(n - 1))
    }

    /// Whether every map in the layer the Transforms window shows still has
    /// the colour and colour index its position implies.
    fn layer_colours_implied(&self) -> bool {
        let layer = self.current_layer() as u16;
        self.scene
            .implied_colours()
            .iter()
            .zip(&self.scene.transforms)
            .filter(|(_, t)| t.layer == layer)
            .all(|(&(c, i), _)| c && i)
    }

    /// Add a transform: a nudged copy of the selection, or (fresh) a small
    /// default one. Rebuilds the GPU pipelines for the new transform count.
    pub fn add_transform(&mut self, fresh: bool) {
        let before = self.edit_snapshot();
        let copy_of = if fresh { None } else { self.selection() };
        let (spec, name, color) = if fresh || self.selection().is_none() {
            (
                TransformSpec {
                    // A fresh map joins whichever layer the Transforms window
                    // is showing.
                    layer: self.current_layer() as u16,
                    matrix: Mat4::from_scale_rotation_translation(
                        Vec3::splat(0.5),
                        glam::Quat::IDENTITY,
                        Vec3::new(0.3, 0.3, 0.0),
                    ),
                    post_affine: Mat4::IDENTITY,
                    color_value: 0.5,
                    weight: 1.0,
                    color_speed: self.scene.color_speed,
                    explicit_color_speed: None,
                    variations: TransformSpec::linear_variations(),
                    // A fresh map joins no group. Duplicating one does inherit
                    // its symmetry, because that comes from cloning the spec —
                    // which is the behaviour you want either way round: "add"
                    // is how you build the deliberate defect (CRAFT §3.6), and
                    // "dup" is how you add a second motif to a group.
                    symmetry: None,
                    xaos: Default::default(),
                },
                None,
                // In a layer that never chose its colours, the new map doesn't
                // either: it takes the hue its position implies, and the rest
                // re-space round it. Otherwise it is the usual pale grey.
                (!self.layer_colours_implied()).then_some(Vec3::splat(0.9)),
            )
        } else {
            let idx = self.selection().unwrap();
            let mut spec = self.scene.transforms[idx].clone();
            // Nudge so the copy's gizmo doesn't sit exactly on the original
            spec.matrix.w_axis += glam::Vec4::new(0.15, 0.0, 0.0, 0.0);
            let name = self.scene.transform_names[idx]
                .as_ref()
                .map(|n| format!("{} copy", n));
            (spec, name, Some(self.scene.colors[idx]))
        };

        // At the end of its own layer's run, so the list stays grouped by
        // layer. For an ordinary scene that is the end, as it always was.
        let at = self
            .scene
            .transforms
            .iter()
            .rposition(|t| t.layer <= spec.layer)
            .map_or(0, |i| i + 1);
        self.scene.insert_transform(at, spec, name, color, copy_of);
        self.transform_enabled.insert(at, true);
        self.selected_transform = Some(at);
        self.selected_subscene = None;
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
        if !self.can_delete_transform(idx) {
            log::warn!("Cannot delete the last transform of {}", self.scene.layer_name(self.scene.transforms[idx].layer as usize));
            return;
        }
        let before = self.edit_snapshot();
        self.scene.remove_transform(idx);
        self.transform_enabled.remove(idx);
        self.selected_transform = Some(idx.min(self.scene.transforms.len() - 1));
        self.drag = Drag::None;
        self.bump_matrix_generation();
        self.scene.regenerate_colormap();
        self.rebuild_pipelines();
        log::info!("Deleted transform T{}", idx);
        self.commit_edit("Delete transform", None, before);
    }

    /// Whether map `idx` may be deleted: not the scene's last, and not a
    /// subscene's last (remove the subscene instead — an empty subscene is
    /// one that can't be saved). The scene's own layer may empty out in a
    /// composite, which is then a pure composite.
    pub fn can_delete_transform(&self, idx: usize) -> bool {
        let Some(t) = self.scene.transforms.get(idx) else { return false };
        let k = t.layer as usize;
        self.scene.transforms.len() > 1 && (self.scene.layer_sizes()[k] > 1 || (k == 0 && self.scene.is_composite()))
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
        self.push_colormap();
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
    /// and "Remove shear" all funnel through this.
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
        self.hold_dimension_through(idx, matrix);
        self.scene.transforms[idx].matrix = matrix;
        self.bump_matrix_generation();
        self.sync_transforms_to_gpu();
        self.commit_edit(label, coalesce_key.as_deref(), before);
    }

    /// Set a transform's post-affine slot — the matrix applied *after* the
    /// variation blend (see `scene::TransformSpec::post_affine`).
    ///
    /// Its own entry point rather than a flag on `set_transform_matrix`
    /// because the two halves are edited by different controls and want
    /// different history labels, and because the dimension lock has to see the
    /// right pair: the post-affine carries contraction too, so changing it
    /// moves `d` exactly as a pre-affine scale does.
    pub fn set_transform_post_affine(
        &mut self,
        idx: usize,
        post: Mat4,
        label: impl Into<String>,
        coalesce_key: Option<String>,
    ) {
        if idx >= self.scene.transforms.len() {
            return;
        }
        let before = self.edit_snapshot();
        self.hold_dimension_through_post(idx, post);
        self.scene.transforms[idx].post_affine = post;
        self.bump_matrix_generation();
        self.sync_transforms_to_gpu();
        self.commit_edit(label, coalesce_key.as_deref(), before);
    }

    /// The number of maps the attractor is actually made of: every transform
    /// counted once, or `|G|` times if it is a motif.
    pub fn effective_map_count(&self) -> usize {
        self.scene
            .transforms
            .iter()
            .map(|t| t.symmetry.as_ref().map_or(1, |g| g.order()))
            .sum()
    }

    /// Enrol a transform in a symmetry group, change the group it is in, or
    /// (with `None`) withdraw it.
    ///
    /// **The undo label carries the arithmetic**, and that is not decoration.
    /// "Edited transform" is a lie about an edit that took a scene from 15 maps
    /// to 180 — it reads like a nudge and undoes a transformation. History here
    /// snapshots whole scenes, so nothing about this is structurally expensive;
    /// the only thing that can be got wrong is what it *says*, and this is the
    /// one edit in the program whose magnitude isn't visible from its name.
    pub fn set_transform_symmetry(
        &mut self,
        idx: usize,
        symmetry: Option<crate::symmetry::Symmetry>,
    ) {
        if idx >= self.scene.transforms.len() {
            return;
        }
        let before = self.edit_snapshot();
        let was = self.scene.transforms[idx]
            .symmetry
            .as_ref()
            .map(|s| s.label())
            .unwrap_or_else(|| "none".to_string());
        let now = symmetry.as_ref().map(|s| s.label()).unwrap_or_else(|| "none".to_string());
        let maps_before = self.effective_map_count();

        self.scene.transforms[idx].symmetry = symmetry;
        let maps_after = self.effective_map_count();

        // The gizmo ghosts and the drawn axis are keyed on this, so a group
        // change has to bump it or the 3D view keeps drawing the old orbit.
        self.bump_matrix_generation();
        self.sync_transforms_to_gpu();

        let label = if maps_before == maps_after {
            format!("Symmetry {} \u{E06C} {}", was, now)
        } else {
            format!(
                "Symmetry {} \u{E06C} {} ({} \u{E06C} {} transforms in total)",
                was, now, maps_before, maps_after
            )
        };
        // Deliberately not coalesced. Each of these is a discrete structural
        // decision, and stepping back through them one at a time is the point;
        // a group picker clicked three times should be three undo steps, unlike
        // a slider dragged for three seconds.
        self.commit_edit(label, None, before);
    }

    /// The dimension-lock gate for a post-affine edit. Same shape as
    /// [`Self::hold_dimension_through`], with the roles of the two matrices
    /// swapped: here the pre-affine is the factor that stays put.
    fn hold_dimension_through_post(&mut self, idx: usize, new_post: Mat4) {
        if !self.ui_state.dimension_lock {
            return;
        }
        let Some(spec) = self.scene.transforms.get(idx) else { return };
        if !(spec.weight > 0.0) {
            return;
        }
        let pre_det = spec.matrix.determinant();
        let old_s = (pre_det * spec.post_affine.determinant()).abs().powf(1.0 / 3.0);
        let new_s = (pre_det * new_post.determinant()).abs().powf(1.0 / 3.0);
        let participates = |s: f32| s > 0.0 && s < 1.0;
        if !participates(old_s) || !participates(new_s) || (old_s - new_s).abs() <= 1e-6 {
            return;
        }
        self.apply_dimension_lock(idx, old_s, new_s);
    }

    /// Gate for the dimension lock: decide whether replacing transform `idx`'s
    /// matrix with `new` is an edit the lock should balance, and balance it.
    ///
    /// Call *before* storing `new` — the balance is computed against the state
    /// the edit is leaving. A no-op when the lock is off, which is why every
    /// matrix-setting path can call it unconditionally.
    fn hold_dimension_through(&mut self, idx: usize, new: Mat4) {
        if !self.ui_state.dimension_lock {
            return;
        }
        let Some(spec) = self.scene.transforms.get(idx) else { return };

        // The edited map has to be one of the maps `d` is computed from, or
        // the balance below is solving against a sum it isn't part of. Same
        // filter as `similarity_dimension`.
        if !(spec.weight > 0.0) {
            return;
        }
        // Both sides have to include the post-affine slot, or a map that keeps
        // its scale there is measured as if it did not contract. Only the
        // pre-affine is being replaced, so the post-affine determinant is the
        // same factor on each side.
        let post_det = spec.post_affine.determinant();
        let old_s = (spec.matrix.determinant() * post_det).abs().powf(1.0 / 3.0);
        let new_s = (new.determinant() * post_det).abs().powf(1.0 / 3.0);
        let participates = |s: f32| s > 0.0 && s < 1.0;
        if !participates(old_s) || !participates(new_s) {
            return;
        }
        // Rotation and translation edits come through here too and leave the
        // determinant alone; there is nothing to balance.
        if (old_s - new_s).abs() <= 1e-6 {
            return;
        }
        self.apply_dimension_lock(idx, old_s, new_s);
    }

    /// Rescale every *other* map so the similarity dimension survives a change
    /// to map `edited_idx`'s contraction. See
    /// [`crate::scene::dimension_lock_factor`] for the balance itself.
    ///
    /// `d` is re-measured from the live transforms on every call rather than
    /// anchored at the start of a drag, and that is deliberate: the balance has
    /// `d` as an exact fixed point, so re-measuring re-anchors to the truth
    /// each frame instead of accumulating the drift a held target would.
    fn apply_dimension_lock(&mut self, edited_idx: usize, old_s: f32, new_s: f32) {
        // Under xaos the dimension is a spectral radius over the transition
        // graph, not `Σ sᵈ`, and no single factor on the other maps balances
        // it. The status bar greys the lock and says so.
        if crate::xaos::layer_has_xaos(&self.measured_maps().0) {
            return;
        }
        let Some(d) = crate::scene::similarity_dimension(&self.scene.transforms) else {
            return;
        };

        let Some(factor) = crate::scene::dimension_lock_factor(d, old_s, new_s) else {
            // No balance exists — the edited map was already the whole sum, or
            // the new scale would need the others to vanish. Leave the scene be.
            return;
        };

        for (i, t) in self.scene.transforms.iter_mut().enumerate() {
            if i == edited_idx {
                continue;
            }
            // Only maps that *count toward* `d` may be moved to hold it. A
            // zero-weight map contributes nothing to the sum, so rescaling it
            // would silently reshape a map that is not part of the balance;
            // an expanding one is excluded from `d` entirely, and shrinking it
            // under 1.0 would make it join the sum and step `d` discontinuously.
            // The filter has to match `similarity_dimension`'s exactly.
            let s = t.linear_determinant().abs().powf(1.0 / 3.0);
            if !(t.weight > 0.0 && s > 0.0 && s < 1.0) {
                continue;
            }
            // Scale the linear part in place rather than decomposing. A
            // `to_scale_rotation_translation` round trip silently discards
            // shear and mangles a mirrored (det<0) matrix — the repo carries
            // `Trs::is_faithful` precisely because that decomposition is not
            // always honest, and the inspector falls back to a raw matrix grid
            // when it isn't. Scaling the three basis columns multiplies the
            // determinant by `factor³` and so the contraction by `factor`,
            // which is the whole intent, and it is what the gizmo's own
            // uniform-scale drag already does. `w_axis` is left alone, matching
            // that drag: scaling a map changes its size, not its offset.
            t.matrix.x_axis *= factor;
            t.matrix.y_axis *= factor;
            t.matrix.z_axis *= factor;
        }
    }

    /// Discard any shear/mirroring in a transform's matrix by rebuilding it
    /// from its own (possibly approximate) scale/rotation/translation
    /// decomposition — the inspector's "Remove shear" button. Always
    /// its own history entry (no coalescing), since it's an explicit,
    /// infrequent action.
    pub fn orthogonalize_transform(&mut self, idx: usize) {
        let Some(spec) = self.scene.transforms.get(idx) else { return };
        let (scale, rotation, translation) = spec.matrix.to_scale_rotation_translation();
        let orthogonalized = Mat4::from_scale_rotation_translation(scale, rotation, translation);
        self.set_transform_matrix(idx, orthogonalized, format!("Remove shear T{}", idx), None);
    }

    /// Rename a transform (inspector name field / list row's inline rename).
    // === Xaos: which map may follow which (see crate::xaos) ===

    /// The chain of `layer`'s maps, built once per change to its rows, weights
    /// or enabled flags. Built for a layer without xaos too — its rows are the
    /// plain weights — so the inspector can show probabilities before the
    /// first edge is ever cut.
    pub fn xaos_summary(&mut self, layer: usize) -> Option<std::rc::Rc<XaosSummary>> {
        use std::hash::{Hash, Hasher};
        let idx: Vec<usize> =
            (0..self.scene.transforms.len()).filter(|&i| self.scene.transforms[i].layer as usize == layer).collect();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        layer.hash(&mut h);
        for &i in &idx {
            let t = &self.scene.transforms[i];
            i.hash(&mut h);
            t.weight.to_bits().hash(&mut h);
            self.is_transform_enabled(i).hash(&mut h);
            t.xaos.rest.to_bits().hash(&mut h);
            for e in &t.xaos.exceptions {
                (e.0, e.1.to_bits()).hash(&mut h);
            }
        }
        let key = h.finish();
        if let Some((k, s)) = &self.xaos_cache {
            if *k == key {
                return Some(s.clone());
            }
        }
        let maps: Vec<TransformSpec> = idx.iter().map(|&i| self.scene.transforms[i].clone()).collect();
        let enabled: Vec<bool> = idx.iter().map(|&i| self.is_transform_enabled(i)).collect();
        let chain = crate::xaos::Chain::build(&maps, &enabled)?;
        let shares = chain.shares();
        let summary = std::rc::Rc::new(XaosSummary {
            has_xaos: crate::xaos::layer_has_xaos(&maps),
            idx,
            chain,
            shares,
        });
        self.xaos_cache = Some((key, summary.clone()));
        Some(summary)
    }

    /// The overlay edge under the pointer, if the overlay is showing.
    fn xaos_edge_under_cursor(&mut self) -> Option<(usize, usize)> {
        if !crate::ui::xaos::overlay_active(self) {
            return None;
        }
        let (edges, _) = crate::ui::xaos::overlay_edges(self);
        crate::ui::xaos::pick_edge(&edges, self.cursor)
    }

    /// Run one xaos edit: snapshot, change the rows, re-upload, commit.
    fn xaos_edit(&mut self, label: String, key: Option<String>, edit: impl FnOnce(&mut Scene)) {
        let before = self.edit_snapshot();
        edit(&mut self.scene);
        self.sync_transforms_to_gpu();
        self.commit_edit(label, key.as_deref(), before);
    }

    fn short_name(&self, i: usize) -> String {
        self.scene.transform_names.get(i).cloned().flatten().unwrap_or_else(|| format!("T{i}"))
    }

    /// Set the multiplier from map `from` to map `to` (scene indices, one
    /// layer). A drag lands as one undo step per edge.
    pub fn set_xaos_edge(&mut self, from: usize, to: usize, v: f32) {
        self.set_xaos_edges(&[(from, to, v)], None);
    }

    /// Several edges as one edit — the matrix's paint stroke and the
    /// viewport knife. `label` names it in History; one edge names itself.
    pub fn set_xaos_edges(&mut self, edges: &[(usize, usize, f32)], label: Option<String>) {
        let n = self.scene.transforms.len();
        let edges: Vec<(usize, usize, f32)> = edges
            .iter()
            .copied()
            .filter(|&(f, t, _)| f < n && t < n && self.scene.transforms[f].layer == self.scene.transforms[t].layer)
            .map(|(f, t, v)| (f, t, if v.is_finite() { v.max(0.0) } else { 1.0 }))
            .collect();
        let Some(&(f0, t0, v0)) = edges.first() else { return };
        let (label, key) = match (label, edges.len()) {
            (Some(l), _) => (l, None),
            (None, _) => (
                format!("Xaos {} \u{E06C} {} ×{}", self.short_name(f0), self.short_name(t0), crate::ui::num::fixed(v0, 2)),
                Some(format!("xaos:T{f0}>T{t0}")),
            ),
        };
        self.xaos_edit(label, key, |scene| {
            for (f, t, v) in edges {
                let pos = scene.layer_position(t);
                scene.transforms[f].xaos.set(pos, v);
            }
        });
    }

    /// Set the multiplier for every target `from`'s row doesn't list.
    pub fn set_xaos_rest(&mut self, from: usize, c: f32) {
        if from >= self.scene.transforms.len() {
            return;
        }
        let c = if c.is_finite() { c.max(0.0) } else { 1.0 };
        let label = format!("Xaos {} \u{E06C} every other ×{}", self.short_name(from), crate::ui::num::fixed(c, 2));
        self.xaos_edit(label, Some(format!("xaos:T{from}>*")), |scene| scene.transforms[from].xaos.set_rest(c));
    }

    /// Back to "any map may follow".
    pub fn reset_xaos_row(&mut self, from: usize) {
        if from >= self.scene.transforms.len() {
            return;
        }
        let label = format!("Xaos {}: reset", self.short_name(from));
        self.xaos_edit(label, None, |scene| scene.transforms[from].xaos = Default::default());
    }

    /// Make `to` follow `from` with probability `p`, holding the rest of the
    /// row: solved for the multiplier exactly (`xaos::multiplier_for`). `p = 1`
    /// is "only this exit". Does nothing when no multiplier can do it.
    pub fn set_xaos_probability(&mut self, from: usize, to: usize, p: f32) {
        let Some(sum) = self.xaos_summary(self.scene.transforms.get(from).map_or(0, |t| t.layer as usize)) else {
            return;
        };
        let (Some(i), Some(j)) = (sum.idx.iter().position(|&x| x == from), sum.idx.iter().position(|&x| x == to)) else {
            return;
        };
        let maps: Vec<TransformSpec> = sum.idx.iter().map(|&k| self.scene.transforms[k].clone()).collect();
        match crate::xaos::multiplier_for(&maps, i, j, p as f64) {
            Some(x) if x.is_infinite() => {
                let label = format!("Xaos {}: only \u{E06C} {}", self.short_name(from), self.short_name(to));
                self.xaos_edit(label, None, |scene| {
                    let mut row = crate::xaos::Xaos::default();
                    row.set_rest(0.0);
                    row.set(j, 1.0);
                    scene.transforms[from].xaos = row;
                });
            }
            Some(x) => self.set_xaos_edge(from, to, x),
            None => {}
        }
    }

    /// Write a preset's rows over every map of `layer`.
    pub fn apply_xaos_preset(&mut self, layer: usize, preset: crate::xaos::Preset) {
        let label = format!("Xaos preset: {}", preset.name());
        self.xaos_edit(label, None, |scene| {
            let idx: Vec<usize> =
                (0..scene.transforms.len()).filter(|&i| scene.transforms[i].layer as usize == layer).collect();
            for (row, &i) in preset.rows(idx.len()).into_iter().zip(&idx) {
                scene.transforms[i].xaos = row;
            }
        });
    }

    /// Apophysis 7X's linked transform: a new map that fires right after
    /// `after` and only there. `after`'s row becomes "only → the new map", the
    /// new map takes `after`'s old row (so the walk carries on as it would
    /// have), and every other row cuts the new map. It starts as the identity,
    /// so nothing moves until it is edited — it is a slot for a post-like
    /// effect on one map, which is how flame artists used it before final
    /// transforms existed.
    pub fn insert_linked_transform(&mut self, after: usize) {
        if after >= self.scene.transforms.len() {
            return;
        }
        let before = self.edit_snapshot();
        let layer = self.scene.transforms[after].layer;
        let old_row = self.scene.transforms[after].xaos.clone();
        let spec = TransformSpec {
            layer,
            matrix: Mat4::IDENTITY,
            post_affine: Mat4::IDENTITY,
            color_value: self.scene.transforms[after].color_value,
            weight: 1.0,
            color_speed: self.scene.color_speed,
            explicit_color_speed: None,
            variations: TransformSpec::linear_variations(),
            symmetry: None,
            xaos: old_row,
        };
        let name = Some(format!("{} link", self.short_name(after)));
        let color = self.scene.colors.get(after).copied().unwrap_or(Vec3::ONE);
        let at = self.scene.transforms.iter().rposition(|t| t.layer <= layer).map_or(0, |i| i + 1);
        self.scene.insert_transform(at, spec, name, Some(color), None);
        self.transform_enabled.insert(at, true);
        let after = if after >= at { after + 1 } else { after };
        let pos = self.scene.layer_position(at);
        for i in 0..self.scene.transforms.len() {
            if self.scene.transforms[i].layer == layer && i != after {
                self.scene.transforms[i].xaos.set(pos, 0.0);
            }
        }
        let mut only = crate::xaos::Xaos::default();
        only.set_rest(0.0);
        only.set(pos, 1.0);
        self.scene.transforms[after].xaos = only;
        self.selected_transform = Some(at);
        self.selected_subscene = None;
        self.bump_matrix_generation();
        self.scene.regenerate_colormap();
        self.rebuild_pipelines();
        self.commit_edit(format!("Linked map after {}", self.short_name(after)), None, before);
    }

    pub fn rename_transform(&mut self, idx: usize, name: Option<String>) {
        if idx >= self.scene.transform_names.len() {
            return;
        }
        let before = self.edit_snapshot();
        // Tracks name maps the way `-S` does: by name, or by index where there
        // is none. Carry them over, or a rename leaves them aimed at nothing.
        if self.scene.transforms[idx].layer == 0 {
            let refer = |n: &Option<String>| n.clone().unwrap_or_else(|| self.scene.layer_position(idx).to_string());
            let (old, new) = (refer(&self.scene.transform_names[idx]), refer(&name));
            crate::track::rename_map(&mut self.scene.tracks, &old, &new);
        }
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
        self.push_colormap();
        self.commit_edit(
            format!("Recolour T{}", idx),
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
            format!("Gradient position T{}", idx),
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
            format!("Colour speed T{}", idx),
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

    /// The transform the emphasis should be measuring this frame.
    ///
    /// Edit mode only, and only with something selected — in view mode there is
    /// no transform under the cursor and the glow would just be a stain on the
    /// artwork. Turning `show_emphasis` off is the way to edit against the
    /// picture you are going to keep.
    fn emphasis_target(&self) -> Option<u32> {
        // Nor while mute/solo is hiding points: the glow measures every point,
        // hidden ones included, and would light material that isn't drawn.
        if !self.show_emphasis || !self.show_gizmos || self.map_view.active() {
            return None;
        }
        self.selected_transform.map(|i| i as u32)
    }

    /// Point the chaos pass at that transform, and make sure the glow is
    /// reading the buffer the chaos pass is writing.
    fn sync_emphasis(&mut self) {
        let target = self.emphasis_target();
        self.point_compute.set_emphasis(&self.gpu.device, &self.gpu.queue, target);
        self.emphasis_renderer.rebind(
            &self.gpu.device,
            &self.point_compute.point_buffer,
            &self.point_compute.sensitivity_buffer,
            &self.point_compute.stats_buffer,
        );
    }

    /// Hand the mute/solo mask to the chaos pass and both renderers.
    fn sync_map_view(&mut self) {
        let n = self.scene.transforms.len();
        if n != self.map_view_count {
            self.map_view.clear();
            self.map_view_count = n;
        }
        let hidden = self.map_view.hidden(n);
        self.point_compute.set_hidden(&self.gpu.device, &self.gpu.queue, hidden.as_deref());
        let binding = self.point_compute.mask_binding();
        self.point_renderer.bind_mask(&self.gpu.device, &binding);
        self.splat_renderer.bind_mask(&self.gpu.device, &binding);
    }

    /// Mute or unmute map `i`'s points (a view; see `map_view`).
    pub fn toggle_map_mute(&mut self, i: usize) {
        self.map_view.toggle_mute(i);
    }

    /// Solo or unsolo map `i`'s points; with `only`, this map alone, or
    /// nothing soloed if it already was.
    pub fn toggle_map_solo(&mut self, i: usize, only: bool) {
        if only {
            self.map_view.solo_only(i);
        } else {
            self.map_view.toggle_solo(i);
        }
    }

    /// Whether the ring still holds points that predate the current emphasis
    /// target — measured against a transform that is no longer the one being
    /// asked about, and therefore not drawn at all until they are replaced.
    fn emphasis_filling(&self) -> bool {
        self.point_compute.emphasis_target().is_some()
            && self.point_compute.emphasis_window().1 < self.point_compute.buffer_capacity
    }

    /// Turn the emphasis glow on or off.
    pub fn toggle_emphasis(&mut self) {
        self.show_emphasis = !self.show_emphasis;
        log::info!(
            "Emphasis: {}",
            if self.show_emphasis { "on" } else { "off" }
        );
    }

    /// The chaos pass's view of the scene: its maps grouped by layer, the
    /// walker split between subscenes, and each layer's colour. Built from the
    /// live state, so a disabled transform or a hidden subscene is honoured.
    fn layer_table(&self) -> crate::gpu::points::compute::LayerTable {
        crate::gpu::points::compute::LayerTable::build(
            &self.scene,
            &self.transform_enabled,
            self.color_contrast,
        )
    }

    /// Recreate the GPU pipelines after the transform count changed.
    /// The chaos game restarts from warmup.
    fn rebuild_pipelines(&mut self) {
        self.scene.resolve_all_color_speeds(self.color_falloff);
        self.point_compute = PointCompute::with_pipelines(
            &self.gpu.device,
            &self.chaos_pipelines,
            &self.layer_table(),
            self.buffer_capacity,
            crate::gpu::points::compute::DEFAULT_SEED,
            crate::gpu::points::compute::DEFAULT_WORKGROUPS,
        );
        self.point_renderer = PointRenderer::new(
            &self.gpu.device,
            self.gpu.format,
            &self.point_compute.point_buffer,
            &self.point_compute.colormap_buffer,
        );
        self.splat_renderer = SplatRenderer::with_pipelines(
            &self.gpu.device,
            &self.splat_pipelines,
            &self.point_compute.point_buffer,
            &self.point_compute.colormap_buffer,
        );
        // A fresh `PointCompute` is back to the stand-in sensitivity buffer,
        // so the glow has to be built against that one or it reads a buffer
        // nothing writes.
        self.emphasis_renderer = EmphasisRenderer::new(
            &self.gpu.device,
            self.gpu.format,
            &self.point_compute.point_buffer,
            &self.point_compute.sensitivity_buffer,
            &self.point_compute.stats_buffer,
        );
        self.gizmo_renderer = GizmoRenderer::new(
            &self.gpu.device,
            self.gpu.format,
            self.overlay.samples(),
            &self.gizmo_specs(),
        );
        // A fresh renderer knows nothing about the selection, so the x-ray slot
        // and the tip handles are off until something tells it again — and
        // `refresh_indicators` only speaks up when its key *changes*. Clear the
        // key so the next frame re-states the selection instead of waiting for
        // one to happen.
        self.indicator_key = (None, None);
        let table = self.layer_table();
        self.point_compute.update_weights(&self.gpu.device, &self.gpu.queue, &table);
        self.gizmo_renderer.update_alpha(&self.gpu.queue, &self.gizmo_enabled());
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
        self.scene.resolve_all_color_speeds(self.color_falloff);
        let table = self.layer_table();
        self.point_compute.update_weights(&self.gpu.device, &self.gpu.queue, &table);
        let specs = self.gizmo_specs();
        if self.gizmo_renderer.instance_count() == specs.len() as u32 + 1 {
            self.gizmo_renderer.update_transforms(&self.gpu.queue, &specs);
        } else {
            self.rebuild_gizmos();
        }
        // The renormalizing map may itself have just been dragged
        self.refresh_zoom();
        self.reset();
        if self.show_traces {
            self.regenerate_traces();
        }
    }

    pub fn toggle_render_mode(&mut self) {
        // Points, splat, volume, round again. A GPU without fp32 blending has
        // no volume mode, and the cycle simply skips it.
        self.render_mode = match self.render_mode {
            RenderMode::Points => RenderMode::Splat,
            RenderMode::Splat if self.volume_available() => RenderMode::Volume,
            RenderMode::Splat | RenderMode::Volume => RenderMode::Points,
        };
        log::info!("Renderer: {}", self.render_mode.label());
    }

    pub fn adjust_exposure(&mut self, increase: bool) {
        let before = self.edit_snapshot();
        let factor = if increase { 1.25 } else { 0.8 };
        self.exposure = (self.exposure * factor).clamp(0.01, 100.0);
        log::info!("Splat exposure: {:.2}", self.exposure);
        // An edit now, because it changes what Ctrl+S writes. That's the whole
        // rule: if it changes the file, it goes on the history stack.
        self.commit_edit("Exposure", Some("exposure"), before);
    }

    /// Set the splat exposure directly (the Render window's slider).
    pub fn set_exposure(&mut self, exposure: f32) {
        let exposure = exposure.clamp(0.01, 100.0);
        if self.exposure == exposure {
            return;
        }
        let before = self.edit_snapshot();
        self.exposure = exposure;
        self.commit_edit("Exposure", Some("exposure"), before);
    }

    pub fn adjust_point_size(&mut self, increase: bool) {
        let before = self.edit_snapshot();
        let factor = if increase { 1.1 } else { 0.909 };
        self.point_size = (self.point_size * factor).clamp(MIN_POINT_SIZE, 0.1);
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
        self.scene.resolve_all_color_speeds(self.color_falloff);
        let table = self.layer_table();
        self.point_compute.update_weights(&self.gpu.device, &self.gpu.queue, &table);
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
        self.commit_edit("Colour falloff", Some("color_falloff"), before);
    }

    pub fn adjust_color_contrast(&mut self, increase: bool) {
        let before = self.edit_snapshot();
        let factor = if increase { 1.15 } else { 1.0 / 1.15 };
        self.color_contrast = (self.color_contrast * factor).clamp(0.25, 16.0);
        log::info!("Color contrast: {:.2}", self.color_contrast);
        self.commit_edit("Colour contrast", Some("color_contrast"), before);
    }

    // === Render window (Phase 5) ===
    //
    // Absolute-value setters for the controls that own a slider, alongside
    // the relative `adjust_*` nudges the keybinds use. Which of these commit
    // history is deliberate and follows Phase 3: parameters that get written
    // back into the scene TOML by Ctrl+S (point size, color falloff, color
    // contrast, exposure — see `adjust_exposure`/`set_exposure` above) are
    // edits; the renderer mode (points/splat) is the one knob here that
    // stays view-only, because nothing in `Scene`/`SceneMeta` records it —
    // Ctrl+S can't write what doesn't change, so it can't be undoable
    // either. It's still saved, just not as scene data: `View::renderer`
    // (src/view.rs) carries it, written by `V` / `App::save_view`. Matches
    // the keyboard path (`toggle_render_mode`) exactly.

    pub fn set_render_mode(&mut self, mode: RenderMode) {
        if mode == RenderMode::Volume && !self.volume_available() {
            return;
        }
        if self.render_mode != mode {
            self.render_mode = mode;
            log::info!("Renderer: {}", self.render_mode.label());
        }
    }

    /// Whether this GPU can run volume mode (it needs fp32 blending). The
    /// Render window greys the segment out, saying why, when it cannot.
    pub fn volume_available(&self) -> bool {
        self.splat_renderer.volume_available()
    }

    /// Set the volume-mode opacity (the Render window's slider). Session state:
    /// not an edit, not in the history, not saved.
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.0, crate::gpu::points::volume::MAX_OPACITY);
    }

    /// The reference length opacity is measured against, for this frame's
    /// camera: the measured attractor's radius; under infinite zoom, which has
    /// nothing to measure, the eye's distance from the fixed point.
    fn volume_framing(&mut self) -> crate::gpu::points::volume::Framing {
        use crate::gpu::points::volume::{self, Framing};
        let eye = self.camera.eye();
        if let Some(z) = self.zoom() {
            return Framing::for_zoom(eye, z.fixed_point);
        }
        let sphere = if self.scene.is_composite() {
            // `self.attractor` walks every map as one IFS, which describes no
            // composite; measure it layer by layer, and only when it changes.
            let key = self.attractor_fingerprint();
            if self.volume_bounds_key != key {
                self.volume_bounds = volume::scene_bounds(&self.scene);
                self.volume_bounds_key = key;
            }
            self.volume_bounds
        } else {
            self.attractor.as_ref().map(volume::bounding_sphere)
        };
        match sphere {
            Some((_, radius)) => Framing::around(radius),
            None => Framing::for_distance(self.camera.distance),
        }
    }

    /// Hand the splat renderer its volume state for the frame about to be
    /// drawn: on in volume mode with opacity above zero, off otherwise. `samples`
    /// and `height` are exactly what exposure is normalised by at that call
    /// site, so absorbance scales the way brightness does.
    fn sync_volume(&mut self, samples: f64, height: f32) {
        use crate::gpu::points::volume::VolumeParams;
        let params = if self.render_mode == RenderMode::Volume {
            let framing = self.volume_framing();
            VolumeParams::new(self.opacity, samples, height, self.camera.fov, framing)
        } else {
            VolumeParams::OFF
        };
        self.splat_renderer.set_volume(&self.gpu.queue, params);
    }

    pub fn set_point_size(&mut self, size: f32) {
        let before = self.edit_snapshot();
        self.point_size = size.clamp(MIN_POINT_SIZE, 0.1);
        self.commit_edit("Point size", Some("point_size"), before);
    }

    pub fn set_color_falloff(&mut self, falloff: f32) {
        let before = self.edit_snapshot();
        self.color_falloff = falloff.clamp(0.0, 4.0);
        self.refresh_color_speeds();
        self.commit_edit("Colour falloff", Some("color_falloff"), before);
    }

    // === Palette =========================================================
    //
    // Every one of these is a scene edit, so they take a snapshot and commit,
    // and every one has to push the rebuilt colormap to the GPU: existing
    // points keep their 8-bit index, so re-uploading the 256 entries recolours
    // the whole buffer on the next frame without re-running the chaos game.
    // That is what makes dragging a gradient handle feel live.

    /// Rebuild the colormap and hand it to the GPU. The one path — anything
    /// that changes colour goes through here rather than doing it by hand.
    ///
    /// Both colour buffers, not just the colormap: `ColorMode::Mix` reads each
    /// transform's RGB out of the *transform* buffer, so a colour edit that
    /// only re-uploaded the 256 entries would leave mix mode showing the old
    /// colours until something else happened to touch a weight.
    fn push_colormap(&mut self) {
        self.scene.regenerate_colormap();
        self.point_compute.update_colormap(&self.gpu.queue, &self.scene.colormap);
        self.sync_transforms_to_gpu();
    }

    /// Switch colour source (the `transforms | palette | mix` toggle). The
    /// transform re-upload isn't redundant with the colormap one: `mix` reads
    /// the per-transform RGBs off the GPU struct, not the colormap.
    pub fn set_color_mode(&mut self, mode: crate::scene::ColorMode) {
        if self.scene.color_mode == mode {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.set_color_mode(mode);
        self.point_compute.update_colormap(&self.gpu.queue, &self.scene.colormap);
        self.sync_transforms_to_gpu();
        self.commit_edit(format!("Colour: {}", mode.name()), None, before);
    }

    /// Install a gradient (library pick, random roll, or import).
    pub fn set_palette(&mut self, palette: crate::palette::Palette, label: &str) {
        let before = self.edit_snapshot();
        self.scene.set_palette(palette);
        self.point_compute.update_colormap(&self.gpu.queue, &self.scene.colormap);
        self.commit_edit(label.to_string(), None, before);
    }

    /// Edit the current palette in place. The closure gets `&mut Palette`;
    /// `key` coalesces a whole drag into one undo step, as elsewhere.
    pub fn edit_palette(
        &mut self,
        label: impl Into<String>,
        key: Option<&str>,
        f: impl FnOnce(&mut crate::palette::Palette),
    ) {
        let before = self.edit_snapshot();
        let Some(p) = self.scene.palette.as_mut() else { return };
        f(p);
        self.push_colormap();
        self.commit_edit(label.into(), key, before);
    }

    /// Move a control point along the gradient.
    ///
    /// Stops are kept sorted, so dragging one past its neighbour reorders the
    /// list and the index the caller was holding no longer refers to the same
    /// stop. Returns where it ended up so the GUI can keep hold of it through
    /// the drag instead of grabbing whichever stop inherited the old index.
    pub fn set_palette_stop_at(&mut self, idx: usize, at: f32) -> usize {
        let mut moved = idx;
        // One coalescing key for every stop, not one per index: a drag that
        // crosses a neighbour changes the index mid-gesture, and a key
        // carrying the index would split that one drag into two undo steps.
        // Only one stop can be dragged at a time, so a shared key is safe.
        self.edit_palette("Move palette stop", Some("pal:stop"), |p| {
            moved = p.move_stop(idx, at);
        });
        moved
    }

    pub fn set_palette_stop_color(&mut self, idx: usize, color: Vec3) {
        self.edit_palette("Recolour palette stop", Some(&format!("pal:col:{idx}")), |p| {
            if let Some(s) = p.stops_mut().and_then(|s| s.get_mut(idx)) {
                s.color = color;
            }
        });
    }

    /// Add a control point at `at`, taking the colour the gradient already
    /// has there — so adding a handle never changes the picture, it only
    /// gives you somewhere to grab.
    pub fn add_palette_stop(&mut self, at: f32) -> Option<usize> {
        let color = self.scene.palette.as_ref()?.sample(at);
        let mut added = None;
        self.edit_palette("Add palette stop", None, |p| {
            let Some(stops) = p.stops_mut() else { return };
            stops.push(crate::palette::Stop { at: at.clamp(0.0, 0.999), color });
            stops.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
            added = stops.iter().position(|s| s.at == at.clamp(0.0, 0.999));
        });
        added
    }

    /// Remove a control point. Two is the floor: one stop is a flat colour
    /// and zero is white, and neither is a state a delete button should be
    /// able to strand someone in.
    pub fn remove_palette_stop(&mut self, idx: usize) {
        if self.scene.palette.as_ref().and_then(|p| p.stops()).is_none_or(|s| s.len() <= 2) {
            return;
        }
        self.edit_palette("Delete palette stop", None, |p| {
            if let Some(stops) = p.stops_mut() {
                if idx < stops.len() {
                    stops.remove(idx);
                }
            }
        });
    }

    /// Freeze a procedural or imported gradient into editable control points.
    /// A cosine palette has no handles to grab; this is the "I want to edit
    /// this one" button, and it keeps the colours you were already looking at.
    pub fn convert_palette_to_stops(&mut self, n: usize) {
        let Some(p) = self.scene.palette.as_ref() else { return };
        if p.stops().is_some() {
            return;
        }
        let frozen = p.to_stops(n);
        let before = self.edit_snapshot();
        self.scene.palette = Some(frozen);
        self.push_colormap();
        self.commit_edit("Convert palette to stops", None, before);
    }

    /// Roll a new gradient, honouring the same generators as
    /// `--random-palette`. Returns what it landed on, for the status line.
    pub fn randomize_palette(
        &mut self,
        generator: Option<crate::palette::random::Generator>,
    ) -> String {
        let mut rng = rand::thread_rng();
        let p = match generator {
            Some(g) => crate::palette::random::from(g, &mut rng),
            None => crate::palette::random::palette(&mut rng),
        };
        let described = p.describe();
        self.set_palette(p, "Random palette");
        described
    }

    /// Haze band in world units: the pinned one, or auto-ranged off the
    /// current camera distance so it tracks the framing.
    ///
    /// **A pin is ignored under infinite zoom**, and that is a correctness
    /// requirement rather than a convenience. A band pinned in world units
    /// does not scale with the wrap, so the image drifts out of the haze
    /// across a period and snaps back at the seam — measured offline on
    /// `wellspiral` as a 12% brightening undone by an 11% drop in one frame
    /// (`offline::Haze`). Every legacy view counts as pinned (`apply_view`),
    /// so this is reachable by loading an old view over a zoom scene, which is
    /// how it went unnoticed: the offline renderer was fixed and the window
    /// was not.
    pub fn haze_range(&self) -> (f32, f32) {
        match self.haze_band {
            Some(band) if self.zoom().is_none() => band,
            _ => crate::haze::auto_band(self.camera.distance),
        }
    }

    /// Whether a hand-pinned haze band is being overridden by the auto range
    /// because the scene zooms. The panel says so rather than leaving two
    /// sliders that appear to do nothing.
    pub fn haze_band_overridden(&self) -> bool {
        self.haze_band.is_some() && self.zoom().is_some()
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
        self.commit_edit("Colour contrast", Some("color_contrast"), before);
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

    /// CPU threads a render job may use — the preference, or one less than the
    /// machine has if nothing has been chosen.
    ///
    /// A machine setting, so prefs and never scene or view data: what is right
    /// here is a fact about this box, and replaying it somewhere else would be
    /// wrong advice rather than a faithful reproduction.
    pub fn render_threads(&self) -> usize {
        self.prefs
            .threads
            .map(|n| n.clamp(1, Self::max_render_threads()))
            .unwrap_or_else(crate::render_job::default_threads)
    }

    /// Everything the machine has. The ceiling on the control, not its default
    /// — the default deliberately holds one back.
    pub fn max_render_threads() -> usize {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
    }

    /// Remember a thread count. Deferred write, like every other slider-driven
    /// pref, so dragging doesn't rewrite prefs.toml every frame.
    pub fn set_render_threads(&mut self, n: usize) {
        let n = n.clamp(1, Self::max_render_threads());
        if self.prefs.threads == Some(n) {
            return;
        }
        self.prefs.threads = Some(n);
        self.prefs_dirty_since = Some(std::time::Instant::now());
    }

    /// The render-job preset last chosen, by name: what the dialog opens on.
    pub fn render_preset(&self) -> Option<&str> {
        self.prefs.render_preset.as_deref()
    }

    pub fn set_render_preset(&mut self, name: Option<String>) {
        if self.prefs.render_preset != name {
            self.prefs.render_preset = name;
            self.prefs_dirty_since = Some(std::time::Instant::now());
        }
    }

    /// Fraction of the GPU a render job may take. See [`prefs::Prefs::gpu_share`].
    pub fn render_gpu_share(&self) -> f32 {
        self.prefs.gpu_share.unwrap_or(1.0).clamp(0.05, 1.0)
    }

    /// Remember it. Deferred write, like every other slider-driven pref, so
    /// dragging doesn't rewrite prefs.toml every frame.
    pub fn set_render_gpu_share(&mut self, share: f32) {
        let share = share.clamp(0.05, 1.0);
        if self.prefs.gpu_share == Some(share) {
            return;
        }
        self.prefs.gpu_share = Some(share);
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
    /// Turn everything except `idx` off — or, if it is already the only one on,
    /// turn everything back on.
    ///
    /// The toggle-back is what makes solo usable rather than a trap: the
    /// alternative is remembering by hand which of forty transforms were off
    /// before you soloed. One undo step either way.
    pub fn solo_transform(&mut self, idx: usize) {
        if idx >= self.transform_enabled.len() {
            return;
        }
        let already_solo = self
            .transform_enabled
            .iter()
            .enumerate()
            .all(|(i, &on)| on == (i == idx));
        let before = self.edit_snapshot();
        if already_solo {
            self.transform_enabled.fill(true);
            log::info!("Solo off — all transforms enabled");
        } else {
            for (i, on) in self.transform_enabled.iter_mut().enumerate() {
                *on = i == idx;
            }
            log::info!("Solo T{}", idx);
        }
        self.sync_transforms_to_gpu();
        self.commit_edit(
            if already_solo { "Show all transforms".to_string() } else { format!("Show only T{}", idx) },
            None,
            before,
        );
    }

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

        let table = self.layer_table();
        self.point_compute.update_weights(&self.gpu.device, &self.gpu.queue, &table);
        self.gizmo_renderer.update_alpha(&self.gpu.queue, &self.gizmo_enabled());
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

    /// What the live pass does with alpha. Screenshots don't ask this: they
    /// go to a file, which wants straight alpha or none (`transparent_render`).
    fn window_alpha(&self) -> crate::gpu::buffers::Alpha {
        if self.transparent_window {
            crate::gpu::buffers::Alpha::Premultiplied
        } else {
            crate::gpu::buffers::Alpha::Opaque
        }
    }

    /// The live pass's clear. Transparent is all zeros, not the background at
    /// alpha 0: premultiplied, any colour left in an empty pixel is light
    /// added on top of the desktop.
    fn window_clear(&self) -> wgpu::Color {
        if self.transparent_window {
            wgpu::Color::TRANSPARENT
        } else {
            crate::scene::clear_color(self.scene.background, 1.0)
        }
    }

    /// The point size as it is actually drawn.
    ///
    /// Under an infinite zoom this tracks the eye, because a size fixed in
    /// world units is not wrap-invariant — see `Renorm::point_scale`. Without
    /// one it is the authored size unchanged.
    ///
    /// Everything that reasons about how big a dot lands on screen must go
    /// through this and not `self.point_size`, the primitive choice below
    /// included: that decision is a threshold on the projected size, so
    /// feeding it the unscaled number would flip it partway through a period
    /// and change renderer mid-flight.
    pub fn rendered_point_size(&self) -> f32 {
        let Some(z) = self.zoom() else { return self.point_size };
        // Against the scene's own framing, not against `band`: `band` is the
        // orbit radius and the wrap measures the eye's distance from the fixed
        // point, which are different numbers whenever the focus is off the
        // fixed point. Referred to the framing the size was chosen at, the
        // factor is exactly 1 there and moves only as the eye does.
        let reference = self.scene_reference_eye(z);
        let eye = (self.camera.eye() - z.fixed_point).length();
        self.point_size * (eye / reference.max(1e-9))
    }

    /// How far the eye sits from the zoom's fixed point at the scene's *own*
    /// framing — the framing an authored `point_size` was chosen against.
    fn scene_reference_eye(&self, z: &crate::renorm::Renorm) -> f32 {
        // `OrbitCamera::eye`, without building one: the scene holds the
        // framing as an orientation, not as the chart the file is written in.
        let eye = self.scene.camera_focus
            + self.scene.camera_distance * self.scene.camera_orientation.rotate(glam::Vec3::Z);
        (eye - z.fixed_point).length()
    }

    /// Use native 1px point primitives (~3x faster) when the projected point
    /// size at the orbit distance would be subpixel anyway
    fn use_point_primitives(&self, screen_height: f32) -> bool {
        self.projected_point_pixels(screen_height) <= 1.5
    }

    /// What the shaders will make of the current point size at the focus, in
    /// render-target pixels and before any clamp. One place, so the fast path
    /// above and the readout below cannot drift apart from each other or from
    /// `CameraUniforms::projected_point_size`.
    fn projected_point_pixels(&self, screen_height: f32) -> f32 {
        OrbitCamera::point_pixels(
            self.rendered_point_size(),
            self.camera.distance,
            screen_height,
            self.camera.fov,
        )
    }

    /// **How big a dot actually lands on screen, in pixels, at the focus** —
    /// and whether asking for a different size would change that.
    ///
    /// The size control is in world units, so the same number means wildly
    /// different things from scene to scene: `jspace` is framed at distance
    /// 3.0 and `sierpinski-infinity` at 0.0186, a factor of 160, and a size
    /// that is coarse grain on one is invisible on the other. This is that
    /// number in the unit the person is actually looking at.
    ///
    /// **At the focus**, because perspective means there is no single answer:
    /// a dot at the front of the attractor covers more screen than the same
    /// dot at the back. The focus is the one depth the framing is *about*.
    ///
    /// The second return is "this is saturated": below 1.5px the renderer
    /// swaps to native 1px primitives and above `MAX_POINT_PIXELS` the shader
    /// clamps, and in both regimes dragging the control changes nothing you
    /// can see. That is worth saying — it is the whole reason a size of 1e-5
    /// and one of 1e-6 render byte-identical on a zoom scene.
    pub fn point_pixels_at_focus(&self, screen_height: f32) -> (f32, bool) {
        if self.use_point_primitives(screen_height) {
            return (1.0, true);
        }
        let raw = self.projected_point_pixels(screen_height);
        let max = crate::gpu::buffers::MAX_POINT_PIXELS;
        (raw.min(max), raw > max)
    }

    /// Viewport height in physical pixels — what a projected size is measured
    /// against, and the panel has no other way to ask.
    pub fn viewport_height(&self) -> f32 {
        self.gpu.size().1 as f32
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
        // Nor for a composite. The budget draws a *prefix* of the ring, and in
        // a composite each stretch of the ring was written by one layer's
        // walkers — a prefix would draw some subscenes and drop others. The
        // measurement is of the wrong thing too: it walks every map as one IFS.
        if self.scene.is_composite() {
            return valid;
        }
        let Some(stats) = self.attractor else { return valid };
        let depth = (stats.center - self.camera.eye()).length();
        let r_px =
            stats.radius * OrbitCamera::pixels_per_world_unit(depth, screen_height, self.camera.fov);
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
        if self.scene.is_composite() {
            // The loader refuses this pairing; an in-app edit can still reach
            // it, and says so rather than renormalizing placed points about a
            // map that knows nothing of their placement.
            self.point_compute.zoom = None;
            self.zoom_error = Some("Infinite zoom is off, because it doesn't work with subscenes yet".into());
            return;
        }
        match crate::renorm::Renorm::build(&spec, &self.scene.transforms, self.scene.camera_distance)
        {
            Ok(r) => {
                // A zoom loop is derived from this map, so re-derive it too —
                // otherwise dragging the map leaves the loop closing under the
                // similarity it used to have, and the seam quietly opens.
                if let Some(path) = self.scene.camera_path.as_mut() {
                    if let Some(z) = path.loops.zoom() {
                        path.loops = crate::path::Loop::Zoom(r.loop_similarity(z.periods));
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
    ///
    /// This writes `self.scene.zoom` — the same `[zoom]` block `set_zoom_spec`
    /// edits and Ctrl+S writes back — so it has to go through `commit_edit`
    /// too, on the same terms as any other discrete pick (`set_color_mode`):
    /// no coalesce key, since choosing a map or switching zoom off is a
    /// one-shot action, not a drag.
    pub fn set_zoom_map(&mut self, map: Option<usize>) {
        let before = self.edit_snapshot();
        self.scene.zoom = map.map(|map| crate::renorm::ZoomSpec {
            map,
            ..self.scene.zoom.clone().unwrap_or_default()
        });
        self.refresh_zoom();
        self.zoom_level = 0;
        self.zoom_turns = 0;
        self.zoom_turns_base = 0;
        self.zoom_turns_drawn = 0;
        self.pending_rewrap = 0;
        self.point_compute.reset(&self.gpu.queue);
        self.frame_count = 0;
        self.commit_edit(
            match map {
                Some(_) => "Infinite zoom on",
                None => "Infinite zoom off",
            },
            None,
            before,
        );
    }

    /// The scene's authored band settings, for the Render window's sliders.
    pub fn zoom_spec(&self) -> Option<&crate::renorm::ZoomSpec> {
        self.scene.zoom.as_ref()
    }

    /// Edit the band (radius / levels / octave_fade / octave_falloff).
    ///
    /// Every point's octave is drawn from these, so the whole cloud has to
    /// re-form — the same refill `set_zoom_map` does. Undoable, because these
    /// are written back into the scene's `[zoom]` by Ctrl+S, which is the line
    /// this panel draws between an edit and a view knob.
    pub fn set_zoom_spec(&mut self, spec: crate::renorm::ZoomSpec) {
        let before = self.edit_snapshot();
        self.scene.zoom = Some(spec);
        self.refresh_zoom();
        self.point_compute.reset(&self.gpu.queue);
        self.frame_count = 0;
        self.commit_edit("Zoom band", Some("zoom_band"), before);
    }

    /// Keep the eye inside one zoom period; see `renorm::Renorm::wrap`
    fn wrap_zoom(&mut self) {
        let Some(zoom) = self.point_compute.zoom else { return };
        let levels = zoom.wrap(&mut self.camera);
        self.zoom_level += levels;
        self.zoom_turns += levels;

        // The points go with the camera, or the wrap resamples the whole dot
        // field in one frame and a sparse scene twitches (see `rewrap` in
        // `shaders/points/chaos.wgsl`).
        //
        // Against the monotone count, and not against `levels`, which is the
        // same trap the zoom readout fell into: while a path is flying, what
        // `wrap` returns is the absolute depth of an unwrapped spline sample,
        // so carrying the buffer by it would carry it nine periods a frame
        // rather than the nought or one it actually moved.
        self.pending_rewrap += self.zoom_turns - self.zoom_turns_drawn;
        self.zoom_turns_drawn = self.zoom_turns;
    }

    /// The frame the world axes have been carried into by the zoom.
    ///
    /// A wrap is invisible in the fractal — that is the whole construction —
    /// but it is *not* invisible in the world, because it turns the camera by
    /// the map's rotation. Anything drawn in world axes therefore jumps by
    /// that rotation on the frame the camera folds, while the picture behind
    /// it does not move at all. The axis cross is the one such thing on
    /// screen, and on a scene that spirals rather than descending straight in
    /// it visibly snaps once a period.
    ///
    /// Carrying the axes by `A⁻ᵗᵘʳⁿˢ` cancels it exactly. `wrap` premultiplies
    /// the camera's orientation by `rot⁻¹` per level, so a direction shown as
    /// `rot⁻ᵗᵘʳⁿˢ·w` keeps the screen position `w` had before the fold. What
    /// the cross then reads is the frame the *picture* has been accumulating,
    /// which is the honest answer for a camera spiralling inward forever:
    /// it keeps turning rather than closing, because the descent does.
    ///
    /// Identity for every scene without an infinite zoom, which is most.
    pub fn zoom_frame(&self) -> crate::rot::Orientation {
        match self.zoom() {
            Some(z) => z.carried_frame(self.zoom_turns),
            None => crate::rot::Orientation::IDENTITY,
        }
    }

    /// What the drawn-point budget is keyed on: any change to the maps, to
    /// which of them are enabled, or to how many there are.
    fn attractor_fingerprint(&self) -> u64 {
        let mut h = self.matrix_generation
            .wrapping_mul(0x100000001b3)
            ^ (self.scene.transforms.len() as u64)
            // A composite measures the listed layer, so changing it re-measures.
            ^ ((self.current_layer as u64) << 48);
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

    /// Re-measure lacunarity on a slower cadence (once per second). More
    /// expensive than the attractor measure since it runs multiple resolutions.
    fn refresh_lacunarity(&mut self) {
        let key = self.attractor_fingerprint();
        if key == self.lacunarity_key {
            return;
        }
        if self.lacunarity.is_some()
            && self.lacunarity_measured_at.elapsed() < Duration::from_secs(1)
        {
            return;
        }
        self.lacunarity_key = key;
        self.lacunarity_measured_at = Instant::now();
        let (maps, on) = self.measured_maps();
        self.lacunarity = crate::trace::lacunarity_summary(&maps, &on);
    }

    /// Current lacunarity summary for the status bar.
    /// The maps the status bar's `d` and `Λ` describe, with their enabled
    /// state: every map for an ordinary scene (borrowed — L-system scenes run
    /// to tens of thousands), the current layer's in a composite, where the
    /// maps of two different attractors have no joint dimension.
    pub fn measured_maps(&self) -> (std::borrow::Cow<'_, [TransformSpec]>, std::borrow::Cow<'_, [bool]>) {
        use std::borrow::Cow;
        if !self.scene.is_composite() {
            return (Cow::Borrowed(&self.scene.transforms), Cow::Borrowed(&self.transform_enabled));
        }
        let k = self.current_layer() as u16;
        let (maps, on): (Vec<TransformSpec>, Vec<bool>) = self
            .scene
            .transforms
            .iter()
            .enumerate()
            .filter(|(_, t)| t.layer == k)
            .map(|(i, t)| (t.clone(), self.is_transform_enabled(i)))
            .unzip();
        (Cow::Owned(maps), Cow::Owned(on))
    }

    pub fn lacunarity(&self) -> Option<f32> {
        self.lacunarity
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
        self.refresh_lacunarity();
        self.refresh_indicators();
        self.refresh_path_lines();
        self.poll_job();

        self.advance_camera_glide();
        self.expire_zoom_cursor();

        let should_log = self.fps_tracker.frame();
        self.frame_count += 1;
        // One clock. The camera flies the path on it — which path, the scene's
        // own keys or the default orbit, is `camera_path`'s business, not this
        // loop's — and every track is sampled on it (`refresh_animation`).
        if self.playing {
            self.time += dt as f64;
        }
        if self.camera_follows {
            let (duration, wraps, playable, periods) = {
                let p = self.camera_path();
                let periods = match p.loops {
                    crate::path::Loop::Zoom(z) => z.periods as i32,
                    _ => 0,
                };
                (p.duration() as f64, p.wraps(), p.playable(), periods)
            };
            if !playable {
                self.stop_following();
            } else {
                let _ = duration;
                let local = self.camera_local();
                // Each pass of a zoom loop descends `periods` periods and
                // arrives on the frame it left — the same picture, one level
                // down. The lap rolling over is the only record that it
                // happened, so it is where the monotone count is kept whole.
                let lap = local.floor() as i64;
                if wraps && lap > self.camera_lap {
                    self.zoom_turns_base += (lap - self.camera_lap) as i32 * periods;
                }
                self.camera_lap = lap;
                // Folded in f64, where the clock lives, before it meets the
                // spline's f32: an hour in, `local` is ~100 laps and an f32
                // would have left it a few bits of fraction.
                let t = if wraps { local.rem_euclid(1.0) } else { local.clamp(0.0, 1.0) } as f32;
                // The path carries a framing, never a lens: playing one back must not
                // change the FoV out from under the person watching.
                let cam = self.camera_path().sample(t, self.camera.fov);
                self.camera = cam;
                // A path key's distance is unwrapped — that's the point, it's
                // how a path descends nine zoom periods in one straight
                // interpolation — so the sample arrives outside the band every
                // frame and `wrap_zoom` puts it back. Its return is then the
                // absolute depth of this sample, not a step, so the counter
                // has to be reset rather than added to. Miss this and the
                // reading runs away by nine per frame.
                self.zoom_level = 0;
                self.zoom_turns = self.zoom_turns_base;
            }
        }
        if self.playing {
            if let Some((start, end)) = self.active_loop_region() {
                // The region governs on its own: the timeline's end doesn't
                // stop a loop set past it. Carried across the seam exactly as
                // the whole-timeline loop is below.
                if self.time >= end {
                    let span = end - start;
                    let back = ((self.time - start) / span).floor() * span;
                    self.time -= back;
                    if self.path_is_default() {
                        self.camera_epoch -= back;
                    }
                    self.camera_lap = self.camera_local().floor() as i64;
                }
            } else if let Some(end) = self.timeline_end().filter(|&end| self.time >= end) {
                if self.loop_playback && !self.scene.tracks.is_empty() {
                    // Round again. The default orbit is a cycle with no end of
                    // its own, so it is carried across the seam rather than
                    // sent back to where it began.
                    self.time -= end;
                    if self.path_is_default() {
                        self.camera_epoch -= end;
                    } else {
                        self.camera_lap = 0;
                    }
                } else {
                    self.time = end;
                    self.playing = false;
                    // A scene without tracks is the old camera-only transport,
                    // where finishing the path let go of the camera.
                    if self.scene.tracks.is_empty() {
                        self.camera_follows = false;
                    }
                    log::info!("Timeline: finished");
                }
            }
        }
        self.refresh_animation();
        self.sync_record_base();

        // Keep the eye inside one zoom period. Last, so it catches the camera
        // however it moved this frame — dragged, scrolled, or flown along the
        // path — and the picture it lands on is identical to the one it left.
        self.wrap_zoom();

        self.refresh_window_title();

        if should_log {
            let point_count = self.point_compute.valid_point_count();
            log::info!(
                "FPS: {:.1} | Frametime: {:.2}ms | Points: {}",
                self.fps_tracker.current_fps,
                self.fps_tracker.current_frametime_ms,
                point_count,
            );
        }

    }

    /// Keep the window title saying which document is open.
    ///
    /// `document — application`, with a leading `*` while there are unsaved
    /// edits: the convention every file-editing program has used for decades,
    /// and the only place the taskbar, the alt-tab list and the window switcher
    /// can read it. It used to be a frame-rate HUD — which is genuinely useful
    /// information that the status bar already carries with more detail and a
    /// sparkline, and which told the window switcher nothing at all about what
    /// you had open. The FPS line still goes to `RUST_LOG` once a second.
    ///
    /// A build from any branch but main appends `[branch]`
    /// (`version::feature_branch`), so a worktree's build open beside main's
    /// can be told apart from the taskbar.
    ///
    /// Recomputed every frame but only *set* when it changes: `set_title` is a
    /// round trip to the window manager, and the string is stable for minutes
    /// at a time.
    // === The timeline: one clock for the camera and every track ===

    /// Where playback stops (or goes round): the timeline's length. `None`
    /// when nothing would ever end it — a scene with no tracks whose camera
    /// path loops, which is every scene that ever flew the default orbit.
    pub fn timeline_end(&self) -> Option<f64> {
        if self.scene.tracks.is_empty() {
            let p = self.camera_path();
            return (!p.wraps()).then(|| self.camera_epoch + p.duration() as f64);
        }
        Some(self.timeline_length())
    }

    /// The timeline's length in seconds (`track::timeline_length`): what an
    /// animation of this scene renders.
    pub fn timeline_length(&self) -> f64 {
        crate::track::timeline_length(&self.scene, self.camera_path())
    }

    pub fn playhead(&self) -> f64 {
        self.time
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Move the playhead (the Timeline window's ruler). Scrubbing is view
    /// state, like orbiting: it lands no history entry. The camera goes to
    /// where the scene's own path has it at that moment; the default orbit has
    /// no moments to go to, so a free camera stays put.
    pub fn scrub_to(&mut self, t: f64) {
        // Only the clock moves. An attached camera follows it on the next
        // update, as it does when playing; a detached one stays put.
        self.time = t.max(0.0);
    }

    /// Play or pause the timeline's clock (the Timeline's own button). Unlike
    /// O, which flies the camera, this leaves the camera as attached or
    /// detached as it is.
    pub fn toggle_playback(&mut self) {
        if self.playing {
            self.playing = false;
            return;
        }
        if self.timeline_end().is_some_and(|end| self.time >= end - 1e-6) {
            self.time = 0.0;
            if !self.path_is_default() {
                self.camera_epoch = 0.0;
            }
        }
        self.enter_loop_region();
        self.playing = true;
    }

    /// The loop region playback goes round in, when looping is on and a
    /// region is set.
    pub fn active_loop_region(&self) -> Option<(f64, f64)> {
        self.loop_region.filter(|&(a, b)| self.loop_playback && b - a > 1e-3)
    }

    /// Set or clear the loop region (the Timeline's ruler). Its ends are put
    /// in order, and setting one turns looping on: a region you just drew is
    /// one you want to hear go round.
    pub fn set_loop_region(&mut self, region: Option<(f64, f64)>) {
        self.loop_region = region.map(|(a, b)| (a.min(b).max(0.0), a.max(b).max(0.0)));
        if self.loop_region.is_some() {
            self.loop_playback = true;
        }
    }

    /// Play starting outside the loop region starts at its beginning, as a
    /// DAW's does.
    fn enter_loop_region(&mut self) {
        if let Some((a, b)) = self.active_loop_region() {
            if self.time < a || self.time >= b {
                self.scrub_to(a);
            }
        }
    }

    /// Back to zero, playing or not (the transport's return-to-start).
    pub fn return_to_start(&mut self) {
        self.time = 0.0;
        if !self.path_is_default() {
            self.camera_epoch = 0.0;
        }
        self.camera_lap = self.camera_local().floor() as i64;
    }

    /// Frames a second, for showing the playhead in frames and stepping by
    /// one: the Render job's, so a frame here is a frame of the file it writes.
    pub fn frame_rate(&self) -> u32 {
        self.ui_state.render_job.fps.max(1)
    }

    /// Step the playhead by `frames` frames (Left / Right), landing on a
    /// whole frame.
    pub fn step_frames(&mut self, frames: i64) {
        let fps = self.frame_rate() as f64;
        let here = (self.time * fps + 1e-6).floor() as i64;
        let to = if frames < 0 && (self.time * fps - here as f64).abs() > 1e-6 { here } else { here + frames };
        self.scrub_to(to as f64 / fps);
    }

    /// Every moment something is keyed: each track's keys and the camera's
    /// own keys (not their repeats on later laps). Sorted, with repeats
    /// dropped. What jumping between keys and snapping a dragged key go to.
    pub fn key_times(&self) -> Vec<f64> {
        let mut times: Vec<f64> = self
            .scene
            .tracks
            .iter()
            .flat_map(|t| match &t.source {
                crate::track::Source::Keys { keys, .. } => keys.iter().map(|k| k.t).collect(),
                _ => Vec::new(),
            })
            .collect();
        if !self.path_is_default() {
            let p = self.camera_path();
            times.extend((0..p.keys.len()).map(|k| p.time(k) as f64));
        }
        times.sort_by(f64::total_cmp);
        times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        times
    }

    /// Put the playhead on the next key after it (`forward`) or the one
    /// before (Shift+Right / Shift+Left). Returns false when there is none.
    pub fn jump_to_key(&mut self, forward: bool) -> bool {
        let times = self.key_times();
        let here = self.time;
        let to = if forward {
            times.into_iter().find(|&t| t > here + 1e-4)
        } else {
            times.into_iter().rev().find(|&t| t < here - 1e-4)
        };
        match to {
            Some(t) => {
                self.scrub_to(t);
                true
            }
            None => false,
        }
    }

    /// Mute or unmute one track (the Timeline window). An edit: it changes
    /// what Ctrl+S writes.
    pub fn set_track_muted(&mut self, i: usize, mute: bool) {
        if self.scene.tracks.get(i).is_none_or(|t| t.mute == mute) {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.tracks[i].mute = mute;
        let target = self.scene.tracks[i].target.clone();
        self.commit_edit(format!("{} track: {}", if mute { "Mute" } else { "Unmute" }, target), None, before);
    }

    /// Delete one track. The value it animated goes back to what the scene
    /// authors.
    pub fn delete_track(&mut self, i: usize) {
        if i >= self.scene.tracks.len() {
            return;
        }
        let before = self.edit_snapshot();
        let gone = self.scene.tracks.remove(i);
        self.commit_edit(format!("Delete track: {}", gone.target), None, before);
    }

    /// The scene as the window holds it, with the values it keeps live on
    /// `App` folded in: what a rate or cycle moves about, and what a new
    /// track's first key starts from.
    fn scene_with_live_values(&self) -> Scene {
        let mut base = self.scene.clone();
        base.point_size = self.point_size;
        base.haze = self.haze_amount;
        base.exposure = self.exposure;
        base.color_contrast = self.color_contrast;
        base.color_falloff = self.color_falloff;
        base
    }

    /// Start a keyed track on an `-S` path, with one key at the playhead
    /// holding the value the scene has there now — so adding a track changes
    /// nothing on screen until a second key says otherwise. Returns its index.
    pub fn add_track(&mut self, target: &str, start: Option<toml::Value>) -> Result<usize, String> {
        let target = target.trim();
        if let Some(i) = self.scene.tracks.iter().position(|t| t.target == target) {
            return Err(format!("{target} already has a track (row {})", i + 1));
        }
        // The caller may know the value already (the parameter picker does,
        // including for things the file leaves out, like an unused variation).
        let value = match start {
            Some(v) => v,
            None => crate::track::authored_value(&self.scene_with_live_values(), target)?.ok_or_else(|| {
                format!("this scene doesn't write {target} down yet; set it once, then add the track")
            })?,
        };
        let track = crate::track::one_key(target, self.time, value)?;
        let before = self.edit_snapshot();
        self.scene.tracks.push(track);
        self.commit_edit(format!("Add track: {target}"), None, before);
        Ok(self.scene.tracks.len() - 1)
    }

    /// Key track `i` at the playhead with the value it has there, so the curve
    /// is unchanged and the new key is ready to be moved. Returns the key's
    /// index, or `None` for a track that isn't keyed (or already has a key at
    /// this moment).
    pub fn add_key_at_playhead(&mut self, i: usize) -> Option<usize> {
        self.add_key_at(i, self.time, None)
    }

    /// Key track `i` at `t` seconds (the Timeline's right-click, a paste, a
    /// duplicate): with `value`, that value, replacing a key already at that
    /// moment; without, the value the track has there, leaving one already
    /// there alone. Returns the key's index, or `None` for a track that isn't
    /// keyed or a value of the wrong kind for it.
    pub fn add_key_at(&mut self, i: usize, t: f64, value: Option<crate::track::KeyValue>) -> Option<usize> {
        use crate::track::KeyValue;
        let t = t.max(0.0);
        let track = self.scene.tracks.get(i)?;
        let crate::track::Source::Keys { keys, .. } = &track.source else { return None };
        let existing = keys.iter().position(|k| (k.t - t).abs() < 1e-3);
        // A number goes on a number track, a pose on a pose track; anything
        // else only on a track already stepping between such values.
        let fits = |v: &KeyValue| {
            keys.iter().all(|k| std::mem::discriminant(&k.value) == std::mem::discriminant(v))
        };
        let value = match value {
            Some(v) if !fits(&v) => return None,
            Some(v) => v,
            None if existing.is_some() => return existing,
            None => track.sample(t, None),
        };
        let before = self.edit_snapshot();
        let crate::track::Source::Keys { keys, .. } = &mut self.scene.tracks[i].source else { return None };
        let key = crate::track::Key { t, value };
        let at = match existing {
            Some(j) => {
                keys[j] = key;
                j
            }
            None => {
                let j = keys.partition_point(|k| k.t < t);
                keys.insert(j, key);
                j
            }
        };
        let target = self.scene.tracks[i].target.clone();
        self.commit_edit(format!("Key {target} at {t:.2}s"), None, before);
        Some(at)
    }

    /// Move key `k` of track `i` in time, keeping it strictly between its
    /// neighbours (keys are in time order, and two at one moment would make
    /// the curve jump). A drag is one history entry.
    pub fn set_key_time(&mut self, i: usize, k: usize, t: f64) {
        let Some(crate::track::Source::Keys { keys, .. }) = self.scene.tracks.get(i).map(|t| &t.source) else { return };
        let Some(key) = keys.get(k) else { return };
        let lo = if k > 0 { keys[k - 1].t + 0.01 } else { f64::MIN };
        let hi = keys.get(k + 1).map_or(f64::MAX, |n| n.t - 0.01);
        let t = t.max(0.0).clamp(lo.min(hi), hi.max(lo));
        if (key.t - t).abs() < 1e-9 {
            return;
        }
        let before = self.edit_snapshot();
        if let crate::track::Source::Keys { keys, .. } = &mut self.scene.tracks[i].source {
            keys[k].t = t;
        }
        self.commit_edit("Move key", Some(&format!("timeline:key-time:{i}:{k}")), before);
    }

    /// Set the value of key `k` of track `i` (numbers only; a step key's value
    /// is edited in the file).
    pub fn set_key_value(&mut self, i: usize, k: usize, v: f64) {
        let Some(crate::track::Source::Keys { keys, integral, .. }) = self.scene.tracks.get(i).map(|t| &t.source) else { return };
        let v = if *integral { v.round() } else { v };
        if !matches!(keys.get(k).map(|k| &k.value), Some(crate::track::KeyValue::Number(x)) if *x != v) {
            return;
        }
        let before = self.edit_snapshot();
        if let crate::track::Source::Keys { keys, .. } = &mut self.scene.tracks[i].source {
            keys[k].value = crate::track::KeyValue::Number(v);
        }
        self.commit_edit("Key value", Some(&format!("timeline:key-value:{i}:{k}")), before);
    }

    /// Set rotation key `k` of track `i` to an Euler triple (degrees, XYZ).
    /// The route out of it is read from the triples again, so an explicit
    /// `turns`/`route` on it goes. Coalesced, like any drag.
    pub fn set_key_rotation(&mut self, i: usize, k: usize, euler: [f64; 3]) {
        let Some(crate::track::Source::Keys { keys, .. }) = self.scene.tracks.get(i).map(|t| &t.source) else { return };
        if !matches!(keys.get(k).map(|k| &k.value), Some(crate::track::KeyValue::Rotation(r)) if r.euler != Some(euler)) {
            return;
        }
        let before = self.edit_snapshot();
        if let crate::track::Source::Keys { keys, .. } = &mut self.scene.tracks[i].source {
            keys[k].value = crate::track::KeyValue::Rotation(crate::track::RotKey::from_euler(euler));
        }
        self.commit_edit("Key rotation", Some(&format!("timeline:key-value:{i}:{k}")), before);
    }

    /// Key whole-rotation track `i` at the playhead with the rotation its map
    /// has in the scene (the inspector's, or a gizmo drag's): pose, then key.
    /// Replaces a key already at this moment. Returns the key's index.
    pub fn key_rotation_from_scene(&mut self, i: usize) -> Option<usize> {
        let target = self.scene.tracks.get(i)?.target.clone();
        let value = crate::track::authored_value(&self.scene_with_live_values(), &target).ok().flatten();
        let e: Vec<f64> = value?.as_array()?.iter().filter_map(|v| v.as_float().or(v.as_integer().map(|i| i as f64))).collect();
        let [x, y, z] = e[..] else { return None };
        let t = self.time;
        let before = self.edit_snapshot();
        let crate::track::Source::Keys { keys, .. } = &mut self.scene.tracks[i].source else { return None };
        let key = crate::track::Key { t, value: crate::track::KeyValue::Rotation(crate::track::RotKey::from_euler([x, y, z])) };
        let at = match keys.iter().position(|k| (k.t - t).abs() < 1e-3) {
            Some(j) => {
                keys[j] = key;
                j
            }
            None => {
                let j = keys.partition_point(|k| k.t < t);
                keys.insert(j, key);
                j
            }
        };
        self.commit_edit(format!("Key {target} at {t:.2}s"), None, before);
        Some(at)
    }

    /// Delete key `k` of track `i`. The last key takes the track with it,
    /// since a track with no keys has nothing to say.
    pub fn delete_key(&mut self, i: usize, k: usize) {
        let Some(crate::track::Source::Keys { keys, .. }) = self.scene.tracks.get(i).map(|t| &t.source) else { return };
        if k >= keys.len() {
            return;
        }
        if keys.len() == 1 {
            self.delete_track(i);
            return;
        }
        let before = self.edit_snapshot();
        if let crate::track::Source::Keys { keys, .. } = &mut self.scene.tracks[i].source {
            keys.remove(k);
        }
        self.commit_edit("Delete key", None, before);
    }

    /// Replace track `i`'s source: its interpolation, extrapolation, or a
    /// cycle's or rate's numbers (the Timeline's detail column). Coalesced per
    /// track, so dragging a period is one step.
    pub fn set_track_source(&mut self, i: usize, source: crate::track::Source) {
        if self.scene.tracks.get(i).is_none_or(|t| t.source == source) {
            return;
        }
        let before = self.edit_snapshot();
        self.scene.tracks[i].source = source;
        let target = self.scene.tracks[i].target.clone();
        self.commit_edit(format!("Track: {target}"), Some(&format!("timeline:source:{i}")), before);
    }

    /// Every parameter of the scene a person might animate (`params::catalogue`),
    /// built once per document state: it tries each path on the scene's text,
    /// which is too much to redo every frame a menu is open.
    pub fn param_catalogue(&mut self) -> std::rc::Rc<Vec<crate::params::Param>> {
        let key = (self.history.top_serial(), self.scene_generation);
        if let Some((k, cat)) = &self.param_cache {
            if *k == key {
                return cat.clone();
            }
        }
        let cat = std::rc::Rc::new(crate::params::catalogue(&self.scene_with_live_values()));
        self.param_cache = Some((key, cat.clone()));
        cat
    }

    /// Why the scene's tracks aren't playing, when they aren't.
    pub fn track_error(&self) -> Option<&str> {
        self.track_error.as_deref()
    }

    /// The track that has taken a value the scene can't use at the playhead.
    pub fn track_invalid(&self) -> Option<&str> {
        self.track_invalid.as_deref()
    }

    /// Put the scene at the playhead on screen.
    ///
    /// The animated scene is never `self.scene`. That is the document — what
    /// the inspector edits, what history snapshots and what Ctrl+S writes — and
    /// an animated value in it would be saved as the authored one. So a frame
    /// is built aside (`Animator::at`), its maps and colours are pushed to the
    /// GPU with the frame swapped in for exactly as long as that takes, and
    /// the values the window keeps live on `App` (point size, exposure, haze,
    /// contrast) and the background are held in `anim_view` and swapped in only
    /// around the frame's drawing (`render`). The gizmos follow the frame, so
    /// a map you are watching move is drawn where it is.
    ///
    /// Rebuilt when the document changes (a history position, or a new
    /// scene), re-evaluated when the playhead moves, and free when neither
    /// does — a paused timeline costs nothing.
    fn refresh_animation(&mut self) {
        if self.scene.tracks.is_empty() {
            if self.live_anim.take().is_some() {
                // The last track went (deleted, or undone away): the base
                // scene goes back on screen.
                self.anim_view = AnimView::default();
                self.track_error = None;
                self.push_colormap();
            }
            return;
        }
        let key = (self.history.top_serial(), self.scene_generation);
        if self.live_anim.as_ref().is_none_or(|a| a.key != key) {
            let had_frame = self.live_anim.as_ref().is_some_and(|a| a.animator.is_some());
            // Built from the scene as the window holds it: the live values
            // folded in, so a rate or a cycle moves about what is on screen.
            let base = self.scene_with_live_values();
            let animator = match crate::track::Animator::for_scene(&base) {
                Ok(a) => {
                    // Skipped tracks play no part, and say so in the Timeline.
                    let broken = a.as_ref().and_then(|a| a.broken.first()).map(|(t, why)| format!("{t}: {why}"));
                    if broken.is_some() && broken != self.track_error {
                        self.set_notice(format!("A track doesn't apply and is skipped — {}", broken.as_deref().unwrap_or("")), Tone::Error);
                    }
                    self.track_error = broken;
                    a
                }
                Err(e) => {
                    if self.track_error.as_deref() != Some(e.as_str()) {
                        self.set_notice(format!("Tracks not playing: {e}"), Tone::Error);
                    }
                    self.track_error = Some(e);
                    None
                }
            };
            let mut view_targets = ViewTargets::default();
            for t in animator.iter().flat_map(|a| a.tracks()) {
                match t.target.split('.').nth(1).filter(|_| t.target.starts_with("meta.")) {
                    Some("point_size") => view_targets.point_size = true,
                    Some("exposure") => view_targets.exposure = true,
                    Some("haze" | "fog") => view_targets.haze = true,
                    Some("color_contrast") => view_targets.contrast = true,
                    Some("color_falloff") => view_targets.falloff = true,
                    Some("background") => view_targets.background = true,
                    _ => {}
                }
            }
            if animator.is_none() {
                self.anim_view = AnimView::default();
                if had_frame {
                    self.push_colormap();
                }
            }
            self.live_anim = Some(LiveAnimation { key, animator, shown_at: None, view_targets });
        }
        let a = self.live_anim.as_mut().expect("just built");
        if a.shown_at == Some(self.time) {
            return;
        }
        a.shown_at = Some(self.time);
        let Some(animator) = &a.animator else { return };
        let vt = a.view_targets;
        let class = animator.class();
        let frame = match animator.at(self.time) {
            Ok(f) => {
                if self.track_invalid.take().is_some() {
                    log::info!("Tracks: valid again at {:.2}s", self.time);
                }
                f
            }
            Err(e) => {
                // Said once per stretch of invalid frames, naming the track
                // rather than the value: the value is new every frame, so a
                // message carrying it was a new message every frame. The last
                // frame that worked stays on screen meanwhile.
                // Named once, at the start of the stretch: finding the culprit
                // parses the scene once per track, which is not a per-frame cost.
                if self.track_invalid.is_some() {
                    return;
                }
                let who = animator.culprit(self.time).unwrap_or_else(|| "a track".to_string());
                if self.track_invalid.as_deref() != Some(who.as_str()) {
                    self.set_notice(
                        format!("Track '{who}' has taken a value the scene can't use; holding the last frame that worked ({e})"),
                        Tone::Error,
                    );
                    self.track_invalid = Some(who);
                }
                return;
            }
        };
        self.anim_view = AnimView {
            point_size: vt.point_size.then_some(frame.point_size),
            exposure: vt.exposure.then_some(frame.exposure),
            haze: vt.haze.then_some(frame.haze),
            contrast: vt.contrast.then_some(frame.color_contrast),
            background: vt.background.then_some(frame.background),
        };
        if class.is_some_and(|c| c > crate::track::Class::View) {
            let falloff = if vt.falloff { frame.color_falloff } else { self.color_falloff };
            let held = self.anim_view.contrast.unwrap_or(self.color_contrast);
            let contrast = std::mem::replace(&mut self.color_contrast, held);
            let base = std::mem::replace(&mut self.scene, frame);
            self.scene.resolve_all_color_speeds(falloff);
            self.point_compute.update_colormap(&self.gpu.queue, &self.scene.colormap);
            let table = self.layer_table();
            self.point_compute.update_weights(&self.gpu.device, &self.gpu.queue, &table);
            let specs = self.gizmo_specs();
            if self.gizmo_renderer.instance_count() == specs.len() as u32 + 1 {
                self.gizmo_renderer.update_transforms(&self.gpu.queue, &specs);
            }
            if self.scene.zoom.is_some() {
                self.refresh_zoom();
            }
            self.scene = base;
            self.color_contrast = contrast;
        }
    }

    /// Whether the attractor itself is moving under a playing timeline, so
    /// the ring should turn over at the fill rate: ~1 s instead of ~13, which
    /// trades grain for a ghost that keeps up.
    fn anim_churning(&self) -> bool {
        self.playing
            && self.live_anim.as_ref().is_some_and(|a| {
                a.animator.as_ref().and_then(|x| x.class()).is_some_and(|c| c.refills())
            })
    }

    /// Swap the playhead's view values in (and, called again with what it
    /// returned, back out). See `refresh_animation`.
    fn swap_anim_view(&mut self, mut v: AnimView) -> AnimView {
        fn swap<T: Copy>(slot: &mut T, held: &mut Option<T>) {
            if let Some(x) = held {
                std::mem::swap(slot, x);
            }
        }
        swap(&mut self.point_size, &mut v.point_size);
        swap(&mut self.exposure, &mut v.exposure);
        swap(&mut self.haze_amount, &mut v.haze);
        swap(&mut self.color_contrast, &mut v.contrast);
        swap(&mut self.scene.background, &mut v.background);
        v
    }

    fn refresh_window_title(&mut self) {
        // The filename, not the scene's display name: it's what the person
        // typed, what Ctrl+S writes, and what they'll look for on disk.
        let doc = self
            .scene_path
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .map(str::to_string)
            .or_else(|| {
                let png = self.scene_png.as_deref()?.file_name()?.to_string_lossy().into_owned();
                Some(format!("{} (from {})", self.scene.name, png))
            })
            .unwrap_or_else(|| format!("{} (unsaved)", self.scene.name));
        let previewing = self.preview_label().map_or(String::new(), |p| format!("{p} · "));
        let title = format!(
            "{previewing}{}{} — Fracturize{}",
            if self.is_dirty() { "*" } else { "" },
            doc,
            crate::version::feature_branch().map(|b| format!(" [{b}]")).unwrap_or_default(),
        );
        if self.window_title != title {
            self.window.set_title(&title);
            self.window_title = title;
        }
    }

    pub fn take_screenshot(&mut self) {
        let point_count = self.point_compute.valid_point_count();
        if point_count == 0 {
            log::warn!("No points to screenshot");
            self.pending_screenshot = false;
            if self.screenshot_to.take().is_some() {
                self.collab_screenshot_done(None);
            }
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
            self.camera.fov,
            SCREENSHOT_HEIGHT as f32,
            self.rendered_point_size(),
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
            self.scene.packs_rgb(),
        )
        .with_zoom_guard(self.zoom(), self.camera.eye());

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
            RenderMode::Splat | RenderMode::Volume => {
                // The accum texture resizes to the screenshot dimensions
                // here and back to the window's on the next frame
                self.splat_renderer.upload_camera(&self.gpu.queue, &camera);
                self.splat_renderer.upload_params(
                    &self.gpu.queue,
                    self.exposure,
                    self.buffer_capacity as f64,
                    SCREENSHOT_HEIGHT as f32,
                    crate::scene::clear_color(self.scene.background, alpha),
                    self.transparent_render.into(),
                    // A screenshot is what the window is showing, grade
                    // included — anything else and S would silently save a
                    // different picture from the one on screen.
                    self.grade,
                );
                self.sync_volume(self.buffer_capacity as f64, SCREENSHOT_HEIGHT as f32);
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

            if let Some(path) = self.screenshot_to.take() {
                // A render a thread message is waiting for: it says so if it
                // couldn't be made, and the message goes without it.
                let saved = image::save_buffer(&path, &pixels, SCREENSHOT_WIDTH, SCREENSHOT_HEIGHT, image::ColorType::Rgba8);
                if let Err(e) = &saved {
                    log::warn!("Couldn't render the view for the message: {e}");
                }
                drop(data);
                self.screenshot_buffer.unmap();
                self.pending_screenshot = false;
                self.collab_screenshot_done(saved.is_ok().then_some(path.as_path()));
                return;
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
            HelpAction::ToggleEmphasis => self.toggle_emphasis(),
            HelpAction::CameraMotion => self.toggle_camera_motion(),
            HelpAction::StepFrame => {
                if shift {
                    self.jump_to_key(true);
                } else {
                    self.step_frames(1);
                }
            }
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
            HelpAction::NewScene => self.new_blank_scene(),
            HelpAction::Quit => self.request_quit(),
            HelpAction::FrameSelected => self.frame_selected_transform(),
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
        // The playhead's view values, for this frame's drawing only. The UI
        // has already run, so nothing that edits the scene can see them.
        let held = self.swap_anim_view(self.anim_view);
        let outcome = self.render_frame(egui_renderer, paint_jobs, textures_delta, pixels_per_point);
        self.swap_anim_view(held);
        outcome
    }

    fn render_frame(
        &mut self,
        egui_renderer: &mut egui_wgpu::Renderer,
        paint_jobs: &[egui::ClippedPrimitive],
        textures_delta: &egui::TexturesDelta,
        pixels_per_point: f32,
    ) -> FrameOutcome {
        if self.pending_screenshot {
            self.take_screenshot();
        }
        // Before egui paints, so a thumbnail built or flown this frame is the
        // one it draws.
        self.thumbs.service(&self.gpu.device, &self.gpu.queue, egui_renderer);

        let (width, height) = self.gpu.size();
        let aspect = width as f32 / height as f32;
        let view_proj = self.camera.view_proj(aspect);

        // Advance circular buffer and get valid point count. What we *draw*
        // can be less — see `drawn_points` for the collapsed-attractor case.
        //
        // The emphasis is settled first, because what the chaos pass measures
        // this frame is part of the params this upload writes.
        self.sync_emphasis();
        self.sync_map_view();
        if self.emphasis_filling() || self.point_compute.mask_filling() || self.anim_churning() {
            // The ring still holds points measured against a different
            // transform. Churning at the fill rate replaces them in about a
            // second instead of thirteen, and changes nothing about the
            // picture while it does — same scene, same walkers, same
            // distribution, just dealt faster.
            self.point_compute.advance_fill(&self.gpu.queue);
        } else {
            self.point_compute.advance_frame(&self.gpu.queue, self.frame_dt);
        }
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

        // === STEP 1: CARRY THE POINTS THROUGH ANY WRAP, THEN RUN CHAOS ===
        // Before the chaos dispatch, which writes fresh points into the band
        // this pass is re-folding. In the frame's own encoder rather than a
        // submit of its own: it is one pass over the whole buffer, landing on
        // the single frame a period that is already doing the most work, and a
        // separate submit adds a queue flush to exactly that frame.
        self.point_compute
            .rewrap_in(&self.gpu.queue, &mut encoder, self.pending_rewrap);
        self.pending_rewrap = 0;
        self.point_compute.dispatch(&mut encoder);
        // Re-measure the reference the glow normalizes against, over the
        // points that dispatch just wrote included.
        self.point_compute.dispatch_emphasis_stats(&mut encoder);

        // === STEP 2: RENDER POINTS ===
        let haze = self.haze_uniforms();
        let camera = CameraUniforms::new(
            view_proj,
            self.camera.fov,
            height as f32,
            self.rendered_point_size(),
            aspect,
            1.0,
            haze.0,
            haze.1,
            haze.2,
            haze.3,
            self.color_contrast,
            self.scene.background.to_array(),
            // Overridden below when the window is transparent.
            false,
            self.scene.packs_rgb(),
        )
        .with_alpha(self.window_alpha())
        // Every frame, from this frame's eye: that is what makes the zoom's
        // edge guard track the camera instead of the world.
        .with_zoom_guard(self.zoom(), self.camera.eye());
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
                            load: wgpu::LoadOp::Clear(self.window_clear()),
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
            RenderMode::Splat | RenderMode::Volume => {
                self.splat_renderer.upload_camera(&self.gpu.queue, &camera);
                self.splat_renderer.upload_params(
                    &self.gpu.queue,
                    self.exposure,
                    // The *drawn* count, not the buffer's: exposure is
                    // normalized by it, so capping the draw (see
                    // `drawn_points`) doesn't change how bright the result is.
                    // During warmup this is below capacity too, which is the
                    // brightness ramp that has always been there.
                    self.buffer_capacity.min(point_count.max(1)) as f64,
                    height as f32,
                    self.window_clear(),
                    self.window_alpha(),
                    // The window's own grade — the whole point of it being a
                    // live control rather than a render setting.
                    self.grade,
                );
                // Absorbance is normalised by the same two numbers exposure is.
                self.sync_volume(self.buffer_capacity.min(point_count.max(1)) as f64, height as f32);
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

        // === STEP 2.4: LIGHT UP WHAT THE SELECTED TRANSFORM MOVES ===
        //
        // After the cloud and before the gizmos: it is light added to the
        // finished picture, and it belongs under the handles you are about to
        // grab rather than over them.
        let (run_start, run_len) = self.point_compute.emphasis_window();
        if run_len > 0 {
            self.emphasis_renderer.upload_camera(&self.gpu.queue, &camera);
            self.emphasis_renderer.upload_params(
                &self.gpu.queue,
                &EmphasisParams {
                    run_start,
                    run_len,
                    capacity: self.point_compute.buffer_capacity,
                    // Gather the glow the way the picture under it was drawn.
                    splat_mode: u32::from(self.render_mode.is_splat_family()),
                    background: self.scene.background.to_array(),
                    ..self.emphasis_params
                },
            );
            self.emphasis_renderer.render(
                &self.gpu.device,
                &mut encoder,
                &color_view,
                width,
                height,
                run_len,
            );
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

/// Free-standing so it can be tested without a GPU: `App` needs a device, and
/// this is a decision about two plain values.
fn grade_for_view(
    mode: RenderMode,
    grade: crate::gpu::points::splat::Grade,
) -> Option<crate::gpu::points::splat::Grade> {
    match mode {
        RenderMode::Points => None,
        RenderMode::Splat | RenderMode::Volume if grade.is_neutral() => None,
        RenderMode::Splat | RenderMode::Volume => Some(grade),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The label a never-renamed view gets. Anchored on real timestamps
    /// checked against `date -u`, including a leap day and a year boundary,
    /// because the whole point of the branch-free civil-date form is that it
    /// is right at the awkward dates rather than plausible at the easy ones.
    #[test]
    fn a_timestamped_view_reads_as_a_date() {
        assert_eq!(view_display_name("0"), "0");
        assert_eq!(view_display_name("1"), "1970-01-01 00:00");
        assert_eq!(view_display_name("1755353324"), "2025-08-16 14:08");
        // 2024-02-29, a leap day, and one second before 2025.
        assert_eq!(view_display_name("1709208000"), "2024-02-29 12:00");
        assert_eq!(view_display_name("1735689599"), "2024-12-31 23:59");
    }

    /// A name somebody typed is shown as they typed it — only a bare
    /// timestamp is reinterpreted, and "hero-shot-2" is not one.
    #[test]
    fn a_chosen_view_name_is_left_alone() {
        for name in ["hero-shot", "hero-shot-2", "", "12-34", "-1"] {
            assert_eq!(view_display_name(name), name);
        }
    }

    const STILL: Duration = Duration::from_secs(5);
    const JUST_MOVED: Duration = Duration::from_millis(50);

    /// A view file should carry a grade only when there is a grade to carry.
    ///
    /// Both silences matter and for different reasons. Under the points
    /// renderer there is no tonemap for a grade to act on, so writing one would
    /// describe a setting that could not have applied. And a *neutral* grade is
    /// the absence of one — `View::grade` resolves a missing field back to
    /// neutral — so writing it out would put three lines in every view file to
    /// say nothing happened.
    #[test]
    fn a_view_only_carries_a_grade_when_there_is_one() {
        use crate::gpu::points::splat::Grade;
        let graded = Grade { gamma: 2.4, gamma_threshold: 0.3, vibrancy: 0.8 };

        assert_eq!(grade_for_view(RenderMode::Splat, graded), Some(graded));
        assert_eq!(grade_for_view(RenderMode::Splat, Grade::NEUTRAL), None);
        // Points: silent even when the grade is set, because switching to the
        // points renderer does not clear it — it just has nowhere to apply.
        assert_eq!(grade_for_view(RenderMode::Points, graded), None);
        assert_eq!(grade_for_view(RenderMode::Points, Grade::NEUTRAL), None);
    }

    #[test]
    fn the_pointer_goes_when_it_is_parked_on_the_artwork_in_view_mode() {
        assert!(cursor_should_hide(true, false, false, STILL));
    }

    #[test]
    fn edit_mode_always_keeps_the_pointer() {
        // Gizmos are things you aim at. Losing the cursor while lining one up
        // loses it at exactly the moment it is in use — so this holds however
        // long the hand has been still.
        assert!(!cursor_should_hide(false, false, false, STILL));
        assert!(!cursor_should_hide(false, false, false, Duration::from_secs(600)));
    }

    #[test]
    fn a_pointer_parked_on_a_panel_keeps_it() {
        assert!(!cursor_should_hide(true, true, false, STILL));
    }

    #[test]
    fn a_held_drag_keeps_it() {
        // An orbit drag held still for a few seconds is still a gesture in
        // progress, and the thing being dragged is under the pointer.
        assert!(!cursor_should_hide(true, false, true, STILL));
    }

    #[test]
    fn it_stays_while_the_pointer_is_being_used() {
        assert!(!cursor_should_hide(true, false, false, JUST_MOVED));
        // Right up to the threshold, and not before it.
        assert!(!cursor_should_hide(true, false, false, CURSOR_IDLE_HIDE - Duration::from_millis(1)));
        assert!(cursor_should_hide(true, false, false, CURSOR_IDLE_HIDE));
    }
}


/// What an edit from `was` to `now` asks record to key, in the paths a track
/// names (`App::autokey`): a vector as its three parts, a rotation as the
/// whole pose (or the one angle, where `tracks` already keys that angle).
/// `None` for an edit to the timeline itself, which keys nothing.
fn autokey_targets(
    was: &Scene,
    now: &Scene,
    tracks: &[crate::track::Track],
) -> Option<Vec<(String, crate::track::KeyValue)>> {
    use crate::scene_diff::Change;
    use crate::track::KeyValue;
    let changes = crate::scene_diff::diff(was, now).ok()?;
    // The camera's keys are a `[[track]]` too.
    if changes.iter().any(|c| matches!(c, Change::Replaced(p) | Change::Unset(p) if p.starts_with("track"))) {
        return None;
    }
    // (target, value), in the parts a track names.
    let mut keys: Vec<(String, KeyValue)> = Vec::new();
    let mut push = |target: String, v: KeyValue| {
        if !keys.iter().any(|(k, _)| *k == target) {
            keys.push((target, v));
        }
    };
    for c in &changes {
        let Change::Set(set) = c else { continue };
        let Some((path, text)) = set.split_once('=') else { continue };
        if path.starts_with("camera.") {
            continue;
        }
        let Some(value) = toml::from_str::<toml::Table>(&format!("v = {text}")).ok().and_then(|t| t.get("v").cloned())
        else {
            continue;
        };
        let number = |v: &toml::Value| v.as_float().or(v.as_integer().map(|i| i as f64));
        // A rotation, by an angle, a triple or the exact form: the pose.
        let stem = path
            .strip_suffix(".rotvec")
            .or_else(|| path.strip_suffix(".rotation"))
            .or_else(|| path.rsplit_once(".rotation.").map(|(s, _)| s));
        if let Some(stem) = stem.filter(|s| crate::track::takes_rotations(&format!("{s}.rotation"))) {
            let whole = format!("{stem}.rotation");
            // Keyed by one angle already: that angle stays its own track.
            if path.contains(".rotation.") && tracks.iter().any(|tr| tr.target == path) {
                if let Some(x) = number(&value) {
                    push(path.to_string(), KeyValue::Number(x));
                }
                continue;
            }
            let pose = match crate::track::authored_value(now, &whole).ok().flatten() {
                Some(toml::Value::Array(a)) => match a.iter().filter_map(number).collect::<Vec<_>>()[..] {
                    [x, y, z] => Some(crate::track::RotKey::from_euler([x, y, z])),
                    _ => None,
                },
                _ => match crate::track::authored_value(now, &format!("{stem}.rotvec")).ok().flatten() {
                    Some(toml::Value::Array(a)) => match a.iter().filter_map(number).collect::<Vec<_>>()[..] {
                        [x, y, z] => Some(crate::track::RotKey::from_rotvec(glam::vec3(x as f32, y as f32, z as f32))),
                        _ => None,
                    },
                    _ => None,
                },
            };
            if let Some(pose) = pose {
                push(whole, KeyValue::Rotation(pose));
            }
            continue;
        }
        match &value {
            toml::Value::Array(a) if a.len() == 3 => {
                let parts = if path.ends_with("color") || path.ends_with("background") { ["r", "g", "b"] } else { ["x", "y", "z"] };
                for (part, v) in parts.iter().zip(a) {
                    if let Some(x) = number(v) {
                        push(format!("{path}.{part}"), KeyValue::Number(x));
                    }
                }
            }
            // A uniform scale is three equal parts, as the parameter
            // tree offers it.
            v if path.ends_with("scale") && number(v).is_some() => {
                for part in ["x", "y", "z"] {
                    push(format!("{path}.{part}"), KeyValue::Number(number(v).unwrap()));
                }
            }
            v => match number(v) {
                Some(x) => push(path.to_string(), KeyValue::Number(x)),
                None => push(path.to_string(), KeyValue::Other(v.clone())),
            },
        }
    }
    Some(keys)
}

#[cfg(test)]
mod axis_scale_tests {
    use super::*;

    fn cell() -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::new(0.6, 0.4, 0.5),
            glam::Quat::from_euler(glam::EulerRot::XYZ, 0.3, -0.7, 0.2),
            Vec3::new(0.1, -0.2, 0.3),
        )
    }

    /// The scale handle exists to change one axis. If it moved anything else,
    /// it would be the uniform-scale drag with extra steps.
    #[test]
    fn only_the_dragged_column_moves() {
        let start = cell();
        for k in 0..3 {
            let dir = start.col(k).truncate().normalize();
            let out = with_scaled_column(start, k, dir, 1.25);
            for other in (0..3).filter(|&o| o != k) {
                assert_eq!(
                    out.col(other), start.col(other),
                    "scaling axis {k} disturbed axis {other}"
                );
            }
            assert_eq!(out.w_axis, start.w_axis, "scaling axis {k} moved the origin");
            assert!((out.col(k).truncate().length() - 1.25).abs() < 1e-5);
        }
    }

    /// Dragging the handle back through the origin mirrors the map. It has to
    /// be one continuous move — no jump, no branch — because it is reached by
    /// dragging, not by choosing it.
    #[test]
    fn passing_through_zero_mirrors_the_axis() {
        let start = cell();
        let dir = start.col(1).truncate().normalize();

        let before = with_scaled_column(start, 1, dir, 0.05);
        let after = with_scaled_column(start, 1, dir, -0.05);

        // The column reverses, and nothing else changes.
        assert!((before.col(1) + after.col(1)).length() < 1e-6);
        assert_eq!(before.col(0), after.col(0));
        assert_eq!(before.col(2), after.col(2));
        // Handedness flips, which is what "mirrored" means.
        assert!(before.determinant() * after.determinant() < 0.0);
    }

    /// The guard that keeps this feature saveable: an axis-scale drag must
    /// never introduce shear, because the scene format has no way to write it
    /// (GIZMO-PLAN.md §1.1). Columns stay perpendicular because the direction
    /// is fixed at grab time and only the length moves.
    #[test]
    fn scaling_every_axis_leaves_the_matrix_decomposable() {
        let mut m = cell();
        for (k, len) in [(0usize, 0.9f32), (1, 0.15), (2, 1.7)] {
            let dir = m.col(k).truncate().normalize();
            m = with_scaled_column(m, k, dir, len);
        }
        let trs = crate::rot::Trs::of(m);
        assert!(
            trs.is_faithful(m),
            "an axis-scale drag produced a matrix the save path cannot write"
        );
        // And the lengths are the ones asked for, in order.
        for (k, len) in [(0usize, 0.9f32), (1, 0.15), (2, 1.7)] {
            assert!((m.col(k).truncate().length() - len).abs() < 1e-5, "axis {k}");
        }
    }

    /// A mirrored matrix is still a *rigid* frame — perpendicular columns, one
    /// of them reversed — so it round-trips through the decomposition exactly.
    /// `Trs::is_faithful` rejects it anyway on the determinant sign alone; that
    /// is slice 2's job, and this test pins the fact it depends on.
    #[test]
    fn a_mirrored_cell_recomposes_exactly() {
        let start = cell();
        let dir = start.col(2).truncate().normalize();
        let m = with_scaled_column(start, 2, dir, -0.5);

        let trs = crate::rot::Trs::of(m);
        let back = trs.matrix();
        for k in 0..4 {
            assert!(
                (m.col(k) - back.col(k)).length() < 1e-5,
                "column {k} did not survive the round trip: {:?} vs {:?}",
                m.col(k), back.col(k)
            );
        }
        assert!(m.determinant() < 0.0);
    }

    #[test]
    fn the_grab_offset_means_a_drag_starts_where_you_grabbed() {
        // Grabbed 0.1 short of the endpoint: the length shouldn't jump to meet
        // the pointer, it should move with it.
        let len = axis_scale_length(0.6, 0.5, 0.5, false);
        assert!((len - 0.6).abs() < 1e-6, "a still pointer changed the length");
        let len = axis_scale_length(0.6, 0.8, 0.5, false);
        assert!((len - 0.9).abs() < 1e-6);
    }

    #[test]
    fn a_column_never_reaches_zero_and_keeps_its_sign() {
        // Just above zero clamps up and stays positive...
        let len = axis_scale_length(0.5, 5e-5, 0.5, false);
        assert!(len > 0.0 && len <= MIN_AXIS_LEN, "got {len}");
        // ...and just below stays negative, so the mirror doesn't stick on the
        // way through: the sign the pointer asked for is the sign it gets.
        let len = axis_scale_length(0.5, -5e-5, 0.5, false);
        assert!(len < 0.0 && len >= -MIN_AXIS_LEN, "got {len}");
        // Exactly zero has to go somewhere; positive is the tie-break, so a
        // drag that stops dead on the origin leaves the map unmirrored.
        assert!(axis_scale_length(0.5, 0.0, 0.5, false) > 0.0);
    }

    #[test]
    fn alt_snaps_to_tenths_including_through_zero() {
        assert!((axis_scale_length(0.63, 0.5, 0.5, true) - 0.6).abs() < 1e-6);
        assert!((axis_scale_length(0.66, 0.5, 0.5, true) - 0.7).abs() < 1e-6);
        assert!((axis_scale_length(0.6, 0.5, 1.2, true) + 0.1).abs() < 1e-6);
    }
}

#[cfg(test)]
mod autokey_tests {
    use super::*;
    use crate::track::KeyValue;

    fn scene() -> Scene {
        Scene::load(concat!(env!("CARGO_MANIFEST_DIR"), "/scenes/sierpinski.toml")).unwrap()
    }

    /// `scene` with `-S` specs applied, as an edit would leave it.
    fn edited(scene: &Scene, specs: &[&str]) -> Scene {
        let mut doc: toml_edit::DocumentMut = scene.to_toml_string().unwrap().parse().unwrap();
        for s in specs {
            crate::set::apply_to_doc(&mut doc, s).unwrap();
        }
        Scene::from_toml_str(&doc.to_string(), Path::new("scenes")).unwrap()
    }

    fn targets(was: &Scene, now: &Scene) -> Vec<String> {
        autokey_targets(was, now, &was.tracks).unwrap().into_iter().map(|(t, _)| t).collect()
    }

    /// A map picked up and moved keys its whole position, every axis, in the
    /// paths the parameter tree offers.
    #[test]
    fn a_move_keys_the_whole_position() {
        let a = scene();
        let b = edited(&a, &["transform.1.translation.y=0.9"]);
        assert_eq!(targets(&a, &b), ["transform.1.translation.x", "transform.1.translation.y", "transform.1.translation.z"]);
        let keys = autokey_targets(&a, &b, &[]).unwrap();
        assert_eq!(keys[1].1, KeyValue::Number(0.9));
        let cat = crate::params::catalogue(&a);
        for t in targets(&a, &b) {
            assert!(cat.iter().any(|p| p.path == t), "{t} isn't something a track can start on");
        }
    }

    /// A turn keys the pose, not one Euler angle.
    #[test]
    fn a_turn_keys_the_pose() {
        let a = scene();
        let b = edited(&a, &["transform.2.rotation.z=30.0"]);
        let keys = autokey_targets(&a, &b, &[]).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].0, "transform.2.rotation");
        assert!(matches!(keys[0].1, KeyValue::Rotation(r) if r.euler == Some([0.0, 0.0, 30.0])));
    }

    #[test]
    fn a_weight_keys_a_number() {
        let a = scene();
        let b = edited(&a, &["transform.0.variations.julia=0.4"]);
        let keys = autokey_targets(&a, &b, &[]).unwrap();
        assert!(keys.iter().any(|(t, v)| t == "transform.0.variations.julia" && *v == KeyValue::Number(0.4)), "{keys:?}");
    }

    /// Editing the timeline itself is not something to key.
    #[test]
    fn a_timeline_edit_keys_nothing() {
        let a = scene();
        let mut b = a.clone();
        b.camera_path = Some(crate::path::CameraPath::new(
            vec![crate::path::PathKey::from_camera(&OrbitCamera::from_chart(0.3, 0.2, 0.0, 3.0, Vec3::ZERO))],
            crate::path::Loop::Once,
        ));
        assert!(autokey_targets(&a, &b, &[]).is_none_or(|k| k.is_empty()));
    }
}
