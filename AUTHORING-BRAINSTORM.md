# Staying on the interesting path — a brainstorm

*Status: brainstorm, not a plan. Nothing here is measured unless it says so; the
things that cite CRAFT.md or a file:line are grounded, the rest are proposals with
an experiment attached. Written 2026-08-07.*

**If you are picking this up to implement:** four things are Hazel's decisions, not
open proposals — the per-transform **post-affine slot** (§3.1), **symmetry declared
in the scene file and kept live in the renderer** rather than expanded at load
(§3.2), a **`symmetry` summary in `--info`** (§5, item 2), and **symmetry drawn into the
3D view with gizmos and editable from the GUI** (§4). §6 ranks the build order.
Everything else here is still a brainstorm, and §1's two diagnostics explicitly want
their calibration run *before* anyone writes a gate against them.

The ask: keep human and LLM authors in the corridor between **exploding fuzz** and
**trivial structureless points**, and make it possible to author *large* scenes —
probably via symmetry in how transforms are placed.

Those turn out to be the same problem, which is the main claim of this document.

---

## 0. One measurement first

The chaos shader selects transforms by binary search over a cumulative-weight array
in a **storage** buffer (`shaders/points/chaos.wgsl:85-127`) — there is no
`MAX_TRANSFORMS` anywhere in `src/` or `shaders/`. Hundreds of maps cost
`log₂(N)` per iteration and nothing else.

Meanwhile the largest scene in the repo is `menger.toml` at **20** transforms, and
the median is 4. `tools/lsystem_to_ifs.py --depth N` exists precisely to multiply
transform counts, and nothing it produced was ever checked in.

> **The engine has been ready for 200-transform scenes for a while. The authoring
> surface tops out around 6.** Everything below is about closing that gap.

---

## 1. Naming the two walls, numerically

Both walls are currently detected by eye. The repo has three numbers that touch
them, and they are the wrong three for this job:

| Exists | Where | Catches |
|---|---|---|
| radius bounds, axis-spread ratio | `randomize.rs:267` `acceptable()` | divergence, collapse to a point/line |
| `occupancy` ≤ max | `trace.rs:301` | solid bricks |
| similarity dimension `d` | CRAFT §2.1 — **computed by hand, not by the tool** | dust/shell/mush |

What's missing is a number for the thing Hazel actually means by *fuzz*. Fuzz is
not divergence and it is not high occupancy. A fuzzball is a **converged attractor
whose measure is smooth** — an IFS of randomly-oriented contractions produces
something very close to a Gaussian blob, because composing many incoherent
rotations is a random walk on SO(3) and the central limit theorem does the rest.
It passes every gate in `acceptable()`. It is exactly what `--random` throws when
it throws something dull.

### 1.1 The two diagnostics I'd build

**(a) Rotation coherence — why a scene is fuzzy.**
Take the rotation parts `Rᵢ` of the maps. Generate all words up to length 4 and
look at the resulting quaternion set. If the maps' rotations generate a *finite*
subgroup of SO(3), the word set closes: 60 distinct elements for icosahedral, 24
for octahedral, `n` for a cyclic axis. If they generate a dense subgroup, the count
grows as `Nᵏ` and the minimum pairwise geodesic distance collapses toward zero.

One number: **effective group order** (distinct rotations up to a tolerance, at
word length 4). Small and stable → crystalline. Growing without bound → fuzz.
Cost: a few thousand quaternion multiplies, microseconds, at design time.

This is the diagnostic that tells an author *which knob* — not "your image is
mush" but "your rotations don't close; snap them to a 5-fold axis".

**(b) A structure spectrum — whether a scene has anything to look at.**
Voxelise the CPU-sampled cloud (`trace::measure` already produces it) at a ladder
of resolutions and compute the classic gliding-box **lacunarity**
`Λ(r) = 1 + Var[mass]/Mean[mass]²` at each. Plot `Λ` against `log r`.

The hypothesis — and this is a hypothesis, see the experiment below — is that the
corridor is *a long plateau of elevated Λ*:

```
fuzzball        Λ near 1 at every scale            — smooth measure, nothing to resolve
menger          Λ near 1 at every scale            — uniform measure on a solid support
trivial/sparse  Λ high at coarse r, collapses fast — a few blobs, no depth
wellspiral      Λ elevated across many decades     — structure at every scale  <- the target
```

CRAFT §2.1 already argues, from measurement, that **density variance — not
dimension — predicts whether splat can recover a picture**. Λ is that sentence
written as a number, and it is the piece §2.1 explicitly leaves as a judgement call.

**The calibration experiment, which must come before any of this ships as a gate:**
compute Λ(r) for `wellspiral` (d = 1.91, known good), `menger` (flat measure by
construction — CRAFT §3.6), the d = 3.42 ring from §2.1 (known *recoverable* under
splat), `rimefall`, `glasshouse`, `pearl`, and a set of `--random` rolls Hazel sorts
by hand into keep/discard. If the curve doesn't separate keep from discard, throw
this section away and say so in the discovery log. If it does, it is the single best
number in the tool, because it works on both walls at once.

**Better than random rolls, for the fuzz half: use scenes other models wrote.**
Hazel's read of the Gemini-authored scenes arriving via Antigravity (e.g.
`scenes/astral_lattice.toml`) is that they're creative and small, with a standing
tendency toward *fuzzy, complex* forms — which is the fuzz wall, named as a habit
rather than an accident. That makes an LLM-authored corpus a sharper calibration set
than `--random` output, because it is the actual failure the diagnostic exists to
catch, produced by the actual authors it is meant to help. A Λ curve or a
group-order estimate that can't tell a Gemini fuzzball from `wellspiral` has not
earned its place in `--info`. (Hazel's judgement, not my measurement — I haven't
rendered any of them.)

### 1.2 Redundancy — the number that only matters once scenes are big

At N = 200 the first question is not "is this good" but "which of these 200 maps is
doing anything". Leave-one-out: re-run `trace::measure` with map *i* disabled and
compare occupancy/radius/centroid. A map whose removal changes nothing is
decoration, and in a generated scene there will be dozens.

`--info`'s `maps` block already prints weight share. Adding **contribution share**
next to it — "3.1% of the walk, 0.2% of the shape" — makes a 200-map scene
readable, and is the first thing that makes a big scene *editable* rather than
merely large.

---

## 2. Making the corridor a UI object, not a report

The numbers above are worth little as a report you have to remember to run. The
interesting versions put them *in the gesture*.

### 2.1 A corridor band drawn on the slider

`d` is a 200-step bisection — microseconds. So when you grab a `scale` slider, the
tool can solve for the range of that scale which keeps `d` inside the band, and
**shade it on the slider track before you drag**. Same for weight, same for a
gizmo's uniform-scale handle.

This is the most direct possible answer to "keep authors on the interesting path":
the dial shows where the good numbers are, in the moment you reach for it. Nothing
is forbidden — you can drag straight out of the band — but you can't do it by
accident, and you learn the shape of the space by seeing the band move as you edit
other maps.

The band's edges are a taste setting, not a law. Ship three presets matching CRAFT
§2.1's own table: **lace** (d 1.7–2.1), **shell** (2.1–2.6), **solid** (2.6+, with
splat implied).

### 2.2 Dimension lock

CRAFT §1 says form, density and colour are "genuinely separable" levers, then §2.1
shows that changing one map's scale moves `d` and therefore changes density
whether you wanted it to or not. They're separable in principle and coupled in the
UI.

**Dimension lock**: hold `d` fixed while editing. Drag one map's scale up and every
other map's scale is rescaled by the factor that re-solves `Σsᵢᵈ = 1` at the old
`d`. You get *"the same amount of stuff, arranged differently"* — a shape edit that
cannot explode and cannot collapse.

I think this is the sleeper feature in this document. It converts the most common
destructive edit into a safe one, it's about thirty lines, and it makes the
lace/shell/solid choice a mode you work *inside* rather than a thing you keep
falling out of.

Companions: **radius lock** (hold the framing), **share lock** (hold each map's
percentage of the walk while weights are edited elsewhere).

### 2.3 Live numbers where you're already looking

The status bar has an FPS sparkline (`src/ui/status_bar.rs`). Give it a second
readout: `d 2.14 · Λ▂▄▆▆▅ · 6 maps`, with `d` amber outside the active band. During
a gizmo drag it updates live, so you feel the dimension move under your hand.

### 2.4 Sweep and mutate, in the app

The CLI's `--sweep` and `--mutations` are, per CRAFT §8, the best things in the
tool — and they're unavailable while you're actually editing. Two ports:

- **Right-click any number → filmstrip.** Five tiles across the parameter's local
  neighbourhood, rendered at low effort and ~200px. Click one to adopt it.
- **The 3×3 mutation grid** already in `todo.txt`, with the centre tile as "now"
  and the eight neighbours as directions. This is the Apophysis interaction and
  the reason breed-and-select is the working method (CRAFT §1, §5).

Both should annotate each tile with `d` and the failure-mode word, so the sheet
teaches the numbers rather than just showing pictures.

### 2.5 Crossover, which symmetry makes newly meaningful

CRAFT §8.3 wants crossover and notes it's missing. Blending two flat transform
lists is ill-defined — which map pairs with which? Once scenes are *motif +
group* (§3), crossover has an obvious definition: **swap motifs between scenes and
keep the group, or swap groups and keep the motifs.** That's Electric Sheep's loop
with a genuine genome instead of a bag of floats.

---

## 3. Symmetry as the large-scene engine

This is the centre of the brainstorm, and it connects back to §1.1(a): the reason a
scene is fuzzy is that its rotations generate a dense group. **Constraining
rotations to a finite subgroup of SO(3) is simultaneously the anti-fuzz device and
the way to author 200 transforms from 3.**

### 3.1 The theorem, and the one thing it needs from the engine

For a finite group `G` and maps `{fᵢ}`, the IFS `{g ∘ fᵢ : g ∈ G}` has an attractor
that is exactly `G`-symmetric. Proof is one line: `A = ⋃_{g,i} g(fᵢ(A))`, so for any
`h ∈ G`, `h(A) = ⋃ hg fᵢ(A) = A`, because `hG = G`.

Note the composition order: `g` is applied **after** `f`. CRAFT §6.1 spotted this
already and correctly identified it as blocked: a transform here is
affine-then-variations, so `R ∘ f` is only expressible when `f` is pure affine
(fold `R` into the matrix). With any variation in the mix, it cannot be written
down.

> **So the enabling change is a per-transform post-affine slot** — one more `Mat4`
> applied after `apply_variations`, ~176→240 bytes per `GpuTransform` and one
> matrix multiply in the walk (`shaders/points/chaos.wgsl:430`).
>
> It is strictly cheaper than the final transform CRAFT §6.1 asks for, it is a
> different feature (a final transform gives `F(A)`, which is *not* symmetric), and
> it is the prerequisite for everything else in this section. If one thing gets
> built from this document, it's this.

Note also that placement-style generators (the L-system's "put a copy of the whole
attractor at this pose") are post-composition too — `tools/lsystem_to_ifs.py`
builds `pos + orient·(s·p)`, which is exactly `T ∘ S`. So the same slot serves both
symmetry groups and instancing arrays.

And the two features collapse into one mechanism, which is the nice part:

> **A post-affine slot is a symmetry group of order 1.** Fixed post-affine: apply
> `g`. Symmetry group: draw `g` uniformly from `G` each iteration. Same buffer, same
> multiply, one extra RNG draw. Build the group case and the fixed case is `|G| = 1`.

The chaos game is unchanged in the maths: picking `fᵢ` with weight `wᵢ` and then `g`
uniformly from `G` is *exactly* sampling the `|G|·N` map set `{g ∘ fᵢ}` with weights
`wᵢ/|G|`. No approximation, no convergence penalty.

### 3.2 Where the group lives: the renderer, not the loader

Hazel's call, and having thought about it properly I think it's right — but not for
the reason I'd have guessed. Two designs:

| | **A: loader macro** | **B: live in the renderer** |
|---|---|---|
| Scene file | `[[symmetry]]` block | `[[symmetry]]` block |
| On load | expand to `\|G\|·N` flat transforms | stays a group + `N` motifs |
| GPU sees | 120 transforms | 2 motifs + 60 group elements |
| Renderer | untouched | one buffer, one multiply, one RNG draw |

**First, the correction:** A does *not* prevent after-the-fact editing. The
expansion is a pure function of `(motifs, group)`, so the `Scene` struct keeps both
and re-expands whenever a motif is edited — the Blender modifier model. If the worry
was "flattening loses the symmetry", that worry is answerable in either design.
(§4 of the first draft muddied this by raising a "flatten is a one-way door"
question; that was about hand-editing a *single orbit member*, which is a much
narrower problem and mostly a thing to just disallow.)

**But B is still the better choice, because of what the ghosts do to everything
downstream.** In A, the transform list the rest of the program sees is 120 long, and
every consumer of it has to learn that rows 3–62 are copies of row 2:

- `gpu/gizmo.rs:238` builds one gizmo instance per transform — 120 gizmos.
- `pick.rs` ray-selection hits ghosts, so clicking a petal selects an uneditable copy.
- `src/ui/transforms.rs` grows a 120-row list and hits the overflow-eats-clicks bug
  recorded in the camera-window memory note.
- Colour indexing, `--info`'s `maps` block, weight share, mutation operators — all
  of them 60× inflated with data that carries no information.

In B none of that happens. The ghost problem doesn't get solved, it doesn't exist:
the program sees two transforms, and the orbit is drawn deliberately, where and when
it's wanted. **The complexity moves from twelve places that don't want it to one
place that does** — which is the trade worth taking, and it's a stronger argument
than the one about the file format.

Costs of B, stated honestly:

- **Colour-by-orbit changes meaning.** With `g` drawn per iteration, a hue offset by
  group index colours the walker by the *most recent* group element, not by "which
  copy this is" — a walker crosses between copies constantly. That's still a
  legitimate colouring (it reads as an interference pattern rather than 60 solid
  petals) but it is not the one you'd assume, and it is the only place where A and B
  genuinely differ in output.
- **Multiple groups need a per-transform group id**, so motifs under different
  symmetries can coexist. One `u32` in `GpuTransform`.
- **`trace.rs` and `randomize.rs` have to learn the group too**, since the CPU chaos
  game must mirror the GPU one — it already carries that obligation
  (`trace.rs:304-312`), this just adds to it.
- The asymmetric map of §3.5 is a transform with the identity group, which falls out
  for free.

### 3.3 What the scene format could look like

```toml
[[symmetry]]
group = "icosahedral"          # C<n> | D<n> | T | O | I, optionally + reflections
axis  = [0.0, 1.0, 0.0]        # for C/D; ignored for the polyhedral groups
applies_to = ["petal", "spike"]
color = "orbit"                # "shared" (all copies same hue) | "orbit" (hue rotates with the copy)
```

Two authored maps × I(60) = an effective map set of **120**, from six lines — while
the file, the panel, `--info` and the GPU all still see two transforms plus a group
(§3.2). Weights divide by `|G|` so each motif keeps the share it was written with.

Groups worth having, in order of value per line: `C_n` about an arbitrary axis
(the mandala case — `rose_window.toml` is a hand-built C3 already, and would become
three lines), `D_n` (adds the flip), then `T`/`O`/`I` for the polyhedral solids that
have no hand-authored equivalent in the repo at all. Reflection-extended variants
double each and are where the crystalline/glassy looks live, given
`absfold`/`boxfold` already fold on coordinate planes (CRAFT §4.1).

### 3.4 Progressions, for the shapes groups can't make

Groups give you closure. Growth needs a *progression* — the same motif at
`Sᵏ` for increasing `k`:

```toml
[[transform]]
name = "frond"
scale = 0.62
variations = { linear = 1.0 }

  [transform.repeat]
  count     = 21
  rotate    = [0.0, 137.5078, 0.0]   # golden angle
  scale     = 0.97
  translate = [0.0, 0.04, 0.0]
  weight    = 0.92                   # geometric falloff per instance
```

Instance `k` is `Sᵏ ∘ f`. Twenty-one transforms, six lines, and phyllotaxis,
helices, and log-spiral shells all fall straight out. `nautilus`, `koru`,
`helix` and `ammonite` are all reaching for this by hand.

### 3.5 Symmetry alone is boring — build in the defect

**Now written up as CRAFT §3.6**, since it's an authoring note rather than a feature
— including the mechanism I'd missed on the first pass: a group orbit flattens the
measure *by construction* (all `|G|` copies share a contraction and a weight), which
is CRAFT §2.1's unrecoverable-scene failure mode arrived at from a new direction.
`menger` turns out to be exactly this: a 20-map octahedral orbit with no `weight`
line in the file at all.

The feature consequences that stay here:

- The symmetry block needs an obvious **"add a map outside this group"** affordance
  sitting right next to it, not buried in the transform list. The defect is not an
  advanced option; it's the second thing you do.
- `--info` should raise a `notes` line when a scene is 100% symmetric — that's a
  one-line check (`every transform belongs to a group orbit`) against a known
  failure mode, which is exactly what the `notes` block is for.
- The dose is unmeasured. CRAFT §3.6 has the sweep to run.

### 3.6 Detecting symmetry that's already there — and what that does to `--factor`

The inverse operation, and it's cheap: compare each map's rotation part against
every other's, look for a common axis and angles that are multiples of `360/n`.
`rose_window.toml` (three maps at 0°/120°/−120° about Y) would be recognised
instantly.

**This is where Hazel's question lands, and the answer is no — but `--factor` is
demoted.** Making symmetry native doesn't forbid factoring; it removes the job
`--factor` was going to be most valuable for. If scenes are *born* with their group
in the file, the generator is never lost, so there is nothing to recover. The
"240 transforms come back into context as eight lines" argument evaporates, because
they were eight lines the whole time. That was the right call — solving the problem
beats providing a recovery tool for it.

What `--factor` is still genuinely for, once it's not the headline:

1. **An importer for the 42 scenes that already exist.** `rose_window` is a
   hand-built C3, `octahedron` and `menger` are polyhedral orbits. Nothing converts
   them to the new form except this.
2. **An importer for anything a tool emitted.** `tools/lsystem_to_ifs.py --depth N`
   produces flat composed words by construction, and `--random` has no idea groups
   exist. Both keep producing flat scenes forever.
3. **"Snap to symmetry", which is the interesting one.** `--mutations` drifts a
   symmetric scene off its group by a degree here and there. Detecting *near*
   symmetry — three maps at 0°/119.2°/−121°, say — and offering to snap them exact
   is a creative tool, not an import path. It's how you get from a lucky mutation
   back onto the interesting path, and it pairs directly with the 15° gizmo snap
   already in `app.rs:2545` ("IFS aesthetics live on clean rotational symmetry").

So: keep it, build it late, and stop calling it a context-window unlock.

---

## 4. Symmetry in the GUI: gizmos, the panel, and editing it after the fact

Design B (§3.2) buys the panel back: with the group live in the renderer,
`src/ui/transforms.rs` shows **2 motifs, not 120 rows**, and never goes near the
overflow-eats-clicks failure recorded in the camera-window memory note. The list
stops being the problem. The 3D view becomes the problem instead, which is the
better trade — a symmetry you can't see is a symmetry you can't edit.

### 4.1 What a symmetry gizmo actually is

Three things drawn into the scene, none of which exist today:

**The axis.** For `C_n`/`D_n`, a line through the attractor with `n` tick marks
around it — the fold count readable at a glance, and **draggable to re-aim the whole
group**. This is the gesture with no current equivalent and I think the most fun one
in the whole document: swinging a 5-fold axis through a scene and watching the form
reorganise around it. For the polyhedral groups there is no single axis, so draw the
group's namesake solid as a wireframe cage scaled to the attractor radius —
a tetrahedron, cube or icosahedron says "you are working in T/O/I" faster than any
label.

**Orbit ghosts.** The selected motif's gizmo drawn solid, its `|G|−1` images drawn
thin and dimmed. `gpu/gizmo.rs:414` already rewrites a matrix array every frame, so
the ghosts are extra instances of the same buffer with group elements applied —
no new machinery, just more matrices. At `|G| = 60` that is a lot of arrows, so:
ghosts for the **selected** motif only, behind a toggle, default on for `|G| ≤ 12`
and off above it.

**Live orbit under drag.** Free in Design B. The orbit is composed at walk time, so
dragging a motif's translate handle moves all 60 copies with zero extra plumbing —
the ghosts follow because they're the same matrix times the same group elements.

Two details worth getting right:

- **Ghosts must not be pickable**, or clicking a petal selects a copy you can't
  edit. Either exclude them from `pick.rs` entirely, or have a ghost hit resolve to
  its motif with a hint saying so ("`petal`, copy 7 of 60"). I'd start with the
  former and add the latter only if people click ghosts anyway.
- **The group supplies the snap increment.** `app.rs:2545` already snaps rotation to
  15° on Alt, with the comment that "IFS aesthetics live on clean rotational
  symmetry, and a fifteenth of a degree off exact is visible as a smear". With a
  group active the right increment is the group's own — 72° under C5 — so that
  constant becomes a lookup. Small change, exactly in the spirit of the existing one.

### 4.2 The panel, and editing symmetry after the fact

Hazel's requirement is that the symmetry stays editable, so the panel needs the
verbs, not just the display:

- **A symmetry is a header row above the motifs it governs**, with a live count
  badge: `C5 · Y axis · ×3 motifs = 15 maps`. Collapsed by default.
- **Changing the group is a picker on that row** — showing the point-group figure,
  not just a name. `C5 → I` is a two-click, 12× change in the rendered set.
- **Enrolling and withdrawing a map is drag-and-drop** between the symmetry block
  and the loose-transform list. This is the whole "editable after the fact"
  requirement in one gesture: a map that was symmetric stops being, or a map you
  just built joins the group and instantly has 59 siblings.
- **"Add a map outside this group"** as a button on the symmetry row, per §3.5 and
  CRAFT §3.6 — the defect is the second thing you do, so it should be the second
  button you see.
- **Sort by contribution** (§1.2) once scenes get big, so the maps that matter float
  up.

**History.** `history.rs` snapshots whole scenes, so none of this costs anything
structurally — but the undo *labels* have to carry the weight. "Edited transform" is
wrong for an edit that went from 15 maps to 180; the entry should read
`group C5 → I (15 → 180 maps)`. That's a labelling job, not an architecture job,
and it's the kind of thing that's much cheaper to do while building than after.

**Colour, still the open risk.** Sixty copies at one `color_value` is a monochrome
mandala, and CRAFT §2.3's trap is close by: with `color_falloff > 0` the effective
`color_speed` comes from contraction, and a post-affine rotation doesn't change the
determinant, so every orbit copy resolves to the *same* speed. Add to that §3.2's
note that colour-by-orbit under a per-iteration group draw means "most recent group
element" rather than "which copy this is". I don't think either is fatal, but this
is the same class of bug that cost six renders in §2.3, so it wants a deliberate
experiment rather than a guess.

---

## 5. The LLM-facing surface

CRAFT §8 rates `--info` as "an excellent agent interface" and it's right to. The
additions that matter most for an LLM author, roughly in order:

1. **`--stats` on the rendered image.** CRAFT §8.1's ask, unchanged: mean/max
   luminance, % clipped, % empty, and the **colour-index histogram**. The §2.3
   monochrome failure is a spike in that histogram and is invisible in the picture.
   Still the most agent-shaped thing missing.

2. **A `symmetry` block in `--info`.** Requested, and it's the piece that keeps a
   180-map scene legible in text. It has to summarise, not enumerate — the whole
   point is that the orbit never appears in the report:

   ```
   symmetry   C5 about [0.000 1.000 0.000]      3 motifs -> 15 maps
              defect    spine, 1 map outside the group, 4.2% of the walk
              near      petal-b sits 1.8 deg off exact
                        -S transform.petal-b.rotation=[0.0,72.0,5.0]
   ```

   Three jobs in six lines: say what the group is, confirm the §3.5 defect exists
   (or raise a `notes` line when it doesn't), and catch drift with a paste-ready fix
   — which is the `shape` block's own idiom, "emit the flag rather than the number"
   (CRAFT §8).

3. **`d`, Λ, and resolved `color_speed` in `--info`.** CRAFT §8.2's ask. `--info`
   knows every contraction and doesn't compute the one number CRAFT calls the best
   predictor of the look.

4. **`--diagnose`: the `notes` block grown up.** Not "occupancy is 0.61" but a
   named failure mode with a paste-ready fix:

   ```
   fuzz    rotations do not close (est. group order >400 at word length 4)
           and Λ is flat past 3 levels — the measure is smooth.
           try: snap the rotations to a 5-fold Y axis
                -S transform.b.rotation=[0,72,5] -S transform.c.rotation=[0,144,5]
   ```

   This is the difference between a tool that reports and a tool that teaches. The
   existing `notes` block is already the right shape; this is just more of it,
   pointed at the two walls.

5. **Native symmetry *is* the LLM unlock — `--factor` isn't.** Worth stating
   plainly, because the first draft had this backwards. LLMs cannot reliably emit
   sixty consistent rotation matrices; the arithmetic is exactly the thing they're
   worst at and it costs thousands of tokens to get wrong. But every LLM knows what
   "icosahedral" means. A format where the model writes `group = "I"` and the
   *loader* does the trigonometry moves the hard part to where it belongs, and a
   180-map scene costs eight lines of context in both directions — writing it and
   reading it back. `--factor` was going to recover that legibility for scenes that
   had lost it; native symmetry means they never lose it. See §3.6 for what
   `--factor` is still worth building for.

6. **Guard rails as a shared vocabulary.** `randomize.rs`'s gate is a private set
   of constants. Exposing the same thresholds as a named **taste profile** —
   `--taste lace|shell|solid` — that governs `--random`, the mutation grid, the
   slider corridors and `--diagnose` alike means human and model are steering by
   the same numbers, and a scene can record which profile it was built under.

---

## 6. If I were picking, in order

Ranked by expressive power per line of code, CRAFT §6 style. Items 1–5 are one
coherent feature and want building in that order; 6 onward are independent.

1. **Group-aware post-composition in the walk.** One `Mat4` applied after
   `apply_variations`, drawn from a group-element buffer. `|G| = 1` is the fixed
   post-affine slot CRAFT §6.1 asks for; `|G| = 60` is icosahedral symmetry. Same
   code, and the CPU mirror in `trace.rs` has to learn it at the same time.
2. **`[[symmetry]]` in the scene format, staying live.** The large-scene lever, and
   the thing that keeps 180 maps down to eight lines in a file and in an LLM's
   context.
3. **The `symmetry` block in `--info`.** Cheap once (2) exists, and it's what makes
   the feature usable from the CLI and by an agent at all — including the
   "no defect" `notes` line that catches CRAFT §3.6's failure mode.
4. **Symmetry gizmos: axis handle, polyhedral cage, orbit ghosts.** A symmetry you
   can't see is one you can't edit. The draggable axis is the gesture worth building
   the rest for.
5. **Panel verbs: group picker, drag-to-enroll, add-a-defect, honest undo labels.**
   "Editable after the fact" in concrete gestures rather than in principle.
6. **`d` in the status bar + corridor bands on sliders + dimension lock.** The
   corridor made tangible; small, self-contained, and it changes what editing
   *feels* like rather than adding a panel. Independent of all the above.
7. **Rotation coherence + lacunarity, with the calibration run in §1.1 done first
   and written into the discovery log.** Do the measurement before the feature; if
   the curve doesn't separate good from bad on the repo's own scenes, kill it and
   record that.
8. **`repeat` progressions.** Cheap once (1) exists, and immediately useful to four
   existing scenes.
9. **`--stats` and `--diagnose`.** Highest value for LLM authors specifically.
10. **`--factor`, as importer and snap-to-symmetry** (§3.6) — not as the context
    unlock it was billed as in the first draft.
11. **Crossover over motifs and groups.** The end of the genetic loop, and it only
    becomes well-defined once scenes have a genome.

---

## 7. Things I think are traps

- **Don't gate on the new numbers before calibrating them.** A quality gate tuned
  on a hypothesis will silently delete the good accidents, and CRAFT §5 is explicit
  that keeping the accidents is where `blossom.toml` came from. Diagnose loudly,
  refuse rarely.
- **Don't let the corridor become a fence.** Every band should be a suggestion with
  a visible edge, never a clamp. The whole method is breed-and-select; selection is
  the human's job.
- **Don't build the symmetry generator as a Python script.** CRAFT §6.4 notes one
  already exists in the experiment log and that it's "one click in Apophysis and a
  Python script here". A script produces flat TOML that can never be edited back
  into a generator — it makes the large-scene problem worse, not better, and it is
  the exact thing Design B (§3.2) exists to avoid.
- **Watch the colour EMA.** Every feature here multiplies the effective map count,
  and CRAFT §2.3's expanding-map trap says the colour system has sharp edges around
  contraction. Sixty copies sharing one contraction is a case nobody has rendered
  yet — see §4.2.
