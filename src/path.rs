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
//! an explicit `winding`: `0` is the short way, `1` takes an extra whole turn,
//! `-1` goes the long way against it. The endpoints are always hit exactly,
//! whatever the winding says — a route can be surprising, but it can never
//! land in the wrong place. See [`crate::rot`] for why this is a `i32` and not
//! a stored displacement.
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
//! - Open paths ease in/out by default (smoothstep on path time); closed
//!   paths default to constant speed so the loop seam is invisible.

use glam::Vec3;

use crate::camera::OrbitCamera;
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

    pub fn to_camera(self) -> OrbitCamera {
        OrbitCamera {
            orientation: self.orientation,
            distance: self.distance,
            focus: self.focus,
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

/// A spline camera path through two or more keypoints
#[derive(Clone, Debug)]
pub struct CameraPath {
    pub keys: Vec<PathKey>,
    /// Extra whole turns taken on each segment, beyond the short way round.
    ///
    /// One entry per segment (see [`CameraPath::segments`]); segment `i` runs
    /// from key `i` to key `i+1`, wrapping on a closed path. Missing or short
    /// entries read as `0`, so a path can always be built without thinking
    /// about it and a hand-written scene never has to mention it.
    pub windings: Vec<i32>,
    /// Loop back to the first key after the last (seamless loops)
    pub closed: bool,
    /// Close under the scene's zoom symmetry instead of by returning to the
    /// first key: one loop descends `periods` zoom periods and lands on an
    /// identical frame. See [`ZoomLoop`].
    pub zoom_loop: Option<ZoomLoop>,
    /// Ease in/out (smoothstep on path time); None = default (!closed)
    pub ease: Option<bool>,
    /// Suggested playback/render duration; None = 3s per segment
    pub seconds: Option<f32>,
}

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

/// Uniform Catmull-Rom: interpolate between p1 and p2 with neighbors p0, p3
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
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
fn cumulative_basis(u: f32) -> [f32; 3] {
    let (u2, u3) = (u * u, u * u * u);
    [
        1.0 + 0.5 * u3 - u2 + 0.5 * u,
        -u3 + 1.5 * u2 + 0.5 * u,
        0.5 * (u3 - u2),
    ]
}

impl CameraPath {
    /// A path through `keys`, taking the short way round on every segment.
    pub fn new(keys: Vec<PathKey>, closed: bool) -> Self {
        Self {
            keys,
            windings: Vec::new(),
            closed,
            zoom_loop: None,
            ease: None,
            seconds: None,
        }
    }

    /// Whether playback loops: the last frame runs into the first.
    ///
    /// True for both kinds of loop — returning to the first key, and closing
    /// under the zoom symmetry — because everything that cares about *time*
    /// (t wrapping, dropping the duplicate final frame, not easing into a
    /// seam) wants the same answer for both. Only the spline itself
    /// distinguishes them.
    pub fn wraps(&self) -> bool {
        self.closed || self.zoom_loop.is_some()
    }

    /// Whether there is enough here to interpolate.
    ///
    /// Two keys, normally — a spline needs two ends, and one key is a scene
    /// mid-authoring. A zoom loop is the exception and needs only one: its
    /// closing segment runs to that key's own image under the symmetry, which
    /// is a real segment through real geometry.
    pub fn playable(&self) -> bool {
        self.keys.len() >= 2 || (self.zoom_loop.is_some() && !self.keys.is_empty())
    }

    /// Number of spline segments (looping paths add the wrap-around segment).
    ///
    /// A zoom loop is the one path that can have a single key and still be a
    /// path: its closing segment runs from that key to the key's own image
    /// under the symmetry, which is a real segment through real geometry.
    pub fn segments(&self) -> usize {
        match (self.keys.len(), self.wraps()) {
            (0, _) => 0,
            (1, true) => 1,
            (1, false) => 0,
            (n, true) => n,
            (n, false) => n - 1,
        }
    }

    /// Playback duration: explicit `seconds`, or 3s per segment
    pub fn duration(&self) -> f32 {
        self.seconds
            .unwrap_or(3.0 * self.segments().max(1) as f32)
            .max(0.1)
    }

    fn eased(&self) -> bool {
        self.ease.unwrap_or(!self.wraps())
    }

    /// Extra whole turns on segment `i`, or 0 where nothing was authored.
    pub fn winding(&self, i: usize) -> i32 {
        self.windings.get(i).copied().unwrap_or(0)
    }

    /// Grow or shrink `windings` to match the segment count, so the two never
    /// drift apart as keys are added and removed.
    pub fn fit_windings(&mut self) {
        self.windings.resize(self.segments(), 0);
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
        Self {
            keys,
            windings: vec![0; 4],
            closed: true,
            zoom_loop: None,
            ease: Some(false),
            seconds: Some(tau / ORBIT_RATE),
        }
    }

    /// Key for spline index i, where i ranges over -1..=segments()+1.
    /// Open paths clamp at the ends; closed paths wrap; a zoom loop's
    /// out-of-range keys are the in-range ones carried by the symmetry.
    fn key(&self, i: isize) -> PathKey {
        let n = self.keys.len() as isize;
        let idx = i.rem_euclid(n) as usize;
        let turns = ((i - idx as isize) / n) as i32; // whole loops i is offset by

        // Carrying by the symmetry is what makes the spline periodic *in
        // appearance* rather than in parameter space. It also means no
        // clamping at the ends: the seam gets the same treatment as any
        // interior segment, so the loop has no velocity kink either.
        if let Some(z) = &self.zoom_loop {
            return z.advance(self.keys[idx], turns);
        }
        if !self.closed {
            return self.keys[i.clamp(0, n - 1) as usize];
        }
        self.keys[idx]
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

        // A zoom loop's segments are all the same turn — the symmetry's, seen
        // from the key. Conjugating preserves the magnitude exactly, so a
        // multi-period loop keeps its whole sweep and cannot fold.
        if let Some(z) = &self.zoom_loop {
            let from = self.key(i);
            return Turn::from_rotation_vector(
                from.orientation.inverse().rotate(z.turn.as_rotation_vector()),
            );
        }

        // Past the ends of an open path the neighbour keys are clamped copies,
        // so there is nowhere to go.
        if !self.closed && (i < 0 || i >= segs) {
            return Turn::ZERO;
        }

        let from = self.key(i);
        let to = self.key(i + 1);
        from.orientation
            .turn_to(to.orientation, self.winding(i.rem_euclid(segs) as usize))
    }

    /// Sample the path at t in [0, 1] (clamped; closed paths wrap seamlessly)
    pub fn sample(&self, t: f32) -> OrbitCamera {
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
                .to_camera();
        }

        // Orientation is genuinely periodic, so a looping path needs no
        // accumulation across whole loops: the second pass *is* the first.
        let t = if self.wraps() {
            t.rem_euclid(1.0)
        } else {
            t.clamp(0.0, 1.0)
        };
        let t = if self.eased() { t * t * (3.0 - 2.0 * t) } else { t };

        let x = t * segs as f32;
        // For open paths t=1 must land inside the last segment
        let seg = (x.floor() as isize).min(segs as isize - 1);
        let u = x - seg as f32;

        let (k0, k1, k2, k3) = (
            self.key(seg - 1),
            self.key(seg),
            self.key(seg + 1),
            self.key(seg + 2),
        );

        // The cumulative form: start at the key *before* the segment and walk
        // three weighted displacements. At u=0 the weights are (1,0,0), which
        // lands exactly on k1; at u=1 they are (1,1,0), which lands exactly on
        // k2. Keys are hit whatever the windings say.
        let b = cumulative_basis(u);
        let orientation = k0
            .orientation
            .then_body(self.segment_turn(seg - 1) * b[0])
            .then_body(self.segment_turn(seg) * b[1])
            .then_body(self.segment_turn(seg + 1) * b[2]);

        let cr = |f: fn(&PathKey) -> f32| catmull_rom(f(&k0), f(&k1), f(&k2), f(&k3), u);
        OrbitCamera {
            orientation,
            distance: cr(|k| k.distance.max(1e-3).ln()).exp(),
            focus: Vec3::new(cr(|k| k.focus.x), cr(|k| k.focus.y), cr(|k| k.focus.z)),
        }
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
                let cam = path.sample(i as f32 / SAMPLES as f32);
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
                    .map(|i| path.sample(i as f32 / 64.0))
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
                assert_eq!(x.windings, y.windings, "{}: routes changed across a save", name);
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

        // Only the [[camera.path]] keys; the base [camera] block has a yaw too.
        let text = std::fs::read_to_string(&tmp).unwrap();
        let yaws: Vec<f64> = text
            .split("[[camera.path]]")
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
        let mut prev = p.sample(from).orientation;
        for i in 1..=n {
            let t = from + (to - from) * i as f32 / n as f32;
            let cur = p.sample(t).orientation;
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
        CameraPath {
            keys,
            windings: Vec::new(),
            closed: false,
            zoom_loop: Some(ZoomLoop {
                periods,
                center: Vec3::ZERO,
                scale: 0.6f32.powi(periods as i32),
                turn: Turn::about(Vec3::Y, 34f32.to_radians() * periods as f32),
            }),
            ease: None,
            seconds: Some(10.0),
        }
    }

    fn linear_path(closed: bool) -> CameraPath {
        CameraPath {
            keys: vec![
                key(0.0, 0.1, 2.0, Vec3::ZERO),
                key(1.0, 0.3, 4.0, Vec3::X),
                key(2.0, 0.2, 3.0, Vec3::Y),
            ],
            windings: Vec::new(),
            closed,
            zoom_loop: None,
            ease: Some(false),
            seconds: None,
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
        for closed in [false, true] {
            let p = linear_path(closed);
            let n = if closed { p.keys.len() } else { p.keys.len() - 1 };
            for (i, k) in p.keys.iter().enumerate() {
                let t = i as f32 / n as f32;
                let cam = p.sample(t);
                assert!(
                    cam.orientation.angle_to(k.orientation) < 1e-4,
                    "closed={} key {} orientation",
                    closed,
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
            windings: Vec::new(),
            closed: false,
            zoom_loop: None,
            ease: Some(false),
            seconds: None,
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
            let got = p.sample(t).chart();
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
        let p = linear_path(true);
        let a = p.sample(0.0);
        let b = p.sample(1.0);
        assert!((a.eye() - b.eye()).length() < 1e-3);
        assert!((a.focus - b.focus).length() < 1e-4);
        // C1 continuity at the seam: second-order one-sided derivative
        // estimates from both sides must agree
        let eps = 1e-3;
        let f = |t: f32| p.sample(t).eye();
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
            windings: Vec::new(),
            closed: true,
            zoom_loop: None,
            ease: Some(false),
            seconds: None,
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
        let short = CameraPath { windings: vec![0], ..CameraPath::new(keys.clone(), false) };
        let long = CameraPath { windings: vec![1], ..CameraPath::new(keys.clone(), false) };

        for p in [&short, &long] {
            assert!(p.sample(0.0).orientation.angle_to(keys[0].orientation) < 1e-4);
            assert!(p.sample(1.0).orientation.angle_to(keys[1].orientation) < 1e-4);
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
            windings: vec![1, 1],
            closed: false,
            zoom_loop: None,
            ease: Some(false),
            seconds: None,
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
                false,
            )
        };
        assert!((p.sample(0.5).distance - 2.0).abs() < 1e-3, "got {}", p.sample(0.5).distance);
    }

    #[test]
    fn roll_interpolates_along_the_path() {
        // Only the roll differs, so the segment turns share the body Z axis and
        // the spline is exactly a scalar roll ramp.
        let p = CameraPath {
            ease: Some(false),
            ..CameraPath::new(
                vec![rolled_key(0.0, 0.0, 0.0, 3.0), rolled_key(0.0, 0.0, 1.0, 3.0)],
                false,
            )
        };
        assert!(p.sample(0.0).chart().roll.radians().abs() < 1e-4);
        assert!((p.sample(0.5).chart().roll.radians() - 0.5).abs() < 1e-3);
        assert!((p.sample(1.0).chart().roll.radians() - 1.0).abs() < 1e-4);
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
                false,
            )
        };
        let mut prev = p.sample(0.0).forward();
        for i in 1..=200 {
            let cam = p.sample(i as f32 / 200.0);
            let f = cam.forward();
            assert!(f.is_finite() && cam.up().is_finite(), "basis blew up at t={}", i);
            assert!((cam.up().length() - 1.0).abs() < 1e-3, "basis stopped being unit");
            assert!(f.dot(prev) > 0.99, "the view snapped at t={}: dot {}", i, f.dot(prev));
            prev = f;
        }
        // ...and it really did pass over the top.
        assert!(p.sample(0.5).forward().y < -0.99, "should be looking straight down at the midpoint");
    }

    // -- zoom loops --------------------------------------------------------

    #[test]
    fn a_zoom_loop_ends_on_the_frame_it_started() {
        // The whole claim: the last frame is not *near* the first, it is the
        // first — because the scene is invariant under the similarity that
        // separates them.
        let path = zoom_loop_path(1, vec![key(0.3, 0.5, 3.6, Vec3::ZERO)]);
        let (start, end) = (path.sample(0.0), path.sample(SEAM));
        let z = path.zoom_loop.unwrap();
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
        let z = path.zoom_loop.unwrap();
        let carried = z.advance(PathKey::from_camera(&path.sample(SEAM)), -1);
        let start = PathKey::from_camera(&path.sample(0.0));
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
        let z = path.zoom_loop.unwrap();
        let start = path.sample(0.0).orientation;
        for i in 0..=20 {
            let t = i as f32 / 20.0 * SEAM;
            let want_dist = 4.0f32.ln() + t * 0.6f32.ln();
            let got = path.sample(t);
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
        let ratio = path.sample(SEAM).distance / 4.0;
        assert!((ratio - 0.6f32.powi(3)).abs() < 1e-3, "descended {ratio}x");
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
            windings: Vec::new(),
            closed: false,
            zoom_loop: Some(ZoomLoop {
                periods: 1,
                center: Vec3::ZERO,
                scale: 0.6,
                turn: twist,
            }),
            ease: None,
            seconds: Some(10.0),
        };
        let swept = swept_about_y(&path, 0.0, SEAM, 256);
        assert!(
            swept.abs() < 1f32.to_radians(),
            "swept {:.1}° to make a half-degree turn",
            swept.to_degrees()
        );
        assert!(swept < 0.0, "359.5° is a turn the other way, swept {:.1}°", swept.to_degrees());

        // And it still closes exactly where the symmetry says.
        let z = path.zoom_loop.unwrap();
        let carried = z.center + z.turn.exp().rotate((path.sample(0.0).eye() - z.center) * z.scale);
        assert!(
            (path.sample(SEAM).eye() - carried).length() < 1e-3,
            "ended at {:?}, symmetry says {carried:?}",
            path.sample(SEAM).eye()
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
            windings: Vec::new(),
            closed: false,
            zoom_loop: Some(ZoomLoop {
                periods: 4,
                center: Vec3::ZERO,
                scale: 0.6f32.powi(4),
                turn: Turn::about(Vec3::Y, per_period * 4.0),
            }),
            ease: None,
            seconds: Some(10.0),
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
        let z = path.zoom_loop.expect("pythagoras-zoomy closes under its zoom symmetry");

        let start = path.sample(0.0);
        let end = path.sample(SEAM);
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
        let bare = CameraPath { zoom_loop: None, ..one };
        assert!(!bare.playable(), "one key and no zoom loop is a scene mid-authoring");
    }

    #[test]
    fn a_zoom_loop_does_not_ease() {
        assert!(!zoom_loop_path(1, vec![key(0.0, 0.4, 4.0, Vec3::ZERO)]).eased());
    }

    // -- defaults, resolution, housekeeping --------------------------------

    #[test]
    fn full_orbit_matches_auto_orbit() {
        let base = OrbitCamera::from_chart(0.7, 0.3, 0.0, 4.0, Vec3::X);
        let p = CameraPath::full_orbit(&base);
        let mid = p.sample(0.5).chart();
        let want = base.chart();
        assert!(
            (Angle::from_radians(want.yaw.radians() + std::f32::consts::PI)
                .shortest_to(mid.yaw)
                .abs())
                < 1e-3
        );
        assert!((mid.pitch.radians() - want.pitch.radians()).abs() < 1e-4);
        assert!((p.sample(0.5).distance - base.distance).abs() < 1e-3);
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
        let one_key = CameraPath::new(vec![key(1.0, 0.2, 2.0, Vec3::Z)], false);
        assert!(std::ptr::eq(resolve(Some(&one_key), &default), &default));

        let authored = linear_path(false);
        assert!(std::ptr::eq(resolve(Some(&authored), &default), &authored));
    }

    #[test]
    fn open_path_eases_by_default() {
        let p = CameraPath { ease: None, ..linear_path(false) };
        assert!(p.eased());
        // Eased start moves slower than constant speed
        assert!(swept_about_y(&p, 0.0, 0.05, 32).abs() < 0.05);
        let p_closed = CameraPath { ease: None, ..linear_path(true) };
        assert!(!p_closed.eased());
    }

    #[test]
    fn degenerate_paths_are_safe() {
        let empty = CameraPath::new(vec![], false);
        assert!(empty.sample(0.5).distance > 0.0);

        let single = CameraPath { closed: true, ..CameraPath::new(vec![key(1.0, 0.2, 2.0, Vec3::Z)], true) };
        let cam = single.sample(0.7);
        assert!(cam.orientation.angle_to(single.keys[0].orientation) < 1e-6);
        assert!((cam.focus - Vec3::Z).length() < 1e-6);
    }

    #[test]
    fn windings_keep_pace_with_the_keys() {
        // The route list and the segment list must never drift apart, or a
        // winding silently starts describing a different segment.
        let mut p = linear_path(false);
        p.fit_windings();
        assert_eq!(p.windings.len(), p.segments());

        p.keys.push(key(3.0, 0.1, 2.0, Vec3::ZERO));
        p.fit_windings();
        assert_eq!(p.windings.len(), p.segments());

        p.closed = true;
        p.fit_windings();
        assert_eq!(p.windings.len(), p.segments(), "closing adds the wrap-around segment");

        p.keys.truncate(2);
        p.fit_windings();
        assert_eq!(p.windings.len(), p.segments());

        // A path that never mentions windings behaves as all-zero.
        assert_eq!(CameraPath::new(vec![], false).winding(7), 0);
    }
}

