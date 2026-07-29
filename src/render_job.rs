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

/// The same for a PNG: deflate on an already-rendered buffer, cheap enough
/// that it only shows up on very large stills.
pub const PNG_SECS_PER_PIXEL: f32 = 2.0e-8;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum JobKind {
    Still { width: u32, height: u32 },
    Animation { width: u32, height: u32, fps: u32, seconds: f32, quality: u8 },
    /// No render at all: write a view file describing the current framing, to
    /// be rendered later (or by the CLI, or on another machine).
    ViewDescriptor,
}

impl JobKind {
    pub fn extension(&self) -> &'static str {
        match self {
            JobKind::Still { .. } => "png",
            JobKind::Animation { .. } => "avif",
            JobKind::ViewDescriptor => "toml",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            JobKind::Still { .. } => "still",
            JobKind::Animation { .. } => "animation",
            JobKind::ViewDescriptor => "view descriptor",
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

#[derive(Clone, Debug)]
pub struct JobParams {
    pub kind: JobKind,
    pub out_path: PathBuf,
    /// Points in the chaos-game buffer. Job-scoped: never read from or
    /// written back to the interactive capacity or prefs.
    pub points: usize,
    /// Extra chaos-game frames after the buffer fills
    pub accumulate: u32,
    pub splat: bool,
    pub exposure: f32,
    pub transparent: bool,
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
    pub fn total_bytes(&self) -> u64 {
        let (w, h) = self.kind.size();
        let pixels = w as u64 * h as u64;
        let target = pixels * 4 * 2;
        let accum = if self.splat { pixels * 8 } else { 0 };
        self.point_buffer_bytes() + target + accum
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
    Done(Result<PathBuf, String>),
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

/// Sentinel error a cancelled job returns, so callers can tell "you stopped
/// this" apart from "this broke" and skip the failure reporting.
pub const CANCELLED: &str = "cancelled";

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

#[cfg(test)]
mod tests {
    use super::*;

    fn still(points: usize, w: u32, h: u32) -> JobParams {
        JobParams {
            kind: JobKind::Still { width: w, height: h },
            out_path: PathBuf::from("renders/x.png"),
            points,
            accumulate: 32,
            splat: false,
            exposure: 1.0,
            transparent: false,
        }
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
        };
        assert_eq!(k.frames(), 60);
        // Even a sub-frame duration renders something playable
        let tiny = JobKind::Animation {
            width: 8,
            height: 8,
            fps: 24,
            seconds: 0.01,
            quality: 60,
        };
        assert_eq!(tiny.frames(), 2);
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
