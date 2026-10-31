# Feature plans: exploration, multiview, render jobs, time

Hazel's list of 2026-10-19, written up as plans. Fourteen and a half items (mute/solo added as 14), but
they fall into four tracks, and inside each track the items share most of their
machinery. Every plan below says what it reuses, what it needs that doesn't
exist yet, and which questions are Hazel's to answer.

| Track | Items | Shared machinery |
|---|---|---|
| A. One generator | 1, 3, 4, 5 | random batches and mutation unified: one settings profile, one generate-and-check loop, one list model |
| B. Many views | 2, 14 (and 3's grid) | a pane layout whose panes are cameras on the scene or scenes of their own; a per-point "which map" view |
| C. Render jobs | 6, 7 | a job spec that can be saved, read back out of a PNG, and queued |
| D. Time | 8–13 | one track model (key, interpolate, extrapolate) aimed at `-S` paths |

The last section is the answer to "am I missing obvious combinations".

---

## Track A — one generator: random batches and mutation, unified

**The key idea (Hazel's):** mutation stops being a separate system. Random
batches already have the richer half: a full settings profile
(`RandomSettings`), a rejection loop with a try budget, rejections counted by
reason, worker-thread rolling, a growing list with live thumbnails, and seeds
that reproduce from the CLI. Mutation (`src/mutate.rs`) has 1–3 hardcoded
operators, one strength, no settings, no check that the result renders, and
nothing for symmetry, xaos or zoom. Unifying them means mutation inherits all
of the first list and the second list goes away.

### What exists, and gets reused rather than rewritten

- `randomize::roll()` — `build_candidate` → `steer_dimension` →
  `measure_candidate` (`trace::measure`) → lacunarity → `finish`, inside a
  loop of `settings.attempts`, counting `Rejections` by reason and returning a
  `RollFailure` with the last candidate when the budget runs out.
- The part generators inside `build_candidate`: `random_transform`,
  `roll_weight` (variation weights from the enabled list and its ranges),
  `apply_symmetry`, the transform colour scheme, `palette::random`, the
  background roll in `finish`.
- `random_list.rs` — rolls on 2–3 worker threads, generations so results
  rolled under old settings are dropped, "stops after eight empty rolls in a
  row and offers Keep trying".
- `gpu/thumbs.rs` / `ThumbCache` — live thumbnails keyed by `ThumbKey`, shared
  by the scene browser and the Random flames list; converge once then a still;
  only the hovered one flies its camera.

### 1. One settings profile, save/load, read by both

**Built** (2026-10-24, `src/dice/`; AGENTS.md "The dice"). As planned, plus
`--random-sheet N` (the CLI batch sheet of fresh rolls) and two shared
profiles in `scenes/dice/`. "Revert to defaults" is loading the built-in
`default` profile; Revert goes back to the loaded one. Composites mutate per
layer and are gated per layer; placement isn't a part yet.

**Reshaped the same day** (Hazel: "there is a profile of things doable, and
it's the same for both"): no roll half and mutate half. One list of parts,
each on or off; a roll draws every part that's on and keeps the base scene's
for the rest, a mutation changes a few that are on. Built-in profiles
(`default`, `anything`, `no-symmetry`, `colour-only`, `no-colour`,
`symmetric`) are compiled in; the picker sits in the Random flames list, the
Mutate window and Explore. AGENTS.md "The dice" is current.

1. **`RandomSettings` grows a `[mutate]` half**, so one file says what the
   dice may do, fresh or from a parent. Everything a roll can produce is a
   *part* (maps, variations, symmetry, colour, palette, background, and from
   4 and 5 xaos and zoom), and for each part the mutate half says how often a
   mutation touches it and how: **nudge** (perturb what's there, scaled by
   strength) or **re-roll** (replace that part using the very generator a
   fresh roll uses, under the same ranges). Re-roll is where the sharing is
   deepest: "give map 2 new variations" *is* `roll_weight` over the enabled
   list, and "new palette" *is* `palette::random`. The existing `mutate.rs`
   operators become the nudge half and move into `randomize.rs` (or a
   `dice/` module holding both).
2. **One loop.** `roll()` becomes `generate(settings, source, rng)` with
   `source` either `Fresh` or `From(parent)`. Both run the same checks, count
   the same `Rejections`, spend the same budget and fail with the same
   `RollFailure`. `roll_seeded` / `mutate_seeded` are the two thin entries,
   and both name their seed.
3. **Named profiles.** `~/.config/fracturize/dice/<name>.toml` plus
   `scenes/dice/` for shared ones; the Random settings window gets load / save /
   save as / revert to defaults, with a `*` when changed since loading.
   prefs.toml keeps the current profile and which one it came from.
   `--random-config <name|file>` works for `--random` and `--mutations` alike.
   Mutation strength moves into the profile; the Mutate window's slider edits
   it.

**Rejecting without starving.** Reject, as Hazel says, so long as something is
still found. What already does this for rolls: the try budget, the
eight-empty-rolls stop with Keep trying, and the cheap dimension steer that
removes the commonest rejection. Two things mutation needs on top:

- **Judge a mutation against its parent, not against absolute bounds.** A
  hand-authored scene can sit outside the profile's dimension or lacunarity
  bounds, and every child of it would then fail forever. Rule: a bound the
  parent meets, the child must meet; a bound the parent breaks, the child may
  not break *further*. The shape check (bounded, real extent, fractal
  occupancy) always applies.
- **Back off strength when rejecting.** If a run of attempts is rejected, halve
  the strength for the next ones (an adaptive step, like an MCMC proposal), and
  report it ("strength backed off to 0.25 to find these"). A strong mutation of
  a delicate scene then still returns something close, instead of nothing.

Dimension steering stays for fresh rolls. For mutations it's off by default —
it rescales every map, which is a bigger change than the mutation — but it's a
profile switch, since for a "keep d where it was" workflow it's exactly right.

### 4. Xaos in the profile

**Built** (2026-10-25; AGENTS.md "The dice"). A part, `xaos`, drawn as one of
none / a preset / a random graph by odds; `default` draws none and no die for
it, so its seeds hold; the new built-in `xaos-fun` rolls it six times in
seven. Differences from the plan below: `cycle` can't be rolled (one exit a
map collapses the picture to dots), `no-self-loops` needs 3 maps and
`blocks:N` 2 a block; the dimension *is* steered under xaos, by bisection;
the starvation floor is relative to a map's own weight share. Linked map as a
mutation is left for the count part.

XAOS-PLAN.md §6, as profile settings:

- **Fresh:** chance of xaos; sparsity; no-self-loop chance; preset odds.
  **Strongly connected by construction**: seed a random Hamiltonian cycle,
  cut only edges off it.
- **From a parent:** nudge a multiplier (in log space), cut, restore, apply a
  preset, linked map (`insert_linked_transform` exists).
- **Check:** reject if any map's walk share is under a floor; dimension by
  Mauldin–Williams (`xaos.rs` has both). No dimension steer under xaos.

### 5. Infinite zoom in the profile

- **Fresh:** chance of a zoom scene; one map rolled as the zoom map — pure
  affine, contracting, zero translation, near-uniform, with a rotation; maybe a
  one-key `path_loop = "zoom"` path so it opens falling.
- **From a parent:** the zoom map only takes rotation and uniform-scale nudges,
  never variations; other maps mutate freely.
- **Check:** `Renorm::build` succeeds, `Renorm::defect` under a threshold, and
  with xaos every map that fires may be followed by the zoom map (a load error
  today, so generation must guarantee it).
- Thumbnails must frame zoom scenes inside the band; check `frame_setup`.

### 3. The mutation grid: 3×3 and 5×5, click to accept, recurse

*Built 2026-10-26 as the Mutate window (`src/mutation_grid.rs`, AGENTS.md "The
mutation grid"): points 1, 2, 3 and 5, with masks as profiles (4). Ctrl+Z gets
the old grid back by remembering a seed per parent rather than by a history
entry carrying seeds. Not built: the onion skin (6), and the grid living in
multiview panes (it has its own layout until those exist).*

The Random flames list *is* this feature with a different source: rows of
gated generations with live thumbnails, rolled off-thread, click to open.
So the grid is the same model (`random_list.rs`, generalised to take a
`source`) shown in a different layout, and from item 2 that layout is the
multiview pane grid.

1. **Centre is the parent**, the ring is 8 (3×3) or 24 (5×5) generations
   `From(parent)`. In 5×5 the inner ring uses the set strength and the outer
   ring double, so one grid shows near and far.
2. **Click a tile** → it becomes the scene, as one history entry naming its
   operators and seed; the ring regenerates around it. Ctrl+Z restores the
   previous parent *and its grid* (the entry carries the grid's seeds), so you
   can pick a sibling instead.
3. **Hover flies that tile's camera**; nothing else moves. Every tile uses the
   parent's camera, so the comparison is of form.
4. **Masks** are profiles: only the selected transform, only colour, only
   variations, only xaos — the mutate half with other parts zeroed. With
   mute/solo (item 14) a soloed set of maps is a natural mask.
5. CLI twin: `--mutations 8|24` lays out 3×3 / 5×5 with the parent centred,
   so an agent sees the sheet the window shows.
6. **Where layers help.** A tile doesn't need to be a subscene (tiles have
   their own cameras, which layers don't, and 5×5 is past `MAX_LAYERS` = 16).
   But an *onion skin* does: the hovered child with the parent drawn as a dim
   second layer behind it, in the main view, which is exactly a two-layer
   composite. That shows *what moved* where side-by-side tiles only show
   *what it looks like*.

---

## Track B — many views

### 2. Multiview, and the pane grid it's built on

> **Status, 2026-10-31 (Opus 5.5): built and accepted.** Three layouts
> (one view, four views with the perspective top-left as Hazel asked, one big
> and three small: `T`, or the toolbar's radio), a perspective pane and three axis panes on the one point buffer,
> each with its own axis cross; gizmos pick and drag in any pane. Two of the
> decisions below went this way: the axis panes are **pickable** (click a ball
> on a pane's cross; defaults top / front / right, front being +z where the
> default camera stands, not −z), and the layout is a **preference**. Point 1
> went another way: the axis panes are a 1° long lens, not an orthographic
> projection, because every shader reads `clip.w` as eye distance (AGENTS.md
> "Multiview"). Not done: the mutation grid in these panes (it stays its own
> layout), per-pane renderer, a pane per scene.

Two jobs for one piece of machinery:
- **better 3D perspectives while tweaking**: perspective + +x / +y / −z, as a
  2×2 or one big view with three stacked on the right;
- **the mutation grid**: 3×3 / 5×5 panes, each a different scene (item 3).

**A pane has a rect and a source.** Two kinds of source:

- **a camera on the main scene.** The point buffer doesn't depend on the
  camera, so these share one chaos pass and each costs a render pass;
- **a scene of its own.** Its own small `PointCompute`, which is what
  `ThumbCache` already builds (256 walkers, one-shot convergence, hover to
  fly) — just at pane size rather than 96 px, so with a ring scaled to the
  pane's area (`thumbnail_study` measured 8k points reading well at 96 px).

So multiview is the pane layout plus the first source, and the mutation grid is
the same layout with the second. The browser and Random flames list keep their
own rows; they and the grid share the thumbnail cache and its rules.

**What the camera-on-scene panes need.**

1. **An orthographic camera** for the axis views — `camera.rs` is perspective
   only. In an ortho side view, moving along the view axis doesn't move
   anything on screen, which is the point. `Projection::{Perspective, Ortho}`,
   with ortho branches in point sizing, `use_point_primitives` and haze's
   auto band.
2. **Per-pane picking**: `pick.rs` is screen-space projection, so it only needs
   the pane's camera and rect. Dragging the origin dot in an axis pane moves it
   in that plane exactly.
3. **Per-pane exposure** (splat normalises by resolution). Volume mode is five
   passes; probably the main pane only.
4. **Navigation**: orbit in the perspective pane; pan and zoom in axis panes;
   their focus and scale follow the main camera so all panes show the same
   region.
5. **Infinite zoom**: axis panes scale by the same `A⁻¹` as the main camera at
   each wrap, or they'd jump a period every wrap.

**Decisions for Hazel.** Axis panes fixed to +x / +y / −z, or each pickable?
Layout as a preference (like panel geometry) or per scene?

### 14. Transform mute/solo views

**Built 2026-10-30** (branch `mute-solo`): M/S on the Transforms rail, live,
through a per-point map record. See AGENTS.md "Mute and solo". Edge mode and
a render/view key are not done.

**Not the same thing as enable/disable, and the UI has to say so.** The rail's
eye (and alt+click solo) *enables and disables* a map: the map leaves the IFS
and the attractor changes. Mute/solo is a *view*: the IFS is untouched and
you see only the points certain maps placed — `--isolate`'s partition (a point
belongs to the map that moved it last; the parts add up to the picture),
generalised from one map to any set, and live.

- **Mixer semantics**, which also match the audio mixer of 12.5: per map a
  mute (M) and solo (S); any solo → show only soloed; else show all but muted.
  Edge mode (`from>to`) carries over from `--isolate`.
- **Mechanism.** `--isolate` drops points at write time, so a live change
  would take the ring's ~13 s to show. Instead keep each point's last map in a
  side buffer while mute/solo is in use (the way emphasis keeps its side
  buffer), and let the vertex stage clip against a per-frame mask: changing the
  mask is then instant. Point is 16 bytes with no room left in mix mode
  (colour uses the spare 24 bits), hence the side buffer; allocate it only
  while a mute or solo is set and let it fill in at the fill rate, as emphasis
  does.
- **Exposure stays normalised to every point**, as `--isolate` does, so a
  soloed part is at the brightness it has in the full picture.
- Builds on emphasis too: emphasis *brightens* the selected map's material;
  solo *hides everything else*. Same sensitivity to "which map placed this".
- Session state, not scene data, and not undoable — like orbiting. Possibly a
  view-file key so a render can be made of a part (`--isolate` already is that
  for one map).
- Works with multiview (soloing applies to all panes) and as a mutation mask
  (item 3).

---

## Track C — render jobs

### 6. Render job presets, save/load, and load from a PNG

**Where it stands.** `JobParams` (`src/render_job.rs`) is the job. The dialog
has hardcoded size presets only. Every still already carries
`fracturize:render` in the PNG (`src/record.rs`), and the `.render.toml`
sidecar holds the same block.

**Plan.**

1. **A preset is the artistic and output part of `JobParams`**: size,
   samples/effort, renderer, exposure, supersample, filter, radius, bit depth,
   density estimation, transparency, format, fps, quality, and (later) grade.
   **Never** machine settings — `threads` and `gpu_share` stay prefs. A preset
   read out of a PNG made on the T490 mustn't tell the desktop how many cores
   to use. AGENTS already holds that line for sidecars.
2. Stored as `~/.config/fracturize/render-presets/<name>.toml`, the same
   format as the record's `[render]` block, so **loading from a PNG is reading
   that block** (and from a `.render.toml` sidecar for free). Unknown or
   machine keys in a record are ignored with a note.
3. Dialog: preset dropdown at the top, save / save as / delete, a `*` when the
   fields differ from the loaded preset. Ship a few built-ins (preview, 1080p
   still, 4K still, A2 print, 1080p loop mp4, square social loop).
4. CLI: `--preset <name|file|png>`, with explicit flags winning over it, field
   by field — the same ladder `--view` uses.
5. **Apophysis as reference**: its render dialog has a preset dropdown with
   save/delete, and size / quality / oversample / filter radius / output format
   / "maintain aspect". Worth opening it under wine before building the UI,
   mostly to copy the *placement* (preset first, then size, then quality) and
   to note what it got wrong.

**Decisions for Hazel.** Does a preset carry the output *path pattern*
(`renders/{scene}-{preset}.png`)? (Suggest yes, it's what makes a queue of
presets useful.) Separate size presets from quality presets, or one bundle?

### 7. Render job queueing

1. **A job snapshots its scene.** Today one job runs and the scene can't
   change underneath it in a way that matters. In a queue it can: you queue a
   render, keep editing, and the render must be of what you queued. So a queued
   job holds the scene **TOML text** at enqueue time (`Scene::to_toml_string`,
   which the record already uses), and the camera.
2. A queue panel in the Render job window: list, reorder, remove, pause the
   queue, per-job progress, done/partial/failed. One job runs at a time on the
   second device (unchanged); the rest wait.
3. **Persisted** to `~/.config/fracturize/queue.toml`, so a queue survives a
   restart. A partial job resumes from its checkpoint when it has one
   (`--checkpoint` / `--resume` exist).
4. **CLI**: `fracturize --queue jobs.toml` runs a file of jobs headlessly, so
   agents can batch, and so a GUI queue can be handed to a terminal overnight.
5. Sources that feed it: "queue this" from the dialog; "queue selected" from
   the mutation grid and the Random flames list; "queue with preset X" from the
   scene browser on many scenes.

---

## Track D — time

The biggest track, and the one where getting the model right first saves the
most. Order: **9 → 8 → 10 → 11 → 12 → 12.5 → 13**.

### 9. Animation kinds, and the camera rebuilt on them

> **Status, 2026-10-26 (Opus 5.5): built, short of the file-format move.**
> `src/track.rs` has the one `Track` (keys / rate / cycle, extrapolation
> hold / repeat / pingpong / offset). The camera shares the time model: its
> `Loop` *is* an extrapolation (`Loop::extrapolate`, zoom loop =
> repeat-with-offset), and it is sampled on the same clock in seconds as every
> track. What is **not** done is moving `[[camera.path]]` into `[[track]]`
> form: it is a format change to every scene with a path, which this plan
> itself makes one explicit migration commit, and nothing built so far needs
> it. The views-as-keys decision below is still open. See AGENTS.md, "Tracks
> and the timeline".

**The core idea.** One `Track` type: a target, a source of values over time,
and what happens past its ends.

- **Source** (Hazel's "kinds"):
  - *keyframed*: keys with per-key interpolation (step, linear, smooth,
    Catmull-Rom, and for angles and framings the cumulative form `path.rs`
    already uses);
  - *linear*: a rate (units per second), for things that just run — hue
    rotation, a zoom descent, an orbit;
  - *cyclic*: an oscillator — sine, triangle, saw — with period, phase,
    amplitude and centre. Two of them, 90° apart, on the camera's x/y is the
    sin/cos default Hazel describes.
  - later: *audio* (12) and *data* (a recorded curve).
- **Extrapolation** (what the camera's `Loop` was): hold, repeat, ping-pong,
  and **repeat with offset**. Repeat-with-offset is Blender's "cycles with
  offset" modifier, and it's exactly what `path_loop = "zoom"` already is:
  repeat the segment, but carried by the zoom symmetry so each lap starts where
  the last ended, one period lower. Naming it that way turns the zoom loop from
  a special case into a general one.
- **Easing** stays per track (`path_ease`).

**The camera on top of it.** The camera is the one target that can't be three
independent scalar tracks, because framings are quaternions and `path.rs`'s
routes exist precisely because angles don't interpolate. So the camera is a
*framing track*: its keyed form is today's `CameraPath` spline, routes and all;
its cyclic form is the new default; extrapolation is shared with every other
track.

- **Default camera**: cyclic, and **behaviourally identical to today's
  turntable**. Today's default is `CameraPath::full_orbit`: the base framing
  turned about world Y through the focus, one turn per `TAU / ORBIT_RATE`
  (~35 s). Written as a cycle with `θ = ORBIT_RATE · t` and `d = eye₀ − focus`:

  ```
  eye.x = focus.x + d.x·cos θ + d.z·sin θ
  eye.z = focus.z − d.x·sin θ + d.z·cos θ
  eye.y = eye₀.y                       (constant)
  orientation = R_y(θ) · orientation₀
  ```

  So yes: a sin/cos pair, plus the look direction. Two details. The plane is
  **x/z**, because Y is up here, so a sin/cos on x/y would be a vertical loop.
  And the look direction should come from **turning the base framing**
  (`orientation₀.then_world(Turn::about(Y, θ))`, which is what `full_orbit`'s
  keys already are) rather than from a look-at with world-up. The two agree
  everywhere except straight up or down, where a look-at has no defined roll and
  the turned framing is still exact; it also carries the framing's roll through
  unchanged. Against the current four-key spline it's mathematically
  identical (Catmull-Rom through evenly spaced yaw is exactly linear), so
  renders may differ in the last bit of float, not visibly. The port's test is
  `full_orbit` sampled against the cycle at many `t`, to a tolerance.
- **Adding a view (Y, or V) switches the camera to keyframed**, the first key
  being where the cycle was at that moment, so nothing jumps.
- **Porting**: the loader keeps reading `[[camera.path]]`, `path_loop`,
  `path_seconds`, `path_ease`, `path_zoom_periods` and legacy
  `path_closed`/`path_zoom_loop` forever, and Ctrl+S writes the new form —
  the same treatment `offset` and `fog` got. Scenes in `scenes/` are their
  authors' (memory rule), so the bulk port is **one explicit migration
  commit** run through the loader and saver (comments preserved by
  `toml_edit`), with a before/after `--info` diff and a render of every scene
  with a path, not a side effect of some other change.

**Decision for Hazel.** Do saved views (`views/*.toml`) become keys, or does
only Y add keys?

### 8. A timeline window, wide and low

> **Status, 2026-10-26: a first cut.** Transport, ruler, playhead, scrub,
> loop, a camera row (its keys at the moments it passes them), a row per
> track (keys as diamonds coloured by class, the value as a faint curve),
> mute and delete; select a track to edit its source in a side column, key at
> the playhead, drag keys to retime, type a key's value, add a track on any
> `-S` path. Not yet: keying from the inspector (auto-key), zooming the
> ruler, the curve editor proper. The transport is a clock plus "is the camera
> following", so grabbing the camera leaves an animation playing.

A floating window docked above the status bar, full width, about a fifth of
the height (it's `egui::Window` like everything else, so `default_layout`
places it and prefs remember where you moved it).

- Ruler in seconds (and frames at the render fps when zoomed in), a playhead,
  play/pause/loop-region, scrub by dragging. Zoom and pan the ruler with the
  same gestures the viewport uses.
- One row per track, grouped (camera, transforms by name, colour, render,
  audio). Keys as diamonds, drawn in the track's colour. Drag to retime, alt to
  copy, click to select and edit the value in the inspector. Cyclic and linear
  tracks draw their curve faintly behind the row.
- **Zero animation rule still holds**: the playhead moves because time moves,
  which is content, not chrome. Nothing eases.
- A **curve view** per track (value against time), the thing Apophysis never
  had and JWildfire's "motion curves" do have.

### 10. Animating (almost) every scene parameter

> **Status, 2026-10-26: built as written**, addressing by `-S` path, with the
> four classes, per-frame refills offline (in place, same seed), fill-rate
> churn live, `--time`, `--time-grid` and an `--info` `tracks` section.
> Decided without Hazel, and easy to reverse:
> - **Editing a tracked value edits the base, no auto-key.** For a rate or a
>   cycle the base is what the motion moves about, so that edit is the useful
>   one; for keys it shows only where the keys don't reach. Auto-key waits for
>   key authoring in the GUI.
> - **A track whose path doesn't resolve is skipped and reported, not a load
>   error**, like an xaos dead end: deleting the map a track moves made saved
>   scenes unopenable.
> - Not done: `--key`, `--plot`, `-S track.*`, motion blur, carried walkers.

**Addressing: reuse `-S` paths.** `src/set.rs` already names every scene value
(`transform.whorl.variations.swirl`, `subscene.smoke.translation.y`,
`meta.haze`, `transform.a.xaos.b`). A track targets one of those strings. One
address language for the CLI, sweeps and animation, and `--sweep` becomes
"sample this track at N points". In the file:

```toml
[[track]]
target = "transform.whorl.rotation.y"
keys = [[0.0, 0.0], [4.0, 90.0]]      # [seconds, value]
interpolate = "smooth"
extrapolate = "pingpong"

[[track]]
target = "palette.rotate"
rate = 0.05                           # linear: per second
```

**Not every parameter costs the same, and the engine has to know which is
which.** This is the most important engineering fact in the track:

| Class | Examples | Per-frame cost |
|---|---|---|
| view | camera, point size, exposure, haze, grade | none: re-render |
| colourmap | palette stops, rotate, contrast, transform colours (transforms/palette modes) | re-upload 256 entries |
| placement | subscene translation/rotation/scale | applied at write, so points re-form over a ring lap |
| structure | matrices, variations, weights, xaos, symmetry params, zoom map | the attractor itself changes |

**Live**, structure tracks run into the ring: it takes ~13 s to turn over, so
an animated attractor in the window leaves a 13-second ghost of where it was.
While playing, churn at the warmup rate (10× faster) and accept more grain, or
show the ghost as a deliberate "persistence" look. Probably both, as a
setting.

**Offline**, animation today is one chaos fill plus a cheap render per frame.
A structure track needs the buffer **refilled per frame**. Walkers can be
carried over from the previous frame (the attractor has barely moved, so they
re-converge in a few iterations instead of a burn-in), and the cost is a fill
per frame: 0.08 s for 40M points on the desktop. So that's fine, but the render
job estimate has to know which class the scene's tracks are in.

**Motion blur falls out of this.** If the frame's fill spans the shutter
interval — each walker's iterations spread over sub-frame times — the points
are written by the attractor as it moves through the frame. That's flam3's
temporal samples, for close to free.

**The map set is fixed over an animation; enable/disable is the track
instead.** No adding or removing transforms on the timeline. A map that should
appear or vanish exists throughout and has an `enabled` track (a step: on or
off at a time). Enable is discontinuous in shape: the attractor of an IFS is
the same set for any positive weights, so the shape changes only at the
instant a map turns on or off. For a smooth entrance, pair the step with a
**weight or xaos-multiplier** ramp: weight up from near zero after enabling,
down to near zero before disabling. Density fades, and the step lands where
there is almost nothing left to see. The track editor can offer that pair as
one "fade in / fade out" action. Changing a symmetry group or the variation
list's shape is likewise a step, never interpolated. Keeping the map set fixed
also sidesteps the colour-ring defect (adding a transform moves every
colour).

**CLI, for agents** (who can't watch video):
- `--t <seconds>` (generalising `--path-t`): any instant as a still.
- `--time-grid 4x3`: a contact sheet over time, each tile labelled with its
  time. That's how an agent reads an animation.
- `-S track.<n>.keys=...` and `--key <path>@<t>=<value>` to author keys
  without editing the file; `--info` gains a `tracks` section (target, class,
  range, extrapolation, and notes like "structure track: refills every frame").
- `--plot <path>`: the track's curve as a small PNG.

**Edits**: every track and key edit is a `commit_edit` and lands in history.
Editing a keyed parameter while playing raises the question of auto-key
(Blender) versus editing the base value. That's Hazel's call.

### 11. A timeline reference audio track

- **Input is `.wav`.** `hound` reads it, pure Rust and tiny. Other formats
  can come later through `symphonia` or by converting with ffmpeg on import;
  nothing downstream cares, since everything after the decoder is PCM. Play with
  `cpal`.
- **The audio clock is the master clock while it plays**: the frame time
  follows the audio position, never the other way round, or the picture
  drifts against the sound.
- Waveform in the timeline (a min/max pyramid computed once per file, so
  zooming is free). BPM, a beat grid, and **snap keys to beats**: the obvious
  partner to any music.
- In the scene: `[[audio]] file = "song.flac"`, relative to the scene like
  subscene `file =`, with offset and gain. A missing file opens the scene with
  a notice rather than refusing it, but refuses to *render* anything that
  depends on it.
- The render record names the audio file and its sha256, not its bytes.

### 12. Animation that follows audio

**A modulator is a track source.** It turns audio into a 0–1 curve, and the
track maps that onto a target: range, curve, smoothing.

- **Analysers**: band envelope (bandpass, then an attack/release follower,
  linearised), overall RMS/peak envelope, transient → decay envelope (onset
  detection firing a decaying pulse), beat/onset *counter* (a step that goes up
  by one per beat — drives discrete things like "next palette stop" or "next
  mutation").
- **Value or rate.** A modulator can drive a value, or a *rate* that gets
  integrated. This matters more than it sounds: drive zoom depth or orbit angle
  from amplitude directly and the camera jitters back and forth with the music;
  drive their *speed* and it surges forward on loud passages and never goes
  backwards. Same for hue rotation.
- **Offline is deterministic**: analysis runs on the decoded file at a fixed
  analysis rate (say 1 kHz envelope samples), independent of render fps, and is
  cached beside the file. A render reproduces.
- **Realtime reactive** (mic or loopback via `cpal`): not reproducible as it
  stands, so offer **record**, which writes the live envelopes as data tracks.
  The performance becomes keyframes, and a render can reproduce it.
- **Output is AAC in the `.mp4`.** There's no good pure-Rust AAC encoder, so
  encode outside: **ffmpeg's built-in `aac` encoder as a subprocess** (it's
  installed on the desktop; check the T490), fed the mixdown as PCM on stdin,
  writing a raw ADTS stream that our muxer reads frame by frame. `video.rs`
  gains a second `trak` (`mp4a` with an `esds` box) and keeps control of the
  layout — `moov` first, which the upload pipelines care about. The ADTS
  headers give frame boundaries and the AudioSpecificConfig, so the muxer needs
  no AAC knowledge beyond that. FFI to `fdk-aac` is the alternative if a
  subprocess turns out to be a problem, at the cost of a C dependency and its
  licence. No ffmpeg on the machine means a render with audio refuses by name
  rather than silently writing a silent video. AVIF can't carry audio, so an
  audio render is `.mp4`.

### 12.5. Multi-track audio

- Import stems or several tracks; a small mixer per track: gain, pan, mute,
  solo. The mixdown is what plays and what gets muxed.
- Modulators read **any track or any sum of tracks**, so the kick stem drives
  the zoom and the vocal drives the haze.
- **Pre-fader analysis by default**: turning a stem down in the mix shouldn't
  change the picture. Post-fader as an option. A muted stem can still drive a
  modulator, which is a useful trick (a click track nobody hears).
- Multichannel files: each channel can be a track in its own right.

### 13. Video import, on the timeline and behind the fractal

- **Decoding** is the hard dependency. `openh264` (already a dependency)
  decodes H.264, and the muxer's ISOBMFF knowledge runs backwards into a small
  demuxer, which covers most phone and editor exports. AV1 decode needs
  `dav1d`. Anything else means ffmpeg, which this project has avoided so far.
  Suggest: H.264 MP4 first, and say plainly what's unsupported.
- **On the timeline**: a strip of frames sampled at the ruler's zoom (one per
  N seconds when zoomed out, every frame when zoomed in), decoded lazily and
  cached at thumbnail size.
- **Behind the fractal**: the background stops being a clear colour and
  becomes a texture. That touches more than it looks like it should:
  - the splat tonemap composites against `background` → sample the texture;
  - points-mode haze mixes toward the background colour → per-pixel sample;
  - volume mode's `bg · (1 − coverage) · T_total` → same;
  - the live window too, not just the render.
  It's the same work as the todo list's "bg env render / hdri", so do it once
  for still images, video frames and (later) an environment map.
- **Layered on a transparent render**: `--transparent` already writes alpha.
  For video, composite the fractal over the video frame at encode time, since
  neither codec carries alpha.
- Its own audio comes in as an audio track (12.5).

---

## What's missing: combinations and interactions

### Combinations worth building on purpose

1. **Mutation lineage → animation.** Every tile accepted in the 3×3 grid is a
   scene; the chain of accepted parents is a set of keyframes. Interpolate
   between them and the exploration session *is* the animation — Apophysis's
   "animate between flames". It needs **scene interpolation**: match transforms
   by name or index, interpolate matrices through TRS (never lerp the raw
   matrix, which shears and so can't be saved), variations linearly, xaos
   multipliers in log space, colour in Oklab. A structural difference (a map
   added by a mutation) appears by weight from zero. This joins track A to
   track D and is probably the most *fracturize* thing on the list.
2. **Audio → mutation.** A beat counter (12) stepping through a mutation
   lineage, or through a 5×5 grid's tiles, so the form changes on the beat.
3. **Audio → infinite zoom rate.** A zoom descent whose *speed* follows the
   music (a rate modulator). The zoom is periodic and wraps, so it can surge
   forever without leaving f32. That's a combination few other tools can do.
4. **Random rolls of animations.** The generator profile (1) gains a "roll tracks" part:
   a cyclic track on one map's rotation, a palette rate, a camera cycle. Random
   flames that move.
5. **Multiview while animating.** Axis panes show the structure track's effect
   in plan and elevation as you scrub. That's how you key motion precisely.
6. **Mutate grid of animations.** Mutate tracks (period, amplitude, phase)
   instead of form, shown as hover-flying thumbnails.
7. **Queue + presets + mutation grid.** "Render all five of these tiles at
   the 4K preset tonight" is three features meeting in one button.
8. **Xaos as the fade.** Animating a multiplier 1 → 0 morphs one sub-attractor
   into another smoothly, the way `--sweep` already shows. It's the tool for
   fading parts of a scene in and out, so it belongs in the track UI's
   suggestions.
9. **Subscene placement and share over time.** Two attractors drifting past
   each other, one fading in by share. Cheap (placement class), and composites
   exist.
10. **Video frame → palette.** Sample a frame's colours into the gradient, per
    frame or once. Video and colour meet without the fractal needing to know.

### Interactions with existing features that will bite

- **The point ring vs. animation** (above): live structure animation ghosts
  for 13 s; offline it needs per-frame refills, and the job estimate must know.
- **Volume mode refuses animations**, and `--spp` refuses animations. Per-frame
  refill makes per-frame accumulation possible, so both refusals become worth
  revisiting, but until then animated scenes render in splat or points.
- **Infinite zoom + anything per-camera**: multiview panes, and animated
  zoom-map parameters (the renorm rebuilds and the wrap carry has to be
  re-derived mid-flight, as `App::refresh_zoom` already does when a map is
  dragged).
- **Render records** gain tracks (in the scene TOML, automatically), and media
  references with hashes. The PNG of one frame of an animation should record
  its time `t`.
- **Determinism**: chaos seed per frame, and walkers carried from frame to
  frame, must stay reproducible from the scene plus flags.
- **History and the dirty bit**: key edits are edits. Scrubbing the playhead
  isn't (it's view state, like orbiting).
- **Prefs vs. scene vs. machine**: dice configs and render presets are
  person-level (prefs folder); tracks and audio references are scene data;
  threads and GPU share stay machine. Each new setting should say which class
  it is, as the panels already do.
- **`--info` goldens**: every new section only prints when the scene has the
  feature, so existing goldens don't move. That's what xaos did.
- **Subscene + xaos/zoom constraints** carry into the dice: no zoom in a
  composite, xaos stays inside a layer. Mutation of a composite picks a layer
  (or placement).
- **Colour ring defect**: rolls and mutations that duplicate or delete a map
  shift every colour in `transforms` mode. The grid makes it visible across
  tiles, so it may finally force the decision recorded in todo.txt.

### Smaller things not on the list, that the list implies

- **Orthographic camera** (needed by 2) is useful on its own for flat
  "technical" renders.
- **Scene interpolation** (combination 1) is useful on its own:
  `--morph a.toml b.toml` as a sweep sheet.
- **Markers** on the timeline (named times), which audio sync wants anyway.
- **An environment background** (todo: "bg env render / hdri") shares the
  background-texture work with 13.

---

## Suggested order

1. **Item 1: unify mutation into the random generator** (one profile, one
   generate-and-check loop, judged against the parent, strength backs off),
   then **3** on the Random flames list's model. It fixes the existing `U` key
   on the way. 4 and 5 slot in afterwards as parts of the profile.
2. **14 (mute/solo)** is small and independent, and wanted by both 2 and 3.
3. **6 → 7.** Self-contained, and most of the plumbing (records, job params)
   is there.
4. **2**, after an orthographic camera; the mutation grid moves into its panes once
   they exist.
5. **9 → 8 → 10**, then audio (11 → 12 → 12.5), then 13. The track model
   should be settled before any timeline UI is drawn.
