//! Camera paths: Catmull-Rom splines over orbit-camera keypoints
//!
//! A path is a sequence of [`PathKey`]s — orbit-camera parameters
//! (yaw/pitch/distance/focus) — interpolated by a uniform Catmull-Rom spline.
//! Splining in orbit-parameter space keeps every intermediate camera a valid
//! orbit framing: yaw interpolation sweeps around the subject, and the focus
//! point travels on its own spline, so look directions blend smoothly even
//! while the eye moves (the camera always looks at the interpolated focus).
//!
//! Conventions:
//! - `yaw` is unbounded: keys at 0, 3.14, 6.28 author a full turn, and larger
//!   deltas author multiple turns. Nothing is wrapped behind your back.
//! - `distance` interpolates in log space, so zooms run at a constant
//!   *relative* rate (halving the distance always takes the same time).
//! - Closed paths return to the first key; the closing yaw segment takes the
//!   shortest way around (so keys at 0°/90°/180°/270° close forward through
//!   360°, not backward through 0°).
//! - Open paths ease in/out by default (smoothstep on path time); closed
//!   paths default to constant speed so the loop seam is invisible.

use glam::{Quat, Vec3};

use crate::camera::OrbitCamera;

/// Angular speed of the default orbit, in rad/s — a full turn every ~35s.
///
/// This was `yaw += 0.18 * dt` applied straight to the camera, back when the
/// turntable was its own mechanism. It survives only as [`CameraPath::
/// full_orbit`]'s duration, which is the one place the number still means
/// anything.
pub const ORBIT_RATE: f32 = 0.18;

/// One spline keypoint: a full orbit-camera framing
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathKey {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub focus: Vec3,
    /// View-axis roll (radians). Interpolated like yaw — unwrapped, so a key
    /// authored a full turn away rolls the whole way rather than snapping.
    pub roll: f32,
}

impl PathKey {
    pub fn from_camera(cam: &OrbitCamera) -> Self {
        Self {
            yaw: cam.yaw,
            pitch: cam.pitch,
            distance: cam.distance,
            focus: cam.focus,
            roll: cam.roll,
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
    pub rot: Quat,
}

impl ZoomLoop {
    /// Carry a keypoint `n` loops forward (negative = backward)
    pub fn advance(&self, key: PathKey, n: i32) -> PathKey {
        let mut cam = OrbitCamera {
            yaw: key.yaw,
            pitch: key.pitch,
            distance: key.distance,
            focus: key.focus,
            roll: key.roll,
        };
        // Powers of a similarity are closed-form, so a key twenty loops out
        // costs the same as one loop out.
        let scale = self.scale.powi(n);
        let (axis, angle) = self.rot.to_axis_angle();
        cam.apply_similarity(self.center, scale, Quat::from_axis_angle(axis, angle * n as f32));
        // apply_similarity re-derives yaw from the eye, which wraps it into
        // (-pi, pi]; the path convention is that yaw is unbounded, and a
        // multi-turn loop has to keep counting. Put the turns back.
        cam.yaw = key.yaw + angle_about_y(self.rot) * n as f32;
        PathKey::from_camera(&cam)
    }
}

/// The Y-component of a rotation, as a yaw delta. Exact for the common case of
/// a map that twists about the vertical, and the best available answer for one
/// that doesn't — where the eye path isn't a pure yaw sweep anyway.
///
/// Taken the short way round. `Quat::to_axis_angle` reports the angle in
/// [0, 2π], so a map that twists a fraction of a degree one way comes back as
/// very nearly a full turn the other, and the camera swung 359° to arrive
/// where 1° would have done. Yaw is an angle: representatives 2π apart frame
/// the identical picture, so the nearest one ends the loop on exactly the same
/// frame while sweeping at most half a turn to get there.
///
/// Scaling this by the loop count stays linear, so the spline's out-of-range
/// keys remain evenly spaced and the seam keeps its smooth Catmull-Rom
/// treatment — see [`CameraPath::key`].
fn angle_about_y(rot: Quat) -> f32 {
    let (axis, angle) = rot.to_axis_angle();
    shortest_angle(angle * axis.y)
}

/// A spline camera path through two or more keypoints
#[derive(Clone, Debug)]
pub struct CameraPath {
    pub keys: Vec<PathKey>,
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

/// Wrap an angle difference into (-pi, pi]
fn shortest_angle(d: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let d = d.rem_euclid(tau);
    if d > std::f32::consts::PI { d - tau } else { d }
}

impl CameraPath {
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

    /// A seamless full-turn orbit at the given base framing.
    ///
    /// This is *the* path for a scene that authors none — the same object in
    /// the app (where it's the turntable you watch), in the viewport (where
    /// it's drawn like any other path), and offline (where `--render x.avif`
    /// flies it). There's no second turntable system beside the path system;
    /// there's one path system with this as its default.
    pub fn full_orbit(base: &OrbitCamera) -> Self {
        let tau = std::f32::consts::TAU;
        let keys = (0..4)
            .map(|i| PathKey {
                yaw: base.yaw + i as f32 * tau / 4.0,
                ..PathKey::from_camera(base)
            })
            .collect();
        Self {
            keys,
            closed: true,
            zoom_loop: None,
            ease: Some(false),
            seconds: Some(tau / ORBIT_RATE),
        }
    }

    /// Yaw swept by one full loop of a closed path: the authored keys' net
    /// sweep, closed back to key 0 the shortest way around
    fn winding(&self) -> f32 {
        let (first, last) = (self.keys[0].yaw, self.keys[self.keys.len() - 1].yaw);
        (last - first) + shortest_angle(first - last)
    }

    /// Key value for spline index i, where i ranges over -1..=segments()+1.
    /// Open paths clamp at the ends; closed paths wrap, with yaw continued
    /// by the loop's total winding so multi-turn loops stay monotonic.
    fn key(&self, i: isize) -> PathKey {
        let n = self.keys.len() as isize;
        let idx = i.rem_euclid(n) as usize;
        let turns = ((i - idx as isize) / n) as i32; // whole loops i is offset by

        // A zoom loop's out-of-range keys are the in-range ones carried by the
        // symmetry, which is what makes the spline periodic *in appearance*
        // rather than in parameter space. It also means no clamping at the
        // ends: the seam gets the same smooth Catmull-Rom treatment as any
        // interior segment, so the loop has no velocity kink either.
        if let Some(z) = &self.zoom_loop {
            return z.advance(self.keys[idx], turns);
        }
        if !self.closed {
            return self.keys[i.clamp(0, n - 1) as usize];
        }
        let mut k = self.keys[idx];
        k.yaw += self.winding() * turns as f32;
        k
    }

    /// Sample the path at t in [0, 1] (clamped; closed paths wrap seamlessly)
    pub fn sample(&self, t: f32) -> OrbitCamera {
        let segs = self.segments();
        if segs == 0 {
            let k = self.keys.first().copied().unwrap_or(PathKey {
                yaw: 0.0,
                pitch: 0.0,
                distance: 3.0,
                focus: Vec3::ZERO,
                roll: 0.0,
            });
            return OrbitCamera {
                yaw: k.yaw,
                pitch: k.pitch,
                distance: k.distance,
                focus: k.focus,
                roll: k.roll,
            };
        }

        // Closed paths wrap; whole loops already completed keep accumulating
        // yaw so multi-loop playback stays monotonic
        // A zoom loop needs no accumulation across loops: every loop renders
        // the identical picture, and the renderer folds the camera back into
        // the band anyway (`Renorm::wrap`), so the second pass is the first.
        let (t, loops) = if self.zoom_loop.is_some() {
            (t.rem_euclid(1.0), 0.0)
        } else if self.closed {
            (t.rem_euclid(1.0), t.div_euclid(1.0))
        } else {
            (t.clamp(0.0, 1.0), 0.0)
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

        let cr3 = |f: fn(&PathKey) -> f32| catmull_rom(f(&k0), f(&k1), f(&k2), f(&k3), u);
        OrbitCamera {
            yaw: cr3(|k| k.yaw) + if self.closed { self.winding() * loops } else { 0.0 },
            pitch: cr3(|k| k.pitch),
            distance: cr3(|k| k.distance.max(1e-3).ln()).exp(),
            focus: Vec3::new(
                cr3(|k| k.focus.x),
                cr3(|k| k.focus.y),
                cr3(|k| k.focus.z),
            ),
            roll: cr3(|k| k.roll),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::world_to_screen;

    /// A 0.6 spiral about the origin, twisting 34° about Y — `wellspiral`'s
    /// descent map, as a loop closing over one period.
    fn zoom_loop_path(periods: u32, keys: Vec<PathKey>) -> CameraPath {
        let angle = 34f32.to_radians() * periods as f32;
        CameraPath {
            keys,
            closed: false,
            zoom_loop: Some(ZoomLoop {
                periods,
                center: Vec3::ZERO,
                scale: 0.6f32.powi(periods as i32),
                rot: Quat::from_rotation_y(angle),
            }),
            ease: None,
            seconds: Some(10.0),
        }
    }

    /// `sample` wraps t, so t=1 *is* t=0 — correct for playback, since the
    /// renderer folds the camera back into the band anyway. The seam is
    /// therefore probed just short of it.
    const SEAM: f32 = 1.0 - 1e-4;

    #[test]
    fn a_zoom_loop_ends_on_the_frame_it_started() {
        // The whole claim: the last frame is not *near* the first, it is the
        // first — because the scene is invariant under the similarity that
        // separates them. Asserted the way the wrap test is: a point seen by
        // the end camera lands where its image under the symmetry sat for the
        // start camera, and the invariant set contains both.
        let path = zoom_loop_path(1, vec![key(0.3, 0.5, 3.6, Vec3::ZERO)]);
        let (start, end) = (path.sample(0.0), path.sample(SEAM));
        let z = path.zoom_loop.unwrap();
        let vp_start = start.view_proj(16.0 / 9.0);
        let vp_end = end.view_proj(16.0 / 9.0);
        for i in 0..25 {
            let x = Vec3::new(
                0.9 * (i as f32 * 1.7).sin(),
                0.9 * (i as f32 * 0.9).cos(),
                0.9 * (i as f32 * 2.3).sin(),
            );
            // The end camera is the start camera carried by the symmetry, so
            // it sees A(x) exactly where the start camera saw x — and the
            // invariant set contains both, so the frames match.
            let partner = z.center + z.rot * ((x - z.center) * z.scale);
            match (
                world_to_screen(partner, vp_end, 1280.0, 720.0),
                world_to_screen(x, vp_start, 1280.0, 720.0),
            ) {
                // One ten-thousandth of a loop short of the seam, so a pixel
                // of slack rather than none
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
        // Carrying the last camera back by one loop must land on the first,
        // give or take the sliver of motion we stopped short by. If this
        // drifts, every loop of a played animation steps a little.
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
        assert!((carried.yaw - start.yaw).abs() < 1e-3, "yaw {} vs {}", carried.yaw, start.yaw);
        assert!((carried.pitch - start.pitch).abs() < 1e-3);
    }

    #[test]
    fn a_one_key_zoom_loop_is_a_constant_rate_zoom() {
        // Out-of-range keys are the in-range one carried by the symmetry, so
        // log-distance and yaw are arithmetic sequences, and Catmull-Rom
        // through equally-spaced collinear points is exactly linear. This is
        // why the loop has no velocity kink and needs no easing.
        let path = zoom_loop_path(1, vec![key(0.0, 0.4, 4.0, Vec3::ZERO)]);
        assert_eq!(path.segments(), 1, "a single key plus its image is one segment");
        for i in 0..=20 {
            let t = i as f32 / 20.0 * SEAM;
            let want = 4.0f32.ln() + t * 0.6f32.ln();
            let got = path.sample(t).distance.ln();
            assert!((got - want).abs() < 1e-3, "t={t}: log-distance {got}, constant rate wants {want}");
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
        // that turns 359.5° the other, and `Quat::to_axis_angle` hands back the
        // latter whenever the scalar part comes out negative. Read literally,
        // that swung the camera almost the whole way round to arrive where a
        // half-degree nudge would have done.
        let path = CameraPath {
            keys: vec![key(0.0, 0.4, 4.0, Vec3::ZERO)],
            closed: false,
            zoom_loop: Some(ZoomLoop {
                periods: 1,
                center: Vec3::ZERO,
                scale: 0.6,
                rot: Quat::from_rotation_y(359.5f32.to_radians()),
            }),
            ease: None,
            seconds: Some(10.0),
        };
        let swept = path.sample(SEAM).yaw - path.sample(0.0).yaw;
        assert!(
            swept.abs() < 1f32.to_radians(),
            "swept {:.1}° to make a half-degree turn",
            swept.to_degrees()
        );
        assert!(swept < 0.0, "359.5° is a turn the other way, swept {:.1}°", swept.to_degrees());

        // And it still closes: the two representatives are a full turn apart,
        // so the eye ends exactly where the symmetry carries it either way.
        let z = path.zoom_loop.unwrap();
        let carried = z.center + z.rot * ((path.sample(0.0).eye() - z.center) * z.scale);
        let landed = path.sample(SEAM).eye();
        assert!(
            (landed - carried).length() < 1e-3,
            "ended at {landed:?}, symmetry says {carried:?}"
        );
    }

    #[test]
    fn one_key_plus_a_zoom_loop_is_playable() {
        // The transport gate used to count keypoints instead of asking this,
        // so a one-key zoom loop could be authored, drawn and flown — but not
        // *started*, since two keys were demanded to begin playing.
        let one = zoom_loop_path(1, vec![key(0.0, 0.4, 4.0, Vec3::ZERO)]);
        assert!(one.playable(), "a single key closing under the symmetry is a path");

        // Without the symmetry there is nothing for that key to run to.
        let bare = CameraPath { zoom_loop: None, ..one };
        assert!(!bare.playable(), "one key and no zoom loop is a scene mid-authoring");
    }

    #[test]
    fn a_zoom_loop_does_not_ease() {
        // Easing parks the camera at both ends, which on a loop is a visible
        // stall at the seam — the one place a zoom must not pause.
        assert!(!zoom_loop_path(1, vec![key(0.0, 0.4, 4.0, Vec3::ZERO)]).eased());
    }


    fn key(yaw: f32, pitch: f32, dist: f32, focus: Vec3) -> PathKey {
        PathKey { yaw, pitch, distance: dist, focus, roll: 0.0 }
    }

    fn linear_path(closed: bool) -> CameraPath {
        CameraPath {
            keys: vec![
                key(0.0, 0.1, 2.0, Vec3::ZERO),
                key(1.0, 0.3, 4.0, Vec3::X),
                key(2.0, 0.2, 3.0, Vec3::Y),
            ],
            closed,
            zoom_loop: None,
            ease: Some(false),
            seconds: None,
        }
    }

    #[test]
    fn passes_through_keys() {
        let p = linear_path(false);
        for (i, k) in p.keys.iter().enumerate() {
            let t = i as f32 / (p.keys.len() - 1) as f32;
            let cam = p.sample(t);
            assert!((cam.yaw - k.yaw).abs() < 1e-4, "key {} yaw", i);
            assert!((cam.pitch - k.pitch).abs() < 1e-4, "key {} pitch", i);
            assert!((cam.distance - k.distance).abs() < 1e-3, "key {} dist", i);
            assert!((cam.focus - k.focus).length() < 1e-4, "key {} focus", i);
        }
    }

    #[test]
    fn closed_path_is_seamless() {
        let p = linear_path(true);
        let a = p.sample(0.0);
        let b = p.sample(1.0);
        // Same eye position at the seam (yaw may differ by a full winding)
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
        // forward through 360, not swing back through 0
        let tau = std::f32::consts::TAU;
        let p = CameraPath {
            keys: (0..4)
                .map(|i| key(i as f32 * tau / 4.0, 0.2, 3.0, Vec3::ZERO))
                .collect(),
            closed: true,
            zoom_loop: None,
            ease: Some(false),
            seconds: None,
        };
        // Uniform keys on a line: CR reproduces linear motion exactly
        for i in 0..=16 {
            let t = i as f32 / 16.0;
            let cam = p.sample(t);
            assert!(
                (cam.yaw - t * tau).abs() < 1e-3,
                "t={}: yaw {} vs {}",
                t,
                cam.yaw,
                t * tau
            );
        }
    }

    #[test]
    fn full_orbit_matches_auto_orbit() {
        let base = OrbitCamera { yaw: 0.7, pitch: 0.3, distance: 4.0, focus: Vec3::X, roll: 0.0 };
        let p = CameraPath::full_orbit(&base);
        let mid = p.sample(0.5);
        assert!((mid.yaw - (base.yaw + std::f32::consts::PI)).abs() < 1e-3);
        assert!((mid.pitch - base.pitch).abs() < 1e-4);
        assert!((mid.distance - base.distance).abs() < 1e-3);
    }

    #[test]
    fn resolve_prefers_authored_keys_but_needs_two() {
        let base = OrbitCamera { yaw: 0.0, pitch: 0.0, distance: 3.0, focus: Vec3::ZERO, roll: 0.0 };
        let default = CameraPath::full_orbit(&base);

        // No path at all, and a path still being built one key at a time: the
        // default flies, so the camera is never stranded on an unflyable path.
        assert!(std::ptr::eq(resolve(None, &default), &default));
        let one_key = CameraPath {
            keys: vec![key(1.0, 0.2, 2.0, Vec3::Z)],
            closed: false,
            zoom_loop: None,
            ease: None,
            seconds: None,
        };
        assert!(std::ptr::eq(resolve(Some(&one_key), &default), &default));

        // Two keys is a path, and it wins.
        let authored = linear_path(false);
        assert!(std::ptr::eq(resolve(Some(&authored), &default), &authored));
    }

    #[test]
    fn full_orbit_turns_at_the_orbit_rate() {
        // The default path's duration *is* the old turntable's angular speed:
        // one full turn of yaw, at ORBIT_RATE rad/s. Pin it, since the app and
        // the offline animation path both take their default motion from here.
        let base = OrbitCamera { yaw: 0.0, pitch: 0.2, distance: 3.0, focus: Vec3::ZERO, roll: 0.0 };
        let p = CameraPath::full_orbit(&base);
        let swept = p.sample(1.0).yaw - p.sample(0.0).yaw;
        assert!((swept - std::f32::consts::TAU).abs() < 1e-3, "swept {}", swept);
        assert!((swept / p.duration() - ORBIT_RATE).abs() < 1e-4);
    }

    #[test]
    fn roll_interpolates_along_the_path() {
        let p = CameraPath {
            keys: vec![
                PathKey { yaw: 0.0, pitch: 0.0, distance: 3.0, focus: Vec3::ZERO, roll: 0.0 },
                PathKey { yaw: 1.0, pitch: 0.0, distance: 3.0, focus: Vec3::ZERO, roll: 1.0 },
            ],
            closed: false,
            zoom_loop: None,
            ease: Some(false),
            seconds: None,
        };
        assert!((p.sample(0.0).roll - 0.0).abs() < 1e-4);
        assert!((p.sample(0.5).roll - 0.5).abs() < 1e-3);
        assert!((p.sample(1.0).roll - 1.0).abs() < 1e-4);
    }

    #[test]
    fn distance_interpolates_geometrically() {
        // Two keys differing only in distance: the midpoint of a log-space
        // lerp is the geometric mean
        let p = CameraPath {
            keys: vec![
                key(0.0, 0.0, 1.0, Vec3::ZERO),
                key(0.0, 0.0, 4.0, Vec3::ZERO),
            ],
            closed: false,
            zoom_loop: None,
            ease: Some(false),
            seconds: None,
        };
        let mid = p.sample(0.5);
        assert!((mid.distance - 2.0).abs() < 1e-3, "got {}", mid.distance);
    }

    #[test]
    fn open_path_eases_by_default() {
        let p = CameraPath { ease: None, ..linear_path(false) };
        assert!(p.eased());
        // Eased start moves slower than constant speed
        let early = p.sample(0.05);
        assert!((early.yaw - p.keys[0].yaw).abs() < 0.05);
        let p_closed = CameraPath { ease: None, ..linear_path(true) };
        assert!(!p_closed.eased());
    }

    #[test]
    fn degenerate_paths_are_safe() {
        let empty = CameraPath { keys: vec![], closed: false, zoom_loop: None, ease: None, seconds: None };
        let cam = empty.sample(0.5);
        assert!(cam.distance > 0.0);
        let single = CameraPath {
            keys: vec![key(1.0, 0.2, 2.0, Vec3::Z)],
            closed: true,
            zoom_loop: None,
            ease: None,
            seconds: None,
        };
        let cam = single.sample(0.7);
        assert!((cam.yaw - 1.0).abs() < 1e-6);
        assert!((cam.focus - Vec3::Z).length() < 1e-6);
    }

    #[test]
    fn multi_turn_spiral() {
        // Authored yaw spanning two full turns stays monotonic (no wrapping)
        let tau = std::f32::consts::TAU;
        let p = CameraPath {
            keys: vec![
                key(0.0, 0.0, 5.0, Vec3::ZERO),
                key(tau, 0.0, 3.0, Vec3::ZERO),
                key(2.0 * tau, 0.0, 1.0, Vec3::ZERO),
            ],
            closed: false,
            zoom_loop: None,
            ease: Some(false),
            seconds: None,
        };
        let mut last = -1.0f32;
        for i in 0..=32 {
            let cam = p.sample(i as f32 / 32.0);
            assert!(cam.yaw > last, "yaw must increase monotonically");
            last = cam.yaw;
        }
        assert!((last - 2.0 * tau).abs() < 1e-2);
    }
}
