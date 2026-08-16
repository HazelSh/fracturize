//! Sliced Wasserstein between two point clouds.
//!
//! Project both clouds onto a random unit direction, sort, and take the mean
//! squared difference between the sorted sequences: in one dimension optimal
//! transport *is* the sorted pairing, so each slice is an exact Wasserstein-2
//! distance, and the mean over directions is a metric on the clouds. A
//! projection and a sort — no rasteriser, no backward pass through a splat,
//! and it costs the same in five dimensions as in two, which is what lets
//! colour ride along in the same loss as shape (LIKENESS-PLAN.md §0.7).
//!
//! Its failure mode is the reason it was chosen over an image-space
//! objective. Sliced Wasserstein fails *blurry and shape-y* — right
//! silhouette, wrong substance — where a pixel loss fails toward noise. We
//! want the representation to break down legibly.

use glam::{Vec2, Vec3};
use rand::Rng;

/// Directions held in the bank. Redrawn per iteration from here rather than
/// generated fresh, because the expensive half of a direction is the sorted
/// target it is compared against, and that can be precomputed once.
const BANK: usize = 192;

/// A target cloud, pre-projected onto a bank of directions.
pub struct Target {
    dirs: Vec<[f32; 5]>,
    /// Per direction: `n` evenly-spaced quantiles of the target's projection,
    /// ascending.
    ///
    /// Quantiles rather than a subsample, so the loss sees the whole target
    /// however few points the attractor is walked with — the pairing needs
    /// two equal-length sorted sequences, and shrinking the *target* to get
    /// them would let the fit overfit to whichever points were drawn.
    quantiles: Vec<Vec<f32>>,
    /// Points the attractor must supply.
    pub n: usize,
    /// Where the target's colours sit and how far they spread, so the colour
    /// axes arrive in the same box the positional ones are normalised into.
    ///
    /// Without this the weight would mean something different for every
    /// reference: a high-contrast card's colour axes would swamp its geometry
    /// and a muted one's would vanish, at the same `--fit-color`. The weight
    /// itself lives in the directions, not here.
    col_center: Vec3,
    col_scale: f32,
}

fn feature(p: Vec2, c: Vec3, center: Vec3, scale: f32) -> [f32; 5] {
    let c = (c - center) * scale;
    [p.x, p.y, c.x, c.y, c.z]
}

fn project(d: &[f32; 5], f: &[f32; 5]) -> f32 {
    d[0] * f[0] + d[1] * f[1] + d[2] * f[2] + d[3] * f[3] + d[4] * f[4]
}

impl Target {
    /// `n` is how many points each evaluation will walk; `color_share` is the
    /// fraction of the metric that measures colour rather than position.
    pub fn new(
        pos: &[Vec2],
        col: &[Vec3],
        n: usize,
        color_share: f32,
        rng: &mut impl Rng,
    ) -> Self {
        let dirs: Vec<[f32; 5]> = (0..BANK).map(|_| random_direction(color_share, rng)).collect();

        // Same statistic the positions were normalised with, for the same
        // reason: median and a high percentile, so one stray highlight can't
        // decide the scale of the whole colour axis.
        let col_center = Vec3::new(
            super::median(col.iter().map(|c| c.x)),
            super::median(col.iter().map(|c| c.y)),
            super::median(col.iter().map(|c| c.z)),
        );
        let spread = (0..3)
            .map(|axis| {
                let d = |c: &Vec3| (c[axis] - col_center[axis]).abs();
                super::percentile(col.iter().map(d), 0.98)
            })
            .fold(0.0f32, f32::max);
        // Normalisation only. How much colour *counts* is decided by the
        // direction split in `random_direction`, not by inflating the axes.
        let col_scale = super::BOX / spread.max(1e-4);

        let feats: Vec<[f32; 5]> = pos
            .iter()
            .zip(col)
            .map(|(p, c)| feature(*p, *c, col_center, col_scale))
            .collect();

        let mut scratch = vec![0.0f32; feats.len()];
        let quantiles = dirs
            .iter()
            .map(|d| {
                for (s, f) in scratch.iter_mut().zip(&feats) {
                    *s = project(d, f);
                }
                scratch.sort_unstable_by(f32::total_cmp);
                quantiles_of(&scratch, n)
            })
            .collect();

        Self { dirs, quantiles, n, col_center, col_scale }
    }

    /// Mean sliced Wasserstein-2 over `slices` directions drawn from the bank.
    ///
    /// `slice_seed` chooses the directions, and the caller passing the *same*
    /// seed for two candidates is the difference between an optimiser that
    /// works and one that random-walks. The estimate over a handful of
    /// directions is noisy enough to swamp the effect of nudging one map;
    /// scoring both candidates through the same directions cancels that noise
    /// exactly, leaving a difference that is entirely due to the parameters.
    /// (Common random numbers — the same reason the chaos-game stream is held
    /// fixed across a block.)
    ///
    /// `scratch` is the caller's, reused across iterations — this is the inner
    /// loop of the whole fit and it should not be allocating.
    pub fn eval(
        &self,
        pos: &[Vec2],
        col: &[Vec3],
        slices: usize,
        slice_seed: u64,
        scratch: &mut Vec<f32>,
    ) -> f32 {
        use rand::SeedableRng;
        let mut pick = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(slice_seed);

        debug_assert_eq!(pos.len(), self.n);
        scratch.clear();
        scratch.resize(self.n, 0.0);

        let mut total = 0.0f64;
        for _ in 0..slices {
            let k = pick.gen_range(0..self.dirs.len());
            let d = &self.dirs[k];
            for (s, (p, c)) in scratch.iter_mut().zip(pos.iter().zip(col)) {
                *s = project(d, &feature(*p, *c, self.col_center, self.col_scale));
            }
            scratch.sort_unstable_by(f32::total_cmp);

            let q = &self.quantiles[k];
            let mut acc = 0.0f64;
            for (a, b) in scratch.iter().zip(q) {
                let e = (a - b) as f64;
                acc += e * e;
            }
            total += acc / self.n as f64;
        }
        (total / slices as f64) as f32
    }
}

/// A random direction whose colour share is exactly `share`.
///
/// Not a uniform direction on the 5-sphere, and the difference is the whole
/// point. Colour occupies three of the five axes, so a uniform direction puts
/// about 60% of its squared length on colour whatever weight you thought you
/// asked for — measured on a Sierpinski reference, "colour 0.5" starved the
/// geometry badly enough that the fit returned filaments instead of a
/// triangle, and the same fit at colour 0 found the gasket.
///
/// So the split is drawn explicitly: a 2-vector for position and a 3-vector
/// for colour, each uniform on its own sphere, scaled by `√(1−share)` and
/// `√share`. Because the two blocks are orthogonal, the expected squared
/// projection splits in exactly that ratio, and `--fit-color 0.15` means
/// fifteen percent of the metric rather than an unknown fraction of it.
fn random_direction(share: f32, rng: &mut impl Rng) -> [f32; 5] {
    let share = share.clamp(0.0, 1.0);
    let mut d = [0.0f32; 5];
    fill_unit(&mut d[0..2], (1.0 - share).sqrt(), rng);
    fill_unit(&mut d[2..5], share.sqrt(), rng);
    d
}

/// A uniform direction on the unit sphere of `slot`, scaled to length `len`.
fn fill_unit(slot: &mut [f32], len: f32, rng: &mut impl Rng) {
    if len <= 0.0 {
        slot.fill(0.0);
        return;
    }
    loop {
        for v in slot.iter_mut() {
            // Box–Muller, one sample per call; a normal vector normalised is
            // uniform on the sphere and nothing else this simple is.
            let u1: f32 = rng.gen_range(1e-7f32..1.0);
            let u2: f32 = rng.r#gen();
            *v = (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos();
        }
        let norm = slot.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 1e-6 {
            for v in slot.iter_mut() {
                *v *= len / norm;
            }
            return;
        }
    }
}

/// `n` evenly-spaced quantiles of an already-sorted sequence, at the midpoints
/// `(i + 0.5)/n` — the same positions the sorted attractor's own order
/// statistics sit at, so the pairing is the one the transport wants.
fn quantiles_of(sorted: &[f32], n: usize) -> Vec<f32> {
    if sorted.is_empty() {
        return vec![0.0; n];
    }
    let last = (sorted.len() - 1) as f32;
    (0..n)
        .map(|i| {
            let t = ((i as f32 + 0.5) / n as f32) * last;
            let lo = t.floor() as usize;
            let hi = (lo + 1).min(sorted.len() - 1);
            sorted[lo] + (sorted[hi] - sorted[lo]) * (t - lo as f32)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn cloud(n: usize, shift: f32, rng: &mut impl Rng) -> (Vec<Vec2>, Vec<Vec3>) {
        (0..n)
            .map(|_| {
                (
                    Vec2::new(rng.gen_range(-1.0..1.0) + shift, rng.gen_range(-1.0..1.0)),
                    Vec3::splat(0.5),
                )
            })
            .unzip()
    }

    #[test]
    fn identical_clouds_score_near_zero() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let (pos, col) = cloud(4000, 0.0, &mut rng);
        let target = Target::new(&pos, &col, 4000, 0.0, &mut rng);
        let mut scratch = Vec::new();
        let same = target.eval(&pos, &col, 64, 1, &mut scratch);
        assert!(same < 1e-4, "a cloud against itself should be ~0, got {same}");
    }

    #[test]
    fn distance_grows_with_displacement() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(11);
        let (pos, col) = cloud(4000, 0.0, &mut rng);
        let target = Target::new(&pos, &col, 4000, 0.0, &mut rng);
        let mut scratch = Vec::new();

        let (near, ncol) = cloud(4000, 0.1, &mut rng);
        let (far, fcol) = cloud(4000, 0.8, &mut rng);
        let a = target.eval(&near, &ncol, 96, 2, &mut scratch);
        let b = target.eval(&far, &fcol, 96, 2, &mut scratch);
        assert!(a < b, "0.1 apart ({a}) should beat 0.8 apart ({b})");
    }

    #[test]
    fn colour_is_measured_when_weighted() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(3);
        let (pos, _) = cloud(3000, 0.0, &mut rng);
        let red = vec![Vec3::new(1.0, 0.0, 0.0); 3000];
        let blue = vec![Vec3::new(0.0, 0.0, 1.0); 3000];

        let mut scratch = Vec::new();
        let geometric = Target::new(&pos, &red, 3000, 0.0, &mut rng);
        let shape_only = geometric.eval(&pos, &blue, 96, 3, &mut scratch);
        assert!(shape_only < 1e-4, "colour must be invisible at weight 0, got {shape_only}");

        let coloured = Target::new(&pos, &red, 3000, 1.0, &mut rng);
        let wrong = coloured.eval(&pos, &blue, 96, 3, &mut scratch);
        assert!(wrong > 1e-2, "red vs blue must cost something, got {wrong}");
    }
}
