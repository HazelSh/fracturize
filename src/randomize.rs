//! Random flame generator: a whole new `Scene` from nothing, for the Explore
//! menu's dice, the Random flames list, and `--random`.
//!
//! Randomising an IFS is easy; randomising one that *renders* is not. Most
//! random parameter sets either collapse to a point or diverge to a haze, so
//! every candidate is run through a short CPU chaos game (the same `trace.rs`
//! walkers the GPU shader mirrors) and rejected unless it settles into a
//! bounded attractor with real extent on more than one axis.
//!
//! **What gets rolled is configurable** ([`RandomSettings`]): which variations
//! may appear and at what weights, how many maps a flame has and how big they
//! are, and bounds on the similarity dimension and the lacunarity. The two
//! bounds are met in different ways, because they cost different amounts:
//!
//! - **Dimension is steered, not rejected.** It is analytic (`Σ sᵢᵈ = 1` over
//!   the maps' contractions), and scaling every map by one factor `k` moves it
//!   predictably: `k = (Σ sᵢᵗ)^(-1/t)` puts it exactly on a target `t`. So a
//!   flame outside the bound is rescaled onto a target drawn inside it, which
//!   keeps the maps' relative sizes and costs nothing. Only a rescale that
//!   would stop a map contracting is a rejection.
//! - **Lacunarity is rejected.** It is measured by a chaos walk, and nothing
//!   predicts it from the parameters, so a candidate outside the bound is
//!   thrown away and another rolled — up to [`RandomSettings::attempts`]
//!   tries, with the reasons counted ([`Rejections`]) so a bound that is too
//!   tight can say so instead of silently returning nothing.
//!
//! Colour is rolled as well as form — the colour source (all three modes),
//! the gradient a palette-mode roll needs, and the background it all sits on.
//! Every draw comes off the caller's rng, so `--random --seed N` reproduces a
//! roll exactly, colours included, given the same settings.

use std::collections::BTreeMap;

use glam::{Mat4, Quat, Vec3};
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::path::CameraPath;
use crate::scene::{
    generate_colormap, resolve_color_speeds, similarity_dimension, Scene, TransformSpec,
    NUM_VARIATIONS, VARIATION_NAMES,
};
use crate::trace::{self, AttractorStats};

/// Variations rolled unless the settings say otherwise: the ones that stay
/// bounded under repeated application in 3D. Notably absent: `spherical`,
/// whose 1/r² inversion reliably blows scenes out (see AGENTS.md) — a fine
/// thing to reach for by hand, and something the settings can switch on, but
/// a bad default for dice.
const FRIENDLY: &[&str] = &[
    "sinusoidal",
    "swirl",
    "bubble",
    "fisheye",
    "julia",
    "horseshoe",
    "disc",
    "spiral",
    "bulb",
];

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

/// One variation's place in the dice: whether it can be rolled, and the
/// range its weight is drawn from (a magnitude; the sign is
/// [`RandomSettings::negative`]'s job).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct VariationRange {
    pub enabled: bool,
    pub weight: [f32; 2],
}

impl Default for VariationRange {
    fn default() -> Self {
        Self { enabled: false, weight: [0.12, 0.75] }
    }
}

/// Whether, and how, a rolled flame is symmetric (`crate::symmetry`).
///
/// Off unless `chance` says otherwise, so the default dice are unchanged. When
/// a roll takes a group, it picks one of the enabled kinds, makes some of the
/// flame's maps its motifs and leaves `free` of them outside — CRAFT §3.6:
/// *symmetry gets you a form, one map outside the group gets you a picture*,
/// because an orbit spreads measure evenly by construction and a wholly
/// symmetric flame reads as wallpaper.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct SymmetrySettings {
    /// Chance a flame is symmetric at all.
    pub chance: f32,
    /// `Cyc<n>`: n-fold about an axis.
    pub cyclic: bool,
    /// `Dih<n>`: n-fold plus the half-turn flip.
    pub dihedral: bool,
    /// The fold `n` for cyclic and dihedral.
    pub fold: [u32; 2],
    pub tetra: bool,
    pub octa: bool,
    pub icosa: bool,
    /// A repeat: copies stepped along a helix, which is not a group (see
    /// `symmetry::OrbitKind`) but is the same machinery.
    pub repeat: bool,
    /// Copies in a repeat.
    pub repeat_count: [u32; 2],
    /// Maps kept outside the group. At least one motif is always left.
    pub free: [u32; 2],
    /// Chance the group is extended by the central inversion, doubling it.
    pub mirror: f32,
    /// Chance the orbit is coloured by group element rather than shared.
    pub orbit_color: f32,
}

impl Default for SymmetrySettings {
    fn default() -> Self {
        Self {
            chance: 0.0,
            cyclic: true,
            dihedral: true,
            fold: [3, 8],
            tetra: true,
            octa: true,
            icosa: true,
            repeat: false,
            repeat_count: [5, 16],
            free: [1, 1],
            mirror: 0.15,
            orbit_color: 0.25,
        }
    }
}

impl SymmetrySettings {
    /// Whether any kind is switched on — with none, `chance` means nothing.
    pub fn any_kind(&self) -> bool {
        self.cyclic || self.dihedral || self.tetra || self.octa || self.icosa || self.repeat
    }
}

/// A bound that can be switched off without forgetting its range.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Bound {
    pub on: bool,
    pub range: [f32; 2],
}

impl Bound {
    fn off(range: [f32; 2]) -> Self {
        Self { on: false, range }
    }

    /// Whether `v` satisfies the bound. Off passes everything; on, a value
    /// that couldn't be measured fails.
    pub fn admits(&self, v: Option<f32>) -> bool {
        !self.on || v.is_some_and(|v| v >= self.range[0] && v <= self.range[1])
    }
}

impl Default for Bound {
    fn default() -> Self {
        Self::off([0.0, 1.0])
    }
}

/// What the dice may roll. Persisted to prefs as `[random]`, edited in the
/// Random settings window, and readable by `--random-config`.
///
/// Every range is `[min, max]`, inclusive; [`Self::normalized`] puts them in
/// order and inside what the generator can use, so a hand-edited file can't
/// hand it an inverted or impossible range.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct RandomSettings {
    /// Maps per flame.
    pub transforms: [u32; 2],
    /// Each map's base scale, drawn log-uniformly — before any dimension
    /// rescale, which keeps the maps' ratios but not this range.
    pub scale: [f32; 2],
    /// Chance a map has one axis squashed, which is what turns blobs into
    /// sheets and ribbons.
    pub squash: f32,
    /// Chance a flame carries no variations at all — pure affine.
    pub affine: f32,
    /// How many different variations a flame draws its maps' variations
    /// from. One kind across several maps reads as a coherent form; many
    /// kinds read as a pile of unrelated distortions.
    pub kinds: [u32; 2],
    /// Variations on each map, chosen from the flame's kinds. At least one
    /// map always gets one (unless the flame rolled affine).
    pub per_map: [u32; 2],
    /// Chance a rolled variation weight is negative (Apophysis-style).
    pub negative: f32,
    /// Per variation, by name. A name not listed takes the default: enabled
    /// if it is one of the bounded variations, at `[0.12, 0.75]`. `linear` is
    /// never listed — it is what's left: `1 − Σ|w|`, floored at 0.
    pub variations: BTreeMap<String, VariationRange>,
    /// Symmetry groups, and how often.
    pub symmetry: SymmetrySettings,
    /// Bound on the similarity dimension of the maps' affine parts. Steered:
    /// see the module docs.
    pub dimension: Bound,
    /// Bound on the lacunarity summary (1 = a random scatter, higher =
    /// clumpier). Rejection-sampled.
    pub lacunarity: Bound,
    /// Candidates tried per flame before giving up.
    pub attempts: u32,
}

impl Default for RandomSettings {
    fn default() -> Self {
        let variations = VARIATION_NAMES
            .iter()
            .skip(1)
            .map(|&name| {
                let range = VariationRange { enabled: FRIENDLY.contains(&name), ..Default::default() };
                (name.to_string(), range)
            })
            .collect();
        Self {
            transforms: [2, 5],
            scale: [0.35, 0.75],
            squash: 0.3,
            affine: 0.08,
            kinds: [1, 2],
            per_map: [0, 1],
            negative: 0.15,
            variations,
            symmetry: SymmetrySettings::default(),
            dimension: Bound::off([1.5, 2.5]),
            // p10-p75 of default rolls (`random_flame_study`): on, it keeps
            // most of what the dice make and trims the smooth and the sparse.
            lacunarity: Bound::off([8.0, 30.0]),
            attempts: 60,
        }
    }
}

/// Ranges' outer limits, shared by `normalized` and the settings window.
pub const MAX_TRANSFORMS: u32 = 12;
pub const SCALE_LIMITS: [f32; 2] = [0.05, 0.95];
pub const WEIGHT_LIMIT: f32 = 2.0;
pub const DIMENSION_LIMITS: [f32; 2] = [0.1, 6.0];
pub const LACUNARITY_LIMITS: [f32; 2] = [0.1, 1000.0];
pub const MAX_ATTEMPTS: u32 = 1000;

fn ordered<T: PartialOrd + Copy>(r: [T; 2], lo: T, hi: T) -> [T; 2] {
    let clamp = |v: T| if v < lo { lo } else if v > hi { hi } else { v };
    let (a, b) = (clamp(r[0]), clamp(r[1]));
    if a <= b { [a, b] } else { [b, a] }
}

impl RandomSettings {
    /// Ranges in order and in bounds, and every variation listed.
    pub fn normalized(mut self) -> Self {
        self.transforms = ordered(self.transforms, 1, MAX_TRANSFORMS);
        self.scale = ordered(self.scale, SCALE_LIMITS[0], SCALE_LIMITS[1]);
        self.squash = self.squash.clamp(0.0, 1.0);
        self.affine = self.affine.clamp(0.0, 1.0);
        self.kinds = ordered(self.kinds, 1, NUM_VARIATIONS as u32 - 1);
        self.per_map = ordered(self.per_map, 0, NUM_VARIATIONS as u32 - 1);
        self.negative = self.negative.clamp(0.0, 1.0);
        let defaults = Self::default().variations;
        // Unknown names dropped, missing ones filled in.
        self.variations.retain(|k, _| defaults.contains_key(k));
        for (name, d) in defaults {
            let v = self.variations.entry(name).or_insert(d);
            v.weight = ordered(v.weight, 0.0, WEIGHT_LIMIT);
        }
        let sym = &mut self.symmetry;
        sym.chance = sym.chance.clamp(0.0, 1.0);
        sym.fold = ordered(sym.fold, 2, crate::symmetry::MAX_FOLD);
        sym.repeat_count = ordered(sym.repeat_count, 2, crate::symmetry::MAX_REPEAT);
        sym.free = ordered(sym.free, 0, MAX_TRANSFORMS);
        sym.mirror = sym.mirror.clamp(0.0, 1.0);
        sym.orbit_color = sym.orbit_color.clamp(0.0, 1.0);
        self.dimension.range = ordered(self.dimension.range, DIMENSION_LIMITS[0], DIMENSION_LIMITS[1]);
        self.lacunarity.range =
            ordered(self.lacunarity.range, LACUNARITY_LIMITS[0], LACUNARITY_LIMITS[1]);
        self.attempts = self.attempts.clamp(1, MAX_ATTEMPTS);
        self
    }

    /// The variation in GPU slot `slot` (1.., never linear), defaulted if
    /// the settings don't list it.
    pub fn variation(&self, slot: usize) -> VariationRange {
        let name = VARIATION_NAMES[slot];
        self.variations.get(name).copied().unwrap_or(VariationRange {
            enabled: FRIENDLY.contains(&name),
            ..Default::default()
        })
    }

    /// Slots that may be rolled.
    pub fn enabled_slots(&self) -> Vec<usize> {
        (1..NUM_VARIATIONS).filter(|&s| self.variation(s).enabled).collect()
    }

    /// Read settings from a TOML file: either the table itself, or a file
    /// with a `[random]` table in it — which is what prefs.toml is, so the
    /// GUI's settings can be handed straight to `--random-config`.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let value: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let table = match value.get("random") {
            Some(toml::Value::Table(t)) => t.clone(),
            _ => value,
        };
        let settings: Self = toml::Value::Table(table).try_into().map_err(|e: toml::de::Error| e.to_string())?;
        Ok(settings.normalized())
    }
}

/// Why candidates were thrown away, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rejections {
    /// Collapsed, diverged, a line, or a lump: the quality gate.
    pub shape: u32,
    /// No rescale could put the dimension in bounds without a map ceasing to
    /// contract.
    pub dimension: u32,
    /// Lacunarity outside its bound.
    pub lacunarity: u32,
}

impl Rejections {
    pub fn total(&self) -> u32 {
        self.shape + self.dimension + self.lacunarity
    }

    /// "52 lacunarity, 7 shape", biggest first, for messages.
    pub fn describe(&self) -> String {
        let mut parts = [
            (self.lacunarity, "lacunarity"),
            (self.dimension, "dimension"),
            (self.shape, "shape"),
        ];
        parts.sort_by(|a, b| b.0.cmp(&a.0));
        parts
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, what)| format!("{n} {what}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A flame that met the settings, and what it measured.
pub struct Roll {
    pub scene: Scene,
    /// Similarity dimension of the maps' affine parts.
    pub dimension: Option<f32>,
    /// Lacunarity summary (`trace::lacunarity_summary`).
    pub lacunarity: Option<f32>,
    /// Candidates it took, this one included.
    pub attempts: u32,
    pub rejected: Rejections,
}

/// Nothing met the settings in the budget.
pub struct RollFailure {
    pub attempts: u32,
    pub rejected: Rejections,
    /// The last candidate, framed neutrally, for a caller that must return
    /// *something*.
    pub last: Scene,
}

impl RollFailure {
    pub fn message(&self) -> String {
        format!(
            "no flame met the random settings in {} tries (rejected for {}) — loosen a bound or allow more tries",
            self.attempts,
            self.rejected.describe(),
        )
    }
}

/// Only the tests call this now: the app and `--random` go through
/// [`roll_seeded`], so every roll has a seed it can be had again from.
#[cfg_attr(not(test), allow(dead_code))]
/// Build a random flame with the default settings. Always returns one: if
/// nothing passes the gate in the budget, the last roll comes back with a
/// neutral framing rather than an error the dice can't act on.
pub fn random_flame(rng: &mut impl Rng) -> Scene {
    match roll(&RandomSettings::default(), rng) {
        Ok(r) => r.scene,
        Err(f) => {
            log::warn!("Random flame: {}", f.message());
            f.last
        }
    }
}

/// One roll from a seed: what `--random --seed N` does, and what the dice
/// and the Random flames list do with a seed of their own, so any of them can
/// be had again from the command line. The scene is named after its seed.
pub fn roll_seeded(settings: &RandomSettings, seed: u64) -> Result<Roll, RollFailure> {
    use rand::SeedableRng;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut rolled = roll(settings, &mut rng);
    let name = format!("random-{seed}");
    match &mut rolled {
        Ok(r) => r.scene.name = name,
        Err(f) => f.last.name = name,
    }
    rolled
}

/// Build a random flame that meets `settings`.
pub fn roll(settings: &RandomSettings, rng: &mut impl Rng) -> Result<Roll, RollFailure> {
    let mut rejected = Rejections::default();
    let mut last = None;
    for attempt in 1..=settings.attempts.max(1) {
        let mut c = build_candidate(settings, rng);
        let dimension = match steer_dimension(&mut c.transforms, settings, rng) {
            Ok(d) => d,
            Err(()) => {
                rejected.dimension += 1;
                last = Some(c);
                continue;
            }
        };
        let Some(stats) = measure_candidate(&c.transforms) else {
            rejected.shape += 1;
            last = Some(c);
            continue;
        };
        log::debug!(
            "Random flame attempt {}: r95 {:.3}, spread {:?}, occupancy {:.3} -> {}",
            attempt,
            stats.radius,
            stats.spread,
            stats.occupancy,
            if stats.acceptable() { "accept" } else { "reject" },
        );
        if !stats.acceptable() {
            rejected.shape += 1;
            last = Some(c);
            continue;
        }
        let lacunarity = trace::lacunarity_summary(&c.transforms, &vec![true; c.transforms.len()]);
        if !settings.lacunarity.admits(lacunarity) {
            rejected.lacunarity += 1;
            last = Some(c);
            continue;
        }
        return Ok(Roll {
            scene: finish(c, stats.center, stats.radius, rng),
            dimension,
            lacunarity,
            attempts: attempt,
            rejected,
        });
    }
    let last = last.unwrap_or_else(|| build_candidate(settings, rng));
    Err(RollFailure {
        attempts: settings.attempts.max(1),
        rejected,
        last: finish(last, Vec3::ZERO, 1.0, rng),
    })
}

/// A candidate before quality checking: just the parts the chaos game needs.
struct Candidate {
    transforms: Vec<TransformSpec>,
    names: Vec<Option<String>>,
    colors: Vec<Vec3>,
}

fn build_candidate(settings: &RandomSettings, rng: &mut impl Rng) -> Candidate {
    let n = rng.gen_range(settings.transforms[0]..=settings.transforms[1]).max(1) as usize;

    let mut transforms = Vec::with_capacity(n);
    for _ in 0..n {
        transforms.push(random_transform(settings, rng));
    }

    // An IFS only converges if it contracts on average. Rescale everything if
    // the mean linear contraction is too close to 1.
    let mean: f32 = transforms
        .iter()
        .map(|t| mean_singular_value(t.matrix))
        .sum::<f32>()
        / n as f32;
    if mean > 0.85 {
        rescale(&mut transforms, 0.75 / mean);
    }

    apply_symmetry(&mut transforms, &settings.symmetry, rng);

    // Variations. Purely affine IFS are a real and worthwhile family, but a
    // *random* one is almost always a dull Sierpinski variant — the good ones
    // come from deliberate symmetry, not dice. So by default every roll
    // carries some nonlinearity, and only rarely stays affine.
    //
    // The flame draws a few characteristic kinds, and each map takes its
    // variations from those. Picking kinds per flame (rather than freely per
    // map) is what makes the result read as a coherent form instead of a pile
    // of unrelated distortions; varying which maps carry them, and at what
    // strength, is what keeps it from looking uniform.
    let pool = settings.enabled_slots();
    if !pool.is_empty() && rng.r#gen::<f32>() >= settings.affine {
        let k = (rng.gen_range(settings.kinds[0]..=settings.kinds[1]) as usize).clamp(1, pool.len());
        let kinds = choose(rng, &pool, k);

        let mut any = false;
        for t in transforms.iter_mut() {
            let m = (rng.gen_range(settings.per_map[0]..=settings.per_map[1]) as usize).min(kinds.len());
            for slot in choose(rng, &kinds, m) {
                t.variations[slot] = roll_weight(settings, slot, rng);
                any = true;
            }
        }
        // A flame that rolled kinds but put none of them anywhere is just
        // affine by accident; one map always carries one.
        if !any {
            let idx = rng.gen_range(0..n);
            let slot = kinds[rng.gen_range(0..kinds.len())];
            transforms[idx].variations[slot] = roll_weight(settings, slot, rng);
        }
        // Keep the total blend near 1 so each map's overall scale stays in
        // the range the contraction check just established.
        for t in transforms.iter_mut() {
            let nonlinear: f32 = t.variations[1..].iter().map(|w| w.abs()).sum();
            t.variations[0] = (1.0 - nonlinear).max(0.0);
        }
    }

    // Palette: one base hue plus a scheme, so the colors relate rather than
    // clash. color_value spreads the transforms around the gradient.
    let base_hue: f32 = rng.gen_range(0.0..360.0);
    let scheme = rng.gen_range(0..3);
    let colors: Vec<Vec3> = (0..n)
        .map(|i| {
            let offset = match scheme {
                0 => rng.gen_range(-25.0..25.0),                  // analogous
                1 => if i % 2 == 0 { 0.0 } else { 180.0 },        // complementary
                _ => (i % 3) as f32 * 120.0,                      // triadic
            };
            hsv_to_rgb(
                base_hue + offset,
                rng.gen_range(0.6..1.0),
                rng.gen_range(0.7..1.0),
            )
        })
        .collect();

    for (i, t) in transforms.iter_mut().enumerate() {
        t.color_value = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.5 };
    }

    Candidate {
        transforms,
        names: (0..n).map(|i| Some(format!("t{}", i))).collect(),
        colors,
    }
}

/// Roll a symmetry onto some of the maps, if the settings ask for one.
///
/// The motifs shrink as they join, by `(Σ eᵢ^D)^(-1/D)` over the elements'
/// sizes `eᵢ` (all 1 for a group, a taper for a repeat) with `D = 2.2`: what
/// keeps the effective map set's dimension near where the maps alone had it,
/// instead of sixty full-size copies filling space into a lump the shape gate
/// throws away. Reliquary's hand-sized icosahedral facet is about this size.
/// A dimension bound, if on, then steers from there as usual.
fn apply_symmetry(transforms: &mut [TransformSpec], s: &SymmetrySettings, rng: &mut impl Rng) {
    use crate::symmetry::{OrbitColor, OrbitKind, Repeat, Symmetry};
    if !s.any_kind() || rng.r#gen::<f32>() >= s.chance {
        return;
    }
    let mut kinds = Vec::new();
    let fold = |rng: &mut _| rng_range_u32(rng, s.fold);
    if s.cyclic {
        kinds.push(OrbitKind::Cyclic(fold(rng)));
    }
    if s.dihedral {
        kinds.push(OrbitKind::Dihedral(fold(rng)));
    }
    if s.tetra {
        kinds.push(OrbitKind::Tetrahedral);
    }
    if s.octa {
        kinds.push(OrbitKind::Octahedral);
    }
    if s.icosa {
        kinds.push(OrbitKind::Icosahedral);
    }
    let axis = random_quat(rng) * Vec3::Y;
    if s.repeat {
        // A helix up the axis that tapers: the turn anywhere, the climb a
        // fraction of a unit per copy, so the whole run stays near a frame.
        let count = rng_range_u32(rng, s.repeat_count);
        let climb = rng.gen_range(0.6f32..1.6) / count as f32;
        kinds.push(OrbitKind::Repeat(Repeat {
            count,
            translate: axis * climb,
            turn: rng.gen_range(20.0..160.0),
            scale: rng.gen_range(0.85..0.98),
        }));
    }
    let kind = kinds[rng.gen_range(0..kinds.len())];
    let mirror = kind.is_group() && rng.r#gen::<f32>() < s.mirror;
    let color = if rng.r#gen::<f32>() < s.orbit_color { OrbitColor::Orbit } else { OrbitColor::Shared };
    let Ok(sym) = Symmetry::new(kind, axis, mirror, color) else { return };

    let n = transforms.len();
    let free = (rng_range_u32(rng, s.free) as usize).min(n - 1);
    let order = choose(rng, &(0..n).collect::<Vec<_>>(), n - free);
    const D: f32 = 2.2;
    let k = sym.element_scales().iter().map(|e| e.powf(D)).sum::<f32>().powf(-1.0 / D);
    for i in order {
        let t = &mut transforms[i];
        let (scale, rot, mut p) = t.matrix.to_scale_rotation_translation();
        // A motif on the centre is copied onto itself; push it out to where
        // its copies land apart.
        if p.length() < 0.4 {
            p = if p.length() > 1e-3 { p.normalize() } else { random_quat(rng) * Vec3::X } * 0.4;
        }
        t.matrix = Mat4::from_scale_rotation_translation(scale * k, rot, p);
        t.symmetry = Some(sym.clone());
    }
}

fn rng_range_u32(rng: &mut impl Rng, r: [u32; 2]) -> u32 {
    rng.gen_range(r[0]..=r[1].max(r[0]))
}

/// `k` distinct items of `from`, in random order.
fn choose(rng: &mut impl Rng, from: &[usize], k: usize) -> Vec<usize> {
    let mut v = from.to_vec();
    for i in (1..v.len()).rev() {
        v.swap(i, rng.gen_range(0..=i));
    }
    v.truncate(k);
    v
}

/// A weight for `slot`, from its range, negative at the settings' odds.
fn roll_weight(settings: &RandomSettings, slot: usize, rng: &mut impl Rng) -> f32 {
    let [lo, hi] = settings.variation(slot).weight;
    let w = if hi > lo { rng.gen_range(lo..=hi) } else { lo };
    if rng.r#gen::<f32>() < settings.negative { -w } else { w }
}

/// Scale every map's linear part by `k`, about its own translation.
fn rescale(transforms: &mut [TransformSpec], k: f32) {
    for t in transforms {
        let (s, r, p) = t.matrix.to_scale_rotation_translation();
        t.matrix = Mat4::from_scale_rotation_translation(s * k, r, p);
    }
}

/// Put the similarity dimension inside the settings' bound, if it has one,
/// by rescaling every map together. Returns the dimension it ends on; `Err`
/// when the rescale that would do it leaves a map that barely contracts.
fn steer_dimension(
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

fn random_transform(settings: &RandomSettings, rng: &mut impl Rng) -> TransformSpec {
    // Translation: uniform-ish inside a ball, so transforms don't all pile up
    // near the origin the way independent per-axis ranges would.
    let dir = Vec3::new(
        rng.gen_range(-1.0..1.0),
        rng.gen_range(-1.0..1.0),
        rng.gen_range(-1.0..1.0),
    );
    let dir = if dir.length_squared() < 1e-6 { Vec3::X } else { dir.normalize() };
    let translation = dir * rng.gen_range(0.0f32..0.9).cbrt() * 0.9;

    let rotation = random_quat(rng);

    // Log-uniform so 0.35 and 0.75 are equally likely to be *interesting*,
    // rather than the top of the range dominating.
    let [lo, hi] = settings.scale;
    let base = if hi > lo { rng.gen_range(lo.ln()..=hi.ln()).exp() } else { lo };
    let scale = if rng.r#gen::<f32>() < settings.squash {
        // Squash one axis: this is what turns blobs into sheets and ribbons.
        let mut s = Vec3::splat(base);
        s[rng.gen_range(0..3)] = base * rng.gen_range(0.15..0.6);
        s
    } else {
        Vec3::splat(base)
    };

    let mut variations = [0.0f32; NUM_VARIATIONS];
    variations[0] = 1.0;

    TransformSpec {
        layer: 0,
        matrix: Mat4::from_scale_rotation_translation(scale, rotation, translation),
        post_affine: Mat4::IDENTITY,
        color_value: 0.5,
        weight: rng.gen_range(0.5..2.0),
        color_speed: 0.5,
        explicit_color_speed: None,
        symmetry: None,
        variations,
    }
}

/// Uniformly-distributed random rotation (Shoemake's subgroup method).
fn random_quat(rng: &mut impl Rng) -> Quat {
    let u1: f32 = rng.r#gen();
    let u2: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
    let u3: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
    let (s1, s2) = ((1.0 - u1).sqrt(), u1.sqrt());
    Quat::from_xyzw(s1 * u2.sin(), s1 * u2.cos(), s2 * u3.sin(), s2 * u3.cos())
}

/// Measure a candidate flame. Everything is enabled at this point — a rolled
/// flame has no disabled transforms — so this is just `trace::measure` with
/// that spelled out.
fn measure_candidate(transforms: &[TransformSpec]) -> Option<AttractorStats> {
    trace::measure(transforms, &vec![true; transforms.len()])
}

/// Geometric mean of a matrix's axis lengths — a cheap stand-in for "how much
/// does this map shrink space", good enough for the contraction check.
fn mean_singular_value(m: Mat4) -> f32 {
    let (s, _, _) = m.to_scale_rotation_translation();
    (s.x.abs() * s.y.abs() * s.z.abs()).cbrt()
}

impl AttractorStats {
    fn acceptable(&self) -> bool {
        if !(self.radius.is_finite() && (MIN_RADIUS..=MAX_RADIUS).contains(&self.radius)) {
            return false;
        }
        // A form, not a point or a line: at least two axes with real extent,
        // both in absolute terms and relative to the widest one.
        let mut axes = [self.spread.x, self.spread.y, self.spread.z];
        axes.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        if axes[1] <= MIN_AXIS_SPREAD || axes[0] <= 0.0 {
            return false;
        }
        if axes[1] / axes[0] < MIN_AXIS_RATIO {
            return false;
        }
        // Structure, not a lump.
        self.occupancy <= MAX_OCCUPANCY
    }
}


/// Wrap an accepted candidate up as a full `Scene`, framed for its size.
fn finish(mut c: Candidate, center: Vec3, radius: f32, rng: &mut impl Rng) -> Scene {
    let color_speed = 0.5;
    let color_falloff = if rng.r#gen::<f32>() < 0.4 { rng.gen_range(0.6..1.2) } else { 0.0 };
    resolve_color_speeds(&mut c.transforms, color_speed, color_falloff);
    let colormap = generate_colormap(&c.colors);

    // Which colour source this flame renders through, and — for palette mode —
    // the gradient itself. Rolled before the scene is built so the background
    // can be tinted from whatever will actually be on screen.
    let (color_mode, palette) = random_color_mode(rng);

    let mut scene = Scene {
        subscenes: Vec::new(),
        root_share: 1.0,
        root_visible: true,
        name: format!("random-{}", crate::app::unix_timestamp()),
        author: "fracturize random flame".to_string(),
        // Scale the dot size with the attractor: the same point size that
        // looks dusty on a radius-3 form is a solid mass on a radius-0.3 one.
        point_size: (radius * 0.0018).clamp(0.0008, 0.012),
        points_per_frame: 100_000,
        point_count: 500_000,
        point_count_defaulted: true,
        // A random flame states no lens, so whoever opens it renders it
        // through their own — same as a scene file with no `fov` key.
        camera_fov: crate::camera::Fov::default(),
        camera_fov_authored: false,
        decay: 0.8,
        color_speed,
        color_falloff,
        color_contrast: 1.0,
        // A little haze by default. A freshly rolled flame is a form nobody has
        // seen before, and with no shading and no occlusion there is nothing
        // else telling you which arm is in front — hand-authored scenes can opt
        // in, but a random one has no author to make that call.
        haze: 0.35,
        exposure: 1.0,
        transforms: c.transforms,
        transform_names: c.names,
        colors: c.colors,
        // Filled in below: `set_palette` / `set_color_mode` install the rolled
        // mode and rebuild the colormap from whichever source it selects.
        palette: None,
        color_mode: crate::scene::ColorMode::Transforms,
        colormap,
        camera_focus: center,
        // ~2.4x the 95th-percentile radius fills the frame without clipping
        // the tail. Framing on the raw maximum instead left most flames a
        // speck in the middle of an empty view.
        camera_distance: (radius * 2.4).clamp(0.8, 12.0),
        camera_orientation: crate::rot::Orientation::from_yaw_pitch_roll(
                crate::rot::Angle::ZERO,
                crate::rot::Angle::from_radians(0.3),
                crate::rot::Angle::ZERO,
            ),
        // Replaced below, once the colours it has to sit behind are settled.
        background: crate::scene::DEFAULT_BACKGROUND,
        camera_path: None::<CameraPath>,
        // Not rolled at random: infinite zoom needs a map that contracts on
        // every axis, and the generator has no reason to guarantee one.
        // `--zoom <n>` turns it on for a roll worth keeping.
        zoom: None,
    };

    match palette {
        Some(p) => scene.set_palette(p),
        None => scene.set_color_mode(color_mode),
    }
    let tint = tint(&scene, rng);
    scene.background = random_background(rng, tint);
    scene
}

/// Odds of each colour source. `transforms` keeps the majority because it is
/// the mode the per-transform colours were rolled *for* — the ring spreads
/// them around the gradient by `color_value` — but the other two are just as
/// much a part of what this renderer does, and a dice button that can only
/// produce one third of the palette space is under-selling the instrument.
const P_MIX: f32 = 0.25;
const P_PALETTE: f32 = 0.25;

/// Roll a colour source, plus the gradient if the roll wants one.
///
/// Palette mode is the only one that needs anything built: `mix` reads the
/// per-transform RGBs the candidate already has, and `transforms` is what
/// those RGBs were made for.
fn random_color_mode(rng: &mut impl Rng) -> (crate::scene::ColorMode, Option<crate::palette::Palette>) {
    use crate::scene::ColorMode;
    let roll: f32 = rng.r#gen();
    if roll < P_PALETTE {
        (ColorMode::Palette, Some(crate::palette::random::palette(rng)))
    } else if roll < P_PALETTE + P_MIX {
        (ColorMode::Mix, None)
    } else {
        (ColorMode::Transforms, None)
    }
}

/// A colour the flame actually renders in, to hang the background off.
///
/// Sampled rather than averaged: the mean of a complementary or triadic
/// scheme is mud, and a background tinted from mud is just grey. One real
/// colour out of the set keeps the background in the flame's family.
fn tint(scene: &Scene, rng: &mut impl Rng) -> Vec3 {
    match scene.color_mode {
        crate::scene::ColorMode::Palette => scene
            .palette
            .as_ref()
            .map(|p| p.sample(rng.r#gen()))
            .unwrap_or(Vec3::ONE),
        _ if scene.colors.is_empty() => Vec3::ONE,
        _ => scene.colors[rng.gen_range(0..scene.colors.len())],
    }
}

/// How often the background comes out dark.
const P_DARK_BACKGROUND: f32 = 2.0 / 3.0;
/// Brightest channel a dark background may reach, linear. The old fixed
/// default sat at 0.05 on its blue channel, so this is the band that has
/// always read as "the fractal is in the dark".
const DARK_BACKGROUND_LEVEL: f32 = 0.06;
/// And the band for the other third. Stops well short of white on purpose:
/// the render composites the flame *over* the background by coverage, so
/// contrast is the gap between the two, and rolled flame colours are bright
/// (value 0.7–1.0). A paper-white ground would swallow them.
const LIGHT_BACKGROUND_LEVEL: std::ops::Range<f32> = 0.06..0.35;

/// Roll a background that belongs behind `tint`.
///
/// Hue comes from the flame rather than from nowhere, so the two agree; the
/// level and the chroma are what make it a *ground* rather than another
/// element — dark and saturated, or mid and nearly neutral.
fn random_background(rng: &mut impl Rng, tint: Vec3) -> Vec3 {
    // Mostly the flame's own hue family, which reads as one scene; sometimes
    // its opposite, which makes the form pop off the ground. Rotating in
    // Oklab keeps the shift a hue shift rather than a lightness one.
    let shift = if rng.r#gen::<f32>() < 0.3 {
        rng.gen_range(150.0..210.0)
    } else {
        rng.gen_range(-40.0f32..40.0)
    };
    let hue = crate::palette::random::rotate_hue(tint, shift).max(Vec3::ZERO);

    // Normalise to the brightest channel so `level` means what it says
    // whatever colour came in. A colour with no chroma to keep (or one the
    // rotation pushed out of gamut) falls back to neutral.
    let peak = hue.max_element();
    let hue = if peak > 1e-4 { hue / peak } else { Vec3::ONE };

    let (level, chroma) = if rng.r#gen::<f32>() < P_DARK_BACKGROUND {
        // Deep and tinted: at this level chroma costs nothing legibility-wise,
        // and a near-black that is *some* colour beats one that is grey.
        (rng.gen_range(0.004..DARK_BACKGROUND_LEVEL), rng.gen_range(0.35..1.0))
    } else {
        // Mid and mostly neutral: a saturated light ground competes with the
        // flame for the eye, and loses interestingly to nothing.
        (rng.gen_range(LIGHT_BACKGROUND_LEVEL), rng.gen_range(0.08..0.4))
    };

    (Vec3::ONE.lerp(hue, chroma) * level).clamp(Vec3::ZERO, Vec3::ONE)
}

/// HSV (h in degrees, s/v in 0-1) to linear RGB (0-1). Mirrors the private
/// helper in app.rs; kept local so this module stays free of UI deps.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Vec3 {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Vec3::new(r, g, b) + Vec3::splat(v - c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    /// The generator's whole job is to not hand back garbage. Roll a batch
    /// across different seeds and hold every one to the same bar the gate
    /// applies — no panics, no NaN, no collapsed or exploded attractors.
    #[test]
    fn random_flames_are_renderable() {
        for seed in 0..12u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let scene = random_flame(&mut rng);

            assert!(
                (2..=5).contains(&scene.transforms.len()),
                "seed {}: transform count {} out of range",
                seed,
                scene.transforms.len()
            );
            assert_eq!(scene.colors.len(), scene.transforms.len());
            assert_eq!(scene.transform_names.len(), scene.transforms.len());
            assert!(scene.point_size > 0.0 && scene.point_size.is_finite());
            assert!(scene.camera_distance.is_finite() && scene.camera_distance > 0.0);

            for t in &scene.transforms {
                assert!(
                    t.matrix.to_cols_array().iter().all(|v| v.is_finite()),
                    "seed {}: non-finite matrix",
                    seed
                );
                assert!(t.weight > 0.0, "seed {}: non-positive weight", seed);
            }

            let stats = measure_candidate(&scene.transforms)
                .unwrap_or_else(|| panic!("seed {}: no walker could be built", seed));
            assert!(
                stats.acceptable(),
                "seed {}: attractor radius {:.3}, spread {:?} — the gate let a bad flame through",
                seed,
                stats.radius,
                stats.spread
            );
        }
    }

    /// The background is a scene parameter the renderer clears to and the
    /// haze fades toward, so a non-finite or out-of-range roll is a bad
    /// frame, not a bad colour.
    #[test]
    fn backgrounds_are_in_range_and_mostly_dark() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let mut dark = 0;
        const ROLLS: usize = 600;
        for _ in 0..ROLLS {
            let tint = hsv_to_rgb(rng.gen_range(0.0..360.0), 0.8, 0.9);
            let bg = random_background(&mut rng, tint);
            assert!(
                bg.to_array().iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c)),
                "background {:?} out of range",
                bg
            );
            if bg.max_element() <= DARK_BACKGROUND_LEVEL {
                dark += 1;
            }
        }
        // 2/3, with room for the binomial spread at this sample count.
        let ratio = dark as f32 / ROLLS as f32;
        assert!((0.60..0.73).contains(&ratio), "dark background ratio {:.3}", ratio);
    }

    /// All three colour sources should show up on the dice, and a palette-mode
    /// flame must actually carry a palette — the mode without one renders
    /// through a fallback, which is a bug the generator shouldn't ship.
    #[test]
    fn all_color_modes_are_rolled_and_consistent() {
        use crate::scene::ColorMode;
        let mut rng = rand::rngs::StdRng::seed_from_u64(3);
        let mut seen = [0usize; 3];
        for _ in 0..300 {
            let (mode, palette) = random_color_mode(&mut rng);
            match mode {
                ColorMode::Transforms => seen[0] += 1,
                ColorMode::Palette => seen[1] += 1,
                ColorMode::Mix => seen[2] += 1,
            }
            assert_eq!(
                palette.is_some(),
                mode == ColorMode::Palette,
                "{:?} rolled the wrong palette state",
                mode
            );
        }
        assert!(seen.iter().all(|&n| n > 0), "not every colour mode was rolled: {:?}", seen);
    }

    /// Whatever mode a rolled flame lands in, it has to be renderable in it.
    #[test]
    fn rolled_flames_carry_their_color_mode() {
        for seed in 0..8u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let scene = random_flame(&mut rng);
            if scene.color_mode == crate::scene::ColorMode::Palette {
                assert!(scene.palette.is_some(), "seed {}: palette mode with no palette", seed);
            }
            assert!(
                scene.colormap.iter().flatten().all(|c| c.is_finite()),
                "seed {}: non-finite colormap",
                seed
            );
            assert!(
                scene.background.to_array().iter().all(|c| (0.0..=1.0).contains(c)),
                "seed {}: background {:?} out of range",
                seed,
                scene.background
            );
        }
    }

    /// `--random --seed n` promises the same flame twice. The colour rolls
    /// have to come off the seeded rng like everything else for that to hold.
    #[test]
    fn the_same_seed_gives_the_same_colours() {
        for seed in 0..4u64 {
            let a = random_flame(&mut rand::rngs::StdRng::seed_from_u64(seed));
            let b = random_flame(&mut rand::rngs::StdRng::seed_from_u64(seed));
            assert_eq!(a.color_mode, b.color_mode, "seed {}: mode not reproducible", seed);
            assert_eq!(a.background, b.background, "seed {}: background not reproducible", seed);
            assert_eq!(a.color_falloff, b.color_falloff, "seed {}: falloff not reproducible", seed);
        }
    }

    #[test]
    fn spherical_is_never_rolled() {
        // Guard the AGENTS.md rule: spherical's 1/r^2 inversion blows scenes
        // out, so it must never appear in a rolled flame.
        let slot = VARIATION_NAMES.iter().position(|&v| v == "spherical").unwrap();
        for seed in 0..24u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let scene = random_flame(&mut rng);
            for t in &scene.transforms {
                assert_eq!(t.variations[slot], 0.0, "seed {}: rolled spherical", seed);
            }
        }
    }

    /// Only enabled variations are rolled, and only at their weights.
    #[test]
    fn rolls_keep_to_the_variation_pool_and_ranges() {
        let mut settings = RandomSettings::default();
        for v in settings.variations.values_mut() {
            v.enabled = false;
        }
        settings.variations.insert("julia".into(), VariationRange { enabled: true, weight: [0.3, 0.4] });
        settings.variations.insert("absfold".into(), VariationRange { enabled: true, weight: [0.2, 0.25] });
        settings.affine = 0.0;
        settings.negative = 0.0;
        settings.transforms = [3, 4];
        settings.per_map = [1, 2];
        let settings = settings.normalized();
        let julia = VARIATION_NAMES.iter().position(|&v| v == "julia").unwrap();
        let absfold = VARIATION_NAMES.iter().position(|&v| v == "absfold").unwrap();
        let mut rolled = 0;
        for seed in 0..10u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let Ok(r) = roll(&settings, &mut rng) else { continue };
            rolled += 1;
            assert!((3..=4).contains(&r.scene.transforms.len()), "seed {seed}: count");
            for t in &r.scene.transforms {
                for (slot, &w) in t.variations.iter().enumerate().skip(1) {
                    match slot {
                        s if s == julia => assert!(w == 0.0 || (0.3..=0.4).contains(&w)),
                        s if s == absfold => assert!(w == 0.0 || (0.2..=0.25).contains(&w)),
                        _ => assert_eq!(w, 0.0, "seed {seed}: rolled {}", VARIATION_NAMES[slot]),
                    }
                }
                // Each map takes at least one variation.
                assert!(t.variations[1..].iter().any(|&w| w != 0.0), "seed {seed}: bare map");
            }
        }
        assert!(rolled >= 5, "only {rolled} of 10 rolls passed");
    }

    /// Dimension is steered rather than rejected, so even a narrow bound is
    /// met nearly every time.
    #[test]
    fn dimension_bounds_are_met() {
        for range in [[1.2, 1.4], [2.2, 2.4], [2.8, 3.0]] {
            let settings = RandomSettings {
                dimension: Bound { on: true, range },
                ..Default::default()
            };
            let mut met = 0;
            for seed in 0..12u64 {
                let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
                if let Ok(r) = roll(&settings, &mut rng) {
                    let d = similarity_dimension(&r.scene.transforms).unwrap();
                    assert!(
                        d >= range[0] - 2e-3 && d <= range[1] + 2e-3,
                        "seed {seed}: d {d} outside {range:?}"
                    );
                    met += 1;
                }
            }
            assert!(met >= 10, "{range:?}: only {met} of 12 met the bound");
        }
    }

    /// Lacunarity is rejection-sampled: whatever comes back is inside, and a
    /// bound nothing can meet fails with the reason counted.
    #[test]
    fn lacunarity_bounds_are_met_or_reported() {
        let settings = RandomSettings {
            lacunarity: Bound { on: true, range: [3.0, 30.0] },
            ..Default::default()
        };
        for seed in 0..6u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            if let Ok(r) = roll(&settings, &mut rng) {
                let l = r.lacunarity.unwrap();
                assert!((3.0..=30.0).contains(&l), "seed {seed}: lacunarity {l}");
            }
        }
        let impossible = RandomSettings {
            lacunarity: Bound { on: true, range: [900.0, 1000.0] },
            attempts: 8,
            ..Default::default()
        };
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let Err(f) = roll(&impossible, &mut rng) else { panic!("met an impossible bound") };
        assert_eq!(f.attempts, 8);
        assert_eq!(f.rejected.total(), 8);
        assert!(f.rejected.lacunarity > 0, "{:?}", f.rejected);
    }

    #[test]
    fn settings_read_bare_or_from_prefs() {
        let bare = "transforms = [6, 3]\n[variations.spherical]\nenabled = true\n";
        let s = RandomSettings::from_toml(bare).unwrap();
        assert_eq!(s.transforms, [3, 6], "range put in order");
        assert!(s.variation(2).enabled, "spherical switched on");
        assert!(s.variation(3).enabled, "swirl kept its default");
        assert_eq!(s.variations.len(), NUM_VARIATIONS - 1);

        let prefs = "invert_pitch = true\n[random]\nattempts = 5\n[random.lacunarity]\non = true\nrange = [4.0, 2.0]\n";
        let s = RandomSettings::from_toml(prefs).unwrap();
        assert_eq!(s.attempts, 5);
        assert!(s.lacunarity.on);
        assert_eq!(s.lacunarity.range, [2.0, 4.0]);
        assert!(RandomSettings::from_toml("transforms = \"many\"").is_err());
    }

    /// Not a correctness test: how dimension and lacunarity fall across
    /// default rolls, which is what says whether a bound is improbable.
    /// `cargo test --release random_flame_study -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement; prints a distribution"]
    fn random_flame_study() {
        let settings = RandomSettings::default();
        let mut ds = Vec::new();
        let mut ls = Vec::new();
        let mut tries = 0;
        let t = std::time::Instant::now();
        for seed in 0..200u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            if let Ok(r) = roll(&settings, &mut rng) {
                tries += r.attempts;
                ds.extend(r.dimension);
                ls.extend(r.lacunarity);
            }
        }
        let pct = |v: &mut Vec<f32>, p: f32| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[((v.len() - 1) as f32 * p) as usize]
        };
        println!("{} rolls, {} candidates, {:.1} ms/roll", ls.len(), tries, t.elapsed().as_secs_f64() * 1e3 / 200.0);
        for p in [0.05, 0.25, 0.5, 0.75, 0.95] {
            println!("p{:02}: d {:.2}  lacunarity {:.2}", (p * 100.0) as u32, pct(&mut ds, p), pct(&mut ls, p));
        }
    }


    fn symmetric() -> RandomSettings {
        RandomSettings {
            symmetry: SymmetrySettings { chance: 1.0, repeat: true, ..Default::default() },
            ..Default::default()
        }
    }

    /// At chance 1 every roll that passes is symmetric, keeps a map outside
    /// the group, and every motif shares one group.
    #[test]
    fn symmetric_rolls_have_a_group_and_a_defect() {
        let settings = symmetric();
        let mut rolled = 0;
        for seed in 0..24u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let Ok(r) = roll(&settings, &mut rng) else { continue };
            rolled += 1;
            let ts = &r.scene.transforms;
            let motifs: Vec<_> = ts.iter().filter_map(|t| t.symmetry.as_ref()).collect();
            assert!(!motifs.is_empty(), "seed {seed}: no group");
            assert_eq!(ts.len() - motifs.len(), 1, "seed {seed}: one map stays outside");
            assert!(motifs.iter().all(|g| *g == motifs[0]), "seed {seed}: two groups");
            // And it survives a save: the group is regrouped into a block and
            // read back onto the same motifs.
            let text = r.scene.to_toml_string().unwrap();
            let path = std::env::temp_dir().join(format!("fracturize-symroll-{}-{seed}.toml", std::process::id()));
            std::fs::write(&path, &text).unwrap();
            let back = Scene::load(&path).unwrap_or_else(|e| panic!("seed {seed}: {e}\n{text}"));
            std::fs::remove_file(&path).ok();
            assert_eq!(
                back.transforms.iter().filter(|t| t.symmetry.is_some()).count(),
                motifs.len(),
                "seed {seed}: symmetry lost in a save"
            );
        }
        assert!(rolled >= 16, "only {rolled} of 24 symmetric rolls passed");
    }

    /// Steering has to count the group's copies, or it lands somewhere else.
    #[test]
    fn dimension_steering_counts_the_group() {
        let mut settings = symmetric();
        settings.dimension = Bound { on: true, range: [2.0, 2.1] };
        let mut met = 0;
        for seed in 0..12u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            if let Ok(r) = roll(&settings, &mut rng) {
                let d = similarity_dimension(&r.scene.transforms).unwrap();
                assert!((1.998..=2.102).contains(&d), "seed {seed}: d {d}");
                met += 1;
            }
        }
        assert!(met >= 8, "only {met} of 12");
    }

    /// How often symmetric rolls pass, and what they cost.
    /// `cargo test --release symmetry_study -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement; prints a distribution"]
    fn symmetry_study() {
        let settings = symmetric();
        let (mut ok, mut tries, mut ds, mut ls) = (0, 0, Vec::new(), Vec::new());
        let t = std::time::Instant::now();
        for seed in 0..200u64 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            match roll(&settings, &mut rng) {
                Ok(r) => {
                    ok += 1;
                    tries += r.attempts;
                    ds.extend(r.dimension);
                    ls.extend(r.lacunarity);
                }
                Err(f) => println!("seed {seed}: {}", f.message()),
            }
        }
        ds.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ls.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "{ok}/200 rolled, {:.2} tries each, {:.1} ms/roll; d median {:.2}, lacunarity median {:.1}",
            tries as f32 / ok as f32,
            t.elapsed().as_secs_f64() * 1e3 / 200.0,
            ds[ds.len() / 2],
            ls[ls.len() / 2],
        );
    }

}
