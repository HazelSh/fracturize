# Feature plans: exploration, multiview, render jobs, time

Hazel's list of 2026-10-19, written up as plans. Thirteen and a half items, but
they fall into four tracks, and inside each track the items share most of their
machinery. Every plan below says what it reuses, what it needs that doesn't
exist yet, and which questions are Hazel's to answer.

| Track | Items | Shared machinery |
|---|---|---|
| A. The dice | 1, 3, 4, 5 | one generator spec that both rolling and mutating read, and one quality gate both pass through |
| B. Many views | 2 (and 3's grid) | a pane layout; per-pane camera, picking and exposure |
| C. Render jobs | 6, 7 | a job spec that can be saved, read back out of a PNG, and queued |
| D. Time | 8–13 | one track model (key, interpolate, extrapolate) aimed at `-S` paths |

The last section is the answer to "am I missing obvious combinations".

---

## Track A — the dice

### 1. A standard randomize config, save/load, and mutation reads it too

**Where it stands.** `RandomSettings` (`src/randomize.rs`) is a preference,
lives in `[random]` in prefs.toml, and is edited in the Random settings window.
`--random-config FILE` already reads either a bare settings table or any file
with a `[random]` table. Mutation (`src/mutate.rs`) knows nothing of it: it is
1–3 hardcoded operators scaled by one strength, it never sees symmetry, xaos or
zoom, and it runs **no quality gate** — a mutation that blows the attractor up
or collapses it to a speck goes straight onto the screen. Undo makes that
survivable for one `U` press. A 3×3 grid (item 3) would put four broken tiles
in front of you every time, so this is a prerequisite rather than a nicety.

**Plan.**

1. **Named settings files.** `RandomSettings` serialises already. Add a folder
   of named configs (`~/.config/fracturize/dice/<name>.toml`, plus anything
   under `scenes/dice/` so configs can be shared in the repo). The Random
   settings window gets a picker: load, save, save as, revert to defaults.
   prefs.toml keeps the *current* settings and the name they came from, with a
   `*` when they've been changed since loading, the same document convention
   scenes use. `--random-config <name|file>`.
2. **A `[mutate]` section in the same struct**, beside the roll settings, so
   one file describes "what the dice may do": per-operator odds (translate,
   rotate, scale, variation nudge, variation swap, colour, duplicate, delete,
   and the new ones from 4 and 5), strength, and how many operators a
   mutation applies. The variation list and its weight ranges are **shared**:
   a mutation that adds a variation draws from the same enabled list and
   ranges a roll would. That is the "reuse" — one list, not two that drift.
3. **Mutation goes through the gate.** `roll()` already runs a candidate
   through `trace::measure` and the dimension/lacunarity bounds and retries.
   Pull that into `gate(&Scene, &RandomSettings) -> Result<Measured, Reject>`
   and have `mutate` retry up to N times against it. Dimension *steering* is
   probably wrong for mutation (it rescales every map, which is a bigger change
   than the mutation itself), so mutation rejects instead of steering. That's a
   decision below.
4. **Seeded and reproducible**, like rolls: `mutate_seeded(scene, settings,
   seed)`, and the operator log names the seed. `--mutations` gains
   `--random-config`.

**Decisions for Hazel.**
- Does mutation steer dimension back into bounds, or only reject? (Suggest:
  reject. Steering hides what the mutation did.)
- Should mutation strength stay its own preference (the Mutate window slider),
  or move into the dice file? (Suggest: into the file, with the slider editing
  it.)
- Should a scene remember which dice config it was rolled from? It's
  provenance, not scene data. Suggest the render record and the `random-<seed>`
  name carry it, and the scene file doesn't.

### 4. Xaos in mutation and random rolls

XAOS-PLAN.md §6 already sketches this. As settings:

- **Roll:** chance a flame has xaos at all; sparsity (fraction of edges cut);
  no-self-loop chance; preset odds (cycle, blocks:N). **Strongly connected by
  construction**: seed a random Hamiltonian cycle and only cut edges off it,
  so no roll ever ships a closed class that drains everything.
- **Gate:** reject if any map's *walk share* falls under a floor (a few %).
  `xaos.rs` already computes walk shares. A map with no walk share is a map you
  can see a gizmo for and nothing on screen.
- **Mutate:** cut an edge, restore one, nudge a multiplier (in log space),
  apply a preset, "linked map" (Apophysis's pre-final trick, which already
  exists as `insert_linked_transform`).
- Measure with Mauldin–Williams dimension when xaos is present, which `xaos.rs`
  has. The dimension *steer* doesn't apply under xaos (no single factor
  balances a spectral radius), so xaos rolls reject only, as the lock already
  does.

### 5. Infinite zoom in mutation and random rolls

- **Roll:** chance of a zoom scene. If chosen, one map is rolled as the zoom
  map: pure affine, contracting on all three axes, **zero translation** (CRAFT
  and AGENTS both say so: the fixed point should be the origin), near-uniform
  scale (an anisotropic zoom map shows a seam, `Renorm::defect` measures it)
  and a rotation. Optionally emit a `path_loop = "zoom"` single-key path, so
  the rolled flame opens already falling.
- **Gate:** `Renorm::build` succeeds; the defect is under a threshold; with
  xaos, every map that fires may be followed by the zoom map. That last one is
  a load error today, so the generator must guarantee it rather than discover
  it.
- **Mutate:** operators that touch the zoom map must keep it eligible: no
  variations, still contracting, still uniform within tolerance. Simplest rule:
  the zoom map only gets rotation and uniform-scale nudges. Separate operators
  for `radius`, `levels` and `edge_guard` aren't worth having — those are
  framing, not form.
- Thumbnails (item 3, the Random flames list) must frame a zoom scene inside
  its band. Check that `offline::frame_setup` folds the distance the way
  `--render` does before trusting a thumbnail.

### 3. Clickable mutation grids, 3×3 and 5×5

The Apophysis workflow, and the item AGENTS.md calls "the highest-value single
addition left": mutate eight times, show them around the parent, click one, it
becomes the parent, repeat.

**It's thumbnails, not multiview.** This looks like item 2 and isn't: item 2
is one scene from several cameras (one chaos pass, N cheap render passes),
while this is N *different* scenes, each with its own chaos game. The machinery
already exists in `gpu/thumbs.rs`: a 256-walker ring, converge once and then
it's a still, ~0.7 ms each on the GTX 1080. 24 tiles for 5×5 is ~17 ms to
build, spread across frames by the existing 4 ms budget.

**Plan.**

1. A **Mutate grid window** (or the Mutate window grows a grid). The centre
   tile is the current scene; the ring around it holds 8 (3×3) or 24 (5×5)
   gated mutations from item 1. Mutations are rolled on worker threads like
   the Random flames list (`random_list.rs` is the model), so a tight gate
   never costs a frame.
2. **Click a tile → it becomes the scene**, as one history entry ("mutation
   accepted: T2 rotate 21°, T1 +swirl 0.18"), and the ring re-rolls around it.
   Ctrl+Z steps back to the previous parent *and* restores the grid it was
   chosen from, so you can choose its sibling instead. That means the history
   entry carries the seeds of the grid, which is cheap.
3. **Hover flies the tile's camera**, as the browser does. Nothing else moves.
4. **Strength per ring** in 5×5: inner ring at the set strength, outer ring at
   double, so one grid shows near and far. Apophysis's "speed" slider is this.
5. **Masks**: mutate only the selected transform; only colour; only
   variations; only xaos. These are just dice settings with operators zeroed,
   so they're presets of the item-1 config, not new code.
6. **Lock framing**: every tile renders the *parent's* camera, so the
   comparison is of form. A mutation that moves the attractor off-frame shows
   as such. That's honest, and it's what the gate's framing check can catch.
7. CLI twin already exists: `--mutations 8`. Add `--mutations 24`, layout 5×5,
   centre tile = parent, matching the window. Agents and the GUI should see the
   same sheet.

**Decisions for Hazel.** Separate window or inside Mutate? Does accepting a
tile also adopt its colour, or keep the parent's? (Colour is an operator now,
so mask settings answer it.)

---

## Track B — many views

### 2. Multiview: +x, +y, −z beside the perspective view, while editing

Layouts: **2×2** (perspective + three axis views) and **1 + 3** (big
perspective view, three axis views stacked on the right).

**What makes it cheap.** The point buffer doesn't depend on the camera. N panes
cost one chaos pass and N render passes. Fill cost scales with total pane
area, not pane count, so four quarter-size panes cost about what one full view
does (to be measured). The drawn-point budget, the overlay MSAA target and the
gizmo pass all repeat per pane.

**What it needs.**

1. **An orthographic camera.** `camera.rs` is perspective only
   (`perspective_rh`). CAD axis views are orthographic for a reason: in a
   perspective side view a translation along the view axis still moves things
   on screen, and the whole point of the side view is that it doesn't. Add
   `Projection::{Perspective(fov), Ortho(half_height)}` to `OrbitCamera`. Point
   size and the `use_point_primitives` test then need an ortho branch
   (projected size no longer depends on depth), and so does haze's auto band.
2. **Panes as a layout** in `App`: a list of `{rect, camera, kind}`, rendering
   each into a viewport rectangle of the same surface (`set_viewport` +
   scissor). The axis panes take the main camera's focus, and an ortho height
   that tracks the main camera's distance, so all four stay on the same part of
   the attractor.
3. **Per-pane picking.** `pick.rs` is pure screen-space projection, so it just
   needs the pane's camera and rect. In an axis pane, translating the origin
   dot moves it **in that plane exactly** — which is the editing win this
   feature exists for. Rotation edges in an axis pane rotate about that axis.
4. **Per-pane exposure.** Splat exposure is normalised by resolution; each
   pane has its own. Volume mode is 5 passes, so offer volume in the main pane
   only and splat in the others, or accept the cost.
5. **Navigation**: orbit only in the perspective pane; axis panes pan and zoom
   (scroll zooms the ortho height). Clicking an axis pane's title makes it the
   big one in 1+3 — the corner axis cross already does "look down this axis".

**The trap: infinite zoom.** The wrap carries the point buffer with the *main*
camera (`rewrap`). An axis pane at a fixed ortho height would see the attractor
jump by one zoom period every wrap. Either axis panes follow the main camera's
fold depth (scale their height by the same `A⁻¹`, which is exactly what keeps
them stable), or multiview turns off under zoom. Suggest the first: it's one
multiply.

**Decisions for Hazel.** Ortho for axis views (suggest yes)? Should axis panes
be configurable (any axis, any sign) or fixed to +x / +y / −z? Does the layout
persist per scene or as a preference? (Suggest: preference, like panel
geometry.)

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

- **Default camera**: cyclic, sin/cos on the eye's x/y around a fixed look-at.
  Note that's a different shot from today's turntable (a full yaw orbit at 0.18
  rad/s): a sin/cos on x/y with a fixed focus sways and tilts rather than
  circling. Decision below.
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

**Decisions for Hazel.** Default camera: keep the turntable (a yaw cycle) as
the default cyclic shot, or the sin/cos sway? Do saved views (`views/*.toml`)
become keys, or does only Y add keys?

### 8. A timeline window, wide and low

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

**Not animatable, by design**: anything discrete that changes the map set —
adding or removing a transform, changing a symmetry group, the variation
list's shape. To fade a part in or out, animate its **weight or an xaos
multiplier** towards 0. That's smooth, because a multiplier only thins density
until it reaches exactly zero. The colour-ring defect (adding a transform moves
every colour) is another reason the map set stays fixed over an animation.

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

- Decode with `symphonia` (pure Rust: wav, flac, mp3, ogg, aac-in-mp4), play
  with `cpal`. Two new dependencies, neither with a C toolchain behind it.
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
- **Muxing audio into video.** `video.rs` is a hand-rolled ISOBMFF muxer with
  one video `trak`; audio is a second `trak`. Codec choice: **AAC** is what
  every upload pipeline takes, and there's no good pure-Rust AAC encoder.
  **Opus in MP4** is standard, small, and has a solid library (`libopus`
  bindings), and YouTube and browsers take it, but some social platforms
  don't. AVIF can't carry audio at all, so audio means `.mp4` (or a `.webm`
  muxer). Decision below.

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
4. **Random rolls of animations.** The dice (1) gain a "roll tracks" section:
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

1. **Item 1 with the gate on mutation**, then **3**. Smallest step with the
   most immediate payoff, and it fixes the existing `U` key on the way. 4 and 5
   slot in afterwards as settings plus operators.
2. **6 → 7.** Self-contained, and most of the plumbing (records, job params)
   is there.
3. **2**, after an orthographic camera.
4. **9 → 8 → 10**, then audio (11 → 12 → 12.5), then 13. The track model
   should be settled before any timeline UI is drawn.
