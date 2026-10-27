# Timeline: what's left to investigate

Tracks and the one clock landed on 2026-10-26 (AGENTS.md, "Tracks and the
timeline"; FEATURE-PLANS.md items 8–10). Two questions were left open because
they change how people work or what's on disk, and they need Hazel's eye before
anything is built. This file is where they are thought through. Nothing here is
decided.

---

## 1. Auto-key: what an inspector edit does to a tracked value

### Where it stands

Editing a value in the Transforms, Render or Subscenes windows edits the
**scene's own value**, whatever tracks exist. Key values are edited only in the
Timeline's detail column. That was chosen because it is the one behaviour that
is never surprising in a destructive way, not because it is the best one.

What it means for each kind of track:

| Track on the value | What an inspector edit visibly does |
|---|---|
| none | changes it, as always |
| rate, cycle | moves the whole motion — they move *about* the scene's value. This is useful and probably right. |
| keys | **nothing on screen.** The keys override the value at every time, so the edit lands in the file but is never seen. This is the problem. |

### Options

1. **Blender's auto-key**: a record toggle in the Timeline; while on, an edit
   to a keyed value writes (or replaces) a key at the playhead. While off,
   editing a keyed value does what it does now. Familiar to anyone from
   Blender, After Effects or JWildfire; a mode, and modes get forgotten.
2. **Always key when keyed**: an edit to a value that has a keyed track keys the
   playhead; no toggle. No mode to forget, but an edit made while just looking
   around quietly adds a key.
3. **Ask the first time**: a keyed value's field gets a small key marker; an
   edit to it while unmarked offers "key at 2.40 s / change the base".
   Discoverable, more chrome.
4. **Leave it, but say so**: the field shows that it is keyed (a diamond beside
   it, tinted when the playhead is on a key, as Blender does) and its tooltip
   says edits go to the base. The minimum honest fix, and compatible with any
   of the above later.

### What has to be true whichever is chosen

- **One edit, one history entry**: a drag that keys must coalesce like any
  drag (`commit_edit`'s coalescing key), so one Ctrl+Z takes the key away.
- **A field needs to know its path.** Inspector fields edit `TransformSpec` and
  `App` values directly; none of them knows its `-S` path. `params::catalogue`
  knows the mapping the other way. Auto-key needs a field → path link for every
  keyable field: probably a small table beside each panel, tested against the
  catalogue so it can't drift.
- **Matrix fields are the hard case.** The inspector shows translation /
  rotation / scale decomposed from the matrix; a track keys one Euler angle of
  the triple the scene *writes*. Keying "rotation y" from a gizmo drag, which
  changes the matrix, means decomposing it the same way the writer does — and
  near |y| = 90° that is the other branch (see the Euler note in AGENTS.md).
  Gizmo drags may be better kept as base edits, or key all three angles.
- **Rates and cycles**: an edit should keep moving the base (it does now).
- The live window's view values (point size, haze, exposure, contrast) live on
  `App` and are swapped for the playhead's during drawing; an auto-key path
  must write the key, not the swapped value.

### To find out

- How often a scene's tracks will be on values people also edit by hand. If
  tracks are mostly cycles and rates, option 4 may be all that's ever needed.
- Whether gizmo drags should key at all.

---

## 2. Moving the camera path into `[[track]]` form

**Done, 2026-10-28.** Hazel asked for it: the camera animated as views at
keyframe points, interpolated, reproducing the old flights, with spirals kept.
What was settled, against the questions below:

- **Keys have times**, and the spline honours them: `path.rs`'s cumulative
  quaternion Catmull-Rom took per-second tangents (`timed_basis`), which at
  even spacing is exactly the old uniform spline. Routes (`turns`, `route`,
  the chained yaw column) are unchanged, so winze still corkscrews.
- **The file**: `[[track]] target = "camera"`, `extrapolate` (hold / pingpong /
  repeat / offset = the zoom loop), `period` (a loop's lap), `zoom_periods`,
  `ease`, then `[[track.keys]]` tables with `t` and a view's camera fields.
  Written first among the tracks; comments on the old `[[camera.path]]` keys
  move onto the new ones. The old spelling still loads.
- **Easing** is now coming to rest at the first and last key (zero end
  tangents), not a smoothstep on the whole lap, so every key is passed at its
  own time. Old eased paths load with their keys placed when the smoothstep
  passed them, so they fly nearly as before (ammonite,
  taking_a_line_for_an_orbit and random-1785761947 are the shipped ones).
- **The default orbit** stays four quarter-turn keys on a repeat, never
  written until edited.
- **Views as keys**: yes — the Timeline keys the framing on screen, or a saved
  view, at the playhead; Y still appends after the last key.
- **In memory** the camera is `Scene::camera_path`, not an entry in
  `Scene::tracks`: it is the one track every scene has and drives the view,
  not an `-S` path.

The text below is the reasoning from before it was done.

### Where it stands

The camera shares the *time model* with tracks — one clock, its loop is an
`Extrapolate`, the zoom loop is repeat-with-offset — but its **file format is
unchanged**: `[camera] path_loop / path_seconds / path_ease /
path_zoom_periods` plus `[[camera.path]]` keypoints, with routes, `turns` and
`rotvec`. FEATURE-PLANS item 9 proposed writing it as a track.

### Why it wasn't done

- **It changes every scene with a path.** The plan itself makes that one
  explicit migration commit, run through the loader and saver with a
  before/after `--info` diff and a render of every scene with a path. Scene
  files are their authors' (see the memory rule); that commit is Hazel's to
  approve.
- **Nothing built so far needs it.** The Timeline draws the camera's keys at the
  moments it passes them, edits its duration and loop, and attaches/detaches
  it, all through the existing `CameraPath`.

### What a `[[track]]` camera would have to carry

A camera track is a *framing* track, not a number: its keys are orientations,
distances and focus points; its segments carry routes (`Turns(n)` or an exact
rotation vector, checked against the far key on load); its spline is the
cumulative quaternion Catmull-Rom of `path.rs`. A sketch:

```toml
[[track]]
target = "camera"                # the one non-path target
keys = [
  { t = 0.0, yaw = 0.0, pitch = 0.62, distance = 3.0 },
  { t = 5.3, yaw = 2.09, pitch = 0.45, turns = 1 },
  { t = 10.7, rotvec = [0.1, 4.19, 0.0] },
]
extrapolate = "repeat"           # once/pingpong/closed/zoom → hold/pingpong/repeat/offset
ease = false
```

Differences from today worth deciding on purpose:

- **Keys get times.** Today keys are evenly spaced in path time (uniform
  Catmull-Rom); `path_seconds` sets the whole length. Timed keys would mean a
  non-uniform spline, which changes every existing flight unless migrated
  scenes write evenly spaced times — and then retiming a key changes the curve
  shape around it. `golden/flights.txt` pins every shipped flight and would
  have to be re-blessed deliberately, or the migration must reproduce them
  exactly.
- **The zoom loop's offset** is derived from the zoom map and re-derived when
  it is edited. As `extrapolate = "offset"` on the camera track it stays
  derived; the file says only "offset", and `path_zoom_periods` becomes a
  property of it.
- **The default orbit** has no keys; it would be a camera track with a cyclic
  source, or the absence of a camera track (as now).
- **Views as keys**: whether a saved view (`V`) can be dropped onto the
  timeline as a camera key. Open in FEATURE-PLANS item 9.

### Migration, when it happens

1. The loader keeps reading every old spelling forever (as it does `offset`,
   `fog`, `path_closed`).
2. One commit runs every scene with a path through load and save, with an
   `--info` diff and a `--time-grid` render of each before and after, and the
   flights golden compared.
3. The saver writes the new form only from then on.

### To find out

- Whether timed camera keys are wanted (they are what makes the camera
  editable on the Timeline like any other track — drag a camera key to retime
  it), and whether that is worth a re-blessed flights golden.
- Whether views-as-keys is wanted, since it shapes the key format.

---

## 3. Rotation tracks and the Euler branch

### The problem

A map's rotation is held as a **matrix**; the file writes it as three Euler
angles (extrinsic XYZ, degrees), or as a `rotvec` where no Euler triple
reproduces it closely enough (near a quarter turn about y). Every rotation has
two Euler triples, and the saver always writes the one with |y| ≤ 90°: an
authored `[0, 135, 0]` comes back as `[180, 45, 180]`. Same pose, different
numbers.

A track on `transform.x.rotation.y` sets **one number of the triple the scene
writes** (the animator works on the text a save would produce, so the app and
the CLI agree). So:

- **The motion depends on which triple is written.** Sweeping y from 135° to
  180° on `[0, 135, 0]` is a turn about y; the same sweep applied to
  `[180, 45, 180]` sets y to 135…180 inside a triple whose x and z are 180°,
  which is a different motion that doesn't start at the authored pose.
- **A save can switch the branch under an existing track**, if the map's
  rotation was authored past |y| = 90°.
- **The saver may write `rotvec` instead**, and then `rotation.*` doesn't
  resolve: the track is skipped and reported (it is not refused, by design).
- Separately from the branch problem, animating one Euler angle is a poor
  control anyway: y alone is a turn about a fixed axis only when x is zero,
  and near y = ±90° (gimbal lock) x and z become the same control.

Today the only defence is a warning, and it appears in the terminal and in
agent reports but **not in the Timeline**.

### Is a matrix fallback the answer?

**No, and the file doesn't have one.** `[[transform]]` is translation, scale,
`rotation` or `rotvec`. The raw 3×4 grid exists only in the inspector, for
sheared matrices, which the file can't write at all (the gizmo plan's shear
note). And animating matrix entries would be worse than Euler angles:
interpolating two rotation matrices element by element passes through
matrices that shear and shrink, which is neither a rotation nor saveable.
Interpolating `rotvec` components has the same flaw, more mildly, except about
a fixed axis. Rotations want to be animated as rotations.

### Mitigations, cheapest first

**A. Say it where people look.** Flag such tracks in the Timeline (a warning
mark on the row, the reason in its tooltip), and in the parameter picker mark
or grey the x/y/z rotation leaves of a map whose y is past 90°, pointing at B
or C below. `track::Animator` already detects the case. Small, and worth
doing whatever else happens.

**B. Remember the authored triple.** Keep the Euler triple a map was read with
beside its matrix (as `colours_left_out` remembers which colours were
implied). The saver writes it back whenever it still reproduces the matrix,
and the animator's text then carries the author's branch. When an edit changes
the rotation (a gizmo drag), decompose onto the branch **nearest the
remembered triple**, so the numbers move continuously instead of jumping
between branches. This fixes the save flip for every consumer: tracks,
`--diff`, and `--info`'s "authored (-26, 138, 0) prints as (154, 42, -180)".
It doesn't fix gimbal lock or make one-angle tracks a good control.

**C. Rotation tracks that aren't Euler angles (recommended).** Add two
angle-valued targets that compose a rotation onto the map's authored pose
instead of overwriting one of its numbers:

- **spin**: about one of the map's own axes, through its own origin. The
  translation stays put. This is "turn the petal about itself".
- **turn**: about a world axis through the origin (or the zoom fixed point, or
  a symmetry axis), moving the translation with it. This is "carry the motif
  round the group".

These are exactly `--derive`'s `--spin` and `--turn` (`src/derive.rs`), whose
quaternion products are already written and tested. The value is still one
number, degrees, so keys, rates, cycles, extrapolation, clamping, the picker
and the Timeline's curve all work unchanged. A `rate` spins forever with no
wrap and no gimbal lock, and editing the base pose in the inspector moves the
whole motion, as rates and cycles already do.

A sketch of the file form:

```toml
[[track]]
target = "transform.petal.spin.y"    # own y axis; or turn.x / turn.y / turn.z
rate = 45.0                           # degrees per second
```

What it needs:

- **Targets that aren't `-S` paths.** The animator applies the `-S` tracks to
  the text, parses, then multiplies the spin/turn rotations into the parsed
  frame's matrices (`M · R` for spin, `W · M` for turn). Several on one map
  compose in a fixed, documented order; a free `axis = [x, y, z]` could come
  later.
- **The picker** offers "Spin about its own x / y / z" and "Turn about world
  x / y / z" under each map, and could stop offering raw Euler angles, or keep
  them for maps already at |y| ≤ 90°.
- **Symmetry**: a spin on a motif applies before the group, so every copy
  spins; a turn about the group's own axis is the natural "the whole flower
  rotates" track.
- **Infinite zoom**: a spin or turn on the zoom map changes the similarity per
  frame, the same live re-derivation `App::refresh_zoom` already does when the
  map is dragged.
- **Classes**: structure, like any matrix change.

**D. Pose keys — done 2026-10-28.** A track on `transform.<map>.rotation` (or a subscene's) keys whole poses, `[[track.keys]]` tables with `t` and `rotation` or `rotvec`; interpolated with the camera's spline and routes, the route read from the Euler column (one angle changing turns about that angle's axis by exactly that much), applied as `rotvec`. Linear, smooth, spline and step all mean what they mean for numbers, and `offset` keeps turning. C (rate tracks that spin about an axis forever) is still open, and offset on a pose covers much of it. The original note:

**D. Pose keys (later, if wanted).** Keys that are whole orientations,
interpolated as rotations with the camera path's cumulative quaternion spline
and its routes. That is the general "pose A at 0 s, pose B at 4 s" tool and
handles any pair of poses, not just one axis. It is the same machinery a
`[[track]]`-form camera would use (§2), so it belongs with that decision rather
than before it.

### Suggested order

A now (cheap, honest). B in the next pass that touches the saver. C as the
real answer for rotation in animation. D with the camera-format decision.
