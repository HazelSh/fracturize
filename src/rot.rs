//! Rotations, as two kinds of thing that must never be confused
//!
//! Almost every rotation bug this engine has had came from one mistake:
//! treating an angle-valued *coordinate* as if it were an angle-valued
//! *displacement*. They are different objects.
//!
//! - A coordinate lives on a circle (or on SO(3)). It is 2π-periodic, it has
//!   no canonical representative, and **subtracting two of them requires
//!   choosing a branch**. `yaw = 0.01` and `yaw = 6.29` frame the identical
//!   picture.
//! - A displacement lives in a vector space. It is unbounded, and adding,
//!   scaling and interpolating it are unambiguous. Sweeping +0.01 and sweeping
//!   −6.27 are different journeys to the same place.
//!
//! When the two are both `f32`, the compiler cannot tell them apart, and
//! `lerp(a.yaw, b.yaw, t)` silently picks whichever branch the subtraction
//! happened to land on. That is how a 1° yaw change came to swing 359° the
//! wrong way round.
//!
//! So this module gives them different types:
//!
//! | | coordinate (a point) | displacement (a journey) |
//! |---|---|---|
//! | on SO(3) | [`Orientation`] | [`Turn`] |
//! | on a circle | [`Angle`] | [`Turn1D`] |
//!
//! Neither [`Orientation`] nor [`Angle`] implements `Sub`. The only ways to
//! get from two coordinates to a displacement are named, and each names its
//! branch: [`Orientation::shortest_turn_to`] and [`Orientation::turn_to`].
//! There is no way to write the bug without saying out loud which way round
//! you meant. On the circle only [`Angle::shortest_to`] exists — routes are a
//! 3-D idea and live on paths, so a bare angle cannot express a wrong one.
//!
//! # Branches
//!
//! If `exp(ω) = Δq` with principal axis/angle `(n̂, θ)`, the displacements
//! landing on `Δq` are exactly
//!
//! ```text
//! { n̂(θ + 2πk) }  ∪  { −n̂(2π − θ + 2πk) }     for k ∈ ℤ
//! ```
//!
//! — one ray in each direction, spaced 2π apart. **The axis is forced; only
//! the magnitude is free.** That is why a route only ever needs one integer to
//! pin it down, and why routes are stored as `i32` windings rather than as
//! `Turn`s: an integer cannot disagree with the endpoints it connects, and a
//! stored `Turn` could.
//!
//! # Charts
//!
//! A chart is a set of angle coordinates naming an orientation. Charts are
//! convenient for humans and degenerate at their poles, so they live only at
//! the edges — file formats, CLI flags, UI readouts — and every one of them is
//! named here and pinned by a test:
//!
//! - [`Orientation::from_yaw_pitch_roll`] — the camera chart, `Ry(yaw) ·
//!   Rx(−pitch) · Rz(−roll)`. Degenerate looking straight up or down.
//! - [`Orientation::from_xyz_degrees`] — the IFS transform chart, extrinsic
//!   XYZ in degrees, as `[[transform]] rotation` has always meant.
//!
//! Between them, [`Turn`] is the exact form: a rotation vector, no chart, no
//! degeneracy, and its magnitude carries the winding for free. That is what
//! `rotvec` is on disk.

use glam::{Mat3, Quat, Vec3};
use std::f32::consts::{PI, TAU};

/// Below this, a rotation is treated as the identity and its axis as unknown.
/// Chosen so that `sin(θ/2) ≈ θ/2` holds to well under f32 precision.
const TINY: f32 = 1e-8;

// ---------------------------------------------------------------------------
// SO(3)
// ---------------------------------------------------------------------------

/// A point in SO(3): an orientation, with no notion of how it got there.
///
/// Canonicalized to the `w ≥ 0` hemisphere on construction, so the two
/// quaternions naming one rotation always compare equal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orientation(Quat);

/// A displacement in SO(3): a rotation vector, axis · angle.
///
/// The magnitude is **unbounded**. `|ω| > π` is not an error to be normalized
/// away — it is the whole point, and it is how a multi-turn corkscrew is
/// represented.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turn(Vec3);

impl Orientation {
    pub const IDENTITY: Self = Self(Quat::IDENTITY);

    /// Pick one of the two quaternions naming this rotation, so equal
    /// rotations compare equal.
    ///
    /// `w ≥ 0`, but with a deadband: exactly at a half turn `w` is zero, both
    /// representatives are equally valid, and which one you get rides on
    /// floating-point noise. `Quat::from_rotation_y(f32::consts::PI)` lands at
    /// `w ≈ -4e-8` — a hair past π, because `f32`'s π is — and flipping there
    /// reverses the axis the shortest turn reports. Nothing about the
    /// *rotation* changes, but the winding describing it comes back as ∓1
    /// instead of 0, and an ordinary half-turn sweep starts writing
    /// `turns = -1` into scene files. Holding steady through the crossing
    /// costs a microradian of "at most half a turn" and buys a stable answer.
    fn canonical(q: Quat) -> Self {
        let q = q.normalize();
        Self(if q.w < -1e-6 { -q } else { q })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Self {
        Self::canonical(Quat::from_axis_angle(axis.normalize_or(Vec3::Y), angle))
    }

    /// Build from a rotation matrix. The caller is responsible for it being
    /// orthonormal with positive determinant; use [`orthonormalize`] first if
    /// that isn't already established.
    pub fn from_mat3(m: &Mat3) -> Self {
        Self::canonical(Quat::from_mat3(m))
    }

    /// Build from an orthonormal right-handed basis given as columns.
    pub fn from_basis(x: Vec3, y: Vec3, z: Vec3) -> Self {
        Self::from_mat3(&Mat3::from_cols(x, y, z))
    }

    /// The underlying quaternion, for building matrices and rotating vectors.
    ///
    /// **Rendering and composition only — never interpolate this.**
    /// `Quat::slerp` and `Quat::lerp` hemisphere-fix internally: they *are*
    /// the take-the-short-way operators, and reaching for one here puts the
    /// wrong-way-round bug straight back. Interpolate [`Turn`]s instead.
    pub fn as_quat(self) -> Quat {
        self.0
    }

    pub fn inverse(self) -> Self {
        Self(self.0.conjugate())
    }

    /// Compose: apply `self` first, then `other`, both read in world terms.
    pub fn then(self, other: Self) -> Self {
        Self::canonical(other.0 * self.0)
    }

    /// Turn by `t` measured in **this orientation's own frame** (post-multiply).
    ///
    /// This is the one to use for controls that should feel attached to the
    /// object or the screen: roll about the view axis, pitch about the
    /// camera's own right. It never degenerates, because a body axis is always
    /// well defined.
    pub fn then_body(self, t: Turn) -> Self {
        Self::canonical(self.0 * t.exp().0)
    }

    /// Turn by `t` measured in **world axes** (pre-multiply).
    ///
    /// For controls anchored to the world: orbiting about world up, so the
    /// horizon stays put no matter how the camera is rolled.
    pub fn then_world(self, t: Turn) -> Self {
        Self::canonical(t.exp().0 * self.0)
    }

    /// The unique [`Turn`], of magnitude at most π, taking `self` to `other`
    /// in `self`'s own frame.
    ///
    /// "Shortest" is a real choice, not a default: it is right for a control
    /// being dragged and wrong for a route an author has written down. Say
    /// which you meant.
    pub fn shortest_turn_to(self, other: Self) -> Turn {
        Turn(principal_log(self.0.conjugate() * other.0))
    }

    /// The [`Turn`] taking `self` to `other`, carried `winding` extra whole
    /// turns about its own axis. `winding == 0` is [`shortest_turn_to`].
    ///
    /// Negative windings run the other way round: `-1` takes the long way
    /// against the short route. If the two orientations are equal the axis is
    /// genuinely undecidable, and a nonzero winding is taken about world Y —
    /// the axis a camera path almost always means. Pin that with a test rather
    /// than relying on it.
    ///
    /// [`shortest_turn_to`]: Orientation::shortest_turn_to
    pub fn turn_to(self, other: Self, winding: i32) -> Turn {
        let short = self.shortest_turn_to(other);
        if winding == 0 {
            return short;
        }
        let axis = branch_axis(short);
        Turn(axis * (short.0.dot(axis) + TAU * winding as f32))
    }

    /// How many whole turns `route` takes beyond the short way, as the winding
    /// integer [`turn_to`] wants. The inverse of the branch choice.
    ///
    /// [`turn_to`]: Orientation::turn_to
    pub fn winding_of(self, other: Self, route: Turn) -> i32 {
        let short = self.shortest_turn_to(other);
        let axis = branch_axis(short);
        ((route.0.dot(axis) - short.0.dot(axis)) / TAU).round() as i32
    }

    pub fn rotate(self, v: Vec3) -> Vec3 {
        self.0 * v
    }

    /// Angle between two orientations, in radians, always in `[0, π]`.
    pub fn angle_to(self, other: Self) -> f32 {
        self.shortest_turn_to(other).magnitude()
    }

}

impl Turn {
    pub const ZERO: Self = Self(Vec3::ZERO);

    pub fn about(axis: Vec3, angle: f32) -> Self {
        Self(axis.normalize_or_zero() * angle)
    }

    /// From a rotation vector — the wire form. Unbounded by design.
    pub fn from_rotation_vector(v: Vec3) -> Self {
        Self(v)
    }

    pub fn as_rotation_vector(self) -> Vec3 {
        self.0
    }

    /// Total angle swept, counting whole turns. Never folded into `[0, π]`.
    pub fn magnitude(self) -> f32 {
        self.0.length()
    }

    /// The axis, or `None` for a turn too small to have one.
    pub fn axis(self) -> Option<Vec3> {
        (self.0.length() > TINY).then(|| self.0.normalize())
    }

    pub fn exp(self) -> Orientation {
        Orientation::canonical(exp_rotvec(self.0))
    }

    /// The principal representative: the shortest displacement landing on the
    /// same orientation — this turn with its whole extra turns stripped.
    ///
    /// `self - self.principal()` is therefore the whole-turn part, colinear
    /// with the axis and an exact multiple of 2π. The split exists for the
    /// spline: whole turns are invisible at a segment's endpoints, so a
    /// smoothing weight that overshoots 1 by a few percent turns them into
    /// enormous mid-segment wobble unless they are weighted separately.
    ///
    /// Returns `self` unchanged (bit-for-bit) when the magnitude is already
    /// at most π, so ordinary short segments cannot pick up round-trip noise.
    pub fn principal(self) -> Turn {
        if self.magnitude() <= PI {
            self
        } else {
            Turn(principal_log(exp_rotvec(self.0)))
        }
    }
}

impl std::ops::Add for Turn {
    type Output = Turn;
    fn add(self, o: Turn) -> Turn {
        Turn(self.0 + o.0)
    }
}

impl std::ops::Sub for Turn {
    type Output = Turn;
    fn sub(self, o: Turn) -> Turn {
        Turn(self.0 - o.0)
    }
}

impl std::ops::Mul<f32> for Turn {
    type Output = Turn;
    fn mul(self, s: f32) -> Turn {
        Turn(self.0 * s)
    }
}

impl std::ops::Neg for Turn {
    type Output = Turn;
    fn neg(self) -> Turn {
        Turn(-self.0)
    }
}

/// The axis a winding is counted about, for a segment whose short way is
/// `short`. Shared by [`Orientation::turn_to`] and [`Orientation::winding_of`]
/// so the two stay exact inverses.
///
/// The fallback matters more than it looks. When two framings are nearly
/// equal, the short way between them is a rounding-error nudge whose direction
/// is noise — and counting whole turns about a noisy axis flips their sign. A
/// full turn authored as `yaw = 0 → -2π` came back as `+1` instead of `-1`:
/// the same journey, labelled backwards, which then writes the wrong `turns`
/// into the file. Below a tenth of a milliradian there is no honest axis to
/// read, so world Y — what a camera path means by "all the way round" — is
/// used for both directions of the conversion.
fn branch_axis(short: Turn) -> Vec3 {
    const MEANINGFUL: f32 = 1e-4;
    if short.magnitude() > MEANINGFUL {
        short.0.normalize()
    } else {
        Vec3::Y
    }
}

/// The principal logarithm: the rotation vector of magnitude at most π.
///
/// `Quat::to_axis_angle` and `Quat::to_scaled_axis` report the angle over
/// `[0, 2π]` and carry the direction in the axis, so neither is the principal
/// log — a 359° rotation comes back as 359°, not as −1°. Every wrong-way-round
/// bug in this engine's history has been a caller trusting one of them. They
/// are not used outside this function.
fn principal_log(q: Quat) -> Vec3 {
    // Flip only when `w` is decisively negative. Exactly at half a turn the
    // two representatives are the same length and `w` is zero, so a rotation
    // built from `f32::consts::PI` — which is a hair *over* π — lands at
    // w ≈ -4e-8 and flips the axis. The turn stays correct either way, but the
    // winding that describes it comes out as ∓1 instead of 0, and an ordinary
    // half-turn sweep starts writing `turns = -1` into scene files. Holding
    // the axis steady through the crossing costs a microradian of "at most π".
    let q = if q.w < -1e-6 { -q } else { q };
    let v = Vec3::new(q.x, q.y, q.z);
    let s = v.length();
    if s < TINY {
        // sin(θ/2) → θ/2, so the vector part is already half the rotation
        // vector. Exact to f32 well past where atan2 would start losing the
        // direction.
        v * 2.0
    } else {
        // w ≥ 0 here, so atan2 lands in [0, π/2] and the angle in [0, π].
        v * (2.0 * s.atan2(q.w) / s)
    }
}

fn exp_rotvec(v: Vec3) -> Quat {
    let angle = v.length();
    if angle < TINY {
        Quat::from_xyzw(v.x * 0.5, v.y * 0.5, v.z * 0.5, 1.0).normalize()
    } else {
        Quat::from_axis_angle(v / angle, angle)
    }
}

/// Gram-Schmidt a matrix into an honest rotation, and score how far it had to
/// move: `0` for a matrix that was already a rotation (times a uniform scale).
///
/// The score is the entrywise deviation of `QᵀQ` from the identity, which
/// catches both non-uniform scale and shear — the two ways a "rotation" can
/// turn out not to be one.
pub fn orthonormalize(q: Mat3) -> (Orientation, f32) {
    let x = q.x_axis.normalize_or(Vec3::X);
    let y = (q.y_axis - x * x.dot(q.y_axis)).normalize_or(Vec3::Y);
    let z = x.cross(y);

    let m = q.transpose() * q - Mat3::IDENTITY;
    let defect = m.to_cols_array().iter().fold(0.0f32, |acc, v| acc.max(v.abs()));

    (Orientation::from_basis(x, y, z), defect)
}

// ---------------------------------------------------------------------------
// The circle
// ---------------------------------------------------------------------------

/// A point on the circle: an angle-valued coordinate, in radians.
///
/// Deliberately has no `Sub`. Two angles do not have a difference until you
/// say which way round — see [`Angle::shortest_to`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Angle(f32);

/// A displacement on the circle, in radians. Unbounded.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Turn1D(f32);

impl Angle {
    pub const ZERO: Self = Self(0.0);

    pub fn from_radians(r: f32) -> Self {
        Self(r)
    }


    /// The representative this angle was built from, unchanged.
    ///
    /// Two angles 2π apart are the same coordinate but return different
    /// numbers here. That is fine for display and for handing to `sin`/`cos`;
    /// it is not a difference, and it is not a route.
    pub fn radians(self) -> f32 {
        self.0
    }

    pub fn degrees(self) -> f32 {
        self.0.to_degrees()
    }


    /// The shorter of the two ways round from `self` to `other`, in `(-π, π]`.
    pub fn shortest_to(self, other: Self) -> Turn1D {
        Turn1D((other.0 - self.0).rem_euclid(TAU)).wrapped()
    }

}

impl Turn1D {
    pub fn radians(self) -> f32 {
        self.0
    }

    /// Magnitude, for "did the pointer move far?" checks.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn abs(self) -> f32 {
        self.0.abs()
    }

    fn wrapped(self) -> Self {
        let d = self.0.rem_euclid(TAU);
        Self(if d > PI { d - TAU } else { d })
    }

}

impl std::ops::Add<Turn1D> for Angle {
    type Output = Angle;
    fn add(self, t: Turn1D) -> Angle {
        Angle(self.0 + t.0)
    }
}

impl std::ops::Add for Turn1D {
    type Output = Turn1D;
    fn add(self, o: Turn1D) -> Turn1D {
        Turn1D(self.0 + o.0)
    }
}

impl std::ops::Sub for Turn1D {
    type Output = Turn1D;
    fn sub(self, o: Turn1D) -> Turn1D {
        Turn1D(self.0 - o.0)
    }
}

impl std::ops::Mul<f32> for Turn1D {
    type Output = Turn1D;
    fn mul(self, s: f32) -> Turn1D {
        Turn1D(self.0 * s)
    }
}

impl std::ops::Neg for Turn1D {
    type Output = Turn1D;
    fn neg(self) -> Turn1D {
        Turn1D(-self.0)
    }
}

// ---------------------------------------------------------------------------
// Charts
// ---------------------------------------------------------------------------

/// The camera chart: yaw about world up, pitch elevating, roll about the view
/// axis. Degenerate looking straight up or down, where yaw and roll become the
/// same control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YawPitchRoll {
    pub yaw: Angle,
    pub pitch: Angle,
    pub roll: Angle,
}

/// How near the pole the camera chart stops being trustworthy. At 89.4° the
/// yaw/roll split is already badly conditioned; past it, write `rotvec`.
pub const CHART_EPS: f32 = 0.01;

impl Orientation {
    /// The camera chart: `q = Ry(yaw) · Rx(−pitch) · Rz(−roll)`.
    ///
    /// The camera looks down its own −Z, so `q · Ẑ` is the direction from the
    /// focus out to the eye. The two negative signs are not a taste: they are
    /// what makes this agree with the `look_at_rh` basis this engine used
    /// before quaternions, and `chart_matches_look_at` pins it.
    pub fn from_yaw_pitch_roll(yaw: Angle, pitch: Angle, roll: Angle) -> Self {
        // Written as an explicit product rather than `Quat::from_euler`:
        // glam's intrinsic/extrinsic naming has moved between versions, and
        // this convention has to outlive that.
        Self::canonical(
            Quat::from_rotation_y(yaw.radians())
                * Quat::from_rotation_x(-pitch.radians())
                * Quat::from_rotation_z(-roll.radians()),
        )
    }

    /// Read the camera chart back. Ill-conditioned within [`CHART_EPS`] of the
    /// poles — see [`Orientation::chart_is_faithful`].
    pub fn yaw_pitch_roll(self) -> YawPitchRoll {
        let back = self.rotate(Vec3::Z); // focus → eye
        let pitch = back.y.clamp(-1.0, 1.0).asin();
        let yaw = back.x.atan2(back.z);

        // With yaw and pitch fixed, roll is whatever is left over.
        let level = Self::from_yaw_pitch_roll(
            Angle(yaw),
            Angle(pitch),
            Angle::ZERO,
        );
        let residual = level.shortest_turn_to(self);
        // The leftover is a rotation about the view axis, which is body −Z;
        // the chart's roll is its negation, by the sign convention above.
        let roll = -residual.as_rotation_vector().z;

        YawPitchRoll { yaw: Angle(yaw), pitch: Angle(pitch), roll: Angle(roll) }
    }

    /// Whether the camera chart round-trips this orientation faithfully. False
    /// near the poles, where yaw and roll collapse into one control and the
    /// numbers stop meaning anything individually.
    pub fn chart_is_faithful(self) -> bool {
        let c = self.yaw_pitch_roll();
        c.pitch.radians().cos().abs() > CHART_EPS
            && Self::from_yaw_pitch_roll(c.yaw, c.pitch, c.roll).angle_to(self) < 1e-4
    }

    /// The IFS transform chart: extrinsic XYZ in degrees, exactly what
    /// `[[transform]] rotation` has always meant. A different convention from
    /// the camera's, on a different object — deliberately not unified, because
    /// unifying the *meaning* would silently reinterpret every scene on disk.
    pub fn from_xyz_degrees(d: [f32; 3]) -> Self {
        Self::canonical(Quat::from_euler(
            glam::EulerRot::XYZ,
            d[0].to_radians(),
            d[1].to_radians(),
            d[2].to_radians(),
        ))
    }

    pub fn to_xyz_degrees(self) -> [f32; 3] {
        let (x, y, z) = self.0.to_euler(glam::EulerRot::XYZ);
        [x.to_degrees(), y.to_degrees(), z.to_degrees()]
    }
}

// ---------------------------------------------------------------------------
// Affine transforms
// ---------------------------------------------------------------------------

/// A matrix split into scale, rotation and translation.
///
/// Not every matrix splits: shear and mirroring are both legitimate here (an
/// IFS transform is free to be either) and neither survives the trip. Ask
/// [`Trs::is_faithful`] before showing a person these numbers or writing them
/// to a file; the matrix stays the source of truth either way.
#[derive(Clone, Copy, Debug)]
pub struct Trs {
    pub scale: Vec3,
    pub rotation: Orientation,
    pub translation: Vec3,
}

impl Trs {
    pub fn of(m: glam::Mat4) -> Self {
        let (scale, rotation, translation) = m.to_scale_rotation_translation();
        Self { scale, rotation: Orientation::canonical(rotation), translation }
    }

    pub fn matrix(&self) -> glam::Mat4 {
        glam::Mat4::from_scale_rotation_translation(
            self.scale,
            self.rotation.as_quat(),
            self.translation,
        )
    }

    /// Whether recomposing reproduces `m` — false for shear and for a
    /// negative determinant, where the split is a lie.
    pub fn is_faithful(&self, m: glam::Mat4) -> bool {
        let back = self.matrix();
        let norm = m.to_cols_array().iter().fold(1.0f32, |a, v| a.max(v.abs()));
        let diff = m
            .to_cols_array()
            .iter()
            .zip(back.to_cols_array().iter())
            .fold(0.0f32, |a, (x, y)| a.max((x - y).abs()));
        diff < 1e-4 * norm && m.determinant() >= 0.0
    }
}

/// Turn a matrix's linear part about its own origin, leaving the translation
/// where it is.
///
/// The one way to rotate a transform in place, shared by the gizmo drag and
/// the mutator so they cannot drift apart. Left-multiplies, so `t` is read in
/// world axes — which is what both callers mean by "spin it about this axis".
pub fn turn_linear_part(m: glam::Mat4, t: Turn) -> glam::Mat4 {
    let rot = glam::Mat4::from_quat(t.exp().as_quat());
    let mut out = rot * glam::Mat4::from_cols(m.x_axis, m.y_axis, m.z_axis, glam::Vec4::W);
    out.w_axis = m.w_axis;
    out
}

/// The route an author drew in the yaw/pitch/roll chart, as a winding.
///
/// The chart's angles are unbounded on disk — keys at `0`, `3.14`, `6.28`
/// author a full turn, and that is a *route*, not a coordinate. Orientations
/// alone cannot carry it (a full turn and no turn are the same orientation),
/// so it has to be read off the curve the author actually wrote before the
/// chart is thrown away.
///
/// Method: walk the chart path in small steps, sum the principal logs as a
/// plain vector, and pick the branch nearest that sum. The whole vector
/// matters, not just its length — a segment sweeping exactly half a turn has
/// two branches of equal length, and only the direction tells them apart.
///
/// Exact when the chart path is a one-parameter subgroup, which covers every
/// multi-turn path anyone actually writes (they sweep yaw). A contrived
/// segment that corkscrews through all three angles at once could be given a
/// route its author didn't intend — but never a wrong *framing*, since the
/// keys are hit whatever the winding says.
pub fn winding_from_chart(a: YawPitchRoll, b: YawPitchRoll) -> i32 {
    const STEPS: usize = 64;
    let at = |s: f32| {
        let lerp = |x: Angle, y: Angle| Angle(x.0 + (y.0 - x.0) * s);
        Orientation::from_yaw_pitch_roll(
            lerp(a.yaw, b.yaw),
            lerp(a.pitch, b.pitch),
            lerp(a.roll, b.roll),
        )
    };

    let mut route = Vec3::ZERO;
    let mut prev = at(0.0);
    for i in 1..=STEPS {
        let cur = at(i as f32 / STEPS as f32);
        route += prev.shortest_turn_to(cur).as_rotation_vector();
        prev = cur;
    }
    at(0.0).winding_of(at(1.0), Turn::from_rotation_vector(route))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32, what: &str) {
        assert!((a - b).abs() < eps, "{}: {} vs {}", what, a, b);
    }

    #[test]
    fn shortest_turn_is_never_more_than_half_a_turn() {
        // The bug, stated as a test: a rotation 0.5° short of a full turn is a
        // 0.5° journey backwards, not a 359.5° journey forwards.
        let a = Orientation::IDENTITY;
        let b = Orientation::from_axis_angle(Vec3::Y, 359.5f32.to_radians());
        let t = a.shortest_turn_to(b);
        approx(t.magnitude().to_degrees(), 0.5, 1e-3, "magnitude");
        assert!(t.as_rotation_vector().y < 0.0, "should go backwards, got {:?}", t);
        assert!(a.then_body(t).angle_to(b) < 1e-5, "must still land on b");
    }

    #[test]
    fn principal_log_survives_the_negative_hemisphere() {
        // to_axis_angle reports [0, 2π] and hands the direction to the axis, so
        // a quaternion that came back with w < 0 used to read as its own long
        // way round. Straddle w = 0 and check the log stays continuous.
        for &w in &[-1e-3f32, -1e-6, 0.0, 1e-6, 1e-3] {
            let s = (1.0 - w * w).sqrt();
            let q = Quat::from_xyzw(0.0, s, 0.0, w);
            let v = principal_log(q);
            assert!(v.length() <= PI + 1e-3, "w={}: |log| = {}", w, v.length());
            let back = exp_rotvec(v);
            let same = (back.dot(q).abs() - 1.0).abs();
            assert!(same < 1e-4, "w={}: exp(log(q)) != ±q ({})", w, same);
        }
    }

    #[test]
    fn windings_are_a_ray_spaced_two_pi_apart() {
        let a = Orientation::from_yaw_pitch_roll(
            Angle::from_radians(0.3),
            Angle::from_radians(-0.2),
            Angle::from_radians(0.1),
        );
        let b = Orientation::from_yaw_pitch_roll(
            Angle::from_radians(2.4),
            Angle::from_radians(0.5),
            Angle::from_radians(-0.4),
        );
        let short = a.shortest_turn_to(b).magnitude();

        for k in -3..=3 {
            let t = a.turn_to(b, k);
            // Every branch lands on b...
            assert!(a.then_body(t).angle_to(b) < 1e-4, "k={} missed b", k);
            // ...and they are spaced exactly a full turn apart along one ray.
            approx(t.magnitude(), (short + TAU * k as f32).abs(), 1e-3, &format!("k={}", k));
            // ...and the winding reads back.
            assert_eq!(a.winding_of(b, t), k, "winding_of round trip, k={}", k);
        }
    }

    #[test]
    fn a_full_turn_between_equal_orientations_goes_about_y() {
        // Genuinely undecidable — documented to fall back to world Y, because
        // that is what a camera path means by "spin all the way round".
        let a = Orientation::from_yaw_pitch_roll(
            Angle::from_radians(1.0),
            Angle::ZERO,
            Angle::ZERO,
        );
        let t = a.turn_to(a, 1);
        approx(t.magnitude(), TAU, 1e-4, "one whole turn");
        approx(t.as_rotation_vector().normalize().y, 1.0, 1e-4, "about Y");
    }

    #[test]
    fn turns_round_trip_at_multi_turn_magnitudes() {
        // winze's whole loop is ~10.4 radians. exp/log has to be exact there,
        // not just near the identity.
        for &mag in &[0.001f32, 1.0, PI - 1e-3, PI + 1e-3, 6.0, 10.43, 20.0] {
            let axis = Vec3::new(0.3, 0.9, -0.2).normalize();
            let t = Turn::about(axis, mag);
            let q = t.exp();
            let back = Orientation::IDENTITY.turn_to(q, Orientation::IDENTITY.winding_of(q, t));
            assert!(
                (back.as_rotation_vector() - t.as_rotation_vector()).length() < 1e-3,
                "mag {}: {:?} vs {:?}",
                mag,
                back,
                t
            );
        }
    }

    #[test]
    fn principal_strips_whole_turns_and_nothing_else() {
        // Short turns come back bit-identical — the spline relies on that to
        // leave ordinary segments untouched.
        let short = Turn::about(Vec3::new(0.3, 0.9, -0.2).normalize(), 2.5);
        assert_eq!(short.principal().as_rotation_vector(), short.as_rotation_vector());

        // A multi-turn corkscrew splits into a sub-π principal part and a
        // colinear whole-turn remainder.
        for &mag in &[4.0f32, 6.0, 10.43, 20.0] {
            let axis = Vec3::new(0.1, -0.7, 0.4).normalize();
            let t = Turn::about(axis, mag);
            let p = t.principal();
            assert!(p.magnitude() <= PI + 1e-4, "mag {}: |principal| = {}", mag, p.magnitude());
            assert!(
                p.exp().angle_to(t.exp()) < 1e-4,
                "mag {}: principal must land on the same orientation",
                mag
            );
            let whole = (t - p).as_rotation_vector();
            let k = whole.length() / TAU;
            assert!((k - k.round()).abs() < 1e-3, "mag {}: remainder is {} turns", mag, k);
            assert!(
                whole.normalize().dot(axis).abs() > 1.0 - 1e-4,
                "mag {}: remainder must stay on the axis",
                mag
            );
        }
    }

    #[test]
    fn chart_matches_look_at() {
        // THE pin. `Ry(yaw)·Rx(-pitch)·Rz(-roll)` must reproduce, exactly, the
        // basis the engine built with look_at_rh before quaternions:
        //   eye  = focus + distance * (cos p sin y, sin p, cos p cos y)
        //   up   = Quat::from_axis_angle(forward, roll) * Vec3::Y
        // Written out longhand here so this test still means something after
        // the old code is gone.
        for &yaw in &[-2.7f32, -0.4, 0.0, 1.1, 3.0] {
            for &pitch in &[-1.4f32, -0.3, 0.0, 0.7, 1.4] {
                for &roll in &[-2.0f32, 0.0, 0.9] {
                    let (sp, cp) = pitch.sin_cos();
                    let (sy, cy) = yaw.sin_cos();
                    let old_back = Vec3::new(cp * sy, sp, cp * cy);
                    let old_forward = -old_back;
                    let old_up_ref = Quat::from_axis_angle(old_forward, roll) * Vec3::Y;
                    let old_right = old_forward.cross(old_up_ref).normalize();
                    let old_up = old_right.cross(old_forward);

                    let q = Orientation::from_yaw_pitch_roll(
                        Angle::from_radians(yaw),
                        Angle::from_radians(pitch),
                        Angle::from_radians(roll),
                    );

                    let tag = format!("y{} p{} r{}", yaw, pitch, roll);
                    assert!(
                        (q.rotate(Vec3::Z) - old_back).length() < 1e-5,
                        "{}: back {:?} vs {:?}",
                        tag,
                        q.rotate(Vec3::Z),
                        old_back
                    );
                    assert!(
                        (q.rotate(Vec3::X) - old_right).length() < 1e-5,
                        "{}: right {:?} vs {:?}",
                        tag,
                        q.rotate(Vec3::X),
                        old_right
                    );
                    assert!(
                        (q.rotate(Vec3::Y) - old_up).length() < 1e-5,
                        "{}: up {:?} vs {:?}",
                        tag,
                        q.rotate(Vec3::Y),
                        old_up
                    );
                }
            }
        }
    }

    #[test]
    fn chart_round_trips_away_from_the_poles() {
        for &yaw in &[-3.0f32, -1.0, 0.0, 2.2] {
            for &pitch in &[-1.2f32, 0.0, 0.5, 1.5] {
                for &roll in &[-2.5f32, 0.0, 1.3] {
                    let q = Orientation::from_yaw_pitch_roll(
                        Angle::from_radians(yaw),
                        Angle::from_radians(pitch),
                        Angle::from_radians(roll),
                    );
                    let c = q.yaw_pitch_roll();
                    let back = Orientation::from_yaw_pitch_roll(c.yaw, c.pitch, c.roll);
                    assert!(
                        back.angle_to(q) < 1e-4,
                        "y{} p{} r{} -> {:?}",
                        yaw,
                        pitch,
                        roll,
                        c
                    );
                    assert!(q.chart_is_faithful(), "y{} p{} r{} should be faithful", yaw, pitch, roll);
                }
            }
        }
    }

    #[test]
    fn the_chart_gives_up_at_the_poles() {
        // Looking straight down, yaw and roll are the same control. The chart
        // can still name the orientation, but the split is meaningless, and
        // `chart_is_faithful` has to say so or the save path will write
        // nonsense instead of a rotvec.
        let pole = Orientation::from_yaw_pitch_roll(
            Angle::from_radians(0.7),
            Angle::from_radians(PI / 2.0),
            Angle::from_radians(0.2),
        );
        assert!(!pole.chart_is_faithful(), "straight up must not be chart-faithful");

        // ...but the orientation itself is perfectly ordinary, and a rotvec
        // round-trips it exactly.
        let t = Orientation::IDENTITY.shortest_turn_to(pole);
        assert!(t.exp().angle_to(pole) < 1e-5, "rotvec must round-trip at the pole");
    }

    #[test]
    fn body_and_world_turns_differ_and_both_land_somewhere_real() {
        let q = Orientation::from_yaw_pitch_roll(
            Angle::from_radians(0.8),
            Angle::from_radians(0.6),
            Angle::from_radians(0.4),
        );
        let t = Turn::about(Vec3::Y, 0.3);
        assert!(q.then_body(t) != q.then_world(t), "frames must not be interchangeable");
        assert!(q.then_body(t).as_quat().is_finite() && q.then_world(t).as_quat().is_finite());

        // World-frame yaw is exactly a chart yaw bump, at any roll — this is
        // what keeps orbiting horizon-preserving.
        let bumped = q.then_world(t).yaw_pitch_roll();
        let c = q.yaw_pitch_roll();
        approx(bumped.yaw.radians(), c.yaw.radians() + 0.3, 1e-4, "world yaw");
        approx(bumped.pitch.radians(), c.pitch.radians(), 1e-4, "pitch unchanged");
        approx(bumped.roll.radians(), c.roll.radians(), 1e-4, "roll unchanged");
    }

    #[test]
    fn roll_works_looking_straight_down() {
        // Gimbal lock, as a test. The old camera rolled by rotating world Y
        // about the view axis — which at the pole *is* world Y, so roll did
        // nothing at all. A body-frame turn cannot have that problem.
        let down = Orientation::from_yaw_pitch_roll(
            Angle::ZERO,
            Angle::from_radians(PI / 2.0),
            Angle::ZERO,
        );
        let rolled = down.then_body(Turn::about(Vec3::Z, 0.5));
        approx(down.angle_to(rolled), 0.5, 1e-4, "roll must actually rotate the view");
        // ...and it must not have moved where the camera is looking.
        assert!(
            (down.rotate(Vec3::Z) - rolled.rotate(Vec3::Z)).length() < 1e-5,
            "roll must not move the eye"
        );
    }

    #[test]
    fn xyz_degrees_chart_is_unchanged_from_the_scene_loader() {
        // Pins that unifying the *types* did not reinterpret the transform
        // chart: it must still be Rx·Ry·Rz on column vectors, as
        // scene::tests::euler_xyz_is_rx_ry_rz has always required.
        let d = [10.0f32, 30.0, -45.0];
        let q = Orientation::from_xyz_degrees(d);
        let expect = Quat::from_rotation_x(d[0].to_radians())
            * Quat::from_rotation_y(d[1].to_radians())
            * Quat::from_rotation_z(d[2].to_radians());
        assert!(q.as_quat().dot(expect).abs() > 1.0 - 1e-5, "{:?} vs {:?}", q, expect);

        let back = q.to_xyz_degrees();
        for i in 0..3 {
            approx(back[i], d[i], 1e-3, "xyz degrees round trip");
        }
    }

    #[test]
    fn angles_have_no_difference_until_you_pick_one() {
        let a = Angle::from_radians(0.05);
        let b = Angle::from_radians(TAU - 0.05);

        // 0.05 and 6.23 are a tenth of a radian apart, not 6.18. There is no
        // `a - b` to get that wrong with: `shortest_to` is the only way over,
        // and it is named for the choice it makes.
        approx(a.shortest_to(b).radians(), -0.1, 1e-5, "short way");
        approx(b.shortest_to(a).radians(), 0.1, 1e-5, "and back");

        // Representatives a whole turn apart are the same coordinate.
        approx(
            Angle::from_radians(0.3).shortest_to(Angle::from_radians(0.3 + TAU)).radians(),
            0.0,
            1e-4,
            "a full turn apart is no distance at all",
        );
    }

    #[test]
    fn shortest_angle_agrees_with_the_two_helpers_it_replaces() {
        // path::shortest_angle and pick::wrap_angle were separate
        // implementations of the same contract. Both are now this; check the
        // boundary they disagreed about most plausibly.
        let old_path = |d: f32| {
            let d = d.rem_euclid(TAU);
            if d > PI { d - TAU } else { d }
        };
        let old_pick = |delta: f32| {
            let mut d = delta % TAU;
            if d > PI {
                d -= TAU;
            } else if d <= -PI {
                d += TAU;
            }
            d
        };
        for i in -400..400 {
            let d = i as f32 * 0.05;
            let mine = Angle::ZERO.shortest_to(Angle::from_radians(d)).radians();
            approx(mine, old_path(d), 1e-4, &format!("path helper at {}", d));
            approx(mine, old_pick(d), 1e-4, &format!("pick helper at {}", d));
        }
    }

    #[test]
    fn an_authored_chart_sweep_reads_back_as_its_route() {
        let ypr = |y: f32, p: f32, r: f32| YawPitchRoll {
            yaw: Angle::from_radians(y),
            pitch: Angle::from_radians(p),
            roll: Angle::from_radians(r),
        };

        // The documented convention: keys at 0, π, 2π author a full turn.
        assert_eq!(winding_from_chart(ypr(0.0, 0.0, 0.0), ypr(PI, 0.0, 0.0)), 0);
        assert_eq!(winding_from_chart(ypr(0.0, 0.0, 0.0), ypr(TAU, 0.0, 0.0)), 1);
        assert_eq!(winding_from_chart(ypr(0.0, 0.0, 0.0), ypr(2.0 * TAU, 0.0, 0.0)), 2);
        assert_eq!(winding_from_chart(ypr(0.0, 0.0, 0.0), ypr(-TAU, 0.0, 0.0)), -1);

        // An ordinary short segment is winding 0, at any pitch and roll.
        for &p in &[-1.0f32, 0.0, 0.9] {
            for &r in &[-1.5f32, 0.0, 0.8] {
                assert_eq!(
                    winding_from_chart(ypr(0.2, p, r), ypr(1.4, p + 0.2, r - 0.1)),
                    0,
                    "pitch {} roll {}",
                    p,
                    r
                );
            }
        }

        // winze's real segments: ~120° of yaw with pitch moving too. All short.
        let winze = [(2.17f32, 0.45f32), (4.264, 0.18), (6.358, 0.30), (8.452, 0.42), (10.55, 0.25)];
        for w in winze.windows(2) {
            assert_eq!(
                winding_from_chart(ypr(w[0].0, w[0].1, 0.0), ypr(w[1].0, w[1].1, 0.0)),
                0,
                "winze segment {:?} -> {:?}",
                w[0],
                w[1]
            );
        }
        // ...and its whole authored span is 1.66 turns. Asserted as the route
        // it reconstructs, not as the integer: the winding counts turns about
        // the *short way's* axis, and this segment's short way runs backwards,
        // so the label is negative for a forward corkscrew. The integer is an
        // opaque branch selector; what has to be right is where it goes.
        let (a, b) = (ypr(2.17, 0.45, 0.0), ypr(12.64, 0.30, 0.0));
        let (qa, qb) = (
            Orientation::from_yaw_pitch_roll(a.yaw, a.pitch, a.roll),
            Orientation::from_yaw_pitch_roll(b.yaw, b.pitch, b.roll),
        );
        let route = qa.turn_to(qb, winding_from_chart(a, b));
        approx(route.magnitude(), 10.47, 0.05, "winze sweeps 1.66 turns");
        assert!(route.as_rotation_vector().y > 0.0, "and it sweeps forwards: {:?}", route);
        assert!(qa.then_body(route).angle_to(qb) < 1e-3, "and it still lands on the last key");
    }

    #[test]
    fn a_half_turn_sweep_keeps_its_direction() {
        // Both branches are the same length at exactly π, so length alone
        // cannot choose. The direction of travel has to.
        let ypr = |y: f32| YawPitchRoll {
            yaw: Angle::from_radians(y),
            pitch: Angle::ZERO,
            roll: Angle::ZERO,
        };
        let forward = ypr(0.0);
        let back = ypr(PI);

        let k = winding_from_chart(forward, back);
        let route = Orientation::from_yaw_pitch_roll(forward.yaw, forward.pitch, forward.roll)
            .turn_to(
                Orientation::from_yaw_pitch_roll(back.yaw, back.pitch, back.roll),
                k,
            );
        approx(route.magnitude(), PI, 1e-3, "a half turn stays a half turn");
        assert!(route.as_rotation_vector().y > 0.0, "swept the wrong way: {:?}", route);
    }

    #[test]
    fn orthonormalize_scores_shear_and_recovers_rotations() {
        let q = Orientation::from_axis_angle(Vec3::new(0.2, 1.0, -0.3), 0.9);
        let m = Mat3::from_quat(q.as_quat()) * 2.5;
        let (rot, defect) = orthonormalize(m);
        assert!(rot.angle_to(q) < 1e-4, "a scaled rotation must come back exactly");
        assert!(defect > 1.0, "a 2.5x scale is a big defect, got {}", defect);

        let (_, clean) = orthonormalize(Mat3::from_quat(q.as_quat()));
        assert!(clean < 1e-5, "a pure rotation has no defect, got {}", clean);
    }
}
