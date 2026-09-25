//! The parameter vector, the forward walk, and a deliberately crude optimiser.
//!
//! Slice 1 of LIKENESS-PLAN.md: annealing only, no gradients, and a map budget
//! chosen to be too small for the subject. The governing principle is that the
//! solver must **never be able to succeed** — what we are after is the subject
//! assembled out of ghosts of itself at three or four scales, and a fit that
//! converged would just be a bad photograph.

use glam::{Mat4, Quat, Vec2, Vec3};
use rand::Rng;

use crate::scene::TransformSpec;
use crate::symmetry::{OrbitColor, OrbitKind, Symmetry};
use crate::trace::{Walker, BURN_IN_STEPS};

use super::loss;

/// Largest singular value any fitted map may have.
///
/// Contractivity is enforced on the *largest* axis rather than each one, so a
/// map can still be long in one direction and flat in another — which is what
/// trunk-and-branch maps look like, and clamping per-axis would forbid.
const MAX_SINGULAR: f32 = 0.94;
/// Smallest, so a map cannot quietly retire by collapsing to its fixed point.
const MIN_SINGULAR: f32 = 0.03;

/// Walkers run in parallel, each contributing `n / TRAJECTORIES` points.
///
/// One long trajectory would give a strongly autocorrelated cloud — the chaos
/// game only mixes over several steps — and the loss would then be measuring
/// one path through the attractor rather than the attractor.
const TRAJECTORIES: usize = 64;

/// One map's ten geometric numbers, plus the three that give it a colour.
#[derive(Clone, Copy, Debug)]
pub struct MapParams {
    pub translation: Vec3,
    /// Axis-angle, radians. A rotation *vector* rather than a matrix or euler
    /// triple: it is a rotation by construction, so unlike the paper's
    /// `U S Vᵀ` it can never drift off the manifold and needs no
    /// Gram–Schmidt re-projection after a step.
    pub rotvec: Vec3,
    /// Per-axis log scale, saturated into `[MIN_SINGULAR, MAX_SINGULAR]`.
    pub log_scale: Vec3,
    pub log_weight: f32,
    /// Linear RGB, the same space a scene's per-transform `color` is in.
    pub color: Vec3,
    /// Index into [`PALETTE`] — which nonlinearity this map blends toward.
    pub variation: usize,
    /// How far toward it, 0 (pure affine) to [`Config::nonlinear`].
    pub var_amount: f32,
}

/// The nonlinearities a fit may reach for.
///
/// Not all twenty: fitting a free blend over every slot would be 120 extra
/// dimensions for six maps, and most of that space is mush. One choice plus
/// one amount per map is twelve, and it matches what `randomize.rs` found
/// about *rolled* flames — a single characteristic variation reads as a
/// coherent form where a pile of unrelated distortions does not.
///
/// Chosen for representational work rather than for prettiness. Folds
/// (`absfold`, `boxfold`, `spherefold`, `bent`, `horseshoe`) keep points near
/// where they started and buy structure, which is what a subject needs; the
/// strongly non-local ones (`polar`, `disc`, `spiral`) smear a likeness into
/// a spiral of itself and are left out. `spherical`, `julia` and `bulb` are in
/// because the plan names them as the fracturize dial's own ingredients.
pub const PALETTE: [&str; 12] = [
    "sinusoidal",
    "spherical",
    "swirl",
    "horseshoe",
    "julia",
    "bent",
    "fisheye",
    "bubble",
    "absfold",
    "boxfold",
    "spherefold",
    "bulb",
];

/// Slot in `VARIATION_NAMES` for each palette entry.
fn palette_slot(i: usize) -> usize {
    crate::scene::VARIATION_NAMES
        .iter()
        .position(|v| *v == PALETTE[i % PALETTE.len()])
        .expect("palette names must exist in VARIATION_NAMES")
}

/// The whole parameter vector.
///
/// Note what is *not* here: shear. The parameterisation is `R·diag(s)`, which
/// is the paper's `U S Vᵀ` with `V = I`, and that is forced rather than
/// convenient — the scene format has nowhere to write a shear, so a fitter
/// that produced one would emit scenes that silently do not match what was
/// fitted. Restricting the map class is a starvation dial anyway: fewer
/// degrees of freedom, more visible strain.
#[derive(Clone, Debug)]
pub struct Params {
    pub maps: Vec<MapParams>,
    /// The colour EMA rate the walk mixes at, and a fitted parameter rather
    /// than a constant.
    ///
    /// It has to be. The walk's colour is `mix(colour, map_colour, speed)`, so
    /// at 0.5 every point is a blend of the last three maps and random map
    /// colours average to grey — the whole attractor comes out pastel however
    /// saturated the reference is. Saturation lives near 1, flat washes near
    /// 0, and which one a subject wants is not something to decide in advance:
    /// the brass-and-cyan powercell wants a high speed, a soft-lit character
    /// card wants a low one.
    pub color_speed: f32,
    /// How far out of the image plane the maps are allowed to reach, 0–1.
    ///
    /// At 0 the maps rotate only about the view axis and never translate in
    /// z, so the attractor is planar and the fit is an honest 2D problem. At 1
    /// they are unconstrained 3D.
    ///
    /// This is not a tuning knob, it is the prior that makes a single view
    /// solvable at all. Fitting free 3D maps against one projection is wildly
    /// underdetermined — the out-of-plane rotations are free parameters the
    /// loss cannot see except through how they scramble the shadow, and the
    /// solver spends its whole budget in that null space. Measured on a
    /// three-map Sierpinski, which three maps can represent exactly: at depth
    /// 1 the fit returns scattered filaments, at depth 0 it returns the
    /// triangle.
    ///
    /// Above 0 it is [`crate::fit`]'s depth-hallucination dial rather than a
    /// gap — the result reads as itself from one angle and is an abstract
    /// solid from every other.
    pub depth: f32,
    /// A finite group every map is a motif of, if any.
    ///
    /// The strongest prior available, and for the right subject it is both an
    /// artifact dial and the thing that makes a single view well-posed. A
    /// solid of revolution's 3D shape *is* recoverable from one projection
    /// once the axis is fixed, so here the two jobs coincide.
    ///
    /// Note which axis matters. The fit's own failure mode without a group is
    /// radial symmetry about the *view* axis — starbursts — so a `C_n` about
    /// that same axis would only formalise the failure. What a canister has is
    /// revolution about an axis lying nearly *in* the image plane, which
    /// projects to vertical extent with horizontal banding. That is why the
    /// axis is fitted rather than pinned to z.
    pub symmetry: Option<OrbitKind>,
    /// Where [`Self::symmetry`]'s axis is allowed to point.
    pub axis_mode: AxisMode,
    /// Raw axis parameter, interpreted through [`Self::axis_mode`].
    pub axis: Vec3,
    /// Ceiling on every map's [`MapParams::var_amount`].
    ///
    /// Pure affine maps cannot hold a real subject — six of them, or twenty,
    /// still only ever produce unions of parallelograms of the thing, and no
    /// amount of loss engineering changes what the map class can express. A
    /// nonlinearity is far more expressive per parameter than another affine
    /// map, which is the argument for reaching here before reaching for a
    /// bigger budget.
    ///
    /// Note what this does to [`Config::dimension`]: `Σsᵢᵈ = 1` is computed
    /// from the *affine* parts alone, so with variations live the reported
    /// dimension describes the linear skeleton rather than the attractor it
    /// actually produces. The pressure still bites — it still forces the
    /// skeleton to be built out of less — but the number stops being a
    /// measurement of the picture.
    pub nonlinear: f32,
}

/// Where a fitted symmetry axis may point.
///
/// Left free, it does not stay where you want it. Measured on the Powercell
/// with a fitted 3-vector axis: the fit drove the axis to the view direction
/// on most seeds, because a rosette facing the camera is the cheapest way to
/// satisfy a transport loss whose unconstrained failure mode is already
/// radially symmetric. The result was mandalas, which is a different object
/// from the one in the reference.
///
/// So this is the author's choice rather than the solver's — which is the
/// right place for it, the solver being an authoring assist.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AxisMode {
    /// Constrained to the image plane, with the one remaining angle fitted.
    ///
    /// The right prior for a solid of revolution seen side-on: a canister, a
    /// column, a figure. Projects to vertical extent with horizontal banding,
    /// which is what makes a powercell read as a powercell.
    InPlane,
    /// Pinned. `view` (the z axis) is the mandala case — a rosette facing the
    /// camera, which is what card-art crystals tend to be.
    Fixed(Vec3),
}

/// Bounds on the fitted colour EMA rate. Not quite 1: a speed of exactly 1
/// discards the walk's history, and the point of mixing is that distinct
/// transform *combinations* get distinct colours.
const COLOR_SPEED: std::ops::RangeInclusive<f32> = 0.1..=0.95;

impl Params {
    pub fn random(
        maps: usize,
        depth: f32,
        symmetry: Option<OrbitKind>,
        axis_mode: AxisMode,
        nonlinear: f32,
        rng: &mut impl Rng,
    ) -> Self {
        let nonlinear = nonlinear.max(1e-6);
        let m = (0..maps)
            .map(|_| MapParams {
                translation: Vec3::new(
                    rng.gen_range(-0.6..0.6),
                    rng.gen_range(-0.6..0.6),
                    rng.gen_range(-0.3..0.3),
                ),
                rotvec: Vec3::new(
                    rng.gen_range(-1.2..1.2),
                    rng.gen_range(-1.2..1.2),
                    rng.gen_range(-1.2..1.2),
                ),
                log_scale: Vec3::new(
                    rng.gen_range(-1.6..-0.4),
                    rng.gen_range(-1.6..-0.4),
                    rng.gen_range(-1.6..-0.4),
                ),
                log_weight: rng.gen_range(-0.4..0.4),
                color: Vec3::new(rng.r#gen(), rng.r#gen(), rng.r#gen()),
                variation: rng.gen_range(0..PALETTE.len()),
                var_amount: rng.gen_range(0.0..nonlinear),
            })
            .collect();
        Self {
            maps: m,
            color_speed: rng.gen_range(COLOR_SPEED),
            depth,
            symmetry,
            axis_mode,
            nonlinear,
            // Near-vertical: the orientation almost every upright subject has,
            // and a start the fit can walk away from cheaply.
            axis: Vec3::new(rng.gen_range(-0.5..0.5), 1.0, 0.0).normalize(),
        }
    }

    /// The scale a map actually gets: saturated so the IFS stays contractive
    /// whatever the optimiser proposes, without the optimiser having to know.
    fn scale_of(m: &MapParams) -> Vec3 {
        let s = Vec3::new(
            m.log_scale.x.exp(),
            m.log_scale.y.exp(),
            m.log_scale.z.exp(),
        );
        // Rescale first, floor second. The other order lets the rescale push
        // a small axis back under the floor, and the floor is the one that has
        // to hold: a map flat to machine zero on an axis makes the attractor a
        // zero-measure sheet, which renders as nothing at all.
        let peak = s.max_element();
        let s = if peak > MAX_SINGULAR { s * (MAX_SINGULAR / peak) } else { s };
        s.clamp(Vec3::splat(MIN_SINGULAR), Vec3::splat(MAX_SINGULAR))
    }

    pub fn to_transforms(&self) -> Vec<TransformSpec> {
        let n = self.maps.len();
        self.maps
            .iter()
            .enumerate()
            .map(|(i, m)| TransformSpec {
                layer: 0,
                matrix: Mat4::from_scale_rotation_translation(
                    Self::scale_of(m),
                    Quat::from_scaled_axis(self.rotvec_of(m)),
                    Vec3::new(m.translation.x, m.translation.y, m.translation.z * self.depth),
                ),
                post_affine: Mat4::IDENTITY,
                // Unused in mix mode, which is what a fitted scene opens in,
                // but a scene flipped to `transforms` mode should still read
                // as something rather than as one flat colour.
                color_value: i as f32 / n as f32,
                weight: m.log_weight.exp().clamp(1e-3, 1e3),
                color_speed: self.color_speed,
                explicit_color_speed: None,
                variations: {
                    // A blend of linear and one nonlinearity, summing to 1 so
                    // the map keeps the magnitude its affine part was fitted
                    // with. `apply_variations` adds `w · f(p)` over the slots,
                    // so weights that did not sum to 1 would silently rescale
                    // every map.
                    let mut v = [0.0f32; crate::scene::NUM_VARIATIONS];
                    let a = m.var_amount.clamp(0.0, 1.0);
                    v[0] = 1.0 - a;
                    if a > 0.0 {
                        v[palette_slot(m.variation)] += a;
                    }
                    v
                },
                // Every map carries the group, so the whole form is a
                // quotient rather than one symmetric part beside free ones.
                // `Shared` colour: the copies *are* the same map, and
                // colouring them apart would say they aren't.
                symmetry: self.symmetry.and_then(|kind| {
                    Symmetry::new(kind, self.axis_of(), false, OrbitColor::Shared).ok()
                }),
            })
            .collect()
    }

    /// The symmetry axis this parameter vector actually uses.
    ///
    /// One choke point, so no path to a transform can bypass the constraint.
    pub fn axis_of(&self) -> Vec3 {
        match self.axis_mode {
            AxisMode::InPlane => {
                Vec3::new(self.axis.x, self.axis.y, 0.0).normalize_or(Vec3::Y)
            }
            AxisMode::Fixed(v) => v.normalize_or(Vec3::Z),
        }
    }

    /// The rotation a map actually gets, with the out-of-plane components
    /// scaled by `depth`.
    ///
    /// Applied here rather than in the proposal so there is exactly one place
    /// the constraint can be enforced and nowhere it can be forgotten. The
    /// cost is that a proposal may nudge a component that is currently
    /// multiplied by zero; the benefit is that no path to a transform can
    /// bypass the prior.
    fn rotvec_of(&self, m: &MapParams) -> Vec3 {
        Vec3::new(m.rotvec.x * self.depth, m.rotvec.y * self.depth, m.rotvec.z)
    }

    pub fn colors(&self) -> Vec<Vec3> {
        self.maps.iter().map(|m| m.color.clamp(Vec3::ZERO, Vec3::ONE)).collect()
    }
}

/// A walked attractor: the 3D cloud, and its orthographic shadow with the
/// mixed colour each point arrived carrying.
pub struct Walk {
    pub world: Vec<Vec3>,
    pub pos: Vec<Vec2>,
    pub col: Vec<Vec3>,
}

/// Run the chaos game and project it.
///
/// The projection is orthographic down −z, which is the same code a 2D fit
/// would be and costs nothing extra. It leaves z entirely unconstrained — and
/// that is depth hallucination (LIKENESS-PLAN.md §0.5) rather than a gap: the
/// result reads as itself from one angle and is an abstract solid from every
/// other.
pub fn walk(params: &Params, n: usize, rng: &mut impl Rng) -> Option<Walk> {
    let transforms = params.to_transforms();
    let colors = params.colors();
    let enabled = vec![true; transforms.len()];

    let per = n.div_ceil(TRAJECTORIES);
    let mut world = Vec::with_capacity(n);
    let mut pos = Vec::with_capacity(n);
    let mut col = Vec::with_capacity(n);

    for _ in 0..TRAJECTORIES {
        let mut walker = Walker::new(&transforms, &enabled, rng)?;
        let mut rgb = Vec3::splat(0.5);
        for _ in 0..BURN_IN_STEPS {
            let idx = walker.step(rng);
            rgb = rgb.lerp(colors[idx], params.color_speed);
        }
        for _ in 0..per {
            if world.len() == n {
                break;
            }
            let idx = walker.step(rng);
            rgb = rgb.lerp(colors[idx], params.color_speed);
            let p = walker.pos;
            if !p.is_finite() {
                return None;
            }
            world.push(p);
            pos.push(Vec2::new(p.x, p.y));
            col.push(rgb);
        }
    }
    (world.len() == n).then_some(Walk { world, pos, col })
}

pub struct Config {
    pub maps: usize,
    pub iters: usize,
    /// Random starts scored before annealing begins. Cheap, and the annealer
    /// is local enough that where it starts matters more than how long it runs.
    pub inits: usize,
    /// Points walked per evaluation. Small on purpose — this is the whole
    /// reason a gradient-free fit is affordable; save the big cloud for
    /// looking at the answer.
    pub points: usize,
    /// Directions drawn per evaluation.
    pub slices: usize,
    pub seed: u64,
    /// See [`Params::depth`].
    pub depth: f32,
    /// See [`Params::symmetry`].
    pub symmetry: Option<OrbitKind>,
    /// See [`Params::axis_mode`].
    pub axis_mode: AxisMode,
    /// See [`Params::nonlinear`].
    pub nonlinear: f32,
    /// Similarity dimension the fit is pushed *below*, if any.
    ///
    /// The answer to a problem the other dials could not touch: every
    /// improvement to the loss made the pictures mushier. That is not a bug in
    /// any one term, it is what the objective wants — the target is a sampled
    /// photograph, its density is smooth, and the smoothest reachable
    /// attractor scores best. Lace, dust and holes are *penalised*, because
    /// the reference does not have them.
    ///
    /// So the fractal character has to be asked for directly rather than
    /// hoped for as a side effect of starvation. `Σsᵢᵈ = 1` is already
    /// computed for `--info`; pulling `d` down forces the same silhouette to
    /// be built out of less material, which is the difference between a cloud
    /// and a filigree.
    ///
    /// One-sided: a fit that is *already* sparser than asked is left alone.
    /// Nothing here ever wants to push dimension up.
    pub dimension: Option<f32>,
}

/// How hard [`Config::dimension`] pulls, per unit of excess, squared.
///
/// Both loss terms are normalised to about 1 for an uninformative cloud, so
/// this is on the same scale as the whole rest of the objective: half a
/// dimension over budget costs about 6% of a hopeless fit, a full dimension
/// about 25%.
const DIMENSION_WEIGHT: f32 = 0.25;

impl Default for Config {
    fn default() -> Self {
        Self {
            maps: 6,
            iters: 4000,
            inits: 400,
            points: 4096,
            slices: 32,
            seed: 0,
            depth: 0.0,
            symmetry: None,
            axis_mode: AxisMode::InPlane,
            nonlinear: 0.0,
            dimension: None,
        }
    }
}

pub struct Outcome {
    pub params: Params,
    pub loss: f32,
    /// Similarity dimension of the winner, for the log line — this is the
    /// number that says whether the result is lace or cloud.
    pub dimension: Option<f32>,
    /// Loss of the best random start, so the log can say what the annealing
    /// was worth on top of the dice.
    pub initial_loss: f32,
}

/// Re-draw the chaos-game stream every this many steps.
///
/// The loss is stochastic — a fresh walk each evaluation — and a plain
/// accept/reject against a moving number accepts noise as often as progress.
/// Holding the stream for a block makes comparisons within it exact, and
/// rotating it between blocks keeps the fit from settling into whichever
/// trajectories one seed happened to draw. (The paper leans on that noise
/// deliberately; we want it, just not inside a single comparison.)
const BLOCK: usize = 20;

pub fn fit(target: &loss::Target, cfg: &Config) -> Outcome {
    use rand::SeedableRng;
    let mut rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(cfg.seed);
    let mut scratch = Vec::new();

    // One `stream` fixes both halves of the randomness an evaluation has: the
    // chaos-game trajectories and the slice directions. Two candidates scored
    // under the same stream differ only by their parameters.
    let mut measure = |p: &Params, stream: u64| {
        let mut walk_rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(stream);
        match walk(p, target.n, &mut walk_rng) {
            Some(w) => {
                let w = super::normalize_walk(w);
                target.eval(&w.pos, &w.col, cfg.slices, stream, &mut scratch)
                    + dimension_penalty(p, cfg.dimension)
            }
            None => f32::INFINITY,
        }
    };

    // Random starts. A quarter of the whole budget goes here because the
    // annealer only rearranges what it was handed: at six maps most of the
    // basins are hopeless and the cheapest way past them is to roll again.
    let mut cur = Params::random(cfg.maps, cfg.depth, cfg.symmetry, cfg.axis_mode, cfg.nonlinear, &mut rng);
    let mut cur_loss = f32::INFINITY;
    for _ in 0..cfg.inits {
        let candidate = Params::random(cfg.maps, cfg.depth, cfg.symmetry, cfg.axis_mode, cfg.nonlinear, &mut rng);
        let l = measure(&candidate, cfg.seed ^ 0x9e37);
        if l < cur_loss {
            cur_loss = l;
            cur = candidate;
        }
    }
    let initial_loss = cur_loss;

    let mut best = cur.clone();
    let mut best_loss = cur_loss;

    for i in 0..cfg.iters {
        let t = i as f32 / cfg.iters.max(1) as f32;
        // Both schedules are relative — the temperature to the current loss,
        // the step to the parameter's own natural range — so neither has to
        // know what scale the loss came out at for a given reference.
        let temp = cur_loss * lerp(0.12, 0.001, t);
        let step = lerp(1.0, 0.06, t);

        let stream = cfg.seed.wrapping_add((i / BLOCK) as u64).wrapping_mul(0x9e3779b9);
        if i % BLOCK == 0 {
            // New stream: the incumbent has to be re-measured against it or
            // the next comparison is between two different questions.
            cur_loss = measure(&cur, stream);
        }

        let candidate = propose(&cur, step, &mut rng);
        let l = measure(&candidate, stream);

        let delta = l - cur_loss;
        let accept = delta <= 0.0
            || (temp > 0.0 && rng.r#gen::<f32>() < (-delta / temp).exp());
        if accept {
            cur = candidate;
            cur_loss = l;
            if cur_loss < best_loss {
                best_loss = cur_loss;
                best = cur.clone();
            }
        }
    }

    // The running best was picked under a single noisy stream, so it is
    // optimistic by construction. Score both survivors properly and let the
    // honest number decide which one gets emitted.
    let mut verify = |p: &Params| {
        let mut total = 0.0;
        for s in 0..4u64 {
            let stream = 0xf17 ^ s;
            let mut walk_rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(stream);
            total += match walk(p, target.n, &mut walk_rng) {
                Some(w) => {
                    let w = super::normalize_walk(w);
                    target.eval(&w.pos, &w.col, 96, stream, &mut scratch)
                }
                None => f32::INFINITY,
            };
        }
        total / 4.0
    };
    let best_score = verify(&best);
    let cur_score = verify(&cur);
    let (params, loss) =
        if cur_score < best_score { (cur, cur_score) } else { (best, best_score) };

    let dimension = crate::scene::similarity_dimension(&params.to_transforms());
    Outcome { params, loss, initial_loss, dimension }
}

/// What this parameter vector's similarity dimension costs, if a budget was set.
///
/// `None` from `similarity_dimension` means the maps do not form a sensible
/// set to solve `Σsᵢᵈ = 1` over — every map non-contractive, or none weighted.
/// The walk would already have been rejected in that case, so there is nothing
/// to add.
pub fn dimension_penalty(p: &Params, budget: Option<f32>) -> f32 {
    let Some(budget) = budget else { return 0.0 };
    let Some(d) = crate::scene::similarity_dimension(&p.to_transforms()) else {
        return 0.0;
    };
    let excess = (d - budget).max(0.0);
    DIMENSION_WEIGHT * excess * excess
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// One annealing move.
///
/// Mostly a nudge to one or two maps; occasionally a whole map is thrown away
/// and re-rolled. The big move matters more than it looks: with six maps a
/// single one stuck in a useless corner costs a sixth of the picture, and no
/// amount of nudging walks it across the frame.
fn propose(cur: &Params, step: f32, rng: &mut impl Rng) -> Params {
    let mut next = cur.clone();
    let n = next.maps.len();

    if rng.r#gen::<f32>() < 0.04 {
        let i = rng.gen_range(0..n);
        next.maps[i] = Params::random(1, next.depth, next.symmetry, next.axis_mode, next.nonlinear, rng).maps[0];
        return next;
    }
    // The axis is one parameter for the whole scene rather than one per map,
    // and it changes every copy at once, so it gets its own slot rather than
    // competing for a map's turn.
    if next.symmetry.is_some()
        && next.axis_mode == AxisMode::InPlane
        && rng.r#gen::<f32>() < 0.10
    {
        // In the plane, so the tilt of the subject is the only freedom.
        let jitter = Vec3::new(gauss(rng), gauss(rng), 0.0) * 0.12 * step;
        next.axis = (next.axis + jitter).normalize_or(Vec3::Y);
        return next;
    }
    if rng.r#gen::<f32>() < 0.06 {
        next.color_speed =
            (next.color_speed + gauss(rng) * 0.15 * step).clamp(*COLOR_SPEED.start(), *COLOR_SPEED.end());
        return next;
    }

    let ceiling = next.nonlinear;
    let touched = if rng.r#gen::<f32>() < 0.7 { 1 } else { 2.min(n) };
    for _ in 0..touched {
        let m = &mut next.maps[rng.gen_range(0..n)];
        let jitter = |rng: &mut dyn RngCore, s: f32| -> Vec3 {
            Vec3::new(gauss(rng), gauss(rng), gauss(rng)) * s * step
        };
        match rng.gen_range(0..6) {
            0 => m.translation += jitter(rng, 0.22),
            1 => m.rotvec += jitter(rng, 0.35),
            2 => m.log_scale += jitter(rng, 0.25),
            3 => m.log_weight += gauss(rng) * 0.3 * step,
            4 => m.color = (m.color + jitter(rng, 0.2)).clamp(Vec3::ZERO, Vec3::ONE),
            _ if ceiling <= 0.0 => m.translation += jitter(rng, 0.22),
            // Which nonlinearity is discrete, so it cannot be annealed into
            // gradually — a third of the time, jump outright. The rest of the
            // time walk the amount, which is where the fine tuning lives.
            _ if rng.r#gen::<f32>() < 0.33 => m.variation = rng.gen_range(0..PALETTE.len()),
            _ => m.var_amount = (m.var_amount + gauss(rng) * 0.15 * step).clamp(0.0, ceiling),
        }
    }
    next
}

use rand::RngCore;

/// Standard normal by Box–Muller. `rand`'s distributions would do, but this
/// keeps the proposal readable next to the scales it is multiplied by.
fn gauss(rng: &mut dyn RngCore) -> f32 {
    let u1 = ((rng.next_u32() as f64 + 1.0) / (u32::MAX as f64 + 2.0)) as f32;
    let u2 = (rng.next_u32() as f64 / u32::MAX as f64) as f32;
    (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    /// The Sierpinski gasket, exactly: three half-scale maps to the corners of
    /// a triangle, no rotation.
    ///
    /// A ground truth the fitter is *entitled* to reach — three maps
    /// represent it with zero error — which is what makes it the right probe
    /// for telling a weak search apart from a flat objective.
    fn sierpinski() -> Params {
        let corners = [
            Vec2::new(0.0, 0.9),
            Vec2::new(-0.85, -0.6),
            Vec2::new(0.85, -0.6),
        ];
        Params {
            maps: corners
                .iter()
                .map(|v| MapParams {
                    // f(p) = (p + v)/2, so the offset is v/2 at scale 1/2.
                    translation: Vec3::new(v.x * 0.5, v.y * 0.5, 0.0),
                    rotvec: Vec3::ZERO,
                    log_scale: Vec3::splat(0.5f32.ln()),
                    log_weight: 0.0,
                    color: Vec3::splat(0.5),
                    variation: 0,
                    var_amount: 0.0,
                })
                .collect(),
            color_speed: 0.6,
            depth: 0.0,
            symmetry: None,
            axis_mode: AxisMode::InPlane,
            nonlinear: 0.0,
            axis: Vec3::Y,
        }
    }

    /// Score `p` against `target` under its own independent stream.
    fn score(target: &loss::Target, p: &Params, stream: u64) -> f32 {
        let mut walk_rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(stream);
        let w = super::super::normalize_walk(walk(p, target.n, &mut walk_rng).unwrap());
        target.eval(&w.pos, &w.col, 128, stream, &mut Vec::new())
    }

    /// Does the loss actually have a basin at a solution we know is exact?
    ///
    /// This exists to answer one question that decides where effort goes: the
    /// fitter stalls two orders of magnitude short of a known zero, and that
    /// is consistent with either a search too weak to find the basin or an
    /// objective too flat to have one. They have opposite fixes, so guessing
    /// is expensive.
    ///
    /// Geometry only — colour is switched off, because the thing that is
    /// failing is shape and a colour term would only add noise to the reading.
    #[test]
    fn the_loss_has_a_basin_at_a_known_exact_solution() {
        let truth = sierpinski();
        let n = 4096;

        // The target *is* the exact attractor, sampled through the same walk
        // the fit uses. No image round-trip: this isolates the loss and the
        // search from the density map, the saliency weight and the resampling.
        let mut rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(0xdec0de);
        let cloud = super::super::normalize_walk(walk(&truth, 60_000, &mut rng).unwrap());
        // The blend the fitter actually runs, so the probe measures the
        // landscape that is really being searched.
        let target =
            loss::Target::new(&cloud.pos, &cloud.col, n, 0.0, loss::DEFAULT_GRID_SHARE, &mut rng);

        // The noise floor: the exact parameters against themselves, under a
        // stream the target did not come from. No fit can beat this, and a
        // loss that cannot separate anything from it is measuring nothing.
        let floor: f32 =
            (0..5).map(|s| score(&target, &truth, 1000 + s)).sum::<f32>() / 5.0;

        // How the loss rises as the truth is walked away from.
        let mut curve = Vec::new();
        for sigma in [0.02f32, 0.05, 0.1, 0.2, 0.4] {
            let mut worst = 0.0f32;
            let mut total = 0.0f32;
            const TRIALS: usize = 8;
            for t in 0..TRIALS {
                let mut r = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(
                    (sigma * 1000.0) as u64 * 97 + t as u64,
                );
                let mut p = truth.clone();
                for m in &mut p.maps {
                    m.translation +=
                        Vec3::new(gauss(&mut r), gauss(&mut r), 0.0) * sigma;
                    m.rotvec.z += gauss(&mut r) * sigma;
                    m.log_scale += Vec3::splat(gauss(&mut r) * sigma);
                }
                let l = score(&target, &p, 2000 + t as u64);
                total += l;
                worst = worst.max(l);
            }
            curve.push((sigma, total / TRIALS as f32, worst));
        }

        // What the shipped fitter actually reaches on this same target.
        let cfg = Config { maps: 3, depth: 0.0, ..Default::default() };
        let fitted = fit(&target, &cfg);
        let fitted_honest = score(&target, &fitted.params, 3001);

        println!("\n  loss landscape at a known-exact 3-map solution");
        println!("  ---------------------------------------------");
        println!("  noise floor (truth vs itself)      {:.6}", floor);
        for (sigma, mean, worst) in &curve {
            println!(
                "  truth perturbed by sigma {:<5}     {:.6}   (worst {:.6})",
                sigma, mean, worst
            );
        }
        println!("  best random start of {}            {:.6}", cfg.inits, fitted.initial_loss);
        println!("  what the fitter reaches            {:.6}", fitted_honest);
        println!(
            "  fitter is {:.0}x the floor\n",
            fitted_honest / floor.max(1e-9)
        );

        // The landscape claim: the exact solution is meaningfully better than
        // its own neighbourhood. If this fails, the objective is flat and no
        // optimiser will help.
        let nearest = curve[0].1;
        assert!(
            nearest > floor * 2.0,
            "loss cannot tell the exact solution ({floor:.6}) from a 0.02 perturbation \
             ({nearest:.6}) — the objective is flat, not the search"
        );
    }

    #[test]
    fn scales_are_always_contractive() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        for _ in 0..200 {
            let mut p = Params::random(4, 1.0, None, AxisMode::InPlane, 0.0, &mut rng);
            // Even an optimiser that has run away must not produce a map that
            // expands: the walk would diverge and the scene would not render.
            p.maps[0].log_scale = Vec3::splat(9.0);
            p.maps[1].log_scale = Vec3::new(-40.0, 3.0, 0.0);
            for m in &p.maps {
                let s = Params::scale_of(m);
                assert!(s.max_element() <= MAX_SINGULAR + 1e-5, "{s:?} expands");
                assert!(s.min_element() >= MIN_SINGULAR - 1e-5, "{s:?} collapses");
            }
        }
    }

    #[test]
    fn a_walk_produces_the_requested_cloud() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(2);
        let p = Params::random(5, 1.0, None, AxisMode::InPlane, 0.0, &mut rng);
        let w = walk(&p, 2000, &mut rng).expect("contractive maps must walk");
        assert_eq!(w.pos.len(), 2000);
        assert_eq!(w.col.len(), 2000);
        assert!(w.world.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn no_fitted_map_can_shear() {
        // The scene format has nowhere to write a shear, so a fitted matrix
        // must decompose exactly into the scale/rotation/translation the
        // loader reads back. Anything else emits a scene that silently
        // differs from what was fitted.
        let mut rng = rand::rngs::StdRng::seed_from_u64(5);
        let p = Params::random(6, 1.0, None, AxisMode::InPlane, 0.0, &mut rng);
        for t in p.to_transforms() {
            let (s, r, tr) = t.matrix.to_scale_rotation_translation();
            let rebuilt = Mat4::from_scale_rotation_translation(s, r, tr);
            let diff: f32 = (0..4)
                .map(|c| (rebuilt.col(c) - t.matrix.col(c)).abs().max_element())
                .fold(0.0, f32::max);
            assert!(diff < 1e-4, "matrix does not survive decomposition: {diff}");
        }
    }
}
