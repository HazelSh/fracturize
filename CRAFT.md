# CRAFT — authoring IFS as artwork

`AGENTS.md` says what Fracturize *is*. This says what it's like to *make things with*.

It's for the agent who has been handed the keys and wants to make something good
without reading the engine first, and it's a place to write down what you find.
Everything
with a number attached was measured in this repo — if you measure something that
contradicts it, change it and say so. Sections marked **[lore]** are inherited from
the Apophysis / flam3 / DeviantArt era and have been re-tested here where possible;
where a piece of lore didn't survive contact with this engine, that's noted.

Reproductions of the measurements are in the sections that state them. Render
everything with `--effort small --splat --width ~340`: a contact sheet costs **under a
second** on the reference desktop, so the loop is look-change-look, not plan-commit-hope.

The loop, concretely: `--sweep <path>=<a:b>` varies one scene value across a
labelled sheet, `--set <path>=<value>` pins the rest, and `--mutations N` throws
dice when you don't yet know what to vary. None of the three edits the file.
Give `--sweep` twice and the sheet is a **grid**, not a strip: the first value
runs across columns, the second down rows, so two sweeps at `--sweep-steps 4`
are sixteen tiles, every pairing of the two. That is the cheap way to find out
whether two values interact.
When you can't tell which map made which part of the picture, `--map-grid`
renders one tile per map showing that map's points and nothing else, and
`--isolate <map>` does the same for one map in any render.

---

## 1. What the medium actually is

You are not drawing. You are writing down a **rule**, and the picture is that
rule's fixed point.

That one fact generates almost every difficulty and every pleasure in the form:

- **There is no local control.** You cannot fix "that bit in the corner". Every
  parameter is global — a map is applied at every scale, everywhere, forever. Move
  one transform 0.05 and the whole image reorganises.
- **You cannot post-process from inside.** Adding a transform to "shape the
  result" does not shape the result; it changes what the result *is*, because the
  new map's images are then acted on by every other map, including itself. I tried
  this directly (§3.3) and the attractor simply became a different attractor. This
  is exactly the hole that Apophysis's **final transform** fills, and this engine
  does not have one (§6).
- **So the craft is steering, not drawing.** The working method that the flame
  community converged on — and it converged hard, across twenty years — is
  *breed and select*: perturb, render a sheet, keep the good tile, repeat. That is
  what `--mutations N` is for, and it is the single most productive thing in the CLI.
- **The parameters are not the picture, and they are not even a good description
  of it.** You cannot read a TOML and know what it looks like. Render it. This is
  why `--info`'s `shape` block exists — and why `notes`, one line above it, is
  where you should actually start: it is every diagnostic the report found, or
  the word `none`.

The three levers, and they are genuinely separable here:

| Lever | Controlled by | Governs |
|---|---|---|
| **Form** | affine matrices + variation blends | what shapes exist |
| **Density** | `weight`, and contraction | which shapes you can *see* |
| **Colour** | `color_value` + gradient + `color_speed`/`falloff` | what the structure *means* |

Beginners spend all their time on form. The gap between a competent flame and a
good one is almost always density and colour.

---

## 2. Numbers that predict the picture before you render it

### 2.1 Similarity dimension — the most useful number in this file

Solve for `d`:  **Σᵢ sᵢᵈ = 1**, where `sᵢ` is each map's contraction
(`--info` prints it per transform, signed — negative means the map reflects; for
per-axis scale use the cube root of the product). `d` is the attractor's similarity dimension, and in a 3D renderer with
no lighting it predicts the *look* almost perfectly:

| `d` | Look | Why |
|---|---|---|
| **< 2.1** | filigree, lace, dendritic — see-through, detail at every scale | measure is spread thinly; you see *through* to deeper structure |
| **2.1 – 2.6** | a surface, a shell — texture on a skin | material concentrates on something 2D-ish |
| **> 2.7** | reads as a solid *in the points renderer* — but see below | copies overlap; with no lighting, detail is buried and the silhouette is all that's left |

**Caveat, and it's a big one: `d` describes the support, but `--splat` renders the
*density*.** So high `d` is not a death sentence — it only predicts a featureless
solid under the points renderer, where every point is full brightness and anything
past ~1 point/pixel saturates. Under splat's log-density tonemap, structure comes
back wherever density *varies* over that support. Measured on the d = 3.42 case
above: the points renderer gives a flat pastel silhouette with no internal detail
at all, and splat resolves the same scene into clearly layered, scalloped shells.

What predicts recoverability is therefore **density variance**, not dimension:

- **overlapping copies** (my d = 3.42 ring) concentrate measure very unevenly →
  splat recovers a lot. Try it before you throw the scene away.
- **uniform measure on the support** — `menger`, where all 20 sub-cubes carry equal
  weight — has no gradient to recover, and stays a brick under splat at every
  exposure from 0.15 to 2.5, from any angle. Note `menger` is the *lower*-dimension
  object (2.72 vs 3.42) and the *less* recoverable one.

So: read `d` as "how much support there is", then ask separately whether the
measure on it is lumpy. If it is, reach for splat before you reach for smaller
scales. More generally: **before concluding a scene is bad, check you haven't
concluded that about the renderer instead.**

**The calculation is trustworthy** — it reproduces the textbook values exactly for
the two scenes in this repo whose dimension is known independently:
`sierpinski` (4 maps at 0.5) comes out **2.00**, which is log4/log2, and `menger`
(20 maps at 1/3) comes out **2.72**, which is log20/log3 = 2.7268.

Measured, with everything else held fixed (a 5-fold ring plus a spire, only the
scale changed):

```
scale 0.40  ->  d = 1.91  ->  lacy, every level of detail visible
scale 0.52  ->  d = 2.67  ->  solid shell with surface texture
scale 0.60  ->  d = 3.42  ->  featureless blob
```

The repo's best zoom scene, `wellspiral`, sits at **d = 1.91**, and looks like lace.
`menger` at **2.72** is the failure mode: it is genuinely a near-space-filling
object, and with no lighting to model it, it renders as a grey rectangle with noise
on it. `rimefall` was designed to **1.97** on purpose.

**Use it as a design-time dial.** If a scene is mushy, you do not need a new idea,
you need smaller scales. This costs microseconds and saves render cycles:

```python
def dim(scales):                      # scales = per-map contraction
    lo, hi = 0.01, 6.0
    for _ in range(200):
        m = (lo + hi) / 2
        if sum(s**m for s in scales) > 1: lo = m
        else: hi = m
    return (lo + hi) / 2
```

Caveats, stated honestly: the formula is exact only for non-overlapping
similarities. Variations aren't similarities and overlap is common, so treat `d` as
a strong predictor, not a measurement. `--info`'s measured `occupancy` is the
after-the-fact check.

### 2.2 A map's weight is its share of the walk — and its share of the hue

`--info`'s `maps` block prints each map's share as a percentage, which is the
weight normalised. Two consequences
that cost me a render each:

- **Colour balance follows weight, not your swatch list.** I gave a scene five
  transform colours, of which one was gold — and the gold map was 1.5% of the walk.
  The image came out monochrome blue. If you want to *see* a hue, the map carrying
  it needs share.
- **Share is the exposure control that doesn't change structure.** Scrolling on a
  gizmo adjusts weight for exactly this reason. Emphasis without redesign.

### 2.3 Contraction drives the colour EMA — and there is a trap in it

With `color_falloff > 0`, each map's effective `color_speed` is
`1 − contraction^falloff`. `contraction()` is **clamped to [0.05, 0.95]**
(`src/scene.rs`), so:

> **An expanding map (contraction ≥ 1) clamps to 0.95 and gets `color_speed ≈ 0.03`.
> It barely advances the colour at all.**

This bites hard, because Mandelbox-style folds *require* expanding maps (§3.4). I
had two fold maps at 42% of the walk contributing essentially nothing to the colour
index, so the index stayed pinned wherever the contracting map put it, and every
palette I tried rendered as one flat hue. Six palettes, all monochrome, before I
read the code.

**Fix:** set an explicit `color_speed` on expanding maps. It always wins over
`color_falloff`.

### 2.4 The two from `--info` you should never override by eye

`--info`'s `shape` block gives a camera distance and a `point_size` ceiling, as
two flags you can paste:

```
to fill the frame   -S camera.distance=1.42
for crisp points    -S meta.point_size=0.0020
the scene sets      distance 3.400, point_size 0.0018
```

The `point_size` one is load-bearing: exceed it and the renderer leaves the crisp
1px path and every point becomes a multi-pixel billboard — strands turn to chunky
ribbons. Exceeding it also raises a `notes` line, so you no longer have to
compare the two numbers yourself. Size point_size against *your* structure,
never by copying another scene.

### 2.5 Background color: design it with intent, not by relying on defaults

The background color (`background = [r, g, b]` in `[meta]`, linear RGB floats) is
the canvas on which the emissive point cloud and atmospheric haze sit. The default
in Fracturize is a subtle dark-blue expanse (`[0.010, 0.008, 0.016]`), but it should be
chosen with intent for each scene:

- **Color with intent.** A tailored background tint matching or contrasting with your
  palette's shadow stops (e.g. deep celestial indigo, mineral slate, dark teal, or warm
  sepia-charcoal) grounds atmospheric haze and gives the entire piece tonal unity.
- **Background interacts directly with haze.** Atmospheric haze (`haze`) blends distant
  points toward the background color. Designing background color with your palette in
  mind turns haze into an organic depth cue rather than a generic backdrop.

---

## 3. Doses — measured

Almost every failure I had was a dose failure, not a concept failure. The concept
was right and the number was 5× too big. Flame lore is full of "add a touch of X";
here are the touches, measured.

### 3.1 Out-of-plane rotation: 5–12°, and 25° is already too much **[lore]**

The classic 2D variations act on xy and carry z through (§4.1), so the only thing
making them 3D is affine rotation tilting the plane between iterations. Sweeping
the tilt on a fixed scene:

```
 0°  crisp 2D form, coherent curl — but a flat sheet that vanishes edge-on
 5°  still coherent, now with visible relief and feathering   <- the sweet spot
12°  form readable but softening; 3D filaments appear
25°  form dissolving into fuzz
45°  mush
```

This is the central tension of 3D flame art in one sweep: **volume and 2D
coherence trade against each other.** A naive "make it 3D" by rotating everything
buys you a fuzzball. The good 3D flames are 2D forms with a *few degrees* of
relief, or they are built from genuinely-3D operators (§4.2) — not 2D forms
tumbled at random.

### 3.2 Barnsley's degenerate map: ~1–2% of the walk **[lore]**

In the Black Spleenwort fern, one of the four maps collapses the plane onto a line
— that's the stem — and it takes **1%** of the walk. Per-axis scale makes this
expressible here (`scale = [0.05, 0.62, 0.05]`), and the dose transfers exactly.
At 11% of the walk its streaks ate my picture; at 1.5% it reads as a spine holding
the plant up. One map doing almost nothing is what makes the thing stand.

### 3.3 A map added for depth must be a whisper

Adding a "stack" map (z-translation, mild rotation) to a coherent 2D flame, sweeping
only its weight:

```
control (no map)  crisp 2D form
weight 0.15  ( 3.6% of walk)  structure survives, gains a volumetric bloom  <- best
weight 0.50  (11%)            degrading
weight 1.50  (27%)            blob
```

And the structural lesson underneath: **this is not extrusion.** I wanted to sweep
the existing attractor through z. What actually happens is that the new map's
copies are re-processed by every other map, so past a small weight you don't get
your form plus depth, you get a different form. There is no operation in this
engine that transforms the finished attractor. (§6.1)

### 3.4 Folds only bite where they're defined

- **`boxfold` / `spherefold`** do nothing inside ±1 / r=1. `boxfold` is
  `2·clamp(p,−1,1) − p`, which for |p| < 1 is exactly `p`. If your affine keeps
  points near the origin, **the fold is an expensive identity** — my first fold
  scene had two of them and they did nothing at all. The Mandelbox recipe is
  *expand, then fold*: `shatterbox` runs its fold maps at **scale 1.05–1.12 with
  translations past 1.0**, and keeps the system bounded with a separate strong
  contraction (scale 0.35). Copy that shape.
- **`absfold`** always bites (it's `abs(p)`), but it needs something negative to
  reflect — translate into the negative octant first, as `stellate` does.
- **`absfold` dose:** at 0.55 it collapsed all three of my 120°-spaced ornament maps
  into the same octant and flattened a 3D well into a sheet (Y-spread 0.04). At
  **0.15** it seasons the ornament into mirror facets — crystalline rime rather than
  soft foliage — and the 3D structure survives. Sweep it yourself — note the `+`,
  which moves all three maps together, without which you vary one against two
  fixed ones and the tiles barely differ:

  ```sh
  A=transform.facet-1.variations.absfold
  B=transform.facet-2.variations.absfold
  C=transform.facet-3.variations.absfold
  fracturize -s scenes/rimefall.toml -r sweep.png \
    --sweep "$A+$B+$C=0.05:0.55" --sweep-steps 4 \
    --effort small --splat --exposure 1.3 --width 320 --height 320
  ```

### 3.5 Variations that spray **[lore]**

`spherical` is a 1/r² inversion: it throws material everywhere, and since points
render at full brightness in the points renderer, that's visible fuzz across the
whole frame. Lore said "keep spherical low or use bounded variations"; that holds
here. Bounded and well-behaved: `bubble`, `fisheye`, `sinusoidal`, `swirl`,
`julia`. `spherical` is never rolled by `--random` for this reason — fine by hand,
bad by dice.

`sinusoidal` with affine scale > 1.4 saturates onto the ±1 walls (box/room looks);
scale 1.1–1.2 with small rotations gives the classic gnarl swirls.

---

### 3.6 Symmetry plus one — the defect is the picture **[measured once, on one ring]**

Rotational symmetry is the cheapest way off the fuzz wall. Fuzz is what you get when
the maps' rotations don't close — compose enough incoherent rotations and you are
doing a random walk on SO(3), the measure comes out smooth, and no exposure setting
will find structure that isn't there. Constrain the rotations to a finite group and
the attractor is legible by construction. Every mandala in `scenes/` works for this
reason.

But it walks you straight at the *other* wall, and the mechanism is worth
understanding because it is not obvious:

> **A symmetry orbit distributes measure evenly by construction.** All `|G|` copies
> of a map are the same map with a rotation in front, so they carry the same
> contraction and — unless you go out of your way — the same weight. §2.1 says
> flat measure is precisely what splat cannot recover.

`menger` is the worked example. It is not merely a dense scene: it is a **group
orbit** — 20 sub-cubes of a 3×3×3 cube under the octahedral group — and
`scenes/menger.toml` contains no `weight` line anywhere, so all 20 maps are equal by
default. Equal by construction, in fact; that is what an orbit *is*.

Be precise about what fails there, because **it is not the authoring**. `menger` is
a faithful transcription of a classic object and it earns its place: it is the
repo's clearest demonstration that an IFS can make a near-solid body at all. What it
runs into is a renderer that currently has no cue to show that body with (§4.2).
The lesson for *your* symmetric scene is narrower and entirely actionable: an orbit
hands you flat measure, and flat measure is the one thing splat cannot rescue.

So the rule of thumb, which is the same shape as Barnsley's stem in §3.2 and
probably the same insight:

> **Symmetry gets you a form. One map outside the group gets you a picture.**

The defect is what puts density variance back on a support that the group made
uniform — a spine, a seed at the centre, a single off-axis map that the group then
propagates. Without it you have wallpaper: never fuzz, never interesting.

**The dose, measured — and it is the wrong question.** A clean `Cyc6` ring
(one motif at `s = 0.42`, 12° tilt) plus one off-axis map at the same scale,
weight swept so the defect's share runs 0, 0.5, 2, 5, 10, 20, 30 and 50% of the
walk:

```
 0%    the ring — wallpaper, exactly as predicted
 0.5%  the defect's whole copy is ALREADY THERE, as a ghost
 2-5%  a deliberate inclusion; the ring reads intact       <- the band
10%    the defect's colour starts filling the ring's cells
20%+   the symmetry is being eaten; by 50% it is gone
```

The surprise is the second row. **The support changes discontinuously at any
weight above zero** — the attractor contains every map with non-zero weight,
whatever its weight — so what a defect *adds* is decided by its geometry the
moment it exists, and its share is only a brightness dial on that fixed
structure. "How much defect?" is really "how bright should the thing the
defect already made be?", which is why the stem wants 1-2% and `rose_window`'s
core wants 18% without contradiction: they are different structures asking for
different exposure, not different doses of one medicine.

So author the defect's *shape* first (§9, `kintsugi`: where it sits decides
whether it reads as a seam, a shard or a lump), and dial its weight last, by
eye. Reproduce with any ring and `--sweep transform.<defect>.weight=0,0.005,0.02,0.05,0.11,0.25,0.43,1.0`.
One ring at one scale is one measurement; the 2-5% band may move with `s`.

---

## 4. What being native-3D actually changes

### 4.1 Only 9 of the 20 variations are three-dimensional

Derived by reading `shaders/points/chaos.wgsl` line by line — the module comment
above `apply_variations` is not quite right (it lists `swirl` as fully 3D; `swirl`
writes `p.z` through unchanged). This is the most important table in the file for
anyone porting a 2D recipe:

| Genuinely 3D — write all three components | 2D — write xy, carry z through unchanged |
|---|---|
| `linear`, `sinusoidal`, `spherical`, `fisheye`, `bubble`, `absfold`, `boxfold`, `spherefold`, `bulb` | `swirl`, `horseshoe`, `polar`, `disc`, `spiral`, `hyperbolic`, `diamond`, `julia`, `bent`, `cylinder`, `tangent` |

Two footnotes worth having:

- `absfold` / `boxfold` are **per-component**, so they are 3D but axis-aligned:
  they fold on the coordinate planes, which is exactly why they produce flat facets
  and crystal walls rather than curved surfaces.
- Several of the 2D column (`swirl`, `polar`, `disc`, `spiral`, `hyperbolic`,
  `diamond`, `julia`) compute their radius from the **full 3D** `dot(p,p)` even
  though they only write xy. So z silently modulates the xy result: move a sheet
  along z and its pattern changes, though it stays a sheet.

A 2D variation in a 3D engine is a *sheet-maker*. That is not a bug and it is
often what you want — `glasshouse` is receding planes and is one of the best-looking
scenes in the repo — but you must place the sheets deliberately, not hope they
become volume.

**Three cures for flatness, in order of how well they work:**
1. **A few degrees of tilt** (§3.1). Cheapest, keeps the form.
2. **Build on 3D operators.** `bulb` (misty nested pearl shells with 8-fold mandala
   inclusions), `spherefold`+`boxfold` (glassy plane-and-shard architecture),
   `absfold` (crystal facets). These have no 2D equivalent and are where this
   engine's own voice lives.
3. **Place sheets as sheets.** Accept flat elements and compose them in depth.

### 4.2 There are no lights, so the palette is the shading

The renderer is pure emission — no shading, no occlusion, no normals. Depth comes
from exactly three places: **haze** (aerial perspective, the only real depth cue),
**parallax** when it moves, and **silhouette**.

This has a hard aesthetic consequence, visible right across `scenes/`:

> **Legible 3D here means filaments, shells and planes. Filled volumes read as
> flat texture.**

`pearl`, `glasshouse` and `shatterbox` read as three-dimensional objects. `menger`
— a solid cube — reads as a grey rectangle with noise on it. There is no lighting
to tell you it's a cube, so it isn't one. (`menger` is the genuinely hopeless case:
uniform measure, nothing for splat to recover. Most over-dense scenes are *not*
like this — check with splat first, see §2.1.)

Worth being exact about *whose* failure that is, because "hopeless" overstates it.
`menger` is a faithful transcription of the classic sponge and the scene is not the
problem; three cues are missing at once. There is no density gradient for splat to
resolve, no lighting to model the solid, and — as it stands — no colour structure
either, so the thing the sponge is actually *about*, holes within holes within
holes, never arrives. Keep the scene: it is the repo's clearest evidence that an IFS
can build a near-solid body, which is a real and non-obvious fact about the medium.

**The one of those three that might be cheap — tested 2026-10-12, and it works;
see the discovery log and `scenes/ledger.toml`.** `menger`
gives each of its 20 sub-cubes a distinct `color`, and under `color_mode = "mix"`
(`[meta]`, see `src/scene.rs`) the walker carries an RGB EMA over the transforms it
actually took rather than a scalar index. On this scene that EMA is a record of
*which sub-cube path* a point descended, which is exactly the recursive structure
the geometry has and the render doesn't show. It may do nothing. It is one render to
find out, and if it works it is a colour answer to a problem §2.1 frames as a
density one. (I did not check whether `--set` can carry a string value; if it can't,
copy the scene and edit `[meta]` — do not edit `scenes/menger.toml` in place.)

This is also why the library palettes are
required to put their luminance *somewhere* and to rise and fall exactly once: a
gradient at one brightness renders flat however pretty its hues are, because the
gradient is doing the job a light would do in any other renderer.

### 4.3 Under infinite zoom, distance stops being a camera control

`[zoom]` renders a set that is *exactly* scale-invariant. So when you fly in, the
camera wraps and you are looking at a pixel-identical picture. I swept pitch and
distance across six framings of a zoom scene and got six statistically identical
images — because that is what scale invariance means.

> **A still of a scale-invariant set is a texture.** Every framing shows the same
> thing. Composition, in the ordinary sense of arranging a subject in a frame, is
> not available; there is no subject, because there is no privileged scale.

That is not a defect, it's the point — but it changes the deliverable. **The
artwork is the loop, not the frame.** If you want a composed still, turn zoom off
and render the bounded attractor; that's a different picture of the same rule.

### 4.4 Occlusion is the depth cue you don't have, and normals are why

`todo.txt` asks what the normal of "a random bit of grit" even is. It's a real
question: an IFS attractor is a measure, not a surface, so at d ≈ 1.9 there is no
surface to have a normal. Where a normal *does* nearly exist is the shell regime
(d ≈ 2.1–2.7), which is exactly where `pearl` and `menger` live. If lighting is
ever attempted, that's the band where it would mean something — and the systems
that have solved it (§7) all render *shells and solids*, not dust.

---

## 5. Apophysis-era lore: what transferred

Re-tested here unless noted.

- **"Everything is a spiral."** Still true. A mild rotation plus contraction is the
  workhorse; `julia` (half-angle with random branch) doubles the arms.
- **Weights are the exposure of parts.** Confirmed and central (§2.2).
- **Raise `color_speed` for per-branch identity, lower it to blend.** Confirmed —
  colours wash to pastel when transforms mix heavily.
- **Gradient design: put the luminance somewhere; make it rise and fall once.**
  This one is enforced by tests in `src/palette/library.rs` and it is correct. A
  monotone dark→bright ramp puts a hard seam at index 0 in a cyclic map. Invisible
  until you've rendered a hundred bad rolls, as the source comment says.
- **Breed, don't design.** `--mutations N` writes each variant's TOML out and prints
  its operator list per tile. This is the Electric Sheep loop in a single command
  and it is the best thing in the CLI.
- **Keep the accidents.** Standard practice in the flame community, and it earned
  its keep here: an `absfold` dose that was flatly wrong for the piece I was
  building produced sparse floral rosettes on a curving bough — cherry blossom —
  which is nothing I would have thought to aim at. Saved as `scenes/blossom.toml`.

**Lore that did not transfer:** anything that assumes a final transform, a post
transform per xform, xaos, or the blur family — none of which exist here (§6).
A large fraction of published Apophysis recipes use at least one, so expect to
translate rather than port.

---

## 6. The four things most worth adding

Ranked by expressive power per line of code. These are not complaints about the
engine; they're where the ceiling currently is.

### 6.1 A final transform — the big one

In flam3/Apophysis, a *final transform* `F` is applied to every point on its way to
the film, but is **not** fed back into the iteration: you plot `F(xₖ)` while
iterating `xₖ₊₁ = f(xₖ)`. So the rendered set is `F(attractor)`.

That is the missing operation. It is the only way to shape a finished attractor,
and its absence is why §3.3 failed, and why it costs a *scene redesign* rather than
a knob-turn to say "the same thing, but bent through a lens".

Note the asymmetry that makes it worth having: an **affine** final transform is
just a camera move, so it buys nothing. A **nonlinear** one — `spherical`, `julia`,
`bubble` — is the entire "put the whole flame inside an eyeball" idiom of
mid-2000s flame art, and there is no way to express it here at all.

Cost: one extra transform's worth of state, applied at plot time in `chaos.wgsl`
between the walk and the buffer write. It does not touch the iteration.

**It also unlocks rotational symmetry for nonlinear maps.** n-fold symmetry needs
the map set `{Rᵏ ∘ f}` — the rotation applied *after* `f`. Since a transform here is
affine-then-variations, `Rᵏ ∘ f` is only expressible when `f` is pure affine (fold
the rotation into the matrix). I built a 5-fold symmetric IFS this way and it works
— but only because every map was affine. With variations, it cannot be written down.

### 6.2 xaos — a transition matrix

*Built, 2026-10-14: see AGENTS.md "Xaos" and `scenes/parasol.toml`. The entry
below is the case for it as first written.*

flam3's `xaos` replaces the N weights with an N×N matrix: the probability of going
to map *j* given that you just came from map *i*. It's the difference between "how
much of this map" and "this map only ever follows that one", and it is the main
structural lever flame artists reach for after weights. Cheap: the chaos game
already does a binary search over a cumulative weight array; this makes it one row
per current-transform.

### 6.3 The blur family

`pre_blur` / `gaussian_blur` replace the point with a random point in a small
disc, *before* the affine. It's how flame art makes soft volume rather than dust —
in a 3D engine with no lighting, a variation that turns a filament into a tube of
fog is a genuinely new material, and it's about four lines of WGSL.

### 6.4 A symmetry generator

"Make this 5-fold about Y" is one click in Apophysis and a Python script here (I
wrote one; it's in the experiment log). Given §6.1, the affine case is easy and
worth having on its own.

---

## 7. Where to steal from, other than Apophysis

This system is native-3D end to end, which puts it in a lineage Apophysis is
*not* in. Apophysis is the wrong sole ancestor. The 3D-fractal world solved
problems this engine is now hitting:

| Source | What it has that's worth taking |
|---|---|
| **Chaoscope** (3D strange-attractor renderer) | Named *rendering modes* — Gas, Solid, Light, Plasma — as a first-class artistic choice rather than a debug flag. Its Solid mode does lit surfaces from a point cloud, which is precisely the open question in `todo.txt`. The idea that one attractor has several legitimate *materials* is the transferable one; `points`/`splat` is already two thirds of the way there. |
| **JWildfire** | The most complete 3D flame editor there is: huge variation library, depth of field, and "solid rendering" with shading. Also the best argument for §6.3 — its soft/blur variations are what make its 3D work read as volume. |
| **XenoDream** | Treats 3D IFS as *sculpting*, and puts real geometry (not points) at the leaves of the recursion. A different answer to "what is the normal": don't derive it, author it. |
| **Structure Synth / EisenScript** (and Hvidtfeldt's writing on KIFS) | A recursive **rule language** with weights and per-rule transforms, which is a far better human/LLM authoring surface for an IFS than a flat list of matrices. `tools/lsystem_to_ifs.py` is already a step toward this; EisenScript is what the destination looks like. |
| **Mandelbulb3D / Mandelbulber** | *Hybrid formula stacks*: sequence several operators with per-iteration control. Generalises this engine's variation blend from "weighted sum at one site" to "a short programmable pipeline per transform". |
| **Incendia** | 3D IFS with raytraced output and explicit primitives — the other end of the quality/interactivity tradeoff from a chaos game. |
| **Ultra Fractal** | **Layers** with blend modes. Nothing here composites two renders of the same scene, and layering is how a lot of published fractal art actually got its depth. |

The through-line: everything above except Apophysis assumes fractals have
*surfaces and materials*. Fracturize currently assumes they have *measure and
colour*. Both are defensible, but the second is a smaller room than it needs to be,
and `todo.txt`'s lighting question is really a question about which room to live in.

---

## 8. The CLI, judged as an artist's tool

**What is genuinely good** — say so, because it's unusual:

- **`--info` is an excellent agent interface.** `notes` gives you one line to
  branch on before spending a render, `shape` catches the two errors most common
  in hand-authored scenes, and where it has computed a number you will act on it
  emits the flag rather than the number. More tools should do this.

  It used to emit the 24-bit ANSI palette swatch even into a pipe, deliberately,
  on the theory that an agent could then *see* the gradient rather than imagine
  it from floats. That was wrong on the facts — an agent reading Bash output gets
  the escape bytes as literal text — and it was a third of the report's bytes and
  about half its tokens. The hex ramp was always the channel that worked; it now
  carries twelve stops, and the swatch lives behind `--color` for people at a
  terminal.
- **Contact sheets are sub-second.** `--orbit-grid` / `--move-grid` fill the point
  cloud once and re-render per tile, so nine views cost barely more than one. The
  iteration loop is genuinely tight.
- **`--palette` and `--zoom` restyle a scene without editing it**, and print what
  they settled on, ready to paste. Exactly right.
- **`--mutations` writes each variant's TOML out.** Look at sheet, pick tile, load
  its file. That closes the loop instead of just showing you something nice.
- **Reproducible seeds, always printed.** A good roll is never lost.

**What's still missing, in order of what it would buy:**

1. ~~No numeric feedback on the rendered image~~ — `--stats <png>` now prints
   luminance, clipping, how much of the frame is empty *ground* (measured
   against the scene's own background, read from the render's embedded scene),
   and the lit pixels' hues grouped into families. It flags one narrow family,
   which is what §2.3's pinned index looks like, and leaves a base-plus-accent
   picture alone.
2. ~~`--info` doesn't compute the similarity dimension~~ — it does now, orbit
   counted, and prints each map's resolved colour `speed`.
3. **No crossover.** `--mutations` is half the genetic loop; there's no way to breed
   two scenes together, which is what Electric Sheep actually did.

None of these are architectural. The engine is in good shape; the authoring surface
is one notch behind it.

---

## 9. Discovery log

Append findings here. Date them, say what you measured, keep the failures — a
recorded dead end is worth as much as a recipe.

- **2026-09-23, Opus 5.5. A defect that lies *in* the surface, and §3.6's dose.**
  `scenes/kintsugi.toml`: a `Cyc8` bowl (rim, body, foot) mended in gold by one
  map outside the group. The dose result is in §3.6. The authoring result is
  about *where* a defect goes, because the first three placements failed:

  - a degenerate **needle** (`scale = [0.04, 0.55, 0.04]`, the §3.2 stem) gives
    gold sticks through the glaze. Bending it with `julia` / `swirl` /
    `sinusoidal` spread the flecks around but they stayed sticks, and `julia`
    sprayed some off the form;
  - a freely placed **sheet** (thin on one axis) stood off the wall as shards;
  - what worked: **copy a motif, move it half a group step, squash it
    tangentially.** `crack` is `rim` turned 11.25° (half of Cyc8's 45°), so it
    sits on the joint between two rim tiles, with its tangential axis at 0.02.
    Being a thin copy of the whole bowl, it lies in the bowl's own wall, and
    every sub-bowl inherits the same seam at every scale. A further turn about
    the wall normal (40°) makes it run diagonally, the way a break does.
    Face-on it reads as a gold panel, edge-on as a hairline.

  General form: **a defect that conforms is a motif moved off the group, not a
  new shape.** Its images then land on the surface instead of piercing it.
  The arithmetic took scipy the first time; it is one command now, and prints
  a block to paste:

  ```sh
  fracturize -s scenes/kintsugi.toml --derive rim --turn y:11.25 --spin z:40 \
    -S transform.rim.scale.x=0.02
  ```

  Also: in `mix` mode a defect's colour arrives at `color_speed` strength, so at
  0.55 the gold came out cream. A per-map `color_speed = 0.9` on the defect
  fixes it without touching the rest; the global 0.7 is what turns three
  similar jades into light and shade.

- **2026-08-09, Opus 5. `repeat` — and the reason it is filed under symmetry
  while not being one.** A repeat is `count` copies stepped by a similarity
  (turn about an axis, slide along a vector, shrink), which covers a row, a
  helix, a logarithmic spiral and a cone from four numbers. `scenes/fiddlehead`
  is twenty copies at the golden angle: it renders as a conifer, and no group
  in §6.4 can make one.

  **The distinction that matters when authoring: a group flattens the measure,
  a repeat does not.** §3.6's whole argument is that an orbit distributes
  measure evenly *by construction* — every element of a rotation group is
  orthogonal, so every copy shares the motif's contraction and weight, and you
  must author a defect to get variance back. A repeat's k-th copy carries
  `shrinkᵏ`. The variance is already in the structure. `fiddlehead` has a stem
  but no defect, and `--info` correctly declines to ask for one.

  **So the dimension sum changes shape again.** The `Σ|G|·sᵈ` fix in the entry
  below assumes the copies are all the same size, which is exactly what a
  repeat breaks: the sum is `Σₖ (s·shrinkᵏ)ᵈ`. Twenty copies tapering at 0.9
  behave like ~4.8 full-size maps, not twenty, so a repeat tolerates a much
  larger motif than a group of the same count — `fiddlehead` sits at `s = 0.476`
  against `reliquary`'s 0.172. Getting this wrong is not a rounding error: with
  the taper ignored the first draft's `d` is overstated by about a third.

  **The failure I hit: the first draft was dust.** `scale = [0.34, 0.16, 0.34]`
  under a 20-copy repeat measured `d = 1.50`, occupancy 9%, Λ 16.7 — a thin
  scatter. The instinct carried over from `reliquary` ("many copies, so make
  each one small") is backwards under a taper, because most of the copies are
  already small. Solving `Σₖ (s·0.9ᵏ)ᵈ = 1` for `d = 2.2` gives `s ≈ 0.48`, and
  `scale = [0.60, 0.30, 0.60]` landed `d = 2.18`, occupancy 18.8%.

  **`shrink` is capped at 1 and the cap is not a limitation.** `Sᵏ ∘ f` has
  linear part `S_linᵏ · f_lin`, so a growing step makes the far copies expansive
  and the walk unbounded. A growing repeat of `N` copies from `m` is the same
  picture as a shrinking one from `S^(N-1) m` — re-anchor rather than reach for
  it.

  **And the thing worth knowing before asking for more groups:** the finite
  subgroups of SO(3) are `C_n`, `D_n`, `T`, `O`, `I`, full stop. That is a
  classification, not the five somebody got around to implementing. The reason
  `T`/`O`/`I` feel more three-dimensional than `C_n`/`D_n` is exact rather than
  aesthetic — the cyclic and dihedral groups fix an axis, so they are plane
  patterns extruded, while the polyhedral three fix no direction at all. New
  vocabulary has to leave SO(3): reflections, translations (this entry), or
  conformal maps.

- **2026-08-08, Opus 5. Symmetry groups are native now** (`[[symmetry]]` in a
  scene, `src/symmetry.rs`, §3.6's subject). Three things fell out that change
  how you author against this section.

  **The similarity dimension has to count the orbit, and nothing said so.**
  `Σsᵢᵈ = 1` is a sum over the *effective* map set, so a motif under `G`
  contributes `|G|` terms, not one: `Σ|Gᵢ|·sᵢᵈ = 1`. Before this was fixed a
  single map under `C5` reported `d = 0` ("dust") — one contraction can never
  sum to 1 — when the honest answer is `ln|G| / ln(1/s)` = 2.0. Anything in §2.1
  applied to a symmetric scene needs the orbit counted or it is nonsense.

  **The consequence for authoring is that `|G|` and `s` trade off hard, and the
  arithmetic is worth doing before you render.** For one motif,
  `d = ln|G| / ln(1/s)`, so the scale that keeps you in the corridor falls fast
  as the group grows: for `d ≈ 2.2`, `C5` wants `s ≈ 0.48` and `I` wants
  `s ≈ 0.16`. My first `reliquary` draft was `I` at `s = 0.36` and measured
  `d = 4.08`, occupancy 54%, Λ 1.9 — a solid ball, and `--info` said so before
  I looked at it. Sixty copies of anything is a lot of material.

  **Per-axis scale is the way out.** Going to `scale = [0.11, 0.42, 0.11]` —
  the same map as a strut rather than a bead — put the contraction at 0.172 by
  determinant and landed `d = 2.45`, Λ 3.4. Sixty struts weave a case; sixty
  beads are gravel on a sphere. The §3.2 trunk trick, doing a second job.

  Unmeasured still: §3.6's defect dose. `reliquary` follows `rose_window`'s 18%
  because that is the one data point, and it looks right, but one scene at one
  value is not a measurement.

- **2026-08-08, Opus 5. Measured, over all 44 scenes in `scenes/`.** Lacunarity
  works, but only after being normalised — and the un-normalised version is a
  trap worth knowing about.

  Raw gliding-box `Λ = 1 + Var/Mean²` on a voxel ladder (4/8/16/32 per side)
  **rose monotonically on every one of the 44 scenes.** There is no plateau to
  find, so the shape of the raw curve separates nothing. The reason is that
  scattering `N` points over `C` cells at random already scores `Λ ≈ 1 + C/N`
  whatever the shape is; at 2000 sample points the n=32 rung has a floor of
  17.4 that no scene can go below, and every scene sat just above it in
  proportion to how empty its bounding cube was. A one-number summary of that
  is a restatement of `occupancy`, which `--info` already prints.

  Dividing each rung by that chance expectation fixes it. The number becomes an
  *excess*: 1.0 = as clumped as a random scatter, higher = real gaps at that
  scale. On the repo's scenes it orders them the way the eye does, and the
  spread is wide enough to act on:

  | | Λ | |
  |---|---|---|
  | `menger` | 1.6 | flat measure by construction — the §3.6 prediction, confirmed |
  | `diamond`, `vortex` | 2.2–2.6 | regular, little to resolve |
  | `sierpinski` | 4.1 | |
  | `wellspiral` | 10.2 | the known-good target |
  | `pearl` | 17.3 | |
  | `glasshouse`, `blossom` | 40–47 | structure at every scale |

  The cross-check that earns it a place: `menger` is the scene §3.6 fingers by
  hand as a symmetry orbit with a flattened measure, and it is the *only* scene
  the metric puts below 2.0. `--info` now raises a note at `d > 2.5 && Λ < 2.0`
  — diagnosis, never refusal.

  What is **not** shown: that Λ separates a fuzzball from a good scene. Every
  scene here is one somebody kept. The Gemini-authored corpus §1.1 asks for is
  still the sharper test and is still unrun.
- **2026-08-04, Opus 5.** Similarity dimension predicts dust/shell/mush (§2.1).
  Out-of-plane tilt sweet spot is 5–12° (§3.1). Expanding maps break
  `color_falloff` and render monochrome (§2.3). `boxfold` inside ±1 is a no-op
  (§3.4). Infinite-zoom stills are textures, because distance is wrapped (§4.3).
  Scenes: `rimefall.toml`, `blossom.toml`.
- **2026-08-07, Opus 5. Open, not measured.** §3.6: a symmetry orbit flattens the
  measure by construction, which is the §2.1 failure mode arrived at from a new
  direction — `menger` is a 20-map octahedral orbit with no `weight` line in the
  file. Claim: one map outside the group is what makes a symmetric scene readable.
  The dose is the open question; `rose_window`'s core is 18% of the walk and works,
  against §3.2's 1–2% for a degenerate map. Sweep it and write the number down.
  Also open, from the same conversation: `menger` under `color_mode = "mix"` (§4.2).
  Its 20 sub-cubes carry distinct colours, so the RGB EMA should encode which
  sub-cube path a point took — a colour route into structure that §2.1 treats as
  purely a density problem. Untested. One render settles it.
- **2026-08-04.** Dimension describes the *support*; splat renders the *density*.
  A high-`d` scene with lumpy measure recovers under splat (the d = 3.42 case goes
  from flat silhouette to layered shells); one with uniform measure, like `menger`,
  does not. §2.1.
- **2026-08-07, Gemini 3.6 Flash.** Background color design with intent (§2.5). Avoid
  defaulting to generic black (`[0, 0, 0]`); choosing a tailored linear RGB tint
  (`background = [r, g, b]`) grounds atmospheric haze, harmonizes with palette
  shadows, and establishes overall tonal depth.

- **2026-09-26, Opus 5.5. Composites (`[[subscene]]`) — the renderer is chosen
  by the solid, not the flame.** `scenes/censer.toml` puts the Menger sponge
  (flat measure, Λ ~1.9) under the dragon smoke (sheets, Λ ~7.6). In splat the
  sponge is fog: splat is additive, so a ray through a sponge crosses all its
  tunnels at once and the carpet on each face averages away — the §2.1 uniform-
  measure failure, now with nowhere to hide because the smoke beside it shows
  what "structure" looks like. In points it is carved stone: opaque points show
  each face's carpet and hide what's behind. The smoke reads well in both. So a
  composite with a solid in it wants points, and the flame pays the (small)
  price. Two smaller findings from the same piece: turning the sponge to the
  camera's yaw (35.5° here) lines its tunnels up and they read as tunnels; and
  `share` is a weak brightness lever under the log tonemap (0.3 → 1.0 moved the
  smoke's mean luminance 64 → 76) — use it for grain and balance, and read the
  `density` line in `--info`'s `layers` section before rendering.

- **2026-10-12, Opus 5.5. The sponge's holes arrive through colour (§4.2's open
  question, settled).** Bounded `menger` (zoom stripped, seen from outside, the
  points renderer), only the colour changed:

  ```
  transforms ring, any speed      speckle on a grey-violet brick
  mix, color_speed 0.2 -> 0.85    0.2 pastel wash; 0.85 the 20 sub-cubes as
                                  solid blocks, face-centre holes as recesses
  mix, color_falloff 0.15         a fine lattice, washed toward pastel
  mix, color_falloff 0.6          three levels of cubes-of-cubes  <- best
  mix, color_falloff 1.0          two levels, strongly coloured
  ```

  Fixed `color_speed` can only ever show the top level, because the most
  recent map dominates the EMA. `color_falloff` weights each step by the
  scale it controls, so each level of the recursion gets its own share of the
  hue. That is what makes this a colour answer to the problem §2.1 frames as
  density: the measure is still flat (`--info` still raises its lacunarity
  note, correctly about density), but the *path* each point took is not, and
  mix mode records the path. `scenes/ledger.toml`; `menger.toml` itself is
  untouched (it is a zoom scene, and this was measured on the bounded set).

  **Under a native `[[symmetry]]`, mix sees motifs, not copies.** The RGB is
  per transform (`chaos.wgsl`, `color_rgb`), and the drawn group element never
  touches it, so every copy of a motif is one colour to mix. That turns out
  to be enough. I rebuilt the sponge as two motifs under `Octa` (a corner
  cube at weight 8, an edge cube at 12; lacunarity 1.9, the same set). With
  just two colours, corner-versus-edge at every level reads as a two-tone
  fractal, at least as clearly as the 20-colour version, and already at
  falloff 0. Two symbols are enough for the walk's word to say something;
  what mix needs is *some* maps that differ, not every copy distinct.

  The same rebuild shows a dimension caveat. `--info` reports 3.52 against
  menger's 2.72, because it counts all 24 copies of each motif, while a corner
  cube is fixed by 3 rotations and an edge cube by 2. Those copies differ as
  maps but land on the same sub-cube. This is §2.1's overlap caveat, not a
  bug, but it bites hard on a motif that sits on a symmetry axis.

  Same session, `scenes/sheaf.toml`, bred rather than designed: a `Dih5` cage
  of tilted struts (`[0.16, 0.5, 0.16]`, 15% `bubble`) plus one off-group map.
  One `--mutations 8` sheet scaled both maps ~1.15x into a single bundle. The
  dihedral flip gives it two ends, so it reads as a sheaf of straw tied at
  the waist. Two sweeps settled it: rib tilt 8 -> 30° (a column of reeds ->
  splayed ears), and the defect's weight 0.04 -> 0.3, where it stops being a
  ghost and becomes the knot. d = 2.08 with notes `none`, and it looked right
  first time, which is §2.1 earning its keep again.

- **2026-10-12, Opus 5.5. Two lessons from `scenes/sodium_sheaf.toml`.**
  **A post is a copy long enough to overlap its neighbours.** A degenerate
  strut map draws the whole attractor squashed into a column, gaps included,
  so the column is only solid where the copies overlap. At y-scale 0.55 to 0.7
  the posts broke into fragments with the lamp heads floating off them; at 0.9
  they ran solid. `--info` saw it first: d = 1.50 (dust) before, 1.86 after.
  **A part that must stay its own colour needs its own speed.** In mix mode
  the tyre's slabs are copies of the whole piece, gold heads included, and at
  the scene's 0.6 they rendered as gold triangles. A per-map
  `color_speed = 0.95` makes the colour land on arrival, and the band went
  tyre-black. This is kintsugi's gold, run the other way.

- **2026-10-14, Opus 5.5. Xaos, first scene (`scenes/parasol.toml`).** Three
  maps — a `Cyc5` petal, a spine, a relic — and two cut edges. What the
  sheets taught:

  **Cut, don't scale, when you want a different shape.** A sweep of one edge
  through 1, 0.3, 0.1, 0 gives three identical silhouettes and then a new one:
  only the zero moved material. A multiplier is a density dial, the same kind
  of lever as a weight.

  **"Never put X after Y" is the move for keeping a defect where you put it.**
  Without xaos the relic is a map like any other, so every petal carries a copy
  of it and the canopy goes pink. Cutting `petal → relic` keeps it a jewel at
  the heart and the petals pure — weight cannot do that, it only makes the
  relic fainter everywhere at once.

  **A cut can drain the picture.** Cut a map's other exits and its own loop
  becomes an absorbing class: everything else's walk share goes to zero and the
  picture collapses onto that one map. `--info`'s notes say so and print the
  edge that reconnects it; read them before rendering.

  **`--edge-grid` answers "what would this cut remove?"** Each tile is the
  material one edge places at the top level. The canopy is `petal>petal`;
  the relic lives in `spine>relic`. Look before you cut.

  Two famous presets disappoint on a defect scene: `cycle` collapses three maps
  to a finite set (dimension 0, as Mauldin–Williams predicts) and
  `no-self-loops` strips a symmetric motif of its own lace, since the lace *is*
  the motif following itself.

- **2026-10-23, Opus 5.5. A gas ring (`scenes/hob_opus55.toml`): the group's
  order draws the outline, and the defect lands one scale down.** Two motifs
  under a `Cyc` group plus one map outside it.

  **A squashed ring is a ring; stand it on edge.** The first draft squashed
  each jet vertically (`[0.1, 0.3, 0.1]`) and measured a Y-spread of 0.085:
  all jets sit at one height, so the attractor's height `H` solves
  `H = s_y·H` and goes to zero. The mini-hobs read as little crowns. Height
  has to come from turning the ring's *diameter* upright: scale one axis of
  the ring thin, then rotate ~70° about the tangent so the diameter becomes
  the flame's height. Each jet is then a standing loop, limb-bright like a
  real hollow cone.

  **The group's order is a shape control, not a count.** At `Cyc12` and
  `Cyc18` the loops were separate eggs, closed and rounded. Six variations
  on the mantle (julia, swirl, horseshoe, bent, spherical, to 0.45) failed
  to pinch a tip: with the variation weights summing past 1, they mostly
  enlarge the map. At `Cyc30` neighbouring loops overlap, and the overlap
  draws a row of pointed tongues along the rim. d went 1.72 → 2.05 on the
  same motif. When the motif won't take the shape, try the orbit.

  **A defect is inherited.** Every jet is a copy of the whole ring, yellow
  flame included, so every jet carries a small copy of the defect. In
  palette mode the cone's `color_speed` pulls that copy's index back from
  0.8 towards 0.47, through the palette's white stop at 0.62, so each blue
  flame grows a white-hot inner cone that nobody authored. It reads exactly
  like a gas flame's inner cone. Checked: with `-S
  transform.catch.color_value=0.47` the white disappears and the loops stay.
  §3.6 says a defect's *shape* is decided the moment it exists; this is the
  same fact seen from the copies' side. What a defect adds to the motifs is
  also decided by its geometry, so place it with their copies in mind. A
  bigger `catch` (scale.x 0.4 → 0.7) also sprouts fine spikes at every
  jet's tip.

  **`julia` forks in world space.** Its random branch is a point reflection
  through the origin *after* the affine, so on a map far off-centre the
  second fork lands across the ring. In most orbit views the yellow flame's
  twin floated in the middle of the burner. Fine for a centred map,
  surprising on a placed one.

  **`--density-estimation` did nothing here, which was the answer.** A
  premixed flame is laminar and crisp, so the lace is true to the subject.
  The softness belongs to the one diffusion flame, which got `swirl`.

  Tooling found on the way, and fixed: a sweep sheet framed every tile from
  the base scene, so `--sweep camera.pitch=…` gave identical tiles under
  honest-looking labels (AGENTS.md, Parameter sweeps). Also fixed the same
  day: `-S symmetry.<index-or-motif>.<field>` now reaches `[[symmetry]]`, so
  `--sweep 'symmetry.0.group=Cyc12,Cyc18,Cyc30'` is one command.

  **Hazel's verdict, worth keeping: the self-similarity read as glitching in
  the fine detail.** A flame is not a ring, so the small rings inside each
  flame look like artefacts, not structure. Self-similarity is a feature
  when the subject's parts are already like its whole, and a defect when
  the IFS has to hide it. See the next entry.

- **2026-10-23, Opus 5.5. Romanesco (`scenes/romanesco_opus55.toml`): one
  map, a repeat, and the tail of the spiral as one more map.** Chosen as the
  answer to the Hob verdict above: a subject that is self-similar in life.
  `bud` is the whole head at uniform scale. Uniform matters: squash it and
  you get Fiddlehead's conifer, because the part stops resembling the whole.
  A golden-angle repeat whose step has its fixed point at the apex puts the
  buds on a logarithmic spiral on a cone.

  **Packing is the repeat's `shrink`, not the bud's scale.** At shrink 0.93
  to 0.95 the buds stood apart as separate trees on a hill, whatever the bud
  size. At 0.97 they packed into a cone of cones, the Romanesco silhouette,
  d = 2.25 (shell). One `--sweep symmetry.bud.shrink × transform.bud.scale`
  sheet found it.

  **A truncated repeat is finished exactly by one map.** `MAX_REPEAT` is 64.
  Copies past N are `S^N` applied to the whole attractor, so a plain map
  equal to `S^N` (scale `cᴺ`, turn `N·turn`, lift `step·(1−cᴺ)/(1−c)` for a
  step along the axis) *is* the infinite tail, not an approximation.
  Without it the tip is a hole, and so is every bud's tip.

  **And it needs far more than one copy's weight.** A repeat gives each copy
  `w/N` whatever its size, but `S^N` stands in for infinitely many copies.
  At `w/64` its region was a dark well that looked like a colour bug: the
  haze off made no difference, `--isolate apex` showed a thin scatter. 0.12,
  about eight copies' worth, matched the density at the seam. When a map's
  region renders dark under splat, check whether it is under-sampled before
  touching the palette.

  **`color = "orbit"` is `fract(color_value + k/N)`.** It is a ramp only if
  `color_value = 0`, and then copy index reads as age: dark base, pale tip,
  repeated inside every bud. At the default 0.5 the ramp starts mid-palette
  and wraps through the dark end partway up the form.

