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
  main.rs        # CLI args, winit event loop, keybinds, default scene
  app.rs         # App state, mouse/edit handling, render orchestration, HUD, screenshots
  camera.rs      # OrbitCamera (yaw/pitch/distance/focus), ray + projection helpers
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
    gizmo.rs     # Transform gizmos (unit tetrahedra per transform)
    lines.rs     # Trace line-segment renderer
    text.rs      # glyphon text overlay (HUD, transform list, help panel)
    density/     # Inactive experimental hash-grid density renderer
shaders/
  points/chaos.wgsl   # Chaos game + the 16 variation functions
  points/render.wgsl  # vs_main (quads), vs_point (1px points), shared fs
  gizmo.wgsl
scenes/          # TOML scene files
```

## Tech Stack

- **Rust 2024 edition** - `gen` is a reserved keyword, use `r#gen()` for rand
- **wgpu 28** - Vulkan-backed; requires SHADER_F16 (fine on Intel UHD 620 + Mesa)
- **winit 0.30** - ApplicationHandler pattern
- **glyphon** (git, pinned rev) - text overlay. IMPORTANT: keep the `rev =` pin in
  Cargo.toml; later glyphon revisions require wgpu 29 and break the build. Cargo.lock
  is gitignored, so an unpinned git dep resolves differently on fresh checkouts.
- **glam / bytemuck / toml + serde / clap / image** - math, GPU casts, scenes, CLI, PNG

## Rendering Approach

All GPU, three passes per frame:
1. **Chaos compute** (`points/chaos.wgsl`): 16384 walkers each iterate the IFS a few
   times per frame, writing into a circular buffer (full refresh every ~800 frames;
   10x faster during warmup). Per iteration: pick transform by cumulative weight,
   apply affine matrix, then blend the 16 variation functions by weight. Diverged or
   NaN walkers are re-seeded randomly (important for nonlinear variations).
2. **Point render** (`points/render.wgsl`): adaptive pipeline selection. When the
   projected point size at orbit distance is subpixel (the common case), points are
   drawn as native 1px point primitives (~3x faster). Otherwise, 4-vertex instanced
   triangle-strip billboards with perspective sizing.
3. **Gizmos, traces + text**: optional overlays (see keybinds). Traces (X)
   are CPU walkers (trace.rs ports the 16 variations from chaos.wgsl — keep
   them in sync!) rendered as alpha-faded line segments; they regenerate on
   every scene edit.

The chaos churn rate is wall-clock normalized: `advance_frame` takes the
frame dt and scales walker iterations so the buffer refreshes at the same
real-time rate at any refresh rate (60 FPS baseline: full cycle ~13 s).
The auto-orbit is likewise time-based (0.18 rad/s).

Performance on the reference machine (ThinkPad T490, Intel UHD 620, 1280x720):
- ~10M points at ~38 FPS uncapped (subpixel/point-primitive path)
- ~5M points comfortably at 60 FPS; billboard path is ~3x slower per point
- Storage-binding limits are raised to adapter max at startup (default 128MiB cap
  would limit the buffer to ~8M points; buffers are 16 bytes/point)

## Mouse Controls

| Input | Action |
|-------|--------|
| left-drag (empty space) | orbit camera, grab-the-scene: drag right spins it right, drag up tilts its top toward you (pauses auto-orbit) |
| shift+drag / middle-drag | pan the focus in the view plane |
| scroll | zoom |
| drag a gizmo's origin dot | select + translate the transform in the view plane |
| drag an origin→axis gizmo edge | translate along that axis |
| drag an outer gizmo edge | rotate around the third local axis (edge x-y rotates around z) |
| ctrl+drag any gizmo part | uniform scale (drag up = grow) |
| scroll over a gizmo | adjust that transform's chaos weight (probability) — the lever that emphasizes an element without changing structure or color |

Grabbable gizmo parts glow and grow on hover (edges widen and whiten, the
origin dot enlarges) and the cursor switches to a grab hand. Gizmo drags
re-run the chaos game live; the fractal re-forms as you drag (sparse while
moving, densifying when you pause — warmup refills in ~1s). Picking math
lives in `pick.rs`, drag application in `app.rs`.

The camera eye always sits on the orbit sphere: the legacy scene/view
`offset` (which made pitch drift the view distance) is folded into
yaw/pitch/distance at load time and no longer written to files.

## Keybinds (also in-app: press H)

| Key | Action |
|-----|--------|
| H / ? | toggle keybind help panel |
| Esc | quit |
| Space | re-seed points (reset) |
| Up / Down | zoom in / out (selects transform when overlay is on) |
| Enter | enable/disable selected transform |
| T | toggle info overlay (HUD + transform list) |
| G | toggle transform gizmos |
| O | pause / resume camera orbit |
| V | save current view to views/<scene>-<timestamp>.toml |
| S | save screenshot to screenshots/<scene>-<timestamp>.png (never overwrites) |
| Ctrl+S | **save the scene** (with all edits) back to its TOML file |
| U / Shift+U | random scene mutation / undo it (32-deep undo stack) |
| X / Shift+X | chaos-game traces: show (re-rolls each press) / hide |
| I | invert mouse pitch, flightsim style (persisted to prefs) |
| B | scene browser overlay: arrows + Enter or click to load a scene in place |
| P | background high-quality render of the current framing to renders/ (own GPU device; the realtime view keeps running; pauses orbit) |
| A / Shift+A | duplicate selected transform / add a fresh one (rebuilds pipelines) |
| Delete | delete selected transform |
| , / . | selected transform's chaos weight down / up |
| J / K / L | selected transform's color: hue / saturation / value up (+Shift = down) |
| E / Shift+E | cycle the variation slot targeted by - / = (shown in HUD) |
| - / = | targeted variation weight down / up (0.05 steps) on selected transform |
| [ / ] | shrink / grow point size |
| D / Shift+D | finer / coarser color detail (color_falloff) |
| C / Shift+C | less / more color contrast |
| F / Shift+F | more / less fog |
| N / Shift+N | fog start closer / farther |
| M / Shift+M | fog end closer / farther |

The help panel (H) is clickable: each row triggers its first-listed binding,
shift+click the second. The window is freely resizable. `I` persists to
`~/.config/fracturize/prefs.toml` (user preferences, not scene data).

Ctrl+S also bakes the current camera framing, point size, and color
falloff/contrast into the scene's defaults. **Saving preserves comments**:
existing files are edited in place via toml_edit — only changed values are
rewritten, so header/per-transform comments, inline `# notes`, and formatting
like `6_000_000` survive. Two exceptions: a legacy camera `offset` key is
removed (folded into yaw/pitch/distance), and if transforms were added or
removed the whole [[transform]] array is rebuilt (header/meta/camera comments
still survive). Scenes with no path (built-in default) save to
`scenes/untitled-<timestamp>.toml`.

## Scene Files (TOML)

Use `--scene <path>` to load. See `scenes/` for examples.

```toml
[meta]
name = "Scene Name"
author = "Your Name"
point_size = 0.002        # world-space point size
point_count = 4_000_000   # circular point buffer capacity (default 500k)
color_speed = 0.5         # global color blending speed (0-1); used when color_falloff = 0
color_falloff = 0.0       # scale-aware color accumulation exponent (0 = off, ~1 neutral)
color_contrast = 1.0      # render-time cyclic palette contrast stretch (1 = off)

[camera]                  # optional
focus = [0.0, 0.0, 0.0]   # orbit center / look-at
distance = 3.0            # orbit radius (true eye-focus distance)
yaw = 0.0                 # orbit angle around Y, radians
pitch = 0.32              # orbit elevation, radians (positive = above)
# legacy: offset = [x,y,z] (eye displacement) still loads, but is folded
# into yaw/pitch/distance and never written back

[[transform]]
name = "whorl"                 # optional label shown in overlays
translation = [0.0, 0.0, 0.5]
scale = 0.5                    # uniform scale
rotation = [0, 0, 0]           # Euler degrees (XYZ)
color = [1.0, 0.2, 0.2]        # contributes to the cyclic colormap
weight = 1.0                   # selection probability
color_value = 0.25             # optional explicit colormap index (0-1)
color_speed = 0.5              # optional per-transform override (wins over color_falloff)
# Nonlinear variation blend; omit for classic affine ({ linear = 1.0 })
variations = { swirl = 0.35, linear = 0.65 }
```

Available variations (slot order in `scene.rs` / `chaos.wgsl`):
`linear, sinusoidal, spherical, swirl, horseshoe, polar, disc, spiral,
hyperbolic, diamond, julia, bent, fisheye, bubble, cylinder, tangent`

Scene-design notes learned the hard way:
- Every point renders at full brightness (no log-density), so unbounded variations
  (`spherical` especially) spray faint fuzz everywhere. Prefer bounded ones
  (`bubble`, `fisheye`, `sinusoidal`, `swirl`, `julia`) or keep spherical weights low.
- `sinusoidal` with affine scale >1.4 saturates onto ±1 walls (box/room looks);
  scale ~1.1-1.2 with small rotations gives classic gnarl swirls.
- Colors wash out to pastel when transforms mix heavily; raise `color_speed`
  (0.5-0.7) for stronger per-branch color identity.
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

## View Files & Offline Rendering

Press `V` in-app to dump the current view (yaw, pitch, distance, focus, offset,
point size, fog, color falloff/contrast) to `views/<scene>-<timestamp>.toml`.
View color params override the scene's when present. Load one with `--view <path>`;
in windowed mode the orbit starts paused so the framing holds (press O to resume).

`--render <out.png>` renders **headlessly** — no window, no event loop, no focus
stealing — and exits, printing a timing breakdown (`setup | chaos fill | render |
encode+save | total`) to stdout so you can budget effort. Options:

- `--effort draft|low|medium|high|ultra` — named presets for point count +
  accumulation: draft 1M/4, low 4M/16, medium 12M/48, high 40M/128, ultra
  100M/256. Explicit `--points N` / `--accumulate N` override the preset.
  Point count is a *render* property, not a scene property: the scene's
  `point_count` is just the interactive default (16 bytes/point of GPU memory).
- `--width/--height` (default 1920x1080) — per tile when a grid mode is used.
- `--view <path>` for exact framing (views store yaw + pitch), `--fog`.

**Grid contact sheets** (for exploring 3D framing cheaply — the point cloud is
filled once and re-rendered per tile, so 9 tiles cost barely more than 1):

- `--orbit-grid 4x2` — 8 views evenly spaced around a full horizontal orbit,
  starting at the base yaw. Row-major; per-tile yaw printed to stdout.
- `--move-grid 3x3 [--move-step 0.25]` — camera nudged left/center/right
  (columns) × up/center/down (rows) in the view plane by `move-step` × orbit
  distance, all still looking at the focus. Center tile = base view.

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
  (`GpuTransform` is 144 bytes; `CameraUniforms` is 112 and is declared in
  points/render.wgsl, gizmo.wgsl, AND density/voxel_render.wgsl — update all
  three; size tests in buffers.rs/compute.rs guard the Rust side)
- New variations: append to `VARIATION_NAMES` in scene.rs AND the matching slot
  in `apply_variations()` in chaos.wgsl; slots must stay in sync
