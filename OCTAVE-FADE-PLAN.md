# Plan: fade the zoom band's outer edge instead of cutting it

*Claude Opus 5, 2026-08-03. Design only — no code written for this yet.*

---

## IMPLEMENTED, with two of its decisions reversed by measurement

*Claude Opus 5, 2026-08-03, second session. Read this before the plan below;
§0 said measuring first might resize everything, and it did.*

Built as designed — `ZoomSpec::octave_fade`, the two uniforms, the two-piece
inverse CDF in `octave_offset`, `[zoom] octave_fade`, `--zoom-fade`, and a CPU
mirror (`Renorm::octave_offset`) that the distribution is asserted against
rather than eyeballed.

**A better measurement than §6.** A wrap moves every shell one octave inward,
so rendering the same scene at `radius` and at `radius · s` gives exactly the
two frames either side of a wrap — no animation, no interpolation, no noise.
The offline renderer is bit-deterministic (two identical renders diff to 0.0),
so everything left is signal. §6's frame-to-frame RMSE is useless here by
comparison: at draft effort the noise floor is 0.10 RMSE and swamps the wrap.

**Reversal 1 — the taper cannot make the wrap's step smaller, only wider.**
Octave `k`'s share after a wrap is octave `k−1`'s share before it, so the
change summed over the band telescopes to exactly one octave's worth for *any*
monotone ramp, hard cut included. Measured: `octave-edge-test` loses 3.40% of
frame brightness at the wrap with a hard edge and 3.44% with a 2.3-octave fade.
§2 is not wrong about what it does, but "replaces the worst instance with a
mild one" is not what happens — the total is conserved and only its
distribution changes. That is still worth having: worst pixel goes 0.399 →
0.298 and the difference image goes from one solid slab of structure to faint
texture across the frame, which is exactly Hazel's "no hard cuts". It is a
change of character, not of magnitude, and the docs now say so.

**Reversal 2 — "on by default" is wrong.** Three of the four scenes measured
have nothing to fix: `wellspiral` wraps at 0.9999 and `pythagoras-zoomy` at
1.0000 with a hard edge, their outermost octave simply not being in the
picture. Fading them is pure cost — three octaves puts a 4–6% step into a wrap
that had none, and `pythagoras-zoomy` gains a 3.0% mid-loop pop against 0.13%.
§5's own guard says refuse that, so: **default 0, opt in per scene.** A
haze-derived default was tried first and also rejected — `pythagoras-zoomy` has
`haze = 0.0` *and* a perfect wrap, so haze does not predict which scenes need
it. Nothing cheap does; the two-render check does, and it is documented.

**§3 (widen the band) landed, but does less than claimed.** `DEFAULT_RADIUS`
3.0 → 4.8 and `DEFAULT_LEVELS` 14 → 15. It cannot help a low-haze scene at all,
because the rendered set is scale-invariant *by construction*: the outermost
octave subtends the same solid angle whatever `R` is. Measured on
`octave-edge-test` (haze 0.12), the wrap loses 3.41% at `radius = 3.0` and
3.31% at `radius = 4.8`. What headroom buys is margin for a scene framed
differently from how it was authored — real, but not the thing §3 argued for.
The four shipped zoom scenes set `radius`/`levels` explicitly and are
untouched (§7 Q2), which given the above is the right outcome anyway.

**Answering §7.** Q1: yes, on a scene built for it — `scenes/octave-edge-test.toml`
is that scene, and its outermost octave carries 3.4% of the frame as one
recognisable slab. Q2: leave them; they measure clean. Q3: still no, and now
also because the inner edge is 15 octaves down and nothing telescopes there.
Q4: `1/16` is right — the depth barely matters next to the width, since the
total is conserved either way.

---

Hazel's report: *"there's quite a bit of visible 'cutting' / jumps between
octaves of the infinite zoom, depending on the scene. scenes that zoom with the
bulk of the fractal 'visible' need to do something other than cut away large
chunks all at once. could decrease density over several octaves instead, fading
out the count of points in these regions?"*

Decision taken: **on by default, and widen the band as well.**

---

## 0. Measure again before building any of this

A separate bug was found and fixed in the same session (`1fcb3f4`): the offline
renderer froze the haze band at the scene's authored camera distance instead of
re-deriving it per frame, so a zoom loop's image drifted out of the haze as the
loop ran and snapped back at the wrap. Measured on `wellspiral`, that was +12%
core brightness across the loop undone by an 11% drop in one frame.

**Some of what read as "cutting" was almost certainly that.** It was a
whole-frame brightness ramp with a hard step at the wrap, which is exactly what
"jumps between octaves" describes. The taper below is worth doing on its own
merits, but the first step is a fresh render on the fixed build to see what is
actually left, and on which scenes.

The measurement protocol that found it works and should be reused (§6).

---

## 1. What the code does now

`octave_offset` in `shaders/points/chaos.wgsl:265` — the only copy;
`shaders/density/chaos.wgsl` has no renormalization.

```wgsl
fn octave_offset(rng: ptr<function, vec4<u32>>) -> f32 {
    let levels = params.zoom_levels;
    if levels <= 1.0 { return 0.0; }
    let u = rand_float(rng);
    let q = params.zoom_octave_q;
    if q > 0.9999 { return floor(u * levels); }          // flat deal
    let tail = pow(q, levels);                            // truncated geometric
    return min(floor(log(1.0 - u * (1.0 - tail)) / log(q)), levels - 1.0);
}
```

Read alongside `renormalize()` just below it:

```wgsl
var m = round(log(params.zoom_fixed.w / r) / params.zoom_log_scale);
m -= octave_offset(rng);
```

`m` larger means `f⁻ᵐ` applied more times, so **larger radius**. Subtracting the
offset moves a point *inward*. Therefore:

- **`k = 0` is the outermost shell**, radius ≈ `R`.
- **`k = levels−1` is the innermost**, radius ≈ `R·sᶫᵉᵛᵉˡˢ`.

Both edges are hard: outside `[R·sᶫᵉᵛᵉˡˢ, R]` there is simply nothing.

### Which edge is the one that cuts

The **outer** one. The camera is kept at `|eye − p| ∈ [band·s, band)`, and the
bulk of a fractal sits at large radius, so a scene whose `radius` is too small
lets the outer edge into the frustum. The inner edge is ~2⁻¹⁴ of `R` — far
below a pixel — and the wrap stops the camera reaching it anyway.

This is the same failure `edfc5a9` fixed once already by deriving `MIN_RADIUS`
from `haze::FAR_FRAC` rather than guessing it. That commit made the edge sit
*outside* the frustum for a correctly-sized band; it did nothing for what
happens when a band is under-sized, which is still a cliff.

### The constraint this runs into

`renorm.rs:142`, on `octave_falloff`:

> **Zero, and that is a correctness requirement rather than a preference.** A
> wrap moves the octave that fills the screen along by one, so if octave `k`
> and octave `k−1` hold different numbers of points, the density on screen
> jumps by exactly that ratio every period. Measured on `wellspiral`: the
> discontinuity across a wrap runs 1.9x an equal-sized camera move at falloff
> 0, and 3.2x at falloff 2.

This is real and the plan does not get to wave it away. The argument for
proceeding is narrow and should be stated honestly:

- A hard cut **is** a density change between neighbouring octaves — the largest
  one possible, a ratio of ∞. The taper does not introduce a new class of
  problem; it replaces the worst instance of it with a mild one.
- The taper lives only in the outermost few octaves. For a correctly-sized
  band those are out of frame, so nothing changes. It is a graceful-degradation
  measure for under-sized bands, not a general density profile.
- Widening the band (§3) pushes the tapered region further out of frame, which
  is why the two halves of this plan belong together.

---

## 2. The taper

Target distribution, unnormalised, over integer `k ∈ [0, levels)`:

```
p(k) = g^(F − k)     for k < F        rises from g^F at the edge to 1
p(k) = 1             for k ≥ F        unchanged
```

with `F` the fade width in periods and `g < 1` the per-period attenuation. Both
pieces are geometric, so the inverse CDF stays closed-form — one sample per
point, no rejection, which is what the existing code buys and should not lose.

```
M₁ = Σ_{j=1..F} gʲ = g(1 − g^F)/(1 − g)          mass of the tapered part
M₂ = levels − F                                   mass of the flat part

u ~ U(0,1)·(M₁ + M₂)

u < M₁:   v = u/M₁
          j = floor( log(1 − v(1 − g^F)) / log g )     geometric in j
          k = F − 1 − j
u ≥ M₁:   k = floor( F + (u − M₁) )
```

Mirror the existing inversion's shape so the two read alike, including its
harmless off-by-one convention.

### Composing with `octave_falloff`

Piece 1 becomes geometric with ratio `q/g` and the algebra still closes, but
**don't**. `octave_falloff` is documented as a stills-only knob precisely
because it breaks the wrap, and the taper is for scenes that are flown. Nothing
should want both. Implement the taper on the `q ≈ 1` path, leave the geometric
path exactly as it is, and say so in the comment.

### Parameters

- `ZoomSpec::octave_fade: f32` — **in octaves**, like `levels`, converted to
  this map's own periods in `Renorm::build` by the same `·ln2/log_scale` that
  `levels` already goes through. Default **3.0**.
- `g` derived, not authored: fix the density at the outermost shell to a
  constant fraction of full (suggest `1/16`) and set `g = (1/16)^(1/F)`. One
  knob is enough; two invites tuning a thing nobody can see.
- New uniforms `zoom_octave_fade` and `zoom_fade_g` in `PointComputeParams`
  (`src/gpu/buffers.rs:255`). **Watch the std140 padding** — the struct's
  scalar run is hand-packed with an explicit `_pad`, and two more `f32`s change
  where the following `vec4`s land.

---

## 3. Widening the band

```
MIN_RADIUS    = 1 + haze::FAR_FRAC = 2.4167     derived, don't touch
DEFAULT_RADIUS: 3.0  ->  4.8                     ≈ 2 × MIN_RADIUS
DEFAULT_LEVELS: 14.0 ->  15.0                    one more octave
```

`radius` is a multiple of the reference eye distance, so 3.0 was only 1.24×
the derived minimum — very little headroom for a scene whose framing isn't
exactly the authored one. 4.8 doubles it.

Each doubling of `R` costs an octave of *visible* depth, because the band is
`[R·2⁻ˡᵉᵛᵉˡˢ, R]`. 3.0 → 4.8 is 0.68 octaves, so `levels` goes up by one to
land the inner edge at least as deep as before. The cost is density: the same
point budget over more octaves. Hazel has accepted that ("everything gets a bit
sparser").

**The four shipped zoom scenes set both explicitly and will not pick up new
defaults.** `wellspiral`, `ammonite`, `pythagoras`, `bicameral` all carry
`radius = 3.0, levels = 14`. Editing them is a content change to authored work
and should be a separate, stated commit — or left to Hazel. Decide after §0.

---

## 4. Build order

1. **Re-measure on the fixed build** (§0, §6). Establish what cutting remains
   and on which scenes. This may resize everything below.
2. **Widen the defaults.** Two constants and their tests. Cheapest, safest,
   and independently useful; ship it on its own.
3. **Plumb the parameters.** `ZoomSpec::octave_fade`, `Renorm` fields, the two
   uniforms, `[zoom] octave_fade` in the scene format with round-trip coverage.
   No shader change yet — assert the values arrive.
4. **The shader taper.** `octave_offset` as §2. Verify the distribution before
   trusting the picture: a debug histogram of sampled `k` over a few million
   points should show the ramp and integrate to the mass ratios above.
5. **Measure again**, same protocol, on a scene chosen in step 1.
6. **Scene files**, if step 1 says they need it.

---

## 5. What could go wrong

- **The taper makes the wrap worse, visibly.** It is a density step by
  construction. Guard by measuring the wrap discontinuity the way `renorm.rs`
  already reports it (1.9× an equal camera move at falloff 0) and refusing to
  ship a default that raises that number for a correctly-sized band.
- **The fade eats the material the scene is about.** If the bulk sits in the
  outermost octaves, tapering them thins exactly what you came to look at. This
  is the argument for widening the band *first*: put the fade somewhere the
  scene isn't.
- **std140 padding.** Silent misalignment here shows up as garbage zoom
  parameters, not a compile error.

---

## 6. How to measure

This found the haze bug and should be the standard check.

```bash
fracturize --scene scenes/wellspiral.toml --render /tmp/t.avif \
  --width 640 --height 400 --fps 24 --seconds 2.8 --effort draft --splat
ffmpeg -v error -i /tmp/t.avif -vsync 0 /tmp/f_%03d.png

# brightness of the core across the loop — looking for a ramp or a step
for i in $(seq 1 67); do
  convert $(printf /tmp/f_%03d.png $i) -crop 160x160+240+120 +repage \
    -format "%[fx:mean]\n" info:
done

# the wrap should be no worse than an ordinary frame step
compare -metric RMSE /tmp/f_001.png /tmp/f_002.png null:   # baseline
compare -metric RMSE /tmp/f_067.png /tmp/f_001.png null:   # the wrap
```

A one-loop render at 640×400 draft takes about a minute, so this is cheap
enough to run on every change. Crop the region under test rather than the whole
frame: the haze drift was ~12% in the core and invisible in a whole-frame mean.

---

## 7. Open questions for Hazel

1. **After §0 — is there still cutting, and where?** Everything else is
   contingent on this.
2. **Edit the four shipped scenes' `radius`/`levels`, or leave them?** They
   won't benefit from wider defaults otherwise.
3. **Should the inner edge taper too?** Argued no above, but a scene that flies
   *into* the core rather than looping (the case `octave_falloff` exists for)
   would meet it.
4. **Is `1/16` at the outermost shell the right depth of fade,** or should it
   go to something nearer zero over more octaves?
