# Xaos — plan

*Status: plan, 2026-09-26. Not started. Built from `XAOS-AND-BEYOND-BRAINSTORM.md`
§1–2 (cited as `BS §n`) and Hazel's brief: a graph view overlaid on the
viewport, an N×N matrix view for larger scenes, per-transform editing in the
Transforms window, agent CLI, and TOML that names maps and works in rows or a
whole matrix. Opt-in: a scene that says nothing about xaos behaves exactly as
today.*

*Decisions marked **Hazel's call** are listed together in §9. Everything else
is a recommendation with its reasoning. GUI wording in here is placeholder;
it is Hazel's to set.*

---

## 1. The model

Each map `i` has an outgoing row of multipliers `x[i][j]`: after `i`, how
likely `j` is next, relative to its weight. Following the flam3 convention
(BS §1.1):

```
P(j | i) = w_j · x_ij / Σ_k w_k · x_ik        (w = effective weight; 0 if disabled)
```

The scene stops being an IFS and becomes a **graph-directed IFS**. Four facts
about that drive most of what follows:

1. **Only zeros change the shape.** The attractor, as a set, is determined by
   which map *sequences* are allowed, which is the zero pattern of `x`. A
   positive multiplier, like a weight, only moves *where the points go*. So
   cutting an edge is a structural edit and scaling one is a density edit. The
   UI should draw the two differently, and `--info` should say which one a
   change was. This is the most useful sentence to teach a user or an agent.
2. **Weights stop meaning "how often".** How often a map fires is the chain's
   *stationary distribution* (the "walk share"), not its weight share. A
   weight-1 map can be starved to nothing (BS §1.4). The GUI and `--info` show
   walk share next to weight wherever xaos is on. Without that, the weight bar
   gives the wrong answer.
3. **Dimension changes formula.** Σ sᵢᵈ = 1 is wrong for a graph-directed IFS.
   The Mauldin–Williams dimension is the `d` at which the spectral radius of
   `M_ij = [x_ij > 0] · s_jᵈ` is 1. It depends only on the zero pattern, which
   is fact 1 again. `--info` switches formulas when xaos is present.
4. **Disconnected parts are layers.** A block-diagonal `x` is two attractors
   sharing no walker path, which is what subscenes already are. §8 is how the
   two features become one. Phase 1 leaves layers alone and keeps xaos inside
   each layer.

**Defaults and edge cases:**

- **No row (the default) means every entry is 1**, which is today's behaviour,
  byte for byte (§4 proves it).
- **The first step, and the step after a re-seed**, have no previous map, so
  they draw from the plain weights.
- **A dead end** is a row whose mass is zero: every exit cut, or every
  remaining target disabled. It restarts the chain from the plain weights.
  Numerically that is the same as a uniform row, but it is reported as a dead
  end (§9, decision 1).
- **Symmetry is untouched.** Xaos is over motifs; the group element is still
  drawn independently and uniformly after the map. If the group draw ever
  depended on history, exact symmetry would be lost (BS §1.3).
- **`repeat`** works the same way.
- **Infinite zoom needs `f_z(S) ⊆ S`.** That holds only if every map that
  fires can be followed by the zoom map, meaning column `z` has no zero among
  maps with positive walk share. A scene that breaks this is a load error
  naming the edge, just as zoom plus subscenes is today. The error is deferred
  rather than worked around.

## 2. Scene format

### 2.1 Per-transform rows (canonical)

```toml
[[transform]]
name = "petal"
xaos = { petal = 0, spike = 2.5 }     # unlisted targets keep 1

[[transform]]
name = "spike"
xaos = { "*" = 0, petal = 1 }         # only petal may follow spike
```

- **Keys are target names**, resolved within the map's own layer.
  - An index written as a string (`"3"`) also works, as `[zoom] map` and
    `applies_to` already do.
  - Names are not unique today, so a key that matches two maps is a load
    error naming both.
  - `*` becomes illegal as a transform name.
- **`"*"` is the rest of the row**: the multiplier for every unlisted target,
  1 when absent.
  - `{ "*" = 0, … }` is the "only these" form. It is what makes cycles and
    splices one line each on a 40k-map scene.
  - The alternative is a sibling key, `xaos_rest = 0` (§9, decision 2).
- **Values:** finite and ≥ 0. A target in another layer is a load error in
  phase 1 ("coupling between subscenes isn't supported yet").
- **Save** writes names where they're unique and indices where they aren't,
  and omits `xaos` entirely for an all-default row. Nothing new is written for
  scenes that don't use the feature.
- **Subscenes:** `[[subscene.transform]]` rows work the same way, and a
  borrowed `file =` subscene carries its file's xaos with it.

### 2.2 The whole matrix (sugar, for small scenes)

```toml
[xaos]
maps = ["petal", "spike", "stem"]   # row and column order: every map, once
matrix = [
#   petal spike stem       ← to
  [ 0,    2.5,  1 ],     # from petal
  [ 1,    0,    0 ],     # from spike
  [ 1,    1,    1 ],     # from stem
]
```

The matrix is the most readable form for up to about a dozen maps, and what
an agent writes when it has composed a whole graph at once.

- **It resolves onto the per-transform rows at load and is regrouped at
  save.** That is the `[[symmetry]]` precedent: sugar over a per-transform
  property.
- **Strict.** `maps` must list every map in the layer exactly once, and rows
  must be complete. Errors name the missing maps.
- **Mutually exclusive** with any per-transform `xaos =` in the same layer,
  and both present is a load error. Two spellings of one edge would be two
  sources of truth.
- **Save preserves the form the file used.** A GUI-authored graph on a scene
  that had neither form gets per-transform rows, which scale. Inline
  subscenes take `[subscene.xaos]`.
- **Considered and rejected:** an `[[xaos]]` edge list
  (`from`/`to`/`weight` tables). It would be a third spelling, it scatters one
  row across many blocks, and `-S` already covers the single-edge edit an
  edge list would be for.

### 2.3 Runtime representation

`TransformSpec` gains `xaos: Xaos { rest: f32, exceptions: Vec<(u32, f32)> }`,
with the default rest 1 and no exceptions.

- **Targets are indices at runtime.** Every structural edit remaps them: delete
  drops entries pointing at the deleted map and shifts the rest; add appends
  nothing. Today delete and duplicate go through
  `App::delete_selected_transform` / `add_transform` and the scene's parallel
  vecs, so phase 1 first moves those into one `Scene::remove_transform` /
  `Scene::insert_transform`, where the remap can live and be tested.
- **Duplicate** copies the outgoing row, and also copies the incoming column
  onto the new map, so a duplicate behaves as "a second copy of this map"
  (§9, decision 5).
- **Undo is free**, because `EditSnapshot` clones the whole scene.

## 3. CLI, for agents

- **`-S transform.<from>.xaos.<to>=0`** sets one edge. Other forms:
  - `…xaos.*=0` sets the rest (quote it for the shell).
  - `…xaos={}` resets a row.
  - In a matrix-form scene, `set.rs` rewrites the matrix cell, so an agent
    uses one spelling whichever form the file is in.
  - `struct_fields!` gains the field. Unknown map names are errors that list
    the layer's maps, as today.
  - A no-op, such as setting an edge to the value it already has through
    `*`, is refused.
- **`-S transform.<from>.xaos_p.<to>=0.4`** sets a *probability* rather
  than a multiplier. Hazel's brief asks for "directly set probabilities",
  and an agent reasons in probabilities.
  - It solves, exactly and for that one edge, for the multiplier that makes
    `P(to | from) = 0.4` with the rest of the row held:
    `x = p·Σ_{k≠to} w_k x_k / ((1−p)·w_to)`.
  - The file still stores the multiplier, so there is one source of truth,
    and `--info` echoes the resulting probability.
  - `p = 1` is "only this exit" and writes `{ "*" = 0, to = 1 }`.
  - The GUI's probability readouts are editable on the same terms.
- **`--sweep transform.a.xaos.b=1:0`** morphs between sub-attractors. The
  brainstorm (BS §1.9) says this is far better behaved than sweeping affine
  coefficients, and it comes free.
- **`--xaos <preset>`** applies a preset before everything else, like
  `--palette`, so it restyles without editing the file.
  - It prints the `xaos =` lines it implies, as `--random-palette` prints its
    table, so a keeper can be pasted.
  - Presets: `no-self-loops`, `cycle` (rail order), `reset`, and `blocks:N`
    (N groups in rail order, cut from each other). The last is the one that
    previews §8.
- **`--info`** gets an `xaos` section (the key fits the 8-character limit),
  present only when some row is authored, so goldens don't move. Fixed schema
  inside:
  - The form (rows or matrix), how many rows are authored, cut-edge count,
    and dead-end count.
  - Per map: weight share, **walk share**, and edges in/out
    (`in 3/5  out 2/5`). The gap between weight share and walk share is the
    "why is this map invisible" signal (BS §1.6).
  - A compact multiplier grid for up to 12 maps in a layer: `·` for 1, `0`
    for a cut, the value otherwise. It fits in 78 columns, and an agent reads
    the whole graph in twelve lines.
  - `shape`'s dimension row switches to Mauldin–Williams (§1, fact 3).
  - **Notes, each with its fix as a flag:**
    - dead ends;
    - maps drained by an absorbing class, with the `-S` that reconnects them:
      "after burn-in only {a, b} fire; c, d are drained";
    - maps unreachable after the first step;
    - walk share under 2%;
    - slow mixing, flagged when power iteration is slow to converge;
    - the zoom-column conflict.

## 4. Engine — GPU and the CPU mirror

**Gate everything behind `ComputeParams.xaos != 0`,** which is zero for every
scene without an authored row. Plan on proving byte-identity against a
baseline binary exactly as subscenes did (`ADDING-A-SCENE-FEATURE.md` §4).

**Budget.** Everything fits in space that is already padding:

- `WalkerState._pad_tangent` becomes `last: u32` (`0xFFFFFFFF` = none, set on
  seed and re-seed).
- `GpuTransform._pad: [f32; 2]` becomes `xaos_start, xaos_len`, so the struct
  stays 256 bytes.
- One new read-only storage buffer holds the rows. That is the 8th storage
  binding, exactly the WebGPU default limit:
  - Also request the adapter's `max_storage_buffers_per_shader_stage`, the way
    `max_storage_buffer_binding_size` is requested now.
  - Bind a one-entry dummy buffer when no row is authored, because zero-size
    buffers can't be bound.

**The row format has to scale to 40k maps, so it never stores N² entries.** A
row is its rest multiplier `c` plus its **exceptions**, the targets whose
multiplier isn't `c`. Sampling is exact and costs two binary searches:

```
total = c·(W − E) + X     W: layer's base mass, E: exceptions' base mass,
                          X: Σ w_j·x_ij over exceptions
r ∈ [0, total)
r < X  → binary search the exceptions' own CDF (cuts have zero mass there)
else   → s = (r − X) / c, a position in the base CDF with the exceptions'
         intervals removed; step it back into base coordinates past the
         excluded intervals (a binary search over precomputed gap positions
         and prefix shifts), then the existing select_transform
```

- **Cost is proportional to the exceptions only.** "No self-loops" on 40k
  maps is 40k one-entry rows. "Cycle" is 40k `c = 0` rows of one entry each.
  A default row costs nothing and takes the existing path.
- **Rows are rebuilt on the CPU** wherever `update_weights` runs today, by one
  builder, `XaosTable::build`, next to `LayerTable::build`. It is called from
  the live app and from every `PointCompute::new` in `offline.rs`.

**The CPU walkers learn it at the same time.** `trace.rs` feeds traces,
`trace::measure` (the drawn-point budget and `--info` `shape`),
`measure_layers`, lacunarity, and `randomize.rs`'s gate. Its `Walker` gains
`last`, and it samples from the **same built table** through a Rust
transcription of the WGSL arithmetic. One builder with two consumers means the
CPU and GPU can't disagree about the distribution.

**Proof:**

1. **The regression test.** Every ordinary scene renders byte-identically to
   the baseline binary, in both renderers, and `--info` is unchanged.
2. **The new-path test.** Arrange the arithmetic so that a scene whose every
   row is written `{ "*" = 1 }` (so the xaos path runs with no exceptions)
   renders **byte-identically to the same scene without it.** This works
   because `total = W` and `s = r` exactly. It exercises the whole new path on
   real scenes with a zero-tolerance comparison.
3. **Unit tests of the sampler.** Exhaustive small cases: cuts at the ends of
   the range, adjacent cuts, `c = 0`, `c ≠ 1`, and zero-weight targets. The
   empirical `P(j|i)` over 10⁶ CPU steps must match the formula.
4. **Statistical equivalence with subscenes.** `blocks:2` on a two-group
   scene should match the corresponding two-subscene composite. It is not
   byte-identical, because walkers are dealt by a random first draw rather
   than exactly, but it is the first evidence for §8.
5. **Frame time.** Measure an xaos preset on a heavy scene against 120 FPS.

**Analysis lives in `src/xaos.rs`,** used by `--info`, the GUI and the
randomizer:

- **Walk share** by sparse power iteration, with complement rows applied
  analytically, so it costs O(N + exceptions) per step and never builds a
  matrix.
- **Support-graph analysis:** reachability, closed classes and dead ends,
  using the complement-graph BFS trick (expanding a `c > 0` row visits
  "everything unvisited except its cuts"). It is also O(N + exceptions).
  Tarjan over an implicit complete graph would be N², which at 40k maps is
  1.6 × 10⁹.
- **Mauldin–Williams dimension** by bisection on `d` over the same sparse
  operator.

## 5. GUI

Everything below funnels through a small set of `App` methods, so the three
views are the same edit:

- `set_xaos_edge(from, to, x)`, `set_xaos_rest(from, c)`, `reset_xaos_row`;
- `apply_xaos_preset`;
- `insert_linked_transform(after)` (§5.4).

Edge drags coalesce under a key like `xaos:T3>T7`. Walk share is recomputed
on edit and cached on `App`, never per frame, and debounced during a drag.

**It is opt-in by disclosure, not by a mode.** Until a row is authored, the
xaos block in the inspector is one closed line and the overlay is off. Once a
row exists, the block opens by default, following the rule that a disclosure
hides elaboration and never the headline. Nothing is hidden in a way that
stops it saying it exists.

### 5.1 Transforms window — per transform

- **Rail: a walk-share mark on the weight bar.** A tick on the same track
  shows where the chain actually puts the map, beside what the weight asks
  for. Two numbers on one axis are what make the gap readable. The tick is
  drawn only when some row is authored, because otherwise it equals the
  weight and would be clutter.
- **Inspector: a Xaos block** under Behaviour.
  - A summary line: "any map may follow" / "3 exits cut" / "only → stem".
  - A to/from radio ("goes to" / "comes from"), the Apophysis 7X idiom. The
    "from" view edits other maps' rows, and it is the natural place for "only
    reachable from j".
  - One row per target: swatch, name, multiplier (`num::drag`), resulting
    probability readout (fixed width), and a cut/restore toggle. A cut row is
    drawn struck through; a scaled row keeps its normal look, reflecting §1
    fact 1.
  - A "rest" row for `c`.
  - **Past `FILTER_THRESHOLD` (12) targets**, the block lists only the
    exceptions plus a filterable "add a target…" picker. The list is
    virtualized at a fixed row pitch, like the rail and for the same reason.
    It never lays out 40k rows.
  - Actions: **Reset**, **No self-loop**, **Only to…**, **Insert linked
    map**.

### 5.2 Viewport overlay — the graph

This is the Gizmos button's sibling (a toggle beside it). Edges are painted
through egui in screen space between projected origin dots, as the roll ring
is, and picked by the same projection that draws them.

**What is drawn has to stay bounded at any N, and the key is that the default
graph is complete:**

- **Selected map:** its outgoing arrows to every positive target, width by
  `P(j|i)`, plus cut targets as short dashed stubs with a ×. Incoming edges
  appear dimmer, and only in the "from" view.
- **Nothing selected:** only authored deviations (cuts and non-1
  multipliers). An untouched scene draws nothing.
- **Hard cap** of a few hundred edges, with "+N not drawn" in the status bar.
  Hazel's ">12" rule applies here: past 12 maps in a layer, the
  nothing-selected view is off, and the matrix window is where the whole graph
  lives.
- **Shape:** a self-loop is a small loop at the dot. `i→j` and `j→i` bow
  opposite ways so both read.

Interactions reuse idioms that already exist here:

| Input | Action |
|---|---|
| click an edge | select it: a small popover at its midpoint with multiplier, probability, cut/restore |
| alt+scroll over an edge | scale its multiplier (the weight lever, one level down) |
| right-click an edge | cut / restore to 1 / make it the only exit / insert a linked map on it |
| double-click a cut stub | restore it |
| ctrl+right-drag across edges | **knife**: cut every edge the stroke crosses (Blender's node-editor gesture; check it's free — right-drag is roll) |
| Esc | clears an edge selection before the transform selection |

Node size stays the gizmo's own. When xaos is on, the name label gains the
walk share.

### 5.3 Matrix window — the whole graph, any N

This is a new window, `WindowKey::Xaos`, with a toolbar toggle beside
Subscenes, per layer (the same picker the Transforms rail has).

- **A heatmap.** Rows are "from" and columns are "to".
  - The multiplier is coloured on a log scale: 1 is neutral, below 1 cool,
    above 1 warm, and a cut is an empty cell with a ×, a different glyph
    rather than a darker colour, per fact 1.
  - A radio switches between showing the multiplier and `P(j|i)`.
  - The diagonal is outlined, since self-loops are the commonest edit.
- **Headers** are map swatches and names. Clicking one selects that map, in
  both directions with the rail. Header context menus hold the row and column
  operations: reset, cut all but…, no self-loop, and only reachable from….
- **A walk-share strip under the columns**, bars per target, is the matrix's
  answer to "what fires". Drained and unreachable columns are tinted, and a
  hover says why.
- **Cells:**
  - click toggles cut/1;
  - drag vertically scales;
  - alt+scroll scales;
  - shift+drag across cells paints the current brush (cut or restore) as one
    undo entry.
  - Hovering a cell sets the status hint: "petal → spike ×2.5, 41% of petal's
    exits".
- **Scale:** virtualized on both axes with sticky headers, using a fixed cell
  pitch for the same reason the rail has `TAB_HEIGHT`. Past a few thousand
  maps in a layer, it switches to a **sparse view**: only rows and columns
  with an exception, plus the selection and a filter. The full matrix of a
  40k-map layer is not a thing anyone looks at.
- **Preset buttons**: the `--xaos` presets.

### 5.4 Ideas worth taking from elsewhere

- **Apophysis 7X's "linked transform"** is the standout. It inserts a new map
  that fires right after `a` and only there:
  - `a`'s row becomes `{ "*" = 0, new = 1 }`;
  - `new` inherits `a`'s old row;
  - every other row cuts `new`.
  - It is how Apophysis users built chained and post-like effects before
    final transforms existed. Here it is one action on the inspector, the
    edge menu (splice onto `a→b`), and the matrix header.
- **The to/from toggle** from the same editor, adopted in §5.1.
- **Blender's knife** for cutting links by stroke (§5.2). It is the fastest
  way to make a structural edit to several edges.
- **Walk share beside weight** doesn't come from another editor, but it is
  the thing no editor I know of shows and every xaos user needs. It is §5.1's
  rail mark.
- **Later, "hold shares":** while it is on, xaos edits re-solve the weights
  so each map's walk share stays put. That makes xaos a pure "where" control
  and weight a pure "how often" control, the split BS §1.6 wants for dosing a
  symmetry defect. It is not always solvable (a cycle forces equal shares
  whatever the weights), so the toggle has to say when it can't hold them.
  Deferred, but the analysis in `src/xaos.rs` is most of it.

## 6. Random flames and mutation (after the editor)

This follows BS §1.8:

- **Generator knobs:** sparsity, no-self-loop chance, and preset.
- **Strong connectivity by construction:** seed a random Hamiltonian cycle,
  then cut other edges.
- **Reject** any candidate where a map's walk share is under a few percent,
  in the same gate that already runs `trace::measure`.
- **Mutation operators:** flip an edge, nudge a multiplier, apply a preset.
- The broader "Apophysis-style generator controls" want lands in this phase
  too.

## 7. Phases

| # | Scope | Done when |
|---|---|---|
| 1 | Model, TOML (rows and matrix), save, remap on delete/duplicate, GPU path, CPU mirror, zoom-column check | byte-identity (both forms of it, §4); sampler tests; round-trip tests incl. matrix form and remap |
| 2 | `-S` / `--sweep` paths, `--xaos` presets, `--info` `xaos` section, Mauldin–Williams, `src/xaos.rs` analysis | goldens unchanged; notes fire on constructed dead-end / absorbing / starved scenes |
| 3 | Transforms window: Xaos block, rail walk-share mark, linked-map action | Xephyr: cut, scale, reset, undo, linked map on a small scene; a 40k-map scene opens the block without a frame hitch |
| 4 | Viewport graph overlay, edge selection, popover, knife | Xephyr: click-select, alt+scroll, right-click menu, knife |
| 5 | Matrix window | Xephyr at 5, 40 and 2000 maps; sparse view past the threshold |
| 6 | Random/mutation knobs and gate | no roll with a starved or drained map, over a seed sweep |
| 7 | Coupling between subscenes (§8) | only if wanted after 1–6 |

A demo scene lands with phase 2. It is a symmetric scene where one cut edge
moves a defect map to the petal tips only (BS §1.6), tuned with the `xaos`
section and `--sweep` over an edge.

## 8. Subscenes, later

The earlier conclusion, now with a plan behind it: **xaos becomes the engine,
and a subscene becomes a named group of maps whose default xaos is "within
the group only".** Phase 7 is the step that makes cross-group edges possible.
What it needs:

- **One coordinate frame.** A walker that crosses from subscene A's maps to
  B's must be in scene space, so placement moves from "applied at point write"
  to "conjugated into the maps" (`P·M·P⁻¹`, which `Scene::world_transforms`
  already computes for the gizmos) when the GPU table is built. The file and
  the inspector keep maps in their subscene's own space, as now.
- **Walkers are dealt per closed class, not per layer.** The shares of
  subscenes coupled into one class sum. A class with no exit gets its own
  walkers; transient maps get none, and are fed by their class.
- **Colour** stays per group, resolved at write by the current map's group,
  as now.
- **The layer table** survives as the walker partition, computed from the
  graph rather than declared. The Subscenes window, import, provenance and
  the whole-subscene gizmo are unchanged. They were always about groups.

None of this blocks phases 1–6. Phase 1's only obligation is to keep
cross-layer references a named load error, rather than something that
silently loads.

## 9. Decisions — Hazel's call

1. **Dead ends restart from the plain weights** (recommended), versus
   re-seeding the walker's position (which throws burn-in points into the
   picture) or refusing to load (which random generation would hit
   constantly).
2. **`"*"` for the rest of a row** (recommended: one table holds the whole
   row), versus a sibling `xaos_rest =` key (safer from shell globbing in
   `-S`).
3. **Include the matrix form**, strict and exclusive, with save preserving the
   form the file used (recommended), versus rows only.
4. **Presets are operations** that write rows (recommended: a pasted row is
   just a row), versus live rules like `no_self_loops = true` that follow the
   scene as maps are added. Live rules are the symmetry precedent ("the group
   stays live"), and "no self-loops" is the one preset where that genuinely
   matters. Could be revisited for that one alone.
5. **Duplicate copies the incoming column too** (recommended), versus
   outgoing only (the Apophysis behaviour, I believe, but unverified).
6. **The knife gesture binding**: ctrl+right-drag, if nothing claims it.
7. **§8**: whether to go there at all, and when.
8. All GUI wording.
