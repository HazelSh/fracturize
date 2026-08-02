# Fracturize - AI Agent Guidelines

A 3D fractal flame renderer inspired by Apophysis, built with Rust and wgpu.

## Project Overview

Fracturize renders IFS (Iterated Function System) fractals in 3D using the chaos game
algorithm, entirely on the GPU. A compute shader runs thousands of parallel "walkers"
that iterate through weighted random transforms (affine matrix + nonlinear variation
blend), writing positions into a circular point buffer that is rendered every frame.

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
    toolbar.rs     # Top icon strip: panel toggles + scene name
    status_bar.rs  # Bottom bar: context hints, FPS/p99 sparkline, point stats
    hints.rs       # hinted(): tooltip + status-bar hint on one widget response
    transforms.rs  # Transform tab rail + selected-transform detail pane
    explore.rs     # Random flame, mutate + strength, undo/redo, history list
    render_panel.rs# Renderer mode, exposure, point size + count, color, haze, output
    save_as.rs     # "Save scene as…" modal (fork the scene under a new name)
  render_job.rs  # Batch render dialog: setup, estimates, progress, pause/cancel
    camera_panel.rs# Framing, saved views, the camera path, render/screenshot/save
    browser.rs     # Scene picker (B)
    shortcuts.rs   # Keybind reference window (H)
    labels.rs      # World-anchored transform name labels
    icons.rs       # Phosphor codepoints (vendored font, see assets/fonts/)
  camera.rs      # OrbitCamera (yaw/pitch/distance/focus), ray + projection helpers
  path.rs        # CameraPath: Catmull-Rom splines over orbit keypoints
  avif.rs        # Animated AVIF writer: rav1e AV1 encode + minimal ISOBMFF muxer
  pick.rs        # Gizmo hit-testing and drag geometry (pure math, unit-tested)
  mutate.rs      # Random scene mutation operators (U key, --mutations)
  trace.rs       # CPU chaos walkers (variation port) for the trace overlay
  prefs.rs       # Persistent user prefs (~/.config/fracturize/prefs.toml)
  scene.rs       # TOML scene parsing AND saving, TransformSpec, variation names/slots
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
| scroll | zoom |
| drag a gizmo's origin dot | select + translate the transform in the view plane |
| drag an origin→axis gizmo edge | translate along that axis |
| drag an outer gizmo edge | rotate around the third local axis (edge x-y rotates around z) |
| ctrl+drag any gizmo part | uniform scale (drag up = grow) |
| scroll over a gizmo | adjust that transform's chaos weight (probability) — the lever that emphasizes an element without changing structure or color |
| click a Transforms row | select that transform (two-way with gizmo selection) |
| right-click a Transforms row | duplicate / enable-disable / delete / rename |
| drag any panel DragValue | change the value; click it to type an exact one |

Grabbable gizmo parts glow and grow on hover (edges widen and whiten, the
origin dot enlarges) and the cursor switches to a grab hand. Gizmo drags
re-run the chaos game live; the fractal re-forms as you drag (sparse while
moving, densifying when you pause — warmup refills in ~1s). Picking math
lives in `pick.rs`, drag application in `app.rs`.

The camera eye always sits on the orbit sphere: the legacy scene/view
`offset` (which made pitch drift the view distance) is folded into
yaw/pitch/distance at load time and no longer written to files.

Roll is the fourth camera parameter and travels with the other three
everywhere — scene `[camera]`, view files, path keypoints, the offline
renderer. `OrbitCamera` deliberately has no `Default`, so a struct literal
that forgets `roll` fails to compile rather than silently levelling someone's
framing on the next save.

## Keybinds (also in-app: press H)

| Key | Action |
|-----|--------|
| H / ? | toggle the Keybinds window |
| Esc | quit |
| Space | re-seed points (reset) |
| Up / Down | zoom in / out (selects transform when a transform is selected) |
| Enter | enable/disable selected transform |
| G | toggle transform gizmos and their name labels |
| O or Z | play / stop the camera flying its path (two keys, one action) |
| Y / Shift+Y | add current framing as a keypoint of this scene's own path / remove the last one |
| Ctrl+Y | toggle camera path closed (seamless loop) |
| V | save current view to views/<scene>-<timestamp>.toml |
| S | save screenshot to screenshots/<scene>-<timestamp>.png (never overwrites) |
| Ctrl+S | **save the scene** (with all edits) back to its TOML file |
| U / Shift+U | random scene mutation / undo it |
| Ctrl+Z / Ctrl+Shift+Z | undo / redo *any* edit (see `src/history.rs`) |
| X / Shift+X | chaos-game traces: show (re-rolls each press) / hide |
| I | invert mouse pitch, flightsim style (persisted to prefs) |
| B | Scenes window: arrows + Enter, or click a row, to load a scene in place |
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

The Keybinds window (H) is clickable: each row triggers its first-listed
binding, shift+click the second. `I` persists to
`~/.config/fracturize/prefs.toml` (user preferences, not scene data).

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

Layout: a thin top toolbar of icon toggles (with the scene name at its right
end), floating `egui::Window` panels over a full-surface viewport, and an
Inkscape-style bottom status bar. Nothing shrinks the drawable area, so
aspect and picking math are unaffected by which panels are open.

| Window | What lives there |
|--------|------------------|
| Transforms | Vertical tab rail (colour swatch, name, eye toggle, relative-weight bar) plus a detail pane for the selected transform: position/rotation/scale, weight, colour, variations |
| Explore | New random flame, new blank scene, mutate + strength, undo/redo, and the history list (click a row to jump N steps in one rebuild) |
| Render | Renderer mode, exposure, point size, point count, color falloff/contrast, haze |
| Camera | Framing, saved views, the camera path (keypoints + playback), render job / screenshot / save scene |
| Scenes (B) | Scene picker; the same selection the arrow keys walk |
| Keybinds (H) | The table above, scrollable, rows clickable |

Conventions worth keeping:

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
  undo step. Camera moves are deliberately *not* history entries.
- When testing anything that touches prefs, set an isolated `XDG_CONFIG_HOME`
  rather than writing the developer's real `~/.config/fracturize/prefs.toml`.

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
**Not supported for `.avif`**: `src/avif.rs` reads only r/g/b converting to
YUV, so `--transparent` with an `.avif` output errors rather than quietly
producing opaque video.

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
# legacy: offset = [x,y,z] (eye displacement) still loads, but is folded
# into yaw/pitch/distance and never written back
path_closed = true        # optional camera path: loop back to key 1 (seamless)
path_seconds = 14.0       # playback/render duration (default 3s per segment)
path_ease = false         # smoothstep time; default: open paths ease, loops don't

# Camera path spline keypoints (2+ = a path; see src/path.rs). A uniform
# Catmull-Rom spline runs through the keys in orbit-parameter space: yaw is
# unbounded (keys spanning 2*TAU author a two-turn corkscrew — nothing wraps),
# distance interpolates in log space (constant-relative-rate zooms), and focus
# travels on its own spline so look directions blend smoothly while the eye
# moves. Omitted fields inherit the base [camera] framing. Closed paths take
# the shortest yaw route back to key 1. In-app: Y appends the current framing
# as a keypoint, Shift+Y removes, Ctrl+Y toggles the loop, O or Z flies it.
#
# EVERY SCENE HAS A PATH. Omit these keypoints (or author fewer than two) and
# the path is a seamless full orbit around the current framing, at 0.18 rad/s
# — the "turntable". That default is not a second system: it is a real
# `CameraPath`, it draws, it plays, and `--render x.avif` flies it, all
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
radius = 1.5              # outer radius of the band, in camera distances
levels = 12               # octaves rendered below it
octave_falloff = 2.0      # point-budget falloff per octave (power of the scale)

[[transform]]
name = "whorl"                 # optional label shown in overlays
translation = [0.0, 0.0, 0.5]
scale = 0.5                    # uniform, or per-axis: scale = [0.05, 0.6, 0.05]
rotation = [0, 0, 0]           # Euler degrees (XYZ)
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
  **the wrap is the level-of-detail system.** The status bar's zoom counter is
  the only thing that moves.

The honest limit: the zoom is infinite *toward `p`*. Fly off sideways and you
leave the band and get the ordinary bounded attractor. Every self-similar zoom
has a centre; this one's is the fixed point of the map you picked.

### Using it

```toml
[zoom]
map = "descent"        # transform name, or its index as a string ("0")
radius = 1.5           # outer radius of the band, in camera distances
levels = 12            # octaves rendered below `radius`
octave_falloff = 2.0   # point-budget falloff per octave, as a power of `s`
```

- **Give the nominated map zero translation.** A map with no translation has its
  fixed point at the origin, which is where a zoom centre wants to be. Otherwise
  `p = (I − A)⁻¹b` lands somewhere arbitrary and the camera has to be aimed at
  it by hand — the CLI prints where.
- `radius` has to clear the frustum corner (~0.85 × distance at 16:9) or the
  outermost octave's edge cuts a hard line across the frame. 1.5 is safe; below
  ~1.0 the cut shows.
- `levels` is how far in the band extends. `octave_falloff` keeps that cheap:
  octave *k* is a copy of the attractor at scale `sᵏ`, covering `sᵏ` of the
  frame, so it gets `s^(2k)` of the points and stays at the same on-screen
  density. Flat (`0`) spends most of the buffer on specks. Twelve octaves at
  falloff 2 cost about the same as three at falloff 0.
- **Anisotropic maps are allowed but flagged.** A non-uniform scale still gives
  an exactly invariant set (self-*affine* rather than self-similar), but the
  camera wrap can't reproduce it, so the zoom shows a seam. `Renorm::defect`
  measures this and the CLI/status bar say so rather than pretending.

In the app: right-click a transform (its row or its gizmo) → **Zoom about this**,
which toggles, tells you the period and fixed point on hover, and greys out with
the reason when that map can't be a scale symmetry. The status bar reads
`zoom +N`.

From the CLI, on any scene, without editing it:

```
# make an existing scene scale-invariant and look at it
fracturize --scene scenes/sierpinski.toml --zoom 0

# tune the band; --zoom-levels / --zoom-radius / --zoom-falloff need --zoom
fracturize --scene scenes/lsys_kelp.toml --zoom trunk --zoom-levels 16 \
  --render /tmp/t.png --effort low --width 640 --height 400

# a map that can't be a scale symmetry says why and exits, rather than
# quietly rendering the ordinary bounded attractor:
#   infinite zoom: zoom map 1 uses variations (spherical 1.00);
#   the renormalizing map must be pure affine
```

A bad map is fatal at startup rather than silently disabling the feature: a
scene that quietly isn't infinite looks like a bug in the maths, and that is an
expensive thing to go looking for.

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

- **Modes**: still (PNG), animation (AVIF along the camera path), or view
  descriptor — write a `.toml` view of this framing and render it later.
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

`--info` prints what a scene *is*: every transform with its share of the chaos
walk (weights are unnormalized in the file, so this is the number you actually
wanted), its contraction and variation blend; what the attractor **measures**
— centre, 95th-percentile radius, per-axis spread, occupancy, from the same CPU
walkers `randomize.rs` gates on — with the camera distance and maximum
`point_size` that measurement implies; the render and colour properties; the
camera and path; and which maps are eligible to carry infinite zoom, with the
reason for each that isn't.

```
fracturize --scene scenes/koru.toml --info
fracturize --random --seed 42 --info      # inspect a roll before rendering it
```

The measurement block is the part worth reading first: `point_size` and
`camera distance` are the two things most often wrong in a hand-authored scene,
and neither can be checked by looking at the file. Rotations are re-derived
from the matrix, so an authored `(-26, 138, 0)` can print as `(154, 42, -180)`
— same rotation, other euler branch.

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

**Animation** (`--render <out.avif>`): an animated AVIF of the camera flying
the scene's path — its `[[camera.path]]` spline, or the default full-turn orbit
of the base framing when it authors fewer than two keypoints. Same rule the app
previews with (`path::resolve`), so what you watch in the window is what this
writes. The point cloud is fixed, so
this is one chaos fill plus a cheap render pass per frame; frames stream
straight into rav1e (AV1) and a hand-rolled ISOBMFF muxer (src/avif.rs), no
external tools. Cannot combine with grid/mutation sheets. Options:

- `--fps N` (default 30), `--seconds S` (default: the path's `path_seconds`,
  else 3s per spline segment — but the *default* orbit carries its own
  `path_seconds` of ~35s, one turn at 0.18 rad/s, so a pathless scene animates
  a full slow turn unless you pass `--seconds`), `--quality 0-100` (default 60).
- Closed paths omit the final frame so the loop wraps without a stutter.
- Odd `--width`/`--height` are rounded down (4:2:0 chroma needs even sizes).
- Budget: encoding dominates and scales with pixels — measured at ~6e-7
  s/pixel on the desktop (0.34s/frame at 960x540, 0.53s at 720p, 1.08s at
  1080p), so a 14s 24fps 720p loop is ~3 minutes. Most of that lands in
  rav1e's final flush rather than the per-frame push. The Render job dialog
  estimates from this same figure; preview cheap first with
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
