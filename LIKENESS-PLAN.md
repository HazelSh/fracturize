# Likeness: making a fractal *of* something

Fracturize renders fractals. It does not yet turn a *thing* into a fractal — which is
what the card claims to do, and what the token art wants. This plan is for the missing
half: point at a picture of something, get back a scene whose attractor is a fractal
that reads as that thing.

The target is a `scenes/*.toml` like any other. Not a new format, not a locked artifact —
an ordinary IFS the transform panel can open, the gizmos can push around, `mutate` can
wreck and `renorm` can zoom into. The solver is an *authoring assist*, and it hands off.

## 0. The aesthetic is the spec

The usual framing of this problem — "the IFS inverse problem" — measures success as
fidelity. That's the wrong objective here. We want the representation to *visibly fail*,
and to fail in a way that is legible as fractal-ness rather than as noise. A perfect
reconstruction would be a photograph, and would be worthless: nothing about it would say
"this thing was made of copies of itself".

So the governing design principle:

> **Never give the solver the ability to succeed.** Choose a map budget and a map class
> too small to hold the subject, and spend the residual error deliberately.

That flips a research problem (hard, unsolved in general) into an art problem (tractable,
and the interesting bit). It also means the parts that are cheap to build are the parts
that matter, and the expensive fidelity machinery is optional.

### The artifact palette

These are the dials. Each one is a *way of being wrong*, and each is either free or a
small term on top of the solver. Ranked by how much picture they buy:

1. **Map starvation.** The master dial. Six maps cannot hold a face; they can hold a
   face's worth of echoes. What comes out is the subject assembled from ghosts of itself
   at three or four scales. This artifact is free — it is just `--maps 5` — and it is
   probably 70% of the whole look.

2. **Dimension pressure.** Add a term pulling `similarity_dimension` below what the
   target actually needs. A solid silhouette wants d ≈ 2; ask for 1.6 and the solver must
   render it as lace, dust, Sierpinski holes. We already compute this (`scene.rs`) and
   `trace::lacunarity_spectrum` measures how the holes are distributed, so this is a
   loss term over machinery that exists. Steerable *gappiness*.

3. **Nonlinear contamination — the fracturize dial.** Fit with pure affine maps, then
   blend variations in (`spherical`, `julia`, `bulb`, `absfold`) with a single amount
   slider. At 0 it's the likeness; at 1 it's a flame. The subject dissolves into a flame
   *of itself* along the way, and the interesting frames are around 0.2–0.4. Two ways to
   drive it: post-hoc (a slider, no solver involved) or annealed (re-fit at each step so
   the shape fights to survive the warp). Do the slider first, it's nearly free. (The
   useful range is a guess until we've looked — the slider exists to find it.)

4. **Forced symmetry.** Fit the target *under* a group — `C5`, `D4`, icosahedral — so the
   solver only gets a quotient of the subject and the group rebuilds the rest. The thing
   comes back as an ornament of itself. Native symmetry already exists and doesn't expand
   the transform list, so a C5-quotient fit of a wing is still a five-line scene. This is
   the one that most looks like *card art*: the Core Fractal proxy is a crystal, and a
   crystal is a symmetric quotient of something.

5. **Depth hallucination.** Fracturize is 3D; a card is one view. Fitting 3D maps against
   one view is wildly underdetermined, and the solver will invent
   structure in z. That is not a bug to fix, it's the best trick available: an object that
   only reads as itself from one angle and is an abstract fractal solid from every other.
   Anamorphic. Control it by *adding* views (2–3 silhouettes at different yaws tames it;
   one view lets it run).

6. **Scale dishonesty.** The fit is only valid at the scale it was fit at. Zoom in and the
   subject comes apart into copies of itself. We already have infinite zoom and `renorm`,
   so this artifact is a camera path, not code — and it's the most explicitly "fractal"
   thing in the list. Fracturize the whole card frame, zoom the gem, find the card again.

7. **Colour by provenance.** Colour is what keeps a subject recognisable after its
   geometry stops being recognisable, so it should not be an afterthought. Carry RGB on
   the target points — each sample keeps the reference colour where it was drawn — and the
   target is `(x, y, r, g, b)`. Sliced Wasserstein in 5D costs exactly what it costs in
   2D, so shape and colour can be matched in *one* loss with one weight between them,
   which also makes "match the shape, ignore the palette" and the reverse into dial
   positions rather than different programs. Then read each map's colour off its fixed
   point and switch to `ColorMode::Mix`, so combinations of maps get mixed colours.

   A post-hoc colorise pass over a monochrome fit stays available and is simpler; the
   reason to prefer the 5D target is that it lets colour *drive* geometry — maps get
   pulled toward regions that match their colour, so the brass and the cyan core end up
   as separate structures rather than one structure tinted afterwards.

8. **Early stopping / greedy assembly.** If maps are added one at a time (slice 4), every
   intermediate is a legitimate output, and the good one is usually not the last one.
   Ship the whole ladder; let the eye pick.

9. **Hybridising.** Interpolate the fitted IFS toward a hand-authored scene — half a
   powercell, half a lacewing. `mutate.rs` already has the machinery for moving through
   transform space.

Dials 1, 3, 7 and 8 need nothing but the crudest solver. That's the argument for shipping
a bad solver early.

## 1. Technical approaches

Six routes to "find some maps". Honest assessment of each, then a recommendation.

### A. Collage authoring — no solver at all

Barnsley's original method. Overlay the reference in the viewport, and place transforms by
hand so that the union of transformed copies of the target's silhouette covers the target.
The Collage Theorem bounds the result: `d_H(A, T) ≤ collage_error / (1 − s)`, so a sloppy
collage with strongly contractive maps is still close-ish, and you can *see* the error
while you work.

- **For:** zero new maths. Uses the gizmos, the transform panel, undo. Always under
  artistic control. Works in 3D against several reference views at once. The human's
  sloppiness *is* the artifact, and it's a sloppiness with taste in it.
- **Against:** laborious. Hard for organic subjects. Doesn't scale to a batch of tokens.
- **Verdict:** build the substrate for this regardless (reference image in the viewport),
  because every other approach needs the same substrate, and this one turns it on for
  free.

### B. Gradient-free fit around the existing renderer

Wrap CMA-ES or the paper's simulated annealing around a low-res render of the existing
GPU chaos game. ~9–12 parameters per map; six maps is ~60 dimensions, which CMA-ES eats.

- **For:** no autodiff, no new forward model — `offline.rs` already renders a scene to a
  buffer. Accepts *any* loss, including the non-differentiable art terms (dimension
  bands, lacunarity, "must contain a map with contraction < 0.2").
- **Against:** thousands of evaluations, each a GPU fill; convergence is loose.
- **Verdict:** good as the first engine precisely because it's loose. Keep it as the
  fallback path even after gradients land — it can optimise things gradients can't.

### C. Optimal-transport fit on point clouds — **recommended first real engine**

Skip images entirely. Sample the reference into a target *point cloud* (§4 for the density
map, §5 for subject weighting), and measure the attractor cloud against it with sliced
Wasserstein or Chamfer distance. Both are differentiable, cheap, and need no rasteriser.

For a 2D reference against a 3D attractor: project the attractor through the camera and
run the loss in the image plane. Several views = sum over views (§0.5).

- **For:** far less machinery than differentiable splatting — no rasteriser backward pass,
  no LPIPS, no alpha-blending gradients. Natively 3D, so a 3D reference (a mesh, a scan,
  another fractal) needs no special case. Sliced Wasserstein is a projection and a sort.
  Its failure mode is *blurry and shape-y* — "right silhouette, wrong substance" — which
  is exactly the aesthetic; a pixel loss fails toward noise instead.
- **Against:** less sharp likeness than the image-space objective. Insensitive to fine
  interior structure.
- **Gradients:** the chaos game is deep, but you don't need to differentiate all of it.
  The paper's key move (its §4.1) is that trajectories are short: warm up `m` steps
  *detached*, then backprop through a window of `l` steps. Per point that's `l` affine
  maps to replay — so a backward kernel stores the map index chain (one byte per step per
  trajectory) and accumulates ∂L/∂θ over the suffix. That is very implementable in WGSL,
  and implementable on CPU first via `trace.rs`, which is already a faithful port of the
  walker.

### D. Differentiable point splatting — the paper (Djeacoumar et al. 2025)

Already in the repo (`2502.17230v1.pdf`). Image-space loss: multiscale MSE + D-SSIM +
LPIPS + a regulariser, over 500k points splatted as Gaussians at 5× supersampling, with a
hybrid Adam/simulated-annealing optimizer (250 gradient steps, then 10 annealing
candidates, repeat for the first half of training). Their parameterisation is per-map SVD
`M = U S Vᵀ` with sigmoid'd singular values for guaranteed contractivity, tanh'd offsets,
and Gram–Schmidt re-projection after each step — worth stealing wholesale.

- **For:** best likeness of anything here. Their forward model is almost exactly ours: we
  already splat points with a tiled rasteriser and a grade curve. LPIPS genuinely helps
  (their finding, counter-intuitively, on non-natural images).
- **Against:** 2D-affine only as published; extending to 3D means differentiating through
  the camera and the splat. ~250 ms/iteration on an A40 at 500k points, and it wants
  thousands of iterations — on an 8 GB desktop card that's a coffee, on the T490 it isn't
  happening. Real work.
- **Verdict:** the refinement stage, not the first stage. And prototype it in PyTorch
  (`tools/fit_ifs.py`, emitting TOML) before writing a line of WGSL backward pass — the
  autodiff and LPIPS are free there, and the question we need answered is *"is the extra
  sharpness worth it artistically?"*, which a prototype answers.

### E. Fractal image compression (Jacquin / PIFS) — not as the main path, but salvageable

The classic "IFS compression" is *partitioned*: it matches range blocks to domain blocks
with per-block contractive grey maps. Its attractor is an *image*, not a point cloud, so
it does not produce a fracturize scene, and its self-similarity is local and blocky rather
than global. Wrong model for us.

But it's cheap, it's pure CPU, and it hands you a ranked list of the actual self-similar
correspondences in the picture. So: run it, keep the K most-used domain→range maps,
promote them to global IFS maps, and throw the other several thousand away. That's a
**salvage initializer** for B/C — and the throwing-away is precisely the artifact we want.
The image tells you which few self-similarities it has; you keep only those; the rest of
the subject has to be reconstructed out of them or not at all.

### F. Skeleton → L-system, for organic subjects

For plants, wings, feathers, creatures: don't fit pixels. Extract a medial axis / branch
graph from the reference and feed it to `tools/lsystem_to_ifs.py`, which already turns a
bracketed 3D production into trunk-and-branch maps with per-axis scale. The subject comes
back as a *plant version of itself*. Nearly free — the tool exists, and the missing piece
is "skeletonise a PNG", which is a well-trodden CPU operation.

Not general, but for anything branching it will beat the general solver, and it's the
right answer for a set of nature-flavoured tokens.

### Recommendation

Stage it: **A's substrate → C's engine → the artifact dials → D as refinement if the
pictures ask for it.** E and F are cheap side-doors worth a spike each. B stays as the
loss-agnostic fallback.

The reason to lead with C rather than D is not that C is better. It's that C reaches a
picture in one slice, its characteristic failure is the one we want, and it is 3D-native
in an app whose whole point is that its fractals are 3D.

## 2. Slices

Standard rules: each slice ends at something you can look at. Effort is measured in
renders and sweeps, not clocks.

### Slice 0 — the reference substrate

Load an image (`image` is already a dependency) and turn it into a target point set:
crop the art box, pick a density map (§4), weight it by the soft subject separation (§5),
importance-sample ~50–100k points *carrying RGB*, normalise to the paper's convention
(centred, 25% padding). Dump it to a PNG so you can see what the solver will actually be
aiming at, which is the whole deliverable of this slice.

Prototyped and working in `tools/image_to_target.py`; the Rust version is a port, not a
design problem.

Also: the reference as a viewport overlay at adjustable opacity, which turns on approach A
by itself.

### Slice 1 — a bad solver, on purpose

`fracturize fit ref.png --maps 6 -o out.toml`. Loss = sliced Wasserstein between the
projected attractor cloud and the target cloud. Optimiser = annealing only, reusing the
candidate/quality machinery in `randomize.rs` (which already runs a CPU chaos game and
rejects degenerate attractors — that's the gate a fitter needs anyway).

No gradients yet. Expect poor likeness. Ship it anyway, and render a contact sheet of
twenty seeds: the question this slice answers is *does a starved IFS of a real subject
look like anything*, and a sweep answers it better than a good solver would.

#### Concretely, so this can be built without re-deciding it

- **Maps are 3D, the loss is 2D.** Fit ordinary 3D transforms, project the attractor
  orthographically down −z, and measure in the image plane. This is the same code as a
  2D fit and costs nothing extra, but leaves z unconstrained — which is dial 5 rather
  than a gap. A `--views` flag adding yawed projections is how you take it back.
- **Parameter vector**, per map: translation `(3)`, rotation vector `(3)`, log per-axis
  scale `(3)`, weight `(1)`. Ten numbers; six maps is 60 dimensions. Contractivity by a
  saturating map on the scales — clamp the largest singular value below 1 rather than
  each axis, so a squashed map can still be long in one direction (which is how trunk
  and branch maps look).
- **No shear, and that is not optional.** The parameterisation above is `R·diag(s)`,
  which is the paper's `U S Vᵀ` with `V = I` — strictly less general, because it cannot
  shear. That is forced: the scene format has nowhere to write shear, and the codebase
  keeps it out of the gizmo for exactly this reason (`scene.rs`, `app.rs`, and see
  GIZMO-PLAN.md §1.1). A fitter that produced sheared maps would emit scenes that
  silently don't match what was fitted, which is the worst possible failure — invisible.
  It also saves us the paper's Gram–Schmidt re-projection step, since a rotation vector
  is a rotation by construction and cannot drift off the manifold. And restricting the
  map class is a dial-1 move anyway: fewer degrees of freedom, more visible strain.
- **Weights are fittable, and this is where we part from the paper.** It notes that the
  probabilities `pᵢ` do not affect the attractor's *shape*, only convergence rate and
  local density — true, and irrelevant here, because fracturize renders density. Our
  loss sees the picture, so weights change it and belong in θ.
- **The loss.** Sliced Wasserstein: draw `K ≈ 64` random unit directions, project both
  clouds onto each, sort, and take the mean squared difference between the sorted
  sequences. Both clouds must be the same size for the paired sort — subsample the
  attractor to the target's count, or use quantile interpolation. In 5D (with colour,
  dial 7) the directions are 5-vectors and nothing else changes.
- **Scale.** ~8k points per side per iteration is enough for the loss to be steady;
  save the 500k-point evaluation for looking at the result. This is the whole reason the
  gradient-free slice is affordable.
- **Normalisation is part of the answer, not scratch work.** The fit centres and scales
  the attractor every iteration to compare it with the target. Whatever transform that
  is at the end must be written into the emitted scene's `[camera]` — otherwise opening
  the result shows a correct fractal from an angle and distance that hide the likeness
  entirely, which looks exactly like a failed fit. Emit the framing that the fit was
  judged from, and make it the scene's default camera.
- **Output** is a `scenes/*.toml` with `[meta]`, the fitted `[[transform]]` blocks, and
  that camera. Nothing bespoke.

### Slice 2 — gradients

Analytic ∂L/∂θ through the truncated trajectory suffix (§1.C). CPU first via `trace.rs`,
then a WGSL backward kernel once the CPU version is known-correct — finite-difference the
CPU gradient in a test, and check the GPU against the CPU.

Parameterisation: translation, rotation vector, log per-axis scale, with contractivity
enforced by a saturating map on the singular values. Deliberately chosen so a fitted map
is an *ordinary* `[[transform]]` — `scale = [x,y,z]` and `rotvec` are both already in the
file format, so nothing here invents a scene key.

Optimiser: Adam plus the paper's annealing alternation. Their finding that pure gradient
descent is insufficient will hold for us too; the parameter set is small and every
parameter is global.

### Slice 3 — the dials

Dimension target, symmetry-group quotient fitting, view count, map budget ladder, the
fracturize (nonlinear) amount, colour-by-fixed-point. Each is a flag and a loss term or a
post-pass. This is the slice that decides whether the feature is art or a demo.

### Slice 4 — in the app

Reference overlay, "fit" with live progress and a cancel (the render-job model already
handles pause/cancel/estimates), greedy add-a-map showing each stage, the fracturize
slider as a live control. GUI wording is Hazel's call; propose, don't merge.

### Slice 5 — image-space refinement (conditional)

Only if slice 3's pictures say the likeness is short. PyTorch prototype first; native port
only if the prototype's output is visibly better on a real target.

### Slice 6 — the token pipeline

Batch: N references → N scenes → N prints at card-art resolution with a shared camera
treatment, palette family and grade. Rendering at print scale is solved (A2 at 600 ppi,
tiled), so this slice is mostly about consistency: the set has to look like a set.

## 3. Risks and open questions

- **Likeness may simply not arrive** for complex subjects. The paper's own results are
  loose on non-fractal targets. Mitigated by the fact that we want loose — but "loose"
  and "unrecognisable" are different, and only pictures can tell us which we got. Slice 1
  exists to find out early and cheaply.
- **Single-view 3D is underdetermined.** Feature, per §0.5, but it needs *some* prior or
  the z-structure will be uniformly ugly rather than interestingly wrong. Candidate
  priors: penalise z-extent, or fit 2D and inflate to 3D afterwards by adding rotation
  about the image-plane axes.
- **Stochastic loss.** The chaos game resamples every iteration, so the loss is noisy. The
  paper leans into this deliberately (it stops the fit overfitting to particular
  trajectories). We should follow them rather than fixing RNG streams, but it means
  convergence plots will look bad and the stopping criterion has to be a picture.
- **Divergence during optimisation.** Non-contractive intermediate states blow up the
  walker. Saturating parameterisation plus the existing re-seed handles most of it.
- **Which subject first?** Settled: the Powercell (§4), against the wider card set in §5.
  No synthetic control targets — the bar is *random cards*, and a method that handles a
  drawn-from-the-box spread of real ones has by definition worked. Nobody is printing
  cards designed to be awkward to fracturize, so adversarial robustness is not a
  requirement and should not be designed for.

## 4. First target: the Powercell (MRC EN-064A)

Chosen because it's the thing that actually gets fracturized. `tools/image_to_target.py`
is slice 0 as a prototype; `powercell-maps.png` is the three density maps side by side:

    for m in matte luminance edges; do
        python3 tools/image_to_target.py 'powercell-token[MRC-C-A].jpg' --map $m -o pc-$m.png
    done

### What the reference is

A brass canister with a cyan core, tilted about 20° off vertical, floating over a charge
pad, with lightning arcs, a mechanic and workshop clutter behind. So the first decision is
*what the subject is*: the token is the canister, and the workshop is not. How that
separation is made is §5.

### The finding that changes the plan

**How you turn pixels into density is an artistic dial, not preprocessing.** The same
reference gives three unrelated targets, and the sheet makes the difference obvious:

- **matte** — a solid body. Silhouette and mass; the hoops are only faintly there. A
  starved fit against this has nothing to spend maps on but the outline.
- **luminance** — the body plus interior banding, with the glow pulling density into the
  core. The middle option, and the one that keeps the light.
- **edges** — hoops, seams, cap and clamps all survive as line structure. Structurally the
  richest, and for *this* subject the right target: what makes a powercell recognisable is
  its banding, not its outline.

(An earlier pass called the matte an amorphous lump. That was true before the subject
weighting of §5 existed and is no longer — it's a clean canister now. The ranking survives
the correction, but it's worth knowing that improving the *weight* improved every map, so
these three should be re-judged whenever the front of the pipeline changes.)

So the density map is a first-class choice, not a hidden default — `--map` in the
prototype, and it should stay a flag in the Rust port.

### Why this subject is unusually favourable

The canister is a **solid of revolution with stacked hoops**, and both halves of that are
natively expressible:

- Rotational symmetry about the canister axis is a forced symmetry group (§0.4) — `C8`,
  `C12`.
- The hoop stack along the axis is a `repeat` progression, which the symmetry work already
  flagged as the next thing to reach for.

So the structure the subject actually has is the structure the file format already speaks,
and a fit does not have to discover it — it can be *imposed*, leaving the solver only the
profile to find. That is a much smaller problem than the general one.

It also resolves the single-view ill-posedness almost for free. §0.5 treats depth
hallucination as a dial, but a solid of revolution's 3D shape *is* recoverable from one
view once you fix the axis — so here forced symmetry is simultaneously an artifact dial
and the prior that makes the 3D fit well-posed. Rare, and worth exploiting first.

### Suggested first fit

Edge-density target, `C8` or `C12` about the tilt axis, a repeat progression along it,
four to six motifs, no nonlinear variations. Then the dials: colour by fixed point
(brass/cyan is the whole identity of this token and survives geometric destruction),
then the fracturize amount.

### The second subject

The lightning arcs are branching and thin — the L-system door (§1.F), not the point fit.
Fitting them separately and compositing behind the canister is likely how the finished
token art gets made: two engines, two subjects, one frame. Worth keeping in mind so the
CLI doesn't assume one scene per reference.

## 5. Subject and setting

The bar is *random cards*, so this has to work without per-card babysitting. Tested over
five (`cards-targets.png`): Clarent Reimagined, Inert Sword, Shadow's Twin, Sword Saint's
Vow, Powercell.

    python3 tools/image_to_target.py clarent-reimagined-prxy.png inert-sword-dtr.png \
        shadows-twin-alcsd.png sword-saints-vow-hvn.png 'powercell-token[MRC-C-A].jpg' \
        -o cards-targets.png

### Brightness keying is dead

The first prototype kept the largest bright connected blob. It works on the Powercell and
fails on almost everything else, because card art does not reliably light the subject
brighter than the setting: *Inert Sword* is a small sword against a bright starfield,
*Sword Saint's Vow* has cushions that outshine the blade, *Shadow's Twin* is a
low-contrast purple room. On Clarent it returns an unrecognisable fragment of hair —
`clarent-weights.png` is the three weighting modes on that card, and the failure is
unmistakable. Deleted rather than fixed.

### A soft weight, not a mask

The replacement is a weight in [0,1], and the change from mask to weight is the important
half. **A wrong mask deletes the subject; a wrong weight only lets some scenery in, and
scenery at low density reads as atmosphere rather than as error.** Nothing downstream
needs a binary decision — the fit consumes a density — so there is no reason to make one.

Three signals, multiplied, none of them about brightness:

- **Border distinctiveness.** Build a colour histogram from a ring around the art box and
  treat it as a background model; distance from it is a subject signal. Card art bleeds
  its setting to the edges and the subject rarely touches all four.
- **Detail density.** Local gradient energy at a coarse scale. Illustrators render the
  subject crisply and let the setting go soft — depth of field, haze, looser brushwork —
  and that difference survives when brightness doesn't. This is what rescues *Inert
  Sword*.
- **Centre prior.** Mild, isotropic. Composition does most of the work for us.

Then a **margin taper**: fade the outermost band of the art box to zero. The painted card
border is a very strong edge, and left in, the fit spends maps drawing a rectangle around
the subject — visible in the first pass, and no crop is precise enough to dodge it on
every card, so taper instead of cropping harder.

Result over the five: four read cleanly as their weapon, in their own colours. Clarent is
diffuse — and instructively so.

### Clarent is not a failure, it's a different card

Character art fills the frame and is rendered as crisply as its background, so no saliency
signal separates it — but *nothing needs separating*, because the subject is the whole
picture. The cards where the weight fails are the cards that don't want one. So `--weight
none` is a legitimate mode, not a fallback, and the full-frame edge target on Clarent is
the richest of the whole set.

Suggested default: `saliency`, with `none` for full-art and character cards. A one-bit
decision per card, and getting it wrong is visible immediately.

### On better segmenters

If the soft weight proves too crude, the escalation order is:

1. **A hand mask** (GIMP, a couple of minutes) for hero pieces that get printed. Exact,
   and cheap at the scale of a token set. Not viable for batch exploration.
2. **A saliency/matting net** — `rembg`/U2-Net or SAM. Pip-installable, CPU-runnable,
   ~200MB of model. This is the real automatic answer if we want one.
3. **Not a VLM.** A local Gemma outputs text, not pixel masks. The most it can contribute
   is naming the subject and giving a bounding box to seed SAM — a much bigger stack than
   the problem currently justifies.

Worth deferring all three until a fit has actually been run: the target only has to be
good enough for a *deliberately starved* solver, and a solver with six maps cannot
resolve the difference between a good mask and a decent one.

## 6. What this doesn't touch

Nothing in the renderer, the camera, the palette or the file format changes. The solver is
additive — `src/fit.rs` (target + loss + optimiser), a `fit` verb in `main.rs`, and later
one panel under `src/ui/` — reading `trace.rs` and `randomize.rs` rather than altering
them. If it's a dead end, deleting it leaves no trace in the scenes it produced: they're
just scenes.

## Starting the PoC

Read §0 for what "good" means here, then slice 1's spec block, then §4 and §5 for what the
target is and how it's cut. `tools/image_to_target.py` already produces the target and is
the reference implementation for the Rust port — run it on the five cards before writing
anything, so you know what the solver is aiming at.

The first picture to chase is not a likeness. It's the answer to: *does a six-map IFS of a
real card read as anything at all?* Twenty seeds on the Powercell, one contact sheet.
