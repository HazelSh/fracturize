//! Batch render jobs: what to render, how to watch it, and how to stop it.
//!
//! The old "HQ render" was one button with no parameters, no progress and no
//! way to stop. A 100M-point 4K frame is minutes of GPU work you have
//! committed to blind — long enough that a wrong parameter is expensive and a
//! misclick on cancel is worse.
//!
//! Two things this module exists to keep true:
//!
//! * **Job parameters are job-scoped.** A batch at 100M points must not touch
//!   the interactive `buffer_capacity` or prefs. Rendering big and continuing
//!   to explore at a comfortable point count is the whole reason to separate
//!   them, and the previous HQ render read the live capacity.
//! * **Estimates don't lie.** Memory is exact arithmetic and is checked
//!   against the device limit before anything starts. Time is extrapolated
//!   from measured throughput and shown as a range, refined from real progress
//!   once the job is running — never a fake-precise countdown.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Bytes per point in the chaos-game buffer (`vec3<f32>` position + `u32`
/// colour index — see `PointCompute`).
pub const BYTES_PER_POINT: u64 = 16;

/// Seconds per output pixel to encode one AV1 frame.
///
/// Measured on the GTX 1080 desktop across four points — 960x540, 1280x720 and
/// 1920x1080, at 12, 24 and 36 frames — which came out at 5.2, 5.8, 6.0 and
/// 6.5e-7 s/px, linear in both pixels and frame count. Note that most of it
/// lands in rav1e's flush rather than in the per-frame push, so a job that
/// looks nearly done can still have real work left; the estimate counts the
/// whole thing.
///
/// This term dominates an animation and nothing else comes close: at 720p it
/// is ~75x the cost of rendering the frame. The first version of the estimator
/// used one small constant for both output kinds and confidently offered
/// "12 – 28s" for a job that would have taken two minutes, which is exactly
/// the kind of lie an estimate must not tell.
pub const AV1_SECS_PER_PIXEL: f32 = 6.0e-7;

/// The same for one H.264 frame, for `.mp4` output.
///
/// Measured on `scenes/winze.toml`, 36 frames at 12 fps, across 480x270,
/// 960x540, 1280x720 and 1920x1080: 4.7, 5.7 and 7.0e-8 s/px for the three
/// successive pairs, so ~6e-8 over the encode-dominated part of the range.
///
/// It is two orders of magnitude under the AV1 figure and that is not a typo.
/// On the same clip and machine, AV1 spent 17.2s where H.264 spent 0.35s — a
/// ~50x gap, because openh264 at constant QP encodes on the way in and has
/// nothing left to flush, while rav1e defers most of its work to the end.
/// Quoting the AV1 constant for an MP4 job would overstate a 1080p animation
/// by minutes, which is exactly the class of lie `AV1_SECS_PER_PIXEL` was
/// introduced to stop telling — in the other direction.
///
/// Both figures include the per-frame render, which is small here but not
/// zero; if this is ever re-measured on another machine, scale them together.
pub const H264_SECS_PER_PIXEL: f32 = 6.0e-8;

/// The same for an 8-bit PNG: readback, then deflate on an already-rendered
/// buffer.
///
/// Measured on the GTX 1080 desktop at 720p, 1080p and 4K, all within 5% of
/// each other per pixel once the image has converged. This is **20x the figure
/// that stood here before**, which was small enough to make saving invisible in
/// the estimate — and saving is the majority of a large still's wall clock
/// (~10.6s of a 13.8s 8K job). An estimate that hides the dominant term is the
/// same class of lie `AV1_SECS_PER_PIXEL` was introduced to stop telling.
///
/// It depends on the *content*, not just the size, because deflate is doing
/// the work: a nearly-empty 1080p frame at 20 spp saves in 0.43s where a
/// converged one at 1,000 spp takes 0.85s, and it saturates there. The
/// converged figure is the one quoted, since accumulation is now the default
/// and a job worth estimating is a job worth converging.
pub const PNG_SECS_PER_PIXEL: f32 = 4.0e-7;

/// The same for a 16-bit PNG: **2.5x** the 8-bit cost, measured the same way
/// (1080p 2.11s and 4K 6.35s against 0.85s and 1.69s).
///
/// Twice the bytes through deflate, and they are the *noisy* low bytes, so it
/// is worse than linear in the data and better than the depth ratio suggests.
/// Quoted separately rather than folded into one average because bit depth is
/// a checkbox in the dialog: picking it should move the estimate, and with one
/// shared constant it would not.
pub const PNG16_SECS_PER_PIXEL: f32 = 1.0e-6;

/// Seconds to fold one histogram texel into the accumulator, as a fraction of
/// the measured point throughput.
///
/// The accumulating path pays this once per lap over `width * height * N²`
/// texels, and it is the term that makes supersampling expensive: at 1080p the
/// per-lap cost goes 0.0019s at 1x to 0.0367s at 4x, tracking texel count and
/// nothing else. Measured at ~1.12ns/texel against ~664M points/s of chaos on
/// the GTX 1080, i.e. the fold moves about 1.34 texels per point-slot of
/// throughput.
///
/// Expressed as a ratio rather than an absolute so it tracks the machine: a
/// GPU that fills points twice as fast folds about twice as fast too, and a
/// hardcoded nanosecond figure would be a GTX 1080 constant quoted at a laptop.
pub const FOLD_TEXELS_PER_POINT: f32 = 1.34;

/// CPU threads a job may use, when nobody said otherwise: **one less than the
/// machine has**.
///
/// Both video encoders used to call `available_parallelism()` themselves and
/// hand the whole answer to the codec. On the reference desktop — an i5-6600,
/// four cores and no SMT — that is every core, with nothing held back for the
/// desktop the render is running behind. It has not bitten yet only because
/// renders finish in under a second; the AV1 flush is ~75x the cost of
/// rendering a frame, so a long animation job would spend the majority of its
/// wall clock fully saturated.
///
/// Held to at least 1, since a zero-thread encoder is not a lighter one.
pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .saturating_sub(1)
        .max(1)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum JobKind {
    Still { width: u32, height: u32 },
    Animation {
        width: u32,
        height: u32,
        fps: u32,
        seconds: f32,
        quality: u8,
        /// `.avif` (AV1) or `.mp4` (H.264) — see `src/video.rs`
        format: crate::video::Format,
    },
    /// No render at all: write a view file describing the current framing, to
    /// be rendered later (or by the CLI, or on another machine).
    ViewDescriptor,
}

impl JobKind {
    pub fn extension(&self) -> &'static str {
        match self {
            JobKind::Still { .. } => "png",
            JobKind::Animation { format, .. } => format.extension(),
            JobKind::ViewDescriptor => "toml",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            JobKind::Still { .. } => "still",
            JobKind::Animation { format, .. } => match format {
                crate::video::Format::Avif => "AVIF animation",
                crate::video::Format::Mp4 => "MP4 animation",
            },
            JobKind::ViewDescriptor => "view descriptor",
        }
    }

    /// Seconds of encoding per output pixel, which is the term that dominates
    /// an animation and differs by an order of magnitude between the codecs —
    /// and dominates a large still too, which the estimate used to miss.
    ///
    /// Takes the depth because a still's cost depends on it and an animation's
    /// does not: video is 8-bit by codec, so the parameter is simply ignored
    /// there rather than being a second thing to keep in step.
    pub fn secs_per_pixel(&self, bit_depth: crate::offline::BitDepth) -> f32 {
        match self {
            JobKind::Still { .. } => match bit_depth {
                crate::offline::BitDepth::Eight => PNG_SECS_PER_PIXEL,
                crate::offline::BitDepth::Sixteen => PNG16_SECS_PER_PIXEL,
            },
            JobKind::Animation { format, .. } => match format {
                crate::video::Format::Avif => AV1_SECS_PER_PIXEL,
                crate::video::Format::Mp4 => H264_SECS_PER_PIXEL,
            },
            JobKind::ViewDescriptor => 0.0,
        }
    }

    /// Frames this job renders — 1 for a still, and for an animation the count
    /// the encoder will actually produce.
    pub fn frames(&self) -> u32 {
        match self {
            JobKind::Still { .. } => 1,
            JobKind::Animation { fps, seconds, .. } => {
                ((seconds * *fps as f32).round() as u32).max(2)
            }
            JobKind::ViewDescriptor => 0,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        match self {
            JobKind::Still { width, height } | JobKind::Animation { width, height, .. } => {
                (*width, *height)
            }
            JobKind::ViewDescriptor => (0, 0),
        }
    }
}

/// How many samples the job asks for — and therefore which of the two
/// renderers runs.
///
/// The distinction is not a implementation detail leaking into the form. The
/// two paths answer the *same* question with opposite scaling, and that is
/// exactly what a person choosing between them needs to see:
///
/// * [`Samples::Ring`] fills the point buffer once and splats it, so the
///   density it delivers is `points / pixels` — it **falls as the output
///   grows**. The same settings that give 21.7 samples/px at 720p give 2.4 at
///   4K, which is why big renders out of this dialog came out grainy.
/// * [`Samples::Accumulate`] folds lap after lap into a persistent histogram
///   until it reaches a target measured *per output pixel*, so the density is
///   resolution-independent by construction and the cost scales with the
///   picture instead of the buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Samples {
    /// One pass over the filled ring, plus `accumulate` extra chaos frames
    /// stirred in before it is splatted. Cheap, bounded, and the only thing an
    /// animation can do — every frame needs its own camera, so there is no
    /// histogram to carry between them.
    Ring { accumulate: u32 },
    /// Accumulate until `spp` samples per output pixel have landed. An anytime
    /// algorithm: stopping early gives a noisier version of the same picture,
    /// never a darker or a partial one.
    Accumulate { spp: u32 },
}

impl Samples {
    /// The per-output-pixel sample target, when there is one.
    pub fn spp(self) -> Option<u32> {
        match self {
            Samples::Ring { .. } => None,
            Samples::Accumulate { spp } => Some(spp),
        }
    }

    /// Extra chaos frames for the ring path. The accumulating path ignores it
    /// — `spp` is what decides how long the chaos game runs there — so this
    /// hands back the default rather than a number that would read as meaning
    /// something.
    pub fn accumulate(self) -> u32 {
        match self {
            Samples::Ring { accumulate } => accumulate,
            Samples::Accumulate { .. } => crate::offline::DEFAULT_ACCUMULATE,
        }
    }
}

#[derive(Clone, Debug)]
pub struct JobParams {
    pub kind: JobKind,
    pub out_path: PathBuf,
    /// Points in the chaos-game buffer. Job-scoped: never read from or
    /// written back to the interactive capacity or prefs.
    ///
    /// Its meaning depends on [`samples`](Self::samples): under
    /// [`Samples::Ring`] the buffer *is* the sample count, and under
    /// [`Samples::Accumulate`] it is only a working set — bigger just means
    /// fewer, larger laps for the same result.
    pub points: usize,
    /// How much sampling to do, and which renderer that implies.
    pub samples: Samples,
    pub splat: bool,
    pub exposure: f32,
    pub transparent: bool,
    /// Render the histogram at `N x` output resolution and filter down. A
    /// cost/quality knob at fixed artistic intent, so it belongs to the job
    /// alongside `points` — not to the scene.
    pub supersample: u32,
    /// Reconstruction kernel for that downsample
    pub filter: crate::gpu::Filter,
    /// Kernel half-width in output pixels
    pub filter_radius: f32,
    /// Bits per channel in the PNG. An output-format choice, not a quality
    /// one — the render is identical either way.
    pub bit_depth: crate::offline::BitDepth,
    /// The variable-width blur: wide kernels where the histogram is sparse and
    /// noisy, narrow where it is dense and detailed.
    ///
    /// Needs an accumulating render, and not for a passing reason —
    /// `TARGET_DENSITY` is calibrated in raw accumulated units, so there has to
    /// be a histogram for it to read a density from.
    pub density_estimation: crate::gpu::points::density::DensityEstimation,
    /// CPU threads this job's encoders may use. A **machine** setting, not
    /// artwork: it describes the box, so it lives in prefs and on the command
    /// line and never in a scene or a view. A sidecar may record what was used,
    /// as information, but nothing should ever *replay* it — `threads = 16` is
    /// actively wrong advice on the laptop.
    ///
    /// One value per job, read by every CPU-side thing the job spawns, so
    /// there is no second place to forget. See [`default_threads`].
    pub threads: usize,
}

impl JobParams {
    /// GPU memory the point buffer will need, in bytes. Exact, not a guess —
    /// it's one multiplication, and it's the number that decides whether the
    /// job can run at all.
    pub fn point_buffer_bytes(&self) -> u64 {
        self.points as u64 * BYTES_PER_POINT
    }

    /// Everything the job allocates on the GPU: the point buffer plus the
    /// render target and its readback staging copy, both `width * height * 4`.
    /// The splat renderer adds an rgba16float accumulation target at 8 bytes
    /// per pixel.
    ///
    /// Supersampling multiplies the per-pixel surfaces by N² and is the term
    /// that grows fastest, so it is counted rather than assumed small: at 4K
    /// and 4x it is over a gigabyte, which is the difference between a job that
    /// runs and one that doesn't.
    pub fn total_bytes(&self) -> u64 {
        let (w, h) = self.kind.size();
        let pixels = w as u64 * h as u64;
        let n2 = (self.supersample.max(1) as u64).pow(2);
        // Output-sized colour target + its readback staging copy
        let target = pixels * 4 * 2;
        let extra = if self.splat {
            // rgba16float accumulation at N x, plus the output-sized resolved
            // copy the tonemap reads when the filter is on
            pixels * 8 * n2 + if n2 > 1 { pixels * 8 } else { 0 }
        } else if n2 > 1 {
            // The points path rasterizes into an N x colour surface with a
            // matching depth buffer, both 4 bytes per texel
            pixels * 8 * n2
        } else {
            0
        };
        self.point_buffer_bytes() + target + extra + self.histogram_bytes()
    }

    /// The persistent accumulation histogram, in bytes — zero unless the job
    /// is accumulating.
    ///
    /// 32 bytes a texel (four channels of 64-bit fixed point) over
    /// `width * height * N²`, and it is far and away the largest thing an
    /// accumulating job allocates: at 4K and 2x it is 1.06 GB against 33 MB of
    /// point buffer. It is also a **single storage buffer**, so it is the term
    /// that runs into `max_storage_buffer_binding_size` first — see
    /// [`rejection`](Self::rejection), and `RENDER-SCALE-PLAN.md` §2 for why
    /// that limit and not VRAM is the ceiling this whole plan is built around.
    pub fn histogram_bytes(&self) -> u64 {
        if self.samples.spp().is_none() {
            return 0;
        }
        let (w, h) = self.kind.size();
        let n2 = (self.supersample.max(1) as u64).pow(2);
        w as u64 * h as u64 * n2 * crate::gpu::points::accumulate::BYTES_PER_TEXEL
    }

    /// How this job will be cut into tiles on a GPU with this binding limit.
    ///
    /// The dialog asks for the same reason the renderer does: past 67 M
    /// histogram texels a render is no longer one pass over one buffer, and
    /// what it costs is decided here rather than by the output size. Built from
    /// the same [`crate::tile::TilePlan`] the renderer uses, so the figure
    /// quoted before you agree to a job is the one the job then runs.
    pub fn tile_plan(
        &self,
        max_buffer_bytes: u64,
    ) -> Result<crate::tile::TilePlan, crate::tile::TileError> {
        let (w, h) = self.kind.size();
        crate::tile::TilePlan::new(
            w,
            h,
            self.supersample,
            crate::tile::Halo::for_settings(self.density_estimation, self.filter_radius),
            crate::tile::Budget {
                binding_limit: max_buffer_bytes,
                resident_limit: max_buffer_bytes,
            },
        )
    }

    /// Laps the accumulating path will run: enough folds of the whole point
    /// buffer to deposit `spp` samples on every output pixel.
    ///
    /// `spp` counts against *output* pixels rather than histogram texels, which
    /// is what makes supersampling cost N² more fill per lap without silently
    /// demanding N² more laps for the same stated quality.
    pub fn laps(&self) -> u32 {
        let Some(spp) = self.samples.spp() else { return 0 };
        let (w, h) = self.kind.size();
        let samples = spp as u64 * w as u64 * h as u64;
        let capacity = (self.points as u64).max(1);
        samples.div_ceil(capacity).min(u32::MAX as u64) as u32
    }

    /// Why this job can't run, if it can't. Checked before anything is
    /// allocated, because the failure mode otherwise is a device-lost panic
    /// several seconds in.
    pub fn rejection(&self, max_buffer_bytes: u64) -> Option<String> {
        if matches!(self.kind, JobKind::ViewDescriptor) {
            return None;
        }
        if self.point_buffer_bytes() > max_buffer_bytes {
            return Some(format!(
                "{} points needs a {} point buffer, over this GPU's {} limit for a single \
                 buffer. Render fewer points, or split the work.",
                self.points,
                human_bytes(self.point_buffer_bytes()),
                human_bytes(max_buffer_bytes),
            ));
        }
        let (w, h) = self.kind.size();
        if w == 0 || h == 0 {
            return Some("Width and height must both be non-zero.".to_string());
        }
        // The histogram is one storage buffer under the same binding limit, and
        // at any real output size it is much the larger of the two. It no
        // longer *refuses* a job for being over it — the renderer tiles — so
        // the only thing left to catch is a request no tiling can satisfy.
        if self.samples.spp().is_some() {
            if let Err(e) = self.tile_plan(max_buffer_bytes) {
                return Some(e.to_string());
            }
        }
        if let JobKind::Animation { fps, seconds, .. } = self.kind {
            if fps == 0 {
                return Some("Frame rate must be at least 1.".to_string());
            }
            if seconds <= 0.0 {
                return Some("Duration must be greater than zero.".to_string());
            }
        }
        None
    }
}

/// What a running job reports back. Ordinary channel messages rather than
/// shared state: the job runs on its own thread with its own wgpu device, and
/// a queue of events is the one shape that can't tear.
#[derive(Debug)]
pub enum JobEvent {
    /// Moved on to a named stage ("setting up", "filling points", …)
    Phase(&'static str),
    Progress { done: u32, total: u32 },
    Log(String),
    /// Finished: where the file went and whether the job ran to its target, or
    /// why it failed. See [`Outcome`] — a job stopped early usually still
    /// writes something, and must never be announced as a completed one.
    Done(Result<(PathBuf, Outcome), String>),
}

/// The job's half of the connection: where to report, and the two flags it
/// polls. Cloned into `OfflineParams` and checked inside the render loops.
#[derive(Clone)]
pub struct JobControl {
    pub events: Sender<JobEvent>,
    pub cancel: Arc<AtomicBool>,
    pub pause: Arc<AtomicBool>,
}

impl JobControl {
    pub fn phase(&self, name: &'static str) {
        let _ = self.events.send(JobEvent::Phase(name));
    }

    pub fn log(&self, msg: impl Into<String>) {
        let _ = self.events.send(JobEvent::Log(msg.into()));
    }

    pub fn progress(&self, done: u32, total: u32) {
        let _ = self.events.send(JobEvent::Progress { done, total });
    }

    /// Block while paused, then report whether the job should stop.
    ///
    /// A sleep loop rather than a condvar: the GPU work is already chunked
    /// into frames and tiles, so the worst-case latency is one poll interval,
    /// and there is no lock here for the interactive renderer to contend on —
    /// the job holds a separate wgpu device.
    ///
    /// Returns `true` if the caller should abandon the job.
    pub fn should_stop(&self) -> bool {
        while self.pause.load(Ordering::Relaxed) && !self.cancel.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Sentinel error a cancelled job returns **when there was nothing worth
/// keeping**, so callers can tell "you stopped this" apart from "this broke"
/// and skip the failure reporting.
///
/// Most cancellations no longer reach this: see [`Outcome::Partial`].
pub const CANCELLED: &str = "cancelled";

/// How a render ended.
///
/// The chaos game is an **anytime algorithm** — a buffer stopped at 60% is the
/// same picture as one stopped at 100%, just noisier — so cancelling used to
/// throw away something genuinely usable. `fill_points` returned
/// `Err(CANCELLED)`, that propagated through `render()`'s `?`, and stopping a
/// job at 99% got you nothing at all. Tolerable when a render took 0.35s;
/// not once renders are long, which is the whole direction of the render
/// quality work.
///
/// An enum rather than a bool or a second string sentinel because every
/// reporting site has to handle both arms, and the compiler is what makes sure
/// a partial render is never announced as a finished one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Ran to the requested target.
    Complete,
    /// Stopped early. The file *was* written, from whatever had accumulated by
    /// then — noisier than asked for, and never silently passed off as
    /// finished.
    Partial,
}

impl Outcome {
    /// Once anything has been cut short, the whole job is partial.
    pub fn and(self, other: Outcome) -> Outcome {
        match (self, other) {
            (Outcome::Complete, Outcome::Complete) => Outcome::Complete,
            _ => Outcome::Partial,
        }
    }

    pub fn is_partial(self) -> bool {
        self == Outcome::Partial
    }
}

/// Bytes as something a person can compare against a GPU spec sheet.
pub fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KIB * KIB * KIB {
        format!("{:.2} GiB", b / (KIB * KIB * KIB))
    } else if b >= KIB * KIB {
        format!("{:.0} MiB", b / (KIB * KIB))
    } else {
        format!("{:.0} KiB", b / KIB)
    }
}

/// A duration as a range, rendered the way an estimate should read.
///
/// Estimates are shown as a span rather than a number because the throughput
/// they come from is measured on a *different* workload — the interactive
/// renderer, at a different point count and resolution. Quoting "4m 12s" from
/// that would be a lie with a decimal point on it.
pub fn format_estimate(low_secs: f32, high_secs: f32) -> String {
    if !low_secs.is_finite() || low_secs <= 0.0 {
        return "unknown".to_string();
    }
    format!("{} – {}", format_duration(low_secs), format_duration(high_secs))
}

pub fn format_duration(secs: f32) -> String {
    let s = secs.max(0.0).round() as u64;
    match s {
        0..=59 => format!("{}s", s),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// A low/high seconds range for the job, from a measured point throughput.
///
/// The terms have wildly different weights depending on what is being
/// rendered, so all of them are counted rather than the small ones assumed
/// away:
///
/// * **the ring path** — filling the buffer (`points × frames`), then one pass
///   over the points to splat each frame. Both ÷ throughput.
/// * **the accumulating path** — `spp × pixels` points of chaos *however the
///   laps are cut*, plus one fold per lap over `pixels × N²` texels. Measured:
///   varying the buffer 6x at fixed `spp` moved the total under 25%, while
///   `spp` and pixels move it exactly in proportion. That is why the buffer is
///   a working set here and not a quality dial.
/// * **encoding** each frame — pixels × a per-format, per-depth constant. This
///   is the *majority* of a large still (~10.6s of a 13.8s 8K job), which the
///   estimate used to miss entirely; see [`PNG_SECS_PER_PIXEL`].
///
/// The ±40% spread is not decoration. The throughput comes from a different
/// workload at a different point count and resolution, so the honest thing is
/// to put the uncertainty in the width of the range rather than to quote a
/// number with a decimal point on it.
///
/// Takes the throughput rather than the `App` so the arithmetic can be checked
/// against real measurements — see the tests, which hold it to that ±40% band
/// on every row that was measured.
pub fn estimate_secs(
    throughput: f32,
    params: &JobParams,
    max_buffer_bytes: u64,
) -> Option<(f32, f32)> {
    if throughput <= 0.0 {
        return None;
    }
    let (w, h) = params.kind.size();
    let pixels = w as f32 * h as f32;
    // Per-codec, and per bit depth for a still: H.264 encodes about an order of
    // magnitude faster than AV1, and one shared constant would misquote
    // whichever it wasn't measured on.
    let encode = pixels * params.kind.secs_per_pixel(params.bit_depth);

    let chaos = match params.samples {
        // Warmup is roughly the buffer refilling once before the extra frames
        // start.
        Samples::Ring { accumulate } => {
            let fill_frames = accumulate.max(1) as f32 + 8.0;
            let per_frame = params.points as f32 / throughput;
            params.points as f32 * fill_frames / throughput
                + per_frame * params.kind.frames() as f32
        }
        Samples::Accumulate { spp } => {
            let n2 = (params.supersample.max(1) as f32).powi(2);
            let chaos = spp as f32 * pixels / throughput;
            let fold =
                params.laps() as f32 * pixels * n2 / (throughput * FOLD_TEXELS_PER_POINT);
            // Every tile re-runs the whole chaos game, because a lap deposits
            // its samples wherever the attractor puts them and a tile keeps
            // only the ones that land in its own window. So the tile count is a
            // straight multiplier on the render, and the largest single term in
            // what a poster costs — quoting a one-tile figure for a nine-tile
            // job would understate it by nine.
            let tiles = params
                .tile_plan(max_buffer_bytes)
                .map(|p| p.tiles.len() as f32)
                .unwrap_or(1.0);
            (chaos + fold) * tiles
        }
    };

    let mid = chaos + encode * params.kind.frames() as f32;
    Some((mid * 0.6, mid * 1.4))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference GTX 1080's binding limit, so the estimator's fixtures are
    /// priced on the machine they were measured on.
    #[allow(non_upper_case_globals)]
    const Budget_LIMIT: u64 = 2_147_483_648;

    fn still(points: usize, w: u32, h: u32) -> JobParams {
        JobParams {
            kind: JobKind::Still { width: w, height: h },
            out_path: PathBuf::from("renders/x.png"),
            points,
            samples: Samples::Ring { accumulate: 32 },
            density_estimation: Default::default(),
            splat: false,
            exposure: 1.0,
            transparent: false,
            supersample: 1,
            filter: crate::gpu::Filter::Gaussian,
            filter_radius: 0.5,
            bit_depth: crate::offline::BitDepth::Eight,
            threads: default_threads(),
        }
    }

    /// Supersampling is the fastest-growing term in a job's footprint, and the
    /// estimate is checked against the device limit *before* anything
    /// allocates — so it has to be in there.
    #[test]
    fn supersampling_is_counted_against_the_memory_budget() {
        // Compared against the *point buffer*, which is the same in both and
        // would otherwise swamp the term under test.
        let one = JobParams { splat: true, ..still(1_000_000, 100, 100) };
        let four = JobParams { supersample: 4, ..one.clone() };
        let px = 100u64 * 100;
        // The N x accumulation is 16x the 1x one, and the resolved copy is new
        assert_eq!(four.total_bytes() - one.total_bytes(), px * 8 * 15 + px * 8);
        // The points renderer pays for an N x colour surface *and* its depth
        let flat = JobParams { splat: false, ..one.clone() };
        let flat2 = JobParams { supersample: 2, ..flat.clone() };
        assert_eq!(flat2.total_bytes() - flat.total_bytes(), px * 8 * 4);
    }

    /// A ring job must not be charged for a histogram it never allocates, and
    /// an accumulating one must be — it is the largest single thing on the
    /// device and the term that decides whether the job can run at all.
    #[test]
    fn only_an_accumulating_job_pays_for_a_histogram() {
        let ring = JobParams { splat: true, ..still(1_000_000, 1920, 1080) };
        assert_eq!(ring.histogram_bytes(), 0);
        let acc = JobParams { samples: Samples::Accumulate { spp: 100 }, ..ring.clone() };
        let px = 1920u64 * 1080;
        assert_eq!(acc.histogram_bytes(), px * 32);
        assert_eq!(acc.total_bytes() - ring.total_bytes(), px * 32);
        // Squared in N, which is why the rejection message says to reach for
        // the supersampling first.
        let acc4 = JobParams { supersample: 4, ..acc.clone() };
        assert_eq!(acc4.histogram_bytes(), px * 16 * 32);
    }

    /// `spp` counts against *output* pixels, not histogram texels — so turning
    /// supersampling on costs N² more work per lap but must not silently demand
    /// N² more laps for the same stated quality.
    #[test]
    fn laps_cover_the_output_pixels_and_ignore_supersampling() {
        let base = JobParams {
            splat: true,
            samples: Samples::Accumulate { spp: 100 },
            ..still(12_000_000, 1920, 1080)
        };
        // 100 x 2,073,600 samples / 12M a lap = 17.28 -> 18 whole laps.
        assert_eq!(base.laps(), 18);
        assert_eq!(JobParams { supersample: 4, ..base.clone() }.laps(), 18);
        // A bigger working set is fewer, larger laps for the same total work.
        assert_eq!(JobParams { points: 24_000_000, ..base.clone() }.laps(), 9);
        // The ring path has no laps at all.
        assert_eq!(JobParams { samples: Samples::Ring { accumulate: 32 }, ..base }.laps(), 0);
    }

    /// A histogram over the binding limit used to be a refusal. It is now a
    /// tiling, and the dialog has to agree with the renderer about that — a
    /// dialog that still refused 4K at 4x would be the only thing standing
    /// between someone and the render tiling was built to deliver.
    #[test]
    fn a_histogram_over_the_binding_limit_tiles_instead_of_refusing() {
        // 2.15 GB, the measured limit on the reference GTX 1080.
        let limit = 2_147_483_648u64;
        let acc = JobParams {
            splat: true,
            samples: Samples::Accumulate { spp: 100 },
            supersample: 4,
            ..still(1_000_000, 3840, 2160)
        };
        // 8.29M px x 16 x 32 B = 4.2 GB, well over the limit.
        assert!(acc.histogram_bytes() > limit);
        assert_eq!(acc.rejection(limit), None, "4K at 4x must now be renderable");
        let plan = acc.tile_plan(limit).expect("and it must have a plan");
        assert!(plan.tiles.len() > 1, "which means more than one tile");

        // And the estimate has to know: every tile re-runs the whole chaos
        // game, so a job that tiles costs a multiple of one that doesn't.
        let one = JobParams { supersample: 1, ..acc.clone() };
        assert!(one.tile_plan(limit).unwrap().is_single());
        let (lo_tiled, _) = estimate_secs(664e6, &acc, limit).unwrap();
        let (lo_one, _) = estimate_secs(664e6, &one, limit).unwrap();
        assert!(
            lo_tiled > lo_one * plan.tiles.len() as f32 * 0.5,
            "tiling must show up in the estimate: {lo_tiled:.1}s over {lo_one:.1}s for {} tiles",
            plan.tiles.len(),
        );
    }

    /// The estimator's whole contract is that it prices a job *before* you
    /// agree to it, so it is held to real measurements rather than to itself.
    ///
    /// Every row is a timed `--spp` render of `scenes/lacewing.toml` on the
    /// reference GTX 1080, at ~664M points/s of measured chaos throughput.
    /// The quoted figure is `chaos fill + encode+save`, i.e. everything the
    /// dialog's estimate covers, and each must land inside the ±40% band the
    /// estimate itself advertises — otherwise the range is a lie in exactly
    /// the way the module doc says it must not be.
    #[test]
    fn the_accumulating_estimate_matches_measured_renders() {
        const THROUGHPUT: f32 = 664e6;
        // width, height, spp, supersample, measured seconds
        let rows: &[(u32, u32, u32, u32, f32)] = &[
            (1920, 1080, 50, 2, 0.27 + 0.61),
            (1920, 1080, 100, 2, 0.49 + 0.76),
            (1920, 1080, 200, 2, 1.00 + 0.93),
            (1920, 1080, 400, 2, 1.93 + 0.88),
            (1920, 1080, 800, 2, 3.87 + 0.92),
            (1280, 720, 200, 1, 0.28 + 0.43),
            (1920, 1080, 200, 1, 0.69 + 0.86),
            (1920, 1080, 200, 4, 1.91 + 0.76),
            (3840, 2160, 200, 1, 3.84 + 3.39),
            (3840, 2160, 100, 2, 3.61 + 3.27),
        ];
        for &(w, h, spp, n, measured) in rows {
            let params = JobParams {
                splat: true,
                samples: Samples::Accumulate { spp },
                supersample: n,
                ..still(12_000_000, w, h)
            };
            let (low, high) = estimate_secs(THROUGHPUT, &params, Budget_LIMIT).expect("a positive throughput");
            assert!(
                (low..=high).contains(&measured),
                "{w}x{h} spp={spp} {n}x: measured {measured:.2}s outside the estimate's \
                 {low:.2}-{high:.2}s band",
            );
        }
    }

    /// Saving is the majority of a large still, so the depth checkbox has to
    /// move the estimate — with one shared constant it would not, and a 16-bit
    /// 4K job would be quoted at well under half its real cost.
    #[test]
    fn bit_depth_moves_a_large_stills_estimate() {
        let eight = JobParams {
            splat: true,
            samples: Samples::Accumulate { spp: 200 },
            ..still(12_000_000, 3840, 2160)
        };
        let sixteen = JobParams { bit_depth: crate::offline::BitDepth::Sixteen, ..eight.clone() };
        let (lo8, hi8) = estimate_secs(664e6, &eight, Budget_LIMIT).unwrap();
        let (lo16, hi16) = estimate_secs(664e6, &sixteen, Budget_LIMIT).unwrap();
        assert!(lo16 > lo8 && hi16 > hi8);
        // Measured at 4K, converged: 3.84 + 3.39 against 3.91 + 7.92.
        assert!((lo8..=hi8).contains(&7.23), "8-bit 4K: {lo8:.2}-{hi8:.2}s");
        assert!((lo16..=hi16).contains(&11.83), "16-bit 4K: {lo16:.2}-{hi16:.2}s");
    }

    /// The default holds a core back for the rest of the desktop, and never
    /// reaches zero however few cores the machine reports.
    #[test]
    fn the_default_thread_count_leaves_a_core_for_the_desktop() {
        let all = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        assert!(default_threads() >= 1);
        assert!(default_threads() < all.max(2), "must hold something back");
    }

    #[test]
    fn memory_is_exact_arithmetic() {
        let j = still(10_000_000, 1920, 1080);
        assert_eq!(j.point_buffer_bytes(), 160_000_000);
        // point buffer + target + readback
        assert_eq!(j.total_bytes(), 160_000_000 + 1920 * 1080 * 4 * 2);
    }

    #[test]
    fn splat_costs_an_extra_hdr_target() {
        let a = still(1_000_000, 100, 100);
        let b = JobParams { splat: true, ..a.clone() };
        assert_eq!(b.total_bytes() - a.total_bytes(), 100 * 100 * 8);
    }

    #[test]
    fn oversized_jobs_are_refused_before_they_allocate() {
        let limit = 256 * 1024 * 1024;
        // 16 M points = 256 MiB exactly: allowed.
        assert!(still(16_777_216, 100, 100).rejection(limit).is_none());
        let over = still(16_777_217, 100, 100).rejection(limit);
        assert!(over.is_some_and(|m| m.contains("over this GPU")), "should name the limit");
    }

    #[test]
    fn view_descriptors_have_nothing_to_refuse() {
        let j = JobParams {
            kind: JobKind::ViewDescriptor,
            ..still(usize::MAX, 0, 0)
        };
        assert!(j.rejection(1).is_none());
    }

    #[test]
    fn zero_sized_and_zero_rate_jobs_are_refused() {
        assert!(still(1000, 0, 1080).rejection(u64::MAX).is_some());
        let j = JobParams {
            kind: JobKind::Animation {
                width: 640,
                height: 480,
                fps: 0,
                seconds: 2.0,
                quality: 60,
                format: crate::video::Format::Avif,
            },
            ..still(1000, 640, 480)
        };
        assert!(j.rejection(u64::MAX).is_some());
    }

    #[test]
    fn animation_frame_count_matches_the_encoder() {
        let k = JobKind::Animation {
            width: 8,
            height: 8,
            fps: 24,
            seconds: 2.5,
            quality: 60,
            format: crate::video::Format::Avif,
        };
        assert_eq!(k.frames(), 60);
        // Even a sub-frame duration renders something playable
        let tiny = JobKind::Animation {
            width: 8,
            height: 8,
            fps: 24,
            seconds: 0.01,
            quality: 60,
            format: crate::video::Format::Avif,
        };
        assert_eq!(tiny.frames(), 2);
    }

    /// The filename the dialog offers and the file the encoder writes are
    /// derived from the same place, so a format that reported the wrong
    /// extension would write an `.avif` named `.mp4` and nobody would notice
    /// until an upload bounced.
    #[test]
    fn animation_format_drives_extension_and_cost() {
        let anim = |format| JobKind::Animation {
            width: 1920,
            height: 1080,
            fps: 30,
            seconds: 4.0,
            quality: 60,
            format,
        };
        let avif = anim(crate::video::Format::Avif);
        let mp4 = anim(crate::video::Format::Mp4);
        assert_eq!(avif.extension(), "avif");
        assert_eq!(mp4.extension(), "mp4");
        assert_eq!(JobKind::Still { width: 8, height: 8 }.extension(), "png");
        // H.264 is the cheap one; quoting AV1's figure for it was the bug the
        // per-codec constant exists to prevent.
        let eight = crate::offline::BitDepth::Eight;
        assert!(mp4.secs_per_pixel(eight) < avif.secs_per_pixel(eight));
        // Video is 8-bit by codec, so the depth must not move an animation's
        // cost — only a still's.
        assert_eq!(avif.secs_per_pixel(crate::offline::BitDepth::Sixteen), avif.secs_per_pixel(eight));
        let still = JobKind::Still { width: 8, height: 8 };
        assert!(still.secs_per_pixel(crate::offline::BitDepth::Sixteen) > still.secs_per_pixel(eight));
        assert!(avif.label().contains("AVIF") && mp4.label().contains("MP4"));
    }

    #[test]
    fn durations_read_as_durations() {
        assert_eq!(format_duration(9.0), "9s");
        assert_eq!(format_duration(75.0), "1m 15s");
        assert_eq!(format_duration(3800.0), "1h 03m");
        assert_eq!(format_estimate(60.0, 90.0), "1m 00s – 1m 30s");
        assert_eq!(format_estimate(0.0, 1.0), "unknown");
    }

    #[test]
    fn byte_sizes_read_as_byte_sizes() {
        assert_eq!(human_bytes(1024), "1 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5 MiB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.00 GiB");
    }
}
