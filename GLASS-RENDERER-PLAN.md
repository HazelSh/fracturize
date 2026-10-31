# Glass: a renderer with partial transparency

*Started 2026-10-31. "Glass" is a working name, so rename it freely.*

This is a fresh renderer, built beside the old volume renderer and not on top
of it. **Don't read the old volume code, don't port from it, and don't
calibrate against it.** The same goes for points and splat: glass is not meant
to reproduce either of them. The goal is to show the IFS itself, the
mathematical object, well. If glass ends up good, the older renderers may be
deleted.

**How we judge it: it has to look good.** This is art. Hacks and cheap
approximations are welcome when the picture holds up. Numbers below are tools
for finding out *why* something looks wrong, not acceptance criteria.

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
  carries half as much. The prototype shows the same object at 30k, 300k and
  3M points, only less speckled.
- **Near dots are big and faint per pixel.** A dot close to the camera
  covers many pixels at low opacity each, so it doesn't go solid.
- **Far, sub-pixel dots cover part of a pixel.** Their opacity shrinks with
  their area instead of being rounded up to a whole pixel. Opacity doesn't
  change with distance or resolution, and no supersampling is needed.

Disc edges get a 1px linear feather (coverage multiplies tau). Sub-pixel dots
go into the one pixel they land in. If low point counts look too speckled,
the cheap fix is to spread each dot's tau bilinearly over 2×2 pixels; not done
yet.

## Compositing: the front K exact, the rest counted

Per pixel we can't sort thousands of fragments. Truncating at K, then
compositing the background, is wrong in exactly the dense regions: there the
material behind the K-th fragment would have made the pixel opaque, and
truncation lets the background through. The prototype shows it plainly:
lacewing's bright core and the apollonian sphere turn to grey haze when
truncated.

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

**K is a quality knob for colour layering only, never for opacity.** Start at
8. Below 4 should look worse; above 64, memory and frame rate will bite.

### On the GPU (not built yet; notes for whoever builds it)

- **K-list entries are one `u32` each**: depth in the high bits and tau,
  quantised to log scale, in the low byte, so `atomicMin` keeps the pair
  together and sorted. View-space depth is positive, so its float bits sort as
  an unsigned int; shifting off 8 mantissa bits is fine. The prototype hasn't
  tried the quantisation yet, so try it there first.
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
  K = 8 at 1440×860 is about 40 MB. Clearing it each frame is a real cost on
  the T490, so measure it there.
- **Throttle GPU benchmarks.** The desktop GPU is Hazel's display as well.

## Decided

- **Opacity is a property of the material** (σ), shared out as 1/N per dot.
- **Density reads as opacity, not brightness. No glow, for now.** In pure
  absorption a pixel is never brighter than the colours of its dots: a thick
  region is more *solid*, not brighter. That's a real difference from splat,
  where density glows. Don't patch it with gain constants. **Revisit later**
  as an emission term separate from absorption (each dot emits a little
  besides what it absorbs). That would also make σ = 0 a splat-like picture,
  if anyone wants that. First get pure absorption looking good.
- **1px feathered disc edges** are fine.
- **K starts at 8.**

## Open

- **L under infinite zoom.** The attractor's radius means nothing in a
  scale-invariant set; something tied to the eye's distance from the fixed
  point probably does. Work it out when glass meets a zoom scene. The
  prototype skipped zoom scenes because it doesn't renormalise.
- **Haze.** Glass has a real depth cue now (occlusion). Haze might scale
  emission only, or nothing. Decide by eye.
- **Mute/solo, `--isolate`, transparent output.** Hidden points should
  presumably add no tau. With `--transparent`, alpha is just
  `1 - exp(-tau_total)`.
- **Where σ lives.** Session state first (a slider in the Render window).
  Scene data once it is known to be worth saving.

## The prototype (done)

`src/glass_proto.rs` is a test-only CPU version: it runs the real CPU walkers,
rasterises dots by the rule above, and composites three ways (exact per-pixel
sort, front-K plus tail, and truncated) into one sheet under
`renders/glass-proto/`.

    systemd-run --user --scope -p MemoryMax=5G nice -n 15 \
      cargo test -j 3 --bin fracturize glass_proto -- --ignored --nocapture
    # env: GLASS_SCENE=lacewing GLASS_SIZE=192x108 GLASS_POINTS=300000 GLASS_SIGMA=2 GLASS_K=4,8

**Build it in debug, capped like that.** A `--release` test build of this
crate OOM-killed the desktop on 2026-10-31. Debug is plenty for the
prototype: 3M points at 192×108 takes 5 s.

What it showed (192×108, RMS error against the exact sort, 0–1 scale):

| scene, N | front+tail K=4 | K=8 | truncated K=8 |
|---|---|---|---|
| lacewing 300k | 0.014 | 0.010 | 0.091 |
| lacewing 3M | | 0.017 | 0.160 |
| apollonian 300k | 0.003 | 0.003 | 0.117 |
| reliquary 300k | 0.007 | 0.007 | 0.087 |

By eye, front + tail can't be told from exact at K = 4. Truncation gets worse
as points are added, which is the wrong direction. Across σ 0.5 → 32 the
dial runs from smoky glass (holes on the sphere's far side visible through
it) to a nearly solid surface.

## Slices

1. **Live, in the window.** A fourth mode in the `R` cycle; the three passes;
   K = 8; σ as a session slider. Judge it on lacewing, apollonian_spheres,
   reliquary and a few random rolls. Hazel looks at it through `tryout`.
2. **Afford K.** Frame rate and memory at K = 4, 8, 16, 32 on the 1080, then
   the T490. Choose a default, and maybe a live/offline split.
3. **Offline stills** (`--glass`, the render dialog). Decide how it meets
   `--supersample` (the dot rule holds at any resolution, so perhaps it just
   works).
4. **Animations, sheets, zoom scenes**, and the open questions above.
5. **Revisit glow.**
6. If it's good: decide what happens to the old volume renderer, and maybe
   to points and splat.
