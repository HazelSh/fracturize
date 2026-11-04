//! How the selected map warps space: a lattice pushed through it.
//!
//! Apophysis's transform preview draws a grid and the selected transform
//! applied to it, and at small variation amounts you can watch the grid start
//! to bend. This is that, in 3D.
//!
//! **The lattice is the map's own frame**: the cube from −1 to +1 on each of
//! its input axes. Through the plain affine that cube is centred on the
//! gizmo's origin dot, its middle lines run along the gizmo's axes and its
//! faces pass through the tips, so a map with no variations draws a cage on
//! its own gizmo and the warp is how far the cage has left it. It used to
//! fill the box of the layer's attractor instead, which wandered away from
//! the gizmo and changed size with the picture (Hazel, 2026-10-31 and
//! 2026-11-04: "can travel far from the transform's gizmo").
//!
//! Each line is sampled finely and taken through the whole map — affine,
//! variation blend, post-affine — by [`trace::apply_variations`], the CPU twin
//! of the shader's, so what is drawn is what the walkers do. The symmetry
//! group is left out: it acts after the map, rigidly, and its copies are
//! already drawn as the orbit ghosts. There is no second, unwarped grid: the
//! gizmo is the reference.
//!
//! Lines are one light grey, opaque, and the line shader fades them toward
//! mid-grey (mixed in display space, so evenly to the eye) with depth across the grid's own front-to-back extent
//! (`LineRenderer::upload_fade`), so the fade always spans near to far
//! whatever the camera distance. Where a variation tears space (spherical's
//! pole, a non-finite value, a jump far longer than its neighbours) a line is
//! broken rather than drawn as a streak across the screen.

use glam::{Mat4, Vec3};
use rand::RngCore;

use crate::gpu::lines::LineVertex;
use crate::scene::{TransformSpec, NUM_VARIATIONS};

/// Lattice lines per axis, across −1..1: the origin, the tips, and three
/// between each.
const LEVELS: usize = 9;
/// Samples along each line. Fine enough that a swirl reads as a curve.
const SAMPLES: usize = 48;
/// The near colour, linear: nearly white (sRGB ≈ 0.96), so the fade toward
/// [`FADE_GREY`] has the whole range above mid-grey to cover.
pub const NEAR_GREY: [f32; 4] = [0.91, 0.91, 0.91, 1.0];
/// What the far side of the grid fades to, linear: sRGB mid-grey.
pub const FADE_GREY: [f32; 3] = [0.214, 0.214, 0.214];
/// Slot 10, julia: the one variation that draws a random branch.
const JULIA: usize = 10;

/// An RNG that always gives the same bits, so julia takes one branch for a
/// whole lattice instead of a different one per vertex.
struct Constant(u32);

impl RngCore for Constant {
    fn next_u32(&mut self) -> u32 {
        self.0
    }
    fn next_u64(&mut self) -> u64 {
        ((self.0 as u64) << 32) | self.0 as u64
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        dest.fill(self.0 as u8);
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

/// The lattice lines in the map's input frame: each a run of points along one
/// axis of the cube −1..1.
fn lattice() -> Vec<Vec<Vec3>> {
    let at = |i: usize, n: usize| -1.0 + 2.0 * i as f32 / n as f32;
    let mut lines = Vec::new();
    for a in 0..3 {
        let (b, c) = ((a + 1) % 3, (a + 2) % 3);
        for i in 0..LEVELS {
            for j in 0..LEVELS {
                let line = (0..=SAMPLES)
                    .map(|s| {
                        let mut p = Vec3::ZERO;
                        p[a] = at(s, SAMPLES);
                        p[b] = at(i, LEVELS - 1);
                        p[c] = at(j, LEVELS - 1);
                        p
                    })
                    .collect();
                lines.push(line);
            }
        }
    }
    lines
}

/// Push one polyline's images as segments, breaking it where space tears.
fn push_polyline(verts: &mut Vec<LineVertex>, points: &[Vec3], far: f32, tear: f32) {
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if !a.is_finite() || !b.is_finite() || a.length() > far || b.length() > far {
            continue;
        }
        if a.distance(b) > tear {
            continue;
        }
        verts.push(LineVertex { position: a.to_array(), color: NEAR_GREY });
        verts.push(LineVertex { position: b.to_array(), color: NEAR_GREY });
    }
}

/// The drawn lattice, and the box it occupies in scene space: what the depth
/// fade is spread across.
pub struct Warp {
    pub verts: Vec<LineVertex>,
    pub extent: Option<[Vec3; 2]>,
}

/// The selected map's warp of its own frame, drawn in scene space through
/// `placement`.
pub fn build(spec: &TransformSpec, placement: Mat4) -> Warp {
    let affine = |p: Vec3| placement.transform_point3(spec.post_affine.transform_point3(spec.matrix.transform_point3(p)));
    let warp = |p: Vec3, rng: &mut Constant| {
        let v = crate::trace::apply_variations(&spec.variations, spec.matrix.transform_point3(p), rng);
        placement.transform_point3(spec.post_affine.transform_point3(v))
    };

    // The plain image's size sets what counts as torn: far past it is a
    // variation throwing points away, and a jump many times a plain segment's
    // length is a seam, not a curve.
    let corners: Vec<Vec3> = (0..8)
        .map(|i| {
            let pick = |k: usize| if i >> k & 1 == 1 { 1.0 } else { -1.0 };
            affine(Vec3::new(pick(0), pick(1), pick(2)))
        })
        .collect();
    let centre = corners.iter().copied().sum::<Vec3>() / 8.0;
    let reach = corners.iter().map(|c| c.distance(centre)).fold(0.0, f32::max).max(1e-4);
    let far = centre.length() + 20.0 * reach;
    let tear = 0.25 * reach;

    let lines = lattice();
    let mut verts = Vec::new();
    let branches: &[u32] = if spec.variations[JULIA] != 0.0 { &[0, u32::MAX] } else { &[0] };
    for &bits in branches {
        let mut rng = Constant(bits);
        for line in &lines {
            let image: Vec<Vec3> = line.iter().map(|&p| warp(p, &mut rng)).collect();
            push_polyline(&mut verts, &image, far, tear);
        }
    }
    let extent = verts.iter().map(|v| Vec3::from(v.position)).fold(None, |acc: Option<[Vec3; 2]>, p| {
        Some(acc.map_or([p, p], |[lo, hi]| [lo.min(p), hi.max(p)]))
    });
    Warp { verts, extent }
}

/// Where the depth fade starts and ends for an eye looking along `forward`:
/// the nearest and farthest of the grid's box corners, measured along the
/// view direction.
pub fn depth_span(extent: [Vec3; 2], eye: Vec3, forward: Vec3) -> (f32, f32) {
    let [lo, hi] = extent;
    (0..8)
        .map(|i| {
            let pick = |k: usize, a: usize| if i >> k & 1 == 1 { hi[a] } else { lo[a] };
            (Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2)) - eye).dot(forward)
        })
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(n, f), d| (n.min(d), f.max(d)))
}

/// What [`build`] depends on, folded into one number, so the lines are rebuilt
/// only when they would differ.
pub fn fingerprint(spec: &TransformSpec, placement: Mat4) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |x: f32| {
        h = (h ^ x.to_bits() as u64).wrapping_mul(0x100000001b3);
    };
    for m in [spec.matrix, spec.post_affine, placement] {
        m.to_cols_array().into_iter().for_each(&mut mix);
    }
    spec.variations[..NUM_VARIATIONS].iter().copied().for_each(&mut mix);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> TransformSpec {
        let mut spec = crate::scene::Scene::blank().transforms[0].clone();
        spec.variations = [0.0; NUM_VARIATIONS];
        spec.variations[0] = 1.0;
        spec
    }

    #[test]
    fn a_plain_map_draws_a_cage_on_its_own_gizmo() {
        // Through the plain affine, the cube's centre is the map's origin and
        // its corners are origin ± each axis: the gizmo's tips are on it.
        let spec = plain();
        let w = build(&spec, Mat4::IDENTITY);
        let origin = spec.matrix.transform_point3(Vec3::ZERO);
        let tip = spec.matrix.transform_point3(Vec3::X);
        let has = |q: Vec3| w.verts.iter().any(|v| Vec3::from(v.position).distance(q) < 1e-4);
        assert!(has(origin), "no line through the origin dot");
        assert!(has(tip), "no line through the x tip");
        let [lo, hi] = w.extent.unwrap();
        assert!(((lo + hi) * 0.5).distance(origin) < 1e-4, "the cage isn't centred on the gizmo");
    }

    #[test]
    fn the_cage_follows_the_map() {
        let mut spec = plain();
        let a = build(&spec, Mat4::IDENTITY).extent.unwrap();
        spec.matrix = Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0)) * spec.matrix;
        let b = build(&spec, Mat4::IDENTITY).extent.unwrap();
        assert!((b[0] - a[0] - Vec3::new(3.0, 0.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn a_variation_bends_the_lattice_off_its_affine_image() {
        let plain_verts = build(&plain(), Mat4::IDENTITY).verts;
        let mut spec = plain();
        spec.variations[0] = 0.5;
        spec.variations[3] = 0.5; // swirl
        let bent = build(&spec, Mat4::IDENTITY).verts;
        assert_eq!(plain_verts.len(), bent.len());
        let moved = plain_verts
            .iter()
            .zip(&bent)
            .filter(|(a, b)| Vec3::from(a.position).distance(Vec3::from(b.position)) > 1e-3)
            .count();
        assert!(moved > plain_verts.len() / 2, "only {moved} of {} vertices moved", plain_verts.len());
    }

    #[test]
    fn julia_draws_both_branches() {
        let mut spec = plain();
        spec.variations[0] = 0.0;
        spec.variations[JULIA] = 1.0;
        let one = build(&plain(), Mat4::IDENTITY).verts.len();
        let both = build(&spec, Mat4::IDENTITY).verts.len();
        assert!(both > one, "{both} vertices against {one}");
    }

    #[test]
    fn spherical_breaks_at_its_pole_instead_of_streaking() {
        let mut spec = plain();
        spec.variations[0] = 0.0;
        spec.variations[2] = 1.0; // spherical
        let w = build(&spec, Mat4::IDENTITY);
        assert!(w.verts.iter().all(|v| Vec3::from(v.position).is_finite()));
        let plain_reach = 3f32.sqrt() * 0.5; // the blank map's cube, centre to corner
        for pair in w.verts.chunks(2) {
            assert!(Vec3::from(pair[0].position).distance(Vec3::from(pair[1].position)) <= 0.25 * plain_reach + 1e-3);
        }
    }

    #[test]
    fn the_fade_spans_the_grid_front_to_back() {
        let extent = [Vec3::splat(-1.0), Vec3::splat(1.0)];
        let (near, far) = depth_span(extent, Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z);
        assert!((near - 4.0).abs() < 1e-5 && (far - 6.0).abs() < 1e-5, "{near} {far}");
    }
}
