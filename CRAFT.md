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
everything with `--effort low --splat --width ~340`: a contact sheet costs **under a
second** on the reference desktop, so the loop is look-change-look, not plan-commit-hope.

The loop, concretely: `--sweep <path>=<a:b>` varies one scene value across a
labelled sheet, `--set <path>=<value>` pins the rest, and `--mutations N` throws
dice when you don't yet know what to vary. None of the three edits the file.

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
    --effort low --splat --exposure 1.3 --width 320 --height 320
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

### 3.6 Symmetry plus one — the defect is the picture **[claim — dose not yet measured]**

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

**The dose is unmeasured, and do not assume it is a whisper.** §3.2's stem wants
1–2%; the one data point here points much higher. `rose_window` is three petals at
120° about Y (weight 1.5 each) plus an on-axis core at weight 1.0 — **18% of the
walk** — and it is one of the better-looking scenes in the repo. My guess is that
the two cases differ because the stem is a *degenerate* map (it collapses the plane
to a line, so a little goes a long way) while a core is an ordinary contraction, but
that is a guess.

**The experiment — one scene and one sweep sheet:** build a clean `C5` or `C6` ring
(five or six maps at equal weight, identical but for the rotation), then sweep a
single extra off-axis map's weight from 0.5% to 30% of the walk. Record where it
stops reading as wallpaper and where it starts eating the symmetry. Put the number
here and delete this paragraph.

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

**The one of those three that might be cheap — untested, go and try it.** `menger`
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

1. **No numeric feedback on the rendered image.** Judging "washed out", "clipping",
   "too sparse" by eye means a visual round trip per exposure guess. A `--stats`
   printing mean/max luminance, % clipped pixels, % empty pixels, and the
   **colour-index histogram** would catch the monochrome failure in §2.3 instantly —
   that histogram is a spike, and a spike is invisible in the picture. This is the
   most agent-shaped affordance missing.
2. **`--info` knows every contraction but doesn't compute the similarity
   dimension** (§2.1), the single best predictor of the look. It also prints
   per-transform contraction but not the *resolved* `color_speed`, which is what
   exposes the expanding-map trap.
3. **No crossover.** `--mutations` is half the genetic loop; there's no way to breed
   two scenes together, which is what Electric Sheep actually did.

None of these are architectural. The engine is in good shape; the authoring surface
is one notch behind it.

---

## 9. Discovery log

Append findings here. Date them, say what you measured, keep the failures — a
recorded dead end is worth as much as a recipe.

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
  as the group grows: for `d ≈ 2.2`, `C5` wants `s ≈ 0.33` and `I` wants
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

