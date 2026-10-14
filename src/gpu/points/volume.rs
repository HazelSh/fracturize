//! Volume mode's host-side numbers: the uniform the shaders read, the depth
//! range the moments are warped into, and the opacity control's limits.
//!
//! The model is in `VOLUME-RENDERER-PLAN.md`; the short version is that splat
//! is additive emission, and volume mode adds absorption so near material hides
//! what is behind it. Two passes — absorbance moments, then emission weighted
//! by the transmittance those moments give (see `moments.rs`) — over the same
//! point buffer. Opacity 0 is today's splat, byte for byte: neither pass runs.

use glam::Vec3;

use crate::camera::Fov;
use crate::trace::AttractorStats;

/// Optical depth along a ray through a *reference-sized* column of material at
/// opacity 1, before calibration against the old screen-space normalisation.
///
/// **Absorbance is a world quantity.** A point carries a cross-section
/// `σ · K · L² / N` in world area, where `L` is a reference length of the scene
/// (the attractor's radius; under infinite zoom the eye's distance from the
/// fixed point, which is what keeps it invariant across a wrap) and `N` the
/// point count. What it deposits in a pixel is that area over the pixel's
/// world area at the point's depth, `(H / (2·z·tan(fov/2)))²` — so a ray's
/// optical depth is a property of the material and not of where the camera
/// stands. The first version normalised in *screen* space (`σ · H² / N`, the
/// way exposure is), which is right for a brightness and wrong for an
/// absorbance: zoom out k times and the attractor covers k² fewer pixels, so
/// per-pixel optical depth rose by k².
///
/// `K` is a calibration, set so that opacity keeps roughly the meaning it had in
/// the screen-space version at the scenes' authored framings. Median `b0` over
/// lit pixels at opacity 2 and K = 1, against the old normalisation, on seven
/// scenes: 1.02 vs 2.86 (nautilus), 0.94 vs 5.1 (censer), 0.71 vs 6.0
/// (jellyfish), 0.08 vs 0.26 (glasshouse), 0.12 vs 0.15 (galaxy), 0.99 vs 0.24
/// (fern_3d), 0.24 vs 0.16 (ammonite): the ratio spans 0.24 to 8.5 because the
/// old number depended on how big each scene happened to be framed, which is the
/// defect. Their geometric mean is about 2, so K = 2.
pub const ABSORBANCE_K: f32 = 2.0;

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
    /// `opacity · ABSORBANCE_K · L² / samples`: a point's cross-section, in
    /// world area. The vertex stage divides it by the pixel's world area.
    pub abs_scale: f32,
    /// `z = ln(depth) · warp_scale + warp_bias`
    pub warp_scale: f32,
    pub warp_bias: f32,
    /// Gaussian sigma, in accumulation pixels, of the blur applied to the
    /// moments before pass B. 0 is off.
    pub blur_px: f32,
    /// Pixels per world unit at unit depth, `H / (2·tan(fov/2))`: the pixel's
    /// world area at depth `z` is `(z / ppu)²`.
    pub ppu: f32,
    /// Pads the uniform to 32 bytes.
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
        blur_px: 0.0,
        ppu: 0.0,
        _pad: [0.0; 2],
    };

    /// `supersample` is the factor the target is supersampled by (1 live), since
    /// the absorbance blur is a width in *output* pixels.
    ///
    /// `samples` and `screen_height` are exactly the two numbers
    /// `SplatRenderer::upload_params` normalises exposure with. The height
    /// still appears squared, through `ppu`, so a point's *screen* deposit is
    /// invariant to resolution and supersampling as before; what changed is
    /// that the depth now enters, and the reference length with it.
    pub fn new(
        opacity: f32,
        samples: f64,
        screen_height: f32,
        supersample: u32,
        fov: Fov,
        range: DepthRange,
    ) -> Self {
        if opacity <= 0.0 || samples <= 0.0 {
            return Self::OFF;
        }
        let ppu = screen_height / (2.0 * (fov.radians() * 0.5).tan());
        let l = range.length as f64;
        let abs_scale =
            (opacity as f64 * ABSORBANCE_K as f64 * l * l / samples) as f32;
        let (warp_scale, warp_bias) = range.warp();
        let blur_px = BLUR_OUTPUT_PX * supersample.max(1) as f32;
        Self {
            enabled: 1.0,
            abs_scale,
            warp_scale,
            warp_bias,
            blur_px,
            ppu,
            _pad: [0.0; 2],
        }
    }
}

/// Share of its own mean colour that a pixel the absorbance has made optically
/// thick is filled with, in the tonemap. See the comment at `OPAQUE_FILL` in
/// `splat.wgsl` for the two failures either side of it; a test holds the two
/// copies together.
pub const OPAQUE_FILL: f32 = 0.3;

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


/// The view-space depth interval the moment targets resolve. Points outside it
/// clamp to its ends, which costs accuracy there and nothing else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthRange {
    pub near: f32,
    pub far: f32,
    /// The scene's reference length `L`, in world units, that opacity is
    /// measured against: the attractor's radius, or the eye's distance from the
    /// fixed point under infinite zoom. See [`ABSORBANCE_K`].
    pub length: f32,
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
        Self { near, far, length: radius.max(1e-9) }
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
        Self { near, far, length: d_eye }
    }

    /// With nothing measured: the span an orbit camera at `distance` would
    /// frame, a few distances either side of the focus.
    pub fn for_distance(distance: f32) -> Self {
        let d = distance.max(1e-9);
        Self { near: d / 10.0, far: d * 4.0, length: d * 0.25 }
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
    fn the_tonemap_fill_matches_the_shader() {
        let wgsl = include_str!("../../../shaders/points/splat.wgsl");
        let key = "const OPAQUE_FILL: f32 =";
        let at = wgsl.find(key).expect("splat.wgsl must declare OPAQUE_FILL");
        let rest = &wgsl[at + key.len()..];
        let value: f32 = rest[..rest.find(';').unwrap()].trim().parse().unwrap();
        assert_eq!(value, OPAQUE_FILL);
    }

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
            ["enabled", "abs_scale", "warp_scale", "warp_bias", "blur_px", "ppu", "_pad0", "_pad1"]
        );
    }

    /// The blur is a width in output pixels, so it follows supersampling.
    #[test]
    fn the_blur_scales_with_the_supersample_factor() {
        let range = DepthRange::for_distance(3.0);
        let one = VolumeParams::new(1.0, 1.0e6, 720.0, 1, Fov::default(), range);
        let two = VolumeParams::new(1.0, 1.0e6, 720.0, 2, Fov::default(), range);
        assert_eq!(two.blur_px, 2.0 * one.blur_px);
    }

    #[test]
    fn the_warp_maps_the_range_onto_minus_one_to_one() {
        let r = DepthRange { near: 0.5, far: 8.0, length: 1.0 };
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

    /// A point's deposit in a pixel is its world cross-section over the pixel's
    /// world area, so it falls as 1/samples and rises as H² (through `ppu`)
    /// exactly as exposure's density does: doubling the points while halving
    /// each one's share cannot change what a pixel adds up to.
    #[test]
    fn absorbance_scales_like_exposure() {
        let fov = Fov::default();
        let range = DepthRange::for_distance(3.0);
        let deposit = |samples: f64, height: f32| {
            let p = VolumeParams::new(1.0, samples, height, 1, fov, range);
            // At unit depth: cross-section times (ppu / z)².
            p.abs_scale as f64 * (p.ppu as f64).powi(2)
        };
        assert!((deposit(1.0e6, 720.0) / deposit(4.0e6, 720.0) - 4.0).abs() < 1e-4);
        assert!((deposit(1.0e6, 1440.0) / deposit(1.0e6, 720.0) - 4.0).abs() < 1e-4);
        assert_eq!(VolumeParams::new(0.0, 1.0e6, 720.0, 1, fov, range), VolumeParams::OFF);
    }

    /// The point of the world-space normalisation: what a pixel holds does not
    /// depend on where the camera stands. Zoom out k times (every depth and the
    /// reference length scale with the scene, not the camera — here the camera
    /// recedes while the length stays) and the per-pixel deposit falls as 1/k²,
    /// while the number of points per pixel falls as 1/k² too, so a ray's
    /// optical depth, the product, is unchanged.
    #[test]
    fn a_rays_optical_depth_does_not_depend_on_camera_distance() {
        let fov = Fov::default();
        let near = DepthRange { near: 1.0, far: 9.0, length: 1.0 };
        let p = VolumeParams::new(2.0, 1.0e6, 1080.0, 1, fov, near);
        let per_point = |z: f64| p.abs_scale as f64 * (p.ppu as f64 / z).powi(2);
        // Points per pixel at depth z: the attractor (area ~ L²) is spread
        // over N points, a pixel's world area is (z/ppu)².
        let points_per_pixel = |z: f64| 1.0e6 * (z / p.ppu as f64).powi(2) / 1.0;
        let tau = |z: f64| per_point(z) * points_per_pixel(z);
        for k in [0.5, 2.0, 4.0, 8.0] {
            assert!((tau(3.0 * k) / tau(3.0) - 1.0).abs() < 1e-9, "k = {}", k);
        }
    }
}
