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

use glam::Vec3;

use crate::camera::OrbitCamera;

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

/// A spline camera path through two or more keypoints
#[derive(Clone, Debug)]
pub struct CameraPath {
    pub keys: Vec<PathKey>,
    /// Loop back to the first key after the last (seamless loops)
    pub closed: bool,
    /// Ease in/out (smoothstep on path time); None = default (!closed)
    pub ease: Option<bool>,
    /// Suggested playback/render duration; None = 3s per segment
    pub seconds: Option<f32>,
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
    /// Number of spline segments (closed paths add the wrap-around segment)
    pub fn segments(&self) -> usize {
        match (self.keys.len(), self.closed) {
            (0, _) | (1, _) => 0,
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
        self.ease.unwrap_or(!self.closed)
    }

    /// A seamless full-turn orbit at the given base framing — the default
    /// animation for scenes that don't author a path
    pub fn full_orbit(base: &OrbitCamera) -> Self {
        let tau = std::f32::consts::TAU;
        let keys = (0..4)
            .map(|i| PathKey {
                yaw: base.yaw + i as f32 * tau / 4.0,
                ..PathKey::from_camera(base)
            })
            .collect();
        Self { keys, closed: true, ease: Some(false), seconds: None }
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
        if !self.closed {
            return self.keys[i.clamp(0, n - 1) as usize];
        }
        let idx = i.rem_euclid(n) as usize;
        let mut k = self.keys[idx];
        let turns = (i - idx as isize) / n; // whole loops i is offset by
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
        let (t, loops) = if self.closed {
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
    fn roll_interpolates_along_the_path() {
        let p = CameraPath {
            keys: vec![
                PathKey { yaw: 0.0, pitch: 0.0, distance: 3.0, focus: Vec3::ZERO, roll: 0.0 },
                PathKey { yaw: 1.0, pitch: 0.0, distance: 3.0, focus: Vec3::ZERO, roll: 1.0 },
            ],
            closed: false,
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
        let empty = CameraPath { keys: vec![], closed: false, ease: None, seconds: None };
        let cam = empty.sample(0.5);
        assert!(cam.distance > 0.0);
        let single = CameraPath {
            keys: vec![key(1.0, 0.2, 2.0, Vec3::Z)],
            closed: true,
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
