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
/// [`DEFAULT_OCTAVE_FADE`] and the two are easy to reach for interchangeably.
/// Octave 0 is the outermost shell and the share of octave `k` is `qᵏ`, so the
/// falloff thins the *innermost* octaves — the small ones clustered around the
/// fixed point, which is the middle of the picture. Measured on
/// `octave-edge-test`, a falloff of 2 changes 40% of the material within 12% of
/// the frame's radius of centre and 25% out at the rim. The fade does the
/// reverse: 2% at centre, 27% at the rim. Neither is backwards; they are two
/// knobs for two ends.
const DEFAULT_OCTAVE_FALLOFF: f32 = 0.0;

/// Octaves over which the band's *outer* edge fades out instead of stopping.
///
/// The edge is a cliff otherwise, and a wrap walks the picture straight off it:
/// a wrap moves the octave filling any given screen region along by one, so the
/// outermost octave is replaced by an octave that doesn't exist, and everything
/// it was drawing vanishes between one frame and the next. Measured on
/// `scenes/octave-edge-test.toml`, that outermost octave carries 3.4% of the
/// frame's brightness and 3.4% of its pixels change by more than 10% — and not
/// as noise, as one recognisable slab of structure.
///
/// [`MIN_RADIUS`] is supposed to keep the edge out of frame, and does for a
/// scene with full haze. But its derivation assumes haze takes material to
/// *nothing* by the far plane, and that only holds at `haze = 1.0`; below that
/// a constant fraction survives and no band radius is far enough. So the edge
/// has to stop being a cliff rather than merely being pushed away.
///
/// Over these octaves the point budget per octave ramps from [`FADE_DEPTH`] of
/// full at the outermost shell up to full, and what falls off the end is a
/// sixteenth of a shell rather than a whole one.
///
/// **Off, and that is a measurement rather than a preference.** The plan this
/// came from decided it should be on by default; the measurement says no, for
/// two reasons that only showed up once there was something to measure.
///
/// The method: a wrap moves every shell one octave inward, so rendering a
/// scene at `radius` and at `radius · s` gives exactly the two frames either
/// side of a wrap — no animation, no interpolation, and the offline renderer
/// is deterministic, so the difference is all signal. Mean brightness ratio
/// across that pair, and the worst single pixel:
///
/// ```text
///                              octave_fade:   0        1        2        3
///   wellspiral        haze 0.50  wrap ratio   0.9999   0.9981   0.9639   0.9399
///   pythagoras-zoomy  haze 0.00  wrap ratio   1.0000   -        -        0.9621
///   octave-edge-test  haze 0.12  wrap ratio   0.9669   0.9654   0.9644   0.9492
///                                worst pixel  0.399    0.413    0.327    0.298
/// ```
///
/// **1. Most scenes have nothing to fix.** `wellspiral` and `pythagoras-zoomy`
/// wrap at 0.9999 and 1.0000 with a hard edge — their outermost octave simply
/// isn't in the picture. Every octave of fade is then pure cost, and three of
/// them put a 4-6% brightness step at each wrap where there had been none. In
/// a rendered loop `pythagoras-zoomy` picks up a 3.0% mid-loop pop against
/// 0.13% with the edge hard.
///
/// **2. The fade cannot make the step smaller, only wider.** The share of
/// octave `k` after a wrap is the share of `k-1` before it, so the change
/// summed over octaves telescopes to exactly one octave's worth for *any*
/// monotone ramp — a hard cut included. On `octave-edge-test` the wrap costs
/// 3.40% of frame brightness with a hard edge and 3.44% with a 2.3-octave
/// fade. What the fade buys is entirely in *where* that change lands: worst
/// pixel 0.399 -> 0.298, and the difference image goes from one solid slab of
/// structure to a faint texture spread over the whole frame. That is the
/// difference between "a branch blinked out" and "the picture dimmed
/// slightly", which is worth having — but it is a redistribution, not a cure,
/// and it is only worth paying for on a scene that has the problem.
///
/// So: off unless asked for, and worth asking for on a scene whose bulk sits
/// far enough from the fixed point to fill the band's outer octaves. The
/// two-render check above is cheap and is the way to tell;
/// `scenes/octave-edge-test.toml` is built to fail it.
///
/// **Measure it live, not offline.** The two-render check above compares
/// stills and is sound, but the obvious end-to-end version of it — render the
/// zoom loop to a file and look for a step at the seam — measures nothing.
/// `offline::render_animation` wraps the camera every frame and a
/// `path_zoom_loop` covers exactly one period, so the seam comes out at 1.04x
/// an ordinary frame step whatever the radius and whatever the fade, even at a
/// radius that makes [`Renorm::summary`] print BAND TOO SHORT. Screen-recording
/// the running app shows it immediately: on `scenes/octave-edge-visual.toml`
/// the wrap is a spike every 2.5s at 35x the median frame step, and the fade
/// takes it to 10x — 42x smaller in absolute terms.
///
/// And it needs the edge at or beyond [`MIN_RADIUS`] to work at all. Pulling
/// the edge inside the frustum to make the artifact easier to see also removes
/// the full-density core the taper ramps up to meet, at which point the taper
/// swings the whole frame's brightness instead of a rim of it: same scene at
/// radius 1.4, the fade takes the spike from 61x to 86x. It fixes an edge; it
/// cannot fix a band that is mostly edge.
const DEFAULT_OCTAVE_FADE: f32 = 0.0;

/// The fade width to reach for on a scene that wants one, in octaves. Not a
/// default — see [`DEFAULT_OCTAVE_FADE`] for why there isn't one — but the
/// value that measured best on the scene built to need it, and so the number
/// the docs and `scenes/octave-edge-test.toml` quote. Referenced from the
/// tests and from prose rather than from code, which is the point of it.
#[allow(dead_code)]
pub const SUGGESTED_OCTAVE_FADE: f32 = 3.0;

/// Density at the outermost shell, as a fraction of a full octave's share.
///
/// Derived rather than authored, and `octave_fade` is the only knob: fixing the
/// depth and letting the width set the steepness means one number controls
/// something visible, instead of two numbers that trade off against each other
/// in a way nobody can see. A sixteenth is dim enough that losing it off the
/// end of the band is not a visible event.
const FADE_DEPTH: f32 = 1.0 / 16.0;

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
    /// Octaves over which the band's outer edge fades out rather than
    /// stopping (see [`DEFAULT_OCTAVE_FADE`]). 0 restores the hard edge.
    ///
    /// In octaves, like `levels` and for the same reason: a zoom period is
    /// whatever the chosen map's contraction happens to be, so a fade authored
    /// in periods would mean a different depth in every scene.
    pub octave_fade: f32,
}

impl Default for ZoomSpec {
    fn default() -> Self {
        Self {
            map: 0,
            radius: DEFAULT_RADIUS,
            levels: DEFAULT_LEVELS,
            octave_falloff: DEFAULT_OCTAVE_FALLOFF,
            octave_fade: DEFAULT_OCTAVE_FADE,
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
    /// How many *periods* at the outer end of the band get less than a full
    /// octave's point share, so the edge fades instead of cutting. The
    /// authored octave count converted to this map's own periods and rounded
    /// to a whole number: a period is the step a wrap takes, so a fractional
    /// one has no meaning here and only costs the sampler its closed form.
    /// Zero disables the taper.
    pub fade_periods: f32,
    /// Per-period attenuation across [`Self::fade_periods`], derived so the
    /// outermost shell lands at [`FADE_DEPTH`] of a full share:
    /// `g = FADE_DEPTH^(1/fade_periods)`. This is also, exactly, how much the
    /// on-screen density of the faded region changes at each wrap.
    pub fade_g: f32,
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

        // The taper, in periods, rounded: a period is the step a wrap takes,
        // so a fraction of one is not a thing the distribution can express,
        // and pretending otherwise costs the sampler its closed form for no
        // visible gain.
        //
        // Never more than half the band. `periods` is clamped above, so a very
        // gentle map's band can end up far shallower than the authored octave
        // count asked for — and a fade authored as "3 octaves" would then eat
        // most of what's left. Half keeps a substantial full-density core no
        // matter how the clamp bites.
        //
        // Composes with `octave_falloff` rather than deferring to it. It used
        // to be forced to zero whenever the falloff was in play, on the theory
        // that nothing wants both — which was wrong twice over. The falloff is
        // the knob you reach for when the *centre* looks wrong and the fade is
        // the knob for the *edge*, so wanting both is ordinary; and because the
        // override was silent, dialling up the falloff turned the fade off
        // under you, which reads as the fade being broken.
        let fade_periods = (spec.octave_fade.max(0.0) * std::f32::consts::LN_2 / log_scale)
            .round()
            .clamp(0.0, (periods * 0.5).floor());
        // g such that fade_periods steps of it take a full share down to
        // FADE_DEPTH. Also the per-wrap density ratio in the faded region.
        let fade_g = if fade_periods >= 1.0 {
            FADE_DEPTH.powf(1.0 / fade_periods)
        } else {
            1.0
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
            fade_periods,
            fade_g,
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
    /// One shape, two pieces, both geometric. The share of octave `k` is
    ///
    /// ```text
    ///     k ≥ F:   qᵏ                    the falloff envelope, untouched
    ///     k < F:   q^F · g^(F−k)         the taper, rising to meet it at F
    /// ```
    ///
    /// so it climbs from [`FADE_DEPTH`] of the envelope at the outermost shell
    /// up to the envelope at `k = F` and is the envelope from there inward.
    /// `q = 1` (no falloff) leaves a flat core; `F = 0` (no fade) leaves the
    /// bare envelope; both off is `floor(u · periods)` exactly. The two knobs
    /// therefore compose without either one having to know about the other.
    ///
    /// The taper is anchored to the envelope's value *at `F`* rather than
    /// multiplied through it, and that is load-bearing. Multiplying would give
    /// share `qᵏ·g^(F−k) = g^F·(q/g)ᵏ`, which falls rather than rises whenever
    /// `q < g` — a steep falloff would invert the taper and make the outermost
    /// shell the brightest, which is the exact opposite of the job. Anchoring
    /// keeps the ramp monotone for every `q`.
    ///
    /// Both pieces being geometric is what keeps the inverse CDF closed-form:
    /// still one sample per point, still no rejection, which is the property
    /// that makes the whole construction cost nothing.
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
        let f = if self.fade_periods >= 1.0 && self.fade_g < 0.9999 {
            self.fade_periods
        } else {
            0.0
        };
        // Masses in units of q^F, which cancels between the two pieces and so
        // never has to be computed.
        let m1 = if f >= 1.0 {
            FADE_DEPTH * geo_sum(1.0 / self.fade_g, f)
        } else {
            0.0
        };
        let m2 = geo_sum(self.octave_q, levels - f);
        let x = u * (m1 + m2);
        if x < m1 {
            geo_pick(x / FADE_DEPTH, 1.0 / self.fade_g, f)
        } else {
            f + geo_pick(x - m1, self.octave_q, levels - f)
        }
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
        let fade = if self.fade_periods >= 1.0 {
            format!(
                ", outer {:.1} octaves faded (x{:.2}/period)",
                self.fade_periods * self.log_scale / std::f32::consts::LN_2,
                self.fade_g
            )
        } else {
            ", hard outer edge".to_string()
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
            fade,
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
    /// by hand.
    fn octave_renorm(levels: f32, fade: f32) -> Renorm {
        Renorm::from_affine(
            Mat4::from_scale(Vec3::splat(0.5)),
            1.0,
            &ZoomSpec { map: 0, radius: 4.8, levels, octave_fade: fade, octave_falloff: 0.0 },
            1.0,
        )
        .unwrap()
    }

    #[test]
    fn the_taper_matches_its_target_distribution() {
        // The taper is a claim about a distribution, and a picture can't check
        // one. Share of octave k is g^(F-k) up to k = F, then flat.
        let r = octave_renorm(15.0, 3.0);
        assert_eq!(r.fade_periods, 3.0, "3 octaves of fade on a 1-octave period");
        let g = r.fade_g;
        let hist = octave_histogram(&r, 2_000_000);

        let flat = hist[r.fade_periods as usize]; // the first un-tapered octave
        assert!(flat > 0.0);
        for k in 0..r.periods as usize {
            let want = if (k as f32) < r.fade_periods {
                (g as f64).powf(r.fade_periods as f64 - k as f64)
            } else {
                1.0
            };
            let got = hist[k] / flat;
            assert!(
                (got - want).abs() < 2e-3,
                "octave {k} holds {got:.5} of a full share, wanted {want:.5}"
            );
        }
        let total: f64 = hist.iter().sum();
        assert!((total - 1.0).abs() < 1e-9, "the deal must be a distribution: {total}");
    }

    #[test]
    fn the_taper_reaches_the_intended_depth_and_no_further() {
        // The whole point: what falls off the end of the band is FADE_DEPTH of
        // an octave rather than a whole one. If this drifts, the fade stops
        // hiding the cut (too shallow) or starts eating the scene (too deep).
        let r = octave_renorm(15.0, 3.0);
        let hist = octave_histogram(&r, 2_000_000);
        let depth = hist[0] / hist[r.fade_periods as usize];
        assert!(
            (depth - FADE_DEPTH as f64).abs() < 1e-3,
            "outermost shell holds {depth:.5} of a share, wanted {}",
            FADE_DEPTH
        );
        // And the shell just inside the fade is at full share, not still ramping
        assert!(
            (hist[3] / hist[4] - 1.0).abs() < 1e-3,
            "the ramp must have finished by octave F"
        );
    }

    #[test]
    fn the_taper_moves_points_inward_rather_than_discarding_them() {
        // Every point still lands somewhere: the taper is a redistribution of
        // a fixed budget, not a cull, so the core gets slightly *denser*.
        let hard = octave_histogram(&octave_renorm(15.0, 0.0), 200_000);
        let faded = octave_histogram(&octave_renorm(15.0, 3.0), 200_000);
        assert!((hard.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!((faded.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        for k in 0..3 {
            assert!(faded[k] < hard[k], "octave {k} should be thinned");
        }
        for k in 3..15 {
            assert!(faded[k] > hard[k], "octave {k} should pick up the difference");
        }
    }

    #[test]
    fn no_fade_is_the_flat_deal_exactly() {
        // The old behaviour has to remain reachable, and reachable *exactly* —
        // this is the escape hatch if the taper turns out wrong for a scene.
        let r = octave_renorm(15.0, 0.0);
        assert_eq!(r.fade_periods, 0.0);
        let hist = octave_histogram(&r, 150_000);
        for k in 0..15 {
            assert!(
                (hist[k] - 1.0 / 15.0).abs() < 1e-4,
                "octave {k} holds {} of the points, wanted 1/15",
                hist[k]
            );
        }
    }

    #[test]
    fn a_falloff_on_its_own_is_still_exactly_geometric() {
        // The falloff's own shape has to survive the fade being bolted on
        // beside it: with no fade asked for, every octave still holds q times
        // its neighbour.
        //
        // Only while there is mass to measure: a geometric deal empties fast,
        // and at q = 0.25 the sixth octave holds ninety samples out of half a
        // million, where the "ratio" is quantisation rather than distribution.
        let spec = ZoomSpec { octave_falloff: 2.0, ..ZoomSpec::default() };
        let r = Renorm::from_affine(Mat4::from_scale(Vec3::splat(0.5)), 1.0, &spec, 1.0).unwrap();
        assert_eq!(r.fade_periods, 0.0);
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
    fn the_fade_and_the_falloff_compose_instead_of_overriding() {
        // They used to be mutually exclusive, falloff winning and silently. It
        // was the wrong call: they act on opposite ends of the band — falloff
        // thins the middle of the picture, the fade thins its rim — so wanting
        // both is ordinary, and because the override was silent, reaching for
        // the falloff turned the fade off under you.
        let spec =
            ZoomSpec { octave_falloff: 2.0, octave_fade: 3.0, levels: 15.0, ..ZoomSpec::default() };
        let r = Renorm::from_affine(Mat4::from_scale(Vec3::splat(0.5)), 1.0, &spec, 1.0).unwrap();
        let f = r.fade_periods;
        assert_eq!(f, 3.0, "the fade must survive a falloff");
        assert!(r.summary(None).contains("octaves faded"));

        let hist = octave_histogram(&r, 500_000);

        // Inward of the fade the falloff is untouched: still q per octave.
        let kf = f as usize;
        for k in kf..(r.periods as usize - 1) {
            if hist[k + 1] < 1e-3 {
                break;
            }
            let ratio = hist[k + 1] / hist[k];
            assert!(
                (ratio / r.octave_q as f64 - 1.0).abs() < 0.01,
                "octave {k}->{} ratio {ratio:.5} inside the envelope, wanted q",
                k + 1
            );
        }

        // Across the fade the ramp rises outward-to-inward — the property a
        // naive product would lose, since q < g here would flip it — and it
        // arrives at FADE_DEPTH of the octave it ramps up to meet.
        for k in 0..kf {
            assert!(
                hist[k] < hist[k + 1],
                "octave {k} ({}) must be dimmer than {} ({}) across the fade",
                hist[k],
                k + 1,
                hist[k + 1]
            );
        }
        let depth = hist[0] / hist[kf];
        assert!(
            (depth / FADE_DEPTH as f64 - 1.0).abs() < 0.02,
            "outermost octave is {depth:.5} of the octave at F, wanted {FADE_DEPTH}"
        );
    }

    #[test]
    fn the_taper_never_eats_more_than_half_the_band() {
        // `periods` is clamped at 64, so a barely-contracting map's band can
        // come out far shallower than the authored octave count asked for. A
        // fade authored in octaves would then swallow most of what's left;
        // this is the floor under how much full-density band always survives.
        let r = Renorm::from_affine(spiral_map(0.97, 5.0), 1.0, &ZoomSpec {
            levels: 40.0,
            octave_fade: 30.0,
            ..ZoomSpec::default()
        }, 1.0)
        .unwrap();
        assert!(
            r.fade_periods <= r.periods * 0.5,
            "fade {} of {} periods",
            r.fade_periods,
            r.periods
        );
        let hist = octave_histogram(&r, 500_000);
        let full: f64 = hist[r.fade_periods as usize..].iter().sum();
        assert!(full > 0.5, "only {full:.3} of the budget is at full share");
    }

    #[test]
    fn a_fade_wider_than_the_band_still_deals_every_point() {
        // Degenerate asks must not produce NaN offsets or an empty band.
        for (levels, fade) in [(1.0, 8.0), (2.0, 30.0), (15.0, 0.5), (15.0, 1000.0)] {
            let r = octave_renorm(levels, fade);
            let hist = octave_histogram(&r, 20_000);
            assert!(
                (hist.iter().sum::<f64>() - 1.0).abs() < 1e-9,
                "levels {levels} fade {fade} lost points"
            );
        }
    }

    #[test]
    fn the_fade_is_off_by_default_and_says_so() {
        // Not a preference: on three of the four scenes measured, the wrap is
        // already seamless with a hard edge (0.9999, 1.0000) and a three-octave
        // fade puts a 4-6% brightness step into it. See DEFAULT_OCTAVE_FADE.
        // Changing this default means re-running that measurement.
        assert_eq!(DEFAULT_OCTAVE_FADE, 0.0);
        let r = Renorm::from_affine(spiral_map(0.6, 34.0), 1.0, &ZoomSpec::default(), 3.6).unwrap();
        assert_eq!(r.fade_periods, 0.0);
        assert_eq!(r.fade_g, 1.0);
        assert!(r.summary(None).contains("hard outer edge"), "{}", r.summary(None));
    }

    #[test]
    fn asking_for_the_fade_turns_it_on_and_reports_it() {
        let spec = ZoomSpec { octave_fade: SUGGESTED_OCTAVE_FADE, ..ZoomSpec::default() };
        let r = Renorm::from_affine(spiral_map(0.6, 34.0), 1.0, &spec, 3.6).unwrap();
        assert!(r.fade_periods >= 1.0, "an authored fade must reach the shader");
        assert!(r.fade_g < 1.0);
        let s = r.summary(None);
        assert!(s.contains("faded"), "{s}");
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
