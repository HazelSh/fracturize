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
//! # The camera is a track too, of a kind that can't be three numbers
//!
//! Framings are rotations, and rotations don't interpolate as numbers — that
//! is what `path.rs`'s routes and cumulative spline exist for. So the camera
//! keeps its own *source* (the `CameraPath` spline, or the default full orbit,
//! which is a cycle in all but name) and shares everything else: it is sampled
//! on the same clock, in seconds, and its loop is one of these same
//! extrapolations — `once` is [`Extrapolate::Hold`], `closed` is `Repeat`,
//! `pingpong` is `PingPong`, and the zoom loop is **repeat with offset**,
//! where the offset is the zoom symmetry rather than a number. That last one
//! is the reason to name them this way: `path_loop = "zoom"` stops being a
//! special case and becomes `Offset` on a framing.
//! [`Extrapolate::fold`] is the one time-folding function; `CameraPath::sample`
//! calls it too.
//!
//! The camera's file format is unchanged. Moving `[[camera.path]]` into
//! `[[track]]` form is a format change to every scene with a path, and the plan
//! makes it one explicit migration commit for Hazel to run, with views-as-keys
//! a decision still open; nothing here needs it.
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

/// A key's value: a number, or anything else TOML can say (a palette name, a
/// boolean), which only ever steps.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyValue {
    Number(f64),
    Other(toml::Value),
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

    /// The value at `t` as the `-S` spec that applies it.
    pub fn spec_at(&self, t: f64, base: Option<f64>) -> String {
        let integral = matches!(self.source, Source::Keys { integral: true, .. });
        format!("{}={}", self.target, value_text(&self.sample(t, base), integral))
    }
}

/// TOML text for a value, written so the loader reads back the same type:
/// a float always carries its point (`90.0`, not `90`, which is an integer).
pub fn value_text(v: &KeyValue, integral: bool) -> String {
    match v {
        KeyValue::Number(x) if integral => format!("{}", x.round() as i64),
        KeyValue::Number(x) if x.is_finite() => format!("{:?}", x),
        KeyValue::Number(_) => "0.0".to_string(),
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
                KeyValue::Other(_) => 0.0,
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

// ---------------------------------------------------------------- the file

/// A `[[track]]` table.
///
/// Exactly one of `keys`, `rate` and the cycle fields (`wave`, `period`,
/// `amplitude`) says which source it is.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TrackDef {
    pub target: String,
    /// `[[seconds, value], ...]`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<Vec<toml::Value>>>,
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
}

/// A TOML number as `f64`, whichever way it was written: `rate = 1` is as good
/// as `rate = 1.0`, and a hand-written file has no reason to know the
/// difference.
fn number(v: &toml::Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
}

impl TrackDef {
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
        crate::set::validate_path(&target).map_err(|e| format!("{who}: {e}"))?;
        if target.starts_with("camera.") {
            return Err(format!(
                "{who}: the camera is animated by its path ([[camera.path]] keys, \
                 path_loop, path_seconds), not by a track — a track on it would fight \
                 the path every frame"
            ));
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

        let source = if let Some(raw) = &self.keys {
            let mut keys = Vec::with_capacity(raw.len());
            for pair in raw {
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
            let integral = raw.iter().all(|p| p.get(1).is_some_and(|v| v.as_integer().is_some()));
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
                            let v = match &k.value {
                                KeyValue::Number(x) if *integral => toml::Value::Integer(x.round() as i64),
                                KeyValue::Number(x) => toml::Value::Float(*x),
                                KeyValue::Other(v) => v.clone(),
                            };
                            vec![toml::Value::Float(k.t), v]
                        })
                        .collect(),
                );
                let numeric = keys.iter().all(|k| matches!(k.value, KeyValue::Number(_)));
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
pub fn resolve_all(defs: &[TrackDef]) -> Result<Vec<Track>, String> {
    let tracks: Vec<Track> = defs.iter().map(TrackDef::resolve).collect::<Result<_, _>>()?;
    for (i, a) in tracks.iter().enumerate() {
        if tracks[..i].iter().any(|b| b.target == a.target) {
            return Err(format!(
                "two tracks animate '{}'; the second would silently win — merge their keys",
                a.target
            ));
        }
    }
    Ok(tracks)
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
        for track in tracks.iter().filter(|t| !t.mute) {
            // Applying the first value is the check that the path resolves on
            // *this* scene (a transform by that name exists, and so on), and
            // what it displaces is the authored value a rate or cycle moves
            // about.
            let probe = track.spec_at(0.0, Some(0.0));
            let applied = crate::set::apply_to_doc(&mut doc.clone(), &probe)
                .map_err(|e| format!("track '{}': {e}", track.target))?;
            let base = applied.from.as_deref().and_then(|s| s.trim().parse::<f64>().ok());
            let needs_base = match &track.source {
                Source::Rate { from, .. } => from.is_none(),
                Source::Cycle { centre, .. } => centre.is_none(),
                Source::Keys { .. } => false,
            };
            if needs_base && base.is_none() {
                return Err(format!(
                    "track '{}': this scene doesn't write {} down, so there is no authored \
                     value for the {} to move about — write it into the scene, or give the \
                     track `{}`",
                    track.target,
                    track.target,
                    track.source.kind(),
                    if matches!(track.source, Source::Rate { .. }) { "from" } else { "centre" },
                ));
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
        Ok(Self { doc, dir: dir.to_path_buf(), tracks: kept, warnings, point_count: None })
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
    fn an_animator_refuses_a_target_this_scene_doesnt_have() {
        let mut s = scene();
        s.tracks.push(Track {
            target: "transform.nope.weight".into(),
            source: Source::Rate { rate: 1.0, from: Some(1.0) },
            mute: false,
        });
        let e = Animator::for_scene(&s).err().expect("should refuse");
        assert!(e.contains("nope"), "{e}");
    }

    #[test]
    fn a_rate_on_an_unwritten_key_needs_a_from() {
        // Refused at load, where the text is what the author wrote. (A scene
        // saved from the app writes every weight, so there it has one.)
        let src = format!("{SCENE}\n[[track]]\ntarget = \"transform.b.weight\"\nrate = 1.0\n");
        let e = Scene::from_toml_str(&src, &std::env::temp_dir()).err().expect("should refuse");
        assert!(e.contains("from"), "{e}");
        let ok = src.replace("rate = 1.0", "rate = 1.0\nfrom = 1.0");
        assert!(Scene::from_toml_str(&ok, &std::env::temp_dir()).is_ok());
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
