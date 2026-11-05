//! First-run point count: climb until the frame rate falters, then step back.
//!
//! A fresh install has no `point_count` in prefs, so until now it took the
//! scene file's, which is whatever suited the box its author had: too few on
//! a desktop card, a slideshow on a laptop. Instead the window measures this
//! machine on its first launch. It starts at [`START`], waits for the ring to
//! fill, and times a short run of frames. If they keep up with the display's
//! refresh rate it multiplies the count by [`STEP`] and tries again; at the
//! first count that falls behind, it settles on the last one that kept up.
//! That count goes into prefs like any other choice of the slider's, and the
//! person moves it from there.
//!
//! The model is a plain state machine fed one frame at a time, so the climb
//! can be tested without a GPU. `App` does the reallocating and the persisting.
//!
//! Things it rests on:
//!
//! * **A measurement is only taken on a full ring.** A reallocation restarts
//!   the fill, and while it fills the chaos pass runs at ten times its steady
//!   rate and fewer points are drawn than will be: neither is the cost being
//!   asked about. So each step waits for the fill, skips [`SETTLE_FRAMES`]
//!   (the stall of the reallocation lands in the first of them), then
//!   measures for [`MEASURE_SECS`].
//! * **"Keeps up" is the mean frame time within [`SLACK`] of the refresh
//!   period.** Under vsync a frame that misses its slot takes two, so the mean
//!   moves in whole missed frames; 4% is a touch, about one frame in
//!   twenty-five late. Without vsync the same rule asks for at least the
//!   refresh rate, which is the rate worth having.
//! * **The step is a factor of 1.5**, so the count it settles on is between
//!   two thirds of the edge and the edge. That margin is wanted: the climb
//!   measures one view of one scene, and opening panels, splat, four views or
//!   flying close (billboards) all cost more.
//! * **Coming down stops when fewer points don't help.** If the start count
//!   already falls behind, the climb steps down, but a frame rate held back by
//!   something else (a compositor, a slow present, another program on the
//!   GPU, a nested X server) barely moves as points are taken away, and
//!   following it down would leave a first run at the floor for nothing. So
//!   once two steps down (2.25 times fewer points) have not cut the frame
//!   time by [`MUST_HELP`], the start count is kept. Measured against the
//!   start, not the step before: under vsync two neighbouring counts can
//!   measure the same while points are still what costs.
//! * **It never climbs past [`CEILING`]** (512 MB of points) or the device's
//!   limit, and gives up climbing at [`DEADLINE_SECS`], keeping the last count
//!   that kept up.

use std::time::Duration;

/// Where the climb starts. Comfortable on the T490's UHD 620 at 60 Hz, which
/// holds about five million.
pub const START: u32 = 1_000_000;
/// Each step's factor, up or down.
pub const STEP: f32 = 1.5;
/// Never below this: the slider's own floor.
pub const FLOOR: u32 = 100_000;
/// Never above this, whatever the card could bind: 32M points is 512 MB, and
/// on an integrated GPU that memory is the machine's RAM.
pub const CEILING: u32 = 32_000_000;
/// Frames skipped after the ring fills, before timing starts.
pub const SETTLE_FRAMES: u32 = 8;
/// How long each count is timed for, once settled.
pub const MEASURE_SECS: f32 = 0.35;
/// ...and at least this many frames, however fast they come.
pub const MEASURE_MIN_FRAMES: u32 = 20;
/// How far over the refresh period a mean frame may run and still keep up.
pub const SLACK: f32 = 1.04;
/// Stop climbing after this long, keeping the last count that kept up.
pub const DEADLINE_SECS: f32 = 15.0;
/// Coming down, two steps must cut the frame time to this share of the
/// start's, or points aren't what is slow.
pub const MUST_HELP: f32 = 0.85;

/// What the climb wants done after a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Keep going; nothing to change.
    Wait,
    /// Reallocate the ring at this count and keep measuring.
    Try(u32),
    /// Finished: this is the count to keep.
    Done(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Waiting for the ring at `count` to fill.
    Filling,
    /// Full; skipping this many more frames before timing.
    Settling(u32),
    /// Timing: frames seen and milliseconds summed.
    Measuring { frames: u32, total_ms: f32 },
}

/// One climb. Made at startup, fed every frame until it says [`Step::Done`].
#[derive(Debug, Clone)]
pub struct Tuner {
    /// The refresh period the frames must keep up with, in milliseconds.
    period_ms: f32,
    /// The most the climb may ask for.
    ceiling: u32,
    /// The count being measured now.
    count: u32,
    /// The highest count measured to keep up, if any has.
    good: Option<u32>,
    /// Whether the climb has turned downward (the start was already too slow).
    descending: bool,
    /// The first count measured and its frame time, for judging a descent.
    first: Option<(u32, f32)>,
    phase: Phase,
    elapsed: f32,
    /// One line per measured step, for the log and for the person asking why.
    pub log: Vec<(u32, f32)>,
}

impl Tuner {
    /// A climb against a display refreshing at `refresh_hz`, on a device that
    /// can bind at most `device_max` points. Returns the tuner and the count
    /// to start at.
    pub fn new(refresh_hz: f32, device_max: u32) -> (Self, u32) {
        let hz = if refresh_hz.is_finite() && refresh_hz >= 20.0 { refresh_hz } else { 60.0 };
        let ceiling = device_max.min(CEILING).max(FLOOR);
        let count = START.clamp(FLOOR, ceiling);
        let t = Self {
            period_ms: 1000.0 / hz,
            ceiling,
            count,
            good: None,
            descending: false,
            first: None,
            phase: Phase::Filling,
            elapsed: 0.0,
            log: Vec::new(),
        };
        (t, count)
    }

    /// The refresh rate being aimed at.
    pub fn target_hz(&self) -> f32 {
        1000.0 / self.period_ms
    }

    /// Feed one frame: its duration, and whether the ring is full (not still
    /// filling after a reallocation, by this climb or anything else).
    pub fn frame(&mut self, dt: Duration, ring_full: bool) -> Step {
        let ms = dt.as_secs_f32() * 1000.0;
        self.elapsed += dt.as_secs_f32();
        if !ring_full {
            // Something rebuilt the ring (this climb, or an edit that adds a
            // transform): whatever was being timed no longer describes it.
            self.phase = Phase::Filling;
            return self.deadline_check();
        }
        match self.phase {
            Phase::Filling => {
                self.phase = Phase::Settling(SETTLE_FRAMES);
                self.deadline_check()
            }
            Phase::Settling(0) => {
                self.phase = Phase::Measuring { frames: 0, total_ms: 0.0 };
                self.deadline_check()
            }
            Phase::Settling(n) => {
                self.phase = Phase::Settling(n - 1);
                self.deadline_check()
            }
            Phase::Measuring { frames, total_ms } => {
                let frames = frames + 1;
                let total_ms = total_ms + ms;
                if frames < MEASURE_MIN_FRAMES || total_ms < MEASURE_SECS * 1000.0 {
                    self.phase = Phase::Measuring { frames, total_ms };
                    return self.deadline_check();
                }
                self.judge(total_ms / frames as f32)
            }
        }
    }

    /// A count has been timed at `mean_ms` a frame: say what comes next.
    fn judge(&mut self, mean_ms: f32) -> Step {
        self.log.push((self.count, mean_ms));
        let first = *self.first.get_or_insert((self.count, mean_ms));
        let keeps_up = mean_ms <= self.period_ms * SLACK;
        if self.descending {
            // Coming down from a start that was already too slow: the first
            // count that keeps up is the one, and the floor is the floor.
            if keeps_up || self.count <= FLOOR {
                return Step::Done(self.count);
            }
            // Two steps down and no faster: the frame rate is held back by
            // something other than points. Keep the start.
            let steps_down = (first.0 as f32 / self.count as f32).ln() / STEP.ln();
            if steps_down >= 1.9 && mean_ms > first.1 * MUST_HELP {
                return Step::Done(first.0);
            }
            return self.try_count(((self.count as f32 / STEP) as u32).max(FLOOR));
        }
        if !keeps_up {
            return match self.good {
                Some(g) => Step::Done(g),
                None => {
                    self.descending = true;
                    if self.count <= FLOOR {
                        return Step::Done(self.count);
                    }
                    self.try_count(((self.count as f32 / STEP) as u32).max(FLOOR))
                }
            };
        }
        self.good = Some(self.count);
        if self.count >= self.ceiling || self.elapsed >= DEADLINE_SECS {
            return Step::Done(self.count);
        }
        self.try_count(((self.count as f32 * STEP) as u32).min(self.ceiling))
    }

    fn try_count(&mut self, count: u32) -> Step {
        self.count = count;
        self.phase = Phase::Filling;
        Step::Try(count)
    }

    /// Out of time between measurements: keep what is known to keep up, else
    /// whatever is in hand.
    fn deadline_check(&self) -> Step {
        if self.elapsed < DEADLINE_SECS + 5.0 {
            return Step::Wait;
        }
        Step::Done(self.good.unwrap_or(self.count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a climb on a machine whose frame time is `cost(count)` ms,
    /// filling the ring in `fill` frames after every change. Returns the
    /// count it settled on and the counts it tried.
    fn climb(hz: f32, device_max: u32, fill: u32, cost: impl Fn(u32) -> f32) -> (u32, Vec<u32>) {
        let (mut t, mut count) = Tuner::new(hz, device_max);
        let mut tried = vec![count];
        let mut since_change = 0;
        for _ in 0..100_000 {
            since_change += 1;
            let ms = cost(count);
            let full = since_change > fill;
            match t.frame(Duration::from_secs_f32(ms / 1000.0), full) {
                Step::Wait => {}
                Step::Try(n) => {
                    count = n;
                    tried.push(n);
                    since_change = 0;
                }
                Step::Done(n) => return (n, tried),
            }
        }
        panic!("never finished");
    }

    /// Vsync: a frame is a whole number of periods.
    fn vsynced(hz: f32, ns_per_point: f32) -> impl Fn(u32) -> f32 {
        let period = 1000.0 / hz;
        move |n| {
            let work = n as f32 * ns_per_point * 1e-6;
            (work / period).ceil().max(1.0) * period
        }
    }

    #[test]
    fn it_settles_one_step_under_the_edge() {
        // The edge at 120 Hz is 8.33 ms / 0.6 ns = ~13.9M points.
        let (n, tried) = climb(120.0, 100_000_000, 80, vsynced(120.0, 0.6));
        assert_eq!(tried.first(), Some(&START));
        assert!(n < 13_900_000 && n as f32 > 13_900_000.0 / STEP, "settled at {n}, tried {tried:?}");
        // It went one past it, and came back.
        assert!(*tried.last().unwrap() > 13_900_000);
    }

    #[test]
    fn a_slow_machine_steps_down() {
        // Even the start misses: edge at 60 Hz with 40 ns/point is ~417k.
        let (n, tried) = climb(60.0, 100_000_000, 80, vsynced(60.0, 40.0));
        assert!(n <= 417_000 && n >= FLOOR, "settled at {n}, tried {tried:?}");
        assert!(tried.windows(2).all(|w| w[1] < w[0]));
    }

    #[test]
    fn a_frame_rate_points_dont_set_keeps_the_start() {
        // What Xephyr measured: ~24 ms of presenting whatever the count, and a
        // little for the points on top. Taking points away buys nothing.
        let (n, tried) = climb(60.0, 100_000_000, 80, |n| 24.0 + n as f32 * 6e-6);
        assert_eq!(n, START, "tried {tried:?}");
        assert!(tried.len() <= 3, "tried {tried:?}");
    }

    #[test]
    fn a_vsync_plateau_on_the_way_down_doesnt_stop_it() {
        // 40 ns a point at 60 Hz: 1M takes three periods, 667k two, 444k
        // still two (a plateau), 296k one. It must get through the plateau.
        let (n, tried) = climb(60.0, 100_000_000, 80, vsynced(60.0, 40.0));
        assert!(n <= 417_000, "settled at {n}, tried {tried:?}");
    }

    #[test]
    fn hopeless_stops_at_the_floor() {
        // Points are what is slow (200 ms for 1M, still 20 at the floor), and
        // the ring fills in two frames: it walks all the way down.
        let (n, _) = climb(60.0, 100_000_000, 2, |n| n as f32 * 2e-4);
        assert_eq!(n, FLOOR);
    }

    #[test]
    fn the_deadline_keeps_it_from_running_on() {
        // At 1M, 10 frames a second, so each fill takes seconds: the deadline stops it
        // part-way down, on a count below the one known to be too slow.
        let (n, tried) = climb(60.0, 100_000_000, 80, |n| n as f32 * 1e-4);
        assert!(n < START && n >= FLOOR, "settled at {n}, tried {tried:?}");
    }

    #[test]
    fn it_never_passes_the_ceiling_or_the_device() {
        let (n, tried) = climb(60.0, 100_000_000, 80, |_| 1.0);
        assert_eq!(n, CEILING);
        assert!(tried.iter().all(|&c| c <= CEILING));
        let (n, _) = climb(60.0, 5_000_000, 80, |_| 1.0);
        assert_eq!(n, 5_000_000);
    }

    #[test]
    fn a_fast_machine_finishes_in_about_ten_seconds() {
        // GTX 1080-ish: edge ~20M at 120 Hz. Count the seconds the climb took.
        let cost = vsynced(120.0, 0.4);
        let (mut t, mut count) = Tuner::new(120.0, 100_000_000);
        let (mut since, mut secs) = (0, 0.0f32);
        loop {
            since += 1;
            let ms = cost(count);
            secs += ms / 1000.0;
            match t.frame(Duration::from_secs_f32(ms / 1000.0), since > 80) {
                Step::Wait => {}
                Step::Try(n) => {
                    count = n;
                    since = 0;
                }
                Step::Done(_) => break,
            }
        }
        assert!(secs < 14.0, "took {secs:.1}s");
    }

    #[test]
    fn a_nonsense_refresh_rate_falls_back_to_sixty() {
        let (t, _) = Tuner::new(0.0, 1 << 30);
        assert!((t.target_hz() - 60.0).abs() < 1e-3);
    }
}
