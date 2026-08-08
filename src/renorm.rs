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

use glam::{Mat3, Mat4, Vec3};

use crate::camera::OrbitCamera;
use crate::rot::{Orientation, Turn};

/// Octaves of scale rendered below the target radius, if a scene doesn't say.
/// Roughly the dynamic range of a 1080p frame plus the outward margin
/// [`MIN_RADIUS`] adds, so the *visible* depth is unchanged by that margin.
///
/// One more than it was, to pay for [`DEFAULT_RADIUS`] going up: the band is
/// `[R·2⁻ˡᵉᵛᵉˡˢ, R]`, so every doubling of `R` costs an octave of depth at the
/// inner end. 3.0 → 4.8 is 0.68 octaves; rounding up to a whole one lands the
/// inner edge slightly deeper than before rather than slightly shallower.
const DEFAULT_LEVELS: f32 = 15.0;

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
///
/// Twice [`MIN_RADIUS`] rather than the 1.24× it used to be. The bound is
/// derived assuming haze takes distant material all the way to nothing, which
/// is only true at `haze = 1.0`; at the strengths scenes actually use (0.3–0.5,
/// and 0.0 in a couple of drafts) something survives at the far plane and the
/// edge is not hidden at all. Headroom is the only defence against that, and
/// against a scene being framed differently from how it was authored.
const DEFAULT_RADIUS: f32 = 4.8;

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
///
/// Note which end of the band it acts on, because it is the opposite end from
/// [`DEFAULT_EDGE_GUARD`] and the two are easy to reach for interchangeably.
/// Octave 0 is the outermost shell and the share of octave `k` is `qᵏ`, so the
/// falloff thins the *innermost* octaves — the small ones clustered around the
/// fixed point, which is the middle of the picture. Measured on
/// `octave-edge-test`, a falloff of 2 changes 40% of the material within 12% of
/// the frame's radius of centre and 25% out at the rim. The guard does the
/// reverse, and only at the rim. Neither is backwards; they are two knobs for
/// two ends.
const DEFAULT_OCTAVE_FALLOFF: f32 = 0.0;

/// Width of the edge guard, in octaves: the ratio band over which the outer
/// edge of the picture is taken to zero at render time.
///
/// The band's outer edge is a cliff otherwise, and a wrap walks the picture
/// straight off it: a wrap moves the octave filling any given screen region
/// along by one, so the outermost octave is replaced by an octave that doesn't
/// exist, and everything it was drawing vanishes between one frame and the
/// next. Measured on `scenes/octave-edge-test.toml`, that outermost octave
/// carries 3.4% of the frame's brightness and 3.4% of its pixels change by
/// more than 10% — and not as noise, as one recognisable slab of structure.
///
/// [`MIN_RADIUS`] is supposed to keep the edge out of frame, and does for a
/// scene with full haze. But its derivation assumes haze takes material to
/// *nothing* by the far plane, and that only holds at `haze = 1.0`; below that
/// a constant fraction survives and no band radius is far enough. So the edge
/// has to stop being a cliff rather than merely being pushed away.
///
/// # Why this is a render-time weight and not a point deal
///
/// This replaces an earlier "octave fade" that thinned the outer shells in the
/// *deal* — the chaos shader dealt the outermost octaves fewer points. That
/// design cannot do the job, and no tuning of it could:
///
/// A static, world-space density profile is invisible across a wrap exactly
/// where it is flat, because the wrap is an exact similarity and only
/// scale-invariant density survives one. Anywhere density varies with radius,
/// the *entire* difference is delivered at the wrap instant, as a step. A fade
/// is by definition density varying with radius, so it spreads the change over
/// **screen area** while leaving all of it in **one frame** — the opposite of
/// what is wanted, which is the change spread over the progress of the zoom.
/// The old code said so itself: the per-wrap density ratio in the faded region
/// was the fade's own per-period attenuation, ≈0.4 for three octaves. Measured
/// live on `scenes/octave-edge-visual.toml`, the wrap spike went 35x the
/// median frame step with a hard edge and 10x with the fade, and 10x was that
/// design's floor rather than a residual bug.
///
/// The guard instead weights every point, every frame, by its distance from
/// the fixed point **in units of the current eye distance**:
///
/// ```text
///     ρ = |pos − p| / d           d = |eye − p| this frame
///     G = 1 − smoothstep(ln ρ)    over [ln ρ_start, ln ρ_end]
/// ```
///
/// `ρ` is invariant under the wrap similarity — it scales `|pos − p|` and `d`
/// by the same factor — so the wrap step is identically zero, at every haze
/// amount, by construction rather than by measurement. And zoom progress is
/// linear in `ln d`, so a feature crosses a ramp taken in `ln ρ` at a constant
/// rate per unit of zoom: material leaves the picture at a steady pace instead
/// of at a moment. Taking the ramp in `ρ` rather than `ln ρ` would fade fast
/// at the near end and slowly at the far end, which is the same complaint in
/// a smaller size.
///
/// It is the last stretch of haze, made mandatory and taken all the way to
/// zero, in ratio space — which is why a scene at `haze = 1.0` never had this
/// problem. Like haze it spends only transmittance, never colour.
///
/// # The width
///
/// One octave. [`Self::guard_span`] puts the ramp's outer end at the band's
/// authored radius (the true outermost material reaches `R/√s`, so a guard
/// that is zero at `R` hides the real edge at every phase, with margin) and
/// its inner end an octave further in. At the default radius of 4.8 that is
/// `[2.4, 4.8] × d`, and 2.4 is [`MIN_RADIUS`] almost exactly: the ramp lives
/// entirely in the part of the field that full haze would have hidden anyway,
/// and at weaker haze it costs a *constant* dimming of the far field, which is
/// invisible in motion because nothing about it changes.
///
/// A wider guard is clamped to the room the band has (see
/// [`Renorm::guard_span`]) so it can't eat into material the frame needs. A
/// scene may set 0 to turn it off, which restores the hard edge; that is for
/// measuring the artifact — `scenes/octave-edge-visual.toml` is built to show
/// it — and not for looking at.
///
/// # Measuring it
///
/// `tools/zoom_seam.py` steps the camera down through two periods and compares
/// mean frame brightness. That works because `--distance` is folded back into
/// the canonical period before anything renders, so the frames either side of
/// a wrap are ordinary stills. On `scenes/octave-edge-visual.toml`, which is
/// built to have the problem (haze 0, heavy structure in the outer shells),
/// the wrap step as a multiple of an ordinary frame step:
///
/// ```text
///     edge_guard = 0     11.9x       the artifact, once per period
///     edge_guard = 1      0.0x       0.00008 against 0.00239 ordinary
/// ```
///
/// On `wellspiral`, whose outermost octave was never in the picture, the wrap
/// measures 0.6-1.0x either way — the guard costs a scene without the problem
/// nothing, which the fade it replaced could not claim (three octaves of that
/// put a 4-6% step into scenes that had none).
///
/// Two things that do **not** measure it. Rendering the zoom loop to a file:
/// `offline::render_animation` wraps the camera every frame and a
/// `path_zoom_loop` covers exactly one period, so the seam comes out at 1.04x
/// an ordinary frame step whatever the radius and whatever the guard, even at
/// a radius that makes [`Renorm::summary`] print BAND TOO SHORT. And
/// per-pixel frame differencing: the cloud is sampled, so two cameras a period
/// apart draw the same structure from different points, and that noise runs
/// ~40x the signal. It cancels in the mean. Screen-recording the running app
/// also sees it, and is the check that matches what a person perceives.
const DEFAULT_EDGE_GUARD: f32 = 1.0;

/// `Σ rⁱ` for `i` in `0..n`, with the removable singularity at `r = 1` filled
/// in. Both pieces of [`Renorm::octave_offset`]'s distribution are geometric,
/// so this and [`geo_pick`] are the whole of its arithmetic.
fn geo_sum(r: f32, n: f32) -> f32 {
    if n <= 0.0 {
        0.0
    } else if (r - 1.0).abs() < 1e-4 {
        n
    } else {
        (1.0 - r.powf(n)) / (1.0 - r)
    }
}

/// Inverse of [`geo_sum`]: the largest `i` with `Σ_{j<i} rʲ ≤ x`, for `x` in
/// `[0, geo_sum(r, n))`. Valid for `r` above and below 1 — above, the sum is
/// rising and `1 − x(1−r) > 1`; below, `x`'s range keeps it above `rⁿ > 0`.
///
/// Capped at `ceil(n) − 1` rather than `n − 1`: a non-integer `n` means a
/// partial top octave, which the flat deal reaches and so this must too, and
/// clamping to `n − 1` would return a fractional octave offset.
fn geo_pick(x: f32, r: f32, n: f32) -> f32 {
    if n <= 0.0 {
        return 0.0;
    }
    let i = if (r - 1.0).abs() < 1e-4 {
        x
    } else {
        (1.0 - x * (1.0 - r)).max(1e-30).ln() / r.ln()
    };
    i.floor().clamp(0.0, n.ceil() - 1.0)
}

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
    /// Octaves over which the picture's outer edge is guarded to zero at
    /// render time (see [`DEFAULT_EDGE_GUARD`]). 0 restores the hard edge,
    /// which is a measurement tool rather than a look.
    ///
    /// In octaves, like `levels` and for the same reason: a zoom period is
    /// whatever the chosen map's contraction happens to be, so a width
    /// authored in periods would mean a different depth in every scene. Unlike
    /// the fade this replaced, it is a *ratio* band around the eye distance
    /// and never a distribution over shells — nothing about the point deal
    /// depends on it.
    pub edge_guard: f32,
}

impl Default for ZoomSpec {
    fn default() -> Self {
        Self {
            map: 0,
            radius: DEFAULT_RADIUS,
            levels: DEFAULT_LEVELS,
            octave_falloff: DEFAULT_OCTAVE_FALLOFF,
            edge_guard: DEFAULT_EDGE_GUARD,
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
    /// Rotation part of `A` (`A ≈ scale · rot`).
    ///
    /// Canonical, so its principal angle is in `[0, π]` and `twist` is
    /// literally how far the map turns. Everything downstream — the camera
    /// wrap, the GPU's closed-form power, the loop similarity — takes its
    /// branch from here and never re-derives one, which is what stops a
    /// fraction of a degree reading as very nearly a whole turn the other way.
    pub rot: Orientation,
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
    /// Width of the render-time edge guard in octaves, resolved: the authored
    /// width clamped to the room the band actually has (see
    /// [`Self::guard_span`]). Zero means no guard — a hard edge.
    ///
    /// In octaves and *not* converted to periods, unlike everything else here:
    /// the guard is a ratio band in world space, evaluated per point per
    /// frame, and knows nothing about which shell a point was dealt into.
    pub guard_width: f32,
    /// One period's rotation, as a displacement — the axis and angle the
    /// closed-form power of a similarity needs (see `renormalize()` in
    /// chaos.wgsl). Taken the short way round, once, here.
    pub twist: Turn,
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
        let (rot, defect) = crate::rot::orthonormalize(q);
        // The branch, chosen once and never again. `Orientation` is canonical,
        // so this is the principal turn: at most half a turn, in the direction
        // the map actually goes. Powers of it are taken by scaling this vector,
        // which stays linear and cannot wrap.
        let twist = Orientation::IDENTITY.shortest_turn_to(rot);

        // Authored in octaves; the shader deals in this map's own periods.
        // Capped at 64 because a barely-contracting map would otherwise ask
        // for thousands, and floored at 1 so the band is never empty.
        let periods = (spec.levels.max(0.0) * std::f32::consts::LN_2 / log_scale).clamp(1.0, 64.0);
        let octave_q = scale.powf(spec.octave_falloff.max(0.0));

        // The guard's width, clamped to the room between the band's edge and
        // the field the camera actually needs. The ramp runs inward from
        // `spec.radius`, so a width of W puts its inner end at
        // `radius / 2^W` — and anything inside MIN_RADIUS is material the
        // frustum wants, which the guard would then be dimming for nothing.
        //
        // Two-sided on purpose: a band with no room at all (an authored radius
        // at or below MIN_RADIUS) still gets the default width rather than
        // nothing, because a ramp eating slightly into the view is a steady
        // dimming while a hard edge is a snap. Such a band already prints BAND
        // TOO SHORT, which is the honest thing to fix.
        //
        // Nothing here touches `octave_q`: the falloff deals points and the
        // guard weights pixels, so they compose without either knowing about
        // the other.
        let room = (spec.radius.max(1e-6) / MIN_RADIUS).log2();
        let guard_width = if spec.edge_guard > 0.0 {
            spec.edge_guard.min(room.max(DEFAULT_EDGE_GUARD))
        } else {
            0.0
        };

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
            periods,
            octave_q,
            guard_width,
            twist,
            similar: defect <= SIMILARITY_TOLERANCE,
            band: reference_distance.max(1e-6),
        })
    }

    /// Which octave below the outer radius a point is dealt into, from a
    /// uniform `u` in `[0, 1)`. **The reference implementation of
    /// `octave_offset()` in `points/chaos.wgsl`** — the shader is the copy that
    /// runs, this is the copy that can be asserted about, and they must agree
    /// arithmetic for arithmetic.
    ///
    /// The share of octave `k` is `qᵏ` — the falloff envelope, and nothing
    /// else. `q = 1` (no falloff, the default and the only setting a flown
    /// scene should use) makes this `floor(u · periods)` exactly.
    ///
    /// It is geometric, so the inverse CDF is closed form: one sample per
    /// point, no rejection, which is the property that makes renormalization
    /// cost nothing in the first place.
    ///
    /// **Nothing about the camera appears here, and nothing may.** The point
    /// buffer is circular and turns over at 1/800th per frame, so a deal that
    /// depended on where the camera is would mix thirteen seconds of stale
    /// camera positions into every frame. That is the structural reason the
    /// edge guard is a render-time weight (see [`DEFAULT_EDGE_GUARD`]) rather
    /// than a taper on this distribution, which is what it used to be.
    ///
    /// **The reference implementation of `octave_offset()` in
    /// `points/chaos.wgsl`** — the shader is the copy that runs, this is the
    /// copy that can be asserted about, and they must agree arithmetic for
    /// arithmetic.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn octave_offset(&self, u: f32) -> f32 {
        let levels = self.periods;
        if levels <= 1.0 {
            return 0.0;
        }
        debug_assert!((0.0..1.0).contains(&u), "octave_offset wants a uniform in [0,1)");
        geo_pick(u * geo_sum(self.octave_q, levels), self.octave_q, levels)
    }

    /// The guard ramp in ratio units: `(ρ_start, ρ_end)`, multiples of the
    /// current eye-to-fixed-point distance. `None` when the guard is off.
    ///
    /// `ρ_end` is the band's authored radius. The band's true outermost
    /// material reaches `R/√s` — the `round()` in the sampler spreads the
    /// outer shell half a period past `R` — and its on-screen ratio is
    /// smallest when the eye is furthest out, at `d = band`, where it is
    /// `spec.radius/√s`. That is strictly greater than `ρ_end`, so a guard
    /// that has reached zero by `ρ_end` hides the real edge at every phase of
    /// the zoom, with margin.
    pub fn guard_span(&self) -> Option<(f32, f32)> {
        if self.guard_width <= 0.0 {
            return None;
        }
        let end = self.radius / self.band;
        Some((end * 2f32.powf(-self.guard_width), end))
    }

    /// The two numbers the shader wants, for an eye at `eye`:
    /// `(ln(ρ_start · d), 1 / ln(ρ_end / ρ_start))`, so a point at world
    /// radius `r` from the fixed point has ramp coordinate
    ///
    /// ```text
    ///     t = (ln r − ln_near) · inv_ln_width
    /// ```
    ///
    /// and weight `1 − smoothstep(0, 1, t)`. `(0, 0)` disables it — the shader
    /// branches on a zero width, so an ordinary scene pays one compare.
    ///
    /// `d` is recomputed every frame, and that is the whole mechanism: the
    /// ramp is nailed to the camera, not to the world, which is what makes it
    /// survive a wrap unchanged and advance smoothly between wraps.
    pub fn guard_params(&self, eye: Vec3) -> (f32, f32) {
        let Some((start, end)) = self.guard_span() else {
            return (0.0, 0.0);
        };
        let d = (eye - self.fixed_point).length().max(1e-20);
        ((start * d).ln(), 1.0 / (end / start).ln())
    }

    /// The share of a point's contribution the guard lets through, for a
    /// camera at `eye`. **The reference implementation of `guard_weight()` in
    /// `points/splat.wgsl` and `points/render.wgsl`**; the shaders are the
    /// copies that run and this is the copy the tests can assert about, so
    /// they must agree arithmetic for arithmetic.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn guard_weight(&self, pos: Vec3, eye: Vec3) -> f32 {
        let (ln_near, inv_ln_width) = self.guard_params(eye);
        if inv_ln_width == 0.0 {
            return 1.0;
        }
        let r = (pos - self.fixed_point).length().max(1e-20);
        let t = ((r.ln() - ln_near) * inv_ln_width).clamp(0.0, 1.0);
        1.0 - t * t * (3.0 - 2.0 * t)
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
        // Deliberately flat, and deliberately *not* [`Self::octave_offset`]:
        // this exists to reason about the geometry one point at a time, where
        // `spread` wants to mean "how far down the band", and 0 wants to mean
        // the outermost shell. How the point budget is actually dealt out is a
        // rendering concern; `octave_offset` is the mirror of that.
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

    /// How many octaves the band holds. The deal in `octave_offset` puts a
    /// point in one of these, so this is what the rewrap counts modulo.
    pub fn octaves(&self) -> f32 {
        self.periods.ceil().max(1.0)
    }

    /// The power of `f⁻¹` that carries a point sitting in octave `octave`
    /// through `turns` zoom periods and lands it back inside the band.
    ///
    /// **The reference implementation of the `m` in `rewrap()` in
    /// `points/chaos.wgsl`**; the shader is the copy that runs and this is the
    /// copy the tests can assert about, so they must agree arithmetic for
    /// arithmetic. Octave 0 is the outermost shell, at radius `radius`.
    ///
    /// It returns `turns` for every point except those that fall off an end of
    /// the band, which is the whole point: those keep their pixel across a
    /// wrap exactly, and only the outermost octave — the one `edge_guard` has
    /// already taken to nothing — is re-dealt.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rewrap_power(&self, octave: f32, turns: i32) -> f32 {
        let n = self.octaves();
        let shifted = octave - turns as f32;
        octave - (shifted - n * (shifted / n).floor())
    }

    /// The frame the world axes have been carried into after `turns` wraps:
    /// `rot⁻ᵗᵘʳⁿˢ`, in closed form.
    ///
    /// [`wrap`](Self::wrap) leaves the picture alone and moves the camera, and
    /// the part of that nobody has to think about until something is drawn in
    /// world axes is that it *turns* the camera — by `rot` per level, about an
    /// axis that is only the vertical for a map that happens to spin about the
    /// vertical. So a world direction holds still on screen across a wrap only
    /// if it is carried by the same rotation the camera was, and this is that
    /// rotation. The scale half needs no counterpart: a direction has none.
    ///
    /// Closed form for the same reason [`loop_similarity`](Self::loop_similarity)
    /// is, and with the same trap avoided — [`twist`](Self::twist) is an
    /// unbounded displacement, so twenty periods of a 45° map is 900° and not
    /// 180° the other way.
    pub fn carried_frame(&self, turns: i32) -> Orientation {
        if turns == 0 {
            return Orientation::IDENTITY;
        }
        (self.twist * -(turns as f32)).exp()
    }

    /// Whether the band reaches far enough out that its edge stays outside the
    /// picture across a wrap. See [`MIN_RADIUS`].
    pub fn band_covers_the_view(&self) -> bool {
        self.radius >= MIN_RADIUS * self.band
    }

    /// The similarity a path closes under when it loops by descending
    /// `periods` zoom periods: `Aᵖᵉʳⁱᵒᵈˢ`, in closed form.
    ///
    /// The rotation is carried as a [`Turn`] rather than a quaternion, and
    /// that is load-bearing. Scaling the twist vector by `n` is linear and
    /// unbounded, so four periods of a 47° map is a 188° sweep. Building a
    /// quaternion instead and reading an angle back out of it — which is what
    /// this used to do — folds 188° into 172° *the other way*, and the camera
    /// flies the loop backwards. Integer powers don't care about the branch;
    /// the path between them is nothing but branch.
    pub fn loop_similarity(&self, periods: u32) -> crate::path::ZoomLoop {
        let n = periods.max(1);
        crate::path::ZoomLoop {
            periods: n,
            center: self.fixed_point,
            scale: self.scale.powi(n as i32),
            turn: self.twist * n as f32,
        }
    }

    /// How far the map turns in one period, in degrees. Always the shorter way
    /// round, because [`Self::twist`] is canonical.
    pub fn twist_degrees(&self) -> f32 {
        self.twist.magnitude().to_degrees()
    }

    /// A one-line report for the CLI and the status bar
    pub fn summary(&self, name: Option<&str>) -> String {
        let short = if self.band_covers_the_view() {
            String::new()
        } else {
            format!(
                ", BAND TOO SHORT (radius {:.2}x the eye distance, needs {:.2}x) \
                 — material will blink out at each wrap",
                self.radius / self.band,
                MIN_RADIUS
            )
        };
        let seam = if self.defect > SIMILARITY_TOLERANCE {
            format!(", NOT a similarity (defect {:.2}) — the zoom wrap will show a seam", self.defect)
        } else {
            String::new()
        };
        let guard = match self.guard_span() {
            Some((start, end)) => format!(
                ", edge guard {:.1} octaves ({:.2}x-{:.2}x the eye distance){}",
                self.guard_width,
                start,
                end,
                // Only when the band has no room for the ramp outside the
                // field: the guard then dims material the frame wanted, which
                // is a steady cost rather than a wrap artifact, but it is the
                // radius that wants raising.
                if start < MIN_RADIUS {
                    " — the ramp reaches into the view; raise radius"
                } else {
                    ""
                }
            ),
            None => ", HARD OUTER EDGE (edge_guard = 0) — the wrap will step".to_string(),
        };
        format!(
            "infinite zoom on transform {}{}: scale {:.3} ({:.2} octaves/period), \
             {:.0}° twist, fixed point ({:.3}, {:.3}, {:.3}), {:.0} periods \
             ({:.1} octaves) rendered{}{}",
            self.map,
            name.map(|n| format!(" \"{}\"", n)).unwrap_or_default(),
            self.scale,
            self.log_scale / std::f32::consts::LN_2,
            self.twist_degrees(),
            self.fixed_point.x,
            self.fixed_point.y,
            self.fixed_point.z,
            self.periods,
            self.periods * self.log_scale / std::f32::consts::LN_2,
            guard,
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;
    use crate::camera::world_to_screen;

    fn spiral_map(scale: f32, twist_deg: f32) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(scale),
            Quat::from_rotation_y(twist_deg.to_radians()),
            Vec3::new(0.3, 0.1, -0.2),
        )
    }

    /// The same, with a band deep enough to have octaves to rotate between.
    /// `renorm`'s one level rounds up to two, which every permutation is.
    fn deep_renorm(scale: f32, twist: f32) -> Renorm {
        Renorm::from_affine(
            spiral_map(scale, twist),
            1.0,
            &ZoomSpec { map: 0, radius: 1.0, levels: 6.0, ..ZoomSpec::default() },
            1.0,
        )
        .unwrap()
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
        assert!((r.twist_degrees() - 31.0).abs() < 0.1, "twist {}", r.twist_degrees());
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
    fn a_wrap_carries_every_octave_but_the_outermost_untouched() {
        // The claim `rewrap` rests on: a wrap moves the camera by A⁻¹, and if
        // the points move by A⁻¹ too then every dot keeps its pixel. That has
        // to be *exactly* the power the rewrap applies, or the fix is a
        // different resample rather than none.
        let r = deep_renorm(0.6, 40.0);
        let n = r.octaves();
        assert!(n >= 3.0, "want a band with room in it, got {n}");

        for octave in 1..n as i32 {
            assert_eq!(
                r.rewrap_power(octave as f32, 1),
                1.0,
                "octave {octave} should ride the wrap out untouched"
            );
        }
        // The outermost is the one with nowhere further out to go, so it comes
        // back at the innermost — where the edge guard has already faded it to
        // nothing on the way past.
        assert_eq!(r.rewrap_power(0.0, 1), -(n - 1.0));
    }

    #[test]
    fn rewrapping_permutes_the_octaves_and_undoes_itself() {
        // Two properties that together say the band cannot drift: the deal is
        // still a deal after a wrap (nothing doubles up, nothing empties), and
        // scrubbing back and forth across the threshold — which a mouse wheel
        // does readily — returns every point exactly where it started.
        let r = deep_renorm(0.6, 40.0);
        let n = r.octaves() as i32;
        for turns in [-5, -1, 1, 2, 7] {
            let landed: Vec<i32> = (0..n)
                .map(|o| o - r.rewrap_power(o as f32, turns) as i32)
                .collect();
            let mut sorted = landed.clone();
            sorted.sort_unstable();
            assert_eq!(
                sorted,
                (0..n).collect::<Vec<_>>(),
                "carrying by {turns} must land one octave on each, got {landed:?}"
            );
            for o in 0..n {
                let there = o - r.rewrap_power(o as f32, turns) as i32;
                let back = there - r.rewrap_power(there as f32, -turns) as i32;
                assert_eq!(back, o, "carrying by {turns} and back lost octave {o}");
            }
        }
    }

    #[test]
    fn wrapping_the_camera_does_not_move_the_carried_axes() {
        // The axis cross's version of the seamlessness claim. A wrap turns the
        // camera, so world axes drawn against it swing by the map's rotation
        // on the frame the camera folds — a visible snap once a period on any
        // scene that spirals in rather than descending straight. Carrying them
        // by `carried_frame` has to cancel that exactly.
        let r = renorm(0.6, 40.0);
        let mut cam = OrbitCamera::from_chart(0.7, 0.35, 0.3, 1.0, r.fixed_point);
        // Start inside the band, so each step down is exactly one wrap.
        cam.distance = r.band * 0.8;

        // What the widget draws: each world axis in camera space.
        let screen = |cam: &OrbitCamera, frame: Orientation| {
            let to_cam = cam.orientation.inverse();
            [Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X, -Vec3::Y, -Vec3::Z]
                .map(|w| to_cam.rotate(frame.rotate(w)))
        };

        let mut turns = 0;
        let before = screen(&cam, r.carried_frame(turns));
        // Three periods, so the closed-form power is exercised rather than
        // just the single step.
        for period in 1..=3 {
            cam.distance *= r.scale;
            turns += r.wrap(&mut cam);
            assert_eq!(turns, period, "one period of zoom wraps exactly once");
            let after = screen(&cam, r.carried_frame(turns));
            for (b, a) in before.iter().zip(after.iter()) {
                assert!(
                    b.distance(*a) < 1e-4,
                    "axis moved {:.4} across wrap {}: {:?} -> {:?}",
                    b.distance(*a),
                    period,
                    b,
                    a
                );
            }
        }

        // And the carry is doing real work — without it the cross swings by
        // the map's 40°, which is what the snap looks like.
        let uncarried = screen(&cam, Orientation::IDENTITY);
        let moved = before
            .iter()
            .zip(uncarried.iter())
            .map(|(b, a)| b.distance(*a))
            .fold(0.0f32, f32::max);
        assert!(moved > 0.5, "uncarried axes should visibly swing, moved {moved:.3}");
    }

    #[test]
    fn wrapping_the_camera_does_not_move_the_picture() {
        // The seamlessness claim, as an assertion: after a wrap, every point of
        // the invariant set projects to the same pixel it did before. We test
        // it by projecting x with the wrapped camera and A(x) — its partner in
        // the invariant set — with the original.
        let r = renorm(0.6, 40.0);
        let cam = OrbitCamera::from_chart(0.7, 0.35, 0.3, 1.0, r.fixed_point + Vec3::new(0.05, -0.02, 0.01));
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
            // Same pixel is not enough: it has to be the same *brightness*, or
            // the seam is a density step instead of a jump. The edge guard is
            // the only camera-dependent weight there is, and it comes out
            // equal because it is a function of |pos − p| / |eye − p| and the
            // wrap scales both by s.
            let w_after = r.guard_weight(x, zoomed.eye());
            let w_before = r.guard_weight(partner, before.eye());
            assert!(
                (w_after - w_before).abs() < 1e-5,
                "point {} is weighted {} after the wrap but {} before",
                i,
                w_after,
                w_before
            );
        }
    }

    #[test]
    fn wrapping_is_idempotent_inside_the_band() {
        let r = renorm(0.5, 15.0);
        let mut cam = OrbitCamera::from_chart(0.2, 0.1, 0.0, r.band * 0.75, r.fixed_point);
        let before = cam;
        assert_eq!(r.wrap(&mut cam), 0);
        assert_eq!(cam.distance, before.distance);
        // Exactly untouched, not merely close: a no-op wrap composes nothing.
        assert_eq!(cam.orientation, before.orientation);
    }

    #[test]
    fn wrapping_recovers_from_far_outside() {
        let r = renorm(0.5, 15.0);
        let mut cam = OrbitCamera::from_chart(0.2, 0.1, 0.0, r.band * 5000.0, r.fixed_point);
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
    fn the_default_band_keeps_real_headroom_over_the_bound() {
        // Clearing MIN_RADIUS is not enough on its own. The bound is derived
        // assuming haze takes distant material all the way to nothing, which
        // only happens at haze = 1.0; every shipped scene runs 0.3-0.5 and two
        // drafts run 0.0, so at those strengths the edge is not hidden and the
        // margin is all there is. Roughly double the bound, not the 1.24x it
        // was.
        assert!(
            DEFAULT_RADIUS >= 1.9 * MIN_RADIUS,
            "default radius {DEFAULT_RADIUS} leaves too little over the {MIN_RADIUS} bound"
        );
    }

    #[test]
    fn widening_the_band_did_not_cost_visible_depth() {
        // The band is [R*2^-levels, R], so every doubling of R costs an octave
        // at the inner end. levels went up alongside radius to pay for it; the
        // inner edge must land at least as deep as the 3.0/14 it replaced.
        let inner_now = DEFAULT_RADIUS * 2f32.powf(-DEFAULT_LEVELS);
        let inner_before = 3.0 * 2f32.powf(-14.0);
        assert!(
            inner_now <= inner_before,
            "inner edge moved out from {inner_before:e} to {inner_now:e}"
        );
    }

    #[test]
    fn a_short_band_says_so() {
        let spec = ZoomSpec { radius: 1.2, ..ZoomSpec::default() };
        let r = Renorm::from_affine(spiral_map(0.6, 34.0), 1.0, &spec, 3.6).unwrap();
        assert!(!r.band_covers_the_view());
        assert!(r.summary(None).contains("BAND TOO SHORT"), "{}", r.summary(None));
    }

    /// Exact quadrature of `octave_offset`'s inverse CDF: `n` evenly spaced
    /// samples of `u` put each octave's count within `1/n` of its true mass,
    /// deterministically and with no RNG to argue with. Returns the fraction
    /// of points landing in each octave, indexed by offset.
    fn octave_histogram(r: &Renorm, n: usize) -> Vec<f64> {
        let mut hist = vec![0.0f64; r.periods.floor() as usize + 2];
        for i in 0..n {
            let u = (i as f32 + 0.5) / n as f32;
            let k = r.octave_offset(u);
            assert_eq!(k, k.floor(), "octave offsets must be whole: u {u} gave {k}");
            assert!(k >= 0.0 && (k as usize) < hist.len(), "offset {k} out of band");
            hist[k as usize] += 1.0 / n as f64;
        }
        hist
    }

    /// A map whose period is exactly one octave, so `periods` and `levels`
    /// are the same number and the arithmetic in these tests can be checked
    /// by hand. Radius 4.8 and a reference distance of 1, so the band is the
    /// default one and `guard_span` reads in the same units the scene uses.
    fn octave_renorm(levels: f32, guard: f32) -> Renorm {
        Renorm::from_affine(
            Mat4::from_scale(Vec3::splat(0.5)),
            1.0,
            &ZoomSpec { map: 0, radius: 4.8, levels, edge_guard: guard, octave_falloff: 0.0 },
            1.0,
        )
        .unwrap()
    }

    #[test]
    fn the_deal_is_flat_whenever_the_falloff_is_zero() {
        // Unconditionally, now: the deal has exactly one knob (`octave_falloff`,
        // for stills) and the edge is handled at render time. An octave holding
        // fewer points than its neighbour steps the on-screen density at every
        // wrap, so anything that is going to be flown wants this flat — and
        // there is no longer any setting that quietly makes it otherwise.
        let r = octave_renorm(15.0, 3.0);
        let hist = octave_histogram(&r, 150_000);
        for k in 0..15 {
            assert!(
                (hist[k] - 1.0 / 15.0).abs() < 1e-4,
                "octave {k} holds {} of the points, wanted 1/15",
                hist[k]
            );
        }
        assert!((hist.iter().sum::<f64>() - 1.0).abs() < 1e-9, "the deal must be a distribution");
    }

    #[test]
    fn a_falloff_on_its_own_is_still_exactly_geometric() {
        // The falloff is the only shape the deal has left, and it is untouched
        // by the guard: every octave still holds q times its neighbour.
        //
        // Only while there is mass to measure: a geometric deal empties fast,
        // and at q = 0.25 the sixth octave holds ninety samples out of half a
        // million, where the "ratio" is quantisation rather than distribution.
        let spec = ZoomSpec { octave_falloff: 2.0, ..ZoomSpec::default() };
        let r = Renorm::from_affine(Mat4::from_scale(Vec3::splat(0.5)), 1.0, &spec, 1.0).unwrap();
        let hist = octave_histogram(&r, 500_000);
        for k in 0..(r.periods as usize - 1) {
            if hist[k + 1] < 1e-3 {
                break;
            }
            let ratio = hist[k + 1] / hist[k];
            assert!(
                (ratio / r.octave_q as f64 - 1.0).abs() < 0.01,
                "octave {k}->{} ratio {ratio:.5}, wanted q = {}",
                k + 1,
                r.octave_q
            );
        }
    }

    #[test]
    fn the_guard_is_on_by_default_and_says_so() {
        // The edge is only lawful because something takes it to zero, so the
        // guard is the default and 0 is the opt-out — the reverse of the fade
        // it replaced, which was off by default because it cost a wrap step to
        // turn on. This one costs nothing at a wrap, by construction.
        assert_eq!(DEFAULT_EDGE_GUARD, 1.0);
        let r = Renorm::from_affine(spiral_map(0.6, 34.0), 1.0, &ZoomSpec::default(), 3.6).unwrap();
        assert_eq!(r.guard_width, 1.0);
        let (start, end) = r.guard_span().expect("the default scene must be guarded");
        assert!((end - DEFAULT_RADIUS).abs() < 1e-4, "the ramp must end at the band's edge");
        assert!((start - DEFAULT_RADIUS / 2.0).abs() < 1e-4, "one octave wide");
        assert!(r.summary(None).contains("edge guard"), "{}", r.summary(None));
    }

    #[test]
    fn turning_the_guard_off_is_reachable_and_loud() {
        // The escape hatch, for measuring the artifact rather than for looking
        // at: `scenes/octave-edge-visual.toml` sets it. It has to say so,
        // because a hard edge is a bug everywhere else.
        let spec = ZoomSpec { edge_guard: 0.0, ..ZoomSpec::default() };
        let r = Renorm::from_affine(spiral_map(0.6, 34.0), 1.0, &spec, 3.6).unwrap();
        assert_eq!(r.guard_width, 0.0);
        assert!(r.guard_span().is_none());
        assert_eq!(r.guard_params(Vec3::ZERO), (0.0, 0.0));
        assert!(r.summary(None).contains("HARD OUTER EDGE"), "{}", r.summary(None));
    }

    #[test]
    fn the_guard_is_clamped_to_the_room_the_band_has() {
        // The ramp runs inward from the band's edge, so a wide one reaches
        // into material the frustum still wants and dims it for nothing. The
        // clamp is what lets a scene ask for 3 without having to know its own
        // radius: it gets as much as the band can give.
        for (radius, want) in [(4.8, 1.0), (9.7, 2.0), (19.4, 3.0)] {
            let spec = ZoomSpec { radius, edge_guard: 3.0, ..ZoomSpec::default() };
            let r = Renorm::from_affine(spiral_map(0.6, 34.0), 1.0, &spec, 1.0).unwrap();
            assert!(
                (r.guard_width - want).abs() < 0.05,
                "radius {radius} allows {} octaves of guard, wanted about {want}",
                r.guard_width
            );
            let (start, _) = r.guard_span().unwrap();
            assert!(
                start >= MIN_RADIUS * 0.99,
                "radius {radius}: ramp starts at {start}, inside the visible field"
            );
        }
    }

    #[test]
    fn a_band_too_short_to_guard_still_gets_the_default_width() {
        // A dimmed-but-steady view beats a snap, so the clamp has a floor: an
        // authored radius inside MIN_RADIUS lets the ramp eat inward rather
        // than giving up the guard. The BAND TOO SHORT warning is what tells
        // you to fix the real problem.
        let spec = ZoomSpec { radius: 1.2, edge_guard: 3.0, ..ZoomSpec::default() };
        let r = Renorm::from_affine(spiral_map(0.6, 34.0), 1.0, &spec, 1.0).unwrap();
        assert_eq!(r.guard_width, DEFAULT_EDGE_GUARD);
        assert!(!r.band_covers_the_view());
        assert!(r.summary(None).contains("BAND TOO SHORT"), "{}", r.summary(None));
    }

    #[test]
    fn the_guard_hides_the_bands_real_edge_at_every_phase() {
        // The outermost material sits at R/sqrt(s), half a period past R,
        // because the sampler rounds. The guard has to have reached zero by
        // there wherever the camera is inside its period, or the cliff shows
        // at some phase and not at others.
        let r = octave_renorm(15.0, 1.0);
        let outermost = r.radius / r.scale.sqrt();
        for i in 0..=20 {
            // Every eye distance in one period, inner edge to outer
            let d = r.band * r.scale.powf(i as f32 / 20.0);
            let eye = r.fixed_point + Vec3::new(0.6, -0.5, 0.62).normalize() * d;
            let pos = r.fixed_point + Vec3::new(-0.3, 0.9, 0.31).normalize() * outermost;
            assert_eq!(
                r.guard_weight(pos, eye),
                0.0,
                "the band's edge is visible at eye distance {d}"
            );
        }
    }

    #[test]
    fn the_guard_advances_at_a_constant_rate_per_octave_of_zoom() {
        // The reason the ramp is taken in ln(rho) and not in rho. Zoom
        // progress is linear in ln d, so equal zoom steps must cross equal
        // fractions of the ramp — otherwise material leaves the picture fast
        // at one end of the ramp and slowly at the other, which is the same
        // "it happens all at once" complaint in a smaller size.
        let r = octave_renorm(15.0, 1.0);
        let pos = r.fixed_point + Vec3::new(1.0, 0.2, -0.4).normalize() * r.band * 3.0;
        let ramp = |d: f32| {
            let eye = r.fixed_point + Vec3::X * d;
            let (ln_near, inv) = r.guard_params(eye);
            ((pos - r.fixed_point).length().ln() - ln_near) * inv
        };
        // Eight equal steps in ln d; the ramp coordinate must step equally too
        let step = (2f32).powf(-0.1);
        let mut d = r.band;
        let first = ramp(d * step) - ramp(d);
        for _ in 0..8 {
            let delta = ramp(d * step) - ramp(d);
            assert!(
                (delta - first).abs() < 1e-4,
                "a 0.1-octave zoom moved the ramp by {delta}, not {first}"
            );
            d *= step;
        }
        // And that rate is exactly "one ramp width per guard_width octaves":
        // zooming in moves material outward through the ramp, so it is a
        // positive step toward the far end.
        assert!(
            (first - 0.1 / r.guard_width).abs() < 1e-4,
            "0.1 octaves of zoom should cross 0.1/{} of the ramp, got {first}",
            r.guard_width
        );
    }

    #[test]
    fn material_leaves_the_picture_smoothly_across_wraps() {
        // The end-to-end statement of what the guard is for, and the thing the
        // fade it replaced could not do: follow one piece of the invariant set
        // out through the guard while the camera zooms, wraps and keeps going,
        // and there must be no frame where it changes by more than the others.
        //
        // The wrap moves the camera by A^-1, so the material that fills a
        // given pixel afterward is the partner under A^-1 of what filled it
        // before — both are in the invariant set. Carrying the tracked feature
        // through the same map is what makes this "the same thing on screen"
        // rather than "the same coordinates".
        let r = octave_renorm(15.0, 1.0);
        let mut cam =
            OrbitCamera::from_chart(0.3, 0.2, 0.0, r.band * 0.999, r.fixed_point);
        let mut pos = r.fixed_point + Vec3::new(0.2, 0.9, -0.4).normalize() * r.band * 2.4;
        let mut prev = r.guard_weight(pos, cam.eye());
        assert!(prev > 0.99, "the feature should start unguarded, at {prev}");

        let mut worst = 0.0f32;
        let mut reached_zero = false;
        // 0.5% per step, ~139 steps per octave: three octaves of zoom, which
        // is three wraps for this map.
        for _ in 0..420 {
            cam.distance *= 0.995;
            for _ in 0..r.wrap(&mut cam) {
                pos = r.fixed_point + r.a_inv * (pos - r.fixed_point);
            }
            let w = r.guard_weight(pos, cam.eye());
            assert!(w <= prev + 1e-6, "the guard must not brighten as we zoom in: {prev} -> {w}");
            worst = worst.max(prev - w);
            prev = w;
            reached_zero |= w == 0.0;
        }
        assert!(reached_zero, "three octaves of zoom should retire the feature entirely");
        // A 0.5% zoom step crosses 0.0072 of a one-octave ramp, and the
        // smoothstep's steepest point is 1.5x its mean slope: 0.011. Anything
        // near a whole octave's worth in one frame is the wrap stepping.
        assert!(worst < 0.02, "worst single-frame change {worst}, wanted a smooth fade");
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
