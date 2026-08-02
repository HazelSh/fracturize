//! Infinite zoom: renormalizing an IFS attractor into a scale-invariant one
//!
//! # The problem
//!
//! An IFS attractor is bounded. Every map contracts, so the whole thing lives
//! inside a box, and zooming *out* runs out of fractal in a second or two.
//! Zooming *in* runs out too, for a different reason: the chaos game spends its
//! points in proportion to the attractor's natural measure, so a window a
//! thousand times smaller than the body gets roughly a thousandth of them (a
//! millionth, in 2D projection) and the picture thins to nothing long before
//! the geometry does. Neither direction is infinite, and the interesting thing
//! about a fractal is supposed to be that both are.
//!
//! # The construction
//!
//! Pick one of the scene's own transforms, `f`, and require it to be affine and
//! contracting. It has a unique fixed point `p`, and about that point it acts as
//! a linear map `A` — a scale `s < 1` and (usually) a rotation.
//!
//! The attractor satisfies `S ⊇ f(S)`, so applying `f⁻¹` grows it:
//!
//! ```text
//!     S ⊆ f⁻¹(S) ⊆ f⁻²(S) ⊆ …          S_m := f⁻ᵐ(S)
//! ```
//!
//! an increasing chain. Its union
//!
//! ```text
//!     S∞ := ⋃_{m ≥ 0} f⁻ᵐ(S)
//! ```
//!
//! is unbounded, and — this is the whole trick — it is *exactly* invariant
//! under `f`:
//!
//! ```text
//!     f⁻¹(S∞) = ⋃_{m ≥ 1} f⁻ᵐ(S) = S∞
//! ```
//!
//! (the `m = 0` term is contained in the `m = 1` term, so dropping it changes
//! nothing). `S∞` is the same set at every scale: scale it about `p` by `s`,
//! rotate it by `A`'s rotation, and you have not changed it at all. It has no
//! privileged size. It is the thing todo.txt asks for — "fixed-point structure
//! with no privileged scale" — and it exists for any IFS with one invertible
//! contracting affine map in it, which is nearly all of them.
//!
//! # Sampling it
//!
//! `S∞` is unbounded, so it cannot be sampled uniformly; but it doesn't need to
//! be. The canonical measure on it is
//!
//! ```text
//!     ν = Σ_{m ∈ ℤ} (f⁻ᵐ)_* μ
//! ```
//!
//! where `μ` is the ordinary chaos-game measure on `S`. `ν` is `f`-invariant
//! (shifting `m` by one is a bijection of the sum), and it is exactly what a
//! self-similar object "weighted equally at every scale" means.
//!
//! Drawing from `ν` is startlingly cheap. Run the chaos game as usual to get a
//! point `x ∈ S`. Let `r = |x − p|`. Choose the integer
//!
//! ```text
//!     m = round( log(R / r) / log(1/s) )
//! ```
//!
//! and emit `f⁻ᵐ(x)`, which lands at radius ≈ `R` from `p`. One `round` and a
//! few matrix multiplies per point, no rejection, nothing wasted: *every* chaos
//! point, however deep in the attractor it fell, is recycled to the scale being
//! looked at. That is [`Renorm::level_for_radius`] and the `renormalize()`
//! function in `chaos.wgsl`.
//!
//! Landing every point on one shell would leave the inside hollow, so `levels`
//! octaves of extra contraction are dealt out uniformly at random (`m` minus a
//! random integer in `0..levels`), filling the range `[R·s^levels, R]`.
//!
//! **Equally per octave**, which is the one part of this that is forced rather
//! than chosen. A wrap moves the octave filling the screen along by one, so an
//! octave holding fewer points than its neighbour makes the picture change
//! density every period. See `DEFAULT_OCTAVE_FALLOFF`. And the band has to
//! reach far enough *out* that its edge never enters the frustum, which is a
//! sharper condition than it looks — see `MIN_RADIUS`.
//!
//! # Zooming forever without leaving f32
//!
//! Because `S∞` is exactly `A`-invariant, zoom is *periodic*: zooming in by `s`
//! and rotating by `A`'s rotation returns the identical image. So the camera
//! never has to leave one period. When the eye crosses the inner edge of the
//! band, [`Renorm::wrap`] applies `A⁻¹` to the whole camera — eye, focus and
//! up together — which puts it back at the outer edge looking at a picture
//! pixel-for-pixel identical to the one it left. The zoom counter goes up by
//! one; nothing else moves.
//!
//! Nothing ever gets small, so nothing loses precision, and the point buffer
//! never has to be regenerated: the wrap *is* the level-of-detail system. Zoom
//! in for an hour if you like.
//!
//! The catch, stated honestly: the zoom is infinite *toward `p`*. Fly off
//! sideways and you leave the band, and what you get is the ordinary finite
//! attractor again. Every self-similar zoom has a center; this one's is the
//! fixed point of the map you chose.

use glam::{Mat3, Mat4, Quat, Vec3};

use crate::camera::OrbitCamera;

/// Octaves of scale rendered below the target radius, if a scene doesn't say.
/// Roughly the dynamic range of a 1080p frame plus the outward margin
/// [`MIN_RADIUS`] adds, so the *visible* depth is unchanged by that margin.
const DEFAULT_LEVELS: f32 = 14.0;

/// Smallest outer radius, as a multiple of the reference eye distance, that
/// doesn't put the band's edge inside the picture. **This is not a matter of
/// taste; getting it wrong is the bug that made whole regions of a zoom
/// animation blink out.**
///
/// A wrap multiplies the eye's distance from the fixed point by `1/s`, so the
/// distance at which the frustum needs material multiplies by `1/s` too — but
/// the band's outer edge is fixed in world space. Anything the old eye could
/// see and the new one can't simply isn't there any more, and it goes at once,
/// mid-flight, in the middle of the frame.
///
/// The bound: the eye sits at most `band` from the fixed point, and haze has
/// taken material to nothing by an eye-distance of `haze::FAR_FRAC · band`
/// (that is what auto-ranging the haze band off the camera distance means).
/// So material is wanted out to `band + FAR_FRAC · band`, giving
///
/// ```text
///     radius ≥ (1 + FAR_FRAC) · band  =  2.42 · band
/// ```
///
/// A scene with little or no haze has nothing hiding the edge and wants more;
/// `Renorm::summary` and `--info` say so when a scene asks for less than this.
pub const MIN_RADIUS: f32 = 1.0 + crate::haze::FAR_FRAC;

/// Outer radius of the band, as a multiple of the reference eye distance:
/// [`MIN_RADIUS`] with a little margin.
const DEFAULT_RADIUS: f32 = 3.0;

/// How steeply the point budget falls off toward the fixed point, as a power
/// of the contraction ratio.
///
/// **Zero, and that is a correctness requirement rather than a preference.** A
/// wrap moves the octave that fills the screen along by one, so if octave `k`
/// and octave `k-1` hold different numbers of points, the density on screen
/// jumps by exactly that ratio every period. Measured on `wellspiral`: the
/// discontinuity across a wrap runs 1.9x an equal-sized camera move at falloff
/// 0, and 3.2x at falloff 2.
///
/// It survives as a knob because it is genuinely useful for a *still* — it
/// evens out on-screen density, which is what an octave falloff is for — and
/// because a scene that is never going to be flown doesn't care.
const DEFAULT_OCTAVE_FALLOFF: f32 = 0.0;

/// How far a map may stray from being a similarity before the camera wrap
/// stops being seamless and we say so. A pure scale+rotation scores 0.
const SIMILARITY_TOLERANCE: f32 = 0.02;

/// A scene's infinite-zoom settings, as authored. Held by [`crate::scene::Scene`]
/// so it survives edits and saves; [`Renorm::build`] resolves it against the
/// live transform list, which may have been dragged around since.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoomSpec {
    /// Which transform renormalizes. Resolved from a name or an index at load.
    pub map: usize,
    /// Radius from the fixed point that points are renormalized onto, as a
    /// multiple of the reference eye distance. The outermost octave.
    pub radius: f32,
    /// Octaves (factors of two) of scale rendered below `radius`.
    ///
    /// Octaves rather than zoom periods on purpose: a period is however big
    /// the chosen map's contraction happens to be, which runs from 0.07
    /// octaves (a 0.95 spiral) to 3.3 (a 0.1 collapse), and a `levels` that
    /// meant periods would cover a hundredth of the frame's dynamic range in
    /// one scene and forty times it in the next. This means the same thing
    /// everywhere. [`Renorm::periods`] converts.
    pub levels: f32,
    /// Point-budget falloff toward the fixed point, as a power of the
    /// contraction ratio (see [`DEFAULT_OCTAVE_FALLOFF`])
    pub octave_falloff: f32,
}

impl Default for ZoomSpec {
    fn default() -> Self {
        Self {
            map: 0,
            radius: DEFAULT_RADIUS,
            levels: DEFAULT_LEVELS,
            octave_falloff: DEFAULT_OCTAVE_FALLOFF,
        }
    }
}

/// The resolved renormalization: a similarity about a fixed point, plus the
/// band the camera is kept inside.
#[derive(Clone, Copy, Debug)]
pub struct Renorm {
    /// Index of the transform this was built from
    pub map: usize,
    /// The map's fixed point — the center of the zoom
    pub fixed_point: Vec3,
    /// Linear part about the fixed point (contracting)
    pub a: Mat3,
    pub a_inv: Mat3,
    /// Contraction factor, `|det A|^(1/3)`
    pub scale: f32,
    /// `ln(1/scale)` — one zoom period in log-radius. Positive.
    pub log_scale: f32,
    /// Rotation part of `A` (`A ≈ scale · rot`)
    pub rot: Quat,
    /// How far `A` is from being a pure similarity; 0 is exact. Above
    /// [`SIMILARITY_TOLERANCE`] the camera wrap leaves a visible seam.
    pub defect: f32,
    /// Outer radius of the renormalized band, in world units
    pub radius: f32,
    /// Zoom periods of spread below `radius` — the authored octave count
    /// converted to this map's own periods, and clamped to something the
    /// renormalization loop can actually walk
    pub periods: f32,
    /// Ratio of one period's point share to the next one in, `scale^falloff`
    pub octave_q: f32,
    /// Rotation axis and angle of `rot`, for the closed-form power of a
    /// similarity (see `renormalize()` in chaos.wgsl)
    pub axis: Vec3,
    pub angle: f32,
    /// Whether `A` is a similarity to within [`SIMILARITY_TOLERANCE`], and so
    /// whether `Aᵏ` can be taken in closed form instead of by iteration
    pub similar: bool,
    /// Eye distance from the fixed point at the top of a zoom period. The
    /// camera is wrapped to keep `|eye − p|` inside `[band·scale, band)`.
    pub band: f32,
}

impl Renorm {
    /// Resolve a [`ZoomSpec`] against the live transforms.
    ///
    /// `reference_distance` is the eye distance one zoom period is measured
    /// from — the scene's authored camera distance. Fails, with something a
    /// human can act on, when the chosen map can't renormalize anything.
    pub fn build(
        spec: &ZoomSpec,
        transforms: &[crate::scene::TransformSpec],
        reference_distance: f32,
    ) -> Result<Self, String> {
        let t = transforms
            .get(spec.map)
            .ok_or_else(|| format!("zoom map {} is out of range ({} transforms)", spec.map, transforms.len()))?;

        // The renormalization has to be invertible in closed form, and a
        // variation blend isn't. Pure-linear transforms only.
        let nonlinear: f32 = t.variations.iter().skip(1).map(|w| w.abs()).sum();
        if nonlinear > 1e-4 {
            return Err(format!(
                "zoom map {} uses variations ({}); the renormalizing map must be pure affine",
                spec.map,
                t.variation_summary()
            ));
        }
        // `linear` may be weighted; fold that into the matrix rather than
        // rejecting a transform that is still affine, just scaled.
        let linear_weight = t.variations[0];
        if linear_weight.abs() < 1e-4 {
            return Err(format!("zoom map {} has no linear component", spec.map));
        }

        Self::from_affine(t.matrix, linear_weight, spec, reference_distance).map(|mut r| {
            r.map = spec.map;
            r
        })
    }

    /// The geometry, separated from the scene plumbing so it can be tested on
    /// a bare matrix. `linear_weight` scales the affine result, as the
    /// variation blend would.
    pub fn from_affine(
        matrix: Mat4,
        linear_weight: f32,
        spec: &ZoomSpec,
        reference_distance: f32,
    ) -> Result<Self, String> {
        let a = Mat3::from_mat4(matrix) * linear_weight;
        let b = matrix.w_axis.truncate() * linear_weight;

        let det = a.determinant().abs();
        if det < 1e-12 {
            return Err("zoom map is singular (zero scale on some axis) and can't be inverted".into());
        }
        // Every direction must contract, or f⁻ᵐ doesn't converge to a shell and
        // the renormalization loop never terminates.
        let sigma_max = largest_singular_value(a);
        if sigma_max >= 0.999 {
            return Err(format!(
                "zoom map does not contract in every direction (largest scale {:.3}); \
                 pick a map with scale < 1 on all three axes",
                sigma_max
            ));
        }

        // Fixed point: p = A p + b  ⇒  (I − A) p = b
        let fixed_point = (Mat3::IDENTITY - a)
            .inverse()
            .mul_vec3(b);
        if !fixed_point.is_finite() {
            return Err("zoom map has no finite fixed point".into());
        }

        let scale = det.powf(1.0 / 3.0);
        let log_scale = (1.0 / scale).ln();
        let q = a * (1.0 / scale);
        // Gram-Schmidt the scaled matrix into an honest rotation, and score how
        // much that had to change it: for a true similarity, QᵀQ is already I.
        let (rot, defect) = orthonormalize(q);
        let (axis, angle) = rot.to_axis_angle();

        Ok(Self {
            map: spec.map,
            fixed_point,
            a,
            a_inv: a.inverse(),
            scale,
            log_scale,
            rot,
            defect,
            radius: (spec.radius * reference_distance).max(1e-6),
            // Authored in octaves; the shader deals in this map's own periods.
            // Capped at 64 because a barely-contracting map would otherwise
            // ask for thousands, and floored at 1 so the band is never empty.
            periods: (spec.levels.max(0.0) * std::f32::consts::LN_2 / log_scale).clamp(1.0, 64.0),
            octave_q: scale.powf(spec.octave_falloff.max(0.0)),
            axis,
            angle,
            similar: defect <= SIMILARITY_TOLERANCE,
            band: reference_distance.max(1e-6),
        })
    }

    /// The integer `m` in `f⁻ᵐ(x)` that lands a point at radius `r` from the
    /// fixed point on the target shell. Negative means contract instead.
    /// This is the whole sampler, and the shader does exactly this.
    pub fn level_for_radius(&self, r: f32) -> f32 {
        if !(r > 1e-20) {
            return 0.0;
        }
        (self.radius / r).ln() / self.log_scale
    }

    /// Apply `f⁻ᵏ` to one point about the fixed point
    pub fn apply_level(&self, pos: Vec3, k: i32) -> Vec3 {
        let mut u = pos - self.fixed_point;
        let k = k.clamp(-48, 48);
        for _ in 0..k {
            u = self.a_inv * u;
        }
        for _ in k..0 {
            u = self.a * u;
        }
        self.fixed_point + u
    }

    /// Renormalize one point, the way the shader does. The reference
    /// implementation of `renormalize()` in `chaos.wgsl` — nothing in the app
    /// calls it (points are renormalized on the GPU, a whole buffer at a
    /// time), but the tests below are the only statement anywhere that the
    /// construction does what the module docs claim, and they need it here in
    /// a language that can assert. `spread` in `[0,1)` deals the octave.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn apply(&self, pos: Vec3, spread: f32) -> Vec3 {
        let r = (pos - self.fixed_point).length();
        if !(r > 1e-20) {
            return pos;
        }
        // CPU-side spread stays flat; the shader's geometric deal is a
        // rendering-budget concern and this is only used for reasoning about
        // one point at a time.
        let m = self.level_for_radius(r).round() - (spread * self.periods).floor();
        self.apply_level(pos, m.clamp(-48.0, 48.0) as i32)
    }

    /// Renormalize a whole chaos-game trace **as a unit**.
    ///
    /// Point-by-point renormalization would send consecutive steps of the walk
    /// to different octaves and turn a path into a scatter of jumps. One level
    /// for the whole trace, taken from where it starts, keeps it the connected
    /// walk it is — and since the invariant set contains every level's copy of
    /// the attractor, a trace drawn at any single level is a real path through
    /// what's on screen.
    pub fn renormalize_trace(&self, positions: &mut [Vec3]) {
        let Some(first) = positions.first() else { return };
        let r = (*first - self.fixed_point).length();
        if !(r > 1e-20) {
            return;
        }
        let k = self.level_for_radius(r).round().clamp(-48.0, 48.0) as i32;
        for p in positions {
            *p = self.apply_level(*p, k);
        }
    }

    /// Keep the eye inside one zoom period, and report how many periods that
    /// took (positive = zoomed in). The camera moves; the picture doesn't.
    pub fn wrap(&self, cam: &mut OrbitCamera) -> i32 {
        let mut levels = 0;
        // A camera dropped in from far outside can need many steps; the cap is
        // only there so a degenerate scene can't hang the frame.
        for _ in 0..256 {
            let d = (cam.eye() - self.fixed_point).length();
            if !(d > 1e-9) {
                break; // sitting on the fixed point; there is no "out" from here
            }
            if d < self.band * self.scale {
                cam.apply_similarity(self.fixed_point, 1.0 / self.scale, self.rot.inverse());
                levels += 1;
            } else if d >= self.band {
                cam.apply_similarity(self.fixed_point, self.scale, self.rot);
                levels -= 1;
            } else {
                break;
            }
        }
        levels
    }

    /// Whether the band reaches far enough out that its edge stays outside the
    /// picture across a wrap. See [`MIN_RADIUS`].
    pub fn band_covers_the_view(&self) -> bool {
        self.radius >= MIN_RADIUS * self.band
    }

    /// A one-line report for the CLI and the status bar
    pub fn summary(&self, name: Option<&str>) -> String {
        let short = if self.band_covers_the_view() {
            String::new()
        } else {
            format!(
                ", BAND TOO SHORT (radius {:.2}x the eye distance, needs {:.2}x) —                  material will blink out at each wrap",
                self.radius / self.band,
                MIN_RADIUS
            )
        };
        let seam = if self.defect > SIMILARITY_TOLERANCE {
            format!(", NOT a similarity (defect {:.2}) — the zoom wrap will show a seam", self.defect)
        } else {
            String::new()
        };
        format!(
            "infinite zoom on transform {}{}: scale {:.3} ({:.2} octaves/period), \
             {:.0}° twist, fixed point ({:.3}, {:.3}, {:.3}), {:.0} periods \
             ({:.1} octaves) rendered{}",
            self.map,
            name.map(|n| format!(" \"{}\"", n)).unwrap_or_default(),
            self.scale,
            self.log_scale / std::f32::consts::LN_2,
            self.rot.to_axis_angle().1.to_degrees(),
            self.fixed_point.x,
            self.fixed_point.y,
            self.fixed_point.z,
            self.periods,
            self.periods * self.log_scale / std::f32::consts::LN_2,
            format!("{}{}", short, seam)
        )
    }
}

/// Largest singular value of a 3x3 matrix, by power iteration on `AᵀA`.
/// Only used at load time, so the loop count is generous rather than clever.
fn largest_singular_value(a: Mat3) -> f32 {
    let ata = a.transpose() * a;
    let mut v = Vec3::new(0.577, 0.577, 0.577);
    for _ in 0..64 {
        let next = ata * v;
        let len = next.length();
        if len < 1e-20 {
            return 0.0;
        }
        v = next / len;
    }
    (ata * v).dot(v).max(0.0).sqrt()
}

/// Gram-Schmidt a near-orthogonal matrix into a rotation, returning it and how
/// far it had to move (the "similarity defect": 0 for a true scale+rotation).
fn orthonormalize(q: Mat3) -> (Quat, f32) {
    let x = q.x_axis.normalize_or(Vec3::X);
    let y = (q.y_axis - x * x.dot(q.y_axis)).normalize_or(Vec3::Y);
    let z = x.cross(y);
    let ortho = Mat3::from_cols(x, y, z);

    // Deviation of QᵀQ from the identity, entrywise: catches both non-uniform
    // scale and shear, which are exactly the two ways the wrap can break.
    let m = q.transpose() * q - Mat3::IDENTITY;
    let defect = m
        .to_cols_array()
        .iter()
        .fold(0.0f32, |acc, v| acc.max(v.abs()));

    (Quat::from_mat3(&ortho).normalize(), defect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::world_to_screen;

    fn spiral_map(scale: f32, twist_deg: f32) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(scale),
            Quat::from_rotation_y(twist_deg.to_radians()),
            Vec3::new(0.3, 0.1, -0.2),
        )
    }

    fn renorm(scale: f32, twist: f32) -> Renorm {
        Renorm::from_affine(
            spiral_map(scale, twist),
            1.0,
            &ZoomSpec { map: 0, radius: 1.0, levels: 1.0, ..ZoomSpec::default() },
            1.0,
        )
        .unwrap()
    }

    #[test]
    fn fixed_point_is_fixed() {
        let m = spiral_map(0.55, 31.0);
        let r = renorm(0.55, 31.0);
        let mapped = m.transform_point3(r.fixed_point);
        assert!(
            (mapped - r.fixed_point).length() < 1e-5,
            "f(p) = {:?} should equal p = {:?}",
            mapped,
            r.fixed_point
        );
    }

    #[test]
    fn scale_and_rotation_are_recovered() {
        let r = renorm(0.55, 31.0);
        assert!((r.scale - 0.55).abs() < 1e-4, "scale {}", r.scale);
        assert!(r.defect < 1e-4, "a pure similarity should have no defect: {}", r.defect);
        let (_, angle) = r.rot.to_axis_angle();
        assert!((angle.to_degrees() - 31.0).abs() < 0.1, "twist {}", angle.to_degrees());
    }

    #[test]
    fn every_point_lands_in_the_band() {
        // Fixed point at the origin, so the test radii survive being written
        // down in f32: `p + 1e-8·dir` for a p of order 1 is just p.
        let r = Renorm::from_affine(
            Mat4::from_scale(Vec3::splat(0.5)),
            1.0,
            &ZoomSpec { map: 0, radius: 1.0, levels: 1.0, ..ZoomSpec::default() },
            1.0,
        )
        .unwrap();
        // Radii spanning sixteen orders of magnitude, in and out
        for e in -8..8 {
            let pos = r.fixed_point + Vec3::new(1.0, 0.4, -0.7).normalize() * 10f32.powi(e);
            let out = r.apply(pos, 0.0);
            let radius = (out - r.fixed_point).length();
            // round() puts it within half a period of the target
            assert!(
                radius > r.radius * r.scale.sqrt() * 0.999
                    && radius < r.radius / r.scale.sqrt() * 1.001,
                "10^{} renormalized to radius {} (target {})",
                e,
                radius,
                r.radius
            );
        }
    }

    #[test]
    fn renormalization_commutes_with_the_map() {
        // The point of the construction: the emitted set is A-invariant, so
        // renormalizing f(x) gives the same answer as renormalizing x.
        let r = renorm(0.62, 25.0);
        let m = spiral_map(0.62, 25.0);
        for i in 0..20 {
            let x = Vec3::new(0.7 * i as f32, 1.3 - 0.1 * i as f32, 0.2 * i as f32 - 1.0);
            let a = r.apply(x, 0.0);
            let b = r.apply(m.transform_point3(x), 0.0);
            assert!(
                (a - b).length() < 1e-3 * a.length().max(1.0),
                "x = {:?}: renorm(x) = {:?} but renorm(f(x)) = {:?}",
                x,
                a,
                b
            );
        }
    }

    #[test]
    fn wrapping_the_camera_does_not_move_the_picture() {
        // The seamlessness claim, as an assertion: after a wrap, every point of
        // the invariant set projects to the same pixel it did before. We test
        // it by projecting x with the wrapped camera and A(x) — its partner in
        // the invariant set — with the original.
        let r = renorm(0.6, 40.0);
        let cam = OrbitCamera {
            yaw: 0.7,
            pitch: 0.35,
            distance: 1.0,
            focus: r.fixed_point + Vec3::new(0.05, -0.02, 0.01),
            roll: 0.3,
        };
        // Sit inside the inner edge so exactly one wrap fires. The margin is
        // for the off-center focus: the wrap measures eye-to-fixed-point, not
        // the orbit radius.
        let mut zoomed = cam;
        zoomed.distance = r.band * r.scale * 0.8;
        let before = zoomed;
        let levels = r.wrap(&mut zoomed);
        assert_eq!(levels, 1, "one period of zoom should wrap exactly once");

        let vp_before = before.view_proj(16.0 / 9.0);
        let vp_after = zoomed.view_proj(16.0 / 9.0);
        for i in 0..25 {
            let x = r.fixed_point
                + Vec3::new(
                    0.4 * (i as f32 * 1.3).sin(),
                    0.4 * (i as f32 * 0.7).cos(),
                    0.4 * (i as f32 * 2.1).sin(),
                );
            // The wrap moved the camera by A⁻¹, so a point seen by the
            // wrapped camera lands where its image under A sat for the
            // original one — and A maps the invariant set onto itself, so the
            // frame is filled with exactly the same material as before.
            let partner = r.fixed_point + r.a * (x - r.fixed_point);
            let a = world_to_screen(x, vp_after, 1280.0, 720.0);
            let b = world_to_screen(partner, vp_before, 1280.0, 720.0);
            match (a, b) {
                (Some(a), Some(b)) => assert!(
                    (a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5,
                    "point {} projects to {:?} after the wrap but {:?} before",
                    i,
                    a,
                    b
                ),
                (None, None) => {}
                _ => panic!("point {} changed visibility across the wrap", i),
            }
        }
    }

    #[test]
    fn wrapping_is_idempotent_inside_the_band() {
        let r = renorm(0.5, 15.0);
        let mut cam = OrbitCamera {
            yaw: 0.2,
            pitch: 0.1,
            distance: r.band * 0.75,
            focus: r.fixed_point,
            roll: 0.0,
        };
        let before = cam;
        assert_eq!(r.wrap(&mut cam), 0);
        assert_eq!(cam.distance, before.distance);
        assert_eq!(cam.yaw, before.yaw);
    }

    #[test]
    fn wrapping_recovers_from_far_outside() {
        let r = renorm(0.5, 15.0);
        let mut cam = OrbitCamera {
            yaw: 0.2,
            pitch: 0.1,
            distance: r.band * 5000.0,
            focus: r.fixed_point,
            roll: 0.0,
        };
        let levels = r.wrap(&mut cam);
        let d = (cam.eye() - r.fixed_point).length();
        assert!(d >= r.band * r.scale && d < r.band, "eye at {} outside band", d);
        assert!(levels < 0, "zooming out should count down, got {}", levels);
    }

    #[test]
    fn the_default_band_reaches_past_the_haze() {
        // The bug this pins: a band whose outer edge sits inside the frustum
        // loses material at every wrap, because a wrap multiplies the distance
        // at which the frustum wants material by 1/s while the edge stays put.
        // Whole regions of a zoom animation blinked out. Do not lower
        // DEFAULT_RADIUS below MIN_RADIUS to make a still look denser.
        assert!(
            DEFAULT_RADIUS >= MIN_RADIUS,
            "default band radius {} is below the {} needed to keep its edge out of view",
            DEFAULT_RADIUS,
            MIN_RADIUS
        );
        let r = Renorm::from_affine(spiral_map(0.6, 34.0), 1.0, &ZoomSpec::default(), 3.6).unwrap();
        assert!(r.band_covers_the_view());
        assert!(!r.summary(None).contains("BAND TOO SHORT"));
    }

    #[test]
    fn a_short_band_says_so() {
        let spec = ZoomSpec { radius: 1.2, ..ZoomSpec::default() };
        let r = Renorm::from_affine(spiral_map(0.6, 34.0), 1.0, &spec, 3.6).unwrap();
        assert!(!r.band_covers_the_view());
        assert!(r.summary(None).contains("BAND TOO SHORT"), "{}", r.summary(None));
    }

    #[test]
    fn the_octave_deal_is_flat_by_default() {
        // Not a preference: an octave holding fewer points than its neighbour
        // makes the density on screen jump every time the camera wraps.
        let r = Renorm::from_affine(spiral_map(0.6, 34.0), 1.0, &ZoomSpec::default(), 3.6).unwrap();
        assert_eq!(r.octave_q, 1.0, "octave weighting must be flat for a seamless wrap");
    }

    #[test]
    fn expanding_maps_are_refused() {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(0.4, 1.2, 0.4),
            Quat::IDENTITY,
            Vec3::ZERO,
        );
        let err = Renorm::from_affine(m, 1.0, &ZoomSpec::default(), 1.0).unwrap_err();
        assert!(err.contains("contract"), "{}", err);
    }

    #[test]
    fn anisotropic_maps_are_allowed_but_flagged() {
        // Self-affine rather than self-similar: still an exact invariant set,
        // but the camera wrap can't reproduce a non-uniform scale, so it must
        // report a defect rather than pretend.
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(0.7, 0.3, 0.5),
            Quat::IDENTITY,
            Vec3::ZERO,
        );
        let r = Renorm::from_affine(m, 1.0, &ZoomSpec::default(), 1.0).unwrap();
        assert!(r.defect > SIMILARITY_TOLERANCE, "defect {}", r.defect);
        assert!(r.summary(None).contains("seam"));
    }
}
