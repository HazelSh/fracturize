# Volume renderer: partial transparency, then light

A third renderer beside `points` and `splat`. Points shows occlusion and loses
the inside; splat shows the inside and loses front from back. This one has a
single opacity control between the two, and later the lighting that occlusion
makes possible.

Status: slice 1 built (live window, one-pass stills, session-only opacity); see
"Slice 1 findings" below. Slices 2-7 are plan.

## The model

Treat the attractor as a glowing fog whose thickness is the point density. This
is the **emission–absorption** model from volume rendering (Max 1995, *Optical
Models for Direct Volume Rendering*). Every point gives off its colour, and every
point blocks a little of what is behind it. A point at depth `z` reaches the eye
attenuated by

    T(z) = exp(−σ · τ(z)),   τ(z) = absorbance deposited in front of z

- `σ = 0` is today's splat exactly: nothing absorbs, `T = 1` everywhere.
- `σ → ∞` approaches the points renderer: the first material a ray meets
  hides everything behind it.
- In between, thin material is see-through and thick cores block.

**Emission is the base case, not an extra.** Every point already emits; that is
what splat is. "Emissive points" in the sense of *some material glows and the
rest is lit by it* is scattering, a different problem, and it is slice 6.

### Opacity belongs to the density, not to the point

This is the trap the whole design turns on. Give each point a fixed alpha and
adding points makes the fractal more solid, so `--points` changes the picture:
the fp16-stall bug again, in a new form. (The points renderer already behaves
like this. At finite N it is stochastic transparency with opacity rising with N,
which is why thinning it breaks into the noise-haze of grit and doesn't read as
glass. That look is the points renderer's own character and stays where it is.)

So each point deposits absorbance `σ · K_ABS · H² / N · kernel` with exactly the
normalisation splat's exposure uses (`exposure_scale ∝ H² / N`). Opacity is then
a property of the scene, invariant to point count, resolution and supersampling,
just as exposure is. The tests in slice 2 enforce this.

## How T(z) is computed: moment-based OIT

Münstermann, Krumpen, Klein & Peters 2018, *Moment-Based Order-Independent
Transparency*. Chosen over the alternatives (see "Rejected" below) because both
of its passes are **additive**, so it fits everything this renderer already is:
lap accumulation, tiling, supersampling, DE, the fixed-point histogram.

- **Pass A (absorbance moments).** Splat every point into fp32 targets holding
  `b0 = Σa` and power moments `b1..b4 = Σ a·zᵏ` of warped depth `z ∈ [0,1]`.
  The warp is logarithmic, as the paper recommends. The depth range comes from
  the attractor's bounds (`trace::measure`'s centre and radius against the eye),
  or from the haze band / guard range under infinite zoom, where there are no
  bounds.
- **Pass B (emission).** Today's `vs_splat`/`fs_splat`, with each point's weight
  multiplied by `T(z)` reconstructed from the pixel's moments: four moments,
  closed-form, a few dozen ALU ops per fragment.
- **Tonemap** is unchanged in the σ = 0 limit (see the open question below).

**Why moments suit this renderer better than the paper's own test scenes:**
moment reconstruction rings when a pixel holds a few sharp layers. A pixel here
holds hundreds to thousands of soft deposits, so `τ(z)` is smooth, which is the
case the method reconstructs best.

Cost: pass A is a second raster of the point buffer, so roughly double splat's
point cost. At the T490's working range (650k–5M points) that is affordable;
drop the count to taste.

## Slices

### 1. The renderer exists, live, at σ = 0

- `RenderMode::Volume`. `R` cycles points → splat → volume, and the Render
  window's segmented radio gains a third segment.
- `VolumeRenderer` wrapping `SplatRenderer`'s pipelines, sharing compiled
  shaders via `SplatPipelines`.
- Pass A writes moments; pass B multiplies by T. With `σ = 0`, pass A is skipped
  and the output must be **byte-identical to splat**. That is the first test.
- The live device requests `FLOAT32_BLENDABLE` when offered (today only the
  offline device does: `gpu/context.rs`). Without it, refuse the mode with a
  reason rather than running moments in fp16, where they are useless. Check that
  Mesa on the T490 exposes it, and stop if it doesn't.

### 2. Opacity, and proof it is a scene property

- `opacity` in `[meta]`: scene data, undoable, saved by Ctrl+S, `-S`-settable,
  in `--info`. The slider is logarithmic, since the useful range spans decades.
  The CLI gets `--opacity`. Follow ADDING-A-SCENE-FEATURE.md.
- A **CPU reference**: a few thousand points on a small image, sorted per pixel,
  composited exactly front to back. The GPU result must match it within a stated
  tolerance across σ. This is the test that says the moments are right rather
  than merely plausible.
- Invariance tests: same scene and σ at two `--points` values, and at
  `--supersample` 1 and 2, give the same mean and the same picture within
  sampling noise.

### 3. Offline: laps, tiles, `.fgrade`

**Laps cannot each compute their own T.** Transmittance is nonlinear, and the
average of `exp(−x)` is not `exp(−average x)`, so per-lap T would be biased
toward see-through. Worse, the bias grows as laps shrink, which is `--points`
changing the picture again. So the render runs in two phases:

1. Run laps folding **moments** into a fixed-point histogram (a second
   `Accumulator`; power moments of warped depth are non-negative, so the
   unsigned format holds).
2. Resolve the moments once, then run laps folding **emission × T** as today.

This roughly doubles the cost of an accumulating render. Phase 1 can stop early,
because the moments converge long before the emission does; measure how early.
Checkpoints store both histograms. A `.fgrade` is post-T, like post-DE, so
re-grading still works and re-opacifying needs a re-render.

### 4. Opacity by material

- **The palette's alpha as an opacity curve**, a volume-rendering transfer
  function. The colormap is already `vec4`. A palette stop gains `opacity`,
  which multiplies σ for points that land on that part of the gradient: glass
  arms, a solid trunk.
- **Per-layer opacity** for subscenes, so censer's sponge can be solid while its
  smoke stays translucent.
- Mix mode has no index to look up, so it gets per-layer only. Note it, don't
  fight it.

### 5. Directional light, with self-shadowing

A light **at infinity** only. A positioned light has a size, and infinite zoom
has no sizes.

- Render pass A's moments **from the light**: an orthographic, light-space
  moment map (Peters & Klein 2015, *Moment Shadow Mapping*; the hair-rendering
  line of Kim & Neumann 2001 *Opacity Shadow Maps*, Lokovic & Veach 2000 *Deep
  Shadow Maps* and Yuksel & Keyser 2008 *Deep Opacity Maps* is the same idea for
  the same kind of material). Each point's emission becomes
  `emission + albedo · light · T_light(p)`.
- **No normals needed.** This is the answer to "what is the normal of a bit of
  grit": grit doesn't have one, and a dust mote in a sunbeam is lit by how much
  light reaches it, not by which way it faces. Add a Henyey–Greenstein phase
  term for forward-scatter rims if it earns its place.
- Fit the light map to the *view* frustum, not to the attractor, or a close-up
  gets one shadow texel.
- **The zoom wrap rotates by Q.** A world-fixed light would jump at every wrap.
  The light turns by `Q^u` with zoom depth, as helical scroll does with `A^u`
  (`renorm.rs`). Under a pure-scale zoom map that is the identity.

### 6. Emission that lights other material (experimental)

Material that glows *and lights its surroundings* is scattering. Under infinite
zoom it is the more natural light, because a `1/r²` falloff is scale-invariant
where a lamp position is not. Two candidates, both approximate:

- **Virtual point lights from the buffer itself** (Keller 1997, *Instant
  Radiosity*). The point buffer is already a sample of the attractor's measure,
  so each point gathers `1/(r² + ε²)` from a few emitters drawn at random from
  the buffer, and laps average away the noise. `ε` scales with eye distance, as
  the edge guard's ratio does, to stay scale-invariant. Unshadowed in the first
  cut; shadowing it needs pairwise transmittance and is expensive.
- **A view-frustum froxel grid** with emission injected and diffused through the
  absorbance (light propagation volumes, Kaplanyan 2009). Coarse, but it
  follows the camera, so it survives zoom.

Prototype offline first. Whether either is worth live cost is a question for
pictures.

### 7. Eye-Dome Lighting, last

Screen-space depth shading (Boucheny 2009, as in CloudCompare and Potree; Luft,
Colditz & Deussen 2006, *unsharp masking the depth buffer*). It works on the
points renderer, and on this one via the depth where `T` crosses 0.5, which the
moments give smoothed for free. Done last on purpose: it is lighting for
surfaces, and the volume renderer is the thing worth lighting.

## Slice 1 findings

Things the build showed that the design above did not say.

- **A point must not be absorbed by itself.** The paper's reconstruction counts
  a share (`overestimation`, 0.25) of the absorbance *at* the queried depth as in
  front of it. For a transparent surface that is a hedge; for emission the
  querying point is itself that mass. At opacity 30 a lone point (absorbance ~11)
  came out at 6% of its brightness, so thin material died first. Pass B now
  subtracts the point's own deposit from the moments (`transmittance_excluding`).
  Setting `overestimation = 0` instead looks crisp but under-absorbs a thick slab
  by 0.6 in T; the subtraction keeps 0.25's slab accuracy (worst 0.20).
- **Brightness falls ~1/opacity once pixels saturate.** Each point's emission is
  fixed while its absorbance scales with opacity, so an opaque pixel emits only
  its front sliver: in the continuum limit pass B's density tends to
  `(1 − e^{−τ}) / (opacity · K)`. At opacity 2 the whole-image mean luminance
  falls ~10% (nautilus 0.204 → 0.184), at 10 ~16%, with the lit material falling
  much more; raising exposure by the opacity restores it. Whether the tonemap
  should compensate (e.g. scale density by `τ / (1 − e^{−τ})` per pixel, which
  conserves energy and leaves occlusion in the *colour*) is the decision the
  "background compositing" question below should be made together with.
- **Four moments are soft in front.** On a thick uniform slab the reconstruction
  is smooth and monotone with correct ends but under-absorbs mid-slab by up to
  0.2 in T and leaks ~5-10% darkening just ahead of the slab's front face.
  No ringing or black holes were seen in renders.
- **The bias is not the constraint.** On f32-built single-layer pixels the
  reconstruction survives down to a bias of 3e-8 and fails at 0 (exactly
  degenerate layers). The shipped 3e-5 is the paper's order and a thousand times
  above where it breaks on the CPU; it is insurance for GPU rounding, not tuned.
- **Requesting `FLOAT32_BLENDABLE` on the live device silently moves live splat
  to fp32** (`accum_format` picks it whenever it can). The window keeps fp16
  emission via `SplatPipelines::with_precision(.., false)`.

## Open questions, decided by looking

- **Background compositing.** Splat composites `mix(bg, colour, coverage)`, with
  coverage taken from emission. With absorption the background should also be
  blocked by the total transmittance. The candidate is
  `alpha = 1 − (1 − coverage) · T_total`, which is exactly today's formula at
  σ = 0. Render it on a pale background before committing.
- **Haze.** It should scale emission only (it sits between the eye and the
  point), not absorbance (a property of the material). The edge guard scales
  both, because it removes material. Haze could later become a uniform
  background absorbance in the same model; don't do that in slice 1.
- **Moment resolution.** At accumulation resolution for simplicity. Lower would
  save memory and bandwidth if the T490 needs it.

## Rejected, so they aren't re-proposed

- **Sorting** (3D Gaussian Splatting, Kerbl et al. 2023): exact, but a per-frame
  radix sort of millions of points is out of reach on the T490, and lap
  accumulation can't sort across laps.
- **Weighted blended OIT** (McGuire & Bavoil 2013): one pass, but its depth
  weight is a heuristic and strong occlusion, the whole point here, is where it
  fails.
- **Stochastic transparency** (Enderton et al. 2010): exact in expectation and
  nearly free on the points renderer, but grainy, and the grain is the look this
  renderer exists to get away from. The CPU reference in slice 2 is the better
  ground truth.
- **World-space voxel grids** (incl. the parked `gpu/density/`): they lose all
  structure below a voxel, and a scale-invariant set has structure below every
  voxel.
- **Depth-slice froxels for transmittance**: exact up to slice count, but
  WebGPU can't choose a 3D texture layer per fragment, so it is eight render
  targets or atomics, and atomics on hot texels are the speck stall
  (654 → 46 FPS).
- **Normals from Lyapunov tangent frames** carried through the chaos game: under
  similarity maps every direction contracts alike and the frame carries no
  information, and that covers most scenes.
