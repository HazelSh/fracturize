//! Tracks: scene values over time, and the one clock they share with the
//! camera.
//!
//! FEATURE-PLANS.md item 9 and 10. A **track** is three things:
//!
//! - a **target**, which is an `-S` path (`transform.whorl.rotation.y`,
//!   `palette.rotate`, `subscene.smoke.share`). There is one address language
//!   for the command line, sweeps and animation, so anything `-S` can reach a
//!   track can animate, and `--sweep` and a track are the same question asked
//!   at different times;
//! - a **source** of values over time — [`Source::Keys`] (keyframes with an
//!   interpolation), [`Source::Rate`] (units per second, for things that just
//!   run) or [`Source::Cycle`] (an oscillator: sine, triangle, saw, square);
//! - for keys, what happens past the ends: [`Extrapolate`].
//!
//! # The camera is a track too: views at moments
//!
//! `[[track]] target = "camera"` is the camera's track. Its keys are *views* —
//! a framing (yaw/pitch/roll or `rotvec`, distance, focus) at a time `t` — and
//! its loop is an [`Extrapolate`] like any other track's: `hold`, `pingpong`,
//! `repeat` (with a `period`, the lap back round to the first key), or
//! `offset`, where the offset is the scene's zoom symmetry rather than a
//! number. That last one is the zoom loop, which stops being a special case.
//!
//! Framings are rotations, and rotations don't interpolate as numbers, so the
//! camera keeps its own spline: `path.rs`'s cumulative quaternion Catmull-Rom
//! with explicit routes, which is what lets a key sequence spiral through
//! several whole turns (winze) instead of taking the short way. Uneven key
//! times are honoured with per-second tangents. The track is resolved by the
//! scene loader, since a key inherits whatever its view leaves out from the
//! scene's `[camera]`, and is held as `Scene::camera_path` — the one track
//! every scene has, the default orbit when none is written.
//! [`Extrapolate::fold`] is the one time-folding function; `CameraPath::sample`
//! folds by the same rule.
//!
//! The old spelling — `[camera] path_loop / path_seconds / path_ease /
//! path_zoom_periods` with `[[camera.path]]` keypoints — still loads, its keys
//! placed when the old flight passed them, and saving writes the track.
//!
//! # Evaluating: the scene at time `t` is the scene with `-S` applied
//!
//! [`Animator`] holds the scene as TOML (with its `[[track]]` tables taken
//! out) and, for a time `t`, applies one `-S` per track and parses the result.
//! The animated scene is an ordinary `Scene`, so every renderer, `--info` and
//! the gizmos draw it without knowing it moved. Costs, measured on the desktop
//! by `an_animated_scene_is_cheap_to_build`: well under a millisecond for an
//! ordinary scene, which is why the live window can afford it every frame.
//!
//! **Rate and cycle tracks move *about* the authored value.** A `rate` starts
//! from what the file says (or `from`), a cycle swings around it (or
//! `centre`). So editing the base value in the inspector moves the whole
//! motion with it, which is the edit people mean; keys, by contrast, *are* the
//! value, and the authored one only shows where there is no track.
//!
//! # What a track costs depends on what it moves
//!
//! [`Class`] is the most important engineering fact here (plan §10):
//!
//! | class     | e.g.                                   | per frame          |
//! |-----------|----------------------------------------|--------------------|
//! | view      | point size, exposure, haze, background | nothing: re-render |
//! | colour    | palette, transform colours             | re-upload colours  |
//! | placement | subscene translation, rotation, scale  | points re-form     |
//! | structure | matrices, variations, weights, xaos    | the attractor moves|
//!
//! Live, anything past *view* runs into the point ring, which takes ~13 s to
//! turn over: the window churns at the fill rate while such a track plays and
//! accepts the grain. Offline, an animation refills the buffer every frame
//! when any track is past *view*, and says so.
//!
//! # Things that are refused, and why
//!
//! - **`camera.*` targets.** The camera's framing is the path; a scalar track
//!   on `camera.distance` would fight it every frame.
//! - **Two tracks on one target.** The second would silently win.
//! - **Interpolating non-numbers.** A string or boolean key (`palette.name`,
//!   `palette.reverse`) is a *step*; asking for `linear` on one is an error.
//! - **A rate or cycle on a key the file doesn't write**, without `from` /
//!   `centre`. Its implied default lives in the loader, not the text, and a
//!   motion around a guessed zero would be a confident wrong picture.
//!
//! The map set is fixed over an animation (no adding maps on the timeline),
//! which is what keeps every frame's GPU layout the same.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::scene::Scene;

/// What happens to a keyed track past its first and last keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Extrapolate {
    /// Hold the end values. The default, and a camera path's `once`.
    #[default]
    Hold,
    /// Play the keys again from the first, forever. A closed camera loop.
    Repeat,
    /// Forward, then backward, then forward. A ping-pong camera loop.
    PingPong,
    /// Repeat, each lap starting where the last one ended: Blender's "cycles
    /// with offset". A number climbs by `last − first` a lap; a camera's zoom
    /// loop is this with the zoom symmetry as the offset.
    Offset,
}

impl Extrapolate {
    pub const ALL: [Self; 4] = [Self::Hold, Self::Repeat, Self::PingPong, Self::Offset];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Repeat => "repeat",
            Self::PingPong => "pingpong",
            Self::Offset => "offset",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hold" | "once" => Some(Self::Hold),
            "repeat" | "loop" | "closed" => Some(Self::Repeat),
            "pingpong" | "ping-pong" => Some(Self::PingPong),
            "offset" | "repeat-offset" => Some(Self::Offset),
            _ => None,
        }
    }

    /// Fold time `t` into `[start, end]`, and say which lap it fell in.
    ///
    /// The one time-folding rule, shared by every keyed track and by the
    /// camera path (`CameraPath::sample`). A zero-length range has nowhere to
    /// fold to and returns `start`. The lap is `floor((t − start) / len)` for
    /// the repeating kinds and 0 for `Hold`, so `t` before the first key is
    /// lap −1 of a repeat — which is what makes `Offset` run backwards
    /// smoothly too.
    pub fn fold(self, t: f64, start: f64, end: f64) -> (f64, i64) {
        let len = end - start;
        if !(len > 0.0) || !t.is_finite() {
            return (start, 0);
        }
        let x = t - start;
        match self {
            Self::Hold => (t.clamp(start, end), 0),
            Self::Repeat | Self::Offset => {
                let lap = (x / len).floor();
                (start + (x - lap * len).clamp(0.0, len), lap as i64)
            }
            Self::PingPong => {
                let lap = (x / (2.0 * len)).floor();
                let r = x - lap * 2.0 * len;
                let r = if r <= len { r } else { 2.0 * len - r };
                (start + r.clamp(0.0, len), lap as i64)
            }
        }
    }
}

/// How the value runs between two keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Interp {
    /// The earlier key's value until the next key arrives.
    Step,
    Linear,
    /// Eased in and out of every key: velocity zero at each one. The default,
    /// because two keys is the common case and this is what it should look
    /// like; through several keys it pauses at each, which is `spline`'s job
    /// to avoid.
    #[default]
    Smooth,
    /// Catmull-Rom through the keys (non-uniform, so uneven key spacing
    /// doesn't kink it): passes every key without stopping.
    Spline,
}

impl Interp {
    pub const ALL: [Self; 4] = [Self::Step, Self::Linear, Self::Smooth, Self::Spline];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::Linear => "linear",
            Self::Smooth => "smooth",
            Self::Spline => "spline",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|i| i.as_str() == s)
    }
}

/// An oscillator's shape. Every one runs −1..1 over one period, starting at 0
/// going up (sine, triangle) or at its own natural start (saw at −1, square
/// at +1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Wave {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
}

impl Wave {
    pub const ALL: [Self; 4] = [Self::Sine, Self::Triangle, Self::Saw, Self::Square];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sine => "sine",
            Self::Triangle => "triangle",
            Self::Saw => "saw",
            Self::Square => "square",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.as_str() == s)
    }

    /// The wave at `x` periods.
    pub fn at(self, x: f64) -> f64 {
        let f = x - x.floor();
        match self {
            Self::Sine => (std::f64::consts::TAU * x).sin(),
            Self::Triangle => {
                // 0 → 1 → 0 → −1 → 0 over one period, in phase with the sine
                if f < 0.25 {
                    4.0 * f
                } else if f < 0.75 {
                    2.0 - 4.0 * f
                } else {
                    4.0 * f - 4.0
                }
            }
            Self::Saw => 2.0 * f - 1.0,
            Self::Square => {
                if f < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
}

/// A key's value: a number, a whole rotation, or anything else TOML can say
/// (a palette name, a boolean), which only ever steps.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyValue {
    Number(f64),
    Rotation(RotKey),
    Other(toml::Value),
}

/// A map's whole rotation at a key: a track on `transform.<map>.rotation`
/// (or a subscene's placement rotation) keys poses, not one Euler angle.
///
/// Poses are interpolated as rotations — the camera's routes and its
/// cumulative quaternion spline (`path.rs`) — so a turn never takes the wrong
/// way round and never shears. The *route* from each key to the next is data,
/// as it is for the camera: written as an Euler triple, a key that changes
/// one angle from the last turns by exactly that many degrees about that
/// angle's axis, so `y` from 0 to 720 is two whole turns; otherwise the short
/// way plus `turns`, or `route`, the journey as a rotation vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotKey {
    pub orientation: crate::rot::Orientation,
    /// The Euler triple (degrees, XYZ, as the scene writes `rotation`) when
    /// the key was written as one; it is what carries the route.
    pub euler: Option<[f64; 3]>,
    /// Extra whole turns on the way out of this key, beyond the short way.
    pub turns: Option<i32>,
    /// The way out of this key exactly, in its own frame (radians).
    pub route: Option<glam::Vec3>,
}

impl RotKey {
    pub fn from_euler(euler: [f64; 3]) -> Self {
        let r = euler.map(|d| (d as f32).to_radians());
        let q = glam::Quat::from_euler(glam::EulerRot::XYZ, r[0], r[1], r[2]);
        Self {
            orientation: crate::rot::Orientation::from_mat3(&glam::Mat3::from_quat(q)),
            euler: Some(euler),
            turns: None,
            route: None,
        }
    }

    pub fn from_rotvec(v: glam::Vec3) -> Self {
        Self {
            orientation: crate::rot::Turn::from_rotation_vector(v).exp(),
            euler: None,
            turns: None,
            route: None,
        }
    }

    /// The rotation as `-S …rotvec` takes it.
    fn rotvec(&self) -> glam::Vec3 {
        crate::rot::Orientation::IDENTITY.shortest_turn_to(self.orientation).as_rotation_vector()
    }

    /// The journey from this key to `next`, in this key's frame.
    pub fn route_to(&self, next: &RotKey) -> crate::rot::Turn {
        use crate::rot::Turn;
        if let Some(v) = self.route {
            return Turn::from_rotation_vector(v);
        }
        if let Some(n) = self.turns {
            return self.orientation.turn_to(next.orientation, n);
        }
        if let (Some(a), Some(b)) = (self.euler, next.euler) {
            let changed: Vec<usize> = (0..3).filter(|&c| (a[c] - b[c]).abs() > 1e-9).collect();
            if let [c] = changed[..] {
                // R = Rx·Ry·Rz: x turns about world X, y about Rx·Y, z about
                // the map's own Z — each a single axis, through the change.
                let rx = glam::Quat::from_rotation_x((a[0] as f32).to_radians());
                let world = match c {
                    0 => glam::Vec3::X,
                    1 => rx * glam::Vec3::Y,
                    _ => self.orientation.rotate(glam::Vec3::Z),
                };
                let body = self.orientation.inverse().rotate(world);
                return Turn::about(body, ((b[c] - a[c]) as f32).to_radians());
            }
        }
        self.orientation.shortest_turn_to(next.orientation)
    }
}

/// Whether a track on `target` may key whole rotations: a map's or a
/// subscene's `rotation`, the ones a `rotvec` can stand in for.
pub fn takes_rotations(target: &str) -> bool {
    target.ends_with(".rotation") && (target.starts_with("transform.") || target.starts_with("subscene."))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    /// Seconds
    pub t: f64,
    pub value: KeyValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Keys {
        keys: Vec<Key>,
        interpolate: Interp,
        extrapolate: Extrapolate,
        /// Every key was written as an integer, so every value is written back
        /// as one — `zoom.levels` is a count, and the loader won't take 15.3.
        integral: bool,
    },
    /// `from + rate · t`; `from` defaults to the authored value.
    Rate { rate: f64, from: Option<f64> },
    /// `centre + amplitude · wave(t / period + phase)`; `centre` defaults to
    /// the authored value. `phase` is in periods, 0–1.
    Cycle {
        wave: Wave,
        period: f64,
        amplitude: f64,
        phase: f64,
        centre: Option<f64>,
    },
}

impl Source {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Keys { .. } => "keys",
            Self::Rate { .. } => "rate",
            Self::Cycle { .. } => "cycle",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// An `-S` path
    pub target: String,
    pub source: Source,
    /// Kept in the file and drawn in the timeline, but not applied.
    pub mute: bool,
}

/// What a track moves, and so what a frame of it costs. See the module doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    View,
    Colour,
    Placement,
    Structure,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Colour => "colour",
            Self::Placement => "placement",
            Self::Structure => "structure",
        }
    }

    /// Whether the points already drawn stop being right when it changes.
    pub fn refills(self) -> bool {
        matches!(self, Self::Placement | Self::Structure)
    }
}

/// Which [`Class`] an `-S` path is in.
pub fn class_of(target: &str) -> Class {
    let segs: Vec<&str> = target.split('.').collect();
    let field = |i: usize| segs.get(i).copied().unwrap_or("");
    match field(0) {
        "meta" => match field(1) {
            "point_size" | "exposure" | "haze" | "fog" | "background" => Class::View,
            "color_speed" | "color_falloff" | "color_contrast" | "color_mode" => Class::Colour,
            _ => Class::Structure,
        },
        "palette" => Class::Colour,
        "transform" => match field(2) {
            "color" | "color_value" | "color_speed" => Class::Colour,
            _ => Class::Structure,
        },
        "subscene" => match field(2) {
            "translation" | "rotation" | "rotvec" | "scale" => Class::Placement,
            "color_mode" | "color_speed" | "color_falloff" | "color_contrast" | "palette" => {
                Class::Colour
            }
            _ => Class::Structure,
        },
        _ => Class::Structure,
    }
}

impl Track {
    pub fn class(&self) -> Class {
        class_of(&self.target)
    }

    /// Where the keys start and end, for a keyed track.
    pub fn key_span(&self) -> Option<(f64, f64)> {
        match &self.source {
            Source::Keys { keys, .. } => Some((keys.first()?.t, keys.last()?.t)),
            _ => None,
        }
    }

    /// How long the track takes to say what it has to say: to its last key,
    /// or one period. A rate never finishes and says nothing.
    pub fn natural_length(&self) -> Option<f64> {
        match &self.source {
            Source::Keys { keys, .. } => keys.last().map(|k| k.t),
            Source::Cycle { period, .. } => Some(*period),
            Source::Rate { .. } => None,
        }
    }

    /// The value at `t`, given the authored value `base` (what rate and cycle
    /// tracks move about).
    pub fn sample(&self, t: f64, base: Option<f64>) -> KeyValue {
        match &self.source {
            Source::Keys { keys, interpolate, extrapolate, .. } => {
                sample_keys(keys, *interpolate, *extrapolate, t)
            }
            Source::Rate { rate, from } => {
                KeyValue::Number(from.or(base).unwrap_or(0.0) + rate * t)
            }
            Source::Cycle { wave, period, amplitude, phase, centre } => KeyValue::Number(
                centre.or(base).unwrap_or(0.0) + amplitude * wave.at(t / period + phase),
            ),
        }
    }

    /// The value at `t` as the `-S` spec that applies it, brought into the
    /// target's range (`params::range_of`): a spline overshoots and a cycle
    /// swings past zero, and a negative xaos multiplier or share is refused
    /// by the loader, a negative weight or point size drawn as nonsense.
    pub fn spec_at(&self, t: f64, base: Option<f64>) -> String {
        let integral = matches!(self.source, Source::Keys { integral: true, .. });
        let value = match self.sample(t, base) {
            KeyValue::Number(x) => KeyValue::Number(crate::params::clamp(&self.target, x)),
            // A pose is written as the exact form, which wins over the Euler
            // triple: no branch to fall onto, whatever the pose.
            KeyValue::Rotation(r) => {
                let v = r.rotvec();
                let stem = self.target.strip_suffix(".rotation").unwrap_or(&self.target);
                let r = |x: f32| (x as f64 * 1e6).round() / 1e6 + 0.0;
                return format!("{stem}.rotvec=[{:?}, {:?}, {:?}]", r(v.x), r(v.y), r(v.z));
            }
            other => other,
        };
        format!("{}={}", self.target, value_text(&value, integral))
    }
}

/// TOML text for a value, written so the loader reads back the same type:
/// a float always carries its point (`90.0`, not `90`, which is an integer).
pub fn value_text(v: &KeyValue, integral: bool) -> String {
    match v {
        KeyValue::Number(x) if integral => format!("{}", x.round() as i64),
        KeyValue::Number(x) if x.is_finite() => format!("{:?}", x),
        KeyValue::Number(_) => "0.0".to_string(),
        KeyValue::Rotation(r) => {
            let e = r.euler.unwrap_or_else(|| euler_of(r.orientation, None));
            format!("[{:?}, {:?}, {:?}]", e[0], e[1], e[2])
        }
        KeyValue::Other(v) => v.to_string(),
    }
}

fn sample_keys(keys: &[Key], interp: Interp, ex: Extrapolate, t: f64) -> KeyValue {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return KeyValue::Number(0.0);
    };
    if keys.len() == 1 {
        return first.value.clone();
    }
    let (local, lap) = ex.fold(t, first.t, last.t);
    // The segment `local` is in: the last key at or before it.
    let i = keys.partition_point(|k| k.t <= local).clamp(1, keys.len() - 1) - 1;
    let (a, b) = (&keys[i], &keys[i + 1]);
    let u = ((local - a.t) / (b.t - a.t)).clamp(0.0, 1.0);

    if let (KeyValue::Rotation(ra), KeyValue::Rotation(rb)) = (&a.value, &b.value) {
        return KeyValue::Rotation(sample_rotation(keys, interp, ex, (local, lap), u, ra, rb));
    }
    let (KeyValue::Number(pa), KeyValue::Number(pb)) = (&a.value, &b.value) else {
        // Not numbers: a step, and the last key holds from its own time on.
        return if u >= 1.0 { b.value.clone() } else { a.value.clone() };
    };
    let (pa, pb) = (*pa, *pb);
    let v = match interp {
        Interp::Step => {
            if u >= 1.0 {
                pb
            } else {
                pa
            }
        }
        Interp::Linear => pa + (pb - pa) * u,
        Interp::Smooth => pa + (pb - pa) * u * u * (3.0 - 2.0 * u),
        Interp::Spline => {
            let num = |k: &Key| match k.value {
                KeyValue::Number(x) => x,
                _ => 0.0,
            };
            // Non-uniform Catmull-Rom as a cubic Hermite: each key's tangent
            // is the slope across its neighbours, one-sided at the ends.
            let slope = |j: usize| -> f64 {
                let (l, r) = (j.saturating_sub(1), (j + 1).min(keys.len() - 1));
                (num(&keys[r]) - num(&keys[l])) / (keys[r].t - keys[l].t)
            };
            let dt = b.t - a.t;
            let (u2, u3) = (u * u, u * u * u);
            (2.0 * u3 - 3.0 * u2 + 1.0) * pa
                + (u3 - 2.0 * u2 + u) * dt * slope(i)
                + (-2.0 * u3 + 3.0 * u2) * pb
                + (u3 - u2) * dt * slope(i + 1)
        }
    };
    let offset = match (ex, &first.value, &last.value) {
        (Extrapolate::Offset, KeyValue::Number(f), KeyValue::Number(l)) => lap as f64 * (l - f),
        _ => 0.0,
    };
    KeyValue::Number(v + offset)
}

/// The pose between rotation keys `i` and `i + 1`, a fraction `u` of the way.
///
/// Step holds, linear turns at a constant rate along the route, smooth eases
/// in and out of every key, and spline is the camera's cumulative quaternion
/// Catmull-Rom through all the keys at their times. `offset` carries the
/// first-to-last turn into each lap, so a map keyed through a quarter turn
/// keeps turning.
#[allow(clippy::too_many_arguments)]
fn sample_rotation(
    keys: &[Key],
    interp: Interp,
    ex: Extrapolate,
    (local, lap): (f64, i64),
    u: f64,
    a: &RotKey,
    b: &RotKey,
) -> RotKey {
    let rot = |k: &Key| match &k.value {
        KeyValue::Rotation(r) => *r,
        _ => RotKey::from_rotvec(glam::Vec3::ZERO),
    };
    let w = match interp {
        Interp::Step => if u >= 1.0 { 1.0 } else { 0.0 },
        Interp::Linear => u,
        Interp::Smooth => u * u * (3.0 - 2.0 * u),
        Interp::Spline => -1.0,
    };
    let route = a.route_to(b);
    let orientation = if w >= 0.0 {
        a.orientation.then_body(route * w as f32)
    } else {
        let mut path = crate::path::CameraPath::new(
            keys.iter()
                .map(|k| crate::path::PathKey { orientation: rot(k).orientation, distance: 1.0, focus: glam::Vec3::ZERO })
                .collect(),
            crate::path::Loop::Once,
        );
        path.times = keys.iter().map(|k| k.t as f32).collect();
        path.routes = keys.windows(2).map(|p| crate::path::Route::Exact(rot(&p[0]).route_to(&rot(&p[1])))).collect();
        path.ease = Some(false);
        path.sample_at(local, crate::camera::Fov::default()).orientation
    };
    let orientation = match ex {
        Extrapolate::Offset if lap != 0 => {
            let (first, last) = (rot(&keys[0]).orientation, rot(&keys[keys.len() - 1]).orientation);
            let step = first.inverse().then(last).as_quat().normalize();
            let power = crate::rot::Orientation::IDENTITY.shortest_turn_to(crate::rot::Orientation::from_mat3(&glam::Mat3::from_quat(step)));
            // Laps compose in the first key's frame: first · Δ^lap · first⁻¹.
            let world = crate::rot::Turn::from_rotation_vector(first.rotate(power.as_rotation_vector()) * lap as f32);
            orientation.then_world(world)
        }
        _ => orientation,
    };
    // A pose sampled on a one-angle segment is that angle, interpolated: the
    // same rotation, and the triple that keeps the route if it is keyed.
    let euler = match (a.euler, b.euler, w >= 0.0, ex) {
        (Some(ea), Some(eb), true, Extrapolate::Hold | Extrapolate::Repeat | Extrapolate::PingPong) => {
            Some([0, 1, 2].map(|c| ea[c] + (eb[c] - ea[c]) * w))
        }
        _ => Some(euler_of(orientation, a.euler)),
    };
    RotKey { orientation, euler, turns: None, route: None }
}

/// An orientation as the scene's Euler triple (degrees, XYZ), each angle
/// taken as near `near`'s as whole turns allow.
pub fn euler_of(o: crate::rot::Orientation, near: Option<[f64; 3]>) -> [f64; 3] {
    let (x, y, z) = o.as_quat().to_euler(glam::EulerRot::XYZ);
    let e = [x, y, z].map(|r| (r as f64).to_degrees());
    match near {
        Some(n) => [0, 1, 2].map(|c| e[c] + 360.0 * ((n[c] - e[c]) / 360.0).round()),
        None => e.map(|d| (d * 1e4).round() / 1e4),
    }
}

// ---------------------------------------------------------------- the file

/// One key as the file writes it: `[seconds, value]` for a number (or a
/// string or boolean), or a table for the camera — a view at a moment,
/// `{ t = 2.0, yaw = 1.2, pitch = 0.3, distance = 4.0, focus = [...] }`,
/// which reads best as its own `[[track.keys]]` section.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum KeyDef {
    Pair(Vec<toml::Value>),
    View(crate::scene::PathKeyDef),
    Turn(RotKeyDef),
    /// A table as read, before the target says which kind it is.
    Table(toml::Table),
}

/// A rotation key as the file writes it: `t`, then `rotation` (Euler degrees,
/// XYZ, as a map writes it) or `rotvec` (radians), and the route out of it —
/// `turns` or `route` — where the triples don't already say it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RotKeyDef {
    pub t: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotvec: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<[f64; 3]>,
}

impl<'de> Deserialize<'de> for KeyDef {
    /// By shape, so a mistyped field in a view says which, rather than
    /// "matched no variant".
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match toml::Value::deserialize(d)? {
            toml::Value::Array(a) => Ok(Self::Pair(a)),
            toml::Value::Table(t) => Ok(Self::Table(t)),
            other => Ok(Self::Pair(vec![other])),
        }
    }
}

/// The camera's track target: not an `-S` path, since a framing isn't one
/// number.
pub const CAMERA: &str = "camera";

/// A `[[track]]` table.
///
/// Exactly one of `keys`, `rate` and the cycle fields (`wave`, `period`,
/// `amplitude`) says which source it is. The camera's (`target = "camera"`)
/// is always keys, each a view, and carries `period`, `ease` and
/// `zoom_periods` instead; `scene.rs` resolves it, since its keys inherit the
/// scene's own framing.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TrackDef {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpolate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extrapolate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wave: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amplitude: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub centre: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mute: bool,
    /// Camera only: smoothstep each lap (default: on for hold and pingpong,
    /// off for the loops, whose seam it would stall)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<bool>,
    /// Camera only, with `extrapolate = "offset"`: zoom periods descended a
    /// lap (default 1)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom_periods: Option<u32>,
    /// `[[seconds, value], ...]`, or the camera's views. Last, because the
    /// camera's are sections and TOML wants a table's values before them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<KeyDef>>,
}

/// A TOML number as `f64`, whichever way it was written: `rate = 1` is as good
/// as `rate = 1.0`, and a hand-written file has no reason to know the
/// difference.
fn number(v: &toml::Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
}

impl TrackDef {
    pub fn is_camera(&self) -> bool {
        self.target.trim() == CAMERA
    }

    /// The camera track's loop, from `extrapolate`: hold, pingpong, repeat,
    /// or offset — which for a framing is the zoom loop, its offset being the
    /// scene's scale symmetry. With the zoom periods a lap.
    pub fn camera_loop(&self) -> Result<(crate::path::LoopKind, u32), String> {
        use crate::path::LoopKind;
        let who = "the camera track";
        if self.rate.is_some()
            || self.from.is_some()
            || self.wave.is_some()
            || self.amplitude.is_some()
            || self.phase.is_some()
            || self.centre.is_some()
            || self.interpolate.is_some()
        {
            return Err(format!(
                "{who}: its keys are views, interpolated as framings — `rate`, cycles and \
                 `interpolate` are for number tracks"
            ));
        }
        let kind = match self.extrapolate.as_deref() {
            None => LoopKind::Once,
            Some("zoom") => LoopKind::Zoom,
            Some(s) => match Extrapolate::parse(s) {
                Some(Extrapolate::Hold) => LoopKind::Once,
                Some(Extrapolate::PingPong) => LoopKind::PingPong,
                Some(Extrapolate::Repeat) => LoopKind::Closed,
                Some(Extrapolate::Offset) => LoopKind::Zoom,
                None => {
                    return Err(format!(
                        "{who}: extrapolate '{s}' is not one of hold, pingpong, repeat, \
                         offset (the zoom loop)"
                    ));
                }
            },
        };
        if self.zoom_periods.is_some() && kind != LoopKind::Zoom {
            return Err(format!("{who}: `zoom_periods` is for extrapolate = \"offset\""));
        }
        Ok((kind, self.zoom_periods.unwrap_or(1).clamp(1, 64)))
    }

    /// The camera track's keys, each a view with its time.
    pub fn camera_keys(&self) -> Result<Vec<crate::scene::PathKeyDef>, String> {
        let who = "the camera track";
        let mut out = Vec::new();
        for (i, k) in self.keys.iter().flatten().enumerate() {
            let v = match k {
                KeyDef::View(v) => *v,
                KeyDef::Table(t) => toml::Value::Table(t.clone())
                    .try_into::<crate::scene::PathKeyDef>()
                    .map_err(|e| format!("{who}: key {}: {}", i + 1, e.to_string().trim()))?,
                _ => {
                    return Err(format!(
                        "{who}: key {} is a [seconds, value] pair; the camera's keys are views \
                         — `{{ t = 0.0, yaw = ..., pitch = ..., distance = ... }}`",
                        i + 1
                    ));
                }
            };
            let v = &v;
            let t = v.t.filter(|t| t.is_finite()).ok_or_else(|| {
                format!("{who}: key {} has no `t` — when, in seconds, the camera is there", i + 1)
            })?;
            if let Some(prev) = out.last().and_then(|p: &crate::scene::PathKeyDef| p.t) {
                if t <= prev {
                    return Err(format!(
                        "{who}: keys must be in time order, each later than the last \
                         ({t} comes after {prev})"
                    ));
                }
            }
            out.push(*v);
        }
        if out.is_empty() {
            return Err(format!("{who}: no keys"));
        }
        Ok(out)
    }

    /// The camera track's lap, for the loops that close: seconds from its
    /// first key round to arriving back there.
    pub fn camera_period(&self) -> Result<Option<f32>, String> {
        let (kind, _) = self.camera_loop()?;
        let period = self.num(&self.period, "period")?;
        let closes = matches!(kind, crate::path::LoopKind::Closed | crate::path::LoopKind::Zoom);
        match period {
            Some(_) if !closes => Err(
                "the camera track: `period` is the lap of a loop that comes back round \
                 (repeat, offset); a hold or pingpong track's lap is its keys"
                    .to_string(),
            ),
            Some(p) => {
                let keys = self.camera_keys()?;
                let span = keys.last().and_then(|k| k.t).unwrap_or(0.0)
                    - keys.first().and_then(|k| k.t).unwrap_or(0.0);
                if p <= span {
                    return Err(format!(
                        "the camera track: `period` {p} must be longer than its keys' span \
                         ({span}): the loop still has to come back round from the last key"
                    ));
                }
                Ok(Some(p as f32))
            }
            None => Ok(None),
        }
    }

    fn num(&self, v: &Option<toml::Value>, key: &str) -> Result<Option<f64>, String> {
        match v {
            None => Ok(None),
            Some(v) => match number(v) {
                Some(x) if x.is_finite() => Ok(Some(x)),
                _ => Err(format!(
                    "track '{}': `{key}` must be a number, not {}",
                    self.target,
                    v.to_string().trim()
                )),
            },
        }
    }

    pub fn resolve(&self) -> Result<Track, String> {
        let target = self.target.trim().to_string();
        let who = format!("track '{target}'");
        if target == CAMERA {
            return Err(format!("{who}: the camera's track is resolved with the scene"));
        }
        crate::set::validate_path(&target).map_err(|e| format!("{who}: {e}"))?;
        if target.starts_with("camera.") {
            return Err(format!(
                "{who}: the camera is animated by its own track, `target = \"camera\"`, whose \
                 keys are whole views — a track on one of its numbers would fight it \
                 every frame"
            ));
        }
        if self.ease.is_some() || self.zoom_periods.is_some() {
            return Err(format!("{who}: `ease` and `zoom_periods` are the camera track's"));
        }

        let is_keys = self.keys.is_some();
        let is_rate = self.rate.is_some() || self.from.is_some();
        let is_cycle = self.wave.is_some()
            || self.period.is_some()
            || self.amplitude.is_some()
            || self.phase.is_some()
            || self.centre.is_some();
        let kinds = [is_keys, is_rate, is_cycle].iter().filter(|b| **b).count();
        if kinds != 1 {
            return Err(format!(
                "{who}: give exactly one source — `keys = [[t, v], ...]`, `rate = <per \
                 second>`, or a cycle (`wave`, `period`, `amplitude`){}",
                if kinds == 0 { "" } else { "; this one mixes them" }
            ));
        }
        if !is_keys && (self.interpolate.is_some() || self.extrapolate.is_some()) {
            return Err(format!(
                "{who}: `interpolate` and `extrapolate` are for keys; a rate or a cycle \
                 runs forever on its own"
            ));
        }

        let tables = self.keys.iter().flatten().filter(|k| !matches!(k, KeyDef::Pair(_))).count();
        let source = if let Some(raw) = self.keys.as_ref().filter(|_| tables > 0) {
            if tables != raw.len() {
                return Err(format!("{who}: keys mix [seconds, value] pairs with tables"));
            }
            if !takes_rotations(&target) {
                return Err(format!(
                    "{who}: keys written as tables are views (the camera's) or whole \
                     rotations (a map's `rotation`); for this one write [seconds, value]"
                ));
            }
            let mut keys: Vec<Key> = Vec::with_capacity(raw.len());
            for (n, k) in raw.iter().enumerate() {
                let d: RotKeyDef = match k {
                    KeyDef::Turn(d) => *d,
                    KeyDef::Table(t) => toml::Value::Table(t.clone())
                        .try_into()
                        .map_err(|e: toml::de::Error| format!("{who}: key {}: {}", n + 1, e.to_string().trim()))?,
                    _ => unreachable!("counted as a table"),
                };
                if !d.t.is_finite() || keys.last().is_some_and(|p| d.t <= p.t) {
                    return Err(format!("{who}: keys must be in time order, each later than the last"));
                }
                let mut r = match (d.rotation, d.rotvec) {
                    (Some(e), None) => RotKey::from_euler(e),
                    (None, Some(v)) => RotKey::from_rotvec(glam::Vec3::from(v.map(|x| x as f32))),
                    _ => {
                        return Err(format!(
                            "{who}: key {} needs one of `rotation = [x, y, z]` (degrees) or \
                             `rotvec = [x, y, z]` (radians)",
                            n + 1
                        ));
                    }
                };
                r.turns = d.turns;
                r.route = d.route.map(|v| glam::Vec3::from(v.map(|x| x as f32)));
                keys.push(Key { t: d.t, value: KeyValue::Rotation(r) });
            }
            let interpolate = match &self.interpolate {
                None => Interp::default(),
                Some(s) => Interp::parse(s).ok_or_else(|| {
                    format!("{who}: interpolate '{s}' is not one of step, linear, smooth, spline")
                })?,
            };
            let extrapolate = match &self.extrapolate {
                None => Extrapolate::default(),
                Some(s) => Extrapolate::parse(s).ok_or_else(|| {
                    format!("{who}: extrapolate '{s}' is not one of hold, repeat, pingpong, offset")
                })?,
            };
            Source::Keys { keys, interpolate, extrapolate, integral: false }
        } else if let Some(raw) = &self.keys {
            let mut keys = Vec::with_capacity(raw.len());
            for pair in raw {
                let KeyDef::Pair(pair) = pair else {
                    return Err(format!(
                        "{who}: each key is [seconds, value]; a table of fields is a camera key"
                    ));
                };
                let [t, v] = pair.as_slice() else {
                    return Err(format!(
                        "{who}: each key is [seconds, value], not {}",
                        toml::Value::Array(pair.clone()).to_string()
                    ));
                };
                let t = number(t).filter(|t| t.is_finite()).ok_or_else(|| {
                    format!("{who}: a key's time must be a number of seconds, not {t}")
                })?;
                if let Some(prev) = keys.last().map(|k: &Key| k.t) {
                    if t <= prev {
                        return Err(format!(
                            "{who}: keys must be in time order, each later than the last \
                             ({t} comes after {prev})"
                        ));
                    }
                }
                let value = match number(v) {
                    Some(x) if x.is_finite() => KeyValue::Number(x),
                    Some(_) => return Err(format!("{who}: a key's value must be finite")),
                    None => match v {
                        toml::Value::Array(_) | toml::Value::Table(_) => {
                            return Err(format!(
                                "{who}: a key's value is one number (or one string or \
                                 boolean); animate an array one element at a time, \
                                 e.g. {target}.x"
                            ));
                        }
                        other => KeyValue::Other(other.clone()),
                    },
                };
                keys.push(Key { t, value });
            }
            if keys.is_empty() {
                return Err(format!("{who}: `keys` is empty"));
            }
            let numeric = keys.iter().filter(|k| matches!(k.value, KeyValue::Number(_))).count();
            if numeric != 0 && numeric != keys.len() {
                return Err(format!("{who}: keys mix numbers with other values"));
            }
            let numeric = numeric != 0;
            let interpolate = match &self.interpolate {
                None if numeric => Interp::default(),
                None => Interp::Step,
                Some(s) => Interp::parse(s).ok_or_else(|| {
                    format!("{who}: interpolate '{s}' is not one of step, linear, smooth, spline")
                })?,
            };
            if !numeric && interpolate != Interp::Step {
                return Err(format!(
                    "{who}: only numbers interpolate; keys that are strings or booleans \
                     step from one to the next (interpolate = \"step\")"
                ));
            }
            let extrapolate = match &self.extrapolate {
                None => Extrapolate::default(),
                Some(s) => Extrapolate::parse(s).ok_or_else(|| {
                    format!(
                        "{who}: extrapolate '{s}' is not one of hold, repeat, pingpong, offset"
                    )
                })?,
            };
            if !numeric && extrapolate == Extrapolate::Offset {
                return Err(format!("{who}: offset adds a number each lap; these keys aren't numbers"));
            }
            let integral = raw.iter().all(|p| {
                matches!(p, KeyDef::Pair(p) if p.get(1).is_some_and(|v| v.as_integer().is_some()))
            });
            Source::Keys { keys, interpolate, extrapolate, integral }
        } else if is_rate {
            let rate = self.num(&self.rate, "rate")?.ok_or_else(|| {
                format!("{who}: `from` needs a `rate` to run at")
            })?;
            Source::Rate { rate, from: self.num(&self.from, "from")? }
        } else {
            let period = self.num(&self.period, "period")?.ok_or_else(|| {
                format!("{who}: a cycle needs a `period`, in seconds")
            })?;
            if period <= 0.0 {
                return Err(format!("{who}: `period` must be more than zero seconds"));
            }
            let amplitude = self.num(&self.amplitude, "amplitude")?.ok_or_else(|| {
                format!("{who}: a cycle needs an `amplitude` — how far it swings each way")
            })?;
            let wave = match &self.wave {
                None => Wave::default(),
                Some(s) => Wave::parse(s).ok_or_else(|| {
                    format!("{who}: wave '{s}' is not one of sine, triangle, saw, square")
                })?,
            };
            Source::Cycle {
                wave,
                period,
                amplitude,
                phase: self.num(&self.phase, "phase")?.unwrap_or(0.0),
                centre: self.num(&self.centre, "centre")?,
            }
        };
        Ok(Track { target, source, mute: self.mute })
    }
}

impl Track {
    /// The file form. Writes only what differs from the defaults, so a track
    /// written by hand comes back the way it was written.
    pub fn to_def(&self) -> TrackDef {
        let f = |x: f64| Some(toml::Value::Float(x));
        let mut def = TrackDef { target: self.target.clone(), mute: self.mute, ..Default::default() };
        match &self.source {
            Source::Keys { keys, interpolate, extrapolate, integral } => {
                def.keys = Some(
                    keys.iter()
                        .map(|k| {
                            if let KeyValue::Rotation(r) = &k.value {
                                let rv = r.rotvec();
                                return KeyDef::Turn(RotKeyDef {
                                    t: k.t,
                                    rotation: r.euler,
                                    rotvec: r.euler.is_none().then(|| rv.to_array().map(|x| x as f64)),
                                    turns: r.turns,
                                    route: r.route.map(|v| v.to_array().map(|x| x as f64)),
                                });
                            }
                            let v = match &k.value {
                                KeyValue::Number(x) if *integral => toml::Value::Integer(x.round() as i64),
                                KeyValue::Number(x) => toml::Value::Float(*x),
                                KeyValue::Other(v) => v.clone(),
                                KeyValue::Rotation(_) => unreachable!("written above"),
                            };
                            KeyDef::Pair(vec![toml::Value::Float(k.t), v])
                        })
                        .collect(),
                );
                let numeric = keys.iter().all(|k| matches!(k.value, KeyValue::Number(_) | KeyValue::Rotation(_)));
                let default_interp = if numeric { Interp::default() } else { Interp::Step };
                if *interpolate != default_interp {
                    def.interpolate = Some(interpolate.as_str().to_string());
                }
                if *extrapolate != Extrapolate::default() {
                    def.extrapolate = Some(extrapolate.as_str().to_string());
                }
            }
            Source::Rate { rate, from } => {
                def.rate = f(*rate);
                def.from = from.and_then(f);
            }
            Source::Cycle { wave, period, amplitude, phase, centre } => {
                if *wave != Wave::default() {
                    def.wave = Some(wave.as_str().to_string());
                }
                def.period = f(*period);
                def.amplitude = f(*amplitude);
                if *phase != 0.0 {
                    def.phase = f(*phase);
                }
                def.centre = centre.and_then(f);
            }
        }
        def
    }
}

/// Resolve a scene file's `[[track]]` tables, refusing two on one target.
///
/// The camera's track is left out: it is a framing, resolved with the scene's
/// camera (`Scene::camera_path`).
pub fn resolve_all(defs: &[TrackDef]) -> Result<Vec<Track>, String> {
    let tracks: Vec<Track> =
        defs.iter().filter(|d| !d.is_camera()).map(TrackDef::resolve).collect::<Result<_, _>>()?;
    for (i, a) in tracks.iter().enumerate() {
        if let Some(b) = tracks.iter().find(|b| takes_rotations(&a.target) && b.target.starts_with(&format!("{}.", a.target))) {
            return Err(format!(
                "'{}' turns the whole rotation and '{}' one angle of it; the whole one wins, \
                 so the other would do nothing — key the angle into the whole rotation",
                a.target, b.target
            ));
        }
        if tracks[..i].iter().any(|b| b.target == a.target) {
            return Err(format!(
                "two tracks animate '{}'; the second would silently win — merge their keys",
                a.target
            ));
        }
    }
    Ok(tracks)
}

/// Point every track that names map `old` at `new` instead — what renaming a
/// map has to do, or its tracks are left pointing at a name that is gone.
/// Covers the map's own fields (`transform.old.weight`) and other maps' xaos
/// toward it (`transform.x.xaos.old`).
pub fn rename_map(tracks: &mut [Track], old: &str, new: &str) {
    for t in tracks {
        let mut segs: Vec<String> = t.target.split('.').map(str::to_string).collect();
        if segs.first().map(String::as_str) != Some("transform") {
            continue;
        }
        if segs.get(1).map(String::as_str) == Some(old) {
            segs[1] = new.to_string();
        }
        if matches!(segs.get(2).map(String::as_str), Some("xaos" | "xaos_p"))
            && segs.get(3).map(String::as_str) == Some(old)
        {
            segs[3] = new.to_string();
        }
        t.target = segs.join(".");
    }
}

/// What `scene` says at `target` right now, as it would be saved: the value a
/// new track's first key starts from. `None` when the scene doesn't write that
/// key down. An error when the path doesn't resolve on this scene.
pub fn authored_value(scene: &Scene, target: &str) -> Result<Option<toml::Value>, String> {
    let text = scene.to_toml_string()?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|e| format!("scene is not valid TOML: {e}"))?;
    let applied = crate::set::apply_to_doc(&mut doc, &format!("{target}=0.0"))
        .map_err(|e| e.strip_prefix("--set ").unwrap_or(&e).to_string())?;
    Ok(applied
        .from
        .and_then(|f| toml::from_str::<toml::Table>(&format!("v = {f}")).ok())
        .and_then(|t| t.get("v").cloned()))
}

/// A new keyed track on `target` with one key, `value` at `t`: what the
/// Timeline's "add" makes. Numbers interpolate smoothly, anything else steps.
pub fn one_key(target: &str, t: f64, value: toml::Value) -> Result<Track, String> {
    if takes_rotations(target) {
        if let Some(a) = value.as_array().filter(|a| a.len() == 3) {
            let e: Vec<f64> = a.iter().filter_map(number).collect();
            if let [x, y, z] = e[..] {
                return TrackDef {
                    target: target.to_string(),
                    keys: Some(vec![KeyDef::Turn(RotKeyDef { t, rotation: Some([x, y, z]), ..Default::default() })]),
                    ..Default::default()
                }
                .resolve();
            }
        }
    }
    let integral = value.as_integer().is_some();
    TrackDef {
        target: target.to_string(),
        keys: Some(vec![KeyDef::Pair(vec![toml::Value::Float(t), value])]),
        ..Default::default()
    }
    .resolve()
    .map(|mut track| {
        if let Source::Keys { integral: i, .. } = &mut track.source {
            *i = integral;
        }
        track
    })
}

// ---------------------------------------------------------------- timeline

/// How long a scene's tracks take to play out: the latest last key, else the
/// longest cycle's period. `None` for a scene with no tracks, or only rates.
pub fn tracks_length(tracks: &[Track]) -> Option<f64> {
    let keyed = tracks.iter().filter_map(Track::key_span).map(|(_, e)| e).fold(None, max_opt);
    keyed.or_else(|| {
        tracks
            .iter()
            .filter(|t| matches!(t.source, Source::Cycle { .. }))
            .filter_map(Track::natural_length)
            .fold(None, max_opt)
    })
}

/// The length of a scene's timeline: what an animation renders and where
/// live playback stops, given the camera path it flies (`path::resolve`).
///
/// The scene's own camera path, or its tracks, whichever runs longer. A scene
/// with neither runs its default orbit's one turn, as it always has; a scene
/// with tracks but no path of its own runs as long as the tracks do, and the
/// default orbit, which is a cycle, just keeps turning through it.
pub fn timeline_length(scene: &Scene, path: &crate::path::CameraPath) -> f64 {
    let own_path = scene.camera_path.as_ref().is_some_and(|p| p.playable());
    let tracks = tracks_length(&scene.tracks);
    let len = match (own_path, tracks) {
        (true, Some(t)) => (path.duration() as f64).max(t),
        (true, None) | (false, None) => path.duration() as f64,
        (false, Some(t)) => t,
    };
    len.max(0.1)
}

/// Whether a track ends `length` seconds exactly where it began, so an
/// animation of that length can loop without a seam.
pub fn loops_over(track: &Track, length: f64) -> bool {
    let whole = |period: f64| {
        period > 0.0 && {
            let n = length / period;
            (n - n.round()).abs() < 1e-6 && n.round() >= 1.0
        }
    };
    match &track.source {
        Source::Rate { rate, .. } => *rate == 0.0,
        Source::Cycle { period, .. } => whole(*period),
        Source::Keys { keys, extrapolate, .. } => {
            let (Some(a), Some(b)) = (keys.first(), keys.last()) else { return true };
            let span = b.t - a.t;
            match extrapolate {
                _ if keys.len() == 1 => true,
                Extrapolate::Repeat => whole(span),
                Extrapolate::PingPong => whole(2.0 * span),
                // Back where it started only if it never went anywhere.
                Extrapolate::Hold | Extrapolate::Offset => a.value == b.value && whole(span),
            }
        }
    }
}

fn max_opt(acc: Option<f64>, x: f64) -> Option<f64> {
    Some(acc.map_or(x, |a| a.max(x)))
}

// ---------------------------------------------------------------- evaluating

/// A scene ready to be evaluated at any time. See the module doc.
pub struct Animator {
    /// The scene's TOML with its `[[track]]` tables taken out, so a frame
    /// parses as a plain scene and doesn't re-validate its own tracks.
    doc: toml_edit::DocumentMut,
    dir: PathBuf,
    /// Every unmuted track, with the authored value it moves about.
    tracks: Vec<(Track, Option<f64>)>,
    /// Things worth telling the person, found while building this.
    pub warnings: Vec<String>,
    /// Tracks that don't apply to this scene, each with why: kept in the
    /// scene and skipped here. See [`Animator::new`].
    pub broken: Vec<(String, String)>,
    /// The buffer size the scene was given, which is a render property and
    /// so not in the text a frame is parsed from (`--points`, an effort tier).
    point_count: Option<(usize, bool)>,
}

impl Animator {
    /// An animator for `scene`, or `None` when nothing in it moves.
    ///
    /// Built from the scene as it would be saved, so the app (whose scene has
    /// been edited) and the command line (whose scene may carry `--zoom` or a
    /// `--palette`) evaluate exactly what they hold.
    pub fn for_scene(scene: &Scene) -> Result<Option<Self>, String> {
        if scene.tracks.iter().all(|t| t.mute) {
            return Ok(None);
        }
        // `file =` references are relative to wherever the scene lives, which
        // the text doesn't know; absolute ones resolve from anywhere.
        let mut rebased;
        let this = if scene.subscenes.iter().any(|s| s.file_abs.is_some()) {
            rebased = scene.clone();
            for s in &mut rebased.subscenes {
                if let Some(abs) = &s.file_abs {
                    s.file = Some(abs.to_string_lossy().into_owned());
                }
            }
            &rebased
        } else {
            scene
        };
        let text = this.to_toml_string()?;
        let mut anim = Self::new(&text, &std::env::temp_dir(), &scene.tracks)?;
        anim.point_count = Some((scene.point_count, scene.point_count_defaulted));
        Ok(Some(anim))
    }

    pub fn new(text: &str, dir: &Path, tracks: &[Track]) -> Result<Self, String> {
        let mut doc: toml_edit::DocumentMut =
            text.parse().map_err(|e| format!("scene is not valid TOML: {e}"))?;
        doc.remove("track");
        let mut kept = Vec::new();
        let mut warnings = Vec::new();
        let mut broken = Vec::new();
        for track in tracks.iter().filter(|t| !t.mute) {
            // Applying the first value is the check that the path resolves on
            // *this* scene (a transform by that name exists, and so on), and
            // what it displaces is the authored value a rate or cycle moves
            // about.
            //
            // A path that doesn't resolve is *reported, never refused*, as an
            // xaos dead end is: edits reach one constantly — delete the map a
            // track moves and the track points at nothing — and a scene the
            // app saved has to open again. So the track stays in the scene,
            // is skipped here, and says why wherever tracks are shown.
            let probe = track.spec_at(0.0, Some(0.0));
            let applied = match crate::set::apply_to_doc(&mut doc.clone(), &probe) {
                Ok(a) => a,
                Err(e) => {
                    let e = e.strip_prefix("--set ").unwrap_or(&e).to_string();
                    broken.push((track.target.clone(), e));
                    continue;
                }
            };
            let base = applied.from.as_deref().and_then(|s| s.trim().parse::<f64>().ok());
            let needs_base = match &track.source {
                Source::Rate { from, .. } => from.is_none(),
                Source::Cycle { centre, .. } => centre.is_none(),
                Source::Keys { .. } => false,
            };
            if needs_base && base.is_none() {
                broken.push((
                    track.target.clone(),
                    format!(
                        "this scene doesn't write {} down, so there is no authored value for \
                         the {} to move about — write it into the scene, or give the track `{}`",
                        track.target,
                        track.source.kind(),
                        if matches!(track.source, Source::Rate { .. }) { "from" } else { "centre" },
                    ),
                ));
                continue;
            }
            // One Euler angle is only meaningful against the triple it sits
            // in, and a saved scene respells a rotation on the branch with y
            // inside ±90°. Past that, the app (which evaluates what it would
            // save) and the file can disagree about which triple the track is
            // turning — so say so, but only then.
            if let Some(stem) = track.target.strip_suffix(|c: char| "xyz012".contains(c))
                .and_then(|s| s.strip_suffix(".rotation."))
            {
                let y = crate::set::apply_to_doc(&mut doc.clone(), &format!("{stem}.rotation.y=0.0"))
                    .ok()
                    .and_then(|a| a.from)
                    .and_then(|v| v.trim().parse::<f64>().ok());
                if y.is_some_and(|y| y.abs() > 90.0) {
                    warnings.push(format!(
                        "track '{}' turns one Euler angle of a rotation whose y is past 90°; \
                         a saved scene respells such a rotation on the other branch, which \
                         changes what the track turns. Keep y within ±90° for a tracked rotation.",
                        track.target
                    ));
                }
            }
            kept.push((track.clone(), base));
        }
        for (target, why) in &broken {
            warnings.push(format!("track '{target}' doesn't apply, so it is skipped: {why}"));
        }
        Ok(Self { doc, dir: dir.to_path_buf(), tracks: kept, warnings, broken, point_count: None })
    }

    /// The `-S` specs that make the scene at `t`.
    pub fn sets_at(&self, t: f64) -> Vec<String> {
        self.tracks.iter().map(|(track, base)| track.spec_at(t, *base)).collect()
    }

    /// The scene at `t`. It has no tracks of its own: it is a frame, not an
    /// animation.
    pub fn at(&self, t: f64) -> Result<Scene, String> {
        let mut doc = self.doc.clone();
        for spec in self.sets_at(t) {
            crate::set::apply_to_doc(&mut doc, &spec)
                .map_err(|e| format!("at {t:.3}s: {e}"))?;
        }
        let mut scene = Scene::from_toml_str(&doc.to_string(), &self.dir)
            .map_err(|e| format!("at {t:.3}s: {e}"))?;
        if let Some((n, defaulted)) = self.point_count {
            scene.point_count = n;
            scene.point_count_defaulted = defaulted;
        }
        Ok(scene)
    }

    /// Which track makes the scene at `t` one the loader refuses, when
    /// [`at`](Self::at) fails: each is tried on its own. The name is what an
    /// error should carry — the value changes every frame, the track doesn't,
    /// so a message naming the value would be new every frame.
    pub fn culprit(&self, t: f64) -> Option<String> {
        self.tracks.iter().find_map(|(track, base)| {
            let mut doc = self.doc.clone();
            let bad = crate::set::apply_to_doc(&mut doc, &track.spec_at(t, *base)).is_err()
                || Scene::from_toml_str(&doc.to_string(), &self.dir).is_err();
            bad.then(|| track.target.clone())
        })
    }

    /// The highest class of anything that moves.
    pub fn class(&self) -> Option<Class> {
        self.tracks.iter().map(|(t, _)| t.class()).max()
    }

    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().map(|(t, _)| t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(pairs: &[(f64, f64)], interpolate: Interp, extrapolate: Extrapolate) -> Track {
        Track {
            target: "meta.haze".into(),
            source: Source::Keys {
                keys: pairs.iter().map(|&(t, v)| Key { t, value: KeyValue::Number(v) }).collect(),
                interpolate,
                extrapolate,
                integral: false,
            },
            mute: false,
        }
    }

    fn num(v: KeyValue) -> f64 {
        match v {
            KeyValue::Number(x) => x,
            other => panic!("not a number: {other:?}"),
        }
    }

    #[test]
    fn fold_covers_every_extrapolation() {
        use Extrapolate::*;
        assert_eq!(Hold.fold(-1.0, 0.0, 2.0), (0.0, 0));
        assert_eq!(Hold.fold(5.0, 0.0, 2.0), (2.0, 0));
        assert_eq!(Repeat.fold(5.0, 0.0, 2.0), (1.0, 2));
        assert_eq!(Repeat.fold(-0.5, 0.0, 2.0), (1.5, -1));
        assert_eq!(PingPong.fold(3.0, 0.0, 2.0), (1.0, 0));
        assert_eq!(PingPong.fold(4.5, 0.0, 2.0), (0.5, 1));
        assert_eq!(Offset.fold(4.5, 1.0, 2.0), (1.5, 3));
        // A degenerate range has nowhere to fold to.
        assert_eq!(Repeat.fold(4.5, 1.0, 1.0), (1.0, 0));
    }

    #[test]
    fn keys_hit_every_key_whatever_the_interpolation() {
        let pairs = [(0.0, 1.0), (1.0, 3.0), (3.0, -2.0), (4.0, 0.5)];
        for interp in Interp::ALL {
            let track = keys(&pairs, interp, Extrapolate::Hold);
            for &(t, v) in &pairs {
                assert!((num(track.sample(t, None)) - v).abs() < 1e-12, "{interp:?} at {t}");
            }
        }
    }

    #[test]
    fn smooth_eases_and_linear_doesnt() {
        let lin = keys(&[(0.0, 0.0), (2.0, 10.0)], Interp::Linear, Extrapolate::Hold);
        let smooth = keys(&[(0.0, 0.0), (2.0, 10.0)], Interp::Smooth, Extrapolate::Hold);
        assert!((num(lin.sample(0.5, None)) - 2.5).abs() < 1e-12);
        assert!(num(smooth.sample(0.5, None)) < 2.5);
        assert!((num(smooth.sample(1.0, None)) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn spline_passes_through_the_middle_key_without_stopping() {
        let s = keys(&[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)], Interp::Spline, Extrapolate::Hold);
        // Collinear keys at even spacing: Catmull-Rom is exactly the line.
        for i in 0..=20 {
            let t = i as f64 / 10.0;
            assert!((num(s.sample(t, None)) - t).abs() < 1e-12, "at {t}");
        }
    }

    #[test]
    fn offset_climbs_a_lap_at_a_time_and_is_continuous_at_the_seam() {
        let s = keys(&[(0.0, 0.0), (2.0, 90.0)], Interp::Linear, Extrapolate::Offset);
        assert!((num(s.sample(3.0, None)) - 135.0).abs() < 1e-9);
        assert!((num(s.sample(4.0 - 1e-9, None)) - num(s.sample(4.0, None))).abs() < 1e-6);
        // and backwards past the start
        assert!((num(s.sample(-1.0, None)) + 45.0).abs() < 1e-9);
    }

    #[test]
    fn steps_hold_until_the_next_key() {
        let t = Track {
            target: "palette.name".into(),
            source: Source::Keys {
                keys: vec![
                    Key { t: 0.0, value: KeyValue::Other("ember".into()) },
                    Key { t: 2.0, value: KeyValue::Other("ice".into()) },
                ],
                interpolate: Interp::Step,
                extrapolate: Extrapolate::Hold,
                integral: false,
            },
            mute: false,
        };
        assert_eq!(t.spec_at(1.99, None), "palette.name=\"ember\"");
        assert_eq!(t.spec_at(2.0, None), "palette.name=\"ice\"");
        assert_eq!(t.spec_at(9.0, None), "palette.name=\"ice\"");
    }

    #[test]
    fn rates_and_cycles_move_about_the_authored_value() {
        let rate = Track {
            target: "palette.rotate".into(),
            source: Source::Rate { rate: 0.1, from: None },
            mute: false,
        };
        assert!((num(rate.sample(2.0, Some(0.25))) - 0.45).abs() < 1e-12);
        let cycle = Track {
            target: "meta.haze".into(),
            source: Source::Cycle {
                wave: Wave::Sine,
                period: 4.0,
                amplitude: 0.1,
                phase: 0.0,
                centre: None,
            },
            mute: false,
        };
        assert!((num(cycle.sample(1.0, Some(0.5))) - 0.6).abs() < 1e-12);
        assert!((num(cycle.sample(3.0, Some(0.5))) - 0.4).abs() < 1e-12);
    }

    #[test]
    fn every_wave_runs_minus_one_to_one() {
        for w in Wave::ALL {
            for i in 0..400 {
                let v = w.at(i as f64 / 100.0 - 2.0);
                assert!((-1.0..=1.0).contains(&v), "{w:?}");
            }
        }
        assert!((Wave::Triangle.at(0.25) - 1.0).abs() < 1e-12);
        assert!((Wave::Triangle.at(0.75) + 1.0).abs() < 1e-12);
    }

    #[test]
    fn floats_are_written_as_floats_and_counts_as_counts() {
        assert_eq!(value_text(&KeyValue::Number(90.0), false), "90.0");
        assert_eq!(value_text(&KeyValue::Number(14.6), true), "15");
    }

    fn parse(src: &str) -> Result<Vec<Track>, String> {
        #[derive(Deserialize)]
        struct F {
            track: Vec<TrackDef>,
        }
        let f: F = toml::from_str(src).map_err(|e| e.to_string())?;
        resolve_all(&f.track)
    }

    #[test]
    fn the_file_form_reads_all_three_sources() {
        let t = parse(
            r#"
            [[track]]
            target = "transform.a.rotation.y"
            keys = [[0, 0], [4, 90.0]]
            extrapolate = "pingpong"
            [[track]]
            target = "palette.rotate"
            rate = 1
            [[track]]
            target = "meta.haze"
            wave = "triangle"
            period = 8
            amplitude = 0.2
            "#,
        )
        .unwrap();
        assert_eq!(t.len(), 3);
        assert!(matches!(
            t[0].source,
            Source::Keys { extrapolate: Extrapolate::PingPong, interpolate: Interp::Smooth, integral: false, .. }
        ));
        assert_eq!(t[1].source, Source::Rate { rate: 1.0, from: None });
        assert!(matches!(t[2].source, Source::Cycle { wave: Wave::Triangle, .. }));
        // and writes back what it read
        for track in &t {
            assert_eq!(&track.to_def().resolve().unwrap(), track);
        }
    }

    #[test]
    fn the_file_form_refuses_what_would_fail_silently() {
        let bad = [
            ("target = \"camera.distance\"\nrate = 1", "the camera"),
            ("target = \"meta.haze\"\nrate = 1\nperiod = 2", "exactly one"),
            ("target = \"meta.haze\"", "exactly one"),
            ("target = \"meta.haze\"\nkeys = [[1, 0.0], [0.5, 1.0]]", "time order"),
            ("target = \"palette.name\"\nkeys = [[0, \"a\"], [1, \"b\"]]\ninterpolate = \"linear\"", "only numbers"),
            ("target = \"meta.haze\"\nwave = \"sine\"\namplitude = 1", "period"),
            ("target = \"meta.hazy\"\nrate = 1", "meta field"),
            ("target = \"meta.haze\"\nkeys = [[0, 0.0]]\nbogus = 1", "unknown field"),
        ];
        for (body, want) in bad {
            let e = parse(&format!("[[track]]\n{body}")).unwrap_err();
            assert!(e.contains(want), "{body:?}: {e}");
        }
        let e = parse("[[track]]\ntarget = \"meta.haze\"\nrate = 1\n[[track]]\ntarget = \"meta.haze\"\nrate = 2")
            .unwrap_err();
        assert!(e.contains("two tracks"), "{e}");
    }

    const SCENE: &str = r#"
[meta]
name = "anim"
haze = 0.5

[[transform]]
name = "a"
translation = [0.5, 0.0, 0.0]
scale = 0.5
rotation = [0.0, 0.0, 0.0]

[[transform]]
name = "b"
translation = [-0.5, 0.0, 0.0]
scale = 0.5

[[track]]
target = "transform.a.rotation.z"
keys = [[0.0, 0.0], [2.0, 90.0]]
interpolate = "linear"

[[track]]
target = "meta.haze"
wave = "sine"
period = 4.0
amplitude = 0.25
"#;

    fn scene() -> Scene {
        Scene::from_toml_str(SCENE, &std::env::temp_dir()).unwrap()
    }

    // -- whole rotations -------------------------------------------------------

    fn poses(eulers: &[(f64, [f64; 3])], interpolate: Interp, extrapolate: Extrapolate) -> Track {
        Track {
            target: "transform.a.rotation".into(),
            source: Source::Keys {
                keys: eulers
                    .iter()
                    .map(|&(t, e)| Key { t, value: KeyValue::Rotation(RotKey::from_euler(e)) })
                    .collect(),
                interpolate,
                extrapolate,
                integral: false,
            },
            mute: false,
        }
    }

    fn pose(v: KeyValue) -> crate::rot::Orientation {
        match v {
            KeyValue::Rotation(r) => r.orientation,
            other => panic!("not a rotation: {other:?}"),
        }
    }

    /// How far the map turns about world Y between `t0` and `t1`, summed in
    /// small steps so whole turns count.
    fn swept_y(track: &Track, t0: f64, t1: f64) -> f64 {
        let n = 720;
        let mut total = 0.0;
        let mut prev = pose(track.sample(t0, None));
        for i in 1..=n {
            let now = pose(track.sample(t0 + (t1 - t0) * i as f64 / n as f64, None));
            total += prev.shortest_turn_to(now).as_rotation_vector().y as f64;
            prev = now;
        }
        total.to_degrees()
    }

    /// The route is in the Euler column: y from 0 to 720 is two whole turns,
    /// 0 to 270 goes the long way as written — never the 90° back the short
    /// way would take — and the keys are hit.
    #[test]
    fn one_angle_changing_turns_that_far_about_its_axis() {
        let two_turns = poses(&[(0.0, [0.0; 3]), (4.0, [0.0, 720.0, 0.0])], Interp::Linear, Extrapolate::Hold);
        assert!((swept_y(&two_turns, 0.0, 4.0) - 720.0).abs() < 0.5);
        let long_way = poses(&[(0.0, [0.0; 3]), (1.0, [0.0, 270.0, 0.0])], Interp::Linear, Extrapolate::Hold);
        assert!((swept_y(&long_way, 0.0, 1.0) - 270.0).abs() < 0.5);
        let half = pose(long_way.sample(0.5, None));
        let want = crate::rot::Orientation::from_axis_angle(glam::Vec3::Y, 135f32.to_radians());
        assert!(half.angle_to(want) < 1e-4);
        // Linear is a constant rate: a quarter of the time, a quarter of the way.
        assert!((swept_y(&two_turns, 0.0, 1.0) - 180.0).abs() < 0.5);
    }

    /// A y change under a tilted x turns about the tilted axis (R = Rx·Ry·Rz),
    /// and z about the map's own: every in-between pose is the triple with
    /// that one angle in between.
    #[test]
    fn the_axis_is_the_one_the_triple_turns_about() {
        for (a, b) in [([30.0, 0.0, 10.0], [30.0, 200.0, 10.0]), ([30.0, 40.0, 0.0], [30.0, 40.0, 300.0])] {
            let tr = poses(&[(0.0, a), (1.0, b)], Interp::Linear, Extrapolate::Hold);
            for i in 0..=8 {
                let u = i as f64 / 8.0;
                let mid: Vec<f64> = (0..3).map(|c| a[c] + (b[c] - a[c]) * u).collect();
                let want = RotKey::from_euler([mid[0], mid[1], mid[2]]).orientation;
                assert!(pose(tr.sample(u, None)).angle_to(want) < 1e-4, "{a:?}->{b:?} at {u}");
            }
        }
    }

    /// Offset keeps turning: keyed through a quarter turn in a second, it is
    /// half a turn round at two seconds and a whole one at four.
    #[test]
    fn offset_keeps_a_rotation_turning() {
        let tr = poses(&[(0.0, [0.0; 3]), (1.0, [0.0, 90.0, 0.0])], Interp::Linear, Extrapolate::Offset);
        assert!((swept_y(&tr, 0.0, 4.0) - 360.0).abs() < 0.5);
        let at = |t| pose(tr.sample(t, None));
        let y = |deg: f32| crate::rot::Orientation::from_axis_angle(glam::Vec3::Y, deg.to_radians());
        assert!(at(2.0).angle_to(y(180.0)) < 1e-4);
        assert!(at(2.5).angle_to(y(225.0)) < 1e-4);
    }

    /// Spline is the camera's spline: through every key, at its time.
    #[test]
    fn a_rotation_spline_passes_its_keys() {
        let ks = [(0.0, [0.0; 3]), (1.0, [40.0, 10.0, 0.0]), (3.0, [40.0, 190.0, 0.0]), (4.0, [0.0, 200.0, 30.0])];
        let tr = poses(&ks, Interp::Spline, Extrapolate::Hold);
        for (t, e) in ks {
            assert!(pose(tr.sample(t, None)).angle_to(RotKey::from_euler(e).orientation) < 1e-3, "at {t}");
        }
    }

    /// In a scene: the file form loads and saves back the same, the frame at
    /// a time has the map at that pose (through `rotvec`), and a whole
    /// rotation can't be tracked beside one of its own angles.
    #[test]
    fn a_rotation_track_in_a_scene() {
        let src = SCENE.replace(
            "[[track]]\ntarget = \"transform.a.rotation.z\"\nkeys = [[0.0, 0.0], [2.0, 90.0]]\ninterpolate = \"linear\"",
            "[[track]]\ntarget = \"transform.a.rotation\"\ninterpolate = \"linear\"\n\n[[track.keys]]\nt = 0.0\nrotation = [0.0, 0.0, 0.0]\n\n[[track.keys]]\nt = 2.0\nrotation = [0.0, 0.0, 450.0]",
        );
        let s = Scene::from_toml_str(&src, &std::env::temp_dir()).unwrap();
        let text = s.to_toml_string().unwrap();
        assert!(text.contains("[[track.keys]]") && text.contains("rotation = [0.0, 0.0, 450.0]"), "{text}");
        let back = Scene::from_toml_str(&text, &std::env::temp_dir()).unwrap();
        assert_eq!(back.tracks, s.tracks);
        // Halfway is 225° about z: the map's x axis points along (-1, -1)/√2.
        let frame = Animator::for_scene(&s).unwrap().unwrap().at(1.0).unwrap();
        let x = frame.transforms[0].matrix.x_axis.truncate().normalize();
        assert!((x.x - x.y).abs() < 1e-4 && x.x < -0.5, "{x:?}");
        assert!((frame.transforms[0].matrix.x_axis.truncate().length() - 0.5).abs() < 1e-5, "scale kept");

        let both = format!("{src}\n[[track]]\ntarget = \"transform.a.rotation.y\"\nrate = 1.0\nfrom = 0.0\n");
        assert!(Scene::from_toml_str(&both, &std::env::temp_dir()).is_err_and(|e| e.contains("whole rotation")));
    }

    #[test]
    fn a_scene_reads_and_saves_its_tracks() {
        let s = scene();
        assert_eq!(s.tracks.len(), 2);
        let text = s.to_toml_string().unwrap();
        let back = Scene::from_toml_str(&text, &std::env::temp_dir()).unwrap();
        assert_eq!(back.tracks, s.tracks);
    }

    #[test]
    fn the_scene_at_a_time_is_the_scene_with_the_tracks_applied() {
        let s = scene();
        let anim = Animator::for_scene(&s).unwrap().unwrap();
        assert_eq!(anim.class(), Some(Class::Structure));
        let at1 = anim.at(1.0).unwrap();
        assert!(at1.tracks.is_empty(), "a frame is not an animation");
        assert!((at1.haze - 0.75).abs() < 1e-6, "haze {}", at1.haze);
        // 45° about z: the map's x axis now points along (1, 1)/√2
        let x = at1.transforms[0].matrix.x_axis.truncate().normalize();
        assert!((x.x - x.y).abs() < 1e-5 && x.x > 0.5, "{x:?}");
        // The untracked map didn't move.
        assert_eq!(at1.transforms[1].matrix, s.transforms[1].matrix);
        // And the base is untouched.
        assert!((s.haze - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_track_aimed_at_nothing_is_skipped_and_said_not_refused() {
        let mut s = scene();
        s.tracks.push(Track {
            target: "transform.nope.weight".into(),
            source: Source::Rate { rate: 1.0, from: Some(1.0) },
            mute: false,
        });
        // The scene still opens and the other tracks still play.
        let anim = Animator::for_scene(&s).unwrap().unwrap();
        assert_eq!(anim.tracks().count(), 2);
        assert_eq!(anim.broken.len(), 1);
        assert!(anim.warnings.iter().any(|w| w.contains("nope")), "{:?}", anim.warnings);
        assert!(anim.at(1.0).is_ok());
        // And a saved scene with one in it loads again.
        let text = s.to_toml_string().unwrap();
        assert!(Scene::from_toml_str(&text, &std::env::temp_dir()).is_ok());
    }

    #[test]
    fn a_rate_on_an_unwritten_key_needs_a_from() {
        // Found where the text is what the author wrote. (A scene saved from
        // the app writes every weight, so there it has one.)
        let src = format!("{SCENE}\n[[track]]\ntarget = \"transform.b.weight\"\nrate = 1.0\n");
        let anim = Animator::new(&src, &std::env::temp_dir(), &resolve_all(&[TrackDef {
            target: "transform.b.weight".into(),
            rate: Some(toml::Value::Float(1.0)),
            ..Default::default()
        }]).unwrap()).unwrap();
        assert!(anim.broken[0].1.contains("from"), "{:?}", anim.broken);
    }

    #[test]
    fn renaming_a_map_carries_its_tracks() {
        let mut tracks = vec![
            Track { target: "transform.a.weight".into(), source: Source::Rate { rate: 1.0, from: None }, mute: false },
            Track { target: "transform.b.xaos.a".into(), source: Source::Rate { rate: 1.0, from: None }, mute: false },
            Track { target: "transform.ab.weight".into(), source: Source::Rate { rate: 1.0, from: None }, mute: false },
        ];
        rename_map(&mut tracks, "a", "arm");
        let t: Vec<&str> = tracks.iter().map(|t| t.target.as_str()).collect();
        assert_eq!(t, ["transform.arm.weight", "transform.b.xaos.arm", "transform.ab.weight"]);
    }

    #[test]
    fn muted_tracks_dont_move_anything() {
        let mut s = scene();
        for t in &mut s.tracks {
            t.mute = true;
        }
        assert!(Animator::for_scene(&s).unwrap().is_none());
    }

    #[test]
    fn a_loop_is_seamless_only_when_every_track_comes_back() {
        let cyc = |period| Track {
            target: "meta.haze".into(),
            source: Source::Cycle { wave: Wave::Sine, period, amplitude: 0.1, phase: 0.0, centre: None },
            mute: false,
        };
        assert!(loops_over(&cyc(4.0), 8.0));
        assert!(!loops_over(&cyc(3.0), 8.0));
        assert!(loops_over(&keys(&[(0.0, 0.0), (2.0, 1.0)], Interp::Linear, Extrapolate::PingPong), 8.0));
        assert!(!loops_over(&keys(&[(0.0, 0.0), (2.0, 1.0)], Interp::Linear, Extrapolate::Hold), 2.0));
        assert!(!loops_over(&keys(&[(0.0, 0.0), (2.0, 1.0)], Interp::Linear, Extrapolate::Offset), 2.0));
    }

    #[test]
    fn a_frame_keeps_the_buffer_size_it_was_given() {
        let mut s = scene();
        s.point_count = 1234;
        let frame = Animator::for_scene(&s).unwrap().unwrap().at(0.5).unwrap();
        assert_eq!(frame.point_count, 1234);
    }

    #[test]
    fn a_track_is_kept_inside_what_the_scene_can_take() {
        // A cycle about 0 would swing an xaos multiplier negative, which the
        // loader refuses; the frame comes out at 0 instead.
        let src = format!(
            "{SCENE}\n[[track]]\ntarget = \"transform.a.xaos.b\"\nwave = \"sine\"\nperiod = 4.0\namplitude = 1.0\ncentre = 0.0\n"
        );
        let s = Scene::from_toml_str(&src, &std::env::temp_dir()).unwrap();
        let anim = Animator::for_scene(&s).unwrap().unwrap();
        assert!(anim.at(3.0).is_ok(), "the trough is clamped to 0");
        assert!(anim.culprit(3.0).is_none());
    }

    #[test]
    fn an_invalid_frame_is_blamed_on_its_track() {
        let src = format!(
            "{SCENE}\n[[track]]\ntarget = \"meta.color_mode\"\nkeys = [[0.0, \"transforms\"], [2.0, \"bogus\"]]\n"
        );
        let s = Scene::from_toml_str(&src, &std::env::temp_dir()).unwrap();
        let anim = Animator::for_scene(&s).unwrap().unwrap();
        assert!(anim.at(1.0).is_ok());
        assert!(anim.at(3.0).is_err());
        assert_eq!(anim.culprit(3.0).as_deref(), Some("meta.color_mode"));
    }

    #[test]
    fn the_tracks_length_is_the_last_key() {
        assert_eq!(tracks_length(&scene().tracks), Some(2.0));
        assert_eq!(tracks_length(&[]), None);
    }

    /// The live window evaluates a frame of the scene every frame it plays, so
    /// this has to stay cheap. Prints the figure; fails only if it is absurd.
    #[test]
    fn an_animated_scene_is_cheap_to_build() {
        let anim = Animator::for_scene(&scene()).unwrap().unwrap();
        let n = 200;
        let start = std::time::Instant::now();
        for i in 0..n {
            anim.at(i as f64 * 0.01).unwrap();
        }
        let each = start.elapsed().as_secs_f64() / n as f64;
        eprintln!("a frame of the scene: {:.3} ms", each * 1e3);
        assert!(each < 0.05, "{each}s a frame");
    }
}
