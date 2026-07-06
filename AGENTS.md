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
  app.rs         # App state, render orchestration, HUD/help text, screenshots
  scene.rs       # TOML scene parsing, TransformSpec, variation names/slots
  gpu/
    context.rs   # wgpu device/surface setup (vsync flag, adapter limits)
    buffers.rs   # GPU struct definitions (GpuTransform, Point, CameraUniforms)
    points/      # Active renderer: chaos compute + point rendering
      compute.rs   # Chaos game dispatch, circular buffer bookkeeping
      renderer.rs  # Dual pipelines: billboard quads / native 1px points
    gizmo.rs     # Transform gizmos (unit tetrahedra per transform)
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
3. **Gizmos + text**: optional overlays (see keybinds).

Performance on the reference machine (ThinkPad T490, Intel UHD 620, 1280x720):
- ~10M points at ~38 FPS uncapped (subpixel/point-primitive path)
- ~5M points comfortably at 60 FPS; billboard path is ~3x slower per point
- Storage-binding limits are raised to adapter max at startup (default 128MiB cap
  would limit the buffer to ~8M points; buffers are 16 bytes/point)

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
| S | save screenshot to screenshots/capture.png |
| [ / ] | shrink / grow point size |
| F / Shift+F | more / less fog |
| N / Shift+N | fog start closer / farther |
| M / Shift+M | fog end closer / farther |

## Scene Files (TOML)

Use `--scene <path>` to load. See `scenes/` for examples.

```toml
[meta]
name = "Scene Name"
author = "Your Name"
point_size = 0.002        # world-space point size
point_count = 4_000_000   # circular point buffer capacity (default 500k)
color_speed = 0.5         # global color blending speed (0-1)

[camera]                  # optional
focus = [0.0, 0.0, 0.0]   # orbit center / look-at
offset = [0.0, 1.0, 0.0]  # added to orbital camera position
distance = 3.0            # orbit radius

[[transform]]
name = "whorl"                 # optional label shown in overlays
translation = [0.0, 0.0, 0.5]
scale = 0.5                    # uniform scale
rotation = [0, 0, 0]           # Euler degrees (XYZ)
color = [1.0, 0.2, 0.2]        # contributes to the cyclic colormap
weight = 1.0                   # selection probability
color_value = 0.25             # optional explicit colormap index (0-1)
color_speed = 0.5              # optional per-transform override
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

## View Files & Offline Rendering

Press `V` in-app to dump the current view (orbit angle, distance, focus, offset,
point size, fog) to `views/<scene>-<timestamp>.toml`. Load one with `--view <path>`;
in windowed mode the orbit starts paused so the framing holds (press O to resume).

`--render <out.png>` renders a single frame **headlessly** — no window, no event
loop, no focus stealing — and exits. It fills the point buffer, renders once, and
saves. Options: `--width/--height` (default 1920x1080), `--points N` (override the
scene's buffer capacity for denser renders; 16 bytes/point of GPU memory),
`--accumulate N` (extra chaos frames after the buffer fills, default 32),
`--view <path>` for exact framing, `--fog`. Example:

```
fracturize --scene scenes/glasshouse.toml --view views/glasshouse-123.toml \
  --render renders/glasshouse_4k.png --width 3840 --height 2160 --points 12000000
```

A 4K render with 12M points takes ~30s on the T490. `--points` also works in
windowed mode. View files are the reliable way for agents to render specific
framings without keyboard interaction.

## Screenshot Support

- Press `S` in-app, or run `--screenshot --delay N` to capture and exit
  (N is in FRAMES, not seconds; at 60 FPS `--delay 150` ≈ 2.5s of accumulation)
- `./screenshot.sh [scene]` wraps build + capture
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
  (`GpuTransform` is 144 bytes; `var_weights` is `array<vec4<f32>, 4>` in WGSL)
- New variations: append to `VARIATION_NAMES` in scene.rs AND the matching slot
  in `apply_variations()` in chaos.wgsl; slots must stay in sync
