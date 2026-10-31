# Glass: a renderer with partial transparency

*Started 2026-10-31. Name agreed with Hazel 2026-11-01.*

This is a fresh renderer, built beside the old volume renderer and not on top
of it. **Don't read the old volume code, don't port from it, and don't
calibrate against it.** The same goes for points and splat: glass is not meant
to reproduce either of them. The goal is to show the IFS itself, the
mathematical object, well. If glass ends up good, the older renderers may be
deleted.

**How we judge it: it has to look good.** This is art. Hacks and cheap
approximations are welcome when the picture holds up. Numbers below are tools
for finding out *why* something looks wrong, not acceptance criteria. Hazel's
eye on a live build is the test.

## The idea

Points shows no inside: everything is opaque. Splat shows no depth: nothing
hides anything. Glass sits between them. The attractor is a **tinted fog**:
material that absorbs light and is drawn in its own colour, so near material
partly hides far material.

Each chaos-game point is a **sample** of that fog, not an object. It is drawn
as a flat disc of its colour, partly transparent, and the discs are composited
front to back with ordinary "over" blending. That's the whole lighting model:
pure absorption, where every disc is its colour at its own opacity.

## The one rule: how opaque is a dot

The material has an opacity **σ** (the dial). The scene has a reference
length **L** (the attractor's radius for now). A fog of σ over the whole
attractor has a total cross-section of `σ·L²`, shared evenly among the **N**
points:

    tau_point = sigma * L^2 / N          (a world area times an optical depth)

A dot spreads that over **the larger of its own disc and the pixel it lands
in**:

    tau_pixel = tau_point / max(disc_area, pixel_area)     (both in world units at the dot's depth)
    alpha     = 1 - exp(-tau_pixel)

This one line gives three properties:

- **Point count changes quality, not the look.** Double N and each dot
  carries half as much. Seen in the prototype: the same object at 30k, 300k
  and 3M points, only less speckled.
- **Near dots are big and faint per pixel.** A dot close to the camera
  covers many pixels at low opacity each, so it doesn't go solid.
- **Far, sub-pixel dots cover part of a pixel.** Their opacity shrinks with
  their area instead of being rounded up to a whole pixel. Opacity doesn't
  change with distance or resolution, and no supersampling is needed.

Disc edges get a 1px linear feather (coverage multiplies tau). Sub-pixel dots
go into the one pixel they land in. **Speckle at low point counts is
intended**: it's the same grit the other renderers have, so don't smooth it
away.

## Compositing: the front K exact, the rest counted

Per pixel we can't sort thousands of fragments. Truncating at K, then
compositing the background, is wrong in exactly the dense regions: there the
material behind the K-th fragment would have made the pixel opaque, and
truncation lets the background through. That greys out the dense cores, and
gets *worse* as points are added.

So we **count everything and sort only the front**:

1. **Front pass.** Each fragment adds its tau to the pixel's **total**, and
   tracks the pixel's **farthest depth**. If it is nearer than the pixel's
   current K-th entry, it inserts `(depth, tau)` into a **K-list** sorted by
   depth.
2. **Weight pass.** Each fragment works out the tau **in front of it**:
   - in the K-list: the sum of the entries nearer than it;
   - behind the K-list: the list's sum, plus the leftover tau times how far
     the fragment sits between the K-th depth and the farthest depth (the tail
     is assumed spread evenly in depth).

   It then adds `colour·w` and `w` to two sums, where
   `w = exp(-tau_front) · (1 - exp(-tau_own))`.
3. **Resolve.** The total opacity is exact from the total tau. The weights
   only decide how that opacity is shared among colours:

       pixel = (sum colour·w / sum w) · (1 - exp(-tau_total)) + background · exp(-tau_total)

**K never changes how opaque a pixel is, only how faithfully colour layers in
thick pixels.** **K is a setting, not a constant**: live, offline and in the
prototype. Hazel wants to be able to scale it up. Start live at 8; expect
offline stills to want 16–32. See the findings for why.

### On the GPU (not built yet; notes for whoever builds it)

- **K-list entries are one `u32` each**: depth in the high bits and tau,
  quantised to log scale, in the low byte, so `atomicMin` keeps the pair
  together and sorted. View-space depth is positive, so its float bits sort as
  an unsigned int; shifting off 8 mantissa bits is fine. The prototype stores
  exact floats, so try the quantisation there first.
- **Insertion**: read the K-th slot with a plain load first and skip if the
  fragment is behind it. Otherwise bubble it in with `atomicMin` down the
  list. Chaos points arrive in random depth order, so a pixel with n
  fragments sees only about `K·(1 + ln(n/K))` insertions. Most fragments in a
  hot pixel cost one read. This matters: the prototype's busiest pixel holds
  30k of 3M fragments, and AGENTS.md records what one hot texel does to
  blending throughput.
- **The tau total and the two weight sums use ordinary additive hardware
  blending** into float targets, with no atomics. The farthest depth is one
  `atomicMax`.
- **Memory**: K × 4 bytes a pixel for the list, plus a few float targets.
  K = 8 at 1440×860 is about 40 MB; K = 64 is about 320 MB. Clearing it each
  frame is a real cost on the T490, so measure it there.
- **Throttle GPU benchmarks.** The desktop GPU is Hazel's display as well.

## Decided

- **The name is glass.**
- **Opacity is a property of the material** (σ), shared out as 1/N per dot.
- **Density reads as opacity, not brightness. No glow, for now.** In pure
  absorption a pixel is never brighter than the colours of its dots: a thick
  region is more *solid*, not brighter. That's a real difference from splat,
  where density glows. Don't patch it with gain constants. **Revisit later**
  as an emission term separate from absorption (each dot emits a little
  besides what it absorbs). That would also make σ = 0 a splat-like picture,
  if anyone wants that. First get pure absorption looking good.
- **1px feathered disc edges** are fine.
- **Low-count speckle is intended.**
- **K is configurable**, defaulting to 8 live.

## Open

- **L under infinite zoom.** The attractor's radius means nothing in a
  scale-invariant set; something tied to the eye's distance from the fixed
  point probably does. Work it out when glass meets a zoom scene. The
  prototype skips zoom scenes because it doesn't renormalise.
- **A better tail.** The "spread evenly in depth" guess is what lets colour
  from deep layers leak forward at small K (see findings). A cheap
  improvement, if K alone isn't enough: a few coarse tau bins between the K-th
  depth and the farthest depth, giving the tail a shape. Try K first.
- **Haze.** Glass has a real depth cue now (occlusion). Haze might scale
  emission only, or nothing. Decide by eye.
- **Mute/solo, `--isolate`, transparent output.** Hidden points should
  presumably add no tau. With `--transparent`, alpha is just
  `1 - exp(-tau_total)`.
- **Where σ and K live.** Session state first (sliders in the Render window).
  Scene, view or prefs data once we know which is right.

## The prototype

`src/glass_proto.rs` is a test-only CPU version of everything above. It is the
reference for slice 1, and the place to try a change to the dot rule or the
tail before touching WGSL.

**What it does.** It loads a scene and frames it as `--render` would
(`offline::effective_camera`). It walks N points with the real CPU chaos
walkers (`trace::Walker`), coloured through the scene's colormap. It
rasterises each point by the dot rule into per-pixel fragment lists
`(depth, tau, colour)`. Then it composites the same fragments three ways:

- `exact`: each pixel's fragments sorted, composited front to back. The
  ground truth.
- `front_k`: the plan's three passes, written as a direct transcription of
  the GPU design.
- `truncated`: the K nearest, then the background. Kept to show what the tail
  is for.

It writes a side-by-side sheet (`renders/glass-proto/<scene>-<W>x<H>-n<N>-s<σ>.png`)
and every panel unscaled in the folder of the same name (`exact.png`,
`front-k8.png`, `trunc-k8.png`, …) for metrics and close looking. It prints the
fragment count, the busiest pixel and RMS error.

**Running it.** Always in debug, memory-capped (see `~/.claude/CLAUDE.md`: a
release test build OOM-killed the desktop on 2026-10-31). The build takes about
3 minutes; after that, run the test binary directly so nothing recompiles:

    systemd-run --user --scope -q -p MemoryMax=5G -p MemorySwapMax=0 nice -n 15 \
      cargo test -j 3 --bin fracturize --no-run
    B=$(ls -t target/debug/deps/fracturize-* | grep -v '\.d$' | head -1)
    GLASS_SCENE=reliquary GLASS_SIZE=384x216 GLASS_POINTS=1000000 GLASS_K=4,8,16 \
      $B glass_proto --ignored --nocapture

Settings: `GLASS_SCENE` (a name in `scenes/`; no zoom scenes, no subscenes),
`GLASS_SIZE`, `GLASS_POINTS`, `GLASS_SIGMA` (default 2), `GLASS_K` (a comma
list), `GLASS_SEED` (default 7; any other seed gets its own folder). 1M points
at 384×216 takes about 7 s.

**Perceptual metrics.** Scored with NVIDIA FLIP (0 = identical; built for
comparing renders) and SSIM, from a throwaway uv environment, since neither is
installed system-wide and the system Python has no venv:

    uv venv /tmp/m && uv pip install --python /tmp/m/bin/python scikit-image flip-evaluator

Score over **lit pixels only**, and look at the 99th percentile as well as the
mean. Whole-frame means are mostly empty background and hide everything.

### What it showed

384×216, 1M points, σ = 2, against the exact sort. FLIP over lit pixels, mean
and 99th percentile:

| | lacewing | apollonian_spheres | reliquary |
|---|---|---|---|
| front+tail K=1 | 0.031 / 0.48 | 0.019 / 0.21 | 0.055 / 0.45 |
| front+tail K=4 | 0.023 / 0.37 | 0.018 / 0.18 | 0.051 / 0.45 |
| front+tail K=8 | 0.016 / 0.24 | 0.016 / 0.17 | 0.045 / 0.43 |
| front+tail K=16 | 0.012 / 0.16 | 0.015 / 0.17 | 0.035 / 0.37 |
| front+tail K=32 | 0.011 / 0.15 | 0.013 / 0.15 | 0.021 / 0.22 |
| front+tail K=64 | 0.008 / 0.08 | 0.010 / 0.13 | 0.010 / 0.09 |
| truncated K=8 | 0.140 / 0.96 | 0.433 / 1.00 | 0.425 / 1.00 |
| truncated K=64 | 0.009 / 0.13 | 0.072 / 0.73 | 0.076 / 0.51 |
| **exact, reseeded** | 0.174 / 0.51 | 0.100 / 0.41 | 0.082 / 0.36 |

The last row is the yardstick: two exact renders that differ only in their
random seed. That much difference is plain grain, and the live ring makes it
anyway as it turns over.

How to read it:

- **Front + tail is always well under the reseeding noise**, even at K = 1.
  Truncation is far worse than it at any K up to 64.
- **But the K error is a bias, not grain**, and a steady bias is easier to see
  than grain that shifts around. It sits in the dense cores, where many layers
  overlap: colour from deep layers leaks forward because the tail is guessed
  as evenly spread in depth. On reliquary at 6× zoom, K = 4's cores are a
  shade more peach where exact is cyan and white. K = 16 is close; K = 64 I
  can't tell from exact.
- So K = 8 should be fine live, where everything moves, and stills will
  likely want 16–32. Hazel's eye decides; that's why K is a setting.

Also seen in the prototype:

- **σ works as one dial**, from smoky glass to solid. At 0.5, holes on the
  apollonian sphere's far side show through it; at 32 it is nearly an opaque
  surface with only the nearest holes showing.
- **Point count only changes quality**: lacewing at 30k, 300k and 3M is the
  same object.

## Building it: the intent for slice 1

The goal of slice 1 is a build Hazel can look at, not a finished feature.

- **A fourth renderer mode, `glass`**, in the `R` cycle and the Render
  window's renderer radio. Live window only, at first. No CLI, no offline, no
  render jobs yet.
- **New files, not edits to the old ones**: say `src/gpu/points/glass.rs` and
  `shaders/points/glass.wgsl`. Read the point buffer the chaos pass already
  writes; share the camera uniform and colour lookup the other renderers use.
  Don't touch splat or volume.
- **The three passes as above**, with K and σ as live session sliders in the
  Render window (K a power of two, 4–64). Resize the K-list buffer when K or
  the window changes.
- **Match the prototype**: the same dot rule, feather, tail and resolve, so
  `glass_proto` stays the reference. If the GPU picture disagrees with
  `exact.png` at the same scene and framing, the prototype says which pass is
  wrong.
- **Check it** on lacewing, apollonian_spheres and reliquary, plus a few
  random rolls. Watch the frame rate at 8M points on the 1080, throttled.
  Stage it through `tryout` (a release build in a worktree) for Hazel.

## Slices

1. **Live, in the window** (above).
2. **Afford K.** Frame rate and memory at K = 4, 8, 16, 32, 64 on the 1080,
   then the T490. Pick defaults, probably different live and offline.
3. **Offline stills** (`--glass`, the render dialog). Decide how it meets
   `--supersample` (the dot rule holds at any resolution, so perhaps it just
   works).
4. **Animations, sheets, zoom scenes**, and the open questions above.
5. **Revisit glow.**
6. If it's good: decide what happens to the old volume renderer, and maybe
   to points and splat.
