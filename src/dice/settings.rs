//! What the dice may do: [`RandomSettings`], one profile read by a roll and by
//! a mutation alike.
//!
//! It is a list of [`Parts`] — the things a flame is made of — each switched
//! on or off, and the ranges each is drawn from. A roll draws every part
//! that's on; a mutation changes a few of them. A part that's off is never
//! touched by either: the base scene's own is kept. There is no roll half and
//! mutate half, because there is nothing either could say that the other
//! should not mean the same way. The only settings one action reads and the
//! other doesn't are [`MutationSettings`] — how big a step — and the parts'
//! odds, which say which of them a mutation picks, since a roll takes all.
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

/// Whether, and how, a rolled flame says which map may follow which
/// (`crate::xaos`).
///
/// A roll draws one of three kinds of graph, by these odds: **none**, a
/// **preset** from the enabled ones, or a **random** graph. Off by default —
/// `default` rolls no xaos at all and draws nothing for it, so every recorded
/// seed still rolls what it always did.
///
/// Only cuts change a flame's shape; a multiplier other than 0 only moves
/// density, like a weight. So a random graph is mostly cuts, and always
/// **strongly connected**: it keeps a random cycle through every map and cuts
/// only edges off it, so no map is shut out and none is a trap. `cycle` is not
/// among the presets on purpose: one exit per map makes the walk
/// deterministic, and the picture collapses to as many dots as there are maps.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct XaosSettings {
    /// Odds of no xaos, against `preset` and `random`.
    pub off: f32,
    /// Odds of a preset from the enabled ones.
    pub preset: f32,
    /// Odds of a random graph.
    pub random: f32,
    /// The `no-self-loops` preset: no map follows itself. Needs three maps;
    /// with two it is a strict alternation, which collapses.
    pub no_self_loops: bool,
    /// The `blocks:N` preset: the maps split into closed groups, each its own
    /// attractor, dealt walkers by chance. Needs two maps a block.
    pub blocks: bool,
    /// Random graphs: the share of edges off the kept cycle that are cut.
    pub cut: [f32; 2],
    /// Random graphs: chance every self-loop is cut (with three maps or more).
    pub self_loops: f32,
    /// Random graphs: chance a kept edge gets a multiplier other than 1.
    pub weighted: f32,
    /// The range such a multiplier is drawn from, log-uniformly.
    pub multiplier: [f32; 2],
    /// The quality check: least share of the walk a map may get under xaos,
    /// or its share of the weights if that is less already. Below it the map
    /// is starved, and draws next to nothing.
    pub min_share: f32,
}

impl Default for XaosSettings {
    fn default() -> Self {
        Self {
            off: 1.0,
            preset: 0.0,
            random: 0.0,
            no_self_loops: true,
            blocks: true,
            cut: [0.2, 0.5],
            self_loops: 0.3,
            weighted: 0.15,
            multiplier: [0.3, 3.0],
            min_share: 0.02,
        }
    }
}

impl XaosSettings {
    /// Whether a roll can draw any xaos at all. With none, a roll draws
    /// nothing for it — not even the die that would say "none".
    pub fn draws(&self) -> bool {
        self.preset > 0.0 || self.random > 0.0
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

/// One part of a flame the dice can make: whether they may, and — for a
/// mutation, which changes only a few parts at a time — how often it is the
/// one changed and how.
///
/// **Its odds are the whole of what the dice may do with it, for a roll and a
/// mutation alike.** Above zero it is on: a roll draws it, and a mutation
/// picks it that often against the other parts. At zero it is off: a roll
/// keeps the base scene's own, and a mutation never picks it. There is no
/// separate switch, so "on but never picked" can't be said two ways — the
/// least a part that's on can have is [`MIN_ODDS`], which is "rolled, and a
/// mutation all but never touches it". So a mask is a profile with some odds
/// at zero, and it means the same thing whichever button is pressed.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Part {
    /// 0 is off. Otherwise how often a mutation picks this part, against the
    /// other parts' odds; never below [`MIN_ODDS`].
    pub odds: f32,
    /// Chance a mutation replaces the part outright — the very draw a roll
    /// makes, under the same ranges — rather than nudging what's there.
    pub reroll: f32,
}

impl Part {
    const fn new(odds: f32, reroll: f32) -> Self {
        Self { odds, reroll }
    }

    /// Whether the dice may make it: odds above zero.
    pub fn on(&self) -> bool {
        self.odds > 0.0
    }

    /// Switch it on (at the least odds, if it was off) or off (odds 0).
    pub fn set_on(&mut self, on: bool) {
        if on != self.on() {
            self.odds = if on { MIN_ODDS } else { 0.0 };
        }
    }
}

impl Default for Part {
    fn default() -> Self {
        Self::new(0.0, 0.0)
    }
}

/// Every part of a flame the dice can make.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Parts {
    /// Each transform's position.
    pub translate: Part,
    /// Each transform's turn, about its own origin.
    pub rotate: Part,
    /// Each transform's size, from the scale range.
    pub scale: Part,
    /// Each transform's chaos-game weight.
    pub weight: Part,
    /// Each transform's variation blend, from the enabled list.
    pub variations: Part,
    /// How many transforms: a roll draws it from the range, a mutation adds
    /// one (a changed copy, or re-rolled a new one) or removes one, within it.
    pub count: Part,
    /// The symmetry group, from the symmetry settings.
    pub symmetry: Part,
    /// The colours: each transform's, the colour source and the gradient.
    pub color: Part,
    /// Where each transform sits on the gradient.
    pub color_value: Part,
    /// The background, drawn from the flame's colours.
    pub background: Part,
    /// Which map may follow which (xaos), from the xaos settings. Last in the
    /// list only because it came last: a mutation picks a part by walking
    /// the list, so putting it anywhere else would change what every
    /// recorded mutation seed picks. Shown and written with the structure.
    pub xaos: Part,
}

impl Parts {
    /// The parts by name, in the order they're written and shown.
    pub const NAMES: [&'static str; 11] = [
        "translate",
        "rotate",
        "scale",
        "weight",
        "variations",
        "count",
        "symmetry",
        "color",
        "color_value",
        "background",
        "xaos",
    ];

    /// The parts that make the form — what the gate measures and the camera
    /// frames. Indices into [`Self::NAMES`]; the rest are colour.
    pub const FORM: [usize; 8] = [0, 1, 2, 3, 4, 5, 6, 10];

    /// The order parts are written in a file and shown in a list: the
    /// structure together, then the colour.
    pub const ORDER: [usize; 11] = [0, 1, 2, 3, 4, 5, 6, 10, 7, 8, 9];

    pub fn all(&self) -> [Part; 11] {
        [
            self.translate,
            self.rotate,
            self.scale,
            self.weight,
            self.variations,
            self.count,
            self.symmetry,
            self.color,
            self.color_value,
            self.background,
            self.xaos,
        ]
    }

    pub fn all_mut(&mut self) -> [&mut Part; 11] {
        [
            &mut self.translate,
            &mut self.rotate,
            &mut self.scale,
            &mut self.weight,
            &mut self.variations,
            &mut self.count,
            &mut self.symmetry,
            &mut self.color,
            &mut self.color_value,
            &mut self.background,
            &mut self.xaos,
        ]
    }

    /// Every part of the form is on: a roll owes nothing to its base scene's
    /// form, and is judged by the bounds alone.
    pub fn whole_form(&self) -> bool {
        let all = self.all();
        Self::FORM.iter().all(|&i| all[i].on())
    }

    /// No part of the form is on: the form is the base scene's, untouched.
    pub fn no_form(&self) -> bool {
        let all = self.all();
        Self::FORM.iter().all(|&i| !all[i].on())
    }
}

impl Default for Parts {
    /// Every part on, so a roll owes nothing to the scene it was rolled from.
    /// The odds are the ones `U` always had, with re-rolls where a fresh draw
    /// is a useful kind of change. The background and the symmetry group are
    /// at the least odds: a roll draws them, and a mutation all but never
    /// changes the ground or the group — a bigger step than the rest, and one
    /// to ask for. So is xaos, whose settings here draw none.
    fn default() -> Self {
        Self {
            translate: Part::new(0.18, 0.0),
            rotate: Part::new(0.15, 0.0),
            scale: Part::new(0.15, 0.0),
            weight: Part::new(0.12, 0.0),
            variations: Part::new(0.18, 0.15),
            count: Part::new(0.08, 0.3),
            symmetry: Part::new(MIN_ODDS, 0.5),
            color: Part::new(0.09, 0.1),
            color_value: Part::new(0.05, 0.0),
            background: Part::new(MIN_ODDS, 0.5),
            xaos: Part::new(MIN_ODDS, 0.5),
        }
    }
}

/// How big a step a mutation takes. A roll draws every part that's on, so
/// none of this applies to it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct MutationSettings {
    /// Scales every nudge: 1 is a noticeable but usually non-destructive step.
    pub strength: f32,
    /// Parts one mutation changes, each picked by the parts' odds.
    pub changes: [u32; 2],
    /// Halve the strength after this many rejected tries in a row (0 = never),
    /// down to a sixteenth — so a strong setting on a delicate scene still
    /// finds something close instead of nothing.
    pub backoff: u32,
    /// Rescale a mutation onto the dimension bound, as a roll is. Off by
    /// default: it resizes every transform, a bigger change than the mutation.
    pub steer_dimension: bool,
}

impl Default for MutationSettings {
    fn default() -> Self {
        Self { strength: 1.0, changes: [1, 3], backoff: 4, steer_dimension: false }
    }
}

/// A profile: the parts of a flame the dice may make, how each is drawn, and
/// how big a step a mutation takes. One profile for a roll and a mutation
/// alike. Persisted to prefs as `[random]`, edited in the Random settings window,
/// saved and loaded as named profiles (`super::profile`), and readable by
/// `--random-config`.
///
/// Every range is `[min, max]`, inclusive; [`Self::normalized`] puts them in
/// order and inside what the generator can use, so a hand-edited file can't
/// hand it an inverted or impossible range.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct RandomSettings {
    /// What the dice may make, part by part.
    pub parts: Parts,
    /// Transforms per flame.
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
    /// Which symmetry groups, and how often a roll takes one.
    pub symmetry: SymmetrySettings,
    /// Whether a roll says which map may follow which, and how.
    pub xaos: XaosSettings,
    /// Bound on the similarity dimension of the maps' affine parts. Steered:
    /// see the module docs.
    pub dimension: Bound,
    /// Bound on the lacunarity summary (1 = a random scatter, higher =
    /// clumpier). Rejection-sampled.
    pub lacunarity: Bound,
    /// Candidates tried per flame (or per mutation) before giving up.
    pub attempts: u32,
    /// How big a step a mutation takes.
    pub mutation: MutationSettings,
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
            parts: Parts::default(),
            transforms: [2, 5],
            scale: [0.35, 0.75],
            squash: 0.3,
            affine: 0.08,
            kinds: [1, 2],
            per_map: [0, 1],
            negative: 0.15,
            variations,
            symmetry: SymmetrySettings::default(),
            xaos: XaosSettings::default(),
            dimension: Bound::off([1.5, 2.5]),
            // p10-p75 of default rolls (`random_flame_study`): on, it keeps
            // most of what the dice make and trims the smooth and the sparse.
            lacunarity: Bound::off([8.0, 30.0]),
            attempts: 60,
            mutation: MutationSettings::default(),
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
pub const MAX_CHANGES: u32 = 8;
/// A rolled xaos multiplier's outer limits.
pub const MULTIPLIER_LIMITS: [f32; 2] = [0.05, 20.0];
/// The most the xaos walk-share floor can ask of every map.
pub const MAX_MIN_SHARE: f32 = 0.2;
/// Past this a part's odds stop meaning anything the others can answer.
pub const ODDS_LIMIT: f32 = 10.0;
/// The least odds a part that's on can have: rolled, and all but never
/// picked by a mutation. Anything below it is off.
pub const MIN_ODDS: f32 = 0.0001;

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
        let x = &mut self.xaos;
        for v in [&mut x.off, &mut x.preset, &mut x.random] {
            *v = if v.is_finite() { v.clamp(0.0, ODDS_LIMIT) } else { 0.0 };
        }
        if x.off + x.preset + x.random <= 0.0 {
            x.off = 1.0;
        }
        x.cut = ordered(x.cut, 0.0, 1.0);
        x.self_loops = x.self_loops.clamp(0.0, 1.0);
        x.weighted = x.weighted.clamp(0.0, 1.0);
        x.multiplier = ordered(x.multiplier, MULTIPLIER_LIMITS[0], MULTIPLIER_LIMITS[1]);
        x.min_share = if x.min_share.is_finite() { x.min_share.clamp(0.0, MAX_MIN_SHARE) } else { 0.0 };
        self.dimension.range = ordered(self.dimension.range, DIMENSION_LIMITS[0], DIMENSION_LIMITS[1]);
        self.lacunarity.range =
            ordered(self.lacunarity.range, LACUNARITY_LIMITS[0], LACUNARITY_LIMITS[1]);
        self.attempts = self.attempts.clamp(1, MAX_ATTEMPTS);
        let m = &mut self.mutation;
        m.strength = if m.strength.is_finite() { m.strength.clamp(STRENGTH_LIMITS[0], STRENGTH_LIMITS[1]) } else { 1.0 };
        m.changes = ordered(m.changes, 1, MAX_CHANGES);
        m.backoff = m.backoff.min(MAX_ATTEMPTS);
        for p in self.parts.all_mut() {
            p.odds = if p.odds.is_finite() && p.odds > 0.0 { p.odds.clamp(MIN_ODDS, ODDS_LIMIT) } else { 0.0 };
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

    /// Whether `other` makes the same rolls: everything but what only a
    /// mutation reads (its step, and the parts' odds and re-roll chances).
    /// The Random flames list starts over only when this changes.
    pub fn same_roll(&self, other: &Self) -> bool {
        let rolled = |s: &Self| {
            let mut s = s.clone();
            s.mutation = MutationSettings::default();
            for p in s.parts.all_mut() {
                *p = Part { odds: if p.on() { 1.0 } else { 0.0 }, reroll: 0.0 };
            }
            s
        };
        rolled(self) == rolled(other)
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
    fn the_mutation_step_and_parts_are_normalized_too() {
        let mut s = RandomSettings::default();
        s.mutation.strength = 40.0;
        s.mutation.changes = [5, 2];
        s.parts.rotate.odds = -1.0;
        s.parts.color.reroll = f32::NAN;
        let s = s.normalized();
        assert_eq!(s.mutation.strength, STRENGTH_LIMITS[1]);
        assert_eq!(s.mutation.changes, [2, 5]);
        assert_eq!(s.parts.rotate.odds, 0.0);
        assert_eq!(s.parts.color.reroll, 0.0);
    }

    /// A roll reads which parts are on and the ranges; only a mutation reads
    /// the step and the odds.
    /// Off is odds 0 and nothing else: a part that's on never sits below the
    /// least odds, and switching one on lands it there.
    #[test]
    fn a_part_is_on_or_off_one_way() {
        let mut s = RandomSettings::default();
        s.parts.rotate.odds = 0.00001;
        s.parts.weight.odds = -3.0;
        let s = s.normalized();
        assert_eq!(s.parts.rotate.odds, MIN_ODDS);
        assert_eq!(s.parts.weight.odds, 0.0);
        let mut p = Part { odds: 0.0, reroll: 0.2 };
        p.set_on(true);
        assert_eq!(p.odds, MIN_ODDS);
        p.odds = 0.4;
        p.set_on(true);
        assert_eq!(p.odds, 0.4, "switching on what's on changes nothing");
        p.set_on(false);
        assert!(!p.on());
    }

    #[test]
    fn the_roll_reads_the_switches_not_the_odds() {
        let a = RandomSettings::default();
        let mut b = a.clone();
        b.mutation.strength = 2.0;
        b.parts.rotate.odds = 3.0;
        assert!(a.same_roll(&b));
        b.parts.color.set_on(false);
        assert!(!a.same_roll(&b));
        let mut c = a.clone();
        c.transforms = [3, 3];
        assert!(!a.same_roll(&c));
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
