# Gizmo plan: more control, and a gizmo you can see

> **Status: slices 1–5 are implemented**, on branch `gizmo-tip-handles`
> (`2c7e720`…`3e70040`, off `main` at `81fca03`). 419 tests pass, and every
> slice was verified by driving the running app as well. Hazel has not used any
> of it yet. Slices 6–7 and everything under "Recorded, deferred" are still
> just design.
>
> Two things worth a second look by eye, both named in §2 and the commits:
> `XRAY_ALPHA` (0.3 reads quite faint over a bright attractor) and the roll
> ring's neutral, which sits close to the reference tetrahedron's grey.

Design for the next round of IFS transform gizmos.
Two Sonnet agents brainstormed the manipulation grammar and the legibility half
separately; this is the synthesis, with their conclusions corrected where the
code disagreed with them.

Read alongside `todo.txt` — the blocks `---- gizmo updates` (~l.280),
`---- unfuck extended gizmo indicators` (~l.51), `---- think about gizmos vs
fractal render` (~l.61), and `---- selected transform's gizmo should draw on top
of fractal` (~l.277). Slices 1–5 close the last two outright and most of the
first; the indicator rework (l.51) is designed here but deferred with slice 7.

---

## 0. The two things you asked for

1. **Tetrahedron endpoints become handles.** Hover-responsive; while held, the
   axis draws as a dashed line extended past the handle; dragging scales that
   axis alone.
2. **Rotation against the camera plane**, shown and draggable.

Both are buildable. Everything else below is either a prerequisite that would
otherwise bite, or an item off your own todo list that falls out cheaply once
the handles exist.

Ask #2 ships in its conflict-free form only: **roll around the view axis, on a
ring**. The arcball tumble is designed but deferred (§2).

---

## 1. What I found first, because it changes the plan

### 1.1 Shear cannot be saved — and that is already losing data today

`Scene::to_scene_file` (`src/scene.rs:1654`) writes every transform by
decomposing it through `Trs::of` → `Mat4::to_scale_rotation_translation`, into
`translation` / `scale` / `rotation`-or-`rotvec`. `TransformDef`
(`src/scene.rs:690`) has no shear field and no raw-matrix escape hatch.
`Trs::is_faithful` is consulted at `scene.rs:1659`, but only to choose between
euler degrees and an exact `rotvec` — **it does not prevent, or even flag, the
loss of shear.**

So a sheared matrix is silently replaced by its nearest shear-free
approximation on save. This is not hypothetical and it is not new: the doc
comment on `decompose_trs` (`src/ui/transforms.rs:843`) says outright that
*"mutate.rs's rotate-after-scale composition is the main source of sheared,
non-faithful matrices in practice."* The inspector already copes — it falls back
to a raw 4×4 grid and offers "Orthogonalize → TRS" — but the file format does
not, so **mutating a scene and saving it can quietly change the attractor.**

Consequences for this plan:

- **Shear is cut from the gizmo work** until the format can hold it. Adding a
  shear drag would turn a rare pre-existing bug into an everyday one: you would
  author a shape with the mouse, save, reload, and find it changed.
- The format fix is small and worth doing on its own: when
  `Trs::is_faithful` is false, write the linear part verbatim as
  `matrix = [[…],[…],[…]]` (3×3, column-major) and have the loader prefer it
  over `scale`/`rotation` when present. That is one optional field, one branch
  in the loader, and it makes the raw-matrix inspector grid round-trip.
- Worth telling you separately from the gizmo work, because it affects scenes
  you already have.

### 1.2 Reflection *does* round-trip — but the app doesn't believe it

Dragging a tip handle through the origin flips that axis negative. I checked
whether that survives a save, since it decides whether the gesture can be
allowed.

glam 0.29.3's `to_scale_rotation_translation`
(`glam-0.29.3/src/f32/sse2/mat4.rs:252`) computes
`scale.x = |x_axis| * signum(det)` and derives the rotation from the
sign-corrected columns. For a matrix whose columns are mutually orthogonal — which
is every matrix a tip-scale drag can produce — the decomposition is **exact**,
and `ScaleDef::PerAxis` already accepts a negative component. So a mirrored
transform saves and reloads correctly today.

But `Trs::is_faithful` (`src/rot.rs:604`) ends with `&& m.determinant() >= 0.0`
— it rejects *every* negative-determinant matrix regardless of whether
recomposition matches. So the moment tip-drag makes reflection a normal gesture,
every mirrored transform drops the inspector into the raw 4×4 grid, for no
reason. **Relax that check to "recomposition matches"** and reflection becomes
a first-class state with friendly TRS fields and a negative scale component.

One honesty note to carry into the UI: glam puts the sign on `scale.x` always,
so mirroring the *Y* axis comes back as a negative *X* scale plus a compensating
rotation. Mathematically identical, surprising to read in a TOML diff. The
inspector should say "mirrored" from `determinant() < 0`, not from which
component happens to carry the minus.

### 1.3 The dimension-lock gate is about to become wrong

`update_gizmo_drag` (`src/app.rs:2724`) calls `hold_dimension_through` only
under `matches!(mode, GizmoDragMode::Scale { .. })`, justified by the comment
*"Only Scale can change a contraction, so the other modes never pay for the
check."* A per-axis scale changes the determinant too. Widen the gate to every
determinant-changing mode — it costs nothing, because
`hold_dimension_through` (`app.rs:3594`) already early-returns when the
determinant is unchanged.

### 1.4 The shader's part encoding is sized for exactly today's geometry

`shaders/gizmo.wgsl:61` does `let local = vertex_index % 42u` and treats part id
6 as "the origin dot" — 42 being 6 edges × 6 verts + one 6-vert dot billboard
(`gpu/gizmo.rs:41`). Adding three tip dots changes the per-instance block to 60
verts and needs the dot branch to say *which* dot. Both constants and
`set_highlight`'s `part_id` match (`gpu/gizmo.rs:437`) must move together — they
are three separate literals encoding one fact, so this is exactly the kind of
drift AGENTS.md's "compiler-enforced invariants" note is about. Derive the block
size from the counts rather than re-typing 42.

---

## 2. Decisions

**All of §7's open questions were answered by Hazel on 2026-08-09; the calls
below are settled, not proposed.** Scope is now **slices 1–5**. Arcball, the
indicator redesign, shear, the TOML format fix and constant-determinant reshape
are all deferred.

| Decision | Call | Why |
|---|---|---|
| Shear | **Deferred** (confirmed) | Unsaveable (§1.1). Reserve `ctrl`+tip for it. |
| Reflection via tip-through-zero | **Allow, no modifier gate** (confirmed) | Continuous in the math, round-trips on disk, standard in every 3D tool. |
| Camera-plane rotation | **Ring band = roll. Arcball deferred.** | Roll is the conflict-free half; the arcball disc is the half that takes territory from camera orbit, so it waits. |
| Ring drawn for | **The selected transform only** | A ring per transform × 20 transforms would carpet the viewport and swallow camera orbit. |
| Manipulating an unselected transform | **Origin dot, selection only — no drag** | Hazel's wording: "selectable & no interaction otherwise". Stricter than the first draft, which also translated; see §3.3. |
| Always-visible origin dots | **Yes, in edit mode** | Confirmed: "can't be worse than always visible whole gizmos". |
| Finding a transform to select | **White-backed label on origin-dot hover** | Restricting manipulation to the selected transform makes selection load-bearing; §3.3. |
| Dashes | **CPU-generated segments in `indicators.rs`** | The overlay is a `LineList` with no dash concept; chopping a straight segment is exact and needs no new pipeline. |
| Dash pitch | **Computed at grab, then fixed in world space** | Screen-sane at any zoom, and provably shimmer-free for the life of the drag. |
| Occlusion | **X-ray ghost pass, selected transform only** | One extra pipeline + one draw call; fixes the twice-noted todo item. |
| `indicators.rs` arrows/arcs | **Replace: single straight lines in 3D, quantities as flat 2D chrome** | Your complaint was that strut-built arrowheads don't read. A single line reads at every angle; a filled 2D triangle reads unambiguously. |

### 2.1 The modifier collision, resolved

`todo.txt` proposes `shift`+drag for shear. It can't have it: **shift is
fine-drag globally**, applied once in `update_gizmo_drag` via `fine_cursor()`
(`app.rs:2654`), which is exactly why it can be trusted without looking at what
is under the cursor. Overloading it per-handle would break the one modifier that
currently means the same thing everywhere.

Shear goes on **`ctrl`+tip** when it eventually lands. That cell is pure
redundancy today: once an unmodified tip drag scales that axis, `ctrl`+tip's
"uniform scale" is already reachable from `ctrl` on the origin, on any axis
shaft, and on any rotate edge. Spending the one redundant cell costs nothing and
keeps `ctrl` meaning "change the shape" everywhere.

---

## 3. The design

### 3.1 Handle inventory

`GizmoPart` grows from 3 variants to 6. Existing hitboxes are unchanged — a dead
zone between "shaft" and "tip" would be worse than an overlap resolved by
priority.

| Part | Pick geometry | Radius | Score bias |
|---|---|---|---|
| `Origin` | point vs. projected origin | 12px (`ORIGIN_RADIUS_PX`) | −8 (existing) |
| `Tip(k)` **new** | point vs. projected axis endpoint | 8px | −4 |
| `Axis(k)` | point vs. segment O→tip | 7px (`EDGE_RADIUS_PX`) | 0 |
| `RotEdge(k)` | point vs. segment tip→tip | 7px | 0 |
| `Roll` **new** | inside the ring annulus | band ±10px | +0.5 |

(`Arcball` — the interior disc — is designed in §3.4 but **deferred**; it is the
only part that would take territory from camera orbit.)

Lower score wins, matching `pick_gizmo`'s existing contest.

**Ring radius is derived per frame**, not a constant:
`R = max over k of |tip_screen(k) − origin_screen| + 20px`, with an absolute
floor so it stays grabbable around a tiny gizmo. Deriving it from the actual
projected silhouette means it can't drift out of register when you drag a tip to
a wildly different length.

**Tip vs. its neighbours.** A tip shares its screen point with the far end of
`Axis(k)` and with two `RotEdge` segments, by construction. The −4 bias makes the
vertex beat the edges that touch it, while still losing to `Origin`'s −8 when a
transform is so small that origin and tip nearly coincide — translate is the
safer default at that size than an uncontrollable scale-to-nothing.

**Sub-pixel axis guard.** Any axis whose projected length is under ~10px drops
out of the `Tip`/`Axis`/adjacent-`RotEdge` candidate set for that frame. That
folds "the axis points at the camera" and "the axis is three pixels long" into
picking, instead of leaving the drag math to discover it mid-gesture and produce
a spike.

### 3.2 Modifier map

Shift is fine everywhere, free, because every mode reads the cursor through
`fine_cursor()`. Alt is snap. Ctrl is uniform scale.

| Handle | (none) | shift | ctrl | alt |
|---|---|---|---|---|
| `Origin` | translate in view plane | fine | uniform scale | — |
| `Axis(k)` | slide along axis | fine | uniform scale | — |
| `Tip(k)` | **scale axis k** | fine | *(reserved: shear)* → uniform scale for now | snap scale to 0.1 steps |
| `RotEdge(k)` | rotate about local axis k | fine | uniform scale | snap 15° / group step |
| `Roll` | rotate about camera-forward | fine | uniform scale | snap 15° / group step |

The existing alt-snap already prefers the transform's own symmetry group step
over 15° when it has one (`app.rs:2688`). `Roll` and `Arcball` must use the same
lookup — a group is a statement about which rotations are exact in this scene,
and that shouldn't depend on which control you grabbed.

### 3.3 Only the selected transform is manipulable

Today `pick_gizmo` considers every transform's every part, with no depth
awareness at all — so a gizmo completely buried in the attractor still takes
your click, which is half of the "zooming breaks horribly as things move to
under the scrollwheel without visibility" complaint.

**Rule: an unselected transform offers only its origin dot, and that dot only
*selects* — it does not drag. Tips, axis shafts, rotate edges and the ring are
offered only by the already-selected transform.**

Note this is stricter than the first draft of this plan, which had an unselected
dot select *and* translate in one gesture. Hazel's call — "selectable & no
interaction otherwise" — is the tighter rule, and it is better: a press that
both selects and starts moving something you had not yet chosen is exactly the
kind of accident this whole section exists to prevent.

Consequence to implement deliberately: pressing on an unselected dot spends the
gesture on selection. It starts no drag at all — not a gizmo drag, and not a
camera orbit either. Beginning an orbit from a press that was aimed at a dot
would swing the camera while you were only trying to pick something. The press
selects, the release does nothing further.

This costs one extra click in a workflow you almost certainly already follow,
and buys: the invisible-click bug mostly gone, a pick contest of ~9 candidates
instead of ~140 at 20 transforms, no ring clutter, and no ambiguity about which
transform a ring belongs to.

**Because selection is now load-bearing, finding things has to get better in the
same slice.** Two changes, both cheap:

- Every transform's origin dot draws through the fractal whenever gizmos are on
  (§3.5) — the one thing you can always grab is the one thing you can always
  see.
- **Hovering an origin dot renders that transform's label on a solid white
  backdrop**, dark text, in `src/ui/labels.rs`. Labels already paint there for
  every transform; today the selected one gets a rounded-rect backdrop and the
  rest are bare text that disappears against a bright attractor. Hover promotes
  a label to maximum contrast, so scrubbing the pointer across a dense scene
  reads out names one at a time. This is the affordance that makes "select
  first" tolerable, so it ships *with* the restriction, not after it.

Hover already recomputes every frame through `update_hover`, and labels are
already drawn per transform, so this is a backdrop and a colour swap keyed off
`app.hovered`, not new machinery.

### 3.4 Math, per mode

Every mode computes from the grab-time matrix and the live cursor — absolute,
never incremental — matching the existing discipline. **None of them decompose.**
`to_scale_rotation_translation` stays reserved for the explicit, named, lossy
`orthogonalize_transform`; drag modes read and write matrix columns.

**`ScaleAxis(k)`** — at grab, capture `dir = start.col(k).truncate().normalize()`
and `s0 = line_param_closest_to_ray(origin, dir, ray_o, ray_d)` (the same helper
`TranslateAxis` already uses). Each frame recompute `s`; set
`col(k) = dir * (s - s0 + |start.col(k)|)`. Because `dir` is fixed and `s` is a
signed scalar, passing through zero is a continuous sign flip — reflection with
no branch. Floor `|col(k)| ≥ 1e-4` to keep the column non-degenerate (glam's
`glam_assert!(det != 0.0)` is a no-op in this build's feature set, but a zero
column poisons every later decomposition regardless). Clamp the per-drag ratio to
the same `[0.02, 50.0]` the existing uniform scale uses.

Critically: this preserves column *direction*, so the three columns stay mutually
orthogonal, so the matrix stays an honest `R·S` and keeps saving faithfully as
`ScaleDef::PerAxis`. That is the whole reason per-axis scale is safe and shear
is not.

**`Roll`** — identical to the existing `RotEdge` path (`screen_angle`,
`shortest_to`, alt-snap, `turn_linear_part`) with one substitution: the axis is
`camera.forward()` captured at grab instead of a matrix column. Implement it as
the existing `Rotate` mode with a different axis source, not a second code path.

**`Arcball`** — at grab, map the cursor to a point on a unit hemisphere in the
camera's right/up basis (in-radius → `z = sqrt(1 − x² − y²)`, outside → normalize
to the rim); call it `p0`. Each frame recompute `p1`;
`axis = p0.cross(p1)`, `angle = acos(p0.dot(p1))`, giving a `Turn`; apply with
`crate::rot::turn_linear_part(start, turn)`, which already means exactly "rotate
the linear part about its own origin, leave the translation". No new
column-juggling.

**Non-orthonormal axes.** After a per-axis scale the columns differ in length but
stay perpendicular, so `RotEdge`'s `axis = col(k).normalize()` is exactly as
well-defined as it is today, and `turn_linear_part` left-multiplies a rigid
rotation — column lengths and mutual angles are preserved, so `R·S` stays
recomposable. `Arcball` derives its axis from the *camera's* basis and never
looks at the transform's columns, so it is immune to the question by
construction. This is the property that keeps §1.1 from biting: no drag mode in
this plan can create shear.

### 3.5 What gets drawn

**Tip handles.** Three new dot billboards at X/Y/Z, in their axis colours
(red/green/blue, matching the shafts). Same `vertex_type == 2` path as the origin
dot. Per-instance block goes 42 → 60 verts; see §1.4.

**Hover vs. held are currently the same thing, and shouldn't be.** `update_hover`
(`app.rs:2400`) is the only writer of the highlight uniform, and it is
deliberately not called during a drag — if it were, the cursor would have moved
off the grabbed part and un-highlighted the thing you are dragging. So "held" is
just "a hover that stopped being recomputed", and there is no feedback at the
instant a press lands. The uniform's `.z` and `.w` are unused padding: pack a
`held` flag into `.z` for free. Hover keeps its 55% white mix
(`gizmo.wgsl:178`); held instead goes **full saturation, no white mix**, at max
width — a different signal, not a louder one.

**The dashed axis extension.** Only while a tip is held. Generated as explicit
short segments in `indicators.rs`, running from the origin out past the handle
(both ways, so a negative scale reads as "you went through zero"). Pitch is
computed once at grab from the projected screen length so dashes start at about
8px on / 8px off, then held fixed in world space for the drag — screen-sane at
any zoom, and provably shimmer-free, since the boundaries sit at fixed `t = k/N`
along a world segment and the camera cannot move mid-drag. `indicators.rs` is
already rebuilt on `matrix_generation`, which bumps every frame of a drag
anyway, so this is free.

**The ring.** A screen-aligned circle at radius `R`, drawn only for the selected
transform. Dashed and neutral pale grey at rest — dashing says "this is UI, not
an edge of the tetrahedron", and neutral keeps it out of the six hues already
spent on axes and faces. Solid the instant it is held. It is the one part where
held *loses* its distinguishing mark rather than gaining one, which is exactly
how a ring reads as engaged. A screen-space circle is also the only rotation
affordance that never degenerates: the three local-axis rotate edges collapse to
a line viewed edge-on, which is precisely when you need this one.

**Screen-constant sizing, and where to stop.** World space: the four corner
positions — where a handle *is*. Screen space: stroke widths, dot sizes, pick
radii, ring band. Do **not** perspective-cancel the tetrahedron to hold a
constant apparent size: a handle that no longer sits on the actual corner makes
the dashed axis extension point somewhere that isn't the axis. When a gizmo is
too small to grab precisely, the answer is `Home` (put the camera on the selected
transform's fixed point, `app.rs:1171`) — navigation, not a rendering lie.

**Occlusion.** The edge/dot pipeline depth-writes, so dense material hides gizmos
completely. Fix: draw the **selected** transform's edge/dot geometry twice — once
with `depth_compare: Always` at low alpha (ghost, first), then the existing
depth-tested pass on top. A visible gizmo looks exactly as it does now; only the
buried part shows through, faintly. One extra pipeline, one extra draw call, no
new bindings, reusing the existing per-instance buffers. Which instance is
selected can ride in the highlight uniform's spare `.w`. Extend the same
treatment to *every* transform's origin dot (6 verts each) — that is the todo's
own "center points" suggestion, and it is what makes §3.3's origin-dot-only rule
honest: the one thing you can always grab is the one thing you can always see.
**Confirmed by Hazel**, on the grounds that always-visible dots can't be worse
than the always-visible whole gizmos we have now.

Scoped to edit mode — which is already what it means for gizmos to be on, so
this needs no new state, just the existing `show_gizmos` gate. View mode is for
looking at the art and keeps the viewport clean, consistent with the cursor
auto-hide that just landed.

Deliberately **not** always-on-top for all gizmos in full: that would defeat the
`G`/Tab toggle that exists so you can look at the art. Dots yes, whole
tetrahedra no.

**Replacing the indicators.** Your note says the line-built arrowheads took far
too long to read, and the instinct — "closer to flat UI, not 3d" — is right for
the *quantity* but not for the reference geometry. Split them:

- Keep in 3D, each a single unbroken straight segment: the offset shaft, and the
  rotation axis. A line reads at any angle; a cluster of short struts does not,
  and an arc seen near edge-on is indistinguishable from scribble.
- Drop the four-strut arrowhead, the arc, and its ticks entirely.
- Move "how much" and "which way" into `src/ui/labels.rs`, which already does the
  `world_to_screen` projection: a small filled 2D triangle at the shaft's
  projected midpoint pointing along its projected direction, plus the magnitude
  as text; and a compact angle dial with degrees near the axis.

Net: *less* GPU geometry, and the ambiguous part moves to the layer where 2D is
honest. `indicators.rs`'s `offset_only_draws_the_vector_but_no_arc` and
`rotation_produces_axis_and_arc` will need rewriting to match.

### 3.6 Hints, history, cursors

New strings in the `hints.rs` voice:

```
HINT_TIP:        "drag: scale this axis (shift: fine, alt: snap) · drag through the origin to mirror"
HINT_ROLL:       "drag: roll around the view axis (shift: fine, alt: snap 15°)"
```

An unselected transform's origin dot needs its own hint, since it no longer does
what a selected one's does:

```
HINT_SELECT:     "click: select this transform"
```

`HINT_AXIS` gains ", shown extended" — the action didn't change, the visibility
did. `HINT_ORIGIN` and `HINT_ROT_EDGE` are unchanged.

History granularity is unchanged: one `commit_edit` per grab-to-release, no
coalescing. `gizmo_drag_label` gains arms — `"Scale X whorl"` and
`"Roll whorl"`. Selecting a transform is not an edit and makes no history entry,
which is already true and stays true under §3.3.

`set_drag_cursor` already maps all `Drag::Gizmo` to `Grabbing`; that stays right
for every new mode.

---

## 4. Slices, in order

Each is independently shippable and independently testable. **Scope is slices
1–5.** Slices 6 and 7 are recorded for later, not to be built now.

**Slice 1 — tip handles that scale one axis.** ✅ `2c7e720` Delivers ask #1 whole.
`GizmoPart::Tip(k)` + pick test + sub-pixel guard; three tip dot billboards and
the 42→60 vertex-block/part-id rework (§1.4); `GizmoDragMode::ScaleAxis`;
the dimension-lock gate widened (§1.3); the dashed extension while held (§3.5);
held-vs-hover split (§3.5). Reuses `line_param_closest_to_ray` verbatim.

**Slice 2 — `is_faithful` accepts reflection.** ✅ `f430f73` (§1.2) Small, and wants to land
right behind slice 1, because slice 1 is what makes mirroring easy to reach.
Pure `rot.rs` change plus a round-trip test; the inspector gains a "mirrored"
readout driven by `determinant() < 0`.

**Slice 3 — the X-ray ghost, always-visible origin dots, and the hover label.** ✅ `c2f97e8`
(§3.5, §3.3) Pure rendering plus one egui backdrop, touches nothing slices 1–2
own, fixes the todo item noted twice. Must land before slice 4: it is what makes
"select first, then manipulate" tolerable, and slice 4 without it would be a
regression rather than a fix.

**Slice 4 — selection scoping.** ✅ `c7d7417` (§3.3) `pick_gizmo` gains a `selected`
parameter and offers non-selected transforms only their origin dot, select-only.
Press-on-unselected-dot must start no drag of any kind.

**Slice 5 — the ring, and roll.** ✅ `3e70040` Ring geometry, radius derivation, annulus
pick, `GizmoDragMode::Roll`. Delivers ask #2 in its conflict-free form. **Stop
here.**

### Recorded, deferred

**Slice 6 — arcball.** The interior disc, competing with camera orbit: inside
the ring, a left-drag would no longer orbit. If it is ever built, a press inside
the disc that never crosses the drag threshold must still fall through to
deselect, exactly as empty space does (`on_mouse_release`, `app.rs:2525`) —
otherwise the ring punches a hole in click-to-deselect.

**Slice 7 — the indicator redesign.** (§3.5) Most invasive, a redesign rather
than an addition, and its look should match whatever slices 1–5 settled on.

**Shear**, and the **TOML format fix** it depends on (§1.1). Deferred together,
deliberately: shear without the format fix authors data the save silently
discards. Note the format bug bites `mutate.rs` output *today*, independently of
whether shear ever becomes a gesture.

**Constant-determinant reshape** (`todo.txt` l.285) — on hold.

---

## 5. Tests

The existing `pick.rs` tests are the right model — pure math, no GPU.

- Tip pick beats the axis shaft and both rotate edges at the shared point;
  origin still beats tip at small projected size.
- An axis under the projection floor offers no tip/shaft/rotate candidates.
- `ScaleAxis` through zero: the column reverses, the other two columns are
  bit-identical, determinant changes sign.
- After `ScaleAxis` on all three axes plus a `RotEdge` drag, `Trs::of` round-trips
  the matrix exactly — the guard that no drag mode introduces shear.
- A mirrored matrix survives `to_scene_file` → load (this is the §1.2 claim, and
  it should be a test rather than a paragraph).
- `Roll` with the camera axis produces the same rotation as `RotEdge` would if
  the transform's axis happened to equal `camera.forward()`.
- Ring radius derivation is stable under a uniform scale of the transform.
- An unselected transform offers exactly one candidate (its origin dot) and a
  selected one offers the full set — the §3.3 rule, as a test rather than a
  convention someone has to remember.

Also worth a golden-layout style check that the new part-id encoding and the
vertex block size agree — three literals encoding one fact is exactly the drift
AGENTS.md warns about.

---

## 6. Branch note

`ui-batch-a-b` was merged to `main` as a fast-forward on 2026-08-09; `main` is
at `81fca03` and 398 tests pass there. This work branches from that.

---

## 7. Questions, answered

Answered by Hazel 2026-08-09. Kept as the record of what was decided and why.

1. **Restrict manipulation to the selected transform?** **Yes** — and tightened:
   an unselected transform's origin dot *selects only*, with no drag of any kind
   (§3.3). Because that makes selection load-bearing, the white-backed hover
   label ships in the same slice.
2. **Arcball?** **No, not yet.** Stop at slice 5. Roll on the ring band covers
   "rotate against the camera plane" without taking territory from camera orbit.
3. **Shear / the format fix?** **Both deferred, together.**
4. **Mirroring with no modifier gate?** **Yes, ungated.**
5. **Always-visible origin dots?** **Yes**, in edit mode — "can't be worse than
   always visible whole gizmos".
6. **Constant-determinant reshape?** **On hold.**

The one place implementation departs from this plan's first draft is §3.3's
select-only rule, which is stricter than what was originally written and is
recorded there.
