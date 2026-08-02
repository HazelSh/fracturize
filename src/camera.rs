//! Orbit camera shared by the interactive app and the offline renderer
//!
//! The camera orbits `focus` at `distance`, parameterized by yaw (around Y)
//! and pitch (elevation). The eye always sits exactly on the orbit sphere —
//! the legacy scene/view `offset` (an eye displacement outside the sphere,
//! which made pitching drift the view distance) is folded into equivalent
//! yaw/pitch/distance at load time via [`OrbitCamera::from_legacy`].

use glam::{Mat4, Quat, Vec3};

pub const FOV_Y_RADIANS: f32 = std::f32::consts::FRAC_PI_4; // 45°
pub const Z_NEAR: f32 = 0.1;
pub const Z_FAR: f32 = 100.0;

/// Keep pitch away from the poles so look_at's Y-up basis stays valid
const PITCH_LIMIT: f32 = 1.53; // ~87.7°

#[derive(Clone, Copy, Debug)]
pub struct OrbitCamera {
    /// Orbit angle around Y (radians)
    pub yaw: f32,
    /// Elevation angle (radians, positive = above the focus)
    pub pitch: f32,
    /// Orbit radius
    pub distance: f32,
    /// Orbit center / look-at point
    pub focus: Vec3,
    /// Rotation of the camera about its own view axis (radians). Not part of
    /// the orbit — it doesn't move the eye — but it is part of the framing,
    /// so it travels with the other three everywhere they go.
    ///
    /// Deliberately not `#[derive(Default)]`-able: a missing `roll` in a
    /// struct literal has to be a compile error, because a silently-zeroed
    /// one would reset the framing on the next save/load round trip and
    /// nothing would report it.
    pub roll: f32,
}

impl OrbitCamera {
    /// Build from legacy parameters where `offset` displaced the eye off the
    /// orbit sphere: fold it into an equivalent on-sphere yaw/pitch/distance
    /// (the eye position is preserved exactly; orbiting then keeps the view
    /// distance constant instead of drifting with pitch)
    pub fn from_legacy(
        focus: Vec3,
        offset: Vec3,
        distance: f32,
        yaw: f32,
        pitch: f32,
        roll: f32,
    ) -> Self {
        let (sp, cp) = pitch.sin_cos();
        let (sy, cy) = yaw.sin_cos();
        let eye = focus + offset + distance * Vec3::new(cp * sy, sp, cp * cy);
        let v = eye - focus;
        let d = v.length().max(1e-4);
        Self {
            yaw: v.x.atan2(v.z),
            pitch: (v.y / d).clamp(-1.0, 1.0).asin(),
            distance: d,
            focus,
            roll,
        }
    }

    /// Rebuild the orbit parameters from a camera basis. `up_reference` is the
    /// world-up handed to `look_at` (what `up_reference()` returns), not
    /// the camera's own up; only its component perpendicular to the view axis
    /// matters, which is what makes this the exact inverse of the accessors.
    ///
    /// The one thing here that isn't obvious is roll: it is recovered as the
    /// signed angle from world Y to `up_reference` about the view axis, since
    /// that is precisely how `up_reference()` builds it.
    pub fn from_eye_focus_up(eye: Vec3, focus: Vec3, up_reference: Vec3) -> Self {
        let v = eye - focus;
        let d = v.length().max(1e-9);
        let forward = -v / d;

        let flatten = |u: Vec3| u - forward * u.dot(forward);
        let base = flatten(Vec3::Y);
        let target = flatten(up_reference);
        let roll = if base.length_squared() < 1e-12 || target.length_squared() < 1e-12 {
            0.0
        } else {
            let (base, target) = (base.normalize(), target.normalize());
            base.cross(target).dot(forward).atan2(base.dot(target))
        };

        Self {
            yaw: v.x.atan2(v.z),
            pitch: (v.y / d).clamp(-1.0, 1.0).asin(),
            distance: d,
            focus,
            roll,
        }
    }

    /// Move the whole camera by a similarity about `center`: scale the eye and
    /// focus toward it by `scale`, and rotate the framing by `rot`.
    ///
    /// This is how [`crate::renorm::Renorm::wrap`] steps a zoom period. It
    /// transforms the camera exactly as the world would be transformed, so a
    /// set invariant under that similarity renders identically afterwards.
    pub fn apply_similarity(&mut self, center: Vec3, scale: f32, rot: Quat) {
        let eye = center + rot * ((self.eye() - center) * scale);
        let focus = center + rot * ((self.focus - center) * scale);
        let up = rot * self.up_reference();
        *self = Self::from_eye_focus_up(eye, focus, up);
    }

    pub fn eye(&self) -> Vec3 {
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        self.focus + self.distance * Vec3::new(cp * sy, sp, cp * cy)
    }

    /// The "world up" handed to `look_at`, rolled about the view axis.
    /// Everything else that needs a camera basis goes through this, so pans
    /// and gizmo drags stay aligned with what's on screen when rolled.
    fn up_reference(&self) -> Vec3 {
        if self.roll == 0.0 {
            Vec3::Y
        } else {
            Quat::from_axis_angle(self.forward(), self.roll) * Vec3::Y
        }
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.focus, self.up_reference())
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(FOV_Y_RADIANS, aspect, Z_NEAR, Z_FAR) * self.view_matrix()
    }

    pub fn forward(&self) -> Vec3 {
        (self.focus - self.eye()).normalize_or(Vec3::NEG_Z)
    }

    /// Camera-space right in world coordinates
    pub fn right(&self) -> Vec3 {
        self.forward().cross(self.up_reference()).normalize_or(Vec3::X)
    }

    /// Camera-space up in world coordinates
    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward())
    }

    /// Mouse drag orbit: dx/dy in pixels. Grab-the-scene convention: drag
    /// right spins the scene rightward, drag up tilts its top toward you.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.006;
        self.pitch = (self.pitch - dy * 0.006).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// How many pixels one world unit spans at depth `distance` from the eye.
    /// The perspective scale factor, in the one form everything here wants it.
    pub fn pixels_per_world_unit(distance: f32, viewport_height: f32) -> f32 {
        let d = distance.max(1e-4);
        viewport_height / (2.0 * d * (FOV_Y_RADIANS * 0.5).tan())
    }

    /// Mouse drag pan: moves the focus in the view plane ("grab the scene")
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        // World size of one pixel at the focus distance
        let per_pixel = 1.0 / Self::pixels_per_world_unit(self.distance, viewport_height);
        self.focus += (self.up() * dy - self.right() * dx) * per_pixel;
    }

    /// Scroll zoom: positive = zoom in
    pub fn zoom(&mut self, steps: f32) {
        self.distance = (self.distance * 0.9f32.powf(steps)).clamp(0.05, 80.0);
    }

    /// Right-drag roll: horizontal drag spins the horizon. Kept unwrapped so
    /// a path key at 2π reads as a full turn rather than as zero — same
    /// convention as `PathKey::yaw`.
    pub fn roll_by(&mut self, dx: f32) {
        self.roll += dx * 0.006;
    }
}

/// Command-line camera overrides: whatever the scene or view says, but with
/// these fields replaced.
///
/// This exists because the alternative was writing a view file for every
/// framing you want to look at once, and a view file is nine required fields
/// (two of them legacy) to change one number. Applied after the scene and any
/// `--view`, so it wins over both.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraOverride {
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub distance: Option<f32>,
    pub roll: Option<f32>,
    pub focus: Option<Vec3>,
}

impl CameraOverride {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn apply(&self, cam: &mut OrbitCamera) {
        if let Some(v) = self.yaw {
            cam.yaw = v;
        }
        if let Some(v) = self.pitch {
            cam.pitch = v;
        }
        if let Some(v) = self.distance {
            cam.distance = v.max(1e-4);
        }
        if let Some(v) = self.roll {
            cam.roll = v;
        }
        if let Some(v) = self.focus {
            cam.focus = v;
        }
    }

    /// How this framing would be written in a scene's `[camera]` block, so a
    /// framing found by flags can be kept without transcribing it by hand.
    pub fn describe(cam: &OrbitCamera) -> String {
        format!(
            "[camera]\nyaw = {:.4}\npitch = {:.4}\ndistance = {:.4}\nfocus = [{:.4}, {:.4}, {:.4}]{}",
            cam.yaw,
            cam.pitch,
            cam.distance,
            cam.focus.x,
            cam.focus.y,
            cam.focus.z,
            if cam.roll == 0.0 { String::new() } else { format!("\nroll = {:.4}", cam.roll) },
        )
    }
}

/// Project a world-space position to screen pixel coordinates
pub fn world_to_screen(pos: Vec3, view_proj: Mat4, w: f32, h: f32) -> Option<(f32, f32)> {
    let clip = view_proj * pos.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(((ndc.x * 0.5 + 0.5) * w, (1.0 - (ndc.y * 0.5 + 0.5)) * h))
}

/// Unproject a cursor position to a world-space ray (origin, direction)
pub fn cursor_ray(inv_view_proj: Mat4, x: f32, y: f32, w: f32, h: f32) -> (Vec3, Vec3) {
    let ndc_x = x / w * 2.0 - 1.0;
    let ndc_y = 1.0 - y / h * 2.0;
    let near = inv_view_proj * glam::Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
    let far = inv_view_proj * glam::Vec4::new(ndc_x, ndc_y, 0.9999, 1.0);
    let near = near.truncate() / near.w;
    let far = far.truncate() / far.w;
    (near, (far - near).normalize_or(Vec3::NEG_Z))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_offset_folding_preserves_eye() {
        // The old camera model: eye = focus + offset + yaw-orbit at distance.
        // from_legacy must produce the identical eye on the orbit sphere.
        let focus = Vec3::new(0.1, 0.2, 0.3);
        let offset = Vec3::new(0.0, 1.5, -0.2);
        let (yaw, distance) = (1.25f32, 3.0f32);
        let legacy_eye =
            focus + offset + Vec3::new(yaw.sin() * distance, 0.0, yaw.cos() * distance);

        let cam = OrbitCamera::from_legacy(focus, offset, distance, yaw, 0.0, 0.0);
        assert!((cam.eye() - legacy_eye).length() < 1e-4);
        // And the distance is now the true eye-focus distance (constant under pitch)
        assert!((cam.distance - (legacy_eye - focus).length()).abs() < 1e-4);
        // Folding is idempotent once offset is zero
        let cam2 =
            OrbitCamera::from_legacy(cam.focus, Vec3::ZERO, cam.distance, cam.yaw, cam.pitch, 0.0);
        assert!((cam2.eye() - cam.eye()).length() < 1e-4);
    }

    #[test]
    fn roll_spins_the_horizon_without_moving_the_eye() {
        let level = OrbitCamera {
            yaw: 0.4,
            pitch: 0.2,
            distance: 3.0,
            focus: Vec3::ZERO,
            roll: 0.0,
        };
        let rolled = OrbitCamera { roll: std::f32::consts::FRAC_PI_2, ..level };

        // The eye is an orbit property; roll must not touch it.
        assert!((rolled.eye() - level.eye()).length() < 1e-5);
        assert!((rolled.forward() - level.forward()).length() < 1e-5);

        // A quarter turn takes the old up to the old right (or its negation,
        // depending on handedness) — either way it stops being the old up.
        assert!(
            rolled.up().dot(level.up()).abs() < 1e-3,
            "90 degrees of roll should leave up perpendicular to where it was"
        );
        // And the basis stays orthonormal, so pans in a rolled view still
        // move the scene with the pointer.
        assert!(rolled.right().dot(rolled.up()).abs() < 1e-5);
        assert!((rolled.right().length() - 1.0).abs() < 1e-5);
        assert!((rolled.up().length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn zero_roll_is_exactly_the_old_behaviour() {
        // Every existing scene and view loads with roll 0; their framing must
        // be bit-for-bit what it was before roll existed.
        let cam = OrbitCamera {
            yaw: 1.1,
            pitch: -0.3,
            distance: 2.5,
            focus: Vec3::new(0.2, 0.0, -0.1),
            roll: 0.0,
        };
        assert_eq!(cam.view_matrix(), Mat4::look_at_rh(cam.eye(), cam.focus, Vec3::Y));
    }

    #[test]
    fn eye_focus_up_roundtrips() {
        // from_eye_focus_up has to be the exact inverse of the accessors, or
        // the zoom wrap drifts a little every period and the seam opens up.
        for &roll in &[0.0f32, 0.9, -2.4, 3.0] {
            let cam = OrbitCamera {
                yaw: 1.9,
                pitch: -0.6,
                distance: 2.75,
                focus: Vec3::new(-0.3, 0.15, 0.8),
                roll,
            };
            let back = OrbitCamera::from_eye_focus_up(cam.eye(), cam.focus, cam.up_reference());
            assert!((back.eye() - cam.eye()).length() < 1e-5);
            assert!((back.distance - cam.distance).abs() < 1e-5);
            assert!((back.roll - cam.roll).abs() < 1e-4, "roll {} -> {}", cam.roll, back.roll);
            assert!(back.view_matrix().abs_diff_eq(cam.view_matrix(), 1e-4));
        }
    }

    #[test]
    fn similarity_moves_the_camera_like_the_world() {
        let cam = OrbitCamera {
            yaw: 0.5,
            pitch: 0.2,
            distance: 3.0,
            focus: Vec3::new(0.1, 0.0, 0.0),
            roll: 0.4,
        };
        let center = Vec3::new(0.2, -0.1, 0.3);
        let (scale, rot) = (0.5, Quat::from_rotation_y(0.6));
        let mut moved = cam;
        moved.apply_similarity(center, scale, rot);

        // A point and its image under the same similarity must land on the
        // same pixel, seen by the moved and original cameras respectively.
        let p = Vec3::new(0.7, -0.4, 0.25);
        let q = center + rot * ((p - center) * scale);
        let a = world_to_screen(q, moved.view_proj(1.6), 1280.0, 800.0).unwrap();
        let b = world_to_screen(p, cam.view_proj(1.6), 1280.0, 800.0).unwrap();
        assert!((a.0 - b.0).abs() < 0.05 && (a.1 - b.1).abs() < 0.05, "{:?} vs {:?}", a, b);
    }

    #[test]
    fn cursor_ray_center_points_at_focus() {
        let cam = OrbitCamera {
            yaw: 0.7,
            pitch: 0.3,
            distance: 3.0,
            focus: Vec3::ZERO,
            roll: 0.0,
        };
        let vp = cam.view_proj(16.0 / 9.0);
        let (origin, dir) = cursor_ray(vp.inverse(), 640.0, 360.0, 1280.0, 720.0);
        // The ray through the screen center passes through the focus
        let to_focus = (cam.focus - origin).normalize();
        assert!(dir.dot(to_focus) > 0.999, "dir {:?} vs {:?}", dir, to_focus);
    }

    #[test]
    fn screen_roundtrip() {
        let cam = OrbitCamera {
            yaw: 0.2,
            pitch: -0.4,
            distance: 4.0,
            focus: Vec3::new(0.5, 0.0, -0.2),
            roll: 0.0,
        };
        let vp = cam.view_proj(16.0 / 9.0);
        let p = Vec3::new(0.3, 0.1, 0.2);
        let (sx, sy) = world_to_screen(p, vp, 1280.0, 720.0).unwrap();
        let (origin, dir) = cursor_ray(vp.inverse(), sx, sy, 1280.0, 720.0);
        // p lies on the reconstructed ray
        let t = (p - origin).dot(dir);
        let closest = origin + dir * t;
        assert!((closest - p).length() < 1e-3);
    }
}
