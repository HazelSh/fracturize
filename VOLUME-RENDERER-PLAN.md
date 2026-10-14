# Volume renderer: partial transparency, then light

A third renderer beside `points` and `splat`. Points shows occlusion and loses
the inside; splat shows the inside and loses front from back. This one has a
single opacity control between the two, and later the lighting that occlusion
makes possible.

Status: slice 1 built (live window, one-pass stills, session-only opacity); see
"Slice 1 findings" below. Slices 2-7 are plan. **The transmittance estimator
described in "How T(z) is computed" was replaced in slice 1d** by per-pixel depth
slices (see "Slice 1d findings"), and slice 1e made it fast and removed its large-
point failure: read the findings sections, newest last, as the current design.
The moment-OIT sections are kept as the record of what was tried.

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
- **Brightness falls ~1/opacity once pixels saturate.** (Addressed in 1b below.) Each point's emission is
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

## Slice 1b findings

- **Brightness compensation is a constant gain, and one constant does not fit
  every scene.** `g = cσ/(1 − e^{−cσ})` on each point's emission (1 at σ = 0, so
  the splat path is untouched). The gain that holds mean excess luminance flat,
  found by bisection, follows that form closely on filament scenes (nautilus,
  censer; c ≈ 1.2) and fails on diffuse ones (fern_3d, ammonite, galaxy,
  glasshouse over-brighten 1.3-2.4x at c = 1.2). Shipped c = 0.4 minimises the RMS
  log deviation over seven scenes. Tables are on `volume::GAIN_C`.
- **The gain stops working past σ ≈ 10, for a different reason than the grit.**
  The ideal gain runs away (nautilus 29 at σ = 10, unbounded at 30). The
  four-moment reconstruction is soft in front, so the front sliver of a deep
  pixel is itself dimmed by `exp(−τ·share)`. Lowering the overestimation from
  0.25 to 0.1 moves the runaway from σ ≈ 5 to ≈ 10-20; 0 removes it, and removes
  occlusion with it (σ 3 and 30 render alike, near-splat). 0.1 is the compromise.
  A sharper reconstruction (six moments, or two layers) is the real fix, and is
  slice-3 territory.
- **The grit hypothesis holds, in a refined form.** It is not points
  "being stochastic" as such but *per-point optical depth ≫ 1*: a point's
  absorbance is `σ·H²/N` (8.7 at σ = 30, N = 4M, 1080p accumulation), so the
  front point of a pixel is lit and the rest are fully hidden — a coin toss per
  sample, with each lit point also boosted by the gain. Evidence: at 4x points the
  grit gets finer and the picture dims (the discrete answer drifting toward the
  continuum one: excess 0.0402 / 0.0377 / 0.0336 / 0.0332 at 1M / 4M / 16M / 64M
  points); at 4x exposure it is the identical pattern, brighter. Blurring the
  absorbance field (sigma 1 output pixel) both removes the grit (seed-to-seed
  RMSE 0.044 → 0.028 at σ = 30; splat's own 0.016) and fixes the convergence
  (0.0199 / 0.0237 / 0.0246 / 0.0248, within 1% from 16M). Blurs of 2-8 pixels
  bought a further 0.003-0.008 of noise and softened silhouettes, so 1 it is.
- **Infinite-zoom scenes work** with `DepthRange::for_zoom`: wellspiral and
  astral_lattice render sensibly at opacity 2 and 10 (no clipped bands, no
  black frames). astral_lattice at 10 shows background snow: the zoom band is
  full of sparse far dust that the gain lifts.

## Slice 1c findings

Prompted by Hazel using volume mode live: "the background bleeds through,
especially on a zoom-out, and points can flicker really badly".

- **The diagnosis was right.** Absorbance was normalised in screen space
  (`σ·H²/N`, as exposure is), so a ray's optical depth depended on camera
  distance. Median `b0` over lit pixels at opacity 2, at 0.5 / 1 / 2 / 4 times the
  authored distance: nautilus 0.53 / 2.86 / 12.3 / 46.9 and censer 1.37 / 5.1 / 20
  / 72.6 before, against 1.89 / 2.04 / 2.03 / 1.90 and 2.00 / 1.89 / 1.85 / 1.71
  after. Absorbance is now a world quantity: `σ·K·L²/N` of cross-section over the
  pixel's world area at the point's depth. Invariance to point count and
  supersample is kept (median `b0` 3.057 / 3.056 / 3.056 at 1M / 4M / 16M points;
  3.01 / 3.06 / 3.04 at supersample 1 / 2 / 3). One scene departs: jellyfish's
  median falls 1.78 → 0.73 over 0.5-4x, because its far dust becomes lit
  pixels as it shrinks (its p90 is flat, 4.28 → 3.93).
- **Flicker, measured live** (Xephyr, camera held by a view file, 100 frames at
  30 fps of the attractor, mean absolute difference of consecutive frames over
  its lit pixels; the gizmos are in frame and static, which dilutes all three
  alike): at 4x the authored distance on nautilus splat 0.045, volume before
  0.174 (3.9x splat), after 0.067 (1.5x). Censer at 4x: 0.067 / 0.132 (2.0x) /
  0.083 (1.2x). At the authored distance on nautilus: splat 0.102, volume before 0.177
  (1.7x), after 0.130 (1.3x); single runs, so read them to about ±0.02. What remains, 1.2-1.8x splat, is the ring turning over under a
  transmittance that amplifies it; the mean level at 4x was 117 before, 167 after,
  against splat's 169 — the dimming that read as bleed.
- **Background compositing by `T_total` helps on a pale ground and needs a
  filler.** `alpha = 1 − (1 − coverage)·T_total` alone makes a sponge on
  `[0.6, 0.6, 0.62]` solid instead of a ghost, but paints thick, dim-emitting
  pixels black (strands end-on, the centre of an infinite zoom). Letting the
  colour take the alpha instead fills everything solid and loses the lanes.
  0.3 of the pixel's own mean colour is the compromise.
- **The emission gain was removed.** With the fill it no longer helps.
- **Near the eye** a point's deposit climbs as 1/z² once its footprint hits the
  cap; it is now scaled by the area past the cap. Median `b0` stays 0.8-2.8 and
  p99 6-60 from 0.5x down to a tenth of the authored distance.
- **Not fixed.** At opacity 10 and beyond, dense sheets sag toward grey (the
  reconstruction's soft front), jellyfish-like translucent forms go flat milky,
  and the infinite-zoom centre can show a few black holes at opacity 3-10
  (reduced, not removed, by the fill).

## Slice 1d findings

Slice 1d built the **exact reference** first (`src/volume_ref.rs`): the same
points through the same vertex stage, kernel, world-space absorbance and emission
weights, written to a list of fragments, sorted per pixel by depth and
composited front to back. It reports the GPU path's error in `L` (what pass B
accumulates) and `T_total`, and *where* the transmittance error sits. Sanity: at
opacity 0.001 the GPU path agrees with it to 0.5%. Everything below is measured
against it, 480x270, 2M points, supersample 1.

**The 1c estimator was badly wrong, and we had been tuning it by eye.**
Nautilus at opacity 2: GPU `L` summed to 2.42x the exact value, 1.50 of relative
L1, per-fragment mean |dT| 0.21, `T_total` off by 0.10. Where it sat:

- **Front fragments**: rank 0 (the nearest in a pixel, exact T = 1) had bias -0.26.
  The four-moment reconstruction is soft in front, and the blur leaked
  neighbours' absorbance into them. Together the front 8 ranks were 5% of
  fragments and 20% of the error at the best moment settings.
- **The transition region** (exact optical depth in front 0.1-3): 70% of the
  error at the best settings; the reconstruction over-absorbs near 0.1-1 and
  under-absorbs at 1-3.
- **Up close** (near third of depths): 48-54% of the error.
- **The blur** contributed `T_total` error of 0.10-0.125 at opacity 2 and made
  every `L` metric worse (blur 0 / 1 / 2 px at overestimation 0.1: |dT| 0.156 /
  0.208 / 0.264).
- **The overestimation was fitted to the wrong thing.** 0.1 was picked in slice 1b
  because it made mean luminance flat; against the truth, 0.25-0.5 is closer in
  `L`, and the best a four-moment estimator did on this frame (global depth
  range, oe 0.25, blur 0) was L ratio 1.10, L1 0.33, |dT| 0.073.

**Experiments, scored on the CPU on the same fragments** (`FRACTURIZE_VOLUME_EMULATE`;
the emulator reproduces the GPU's numbers to the printed digit):

- a. *Blur off*: better on every number. Flicker was not the reason it existed
  (slice 1c fixed that at the root); with it off, lag-1 flicker at 1x is
  unchanged and at 4x zoom-out 0.064 -> 0.077 (splat 0.042), within the
  measurement's noise.
- b. *Exact front K*: helps modestly (nautilus op 2, global range, oe 0.25:
  |dT| 0.073 -> 0.065 / 0.059 / 0.048 / 0.031 at K = 2 / 4 / 8 / 16), because most
  of the error is not at the very front. Not built.
- c. *Per-pixel depth range*: the biggest single lever. Same four moments, the
  pixel's own [nearest, farthest] log span instead of the attractor's padded
  sphere: |dT| 0.073 -> 0.041. A pixel's material sits in a sliver of the
  attractor's depth.
- d. *More moments*: not pursued. With a per-pixel span, plain depth **slices**
  (absorbance accumulated into equal slices of the span, transmittance
  interpolated within a slice) beat every moment configuration, with no
  constants to tune: 8 slices |dT| 0.0145 (4 moments, per-pixel: 0.041), 16:
  0.0085, 32: 0.0051; L ratio 0.97-1.0 unbiased. Global-range slices, by
  contrast, are bad (32 slices: 0.096), which is what the per-pixel span buys.

**What shipped**: 32 slices over a per-pixel span (two atomic passes over the
points, two housekeeping passes over the pixels, an O(1) read per fragment),
no blur, no overestimation constant. Error against the reference, 1c ->
1d (L ratio, L L1, mean |dT|, `T_total` |dT|):

```
scene                       op | 1c: L-ratio L1  |dT| Ttot | 1d: L-ratio L1  |dT| Ttot
volume-crossing-sheets       1 |   1.34  0.34 0.182 0.027 |   1.01  0.02 0.034 0
volume-crossing-sheets       2 |   1.67  0.67 0.229 0.021 |   1.03  0.06 0.039 0
volume-crossing-sheets      10 |   3.15  2.16 0.195 0.005 |   1.09  0.27 0.025 0
volume-core-in-shell         1 |   1.59  0.62 0.214 0.038 |   0.96  0.07 0.034 0
volume-core-in-shell         2 |   1.56  0.60 0.158 0.048 |   0.94  0.09 0.027 0
volume-core-in-shell        10 |   1.33  0.40 0.048 0.047 |   0.97  0.05 0.007 0
volume-depth-range           1 |   3.11  2.39 0.161 0.053 |   0.97  0.04 0.004 0
volume-depth-range           2 |   2.53  1.97 0.083 0.031 |   0.98  0.08 0.004 0
volume-depth-range          10 |   0.15  0.89 0.011 0.006 |   1.08  0.19 0.002 0
volume-thin-before-bright    1 |   2.04  1.08 0.398 0.028 |   1.05  0.05 0.023 0
volume-thin-before-bright    2 |   2.77  1.84 0.387 0.011 |   1.08  0.08 0.019 0
volume-thin-before-bright   10 |   1.83  1.00 0.087 0.002 |   1.18  0.20 0.010 0
volume-zoom-tetra            1 |           n/a (zoom)     |   1.02  0.04 0.008 0
volume-zoom-tetra            2 |           n/a (zoom)     |   1.04  0.06 0.008 0
volume-zoom-tetra           10 |           n/a (zoom)     |   1.06  0.06 0.006 0
nautilus                     1 |   2.24  1.29 0.290 0.116 |   1.00  0.01 0.005 0
nautilus                     2 |   2.41  1.49 0.216 0.100 |   0.99  0.02 0.005 0
nautilus                    10 |   1.84  1.29 0.063 0.029 |   1.01  0.07 0.004 0
censer                       1 |   1.49  0.50 0.199 0.054 |   1.00  0.01 0.007 0
censer                       2 |   1.73  0.74 0.189 0.051 |   1.00  0.01 0.007 0
censer                      10 |   2.24  1.30 0.096 0.028 |   1.00  0.06 0.006 0
```

(The full per-scene reports are `renders/volume-1d/ref/*.report.txt`.) The one
scene where the slices are still visibly off is two dense *surfaces* crossing at
opacity 10 (L ratio 1.09, L1 0.27): all of a sheet's absorbance falls in a
sliver of one slice, which is then read as uniform. 8 and 16 slices do much
worse there (L1 1.23 and 0.63).

**The bugs.**

- *`density <= 0` returned the background whatever `T_total` was.* Fixed: a
  pixel with absorbance and no emission shows `bg * T_total`.
- *The threshold halo.* The mechanism, by the numbers: the blur spread each
  point's absorbance 2-3 pixels while its emission stayed one pixel, so a bright
  point's neighbours (and, at sparse pixels, other points in them) were
  attenuated by its leaked absorbance. That is the 26% mean dimming of the
  *front* fragments in the 1c numbers above, and it is a dimming toward the
  background colour around every bright point. The point-primitive / quad switch
  changes how concentrated the deposit is and what share of it the self-exclusion
  takes back (`own_share`'s variance 1/12 against `R^2/8`), so the leak jumps at
  the switch. **I could not reproduce a visible halo at the switch** in
  straddle renders of nautilus and galaxy (old or new build; aggregates smooth
  to ~1% across it), so this is the mechanism the numbers support, not one
  I watched happen. The blur is gone, which removes it either way.

**The tonemap** (task 4). With the exact transport the 0.3 fill turned out to
be a patch over a transport that did not know how much light a thick pixel
carries. Without it, and with exact transport, a thick pixel is dim in its own
colour -- correct, and too dim: emission per unit of absorbance is `weight /
absorbance`, which falls as 1/opacity (mean luminance 0.4-0.65 of splat's at
opacity 10 on seven scenes; `fern_3d` nearly black). A medium that absorbs also
glows, so the fragment now emits its splat weight **plus a source function times
its absorbance**: what a thick region glows at is then the ratio, constant, and
opacity 0 is still splat. `SOURCE` = 1 (exposure-1 splat density), chosen by eye
once against splat on seven scenes. The background survives twice over:
`bg * (1 - coverage) * T_total`.

**Cost** (the price of all this): live at 8M points on the GTX 1080, 14.7 FPS
against 36 for splat and 24 for 1c; flicker (lag-1 frame difference) at 4x
zoom-out on nautilus 2.1x splat (1c: 1.65x), at 1x 0.46x splat (1c: 0.83x),
censer at 4x 1.4x (1c: 1.3x). The remaining flicker at far zoom is sampling
noise of the *exact* transport of a pixel holding a few points, which no
estimator removes; only spatial smoothing does, and it is what the blur was.

## Slice 1e findings

Hazel's verdict on 1d was "massively improved", with three complaints: black
hard-edged squares around big points (zoomed hard into `cyber_nest`, or point size
scaled up elsewhere) with a frame rate that collapsed alongside; flicker that
depended heavily on the point count; and a volume mode noticeably slower than
splat or points. All three have causes, and none was where it looked.

**The black squares.** Reproduced offline (`cyber_nest`, `--distance 0.3`,
`-S meta.point_size=0.02`), then run through the exact reference: the reference
is smooth, the GPU path has the squares, so it is the estimator and not the
model. `FRACTURIZE_REF_PIXEL` on a dark pixel showed two faint fragments at depth
0.258 (absorbance 0.0014 and 0.0021) and the whole thick sheet, optical depth 18,
at 1.117-1.13. Span pass A0 takes every fragment's depth into the pixel's
[nearest, farthest], whatever its weight; a quad's fringe has a gaussian weight of
1e-3 of the peak but still counts. Those two near fragments stretched the span
from 1.2% to 4.4x the sheet's depth, the 32 slices (4.7% each) could no longer
resolve it, the sheet's front landed in one slice with all its own mass, was read
as uniform, and its front half-lit fragments came out at T ~ 0 instead of 1. The
quad's edge is where that near fragment stops existing, so the hard edge. My first
guess (flat absorbance footprint against a gaussian emission) was wrong: both are
gaussian, and a flat footprint was not the problem; a footprint that included
weightless fragments was.

**The fix is Hazel's clamp**: volume mode draws 1px point primitives only. It
removes the fringe fragments (one fragment a point), the 576x overdraw (the
measured collapse: 6M points, point size 0.02, `d` 0.3, 1d: **1372 ms** a frame
offline, 550 ms at point size 0.006; now **9 ms**, flat in point size), and the
quad-path code (the volume branch of `vs_splat`, `abs_quad`, `a0_quad`, `quad`).
It is a pixelated look, as agreed. The underlying mismatch is not fixed: a lone
near mote over a thick sheet widens that one pixel's span the same way, and now
darkens a pixel instead of a square (the darkest pixels in a close-up `cyber_nest`
frame are at 5% of the exact value). Fixing it means a span that ignores fragments
that carry nothing, which needs a threshold on absorbance and a guard for a pixel
with no qualifying fragment; the aggregate error is already 0.99 / 0.03 / 0.02
(L ratio, L1, |dT|) on that frame, so it was not worth doing once the clamp was in.
A big point's cross-section is spread over the larger of a pixel and its own
footprint (`point_deposit`), so close-up material is more transparent than the
same material far away: mass is not conserved, on purpose, because all of it in one
pixel is an opaque black speck with a source function's worth of emission on it.

**Performance.** `FRACTURIZE_VOLUME_PROFILE=<n>` on a `--render` prints each
pass's GPU-busy time (median of n frames), and for the points and splat renderers
their pass. nautilus, 8M points, 1280x720, supersample 1, GTX 1080:

```
                 1d      helper   sorted   own vertex   (final)
span             2.7     2.6      2.2      2.0
slice           12.8    11.4      2.8      2.2
prefix           0.5     0.5      0.5      0.2     (cyber_nest 4.6 -> 1.3)
emission        21.2    11.5      4.2      3.6
sort                              2.2      2.0
total           37.5    26.1     11.8     10.3
```

splat is 3.2 ms there, points 1.7. What the profile said:

- **The fragment shader's cost was loads, not arithmetic.** Replacing the emission
  pass's lookup with `T = 1` took it from 21 ms to 3 ms. Read-only bindings and a
  single load of the span changed it by 1 ms, so it was not the atomic loads.
- **A 1px point is shaded as a 2x2 quad and the three helper lanes execute the
  loads.** Skipping them (`is_helper`) took 37.5 to 26 ms.
- **The rest was locality.** The ring is in chaos order, so every fragment was a
  random line of a 118 MB buffer. Sorting the points on the CPU into screen tiles
  before profiling took 26 ms to 13, so it was worth a GPU sort. The tile size
  matters in an unexpected direction: 8x8 and 16x16 are 16.7 and 17.5 ms, 64x64,
  128x128 and 1280x64 bands 9.9, 8.4 and 8.7. Fine tiles put one pixel's points in
  one warp and their atomics serialise. A counting sort into ~60 buckets of 128px
  (`bucket.rs`, `bucket.wgsl`, histogram / scan / scatter, one global atomic per
  workgroup and bucket) costs 2 ms at 8M and changes no pixel (images are
  bit-identical with and without it, and opacity 0 is still splat, byte for byte).
- Halving the slices (16) bought 3 ms of the 26 and cost accuracy: not taken.
- Separate vertex stages for the span and slice passes (no colour work: they never
  used it) took another 1.5 ms; `fs_prefix` reading the slices as vec4s took
  cyber_nest's prefix from 4.6 to 1.3.
- Not tried: reusing the previous frame's span (saves the 2 ms span pass, and has
  to survive camera motion and zoom wraps, and the error is only checkable against
  the reference once the span is stale), a compute span pass folded into the sort.

Wall-clock per frame (submit to read, offline, ms), 1d -> now: nautilus 2M 11.3 ->
4.2, 8M 38.5 -> 12.2, 16M 73.9 -> 20.1; cyber_nest 13.2 -> 5.7, 39.4 -> 14.8,
72.4 -> 27.1. Live in a nested X server (whose presentation puts a floor of ~22 ms
on every mode, so these understate the difference), nautilus frame time at 2 / 8 /
16M points: points 23.1 / 24.8 / 24.5, splat 23.4 / 24.4 / 26.5, volume 26.5 /
31.3 / 39.1 ms (1d: 33.2 / 58.6 / 88.8).

**Reference error is unchanged by all of it** (480x270, 2M points, L ratio, L L1,
fragment |dT|): nautilus 0.995 / 0.018 / 0.005 at opacity 2 (1.010 / 0.071 / 0.004
at 10); censer 0.995 / 0.011 / 0.007 (1.004 / 0.056 / 0.007).

**Flicker, and why it depended on point count.** Measured live (nested X server,
camera held by a view file, 100 frames at 30 fps over 240x200 px of nautilus at its
authored distance, mean absolute difference of consecutive frames over lit pixels):

```
lag-1 difference      1M points            4M points            16M points
                    points splat volume  points splat volume  points splat volume
before                1.74  1.47  1.12     0.19  0.26  0.20     0.020 0.115 0.064
after                 0.14  0.12  0.10     0.06  0.10  0.065    0.021 0.128 0.070
```

It is the ring, in all three renderers. Every walker writes at least one point a
dispatch, and there are 16,384 of them, so the ring got at least 16,384 new points a
frame whatever its size, where the design is `capacity / 800`. That turned the
ring over every 61 frames at 1M points and every 244 at 4M, instead of 800 (it
only reaches 800 above 13M). A pixel of a sparse sample is carried by a few
points and each replacement moves it by 1/k, so flicker per frame follows the
share of the ring replaced per frame. My hypothesis (fewer points, each heavier,
re-rolled by the turnover) had the direction right and the cause wrong: weight per
point is not what changed, the turnover rate was. The fix is
`compute::interactive_workgroups`: the live window asks for fewer walkers at small
sizes (1M: 5 workgroups of 256, 12M: 47, never above the 64 a render uses, and
only the window asks, since the walker count is part of the image). Flicker is
now flat in point count (volume 0.099 / 0.065 / 0.070), and volume is at or below
splat's. It is not volume-specific, so it also quietens points and splat at small
counts, which they did not ask for; revert `app.rs` to `DEFAULT_WORKGROUPS` if that
is unwelcome. What remains in volume mode is the sampling noise of exact transport
in a pixel holding a few points, as before: far from the scene (the 4x zoom-out
view, 4M points) volume is 2.4x splat (0.112 against 0.047).

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
