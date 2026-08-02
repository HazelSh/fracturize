# Plan: palette-based colouring, alongside the per-transform mode

*Claude Opus 5, 2026-08-02. Design only — no code written for this yet.*

Hazel's framing: add an Apophysis-style palette mode where the gradient is an
independent asset, and **keep** the current per-transform-RGB mode as the lane
for experimental attempts at surfacing IFS structure through colour. This is
also `todo.txt`'s colour item, which asks for random palette generation, for
careful thought about gradients' strong and weak points, and for colouring to
be "broken out from other rendering steps, made swappable".

---

## 1. What the code already does

This turned out to be the important part, so it goes first.

**The renderer is already a palette renderer.** The chaos shader writes an
8-bit index, not a colour:

```wgsl
let color_idx = u32(clamp(color_val, 0.0, 1.0) * 255.0);
points[output_idx] = Point(out_pos, color_idx);
```

and both point shaders resolve it against a 256-entry storage buffer:

```wgsl
@group(0) @binding(2) var<storage, read> colormap: array<vec4<f32>, 256>;
let f = (f32(color_idx & 0xFFu) + 0.5) / 256.0;
return colormap[u32(fract(0.5 + (f - 0.5) * contrast) * 256.0) & 0xFFu].rgb;
```

`scene::generate_colormap(&scene.colors)` is the **only** producer of that
buffer's contents. It takes the per-transform RGBs and spreads them evenly
around a cyclic ring.

Three consequences, all of which shape the plan:

- **Palette mode needs zero GPU changes.** Not "few" — zero. It is entirely a
  question of what fills those 256 entries. Everything downstream of stage 2
  (below) already works the way Apophysis works.
- Fracturize already has the rest of the flam3 colour model under different
  names: per-transform `color_value` *is* Apophysis's `color` (position in the
  gradient), `color_speed` / `color_falloff` *are* its colour-speed/symmetry
  (how fast the running index moves), and `color_contrast` is a render-time
  cyclic stretch that Apophysis lacks.
- **The current mode is not actually RGB mixing.** It derives a palette from
  RGBs and then does 1-D indexing, so two transforms' colours never mix as
  colours — they mix as *positions*, and what you see between them is whatever
  the ring interpolation put there. Worth knowing before calling it the
  "experimental structure" mode; §3 says what would make it one.

**A defect in the current model, while we're here.** `generate_colormap`
spreads N colours evenly around the ring, so *adding a transform moves every
other transform's colour*. Author a scene you like with four maps, add a fifth,
and all five have shifted. That is a good independent reason to want a palette
that doesn't depend on the transform count.

**Two pieces of free space**, which make §6 much cheaper than expected:

| | size | used | free |
|---|---|---|---|
| `Point` | 16 B | `[f32;3]` pos + **8 bits** of `color_idx` | **24 bits** |
| `WalkerState` | 48 B | pos, `current_color: f32`, rng | **3 f32** (`_pad1..3`) |

So carrying a full RGB triple through the walker *and* storing it per point
costs nothing in memory. That is not a small thing on a renderer whose entire
constraint is VRAM.

---

## 2. The three stages, and which one becomes swappable

Colour currently happens in three fused stages:

1. **Accumulate** — walker history → scalar `c ∈ [0,1]`. An EMA over the
   transforms' `color_value`s, at a rate set by `color_speed` or derived from
   contraction by `color_falloff`.
2. **Map** — `c` → RGB. The 256-entry colormap.
3. **Grade** — render-time. The cyclic contrast stretch; haze desaturation.

Hazel's ask is precisely: **make stage 2 swappable**, with two sources.

- `transforms` (today): the ring built from per-transform RGB.
- `palette` (new): an independent gradient.

Stage 1 stays shared. That is the right seam: it keeps `color_speed`,
`color_falloff` and `color_contrast` meaningful in both modes, and it means the
new mode inherits the scale-aware accumulation work already done.

§6 proposes a *third* source later that also touches stage 1, which is where
the experimental lane actually gets new power.

---

## 3. Thinking carefully about gradients

`todo.txt` asks for this specifically, so here it is rather than a shrug.

### What a gradient is, structurally

A 1-D → RGB lookup sitting downstream of a lossy reduction of the walker's
history to one scalar. The walker's history is a *word* over N symbols; a
scalar cannot distinguish most words. **That reduction, not the gradient, is
where the information goes.** Both current and proposed modes share it.

### Strengths

1. **The palette becomes a portable, curatable asset.** Structure and styling
   become separable jobs — you can restyle a finished flame in one edit, and
   collect gradients that reliably look good. flam3 ships ~700 palettes and
   that library is a large part of why Apophysis output looks coherent even
   from beginners. Nothing in fracturize offers that today.
2. **Harmony by construction.** A harmonious gradient makes every image from it
   harmonious. Under per-transform RGB, harmony is the author's problem on
   every scene, *and* it silently degrades as transforms are added (§1).
3. **Continuity reads as form.** Adjacent indices give adjacent colours, so
   colour varies smoothly with structure. A gradient with a designed luminance
   ramp is what gives flames their pseudo-shading — the renderer has no lights,
   so the palette is the lighting.
4. **Independent of transform count.** Three maps or thirty, same gradient.

### Weaknesses

1. **1-D is a severe bottleneck, and it is the real one.** Unrelated histories
   collide on the same index. A gradient can express *an ordering* of structure
   but can never *label* it.
2. **The index is arbitrary.** There is no natural ordering of transforms, so
   which one gets which colour is a free choice, and neighbouring indices need
   not be structurally related. "0.37" means only "the EMA landed here".
3. **It hides transform identity** — which is exactly what you don't want when
   using colour to understand an IFS. This is the honest reason to keep a
   second mode rather than just a second palette source.
4. **8-bit quantisation.** 256 distinct colours, fewer once `color_contrast`
   stretches them. Fine for grit, visible as banding in smooth splat regions.
5. **It fights `color_falloff`.** Scale-aware accumulation compresses the index
   toward the mean (already documented in AGENTS.md), so with a *designed*
   palette you notice you are only seeing an arc of it. `color_contrast` exists
   to compensate and will need to be surfaced next to the palette strip, not
   buried among the render settings, or people will think their palette is
   broken.

### What else colour could do

Listed because the second mode should eventually be *more* than "the old
palette source", and these are the candidates:

| Idea | What it would show | Cost |
|---|---|---|
| **RGB accumulation** (§6) | Distinct transform *combinations* as distinct colours, not just positions | Free in memory; stage-1 change |
| Last transform only (`color_speed = 1`) | Flat labelling of top-level copies | Already possible |
| Colour by accumulated contraction | Feature **scale** directly — a depth map of the fractal | Small; the value is already computed for `color_falloff` |
| Colour by nearest transform fixed point | Voronoi of the maps' attractors | Cheap CPU-side per point |
| Colour by iteration age since re-seed | Where the chaos game is still transient | Trivial |

The third is the one I would build first after RGB accumulation: under infinite
zoom especially, "what scale is this feature" is a question the renderer can
answer and currently doesn't.

---

## 4. Palette representation

**Canonical form: control points.** A list of `(position, colour)` stops,
interpolated. Reasons: it is what an editor manipulates, it is compact and
diffable in TOML (unlike 256 triples), and it subsumes the current model
exactly — the transforms ring is N stops evenly spaced with `cyclic = true`.

Supported alongside it:

- **256 explicit entries**, for importing flam3 / Apophysis palettes verbatim.
- **Cosine palettes** — Iñigo Quílez's `c(t) = a + b·cos(2π(c·t + d))`, twelve
  numbers, always smooth. Excellent as the *random generator* (§7) and as a
  compact authored form.

Two properties a palette must declare:

- `cyclic` — fracturize's lookup wraps (`& 0xFFu`) and the contrast stretch
  depends on it, so cyclic is the default and matches existing behaviour.
  Imported flam3 palettes are authored for a clamped 0..255 index and may have
  a seam; the flag lets them say so.
- `interpolate` — `rgb` (flam3-compatible, the default, since the goal is to
  match Apophysis more closely) or `oklab` (no muddy midpoints between
  complementary stops; perceptually even). Offer both, default `rgb`.

### Scene format

```toml
[palette]
name = "ember"                 # from the library, OR:
cyclic = true
interpolate = "rgb"
stops = [
  { at = 0.00, color = [0.02, 0.01, 0.05] },
  { at = 0.35, color = [0.85, 0.35, 0.12] },
  { at = 0.70, color = [0.98, 0.90, 0.62] },
]
# ...OR procedural:
# cosine = { a = [...], b = [...], c = [...], d = [...] }

rotate = 0.0                   # shift the whole gradient along the index
reverse = false                # applied on top, so a library palette can be
                               # tuned per-scene without forking it
```

**Presence of `[palette]` selects palette mode.** One mechanism, no redundant
`color_mode` key to fall out of sync. Per-transform `color` stays in the file
regardless — it is still meaningful in the other mode, and a scene can carry
both so you can A/B with `--color-mode`.

In palette mode each transform's `color_value` is its position in the gradient
(auto-spread when absent, as now) — i.e. exactly Apophysis's per-transform
colour.

---

## 5. Surfaces

### CLI

```
--palette <name|path>          # library name or a file; overrides the scene
--color-mode transforms|palette
--random-palette               # honours --seed; prints the palette so a good
                               # roll can be kept (same convention as --random)
--palette-rotate <t>  --palette-reverse
--palettes                     # list the library and exit
```

`--palette` on an existing scene restyles it without editing it — the same
spirit as `--zoom`, and the same reason: trying a thing should not require
authoring a file for it.

`--info` gains a palette section: mode, source, stop count, and **a swatch
printed as 24-bit ANSI colour blocks**. An agent reading `--info` can then
actually see the gradient rather than parse twenty-four floats and imagine it.
That is a small thing that changes how usable the tool is from a terminal.

### GUI

Layout is Hazel's, so this is a proposal, and deliberately additive.

The Render window already owns the colour controls (`color falloff`,
`color contrast`), so the palette belongs there:

- **A gradient strip** — the resolved 256-entry colormap drawn as a bar.
  Control-point handles beneath it: drag to move, double-click to add,
  right-click to delete, click to open egui's colour picker.
- **A row above it**: mode toggle (`transforms | palette`), library dropdown,
  `Random`, `Rotate`, `Reverse`.
- **In `transforms` mode the strip is read-only but still drawn.** You
  currently cannot see the colormap you are getting, and that is a real gap —
  this is nearly free and worth having on its own.
- Move `color_contrast` next to the strip, per §3's fifth weakness: when it is
  compressing a designed palette into an arc, the control that fixes it should
  be adjacent to the thing that looks wrong.

In the Transforms panel, the per-transform colour swatch should become a
`color_value` slider **drawn over the palette strip** when in palette mode, so
you can see where that transform lands in the gradient. That is the Apophysis
idiom and it is the one piece of UI that makes the model click.

---

## 6. The experimental lane: widen the colour channel

Not part of Hazel's ask, but it is what makes the second mode worth keeping,
and §1 found it is nearly free.

Carry a **Vec3** through the walker instead of a scalar (`WalkerState` has
three spare floats), and pack it into the 24 free bits of `Point.color_idx` as
8/8/8. Then per-transform RGB genuinely mixes: a walker that came via a red map
and then a blue one is *purple*, distinguishably from one that came via two
magenta maps. Distinct transform combinations become distinct colours, which is
the thing 1-D indexing structurally cannot do.

Costs: zero extra memory; one branch in the chaos shader; a second shader path
that skips the colormap lookup. It also removes the 8-bit banding for that
mode.

I would treat this as a separate piece of work after the palette mode lands,
not as part of it.

---

## 7. Random palette generation

`todo.txt` asks for this. Four generators, in the order I would build them:

1. **Cosine palettes.** Sample `a, b, c, d`; always smooth, always harmonious,
   good variety, twelve numbers to store or print.
2. **Harmony schemes.** Base hue + analogous/complementary/triadic/split, N
   stops, varied S/V. `randomize.rs` already does exactly this for transform
   colours — reuse it rather than write a second one.
3. **Sample the library.** Highest quality per unit effort, once there is a
   library.
4. **Perturb an existing palette.** Hue-rotate or re-sample; `mutate.rs`
   already has `rotate_hue`. Wire into the mutation operators so `U` mutates
   the palette too, and `--mutations` sheets explore colour as well as form.

**Two constraints that separate "random colours" from "good palettes":**

- **Impose a luminance sweep.** The renderer has no lights; the palette *is*
  the shading. A gradient that is uniformly mid-bright renders flat no matter
  how pretty the hues are. Generators should ramp luminance deliberately.
- **But the colormap is cyclic**, so a monotone ramp puts a bright/dark seam at
  index 0. For cyclic use, luminance should rise and fall once across the
  gradient. This is the kind of thing that is obvious in hindsight and
  invisible until you render a hundred bad rolls.

---

## 8. Phasing

| Phase | Work | Size |
|---|---|---|
| 0 | Extract stage 2: a `Palette` type with `to_colormap()`; `generate_colormap` becomes `Palette::from_transform_colors(…)`. No behaviour change. | S |
| 1 | `[palette]` in scenes; stops + cosine + library; CLI flags; `--info` ANSI swatch | M |
| 2 | GUI: gradient strip + editor, mode toggle, contrast relocation, `color_value` slider over the strip | M–L (the widget is the work) |
| 3 | Random generation + mutation integration | S–M |
| 4 | *Separate:* widen the colour channel (§6) | M |

Phase 0 is worth doing on its own even if the rest waits — it is the
"break colouring out from other rendering steps" that `todo.txt` asks for, and
it makes every later phase small.

---

## 9. Open questions — these are Hazel's calls

1. **Where does library content come from?** A palette mode is only as good as
   its palettes. I can author ~15–20 good ones. Vendoring flam3's
   `flam3-palettes.xml` would give ~700 at once, but it is GPL'd and this
   project's licence isn't stated — I would rather flag that than quietly
   bundle someone's asset. An importer plus your own collection sidesteps it
   entirely.
2. **Default interpolation** — `rgb` for Apophysis fidelity, or `oklab` for
   quality? I have assumed `rgb` because you said "match Apophysis more
   closely", but the muddy-midpoint problem is real and `oklab` is strictly
   nicer to look at.
3. **Should palette mode change what `U` mutates?** Mutating hue per transform
   is meaningless once a palette owns the colours; it should mutate
   `color_value`s and the palette instead. That is a behaviour change to an
   existing key.
4. **How much of §6 do you actually want?** It is the difference between the
   second mode being "the old way" and being a genuinely different instrument.

## 10. Deliberately not doing

- **Not touching stage 1's accumulation.** `color_speed` / `color_falloff` are
  working, documented, and orthogonal to this.
- **Not replacing the per-transform mode**, per your instruction — and §3 gives
  the principled reason it should stay rather than just deference.
- **Not a full Apophysis `.flame` importer.** Palette import is a small,
  well-defined piece of that and useful on its own; the rest is a separate
  project.
