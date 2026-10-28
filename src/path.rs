//! Camera paths: quaternion Catmull-Rom splines over orbit-camera keypoints
//!
//! A path is a sequence of [`PathKey`]s — a full camera framing each — joined
//! by a spline. Framings are [`Orientation`]s, so the camera can be pointed
//! anywhere, including straight up or down, and a path can be authored through
//! framings the old yaw/pitch/roll camera could not even reach.
//!
//! # Routes, not coordinates
//!
//! A spline through orientations has to answer a question a spline through
//! numbers doesn't: *which way round*. Two framings 1° apart are also 359°
//! apart the other way, and both are real journeys someone might want.
//!
//! So this module never derives that from the endpoints. Each segment carries
//! an explicit [`Route`], normally a winding: `0` is the short way, `1` takes
//! an extra whole turn, `-1` goes the long way against it. The endpoints are
//! always hit exactly, whatever the winding says — a route can be surprising,
//! but it can never land in the wrong place. See [`crate::rot`] for why an
//! integer is enough to pin a route down, and [`Route::Exact`] for the one
//! case where it isn't: between two *equal* framings no axis is implied, so a
//! loop in place has to store the displacement itself and be checked on load.
//!
//! # The spline
//!
//! Uniform Catmull-Rom in *cumulative* form (Kim–Kim–Shin):
//!
//! ```text
//! q(u) = q₀ · exp(ω₁·B̃₁(u)) · exp(ω₂·B̃₂(u)) · exp(ω₃·B̃₃(u))
//! ```
//!
//! where `ωⱼ` is the turn from key `j-1` to key `j` and `B̃ⱼ = Σ_{m≥j} Bₘ` are
//! the running sums of the ordinary Catmull-Rom weights. C¹, interpolating,
//! and it has two properties this engine leans on:
//!
//! - **When the segment turns share an axis it is exactly the old scalar yaw
//!   spline.** That covers the default turntable and every constant-pitch
//!   path, which is most scenes — they did not move at all.
//! - **A one-key zoom loop comes out exactly `rot^(1+u)`,** the continuous
//!   similarity flow itself. `B̃₁+B̃₂+B̃₃ = 1+u` identically, so equal segment
//!   turns give a constant-rate sweep with no seam and no velocity kink.
//!
//! Squad, and splining quaternion components with a hemisphere fix, were both
//! rejected: both are built on slerp between control points, and slerp *is*
//! the take-the-short-way operator, so neither can represent a segment longer
//! than half a turn. A corkscrew is not expressible in them at all.
//!
//! Other conventions:
//! - `distance` interpolates in log space, so zooms run at a constant
//!   *relative* rate (halving the distance always takes the same time).
//! - `focus` travels on its own spline, so look directions blend smoothly
//!   while the eye moves.
//! - Easing (smoothstep on path time) defaults to whatever the [`Loop`] wants:
//!   on for a one-shot and for a ping-pong, whose turnarounds it is what makes
//!   smooth; off for the two closing loops, where it would stall the seam.

use glam::Vec3;

use crate::camera::{Fov, OrbitCamera};
use crate::rot::{Orientation, Turn};

/// Angular speed of the default orbit, in rad/s — a full turn every ~35s.
///
/// This was `yaw += 0.18 * dt` applied straight to the camera, back when the
/// turntable was its own mechanism. It survives only as [`CameraPath::
/// full_orbit`]'s duration, which is the one place the number still means
/// anything.
pub const ORBIT_RATE: f32 = 0.18;

/// One spline keypoint: a full camera framing
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathKey {
    pub orientation: Orientation,
    pub distance: f32,
    pub focus: Vec3,
}

impl PathKey {
    pub fn from_camera(cam: &OrbitCamera) -> Self {
        Self {
            orientation: cam.orientation,
            distance: cam.distance,
            focus: cam.focus,
        }
    }

    /// The framing as a camera, under the lens `fov`.
    ///
    /// A keypoint has no FoV of its own — it is a place to stand, not a lens —
    /// so whoever is flying the path has to say which one it is being flown
    /// with. An argument rather than a default so that cannot be forgotten:
    /// silently defaulting here would make a flight ignore the scene's (and
    /// the person's) FoV without ever failing.
    pub fn to_camera(self, fov: Fov) -> OrbitCamera {
        OrbitCamera {
            orientation: self.orientation,
            distance: self.distance,
            focus: self.focus,
            fov,
        }
    }
}

/// The similarity a zoom-looping path closes under: `Aᴺ` for a path that
/// descends `periods` zoom periods per loop.
///
/// A path normally loops by returning to where it started. Under infinite zoom
/// it doesn't have to, because *the scene has a symmetry* — scaling by `s`
/// about the fixed point and turning by the map's rotation leaves the rendered
/// set unchanged (see `renorm.rs`). So a path whose last key is the first key
/// carried forward by that symmetry ends on a frame **identical** to the one it
/// started on, having descended a period. Played on a loop, that is an endless
/// zoom: not an approximation of one, the thing itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomLoop {
    /// Zoom periods descended per loop, as authored
    pub periods: u32,
    /// The similarity, `Aᵖᵉʳⁱᵒᵈˢ`. Resolved from the scene's renormalizing map
    /// and refreshed whenever that map is edited, so it can't go stale.
    pub center: Vec3,
    pub scale: f32,
    /// The rotation, as a displacement rather than a quaternion.
    ///
    /// This is load-bearing. Four periods of a map that twists 47° is a 188°
    /// sweep; stored as a quaternion and read back, it becomes 172° *the other
    /// way* and the loop flies backwards. A [`Turn`] is unbounded, so `n`
    /// periods is just `twist · n` and there is no branch to get wrong.
    pub turn: Turn,
}

impl ZoomLoop {
    /// Carry a keypoint `n` loops forward (negative = backward).
    ///
    /// Powers of a similarity are closed-form, so a key twenty loops out costs
    /// the same as one loop out.
    pub fn advance(&self, key: PathKey, n: i32) -> PathKey {
        let scale = self.scale.powi(n);
        let rot = (self.turn * n as f32).exp();
        PathKey {
            orientation: key.orientation.then(rot),
            distance: key.distance * scale,
            focus: self.center + rot.rotate((key.focus - self.center) * scale),
        }
    }
}

/// How a path gets from its last frame back to its first — the whole of it,
/// in one field.
///
/// There are four ways and a path is always on exactly one of them, which is
/// why this is an enum and not the pair of flags it replaced (`closed: bool`
/// plus `zoom_loop: Option<_>`). Two flags describe four states, but one of
/// those four is "returns to the first key *and* descends a zoom period",
/// which is a contradiction — two different loops claimed at once. Every
/// writer had to remember to clear the other one, and the load path, the
/// keybind and the panel each did it separately. Now it can't be said.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Loop {
    /// Play from the first key to the last and stop.
    Once,
    /// Play to the last key, then back along the path to the first.
    ///
    /// The geometry is the open path; the loop lives entirely in the time
    /// mapping, which folds `t` into a triangle. That's what makes it work on
    /// a path whose ends are nowhere near each other — the return journey is
    /// the outward one reversed, so there is nothing to close.
    PingPong,
    /// Loop back to the first key after the last, seamlessly.
    Closed,
    /// Close under the scene's zoom symmetry instead of by returning to the
    /// first key: one loop descends whole zoom periods and lands on an
    /// identical frame. See [`ZoomLoop`].
    Zoom(ZoomLoop),
}

/// A loop named without the similarity a zoom loop resolves to.
///
/// This is what a radio button, a keybind or a scene file picks from;
/// [`Loop`] is what the spline flies. The two are separate because a zoom
/// loop's `ZoomLoop` is *derived* — from the scene's renormalizing map, and
/// re-derived whenever that map is edited — so it can't be chosen, only
/// resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopKind {
    Once,
    PingPong,
    Closed,
    Zoom,
}

impl LoopKind {
    /// A phrase for logs, `--info` and the CLI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Once => "plays once",
            Self::PingPong => "ping-pong loop",
            Self::Closed => "closed loop",
            Self::Zoom => "zoom loop",
        }
    }
}

impl Loop {
    pub fn kind(self) -> LoopKind {
        match self {
            Self::Once => LoopKind::Once,
            Self::PingPong => LoopKind::PingPong,
            Self::Closed => LoopKind::Closed,
            Self::Zoom(_) => LoopKind::Zoom,
        }
    }

    /// Whether playback repeats: the last frame runs into the first.
    ///
    /// True for all three loops, because everything that cares about *time* —
    /// `t` wrapping, dropping the duplicate final frame of a render, not
    /// easing into a seam — wants the same answer for all of them.
    pub fn wraps(self) -> bool {
        !matches!(self, Self::Once)
    }

    /// Whether the *spline* gains a closing segment.
    ///
    /// Not the same question as [`wraps`](Self::wraps), and conflating the two
    /// is the bug ping-pong would otherwise have: it loops in time while its
    /// geometry stays an open path, so it wraps but does not close.
    pub fn closes(self) -> bool {
        matches!(self, Self::Closed | Self::Zoom(_))
    }

    /// Traversals of the key list per loop — two for a there-and-back.
    ///
    /// Only the default duration reads this, so switching a path to ping-pong
    /// keeps its *speed* and doubles its length, rather than flying the same
    /// route twice as fast.
    pub fn passes(self) -> f32 {
        match self {
            Self::PingPong => 2.0,
            _ => 1.0,
        }
    }

    /// Whether to smoothstep path time when the scene doesn't say.
    ///
    /// A closed loop must not, or the seam gets a visible stall. A ping-pong
    /// very much must: the ease is what takes the camera's velocity to zero at
    /// each turnaround, and without it the reversal is a hard bounce.
    pub fn eases_by_default(self) -> bool {
        matches!(self, Self::Once | Self::PingPong)
    }

    pub fn zoom(self) -> Option<ZoomLoop> {
        match self {
            Self::Zoom(z) => Some(z),
            _ => None,
        }
    }

    /// The same loop as every other track names it: `once` holds, `closed`
    /// repeats, `pingpong` ping-pongs, and a zoom loop is a repeat *with
    /// offset* whose offset is the zoom symmetry. See `crate::track`.
    pub fn extrapolate(self) -> crate::track::Extrapolate {
        use crate::track::Extrapolate;
        match self {
            Self::Once => Extrapolate::Hold,
            Self::PingPong => Extrapolate::PingPong,
            Self::Closed => Extrapolate::Repeat,
            Self::Zoom(_) => Extrapolate::Offset,
        }
    }
}

/// How a segment's route between its two keys is named.
///
/// A winding integer is the ordinary spelling and the safe one. When the
/// endpoints differ, the axis of the journey between them is *forced* — see
/// the log-set argument in [`crate::rot`] — so a single integer pins the route
/// down completely, and it cannot disagree with the keys it connects. That
/// covers everything anyone writes by hand.
///
/// [`Exact`] is for the case that argument excludes. Between *equal*
/// orientations no axis is forced: every axis has a whole-turn route back to
/// where it started, so "pitch three full turns and come back" cannot be said
/// with a winding at all — [`Orientation::turn_to`] has nothing to go on and
/// falls back to guessing world Y, which flies a yaw loop instead. A corkscrew
/// about an axis the endpoints don't imply is unsayable for the same reason.
/// Storing the displacement itself is the only way to say either.
///
/// The price is exactly the one the integer was chosen to avoid: a stored
/// displacement *can* disagree with its endpoints. The loader pays it once, up
/// front — a scene file's `route` is checked against the far key as it is read
/// and refused if it misses (`scene.rs`). Nothing else constructs one, and
/// there is no UI for it: it is a file-format door, opened because some routes
/// have no other spelling.
///
/// [`Exact`]: Route::Exact
/// [`Orientation::turn_to`]: crate::rot::Orientation::turn_to
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Route {
    /// Extra whole turns beyond the short way round, about the short way's own
    /// axis. `Turns(0)` is the short way, and the default for every segment.
    Turns(i32),
    /// The journey itself, in the frame of the key it starts from.
    Exact(Turn),
}

impl Default for Route {
    /// The short way round: what a segment does when nobody has said otherwise.
    fn default() -> Self {
        Self::Turns(0)
    }
}

/// A spline camera path through two or more keypoints
#[derive(Clone, Debug)]
pub struct CameraPath {
    pub keys: Vec<PathKey>,
    /// The route each segment takes between its keys — the short way round
    /// unless something says otherwise.
    ///
    /// One entry per segment (see [`CameraPath::segments`]); segment `i` runs
    /// from key `i` to key `i+1`, wrapping on a closed path. Missing or short
    /// entries read as [`Route::Turns(0)`], so a path can always be built
    /// without thinking about it and a hand-written scene never has to mention
    /// it.
    ///
    /// [`Route::Turns(0)`]: Route::Turns
    pub routes: Vec<Route>,
    /// How playback gets from the last frame back to the first.
    pub loops: Loop,
    /// Ease in/out (smoothstep on the lap's time); None = the loop's own
    /// default
    pub ease: Option<bool>,
    /// When the camera passes each key, in seconds: one per key, rising.
    ///
    /// A key is a view at a moment, as every other track's key is a value at
    /// a moment. Empty (or the wrong length, mid-edit) means the keys are
    /// spaced [`KEY_SPACING`] apart from 0.
    pub times: Vec<f32>,
    /// A looping path's lap, in seconds: from the first key round to
    /// arriving back at it (`Closed`), or at its image under the zoom
    /// symmetry (`Zoom`). The closing segment is whatever is left after the
    /// last key. None = one more [`KEY_SPACING`] after the last key. Unused by
    /// `Once` and `PingPong`, whose lap is their keys' span.
    pub period: Option<f32>,
}

/// Seconds between keys where nobody said: what three seconds a segment
/// always meant.
pub const KEY_SPACING: f32 = 3.0;

/// Which path to fly: the scene's own keypoints when there are enough of them
/// to interpolate, otherwise the default.
///
/// The one rule, in one place. Both the app (`App::camera_path`) and the
/// offline animation renderer go through here, so "what does this scene's
/// camera do" has a single answer — what you watch in the window is what
/// `--render x.avif` writes.
///
/// The threshold is two keys because a spline needs two ends. One key is a
/// scene mid-authoring: it's stored and saved, but the default still flies
/// until it has company.
pub fn resolve<'a>(authored: Option<&'a CameraPath>, default: &'a CameraPath) -> &'a CameraPath {
    match authored {
        Some(p) if p.playable() => p,
        _ => default,
    }
}

/// The running sums `B̃ⱼ = Σ_{m≥j} Bₘ` of the Catmull-Rom weights, for j = 1,2,3.
///
/// `B̃` rather than `B` because the spline is built from *displacements*
/// between keys rather than from the keys themselves — which is the only form
/// that can hold a turn longer than half a rotation.
///
/// Two identities worth knowing, both of which are pinned by tests:
/// `B̃(0) = (1,0,0)` and `B̃(1) = (1,1,0)`, so the curve passes through its
/// keys; and `B̃₁+B̃₂+B̃₃ = 1+u` exactly, so equal displacements give a
/// constant-rate sweep.
#[cfg(test)]
/// Uniform Catmull-Rom between p1 and p2 with neighbours p0, p3: the scalar
/// spline the cumulative form must reproduce.
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

#[cfg(test)]
fn cumulative_basis(u: f32) -> [f32; 3] {
    timed_basis(u, 1.0, 1.0)
}

/// The same running sums for keys that are *not* evenly spaced in time.
///
/// Catmull-Rom is a Hermite spline whose tangent at a key is the mean of the
/// velocities either side of it. With times, those are velocities per second
/// — `½(d₀/D₀ + d₁/D₁)` — and in the segment's own parameter (length `D₁`) the
/// neighbours' displacements are weighted by `r₀ = D₁/D₀` and `r₂ = D₁/D₂`:
///
/// ```text
/// B̃₁ = 1 + ½·h₁₀·r₀    B̃₂ = ½·h₁₀ + h₀₁ + ½·h₁₁    B̃₃ = ½·h₁₁·r₂
/// ```
///
/// At `r₀ = r₂ = 1` this is exactly [`cumulative_basis`], so an evenly timed
/// path flies the uniform spline it always did. Uneven times keep the camera's
/// *speed* continuous through a key rather than its parameter velocity: a key
/// moved later slows the run into it instead of putting a kink in it. The
/// endpoint identities hold for any ratios (`h₁₀` and `h₁₁` vanish at both
/// ends), so keys are still hit exactly.
fn timed_basis(u: f32, r0: f32, r2: f32) -> [f32; 3] {
    let (u2, u3) = (u * u, u * u * u);
    let h10 = u3 - 2.0 * u2 + u;
    let h01 = -2.0 * u3 + 3.0 * u2;
    let h11 = u3 - u2;
    [1.0 + 0.5 * h10 * r0, 0.5 * h10 + h01 + 0.5 * h11, 0.5 * h11 * r2]
}

impl CameraPath {
    /// A path through `keys`, taking the short way round on every segment,
    /// [`KEY_SPACING`] apart.
    pub fn new(keys: Vec<PathKey>, loops: Loop) -> Self {
        Self {
            keys,
            routes: Vec::new(),
            loops,
            ease: None,
            times: Vec::new(),
            period: None,
        }
    }

    /// When the camera passes key `i`, in seconds.
    pub fn time(&self, i: usize) -> f32 {
        if self.times.len() == self.keys.len() {
            self.times[i]
        } else {
            KEY_SPACING * i as f32
        }
    }

    /// Every key's time, spelled out: what a file or the Timeline shows.
    pub fn key_times(&self) -> Vec<f32> {
        (0..self.keys.len()).map(|i| self.time(i)).collect()
    }

    /// The first key's time: where a lap starts.
    pub fn start(&self) -> f32 {
        if self.keys.is_empty() { 0.0 } else { self.time(0) }
    }

    /// One lap, in seconds: the keys' span for a one-shot or one way of a
    /// ping-pong; the period for the two closing loops.
    pub fn lap(&self) -> f32 {
        let n = self.keys.len();
        let span = if n == 0 { 0.0 } else { self.time(n - 1) - self.time(0) };
        if self.loops.closes() {
            self.period.unwrap_or(span + KEY_SPACING).max(span + 1e-3)
        } else {
            span
        }
        .max(0.1)
    }

    /// Scale every key's time (and the period) so that playback takes
    /// `seconds`, keeping the first key where it is: the Camera window's
    /// duration field.
    pub fn retime_to(&mut self, seconds: f32) {
        let k = seconds.max(0.1) / self.duration();
        let t0 = self.start();
        let times: Vec<f32> = self.key_times().iter().map(|t| t0 + (t - t0) * k).collect();
        if self.loops.closes() {
            self.period = Some(self.lap() * k);
        }
        self.times = times;
    }

    /// The seconds each segment lasts, `i` wrapping like [`segment_turn`].
    /// Past the ends of a path that doesn't close there is no segment; those
    /// borrow the nearest real one's length, which only ever multiplies a
    /// zero displacement.
    ///
    /// [`segment_turn`]: Self::segment_turn
    fn segment_seconds(&self, i: isize) -> f32 {
        let n = self.keys.len();
        let segs = self.segments() as isize;
        if segs == 0 {
            return 1.0;
        }
        let i = if self.loops.closes() { i.rem_euclid(segs) } else { i.clamp(0, segs - 1) } as usize;
        let d = if i + 1 < n {
            self.time(i + 1) - self.time(i)
        } else {
            // The closing segment: what the lap leaves after the last key.
            self.start() + self.lap() - self.time(n - 1)
        };
        d.max(1e-4)
    }

    /// Whether playback loops: the last frame runs into the first.
    pub fn wraps(&self) -> bool {
        self.loops.wraps()
    }

    /// Whether there is enough here to interpolate.
    ///
    /// Two keys, normally — a spline needs two ends, and one key is a scene
    /// mid-authoring. A zoom loop is the exception and needs only one: its
    /// closing segment runs to that key's own image under the symmetry, which
    /// is a real segment through real geometry.
    pub fn playable(&self) -> bool {
        self.keys.len() >= 2 || (self.loops.zoom().is_some() && !self.keys.is_empty())
    }

    /// Number of spline segments (closing paths add the wrap-around segment).
    ///
    /// Keyed on `closes()`, not `wraps()`: a ping-pong loops without closing —
    /// it flies these same segments back the other way, so it has exactly the
    /// segments an open path has.
    ///
    /// A zoom loop is the one path that can have a single key and still be a
    /// path: its closing segment runs from that key to the key's own image
    /// under the symmetry, which is a real segment through real geometry.
    pub fn segments(&self) -> usize {
        match (self.keys.len(), self.loops.closes()) {
            (0, _) => 0,
            (1, true) => 1,
            (1, false) => 0,
            (n, true) => n,
            (n, false) => n - 1,
        }
    }

    /// Playback duration: one lap — twice over for a ping-pong, which flies
    /// its keys out and back.
    pub fn duration(&self) -> f32 {
        self.lap() * self.loops.passes()
    }

    fn eased(&self) -> bool {
        self.ease.unwrap_or_else(|| self.loops.eases_by_default())
    }

    /// The route on segment `i`, or the short way where nothing was authored.
    pub fn route(&self, i: usize) -> Route {
        self.routes.get(i).copied().unwrap_or_default()
    }

    /// Extra whole turns on segment `i`, as the scene file's `turns` column
    /// spells it.
    ///
    /// An exact route is not a winding and reports `0` here: it is written to
    /// disk as `route` instead, and the yaw column is left saying only what the
    /// chart can honestly say about it.
    pub fn winding(&self, i: usize) -> i32 {
        match self.route(i) {
            Route::Turns(w) => w,
            Route::Exact(_) => 0,
        }
    }

    /// Grow or shrink `routes` to match the segment count, so the two never
    /// drift apart as keys are added and removed.
    pub fn fit_routes(&mut self) {
        self.routes.resize(self.segments(), Route::default());
    }

    /// Make `times` say every key's time explicitly, so a key can be added,
    /// removed or retimed without the others moving.
    pub fn spell_times(&mut self) {
        if self.times.len() != self.keys.len() {
            self.times = (0..self.keys.len()).map(|i| KEY_SPACING * i as f32).collect();
        }
    }

    /// Put a key at `t` seconds, keeping the keys in time order: replaces the
    /// framing of a key already there, otherwise inserts one. Returns its
    /// index. The route on each segment it touches goes back to the short way,
    /// since the journey it named no longer exists.
    ///
    /// On a looping path the period stretches to keep the closing segment at
    /// least as long as it was, when a key lands past the old last one.
    pub fn key_at(&mut self, t: f32, key: PathKey) -> usize {
        self.spell_times();
        if let Some(i) = self.times.iter().position(|x| (x - t).abs() < 1e-3) {
            self.keys[i] = key;
            return i;
        }
        let closing = if self.keys.is_empty() { KEY_SPACING } else { self.segment_seconds(self.segments() as isize - 1) };
        let was_last = self.times.last().copied();
        let i = self.times.iter().position(|x| *x > t).unwrap_or(self.times.len());
        self.keys.insert(i, key);
        self.times.insert(i, t);
        if i < self.routes.len() {
            self.routes[i] = Route::default();
            self.routes.insert(i, Route::default());
        }
        self.fit_routes();
        if self.loops.closes() && was_last.is_some_and(|l| t > l) {
            let start = self.start();
            if self.period.is_some_and(|p| start + p < t + closing) || self.period.is_none() {
                self.period = Some(t + closing - start);
            }
        }
        i
    }

    /// Where a key appended after the last one goes: [`KEY_SPACING`] later, or
    /// 0 for the first. On a loop it goes where the lap would have closed, so
    /// the closing segment keeps its length after it.
    pub fn times_or_spacing_after_last(&self) -> f32 {
        match self.keys.len() {
            0 => 0.0,
            n if self.loops.closes() => self.start() + self.lap().max(self.time(n - 1) - self.start() + KEY_SPACING),
            n => self.time(n - 1) + KEY_SPACING,
        }
    }

    /// Take key `i` away, keeping every other key's time.
    pub fn remove_key(&mut self, i: usize) {
        if i >= self.keys.len() {
            return;
        }
        self.spell_times();
        self.keys.remove(i);
        self.times.remove(i);
        if i < self.routes.len() {
            self.routes.remove(i);
        }
        if i > 0 && i - 1 < self.routes.len() {
            self.routes[i - 1] = Route::default();
        }
        self.fit_routes();
    }

    /// Move key `i` to `t`, clamped strictly between its neighbours so the
    /// keys stay in order (a retime never reorders: that would change which
    /// journey each route names). On a loop the last key also stays before
    /// the lap's end.
    pub fn set_key_time(&mut self, i: usize, t: f32) {
        if i >= self.keys.len() {
            return;
        }
        self.spell_times();
        const GAP: f32 = 0.01;
        let lo = if i > 0 { self.times[i - 1] + GAP } else { f32::NEG_INFINITY };
        let hi = if i + 1 < self.times.len() {
            self.times[i + 1] - GAP
        } else if self.loops.closes() {
            self.start() + self.lap() - GAP
        } else {
            f32::INFINITY
        };
        // Moving the first key moves where the lap starts; keep the lap's end.
        let end = self.start() + self.lap();
        self.times[i] = t.clamp(lo, hi.max(lo));
        if i == 0 && self.loops.closes() && self.period.is_some() {
            self.period = Some(end - self.times[0]);
        }
    }

    /// A seamless full-turn orbit at the given base framing.
    ///
    /// This is *the* path for a scene that authors none — the same object in
    /// the app (where it's the turntable you watch), in the viewport (where
    /// it's drawn like any other path), and offline (where `--render x.avif`
    /// flies it). There's no second turntable system beside the path system;
    /// there's one path system with this as its default.
    ///
    /// Four quarter-turn keys rather than one key and a winding, because four
    /// short segments about a shared axis are exactly the old scalar yaw
    /// spline — the turntable is bit-for-bit what it always was.
    pub fn full_orbit(base: &OrbitCamera) -> Self {
        let tau = std::f32::consts::TAU;
        let keys = (0..4)
            .map(|i| PathKey {
                orientation: base
                    .orientation
                    .then_world(Turn::about(Vec3::Y, i as f32 * tau / 4.0)),
                ..PathKey::from_camera(base)
            })
            .collect();
        let period = tau / ORBIT_RATE;
        Self {
            keys,
            routes: vec![Route::default(); 4],
            loops: Loop::Closed,
            ease: Some(false),
            times: (0..4).map(|i| i as f32 * period / 4.0).collect(),
            period: Some(period),
        }
    }

    /// Key for spline index i, where i ranges over -1..=segments()+1.
    /// Open paths clamp at the ends; closed paths wrap; a zoom loop's
    /// out-of-range keys are the in-range ones carried by the symmetry.
    fn key(&self, i: isize) -> PathKey {
        let n = self.keys.len() as isize;
        let idx = i.rem_euclid(n) as usize;
        let turns = ((i - idx as isize) / n) as i32; // whole loops i is offset by

        match self.loops {
            // Carrying by the symmetry is what makes the spline periodic *in
            // appearance* rather than in parameter space. It also means no
            // clamping at the ends: the seam gets the same treatment as any
            // interior segment, so the loop has no velocity kink either.
            Loop::Zoom(z) => z.advance(self.keys[idx], turns),
            Loop::Closed => self.keys[idx],
            // A ping-pong's geometry is the open path — it turns round at the
            // ends rather than continuing past them, so the neighbours out
            // there are clamped copies just as they are for a one-shot.
            Loop::Once | Loop::PingPong => self.keys[i.clamp(0, n - 1) as usize],
        }
    }

    /// The turn along segment `i`, in the frame of the key it starts from.
    ///
    /// This is where the route lives. Everything else about the spline is
    /// arithmetic on these.
    fn segment_turn(&self, i: isize) -> Turn {
        let segs = self.segments() as isize;
        if segs == 0 {
            return Turn::ZERO;
        }

        // A one-key zoom loop's only segment is the symmetry itself, seen from
        // the key. Conjugating preserves the magnitude exactly, so a
        // multi-period loop keeps its whole sweep and cannot fold. Only the
        // one-key case may take this shortcut: with authored keys in between,
        // the segments are ordinary key-to-key journeys (the last one to the
        // first key's image under the symmetry, which `key()` already hands
        // out), and pretending each was the whole symmetry skipped every
        // authored framing and overswept the loop by the key count. A
        // multi-key zoom takes each segment the short way plus its authored
        // winding, like any other path — so spread the keys enough that no
        // segment sweeps more than half a turn, or say `turns` on the one
        // that does.
        if let Loop::Zoom(z) = self.loops {
            if self.keys.len() == 1 {
                let from = self.key(i);
                return Turn::from_rotation_vector(
                    from.orientation.inverse().rotate(z.turn.as_rotation_vector()),
                );
            }
        }

        // Past the ends of a path that doesn't close, the neighbour keys are
        // clamped copies, so there is nowhere to go.
        if !self.loops.closes() && (i < 0 || i >= segs) {
            return Turn::ZERO;
        }

        let from = self.key(i);
        let to = self.key(i + 1);
        match self.route(i.rem_euclid(segs) as usize) {
            Route::Turns(w) => from.orientation.turn_to(to.orientation, w),
            // Used exactly as authored — the loader has already checked that
            // it lands on `to`. Body-frame displacements are invariant under
            // the zoom symmetry (`advance` composes on the left), so the same
            // stored route is right on the closing segment too.
            Route::Exact(t) => t,
        }
    }

    /// Sample the path at t in [0, 1] (clamped; closed paths wrap seamlessly),
    /// under the lens `fov` — see [`PathKey::to_camera`] for why that is not
    /// the path's to choose.
    pub fn sample(&self, t: f32, fov: Fov) -> OrbitCamera {
        let segs = self.segments();
        if segs == 0 {
            return self
                .keys
                .first()
                .copied()
                .unwrap_or(PathKey {
                    orientation: Orientation::IDENTITY,
                    distance: 3.0,
                    focus: Vec3::ZERO,
                })
                .to_camera(Fov::default());
        }

        // Folded by the loop's extrapolation, the rule every track folds its
        // time by (`Extrapolate::fold`): held for a one-shot, wrapped for the
        // loops. Spelled here in f32, as it always was, so every flight stays
        // the same to the bit; `the_camera_folds_time_as_tracks_do` holds the
        // two spellings together. Orientation is genuinely periodic, so a
        // looping path needs no accumulation across whole loops: the second
        // pass *is* the first, and a zoom loop's offset — the symmetry — is
        // carried by the wrap, not here.
        //
        // A ping-pong is a triangle wave in path time: out over the first
        // half, back over the second. This is the whole of it — the spline
        // below never learns that the camera is coming home, and doesn't need
        // to, because the return journey is the outward one read backwards.
        use crate::track::Extrapolate;
        let t = match self.loops.extrapolate() {
            Extrapolate::Hold => t.clamp(0.0, 1.0),
            Extrapolate::Repeat | Extrapolate::Offset => t.rem_euclid(1.0),
            Extrapolate::PingPong => 1.0 - (2.0 * t.rem_euclid(1.0) - 1.0).abs(),
        };
        // The lap's own time, then which segment it falls in. The fold above
        // works in fractions so the ping-pong triangle and the ease stay what
        // they always were; this turns the fraction into seconds along the
        // keys, where each segment is as long as its keys say.
        let tau = self.start() + t * self.lap();
        let mut seg: isize = 0;
        while (seg as usize) + 1 < segs && tau >= self.segment_end(seg as usize) {
            seg += 1;
        }
        let seg_start = self.time(seg as usize);
        let d1 = self.segment_seconds(seg);
        let u = ((tau - seg_start) / d1).clamp(0.0, 1.0);

        let (k0, k1, k2, k3) = (
            self.key(seg - 1),
            self.key(seg),
            self.key(seg + 1),
            self.key(seg + 2),
        );

        // The cumulative form: start at the key *before* the segment and walk
        // three weighted displacements. At u=0 the weights are (1,0,0), which
        // lands exactly on k1; at u=1 they are (1,1,0), which lands exactly on
        // k2. Keys are hit whatever the windings — or the key times — say.
        //
        // The neighbour turns are split before weighting. B̃₁ overshoots 1
        // (peak 1.074) and B̃₃ undershoots 0 (trough −0.074) — that is what
        // rounds corners, and on a short turn the few-percent excursion is
        // invisible. But a neighbour carrying a winding is a displacement of
        // whole turns, and 7% of three turns is ~80° of rotation injected
        // mid-segment about the neighbour's axis and removed again by the
        // end: a yaw segment next to a pitch corkscrew visibly *rolled*. The
        // whole-turn part of a displacement is invisible at the endpoints, so
        // it takes no part in corner-rounding: it is pinned at its endpoint
        // weight — all of the previous segment's, none of the next's — and
        // only the sub-π principal parts are smoothed. Both parts share the
        // segment's axis, so each pair still composes as one turn.
        let mut b = timed_basis(
            u,
            d1 / self.segment_seconds(seg - 1),
            d1 / self.segment_seconds(seg + 1),
        );
        // Easing is coming to rest at the ends: the tangent at the first and
        // last key is zero instead of half the segment's own displacement
        // (which is what a clamped neighbour leaves it). So the camera starts
        // from still, settles into the last key, and a ping-pong turns round
        // smoothly — and every key is still passed at its own time, which a
        // smoothstep on the whole lap (what easing used to be) could not say.
        if self.eased() && !self.loops.closes() {
            let (u2, u3) = (u * u, u * u * u);
            if seg == 0 {
                b[1] -= 0.5 * (u3 - 2.0 * u2 + u);
            }
            if seg as usize + 1 == segs {
                b[1] -= 0.5 * (u3 - u2);
            }
        }
        let prev = self.segment_turn(seg - 1);
        let next = self.segment_turn(seg + 1);
        let orientation = k0
            .orientation
            .then_body(prev.principal() * b[0] + (prev - prev.principal()))
            .then_body(self.segment_turn(seg) * b[1])
            .then_body(next.principal() * b[2]);

        // The scalar channels on the same weights, as displacements from k0.
        let cr = |f: fn(&PathKey) -> f32| {
            let (p0, p1, p2, p3) = (f(&k0), f(&k1), f(&k2), f(&k3));
            p0 + b[0] * (p1 - p0) + b[1] * (p2 - p1) + b[2] * (p3 - p2)
        };
        OrbitCamera {
            orientation,
            distance: cr(|k| k.distance.max(1e-3).ln()).exp(),
            focus: Vec3::new(cr(|k| k.focus.x), cr(|k| k.focus.y), cr(|k| k.focus.z)),
            fov,
        }
    }

    /// When segment `i` ends, in seconds from the lap's start time.
    fn segment_end(&self, i: usize) -> f32 {
        if i + 1 < self.keys.len() {
            self.time(i + 1)
        } else {
            self.start() + self.lap()
        }
    }

    /// The path at `seconds` on the timeline (the first key's time is where
    /// a lap starts). See [`sample`](Self::sample) for the fraction form.
    pub fn sample_at(&self, seconds: f64, fov: Fov) -> OrbitCamera {
        let start = self.start() as f64;
        self.sample(((seconds - start) / self.duration() as f64) as f32, fov)
    }
}

/// Golden flights: every scene in `scenes/`, sampled along the path it
/// actually flies, pinned to a checked-in file.
///
/// The rest of this module's tests pin *properties* — that keys are hit, that
/// the seam is smooth, that a zoom loop lands where it started. Those survive
/// a change of representation by construction, which is exactly why they can't
/// catch one moving a scene. This can: it is the literal eye/forward/up of
/// every shipped scene at 32 points around its flight, and it fails loudly
/// with the magnitude of the drift.
///
/// Regenerate deliberately, never to make a red test green:
///     UPDATE_GOLDEN=1 cargo test golden_flights_are_unchanged
///
/// Reading files from `scenes/` departs from this repo's habit of parsing
/// inline TOML in tests (see `scene::tests`). That's the point — the artifacts
/// being protected are the ones on disk.
#[cfg(test)]
mod fold_tests {
    use super::*;

    /// The camera folds its 0..1 path time in f32, by hand, to stay bit-exact
    /// with every flight rendered before tracks existed; tracks fold seconds
    /// in f64 with `Extrapolate::fold`. They must be the same rule.
    #[test]
    fn the_camera_folds_time_as_tracks_do() {
        let cam = PathKey {
            orientation: Orientation::from_yaw_pitch_roll(
                crate::rot::Angle::from_radians(0.3),
                crate::rot::Angle::from_radians(0.4),
                crate::rot::Angle::ZERO,
            ),
            distance: 3.0,
            focus: Vec3::new(0.1, -0.2, 0.3),
        }
        .to_camera(Fov::default());
        let orbit = CameraPath::full_orbit(&cam);
        for loops in [Loop::Once, Loop::PingPong, Loop::Closed] {
            let path = CameraPath { loops, ease: Some(false), ..orbit.clone() };
            let passes = if loops == Loop::PingPong { 2.0 } else { 1.0 };
            for i in -40..=80 {
                let t = i as f32 / 23.0;
                let folded = loops.extrapolate().fold(t as f64 * passes, 0.0, 1.0).0 as f32;
                let a = path.sample(t, cam.fov);
                // A ping-pong's geometry is the open path, so its folded time
                // is read off that path flown once; folding twice would turn
                // round a second time.
                let plain = if loops == Loop::PingPong {
                    CameraPath { loops: Loop::Once, ..path.clone() }
                } else {
                    path.clone()
                };
                let b = plain.sample(folded, cam.fov);
                assert!((a.eye() - b.eye()).length() < 1e-4, "{loops:?} at {t}");
            }
        }
    }
}

#[cfg(test)]
mod golden {
    use super::*;
    use std::fmt::Write as _;

    const SAMPLES: usize = 32;
    const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/golden/flights.txt");

    /// A cheap content hash of the scene file, recorded beside its samples.
    ///
    /// Without this the test cannot tell "the code moved this flight" from
    /// "someone edited this scene", and in a tree where scenes are authored
    /// daily that makes it cry wolf until nobody reads it. With it, an edited
    /// scene reports as an edited scene.
    fn digest(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        h
    }

    /// Every scene's flight, as text. One line per sample:
    /// `<scene> <i> eye.xyz forward.xyz up.xyz distance`
    fn capture() -> String {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/scenes");
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .expect("scenes/ must exist")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();

        let mut out = String::new();
        for file in &files {
            let name = file.file_stem().unwrap().to_string_lossy().into_owned();
            let _ = writeln!(
                out,
                "{} -- {:016x}",
                name,
                digest(&std::fs::read(file).unwrap_or_default())
            );
            let scene = match crate::scene::Scene::load(file) {
                Ok(s) => s,
                // A scene that doesn't load is itself worth pinning: if this
                // rework makes one stop loading, the diff says so by name.
                Err(e) => {
                    let _ = writeln!(out, "{} LOAD-ERROR {}", name, e);
                    continue;
                }
            };
            let base = scene.camera();
            let default = CameraPath::full_orbit(&base);
            let path = resolve(scene.camera_path.as_ref(), &default);

            for i in 0..SAMPLES {
                let cam = path.sample(i as f32 / SAMPLES as f32, Fov::default());
                let (e, f, u) = (cam.eye(), cam.forward(), cam.up());
                let _ = writeln!(
                    out,
                    "{} {:02} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6}",
                    name, i, e.x, e.y, e.z, f.x, f.y, f.z, u.x, u.y, u.z, cam.distance
                );
            }
        }
        out
    }

    /// Every scene in `scenes/`, saved and reloaded, must fly the same path.
    ///
    /// The save path is where this rework could do quiet damage. A route that
    /// survives in memory but not on disk still *loads*, still *plays*, and is
    /// only wrong the next time someone opens the file — by which point the
    /// corkscrew has silently unwound and nothing says when.
    #[test]
    fn a_saved_scene_flies_the_same_path() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/scenes");
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();

        let tmp = std::env::temp_dir().join("fracturize-roundtrip-flight.toml");
        for file in &files {
            let name = file.file_stem().unwrap().to_string_lossy().into_owned();
            let before = crate::scene::Scene::load(file).unwrap();

            let _ = std::fs::remove_file(&tmp);
            before.save(&tmp).unwrap();
            let after = crate::scene::Scene::load(&tmp).unwrap();

            let flight = |s: &crate::scene::Scene| {
                let base = s.camera();
                let default = CameraPath::full_orbit(&base);
                let path = resolve(s.camera_path.as_ref(), &default);
                (0..64)
                    .map(|i| path.sample(i as f32 / 64.0, Fov::default()))
                    .map(|c| (c.eye(), c.up(), c.distance))
                    .collect::<Vec<_>>()
            };

            for (i, (a, b)) in flight(&before).iter().zip(flight(&after).iter()).enumerate() {
                let drift = (a.0 - b.0).length().max((a.1 - b.1).length());
                assert!(
                    drift < 1e-3,
                    "{}: sample {} moved {:.6} across a save\n  eye {:?} -> {:?}",
                    name,
                    i,
                    drift,
                    a.0,
                    b.0
                );
            }

            // Routes specifically, not just where the curve happens to pass.
            if let (Some(x), Some(y)) = (&before.camera_path, &after.camera_path) {
                assert_eq!(x.routes, y.routes, "{}: routes changed across a save", name);
            }
        }
        let _ = std::fs::remove_file(&tmp);
    }

    /// A multi-turn path must still *look* multi-turn in the file.
    ///
    /// The routes are stored separately, so wrapping the yaw column into
    /// (-π, π] would keep playing correctly while destroying the one thing a
    /// person reads that column for. `winze` spans 1.66 turns; its later keys
    /// have to stay above 2π on disk.
    #[test]
    fn a_corkscrew_still_reads_as_one_on_disk() {
        let src = concat!(env!("CARGO_MANIFEST_DIR"), "/scenes/winze.toml");
        let scene = crate::scene::Scene::load(src).unwrap();
        let tmp = std::env::temp_dir().join("fracturize-roundtrip-winze.toml");
        let _ = std::fs::remove_file(&tmp);
        scene.save(&tmp).unwrap();

        // Only the camera track's keys; the base [camera] block has a yaw too.
        let text = std::fs::read_to_string(&tmp).unwrap();
        let yaws: Vec<f64> = text
            .split("[[track.keys]]")
            .skip(1)
            .filter_map(|block| {
                block
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("yaw = "))
                    .and_then(|v| v.trim().parse().ok())
            })
            .collect();
        assert!(yaws.len() >= 6, "expected winze's keys, found {:?}", yaws);
        assert!(
            yaws.iter().any(|&y| y > std::f64::consts::TAU),
            "the yaw column was wrapped — the corkscrew is invisible now: {:?}",
            yaws
        );
        assert!(
            yaws.windows(2).all(|w| w[1] > w[0]),
            "winze's keys must still climb monotonically: {:?}",
            yaws
        );
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn golden_flights_are_unchanged() {
        let fresh = capture();

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/golden")).unwrap();
            std::fs::write(GOLDEN, &fresh).unwrap();
            return;
        }

        let stored = std::fs::read_to_string(GOLDEN).unwrap_or_else(|e| {
            panic!("{}: {}\nrun UPDATE_GOLDEN=1 cargo test golden_flights_are_unchanged", GOLDEN, e)
        });

        // Index both sides by scene: hash line, then sample lines.
        fn index(text: &str) -> std::collections::BTreeMap<&str, (&str, Vec<&str>)> {
            let mut out: std::collections::BTreeMap<&str, (&str, Vec<&str>)> = Default::default();
            for line in text.lines() {
                let mut f = line.splitn(3, ' ');
                let (Some(name), Some(second)) = (f.next(), f.next()) else { continue };
                let entry = out.entry(name).or_default();
                if second == "--" {
                    entry.0 = f.next().unwrap_or("");
                } else {
                    entry.1.push(line);
                }
            }
            out
        }

        let (was, now) = (index(&stored), index(&fresh));
        let mut moved: Vec<String> = Vec::new();
        let mut edited: Vec<&str> = Vec::new();

        for (name, (hash_now, samples_now)) in &now {
            let Some((hash_was, samples_was)) = was.get(name) else { continue };
            // An edited scene is expected to fly differently. That is the
            // author's doing, not the code's, and it is not this test's news.
            if hash_was != hash_now {
                edited.push(name);
                continue;
            }
            let drift = samples_was
                .iter()
                .zip(samples_now.iter())
                .flat_map(|(a, b)| a.split(' ').skip(2).zip(b.split(' ').skip(2)))
                .filter_map(|(x, y)| Some((x.parse::<f64>().ok()? - y.parse::<f64>().ok()?).abs()))
                .fold(0.0f64, f64::max);
            if drift > 1e-5 {
                moved.push(format!("  {:<24} max drift {:.6}", name, drift));
            }
        }

        assert!(
            moved.is_empty(),
            "{} scene(s) fly differently with the same scene file — that is this \
             change's doing:\n{}\nIf it is the accepted cost, re-record with \
             UPDATE_GOLDEN=1 and say so in the commit.{}",
            moved.len(),
            moved.join("\n"),
            if edited.is_empty() {
                String::new()
            } else {
                format!("\n(ignored, edited since recording: {})", edited.join(", "))
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::world_to_screen;
    use crate::rot::Angle;

    fn key(yaw: f32, pitch: f32, dist: f32, focus: Vec3) -> PathKey {
        PathKey::from_camera(&OrbitCamera::from_chart(yaw, pitch, 0.0, dist, focus))
    }

    fn rolled_key(yaw: f32, pitch: f32, roll: f32, dist: f32) -> PathKey {
        PathKey::from_camera(&OrbitCamera::from_chart(yaw, pitch, roll, dist, Vec3::ZERO))
    }

    /// Total rotation about world Y swept between two path times, accumulated
    /// from consecutive samples.
    ///
    /// Orientation is periodic, so "what is the yaw now" cannot tell you which
    /// way the camera went or how many times round. Adding up short steps can,
    /// and it is also what the eye actually does.
    fn swept_about_y(p: &CameraPath, from: f32, to: f32, n: usize) -> f32 {
        let mut total = 0.0;
        let mut prev = p.sample(from, Fov::default()).orientation;
        for i in 1..=n {
            let t = from + (to - from) * i as f32 / n as f32;
            let cur = p.sample(t, Fov::default()).orientation;
            // World-frame step: cur ∘ prev⁻¹.
            let step = prev.inverse().then(cur);
            total += Orientation::IDENTITY
                .shortest_turn_to(step)
                .as_rotation_vector()
                .y;
            prev = cur;
        }
        total
    }

    /// A 0.6 spiral about the origin, twisting 34° per period about Y —
    /// `wellspiral`'s descent map, as a loop closing over `periods` periods.
    fn zoom_loop_path(periods: u32, keys: Vec<PathKey>) -> CameraPath {
        let mut p = CameraPath {
            keys,
            routes: Vec::new(),
            loops: Loop::Zoom(ZoomLoop {
                periods,
                center: Vec3::ZERO,
                scale: 0.6f32.powi(periods as i32),
                turn: Turn::about(Vec3::Y, 34f32.to_radians() * periods as f32),
            }),
            ease: None,
            times: Vec::new(),
            period: None,
        };
        p.retime_to(10.0);
        p
    }

    fn linear_path(loops: Loop) -> CameraPath {
        CameraPath {
            keys: vec![
                key(0.0, 0.1, 2.0, Vec3::ZERO),
                key(1.0, 0.3, 4.0, Vec3::X),
                key(2.0, 0.2, 3.0, Vec3::Y),
            ],
            routes: Vec::new(),
            loops,
            ease: Some(false),
            times: Vec::new(),
            period: None,
        }
    }

    /// `sample` wraps t, so t=1 *is* t=0 — correct for playback, since the
    /// renderer folds the camera back into the band anyway. The seam is
    /// therefore probed just short of it.
    const SEAM: f32 = 1.0 - 1e-4;

    // -- the spline itself ------------------------------------------------

    #[test]
    fn the_cumulative_basis_has_the_two_identities_everything_rests_on() {
        // B̃(0) = (1,0,0) and B̃(1) = (1,1,0) make the curve pass through its
        // keys; B̃₁+B̃₂+B̃₃ = 1+u makes equal displacements a constant-rate
        // sweep, which is what gives a zoom loop no seam.
        let at0 = cumulative_basis(0.0);
        let at1 = cumulative_basis(1.0);
        assert!((at0[0] - 1.0).abs() < 1e-6 && at0[1].abs() < 1e-6 && at0[2].abs() < 1e-6, "{:?}", at0);
        assert!(
            (at1[0] - 1.0).abs() < 1e-6 && (at1[1] - 1.0).abs() < 1e-6 && at1[2].abs() < 1e-6,
            "{:?}",
            at1
        );
        for i in 0..=20 {
            let u = i as f32 / 20.0;
            let b = cumulative_basis(u);
            assert!(
                (b[0] + b[1] + b[2] - (1.0 + u)).abs() < 1e-5,
                "u={}: sum {} wants {}",
                u,
                b[0] + b[1] + b[2],
                1.0 + u
            );
        }
    }

    #[test]
    fn passes_through_keys() {
        for loops in [Loop::Once, Loop::PingPong, Loop::Closed] {
            let p = linear_path(loops);
            let n = p.segments();
            // A ping-pong covers the same segments in the first *half* of its
            // loop and then comes back over them, so its keys sit twice as
            // close together in path time.
            let span = if loops == Loop::PingPong { 0.5 } else { 1.0 };
            for (i, k) in p.keys.iter().enumerate() {
                let t = span * i as f32 / n as f32;
                let cam = p.sample(t, Fov::default());
                assert!(
                    cam.orientation.angle_to(k.orientation) < 1e-4,
                    "loops={:?} key {} orientation",
                    loops,
                    i
                );
                assert!((cam.distance - k.distance).abs() < 1e-3, "key {} dist", i);
                assert!((cam.focus - k.focus).length() < 1e-4, "key {} focus", i);
            }
        }
    }

    #[test]
    fn a_constant_pitch_path_is_exactly_the_old_scalar_yaw_spline() {
        // The back-compat theorem: when the segment turns share an axis they
        // commute, and the cumulative form collapses to the ordinary
        // Catmull-Rom of the yaw values. That is most scenes, and it is why
        // they did not move.
        let yaws = [0.3f32, 1.1, 2.4, 3.0, 3.9];
        let p = CameraPath {
            keys: yaws.iter().map(|&y| key(y, 0.35, 3.0, Vec3::ZERO)).collect(),
            routes: Vec::new(),
            loops: Loop::Once,
            ease: Some(false),
            times: Vec::new(),
            period: None,
        };
        let segs = p.segments();
        for i in 0..=40 {
            let t = i as f32 / 40.0;
            // What the old implementation would have produced, longhand.
            let x = t * segs as f32;
            let seg = (x.floor() as isize).min(segs as isize - 1);
            let u = x - seg as f32;
            let at = |j: isize| yaws[j.clamp(0, yaws.len() as isize - 1) as usize];
            let want = catmull_rom(at(seg - 1), at(seg), at(seg + 1), at(seg + 2), u);

            // Compared as angles: the chart reports a representative in
            // (-π, π] and the old scalar spline didn't, so past half a turn
            // the two agree while the numbers differ by exactly 2π.
            let got = p.sample(t, Fov::default()).chart();
            let off = got.yaw.shortest_to(Angle::from_radians(want)).abs();
            assert!(off < 1e-4, "t={}: yaw {} vs old spline {}", t, got.yaw.radians(), want);
            assert!((got.pitch.radians() - 0.35).abs() < 1e-4, "pitch must not drift at t={}", t);
            assert!(
                got.roll.radians().abs() < 1e-4,
                "t={}: a level path must stay level, got {}° of roll",
                t,
                got.roll.degrees()
            );
        }
    }

    #[test]
    fn closed_path_is_seamless() {
        let p = linear_path(Loop::Closed);
        let a = p.sample(0.0, Fov::default());
        let b = p.sample(1.0, Fov::default());
        assert!((a.eye() - b.eye()).length() < 1e-3);
        assert!((a.focus - b.focus).length() < 1e-4);
        // C1 continuity at the seam: second-order one-sided derivative
        // estimates from both sides must agree
        let eps = 1e-3;
        let f = |t: f32| p.sample(t, Fov::default()).eye();
        let va = (3.0 * f(1.0) - 4.0 * f(1.0 - eps) + f(1.0 - 2.0 * eps)) / (2.0 * eps);
        let vb = (-3.0 * f(0.0) + 4.0 * f(eps) - f(2.0 * eps)) / (2.0 * eps);
        assert!(
            (va - vb).length() < 0.02 * va.length().max(1.0),
            "seam velocity jump: {:?} vs {:?}",
            va,
            vb
        );
    }

    #[test]
    fn closed_orbit_wraps_forward() {
        // Keys at 0/90/180/270 degrees: the closing segment must continue
        // forward through 360, not swing back through 0.
        let tau = std::f32::consts::TAU;
        let p = CameraPath {
            keys: (0..4).map(|i| key(i as f32 * tau / 4.0, 0.2, 3.0, Vec3::ZERO)).collect(),
            routes: Vec::new(),
            loops: Loop::Closed,
            ease: Some(false),
            times: Vec::new(),
            period: None,
        };
        let swept = swept_about_y(&p, 0.0, 1.0, 256);
        assert!((swept - tau).abs() < 1e-2, "swept {} rad, wanted a forward full turn", swept);

        // Uniform keys about one axis: the sweep is exactly linear in t.
        for i in 1..16 {
            let t = i as f32 / 16.0;
            let so_far = swept_about_y(&p, 0.0, t, 128);
            assert!((so_far - t * tau).abs() < 5e-3, "t={}: swept {} vs {}", t, so_far, t * tau);
        }
    }

    #[test]
    fn a_winding_takes_the_long_way_round_without_moving_the_keys() {
        // Two framings a degree apart. Winding 0 nudges; winding 1 goes all the
        // way round the houses. Both land on exactly the same framing — a route
        // can surprise you, but it cannot put the camera in the wrong place.
        let keys = vec![key(0.0, 0.0, 3.0, Vec3::ZERO), key(1f32.to_radians(), 0.0, 3.0, Vec3::ZERO)];
        let with = |w: i32| CameraPath {
            routes: vec![Route::Turns(w)],
            ..CameraPath::new(keys.clone(), Loop::Once)
        };
        let short = with(0);
        let long = with(1);

        for p in [&short, &long] {
            assert!(p.sample(0.0, Fov::default()).orientation.angle_to(keys[0].orientation) < 1e-4);
            assert!(p.sample(1.0, Fov::default()).orientation.angle_to(keys[1].orientation) < 1e-4);
        }
        let s = swept_about_y(&short, 0.0, 1.0, 256);
        let l = swept_about_y(&long, 0.0, 1.0, 256);
        assert!(s.abs() < 0.05, "the short way swept {} rad", s);
        assert!(
            (l - std::f32::consts::TAU).abs() < 0.05,
            "one winding should be a whole extra turn, swept {} rad",
            l
        );
    }

    #[test]
    fn multi_turn_spiral() {
        // Two full turns, authored as two windings, stays monotonic and never
        // wraps back on itself.
        let tau = std::f32::consts::TAU;
        let p = CameraPath {
            keys: vec![
                key(0.0, 0.0, 5.0, Vec3::ZERO),
                key(0.0, 0.0, 3.0, Vec3::ZERO),
                key(0.0, 0.0, 1.0, Vec3::ZERO),
            ],
            routes: vec![Route::Turns(1); 2],
            loops: Loop::Once,
            ease: Some(false),
            times: Vec::new(),
            period: None,
        };
        let mut last = 0.0f32;
        for i in 1..=32 {
            let so_far = swept_about_y(&p, 0.0, i as f32 / 32.0, 256);
            assert!(so_far > last - 1e-3, "sweep must not go backwards: {} then {}", last, so_far);
            last = so_far;
        }
        assert!((last - 2.0 * tau).abs() < 5e-2, "two turns is {}, swept {}", 2.0 * tau, last);
    }

    #[test]
    fn distance_interpolates_geometrically() {
        let p = CameraPath {
            ease: Some(false),
            ..CameraPath::new(
                vec![key(0.0, 0.0, 1.0, Vec3::ZERO), key(0.0, 0.0, 4.0, Vec3::ZERO)],
                Loop::Once,
            )
        };
        assert!((p.sample(0.5, Fov::default()).distance - 2.0).abs() < 1e-3, "got {}", p.sample(0.5, Fov::default()).distance);
    }

    #[test]
    fn roll_interpolates_along_the_path() {
        // Only the roll differs, so the segment turns share the body Z axis and
        // the spline is exactly a scalar roll ramp.
        let p = CameraPath {
            ease: Some(false),
            ..CameraPath::new(
                vec![rolled_key(0.0, 0.0, 0.0, 3.0), rolled_key(0.0, 0.0, 1.0, 3.0)],
                Loop::Once,
            )
        };
        assert!(p.sample(0.0, Fov::default()).chart().roll.radians().abs() < 1e-4);
        assert!((p.sample(0.5, Fov::default()).chart().roll.radians() - 0.5).abs() < 1e-3);
        assert!((p.sample(1.0, Fov::default()).chart().roll.radians() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_path_can_fly_over_the_pole() {
        // The framings the old camera could not reach, and the reason for all
        // of this. Straight up is an ordinary interior point of the spline.
        let p = CameraPath {
            ease: Some(false),
            ..CameraPath::new(
                vec![
                    key(0.0, 1.2, 3.0, Vec3::ZERO),
                    key(0.0, std::f32::consts::FRAC_PI_2, 3.0, Vec3::ZERO),
                    key(std::f32::consts::PI, 1.2, 3.0, Vec3::ZERO),
                ],
                Loop::Once,
            )
        };
        let mut prev = p.sample(0.0, Fov::default()).forward();
        for i in 1..=200 {
            let cam = p.sample(i as f32 / 200.0, Fov::default());
            let f = cam.forward();
            assert!(f.is_finite() && cam.up().is_finite(), "basis blew up at t={}", i);
            assert!((cam.up().length() - 1.0).abs() < 1e-3, "basis stopped being unit");
            assert!(f.dot(prev) > 0.99, "the view snapped at t={}: dot {}", i, f.dot(prev));
            prev = f;
        }
        // ...and it really did pass over the top.
        assert!(p.sample(0.5, Fov::default()).forward().y < -0.99, "should be looking straight down at the midpoint");
    }

    // -- zoom loops --------------------------------------------------------

    #[test]
    fn a_zoom_loop_ends_on_the_frame_it_started() {
        // The whole claim: the last frame is not *near* the first, it is the
        // first — because the scene is invariant under the similarity that
        // separates them.
        let path = zoom_loop_path(1, vec![key(0.3, 0.5, 3.6, Vec3::ZERO)]);
        let (start, end) = (path.sample(0.0, Fov::default()), path.sample(SEAM, Fov::default()));
        let z = path.loops.zoom().unwrap();
        let rot = z.turn.exp();
        let vp_start = start.view_proj(16.0 / 9.0);
        let vp_end = end.view_proj(16.0 / 9.0);
        for i in 0..25 {
            let x = Vec3::new(
                0.9 * (i as f32 * 1.7).sin(),
                0.9 * (i as f32 * 0.9).cos(),
                0.9 * (i as f32 * 2.3).sin(),
            );
            let partner = z.center + rot.rotate((x - z.center) * z.scale);
            match (
                world_to_screen(partner, vp_end, 1280.0, 720.0),
                world_to_screen(x, vp_start, 1280.0, 720.0),
            ) {
                (Some(a), Some(b)) => assert!(
                    (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0,
                    "point {i}: end frame put it at {a:?}, start frame at {b:?}"
                ),
                (None, None) => {}
                _ => panic!("point {i} changed visibility across the loop seam"),
            }
        }
    }

    #[test]
    fn a_zoom_loop_is_continuous_across_the_seam() {
        let path = zoom_loop_path(1, vec![key(0.3, 0.5, 3.6, Vec3::ZERO)]);
        let z = path.loops.zoom().unwrap();
        let carried = z.advance(PathKey::from_camera(&path.sample(SEAM, Fov::default())), -1);
        let start = PathKey::from_camera(&path.sample(0.0, Fov::default()));
        assert!(
            (carried.distance / start.distance - 1.0).abs() < 1e-3,
            "distance {} vs {}",
            carried.distance,
            start.distance
        );
        assert!(
            carried.orientation.angle_to(start.orientation) < 1e-3,
            "framing drifted by {}° per loop",
            carried.orientation.angle_to(start.orientation).to_degrees()
        );
    }

    #[test]
    fn a_one_key_zoom_loop_is_the_similarity_flow_itself() {
        // Every segment turn is the same, so the cumulative form telescopes to
        // exactly rot^(1+u): not an approximation of a constant-rate zoom, the
        // thing itself. Log-distance is linear for the same reason.
        let path = zoom_loop_path(1, vec![key(0.0, 0.4, 4.0, Vec3::ZERO)]);
        assert_eq!(path.segments(), 1, "a single key plus its image is one segment");
        let z = path.loops.zoom().unwrap();
        let start = path.sample(0.0, Fov::default()).orientation;
        for i in 0..=20 {
            let t = i as f32 / 20.0 * SEAM;
            let want_dist = 4.0f32.ln() + t * 0.6f32.ln();
            let got = path.sample(t, Fov::default());
            assert!(
                (got.distance.ln() - want_dist).abs() < 1e-3,
                "t={t}: log-distance {}, constant rate wants {want_dist}",
                got.distance.ln()
            );
            // rot^t applied to the starting framing, exactly.
            let want = start.then((z.turn * t).exp());
            assert!(
                got.orientation.angle_to(want) < 1e-3,
                "t={t}: framing is {}° off the similarity flow",
                got.orientation.angle_to(want).to_degrees()
            );
        }
    }

    #[test]
    fn a_multi_period_zoom_loop_descends_that_many_periods() {
        let path = zoom_loop_path(3, vec![key(0.0, 0.4, 4.0, Vec3::ZERO)]);
        let ratio = path.sample(SEAM, Fov::default()).distance / 4.0;
        assert!((ratio - 0.6f32.powi(3)).abs() < 1e-3, "descended {ratio}x");
    }

    #[test]
    fn a_multi_key_zoom_loop_hits_its_keys_and_still_closes() {
        // The point of allowing more than one key on a zoom loop: an infinite
        // zoom that spirals in on a curving path, shaped by real keys partway
        // down the descent. Each segment used to be replaced wholesale by the
        // symmetry's turn, which skipped every authored framing and overswept
        // the loop by the key count.
        let keys = vec![
            key(0.3, 0.5, 3.6, Vec3::ZERO),
            key(1.1, 0.1, 2.4, Vec3::new(0.2, 0.0, -0.1)),
        ];
        let path = zoom_loop_path(1, keys.clone());
        assert_eq!(path.segments(), 2, "two keys plus the first key's image is two segments");

        for (i, k) in keys.iter().enumerate() {
            let cam = path.sample(i as f32 / 2.0, Fov::default());
            assert!(
                cam.orientation.angle_to(k.orientation) < 1e-4,
                "key {i}: framing missed by {}°",
                cam.orientation.angle_to(k.orientation).to_degrees()
            );
            assert!((cam.distance - k.distance).abs() < 1e-3, "key {i} distance");
            assert!((cam.focus - k.focus).length() < 1e-4, "key {i} focus");
        }

        // ...and the seam still closes under the symmetry, exactly as the
        // one-key loop always has.
        let z = path.loops.zoom().unwrap();
        let carried = z.advance(PathKey::from_camera(&path.sample(SEAM, Fov::default())), -1);
        let start = PathKey::from_camera(&path.sample(0.0, Fov::default()));
        assert!(
            (carried.distance / start.distance - 1.0).abs() < 1e-3,
            "distance {} vs {}",
            carried.distance,
            start.distance
        );
        assert!(
            carried.orientation.angle_to(start.orientation) < 1e-3,
            "framing drifted by {}° per loop",
            carried.orientation.angle_to(start.orientation).to_degrees()
        );
        assert!((carried.focus - start.focus).length() < 1e-3, "focus drifted across the seam");
    }

    #[test]
    fn a_winding_does_not_roll_the_segments_beside_it() {
        // The corkscrew-injection bug, as a test. B̃₁ overshoots 1 by 7.4% and
        // B̃₃ dips 7.4% below 0 — ordinary Catmull-Rom corner-rounding,
        // invisible on a short turn. Applied to a neighbour whose displacement
        // is three whole turns, it was ~80° of pitch-axis rotation swung
        // through the middle of the adjacent yaw segments and back out: on
        // screen, yawing visibly rolled the view. The whole-turn part of a
        // neighbour now holds its endpoint weight, so the wobble is bounded by
        // the sub-π parts, winding or none.
        let p = 0.7f32;
        let path = CameraPath {
            keys: vec![
                key(0.0, 0.0, 3.0, Vec3::ZERO),
                key(0.0, p, 3.0, Vec3::ZERO),
                key(1.2, p, 3.0, Vec3::ZERO),
                key(-0.8, p, 3.0, Vec3::ZERO),
            ],
            routes: vec![Route::Turns(3), Route::default(), Route::default()],
            loops: Loop::Once,
            ease: Some(false),
            times: Vec::new(),
            period: None,
        };

        // The corkscrew itself still happens: segment 0 sweeps its three whole
        // turns of pitch, plus the 0.7 the keys ask for...
        let mut swept_x = 0.0;
        let mut prev = path.sample(0.0, Fov::default()).orientation;
        for i in 1..=300 {
            let cur = path.sample(i as f32 / 900.0, Fov::default()).orientation;
            swept_x += Orientation::IDENTITY
                .shortest_turn_to(prev.inverse().then(cur))
                .as_rotation_vector()
                .x;
            prev = cur;
        }
        let want = 3.0 * std::f32::consts::TAU + p;
        assert!(
            (swept_x.abs() - want).abs() < 0.3,
            "segment 0 swept {} rad of pitch, authored {}",
            swept_x.abs(),
            want
        );

        // ...and its keys are still hit exactly.
        for (i, k) in path.keys.iter().enumerate() {
            let cam = path.sample(i as f32 / 3.0, Fov::default());
            assert!(
                cam.orientation.angle_to(k.orientation) < 1e-4,
                "key {i}: framing missed by {}°",
                cam.orientation.angle_to(k.orientation).to_degrees()
            );
        }

        // The yaw segments beside it stay level. Before the split, the worst
        // horizon tilt here was 40°.
        let mut worst = 0.0f32;
        for i in 0..=300 {
            let t = 1.0 / 3.0 + (2.0 / 3.0) * i as f32 / 300.0;
            worst = worst.max(path.sample(t, Fov::default()).right().y.abs().asin());
        }
        assert!(
            worst.to_degrees() < 3.5,
            "yawing next to a corkscrew tilted the horizon {}°",
            worst.to_degrees()
        );
    }

    #[test]
    fn a_barely_twisting_map_sweeps_the_short_way_round() {
        // A map that turns half a degree one way is the *same rotation* as one
        // that turns 359.5° the other. Read the wrong way, the camera swung
        // almost the whole way round to arrive where a nudge would have done.
        // `Renorm` now settles that branch once, and a Turn cannot re-fold it.
        let nearly_round = Orientation::from_axis_angle(Vec3::Y, 359.5f32.to_radians());
        let twist = Orientation::IDENTITY.shortest_turn_to(nearly_round);
        let path = CameraPath {
            keys: vec![key(0.0, 0.4, 4.0, Vec3::ZERO)],
            routes: Vec::new(),
            loops: Loop::Zoom(ZoomLoop {
                periods: 1,
                center: Vec3::ZERO,
                scale: 0.6,
                turn: twist,
            }),
            ease: None,
            times: vec![0.0],
            period: Some(10.0),
        };
        let swept = swept_about_y(&path, 0.0, SEAM, 256);
        assert!(
            swept.abs() < 1f32.to_radians(),
            "swept {:.1}° to make a half-degree turn",
            swept.to_degrees()
        );
        assert!(swept < 0.0, "359.5° is a turn the other way, swept {:.1}°", swept.to_degrees());

        // And it still closes exactly where the symmetry says.
        let z = path.loops.zoom().unwrap();
        let carried = z.center + z.turn.exp().rotate((path.sample(0.0, Fov::default()).eye() - z.center) * z.scale);
        assert!(
            (path.sample(SEAM, Fov::default()).eye() - carried).length() < 1e-3,
            "ended at {:?}, symmetry says {carried:?}",
            path.sample(SEAM, Fov::default()).eye()
        );
    }

    #[test]
    fn a_multi_period_loop_keeps_its_whole_sweep() {
        // The bug this rework found. pythagoras-zoomy's map twists 46.9°; four
        // periods is 187.6°, which is past half a turn. Stored as a quaternion
        // and read back it came out as 172.4° *the other way*, and the loop
        // flew backwards. A Turn is unbounded, so there is no branch to lose.
        let per_period = 46.9f32.to_radians();
        let path = CameraPath {
            keys: vec![key(0.0, 0.4, 4.0, Vec3::ZERO)],
            routes: Vec::new(),
            loops: Loop::Zoom(ZoomLoop {
                periods: 4,
                center: Vec3::ZERO,
                scale: 0.6f32.powi(4),
                turn: Turn::about(Vec3::Y, per_period * 4.0),
            }),
            ease: None,
            times: vec![0.0],
            period: Some(10.0),
        };
        let swept = swept_about_y(&path, 0.0, SEAM, 512).to_degrees();
        assert!(
            (swept - 187.6).abs() < 1.0,
            "four periods of a 46.9° map is +187.6°, swept {:.1}° \
             (the old code got -172.4° — the wrong way round)",
            swept
        );
    }

    #[test]
    fn a_real_scenes_zoom_loop_closes_under_its_own_symmetry() {
        // pythagoras-zoomy, from disk, because this is the case that was
        // silently wrong and no synthetic fixture caught it.
        //
        // Its map twists 47.02° about (0.117, -0.282, -0.952) — an axis that
        // is mostly *Z*. The old spline tracked only the vertical component of
        // that twist, so it swept the camera -13.27° of yaw across a loop the
        // geometry needed 47.02° of turn to close. The loop did not close, and
        // nothing said so: it just drifted a little every pass.
        let scene = crate::scene::Scene::load(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scenes/pythagoras-zoomy.toml"
        ))
        .unwrap();
        let base = scene.camera();
        let default = CameraPath::full_orbit(&base);
        let path = resolve(scene.camera_path.as_ref(), &default);
        let z = path.loops.zoom().expect("pythagoras-zoomy closes under its zoom symmetry");

        let start = path.sample(0.0, Fov::default());
        let end = path.sample(SEAM, Fov::default());
        // Where the symmetry says a loop has to land.
        let want = z.center + z.turn.exp().rotate((start.eye() - z.center) * z.scale);
        let err = (end.eye() - want).length();
        assert!(
            err < 1e-2 * start.distance,
            "loop misses its own image by {:.4} at radius {:.3}",
            err,
            start.distance
        );
    }

    #[test]
    fn one_key_plus_a_zoom_loop_is_playable() {
        let one = zoom_loop_path(1, vec![key(0.0, 0.4, 4.0, Vec3::ZERO)]);
        assert!(one.playable(), "a single key closing under the symmetry is a path");
        let bare = CameraPath { loops: Loop::Once, ..one };
        assert!(!bare.playable(), "one key and no zoom loop is a scene mid-authoring");
    }

    #[test]
    fn a_zoom_loop_does_not_ease() {
        assert!(!zoom_loop_path(1, vec![key(0.0, 0.4, 4.0, Vec3::ZERO)]).eased());
    }

    // -- ping-pong ---------------------------------------------------------

    #[test]
    fn a_ping_pong_comes_home_the_way_it_went_out() {
        let p = CameraPath { ease: None, ..linear_path(Loop::PingPong) };

        // Out to the last key by halfway...
        let last = *p.keys.last().unwrap();
        let mid = p.sample(0.5, Fov::default());
        assert!(mid.orientation.angle_to(last.orientation) < 1e-4, "halfway is the last key");
        assert!((mid.distance - last.distance).abs() < 1e-3);

        // ...and the return leg is the outward one read backwards, frame for
        // frame. This is the property that makes it work on a path whose ends
        // are nowhere near each other: there is nothing to close.
        for i in 0..=20 {
            let t = i as f32 / 40.0;
            let out = p.sample(t, Fov::default());
            let back = p.sample(1.0 - t, Fov::default());
            assert!(out.orientation.angle_to(back.orientation) < 1e-4, "t={}", t);
            assert!((out.distance - back.distance).abs() < 1e-4, "t={}", t);
            assert!((out.focus - back.focus).length() < 1e-4, "t={}", t);
        }

        // So the last frame runs into the first, with no jump.
        assert!(p.sample(1.0, Fov::default()).orientation.angle_to(p.keys[0].orientation) < 1e-4);
    }

    #[test]
    fn a_ping_pong_turns_round_at_rest() {
        // The ease is not decoration here: a triangle wave reverses direction
        // instantaneously, and smoothstep's zero derivative at both ends is
        // what turns that bounce into a decelerate-and-return. Hence the
        // default, and hence this test.
        let eased = CameraPath { ease: None, ..linear_path(Loop::PingPong) };
        let hard = CameraPath { ease: Some(false), ..linear_path(Loop::PingPong) };
        assert!(eased.eased(), "a ping-pong eases unless the scene says not to");

        let step = |p: &CameraPath, a: f32, b: f32| {
            p.sample(a, Fov::default()).orientation.angle_to(p.sample(b, Fov::default()).orientation)
        };
        const D: f32 = 0.01;
        let cruise = step(&eased, 0.25 - D / 2.0, 0.25 + D / 2.0);
        for turn in [0.5, 1.0] {
            let eased_step = step(&eased, turn - D, turn);
            assert!(
                eased_step < 0.1 * cruise,
                "t={}: {} into the turn vs {} cruising",
                turn,
                eased_step,
                cruise
            );
            assert!(
                eased_step < 0.25 * step(&hard, turn - D, turn),
                "t={}: the ease is what slows the turnaround",
                turn
            );
        }
    }

    #[test]
    fn a_ping_pong_is_an_open_path_flown_twice() {
        let once = linear_path(Loop::Once);
        let pong = linear_path(Loop::PingPong);
        // Same geometry — the loop is entirely in the time mapping.
        assert_eq!(pong.segments(), once.segments());
        assert!(!Loop::PingPong.closes(), "nothing to close");
        assert!(Loop::PingPong.wraps(), "but playback repeats");

        // Twice the journey at the same speed, rather than the same journey
        // twice as fast.
        assert_eq!(pong.duration(), 2.0 * once.duration());
        let mut nine = pong.clone();
        nine.retime_to(9.0);
        assert!((nine.duration() - 9.0).abs() < 1e-5, "a pinned duration is still the whole loop");
    }

    // -- defaults, resolution, housekeeping --------------------------------

    // -- key times ------------------------------------------------------------

    fn timed(times: &[f32], loops: Loop) -> CameraPath {
        let yaws = [0.2f32, 1.0, 1.5, 2.6, 3.1];
        let mut p = CameraPath::new(
            times.iter().zip(yaws).map(|(_, y)| key(y, 0.3, 3.0 + y, Vec3::new(y, 0.0, 0.0))).collect(),
            loops,
        );
        p.times = times.to_vec();
        p.ease = Some(false);
        p
    }

    /// A key is a view at a moment: the camera is there at that time, however
    /// unevenly the moments fall.
    #[test]
    fn uneven_keys_are_hit_at_their_times() {
        let p = timed(&[0.0, 1.0, 5.0, 5.5, 9.0], Loop::Once);
        assert!((p.duration() - 9.0).abs() < 1e-6);
        for (i, &t) in [0.0f32, 1.0, 5.0, 5.5, 9.0].iter().enumerate() {
            let c = p.sample_at(t as f64, Fov::default());
            let k = p.keys[i];
            assert!(c.orientation.angle_to(k.orientation) < 1e-4, "key {i} at {t}s");
            assert!((c.distance - k.distance).abs() < 1e-4 && (c.focus - k.focus).length() < 1e-4);
        }
    }

    /// Uneven times keep the camera's *speed* continuous through a key — what
    /// the per-second tangents are for. A uniform spline reparameterized by
    /// time would jump in speed by the ratio of the two segments' lengths.
    #[test]
    fn speed_runs_through_an_unevenly_timed_key() {
        let p = timed(&[0.0, 1.0, 5.0, 6.0, 7.0], Loop::Once);
        // Speed is continuous through a key; acceleration is not (Catmull-Rom
        // is C¹), so it is measured close to the key.
        let speed = |t: f64| {
            let h = 2e-4;
            let a = p.sample_at(t - h, Fov::default()).focus.x;
            let b = p.sample_at(t + h, Fov::default()).focus.x;
            (b - a) / (2.0 * h as f32)
        };
        for t in [1.0, 5.0, 6.0] {
            let (before, after) = (speed(t - 0.001), speed(t + 0.001));
            assert!(
                (before - after).abs() < 0.02 * before.abs().max(after.abs()).max(0.05),
                "speed jumps at {t}s: {before} -> {after}"
            );
        }
    }

    /// A loop's closing segment is what its period leaves after the last key.
    #[test]
    fn a_loop_closes_over_its_period() {
        let mut p = timed(&[0.0, 2.0, 3.0], Loop::Closed);
        p.period = Some(10.0);
        assert_eq!(p.duration(), 10.0);
        let at = |t: f64| p.sample_at(t, Fov::default());
        assert!(at(10.0).orientation.angle_to(p.keys[0].orientation) < 1e-4, "back home at the period");
        assert!(at(12.0).orientation.angle_to(p.keys[1].orientation) < 1e-4, "and round again");
        // Seven seconds of closing segment: halfway home is well past key 2.
        assert!(at(6.5).orientation.angle_to(p.keys[2].orientation) > 0.1);
    }

    /// Keying at the playhead inserts in time order, replaces a key already
    /// there, and leaves every other key's time alone; retiming never
    /// reorders.
    #[test]
    fn keying_and_retiming_keep_the_keys_in_order() {
        let mut p = timed(&[0.0, 3.0, 6.0], Loop::Once);
        let k = key(9.0, 0.1, 2.0, Vec3::ZERO);
        assert_eq!(p.key_at(4.0, k), 2);
        assert_eq!(p.key_times(), vec![0.0, 3.0, 4.0, 6.0]);
        assert_eq!(p.routes.len(), p.segments());
        assert_eq!(p.key_at(3.0, k), 1, "a key already there is replaced");
        assert_eq!(p.keys.len(), 4);
        p.set_key_time(1, 100.0);
        assert!(p.time(1) < p.time(2), "a retime is clamped short of its neighbour");
        p.remove_key(0);
        assert_eq!(p.keys.len(), 3);
        assert_eq!(p.routes.len(), p.segments());
        assert!((p.time(0) - p.times[0]).abs() < 1e-6);

        // A key past the end of a loop stretches the period to keep the
        // closing segment as long as it was.
        let mut l = timed(&[0.0, 2.0], Loop::Closed);
        l.period = Some(4.0);
        l.key_at(5.0, k);
        assert!((l.lap() - 7.0).abs() < 1e-5, "{}", l.lap());
    }

    /// `retime_to` is the duration field: every key keeps its share of it.
    #[test]
    fn retiming_scales_every_key() {
        let mut p = timed(&[1.0, 2.0, 5.0], Loop::PingPong);
        p.retime_to(16.0);
        assert!((p.duration() - 16.0).abs() < 1e-5);
        assert_eq!(p.time(0), 1.0, "the first key stays put");
        assert!((p.time(1) - 3.0).abs() < 1e-5 && (p.time(2) - 9.0).abs() < 1e-5);
    }

    #[test]
    fn full_orbit_matches_auto_orbit() {
        let base = OrbitCamera::from_chart(0.7, 0.3, 0.0, 4.0, Vec3::X);
        let p = CameraPath::full_orbit(&base);
        let mid = p.sample(0.5, Fov::default()).chart();
        let want = base.chart();
        assert!(
            (Angle::from_radians(want.yaw.radians() + std::f32::consts::PI)
                .shortest_to(mid.yaw)
                .abs())
                < 1e-3
        );
        assert!((mid.pitch.radians() - want.pitch.radians()).abs() < 1e-4);
        assert!((p.sample(0.5, Fov::default()).distance - base.distance).abs() < 1e-3);
    }

    #[test]
    fn full_orbit_turns_at_the_orbit_rate() {
        // The default path's duration *is* the old turntable's angular speed:
        // one full turn of yaw, at ORBIT_RATE rad/s.
        let base = OrbitCamera::from_chart(0.0, 0.2, 0.0, 3.0, Vec3::ZERO);
        let p = CameraPath::full_orbit(&base);
        let swept = swept_about_y(&p, 0.0, 1.0, 512);
        assert!((swept - std::f32::consts::TAU).abs() < 1e-2, "swept {}", swept);
        assert!((swept / p.duration() - ORBIT_RATE).abs() < 1e-3);
    }

    #[test]
    fn resolve_prefers_authored_keys_but_needs_two() {
        let base = OrbitCamera::from_chart(0.0, 0.0, 0.0, 3.0, Vec3::ZERO);
        let default = CameraPath::full_orbit(&base);

        assert!(std::ptr::eq(resolve(None, &default), &default));
        let one_key = CameraPath::new(vec![key(1.0, 0.2, 2.0, Vec3::Z)], Loop::Once);
        assert!(std::ptr::eq(resolve(Some(&one_key), &default), &default));

        let authored = linear_path(Loop::Once);
        assert!(std::ptr::eq(resolve(Some(&authored), &default), &authored));
    }

    #[test]
    fn open_path_eases_by_default() {
        let p = CameraPath { ease: None, ..linear_path(Loop::Once) };
        assert!(p.eased());
        // Eased start moves slower than constant speed
        assert!(swept_about_y(&p, 0.0, 0.05, 32).abs() < 0.05);
        let p_closed = CameraPath { ease: None, ..linear_path(Loop::Closed) };
        assert!(!p_closed.eased());
    }

    #[test]
    fn degenerate_paths_are_safe() {
        let empty = CameraPath::new(vec![], Loop::Once);
        assert!(empty.sample(0.5, Fov::default()).distance > 0.0);

        let single = CameraPath::new(vec![key(1.0, 0.2, 2.0, Vec3::Z)], Loop::Closed);
        let cam = single.sample(0.7, Fov::default());
        assert!(cam.orientation.angle_to(single.keys[0].orientation) < 1e-6);
        assert!((cam.focus - Vec3::Z).length() < 1e-6);
    }

    #[test]
    fn an_exact_route_loops_in_place_about_the_axis_it_names() {
        // Between two *equal* framings there is no implied axis — every axis
        // has a whole-turn route back to where it started — so a winding has
        // nothing to be a winding of, and `turn_to` falls back to guessing
        // world Y. "Pitch two full turns and come back" came out as a yaw
        // loop. An exact route is the only way to say it, and this is it said.
        let tau = std::f32::consts::TAU;
        let here = key(0.0, 0.0, 3.0, Vec3::ZERO);
        let looped = |route: Route| CameraPath {
            routes: vec![route],
            ease: Some(false),
            ..CameraPath::new(vec![here, here], Loop::Once)
        };

        let pitched = looped(Route::Exact(Turn::about(Vec3::X, 2.0 * tau)));
        let c = pitched.sample(0.125, Fov::default());
        assert!(
            c.forward().y.abs() > 0.5,
            "an eighth of the way in, a pitch loop should be looking well off the \
             horizontal; forward is {:?}",
            c.forward()
        );
        assert!(
            (c.right() - Vec3::X).length() < 1e-5,
            "pitching about the camera's own X must leave right where it was: {:?}",
            c.right()
        );

        // And it comes back: the keys are still hit exactly.
        for &t in &[0.0f32, 1.0] {
            assert!(
                pitched.sample(t, Fov::default()).orientation.angle_to(here.orientation) < 1e-5,
                "the loop must start and end on its key"
            );
        }

        // The winding spelling of the same wish can only guess, and guesses
        // world Y — which is the bug this exists to route around.
        let guessed = looped(Route::Turns(2));
        assert!(
            guessed.sample(0.125, Fov::default()).forward().y.abs() < 1e-3,
            "a winding between equal framings has no axis to use but the fallback"
        );
    }

    #[test]
    fn routes_keep_pace_with_the_keys() {
        // The route list and the segment list must never drift apart, or a
        // route silently starts describing a different segment.
        let mut p = linear_path(Loop::Once);
        p.fit_routes();
        assert_eq!(p.routes.len(), p.segments());

        p.keys.push(key(3.0, 0.1, 2.0, Vec3::ZERO));
        p.fit_routes();
        assert_eq!(p.routes.len(), p.segments());

        p.loops = Loop::Closed;
        p.fit_routes();
        assert_eq!(p.routes.len(), p.segments(), "closing adds the wrap-around segment");

        p.keys.truncate(2);
        p.fit_routes();
        assert_eq!(p.routes.len(), p.segments());

        // A path that never mentions its routes takes the short way everywhere.
        assert_eq!(CameraPath::new(vec![], Loop::Once).route(7), Route::Turns(0));
    }
}

