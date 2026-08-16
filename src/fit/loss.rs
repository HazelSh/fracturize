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

/// Resolutions in the density pyramid, coarsest first, as a fraction of the
/// finest. The finest itself is chosen from the point count — see [`Grids`].
const PYRAMID: [usize; 4] = [8, 4, 2, 1];

/// Default share of the loss taken by the structure term.
///
/// Weighted toward structure because that is the half that was missing, but
/// not the whole of it: transport is what carries a badly placed map across
/// the frame to somewhere the grid can see it.
pub const DEFAULT_GRID_SHARE: f32 = 0.6;

/// A multi-scale density pyramid of one cloud, held as `√p` per cell.
///
/// This is the term that sees *local* structure, which the transport term is
/// bad at for a reason worth stating precisely, because the loose version of
/// the claim is wrong.
///
/// Sliced Wasserstein over *all* directions is a genuine metric — it is not
/// blind to two-dimensional structure, and given enough slices it will
/// separate two clouds that share their axis-aligned marginals. The problem is
/// that it is a poor *estimator* of fine structure at the handful of slices an
/// inner loop can afford. Projecting integrates detail away unless the
/// direction happens to align with it, and the squared distance between sorted
/// sequences is dominated by the tails, which is gross extent. So a few dozen
/// slices measure placement well and interior badly — which is exactly what
/// the fits kept producing: a subject's extent and silhouette with none of its
/// inside.
///
/// A density grid measures local agreement directly and at constant cost.
/// Where the two differ most is on clouds related by a small displacement of
/// finely divided mass: transport sees a small translation, the grid sees
/// every occupied cell landing on an empty one.
///
/// The plan defers image-space objectives to a later slice because the paper
/// needs to backpropagate through a splat: LPIPS, alpha-blending gradients,
/// real work. None of that applies to a gradient-free optimiser. Rasterising a
/// few thousand points into a small grid is a few thousand adds — *cheaper*
/// than the sorts the transport term already pays for — so the sharper
/// objective arrives at the crude solver's price.
///
/// Stored as `√p` because the comparison is then the Hellinger distance,
/// `Σ(√p − √q)²`. A chaos game's density spans orders of magnitude and a plain
/// MSE would be a readout of the few densest cells; the square root is the
/// compressive curve that makes the whole measure count, and it comes with the
/// bonus that the result is a real distance between distributions, bounded by
/// 2 and zero only when they agree.
struct Grids {
    /// Finest resolution; the rest are `PYRAMID` divisions of it.
    fine: usize,
    /// Per level, coarsest first: `√p` for every cell, row-major.
    levels: Vec<Vec<f32>>,
}

impl Grids {
    /// Cells per point at the finest level.
    ///
    /// At one cell per point the finest grid is mostly empty and its Hellinger
    /// distance is a readout of sampling noise rather than of shape. Four
    /// points per cell keeps it a measurement.
    const POINTS_PER_CELL: usize = 4;

    fn resolution_for(n: usize) -> usize {
        let cells = (n / Self::POINTS_PER_CELL).max(16);
        ((cells as f32).sqrt() as usize).clamp(8, 64)
    }

    /// Rasterise into every level at once.
    fn build(pos: &[Vec2], fine: usize) -> Self {
        let levels = PYRAMID
            .iter()
            .map(|div| {
                let res = (fine / div).max(2);
                let mut g = vec![0.0f32; res * res];
                for p in pos {
                    deposit(&mut g, *p, res);
                }
                blur(&mut g, res);
                let total: f32 = g.iter().sum();
                let inv = 1.0 / total.max(1e-9);
                for v in &mut g {
                    *v = (*v * inv).sqrt();
                }
                g
            })
            .collect();
        Self { fine, levels }
    }

    /// Hellinger distance to another cloud, averaged over the pyramid.
    fn distance(&self, pos: &[Vec2], scratch: &mut Vec<f32>) -> f32 {
        let mut total = 0.0f64;
        for (div, mine) in PYRAMID.iter().zip(&self.levels) {
            let res = (self.fine / div).max(2);
            scratch.clear();
            scratch.resize(res * res, 0.0);
            for p in pos {
                deposit(scratch, *p, res);
            }
            blur(scratch, res);
            let sum: f32 = scratch.iter().sum();
            let inv = 1.0 / sum.max(1e-9);
            let mut acc = 0.0f64;
            for (a, b) in scratch.iter().zip(mine) {
                let d = (a * inv).sqrt() - b;
                acc += (d * d) as f64;
            }
            total += acc;
        }
        (total / PYRAMID.len() as f64) as f32
    }
}

/// Deposit one point into a grid, spread over the four cells it sits between.
///
/// Bilinear rather than nearest-cell, and that is not a quality nicety — it is
/// what gives the term a gradient at all. With hard binning, a thin target
/// (which `edges` very much is) saturates: the attractor's cells simply do not
/// coincide with the target's, every fine level sits near its maximum
/// distance, and a move that brings a map *nearly* onto a target feature
/// scores exactly the same as one that does not. The fit then only hears the
/// coarse levels, whose advice is "fill the silhouette" — which is precisely
/// the diffuse blob that hard binning produced.
///
/// This is the same reasoning that makes the paper splat Gaussians instead of
/// accumulating points, arrived at from the other end.
///
/// Coordinates are clamped rather than dropped, so an attractor that flings
/// mass outside the frame is penalised for it: the strays pile into border
/// cells where the target has none. Dropping them would make throwing material
/// off-screen free.
fn deposit(g: &mut [f32], p: Vec2, res: usize) {
    let map = |v: f32| ((v * 0.5 + 0.5) * res as f32 - 0.5).clamp(0.0, res as f32 - 1.0);
    let (fx, fy) = (map(p.x), map(p.y));
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(res - 1), (y0 + 1).min(res - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    g[y0 * res + x0] += (1.0 - tx) * (1.0 - ty);
    g[y0 * res + x1] += tx * (1.0 - ty);
    g[y1 * res + x0] += (1.0 - tx) * ty;
    g[y1 * res + x1] += tx * ty;
}

/// One separable 1-2-1 pass, in place.
///
/// Widens each feature's basin of attraction by a further cell. Bilinear
/// deposit alone only reaches the immediately adjacent cells, which for a
/// target made of one-cell-wide lines still leaves most of the frame flat.
///
/// In place, with a rolling window horizontally and a single row of history
/// vertically, because this runs four times per loss evaluation and the whole
/// argument for the term is that it is cheap.
fn blur(g: &mut [f32], res: usize) {
    for y in 0..res {
        let row = y * res;
        let mut left = g[row];
        for x in 0..res {
            let here = g[row + x];
            let right = g[row + (x + 1).min(res - 1)];
            g[row + x] = 0.25 * left + 0.5 * here + 0.25 * right;
            left = here;
        }
    }
    let mut above = vec![0.0f32; res];
    above.copy_from_slice(&g[0..res]);
    for y in 0..res {
        let row = y * res;
        let next = ((y + 1).min(res - 1)) * res;
        for x in 0..res {
            let here = g[row + x];
            let below = g[next + x];
            g[row + x] = 0.25 * above[x] + 0.5 * here + 0.25 * below;
            above[x] = here;
        }
    }
}

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
    /// The target's own density pyramid — the structure-aware half.
    grids: Grids,
    /// Share of the reported loss that comes from the grid term.
    grid_share: f32,
    /// What each term scores against a cloud that knows nothing: uniform
    /// noise filling the box.
    ///
    /// The two terms have unrelated natural scales — transport is in squared
    /// world units, Hellinger is a bounded statistical distance — so blending
    /// them by a raw weight would make the weight meaningless and its useful
    /// range reference-dependent. Dividing each by what a hopeless cloud
    /// scores puts both at about 1 for "no information" and 0 for "exact", so
    /// the share is a share.
    sw_ref: f32,
    grid_ref: f32,
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
        grid_share: f32,
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

        let fine = Grids::resolution_for(n);
        let grids = Grids::build(pos, fine);

        let mut me = Self {
            dirs,
            quantiles,
            n,
            col_center,
            col_scale,
            grids,
            grid_share: grid_share.clamp(0.0, 1.0),
            sw_ref: 1.0,
            grid_ref: 1.0,
        };

        // Calibrate both terms against uniform noise in the box: the cloud
        // that carries no information about the target.
        let noise: Vec<Vec2> = (0..n)
            .map(|_| Vec2::new(rng.gen_range(-0.75..0.75), rng.gen_range(-0.75..0.75)))
            .collect();
        let grey = vec![Vec3::splat(0.5); n];
        let mut scratch = Vec::new();
        me.sw_ref = me.sliced(&noise, &grey, 128, 7, &mut scratch).max(1e-9);
        me.grid_ref = me.grids.distance(&noise, &mut scratch).max(1e-9);
        me
    }

    /// The grid term alone, normalised. Exposed for the diagnostic sheet.
    pub fn structure(&self, pos: &[Vec2], scratch: &mut Vec<f32>) -> f32 {
        self.grids.distance(pos, scratch) / self.grid_ref
    }

    /// The loss the fit actually minimises: transport and structure, blended.
    ///
    /// Both halves are wanted. Transport is smooth and long-range — it will
    /// drag a badly placed map across the frame, which a density grid cannot,
    /// because two clouds that do not overlap have the same grid distance
    /// however far apart they are. Structure is what transport lacks. Running
    /// them together gives the annealer a gradient to follow from far away and
    /// something worth resolving once it arrives.
    pub fn eval(
        &self,
        pos: &[Vec2],
        col: &[Vec3],
        slices: usize,
        slice_seed: u64,
        scratch: &mut Vec<f32>,
    ) -> f32 {
        let w = self.grid_share;
        let transport = if w < 1.0 {
            self.sliced(pos, col, slices, slice_seed, scratch) / self.sw_ref
        } else {
            0.0
        };
        let structure = if w > 0.0 {
            self.grids.distance(pos, scratch) / self.grid_ref
        } else {
            0.0
        };
        (1.0 - w) * transport + w * structure
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
    fn sliced(
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
        let target = Target::new(&pos, &col, 4000, 0.0, 0.0, &mut rng);
        let mut scratch = Vec::new();
        let same = target.eval(&pos, &col, 64, 1, &mut scratch);
        assert!(same < 1e-4, "a cloud against itself should be ~0, got {same}");
    }

    #[test]
    fn distance_grows_with_displacement() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(11);
        let (pos, col) = cloud(4000, 0.0, &mut rng);
        let target = Target::new(&pos, &col, 4000, 0.0, 0.0, &mut rng);
        let mut scratch = Vec::new();

        let (near, ncol) = cloud(4000, 0.1, &mut rng);
        let (far, fcol) = cloud(4000, 0.8, &mut rng);
        let a = target.eval(&near, &ncol, 96, 2, &mut scratch);
        let b = target.eval(&far, &fcol, 96, 2, &mut scratch);
        assert!(a < b, "0.1 apart ({a}) should beat 0.8 apart ({b})");
    }

    /// The claim the structure term exists to make good on.
    ///
    /// A checkerboard of small blobs against its own complement. The two are
    /// related by a displacement of one cell, so every projection of one is a
    /// slightly shifted projection of the other and transport reads them as
    /// nearly the same cloud — while locally they agree nowhere at all, every
    /// occupied cell of one landing on an empty cell of the other.
    ///
    /// This is the situation a starved IFS fit is actually in: mass in roughly
    /// the right place, divided wrongly. If the two terms did not separate
    /// here there would be no reason to pay for both.
    #[test]
    fn structure_sees_what_transport_glosses_over() {
        const CELLS: usize = 10;
        const SPAN: f32 = 0.7;
        let step = 2.0 * SPAN / CELLS as f32;

        let board = |parity: usize, rng: &mut rand::rngs::StdRng| {
            let mut pts = Vec::new();
            for iy in 0..CELLS {
                for ix in 0..CELLS {
                    if (ix + iy) % 2 != parity {
                        continue;
                    }
                    let cx = -SPAN + (ix as f32 + 0.5) * step;
                    let cy = -SPAN + (iy as f32 + 0.5) * step;
                    for _ in 0..80 {
                        pts.push(Vec2::new(
                            cx + rng.gen_range(-0.2..0.2) * step,
                            cy + rng.gen_range(-0.2..0.2) * step,
                        ));
                    }
                }
            }
            pts
        };

        let mut rng = rand::rngs::StdRng::seed_from_u64(13);
        let target_pts = board(0, &mut rng);
        let other_pts = board(1, &mut rng);
        let n = target_pts.len().min(other_pts.len());
        let (target_pts, other_pts) = (&target_pts[..n], &other_pts[..n]);
        let grey = vec![Vec3::splat(0.5); n];

        let mut scratch = Vec::new();
        let transport = Target::new(target_pts, &grey, n, 0.0, 0.0, &mut rng);
        let sw = transport.eval(other_pts, &grey, 128, 5, &mut scratch);

        let structural = Target::new(target_pts, &grey, n, 0.0, 1.0, &mut rng);
        let grid = structural.eval(other_pts, &grey, 128, 5, &mut scratch);

        // Both are normalised against uninformative noise, so they compare
        // directly: what fraction of "knows nothing" does each still see?
        println!("  transport {sw:.3}   structure {grid:.3}   ratio {:.2}x", grid / sw);
        assert!(
            grid > sw * 2.0,
            "structure ({grid:.3}) should see substantially more than transport \
             ({sw:.3}) between a checkerboard and its complement"
        );
    }

    #[test]
    fn colour_is_measured_when_weighted() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(3);
        let (pos, _) = cloud(3000, 0.0, &mut rng);
        let red = vec![Vec3::new(1.0, 0.0, 0.0); 3000];
        let blue = vec![Vec3::new(0.0, 0.0, 1.0); 3000];

        let mut scratch = Vec::new();
        let geometric = Target::new(&pos, &red, 3000, 0.0, 0.0, &mut rng);
        let shape_only = geometric.eval(&pos, &blue, 96, 3, &mut scratch);
        assert!(shape_only < 1e-4, "colour must be invisible at weight 0, got {shape_only}");

        let coloured = Target::new(&pos, &red, 3000, 1.0, 0.0, &mut rng);
        let wrong = coloured.eval(&pos, &blue, 96, 3, &mut scratch);
        assert!(wrong > 1e-2, "red vs blue must cost something, got {wrong}");
    }
}
