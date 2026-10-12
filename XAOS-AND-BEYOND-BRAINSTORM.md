# Xaos, rendering, and authoring — a brainstorm

*Status: brainstorm, not a plan. Written 2026-09-26 from a chat session between
Hazel and Claude (Opus 5.5, claude.ai). Nothing here is measured. Items marked
**Hazel's call** are decisions; everything else is a proposal. Cross-references
to `AUTHORING-BRAINSTORM.md` are written as `AB §n`.*

**If you are picking this up to implement:** xaos is the next feature (**Hazel's
call**). It's the natural direction for a lot of reasons, and it directly
addresses the current authoring problem: it's hard to generate good structure.
The rendering and authoring sections are a menu for later, not a queue.

Two standing constraints from Hazel:

- **Realtime 120 FPS is one of the best things about this program.** Anything
  that threatens the interactive framerate belongs in a final-render mode, not
  the editing view.
- **Dense voxel grids are out** on memory grounds. A froxel renderer with point
  reprojection between frames was already attempted (by Opus 4.7/4.8) and did
  not work. Don't propose it again without a specific reason it'll go
  differently this time.

---

## 1. Xaos

### 1.1 The model

Each transform `i` gets a row of weights `xaos[i][j]`: "after applying `i`, how
likely is `j` next." Following the flam3/Apophysis convention, xaos *multiplies*
base weight, so:

```
P(j | i) = w_j · x_ij / Σ_k (w_k · x_ik)
```

All-ones reproduces current behaviour exactly — that's the regression test.
`.flam3` compatibility isn't a priority, but matching the convention costs
nothing and keeps a future importer trivial (flam3 stores each xform's
outgoing row as `chaos="1 0 0.5 ..."`).

Conceptually this turns the scene from an IFS into a **graph-directed IFS**:
the attractor is shaped by which transform *sequences* are allowed, not just by
the transforms themselves.

### 1.2 Scene format

Suggest a **name-keyed, sparse** row per transform, defaulting to 1:

```toml
[[transform]]
name = "petal"
xaos = { petal = 0.0, spike = 2.0 }   # unlisted targets = 1.0
```

Why names rather than a positional array: it survives reordering, adding and
deleting transforms; it's legible to agents and humans; and the common edit
("cut this one edge") is one key rather than a whole row.

### 1.3 Implementation

- Chaos shader already selects by binary search over a cumulative-weight array
  in a storage buffer. With xaos this becomes **one CDF row per transform**
  (an N×N buffer) indexed by the walker's previous transform. Each walker
  carries `last: u32` as state.
- Rebuild the CDF rows on CPU whenever weights or xaos change.
- **With symmetry Design B (AB §3.2), xaos is over motifs, not orbit copies.**
  Two motifs under `I` is a 2×2 matrix, not 120×120. The group element is still
  drawn uniformly and independently after the motif is chosen — keep those
  separate. If the group draw ever depends on the previous group element, the
  exact-symmetry guarantee of AB §3.1 no longer holds in general.
- **Final transform** (if/when it lands) stays outside the chain, as in flam3.
- **`trace.rs` and `randomize.rs` must learn xaos at the same time**, because
  the CPU chaos game mirrors the GPU one and the randomizer's acceptance gate
  runs on it.
- Adding/removing transforms: new edges default to 1.

### 1.4 Gotchas

- **All-zero rows.** A walker whose outgoing row is all zero is stuck. Decide
  explicitly: treat as uniform, or respawn the walker. Random generation will
  hit this constantly.
- **Absorbing subsets.** If {2, 3} can reach each other but never leave, every
  walker eventually falls in and the rest of the fractal disappears after
  burn-in. Check strong connectivity of the nonzero-edge graph (at least in
  `--random` and mutation).
- **Invisible transforms.** Density contribution follows the chain's
  *stationary distribution*, not base weights. A weight-1 transform can end up
  nearly invisible. Compute it with ~50 steps of power iteration.
- **Burn-in.** Slow-mixing (near-reducible) chains need longer burn-in or you
  get ghost points from the starting distribution.
- **Colour.** Colour speed interacts with how often a transform actually fires.
  Same class of risk as the colour EMA trap flagged in AB §7 — render a few
  deliberately before trusting it.

### 1.5 Patterns worth having as one-click presets

- **No self-loops** (`x_ii = 0`). The classic trick: a transform can't apply
  twice in a row. One toggle, dramatic effect on spirals and dense self-copies.
- **Cycle** (`i → i+1` only). Forces ordered sequences; often architectural,
  non-organic results.
- **Blocks.** Two sub-IFSes with weak cross-links: two interleaved fractals
  inheriting structure from each other. (See §2 — this is also composition.)

### 1.6 What xaos does for the existing plans

- **Dosing the symmetry defect (AB §3.5).** Weight controls *how often* the
  defect map fires; xaos controls *where*. If the defect can only follow one
  specific motif, its effect lands in one structural place (petal tips only,
  say) instead of smearing across every orbit copy. The "dose is unmeasured"
  question gets a second, more authorable dial.
- **Rotation coherence (AB §1.1a).** The effective-group-order diagnostic
  generates all words up to length 4. With xaos, only words that follow allowed
  edges occur, so the generator should walk the xaos graph instead of taking
  all Nᵏ products. Consequence: **xaos can make a fuzzy scene crystalline
  without touching a single rotation**, by pruning the reachable word set. A
  new anti-fuzz lever, and the diagnostic would show it working.
- **`--info` maps block.** With xaos, weight share, walk share (stationary
  distribution) and shape contribution (AB §1.2) all diverge. Report all three;
  the gap between weight and walk share is exactly the "why is this transform
  invisible" signal.
- **Symmetry block in `--info` (AB §5.2)** should mention when xaos breaks
  per-motif uniformity.

### 1.7 UI

- **Heatmap grid editor** — the practical one.
- **Node graph** — the fun one. Transforms as nodes, edge thickness = xaos
  weight, node size = stationary share. Makes structure legible at a glance.
- **Stationary share shown live** next to each transform while editing.
- **Agent verbs** in the CLI: cut edge `i→j`, "only reachable from `j`",
  remove all self-loops, apply a pattern preset.

### 1.8 Random generation and mutation

Knobs: xaos sparsity (probability an edge is zero), no-self-loop chance,
pattern preset (random / cycle / blocks). Guarantee strong connectivity —
either rejection-sample, or seed a random Hamiltonian cycle before zeroing
other edges. Reject scenes where any transform's stationary share is below a
few percent. Mutation operators: flip an edge on/off, nudge a weight, swap a
preset.

This is also where Hazel's earlier want lands: Apophysis-style control over
random-scene generation (min/max transform counts, allowed variations, etc.)
belongs alongside these knobs.

### 1.9 Animation

Interpolating xaos weights morphs smoothly between sub-attractors, and is far
better-behaved than interpolating affine coefficients. Ramping a single edge
from 1 to 0 over a camera path looks lovely.

---

## 2. Multi-scene composition is block-structured xaos

Another Opus is working on multi-scene composition. The observation worth
handing over: **composing two scenes is an xaos matrix with two diagonal
blocks.** Off-diagonal entries are the coupling between scenes.

- Zero coupling: two independent attractors side by side.
- Small coupling: each scene starts inheriting structure from the other —
  where the interesting hybrids live.
- Catch: at exactly zero coupling a single walker is trapped in whichever block
  it starts in. Separate walker populations per scene is the correct
  zero-coupling behaviour, so if composition is being built that way, it's
  right.

The design ask: **leave room in the data model for a scene-to-scene coupling
matrix**, so composition and xaos become the same feature at two zoom levels
rather than two features that fight each other.

---

## 3. Rendering

The core tension: **splats** give accumulation (translucency, density) but no
depth ordering; **points** give depth ordering but no accumulation. Most good
options bolt the missing half onto one of them.

### 3.1 Cheap — interactive-view candidates

- **Eye-dome lighting (EDL)** on the points renderer. Screen-space post-pass
  that darkens pixels by how much neighbours' log-depths exceed their own. Used
  by Potree/CloudCompare for exactly this problem. ~30 lines of shader, no
  normals needed. First thing to try.
- **Weighted blended order-independent transparency** (McGuire & Bavoil) on the
  splat renderer. Depth-weighted accumulation into two buffers, one resolve
  pass, no sorting. Not physically correct, but gives front/back sense while
  keeping the glow.
- **Screen-space normals + SSAO** from the points depth buffer, once density is
  high enough that depth is near-continuous.
- **Depth of field** — nearly free in the splat renderer (scale splat by circle
  of confusion).

### 3.2 Fog

The existing fog "isn't good somehow." Suggested loop for an agent with vision:

1. Have it **describe what's wrong** before touching values. Likely culprits
   are the model, not the numbers: linear fog on a log-density image, fogging
   toward grey instead of the background colour, fog applied before
   tonemapping.
2. Render a `--sweep` grid over fog parameters and pick against stated
   criteria, e.g. "far structure recedes but stays legible; no flat grey wash;
   near detail keeps full contrast." A reference image of fog Hazel likes is
   even better than criteria.
3. Refine around the chosen cell.

### 3.3 Heavier — final-render modes

- **Density volume + emission/absorption raymarch.** The flame histogram lifted
  to 3D. One slider — absorption:emission ratio — spans glowing nebula to solid
  self-occluding object, with the interesting looks in between. Flame
  tonemapping (log density, gamma, vibrancy) carries over per voxel. Memory is
  the blocker: if revisited, **a spatial hash grid** (allocate only voxels that
  get hit) is much simpler than an octree and captures most of the savings,
  since fractals are mostly empty space.
- **Lighting a volume**, if one exists: light-transmittance volume for
  self-shadowing, density gradient as normals, mip-based ambient occlusion.
- **Path-traced heterogeneous medium** (delta/ratio tracking). Wallpaper mode.
- **Isosurface extraction** (marching cubes over density, iso threshold on a
  slider). Mesh export for Blender and 3D printing.

### 3.4 Jacobian splats

Probably the most distinctive-to-IFS idea here. Draw each point as an
anisotropic Gaussian shaped by the local Jacobian, so splats align with the
fractal's structure — flat along sheets, thin along filaments — and fewer
points give smooth coverage.

**Correction to how this was first described in chat:** the Jacobian of the
*full* chain shrinks to zero as contractions multiply, so it can't size splats
directly. Use the product of the **last few** maps' Jacobians (a window), or an
exponential moving average — the colour system's contraction-based EMA is
precedent. Costs a mat3 per walker, which matters against the 120 FPS budget,
so this is probably a final-render mode. Shares code with the Jacobian warp
colouring in §4.2.

---

## 4. Authoring tools

### 4.1 Gizmo: endpoint shear

**Hazel's design:** the open right-tetrahedron gizmo (edges slide along axis,
origin translates in screen plane, ring rolls, outer edges rotate in their
plane, ctrl-drag scales, selected transform's contribution lights up). The one
planned-but-unbuilt handle is dragging an endpoint across the plane of the
other two axes — and there's currently no way to author shear in the GUI.

Useful property: each tetrahedron edge is one column of the linear part.
Moving the endpoint of column **a** within the plane spanned by **b** and
**c** adds multiples of other columns, which **never changes the
determinant**. So this handle is a *pure volume-preserving shear* — it can't
make the fractal swell or collapse. Nice to state in the UI.

Implementation notes:

- Map the mouse by intersecting the mouse ray with that plane, not in screen
  space.
- The plane goes edge-on to the camera in some views; detect grazing angles and
  fall back to a screen-parallel drag or lock to the most visible in-plane axis.
- A modifier for an unconstrained endpoint drag (edit the whole column,
  stretch included) is occasionally handy.

### 4.2 Seeing what a transform does

- **Warp lattice.** Hazel's idea: show how the selected transform warps a
  default 3D grid, to build intuition the way Apophysis's variation preview
  does. Extensions:
  - **Colour by Jacobian determinant** — red expands, blue contracts. Reveals
    where nonlinear variations blow up vs collapse.
  - **Lines vs points.** Lines show shear/twist better, but may be too busy
    (Hazel to test). If busy: show a single sweepable lattice *sheet* rather
    than the full volume; fade by depth; or only warp the gizmo's own
    subdivided volume.
  - **Scrub from identity** — animate the lattice from undeformed to fully
    transformed.
- **Fixed point + principal axes.** Mark each transform's fixed point (where
  its copy of the fractal "sits") and draw the eigenvectors of its linear part.

### 4.3 Seeing where things come from

- **Solo shortcut** alongside the existing visibility toggles.
- **Truncated address colouring.** Take the existing transform-colour-mix mode
  and limit it to the last 2–3 transforms. Reveals copies-within-copies; a
  render mode in its own right.
- **Point picking**, possibly replacing the chaos-trails feature. Click a point
  and show the **nested copies** it belongs to: if its address is `3, 1, 4…`,
  highlight the region made by transform 3, then 3∘1, then 3∘1∘4, as
  progressively tighter glows. Shows *why* the point is where it is. Picking
  pass records the last k transform indices in a small per-walker byte ring
  buffer. Also doubles as a way to set an infinite-zoom target.

### 4.4 Seeing inside

**Clipping plane / slab.** Draggable cut plane, or a thin slab swept through the
attractor. Directly addresses "no interior structure" with no new renderer.
Slab sweeps also animate well.

### 4.5 Exploration

- **Mutation grid, 5×5, full screen** (Hazel's preference over a small
  Apophysis-style window; `todo.txt` currently says 3×3). Centre = parent.
  Render cells progressively. Toggles for which mutation kinds are allowed
  (affine, variation swap, xaos edge, colour); lock specific transforms;
  mutation-strength slider.
- **Branching history.** Extend existing save/undo into a lineage tree: every
  edit or mutation is a node; jump to any ancestor and branch. TOML scenes make
  this cheap. Every grid click lands in the tree so lucky finds are never lost.
- **Crossover** — already covered in AB §2.5 (swap motifs/groups).

### 4.6 Symmetry additions

Existing: platonic groups, circular, probably mirrors (Hazel to check). Wanted:

- **Mirror across a plane** and **inversion through a point** (both
  determinant −1).
- **Rotoreflection** (rotate 360°/n, then reflect through the perpendicular
  plane) — a twisted, alternating symmetry unlike anything the platonic groups
  give.
- **Crystallographic groups** — a Claude is to investigate. Start with the 17
  wallpaper groups (2D, easy to visualise) before the 230 space groups.

### 4.7 Scene interpolation

Animation is camera-only today. When scene keyframes arrive, don't lerp matrix
entries (transforms shrink/collapse mid-rotation). Instead:

1. **Polar decomposition** of the linear part into rotation × stretch. Slerp
   rotation; interpolate stretch **in log space** (halfway between 0.25 and 1
   should be 0.5, not 0.625). If keyframes differ in orientation (determinant
   sign), there's no continuous path — snap at midpoint or disallow.
2. **Interpolate around the fixed point, not the origin.** Compute each
   keyframe's fixed point (always exists for contractive maps), lerp it,
   interpolate the linear part as above, and rebuild translation from those.
   Each copy of the fractal then rotates in place instead of swinging wildly.

Xaos weights (§1.9) interpolate trivially and are a good first animatable
scene parameter.

---

## 5. If picking, in order

1. **Xaos core** — GPU CDF rows + walker state, CPU mirror in `trace.rs`,
   name-keyed TOML, all-ones regression test. Zero-row policy decided.
2. **Stationary share** in `--info` and the GUI, plus strong-connectivity
   check.
3. **Xaos presets + random/mutation knobs** (§1.5, §1.8).
4. **Xaos editor** — heatmap first, node graph when it's fun.
5. **Composition coupling** hook agreed with the composition work (§2).
6. **EDL** and **solo shortcut** — each an afternoon.
7. **Endpoint shear handle** — closes the last gizmo gap.
8. **Warp lattice with determinant colouring**, then **clipping plane**.
9. **Truncated address colouring** and **point picking with nested copies**.
10. **5×5 mutation grid + lineage tree.**
11. Everything in §3.3–3.4 as final-render modes when the mood strikes.

## 6. Traps

- **Reducible xaos matrices** silently delete parts of the fractal. Check
  connectivity; show stationary share.
- **Don't let group draws depend on history** (§1.3) — it breaks exact symmetry.
- **Don't spend the frame budget.** Interactive view stays at 120 FPS; heavy
  modes are opt-in.
- **Full-chain Jacobians vanish** (§3.4) — window or EMA them.
- **Don't retry froxels** without a new idea for why reprojection will work.
