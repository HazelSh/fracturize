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
        let varied = apply_variations(&t.variations, affine, rng);
        self.pos = t.post_affine.transform_point3(varied);

        // Then the symmetry group, drawn uniformly and composed on the outside
        // of the whole map — the CPU half of the same step chaos.wgsl takes.
        // It has to be here, not just on the GPU: `randomize.rs` gates rolls on
        // these walkers and `App::drawn_points` sizes the buffer from their
        // radius, so a CPU walk that skipped the group would measure a
        // completely different attractor from the one on screen.
        let mut target = t.color_value;
        if let Some(sym) = t.symmetry.as_ref() {
            let elements = sym.elements();
            let pick = rng.gen_range(0..elements.len());
            self.pos = elements[pick].transform_point3(self.pos);
            if sym.color() == crate::symmetry::OrbitColor::Orbit {
                target = (target + pick as f32 / elements.len() as f32).fract();
            }
        }

        // Re-seed diverged/NaN walkers, like the shader
        if !(self.pos.dot(self.pos) < 1e12) {
            self.pos = Vec3::new(
                rng.gen_range(-1.0..1.0),
                rng.gen_range(-1.0..1.0),
                rng.gen_range(-1.0..1.0),
            );
        }

        self.color_val = self.color_val * (1.0 - t.color_speed) + target * t.color_speed;
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

/// Steps sampled when measuring an attractor. Enough for a stable 95th
/// percentile without being something you'd notice on a scene edit.
pub const MEASURE_STEPS: usize = 4_000;

/// Where a chaos game lands and how it's shaped — measured on the CPU, from
/// the same walkers that draw the trace overlay.
///
/// Two callers want this and want it for different reasons, which is why it
/// lives here with the walkers rather than with either of them:
/// `randomize.rs` gates a rolled flame on it, and `App` uses the radius to
/// work out how many points are worth *drawing* (see `App::drawn_points`) —
/// an attractor that has collapsed to a speck doesn't get more legible for
/// having six million points stacked on the same pixel, it just gets slower.
#[derive(Clone, Copy, Debug)]
pub struct AttractorStats {
    /// Centroid of the visited points — the attractor rarely sits on the
    /// origin, and framing the camera there instead pushes it off to one
    /// side of the view.
    pub center: Vec3,
    /// 95th-percentile distance from the centroid. Deliberately *not* the
    /// maximum: chaos-game walkers throw occasional far outliers, and framing
    /// the camera on those leaves the actual form a speck in the middle of an
    /// empty frame.
    pub radius: f32,
    /// Per-axis standard deviation of the visited points
    pub spread: Vec3,
    /// Fraction of the bounding box's cells that contain any point
    pub occupancy: f32,
}

/// Run the CPU chaos game over the enabled transforms and measure where it
/// lands. `None` when no walker can be built (all weights zero, nothing
/// enabled) or the orbit goes non-finite.
///
/// Deterministic: a fixed seed, so the answer for a given set of transforms is
/// always the same. `randomize.rs` needs that — a candidate that passed the
/// quality gate must not fail it on a re-roll — and everything else benefits
/// from a measurement that doesn't shimmer.
pub fn measure(transforms: &[TransformSpec], enabled: &[bool]) -> Option<AttractorStats> {
    use rand::SeedableRng;
    let mut rng = rand::rngs::StdRng::from_seed([0x5c; 32]);
    let mut walker = Walker::new(transforms, enabled, &mut rng)?;

    for _ in 0..BURN_IN_STEPS {
        walker.step(&mut rng);
    }

    let mut points = Vec::with_capacity(MEASURE_STEPS);
    let mut sum = Vec3::ZERO;
    let mut sum_sq = Vec3::ZERO;
    for _ in 0..MEASURE_STEPS {
        walker.step(&mut rng);
        let p = walker.pos;
        if !p.is_finite() {
            return None;
        }
        sum += p;
        sum_sq += p * p;
        points.push(p);
    }

    let n = MEASURE_STEPS as f32;
    let mean = sum / n;
    let var = (sum_sq / n - mean * mean).max(Vec3::ZERO);

    let mut radii: Vec<f32> = points.iter().map(|p| (*p - mean).length()).collect();
    radii.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let radius = radii[(radii.len() as f32 * 0.95) as usize % radii.len()];

    Some(AttractorStats {
        center: mean,
        radius,
        spread: Vec3::new(var.x.sqrt(), var.y.sqrt(), var.z.sqrt()),
        occupancy: occupancy(&points, mean, radius),
    })
}

/// Fraction of a coarse grid over the attractor's core that holds any point.
/// Points beyond `radius` (the 5% outlier tail) are ignored so a few stray
/// walkers can't inflate the box and make a blob look sparse.
fn occupancy(points: &[Vec3], center: Vec3, radius: f32) -> f32 {
    if radius <= 0.0 {
        return 1.0;
    }
    const N: usize = 10;
    let mut grid = [false; N * N * N];
    let mut counted = 0usize;
    for p in points {
        let d = (*p - center) / radius;
        if d.abs().max_element() > 1.0 {
            continue;
        }
        let idx = |v: f32| ((v * 0.5 + 0.5) * N as f32).clamp(0.0, N as f32 - 1.0) as usize;
        grid[idx(d.x) * N * N + idx(d.y) * N + idx(d.z)] = true;
        counted += 1;
    }
    if counted == 0 {
        return 1.0;
    }
    grid.iter().filter(|c| **c).count() as f32 / grid.len() as f32
}

/// Cells per side at each rung of the lacunarity ladder.
pub const LACUNARITY_RESOLUTIONS: [usize; 4] = [4, 8, 16, 32];

/// Lacunarity spectrum: how much *clumpier than chance* the measure is, at
/// each of [`LACUNARITY_RESOLUTIONS`].
///
/// The textbook gliding-box lacunarity is `Λ = 1 + Var[mass]/Mean[mass]²`, and
/// reporting that raw was measured to be a mistake (2026-08-08 — see CRAFT's
/// discovery log). Scattering `N` points over `C` cells at random already
/// gives `Λ ≈ 1 + C/N` whatever the shape is, so at the fine end of the ladder
/// — where `C` overtakes `N` — the number is a readout of the sample budget,
/// not of the attractor. Across all 44 scenes in `scenes/` the raw curve rose
/// monotonically every time, and its summary tracked `occupancy`, which
/// `--info` already prints.
///
/// So each rung is divided by that chance expectation, and what comes back is
/// an *excess*:
///
/// ```text
///   1.0   as clumped as a random scatter — a smooth, structureless measure
///   > 1   clumped: gaps at this scale, which is what there is to look at
///   < 1   more even than random — a filled, regular solid
/// ```
///
/// This is what makes the two walls separable rather than a restatement of how
/// much of the bounding cube is empty: `menger` lands near 1.5 (flat measure by
/// construction, exactly as predicted), `wellspiral` near 9, `blossom` near 40.
///
/// `None` when no walker can be built or the sample is too sparse to divide by.
pub fn lacunarity_spectrum(transforms: &[TransformSpec], enabled: &[bool]) -> Option<Vec<f32>> {
    use rand::SeedableRng;
    let mut rng = rand::rngs::StdRng::from_seed([0x5d; 32]);
    let mut walker = Walker::new(transforms, enabled, &mut rng)?;

    for _ in 0..BURN_IN_STEPS {
        walker.step(&mut rng);
    }

    let steps = 2000;
    let mut points = Vec::with_capacity(steps);
    let mut sum = Vec3::ZERO;
    for _ in 0..steps {
        walker.step(&mut rng);
        let p = walker.pos;
        if !p.is_finite() {
            return None;
        }
        sum += p;
        points.push(p);
    }

    if points.len() < 100 {
        return None;
    }

    let mean = sum / points.len() as f32;
    let mut radii: Vec<f32> = points.iter().map(|p| (*p - mean).length()).collect();
    radii.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let radius = radii[(radii.len() as f32 * 0.95) as usize % radii.len()];

    if radius <= 0.0 {
        return None;
    }

    let resolutions = LACUNARITY_RESOLUTIONS;
    let mut spectrum = Vec::with_capacity(resolutions.len());

    for &n in &resolutions {
        let mut counts = vec![0u32; n * n * n];
        let mut total = 0u32;

        for p in &points {
            let d = (*p - mean) / radius;
            if d.abs().max_element() > 1.0 {
                continue;
            }
            let idx = |v: f32| ((v * 0.5 + 0.5) * n as f32).clamp(0.0, n as f32 - 1.0) as usize;
            counts[idx(d.x) * n * n + idx(d.y) * n + idx(d.z)] += 1;
            total += 1;
        }

        if total < 10 {
            return None;
        }

        let n_cells = (n * n * n) as f32;
        let mean_mass = total as f32 / n_cells;
        let var_mass: f32 = counts
            .iter()
            .map(|&c| {
                let diff = c as f32 - mean_mass;
                diff * diff
            })
            .sum::<f32>()
            / n_cells;

        let lambda = 1.0 + var_mass / (mean_mass * mean_mass).max(1e-9);

        // What a uniform random scatter of the same `total` points over the
        // same `n_cells` would score. Poisson has Var = Mean, so the whole
        // expectation collapses to `1 + 1/mean_mass` — and dividing it out is
        // what leaves a number about the attractor rather than about how many
        // walkers we could afford.
        let chance = 1.0 + (1.0 - mean_mass.min(1.0)) / mean_mass.max(1e-9);
        spectrum.push(lambda / chance.max(1e-9));
    }

    Some(spectrum)
}

/// One number for the whole ladder: the geometric mean of
/// [`lacunarity_spectrum`], so it keeps that function's units — 1.0 is a
/// random scatter, higher is clumpier at more scales.
///
/// Geometric rather than arithmetic because the rungs span an order of
/// magnitude and the question is "elevated across the ladder", not "how big
/// does it get at its worst".
pub fn lacunarity_summary(transforms: &[TransformSpec], enabled: &[bool]) -> Option<f32> {
    let spectrum = lacunarity_spectrum(transforms, enabled)?;
    if spectrum.is_empty() {
        return None;
    }
    let log_sum: f32 = spectrum.iter().map(|&l| l.max(1e-9).ln()).sum();
    Some((log_sum / spectrum.len() as f32).exp())
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
            post_affine: Mat4::IDENTITY,
            color_value: 0.5,
            weight: 1.0,
            color_speed: 0.5,
            explicit_color_speed: None,
            symmetry: None,
            variations: TransformSpec::linear_variations(),
        })
        .collect()
    }

    /// One off-axis contracting map, optionally enrolled in a group.
    fn one_map(symmetry: Option<crate::symmetry::Symmetry>) -> Vec<TransformSpec> {
        vec![TransformSpec {
            matrix: Mat4::from_scale_rotation_translation(
                Vec3::splat(0.5),
                Quat::IDENTITY,
                Vec3::new(0.7, 0.2, 0.0),
            ),
            post_affine: Mat4::IDENTITY,
            color_value: 0.5,
            weight: 1.0,
            color_speed: 0.5,
            explicit_color_speed: None,
            symmetry,
            variations: TransformSpec::linear_variations(),
        }]
    }

    /// The claim the whole feature rests on: `{g ∘ f : g ∈ G}` has a
    /// `G`-symmetric attractor. Checked on the measure rather than on the
    /// matrices, because matrices multiplying correctly is what
    /// `symmetry.rs`'s own tests already cover — what this has to catch is the
    /// group being composed on the wrong side, or in the wrong place in the
    /// step, either of which still produces a plausible-looking cloud.
    ///
    /// Under `C4` about Y, the x and z marginals of the attractor are equal and
    /// the centroid sits on the axis. Both are properties a wrongly-composed
    /// group breaks.
    /// A repeat has to actually reach along its step. The control is the same
    /// single map, whose attractor is one point; a repeat of eight copies down
    /// +Y must stretch it into a column and drag the centroid up with it.
    #[test]
    fn a_repeat_stretches_the_attractor_along_its_step() {
        let step = crate::symmetry::Repeat {
            count: 8,
            translate: Vec3::new(0.0, 0.35, 0.0),
            turn: 0.0,
            scale: 1.0,
        };
        let sym = crate::symmetry::Symmetry::new(
            crate::symmetry::OrbitKind::Repeat(step),
            Vec3::Y,
            false,
            crate::symmetry::OrbitColor::Shared,
        )
        .unwrap();

        let plain = measure(&one_map(None), &[true]).expect("a single map converges");
        let repeated = measure(&one_map(Some(sym)), &[true]).expect("and so does its chain");

        assert!(
            repeated.spread.y > 5.0 * plain.spread.y.max(1e-6),
            "the chain should open the attractor along y: {:?} vs {:?}",
            repeated.spread,
            plain.spread
        );
        assert!(
            repeated.center.y > plain.center.y + 0.3,
            "and carry the centroid up the step: {} vs {}",
            repeated.center.y,
            plain.center.y
        );
        // The step is along y alone, so x is left as the control found it —
        // this is what separates "walked the step" from "blew up".
        assert!(
            repeated.spread.x < 5.0 * plain.spread.x.max(1e-3) + 0.2,
            "x should be largely untouched, got {:?}",
            repeated.spread
        );
    }

    #[test]
    fn a_group_makes_the_attractor_symmetric_about_its_axis() {
        let sym = crate::symmetry::Symmetry::new(
            crate::symmetry::OrbitKind::Cyclic(4),
            Vec3::Y,
            false,
            crate::symmetry::OrbitColor::Shared,
        )
        .unwrap();

        let plain = measure(&one_map(None), &[true]).expect("a single map converges");
        let symmetric = measure(&one_map(Some(sym)), &[true]).expect("so does its orbit");

        // A single contracting map has one fixed point, and it is nowhere near
        // the axis: this is the control, and it is what the group has to move.
        assert!(
            plain.center.x.abs() > 0.5,
            "control: the unsymmetrized fixed point should be off-axis, got {:?}",
            plain.center
        );

        assert!(
            symmetric.center.x.abs() < 0.02 && symmetric.center.z.abs() < 0.02,
            "a C4 attractor's centroid must sit on its own axis, got {:?}",
            symmetric.center
        );
        assert!(
            (symmetric.spread.x - symmetric.spread.z).abs() < 0.05 * symmetric.spread.x.max(1e-6),
            "C4 about Y must spread x and z alike, got {:?}",
            symmetric.spread
        );
        // And it is genuinely bigger than the point the control collapsed to.
        assert!(
            symmetric.radius > 10.0 * plain.radius,
            "the orbit should open the attractor out: {} vs {}",
            symmetric.radius,
            plain.radius
        );
    }

    /// The polyhedral groups have no axis, so the test is isotropy: an
    /// icosahedral attractor is as wide one way as another.
    #[test]
    fn an_icosahedral_group_spreads_every_axis_alike() {
        let sym = crate::symmetry::Symmetry::new(
            crate::symmetry::OrbitKind::Icosahedral,
            Vec3::Y,
            false,
            crate::symmetry::OrbitColor::Shared,
        )
        .unwrap();
        let stats = measure(&one_map(Some(sym)), &[true]).expect("60 copies still converge");

        assert!(stats.center.length() < 0.05, "centred, got {:?}", stats.center);
        let (lo, hi) = (stats.spread.min_element(), stats.spread.max_element());
        assert!(
            hi - lo < 0.1 * hi,
            "an icosahedral attractor should be near-isotropic, got {:?}",
            stats.spread
        );
    }

    /// The similarity dimension has to count the orbit.
    ///
    /// A group carries no contraction — its elements are orthogonal — but it
    /// multiplies the *number* of maps, and `d` depends on both: `Σsᵢᵈ = 1`
    /// becomes `|G|·sᵈ = 1` for one motif, so `d = ln|G| / ln(1/s)` in closed
    /// form. Getting this wrong is not a rounding error; a symmetric scene
    /// reads as `d = 0`, "dust", which is the opposite of what it is.
    #[test]
    fn the_dimension_counts_the_whole_orbit() {
        for kind in [
            crate::symmetry::OrbitKind::Cyclic(5),
            crate::symmetry::OrbitKind::Dihedral(3),
            crate::symmetry::OrbitKind::Icosahedral,
        ] {
            let sym = crate::symmetry::Symmetry::new(
                kind,
                Vec3::Y,
                false,
                crate::symmetry::OrbitColor::Shared,
            )
            .unwrap();
            let order = sym.order() as f32;
            let maps = one_map(Some(sym));
            // `one_map` is a uniform half-scale, so s is exactly 0.5.
            let s = maps[0].contraction();
            let expected = order.ln() / (1.0 / s).ln();

            let d = crate::scene::similarity_dimension(&maps).expect("it contracts");
            assert!(
                (d - expected).abs() < 1e-3,
                "{:?}: d = {d:.4}, closed form says {expected:.4}",
                kind
            );
        }

        // And without a group the single map is still the degenerate case it
        // always was — one contraction can never sum to 1.
        assert_eq!(crate::scene::similarity_dimension(&one_map(None)), Some(0.0));
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

