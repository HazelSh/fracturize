# Fracturize - AI Agent Guidelines

A 3D fractal flame renderer inspired by Apophysis, built with Rust and wgpu.

## Project Overview

Fracturize renders IFS (Iterated Function System) fractals in 3D using the chaos game
algorithm, entirely on the GPU. A compute shader runs thousands of parallel "walkers"
that iterate through weighted random transforms (affine matrix + nonlinear variation
blend), writing positions into a circular point buffer that is rendered every frame.

**Making artwork with it? Read `CRAFT.md` first.** This file is the reference for
what Fracturize *is*; CRAFT.md is the craft guide for authoring scenes as art —
measured doses, the numbers that predict a picture before you render it (start with
the similarity dimension), which variations are actually 3D, the traps, inherited
Apophysis-era lore and what of it survives here. It has a discovery log at the end;
add to it.

## Architecture

```
src/
  main.rs        # CLI args, winit event loop + egui event gating, keybinds, default scene
  app.rs         # App state, mouse/edit handling, render orchestration, screenshots
  history.rs     # Unified snapshot undo/redo behind App::commit_edit (all edits)
  haze.rs        # Aerial perspective: one amount, band and falloffs derived
  indicators.rs  # Selection offset/rotation lines, and camera-path polylines
  randomize.rs   # Random flame generator with a CPU chaos-game quality gate
  renorm.rs      # Infinite zoom: renormalize an IFS into a scale-invariant set
  render_job.rs  # Render job model: params, events, pause/cancel, estimates
  ui/            # egui layer (see "Human Interface" below)
    mod.rs         # EguiLayer, UiState, per-frame draw order, font install
    toolbar.rs     # Top strip: File menu, panel toggles, undo/redo, quick controls
    status_bar.rs  # Bottom bar: context hints, FPS/p99 sparkline, point stats
    hints.rs       # hinted(): tooltip + status-bar hint on one widget response
    num.rs         # Numeric readouts that don't move: the fixed-width house rule
    confirm.rs     # Click-wait-click Arm, and the unsaved-changes modal
    transforms.rs  # Transform tab rail + selected-transform detail pane
    explore.rs     # Random flame, mutate + strength, undo/redo, history list
    render_panel.rs# Renderer mode, exposure, point size, colour, haze, infinite zoom
    save_as.rs     # "Save scene as…" modal (fork the scene under a new name)
  render_job.rs  # Batch render dialog: setup, estimates, progress, pause/cancel
    camera_panel.rs# Framing, saved views, the camera path (incl. how it loops)
    axis_widget.rs # Corner orientation cross; click a ball to look down that axis
    radio.rs       # Segmented one-of-n radio, used by all three such settings
    browser.rs     # Scene picker (Ctrl+O / B) — what File > Open… shows
    shortcuts.rs   # Controls window (H / F1): input prefs + the keybind table
    labels.rs      # World-anchored transform name labels
    icons.rs       # Phosphor codepoints (vendored font, see assets/fonts/)
  camera.rs      # OrbitCamera (yaw/pitch/distance/focus), ray + projection helpers
  path.rs        # CameraPath: Catmull-Rom splines over orbit keypoints
  video.rs       # Animation formats: Format (avif/mp4), RGBA->YUV, shared ISOBMFF muxer
  avif.rs        # AV1 backend for .avif (rav1e)
  h264.rs        # H.264 backend for .mp4 (openh264)
  pick.rs        # Gizmo hit-testing and drag geometry (pure math, unit-tested)
  mutate.rs      # Random scene mutation operators (U key, --mutations)
  trace.rs       # CPU chaos walkers (variation port) for the trace overlay
  prefs.rs       # Persistent user prefs (~/.config/fracturize/prefs.toml)
  scene.rs       # TOML scene parsing AND saving, TransformSpec, variation names/slots
  symmetry.rs    # Finite subgroups of SO(3) (C/D/T/O/I) by closure, plus
                 # `repeat` progressions, which are not groups
  gpu/
    context.rs   # wgpu device/surface setup (vsync flag, adapter limits)
    buffers.rs   # GPU struct definitions (GpuTransform, Point, CameraUniforms)
    points/      # Active renderer: chaos compute + point rendering
      compute.rs   # Chaos game dispatch, circular buffer bookkeeping
      renderer.rs  # Dual pipelines: billboard quads / native 1px points
      splat.rs     # Splat mode: additive log-density accumulation + tonemap
    gizmo.rs     # Transform gizmos (unit tetrahedra per transform)
    lines.rs     # In-world line renderer (traces, camera path, indicators)
    overlay.rs   # MSAA target for the in-world UI: depth blit, composite
    density/     # Inactive experimental hash-grid density renderer
shaders/
  points/chaos.wgsl   # Chaos game + the 20 variation functions
  points/render.wgsl  # vs_main (quads), vs_point (1px points), shared fs
  points/splat.wgsl   # Splat accumulate (gaussian kernels) + log tonemap
  gizmo.wgsl
scenes/          # TOML scene files
```

## Tech Stack

- **Rust 2024 edition** - `gen` is a reserved keyword, use `r#gen()` for rand
- **wgpu 29** - Vulkan-backed; requires SHADER_F16 (fine on Intel UHD 620 + Mesa)
- **winit 0.30** - ApplicationHandler pattern
- **egui / egui-wgpu / egui-winit 0.35** - the entire human interface. Replaced the
  hand-rolled glyphon overlay (deleted); these three must stay version-locked to each
  other, and 0.35 is what pins wgpu to 29 and winit to >=0.30.13.
- **Phosphor icons, vendored** - `assets/fonts/Phosphor.ttf` (MIT) registered as a
  font fallback in `ui::install_fonts`, with the codepoints we use hand-copied into
  `src/ui/icons.rs`. The `egui-phosphor` crate has no egui-0.35-compatible release.
- **glam / bytemuck / toml + serde / clap / image** - math, GPU casts, scenes, CLI, PNG

## Rendering Approach

All GPU, three passes per frame:
1. **Chaos compute** (`points/chaos.wgsl`): 16384 walkers each iterate the IFS a few
   times per frame, writing into a circular buffer (full refresh every ~800 frames;
   10x faster during warmup). Per iteration: pick transform by cumulative weight,
   apply affine matrix, then blend the 20 variation functions by weight. Diverged or
   NaN walkers are re-seeded randomly (important for nonlinear variations).
2. **Point render** — two interchangeable modes over the same point buffer
   (`R` toggles live; `--splat` / a view's `renderer = "splat"` select at launch):
   - **points** (`points/render.wgsl`, default): opaque depth-tested points with
     adaptive pipeline selection. When the projected point size at orbit distance
     is subpixel (the common case), points are drawn as native 1px point
     primitives (~3x faster). Otherwise, 4-vertex instanced triangle-strip
     billboards with perspective sizing. Every point is full brightness.
     Near-field growth is capped (12px in both modes) so points brushing
     past the camera in volume-filling scenes render as motes, not
     screen-eating squares/gaussian washes.
   - **splat** (`points/splat.wgsl`): additive log-density accumulation,
     flame-style. Each point deposits ~1 unit of energy as a gaussian splat into
     an rgba16float HDR target (same subpixel fast path: 1px additive points);
     a fullscreen pass then applies `log2(1 + density·exposure)` tonemapping.
     Isolated points stay visible grit; overlapping ones form smooth density
     gradients instead of clipping — this is the fix for diffuse scenes that
     saturate into a pastel blob under the point renderer. No occlusion (the
     fractal is pure emission; haze thins each point before accumulation).
     Exposure is normalized by point capacity and resolution, so brightness is
     stable across effort levels and render sizes; `W`/`Shift+W` (or
     `--exposure`) scale it. ~75% of the point renderer's FPS.
3. **Gizmos, traces + indicators**: optional overlays (see keybinds). Traces
   (X) are CPU walkers (trace.rs ports the 20 variations from chaos.wgsl —
   keep them in sync!) rendered as alpha-faded line segments; they regenerate
   on every scene edit.

   **The in-world UI is multisampled; the artwork is not.** These two
   requirements can't share a render target — the point cloud's aliasing *is*
   the look, and multisampling 1px point primitives would soften exactly the
   grit the renderer exists to make — so they don't. Traces, indicators, the
   camera path and the gizmos all draw into `src/gpu/overlay.rs`'s own MSAA
   target (2 samples where the adapter offers it, else 4) and are composited
   over the finished frame. Three things make it work:

   - the main pass's single-sample depth is **blitted** into the overlay's
     multisampled depth by a fullscreen triangle writing `frag_depth`, so the
     point cloud still occludes the gizmos exactly as it did when they shared
     one buffer. This is why the main depth texture carries `TEXTURE_BINDING`;
   - the overlay clears to transparent black, so its contents come out
     **premultiplied** (straight-alpha shaders blending against zero leave
     `rgb * a`), and the composite blends `One / OneMinusSrcAlpha`;
   - the composite averages the samples in the shader rather than using a
     hardware `resolve_target`, which would need a third full-size texture to
     resolve into for the same arithmetic.

   Cost on the reference desktop: **0.19 ms/frame** and ~20 MB at 1440x860.
   Every pipeline that draws into it (`gizmo.rs`, `lines.rs`) takes a `samples`
   argument and must be rebuilt if that changes.

   2x MSAA is not guaranteed by the WebGPU spec — only 1 and 4 are — so the
   device requests `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` when the adapter
   has it, and `GpuContext::surface_sample_counts` reports only what can
   actually be created. Don't trust `get_texture_format_features` alone; without
   that feature wgpu holds the device to [1, 4] whatever the adapter says.

   An earlier attempt anti-aliased the lines analytically instead (screen-space
   ribbons with a coverage feather) and was reverted. It's recorded here so it
   isn't retried: without mitre joins the ribbon wobbles along a polyline, and
   the feathered band still wrote depth, punching a hole through the gizmo
   faces behind it. The rasterizer knows how to do this.

The chaos churn rate is wall-clock normalized: `advance_frame` takes the
frame dt and scales walker iterations so the buffer refreshes at the same
real-time rate at any refresh rate (60 FPS baseline: full cycle ~13 s).
The default camera orbit is likewise time-based (`path::ORBIT_RATE`, 0.18
rad/s — one turn every ~35s), expressed as that path's duration.

**Drawing fewer points than we have.** The live window draws
`App::drawn_points`, not the whole buffer. Normally those are the same; the
exception is an attractor that has collapsed to a speck, which is the state a
scene is in for the first few seconds of being built from nothing (a single
enabled transform has exactly one fixed point, so every point in the buffer
lands on the same pixel). Blending is serialized per sample, so six million
fragments on one texel is six million operations the GPU cannot overlap:
measured on the reference desktop, **654 FPS → 46 FPS** at 6M points, scaling
linearly with the count. The budget is the attractor's screen footprint
(measured by `trace::measure`, the same CPU walkers `randomize.rs` gates on)
times a generous points-per-pixel, floored well past where more points change
anything. It restores 46 → 567 FPS and leaves real scenes untouched. Splat
exposure is normalized by the drawn count, so brightness doesn't move either;
the status bar says when it's engaged. `--render` and screenshots always draw
everything — they're one-off and must stay reproducible from the parameters.

Performance on the reference machine (ThinkPad T490, Intel UHD 620, 1280x720):
- ~10M points at ~38 FPS uncapped (subpixel/point-primitive path)
- ~5M points comfortably at 60 FPS; billboard path is ~3x slower per point
- Storage-binding limits are raised to adapter max at startup (default 128MiB cap
  would limit the buffer to ~8M points; buffers are 16 bytes/point)

## Mouse Controls

| Input | Action |
|-------|--------|
| left-drag (empty space) | orbit camera, grab-the-scene: drag right spins it right, drag up tilts its top toward you (takes the camera off its path) |
| shift+drag / middle-drag | pan the focus in the view plane |
| right-drag (empty space) | roll the camera about its view axis |
| right-click a gizmo | that transform's context menu: duplicate / enable / delete / rename — the same menu its row in the Transforms window has |
| scroll | zoom — **always**, whatever is under the pointer |
| click empty space | deselect (the only way to clear a selection) |
| click an unselected gizmo's origin dot | select it, and nothing else — the press starts no drag at all, not even a camera orbit |
| drag the selected gizmo's origin dot | translate the transform in the view plane |
| drag an axis endpoint (tip dot) | scale that one axis; drag past the origin to mirror it |
| drag an origin→axis gizmo edge | translate along that axis |
| drag an outer gizmo edge | rotate around the third local axis (edge x-y rotates around z) |
| ctrl+drag any gizmo part | uniform scale (drag up = grow) |
| shift during a gizmo drag | fine: a fifth of the travel. Anchored, so pressing or releasing it mid-drag doesn't jump |
| alt+drag an outer gizmo edge | snap the rotation to 15° |
| alt+drag an axis endpoint | snap that axis's scale to 0.1 |
| drag the ring around the selected gizmo | roll it about the camera's view axis, through its own origin |
| alt+drag the ring | snap the roll to 15° (or the transform's symmetry-group step) |
| alt+scroll over a gizmo | adjust that transform's chaos weight (probability) — the lever that emphasizes an element without changing structure or color |
| click a Transforms row | select that transform (two-way with gizmo selection) |
| double-click a Transforms row | rename it inline |
| drag a row's weight bar | change that transform's chaos weight |
| alt+click a row's eye | solo it; alt+click again brings everything back |
| right-click a Transforms row | duplicate / enable-disable / delete / rename |
| click the corner axis cross | look down that axis (the six balls, bottom right) |
| drag any panel DragValue | change the value; click it to type an exact one |

A button-down becomes a drag past a **4px threshold**, which is what lets a
click be told from a drag — needed by click-to-deselect, and by "a click on a
gizmo that didn't move shouldn't land a history entry".

**It is not a dead zone on the camera.** The obvious reading is to withhold
movement until the pointer has travelled far enough, and that's wrong here:
orbiting is the gesture you use continuously, so a dead zone on it is felt as
stiction at the start of every single drag — a constant cost, paid to prevent a
two-pixel camera nudge that is imperceptible and that nothing records. The
camera tracks the pointer from the first pixel. **Gizmo** drags do wait for the
threshold, because those write to the artwork and land an undo entry, and
they're a careful deliberate gesture where a few pixels of settling costs
nothing. The pointer says which gesture is in flight: grabbing for orbit, move
for pan, horizontal-resize for roll.

**Scroll is navigation and only navigation.** The chaos-weight lever used to be
on plain scroll and had to move: scroll is the gesture people use continuously
and without looking, the fractal fully occludes gizmos while they keep taking
input, and so zooming through a scene silently edited whatever happened to pass
under the pointer. A navigation gesture must not be able to change the artwork.

**The pointer hides itself in view mode**, once it has sat still over the
artwork for two seconds (`App::update_cursor_visibility`, and
`cursor_should_hide` for the decision on its own, which is unit-tested).
Anything at all — motion, a click, a wheel notch — brings it straight back.
It is a black arrow parked in the middle of a picture, and this is a program
for looking at pictures.

**Edit mode always keeps it**, however long the hand has been still: every
gizmo is a thing you aim at, hover-highlight and grab, so a cursor that
vanishes while you line one up vanishes at exactly the moment it is in use.
Nor does it go while the pointer is over a panel, or while a drag is being
held. This is not a breach of the zero-animation rule below: the cursor is the
operating system's furniture rather than this app's chrome, and it doesn't
fade — it is drawn on one frame and not on the next.

Grabbable gizmo parts glow and grow on hover (edges widen and whiten, dots
enlarge) and the cursor switches to a grab hand. **Held is a separate state
from hovered**: a held part drops the white mix and shows its own colour at
full strength, rather than glowing harder. They used to be indistinguishable,
and not by choice — `update_hover` is deliberately not called during a drag
(by then the pointer has left the part, and recomputing would un-highlight the
thing being dragged), so "held" was only ever "a hover that stopped being
recomputed". `try_grab_gizmo` now says so explicitly, via the `held` flag in
the highlight uniform. Gizmo drags re-run the chaos game live; the fractal
re-forms as you drag (sparse while moving, densifying when you pause — warmup
refills in ~1s). Picking math lives in `pick.rs`, drag application in `app.rs`.

**Only the selected transform is manipulable.** An unselected one offers its
origin dot and nothing else, and that dot only selects. Tips, shafts and rotate
edges belong to the transform you have already chosen. This is what closes the
invisible-click hole: picking (`pick.rs`) is pure screen-space projection with
no depth awareness, so every part of every gizmo used to be grabbable whether
or not the attractor hid it. Now the pickable set is the visible set, and the
contest shrinks from ~140 candidates at twenty transforms to ~9. The cost is
one extra click to switch transforms, which is the order people work in anyway.

The **roll ring** is painted in screen space through egui
(`src/ui/gizmo_ring.rs`), not built as world geometry, and that is the honest
choice rather than the cheap one: the view plane is a screen-space idea with no
world shape to be faithful to. It also falls out that the ring is exactly
screen-constant and costs no vertex-buffer churn — `indicators.rs` rebuilds only
when the *matrix* changes, so a camera-facing ring built there would have sat
still while the camera orbited around it. `pick::roll_ring` is the single
definition of its centre and radius, called by both the drawing and the picking,
because a ring you can see in one place and grab in another is worse than no
ring. It is derived from the gizmo's projected silhouette so it always sits just
outside the tetrahedron, and it is dashed at rest and solid while held — the one
part whose held state *drops* its distinguishing mark rather than gaining one,
which is how a ring reads as engaged. A circle also never degenerates, unlike
the three local-axis rotate edges, which collapse to a line seen edge-on —
exactly when you need another way round.

Gizmos are drawn in three passes, in this order (`GizmoRenderer::draw`):
**x-ray** (no depth test, no depth write), then **edges+dots** (depth-write on),
then **faces** (depth-write off). The x-ray pass goes first so the solid pass
paints over it wherever the gizmo is really visible: a visible gizmo looks
exactly as it always did, and only the part buried in the attractor shows
through, faintly. It draws two things -- every transform's origin dot, and the
*selected* transform's whole gizmo. Not every gizmo in full: that would defeat
the G/Tab toggle that exists so the art can be looked at. The origin dot is the
one part of an unselected gizmo you can grab, so it is the one part that must
never be invisible while still taking clicks. `XRAY_ALPHA` is the knob if the
ghost reads too faint or too loud over a bright attractor.

Hovering a gizmo also promotes that transform's name label to the solid
backdrop the selected one gets (`src/ui/labels.rs`). Bare white text disappears
against a bright attractor, and reading a name is how you decide whether this is
the transform you meant -- which matters most before you have selected it.

Three rules the gizmo geometry depends on:

* **The tip handles scale by rewriting one matrix column, never by decomposing.**
  The axis direction is captured at grab and held fixed, so the column only
  changes length, the three columns stay perpendicular, and the matrix stays a
  faithful T·R·S. This is not fussiness: the scene format has no way to write a
  sheared matrix, so a drag that introduced shear would author something the
  save silently discards (see `GIZMO-PLAN.md` §1.1, which also notes that
  `mutate.rs` already hits this).
* **An axis that projects shorter than `pick::MIN_AXIS_PX` offers no tip, no
  shaft and no adjacent rotate edge.** An axis pointing near the camera has
  almost no screen gain, so dragging along it doesn't degrade — it explodes.
  Zooming in is the fix, and it is the honest one.
* **`EDGE_DOT_VERTS` in `gpu/gizmo.rs` and the modulus in `shaders/gizmo.wgsl`
  are one fact in two languages.** WGSL is opaque to the compiler, so this is
  the one invariant here that a test guards rather than the build
  (`the_shader_agrees_about_the_vertex_block`). The reference tetrahedron's tip
  dots are built *degenerate* rather than transparent, because that pipeline
  writes depth and an invisible quad would still punch a hole.

The camera eye always sits on the orbit sphere: the legacy scene/view
`offset` (which made pitch drift the view distance) is folded into the
framing and distance at load time and no longer written to files.

**The framing is a quaternion, not three angles.** `OrbitCamera` holds an
`Orientation` (see `src/rot.rs`), so there are no poles and nothing is
clamped: drag straight up and you go over the top and come out inverted, and
roll works looking straight down. All three of those were impossible before —
pitch stopped at ±87.7°, and at the pole roll silently did nothing because the
axis it rotated world-up about *was* world-up.

Yaw/pitch/roll survive as a **chart**: a human-readable naming of a framing,
used by scene files, `--yaw/--pitch/--roll`, and the Camera panel readout.
Charts have poles. Looking straight up or down, yaw and roll become the same
control and neither means anything on its own — so anything that writes a
framing checks first and falls back to `rotvec` (an exact rotation vector in
radians) where the chart can't say it. `--rotvec x,y,z` is the CLI equivalent,
and `--render` prints whichever form the framing it landed on needs.

## Keybinds (also in-app: press H)

| Key | Action |
|-----|--------|
| H / ? / F1 | toggle the Controls window (input prefs + every keybind) |
| Esc | **cancel**: a menu, then a dialog, then the browser, then the selection |
| Ctrl+Q | quit (asks first if the scene has unsaved edits, as does the window's close button) |
| Up / Down | zoom in / out (steps the selection when a transform is selected) |
| Home | put the camera on the selected transform's fixed point |
| Enter | enable/disable selected transform |
| G / Tab | toggle transform gizmos and their name labels |
| O, Z, or Space | play / stop the camera flying its path (three keys, one action) |
| Y / Shift+Y | add current framing as a keypoint of this scene's own path / remove the last one |
| Ctrl+Y | redo (the Windows binding, so an Apophysis refugee's reflex lands somewhere safe) |
| V | save current view to views/<scene>-<timestamp>.toml |
| S | save screenshot to screenshots/<scene>-<timestamp>.png (never overwrites) |
| Ctrl+S | **save the scene** (with all edits) back to its TOML file |
| U / Shift+U | random scene mutation / undo it |
| Ctrl+Z / Ctrl+Shift+Z | undo / redo *any* edit (see `src/history.rs`) |
| X / Shift+X | chaos-game traces: show (re-rolls each press) / hide |
| I | invert mouse pitch, flightsim style (persisted to prefs) |
| Ctrl+O or B | Scenes window: arrows + Enter, or click a row, to load a scene in place |
| Ctrl+N | start over on a blank canvas (an edit, so one Ctrl+Z brings back what was there) |
| P | open the Render job dialog (see Render Jobs) |
| A / Shift+A | duplicate selected transform / add a fresh one (rebuilds pipelines) |
| Delete | delete selected transform |
| , / . | selected transform's chaos weight down / up |
| J / K / L | selected transform's color: hue / saturation / value up (+Shift = down) |
| E / Shift+E | cycle the variation slot targeted by - / = (shown in the status bar) |
| - / = | targeted variation weight down / up (0.05 steps) on selected transform |
| R | toggle renderer: points / splat (additive log-density) |
| W / Shift+W | splat exposure up / down |
| [ / ] | shrink / grow point size |
| D / Shift+D | finer / coarser color detail (color_falloff) |
| C / Shift+C | less / more color contrast |
| F / Shift+F | more / less atmospheric haze (the depth cue; see "Haze") |
| Ctrl+Shift+S | save scene as… (fork under a new name) |

The Controls window (H or F1) is clickable: each row triggers its first-listed
binding, shift+click the second. `I` persists to
`~/.config/fracturize/prefs.toml` (user preferences, not scene data).

**Escape never quits.** In every desktop program written this century it means
*cancel the thing in front of me*, so making it the quit key means the reflex
that closes a popup closes the app. It falls through: transform menu → dialog →
scene browser → the transform selection → nothing. Quitting is Ctrl+Q or the
window's close button, and both check for unsaved edits first, as does opening
a scene (which clears the undo stack).

**A scene is a document.** The window title is `document — application` with a
`*` when there are unsaved edits, and the toolbar's scene name carries the same
marker. The rule that settles what counts as an edit: **if it changes what
Ctrl+S writes, it is an undoable edit** — with one conventional exception,
continuous view state, since nobody expects Ctrl+Z to un-orbit a camera and no
3D application offers it.

**The dirty bit is a position, not a flag.** Every history entry carries a
unique serial (`History::top_serial` names the state the stacks are currently
at); `App::saved_serial` remembers what that was when the scene was last
written, and `is_dirty` compares the two. So undoing back past your last save
reports the scene as clean — which it is, byte for byte — and redoing forward
past it reports dirty again. Undo and redo need no special case and deliberately
don't have one: they move the history, and the dirty bit reads where the history
is. A boolean could only be raised by an edit and cleared by a save, so a scene
you had edited and then completely undone still demanded an answer, on quit,
about work that no longer existed. It fails safe under the history caps: an
entry evicted from the bottom of the stack takes its serial with it, so an
evicted save point never compares equal again and the scene stays dirty. A
*coalesced* commit takes a fresh serial even though it adds no entry — else
saving mid-drag and dragging on within the coalescing window would leave the
file reading clean while the scene moved.

**Orbit style** (Controls window, `orbit_style` in prefs) picks what a
horizontal drag yaws about, and is the other preference of this kind.
`trackball` (the default) yaws about the camera's *own* up: body-frame turns
compose on the right, so a drag applies the same rotation from every framing
and the controls feel identical however the camera got there — which is the
point, since these scenes have no horizon to orient by and every zoom wrap
twists the framing. The price is stated rather than papered over: rotations
about different axes don't commute, so circling drags accumulate roll (the
roll readout shows it, `level` clears it, and nothing ever re-levels behind
your back). `turntable` restores the old world-Y yaw, which holds the horizon
level at the cost of the feel depending on where you're pointing — near the
poles world up is nearly the view axis, so the same drag reads as roll. The
two coincide exactly when level. Camera *paths* are unaffected either way: a
path is a shot, not a feel, and the default full orbit still flies world-Y
circles.

Every keybind above has a mouse equivalent in the panels (see "Human
Interface"), and both go through the same `App` methods — neither is a
reimplementation of the other, so they cannot drift.

Ctrl+S also bakes the current camera framing, point size, and color
falloff/contrast into the scene's defaults. It no longer writes
`point_count` (that's a render property now — see "Human Interface"), though
a hand-authored `point_count` line in an existing file is preserved. **Saving preserves comments**:
existing files are edited in place via toml_edit — only changed values are
rewritten, so header/per-transform comments, inline `# notes`, and formatting
like `6_000_000` survive. Two exceptions: a legacy camera `offset` key is
removed (folded into yaw/pitch/distance), and if transforms were added or
removed the whole [[transform]] array is rebuilt (header/meta/camera comments
still survive). Scenes with no path (built-in default) save to
`scenes/untitled-<timestamp>.toml`.

## Human Interface (egui panels)

The app is Fracturize's *human* interface; scene/view TOMLs are its LLM and
CLI interface. Both drive the same `App` methods, so an edit made by dragging
a slider and one made by a keybind are the same edit, land in the same
history, and save identically.

Layout: a thin top toolbar over a full-surface viewport, floating
`egui::Window` panels, and an Inkscape-style bottom status bar. Nothing shrinks
the drawable area, so aspect and picking math are unaffected by which panels
are open. The toolbar runs

```
File | Transforms  Gizmos | Explore | Camera  Render | Undo Redo | quick controls … scene name | Help
```

ordered by task rather than by module: what am I working on, then how am I
looking at it, then help. Gizmos sits with Transforms because it is a *view of
the same object*, not a peer of the panel toggles.

**File is a menu, and the only one** — not the first entry of a
File/Edit/View/Help bar. The scene is a document, and the value of that
abstraction is that people can port intuition from every file-editing program
they have used; but intuition needs furniture to attach to, and a File menu
with the conventional contents in the conventional order is that furniture.
What it deliberately is *not* is a menu bar: Edit and View would be near-empty
duplicates of the toolbar toggles and the Explore window, and a menu bar that
is mostly empty teaches people that menus here aren't worth opening. File
operations are numerous, conventional and infrequent — the trade a menu is
right for. Undo and redo are the opposite trade, so they get visible buttons.

| Window | What lives there |
|--------|------------------|
| Transforms | Add/duplicate/delete over a vertical tab rail (colour swatch, name, eye toggle, draggable weight bar, filter box past a dozen) plus a detail pane grouped Shape / Post-affine / Symmetry / Behaviour / Appearance, under a header whose name is click-to-edit |
| Explore | New random flame, new blank scene, mutate + strength, undo/redo, and the history list (click a row to jump N steps in one rebuild) |
| Render | Renderer mode, exposure, point size, colour falloff/contrast, haze, infinite zoom (incl. the zoom-map picker) — then point count, under its own heading |
| Camera | Framing, saved views, the camera path (keypoints, how it loops, playback) |
| Scenes (Ctrl+O, B) | Scene picker; the same selection the arrow keys walk. What File → Open… shows you |
| Controls (H, F1) | Orbit style, invert pitch, panel scale — then the keybind table, scrollable, rows clickable |

All six persist their open state to prefs, and all six behave alike.

**Panels group by persistence class, with headings that say so.** Three kinds
of value used to stack with nothing admitting it: session state that evaporates
at exit, preferences that follow the person across every scene, and scene data
that goes in the file. Nothing on screen distinguished a slider whose value
would be in your file tomorrow from one that wouldn't — and that gap is exactly
what let `exposure` go years without being saved at all. Group *within* a
window rather than splitting across windows: splitting would scatter things
that belong together by task, and the task is why the panel was opened.

Conventions worth keeping:

- **A choose-1-of-n setting is a segmented radio** (`src/ui/radio.rs`), not a
  row of toggle buttons. Three settings are genuinely one-of-n with no off
  state — the renderer (points/splat), the colour source
  (transforms/palette/mix) and how the camera path loops
  (once/ping-pong/loop/zoom) — and all three used to be drawn as loose
  `selectable_label`s, or in the camera's case as two checkboxes for a
  four-way choice. That draws a picture that isn't true: n independent toggles
  that could all be off, or all on. One connected pill with the chosen segment
  lit says the thing that's actually so. Each segment keeps its own `hinted()`
  tooltip and status-bar hint, and a segment can be greyed *in place* when it
  isn't available (the zoom loop without a zoom map), because a mode that
  isn't drawn can't tell you it exists.

  Each segment carries the ring-and-dot mark, filled on the one chosen — the
  same two `CircleShape`s egui's own `RadioButton` paints. **Drawn, not
  typed**: the obvious characters (U+2B58 `⭘`, U+25C9 `◉`) are in *none* of
  this app's fonts — Ubuntu-Light, NotoEmoji, emoji-icon-font, Phosphor, or
  Envy Code R — so they would render as tofu. `◉` exists only in Hack, which
  is the *monospace* family, and buttons are proportional. egui draws its own
  for the same reason; check `fc-list`/fontTools before putting a symbol in a
  label here.
- **Every numeric readout goes through `src/ui/num.rs`.** A house rule, not a
  per-site fix, and the one convention here whose absence no screenshot can
  show. `format!("{:.N}", v)` into a proportional label sized to its own
  content is unstable twice over: the *string* changes length as the value
  changes, and the *widget* is sized to the string. A value dithering around a
  boundary re-lays-out its whole row every frame, at up to 120fps, and it reads
  as a physical vibration of the interface. Four rules: **round to display
  precision before testing the sign** (`-0.004` is `"0.00"`, not `"-0.00"` —
  and note this is *not* IEEE negative zero, so a `v == 0.0` guard catches
  neither case); **monospace**, so digit advances are equal; **a declared
  character budget** per readout, right-aligned, picked from the range the
  value can actually take; and **size the widget, not just the string**
  (`add_sized`), because a short string in a content-sized widget still shrinks
  the widget. The last is the one that can't be done by formatting alone, and
  it is what stops dragging **x** through zero from shoving **y** and **z**
  sideways while your pointer is on the control.

  The same module's `icon_button_width` / `text_button_width` do this for a
  *button* whose caption changes length between states (Edit Mode/View Mode,
  Play/Pause, the point-count readout). **Pin the width, then pin the caption
  to one end of it**: egui centres a button's atoms inside `min_size`, so a
  widened button holds its neighbours still while its own icon and text slide
  by half the difference every time the caption changes — the same jitter,
  moved inside the control. Append an `egui::Atom::grow()` after the text to
  spend the slack on the right. Left rather than centred because the icon is
  the part you aim at, and a column of buttons whose icons don't line up reads
  as a column that is subtly misaligned.
- **Destructive confirmations use `ui::confirm::danger_button`.** One widget,
  five steps, and every step is visible:

  1. Click it. It becomes **disabled and greyed**, reading `waiting (3)`.
  2. It counts down, taking no input at all.
  3. At zero it becomes **enabled again in danger colours**, with a different
     and more explicit label — `Cancel` → `Abort render`, `Discard` →
     `Discard edits`.
  4. **It sits there.** Nothing expires.
  5. **Clicking anywhere else puts it back**, with no other effect.

  The disabled countdown is the load-bearing part. A plain two-click guard is
  defeated by a double-click — the commonest accidental mouse input there is —
  and the obvious fix, *ignoring* clicks during a wait, is worse than it looks:
  a button that silently swallows input is indistinguishable from a broken one,
  and the person this guard exists for is precisely the one clicking fast. A
  disabled control with a number ticking on it is the affordance everyone
  already has from download and install dialogs: it says "not yet, and here is
  exactly how long". The second click then lands on a control that has visibly
  *become available*.

  Step 4 is why there is no arm window. A timeout is a clock guessing at whether
  you are still engaged, and it guesses wrong in the dangerous direction — too
  short and your confirming click quietly does nothing, too long and a live
  one-click trigger outlives your attention. Clicking elsewhere is the same
  signal, measured instead of guessed. It also disposes of the
  degraded-machine worry that shaped an earlier design: if the box has stopped
  repainting you just wait longer for the frame that draws the confirm, and it
  is still there when it arrives.

  All three states are drawn in **one fixed-width cell**, sized to the longest,
  so the row doesn't re-lay-out under a pointer that is aimed at it. The icon
  (`danger_button` takes it as its own argument) leads all three captions, the
  countdown included: it names the action, which doesn't change while the
  button arms, and a glyph coming and going between states would be the control
  flickering at you during the one interaction where it must look deliberate.

  Note that confirmation-as-a-*checkbox*
  (Save-As's overwrite acknowledgement) is inherently safe here in a way
  confirmation-as-a-second-click is not — the second click of a double-click
  toggles a checkbox back off.
- **Every interactive widget goes through `hints::hinted()`**, which attaches
  a tooltip *and* sets the status bar's left-hand hint while hovered. The one
  documented exception is a bare camera drag on empty viewport space, which
  has no widget to hang a hint on and falls back to `HINT_VIEWPORT`.
- **The status bar's right side is the performance instrument**: FPS, mean
  frametime, p99 frametime and a 120-sample sparkline, plus live point stats.
  p99 is the number to watch — mean FPS hides the stutters.
- **The Transforms panel is a tab rail, not a list above fields.** The active
  tab is filled with the detail pane's own colour and runs past the rail's
  right edge, with the rail recessed behind it — that continuity is what makes
  it read as selector-and-detail. Don't reduce it to a highlighted row; a
  highlight that never touches the fields can't say which fields it owns.

  Two things about the rail are load-bearing and easy to undo by accident.
  **A tab must occupy exactly `TAB_HEIGHT`** (`ui::transforms`, enforced with
  `set_min_height`), because that is the row pitch the rail's virtualization is
  told about, and a tab that lays out shorter mis-virtualizes the list: egui
  draws `viewport / TAB_HEIGHT` tabs and stops, so rows past that are
  unreachable, the drawn ones leave a band of dead rail below them, and the
  scrollbar sizes itself to a content height the content doesn't have. Three
  symptoms, one number, and none of them looks like the same bug from outside.
  **And the list takes `ui.available_height()`**, not a constant — a fixed
  height means growing the window grows empty rail instead of showing more
  transforms.

  Add / duplicate / delete sit *above* the list. They act on the list, so they
  belong at one of its ends, and the end that doesn't move is the top; below it
  they sat wherever the list happened to stop.
- **A transform's name is edited where it is read.** The detail pane's header
  name is a label until clicked, then a text field in the same spot
  (`transforms::draw_header_name`). There is no "Identity" block — a heading
  and a rule around one field, five headings down a pane you have to scroll, is
  furniture that doesn't earn itself, and it put the most-edited property of a
  transform as far as possible from the header already displaying it. The
  action row's Rename button stays, because click-to-edit is invisible until
  you hover the right four words and the button is what says the gesture is
  there.
- **A control stacked on top of another says where it is.** The weight bar
  along a tab's bottom edge is registered after the tab and so takes the
  pointer inside its own strip — which means every pixel it covers is a place
  where clicking does not do what the tab under it advertises. So the strip is
  small (`BAR_GRAB`, centred on the drawn bar) and it *shows* itself on hover,
  growing into a full-width track with the value filled in. It was 40% of the
  tab, silently, with only the cursor to say so — and the cursor changes at the
  top of the screen, nowhere near the thing it is talking about.
- **Gizmo indicators** (`src/indicators.rs`) draw the selected transform's
  relationship to the grey identity cell: an offset vector from the world
  origin with an arrowhead and a length label, and the rotation as an axis
  through the origin plus an arc sweeping its angle. Euler angles remain the
  *editable* representation (three fields you can type into); axis-and-arc is
  the *readable* one. The indicator pass must store depth, not discard it —
  the gizmo pass depth-tests against the same buffer right after.
- **Inspector fields are Mat4 <-> TRS.** A matrix that doesn't decompose
  faithfully (shear, or a mirrored det<0 matrix) routes to a raw 3x4 grid
  plus an "Orthogonalize -> TRS" button. Mutations produce such matrices, so
  this path is load-bearing, not a corner case.
- **Variation weights may be negative** (Apophysis-style: the blend is
  `out += w * f(p)`), and a row stays put at 0 so a drag can pass through it.
- **Point count is a render property, not scene data.** `App::buffer_capacity`
  owns it and it persists to prefs. Startup precedence is `--points` > prefs >
  the scene file > default. The offline `--render` path never reads prefs, so
  CLI renders stay reproducible from flags plus scene. The Render window's
  control is a *logarithmic slider that applies live*, rate-limited to one
  reallocation per 250ms: it's the dial that decides whether the machine stays
  responsive, so you must be able to feel it load up under your hand and drag
  back, not commit blind to a number and find out afterwards.
- **A panel window has a minimum size, and the height one is load-bearing**
  (`ui::WindowKey::min_size`). A window too short for its fixed furniture
  hands its flexible middle a *negative* height; the middle draws anyway, over
  the rows pinned below it by `egui::Panel::bottom` — and it takes their
  clicks, because egui gives the pointer to whichever widget was registered
  later and a bottom panel's contents are registered before the body that
  follows. The buried controls still paint, still highlight on hover and still
  show tooltips, so this reads as "that control is broken" rather than "that
  window is too short". The Camera window is the one that hits it. Widths are
  only a legibility floor — a narrow row clips, which is graceful.
- **Panel geometry persists** to `prefs.window_geometry` (see `ui::WindowKey` /
  `ui::remember`); `ui::default_layout` is only what you get before you've
  moved anything. Writes are deferred by `App::flush_dirty_prefs` so dragging a
  window doesn't rewrite prefs.toml every frame.
- **The status bar's `ui Xms`** is egui's own build+tessellate cost. Note that
  it rises when the *GPU* is saturated too (at 110M points it went 2.4ms ->
  27ms), so a high reading isn't automatically the panels' fault. For a
  per-panel breakdown run with `FRACTURIZE_UI_PROFILE=1 RUST_LOG=info`.
- **All edits funnel through `App::commit_edit`** (`src/history.rs`), which
  coalesces same-key edits inside 1s so a held key or a whole drag is one
  undo step. Camera *moves* are deliberately not history entries; camera
  *paths* are, all six operations of them — adding a keypoint is a discrete
  authoring act that changes what Ctrl+S writes. History is capped by entry
  count and by bytes, but the byte cap stops at ten entries (`MIN_ENTRIES`)
  rather than grinding down to one on a 40k-transform scene, and it counts what
  it dropped so the Explore list can say so instead of quietly ceasing to be
  the beginning.
- **A disclosure hides elaboration, never the headline.** Haze gets this right:
  the amount is always visible and only the band's pin/near/far fold away. If a
  section is closed while its feature is *on*, the disclosure is hiding the
  wrong thing — which is why infinite zoom opens by default once a scene has a
  zoom map.
- **Disabled, not hidden, with the tooltip saying how to un-disable.** A
  control that vanishes cannot tell you the feature exists. `hinted()` handles
  the greyed path explicitly, because egui silently drops `on_hover_text` on a
  disabled widget — so every "disabled rather than hidden, because it says why"
  control in this UI was at one point saying nothing at all.
- When testing anything that touches prefs, set an isolated `XDG_CONFIG_HOME`
  rather than writing the developer's real `~/.config/fracturize/prefs.toml`.

### Deferred on purpose

Not oversights. Each is a real gap, and each is a deliberate no-for-now — an
undefended absence reads as an accident, so they are written down:

- **Multi-select.** One transform at a time; moving three arms together isn't
  possible. A real gap for the modelling workflow, and the largest of these.
- **Numeric entry mid-drag** (Blender's `G`, then type `2.5`, Enter). A deep
  Blender idiom, but the fallback here — drag roughly, then type exactly into
  the inspector — is acceptable.
- **Transform reordering.** Meaningless for an IFS: order doesn't affect the
  attractor, weights do. This one is deferred permanently.
- **A menu bar.** See the File-menu note above.
- **The 3×3 mutation grid** — Apophysis's signature exploration UI, showing
  eight perturbations around the current flame so mutation becomes *look,
  choose* rather than *roll, judge, undo*. The highest-value single addition
  left, and the largest job: it needs offscreen thumbnail rendering, which
  `render_job.rs` / `offline.rs` can already do at arbitrary point counts.
  Eight 200×200 thumbnails at low point counts are cheap next to a 50M-point
  live view. Still on `todo.txt`.

### The chrome doesn't move

Small conventions, easy to break by accident because breaking each one looks
locally reasonable. Read the module before adding a widget that resembles
what it owns — each is a precedent, not a helper you can route around:

- **A numeric readout never resizes itself** (`src/ui/num.rs`). Round to
  display precision *before* testing the sign (`-0.004` at two places is
  `"0.00"`, not `"-0.00"`, and this is not IEEE negative zero so `v == 0.0`
  doesn't catch it), use a monospace face, and size the *widget* to a
  declared character budget — not just the string. A bare
  `format!("{:.2}", v)` dropped into a content-sized label is a regression:
  a value dithering across a boundary re-lays-out its row every frame and
  reads as the interface vibrating.
- **Every interactive widget is hinted** (`src/ui/hints.rs`). `hinted()`
  attaches a tooltip and sets the status bar's left-hand hint in the same
  call, and it works on disabled widgets, where egui's own `on_hover_text`
  silently shows nothing. A widget with no `hinted()` call is one nobody can
  find out about — the one documented exception is a bare camera drag on
  empty viewport space, which falls back to `HINT_VIEWPORT`.
- **A destructive action is armed, not asked-to-confirm**
  (`src/ui/confirm.rs`). `danger_button` disables the control for a fixed
  countdown, then makes it available in danger colours with a different,
  more explicit label; clicking anywhere else disarms it, and nothing times
  out once armed. New destructive actions route through this, not a bespoke
  are-you-sure popup — see the module doc for why a plain two-click guard
  and an arm-window timeout both fail in the dangerous direction.
- **Choose-1-of-n with no off state is a segmented radio** (`src/ui/radio.rs`),
  never a row of toggle buttons. A row of independent toggles can (as drawn)
  all be off or all be on at once; a control that always has exactly one
  answer shouldn't be able to draw a picture that isn't true.
- **Icons name actions and windows, never domain objects** (`src/ui/icons.rs`).
  A transform is identified by its colour swatch and name, not a glyph.
  Icons live on the things you click to *do* something — toolbar toggles,
  buttons — not on the things the scene is made of.

**Zero animation, ever.** This is a realtime renderer: the fractal is the
thing on screen that's supposed to be moving, every frame, continuously.
Chrome that also moves — a menu fading in, a panel easing open, a scrollbar
coasting to a stop — competes with it for attention, and in a program whose
entire point is instantaneous visual feedback, chrome with its own
independent sense of time reads as the UI lagging behind the click that just
landed. So: no fade-ins on menus, popups or tooltips, no easing on
collapsing disclosures, no smoothed scroll-to. A click's effect is drawn in
full on the very next frame, or it hasn't happened yet.

Enforced once, not per call site: `EguiLayer::new` (`src/ui/mod.rs`) sets
`Style::animation_time = 0.0` and `Style::scroll_animation =
ScrollAnimation::none()` via `ctx.all_styles_mut`, at startup. The first
field is the one egui checks everywhere it would otherwise interpolate —
window/menu/tooltip fade-in (`Area`'s `fade_in`, which every popup and menu
is built on), collapsing headers, side panels, scrollbar fade-and-grow — all
of it funnels through `animate_bool`/`animate_value`, which divide elapsed
time by this field and snap to the target when it's zero. The second field
is separate because `scroll_to_me`/`scroll_to_rect`/`scroll_to_cursor` (e.g.
the Scenes browser snapping the selected row into view) glide on their own
`ScrollAnimation { points_per_second, duration }`, not on `animation_time`.

One thing egui 0.35 hands us no knob for: a mouse wheel's discrete notches
are low-pass filtered over several frames before egui ever sees them
(`WheelState::after_events`, hardcoded, not read from `Style`) — that's
input smoothing so one loud notch on a mouse doesn't register as fourteen,
not a UI animation, and it only touches wheel-scrolled lists (Scenes,
Keybinds). Anything with its own `.animate(true)` builder flag (`egui::
ProgressBar` has one) bypasses `animation_time` entirely and has to be
turned off at the call site. The render-job dialog is the one place in the
app that still animates, and it does so deliberately, in exactly one spot:
its **encode** bar pulses while encoding is live. That phase reports no
progress at all — `rav1e` defers virtually all its work to the flush, so
`offline.rs` sends the `"encoding"` phase and then nothing until it's done —
and a pulse is the one case where motion carries information a static bar
cannot: that work is happening, with unknown extent. The render bar next to
it always has a real fraction and does *not* animate. If encode progress
ever gets plumbed through (`src/avif.rs`'s `drain()` already knows its own
done/total), the pulse should go with it and this exception disappears.

## Random Flames

`--random` (windowed or with `--render`) and the Explore window's dice button
both call `randomize::random_flame`. Randomising an IFS is easy; randomising
one that renders is not, so every candidate runs a short CPU chaos game
(`trace.rs` walkers, the same variation port the shader uses) and is rejected
unless it lands in a bounded attractor with real extent on two axes and
*fractal* rather than solid occupancy. Up to 20 tries, then the last roll is
kept so the button always returns something.

`--random --seed N` is reproducible byte-for-byte; the seed is printed either
way, so an interesting roll can always be recovered. `spherical` is never
rolled (its 1/r^2 inversion blows scenes out — fine by hand, bad by dice).
A rolled flame is a normal history entry: one Ctrl+Z restores what was there.

Colour is rolled too, not just form. The colour *source* is one of all three
modes — `transforms` half the time (the per-transform RGBs are generated for
that ring), `palette` and `mix` a quarter each; a palette-mode roll rolls its
gradient from `palette::random`. The **background** is rolled from a colour
the flame actually renders in (one of the transform colours, or a sample of
the gradient), hue-rotated in Oklab — usually within the flame's own family,
sometimes to its complement. Two thirds of rolls are dark and saturated; the
rest are mid and near-neutral, and deliberately stop short of white, because
the point pass composites the flame over the background by coverage and a
paper-white ground would swallow colours rolled at value 0.7–1.0. Override any
of it with `--palette` / `--random-palette` / `--color-mode`, which apply after
the roll.

## Starting From Nothing

`--blank`, and the Explore window's "Blank scene", give you `Scene::blank()`:
two plain half-scale transforms at ±(0.5, 0.5, 0), no rotation, no variations,
default everything else. Two rather than one because a single contracting map
converges to a point — two give a visible line of dust with each transform's
own colour on its half, so there's something on screen to build against. Like a
random flame it's an *edit*, so one Ctrl+Z brings back what was there.

From there: `Shift+A` (or the Transforms window's "+ add") adds a transform,
`A` / "dup" duplicates the selected one, `Delete` removes it, and right-clicking
either a row in the Transforms window *or* a gizmo in the viewport gives the
same menu — duplicate, enable/disable, delete, rename. A scene always keeps at
least one transform (the chaos game needs somewhere to send the point), so the
last Delete is disabled rather than hidden.

**A scene's name and author** are editable from the toolbar: the readout on the
right is a button, and clicking it opens fields for both. The name is not just
a caption — it's the slug behind `views/`, screenshot and render filenames — so
"Save as…" takes a name too, prefilled from the filename you type and following
it until you edit it. A fork that kept the original's name would leave the
toolbar saying "Koru" while you worked on `koru-v2.toml`. The author is
remembered in prefs and fills itself in on scenes started in-app.

## Background & Transparent Output

`background` is scene data (linear RGB, see "Scene Files"), picked in the
Render window and undoable. It reaches both renderers as the pass clear value
and, for splat, as the colour the tonemap composites against.

The splat tonemap **composites** rather than adds: `mix(background,
mean_color, clamp(brightness))`. It used to be `background + mean_color *
brightness`, pure emission, which is only right when the background is black —
put a fractal on a light background that way and every pixel clips to white.
Treating log density as coverage fixes that and collapses the two output modes
into one model: an opaque render is now exactly the transparent one composited
over the background. On the near-black default the difference from the old
formula is ~1% RMSE, so existing scenes look the same; `--render` of a *points*
scene is byte-identical.

`--transparent` (and the Render window's checkbox, which covers `S` screenshots
and render jobs) writes an alpha channel: the clear alpha goes to 0, the
points renderer's own alpha marks where points landed, and the splat renderer
writes straight-alpha coverage so dusty edges stay dusty instead of becoming a
cutout. The live window is always opaque — its swapchain has nothing behind it.
**Not supported for animation**: frames are converted to YUV from r/g/b only,
and neither AV1 nor H.264 carries an alpha plane here, so `--transparent` with
an `.avif` or `.mp4` output errors rather than quietly producing opaque video.

Save-as / fork is `Ctrl+Shift+S` or the Camera window's button — a small modal
(`src/ui/save_as.rs`), not a native dialog: one text field doesn't justify an
`rfd` dependency when the app already browses `scenes/` with `B`. It refuses to
overwrite without an explicit tick, because undo only covers the scene you have
open.

## Haze

Depth cue, and the only one this renderer has — additive point clouds have no
shading and no occlusion, so nothing else says which arm is in front, and
without it the 3D projection stops reading as one. One control
(`src/haze.rs`), `haze` 0–1, scene data, undoable, saved by Ctrl+S.

**It is aerial perspective, not fog.** Distance costs a point *transmittance*
— it contributes less, so the pixel resolves toward the background — and
*saturation*. It used to multiply the colour toward black instead, and that is
only indistinguishable from distance when the background is already black:
against a pale background it made far material darker and therefore
higher-contrast, which reads as *nearer*. Even on the dark default it punched
holes, since the far tail of an arm went to black, which reads as absence. The
name changed with the behaviour; the scene key reads `fog` as an alias, and so
do view files (`fog_near`, `fog_brightness`, …).

How each renderer applies it:

- **splat** scales the point's deposited *density*, so the tonemap's coverage
  falls and the pixel composites toward the background — no knowledge of the
  background needed;
- **points** is opaque and depth-tested, so it mixes toward the background
  colour directly, which is why `CameraUniforms` carries the background. When
  the pass is writing a file with an alpha channel it spends the haze as
  *transparency* instead — the point's own colour at `alpha = transmittance`.
  Fading toward a background that isn't going to be there would bake it into
  the far material, which is the thing transparency exists to avoid.
  `CameraUniforms.transparent` is how the shader knows which it's doing; the
  window is always opaque, since its swapchain has nothing behind it.

The shader's four parameters are *not* the user's parameters:

- the near/far band auto-ranges off the camera distance (`haze::auto_band`), so
  it follows the framing instead of being re-dialled on every zoom. The Render
  window's "haze band" disclosure can pin it to fixed world units;
- transmittance and saturation falloff come from the amount
  (`haze::falloff`), linearly: amount 0.5 leaves half the contribution, and
  **amount 1.0 dissolves the far plane completely** — into the background for
  an opaque render, into transparency for one with an alpha channel.

`--fog` is a legacy on-switch meaning "on at the old default strength"; a
scene's own `haze` wins over it. Views written before this carry the four raw
values and are converted on load (`haze::amount_from_brightness`). Random
flames get a little haze by default; hand-authored scenes default to none.

## Scene Files (TOML)

Use `--scene <path>` to load. See `scenes/` for examples.

```toml
[meta]
name = "Scene Name"
author = "Your Name"       # agents: sign with your model name ("Claude Fable 5",
                           # "Claude Opus 4.8"), not just the family name
point_size = 0.002        # world-space point size
point_count = 4_000_000   # circular point buffer capacity (default 500k)
color_speed = 0.5         # global color blending speed (0-1); used when color_falloff = 0
color_falloff = 0.0       # scale-aware color accumulation exponent (0 = off, ~1 neutral)
color_contrast = 1.0      # render-time cyclic palette contrast stretch (1 = off)
haze = 0.0                # aerial-perspective depth cue, 0-1 (see "Haze";
                          # reads `fog` as an alias, its old name)
exposure = 1.0            # splat-renderer brightness. Scene data because it is
                          # the splat renderer's primary look control — before
                          # it was, scenes recorded it in a *comment* saying
                          # which --exposure to pass. Ignored by `points`.
background = [0.02, 0.02, 0.05]   # LINEAR rgb behind the fractal. Linear, not
                          # sRGB: this is the clear value, and 0.02 reads as
                          # sRGB #282a45. Use the Render window's picker.

[camera]                  # optional
focus = [0.0, 0.0, 0.0]   # orbit center / look-at
distance = 3.0            # orbit radius (true eye-focus distance)
yaw = 0.0                 # orbit angle around Y, radians
pitch = 0.32              # orbit elevation, radians (positive = above)
roll = 0.0                # optional: rotation about the view axis, radians
                          # (omitted when level; right-drag sets it in-app)
# rotvec = [x, y, z]      # optional: the exact framing, as a rotation vector
                          # in radians. Wins over yaw/pitch/roll, and is what
                          # gets written for a framing at the poles — where
                          # yaw and roll are the same control and the three
                          # angles stop naming it. Ordinary scenes never see
                          # this; it exists because the camera can now be
                          # pointed straight up.
# legacy: offset = [x,y,z] (eye displacement) still loads, but is folded
# into the framing/distance and never written back
path_loop = "closed"      # how playback gets from the last frame back to the
                          # first. One of:
                          #   "once"     play through and stop (the default)
                          #   "pingpong" out to the last key, then back again
                          #   "closed"   loop back to key 1 (seamless)
                          #   "zoom"     close under the [zoom] symmetry, so the
                          #              animation loops as an endless zoom
                          # See `path::Loop`. Legacy `path_closed = true` and
                          # `path_zoom_loop = N` still load and are migrated on
                          # save; neither is written back.
path_zoom_periods = 1     # zoom periods descended per loop, with path_loop =
                          # "zoom" (default 1; see "Infinite Zoom")
path_seconds = 14.0       # playback/render duration (default 3s per segment
                          # *travelled* — which a ping-pong does twice over)
path_ease = false         # smoothstep time; default: once and pingpong ease,
                          # the two closing loops don't

# Camera path spline keypoints (2+ = a path; see src/path.rs). A uniform
# Catmull-Rom spline runs through the keys, in *cumulative* form so it splines
# framings rather than angles: yaw is unbounded (keys spanning 2*TAU author a
# two-turn corkscrew — nothing wraps), distance interpolates in log space
# (constant-relative-rate zooms), and focus travels on its own spline so look
# directions blend smoothly while the eye moves. Omitted fields inherit the
# base [camera] framing. Closed paths take the shortest yaw route back to key
# 1. In-app: Y appends the current framing as a keypoint, Shift+Y removes,
# O or Z flies it; the Camera window's Loop radio picks any of the four. All
# six path operations are undoable.
#
# ROUTES. Which way round a segment goes is data, not something re-derived
# from its endpoints — that is what used to make a 1° change swing 359° the
# wrong way. Normally the yaw column says it: keys at 0, 3.14, 6.28 author a
# full turn, and that is read once at load and kept. A key can also carry
# `turns = N` to state it outright, which is written only where the yaw
# column can't (the closing segment of a loop, or a key written as `rotvec`),
# so a hand-edited yaw can never contradict a `turns` beside it. Whatever the
# route says, the keys themselves are always hit exactly.
#
# A key may instead carry `route = [x, y, z]` — the segment's journey as a
# rotation vector, winning over `turns` where both are given. This is for the
# routes an integer cannot name, and only those: between two *equal* framings
# no axis is implied, so "pitch three full turns and come back" is unsayable
# as a winding (it collapses to a yaw loop). The catch of storing a
# displacement is that it can contradict its keys, so it is checked as the
# scene loads — `exp(route)` must land on the next key within half a degree,
# or the load fails naming the key and the miss. No UI writes one; it is a
# file-format door.
#
# A key may use `rotvec = [x, y, z]` instead of yaw/pitch/roll, on the same
# terms as [camera] above: exact, no poles, and what gets written for a
# keypoint framing the three angles can't name.
#
# EVERY SCENE HAS A PATH. Omit these keypoints (or author fewer than two) and
# the path is a seamless full orbit around the current framing, at 0.18 rad/s
# — the "turntable". That default is not a second system: it is a real
# `CameraPath`, it draws, it plays, and `--render x.avif|x.mp4` flies it, all
# through the same code (`path::resolve`, used by both `App::camera_path` and
# src/offline.rs). Editing it — a keypoint, the loop flag, the duration — is
# what turns it into scene data; until then no scene grows a path it never
# asked for, and Ctrl+S writes no [[camera.path]] block.
#
# Paths are drawn in the viewport when gizmos are on (G): the eye's route as a
# green polyline with a cross at each keypoint — but only while the path ISN'T
# playing. During playback the camera stands on the line, so drawing it is a
# smear across the shot that says nothing; it reappears the moment you take the
# camera back by hand, which is when the route is what you're positioning
# against.
[[camera.path]]
yaw = 0.0
pitch = 0.9
distance = 5.5

[[camera.path]]
yaw = 3.14                # half a turn later...
pitch = 0.1
distance = 1.6            # ...swooped in close
focus = [0.0, -0.5, 0.0]  # looking lower
roll = 0.4                # ...and tilted

[zoom]                    # optional: infinite zoom (see "Infinite Zoom")
map = "whorl"             # a transform name, or its index as a string
radius = 4.8              # outer radius of the band, in camera distances
levels = 15               # octaves rendered below it
edge_guard = 1.0          # octaves the picture's outer edge fades over (0 = hard)
octave_falloff = 0.0      # point-budget falloff per octave (power of the scale)

# Symmetry: a finite rotation group applied to named motifs (see "Symmetry").
# Repeatable — a scene can hold several groups over different motifs.
[[symmetry]]
group = "Icosa"           # Cyc<n> | Dih<n> | Tetra | Octa | Icosa. Cyc is
                          # n-fold about an axis, Dih adds the half-turn flip,
                          # and the three polyhedral groups are the rotations
                          # of a tetrahedron (12), a cube (24) and an
                          # icosahedron (60). The mathematical C<n>/D<n>/T/O/I
                          # parse too, and always will
axis = [0.0, 1.0, 0.0]    # Cyc/Dih only; the polyhedral groups have no single
                          # axis and are generated in a canonical orientation
mirror = false            # extend by the central inversion, doubling |G|
applies_to = ["petal"]    # the motifs, by name (or index as a string)
color = "shared"          # "shared" (every copy the motif's colour) or
                          # "orbit" (index offset by the drawn group element)

# The same block, in its other form: a repeat instead of a group. `repeat` and
# `group` are mutually exclusive, and a repeat is NOT a symmetry — see below.
[[symmetry]]
repeat = 20               # how many copies, counting the motif itself
axis = [0.0, 1.0, 0.0]    # the axis `turn` turns about
step = [0.0, 0.135, 0.0]  # slide per copy
turn = 137.5              # degrees per copy; 137.5 is the golden angle
shrink = 0.9              # size multiplier per copy, 0 < shrink <= 1
applies_to = ["frond"]

[[transform]]
name = "whorl"                 # optional label shown in overlays
translation = [0.0, 0.0, 0.5]
scale = 0.5                    # uniform, or per-axis: scale = [0.05, 0.6, 0.05]
rotation = [0, 0, 0]           # Euler degrees (XYZ)
# rotvec = [x, y, z]           # optional: the exact rotation, as a rotation
                               # vector in radians. Wins over `rotation`, and
                               # is written where XYZ euler can't reproduce
                               # the matrix — a rotation near a quarter turn
                               # about Y reads back as [179.2, 87.6, -179.3],
                               # correct but one rounding error from wrong.
                               # tools/lsystem_to_ifs.py emits it too.
color = [1.0, 0.2, 0.2]        # contributes to the cyclic colormap
weight = 1.0                   # selection probability
color_value = 0.25             # optional explicit colormap index (0-1)
color_speed = 0.5              # optional per-transform override (wins over color_falloff)
# Nonlinear variation blend; omit for classic affine ({ linear = 1.0 })
variations = { swirl = 0.35, linear = 0.65 }
```

Per-axis scale makes L-system-style geometry expressible: a transform with
`scale = [0.06, 0.5, 0.06]` squashes the whole attractor onto a thin vertical
segment — the "visible trunk" trick for IFS trees. `tools/lsystem_to_ifs.py`
turtle-interprets a bracketed 3D L-system production (F draw, X recurse,
+-^&/\ turns, [] push/pop) and emits a scene this way; its `--depth N` flag
expands to all length-N transform words (same attractor, N^branches
transforms) for stress testing. Transform count is a storage buffer with no
hard cap — tested to ~40k transforms; selection is a binary search over
cumulative weights, so even thousands of transforms fill at full speed.

Available variations (slot order in `scene.rs` / `chaos.wgsl`):
`linear, sinusoidal, spherical, swirl, horseshoe, polar, disc, spiral,
hyperbolic, diamond, julia, bent, fisheye, bubble, cylinder, tangent,
absfold, boxfold, spherefold, bulb`

The last four are fold/escape-time imports (all bounded):
- `absfold` — KIFS reflect-into-positive-octant (McGraw 2015). Its output
  always has non-negative components, so the attractor gets flat facets on
  the fold planes — deliberate crystal walls, not a bug (see
  `scenes/stellate.toml`). Pair with affine rotations for kaleidoscopes.
- `boxfold` / `spherefold` — the two Mandelbox operators (fold off ±1 walls;
  sphere-invert with minR2 0.25 / fixR2 1). Blended together on a transform
  with scale ~1 they make glassy plane-and-shard architecture
  (`scenes/shatterbox.toml`).
- `bulb` — power-8 mandelbulb angle map, radius-preserving (angles ×8, r
  kept, so it neither diverges nor collapses). High weight = misty pearl
  volumes with 8-fold mandala inclusions (`scenes/pearl.toml`).

Scene-design notes learned the hard way:
- Every point renders at full brightness (no log-density), so unbounded variations
  (`spherical` especially) spray faint fuzz everywhere. Prefer bounded ones
  (`bubble`, `fisheye`, `sinusoidal`, `swirl`, `julia`) or keep spherical weights low.
- `sinusoidal` with affine scale >1.4 saturates onto ±1 walls (box/room looks);
  scale ~1.1-1.2 with small rotations gives classic gnarl swirls.
- Colors wash out to pastel when transforms mix heavily; raise `color_speed`
  (0.5-0.7) for stronger per-branch color identity.
- `point_size` is world-space: size it against the *structure*, not by
  copying another scene. A scene whose attractor spans ~1 world unit needs
  roughly half the point_size of one spanning 2. Sanity check: keep
  `point_size ≤ 1.5 × camera_distance / window_height` or the renderer
  leaves the crisp 1px path at the default view on tall windows and every
  point becomes a multi-pixel billboard — strands turn to chunky ribbons
  (stellate shipped with 0.0025 and needed 0.0012).
- Diffuse volumetric scenes (broad clouds rather than thin filaments) saturate
  small renders into a solid pastel blob: every point is full brightness, so
  once density passes ~1 point/pixel all structure is gone. Judge such scenes
  at higher resolution or lower effort; filamentous scenes are unaffected.
  Or switch to the splat renderer (R / `--splat`): its log-density tonemap keeps
  structure visible at any density and such scenes generally look far better.
- Color variation follows *coarse* structure by default: the most recent transform
  in a walker's history decides which top-level copy a point lands in, and older
  iterations address progressively finer scales — but the fixed-rate color EMA
  weights recent history heaviest, so fine structure barely registers in color.
  `color_falloff` switches the EMA to scale-aware accumulation: each step retains
  `contraction^falloff` of the running color, so a step's color weight equals the
  spatial scale it controls, raised to `falloff`. Color variation amplitude then
  follows a pure power law of feature scale — self-similar coloring with detail
  at every scale and no resonant size. ~1.0 is neutral (≈ classic look for
  scale-0.5 scenes); 0.3-0.6 surfaces fine detail. Lower falloff compresses the
  palette range toward the mean — raise `color_contrast` (2-4) to re-stretch it
  (cyclic: large stretches also rotate hues when the mean index is off-center).
  Both are keybind-tunable live (D / C) and saved into view files.

## Symmetry

A transform can be a **motif** of a finite rotation group. For a group `G` and
maps `{fᵢ}`, the IFS `{g ∘ fᵢ : g ∈ G}` has an attractor that is exactly
`G`-symmetric — one line of proof (`hG = G`), and the reason two authored maps
under `I` are an effective map set of 120. `src/symmetry.rs` owns the groups;
`scenes/reliquary.toml` is the worked example.

**The group stays live; the orbit is never expanded.** A scene under `I` holds
two transforms and a group, not 120 transforms. So the panel shows two rows,
`pick.rs` has two things to hit, `--info` prints two maps, mutation operators
have two maps to perturb, and the file is eight lines. The complexity lives in
the one place that wants it — the walk — rather than in the twelve places that
don't. This is the whole design, and it is why symmetry is not a loader macro.

Mechanically it is **one matrix drawn uniformly per iteration**, applied after
`post_affine` (`shaders/points/chaos.wgsl`, mirrored in `trace.rs` — keep them
in sync, as ever). That is not an approximation of the `|G|·N` map set: picking
`fᵢ` with weight `wᵢ` then `g` uniformly *is* sampling `{g ∘ fᵢ}` with weights
`wᵢ/|G|`, so a motif keeps the share it was written with and convergence is
unchanged. Cost is one RNG draw and one matrix multiply.

- **The elements are generated by closure, not tabulated.** Two generators are
  multiplied until the set stops growing. Sixty hand-written rotation matrices
  is exactly the arithmetic that is silent when wrong — a near-group gives a
  near-symmetric attractor, which reads as a smear rather than as an error — so
  the tests check the group axioms and the known orders instead.
- **Symmetry is a property of a transform** (`TransformSpec::symmetry`), which
  is why it is a block in the Transforms inspector rather than a window of its
  own. `[[symmetry]]` in a scene file is sugar that names one group and its
  motifs, because that is how it reads best on a page; it resolves onto the
  transforms at load and is regrouped on save.
- **`|G|` counts toward the similarity dimension.** `Σsᵢᵈ = 1` becomes
  `Σ|Gᵢ|·sᵢᵈ = 1`. Without this every symmetric scene reads as `d = 0` — one
  contraction can never sum to 1 — which is the opposite of what it is.
- **The group supplies the gizmo's snap increment**: 72° under `Cyc5`, where an
  ordinary map snaps to 15°.
- **The names are spelled out**, `Cyc5` / `Dih3` / `Tetra` / `Octa` / `Icosa`,
  not the mathematician's bare `C5`/`D3`/`T`/`O`/`I`. The notation is correct
  and unreadable to anyone not already holding it, and both a scene file and a
  panel badge get read by people meeting the feature for the first time. The
  bare letters parse and always will, so notation-first authors lose nothing;
  `label()` emits the long form, so a save normalises to it.
- **In the viewport** (gizmos on, a motif selected): the axis with one spoke
  per fold for Cyc/Dih, the namesake solid as a wireframe for the polyhedra, and
  the motif's `|G| − 1` orbit ghosts drawn dimmed. Ghosts are deliberately not
  pickable — a ghost is an image of an object, not an object.
- **Symmetry alone is wallpaper.** An orbit distributes measure evenly by
  construction (every copy shares a contraction and a weight), which is the one
  thing splat cannot recover. `--info` raises a `notes` line when every map is
  in a group, and the panel puts "Add a map outside this group" next to the
  picker. See CRAFT §3.6: *symmetry gets you a form, one map outside the group
  gets you a picture.*
- **A `repeat` is not a group, and the code says so out loud.** `OrbitKind`
  holds five groups and one progression. For a group `hG = G`, so the attractor
  of `{g ∘ fᵢ}` is *exactly* `G`-invariant; a repeat is the truncation
  `{S⁰ … S^(N-1)}`, which is not closed (`S·S^(N-1)` is outside it), so its
  attractor repeats without being invariant. `--info` prints "a repeat, not a
  group", the panel badge says "repeats, not symmetric", and
  `OrbitKind::is_group` is the predicate. Everything downstream — the GPU
  table, the ghosts, the chaos walk — takes a list of matrices and never needed
  to know the difference.
- **A repeat's step is capped at `shrink <= 1`.** `Sᵏ ∘ f` has linear part
  `S_linᵏ · f_lin`, so a growing step makes the far copies expansive and the
  walk unbounded. No pictures are lost: a growing repeat of `N` copies from `m`
  is the same set as a shrinking one from `S^(N-1) m`.
- **A repeat is also the one case where `|G|·sᵈ` is wrong.** Its k-th copy
  carries `shrinkᵏ`, so the copies are not all the same size and the dimension
  sum is `Σₖ (s·shrinkᵏ)ᵈ`. `Symmetry::element_scales` is all ones for a group
  and a geometric run for a repeat; without it a tapering helix reads as `count`
  copies of its widest turn. For the same reason a shrinking repeat is exempt
  from the flat-measure complaint below — the taper already carries the variance
  a defect would restore.
- **The five rotation groups are all of them.** The finite subgroups of SO(3)
  are `C_n`, `D_n`, `T`, `O`, `I` and nothing else — a classification, not a
  selection, so there is no sixth polyhedral group to add. Anything genuinely
  new has to leave SO(3): reflections (the point groups, of which only `C_nv`
  and `T_d` are unreachable from the current `mirror`), translations (`repeat`),
  or conformal maps.
- **`color = "orbit"` is not "colour each copy".** With `g` redrawn every
  iteration it tracks the walker's most recent group element, so it reads as an
  interference pattern through the form rather than as `|G|` solid petals. A
  good look, but not the one the name suggests, so it is opt-in.

## Colour: two sources, one colormap

**The renderer has always been a palette renderer.** `chaos.wgsl` writes an
8-bit *index*, never a colour, and both point shaders resolve it against a
256-entry storage buffer. Colouring is three stages:

1. **Accumulate** — walker history → a scalar `c ∈ [0,1]`, an EMA over the
   transforms' `color_value`s at a rate set by `color_speed` / `color_falloff`.
2. **Map** — `c` → RGB, via the 256-entry colormap. **This is the swappable
   stage** (`src/palette/`).
3. **Grade** — the render-time cyclic contrast stretch, and haze desaturation.

Stage 1 is shared, so `color_speed`, `color_falloff` and `color_contrast` mean
the same thing whichever source fills the colormap.

| `color_mode` | Source | What it's for |
|---|---|---|
| `transforms` (default) | The per-transform RGBs, spread evenly around a cyclic ring | Reading IFS **structure** — each transform keeps its identity |
| `palette` | An independent gradient | Styling. Apophysis's model: structure and colour become separable jobs |
| `mix` | The per-transform RGBs, mixed as a **3-vector** along the walk | Telling transform *combinations* apart — see below. Skips stages 2 and 3 |

All three are kept deliberately. A gradient can express *an ordering* of
structure but can never *label* it — a 1-D index means only "the EMA landed
here" — so the transform ring stays as the lane for using colour to understand
a flame.

**A defect the transform ring has, and palette mode doesn't:** its stops sit at
`k/N`, so *adding a transform moves every other transform's colour*. Author a
scene you like with four maps, add a fifth, and all five have shifted. A
palette doesn't depend on the transform count at all.

`Scene::color_mode` selects the source; `Scene::regenerate_colormap` resolves
it. Call it after **any** colour edit and after adding or removing a transform.
`App::push_colormap` does that and re-uploads to the GPU — points keep their
8-bit index, so re-uploading 256 entries recolours the whole buffer on the next
frame without re-running the chaos game. That is what makes dragging a gradient
handle feel live.

**Palette mode needed zero GPU changes.** Not few — zero. Everything downstream
of stage 2 already worked the way Apophysis works.

### `[palette]` in a scene

Its **presence selects palette mode** — one mechanism, rather than a
`color_mode` key that can drift out of sync with it. Per-transform `color`
stays in the file regardless: it's still what `transforms` mode renders, so a
scene can carry both and be flipped between them with `--color-mode`. The one
exception is `enabled = false`, which keeps the gradient on file while
rendering the ring — without it the in-app A/B toggle would be destroyed by
every save.

```toml
[palette]
name = "ember"                # a library palette (--palettes lists them)...

# ...or authored here. `stops`, `cosine` or `entries`; the first present wins
# and all three override `name` (which then survives as provenance).
cyclic = true                 # index 255 wraps to 0. The default: the shader's
                              # lookup wraps and the contrast stretch assumes it
interpolate = "rgb"           # or "oklab" — perceptually even, no grey midpoint
                              # between complementary stops
stops = [
  { at = 0.00, color = "#0c040a" },
  { at = 0.38, color = "#b23818" },
  { at = 0.70, color = "#fcde9e" },
]
# cosine = { a = [...], b = [...], c = [...], d = [...] }   # Iñigo Quílez form
# entries = ["#…", …]         # 256 verbatim, for an import

rotate = 0.0                  # shift along the index; reverse applies first
reverse = false               # both sit on top, so a library palette can be
                              # tuned per-scene without forking it
```

**A stop's `color` is sRGB hex or linear floats.** `"#b23818"` is what a colour
picker shows; `[0.44, 0.04, 0.01]` is the linear triple the GPU gets, matching
per-transform `color` and `background`. Both parse; saving writes hex, because
a palette is a thing you look at. Everything that *displays* a palette (the
`--info` swatch, the GUI strip) encodes to sRGB first — which is why the strip
reads brighter than the per-transform swatches beside it. The strip is right:
it's what renders.

A malformed or unknown palette is a **load error**, not a fall-back to the
other mode — a scene that quietly rendered the wrong colours would be worse
than one that refused to load.

### CLI

```
--palette <name|path>          # library name, a file, or file.ugr#gradient-name
--color-mode transforms|palette|mix
--random-palette [cosine|harmony|library]   # honours --seed; prints the
                                            # [palette] table so a roll can be kept
--palette-rotate <t>  --palette-reverse  --palette-interpolate rgb|oklab
--palettes                     # list the library, and exit
--color                        # paint --info and --palettes with 24-bit ANSI
```

`--palette` restyles a scene without editing it, same spirit as `--zoom`.
`--info` gains a colour section: mode, source, a **12-stop hex ramp**, and the
luminance profile.

**Colour is opt-in, via `--color`, and it is additive.** The hex ramp is the
channel that works everywhere, so it is always there; `--color` adds a
continuous 24-bit ANSI swatch beneath it and paints each hex stop in the colour
it names. Nothing is replaced, so the coloured output is a strict superset of
the default and the golden files test what most callers actually receive.

This used to run the other way: the swatch was emitted unconditionally,
including into a pipe, on the reasoning that an agent which can see the
gradient decides better than one imagining it from floats. It cannot see it. An
agent reading Bash output receives the escape bytes as literal text — and they
are expensive text: 898 of blossom's 2837 bytes were the swatch line, and
because escape sequences are runs of digits and punctuation that BPE splits
near one token per one or two characters, that was a little over *half* the
report's tokens for a line rendering as nothing.

The same arithmetic runs the other way for whitespace, which is why `--info`
aligns its columns: a run of spaces merges into one or two tokens, so padding
every row of the report to a grid costs tens of tokens, not hundreds. **Buy the
alignment; sell the escape codes.**

Auto-detecting a TTY would get the same answer nine times in ten, and was
rejected: the same command would emit different bytes depending on how the
harness spawned it, a golden test would have to pin the environment, and an
agent in a pty-allocating harness would silently pay for a picture it cannot
see. A flag says plainly that colour is a human affordance; a heuristic
pretends the tool can tell.

### The library, and importing

`src/palette/library.rs` holds ~20 hand-authored gradients. flam3's ~700 are
**not vendored**: `flam3-palettes.xml` is GPL'd and this project's licence
isn't stated. Instead `src/palette/import.rs` reads what a flame user already
has — Apophysis/UltraFractal `.ugr` / `.gradient` (many gradients per file;
`color=` is **BGR**-packed and indices run 0..399, not 0..255), and a `.flame`'s
`<palette>` hex blob. Imported colours are display sRGB and are decoded to
linear on the way in.

Two rules every library entry follows, both enforced by tests:

- **Luminance has to go somewhere.** The renderer has no lights, so the palette
  *is* the shading. A gradient at one brightness renders flat however pretty
  its hues are.
- **Luminance rises and falls once.** The colormap is cyclic, so a monotone
  dark→bright ramp puts a hard seam at index 0. This is obvious in hindsight
  and invisible until you've rendered a hundred bad rolls.

`palette::random` enforces both on generated palettes via `score()`: each
generator rolls a dozen candidates and keeps the best. `U` mutates the palette
in palette mode instead of per-transform hue, which nothing downstream reads
there.

### GUI

The gradient strip lives in the Render window (`src/ui/gradient.rs`) and is
drawn in **both** modes — in `transforms` mode it's read-only, and being able
to see the colormap you're getting is worth having on its own. Ticks along it
mark each transform's `color_value`.

`color_contrast` was moved out of the slider stack to sit directly under the
strip, with a second thinner strip above it showing the colormap *after* the
stretch whenever it isn't 1. A designed palette compressed into an arc of
itself otherwise looks like a broken palette rather than a contrast setting.

In the Transforms window, palette mode replaces the per-transform RGB swatch
(which renders nothing in that mode) with the strip and a draggable marker for
that transform's `color_value`. That's the Apophysis idiom and the one piece of
UI that makes the model click.

### `mix`: three channels instead of one

Both other modes reduce the walker's history to **one scalar** and look it up.
That reduction, not the gradient, is where the information goes. A walker's
history is a word over N symbols, and a scalar can't distinguish most words:
arrive at the same EMA by two different routes and you get the same colour.
Three channels can. A walker that went through a red map and then a blue one is
purple, and *distinguishable* from one that went through two magenta maps —
so distinct transform **combinations** get distinct colours.

The plumbing costs nothing, because both ends already had room:

- `WalkerState` had three spare f32 pads; they're now `current_rgb_r/g/b`.
- `Point.color_idx` had 24 free bits above the 8-bit index; they now hold 8
  bits per channel. The index is still written underneath, so nothing else in
  the point path changed.
- `GpuTransform` gained `color_rgb: vec3<f32>` (which is what takes it from 160
  to **176** bytes — WGSL aligns the vec3 to 16, and Rust needs the explicit
  `_pad` to agree).

The mix is the *same EMA* as stage 1, run on a vector: `color_rgb = mix(...,
transforms[i].color_rgb, speed)`, so `color_speed` and `color_falloff` still
mean what they meant. Packing companies through a sqrt (`pack_rgb` in
chaos.wgsl, `unpack_rgb` in **both** render.wgsl and splat.wgsl, which must
stay in step) — 8 bits is coarse in the darks, and one multiply on read buys
that back.

Stages 2 and 3 are skipped: `camera.color_rgb_mode` makes `lookup_color` return
the unpacked RGB before it ever touches the colormap. **`color_contrast` does
nothing in mix mode** — it stretches a 1-D index and there isn't one. `--info`
and the GUI both suppress it there rather than showing a dead control.

Because the transform RGBs now reach the GPU as *data* and not just as a
colormap, `App::push_colormap` also calls `sync_transforms_to_gpu()`. Editing a
transform's colour in mix mode otherwise changes nothing on screen.

Mix has no `[palette]` table to be inferred from, so `[meta] color_mode = "mix"`
is the only thing carrying it across a save — the one place an explicit key
exists, and it outranks the palette-presence rule in both directions. An
unrecognised mode is a load error, not a silent fall-back.

## Infinite Zoom

An IFS attractor has a size. Zooming out runs off the end of it in a second;
zooming in runs out too, more insidiously, because the chaos game spends points
in proportion to the natural measure and a window a thousand times smaller gets
a thousandth of them. `[zoom]` removes both limits at once, exactly rather than
approximately. The derivation is in `src/renorm.rs`'s module docs; the short
version:

Nominate one of the scene's own transforms `f` — pure affine, contracting on
all three axes. It has a fixed point `p` and acts about it as a similarity
`A = s·Q`. Because `S ⊇ f(S)`, applying `f⁻¹` grows the attractor, and the
union of all such expansions

```
    S∞ = ⋃_{m ≥ 0} f⁻ᵐ(S)
```

is unbounded and *exactly* invariant under `f`. It is the same set at every
scale — no largest feature, no smallest, no privileged size. That is the object
`[zoom]` renders, and it exists for nearly any IFS, because nearly every IFS has
one plain affine contraction in it.

Two consequences do all the work:

- **Sampling costs one `round()`.** A chaos point at radius `r` from `p` belongs
  on the band at radius `R` after `m = round(log(R/r) / log(1/s))` applications
  of `f⁻¹`. Every point is recycled to the scale being looked at; none is
  discarded for having fallen too deep or too far out. `renormalize()` in
  `points/chaos.wgsl`. For a similarity the power `Aᵏ` is taken in closed form
  (`s⁻ᵏ`, and `k` times the rotation angle), so a gentle 0.95 spiral needing
  sixty periods to cross the band costs the same as a 0.5 one needing two;
  only the anisotropic fallback iterates. Measured on the reference desktop:
  a 40M-point chaos fill takes **0.08s either way**, and the live window holds
  120 FPS at 12M points with it on. It does not show up.
- **Zoom is periodic, so the camera never leaves f32.** Scaling by `s` and
  rotating by `Q` is a symmetry of `S∞`, so when the eye crosses the inner edge
  of the band, `Renorm::wrap` applies `A⁻¹` to eye, focus and up together. The
  camera is back at the outer edge looking at a pixel-identical picture. Nothing
  gets small, no precision is spent, and the point buffer is never regenerated —
  **the wrap is the level-of-detail system.**
- **The points are carried through the wrap with the camera.** The line above is
  exact for the *set* and not for a **sample** of it. Moving the camera by `A⁻¹`
  and leaving the buffer alone sends every dot on screen to a different pixel:
  the distribution is unchanged, so no light moves and mean frame brightness
  does not budge, but the whole dot field is resampled in one frame. On a dense
  scene that is invisible; on a sparse one, where individual points are
  resolvable, it reads as a twitch you can see and cannot name. `rewrap` in
  `points/chaos.wgsl` carries the buffer by the same `A⁻¹`, because
  `screen(A⁻¹x, A⁻¹C) == screen(x, C)`, and re-folds only what falls off the
  band's outer end — the octave `edge_guard` has already taken to nothing. What
  it costs is the octave assignment rotating by one, and `m` is written as a
  single power so it stays inside one band's worth however far the wrap jumped.
  Carrying camera *and* points with no re-fold would just be the wrap undone,
  and the precision would go with it. Measured by `tools/zoom_twitch.py`: on
  `astral_lattice` the seam went from 1.85× an ordinary frame step to 0.99×,
  and on `wellspiral` from 1.52× to 1.00×.

  It costs 0.93ms on an 8M buffer, once per period, and is at the memory floor
  (one streaming read and write of every point). Do not try to spread it:
  moving a point ahead of the wrap does not spare it, because what has to be
  preserved is its screen position measured from wherever it sits *at the wrap
  instant*, so an early move twitches twice and leaves the wrap's own twitch
  untouched. Doing the carry at draw time instead is exact and simple — it is
  only ever two matrices split at one radius — and measures 0.85ms **per
  frame** against 0.93ms per period. All three dead ends, with numbers, are in
  `NOTES-infinite-zoom.md`; the short version is that the pass does not show up
  in frame times on the reference desktop at all.

  Every renderer that flies a wrapping camera has to do this — the window
  (`App::wrap_zoom`), animation and stills (`effective_camera_folded`) — and all
  three carry by the **change** in fold depth, never by what `Renorm::wrap`
  returns. Along a path that return is the absolute depth of an unwrapped
  spline sample, so using it directly carries the buffer nine periods a frame.
  Same trap as the zoom readout's, one layer down.

The honest limit: the zoom is infinite *toward `p`*. Fly off sideways and you
leave the band and get the ordinary bounded attractor. Every self-similar zoom
has a centre; this one's is the fixed point of the map you picked.

### Using it

```toml
[zoom]
map = "descent"        # transform name, or its index as a string ("0")
radius = 4.8           # outer radius of the band, in camera distances
levels = 15            # octaves rendered below `radius`
edge_guard = 1.0       # octaves the picture's outer edge fades over (0 = hard cut)
octave_falloff = 0.0   # point-budget falloff per octave, as a power of `s`
```

- **Give the nominated map zero translation.** A map with no translation has its
  fixed point at the origin, which is where a zoom centre wants to be. Otherwise
  `p = (I − A)⁻¹b` lands somewhere arbitrary and the camera has to be aimed at
  it by hand — the CLI prints where.
- **`radius` is not a look control.** A wrap multiplies the eye's distance from
  the fixed point by `1/s`, so the distance at which the frustum wants material
  multiplies by `1/s` too — while the band's outer edge stays fixed in world
  space. Anything the old eye could see and the new one can't is simply gone,
  and it goes *at once, mid-flight, in the middle of the frame*. That is what a
  short band looks like: not a visible edge, but regions blinking out. The
  bound is `radius ≥ 1 + haze::FAR_FRAC = 2.42`, because haze has finished
  hiding material by an eye-distance of `FAR_FRAC × band`; the default is 4.8,
  and anything below the bound is reported by the CLI, `--info` and the status
  bar.

  **Raising it past the bound only helps a scene that has haze.** The bound is
  derived assuming haze takes material all the way to nothing, which happens
  only at `haze = 1.0`; below that a fixed fraction survives at the far plane.
  And more radius does not compensate, because the rendered set is
  scale-invariant *by construction* — the outermost octave subtends the same
  solid angle whatever `R` is, so pushing it out doesn't shrink it. Measured on
  `scenes/octave-edge-test.toml` (haze 0.12): the wrap loses 3.41% of frame
  brightness at `radius = 3.0` and 3.31% at `radius = 4.8`. What headroom
  actually buys is margin for a scene framed differently from how it was
  authored, and margin for haze to do its job. If your band's edge is visible,
  the fixes are haze and `edge_guard`, not radius.
- **`edge_guard` fades the outer edge to nothing, and is on by default.**
  Material more than `radius` eye-distances from the fixed point is weighted to
  zero at render time, ramping down over this many octaves — so the picture
  stops before the band's edge can, and there is nothing at the edge to lose.
  Leave it alone unless you are measuring; the interesting part is *why* it is
  a render-time weight rather than a fade on the band itself.

  **A fade baked into the band cannot work, and this used to be one.** The old
  `octave_fade` dealt the outer shells fewer points, ramping to a sixteenth at
  the rim. A wrap is an exact similarity, and it is invisible exactly where the
  point density is scale-invariant — i.e. flat. Anywhere density varies with
  radius, *the whole difference arrives at the wrap instant*. So a static fade
  spreads the change over **screen area** while leaving all of it in **one
  frame**, which is the opposite of what is wanted. Measured live on
  `octave-edge-visual`, the wrap spike went 35× the median frame step with a
  hard edge and 10× with three octaves of fade — and 10× was that design's
  floor, not a residual bug. `octave_fade` in an old scene still loads, and
  now sets this.

  **The guard is measured against the camera, which is what fixes it.** The
  weight is a smoothstep in `ln(|pos − p| / d)`, where `d` is this frame's
  eye-to-fixed-point distance. That ratio is exactly invariant under the wrap
  — the wrap scales both — so the wrap step is *zero by construction*, at
  every haze amount. And zoom progress is linear in `ln d`, so material crosses
  the ramp at a constant rate per octave of zoom: it leaves the picture at a
  steady pace instead of at a moment. It is the last stretch of haze, made
  mandatory and taken to zero, in ratio space — which is why a scene at
  `haze = 1.0` never had this problem.

  **Width, and why 1 octave.** The ramp ends at `radius` and starts an octave
  in, so at the default 4.8 it runs `[2.4, 4.8] × d` — and 2.4 is `MIN_RADIUS`
  almost exactly, so the ramp sits entirely in the part of the field full haze
  would have hidden anyway. At weaker haze it costs a *constant* dimming of the
  far field, which is invisible in motion because nothing about it changes.
  Asking for more than the band has room for is clamped (`--info` prints the
  resolved span, and says when the ramp reaches into the view).

  **`edge_guard = 0` restores the hard edge.** That is a measuring tool —
  `scenes/octave-edge-visual.toml` ships that way to demonstrate the artifact —
  and not a look.
- **`octave_falloff` acts on the opposite end of the band from `edge_guard`,**
  and they are easy to reach for interchangeably. Octave 0 is the outermost
  shell and octave `k` gets share `qᵏ`, so the falloff thins the *innermost*
  octaves — the small ones around the fixed point, which is the middle of the
  picture. On `octave-edge-test`, a falloff of 2 moves 40% of the material
  within 12% of the frame radius of centre and 25% at the rim. The guard only
  ever touches the rim.

  They cannot interfere: the falloff deals points and the guard weights pixels,
  so nothing in the deal knows the guard exists. That separation is forced,
  not tidy — the point buffer is circular and turns over at 1/800th per frame,
  so a deal that depended on the camera would mix thirteen seconds of stale
  camera positions into every frame.
- **Leave `octave_falloff` at 0 for anything that will be flown.** A wrap moves
  the octave filling the screen along by one, so if neighbouring octaves hold
  different numbers of points, the density on screen jumps every period.
  Measured on `wellspiral`: the discontinuity across a wrap is 1.9× an
  equal-sized camera move at falloff 0, and 3.2× at falloff 2. It stays a knob
  because it does even out density in a *still*, which never wraps.
- `levels` is how far in the band extends, **in octaves** — not in zoom
  periods, which are however big the chosen map happens to be (0.07 octaves for
  a 0.95 spiral, 3.3 for a 0.1 collapse). `edge_guard` is in octaves for the
  same reason. The CLI prints both. Raise `levels` alongside `radius`: the band
  is `[R·2⁻ˡᵉᵛᵉˡˢ, R]`, so every doubling of `R` costs an octave at the inner
  end, and the default 15 is about ten visible octaves plus the outward margin.
- **Anisotropic maps are allowed but flagged.** A non-uniform scale still gives
  an exactly invariant set (self-*affine* rather than self-similar), but the
  camera wrap can't reproduce it, so the zoom shows a seam. `Renorm::defect`
  measures this and the CLI/status bar say so rather than pretending.

**In the app, infinite zoom has one home: the Render window → infinite zoom.**
It opens by default once a scene has a zoom map, and it carries the whole
control surface:

- a **map picker** listing every transform, the qualifying ones selectable and
  the rest greyed with their reason on hover. `zoom_action()` already ran
  `Renorm::build` per transform and produced both the enabled flag and a
  sentence explaining the failure, so this turns "know the theory" into "read
  the list" out of code that already existed. Picking the zoom map is a
  **choose-one-of-n over transforms** and is now drawn as one, rather than as a
  lone selected button on whichever transform you happened to have selected —
  which had `ui::radio`'s defect spread across time instead of space: you
  couldn't see the alternatives, and nothing said only one could be lit.
- `edge guard`, with `radius`, `levels` and `octave falloff` under *band size*.

All of it is undoable and is what Ctrl+S writes into `[zoom]`. It sits next to
haze deliberately — haze is the other half of whether the band's edge is
visible. Changing any of it re-forms the point cloud, since every point's
octave is drawn from them. The status bar reads `zoom +N`.

**Zoom about this** stays on a transform's row and gizmo context menus, and in
the inspector's action row, as shortcuts for the map you already have selected.

From the CLI, on any scene, without editing it:

```
# make an existing scene scale-invariant and look at it
fracturize --scene scenes/sierpinski.toml --zoom 0

# tune the band; --zoom-levels / --zoom-radius / --zoom-guard / --zoom-falloff
# all need --zoom
fracturize --scene scenes/lsys_kelp.toml --zoom trunk --zoom-levels 16 \
  --render /tmp/t.png --effort low --width 640 --height 400

# is the wrap seamless? Step the camera down through two zoom periods and
# compare consecutive frames. `--distance` is folded back into the canonical
# period, so the frames either side of a wrap are ordinary renders — no
# animation, nothing to interpolate. tools/zoom_seam.py does the sweep and the
# arithmetic; the number to read is the wrap step as a multiple of an ordinary
# frame step. scenes/octave-edge-visual.toml ships with the guard off, to
# show the artifact: 11.9x as it stands, 0.0x once the guard is on.
python3 tools/zoom_seam.py scenes/octave-edge-visual.toml
python3 tools/zoom_seam.py scenes/octave-edge-visual.toml --guard 1

# ...and the other half of that question, which brightness cannot answer.
# tools/zoom_twitch.py reads per-pixel difference instead, walks the loop with
# --path-t (which names a frame of the *flight*, twist and all, where stepping
# --distance would not), finds the wrap by watching the folded distance step
# up, and reports the seam against two references: an ordinary frame of motion,
# and one camera rendered from two independent fills of the point buffer. That
# second one is the floor a resample costs, and a seam sitting on it is the
# dots being replaced rather than anything moving.
python3 tools/zoom_twitch.py scenes/astral_lattice.toml --splat

# any frame of the flight, as a still. Useful on its own for looking at where
# a loop actually goes, and it is what makes a run of stills a run of frames.
fracturize --scene scenes/astral_lattice.toml --path-t 0.35 --render /tmp/t.png

# ...and the version you can just watch. scenes/octave-edge-visual.toml opens
# zooming with the guard off; the wrap is a visible blink every 2.5s. Drag
# "edge guard" up in the Render window and it goes away. Rendering this scene
# as an *animation* will not show it — a path_zoom_loop covers exactly one
# period and wraps every frame, so the seam always measures as one frame step.
fracturize --scene scenes/octave-edge-visual.toml

# a map that can't be a scale symmetry says why and exits, rather than
# quietly rendering the ordinary bounded attractor:
#   infinite zoom: zoom map 1 uses variations (spherical 1.00);
#   the renormalizing map must be pure affine
```

A bad map is fatal at startup rather than silently disabling the feature: a
scene that quietly isn't infinite looks like a bug in the maths, and that is an
expensive thing to go looking for.

### Looping zoom animations

An animation can loop as an *endless* zoom, and exactly rather than
approximately, because the scene has a symmetry. `path_loop = "zoom"` closes
the path under that symmetry instead of by returning to the first key: one loop
descends `path_zoom_periods` zoom periods and ends on a frame **identical** to the one it
started, since scaling by `sᴺ` about the fixed point and turning by the map's
rotation leaves the rendered set unchanged. Played on a loop, a fourteen-second
file falls forever.

```toml
[camera]
path_loop = "zoom"
path_seconds = 14.0

[[camera.path]]      # one keypoint is enough — and is better than two
yaw = 0.0
pitch = 1.15
distance = 3.6
```

One key is the good case. The spline's out-of-range keys are that key carried
by the symmetry (`ZoomLoop::advance`), so log-distance and yaw come out as
arithmetic sequences and Catmull-Rom through equally spaced collinear points is
exactly linear — a constant-rate descent with no ease, no wobble and no
velocity kink at the seam. Author more keys and you get a flight that closes
one period lower; the last key is still synthesized, never written.

- `path_loop = "zoom"` needs a `[zoom]` map, and says so if there isn't one.
  It is resolved against the live renormalizing map, so dragging that map
  updates the loop rather than staling it.
- It is a *different* loop from `"closed"`, not a variant: closing back to the
  first key would undo the descent. The two were once separate keys that could
  both be set at once, which is a contradiction — now they're two values of
  one, picked from the Camera window's four-way radio.
- Like any looping path it doesn't ease (a stall at the seam is the one thing a
  zoom must not do) and the final duplicate frame is dropped.
- **In the app**: `zoom` is the fourth segment of the Camera window's Loop
  radio, greyed when the scene has no scale symmetry to close under (with the
  reason, and where to get one, on hover) rather than hidden — a mode that
  isn't drawn can't tell you it exists. Choosing it reveals a periods count
  beside it. The similarity is re-derived on every `App::refresh_zoom`, so dragging
  the renormalizing map updates the loop instead of leaving it closing under a
  map that has moved.
- Measured on `wellspiral`: the last-frame-to-first-frame step is **1.12× the
  median adjacent frame step** — the excess is the irreducible difference
  between two point samples of the same structure, not a seam.

### Zoom animations

A camera path key whose `distance` is many periods below the first authors a
descent of exactly that many periods — distance interpolates in log space, so
that's a constant-rate zoom, and the per-frame wrap folds it back into one
period however deep it goes. `scenes/wellspiral.toml` descends nine:

```toml
[[camera.path]]
distance = 3.6
[[camera.path]]
distance = 0.0363   # 3.6 · 0.6^9
```

```
fracturize --scene scenes/wellspiral.toml --render well.avif \
  --effort medium --width 960 --height 540 --fps 24 --splat --exposure 1.4
```

### Things that interact with it

- **Gizmos draw unrenormalized**, at the transforms' true positions, because
  that is where dragging them acts. Under zoom they won't sit on the artwork.
- **Traces are renormalized per-trace, not per-point** (`Renorm::renormalize_trace`):
  one level for the whole walk, so it stays a connected path instead of a
  scatter of jumps between octaves.
- **The drawn-point budget is bypassed** (`App::drawn_points`). It's measured on
  the plain attractor by CPU walkers, which doesn't describe what's on screen
  under zoom — and a scale-invariant set can't collapse to a speck, which is the
  only thing that budget exists to survive.
- **Point size** is world-space and the band spans many scales, so a scene will
  look grittier here than its `point_size` suggests; judge it inside the band.

## Render Jobs

`P`, and the Camera window's "Render job…", open the dialog
(`src/ui/render_job.rs`). There is no separate one-click "HQ render" — that
button and its keybind were folded into this dialog, which does the same job
with its parameters visible, an estimate, and a way to stop it. The model is
`src/render_job.rs`; the work runs on a second wgpu device, so the app stays at
full framerate while a job goes (measured: 119 FPS with a 240-frame animation
rendering).

- **Modes**: still (PNG), animation along the camera path in either `.avif`
  (AV1) or `.mp4` (H.264), or view descriptor — write a `.toml` view of this
  framing and render it later. The Output row has a button per file kind, so
  the codec choice is visible rather than buried: AVIF is the better file,
  MP4 is the one upload pipelines accept.
- **Quality is job-scoped.** Points, accumulate, splat, exposure and
  transparency are the job's own; the interactive `buffer_capacity` and prefs
  are untouched. That separation is the whole point — render at 100M and keep
  exploring at 6M.
- **Estimates.** Memory is exact and is checked against
  `max_storage_buffer_binding_size` *before* anything allocates, because the
  alternative failure mode is a device-lost panic several seconds in. Time is a
  range, from this session's own measured throughput (frame time minus
  `present_wait_ms` — raw frame time under vsync says the GPU is ~6x slower
  than it is) plus a measured per-pixel encode cost, then replaced by a figure
  extrapolated from real progress once the job is running.
- **Pause / cancel.** `JobControl` is checked in three loops in `offline.rs`:
  the `fill_points` accumulation frames, the per-tile loop, and the animation
  frame loop. Pause is a sleep loop — crude but correct, since the work is
  already chunked and the job's device is its own. Cancel takes two clicks
  (armed for 4s), returns `Err(CANCELLED)`, and leaves no partial file, since
  both writers only touch the output at the end.
- The time estimate runs on *working* time, not wall clock: pauses are
  subtracted, or the projected remaining time climbs while progress is frozen,
  which is a countdown that goes up.
- One job at a time. A queue was deferred in the first plan and stays deferred.

The CLI paths pass `control: None` and are unchanged.

## View Files & Offline Rendering

Press `V` in-app to dump the current view (yaw, pitch, distance, focus, offset,
point size, haze, color falloff/contrast — plus `renderer = "splat"` and
`exposure` when splat mode is active) to `views/<scene>-<timestamp>.toml`.
View color params override the scene's when present. Load one with `--view <path>`;
in windowed mode the camera starts stopped so the framing holds (press O to fly the
path).

### Reading a scene without rendering it

`--info` prints what a scene *is*, in eleven labelled sections, without opening
a GPU device. It answers two questions, and the layout is built around which
one you came with:

- **Orient.** A scene you have never seen — a `--random` roll, a `.mutN.toml`
  off the mutation sheet, someone else's file. What is this, is it sound, and
  what do I do to it next?
- **Verify.** You changed something — a hand edit, a `-S`, a `--view` — and you
  want to know it landed and what else it moved. This is a **diff** job:
  `diff <(fracturize -s a.toml --info) <(fracturize -s b.toml --info)`.

```
fracturize --scene scenes/koru.toml --info
fracturize --random --seed 42 --info      # inspect a roll before rendering it
fracturize -s s.toml -S meta.haze=0.9 --info   # what did that actually move?
fracturize -s s.toml --info --color            # ...with the gradient painted
```

The sections, in the order they print:

| | |
|---|---|
| `scene` | name, source, and one line of what it is |
| `notes` | **every diagnostic, or `none`** |
| `set` | what each `-S` displaced (only when one was given) |
| `view` | what a `--view` sets and what each value replaced |
| `shape` | where the attractor lands, and the two flags that would frame it |
| `maps` | four fixed lines per transform |
| `render` | point size, count, haze, decay, exposure, background |
| `colour` | mode, the three dials, luminance, the ramp |
| `camera` | the framing that would render, and the scene's own under it |
| `path` | every keypoint, not just the count |
| `zoom` | the band as rows, or every eligible map as a `--zoom` command |

**Read `notes` first.** It is one block, second, listing every diagnostic the
report found with the section each is about, and a count on its first line —
so "is this scene sound?" is one line to read rather than fifty. `notes none`
is the cheapest possible signal that nothing is wrong. It catches, among
others: a `point_size` over the crisp bound, a framing far from the one that
fills the frame, a map that expands or never fires, a flat gradient, a
`--view` framed against a different scene, an `edge_guard` ramp reaching into
the picture.

**Where the report has computed a number you will act on, it emits the action.**
`shape` prints `-S camera.distance=1.42`, not `~1.42`; the zoom eligibility
list prints `--zoom descent`. A suggestion you have to reassemble into a flag
is a suggestion half-given, and the reader who has the flags memorised is the
smallest slice of who reads this.

`shape` is the part a TOML cannot answer: `point_size` and `camera distance`
are the two things most often wrong in a hand-authored scene, and neither can
be checked by looking at the file. In `maps`, contraction is the **signed** cube
root of the determinant — negative means the map reflects, at or past 1.0 means
it expands — and rotations are re-derived from the matrix, so an authored
`(-26, 138, 0)` can print as `(154, 42, -180)`: same rotation, other euler
branch.

`--info` reports on a `--view` too, and on the camera flags: the `view` block
says what the file sets, what each value replaced, and — in its closing lines —
what a view never carries. The `camera` block below it is then the framing that
would actually **render**, resolved through the same `offline::effective_camera`
a `--render` frames with, with the scene's own kept under it so nothing is lost
by asking about a view.

#### The conventions, if you add to it

Four hold `src/info.rs` together, and anything added should follow them:

- **The report is a value, not a string.** `Section`s of `Row`s, rendered to
  text at the end. Nothing appends to a string mid-computation, which is what
  lets a diagnostic raised while measuring be printed second — and what makes
  the eventual `--info --json` an afternoon rather than a rewrite.
- **Fixed schema.** Every row prints every time, including ones the file left
  out — those read `unset` rather than vanishing. A row that disappears when
  empty is a row nobody can learn to look for, and two reports diff row for row
  only if both have the same rows. Three golden files in `tests/golden/` hold
  this; re-bless them with `FRACTURIZE_BLESS=1 cargo test golden` and **read
  the diff** — that is the review.
- **One writer per quantity.** `point()`, `angle()`, `length()`, `amount()`,
  `size()`, `count()`, `word()` — a position is always `(x, y, z)` to 3dp in 24
  columns, an angle always carries radians *and* degrees, and a word standing
  in for a number is right aligned with the numbers. Each is fixed width with
  the sign in its own column, which is what makes the decimal points line up
  without any column having to know what is in it. Reach for the helper rather
  than a fresh `format!`.
- **78 columns, hard**, and there is a test. A row that wraps is *worse* than
  no table, because wrapping destroys exactly the alignment the table was
  built to provide. Prose goes through `wrap()`; a footnote goes at the end of
  its section, never welded to the header.

### Framing from the command line

`--yaw --pitch --distance --roll --focus x,y,z` override the camera, winning
over both the scene and any `--view`. They exist so that trying a framing
doesn't require authoring a view file for it. When any of them is given,
`--render` prints the `[camera]` block it settled on, ready to paste into a
scene:

```
fracturize --scene scenes/octahedron.toml --distance 4 --pitch 0.4   --render /tmp/o.png --effort low --width 400 --height 300
```

Under infinite zoom the printed distance may not be the one you asked for: the
framing is only defined up to a zoom period, so it is wrapped into the canonical
one first. That's the same framing, said in the band's terms.

View files are also hand-writable now: everything but `yaw`/`distance`/`focus`
defaults, and `yaw` is accepted as an alias for the field the format calls
`rotation`. Four lines is a valid view.

`--render <out.png>` renders **headlessly** — no window, no event loop, no focus
stealing — and exits, printing a timing breakdown (`setup | chaos fill | render |
encode+save | total`) to stdout so you can budget effort. Options:

- `--effort draft|low|medium|high|ultra` — named presets for point count +
  accumulation: draft 1M/4, low 4M/16, medium 12M/48, high 40M/128, ultra
  100M/256. Explicit `--points N` / `--accumulate N` override the preset.
  Point count is a *render* property, not a scene property: the scene's
  `point_count` is just the interactive default (16 bytes/point of GPU memory).
- `--width/--height` (default 1920x1080) — per tile when a grid mode is used.
- `--transparent` — RGBA output for compositing (PNG only; see "Background
  & Transparent Output").
- `--view <path>` for exact framing (views store yaw + pitch), `--fog`
  (legacy on-switch; a scene's own `haze` value wins over it).
- `--zoom <transform>` turns on infinite zoom about a named or indexed map,
  overriding the scene's `[zoom]`; `--zoom-levels/-radius/-falloff` tune the
  band. See "Infinite Zoom".
- No `--scene`? `--random` rolls a flame, `--blank` opens the empty canvas
  (see "Random Flames" and "Starting From Nothing"); otherwise you get the
  built-in default. All three work windowed and with `--render`.
- `--splat [--exposure X]` — render with the splat renderer (also implied by a
  view saved in splat mode; explicit flags win). Works with all grid modes.
  Exposure is capacity-normalized, so the same value looks the same at every
  effort level; raise it (1.5-3) to brighten thin filaments, lower it to
  recover detail in hot cores.

**Grid contact sheets** (for exploring 3D framing cheaply — the point cloud is
filled once and re-rendered per tile, so 9 tiles cost barely more than 1):

- `--orbit-grid 4x2` — 8 views evenly spaced around a full horizontal orbit,
  starting at the base yaw. Row-major; per-tile yaw printed to stdout in
  degrees and radians (radians paste directly into `[camera] yaw`).
- `--move-grid 3x3 [--move-step 0.25]` — camera nudged left/center/right
  (columns) × up/center/down (rows) in the view plane by `move-step` × orbit
  distance, all still looking at the focus. Center tile = base view. Each
  tile prints its equivalent `yaw`/`pitch`/`distance`, so a good framing can
  be adopted directly into a `[camera]` block or view file.

**Parameter sweeps** (`--sweep`, exploring the *scene* rather than the framing):

- `--set <path>=<value>` (repeatable, `-S`) overrides any scene value without
  editing the file — the general form of `--palette`/`--zoom`. Dotted, section
  required: `meta.haze=0.3`, `zoom.edge_guard=1.5`,
  `transform.<name-or-index>.weight=0.5`,
  `transform.facet-1.variations.absfold=0.15`, `transform.0.translation.y=1.25`
  (arrays index by x/y/z or 0/1/2, or take a whole `[a,b,c]`). Applied to the
  TOML text before parsing (`src/set.rs`), so `--info`, grids and animation all
  see an ordinary scene. **An unresolvable path is an error, never a silent
  no-op** — it names the transforms the scene actually has.
- `--sweep <path>=<a:b>` walks `--sweep-steps` values (default 5) between the
  ends; `--sweep <path>=a,b,c` takes a list verbatim (checked first, so a value
  containing a comma is never read as a range). Join paths with `+` to move
  them in lockstep — `t.a.variations.absfold+t.b.variations.absfold=0.05:0.55`
  — which is what you want when several maps must stay equal. Give `--sweep`
  twice for a 2D grid: the first varies across columns, the second down rows.
  Composes with `--set`, which sets the base every tile starts from.
- Sweeps need `--scene`, and **each tile refills the point buffer** (the swept
  parameter changes the IFS), so prefer `--effort draft|low`. Unlike mutation
  sheets no variant files are written: a tile is fully described by one flag,
  and that flag is printed per tile, ready to paste.

**Tile labels.** Every sheet — orbit, move, mutation, sweep — draws its
per-tile parameters into the tile in amber (`src/glyphs.rs`, a 5x7 bitmap font;
the same colour the app uses for world-anchored transform names). Sheets print
to stdout too, but stdout isn't in the PNG, and an agent reads the PNG. A label
too long for its tile is truncated with a trailing `>` rather than running into
its neighbour. `--no-labels` turns them off.

**Animation** (`--render <out.avif|out.mp4>`): the camera flying
the scene's path — its `[[camera.path]]` spline, or the default full-turn orbit
of the base framing when it authors fewer than two keypoints. Same rule the app
previews with (`path::resolve`), so what you watch in the window is what this
writes. The point cloud is fixed, so
this is one chaos fill plus a cheap render pass per frame; frames stream
straight into an encoder chosen by the output extension, then a hand-rolled
ISOBMFF muxer (src/video.rs), no external tools:

- `.avif` — AV1 via rav1e (src/avif.rs). Loops like a GIF at a fraction of the
  size, plays in a browser. The better file.
- `.mp4` — H.264 via openh264 (src/h264.rs), pinned quantizer, `moov` first so
  it plays while it downloads. `--quality` maps onto QP and moves the bitrate
  hard: 3s of `winze` at 960x540/24fps came out 2.5 MB at q25, 5.1 at q40,
  10.3 at q60 and 16.7 at q80, so dial it down for anything with an upload
  size cap. (openh264 must be driven in a real rate-control mode for this to
  work at all — `RateControlMode::Off` ignores the quantizer and emits a
  byte-identical file at every setting. `Bufferbased` specifically: it,
  `Quality` and `Timestamp` give identical output, but the other two warn at
  init on every render and the warning can't be silenced through the config.
  `quality_changes_the_bitrate` guards the knob, because every structural test
  passes while it does nothing.) Bigger, and the one that survives an upload: the
  platforms that loop short clips want H.264, and several reject AV1. Muxing
  our AV1 into `.mp4` would have been nearly free and would have produced a
  file that looks right locally and bounces on upload — hence a real second
  encoder rather than a second extension.

Both go through the same RGBA -> BT.709 limited-range 4:2:0 conversion, so the
two formats are colour-identical by construction. Cannot combine with
grid/mutation sheets. Options:

- `--fps N` (default 30), `--seconds S` (default: the path's `path_seconds`,
  else 3s per spline segment — but the *default* orbit carries its own
  `path_seconds` of ~35s, one turn at 0.18 rad/s, so a pathless scene animates
  a full slow turn unless you pass `--seconds`), `--quality 0-100` (default 60
  — the AV1 quantizer for `.avif`, H.264's QP for `.mp4`).
- Closed paths omit the final frame so the loop wraps without a stutter.
- Odd `--width`/`--height` are rounded down (4:2:0 chroma needs even sizes).
- Budget: encoding dominates and scales with pixels — measured at ~6e-7
  s/pixel on the desktop (0.34s/frame at 960x540, 0.53s at 720p, 1.08s at
  1080p), so a 14s 24fps 720p loop is ~3 minutes. Most of that lands in
  rav1e's final flush rather than the per-frame push. **`.mp4` is ~50x
  cheaper** — measured at ~6e-8 s/pixel on the same clip (36 frames of
  `winze` took 17.2s as AVIF and 0.35s as MP4), because openh264 at constant
  QP encodes on the way in with nothing to flush. The Render job dialog
  estimates from whichever figure matches the chosen format; preview cheap
  first with
  `--width 480 --height 270 --fps 12 --effort low`.
  Verify output with `ffprobe` / extract frames with `ffmpeg`.

**Mutation sheets** (evolutionary exploration): `--mutations N` renders the
scene plus N random variants (tile 0 = original, near-square grid). Each
variant is saved as `<out>.mutN.toml` and its operator list printed per tile
("T2 rotate 21° about (...)", "T1 +swirl 0.18", ...), so you can look at the
sheet, pick a tile, and load/iterate on its TOML. `--seed` reproduces a
sheet (the used seed is always printed), `--mutation-strength` scales the
perturbations (default 1.0). Unlike camera grids, each tile refills the point
buffer, so prefer `--effort draft|low`. The same operators run in-app on `U`.

```
# quick look at a new scene from all sides (fast: use draft + small tiles)
fracturize --scene scenes/koru.toml --render /tmp/koru_orbit.png \
  --effort draft --orbit-grid 4x2 --width 480 --height 270

# test a slight camera move around a saved framing
fracturize --scene scenes/koru.toml --view views/koru-123.toml \
  --render /tmp/koru_move.png --effort low --move-grid 3x3 --width 320 --height 180

# final frame
fracturize --scene scenes/glasshouse.toml --view views/glasshouse-123.toml \
  --render renders/glasshouse_4k.png --width 3840 --height 2160 --effort high

# evolve: original + 8 mutations, reproducible
fracturize --scene scenes/koru.toml --render /tmp/koru_evo.png \
  --effort draft --mutations 8 --seed 42 --width 320 --height 180

# roll random flames offline; --seed makes any roll reproducible, and the
# seed used is always printed so a good one can be recovered
fracturize --random --render /tmp/roll.png --effort low --width 480 --height 270
fracturize --random --seed 7 --render /tmp/roll7.png --effort high

# animated loop of a camera path (scenes/winze.toml authors one; see its
# header comments) — preview small/cheap, then commit to the real encode
fracturize --scene scenes/winze.toml --render renders/winze.avif \
  --effort medium --width 960 --height 540 --fps 24 --splat --exposure 1.5
```

A 4K render with 12M points takes ~30s on the T490; the desktop (8GB VRAM) does
medium-effort grids in well under a second — trust the printed timing line, not
estimates. `--points` also works in windowed mode. View files are the reliable
way for agents to render specific framings without keyboard interaction.

## Screenshot Support

- Press `S` in-app, or run `--screenshot --delay N` to capture and exit
  (N is in FRAMES, not seconds; the count runs at the display refresh rate)
- Files go to `screenshots/<scene-slug>-<unix-time>.png` — existing files are
  never overwritten (a `-N` suffix disambiguates same-second captures)
- `./screenshot.sh [scene]` wraps build + capture and echoes the newest file
- Screenshots render offscreen at 1280x720 and contain only the point cloud
  (no gizmos/HUD). To verify overlays, run windowed and capture with
  ImageMagick: `import -window <id>`; set `FRACTURIZE_SHOW_HELP=1` to start with
  the help panel open.
- `--no-vsync` uncaps the frame rate for benchmarking; FPS is logged once per
  second with `RUST_LOG=info` and shown in the window title/HUD.
- Running the app without `--screenshot` blocks until closed - use timeouts.

## Coding Conventions

- Keep it simple - avoid over-abstraction
- Use glam for all math, not manual array ops
- WGSL structs must match the `#[repr(C)]` Rust structs in `buffers.rs` exactly
  (`GpuTransform` is 160 bytes; `CameraUniforms` is 112 and is declared in
  points/render.wgsl, points/splat.wgsl, gizmo.wgsl, AND density/voxel_render.wgsl
  — update all four; size tests in buffers.rs/compute.rs guard the Rust side)
- New variations: append to `VARIATION_NAMES` in scene.rs AND the matching slot
  in `apply_variations()` in chaos.wgsl; slots must stay in sync
- New CLI flags: put the field in its section in `Args` (the `// ---- Camera`
  banners) and give it that section's `help_heading`, so `-h` stays a grouped
  index rather than one 100-line list. Write the doc comment as **one short
  line, a blank line, then the detail** — clap shows the first line in `-h` and
  the whole thing in `--help`, so the reasoning that belongs in this repo's
  comments can stay without making the summary unreadable. Worked examples go
  in `EXAMPLES` (`--help` only), not in the per-flag help. Three conventions
  make the one-line summaries carry as much as the old paragraphs did, and all
  three are enforced by tests at the bottom of main.rs:
  - **The value name is the type.** `<NAME|INDEX>`, `<X,Y,Z>`, `<0-1>`,
    `<FILE>`, `<OCTAVES>` — not clap's derived `<ACCUMULATE>`. Small enum
    domains go in the summary text with `hide_possible_values`, because clap's
    `[possible values: …]` tail is what pushes a section into its two-line
    layout.
  - **A trailing `[bracket]` says what you get without the flag.** `[the
    scene's, else 15]`, `[--view, else 1.0]`, `[time-based]`, `[else a
    window]`. Options with a real clap `default_value` are exempt — clap prints
    `[default: x]` itself. The point is that *no* option leaves "what happens
    if I omit this?" unanswered; `LEGEND` at the foot of the help explains the
    notation.
  - **Summaries stay under 78 characters**, or the column wraps.
- Short flags are rationed to options typed constantly, with an obvious letter:
  `-s/--scene -S/--set -v/--view -r/--render -p/--points -i/--info`. Adding one
  costs the next flag its obvious letter, so the list is pinned by a test —
  change it deliberately or not at all.
