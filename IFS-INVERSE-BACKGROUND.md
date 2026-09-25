# The IFS inverse problem: what's known, what's possible, what to expect

Background for anyone (human or agent) working on `src/fit/`. This is domain knowledge,
not a plan — LIKENESS-PLAN.md is the plan. The point here is to make the failure modes
*predictable*, so that when a fit comes out badly you can tell which of three things
happened: you hit a theorem, you hit a bug, or you picked a method that was never going
to work.

## 1. Three different problems share this name

Conflating these is the most common way to get lost.

**(a) Global IFS inversion.** Given an image, find N contractive maps whose attractor (or
invariant measure) looks like it. Nonlinear, non-local, non-convex. **Unsolved for
non-self-similar images, after 40 years.** This is what `src/fit/` attempts.

**(b) Partitioned IFS (PIFS) / fractal compression.** Cut the image into range blocks;
for each, search a dictionary of larger domain blocks for one that maps onto it under a
contractive grey-level affine map. **Solved, shipped commercially.** It works because
partitioning throws away the global coupling and leaves thousands of independent
brute-force searches. Its output is an image, not a point cloud, and its self-similarity
is local and blocky.

**(c) Forward generation.** Sampling IFS parameter space for interesting attractors.
Trivially easy, which is why fractal art exists and fractal *inversion* doesn't.

The literature slides between (a) and (b) constantly, and headline claims of "fractal
image compression works" are always about (b).

## 2. The theory, and exactly where it stops helping

**Hutchinson (1981).** For contractions `w_1..w_N` with ratios `s_i < 1`, the operator
`W(S) = ∪ w_i(S)` is a contraction on compact sets under the Hausdorff metric, so it has a
unique fixed point `A` — the attractor. With probabilities you get a unique invariant
measure (Barnsley–Demko), which is what the chaos game samples and what you actually
render.

**The Collage Theorem** is the entire reason an inverse-problem literature exists:

    d_H(T, A)  ≤  d_H(T, W(T)) / (1 − s)

It replaces "the attractor should look like the target" — where the attractor is an
implicit fixed point — with "one application of W to the target should look like the
target", which is explicit, cheap, and differentiable. Every classical method is built on
this substitution.

**Where it betrays you:** the `1/(1−s)` factor. Detail and coverage require `s` near 1.
At `s = 0.9` the bound multiplies collage error by 10; at `s = 0.95`, by 20. A collage
error of 1% — which is a *very* good collage — buys an attractor bound of 10–20% in
Hausdorff distance, which on a unit square is no bound at all. So the collage theorem is a
qualitative guide that tells you a good collage is worth pursuing, and it is *not* a
quantitative guarantee that a good collage yields a good attractor. Papers that report
collage error as their metric are reporting a number that does not control the thing you
care about.

**Fractal flames are outside this theory entirely.** Variations are not contractions,
there is no fixed-point theorem, there may be no attractor, and divergent points are simply
culled. You get no collage bound at all, so you must optimize the rendered result directly.

## 3. Six structural reasons it's hard

Not "it's a hard optimization" — these are specific mechanisms, and each has a signature.

**3.1 There is no locality.** Move one map slightly and the *entire* attractor
redistributes, because almost every point in the attractor uses almost every map
infinitely often. Compare a Gaussian splat: ~9 parameters, but it affects a bounded
region, so error decomposes into independent local subproblems. An affine IFS map has ~6
parameters and affects everything. Similar DOF count, utterly different conditioning.
**This is the deepest reason gradient descent struggles, and it's exactly what PIFS
sidesteps** by imposing locality artificially.

**3.2 The loss landscape is itself fractal.** Parameter → image is continuous (Lipschitz,
constant ~`1/(1−s)`) but its fine structure oscillates at every scale: perturb a
translation by ε and the attractor's detail at scale ε reorganizes completely. Gradients
computed at full resolution are close to noise. This is why every working method
low-passes the landscape — coarse-to-fine schedules, large splat kernels, low training
resolution, or a transport loss. Bannister & Nowrouzezahrai found training *resolution*
was a dominant hyperparameter for exactly this reason; the splat kernel is fixed in pixels,
so lower resolution means more blur means usable gradients.

**3.3 Dimension is a knife-edge.** For similarity maps satisfying the open set condition,
the attractor's dimension `d` solves the Moran equation `Σ sᵢᵈ = 1`. To fill a 2D region
you need `Σ sᵢ² = 1` — with N equal maps, `s = N^(−1/2)`. Below that you get measure-zero
dust; above it you get overlap. And "filling" additionally requires the maps to *tile*
nearly exactly (Sierpinski works because 3 maps at s=1/2 exactly tile a triangle; 4 at
s=1/2 exactly tile a square). Generic optimized maps overlap sloppily, so density becomes
wildly uneven — hot spots and voids. **No amount of optimizer quality fixes this; it's
structural.** It is the reason fitted IFS look lacy and ghosted rather than photographic.

**3.4 IFS measures are natively singular; images are natively smooth.** Getting an IFS
invariant measure to have a smooth density is genuinely delicate mathematics, not an
engineering detail. The canonical example is Bernoulli convolutions — the simplest
possible overlapping IFS, two maps on a line — where Erdős showed singularity for Pisot
ratios in 1939 and Solomyak only established absolute continuity for a.e. ratio in
(1/2,1) in 1995. Fifty-six years for two maps in one dimension. Meanwhile a photograph
is ~99% smoothly-varying density. **Fitting an IFS to a photo is fitting a Cantor measure
to a Gaussian.** This single fact explains most of the perceptual gap.

**3.5 Maps collapse, and nothing punishes it.** Two maps can converge to the same map, or
one map's image can be swallowed inside another's. Capacity silently vanishes: you asked
for 8 maps, 3 are doing work and 5 are stacked. This is the most common *diagnosable*
optimizer pathology and it needs an explicit regularizer or a repulsion term.

**3.6 There is no incremental refinement.** Adding a basis function to a Fourier or
wavelet expansion strictly refines the approximation. Adding a map to an IFS *replaces*
the attractor. So greedy construction — the technique that tames nearly every other
approximation problem — doesn't work natively. You can force it with heavy regularization
(pin the existing maps, add one, re-fit), and it's worth doing, but you're fighting the
mathematics rather than using it.

## 4. Why "how many maps" has a hard floor

Two independent estimates for a 500×700 image, which agree:

- **Entropy.** A decently-compressed card is ~100 KB ≈ 10⁶ bits. A flame map carries maybe
  12–15 meaningful numbers ≈ ~200 bits. → **~5,000 maps** for JPEG-grade content.
- **PIFS cross-check.** ~5,500 range blocks at 8×8, each ~30 bits (domain coords, isometry,
  contrast, brightness) ≈ 20 KB. Same order.

Inverted: **6 maps ≈ 1,200 bits ≈ about a 12×12 greyscale thumbnail of information.**

But the bit count *understates* the problem, because IFS parameters are not freely
spendable. An IFS with N maps at contraction s produces `N^k` copies at scale `s^k`, so at
resolution ε it has `ε^(−d)` distinguishable features where `d = log N / log(1/s)`. That is
as many features as any d-dimensional set — but all of them are determined by 6N numbers,
and *which* features you get is dictated by the recursion, not chosen. **The information
lives in the structure and is unavailable to spend on the target.** This is the real reason
map count doesn't buy likeness the way parameter count buys likeness in a splat model.

## 5. Forty years of attempts

- **Barnsley (1985–88).** Hand collage. Fern, leaf, tree. Every famous IFS likeness is
  hand-fit, including the fern.
- **Barnsley & Sloan → Iterated Systems → FIF.** Shipped in Microsoft Encarta. PIFS.
  Patent-encumbered, which suppressed adoption independent of technical merit.
- **Jacquin (1990 thesis, 1992 IEEE TIP).** Automated PIFS. The commercial reality of the
  field. Asymmetric: slow encode, fast decode.
- **Fisher, *Fractal Image Compression: Theory and Application* (1995).** The reference
  book, with working C. Quadtree and HV partitioning.
- **GNU FIASCO** (Hafner), shipped in netpbm — an installable working codec today.
- **Moment matching** (Vrscay & Roehrig). IFS-measure moments satisfy a linear recursion,
  so you can solve algebraically. Elegant, ill-conditioned, and moments are a poor
  perceptual descriptor. 1D toys.
- **Evolutionary methods.** Nettleton & Garigliano (1994); Lutton, Lévy Véhel et al.,
  "Mixed IFS" via genetic programming (1995); many others through the 2000s. High paper
  count, no benchmark, no cumulative progress, all demos on self-similar targets. Treat
  this subfield as a graveyard rather than a literature to mine.
- **Davis (1998), "A wavelet-based analysis of fractal image compression".** Showed PIFS is
  essentially a crude wavelet-subtree quantization scheme, and that ordinary wavelet coders
  beat it. **This is the paper that ended fractal compression as a serious codec.** JPEG2000
  does the job better. The surviving niche is resolution-independent decoding
  ("fractal zoom", the Genuine Fractals upscaling pitch) — not compression ratio.
- **Modern era.** Djeacoumar et al., *Learning Image Fractals Using Chaotic Differentiable
  Point Splatting* (EG 2025) — current SOTA for (a), via differentiable splatting plus
  hybrid Adam/annealing, SVD-parameterized maps with sigmoid'd singular values for
  guaranteed contractivity. Bannister & Nowrouzezahrai, *Learnable Fractal Flames* (2025) —
  differentiable flames, explicitly art-directed rather than fidelity-directed. Note that
  even SOTA figures are ferns, dragons and Sierpinski variants: **self-similar targets.**

The honest summary of the trendline: whenever anyone needed real image fidelity, they
stopped using IFS. Global IFS inversion has settled into being a toy-and-art problem, and
the 2025 papers are best read as *art tools that work*, not as fidelity results.

## 6. What to expect, calibrated

- **Self-similar or branching subjects** (ferns, trees, coastlines, lightning, romanesco,
  crystals, ornament): genuinely good. This is where every good published figure lives.
- **Silhouette / outer boundary of an arbitrary shape:** achievable. IFS are good at
  boundaries.
- **Gross mass distribution of an arbitrary subject with 3–10 affine maps:** achievable in
  minutes. Reads as "the subject assembled from ghosts of itself."
- **Interior structure and smooth density:** structurally hard (§3.3, §3.4). Expect lace
  and hot spots.
- **Photographic likeness at any map count an optimizer can reach:** no. Not a solver
  problem.
- **10–30 maps:** more structure, sharply diminishing returns, and map collapse (§3.5)
  starts eating the budget.
- **Colour:** disproportionately valuable. Human recognition of a card is heavily
  colour-and-layout driven, so colour buys more recognisability per parameter than geometry
  does.
- **Runtime is not the bottleneck.** Bannister: 1–2 min simple, ~20 min for painting
  emulation on a 2070 Super, at 200×200 training resolution with plain MSE and plain SGD.
  Djeacoumar: ~250 ms/iter at 500k points on an A40. The bottleneck is *which local
  minimum*, not how long to reach one.
- **Seeds matter more than optimizers.** Bannister state plainly that the landscape is
  non-convex and different seeds give different final fractals rather than a common
  optimum. Plan for sweeps and contact sheets, not for a single good run.

## 7. Diagnostics: which of the three failures is it?

**You hit a theorem** if: the attractor is a plausible, converged, non-degenerate fractal
that simply doesn't resemble the target beyond gross blob placement; the loss plateaus at a
mediocre value; different seeds give different, equally-plausible, equally-unlike results;
the interior is lacy where the target is solid. Nothing is broken. This is §3.3/§3.4 and
more optimizer effort will not move it.

**You hit a bug** if: the attractor is degenerate (a point, a line, empty, diverged); loss
doesn't move at all; loss moves but the picture doesn't; the *saved scene* doesn't
reproduce what the fitter was looking at. That last one is the dangerous silent case —
if the fitter can express something the scene format can't store (shear is the known
example in this codebase), the emitted TOML renders as something else entirely and the
solver looks broken when it isn't. **Always dump the target point cloud to a PNG and look
at it first**; a wrong target is the single most common invisible bug, and it makes every
downstream result meaningless.

**You picked the wrong method** if: the loss is raw pixel MSE at full resolution (no
long-range signal — see §3.2); there's no coarse-to-fine or blur schedule; all parameters
are optimized jointly from random init; there's no contractivity constraint (so candidates
diverge and get culled, silently deleting the gradient); there's no anti-collapse
regularizer (§3.5); or a 3D map set is being fit against a single 2D view without
acknowledging that the null space is enormous and will be filled with invented structure.

The order to check them in: **target cloud → degeneracy → round-trip fidelity of the saved
scene → loss long-range signal → map collapse.** Only after all five are clean is a bad
result evidence about the theory.
