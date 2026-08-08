//! Orbit camera shared by the interactive app and the offline renderer
//!
//! The camera orbits `focus` at `distance`, and its framing is a quaternion.
//! The eye always sits exactly on the orbit sphere — the legacy scene/view
//! `offset` (an eye displacement outside the sphere, which made pitching drift
//! the view distance) is folded into equivalent orientation/distance at load
//! time via [`OrbitCamera::from_legacy`].
//!
//! # Why not yaw/pitch/roll
//!
//! It used to be three angles plus a `look_at_rh`, and that gimbal-locked
//! three separate ways:
//!
//! 1. Pitch was clamped to ±87.7°, because past that the Y-up basis handed to
//!    `look_at` degenerated. Whole families of framings were simply
//!    unreachable, and so were the camera paths through them.
//! 2. Roll was applied by rotating world Y about the view axis. Looking
//!    straight down, the view axis *is* world Y, so roll did nothing at all —
//!    and the basis it fed `look_at` collapsed.
//! 3. Splined pitch was unclamped, so Catmull-Rom overshoot past ±π/2 silently
//!    produced a broken basis mid-flight.
//!
//! An [`Orientation`] has no poles, so none of those exist. Yaw/pitch/roll
//! survive as a *chart* — a human-readable naming of an orientation, used by
//! scene files, the CLI and the panel readout, and degenerate only where it
//! has always been degenerate. See [`crate::rot`].

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::rot::{Angle, Orientation, Turn, YawPitchRoll};

pub const FOV_Y_RADIANS: f32 = std::f32::consts::FRAC_PI_4; // 45°
pub const Z_NEAR: f32 = 0.1;
pub const Z_FAR: f32 = 100.0;

/// Radians of orbit or roll per pixel of drag
const DRAG_RATE: f32 = 0.006;

/// Which axis a horizontal orbit drag turns about — the whole of the
/// difference between the two ways a look control can feel.
///
/// Vertical drag is body-frame pitch either way, and so is roll; only the yaw
/// axis is in question, and the choice is a real fork rather than a tuning
/// knob. A turntable's feel is a function of where the camera points relative
/// to its privileged axis, so it changes as you fly; a trackball has no
/// privileged axis, so it can't.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OrbitStyle {
    /// Yaw about the camera's **own** up. A drag has the same effect relative
    /// to the screen from every framing there is — which is the point: you can
    /// step off a path anywhere, or open any scene, and look around without
    /// first working out what the camera had been doing.
    ///
    /// The price is that there is no horizon invariant: rotations about
    /// different axes don't commute, so a closed loop of drags comes back
    /// looking the same way but slightly rolled. That is a theorem about SO(3)
    /// rather than a defect, and it is why the roll readout and the `level`
    /// button exist. Nothing ever re-levels behind your back.
    #[default]
    Trackball,
    /// Yaw about **world** up, keeping the horizon level however the camera is
    /// rolled. The classic orbit, and right when a scene has a ground plane to
    /// be level with — but its feel depends on elevation, and near the poles
    /// world up is nearly the view axis, so the same drag reads as roll.
    Turntable,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    /// The framing. Its `Z` axis points from the focus out toward the eye, its
    /// `X` is camera right and its `Y` is camera up, so the camera looks along
    /// its own `-Z` — the usual right-handed convention.
    pub orientation: Orientation,
    /// Orbit radius
    pub distance: f32,
    /// Orbit center / look-at point
    pub focus: Vec3,
}

impl OrbitCamera {
    /// Build from the yaw/pitch/roll chart — the form scene files, view files
    /// and `--yaw`/`--pitch`/`--roll` are written in.
    pub fn from_chart(yaw: f32, pitch: f32, roll: f32, distance: f32, focus: Vec3) -> Self {
        Self {
            orientation: Orientation::from_yaw_pitch_roll(
                Angle::from_radians(yaw),
                Angle::from_radians(pitch),
                Angle::from_radians(roll),
            ),
            distance,
            focus,
        }
    }

    /// Read the framing back as yaw/pitch/roll.
    ///
    /// Ill-conditioned looking straight up or down, where yaw and roll become
    /// the same control; [`Orientation::chart_is_faithful`] says when. The
    /// orientation is always fine — it is only this naming of it that isn't.
    pub fn chart(&self) -> YawPitchRoll {
        self.orientation.yaw_pitch_roll()
    }

    /// Build from legacy parameters where `offset` displaced the eye off the
    /// orbit sphere: fold it into an equivalent on-sphere orientation and
    /// distance (the eye position is preserved exactly; orbiting then keeps
    /// the view distance constant instead of drifting with pitch)
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
        Self::from_chart(
            v.x.atan2(v.z),
            (v.y / d).clamp(-1.0, 1.0).asin(),
            roll,
            d,
            focus,
        )
    }

    /// Move the whole camera by a similarity about `center`: scale the eye and
    /// focus toward it by `scale`, and rotate the framing by `rot`.
    ///
    /// This is how [`crate::renorm::Renorm::wrap`] steps a zoom period. It
    /// transforms the camera exactly as the world would be transformed, so a
    /// set invariant under that similarity renders identically afterwards.
    ///
    /// Exact, and that matters: this used to rebuild the camera from a
    /// transformed eye/focus/up, which re-derived yaw through `atan2` and so
    /// wrapped it into (-π, π] on every wrap. A multi-turn path then had to
    /// put the lost turns back by hand, and getting that wrong was the whole
    /// wrong-way-round bug. There is nothing to put back now.
    pub fn apply_similarity(&mut self, center: Vec3, scale: f32, rot: Orientation) {
        self.focus = center + rot.rotate((self.focus - center) * scale);
        self.distance *= scale;
        self.orientation = self.orientation.then(rot);
    }

    pub fn eye(&self) -> Vec3 {
        self.focus + self.distance * self.orientation.rotate(Vec3::Z)
    }

    pub fn view_matrix(&self) -> Mat4 {
        // World → camera: undo the framing, then bring the eye to the origin.
        Mat4::from_quat(self.orientation.as_quat().conjugate())
            * Mat4::from_translation(-self.eye())
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(FOV_Y_RADIANS, aspect, Z_NEAR, Z_FAR) * self.view_matrix()
    }

    pub fn forward(&self) -> Vec3 {
        self.orientation.rotate(Vec3::NEG_Z)
    }

    /// Camera-space right in world coordinates
    pub fn right(&self) -> Vec3 {
        self.orientation.rotate(Vec3::X)
    }

    /// Camera-space up in world coordinates
    pub fn up(&self) -> Vec3 {
        self.orientation.rotate(Vec3::Y)
    }

    /// Mouse drag orbit: dx/dy in pixels. Grab-the-scene convention: drag
    /// right spins the scene rightward, drag up tilts its top toward you.
    ///
    /// Pitch turns about the camera's **own** right whatever the style, so it
    /// keeps meaning "tilt what I'm looking at" at every framing — including
    /// straight up, where a world-space pitch axis doesn't exist. Yaw turns
    /// about the axis [`OrbitStyle`] names, and that is the only difference
    /// between the two.
    ///
    /// Under [`OrbitStyle::Trackball`] both turns are body-frame, so they
    /// compose on the right and the drag applies *the same rotation* to every
    /// starting framing. History-independence isn't approximated here, it's
    /// structural — there is no state for the feel to be a function of.
    ///
    /// Nothing is clamped. Drag far enough and you go over the pole and come
    /// out inverted, which is the point: those framings were unreachable
    /// before, and some shots need them.
    pub fn orbit(&mut self, dx: f32, dy: f32, style: OrbitStyle) {
        let yaw = Turn::about(Vec3::Y, -dx * DRAG_RATE);
        let turned = match style {
            OrbitStyle::Trackball => self.orientation.then_body(yaw),
            OrbitStyle::Turntable => self.orientation.then_world(yaw),
        };
        self.orientation = turned.then_body(Turn::about(Vec3::X, dy * DRAG_RATE));
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

    /// Right-drag roll: horizontal drag spins the horizon.
    ///
    /// About the camera's own view axis, so it works at every framing. The old
    /// camera rolled by rotating world Y about the view axis, which meant that
    /// looking straight down — where those are the same axis — roll silently
    /// did nothing.
    pub fn roll_by(&mut self, dx: f32) {
        self.orientation = self.orientation.then_body(Turn::about(Vec3::Z, -dx * DRAG_RATE));
    }

    /// Put the horizon back level, without moving the eye.
    ///
    /// A no-op looking straight up or down, where every roll is level and the
    /// request has no answer.
    pub fn level(&mut self) {
        if !self.orientation.chart_is_faithful() {
            return;
        }
        let c = self.chart();
        self.orientation = Orientation::from_yaw_pitch_roll(c.yaw, c.pitch, Angle::ZERO);
    }

    /// Whether the horizon is level — nothing to do for [`Self::level`].
    pub fn is_level(&self) -> bool {
        !self.orientation.chart_is_faithful() || self.chart().roll.radians().abs() < 1e-6
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
    pub roll: Option<f32>,
    /// The exact form: a rotation vector in radians. Wins over the chart
    /// fields, and is the only way to name a framing at the poles — which is
    /// now reachable by dragging, so it needs to be nameable.
    pub rotvec: Option<Vec3>,
    pub distance: Option<f32>,
    pub focus: Option<Vec3>,
    /// Fly the scene's camera path to `t` in 0..1 and render *that* framing.
    ///
    /// Replaces the whole camera rather than one field of it, so it is applied
    /// in [`effective_camera_folded`](crate::offline::effective_camera_folded),
    /// the only place that has the path to sample; the fields above then adjust
    /// what it lands on. Exists because a camera on a zoom loop is not
    /// reachable by naming a distance: the descent twists as it scales, so
    /// stepping the distance alone walks off the path a little more every
    /// step, and the frames either side of a seam are exactly what anything
    /// measuring the seam needs to compare.
    pub path_t: Option<f32>,
}

impl CameraOverride {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Apply the overrides, reporting anything it could only do badly.
    ///
    /// The chart fields are resolved in **one** round trip, not three: near the
    /// poles yaw and roll are the same control, and replacing them one at a
    /// time would mean the answer depended on the order. Where the chart is
    /// that ill-conditioned there is no good answer at all, so say so rather
    /// than quietly producing a framing nobody asked for.
    pub fn apply(&self, cam: &mut OrbitCamera) -> Option<String> {
        let mut warning = None;

        if let Some(v) = self.rotvec {
            cam.orientation = Turn::from_rotation_vector(v).exp();
        } else if self.yaw.is_some() || self.pitch.is_some() || self.roll.is_some() {
            if !cam.orientation.chart_is_faithful() {
                warning = Some(
                    "--yaw/--pitch/--roll are ill-conditioned at this framing (looking very \
                     nearly straight up or down, where yaw and roll are the same control); \
                     use --rotvec x,y,z for an exact one"
                        .to_string(),
                );
            }
            let c = cam.chart();
            cam.orientation = Orientation::from_yaw_pitch_roll(
                self.yaw.map_or(c.yaw, Angle::from_radians),
                self.pitch.map_or(c.pitch, Angle::from_radians),
                self.roll.map_or(c.roll, Angle::from_radians),
            );
        }

        if let Some(v) = self.distance {
            cam.distance = v.max(1e-4);
        }
        if let Some(v) = self.focus {
            cam.focus = v;
        }
        warning
    }

    /// How this framing would be written in a scene's `[camera]` block, so a
    /// framing found by flags can be kept without transcribing it by hand.
    ///
    /// Falls back to `rotvec` where the chart can't say it — which is exactly
    /// what the save path does, for the same reason.
    pub fn describe(cam: &OrbitCamera) -> String {
        let head = format!(
            "[camera]\ndistance = {:.4}\nfocus = [{:.4}, {:.4}, {:.4}]",
            cam.distance, cam.focus.x, cam.focus.y, cam.focus.z
        );
        if cam.orientation.chart_is_faithful() {
            let c = cam.chart();
            format!(
                "{}\nyaw = {:.4}\npitch = {:.4}{}",
                head,
                c.yaw.radians(),
                c.pitch.radians(),
                if c.roll.radians() == 0.0 {
                    String::new()
                } else {
                    format!("\nroll = {:.4}", c.roll.radians())
                },
            )
        } else {
            let v = Orientation::IDENTITY
                .shortest_turn_to(cam.orientation)
                .as_rotation_vector();
            format!("{}\nrotvec = [{:.6}, {:.6}, {:.6}]", head, v.x, v.y, v.z)
        }
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
    use glam::Quat;

    fn cam(yaw: f32, pitch: f32, roll: f32, distance: f32, focus: Vec3) -> OrbitCamera {
        OrbitCamera::from_chart(yaw, pitch, roll, distance, focus)
    }

    /// The old camera's eye/up/view, written out longhand. Every "is this
    /// still what it was" test compares against this rather than against the
    /// implementation, so it keeps meaning something now the original is gone.
    fn legacy_view(yaw: f32, pitch: f32, roll: f32, distance: f32, focus: Vec3) -> Mat4 {
        let (sp, cp) = pitch.sin_cos();
        let (sy, cy) = yaw.sin_cos();
        let eye = focus + distance * Vec3::new(cp * sy, sp, cp * cy);
        let forward = (focus - eye).normalize();
        let up_reference = if roll == 0.0 {
            Vec3::Y
        } else {
            Quat::from_axis_angle(forward, roll) * Vec3::Y
        };
        Mat4::look_at_rh(eye, focus, up_reference)
    }

    #[test]
    fn the_framing_is_exactly_what_look_at_used_to_build() {
        // THE compatibility pin. Away from the poles — where the old camera
        // could not go anyway — the new view matrix must be the old one.
        for &yaw in &[-2.9f32, -0.7, 0.0, 1.3, 3.05] {
            for &pitch in &[-1.5f32, -0.4, 0.0, 0.9, 1.5] {
                for &roll in &[-2.2f32, 0.0, 1.1] {
                    let focus = Vec3::new(0.2, -0.1, 0.4);
                    let c = cam(yaw, pitch, roll, 2.5, focus);
                    let want = legacy_view(yaw, pitch, roll, 2.5, focus);
                    assert!(
                        c.view_matrix().abs_diff_eq(want, 1e-4),
                        "y{} p{} r{}:\n{:?}\nvs\n{:?}",
                        yaw,
                        pitch,
                        roll,
                        c.view_matrix(),
                        want
                    );
                }
            }
        }
    }

    #[test]
    fn orbiting_at_zero_roll_is_the_old_angle_arithmetic() {
        // The old orbit() was `yaw -= dx*rate; pitch -= dy*rate`. At roll 0
        // that has to survive exactly, or every existing scene's turntable
        // moves. It is the turntable that promises this, and now says so.
        let mut c = cam(0.4, 0.2, 0.0, 3.0, Vec3::ZERO);
        c.orbit(30.0, -20.0, OrbitStyle::Turntable);
        let got = c.chart();
        assert!((got.yaw.radians() - (0.4 - 30.0 * DRAG_RATE)).abs() < 1e-5, "{:?}", got);
        assert!((got.pitch.radians() - (0.2 + 20.0 * DRAG_RATE)).abs() < 1e-5, "{:?}", got);
        assert!(got.roll.radians().abs() < 1e-5, "orbiting must not introduce roll: {:?}", got);
    }

    #[test]
    fn a_trackball_drag_feels_the_same_from_every_framing() {
        // The design goal, written as an assertion: "looking around should
        // feel the same no matter what's been going on" is exactly the claim
        // that the turn a drag applies, *measured in the camera's own frame*,
        // doesn't depend on the framing it started from.
        let (dx, dy) = (23.0, -11.0);
        let reference = {
            let mut c = cam(0.0, 0.0, 0.0, 3.0, Vec3::ZERO);
            let before = c.orientation;
            c.orbit(dx, dy, OrbitStyle::Trackball);
            before.shortest_turn_to(c.orientation).as_rotation_vector()
        };

        let (mut worst_trackball, mut worst_turntable) = (0.0f32, 0.0f32);
        for i in 0..12 {
            let f = i as f32;
            // A dozen framings scattered over SO(3), several of them past the
            // pole and upside down — the states the old controls behaved
            // differently in.
            let start = Orientation::from_axis_angle(
                Vec3::new(f.sin(), (2.0 * f).cos(), (0.7 * f + 1.0).sin()).normalize(),
                0.3 + 0.5 * f,
            );
            for (style, worst) in [
                (OrbitStyle::Trackball, &mut worst_trackball),
                (OrbitStyle::Turntable, &mut worst_turntable),
            ] {
                let mut c = OrbitCamera { orientation: start, distance: 3.0, focus: Vec3::ZERO };
                c.orbit(dx, dy, style);
                let got = start.shortest_turn_to(c.orientation).as_rotation_vector();
                *worst = worst.max((got - reference).length());
            }
        }

        assert!(
            worst_trackball < 1e-6,
            "a trackball drag must be the same turn everywhere; worst framing was off by {} rad",
            worst_trackball
        );
        // And the assertion above has teeth: this is the property the
        // turntable cannot have, and the reason the default changed.
        assert!(
            worst_turntable > 0.05,
            "the turntable is supposed to be framing-dependent — if it isn't, \
             the test above is passing vacuously"
        );
    }

    #[test]
    fn a_trackball_drag_is_undone_by_dragging_back() {
        // Out and back along one axis returns you exactly where you were, from
        // any framing at all. Only along one axis at a time: turns about
        // different axes don't commute, and the roll a round trip leaves
        // behind is the acknowledged price of screen-relative controls, not a
        // bug to be tuned away.
        use std::f32::consts::FRAC_PI_2;
        for &(yaw, pitch, roll) in &[
            (0.0, 0.0, 0.0),
            (0.9, FRAC_PI_2, 0.0),   // straight up
            (-1.2, -FRAC_PI_2, 0.8), // straight down, and rolled
            (2.0, 3.0, -1.5),        // over the pole, inverted
        ] {
            let start = cam(yaw, pitch, roll, 3.0, Vec3::ZERO);
            for &(dx, dy) in &[(40.0, 0.0), (0.0, 40.0), (-140.0, 0.0)] {
                let mut c = start;
                c.orbit(dx, dy, OrbitStyle::Trackball);
                c.orbit(-dx, -dy, OrbitStyle::Trackball);
                assert!(
                    c.orientation.angle_to(start.orientation) < 1e-5,
                    "y{} p{} r{} drag ({}, {}) did not undo: off by {} rad",
                    yaw,
                    pitch,
                    roll,
                    dx,
                    dy,
                    c.orientation.angle_to(start.orientation)
                );
            }
        }
    }

    #[test]
    fn the_two_styles_are_the_same_drag_when_level() {
        // They differ only in the yaw axis, and looking level with no roll the
        // camera's own up *is* world up. So changing the preference moves
        // nothing and costs nothing for anyone flying level — the difference
        // only appears once you leave the horizontal, which is where the
        // turntable's feel starts depending on history.
        let mut a = cam(0.4, 0.0, 0.0, 3.0, Vec3::ZERO);
        let mut b = a;
        a.orbit(30.0, -20.0, OrbitStyle::Trackball);
        b.orbit(30.0, -20.0, OrbitStyle::Turntable);
        assert!(
            a.orientation.angle_to(b.orientation) < 1e-6,
            "level, the two styles must agree exactly: {:?} vs {:?}",
            a.chart(),
            b.chart()
        );

        // Off the horizontal they must not agree, or the preference is a lie.
        let mut a = cam(0.4, 0.9, 0.0, 3.0, Vec3::ZERO);
        let mut b = a;
        a.orbit(30.0, 0.0, OrbitStyle::Trackball);
        b.orbit(30.0, 0.0, OrbitStyle::Turntable);
        assert!(a.orientation.angle_to(b.orientation) > 0.01);
    }

    #[test]
    fn you_can_orbit_over_the_pole() {
        // Gimbal lock #1: pitch used to clamp at ±87.7°, so this framing was
        // unreachable and so was any path through it. Drag a long way up and
        // the camera must keep going, smoothly, and come out the other side.
        // Pitch is body-frame under either style, so both must manage it.
        for style in [OrbitStyle::Trackball, OrbitStyle::Turntable] {
            let mut c = cam(0.0, 0.0, 0.0, 3.0, Vec3::ZERO);
            let step = -20.0; // drag up
            let mut prev = c.forward();
            let mut crossed = false;
            for _ in 0..60 {
                c.orbit(0.0, step, style);
                let f = c.forward();
                // No snap, ever: 20px of drag is a small, bounded rotation.
                assert!(
                    f.dot(prev) > 0.95,
                    "{:?}: the view jumped: dot {} — that is a gimbal snap",
                    style,
                    f.dot(prev)
                );
                assert!(f.is_finite() && c.up().is_finite(), "basis went non-finite");
                assert!((c.up().length() - 1.0).abs() < 1e-4, "basis stopped being unit");
                if c.eye().y < -1.0 {
                    crossed = true;
                }
                prev = f;
            }
            assert!(
                crossed,
                "{:?}: 60 steps of 20px should have gone over the top and down the far side",
                style
            );
        }
    }

    #[test]
    fn roll_works_looking_straight_down() {
        // Gimbal lock #2, at the camera level. Roll used to be applied by
        // rotating world Y about the view axis; looking down, those coincide,
        // so it did nothing whatsoever.
        let mut c = cam(0.0, std::f32::consts::FRAC_PI_2, 0.0, 3.0, Vec3::ZERO);
        let before = c.up();
        let eye_before = c.eye();
        c.roll_by(100.0);

        assert!(
            c.up().dot(before) < 0.99,
            "roll at the pole did nothing: up {:?} -> {:?}",
            before,
            c.up()
        );
        assert!((c.eye() - eye_before).length() < 1e-5, "roll must not move the eye");
        assert!(c.view_matrix().is_finite(), "view matrix went non-finite at the pole");
    }

    #[test]
    fn roll_spins_the_horizon_without_moving_the_eye() {
        let level = cam(0.4, 0.2, 0.0, 3.0, Vec3::ZERO);
        let rolled = cam(0.4, 0.2, std::f32::consts::FRAC_PI_2, 3.0, Vec3::ZERO);

        assert!((rolled.eye() - level.eye()).length() < 1e-5);
        assert!((rolled.forward() - level.forward()).length() < 1e-5);
        assert!(
            rolled.up().dot(level.up()).abs() < 1e-3,
            "90 degrees of roll should leave up perpendicular to where it was"
        );
        assert!(rolled.right().dot(rolled.up()).abs() < 1e-5);
        assert!((rolled.right().length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn levelling_removes_roll_and_leaves_the_eye_alone() {
        let mut c = cam(0.9, -0.4, 1.2, 2.0, Vec3::new(0.1, 0.2, 0.0));
        let eye = c.eye();
        assert!(!c.is_level());
        c.level();
        assert!(c.is_level());
        assert!((c.eye() - eye).length() < 1e-5, "levelling must not move the eye");
        assert!(c.up().dot(Vec3::Y) > 0.0, "levelling should put up back near world up");

        // At the pole there is no such thing as level, and asking must be a
        // no-op rather than a scramble.
        let mut pole = cam(0.3, std::f32::consts::FRAC_PI_2, 0.7, 2.0, Vec3::ZERO);
        let before = pole;
        pole.level();
        assert_eq!(pole, before, "levelling at the pole must do nothing");
    }

    #[test]
    fn similarity_is_exact_and_moves_the_camera_like_the_world() {
        let c = cam(0.5, 0.2, 0.4, 3.0, Vec3::new(0.1, 0.0, 0.0));
        let center = Vec3::new(0.2, -0.1, 0.3);
        let (scale, rot) = (0.5, Orientation::from_axis_angle(Vec3::Y, 0.6));
        let mut moved = c;
        moved.apply_similarity(center, scale, rot);

        // Exact, not merely close: the framing is composed, never re-derived.
        assert_eq!(moved.orientation, c.orientation.then(rot));

        // A point and its image under the same similarity land on the same
        // pixel, seen by the moved and original cameras respectively.
        let p = Vec3::new(0.7, -0.4, 0.25);
        let q = center + rot.rotate((p - center) * scale);
        let a = world_to_screen(q, moved.view_proj(1.6), 1280.0, 800.0).unwrap();
        let b = world_to_screen(p, c.view_proj(1.6), 1280.0, 800.0).unwrap();
        assert!((a.0 - b.0).abs() < 0.05 && (a.1 - b.1).abs() < 0.05, "{:?} vs {:?}", a, b);
    }

    #[test]
    fn a_similarity_no_longer_wraps_the_yaw_it_reports() {
        // The old apply_similarity rebuilt the camera through atan2, folding
        // yaw into (-π, π] every time. Anything tracking a multi-turn route
        // then had to add the lost turns back by hand — which is precisely the
        // bug class this rework exists to delete.
        let c = cam(0.2, 0.1, 0.0, 3.0, Vec3::ZERO);
        let spin = Orientation::from_axis_angle(Vec3::Y, 2.0);
        let mut walked = c;
        for _ in 0..4 {
            walked.apply_similarity(Vec3::ZERO, 1.0, spin);
        }
        // Four 2-radian turns is 8 radians: more than a full turn, and the
        // composed orientation is simply right, with nothing to put back.
        let mut expect = c.orientation;
        for _ in 0..4 {
            expect = expect.then(spin);
        }
        assert_eq!(walked.orientation, expect);
    }

    #[test]
    fn legacy_offset_folding_preserves_eye() {
        let focus = Vec3::new(0.1, 0.2, 0.3);
        let offset = Vec3::new(0.0, 1.5, -0.2);
        let (yaw, distance) = (1.25f32, 3.0f32);
        let legacy_eye =
            focus + offset + Vec3::new(yaw.sin() * distance, 0.0, yaw.cos() * distance);

        let c = OrbitCamera::from_legacy(focus, offset, distance, yaw, 0.0, 0.0);
        assert!((c.eye() - legacy_eye).length() < 1e-4);
        assert!((c.distance - (legacy_eye - focus).length()).abs() < 1e-4);

        let chart = c.chart();
        let c2 = OrbitCamera::from_legacy(
            c.focus,
            Vec3::ZERO,
            c.distance,
            chart.yaw.radians(),
            chart.pitch.radians(),
            0.0,
        );
        assert!((c2.eye() - c.eye()).length() < 1e-4);
    }

    #[test]
    fn overrides_resolve_the_chart_in_one_round_trip() {
        let mut c = cam(0.5, 0.3, 0.2, 3.0, Vec3::ZERO);
        let over = CameraOverride { yaw: Some(1.0), ..Default::default() };
        assert!(over.apply(&mut c).is_none(), "an ordinary framing should not warn");
        let got = c.chart();
        assert!((got.yaw.radians() - 1.0).abs() < 1e-5);
        assert!((got.pitch.radians() - 0.3).abs() < 1e-5, "pitch must survive");
        assert!((got.roll.radians() - 0.2).abs() < 1e-5, "roll must survive");
    }

    #[test]
    fn overrides_say_so_at_the_poles_and_offer_a_way_through() {
        let mut c = cam(0.0, std::f32::consts::FRAC_PI_2, 0.0, 3.0, Vec3::ZERO);
        let warned = CameraOverride { yaw: Some(1.0), ..Default::default() }.apply(&mut c);
        assert!(warned.is_some(), "--yaw at the pole must warn, not silently guess");

        // rotvec always works, and describe() offers it back.
        let mut d = cam(0.0, std::f32::consts::FRAC_PI_2, 0.0, 3.0, Vec3::ZERO);
        let target = Vec3::new(0.3, -1.2, 0.4);
        assert!(
            CameraOverride { rotvec: Some(target), ..Default::default() }.apply(&mut d).is_none()
        );
        assert!(d.orientation.angle_to(Turn::from_rotation_vector(target).exp()) < 1e-5);
        assert!(
            CameraOverride::describe(&c).contains("rotvec"),
            "a pole framing must be describable: {}",
            CameraOverride::describe(&c)
        );
    }

    #[test]
    fn cursor_ray_center_points_at_focus() {
        let c = cam(0.7, 0.3, 0.0, 3.0, Vec3::ZERO);
        let vp = c.view_proj(16.0 / 9.0);
        let (origin, dir) = cursor_ray(vp.inverse(), 640.0, 360.0, 1280.0, 720.0);
        let to_focus = (c.focus - origin).normalize();
        assert!(dir.dot(to_focus) > 0.999, "dir {:?} vs {:?}", dir, to_focus);
    }

    #[test]
    fn screen_roundtrip() {
        let c = cam(0.2, -0.4, 0.0, 4.0, Vec3::new(0.5, 0.0, -0.2));
        let vp = c.view_proj(16.0 / 9.0);
        let p = Vec3::new(0.3, 0.1, 0.2);
        let (sx, sy) = world_to_screen(p, vp, 1280.0, 720.0).unwrap();
        let (origin, dir) = cursor_ray(vp.inverse(), sx, sy, 1280.0, 720.0);
        let t = (p - origin).dot(dir);
        assert!(((origin + dir * t) - p).length() < 1e-3);
    }
}
