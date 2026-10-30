//! The dice: one generator for random flames and mutations alike.
//!
//! **One profile, one list of parts.** A profile ([`RandomSettings`]) says
//! which parts of a flame the dice may make — each transform's position, turn,
//! size, weight and variations, how many transforms, the symmetry group, which
//! map may follow which (xaos), the
//! colours, where each sits on the gradient, the background — and the ranges
//! each is drawn from. Both actions read the same list the same way:
//!
//! - **Roll** draws every part that's on, and keeps the base scene's own for
//!   every part that's off (`roll.rs`). With every part on — the default — the
//!   base is never read, and a seed rolls what it always has.
//! - **Mutate** changes a few of the parts that are on: nudged, or re-rolled
//!   with the very draw a roll makes (`mutate.rs`).
//!
//! So a part that's off means one thing whichever is pressed: *the dice leave
//! this alone*. `colour-only` recolours what's there, by a roll or a mutation;
//! `no-colour` changes the form and keeps the colours, either way.
//!
//! Both go through one search ([`generate`]): draw a candidate, steer its
//! dimension if size is the dice's to change, judge it at the gate, and go
//! again until one passes or the budget is spent. What a candidate is judged
//! against depends only on what it owes the base: a roll with the whole form
//! on is held to the profile's bounds absolutely; anything that keeps some of
//! the base's form — every mutation, and a roll with a form part off — is held
//! to *the base's* standing on each check, no worse than it was (`gate.rs`
//! says why). A mutation's strength halves after a run of rejections, so a
//! delicate scene still yields something close.
//!
//! Randomising an IFS is easy; randomising one that *renders* is not. Most
//! random parameter sets either collapse to a point or diverge to a haze, so
//! every candidate is run through a short CPU chaos game (the same `trace.rs`
//! walkers the GPU shader mirrors). The two bounds a profile can set are met
//! in different ways, because they cost different amounts:
//!
//! - **Dimension is steered, not rejected.** It is analytic (`Σ sᵢᵈ = 1` over
//!   the maps' contractions), and scaling every map by one factor `k` moves it
//!   predictably: `k = (Σ sᵢᵗ)^(-1/t)` puts it exactly on a target `t`. So a
//!   flame outside the bound is rescaled onto a target drawn inside it, which
//!   keeps the maps' relative sizes and costs nothing. Only a rescale that
//!   would stop a map contracting is a rejection. A mutation steers only when
//!   its profile asks (`mutation.steer_dimension`): it resizes every map.
//!   Under xaos the dimension is Mauldin–Williams and has no closed form in
//!   the scale, so the factor is found by bisection instead — it still rises
//!   with the scale.
//! - **Lacunarity is rejected.** It is measured by a chaos walk, and nothing
//!   predicts it from the parameters, so a candidate outside the bound is
//!   thrown away and another drawn.
//!
//! Every roll and every mutation is reproducible from its seed
//! ([`roll_seeded`], [`mutate_seeded`]) given the same profile and base, and
//! profiles are files (`profile.rs`), so `--random --seed N --random-config
//! NAME` gets back exactly what the window rolled.

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

/// What the dice do to the base scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Draw every part that's on; keep the base's for the rest.
    Roll,
    /// Change a few of the parts that are on.
    Mutate,
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
    /// Under xaos, a map the walk starves.
    pub xaos: u32,
}

impl Rejections {
    pub fn total(&self) -> u32 {
        self.shape + self.dimension + self.lacunarity + self.xaos
    }

    fn count(&mut self, why: Reject) {
        match why {
            Reject::Shape => self.shape += 1,
            Reject::Dimension => self.dimension += 1,
            Reject::Lacunarity => self.lacunarity += 1,
            Reject::Xaos => self.xaos += 1,
        }
    }

    /// "52 lacunarity, 7 shape", biggest first, for messages.
    pub fn describe(&self) -> String {
        let mut parts = [
            (self.lacunarity, "lacunarity"),
            (self.dimension, "dimension"),
            (self.shape, "shape"),
            (self.xaos, "starved by xaos"),
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
    /// What a mutation changed, one line per change, for people and agents
    /// to read. Empty for a roll.
    pub log: Vec<String>,
    /// The strength a mutation was made at — less than the profile's when it
    /// backed off to find one. `None` for a roll.
    pub strength: Option<f32>,
}

impl Roll {
    /// "strength backed off to 0.25 to find this", when it did.
    pub fn backoff_note(&self, settings: &RandomSettings) -> Option<String> {
        let s = self.strength?;
        (s < settings.mutation.strength).then(|| format!("strength backed off to {s:.2} to find this"))
    }
}

/// Nothing met the settings in the budget.
pub struct RollFailure {
    pub attempts: u32,
    pub rejected: Rejections,
    /// The last candidate — framed neutrally, for a roll — for a caller that
    /// must return *something*.
    pub last: Scene,
    /// A mutation came up empty, rather than a roll.
    pub mutation: bool,
}

impl RollFailure {
    pub fn message(&self) -> String {
        let (what, fix) = if self.mutation {
            ("mutation of this scene", "lower the strength, loosen a bound or allow more tries")
        } else {
            ("flame", "loosen a bound, switch on more parts or allow more tries")
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
/// be had again from the command line. `base` is the scene whose parts are
/// kept where the profile has them off; with every part on it isn't read. The
/// scene is named after its seed.
pub fn roll_seeded(settings: &RandomSettings, base: &Scene, seed: u64) -> Result<Roll, RollFailure> {
    use rand::SeedableRng;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut rolled = generate(settings, base, Action::Roll, &mut rng);
    // Named after its seed, unless it is the base recoloured, which keeps
    // the base's name.
    if !settings.parts.no_form() {
        let name = format!("random-{seed}");
        match &mut rolled {
            Ok(r) => r.scene.name = name,
            Err(f) => f.last.name = name,
        }
    }
    rolled
}

/// One mutation of `parent` from a seed: `U`, the Mutate window and
/// `--mutations` all come through here, so any mutation can be had again.
/// The child keeps its parent's name.
pub fn mutate_seeded(settings: &RandomSettings, parent: &Scene, seed: u64) -> Result<Roll, RollFailure> {
    use rand::SeedableRng;
    generate(settings, parent, Action::Mutate, &mut rand::rngs::StdRng::seed_from_u64(seed))
}

/// A roll from nothing — the blank scene as base, which only matters for a
/// profile with parts off. What the tests and `random_flame` call.
#[cfg_attr(not(test), allow(dead_code))]
pub fn roll(settings: &RandomSettings, rng: &mut impl Rng) -> Result<Roll, RollFailure> {
    generate(settings, &Scene::blank(), Action::Roll, rng)
}

/// Roll or mutate `base` under `settings`: the one search both go through.
pub fn generate<R: Rng>(settings: &RandomSettings, base: &Scene, action: Action, rng: &mut R) -> Result<Roll, RollFailure> {
    let parts = &settings.parts;
    // A roll with the whole form on owes the base nothing, so the bounds
    // alone judge it; anything that keeps some of the base's form is judged
    // against the base, or a base outside the bounds could never yield one.
    let gate = if action == Action::Roll && parts.whole_form() {
        Gate::absolute(settings)
    } else {
        Gate::relative_to(settings, base)
    };
    let steer = parts.scale.on() && (action == Action::Roll || settings.mutation.steer_dimension);
    match action {
        Action::Roll => {
            match search(settings, &gate, steer, rng, |rng, _| roll::build_candidate(settings, base, rng)) {
                Ok(found) => {
                    let (center, radius) =
                        found.measured.stats.as_ref().map(|s| (s.center, s.radius)).unwrap_or((Vec3::ZERO, 1.0));
                    Ok(Roll {
                        scene: roll::finish(found.candidate, center, radius, settings, base, rng),
                        dimension: found.measured.dimension,
                        lacunarity: found.measured.lacunarity,
                        attempts: found.attempts,
                        rejected: found.rejected,
                        log: Vec::new(),
                        strength: None,
                    })
                }
                Err(lost) => {
                    let last = lost.last.unwrap_or_else(|| roll::build_candidate(settings, base, rng));
                    Err(RollFailure {
                        attempts: lost.attempts,
                        rejected: lost.rejected,
                        last: roll::finish(last, Vec3::ZERO, 1.0, settings, base, rng),
                        mutation: false,
                    })
                }
            }
        }
        Action::Mutate => {
            let m = &settings.mutation;
            let propose = |rng: &mut R, rejected_so_far: u32| {
                let strength = backed_off(m.strength, rejected_so_far, m.backoff);
                let mut scene = base.clone();
                let log = mutate::mutate(&mut scene, settings, strength, rng);
                Child { scene, log, strength }
            };
            match search(settings, &gate, steer, rng, propose) {
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
                    last: lost.last.map(|c| c.scene).unwrap_or_else(|| base.clone()),
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

    fn scene_file(name: &str) -> Scene {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes").join(name);
        Scene::load(path.to_str().unwrap()).unwrap()
    }

    fn builtin(name: &str) -> RandomSettings {
        profile::load(&profile::Entry::builtin(name)).unwrap().settings
    }

    /// A roll that leaves the form alone leaves everything about it alone:
    /// the animation, the camera's flight, the zoom, the name.
    #[test]
    fn a_recolour_keeps_the_animation_and_the_flight() {
        let base = scene_file("parasol_tide_opus55.toml");
        assert!(!base.tracks.is_empty() && base.camera_path.is_some(), "the worked example animates");
        let rolled = roll_seeded(&builtin("colour-only"), &base, 11).map(|r| r.scene).unwrap_or_else(|f| panic!("{}", f.message()));
        // The tracks and the camera's flight, as the file writes them.
        let animation = |s: &Scene| s.to_toml_string().unwrap().split_once("[[track]]").map(|(_, t)| t.to_string());
        assert_eq!(rolled.tracks, base.tracks);
        assert_eq!(animation(&rolled), animation(&base));
        assert_eq!(rolled.name, base.name);
        let zoom = scene_file("wellspiral.toml");
        let rolled = roll_seeded(&builtin("colour-only"), &zoom, 11).map(|r| r.scene).unwrap_or_else(|f| panic!("{}", f.message()));
        assert_eq!(rolled.zoom, zoom.zoom, "a recolour keeps its infinite zoom");
        assert_eq!(animation(&rolled), animation(&zoom), "and its zoom loop");
    }

    /// A roll draws the scene's own maps, so a composite's subscenes come
    /// through even a roll of the whole form.
    #[test]
    fn a_roll_keeps_a_composites_subscenes() {
        let base = scene_file("censer.toml");
        let rolled = roll_seeded(&RandomSettings::default(), &base, 5).map(|r| r.scene).unwrap_or_else(|f| panic!("{}", f.message()));
        assert_eq!(rolled.subscenes.len(), base.subscenes.len());
        let layered = |s: &Scene| {
            s.transforms.iter().filter(|t| t.layer != 0).map(|t| (t.layer, t.matrix, t.variations.clone())).collect::<Vec<_>>()
        };
        assert_eq!(layered(&rolled), layered(&base));
        assert_eq!(rolled.camera_focus, base.camera_focus, "the composition keeps its framing");
        // And it is a scene the loader takes back.
        let text = rolled.to_toml_string().unwrap();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes");
        Scene::from_toml_str(&text, &dir).unwrap();
    }

    /// Mutations of an animated scene keep it animated, and every child
    /// saves to a scene that loads with its tracks.
    #[test]
    fn mutations_keep_an_animated_scene_animated() {
        let base = scene_file("parasol_tide_opus55.toml");
        for seed in 0..12 {
            let Ok(child) = mutate_seeded(&builtin("anything"), &base, seed).map(|r| r.scene) else { continue };
            assert!(!child.tracks.is_empty(), "seed {seed} lost the animation");
            let text = child.to_toml_string().unwrap();
            let back = Scene::from_toml_str(&text, &std::env::temp_dir()).unwrap();
            assert_eq!(back.tracks.len(), child.tracks.len(), "seed {seed}");
        }
    }

    /// Every built-in profile rolls and mutates, and a part that's off is
    /// left alone the same way by both: `colour-only` never moves the form,
    /// `no-colour` never changes a colour, whether rolled or mutated.
    #[test]
    fn every_builtin_profile_rolls_and_mutates() {
        use profile::{load, Entry, BUILTINS};
        let Ok(parent) = roll_seeded(&RandomSettings::default(), &Scene::blank(), 7).map(|r| r.scene) else {
            panic!("no parent")
        };
        let same_form = |c: &Scene| {
            c.transforms.len() == parent.transforms.len()
                && c.transforms.iter().zip(&parent.transforms).all(|(a, b)| {
                    a.matrix == b.matrix && a.variations == b.variations && a.weight == b.weight && a.symmetry == b.symmetry
                })
        };
        let same_colour = |c: &Scene| {
            c.colors == parent.colors
                && c.background == parent.background
                && c.color_mode == parent.color_mode
                && c.colormap == parent.colormap
                && c.transforms.iter().zip(&parent.transforms).all(|(a, b)| a.color_value == b.color_value)
        };
        for name in std::iter::once(profile::BUILTIN).chain(BUILTINS.iter().map(|(n, _)| *n)) {
            let settings = load(&Entry::builtin(name)).unwrap().settings;
            let mut made = Vec::new();
            for seed in 0..10 {
                if let Ok(r) = roll_seeded(&settings, &parent, seed) {
                    made.push(("roll", r));
                }
                if let Ok(r) = mutate_seeded(&settings, &parent, seed) {
                    made.push(("mutation", r));
                }
            }
            let rolls = made.iter().filter(|(k, _)| *k == "roll").count();
            assert!(rolls >= 8, "{name}: only {rolls}/10 rolls passed the gate");
            assert!(made.len() - rolls >= 7, "{name}: only {}/10 mutations passed", made.len() - rolls);
            for (kind, r) in &made {
                match name {
                    "colour-only" => assert!(same_form(&r.scene), "colour-only {kind} changed the form: {:?}", r.log),
                    "no-colour" => assert!(same_colour(&r.scene), "no-colour {kind} changed a colour: {:?}", r.log),
                    _ => {}
                }
            }
            if name == "colour-only" {
                assert!(made.iter().any(|(k, r)| *k == "roll" && !same_colour(&r.scene)), "colour-only rolls never recoloured");
            }
            if name == "no-colour" {
                assert!(made.iter().any(|(k, r)| *k == "roll" && !same_form(&r.scene)), "no-colour rolls never changed the form");
            }
        }
    }

    /// With every part on, a roll owes the base nothing: the same seed rolls
    /// the same flame on any base, which is what keeps every recorded
    /// `--random --seed N` rolling what it always rolled.
    #[test]
    fn with_every_part_on_the_base_is_never_read() {
        let s = RandomSettings::default();
        let other = roll_seeded(&s, &Scene::blank(), 99).map(|r| r.scene).ok().expect("a base");
        for seed in [1, 2, 3, 40, 500] {
            let a = roll_seeded(&s, &Scene::blank(), seed).ok().map(|r| r.scene);
            let b = roll_seeded(&s, &other, seed).ok().map(|r| r.scene);
            let key = |s: &Option<Scene>| s.as_ref().map(|s| (s.transforms.iter().map(|t| t.matrix).collect::<Vec<_>>(), s.colors.clone(), s.background));
            assert_eq!(key(&a), key(&b), "seed {seed} depends on the base");
        }
    }
}
