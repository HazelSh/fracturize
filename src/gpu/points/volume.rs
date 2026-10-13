//! Volume mode's host-side numbers: the uniform the shaders read, the depth
//! range the moments are warped into, and the opacity control's limits.
//!
//! The model is in `VOLUME-RENDERER-PLAN.md`; the short version is that splat
//! is additive emission, and volume mode adds absorption so near material hides
//! what is behind it. Two passes — absorbance moments, then emission weighted
//! by the transmittance those moments give (see `moments.rs`) — over the same
//! point buffer. Opacity 0 is today's splat, byte for byte: neither pass runs.

use glam::Vec3;

use crate::trace::AttractorStats;

/// Absorbance per unit of normalised density at opacity 1.
///
/// "Normalised density" is splat's own `density · H² / samples`, the quantity
/// `exposure_scale` multiplies (without its `EXPOSURE_K`, which is a brightness
/// calibration and has no business in a property of the material). So opacity 1
/// means: where the splat renderer would read a normalised density of 1, a ray
/// crossing the pixel accumulates one unit of optical depth. Because it is
/// built from exactly the same ratio, it is invariant to point count,
/// resolution and supersampling by the same argument exposure is.
pub const ABSORBANCE_K: f32 = 1.0;

/// Opacity is a logarithmic control; these are its ends. 0 is "off" and lives
/// below the slider rather than on it, since a log scale has no zero.
pub const MIN_OPACITY: f32 = 0.01;
pub const MAX_OPACITY: f32 = 100.0;

/// What the window starts with in volume mode, and `--volume` without
/// `--opacity`.
///
/// Chosen by rendering `nautilus`, `censer` and `blossom` at 0.3 / 1 / 2 / 3 /
/// 10 / 30 and looking: at 1 the depth effect is faint, at 2 the far fronds sink
/// behind the near body and the smoke's front sheets stand out of the lace behind
/// them, and by 10 the cores are nearly solid and the thin filaments have gone
/// dim. 2 is the smallest value at which occlusion is unmistakable on all three.
/// A starting position on a dial, not a calibration — and it costs brightness
/// (see the report: saturated pixels carry ~1/opacity of splat's light).
pub const DEFAULT_OPACITY: f32 = 2.0;

/// The uniform both volume passes read. 32 bytes, must match `VolumeParams` in
/// `shaders/points/splat.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VolumeParams {
    /// 1 when the volume passes are drawing, else 0
    pub enabled: f32,
    /// `opacity · ABSORBANCE_K · H² / samples`
    pub abs_scale: f32,
    /// `z = ln(depth) · warp_scale + warp_bias`
    pub warp_scale: f32,
    pub warp_bias: f32,
    /// Constant emission multiplier for the frame, `emission_gain(opacity)`.
    pub emission_gain: f32,
    /// Gaussian sigma, in accumulation pixels, of the blur applied to the
    /// moments before pass B. 0 is off.
    pub blur_px: f32,
    pub _pad: [f32; 2],
}

impl VolumeParams {
    /// Everything off. What a plain splat leaves in the buffer, and what the
    /// vertex stage reads as "skip the volume work".
    pub const OFF: VolumeParams = VolumeParams {
        enabled: 0.0,
        abs_scale: 0.0,
        warp_scale: 0.0,
        warp_bias: 0.0,
        emission_gain: 1.0,
        blur_px: 0.0,
        _pad: [0.0; 2],
    };

    /// `supersample` is the factor the target is supersampled by (1 live), since
    /// the absorbance blur is a width in *output* pixels.
    ///
    /// `samples` and `screen_height` are exactly the two numbers
    /// `SplatRenderer::upload_params` normalises exposure with — the *same
    /// values*, because the whole invariance claim is that they cancel against
    /// the same density in the same way.
    pub fn new(
        opacity: f32,
        samples: f64,
        screen_height: f32,
        supersample: u32,
        range: DepthRange,
    ) -> Self {
        if opacity <= 0.0 || samples <= 0.0 {
            return Self::OFF;
        }
        let abs_scale = (opacity as f64 * ABSORBANCE_K as f64 * screen_height as f64
            * screen_height as f64
            / samples) as f32;
        let (warp_scale, warp_bias) = range.warp();
        let blur_px = BLUR_OUTPUT_PX * supersample.max(1) as f32;
        Self {
            enabled: 1.0,
            abs_scale,
            warp_scale,
            warp_bias,
            emission_gain: emission_gain(opacity),
            blur_px,
            _pad: [0.0; 2],
        }
    }
}

/// Brightness compensation constant; see [`emission_gain`].
///
/// **Measured, not chosen** — and the measurement says no single constant
/// flattens every scene. Whole-image excess luminance above the background,
/// relative to opacity 0 (4M points, 960x540, blur and overestimation as
/// shipped), by opacity 1 / 3 / 10 / 30:
///
/// ```text
///                    c = 1.2                    c = 0.4 (shipped)
///     nautilus       1.00 0.98 0.82 0.53        0.91 0.78 0.58 0.36
///     censer         0.99 1.00 0.95 0.72        0.89 0.79 0.67 0.48
///     jellyfish      0.97 1.02 1.06 0.88        0.88 0.81 0.76 0.60
///     glasshouse     1.12 1.32 1.57 1.57        0.97 0.97 1.07 1.11
///     galaxy         1.09 1.29 1.56 1.66        0.96 0.97 1.08 1.18
///     fern_3d        1.26 1.66 2.20 2.42        1.07 1.21 1.54 1.77
///     ammonite       1.23 1.58 2.09 2.33        1.05 1.16 1.44 1.69
/// ```
///
/// 1.2 is the value that holds nautilus and censer flat (the gain that does,
/// found by bisection per opacity, follows `cσ/(1 − e^{−cσ})` there out to
/// opacity 5 within ~10%), and jellyfish with them; it over-brightens the
/// diffuse, volume-filling scenes by 1.3-2.4x because they absorb less per unit
/// of splat light. 0.4 is the value that minimises the RMS log deviation over
/// all seven scenes at opacity 1, 3 and 10 (0.23, against 0.33 for 1.2), and
/// trades a dimmer filament scene for a milder over-bright diffuse one. The
/// constant is a function of opacity alone, never of the image.
///
/// Past opacity ~10 the gain that would restore a filament scene runs away (a
/// pixel's front sliver is dimmed by the reconstruction's softness in front, and
/// no constant undoes that): see `VOLUME-RENDERER-PLAN.md`.
pub const GAIN_C: f32 = 0.4;

/// Gaussian sigma of the blur on the absorbance field, in *output* pixels.
///
/// Per-point absorbance is `opacity · H²/N` of optical depth and stops being
/// small at high opacity: one point alone can be several units thick, so
/// occlusion becomes a per-sample coin toss — the points renderer's grain
/// returning, and brightness that depends on `--points`. Moments are linear, so
/// blurring them is the same as splatting fatter absorption kernels; emission
/// stays sharp. 1 output pixel took the grit's seed-to-seed noise at opacity 30
/// from 0.044 to 0.028 (splat's own is 0.016), and made the picture converge as
/// points are added; more blur bought little and softened more.
pub const BLUR_OUTPUT_PX: f32 = 1.0;

/// The constant by which every point's emission is multiplied at opacity
/// `sigma`: `g = c·σ / (1 − e^{−c·σ})`.
///
/// Without it a saturated pixel carries about `1/σ` of splat's light, since
/// emission per point is fixed while absorbance scales with σ. This is a
/// **constant for the frame, not a per-pixel normalisation**, and that is the
/// point: a per-pixel `τ/(1 − e^{−τ})` would erase the darker lanes between
/// sheets, which are most of the depth cue. And it is a constant of `σ` alone,
/// never derived from the image — exposure stays scene-independent (AGENTS.md,
/// "Settling decision 5"). It is exactly 1 at σ = 0 and grows linearly at high σ.
pub fn emission_gain(sigma: f32) -> f32 {
    let x = (GAIN_C * sigma) as f64;
    if x < 1e-6 { 1.0 } else { (x / (1.0 - (-x).exp())) as f32 }
}

/// The view-space depth interval the moment targets resolve. Points outside it
/// clamp to its ends, which costs accuracy there and nothing else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthRange {
    pub near: f32,
    pub far: f32,
}

/// Never let `near` be closer than this fraction of `far`: the warp is
/// logarithmic, and a near plane at 1e-6 of far would spend most of the
/// moment-space range on empty space in front of the eye.
const MAX_DEPTH_RATIO: f32 = 100.0;

impl DepthRange {
    /// The interval holding a sphere (`center`, `radius`) seen from `eye`
    /// looking along `forward`, padded generously because the sphere is
    /// estimated from a few thousand CPU steps and the real cloud has tails.
    pub fn around(eye: Vec3, forward: Vec3, center: Vec3, radius: f32) -> Self {
        let d = (center - eye).dot(forward);
        let r = radius.max(1e-9) * PAD;
        let far = (d + r).max(1e-9);
        let near = (d - r).max(far / MAX_DEPTH_RATIO);
        Self { near, far }
    }

    /// Under infinite zoom there are no bounds to measure — the set reaches
    /// out forever and the edge guard fades it. The range is then relative to
    /// the eye, which is the only scale-invariant thing available: from just in
    /// front of the eye (a hundredth of its distance from the fixed point) out
    /// to the band's far side, with the band's radius capped at a few eye
    /// distances because material further out has been faded to nothing.
    pub fn for_zoom(eye: Vec3, forward: Vec3, fixed_point: Vec3, band_radius: f32) -> Self {
        let d_eye = (fixed_point - eye).length().max(1e-12);
        let d = (fixed_point - eye).dot(forward).max(d_eye * 0.01);
        let far = d + band_radius.min(8.0 * d_eye);
        let near = (d_eye * 0.01).max(far / (MAX_DEPTH_RATIO * 10.0));
        Self { near, far }
    }

    /// With nothing measured: the span an orbit camera at `distance` would
    /// frame, a few distances either side of the focus.
    pub fn for_distance(distance: f32) -> Self {
        let d = distance.max(1e-9);
        Self { near: d / 10.0, far: d * 4.0 }
    }

    /// `(scale, bias)` with `z = ln(depth) · scale + bias` mapping
    /// `[near, far]` onto `[-1, 1]`.
    pub fn warp(&self) -> (f32, f32) {
        let (ln_near, ln_far) = (self.near.max(1e-30).ln(), self.far.max(1e-30).ln());
        let span = (ln_far - ln_near).max(1e-6);
        let scale = 2.0 / span;
        (scale, -1.0 - ln_near * scale)
    }

    /// The warp, in plain Rust. The reference for `volume_z` in the shader.
    #[cfg(test)]
    pub fn warped(&self, depth: f32) -> f32 {
        let (s, b) = self.warp();
        (depth.ln() * s + b).clamp(-1.0, 1.0)
    }
}

/// Generous because the measure is a 4,000-step CPU walk and the 2.5–97.5
/// percentile box leaves the tails outside it.
const PAD: f32 = 1.5;

/// `(centre, radius)` of the sphere that should hold the cloud, from the same
/// CPU measure the drawn-points budget uses. The radius is the half-diagonal of
/// the percentile box when that is larger than the 95th-percentile radius, so a
/// flat or elongated attractor is not under-covered along its long axis.
pub fn bounding_sphere(stats: &AttractorStats) -> (Vec3, f32) {
    let half_diag = (stats.bounds[1] - stats.bounds[0]).length() * 0.5;
    let centre = (stats.bounds[0] + stats.bounds[1]) * 0.5;
    (centre, half_diag.max(stats.radius))
}

/// The scene's bounding sphere as the volume passes should frame it, for a
/// caller with no running app to ask (the offline renderer). A composite is
/// measured layer by layer, as `info.rs` does; a plain scene by the walk.
pub fn scene_bounds(scene: &crate::scene::Scene) -> Option<(Vec3, f32)> {
    let stats = if scene.is_composite() {
        crate::trace::measure_layers(scene).union
    } else {
        crate::trace::measure(&scene.transforms, &vec![true; scene.transforms.len()])
    }?;
    Some(bounding_sphere(&stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniform_matches_the_shader() {
        assert_eq!(std::mem::size_of::<VolumeParams>(), 32);
        let wgsl = include_str!("../../../shaders/points/splat.wgsl");
        let at = wgsl.find("struct VolumeParams").expect("splat.wgsl must declare VolumeParams");
        let body = &wgsl[at..at + wgsl[at..].find('}').unwrap()];
        let fields: Vec<&str> = body
            .lines()
            .filter(|l| !l.trim_start().starts_with("///") && l.contains(": f32"))
            .map(|l| l.trim().split(':').next().unwrap())
            .collect();
        assert_eq!(
            fields,
            ["enabled", "abs_scale", "warp_scale", "warp_bias", "emission_gain", "blur_px", "_pad0", "_pad1"]
        );
    }

    /// The gain is exactly 1 where opacity is 0 (so the splat identity holds in
    /// the limit as well as at the point), rises monotonically, and goes
    /// linearly in opacity once it is large.
    #[test]
    fn the_gain_is_one_at_zero_and_linear_at_the_top() {
        assert_eq!(emission_gain(0.0), 1.0);
        let mut prev = 1.0;
        for i in 1..200 {
            let g = emission_gain(i as f32 * 0.25);
            assert!(g > prev, "gain fell at {}", i);
            prev = g;
        }
        let (a, b) = (emission_gain(50.0), emission_gain(100.0));
        assert!((b / a - 2.0).abs() < 0.01, "{} {}", a, b);
    }

    /// The blur is a width in output pixels, so it follows supersampling.
    #[test]
    fn the_blur_scales_with_the_supersample_factor() {
        let range = DepthRange::for_distance(3.0);
        let one = VolumeParams::new(1.0, 1.0e6, 720.0, 1, range);
        let two = VolumeParams::new(1.0, 1.0e6, 720.0, 2, range);
        assert_eq!(two.blur_px, 2.0 * one.blur_px);
    }

    #[test]
    fn the_warp_maps_the_range_onto_minus_one_to_one() {
        let r = DepthRange { near: 0.5, far: 8.0 };
        assert!((r.warped(0.5) + 1.0).abs() < 1e-5);
        assert!((r.warped(8.0) - 1.0).abs() < 1e-5);
        // Logarithmic: the geometric mean is the middle.
        assert!(r.warped(2.0).abs() < 1e-5);
        // Outside clamps.
        assert_eq!(r.warped(0.01), -1.0);
        assert_eq!(r.warped(100.0), 1.0);
    }

    #[test]
    fn a_sphere_in_front_of_the_eye_is_framed_with_room() {
        let r = DepthRange::around(Vec3::new(0.0, 0.0, 3.0), Vec3::NEG_Z, Vec3::ZERO, 1.0);
        assert!(r.near < 2.0 && r.far > 4.0, "{r:?}");
        // The camera inside the cloud: near is floored, not negative.
        let inside = DepthRange::around(Vec3::ZERO, Vec3::NEG_Z, Vec3::ZERO, 1.0);
        assert!(inside.near > 0.0 && inside.far / inside.near <= MAX_DEPTH_RATIO * 1.001);
    }

    /// Opacity is a property of the scene: the deposit scale falls as 1/samples
    /// and rises as H², the way exposure's does, so doubling the points while
    /// halving each one's share cannot change what a pixel adds up to.
    #[test]
    fn absorbance_scales_like_exposure() {
        let range = DepthRange::for_distance(3.0);
        let a = VolumeParams::new(1.0, 1.0e6, 720.0, 1, range);
        let b = VolumeParams::new(1.0, 4.0e6, 720.0, 1, range);
        let c = VolumeParams::new(1.0, 1.0e6, 1440.0, 1, range);
        assert!((a.abs_scale / b.abs_scale - 4.0).abs() < 1e-4);
        assert!((c.abs_scale / a.abs_scale - 4.0).abs() < 1e-4);
        assert_eq!(VolumeParams::new(0.0, 1.0e6, 720.0, 1, range), VolumeParams::OFF);
    }
}
