//! CPU chaos-game traces: watch individual walkers move through the IFS
//!
//! A trace starts at a random point, burns in until it has converged onto
//! the attractor, then records every subsequent position. Rendered as line
//! segments, traces show the *dynamics* the point cloud only implies — which
//! transform pulls where, how content circulates between the maps.
//!
//! This is a faithful CPU port of shaders/points/chaos.wgsl: the affine
//! matrix, the 16-variation blend (slot order = scene::VARIATION_NAMES),
//! divergence re-seeding, and the color EMA all match the GPU walker.

use glam::Vec3;
use rand::Rng;

use crate::scene::{TransformSpec, NUM_VARIATIONS};

const PI: f32 = std::f32::consts::PI;

/// CPU port of apply_variations in chaos.wgsl (must stay in sync)
pub fn apply_variations(
    weights: &[f32; NUM_VARIATIONS],
    p: Vec3,
    rng: &mut impl Rng,
) -> Vec3 {
    let r2 = p.dot(p);
    let r = r2.sqrt();
    let theta = p.y.atan2(p.x);

    let mut out = Vec3::ZERO;

    // 0: linear
    let w = weights[0];
    if w != 0.0 {
        out += w * p;
    }
    // 1: sinusoidal
    let w = weights[1];
    if w != 0.0 {
        out += w * Vec3::new(p.x.sin(), p.y.sin(), p.z.sin());
    }
    // 2: spherical (3D inversion)
    let w = weights[2];
    if w != 0.0 {
        out += w * p / r2.max(1e-9);
    }
    // 3: swirl (rotate xy by r^2, z through)
    let w = weights[3];
    if w != 0.0 {
        let (sr, cr) = r2.sin_cos();
        out += w * Vec3::new(p.x * sr - p.y * cr, p.x * cr + p.y * sr, p.z);
    }
    // 4: horseshoe
    let w = weights[4];
    if w != 0.0 {
        let inv_r = 1.0 / r.max(1e-6);
        out += w * Vec3::new(inv_r * (p.x - p.y) * (p.x + p.y), inv_r * 2.0 * p.x * p.y, p.z);
    }
    // 5: polar
    let w = weights[5];
    if w != 0.0 {
        out += w * Vec3::new(theta / PI, r - 1.0, p.z);
    }
    // 6: disc
    let w = weights[6];
    if w != 0.0 {
        let f = theta / PI;
        out += w * Vec3::new(f * (PI * r).sin(), f * (PI * r).cos(), p.z);
    }
    // 7: spiral
    let w = weights[7];
    if w != 0.0 {
        let inv_r = 1.0 / r.max(1e-6);
        out += w * Vec3::new(inv_r * (theta.cos() + r.sin()), inv_r * (theta.sin() - r.cos()), p.z);
    }
    // 8: hyperbolic
    let w = weights[8];
    if w != 0.0 {
        out += w * Vec3::new(theta.sin() / r.max(1e-6), r * theta.cos(), p.z);
    }
    // 9: diamond
    let w = weights[9];
    if w != 0.0 {
        out += w * Vec3::new(theta.sin() * r.cos(), theta.cos() * r.sin(), p.z);
    }
    // 10: julia (half-angle with random branch)
    let w = weights[10];
    if w != 0.0 {
        let omega = if rng.r#gen::<bool>() { PI } else { 0.0 };
        let a = theta * 0.5 + omega;
        let sr = r.max(0.0).sqrt();
        out += w * Vec3::new(sr * a.cos(), sr * a.sin(), p.z);
    }
    // 11: bent
    let w = weights[11];
    if w != 0.0 {
        let mut b = p;
        if b.x < 0.0 {
            b.x *= 2.0;
        }
        if b.y < 0.0 {
            b.y *= 0.5;
        }
        out += w * b;
    }
    // 12: fisheye (eyefish, 3D)
    let w = weights[12];
    if w != 0.0 {
        out += w * (2.0 / (r + 1.0)) * p;
    }
    // 13: bubble (3D)
    let w = weights[13];
    if w != 0.0 {
        out += w * (4.0 / (r2 + 4.0)) * p;
    }
    // 14: cylinder
    let w = weights[14];
    if w != 0.0 {
        out += w * Vec3::new(p.x.sin(), p.y, p.z);
    }
    // 15: tangent
    let w = weights[15];
    if w != 0.0 {
        let cy = p.y.cos();
        out += w * Vec3::new(
            p.x.sin() / cy.abs().max(1e-3) * cy.signum(),
            p.y.tan(),
            p.z,
        );
    }
    // 16: absfold — KIFS kaleidoscope fold
    let w = weights[16];
    if w != 0.0 {
        out += w * p.abs();
    }
    // 17: boxfold — Mandelbox fold
    let w = weights[17];
    if w != 0.0 {
        out += w * (2.0 * p.clamp(Vec3::NEG_ONE, Vec3::ONE) - p);
    }
    // 18: spherefold — Mandelbox sphere fold (minR2 = 0.25, fixR2 = 1)
    let w = weights[18];
    if w != 0.0 {
        let f = if r2 < 0.25 {
            4.0
        } else if r2 < 1.0 {
            1.0 / r2
        } else {
            1.0
        };
        out += w * f * p;
    }
    // 19: bulb — power-8 mandelbulb angle map, radius-preserving
    let w = weights[19];
    if w != 0.0 {
        let rr = r.max(1e-9);
        let tb = 8.0 * (p.z / rr).clamp(-1.0, 1.0).acos();
        let pb = 8.0 * p.y.atan2(p.x);
        out += w * r * Vec3::new(tb.sin() * pb.cos(), tb.sin() * pb.sin(), tb.cos());
    }

    out
}

/// One recorded chaos-game step
#[derive(Clone, Copy, Debug)]
pub struct TraceStep {
    pub pos: Vec3,
    /// Colormap position (0-1) of the walker at this step
    pub color_val: f32,
}

/// A CPU chaos-game walker matching the GPU implementation
pub struct Walker<'a> {
    transforms: &'a [TransformSpec],
    /// Cumulative selection weights (0 for disabled transforms)
    cumulative: Vec<f32>,
    pub pos: Vec3,
    pub color_val: f32,
}

impl<'a> Walker<'a> {
    /// Returns None when no transform is enabled/weighted
    pub fn new(
        transforms: &'a [TransformSpec],
        enabled: &[bool],
        rng: &mut impl Rng,
    ) -> Option<Self> {
        let total: f32 = transforms
            .iter()
            .enumerate()
            .map(|(i, t)| if enabled.get(i).copied().unwrap_or(true) { t.weight } else { 0.0 })
            .sum();
        if total <= 0.0 {
            return None;
        }
        let mut cumulative = Vec::with_capacity(transforms.len());
        let mut acc = 0.0;
        for (i, t) in transforms.iter().enumerate() {
            if enabled.get(i).copied().unwrap_or(true) {
                acc += t.weight / total;
            }
            cumulative.push(acc);
        }
        Some(Self {
            transforms,
            cumulative,
            pos: Vec3::new(
                rng.gen_range(-1.0..1.0),
                rng.gen_range(-1.0..1.0),
                rng.gen_range(-1.0..1.0),
            ),
            color_val: 0.5,
        })
    }

    /// One chaos-game step; returns the index of the transform applied
    pub fn step(&mut self, rng: &mut impl Rng) -> usize {
        let r: f32 = rng.r#gen();
        let idx = self
            .cumulative
            .iter()
            .position(|&c| r < c)
            .unwrap_or(self.transforms.len() - 1);
        let t = &self.transforms[idx];

        let affine = t.matrix.transform_point3(self.pos);
        self.pos = apply_variations(&t.variations, affine, rng);

        // Re-seed diverged/NaN walkers, like the shader
        if !(self.pos.dot(self.pos) < 1e12) {
            self.pos = Vec3::new(
                rng.gen_range(-1.0..1.0),
                rng.gen_range(-1.0..1.0),
                rng.gen_range(-1.0..1.0),
            );
        }

        self.color_val = self.color_val * (1.0 - t.color_speed) + t.color_value * t.color_speed;
        idx
    }
}

/// Number of steps a walker runs before its path is considered "on the
/// attractor" (contractive maps converge geometrically; 20 is plenty)
pub const BURN_IN_STEPS: usize = 20;

/// Generate `count` traces of `steps` recorded steps each
pub fn generate_traces(
    transforms: &[TransformSpec],
    enabled: &[bool],
    count: usize,
    steps: usize,
    rng: &mut impl Rng,
) -> Vec<Vec<TraceStep>> {
    let mut traces = Vec::with_capacity(count);
    for _ in 0..count {
        let Some(mut walker) = Walker::new(transforms, enabled, rng) else {
            return traces;
        };
        for _ in 0..BURN_IN_STEPS {
            walker.step(rng);
        }
        let mut trace = Vec::with_capacity(steps + 1);
        trace.push(TraceStep { pos: walker.pos, color_val: walker.color_val });
        for _ in 0..steps {
            walker.step(rng);
            trace.push(TraceStep { pos: walker.pos, color_val: walker.color_val });
        }
        traces.push(trace);
    }
    traces
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Mat4, Quat};
    use rand::SeedableRng;
    use rand_xoshiro::Xoshiro256PlusPlus;

    fn sierpinski_transforms() -> Vec<TransformSpec> {
        [
            Vec3::new(0.0, 0.0, 0.5),
            Vec3::new(0.0, 0.47, -0.17),
            Vec3::new(-0.41, -0.24, -0.17),
            Vec3::new(0.41, -0.24, -0.17),
        ]
        .iter()
        .map(|&t| TransformSpec {
            matrix: Mat4::from_scale_rotation_translation(Vec3::splat(0.5), Quat::IDENTITY, t),
            color_value: 0.5,
            weight: 1.0,
            color_speed: 0.5,
            explicit_color_speed: None,
            variations: TransformSpec::linear_variations(),
        })
        .collect()
    }

    #[test]
    fn pure_linear_variation_is_identity() {
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(1);
        let w = TransformSpec::linear_variations();
        let p = Vec3::new(0.3, -0.7, 0.2);
        assert!((apply_variations(&w, p, &mut rng) - p).length() < 1e-7);
    }

    #[test]
    fn spherical_is_inversion() {
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(1);
        let mut w = [0.0; NUM_VARIATIONS];
        w[2] = 1.0;
        let p = Vec3::new(2.0, 0.0, 0.0);
        // p / r^2 = (2,0,0)/4 = (0.5,0,0)
        assert!((apply_variations(&w, p, &mut rng) - Vec3::new(0.5, 0.0, 0.0)).length() < 1e-6);
    }

    #[test]
    fn traces_converge_to_attractor_bounds() {
        // The sierpinski attractor lives inside the tetrahedron of the four
        // fixed points; every recorded step must be within its loose bounds
        let transforms = sierpinski_transforms();
        let enabled = vec![true; 4];
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(99);
        let traces = generate_traces(&transforms, &enabled, 16, 50, &mut rng);
        assert_eq!(traces.len(), 16);
        for trace in &traces {
            assert_eq!(trace.len(), 51);
            for step in trace {
                assert!(step.pos.length() < 1.5, "escaped attractor: {:?}", step.pos);
                assert!((0.0..=1.0).contains(&step.color_val));
            }
        }
    }

    #[test]
    fn disabled_transforms_are_never_selected() {
        // Only T0 enabled: the walker must land on T0's fixed point
        // t = 0.5*t + (0,0,0.5) -> fixed point (0,0,1)
        let transforms = sierpinski_transforms();
        let enabled = vec![true, false, false, false];
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(5);
        let mut walker = Walker::new(&transforms, &enabled, &mut rng).unwrap();
        for _ in 0..60 {
            let idx = walker.step(&mut rng);
            assert_eq!(idx, 0);
        }
        assert!((walker.pos - Vec3::new(0.0, 0.0, 1.0)).length() < 1e-4);
    }
}
