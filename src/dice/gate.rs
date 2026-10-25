//! The quality gate every generated flame passes through, fresh or mutated.
//!
//! Each check is measured as a **fault**: how far past its limit the flame
//! is, 0 when it's within it. That one number makes both kinds of judging the
//! same comparison. A fresh roll is held to a baseline of no faults at all —
//! the absolute bounds. A mutation is held to its *parent's* faults: a check
//! the parent passes, the child must pass; a check the parent fails, the child
//! may not fail further. Without that, a hand-authored scene sitting outside
//! the profile's bounds (plenty of good ones do) could never be mutated at all,
//! because every child of it would be rejected.
//!
//! The checks run cheapest first — dimension (analytic), the walk's shares
//! under xaos (a Markov chain, no walk), shape (a short chaos walk),
//! lacunarity (a longer one) — and stop at the first that fails.

use std::borrow::Cow;

use glam::Mat4;
use rand::Rng;

use super::settings::RandomSettings;
use crate::scene::{similarity_dimension, Scene, TransformSpec};
use crate::trace::{self, AttractorStats};

/// Attractor radius bounds. Below the floor it's collapsed to a speck, above
/// the ceiling it's a diverging haze that no camera framing will save.
const MIN_RADIUS: f32 = 0.15;
const MAX_RADIUS: f32 = 8.0;
/// Per-axis standard deviation under which the attractor is degenerate (a
/// point or a line rather than a form). At least two axes must clear it.
const MIN_AXIS_SPREAD: f32 = 0.02;
/// Fraction of the bounding box's cells the attractor may fill before it
/// reads as a solid lump rather than a fractal. A set that fills space
/// uniformly puts a point in nearly every cell at this sample count; the
/// dusty, self-similar structure this renderer exists for does not.
const MAX_OCCUPANCY: f32 = 0.22;
/// The second-widest axis must be at least this fraction of the widest.
/// The absolute floor above only catches literal degeneracy; this catches
/// the far more common near-miss — a thin ribbon or filament that technically
/// has extent on two axes but reads as a line on screen.
const MIN_AXIS_RATIO: f32 = 0.25;
/// Largest axis scale a map may have after the dimension rescale. Past this
/// it barely contracts, and the walk takes forever to settle, if it does.
const MAX_AXIS_SCALE: f32 = 0.95;
/// How much further than its parent a child may break a check the parent
/// already breaks. The measurements are walks, so two nearly identical flames
/// measure a little differently; holding a child to *exactly* its parent's
/// fault would reject half of the children that changed nothing that matters.
const SLACK: f32 = 0.05;

/// Why a candidate was thrown away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    /// Collapsed, diverged, a line, or a lump.
    Shape,
    /// Dimension out of bounds, or no rescale could bring it in without a map
    /// ceasing to contract.
    Dimension,
    /// Lacunarity out of bounds.
    Lacunarity,
    /// Under xaos, a map starved of the walk: it draws next to nothing.
    Xaos,
}

/// One layer's faults: how far past each limit it is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Faults {
    pub dimension: f32,
    /// No walker could be built, or it diverged: 1, else 0.
    pub unmeasured: f32,
    /// Outside the radius bounds, as a log ratio.
    pub radius: f32,
    /// Second-widest axis under the absolute floor.
    pub flat: f32,
    /// Second-widest axis under its fraction of the widest, as a log ratio.
    pub thin: f32,
    /// Occupancy over the ceiling.
    pub solid: f32,
    pub lacunarity: f32,
    /// Under xaos, how far the most starved map's walk share falls short of
    /// its floor, as a fraction of the floor.
    pub starved: f32,
}

impl Faults {
    fn shape(&self) -> [f32; 5] {
        [self.unmeasured, self.radius, self.flat, self.thin, self.solid]
    }
}

/// `by` (at least a hair) when `fails`, else 0 — so a check that fails at
/// exactly its limit, as `<=` tests do, still counts.
fn fault(fails: bool, by: f32) -> f32 {
    if fails { by.max(f32::MIN_POSITIVE) } else { 0.0 }
}

fn shape_faults(stats: Option<&AttractorStats>) -> Faults {
    let Some(s) = stats else { return Faults { unmeasured: 1.0, ..Default::default() } };
    let mut f = Faults::default();
    f.radius = if !s.radius.is_finite() {
        f32::INFINITY
    } else if s.radius < MIN_RADIUS {
        fault(true, (MIN_RADIUS / s.radius.max(1e-9)).ln())
    } else if s.radius > MAX_RADIUS {
        fault(true, (s.radius / MAX_RADIUS).ln())
    } else {
        0.0
    };
    // A form, not a point or a line: at least two axes with real extent,
    // both in absolute terms and relative to the widest one.
    let mut axes = [s.spread.x, s.spread.y, s.spread.z];
    axes.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    f.flat = fault(axes[1] <= MIN_AXIS_SPREAD || axes[0] <= 0.0, MIN_AXIS_SPREAD - axes[1]);
    let ratio = if axes[0] > 0.0 { axes[1] / axes[0] } else { 0.0 };
    f.thin = fault(ratio < MIN_AXIS_RATIO, (MIN_AXIS_RATIO / ratio.max(1e-9)).ln());
    // Structure, not a lump.
    f.solid = fault(s.occupancy > MAX_OCCUPANCY, s.occupancy - MAX_OCCUPANCY);
    f
}

#[cfg(test)]
impl AttractorStats {
    /// Whether this passes the shape checks outright — what a fresh roll is
    /// held to.
    pub fn acceptable(&self) -> bool {
        shape_faults(Some(self)).shape().iter().all(|&v| v == 0.0)
    }
}

/// How starved the most starved map of a layer is (see [`Faults::starved`]):
/// 0 without xaos, where the walk is the weights. A map's floor is the
/// profile's least share, or its share of the weights if that is already less
/// — the check is for what the graph takes away, not what the weights do.
fn starved(maps: &[TransformSpec], min_share: f32) -> f32 {
    if min_share <= 0.0 || !crate::xaos::layer_has_xaos(maps) {
        return 0.0;
    }
    let Some(chain) = crate::xaos::Chain::build(maps, &[]) else { return 0.0 };
    let shares = chain.shares();
    let mut worst = 0.0f32;
    for (start, share) in chain.start.iter().zip(shares) {
        let floor = (min_share as f64).min(*start);
        if floor > 0.0 && share < floor {
            worst = worst.max(((floor - share) / floor) as f32);
        }
    }
    fault(worst > 0.0, worst)
}

/// Whether `child` is worse than `base` allows.
fn worse(child: f32, base: f32) -> bool {
    child.is_nan() || child > base * (1.0 + SLACK)
}

/// A flame that passed, and what it measured.
pub struct Measured {
    /// The first layer that could be measured — what a fresh roll is framed by.
    pub stats: Option<AttractorStats>,
    /// Similarity dimension and lacunarity of the same layer.
    pub dimension: Option<f32>,
    pub lacunarity: Option<f32>,
}

/// A scene's maps, split by layer. A layer is one attractor, and the maps of
/// two layers have no joint dimension or shape, so each is judged alone.
pub fn layers(scene: &Scene) -> Vec<Cow<'_, [TransformSpec]>> {
    if !scene.is_composite() {
        return vec![Cow::Borrowed(&scene.transforms)];
    }
    (0..scene.layer_count())
        .map(|k| {
            let maps: Vec<_> = scene.transforms.iter().filter(|t| t.layer as usize == k).cloned().collect();
            Cow::Owned(maps)
        })
        .collect()
}

/// The checks, and what they're judged against.
pub struct Gate<'a> {
    settings: &'a RandomSettings,
    /// Per layer; a layer past the end is held to no faults.
    baseline: Vec<Faults>,
}

impl<'a> Gate<'a> {
    /// The absolute bounds: what a fresh roll must meet.
    pub fn absolute(settings: &'a RandomSettings) -> Self {
        Self { settings, baseline: Vec::new() }
    }

    /// No worse than `parent`, check by check.
    pub fn relative_to(settings: &'a RandomSettings, parent: &Scene) -> Self {
        let baseline = layers(parent)
            .iter()
            .map(|maps| {
                let stats = measure(maps);
                Faults {
                    dimension: settings.dimension.excess(similarity_dimension(maps), false),
                    lacunarity: settings.lacunarity.excess(lacunarity(maps), true),
                    starved: starved(maps, settings.xaos.min_share),
                    ..shape_faults(stats.as_ref())
                }
            })
            .collect();
        Self { settings, baseline }
    }

    fn base(&self, layer: usize) -> Faults {
        self.baseline.get(layer).copied().unwrap_or_default()
    }

    /// Judge a candidate's layers. `steered` says the dimension has already
    /// been met by [`steer_dimension`], so it isn't judged again.
    pub fn judge(&self, layers: &[Cow<'_, [TransformSpec]>], steered: bool) -> Result<Measured, Reject> {
        let s = self.settings;
        let dims: Vec<_> = layers.iter().map(|m| similarity_dimension(m)).collect();
        if !steered {
            for (k, &d) in dims.iter().enumerate() {
                if worse(s.dimension.excess(d, false), self.base(k).dimension) {
                    return Err(Reject::Dimension);
                }
            }
        }
        for (k, maps) in layers.iter().enumerate() {
            if worse(starved(maps, s.xaos.min_share), self.base(k).starved) {
                return Err(Reject::Xaos);
            }
        }
        let stats: Vec<_> = layers.iter().map(|m| measure(m)).collect();
        for (k, st) in stats.iter().enumerate() {
            let (child, base) = (shape_faults(st.as_ref()).shape(), self.base(k).shape());
            if child.iter().zip(base).any(|(&c, b)| worse(c, b)) {
                return Err(Reject::Shape);
            }
        }
        let mut lacs = Vec::with_capacity(layers.len());
        for (k, maps) in layers.iter().enumerate() {
            let l = lacunarity(maps);
            if worse(s.lacunarity.excess(l, true), self.base(k).lacunarity) {
                return Err(Reject::Lacunarity);
            }
            lacs.push(l);
        }
        let first = stats.iter().position(Option::is_some).unwrap_or(0);
        Ok(Measured {
            stats: stats.get(first).cloned().flatten(),
            dimension: dims.get(first).copied().flatten(),
            lacunarity: lacs.get(first).copied().flatten(),
        })
    }
}

/// Measure a candidate's maps. Everything is enabled for the gate — it judges
/// the flame, not what the window happens to be showing of it.
pub fn measure(transforms: &[TransformSpec]) -> Option<AttractorStats> {
    trace::measure(transforms, &vec![true; transforms.len()])
}

fn lacunarity(transforms: &[TransformSpec]) -> Option<f32> {
    trace::lacunarity_summary(transforms, &vec![true; transforms.len()])
}

/// Scale every map's linear part by `k`, about its own translation.
pub fn rescale(transforms: &mut [TransformSpec], k: f32) {
    for t in transforms {
        let (s, r, p) = t.matrix.to_scale_rotation_translation();
        t.matrix = Mat4::from_scale_rotation_translation(s * k, r, p);
    }
}

/// Put the similarity dimension inside the settings' bound, if it has one,
/// by rescaling every map together. Returns the dimension it ends on; `Err`
/// when the rescale that would do it leaves a map that barely contracts.
pub fn steer_dimension(
    transforms: &mut [TransformSpec],
    settings: &RandomSettings,
    rng: &mut impl Rng,
) -> Result<Option<f32>, ()> {
    let d = similarity_dimension(transforms);
    if settings.dimension.admits(d) {
        return Ok(d);
    }
    let [lo, hi] = settings.dimension.range;
    let target = if hi > lo { rng.gen_range(lo..=hi) } else { lo };
    if crate::xaos::layer_has_xaos(transforms) {
        return steer_under_xaos(transforms, target, lo, hi);
    }
    // The effective map set, as `similarity_dimension` counts it: a motif
    // contributes one contraction per element of its group (all the motif's
    // size for a group, tapering for a repeat). Scaling every map by `k`
    // scales every one of these by `k`, so the same closed form holds.
    let mut contractions = Vec::new();
    for t in transforms.iter() {
        let s = t.linear_determinant().abs().cbrt();
        if !(s > 0.0 && s.is_finite()) {
            return Err(());
        }
        match &t.symmetry {
            Some(g) => contractions.extend(g.element_scales().into_iter().map(|e| s * e)),
            None => contractions.push(s),
        }
    }
    // Σ (k·sᵢ)ᵗ = 1  ⇒  k = (Σ sᵢᵗ)^(−1/t)
    let sum: f32 = contractions.iter().map(|s| s.powf(target)).sum();
    let k = sum.powf(-1.0 / target);
    let widest = transforms
        .iter()
        .map(|t| t.matrix.to_scale_rotation_translation().0.abs().max_element())
        .fold(0.0f32, f32::max);
    if !k.is_finite() || widest * k > MAX_AXIS_SCALE {
        return Err(());
    }
    rescale(transforms, k);
    let d = similarity_dimension(transforms);
    // Bisection inside `similarity_dimension` lands within its tolerance of
    // the target, which can sit a hair outside a bound placed at the target.
    let slack = 1e-3;
    match d {
        Some(v) if v >= lo - slack && v <= hi + slack => Ok(Some(v.clamp(lo, hi))),
        _ => Err(()),
    }
}

/// [`steer_dimension`] for a layer with xaos. The Mauldin–Williams dimension
/// has no closed form in the scale, but it rises with it — every entry of the
/// matrix whose spectral radius sets it grows — so one factor `k` on every
/// map is found by bisection instead.
fn steer_under_xaos(transforms: &mut [TransformSpec], target: f32, lo: f32, hi: f32) -> Result<Option<f32>, ()> {
    let chain = crate::xaos::Chain::build(transforms, &[]).ok_or(())?;
    let widest = transforms
        .iter()
        .map(|t| t.matrix.to_scale_rotation_translation().0.abs().max_element())
        .fold(0.0f32, f32::max);
    if !(widest > 0.0 && widest.is_finite()) {
        return Err(());
    }
    let dimension_at = |k: f32| {
        let mut maps = transforms.to_vec();
        rescale(&mut maps, k);
        let c: Vec<Vec<f32>> = maps.iter().map(crate::scene::copy_contractions).collect();
        chain.dimension(&c)
    };
    // Between a scale that leaves the widest axis barely contracting and a
    // thousandth of it, in log space.
    let (mut a, mut b) = ((MAX_AXIS_SCALE / widest * 1e-3).ln(), (MAX_AXIS_SCALE / widest).ln());
    if dimension_at(b.exp()).is_none_or(|d| d < target) {
        return Err(());
    }
    for _ in 0..40 {
        let m = 0.5 * (a + b);
        match dimension_at(m.exp()) {
            Some(d) if d >= target => b = m,
            _ => a = m,
        }
    }
    rescale(transforms, b.exp());
    let d = similarity_dimension(transforms);
    let slack = 1e-3;
    match d {
        Some(v) if v >= lo - slack && v <= hi + slack => Ok(Some(v.clamp(lo, hi))),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dice::settings::Bound;

    #[test]
    fn a_child_may_not_break_a_bound_further_than_its_parent() {
        assert!(!worse(0.0, 0.0), "both pass");
        assert!(worse(0.1, 0.0), "parent passes, child fails");
        assert!(!worse(1.0, 1.0), "both fail the same");
        assert!(!worse(1.03, 1.0), "inside the slack");
        assert!(worse(1.2, 1.0), "further out");
        assert!(!worse(f32::INFINITY, f32::INFINITY), "neither could be measured");
        assert!(worse(f32::NAN, 1.0));
    }

    /// A parent outside the dimension bound doesn't doom its children:
    /// judged against it, a child at the same dimension passes, and the same
    /// child fails the absolute gate.
    #[test]
    fn a_parent_outside_the_bounds_can_still_have_children() {
        let path = std::env::temp_dir().join(format!("fracturize-gate-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            "[meta]\nname = \"t\"\n[[transform]]\ntranslation = [0.0, 0.0, 0.5]\nscale = 0.5\n\
             [[transform]]\ntranslation = [0.0, 0.47, -0.17]\nscale = 0.5\n\
             [[transform]]\ntranslation = [-0.41, -0.24, -0.17]\nscale = 0.5\n\
             [[transform]]\ntranslation = [0.41, -0.24, -0.17]\nscale = 0.5\n",
        )
        .unwrap();
        let parent = Scene::load(&path).unwrap();
        std::fs::remove_file(&path).ok();
        // Sierpinski's tetrahedron: d = 2 exactly. Bound it to [2.5, 3].
        let settings = RandomSettings { dimension: Bound { on: true, range: [2.5, 3.0] }, ..Default::default() };
        let maps = layers(&parent);
        assert_eq!(Gate::absolute(&settings).judge(&maps, false).err(), Some(Reject::Dimension));
        assert!(Gate::relative_to(&settings, &parent).judge(&maps, false).is_ok());
        // ...but a child further from the bound than its parent is turned away.
        let mut child = parent.clone();
        rescale(&mut child.transforms, 0.8);
        assert_eq!(
            Gate::relative_to(&settings, &parent).judge(&layers(&child), false).err(),
            Some(Reject::Dimension)
        );
    }

    /// Under xaos there's no closed form, but the bisection still lands the
    /// Mauldin–Williams dimension inside the bound.
    #[test]
    fn the_dimension_is_steered_under_xaos_too() {
        use rand::SeedableRng;
        let mut settings = RandomSettings::default();
        settings.dimension = Bound { on: true, range: [1.4, 1.6] };
        let mut rng = rand::rngs::StdRng::seed_from_u64(3);
        let mut maps: Vec<TransformSpec> =
            (0..4).map(|_| crate::dice::roll::random_transform(&settings, &mut rng)).collect();
        maps[0].xaos.set(1, 0.0);
        maps[1].xaos.set(1, 0.0);
        maps[2].xaos.set(0, 0.0);
        let before = similarity_dimension(&maps);
        let d = steer_dimension(&mut maps, &settings, &mut rng).expect("steered").unwrap();
        assert!((1.4..=1.6).contains(&d), "{before:?} steered to {d}");
        let after = similarity_dimension(&maps).unwrap();
        assert!((after - d).abs() < 2e-3, "reported {d}, measures {after}");
    }

    /// A map the graph starves fails the gate; the same map starved by its
    /// own weight does not.
    #[test]
    fn a_starved_map_is_rejected() {
        let mut rng = rand::thread_rng();
        let settings = RandomSettings::default();
        let mut maps: Vec<TransformSpec> =
            (0..3).map(|_| crate::dice::roll::random_transform(&settings, &mut rng)).collect();
        for t in &mut maps {
            t.weight = 1.0;
        }
        assert_eq!(starved(&maps, 0.05), 0.0, "no xaos, nothing to starve");
        // Every way into map 2 all but cut.
        maps[0].xaos.set(2, 0.001);
        maps[1].xaos.set(2, 0.001);
        maps[2].xaos.set(2, 0.001);
        assert!(starved(&maps, 0.05) > 0.0);
        // Its weight starving it is the weights' business.
        let mut light = maps.clone();
        for t in &mut light {
            t.xaos = Default::default();
        }
        light[0].xaos.set(1, 0.5);
        light[2].weight = 0.01;
        assert_eq!(starved(&light, 0.05), 0.0);
    }
}
