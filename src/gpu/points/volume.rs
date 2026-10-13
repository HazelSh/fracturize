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

/// The uniform both volume passes read. 16 bytes, must match `VolumeParams` in
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
}

impl VolumeParams {
    /// Everything off. What a plain splat leaves in the buffer, and what the
    /// vertex stage reads as "skip the volume work".
    pub const OFF: VolumeParams =
        VolumeParams { enabled: 0.0, abs_scale: 0.0, warp_scale: 0.0, warp_bias: 0.0 };

    /// `samples` and `screen_height` are exactly the two numbers
    /// `SplatRenderer::upload_params` normalises exposure with — the *same
    /// values*, because the whole invariance claim is that they cancel against
    /// the same density in the same way.
    pub fn new(opacity: f32, samples: f64, screen_height: f32, range: DepthRange) -> Self {
        if opacity <= 0.0 || samples <= 0.0 {
            return Self::OFF;
        }
        let abs_scale = (opacity as f64 * ABSORBANCE_K as f64 * screen_height as f64
            * screen_height as f64
            / samples) as f32;
        let (warp_scale, warp_bias) = range.warp();
        Self { enabled: 1.0, abs_scale, warp_scale, warp_bias }
    }
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
        assert_eq!(std::mem::size_of::<VolumeParams>(), 16);
        let wgsl = include_str!("../../../shaders/points/splat.wgsl");
        let at = wgsl.find("struct VolumeParams").expect("splat.wgsl must declare VolumeParams");
        let body = &wgsl[at..at + wgsl[at..].find('}').unwrap()];
        let fields: Vec<&str> = body
            .lines()
            .filter(|l| !l.trim_start().starts_with("///") && l.contains(": f32"))
            .map(|l| l.trim().split(':').next().unwrap())
            .collect();
        assert_eq!(fields, ["enabled", "abs_scale", "warp_scale", "warp_bias"]);
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
        let a = VolumeParams::new(1.0, 1.0e6, 720.0, range);
        let b = VolumeParams::new(1.0, 4.0e6, 720.0, range);
        let c = VolumeParams::new(1.0, 1.0e6, 1440.0, range);
        assert!((a.abs_scale / b.abs_scale - 4.0).abs() < 1e-4);
        assert!((c.abs_scale / a.abs_scale - 4.0).abs() < 1e-4);
        assert_eq!(VolumeParams::new(0.0, 1.0e6, 720.0, range), VolumeParams::OFF);
    }
}
