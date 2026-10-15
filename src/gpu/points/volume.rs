//! Volume mode's host-side numbers: the uniform the shaders read, the reference
//! length absorbance is measured against, and the opacity control's limits.
//!
//! The model is in `VOLUME-RENDERER-PLAN.md`; the short version is that splat
//! is additive emission, and volume mode adds absorption so near material hides
//! what is behind it: each pixel's depth span, absorbance into slices of that
//! span, then emission weighted by the transmittance the slices give, over the
//! same point buffer. Opacity 0 is today's splat, byte for byte: none of it runs.

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

/// The uniform the volume passes read. 32 bytes, must match `VolumeParams` in
/// `shaders/points/splat.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VolumeParams {
    /// 1 when the volume passes are drawing, else 0
    pub enabled: f32,
    /// `opacity · ABSORBANCE_K · L² / samples`: a point's cross-section, in
    /// world area. The vertex stage divides it by the pixel's world area.
    pub abs_scale: f32,
    /// Pixels per world unit at unit depth, `H / (2·tan(fov/2))`: the pixel's
    /// world area at depth `z` is `(z / ppu)²`.
    pub ppu: f32,
    /// The source function in splat density units: `SOURCE · samples / (2·H²)`.
    pub source: f32,
    pub pad: [f32; 4],
}

impl VolumeParams {
    /// Everything off. What a plain splat leaves in the buffer, and what the
    /// vertex stage reads as "skip the volume work".
    pub const OFF: VolumeParams = VolumeParams { enabled: 0.0, abs_scale: 0.0, ppu: 0.0, source: 0.0, pad: [0.0; 4] };

    /// `samples` and `screen_height` are exactly the two numbers
    /// `SplatRenderer::upload_params` normalises exposure with. The height
    /// still appears squared, through `ppu`, so a point's *screen* deposit is
    /// invariant to resolution and supersampling; the depth and the reference
    /// length enter as well, which is what keeps optical depth from depending
    /// on where the camera stands.
    pub fn new(opacity: f32, samples: f64, screen_height: f32, fov: Fov, framing: Framing) -> Self {
        if opacity <= 0.0 || samples <= 0.0 {
            return Self::OFF;
        }
        let ppu = screen_height / (2.0 * (fov.radians() * 0.5).tan());
        let l = framing.length as f64;
        let abs_scale = (opacity as f64 * ABSORBANCE_K as f64 * l * l / samples) as f32;
        // The same N/H² that exposure is normalised by, so the glow of a thick
        // region does not change with point count or resolution either.
        let source = (SOURCE as f64 * samples / (2.0 * screen_height as f64 * screen_height as f64)) as f32;
        Self { enabled: 1.0, abs_scale, ppu, source, pad: [0.0; 4] }
    }
}

/// Points per footprint the coverage rule aims for (`scan` in bucket.wgsl). A
/// random sample leaves e^-k of the frame uncovered, so 3 is about 5% holes.
/// `FRACTURIZE_VOLUME_K` overrides it, for measuring.
pub const COVERAGE_K: f32 = 3.0;

/// The largest footprint radius, in output pixels. A cap and not a target: it is
/// only reached when almost none of the sample is on screen.
pub const MAX_RADIUS: f32 = 12.0;

fn env_f32(name: &str) -> Option<f32> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

/// What the sort needs to size the footprint, for a render at `supersample`.
pub fn footprint(supersample: u32) -> crate::gpu::points::bucket::Footprint {
    let n = supersample.max(1) as f32;
    crate::gpu::points::bucket::Footprint {
        k_cov: env_f32("FRACTURIZE_VOLUME_K").unwrap_or(COVERAGE_K),
        max_radius: MAX_RADIUS * n,
        // Output pixels, like everything the person sees.
        force_radius: env_f32("FRACTURIZE_VOLUME_RADIUS").map_or(0.0, |r| r * n),
    }
}

/// Slices of absorbance per pixel. Mirrored in `splat.wgsl`; a test holds them
/// together. 4 bytes a slice a pixel, one prefix pass over them, and an O(1)
/// read per fragment, so the cost is memory (128 bytes a pixel at 32: 118 MB for
/// a 720p window, 1.1 GB for 4K at supersample 2) and the housekeeping passes,
/// not the per-fragment work.
///
/// Measured against the exact reference on nautilus at opacity 2 (L1 of L, mean
/// |dT| per fragment) and the stress sheet crossing at opacity 10, with frame
/// time at 8M points live and the offline render stage at 20M points 1080p ss2:
///
/// ```text
///     slices   nautilus L1  dT      crossing L1   live FPS   offline
///     8        0.078        0.0145  1.23          16.6       0.43 s
///     16       0.038        0.0085  0.63          15.9       0.47 s
///     32       0.018        0.0051  0.26          14.7       0.56 s
/// ```
pub const BINS: u32 = 32;

/// What a thick region glows at, in units of exposure-1 splat density (`D' =
/// density · exposure_scale`): see `emission_weight` in the shader.
///
/// One constant, by eye, against splat and the exact reference's own pictures on
/// seven scenes (`renders/volume-1d/tm/sheet-source.png`): 0 is the bare
/// transport, which is right and dim (mean luminance 0.4-0.65 of splat's at
/// opacity 10, `fern_3d` nearly black); 1 keeps filament scenes at splat's
/// brightness through opacity 10 (nautilus 0.85 / 0.80 / 0.89 / 1.13 at opacity
/// 1 / 3 / 10 / 30) and lets thick regions of dim scenes glow rather than
/// vanish; 2 and up flatten everything toward one brightness.
pub const SOURCE: f32 = 1.0;

/// Absorbance is stored as u32 fixed point, this many units to 1.0. Mirrored
/// in `splat.wgsl`.
#[cfg(test)]
pub const FIXED: f32 = 1_048_576.0;

/// What opacity is measured against: the scene's reference length `L`.
///
/// Per-pixel slicing finds each pixel's own depth interval, so the only thing
/// the scene has to supply is this length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Framing {
    /// `L`, in world units: the attractor's radius, or the eye's distance from
    /// the fixed point under infinite zoom. See [`ABSORBANCE_K`].
    pub length: f32,
}

impl Framing {
    /// A scene whose measured bounding sphere has this radius.
    pub fn around(radius: f32) -> Self {
        Self { length: radius.max(1e-9) }
    }

    /// Under infinite zoom there are no bounds to measure — the set reaches out
    /// forever and the edge guard fades it — so the length is the eye's
    /// distance from the fixed point, the only scale-invariant thing available
    /// (a wrap scales it and the picture together).
    pub fn for_zoom(eye: Vec3, fixed_point: Vec3) -> Self {
        Self { length: (fixed_point - eye).length().max(1e-12) }
    }

    /// With nothing measured: a quarter of the orbit distance.
    pub fn for_distance(distance: f32) -> Self {
        Self { length: distance.max(1e-9) * 0.25 }
    }
}

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
    fn the_slice_constants_match_the_shader() {
        let wgsl = include_str!("../../../shaders/points/splat.wgsl");
        let grab = |key: &str| -> String {
            let at = wgsl.find(key).unwrap_or_else(|| panic!("splat.wgsl must declare {}", key));
            let rest = &wgsl[at + key.len()..];
            rest[..rest.find(';').unwrap()].trim().to_string()
        };
        assert_eq!(grab("const BINS: u32 ="), format!("{}u", BINS));
        assert_eq!(grab("const FIXED: f32 =").parse::<f32>().unwrap(), FIXED);
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
            ["enabled", "abs_scale", "ppu", "source", "pad_a", "pad0", "pad1", "pad2"]
        );
    }




    /// A point's deposit in a pixel is its world cross-section over the pixel's
    /// world area, so it falls as 1/samples and rises as H² (through `ppu`)
    /// exactly as exposure's density does: doubling the points while halving
    /// each one's share cannot change what a pixel adds up to.
    #[test]
    fn absorbance_scales_like_exposure() {
        let fov = Fov::default();
        let range = Framing::for_distance(3.0);
        let deposit = |samples: f64, height: f32| {
            let p = VolumeParams::new(1.0, samples, height, fov, range);
            // At unit depth: cross-section times (ppu / z)².
            p.abs_scale as f64 * (p.ppu as f64).powi(2)
        };
        assert!((deposit(1.0e6, 720.0) / deposit(4.0e6, 720.0) - 4.0).abs() < 1e-4);
        assert!((deposit(1.0e6, 1440.0) / deposit(1.0e6, 720.0) - 4.0).abs() < 1e-4);
        assert_eq!(VolumeParams::new(0.0, 1.0e6, 720.0, fov, range), VolumeParams::OFF);
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
        let near = Framing { length: 1.0 };
        let p = VolumeParams::new(2.0, 1.0e6, 1080.0, fov, near);
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
