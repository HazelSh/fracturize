//! How the selected map warps space: a lattice pushed through it.
//!
//! Apophysis's transform preview draws a grid and the selected transform
//! applied to it, and at small variation amounts you can watch the grid start
//! to bend away from a plain affine copy. This is that, in 3D.
//!
//! **The lattice fills the box the map's own layer's attractor occupies**,
//! because those are the points the map is actually applied to; space outside
//! the attractor is never fed to it, and a lattice there would show a warp
//! nothing in the picture undergoes. Each line is sampled finely and taken
//! through the whole map — affine, variation blend, post-affine — by
//! [`trace::apply_variations`], the CPU twin of the shader's, so what is drawn
//! is what the walkers do. The symmetry group is left out: it acts after the
//! map, rigidly, and its copies are already drawn as the orbit ghosts.
//!
//! Behind it, faint and grey, is the same lattice through the map with its
//! variation blend replaced by plain `linear = 1` — the map as a pure affine.
//! The gap between the two is the warp. A plain map draws one over the other.
//!
//! Lines are coloured by the input axis they run along, in the gizmo's axis
//! colours, so a line can be followed through a fold. Where a variation tears
//! space (spherical's pole, a non-finite value, a jump far longer than its
//! neighbours) the line is broken rather than drawn as a streak across the
//! screen.

use glam::{Mat4, Vec3};
use rand::RngCore;

use crate::gpu::lines::LineVertex;
use crate::scene::{TransformSpec, NUM_VARIATIONS};

/// Lattice lines per axis.
const LEVELS: usize = 5;
/// Samples along each line. Fine enough that a swirl reads as a curve.
const SAMPLES: usize = 48;
/// An axis under this share of the longest is drawn as one plane through its
/// middle: a flat attractor's map only ever sees that plane, and a stack of
/// nearly coincident ones is clutter.
const FLAT_SHARE: f32 = 0.1;
const WARPED_ALPHA: f32 = 0.85;
const REFERENCE: [f32; 4] = [0.62, 0.62, 0.62, 0.22];
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

/// The lattice lines: each a run of input points, and the axis it runs along.
fn lattice(bounds: [Vec3; 2]) -> Vec<(usize, Vec<Vec3>)> {
    let [lo, hi] = bounds;
    let size = hi - lo;
    let longest = size.max_element();
    if !(longest > 0.0) || !size.is_finite() {
        return Vec::new();
    }
    let levels = |a: usize| -> Vec<f32> {
        if size[a] < FLAT_SHARE * longest {
            vec![(lo[a] + hi[a]) * 0.5]
        } else {
            (0..LEVELS).map(|i| lo[a] + size[a] * i as f32 / (LEVELS - 1) as f32).collect()
        }
    };
    let mut lines = Vec::new();
    for a in 0..3 {
        if size[a] < FLAT_SHARE * longest {
            continue;
        }
        let (b, c) = ((a + 1) % 3, (a + 2) % 3);
        for &vb in &levels(b) {
            for &vc in &levels(c) {
                let line = (0..=SAMPLES)
                    .map(|s| {
                        let mut p = Vec3::ZERO;
                        p[a] = lo[a] + size[a] * s as f32 / SAMPLES as f32;
                        p[b] = vb;
                        p[c] = vc;
                        p
                    })
                    .collect();
                lines.push((a, line));
            }
        }
    }
    lines
}

/// Push one polyline's images as segments, breaking it where space tears.
/// `step` is how long a segment of the plain affine image is, which is what a
/// segment of the warped one is judged against.
fn push_polyline(verts: &mut Vec<LineVertex>, points: &[Vec3], color: [f32; 4], far: f32, tear: f32) {
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if !a.is_finite() || !b.is_finite() || a.length() > far || b.length() > far {
            continue;
        }
        if a.distance(b) > tear {
            continue;
        }
        verts.push(LineVertex { position: a.to_array(), color });
        verts.push(LineVertex { position: b.to_array(), color });
    }
}

/// The selected map's warp of the box `bounds` (in its layer's own space),
/// drawn in scene space through `placement`.
pub fn build(spec: &TransformSpec, placement: Mat4, bounds: [Vec3; 2]) -> Vec<LineVertex> {
    let lines = lattice(bounds);
    if lines.is_empty() {
        return Vec::new();
    }
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
            let pick = |k: usize, a: usize| if i >> k & 1 == 1 { bounds[1][a] } else { bounds[0][a] };
            affine(Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2)))
        })
        .collect();
    let centre = corners.iter().copied().sum::<Vec3>() / 8.0;
    let reach = corners.iter().map(|c| c.distance(centre)).fold(0.0, f32::max).max(1e-4);
    let far = centre.length() + 20.0 * reach;
    let tear = 0.5 * reach.max(1e-4);

    let mut verts = Vec::new();
    for (_, line) in &lines {
        let image: Vec<Vec3> = line.iter().map(|&p| affine(p)).collect();
        push_polyline(&mut verts, &image, REFERENCE, far, f32::INFINITY);
    }
    let branches: &[u32] = if spec.variations[JULIA] != 0.0 { &[0, u32::MAX] } else { &[0] };
    for &bits in branches {
        let mut rng = Constant(bits);
        for (axis, line) in &lines {
            let c = crate::palette::axes::axis_linear(*axis);
            let image: Vec<Vec3> = line.iter().map(|&p| warp(p, &mut rng)).collect();
            push_polyline(&mut verts, &image, [c.x, c.y, c.z, WARPED_ALPHA], far, tear);
        }
    }
    verts
}

/// What [`build`] depends on, folded into one number, so the lines are rebuilt
/// only when they would differ.
pub fn fingerprint(spec: &TransformSpec, placement: Mat4, bounds: [Vec3; 2]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |x: f32| {
        h = (h ^ x.to_bits() as u64).wrapping_mul(0x100000001b3);
    };
    for m in [spec.matrix, spec.post_affine, placement] {
        m.to_cols_array().into_iter().for_each(&mut mix);
    }
    spec.variations[..NUM_VARIATIONS].iter().copied().for_each(&mut mix);
    bounds.iter().flat_map(|v| v.to_array()).for_each(&mut mix);
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

    const BOX: [Vec3; 2] = [Vec3::splat(-1.0), Vec3::splat(1.0)];

    #[test]
    fn a_plain_map_draws_its_warp_over_its_affine_image() {
        let verts = build(&plain(), Mat4::IDENTITY, BOX);
        let (reference, warped): (Vec<&LineVertex>, Vec<&LineVertex>) = verts.iter().partition(|v| v.color == REFERENCE);
        assert_eq!(reference.len(), warped.len());
        for (r, w) in reference.iter().zip(&warped) {
            assert!(Vec3::from(r.position).distance(Vec3::from(w.position)) < 1e-5);
        }
    }

    #[test]
    fn a_variation_bends_the_lattice_off_its_affine_image() {
        let mut spec = plain();
        spec.variations[0] = 0.5;
        spec.variations[3] = 0.5; // swirl
        let verts = build(&spec, Mat4::IDENTITY, BOX);
        let half = verts.len() / 2;
        let moved = (0..half)
            .filter(|&i| Vec3::from(verts[i].position).distance(Vec3::from(verts[half + i].position)) > 1e-3)
            .count();
        assert!(moved > half / 2, "only {moved} of {half} vertices moved");
    }

    #[test]
    fn a_flat_box_is_one_plane() {
        let lines = lattice([Vec3::new(-1.0, -1.0, -0.01), Vec3::new(1.0, 1.0, 0.01)]);
        assert_eq!(lines.len(), 2 * LEVELS);
        assert!(lines.iter().all(|(_, l)| l.iter().all(|p| p.z == 0.0)));
    }

    #[test]
    fn julia_draws_both_branches() {
        let mut spec = plain();
        spec.variations[0] = 0.0;
        spec.variations[JULIA] = 1.0;
        let reference = build(&plain(), Mat4::IDENTITY, BOX).len() / 2;
        let warped = build(&spec, Mat4::IDENTITY, BOX).len() - reference;
        assert!(warped > reference, "{warped} warped vertices against {reference}");
    }

    #[test]
    fn spherical_breaks_at_its_pole_instead_of_streaking() {
        let mut spec = plain();
        spec.variations[0] = 0.0;
        spec.variations[2] = 1.0; // spherical
        let verts = build(&spec, Mat4::IDENTITY, BOX);
        for pair in verts.chunks(2).filter(|p| p[0].color != REFERENCE) {
            assert!(Vec3::from(pair[0].position).distance(Vec3::from(pair[1].position)) <= 0.5 * 3f32.sqrt() * 0.5 + 1e-3);
        }
    }
}
