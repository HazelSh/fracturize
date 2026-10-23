//! The dice: one generator for random flames and mutations alike.
//!
//! A fresh roll and a mutation are the same search with a different source of
//! candidates ([`Source`]). Both draw candidates under one profile
//! ([`RandomSettings`]), put each through the same gate, spend the same try
//! budget, count the same [`Rejections`], and fail the same way
//! ([`RollFailure`]). What differs is only where a candidate comes from and
//! what it is judged against:
//!
//! - **Fresh** (`roll.rs`): built part by part from nothing, and held to the
//!   profile's bounds absolutely.
//! - **From a parent** (`mutate.rs`): the parent with a few parts nudged or
//!   re-rolled, and held to *the parent's* standing on each check — no worse
//!   than it was (`gate.rs` says why). After a run of rejections the strength
//!   halves, so a delicate scene still yields something close.
//!
//! Randomising an IFS is easy; randomising one that *renders* is not. Most
//! random parameter sets either collapse to a point or diverge to a haze, so
//! every candidate is run through a short CPU chaos game (the same `trace.rs`
//! walkers the GPU shader mirrors). The two bounds a profile can set are met
//! in different ways, because they cost different amounts:
//!
//! - **Dimension is steered, not rejected** (for fresh rolls; a mutation can
//!   opt in). It is analytic (`Σ sᵢᵈ = 1` over the maps' contractions), and
//!   scaling every map by one factor `k` moves it predictably: `k = (Σ
//!   sᵢᵗ)^(-1/t)` puts it exactly on a target `t`. So a flame outside the bound
//!   is rescaled onto a target drawn inside it, which keeps the maps' relative
//!   sizes and costs nothing. Only a rescale that would stop a map contracting
//!   is a rejection.
//! - **Lacunarity is rejected.** It is measured by a chaos walk, and nothing
//!   predicts it from the parameters, so a candidate outside the bound is
//!   thrown away and another drawn.
//!
//! Every roll and every mutation is reproducible from its seed
//! ([`roll_seeded`], [`mutate_seeded`]) given the same profile, and profiles
//! are files (`profile.rs`), so `--random --seed N --random-config NAME` gets
//! back exactly what the window rolled.

mod gate;
mod mutate;
pub mod profile;
mod roll;
mod settings;

use std::borrow::Cow;

use glam::Vec3;
use rand::Rng;

use crate::scene::{Scene, TransformSpec};
use gate::{Gate, Measured, Reject};
pub use settings::*;

/// Where a generated flame comes from.
#[derive(Clone, Copy)]
pub enum Source<'a> {
    /// Nothing: a whole new flame.
    Fresh,
    /// A mutation of this scene.
    From(&'a Scene),
}

/// Why candidates were thrown away, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rejections {
    /// Collapsed, diverged, a line, or a lump: the quality gate.
    pub shape: u32,
    /// Out of the dimension bound, or no rescale could put it in without a
    /// map ceasing to contract.
    pub dimension: u32,
    /// Lacunarity outside its bound.
    pub lacunarity: u32,
}

impl Rejections {
    pub fn total(&self) -> u32 {
        self.shape + self.dimension + self.lacunarity
    }

    fn count(&mut self, why: Reject) {
        match why {
            Reject::Shape => self.shape += 1,
            Reject::Dimension => self.dimension += 1,
            Reject::Lacunarity => self.lacunarity += 1,
        }
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
    /// What a mutation changed, one line per operator, for people and
    /// agents to read. Empty for a fresh roll.
    pub log: Vec<String>,
    /// The strength a mutation was made at — less than the profile's when it
    /// backed off to find one. `None` for a fresh roll.
    pub strength: Option<f32>,
}

impl Roll {
    /// "strength backed off to 0.25 to find this", when it did.
    pub fn backoff_note(&self, settings: &RandomSettings) -> Option<String> {
        let s = self.strength?;
        (s < settings.mutate.strength).then(|| format!("strength backed off to {s:.2} to find this"))
    }
}

/// Nothing met the settings in the budget.
pub struct RollFailure {
    pub attempts: u32,
    pub rejected: Rejections,
    /// The last candidate — framed neutrally, for a fresh roll — for a caller
    /// that must return *something*.
    pub last: Scene,
    /// A mutation came up empty, rather than a fresh roll.
    pub mutation: bool,
}

impl RollFailure {
    pub fn message(&self) -> String {
        let (what, fix) = if self.mutation {
            ("mutation of this scene", "lower the strength, loosen a bound or allow more tries")
        } else {
            ("flame", "loosen a bound or allow more tries")
        };
        format!(
            "no {what} met the random settings in {} tries (rejected for {}) — {fix}",
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

/// One mutation of `parent` from a seed: `U`, the Mutate window and
/// `--mutations` all come through here, so any mutation can be had again.
/// The child keeps its parent's name.
pub fn mutate_seeded(settings: &RandomSettings, parent: &Scene, seed: u64) -> Result<Roll, RollFailure> {
    use rand::SeedableRng;
    generate(settings, Source::From(parent), &mut rand::rngs::StdRng::seed_from_u64(seed))
}

/// Build a random flame that meets `settings`.
pub fn roll(settings: &RandomSettings, rng: &mut impl Rng) -> Result<Roll, RollFailure> {
    generate(settings, Source::Fresh, rng)
}

/// Generate a flame that meets `settings`, fresh or from a parent.
pub fn generate<R: Rng>(settings: &RandomSettings, source: Source, rng: &mut R) -> Result<Roll, RollFailure> {
    match source {
        Source::Fresh => {
            let gate = Gate::absolute(settings);
            match search(settings, &gate, true, rng, |rng, _| roll::build_candidate(settings, rng)) {
                Ok(found) => {
                    let (center, radius) =
                        found.measured.stats.as_ref().map(|s| (s.center, s.radius)).unwrap_or((Vec3::ZERO, 1.0));
                    Ok(Roll {
                        scene: roll::finish(found.candidate, center, radius, rng),
                        dimension: found.measured.dimension,
                        lacunarity: found.measured.lacunarity,
                        attempts: found.attempts,
                        rejected: found.rejected,
                        log: Vec::new(),
                        strength: None,
                    })
                }
                Err(lost) => {
                    let last = lost.last.unwrap_or_else(|| roll::build_candidate(settings, rng));
                    Err(RollFailure {
                        attempts: lost.attempts,
                        rejected: lost.rejected,
                        last: roll::finish(last, Vec3::ZERO, 1.0, rng),
                        mutation: false,
                    })
                }
            }
        }
        Source::From(parent) => {
            let gate = Gate::relative_to(settings, parent);
            let m = &settings.mutate;
            let propose = |rng: &mut R, rejected_so_far: u32| {
                let strength = backed_off(m.strength, rejected_so_far, m.backoff);
                let mut scene = parent.clone();
                let log = mutate::mutate(&mut scene, settings, strength, rng);
                Child { scene, log, strength }
            };
            match search(settings, &gate, m.steer_dimension, rng, propose) {
                Ok(found) => Ok(Roll {
                    scene: found.candidate.scene,
                    dimension: found.measured.dimension,
                    lacunarity: found.measured.lacunarity,
                    attempts: found.attempts,
                    rejected: found.rejected,
                    log: found.candidate.log,
                    strength: Some(found.candidate.strength),
                }),
                Err(lost) => Err(RollFailure {
                    attempts: lost.attempts,
                    rejected: lost.rejected,
                    last: lost.last.map(|c| c.scene).unwrap_or_else(|| parent.clone()),
                    mutation: true,
                }),
            }
        }
    }
}

/// Halvings of the strength a mutation backs off by, at most: down to a
/// sixteenth. Past that a "mutation" is indistinguishable from its parent.
const MAX_HALVINGS: u32 = 4;

/// `strength`, halved once per `every` rejections so far.
fn backed_off(strength: f32, rejected: u32, every: u32) -> f32 {
    if every == 0 {
        return strength;
    }
    strength / (1u32 << (rejected / every).min(MAX_HALVINGS)) as f32
}

/// A mutated candidate.
struct Child {
    scene: Scene,
    log: Vec<String>,
    strength: f32,
}

/// What the search loop needs of a candidate, fresh or mutated.
trait Candidate {
    /// Its maps, one slice per layer, for the gate.
    fn layers(&self) -> Vec<Cow<'_, [TransformSpec]>>;
    /// The maps a dimension steer may rescale — `None` when there is no one
    /// set of them to rescale (a composite's layers each have their own).
    fn steerable(&mut self) -> Option<&mut [TransformSpec]>;
}

impl Candidate for roll::Candidate {
    fn layers(&self) -> Vec<Cow<'_, [TransformSpec]>> {
        vec![Cow::Borrowed(&self.transforms)]
    }
    fn steerable(&mut self) -> Option<&mut [TransformSpec]> {
        Some(&mut self.transforms)
    }
}

impl Candidate for Child {
    fn layers(&self) -> Vec<Cow<'_, [TransformSpec]>> {
        gate::layers(&self.scene)
    }
    fn steerable(&mut self) -> Option<&mut [TransformSpec]> {
        (!self.scene.is_composite()).then_some(&mut self.scene.transforms[..])
    }
}

struct Found<C> {
    candidate: C,
    measured: Measured,
    attempts: u32,
    rejected: Rejections,
}

struct Lost<C> {
    last: Option<C>,
    attempts: u32,
    rejected: Rejections,
}

/// The one loop: draw a candidate, steer its dimension if asked, judge it,
/// and go again until one passes or the budget is spent. `propose` is told how
/// many have been rejected so far.
fn search<C: Candidate, R: Rng>(
    settings: &RandomSettings,
    gate: &Gate,
    steer: bool,
    rng: &mut R,
    mut propose: impl FnMut(&mut R, u32) -> C,
) -> Result<Found<C>, Lost<C>> {
    let mut rejected = Rejections::default();
    let mut last = None;
    for attempt in 1..=settings.attempts.max(1) {
        let mut c = propose(rng, rejected.total());
        let mut steered = None;
        if steer {
            if let Some(maps) = c.steerable() {
                match gate::steer_dimension(maps, settings, rng) {
                    Ok(d) => steered = Some(d),
                    Err(()) => {
                        rejected.count(Reject::Dimension);
                        last = Some(c);
                        continue;
                    }
                }
            }
        }
        let verdict = gate.judge(&c.layers(), steered.is_some());
        match verdict {
            Ok(mut measured) => {
                if let Some(d) = steered {
                    measured.dimension = d;
                }
                return Ok(Found { candidate: c, measured, attempts: attempt, rejected });
            }
            Err(why) => {
                log::debug!("dice: candidate {attempt} rejected ({why:?})");
                rejected.count(why);
                last = Some(c);
            }
        }
    }
    Err(Lost { last, attempts: settings.attempts.max(1), rejected })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_halves_per_run_of_rejections_down_to_a_sixteenth() {
        assert_eq!(backed_off(1.0, 0, 4), 1.0);
        assert_eq!(backed_off(1.0, 3, 4), 1.0);
        assert_eq!(backed_off(1.0, 4, 4), 0.5);
        assert_eq!(backed_off(1.0, 9, 4), 0.25);
        assert_eq!(backed_off(1.0, 1000, 4), 1.0 / 16.0);
        assert_eq!(backed_off(2.0, 1000, 0), 2.0, "0 never backs off");
    }
}
