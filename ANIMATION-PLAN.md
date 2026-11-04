# Animation: the timeline as a DAW's automation section

*Status: plan, 2026-10-29. Written by Claude Opus 5.5 from Hazel's test notes
(`animation-test.txt`) on animating the default Sierpiński
(`scenes/sierpinski-anim.toml`). Decisions that are Hazel's are collected at
the end.*

*Status, 2026-11-05: most of slices 2–4 are built (f1be6fc: the transport in
the toolbar, record, a frame counter you can type into, ←/→ stepping, the
pinned ruler, zoom and page-turning, the loop region, key menus, snapping).
Slice 1's map-following is done too. Still open: slice 1's loop-seam note in
`--info` and the Timeline; `[timeline]` scene data (fps, bpm); lanes as
value-height automation curves; box select, summary rows and markers;
slice 5 (vector tracks, field-to-path, gizmo keys); slice 6 (morphs); and
the rotation tracks at the end of §4, moved here from TIMELINE-PLAN.md, which
was deleted once its other questions were settled (record is the auto-key,
the camera is a `[[track]]`, maps have pose keys).*

> **For:** Jay (and Hazel, who brings DAW hands), Remy, Atlas, Mote.
> **Costs:** Pip, unless the transport stays quiet until a scene has tracks.
> **Neutral:** Theo (gets exact times and frames), Rook (gets the
> JWildfire-style morph they already know).

The tracks model is sound: one clock, `-S` paths as targets, extrapolation,
classes. What's missing is everything people's hands expect from a timeline.
Hazel asked for "full DAW timeline automation", and that is the right target.
Blender and the VFX tools agree with a DAW on almost all of it. Where they
disagree, this plan says which to follow and why.

---

## 0. What happened in sierpinski-anim

### The snap at the loop is real, and it's the loop length

The camera ping-pongs over three keys' worth of the default orbit (key
spacing 8.7266 s, which is a quarter turn at 0.18 rad/s), so its round trip
is **52.36 s**. That is the timeline's length, and live playback wraps the
clock there.

The five other tracks ping-pong over 0 → 8.284 s, so they repeat every
**16.57 s**. 52.36 / 16.57 = 3.16, so the clock wraps in the middle of their
fourth cycle. Measured with `--time` (on the 28 Oct binary):

| | julia | hyperbolic | polar | spherefold |
|---|---|---|---|---|
| t = 0 | 1.119 | 1.818 | 0.397 | 1.686 |
| t = 52.36 | 0.831 | 1.350 | 0.295 | 1.252 |

At the wrap every weight jumps back up from about 74% of its starting
value. Your diagnosis was right: the loop wasn't a multiple of the repeat.

**The fix for this scene is one number.** Move the four 8.284 s keys (and the
rotation's) to **8.727 s**, the camera's own key spacing. Each track then
repeats every 17.45 s, which divides 52.36 exactly three times, and every
value at 52.36 matches t = 0 to six digits (checked on a scratch copy; your
file is untouched). That's exactly the "snap to other keys" in your notes. If
snapping had existed, this loop would have closed on its own.

### Why nothing told you

- **The Timeline doesn't show seams.** Nothing marks a track that won't close
  at the loop point.
- **`--info` says `notes none`.** `track::loops_over` already knows the answer
  (the offline renderer asks it before dropping the last frame), but the
  report never says so. An agent writing this scene would have shipped the
  same seam.
- **"Continuous" isn't on offer.** With Loop off, playback stops at the end.
  With Loop on, the clock is set back to 0. There is no mode where the clock
  just keeps running and each track keeps its own cycle, which is what you
  meant by a continuous timeline.

### Other things found while reading it

- **Tracks on unnamed maps use indices (`transform.3…`), and deleting a map
  doesn't touch tracks.** `Scene::remove_transform` fixes up xaos columns but
  not track paths, and there's no index re-mapping in `track.rs`. Delete map 1
  and `transform.2.rotation` quietly starts moving what was map 3. Renaming
  is handled (`track::rename_map`), but deleting and inserting aren't. This is
  a correctness bug: it changes your artwork without saying so. **Fixed
  2026-10-29**: `track::follow_maps` / `follow_subscenes`, called from the
  scene's insert and remove (see AGENTS.md, "Tracks and the timeline").
- The saver writes every camera field into every camera key (pitch, distance,
  focus and roll repeated four times when only yaw moves). The format already
  lets keys inherit from `[camera]`, so the file could read as "yaw swings
  three quarters of a turn, and nothing else moves".
- Someone else currently has uncommitted edits to `path.rs`, `track.rs` and
  `scene.rs` in this checkout, and was rebuilding while this was written.
  Whoever picks up §4 should check what that was first.

---

## 1. What each tradition expects

| | DAW (Hazel) | Blender | VFX (After Effects, Nuke, Houdini) | Flame tools (JWildfire, Apophysis) |
|---|---|---|---|---|
| Clock | bars:beats and timecode, tempo | frames at a scene fps | timecode / frames, comp fps | frames |
| Transport | play, stop (back to start), record, loop brace, RTZ, editable counter | Space play, Esc back to start, ←/→ step, ↑/↓ next key | J/K/L, Home/End, in/out points | play a sequence |
| How a value gets keyed | per-track record arm; modes Read/Touch/Latch/Write record moves *while it plays* | I over a field; auto-key (red dot); diamond beside each keyed field | stopwatch per property; edit at playhead keys it | keyframe whole flames |
| Where keys are edited | automation lanes: value is height; click the line to add; drag points both ways; bend segments | dope sheet (time) and graph editor (value), separate | timeline and graph editor | flame list |
| Moving many at once | select across lanes, drag, snap to grid | box select in dope sheet; summary row moves a column | select keys across layers | — |
| Loops | one loop region on the song; everything sits on a bar grid so it closes | frame range; per-curve Cycles modifier | comp range; `loopOut()` expressions | — |
| Procedural motion | LFOs, random/S&H | modifiers: noise, cycles, stepped | `wiggle()`, `time*45` | — |
| One control, many values | macros, VCAs | drivers, shape keys | pick-whip, null controllers | morph between flames |
| Navigation | horizontal zoom and scroll, sticky ruler, follow playhead | ctrl+scroll zoom, Home = frame all | same | — |

## 2. Where they agree: what's worth living up to

These are near-universal. Someone from any of the four will reach for them
without thinking, and finding nothing there is a "this isn't finished" moment.

1. **A transport you can drive blind.** Play/pause, return to start, record,
   loop, a counter you can type into, step a frame, jump to the previous or
   next key. Always in the same place (the toolbar, as your notes say), and
   always visible, not inside a window that might be closed.
2. **The ruler never scrolls away.** It is pinned at the top of the lanes,
   whatever is open below it. Clicking or dragging on it moves the playhead.
3. **Zoom and scroll in time.** Ctrl+scroll zooms around the pointer;
   shift+scroll (or a horizontal wheel) pans; a key frames everything. This
   matches the house rule that scroll is only for navigating.
4. **Keys are things you can select, copy, move and delete.** Right-click a
   lane to key here; right-click a key to duplicate, copy, paste, delete or
   set the interpolation from here; box-select across lanes; Ctrl+C/V paste
   at the playhead.
5. **Snapping, on by default, with a modifier to bypass it.** Snap to frames
   (always, so a render can land on every key), and as toggles to the grid,
   to other keys (all tracks, the camera's included), to the playhead, to
   markers and to the loop points. The house rule here is already "ctrl
   snaps" on gizmos. In the timeline, snapping is on and ctrl flips it.
6. **Edit where you look, and see what is keyed.** Every keyable field shows
   whether it's animated (Blender's diamond: filled on a key, hollow between
   keys, tinted when edited but not keyed), and a key press over it keys it.
7. **Moving things in the viewport can be keyed.** Drag the gizmo, key the
   whole position, rotation and scale. Your "pick it up and move it" note.
8. **A loop is a region you set, and the tool helps it close.** Every
   tradition has an in/out point. The DAW one is the best of them, because
   a grid is what makes things close.
9. **Time is shown in the units you think in.** Seconds and frames, as you
   asked. Bars and beats when a tempo is set. Always stored as seconds.

## 3. Where they disagree, and which to follow

**Lanes: follow the DAW, not Blender.** Blender splits time (dope sheet)
from value (graph editor) because it animates thousands of channels per
object. Fracturize's tracks are scalar parameter automation, which is exactly
the problem a DAW lane solves: the curve is the lane, its height is the
value range, you click the line to add a point, drag a point in time *and*
value, and alt-drag a segment to bend it. A lane can be resized taller for
fine work. A "graph editor" then needs no window of its own; it's a tall lane.

**Keying: Blender's I-key plus a DAW record button, one behaviour each.**
- *Paused, record off:* hover a field (or select a map) and press the insert
  key: key at the playhead. That works for everyone, needs no mode, and can't
  be forgotten.
- *Paused, record on:* auto-key, as in Blender: any edit to a keyable value
  keys it at the playhead.
- *Playing, record on:* **touch** mode, as in a DAW: while you hold a control
  (a slider, a gizmo), its moves are written in real time, thinned to a few
  keys. When you let go, the existing curve takes over again. No other
  fractal tool records a performance, and a gizmo held while the attractor
  re-forms is an instrument. This is where Hazel's DAW background earns
  something Blender users don't have.

  Per-track arm (the DAW refinement) can come later. One global record button
  covers the common case.

**Looping: keep per-track cycles (Blender, synth LFOs) *and* add a region
(DAW).** Per-track extrapolation is real expressive power, since a fractal
breathing at its own rate is the point. But the transport's loop has to be
a region, set on purpose, with three modes in a segmented radio:
- **Stop at end:** plays the region once.
- **Loop region:** wraps at the out point. The region defaults to the
  *closing length* (below), not the longest track.
- **Run on:** the clock never wraps; every track keeps its own cycle forever.
  This is the "continuous" one. It never seams, and never exactly repeats
  unless the periods agree.

**The closing length.** Round every looping track's period (repeat: its
span, ping-pong: twice its span, cycle: its period, camera: its lap) to whole
frames and take the least common multiple. If that's within a few times the
longest period, it's the default region. If it isn't (periods of 7, 9, 11
and 13 s give 9009 s, which is your "waiting for the LCM and hoping"), say
so, and offer the fix: retime each track's period to the nearest divisor of
the longest. For sierpinski-anim that means moving 8.284 to 8.727, as in §0.
The timeline marks every lane that doesn't close at the out point, right at
the seam.

**Tempo grid: optional, as a DAW has it.** A scene can set `bpm` (and beats
per bar). The grid and snapping then follow bars, the counter reads
bars:beats beside seconds, and a region that is a whole number of bars,
with keys on the grid, closes by construction. This also answers Remy's
"camera paths timed to a beat".

**Procedural motion: add noise.** Rates and cycles cover `time*45` and LFOs.
What's missing is the one every tradition has: smooth random (`wiggle`, a
noise modifier, a random LFO). A `wave = "noise"` source with a seed and a
roughness, still moving about the authored value, is a small addition with a
large payoff on fractals.

## 4. The plan, in slices

Each slice stands on its own and is checkable. Your notes are mapped to them
in brackets.

### Slice 1: fix what's wrong
- ~~Track paths follow inserts and deletes.~~ Done: a deleted map's tracks go
  with it (undo brings them back), and subscene layers get the same treatment.
- `--info` notes when the timeline doesn't loop cleanly, naming the tracks,
  their periods, and the edit that closes them. The Timeline shows the same
  thing as a mark at each seam.
- Every view of the timeline (the window, renders, `--info`) uses the same
  closing-length rule.

### Slice 2: transport in the toolbar  *[rec arm, play/pause, RTZ, time, loop]*
- Record, play/pause, return to start, the stop/loop/run-on radio, and the
  counter in seconds + frames (bars:beats when there's a tempo). The counter
  can be typed into while paused: `12.5`, `300f`, `4:2`.
- Keys: Space/O/Z play (as now). ←/→ step a frame, Shift+←/→ jump to the
  previous or next key. Return to start is a button for now (Home is taken).
- `[timeline]` in the scene: `fps` (the render's default, so `--fps` becomes
  an override), optional `bpm`, the loop region. That's scene data because it
  changes what an animation render writes.

### Slice 3: the lanes  *[zoom/scroll, sticky ruler, clickable ruler, axis widget]*
- The ruler pinned above the scrolling lanes; click or drag it to move the
  playhead; notches with times that follow the zoom level.
- Ctrl+scroll zoom, shift+scroll pan, a key to frame all; while playing, the
  view jumps a page when the playhead runs off the edge (no smooth scrolling,
  per the zero-animation rule).
- Loop in/out points on the ruler, draggable.
- Lanes become DAW automation lanes: value is height, resizable, click the
  line to add a point, drag points in both directions.
- The axis cross sits above the Timeline when they would overlap. (Or the
  Timeline docks as a bottom panel, but that shrinks the viewport, which the
  house rules avoid. Hazel's call.)

### Slice 4: keys as objects  *[right-click add, key menu, snapping]*
- Right-click a lane: key here. Right-click a key: duplicate, copy, paste,
  delete, interpolation from here.
- Box-select across lanes; drag a selection; Ctrl+C/V at the playhead.
- A **summary row** at the top, and one per map: a diamond wherever any child
  lane has a key. Drag it to move them all. In sierpinski-anim, the four
  lanes sharing keys at 0, 8 and 8.28 become one drag.
- Snapping as in §2.5, with the toggles next to the ruler.
- **Markers**: named times on the ruler, which snapping, the CLI and agents
  can refer to.

### Slice 5: keying from the scene  *[3D params as vectors, gizmo keys, search by name]*
- **Whole-vector tracks** for translation, scale, focus, colour and
  background: keys hold `[x, y, z]`. Rotation is already whole (pose keys).
  The picker offers "whole" first and the axes underneath.
- A field-to-path table for every inspector field, tested against
  `params::catalogue`. Inspector fields edit `TransformSpec` and `App`
  directly and none of them knows its `-S` path; record gets round that by
  diffing the scene after an edit, but a diamond beside each field needs the
  link forwards. Keep the rules record already follows: one edit is one undo
  step, a key writes the authored value rather than the playhead's swapped
  view value, and a rotation keys the whole pose. Then the diamond beside each keyed field, and
  the insert key.
- Insert over a selected gizmo keys its whole position, rotation and scale.
- Record: auto-key while paused; touch while playing (§3).
- The picker already matches maps by name. Lane titles should too: "petal ›
  julia" with its colour swatch, not `transform.3.variations.julia`. A nudge
  to name maps when they're first animated would make the paths stable as
  well.

### Slice 6: morphs  *(the one new concept)*
See §5. It serves the GUI as much as the CLI: "remember this state", edit to
another state, and the timeline animates one dial between them. It's also how
to animate from the scene to a mutation-grid tile, which turns Jay's main
workflow into an animation in one click.

### Rotation tracks: spin and turn

*Moved from TIMELINE-PLAN.md §3 C. Pose keys (its option D) are built, and
`offset` on a pose covers some of this. A rotation that runs forever at a
rate, about a chosen axis, is still open.*

Two angle-valued targets that compose a rotation onto the map's authored pose
instead of overwriting one of its numbers:

- **spin**: about one of the map's own axes, through its own origin. The
  translation stays put. "Turn the petal about itself."
- **turn**: about a world axis through the origin (or the zoom fixed point,
  or a symmetry axis), moving the translation with it. "Carry the motif round
  the group."

These are exactly `--derive`'s `--spin` and `--turn` (`src/derive.rs`), whose
quaternion products are already written and tested. The value is one number
in degrees, so keys, rates, cycles, extrapolation, clamping, the picker and
the curve all work unchanged; a `rate` spins forever with no wrap and no
gimbal lock.

```toml
[[track]]
target = "transform.petal.spin.y"    # own y axis; or turn.x / turn.y / turn.z
rate = 45.0                           # degrees per second
```

What it needs:

- **Targets that aren't `-S` paths.** The animator applies the `-S` tracks to
  the text, parses, then multiplies the spin/turn rotations into the parsed
  frame's matrices (`M · R` for spin, `W · M` for turn), several on one map
  in a fixed, documented order.
- **The picker** offers "Spin about its own x / y / z" and "Turn about world
  x / y / z" under each map.
- **Symmetry**: a spin on a motif applies before the group, so every copy
  spins; a turn about the group's own axis is "the whole flower rotates".
- **Infinite zoom**: a spin or turn on the zoom map changes the similarity
  every frame, the same live re-derivation `App::refresh_zoom` already does
  when the map is dragged.
- **Class**: structure, like any matrix change.

### Not in this plan
Motion blur, carrying walkers between frames, and transparent image sequences
(Remy's gap; a VFX person would expect PNG sequences with alpha before
anything else here) are render-side work and belong in a render plan.

---

## 5. The CLI and agent side

### Could I have authored this animation today?

Yes, by writing the TOML directly. `-S` can't create tracks, so the CLI
alone can't do it. Writing the file would work, and these are the steps where
an agent would lose time or get it wrong:

1. **The seam.** I'd have written 8.28 or some other round-looking number,
   `--info` would have said `notes none`, and I'd have shipped the snap. To
   catch it I'd need to know to compare `--time 0` against `--time 52.36`.
   Nothing invites that. This is the most important fix on the agent side.
2. **The camera's arithmetic.** "Swing three quarters of a turn and back" is
   two keys today (yaw −2.19 at 0, yaw 2.52 at 26.18; the other fields
   inherit), but I'd have to work out the yaw values and remember that
   ping-pong doubles the length. The saver's habit of writing every field
   into every key makes the existing scenes worse examples to copy.
3. **Five tracks that are one idea.** The animation is "this variant turns
   back into the plain Sierpiński, and back again". In the file it's four
   weight tracks and a rotation track with identical timing, which have to be
   kept in step by hand. Retiming means editing five places.
4. **Index paths.** `transform.3` is what an unnamed map gets, and it breaks
   on a delete (§0).
5. **Seeing the motion.** `--time-grid` shows it to an agent with good vision.
   For Mote there is no text view of a curve: what the value is at a quarter,
   half and three quarters of the way.

### What would make it clean

**Morphs: a named state, and one dial towards it.** This is Blender's shape
keys, a DAW macro, and JWildfire's flame morph: the same idea, under three
names, in three of the four traditions. It's the concept handle this
animation was missing:

```toml
[[morph]]
name = "plain"             # back to the classic Sierpiński
set = { "transform.0.variations.spherefold" = 0, "transform.1.variations.hyperbolic" = 0,
        "transform.2.variations.polar" = 0, "transform.3.variations.julia" = 0,
        "transform.2.rotation" = [0, 0, 0] }

[[track]]
target = "morph.plain"     # 0 = the scene as written, 1 = fully "plain"
keys = [[0, 0], [8, 1], [8.727, 1]]
extrapolate = "pingpong"
```

Values in between interpolate (numbers linearly, rotations as rotations,
strings switch at the half). Several morphs add up. A morph is just `-S`
pairs, so it uses the address language that already exists, and
`scene_diff` already turns two scenes into exactly those pairs:

```sh
fracturize --diff scenes/sierpinski-anim.toml scenes/sierpinski.toml --as-morph plain
```

writes the `[[morph]]` block. In the app: "Remember as morph" from the
current edits, or from a mutation-grid tile, and the Timeline gets one lane.

This is a scene feature, so it goes through `ADDING-A-SCENE-FEATURE.md`. It's
the biggest single item here, and the one an agent would use most.

**Keys from the command line, in the `-S` language.** `-S path@T=value` keys
the value at T seconds, making the track if it needs to:

```sh
fracturize -s s.toml -S 'morph.plain@0=0' -S 'morph.plain@8=1' \
  --extrapolate morph.plain=pingpong --save-scene out.toml
```

`--save-scene` (the scene after every `-S`, written through the normal saver,
never over the input) is what lets Mote build an animation without writing
nested TOML tables, which is where small models break.

**`--info` that talks about motion.**
- A note when the timeline doesn't close, with the fix as a `-S` line:
  `-S 'transform.3.variations.julia@8.727=0'` (and the same for the other
  four, or one line for a morph).
- Each track gets a five-point value row (start, ¼, ½, ¾, end), so a reader
  without vision can see the curve's shape.
- The camera track gets its own entry in `tracks`, described in words: "yaw
  swings 270° and back over 52.4 s; nothing else moves".
- A note when a track uses an index path: "name map 3 so its tracks survive
  a delete", with the flag.

**Camera sources in words.** A camera track could take `orbit = { turns =
0.75 }` (or a yaw `wave`), so "turn three quarters and back" is one line
instead of two keys whose yaw an agent has to calculate. Lower priority than
the rest. Two keys work.

**The saver writes camera keys sparsely**, leaving out fields that equal
`[camera]`'s, so saved scenes are good examples to copy.

### Persona check
- **Atlas** gets morphs, the seam note, and `--save-scene`. Writing TOML stays
  first-class.
- **Mote** gets `@T` keys and `--save-scene` (no nested tables to write), the
  five-point value row, and a seam note whose fix it can paste. Nothing on
  this list costs Atlas.
- **Remy** gets bpm, markers, and closing loops. Their remaining gap (alpha
  sequences) is on the render side.

---

## 6. Decisions for Hazel

1. **The insert key.** Blender's is `I`, which here is invert mouse pitch (a
   preference, rarely toggled; the Controls window has it too). Move invert
   pitch to the Controls window only, or pick another key?
2. **Return to start.** Home is "camera to the selected map's fixed point".
   Make RTZ a button only, Shift+Space, or something else?
3. **Record.** One global button with auto-key when paused and touch when
   playing (§3), or Blender's auto-key only, without touch?
4. **The loop default.** Closing length when it's reasonable, else the
   longest track with the seams marked. Is that right, and what counts as
   reasonable?
5. **Morphs**: worth a new scene feature? The name "shape" is taken by
   `--info`, so "morph" is the proposal.
6. **Timeline docked or floating**, given the axis-cross overlap.
7. **Tempo** in the scene (`bpm`): wanted now, or later with Remy?
8. **Sierpinski-anim itself**: it's your file, so it's untouched. The
   one-number fix is in §0 if you want it.
