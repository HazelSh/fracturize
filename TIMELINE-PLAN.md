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
