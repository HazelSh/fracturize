//! What the dice may do: [`RandomSettings`], one profile read by a fresh roll
//! and by a mutation alike.
//!
//! The roll half says what a flame may be made of — maps, variations and their
//! weight ranges, symmetry, bounds on the attractor. The [`MutateSettings`]
//! half says what a mutation may change, and how hard. They are one struct
//! because a mutation that *re-rolls* a part draws it from the roll half: "give
//! map 2 new variations" is the same draw over the same enabled list and
//! ranges a fresh roll makes, so the two can't drift apart.
//!
//! How a profile is written to and read from a file is `super::profile`'s job.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::scene::{NUM_VARIATIONS, VARIATION_NAMES};

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
    pub(super) fn off(range: [f32; 2]) -> Self {
        Self { on: false, range }
    }

    /// Whether `v` satisfies the bound. Off passes everything; on, a value
    /// that couldn't be measured fails.
    pub fn admits(&self, v: Option<f32>) -> bool {
        !self.on || v.is_some_and(|v| v >= self.range[0] && v <= self.range[1])
    }

    /// How far outside the bound `v` is: 0 inside it (or with the bound off),
    /// infinite for a value that couldn't be measured. With `log`, measured
    /// as a ratio, for quantities that range over decades.
    pub fn excess(&self, v: Option<f32>, log: bool) -> f32 {
        if !self.on {
            return 0.0;
        }
        let Some(v) = v.filter(|v| v.is_finite()) else { return f32::INFINITY };
        let [lo, hi] = self.range;
        let (below, above) = if log {
            ((lo.max(1e-9) / v.max(1e-9)).ln(), (v.max(1e-9) / hi.max(1e-9)).ln())
        } else {
            (lo - v, v - hi)
        };
        below.max(above).max(0.0)
    }
}

impl Default for Bound {
    fn default() -> Self {
        Self::off([0.0, 1.0])
    }
}

/// How often a mutation touches one part of a flame, and how.
///
/// A touched part is **nudged** — what is there perturbed, scaled by the
/// strength — or, at the `reroll` chance, **re-rolled**: replaced by the very
/// draw a fresh roll makes, under the same ranges.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Part {
    /// Relative odds of touching this part, against the other parts' odds.
    /// 0 leaves it alone — a profile with every part but one at 0 is a mask.
    pub odds: f32,
    /// Chance a touch re-rolls the part rather than nudging it.
    pub reroll: f32,
}

impl Part {
    const fn new(odds: f32, reroll: f32) -> Self {
        Self { odds, reroll }
    }
}

impl Default for Part {
    fn default() -> Self {
        Self::new(0.0, 0.0)
    }
}

/// Every part a mutation can touch. The odds are relative, so they needn't
/// add up to anything.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Parts {
    /// A map's position.
    pub translate: Part,
    /// A map's turn, about its own origin.
    pub rotate: Part,
    /// A map's size. Re-rolled, it takes a size from the roll's scale range.
    pub scale: Part,
    /// A map's chaos-game weight.
    pub weight: Part,
    /// A map's variation blend. A nudge adjusts one weight or brings in one
    /// variation from the enabled list; a re-roll replaces the map's blend
    /// the way a fresh roll makes one.
    pub variations: Part,
    /// A map's colour, or — in palette mode — the gradient.
    pub color: Part,
    /// Where a map sits on the gradient.
    pub color_value: Part,
    /// A new map: a nudged copy of one, or (re-rolled) a fresh one. Never past
    /// the roll's most maps.
    pub add: Part,
    /// A map deleted. Never below the roll's fewest maps.
    pub remove: Part,
    /// The background. Re-rolled, it is drawn from the flame's colours the way
    /// a fresh roll's is.
    pub background: Part,
    /// The symmetry group: a nudge tilts its axis, a re-roll draws another
    /// group from the roll's symmetry settings (or gives the flame one).
    pub symmetry: Part,
}

impl Parts {
    /// The parts by name, in the order they're written and shown.
    pub const NAMES: [&'static str; 11] = [
        "translate",
        "rotate",
        "scale",
        "weight",
        "variations",
        "color",
        "color_value",
        "add",
        "remove",
        "background",
        "symmetry",
    ];

    pub fn all(&self) -> [Part; 11] {
        [
            self.translate,
            self.rotate,
            self.scale,
            self.weight,
            self.variations,
            self.color,
            self.color_value,
            self.add,
            self.remove,
            self.background,
            self.symmetry,
        ]
    }

    pub fn all_mut(&mut self) -> [&mut Part; 11] {
        [
            &mut self.translate,
            &mut self.rotate,
            &mut self.scale,
            &mut self.weight,
            &mut self.variations,
            &mut self.color,
            &mut self.color_value,
            &mut self.add,
            &mut self.remove,
            &mut self.background,
            &mut self.symmetry,
        ]
    }
}

impl Default for Parts {
    /// The odds `U` always had, with re-rolls added where a fresh draw is a
    /// useful kind of change. Background and symmetry are off: a mutation
    /// changing the ground or the group is a bigger step than the rest, and
    /// one to ask for.
    fn default() -> Self {
        Self {
            translate: Part::new(0.18, 0.0),
            rotate: Part::new(0.15, 0.0),
            scale: Part::new(0.15, 0.0),
            weight: Part::new(0.12, 0.0),
            variations: Part::new(0.18, 0.15),
            color: Part::new(0.09, 0.1),
            color_value: Part::new(0.05, 0.0),
            add: Part::new(0.05, 0.3),
            remove: Part::new(0.03, 0.0),
            background: Part::new(0.0, 0.5),
            symmetry: Part::new(0.0, 0.5),
        }
    }
}

/// What a mutation may change, and how hard. The half of a profile `U`, the
/// Mutate window and `--mutations` read.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct MutateSettings {
    /// Scales every nudge: 1 is a noticeable but usually non-destructive step.
    pub strength: f32,
    /// Operators one mutation applies, each to a part drawn by the odds.
    pub operators: [u32; 2],
    /// Halve the strength after this many rejected tries in a row (0 = never),
    /// down to a sixteenth — so a strong setting on a delicate scene still
    /// finds something close instead of nothing.
    pub backoff: u32,
    /// Rescale the child onto the dimension bound, as a fresh roll is. Off by
    /// default: it rescales every map, a bigger change than the mutation.
    pub steer_dimension: bool,
    pub parts: Parts,
}

impl Default for MutateSettings {
    fn default() -> Self {
        Self {
            strength: 1.0,
            operators: [1, 3],
            backoff: 4,
            steer_dimension: false,
            parts: Parts::default(),
        }
    }
}

/// A profile: what the dice may roll, and what a mutation may change.
/// Persisted to prefs as `[random]`, edited in the Random settings window,
/// saved and loaded as named profiles (`super::profile`), and readable by
/// `--random-config`.
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
    /// Candidates tried per flame (or per mutation) before giving up.
    pub attempts: u32,
    /// What a mutation may change.
    pub mutate: MutateSettings,
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
            mutate: MutateSettings::default(),
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
pub const STRENGTH_LIMITS: [f32; 2] = [0.1, 3.0];
pub const MAX_OPERATORS: u32 = 8;
/// Past this a part's odds stop meaning anything the others can answer.
pub const ODDS_LIMIT: f32 = 10.0;

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
        let m = &mut self.mutate;
        m.strength = if m.strength.is_finite() { m.strength.clamp(STRENGTH_LIMITS[0], STRENGTH_LIMITS[1]) } else { 1.0 };
        m.operators = ordered(m.operators, 1, MAX_OPERATORS);
        m.backoff = m.backoff.min(MAX_ATTEMPTS);
        for p in m.parts.all_mut() {
            p.odds = if p.odds.is_finite() { p.odds.clamp(0.0, ODDS_LIMIT) } else { 0.0 };
            p.reroll = if p.reroll.is_finite() { p.reroll.clamp(0.0, 1.0) } else { 0.0 };
        }
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

    /// Whether `other` makes the same fresh rolls: everything but the mutate
    /// half. The Random flames list starts over only when this changes.
    pub fn same_roll(&self, other: &Self) -> bool {
        Self { mutate: other.mutate, ..self.clone() } == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excess_is_zero_inside_and_grows_outside() {
        let b = Bound { on: true, range: [1.0, 2.0] };
        assert_eq!(b.excess(Some(1.5), false), 0.0);
        assert!((b.excess(Some(2.5), false) - 0.5).abs() < 1e-6);
        assert!((b.excess(Some(0.5), true) - 2f32.ln()).abs() < 1e-6);
        assert_eq!(b.excess(None, false), f32::INFINITY);
        assert_eq!(Bound::off([1.0, 2.0]).excess(None, false), 0.0);
    }

    #[test]
    fn the_mutate_half_is_normalized_too() {
        let mut s = RandomSettings::default();
        s.mutate.strength = 40.0;
        s.mutate.operators = [5, 2];
        s.mutate.parts.rotate.odds = -1.0;
        s.mutate.parts.color.reroll = f32::NAN;
        let s = s.normalized();
        assert_eq!(s.mutate.strength, STRENGTH_LIMITS[1]);
        assert_eq!(s.mutate.operators, [2, 5]);
        assert_eq!(s.mutate.parts.rotate.odds, 0.0);
        assert_eq!(s.mutate.parts.color.reroll, 0.0);
    }

    #[test]
    fn the_roll_ignores_the_mutate_half() {
        let a = RandomSettings::default();
        let mut b = a.clone();
        b.mutate.strength = 2.0;
        assert!(a.same_roll(&b));
        b.transforms = [3, 3];
        assert!(!a.same_roll(&b));
    }

    #[test]
    fn parts_are_named_in_field_order() {
        let mut p = Parts::default();
        *p.all_mut()[4] = Part { odds: 9.0, reroll: 0.5 };
        assert_eq!(p.variations, Part { odds: 9.0, reroll: 0.5 });
        assert_eq!(Parts::NAMES[4], "variations");
        // And the names are the serialized keys, which the file format uses.
        let t = toml::Value::try_from(p).unwrap();
        let keys: Vec<_> = t.as_table().unwrap().keys().cloned().collect();
        let mut names: Vec<_> = Parts::NAMES.iter().map(|s| s.to_string()).collect();
        names.sort();
        assert_eq!(keys, names);
    }
}
