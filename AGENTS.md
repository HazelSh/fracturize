# Fracturize - AI Agent Guidelines

A 3D fractal flame renderer inspired by Apophysis, built with Rust and wgpu.

## Project Overview

Fracturize renders IFS (Iterated Function System) fractals in 3D using the chaos game
algorithm, entirely on the GPU. A compute shader runs thousands of parallel "walkers"
that iterate through weighted random transforms (affine matrix, nonlinear variation
blend, then a post-affine matrix), writing positions into a circular point buffer
that is rendered every frame.

**Adding a feature to the scene format? Read `ADDING-A-SCENE-FEATURE.md`.** It is
the checklist of every place a scene feature has to reach — file, save, `-S`,
`--info`, GPU, CPU walkers, GUI, undo, docs — and how to prove each one, with
subscenes as the worked example.

**Making artwork with it? Read `CRAFT.md` first.** This file is the reference for
what Fracturize *is*; CRAFT.md is the craft guide for authoring scenes as art —
measured doses, the numbers that predict a picture before you render it (start with
the similarity dimension), which variations are actually 3D, the traps, inherited
Apophysis-era lore and what of it survives here. It has a discovery log at the end;
add to it.

**Planning UI or workflow? Read `USERS.md`.** It names the people Fracturize is
for (Rook, Pip, Jay, Theo, Remy, Atlas, Mote) and the ones it isn't (Abe, Silk,
Socket), with their goals, failure modes and gaps. Plans say who they serve and
who they cost by those names.

### The other docs

| File | What it is |
|------|------------|
| `USERS.md` | Personas and anti-personas: who to plan for |
| `CRAFT.md` | Authoring scenes as art; the discovery log |
| `ADDING-A-SCENE-FEATURE.md` | Checklist for every place a scene feature reaches |
| `UI-REVIEW-PLAN.md` | The GUI read as a stranger would; a worklist |
| `FEATURE-PLANS.md` | Hazel's 2026-10-19 list as four tracks of plans |
| `AGENT-COLLAB-PLAN.md` | Human-agent proposals, review and the live bridge |
| `TIMELINE-PLAN.md` | Open questions on the timeline: auto-key, and moving the camera path into `[[track]]` form |
| `GIZMO-PLAN.md`, `XAOS-PLAN.md`, `PALETTE-PLAN.md` | Plans for features now built; the reasoning behind them |
| `RENDER-QUALITY-BASELINE.md`, `RENDER-QUALITY-PLAN.md`, `RENDER-GUI-PLAN.md`, `RENDER-SCALE-PLAN.md` | Offline render quality: measurements, plan, GUI, posters |
| `VOLUME-RENDERER-PLAN.md` | The volume renderer's plan and history |
| `NOTES-infinite-zoom.md` | Why `[zoom]` works the way it does, and the dead ends |
| `LIKENESS-PLAN.md`, `FIT-OPTIMIZER-PLAN.md`, `IFS-INVERSE-BACKGROUND.md` | Fitting a scene to a picture: plan, solver, background theory |
| `AUTHORING-BRAINSTORM.md`, `XAOS-AND-BEYOND-BRAINSTORM.md` | Brainstorms: proposals, not decisions |

## Architecture

```
src/
  main.rs        # CLI args, winit event loop + egui event gating, keybinds, default scene
  app.rs         # App state, mouse/edit handling, render orchestration, screenshots
  history.rs     # Unified snapshot undo/redo behind App::commit_edit (all edits)
  haze.rs        # Aerial perspective: one amount, band and falloffs derived
  indicators.rs  # Selection offset/rotation lines, and camera-path polylines
  dice/          # One generator for random flames AND mutations (see "The dice")
    mod.rs         # Action::{Roll, Mutate} on a base scene, the one search loop
    settings.rs    # RandomSettings: one list of Parts (on/off), their ranges
    roll.rs        # Rolls: every part that's on drawn, the rest kept (rng order!)
    mutate.rs      # Mutations: a few parts that are on, nudged or re-rolled
    gate.rs        # The quality gate: faults, judged absolutely or against a parent
    profile.rs     # Profiles as files: format, built-ins, named lookup, comment-keeping save
    profiles/      # The built-in profiles, compiled in (anything, colour-only, …)
  random_list.rs # The Random flames list's model: rolls on worker threads
  mutation_grid.rs # The Mutate window's grid: layout (shared with --mutations 8|24),
                 # tiles mutated on the same workers, a seed remembered per parent
  renorm.rs      # Infinite zoom: renormalize an IFS into a scale-invariant set
  subscene.rs    # Composite scenes: layers, placement, walker (= point) partition
  xaos.rs        # Xaos: per-map transition rows, the exact sampler (GPU twin),
                 # walk shares, Mauldin–Williams dimension, presets
  derive.rs      # --derive: print a map turned about a world axis / spun about its own
  stats.rs       # --stats: image numbers, empty measured against the recorded background
  render_job.rs  # Render job model: params, events, pause/cancel, estimates
  render_preset.rs # Render presets: named size+quality files, built-ins in render_presets/
  ui/            # egui layer (see "Human Interface" below)
    mod.rs         # EguiLayer, UiState, per-frame draw order, font install
    toolbar.rs     # Top strip: File menu, panel toggles, undo/redo, quick controls
    status_bar.rs  # Bottom bar: context hints, FPS/p99 sparkline, point stats
    hints.rs       # hinted(): tooltip + status-bar hint on one widget response
    num.rs         # Numeric readouts that don't move: the fixed-width house rule
    confirm.rs     # Click-wait-click Arm, and the unsaved-changes modal
    transforms.rs  # Transform tab rail + selected-transform detail pane
    subscenes.rs   # Subscenes window: layers, eye, share, import, placement
    xaos.rs        # Xaos: inspector block, matrix window, viewport edge overlay
    history.rs     # History window: every edit, click a row to jump to it
    mutate.rs      # Mutate window: profile, strength, the mutation grid
    random_flames.rs   # Random flames: scrolling list of live-thumbnail rolls
    collab.rs      # Collaborate: the thread, option cards, the composer, conflicts
    agent.rs       # The connection light, and Agent settings
    random_settings.rs # The dice profile: parts on/off with their ranges; the shared picker
    render_panel.rs# Renderer mode, exposure, point size, colour, haze, infinite zoom
    save_as.rs     # "Save scene as…" modal (fork the scene under a new name)
    render_job.rs  # Render job window: presets, sectioned setup, progress, the result
    camera_panel.rs# Framing, saved views, the camera path (incl. how it loops)
    axis_widget.rs # Corner orientation cross; click a ball to look down that axis
    radio.rs       # Segmented one-of-n radio, used by all three such settings
    browser.rs     # Scenes window (Ctrl+O / B): folder tree + places, thumbnail rows
    shortcuts.rs   # Controls window (H / F1): input prefs + the keybind table
    timeline.rs    # Timeline window: transport, ruler, camera row, track rows
    param_picker.rs# Pick a scene parameter: browsable tree + search (reusable)
    labels.rs      # World-anchored transform name labels
    icons.rs       # Phosphor codepoints (vendored font, see assets/fonts/)
  camera.rs      # OrbitCamera (yaw/pitch/distance/focus), ray + projection helpers
  path.rs        # CameraPath: Catmull-Rom splines over orbit keypoints
  track.rs       # Tracks: -S paths over time (keys/rate/cycle), the shared
                 # Extrapolate fold, Animator (the scene at time t), classes
  params.rs      # A scene's settable parameters as a named tree (each an -S
                 # path checked on the scene), and their valid ranges
  video.rs       # Animation formats: Format (avif/mp4), RGBA->YUV, shared ISOBMFF muxer
  avif.rs        # AV1 backend for .avif (rav1e)
  h264.rs        # H.264 backend for .mp4 (openh264)
  pick.rs        # Gizmo hit-testing and drag geometry (pure math, unit-tested)
  trace.rs       # CPU chaos walkers (variation port) for the trace overlay
  volume_ref.rs  # Exact emission-absorption transport for volume mode, to score it
  prefs.rs       # Persistent user prefs (~/.config/fracturize/prefs.toml)
  scene.rs       # TOML scene parsing AND saving, TransformSpec, variation names/slots
  scene_source.rs# A path's scene TOML: the file, or a render PNG's embedded chunk
  scene_diff.rs  # --diff: what changed between two scenes, and the three-way merge
  thread.rs      # Threads with an agent: the message format on disk, reading, posting
  collab.rs      # The CLI side of a thread: --request, --propose, --respond, --wait-reply…
  app_collab.rs  # The GUI side: preview (a sandbox), apply (a merge), send back, keep
  bridge.rs      # The window's socket: JSON lines, one bridge, the client end
  mcp.rs         # `fracturize mcp`: stdio MCP server, tools, channel push
  agent.rs       # [agent] prefs, the headless Claude backend, with-llm
  agent_local.rs # The local-model backend: author.py's loop, in the window
  app_agent.rs   # The window's side: socket calls, dispatch, the runs
  scene_browser.rs# The Scenes window's model: places, a folder's scenes, parsed, sorted
  symmetry.rs    # Finite subgroups of SO(3) (C/D/T/O/I) by closure, plus
                 # `repeat` progressions, which are not groups
  gpu/
    context.rs   # wgpu device/surface setup (vsync flag, adapter limits)
    buffers.rs   # GPU struct definitions (GpuTransform, Point, CameraUniforms)
    points/      # Active renderer: chaos compute + point rendering
      compute.rs   # Chaos game dispatch, circular buffer bookkeeping
      renderer.rs  # Dual pipelines: billboard quads / native 1px points
      splat.rs     # Splat mode: additive log-density accumulation + tonemap;
                   # also volume mode's passes (spans, slices, weighted emission)
      volume.rs    # Volume mode's uniform, reference length, slice constants
      moments.rs   # The retired moment reconstruction, kept for the reference
    thumbs.rs    # Scene thumbnails for the browser: a 256-walker ring, splatted
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
   apply affine matrix, blend the 20 variation functions by weight, then apply the
   post-affine matrix (identity unless the scene sets one). Diverged or
   NaN walkers are re-seeded randomly (important for nonlinear variations).
2. **Point render** — three interchangeable modes over the same point buffer
   (`R` cycles live; `--splat`, `--volume` / a view's `renderer = "splat"` or
   `"volume"` select at launch):
   - **points** (`points/render.wgsl`, default): opaque depth-tested points with
     adaptive pipeline selection. When the projected point size at orbit distance
     is subpixel (the common case), points are drawn as native 1px point
     primitives (~3x faster). Otherwise, 4-vertex instanced triangle-strip
     billboards with perspective sizing. Every point is full brightness.
     Near-field growth is capped at `CameraUniforms::max_point_pixels` — 12
     *output* pixels, in both modes — so points brushing past the camera in
     volume-filling scenes render as motes, not screen-eating squares/gaussian
     washes. It is 12 render-target pixels only at 1x, which is what the live
     window always is; an offline render scales it by the supersample factor,
     for the reason in "Supersampling and the reconstruction filter" below.
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
   - **volume** (`points/splat.wgsl`, `volume.rs`, `volume_ref.rs`): splat with
     absorption, so near material hides what is behind it. The fractal is a
     glowing fog: every point emits (as in splat) and also absorbs, and a point
     reaches the eye attenuated by `T = exp(−τ)`, τ the absorbance in front of
     it. Plan and history are in `VOLUME-RENDERER-PLAN.md`. Live and the one-pass
     still; five passes over the point buffer and the pixels:
     **A0** finds each pixel's nearest and farthest fragment depth (atomics);
     a fullscreen pass rewrites those as a log origin and scale; **A** adds each
     fragment's absorbance, as fixed point, into one of `BINS` = 32 equal slices
     of *that pixel's own span* (a storage buffer, 4 bytes a slice a pixel);
     a fullscreen pass turns the slices into running totals; **B** is the
     ordinary splat accumulation with each fragment's weight multiplied by the
     transmittance of what is in front of it, read as two words (the total
     before its slice, plus its position through its own slice), own deposit
     excluded. Total absorbance is the last running total.

     **Judge changes to it against the exact reference, not by eye.** Slices one
     to three were tuned by eye against symptoms (an overestimation constant, a
     blur, an emission gain, a fill), and the reference showed afterwards how
     wrong they had been: see "The exact reference" below.

     Things that are load-bearing, and easy to undo by accident:

     * **Opacity 0 is not "almost splat", it is splat.** The host draws the
       plain splat pipelines and never runs the volume passes
       (`SplatRenderer::set_volume` with `VolumeParams::OFF`), so
       `--volume --opacity 0` is pixel-identical to `--splat` (checked on
       `blossom` and `censer`; the PNG *files* differ only by their embedded
       record).
     * **Absorbance is a world quantity.** A point carries a cross-section
       `opacity · K · L² / N` in world area (`L` the scene's reference length:
       the attractor's radius, under infinite zoom the eye's distance to the
       fixed point, which is what keeps it wrap-invariant; `K` = 2, a
       calibration) and deposits that over the world area of a pixel at its
       depth, `(H / (2·z·tan(fov/2)))²` (`VolumeParams`, `volume_deposit`). So
       optical depth is a property of the material and does not change with
       point count, resolution, `--supersample` (H still appears squared) or
       **camera distance**. Slice 1b normalised in screen space (`opacity · H² /
       N`, as exposure is), which is right for a brightness and wrong for an
       absorbance: zoom out k times and per-pixel optical depth grew ~k². That
       was the bleed and, once each point became several optical depths thick,
       the flicker. Median `b0` over lit pixels on nautilus at opacity 2 and
       0.5/1/2/4x the authored distance: 0.53 / 2.9 / 12.3 / 46.9 then, 1.89 /
       2.04 / 2.03 / 1.90 now. Past the footprint cap (`max_point_pixels`) a
       point's deposit is scaled by the area it no longer covers, so a mote the
       camera brushes past does not turn solid.
     * **A point does not shadow itself**: its own deposit is taken out of its
       slice before it is read.
     * **The per-pixel span is what makes slices work.** The same slices over one
       depth range for the whole frame are ~7x less accurate: a pixel's material
       sits in a sliver of the attractor's depth. The cost is two housekeeping
       passes and the span jitter of a pixel whose extremes are a few fragments.
     * **Emission has a source function** (`emission_weight`, `volume::SOURCE` =
       1): a fragment emits its splat weight *plus* `SOURCE` times its absorbance,
       in splat-density units scaled like exposure by N/H². The bare transport
       is right and dim — emission per unit of absorbance falls as 1/opacity, so a
       thick pixel carries 1/opacity of splat's light — and a medium that absorbs
       also glows, so what a thick region glows *at* is the ratio, a constant
       here. The extra term vanishes with the absorbance: opacity 0 is splat.
       Chosen by eye, once, against splat and the exact reference's own
       pictures on seven scenes; `0` gives the physical dim picture.
     * **The tonemap** (`fs_tonemap_volume`): `density` is the *visible*
       radiance, `Σ e·T`, so a thick pixel is as dim as the transport says, in its
       own colour; the background survives twice over — past coverage as in
       splat, and past absorbance: `bg · (1 − coverage) · T_total`, `T_total` the
       mean of `exp(−b0)` over the output pixel's block. A pixel with absorbance
       and no emission shows `bg · T_total`, not the background (that early
       return used to ignore `T_total`). There is no fill: the constant
       0.3-of-own-colour of slice 1c was compensating for a transport that did
       not know how much light a thick pixel carries.
     * **There is no blur, no overestimation constant and no emission gain.**
       The blur spread absorbance into neighbours' emission (a halo of dimming
       around bright points, front fragments 26% too dark on average) and added
       0.1 of error to `T_total`; the moment reconstruction was biased by its
       overestimation constant; the gain was a patch for the 1/opacity dimming.
       `moments.rs` is kept only so the reference can still score it.
     * **Haze scales emission only; the zoom edge guard scales both**;
       `--isolate`d points deposit no absorbance (the vertex stage clips them).
     * **It needs the fragment stage to write storage**, six buffers' worth. It
       does not need fp32 blending any more (the absorbance is u32 words), so the
       live device no longer requests `FLOAT32_BLENDABLE`.
     * **Opacity is session state** for now (the Render window's "Session"
       heading, `--opacity`, default 2): not a scene key, not undoable, not in
       views or prefs. A view saved in volume mode records the *mode* only.
     * **One-pass stills and the window only.** `--spp`, `--effort` tiers that
       accumulate, `--resume`, animations, grids and sheets refuse `--volume` by
       name (`offline::reject_volume`); a render job from the dialog renders
       plain splat and says so. A render too large for the slice buffer (32
       slices a accumulation pixel against the GPU's storage limit) says so too.
     * **Cost**: ~5x splat's render stage offline (20M points, 1080p, ss2:
       0.56 s against 0.09 s), and 14.7 FPS against splat's 36 live at 8M points
       on the GTX 1080 (the 1c build was 24). The per-fragment work is O(1); the
       cost is three passes over the point buffer and the pixels.

     **The exact reference** (`volume_ref.rs`; `FRACTURIZE_VOLUME_REFERENCE=
     <prefix>` on a `--volume --render --supersample 1` still, 8-bit). The same
     points go through the same vertex stage, kernel, world-space absorbance and
     emission weights into a *list* of fragments (`fs_dump`) instead of being
     blended; the CPU sorts each pixel's by depth and composites front to back.
     It writes the report (`<prefix>.report.txt`: error of the GPU's `L` and
     `T_total`, and where the transmittance error sits by rank, optical depth,
     deposit size and depth), the reference through the same tonemap
     (`<prefix>.ref.png`), and `FRACTURIZE_VOLUME_EMULATE=1` scores candidate
     estimators on the same fragments without a shader, which is how the slices
     were chosen over moments, per-frame ranges over global ones, and 32 slices
     over 8 or 16. At opacity 0.001 the GPU path agrees with it to 0.5%, which
     is the check that the reference is the same transport. Use it before
     retuning anything here. `FRACTURIZE_VOLUME_STATS=1` prints the absorbance
     field's median, p90 and p99.
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
(measured by `trace::measure`, the same CPU walkers `dice/gate.rs` gates on)
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
| ctrl+drag the origin dot or an axis shaft | uniform scale (drag up = grow) |
| shift during a gizmo drag | fine: a fifth of the travel. Anchored, so pressing or releasing it mid-drag doesn't jump |
| ctrl+drag an outer gizmo edge | snap the rotation to 15° (or the transform's symmetry-group step) |
| ctrl+drag an axis endpoint | snap that axis's scale to 0.1 |
| drag the ring around the selected gizmo | roll it about the camera's view axis, through its own origin |
| ctrl+drag the ring | snap the roll to 15° (or the transform's symmetry-group step) |
| alt+scroll over a gizmo | adjust that transform's chaos weight (probability) — the lever that emphasizes an element without changing structure or color |
| drag a whole subscene's gizmo | move / turn / resize the entire subscene (select its row in the Subscenes window first); its tips resize uniformly, and alt+scroll over it sets its share of the points |
| click a Transforms row | select that transform (two-way with gizmo selection) |
| double-click a Transforms row | rename it inline |
| drag a row's weight bar | change that transform's chaos weight |
| alt+click a row's eye | solo it; alt+click again brings everything back |
| click an xaos edge (overlay on) | open its popover: multiplier, probability, cut/restore, only exit |
| alt+scroll over an xaos edge | scale its multiplier — the weight lever, one level down |
| right-click an xaos edge | its menu: cut, restore, only exit, linked map |
| ctrl+right-drag (overlay on) | knife: cut every edge the stroke crosses, as one edit |
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
backdrop the selected one gets (`src/ui/labels.rs`); a bare label carries a
1px black drop shadow instead, for the same reason the origin dots wear a
rim. Both are drawn over whatever the attractor happens to be, and white on
a pale fractal is not text. Note `Painter::galley`'s colour argument is only
a *fallback*, so the shadow and the label share one galley laid out in
`Color32::PLACEHOLDER` — laid out white, the shadow would paint white too. Bare white text disappears
against a bright attractor, and reading a name is how you decide whether this is
the transform you meant -- which matters most before you have selected it.

**Ctrl snaps wherever the grab has something to snap to** — a tip's scale, a
rotate edge's angle, the ring's — and means uniform scale everywhere else. It
used to be alt, and alt+drag never reaches the app on XFCE, which claims it for
move-window: a modifier the window manager eats is not a modifier. Ctrl was free
to take over on exactly those handles because "ctrl = uniform scale" was
redundant there; uniform scale is still reachable from the origin dot and every
axis shaft. Alt is kept as an alias for desktops that leave it alone, and
alt+scroll (chaos weight) is untouched — the window manager only takes drags.

Three rules the gizmo geometry depends on:

* **The tip handles scale by rewriting one matrix column, never by decomposing.**
  The axis direction is captured at grab and held fixed, so the column only
  changes length, the three columns stay perpendicular, and the matrix stays a
  faithful T·R·S. This is not fussiness: the scene format has no way to write a
  sheared matrix, so a drag that introduced shear would author something the
  save silently discards (see `GIZMO-PLAN.md` §1.1, which also notes that
  `dice/mutate.rs` already hits this).
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
| O, Z, or Space | play / stop the timeline: the camera flying its path, and every track (three keys, one action) |
| Y / Shift+Y | add the current framing as a camera key after the last one (the playhead goes to it) / remove the last one |
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
| R | cycle renderer: points / splat (additive log-density) / volume (splat with absorption; skipped without fp32 blending) |
| W / Shift+W | splat exposure up / down |
| [ / ] | shrink / grow point size |
| D / Shift+D | finer / coarser color detail (color_falloff) |
| C / Shift+C | less / more color contrast |
| F / Shift+F | more / less atmospheric haze (the depth cue; see "Haze") |
| Ctrl+Shift+S | save scene as… (fork under a new name) |

The Controls window (H or F1) is clickable: each row triggers its first-listed
binding, shift+click the second. `I` persists to
`~/.config/fracturize/prefs.toml` (user preferences, not scene data).

**Dropping a file on the window opens it** — a `.toml` scene or a `.png`
render (see "Scene Files") — through exactly the door the Scenes browser uses,
`App::request_load_scene`, so unsaved edits get the same prompt. The file is
parsed *before* that prompt, so a file that won't open never asks about your
edits. A multi-file drop opens the first and says it ignored the rest; a
failure leaves the scene alone and says why in the status bar (`App::notice`,
which also catches browser load failures — those used to be log-only). It
stays until clicked away or the next open, not on a timer. While a file is
hovered over the window, the status bar says whether it will open.

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
File  Explore | Transforms  Subscenes | Camera  Render | Undo Redo | quick controls … scene name | Help
```

ordered by task rather than by module: where does a scene come from, what am I
working on, then how am I looking at it, then help.

**File and Explore are menus, and the only two** — not the first entries of a
File/Edit/View/Help bar. The scene is a document, and the value of that
abstraction is that people can port intuition from every file-editing program
they have used; but intuition needs furniture to attach to, and a File menu
with the conventional contents in the conventional order is that furniture.
What it deliberately is *not* is a menu bar: Edit and View would be near-empty
duplicates of the toolbar toggles, and a menu bar that is mostly empty teaches
people that menus here aren't worth opening. File operations are numerous,
conventional and infrequent — the trade a menu is right for. Undo and redo are
the opposite trade, so they get visible buttons as well as menu rows.

**Explore is the other menu**: the ways of finding a scene by chance, and the
undo that makes them safe to try. Random flame, the Random flames list, Random
settings…; Mutate (U), Mutate…; Undo, Redo, History…. It used to be a window
holding five unrelated jobs in a column (two "new scene" buttons, the mutation
slider, undo/redo, the history list). Each is its own thing now — commands are
menu rows, and what needs room is a window the menu opens, lit in the menu
while it's open. The blank scene went to File (`New blank scene`, Ctrl+N),
where "start a new document" lives in every program.

| Window | What lives there |
|--------|------------------|
| Transforms | Add/duplicate/delete over a vertical tab rail (colour swatch, name, eye toggle, draggable weight bar, filter box past a dozen) plus a detail pane grouped Shape / Post-affine / Symmetry / Behaviour / Appearance, under a header whose name is click-to-edit |
| History | Every edit, newest first, redo rows above the current marker; click a row to jump N steps in one rebuild |
| Mutate | The dice profile picker, mutation strength (the profile's), 3×3 / 5×5, New, Mutate, Undo, Settings… — then the mutation grid: the scene in the middle, its mutations around it, click one to make it the scene. See "The mutation grid" |
| Random flames | The dice profile picker, then a scrolling list of rolls, docked down the left by default; live thumbnails, click one to open it. See "Random Flames" |
| Random settings | The dice profile: picker, save / save as / revert (with a `*` when changed); one list of parts — shape, structure, colour — each switched on or off with its ranges under it; the quality check; how big a mutation's step is; Roll one and Mutate at the foot. See "The dice" |
| Render | Renderer mode, exposure, point size, colour falloff/contrast, haze, infinite zoom (incl. the zoom-map picker) — then point count, under its own heading |
| Camera | Framing, saved views, the camera path (keypoints, how it loops, playback) |
| Scenes (Ctrl+O, B) | Folder tree and places on the left, the folder's scenes (and renders with a scene in them) as thumbnail rows on the right; the same selection the arrow keys walk. What File → Open… shows you. See "The scene browser" |
| Controls (H, F1) | Orbit style, invert pitch, panel scale — then the keybind table, scrollable, rows clickable |
| Subscenes | The layers of a composite: eye (alt+click solos), share of the points, map count; Import a scene file, Remove; the selected subscene's placement, colour source and provenance. See "Subscenes" |
| Collaborate | This scene's thread with an agent: requests, proposals as option cards (Preview, Apply, Send back, Reject, Keep), responses; a composer (Send request, Propose my edit). The toolbar button counts what's waiting on you. See "Threads" |
| Xaos | The current layer's transition matrix as a heatmap (a cut is an ×, not a shade): click, drag, alt+scroll, shift+drag paint; header menus; a walk-share strip; presets. See "Xaos" |
| Timeline | Wide and low, above the status bar: play/pause, the playhead against the timeline's length, loop; a ruler to scrub, the camera's row (link: attach/detach), a row per track named for people, with its keys and a faint value curve (drag a key to retime it); eye to mute, right-click to delete. A column on the right edits the selection — a track's interpolation, extrapolation, keys and numbers; the camera's attachment, duration, loop and keypoints — and "Animate a parameter…" browses the scene's parameters. See "Tracks and the timeline" |

All of them persist their open state to prefs, and all behave alike.

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

  **Every number box is `num::Drag`, never a bare `egui::DragValue`.** Same
  builder methods; the difference is the drag. egui's snaps to the roundest
  number within a pixel each frame and rounds that, so values stick and jump;
  `Drag` accumulates exactly, at half the call site's speed
  (`num::DRAG_GAIN`), so every pixel of travel moves the value one even step.

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
     and more explicit label — `Stop` → `Stop render`, `Discard` →
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
  plus a "Remove shear" button. Mutations produce such matrices, so
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
  it dropped so the History window can say so instead of quietly ceasing to be
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

`--random` (windowed or with `--render`), Explore → Random flame, and every row
of the Random flames list all call `dice::roll_seeded(settings, base, seed)`
— which, with every part of the profile on, ignores the base (see "The
dice").
Randomising an IFS is easy; randomising one that renders is not, so every
candidate runs a short CPU chaos game (`trace.rs` walkers, the same variation
port the shader uses) and is rejected unless it lands in a bounded attractor
with real extent on two axes and *fractal* rather than solid occupancy.

**What gets rolled is `RandomSettings`** — a preference (`[random]` in
prefs.toml, edited in the Random settings window, saved as named profiles —
see "The dice"), never scene data. Its ranges:

- maps per flame, and the range each map's scale is drawn from (log-uniform),
  plus the chance of a squashed axis;
- which variations may be rolled, each with its own weight range (a magnitude;
  a separate chance makes it negative). `linear` is never listed — it is the
  remainder, `1 − Σ|w|`, floored at 0;
- how many *kinds* a flame draws from, and how many each map carries. Kinds are
  picked per flame and shared across its maps, because one kind across several
  maps reads as a coherent form and freely mixed ones read as a pile;
- **symmetry** (`SymmetrySettings`, off at chance 0 by default): which kinds
  may be rolled — Cyc/Dih with a fold range, Tetra, Octa, Icosa, and a
  tapering-helix `repeat` — and how many maps stay *outside* the group (default
  one, per CRAFT §3.6: a wholly symmetric flame is wallpaper), plus mirror and
  orbit-colour chances. Motifs shrink as they join, by `(Σ eᵢ^2.2)^(-1/2.2)`
  over the elements' sizes, so sixty copies under Icosa don't fill space into a
  lump — about Reliquary's hand-sized facet. Measured at chance 1 with every
  kind on: 200/200 rolled, ~2 tries and ~2 ms each;
- bounds on the **similarity dimension** and the **lacunarity summary** — the
  status bar's `d` and `Λ`, the same functions. Dimension steering counts a
  motif once per group element, as `similarity_dimension` does;
- the try budget.

**The two bounds are met in different ways, because they cost different
amounts.** Dimension is analytic and moves predictably with scale: one factor
`k = (Σ sᵢᵗ)^(−1/t)` on every map lands it exactly on a target `t`. So a roll
outside the bound is *steered* onto a target drawn inside it — the maps keep
their ratios, the scale range stops applying, and it costs nothing; only a
rescale that would leave a map barely contracting is a rejection. Lacunarity
has no such lever, so it is rejection-sampled. Measured on 200 default rolls
(`cargo test --release random_flame_study -- --ignored --nocapture`): ~0.9 ms
and ~1.15 candidates per roll, `d` p5–p95 0.75–2.58, `Λ` p5 7.5 / median 16 /
p95 80 — so a lacunarity bound of [5, 15] still accepts about 45%. Rejections
are counted by reason (`Rejections`) and every failure says which bound did it.

**A roll that finds nothing says so.** With the defaults that almost never
happens, and `--random` without a config still falls back to the last
candidate as it always has. With settings, the dice leave your scene alone and
put the reason in the status bar, `--random --random-config` exits 1, and the
list stops rolling after eight empty rolls in a row and offers "Keep trying" —
a list that silently stops growing would read as a bug.

**Every roll is reproducible from its seed**, and is named after it
(`random-<seed>`). `--random --seed N` reproduces a roll made with the
defaults; `--random-config <name|file>` supplies a dice profile (see "The
dice") — by name while the window's settings are an unchanged profile, else
`~/.config/fracturize/prefs.toml`, which holds them as they are — so a row's
tooltip prints exactly the command that rolls it again. A rolled flame
is a normal history entry: one Ctrl+Z restores what was there.

**The Random flames list** (`src/ui/random_flames.rs`, model
`src/random_list.rs`) rolls on two or three worker threads, so a tight
lacunarity bound never costs a frame, and keeps eight rows ahead of the last
one on screen — scroll and it grows, without end. Changing a setting restarts
it once the settings have been still for 400 ms, and drops any roll still in
flight under the old ones. Thumbnails share `gpu::thumbs::ThumbCache` with the
scene browser (keyed by `ThumbKey::Random(id)`), so the same rules hold: only
the hovered row flies its camera, and thumbnails are never reframed. Hover is
reported per frame (`ThumbCache::hover`) rather than set, so two lists open at
once can't clear each other's.

Colour is rolled too, not just form. The colour *source* is one of all three
modes — `transforms` half the time (the per-transform RGBs are generated for
that ring), `palette` and `mix` a quarter each; a palette-mode roll rolls its
gradient from `palette::random`. The **background** is rolled from a colour
the flame actually renders in (one of the transform colours, or a sample of
the gradient), hue-rotated in Oklab — usually within the flame's own family,
sometimes to its complement. Two thirds of rolls are dark and saturated; the
rest are mid and near-neutral — linear 0.06–0.2, which displays as 27–49%
grey — and deliberately stop short of white, because
the point pass composites the flame over the background by coverage and a
paper-white ground would swallow colours rolled at value 0.7–1.0. Override any
of it with `--palette` / `--random-palette` / `--color-mode`, which apply after
the roll.

## The dice: one profile, one list of parts, rolls and mutations alike

`src/dice/` is one generator. A **profile** (`RandomSettings`) is one list of
the **parts** of a flame — each transform's position, turn, size, weight and
variations; how many transforms; the symmetry group; which map may follow
which (xaos); the colours (each
transform's, the colour source, the gradient); where each sits on the
gradient; the background — each **on or off**, with the ranges it is drawn
from. **A part's odds are its only state**: 0 is off, anything above is on,
and the least a part that's on can have is `MIN_ODDS` (0.0001), so on and
off can't be said two ways. Ticking a part on in the window starts it there. Two actions read that list, the same way (`dice::Action`):

- **Roll** draws every part that's on and keeps the base scene's own for
  every part that's off (`roll.rs`).
- **Mutate** changes a few of the parts that are on: nudged (scaled by
  strength), or re-rolled with the very draw a roll makes (`mutate.rs`).

So **a part that's off means one thing whichever button is pressed: the dice
leave it alone.** There is no roll half and mutate half. That is what makes a
mask mean something to both: under `colour-only` a roll recolours the scene
from scratch and a mutation shifts a hue; under `no-colour` a roll makes a
new form in the scene's colours and a mutation reshapes. It used to be two
halves — `[roll]` ranges and `[mutate]` odds — and a mask on the mutate half
quietly did nothing to rolls.

The only settings one action reads and the other doesn't are a mutation's
**step** (`[mutation]`: strength, changes per mutation, ease-off, dimension
steering) and how big each part's odds are and its **re-roll** chance, which
say *which* of the parts a mutation picks and how — a roll takes every part
that's on. A part at `MIN_ODDS` is drawn by every roll and all but never
picked by a mutation. That is how the default treats the background and the
symmetry group (a bigger step, to ask for), and how `no-symmetry` says "a
roll draws no group" — off would instead keep the scene's own.

Both go through **one search**, `dice::generate(settings, base, action, rng)`:
draw a candidate, steer the dimension if size is the dice's to change (a
mutation only with `steer_dimension`), judge it at the gate (`gate.rs`), go
again until one passes or the budget is spent. Both spend the same try
budget, count the same `Rejections`, fail with the same `RollFailure`.
**What a candidate is judged against depends only on what it owes the base**:
a roll with every form part on is held to the profile's bounds absolutely;
anything that keeps some of the base's form — every mutation, and a roll with
a form part off — is held to *the base's* standing on each check, no worse
than it was (5% slack, because the measurements are walks). Without that, a
hand-authored scene outside the bounds — plenty of good ones are — could
never be mutated or recoloured. **A mutation's strength halves after a run of
`backoff` rejections**, down to a sixteenth, and the log says so.

**The base.** `U`, the Mutate window and Roll one use the scene on screen.
The Random flames list takes the scene on screen when it starts over (a
profile or setting change, New rolls), and rows don't follow later edits —
opening a row would otherwise throw the list away. `--random` and
`--random-sheet` take `-s` as the base, or the blank scene without one. With
every part on — the `default` profile — **the base is never read**: every
draw is made in the order it always was and only then are off parts replaced,
so every recorded `--random --seed N` still rolls the same flame (checked
against a build from before the dice: byte-identical `--info`). The one
exception is the transform count: off, it is the base's, and nothing is drawn
for it. A roll that keeps the whole form keeps the base's camera, point size
and look too — it's the same flame, recoloured.

**Profiles are files and have names.** Built in, compiled from
`src/dice/profiles/` so they exist wherever the app is started from:
`default` (the shipped settings: every part on, bounded variations, no
symmetry), `anything` (every variation, symmetry now and then, every part
may mutate, xaos about one roll in three), `no-symmetry` (anything's range,
no group and no xaos ever drawn), `colour-only`, `no-colour`, `symmetric`,
`xaos-fun` (six rolls in seven get a graph, under a dimension bound). Then
`~/.config/fracturize/dice/<name>.toml` (the person's own) and
`scenes/dice/<name>.toml` (shared, relative to where the app runs). A saved
profile can't take a built-in's name. The file reads like a scene file:

```toml
[meta]
name = "spiky"
description = "Few transforms, sharp folds, never smooth"

[parts]                       # one line per part
color = { odds = 0.0 }        # off: kept as it is, by a roll or a mutation
variations = { odds = 0.4, reroll = 0.25 }

[ranges]                      # how each part is drawn
transforms = [2, 4]
lacunarity = { on = true, range = [12.0, 60.0] }

[ranges.variations]           # one line per variation
julia = { enabled = true, weight = [0.2, 0.6] }

[ranges.symmetry]
chance = 0.3

[mutation]                    # how big a step a mutation takes
strength = 0.7
```

Every key is optional and takes the **shipped default at every depth** (a
part given only `odds` keeps its shipped re-roll chance), and a key this version doesn't
know is reported — `--random-config` prints it to stderr, the window puts it
in the status bar — rather than silently ignored. A fresh save writes every
key with a one-line note; saving over an existing file merges with
`toml_edit`, so comments survive the window's Save. Floats are written at
`f32` precision. Two older shapes still read: a bare settings table, and any
file with a `[random]` table — prefs.toml.

**In the app** the profile is picked where it's used: a picker at the top of
the Random flames list, in the Mutate window, and as Explore → Dice profile —
all the same picker (`ui::random_settings::profile_picker`), built-ins under
their own heading, each one's description on hover, a `*` while edited. The
Random settings window edits it in eight folding sections, **each one table
holding one kind of value** (`ui::table`, `egui_extras::TableBuilder` in the
house style: hinted column headers, strong stripes and faint column rules —
you read a table along its rows — exact widths with the name column taking
the slack, and every switch in an **On** column flush right, so the ticks
line up from one table to the next; a part's tick and its odds are one
state, ticking on sets the least odds and dragging the odds to 0 unticks):
**Parts** (part · odds · share of a mutation's changes · re-roll, grouped
Shape / Structure / Colour), **Ranges** (setting · low · high), **Chances**
(setting · chance, as a percentage), **Symmetry groups**, **Xaos** (none /
preset / random graph · odds · share, then the presets), **Variations**
(variation · low · high weight), **Quality check** (bound · low · high, then
the least walk share under xaos and the try budget) and **Mutation step**; Roll one and Mutate at the foot. A
range or chance row greys while the part it draws is off. They used to share
one grid, chances and low/high pairs in the same unruled columns, and read as
a scatter of fields.
prefs.toml keeps the settings (`[random]`) and where they came from
(`random_profile`, or `random_profile_builtin` by name), so the `*` survives
a restart. Loading a profile isn't undoable — it's a preference, not the
scene — which is what Revert and the profile itself are for. Changing only
what a mutation reads doesn't restart the Random flames list
(`RandomSettings::same_roll`).

**Seeds.** Mutations are seeded too (`mutate_seeded(profile, parent,
seed)`), but mutation sheets made before the gate won't reproduce: the gate
changes what is drawn.

**CLI.** `--random-config <name|file>` works for `--random`, `--mutations`
and `--random-sheet`, and is refused without one of them.
`--mutation-strength` overrides the profile's strength. `--random-sheet N`
(1–25) is the random batch sheet: tile N is `--random --seed <seed+N>`,
framed by its own camera on its own background, saved as `<out>.rollN.toml`.
A mutation sheet tile where nothing passed is left empty, labelled, and
explained on stdout.

```
# a sheet of nine rolls under a built-in profile
fracturize --random-sheet 9 --random-config symmetric -r renders/sym.png \
  --effort tiny --width 320 --height 180
# koru recoloured nine ways, by rolls and then by mutations: the form untouched
fracturize -s scenes/koru.toml --random-sheet 9 --random-config colour-only \
  -r renders/koru-col.png --effort tiny --width 320 --height 180
fracturize -s scenes/koru.toml -r renders/koru-mut.png --mutations 8 \
  --random-config colour-only --effort tiny --width 320 --height 180
```

**Xaos is a part** (`[parts] xaos`, `[ranges.xaos]`, `XaosSettings`). A roll
draws one of three kinds of graph by odds — **none**, a **preset**, or a
**random** graph — and `default` has odds only of none and *draws no die
for it*, which is what keeps every recorded `default` seed rolling the same
flame. Four things it rests on:

- **Only some presets can be rolled, and only at some sizes.** `cycle` never:
  one exit per map makes the walk a fixed sequence and the picture collapses
  to n dots. `no-self-loops` needs three maps (with two it is a strict
  alternation, which collapses the same way); `blocks:N` needs two maps a
  block. A preset that doesn't fit isn't drawn.
- **A random graph is strongly connected by construction**: a cycle through
  every map in random order is kept, and edges off it are cut at a share drawn
  from `cut`; optionally every self-loop, and a few kept edges weighted
  (log-uniform `multiplier`). At least one map always keeps a choice.
- **The gate checks the walk**: `Reject::Xaos` when a map's walk share falls
  under `min_share`, or under its share of the weights where that is smaller
  — it is for what the graph takes away. Dimension under xaos is
  Mauldin–Williams (as `similarity_dimension` already was), and **it is
  steered**: no closed form, but it rises with a common scale factor, so
  `gate::steer_under_xaos` bisects for it. Cuts cost half a dimension or more,
  which is why `xaos-fun` turns a dimension bound on — without one a third of
  its rolls thinned to dust.
- **A mutation** of xaos cuts, restores or reweights one edge out of a map
  (never its last exit), or re-rolls the whole layer's graph — which may draw
  none, clearing it. On a zoom scene it never leaves a map that can't be
  followed by the zoom map, which would not load.

A profile saved before xaos was a part kept the base's rows exactly when the
transform count was off; read without an `xaos` key, it gets xaos off where
`count` is off, so it does the same (`profile::migrate`).

Not yet (FEATURE-PLANS.md Track A, item 5): infinite zoom as a part.

## The mutation grid

The Mutate window is look-and-choose (FEATURE-PLANS.md item 3): the scene in
the middle of a 3×3 or 5×5, its mutations around it, and a click makes one
the scene — one edit, labelled `Grid k: <what changed>` — and a new ring grows
around it at once. The model is `src/mutation_grid.rs`; the tiles are made on
the Random flames list's worker pool (`random_list::spawn_workers`, which now
takes a `dice::Action`), through the same `dice::mutate_seeded` U calls, so
each tile went through the gate against the scene in the middle. Thumbnails
are `gpu::thumbs` (`ThumbKey::Mutation`), and as everywhere only the tile
under the pointer flies its camera.

Four things it rests on:

- **Tile `k` is seed `base + k`, laid out by `mutation_grid::cell`**, which
  `--mutations 8|24` also uses: parent in the middle, the inner ring in
  reading order, then the outer one. So the window's grid *is*
  `--mutations 8 --seed <the seed it shows>` (the status line's tooltip prints
  the command), a 3×3 is a 5×5's inner ring tile for tile, and a 5×5's outer
  ring is made at twice the strength (`ring_strength`, capped at the limit).
  Any other `--mutations N` keeps the old near-square sheet.
- **A parent keeps its grid.** The base seed is remembered per parent, by a
  fingerprint of the scene with the camera left out, so after picking a tile
  Ctrl+Z brings back the old scene *and the same tiles*, and a sibling can be
  picked. This touches nothing in the history: the grid follows whatever is on
  screen, and a scene it has seen gets the seed it had. It looks again when
  the history's position or the count of opened files moves, after 400 ms of
  stillness so a drag doesn't restart it per frame; a pick restarts at once.
- **Tiles are framed by the live camera**, baked into the grid's copy of the
  parent when it starts, so the comparison is of form seen the way you were
  looking. Picking a tile puts the scene's own saved framing back. With the
  camera flying, the tiles hold the framing of the moment the grid started.
- **A thumbnail's ring scales with its area** (`thumbs::points_for`): 8k
  points up to the Random flames list's 256×144, then in proportion, to 64k.
  Tiles are drawn from a ladder of widths, so a resize draws them smaller
  rather than re-rendering 25 of them.

Not yet: an onion skin (the hovered tile over the parent in the main view),
masks beyond profiles (only the selected map, a mute/solo set), and moving the
grid into multiview panes once those exist.

## Threads: proposals between a person and an agent

Phase 2 of `AGENT-COLLAB-PLAN.md`: requests, proposals and responses about one
scene, kept as files under `renders/collab/<scene-stem>/` (gitignored, so a
thread never reaches the public repo). Either side can send any kind. An agent
takes part with nothing but the CLI; the person with the Collaborate window.
A Claude Code session can also be connected to the window live, and the window
can drive a backend itself; see "The agent connection" below.

```sh
fracturize --list-threads [--waiting]             # every thread, and what's waiting on whom
fracturize -s s.toml --thread [--since 0003]      # the thread, each option's -S diff included
fracturize -s s.toml --request "text"             # the scene file goes with it as base.toml
fracturize -s s.toml --propose dir/               # dir: message.toml, base.toml, options
fracturize -s s.toml --respond 4 --verdict "B" --pick B
fracturize -s s.toml --respond 4 --send-back B --note "thinner" [--tweak mine.toml]
fracturize -s s.toml --wait-reply --timeout 30m   # exit 0 a message, 3 timed out, 4 withdrawn
fracturize -s s.toml --withdraw 5 --reason "..."
```

`--from NAME` (else `$FRACTURIZE_AGENT`, else `agent`) says who you are; the
window signs with the author name in prefs, else `human`. Anything not from you
is for you. The list flag is `--list-threads` because `--threads` was already
the encoder's thread count. A proposal directory's `message.toml`:

```toml
replying_to = "0001-request"     # optional
revises = "0003-response/B"      # optional: what this is a new round of
message = "Thinner seams, keeping your rotation."
ask = "pick-one"                 # pick-one | pick-any | free

[[option]]
id = "B2"
scene = "B2.toml"                # plain file names beside message.toml
render = "B2.png"                # optional, and worth it: it's what the card shows
note = "seam scale 0.04 → 0.025"
```

Things it rests on:

- **A message is written once.** Each is a directory built under a temporary
  name and renamed into place (`thread::post`), so a reader never sees half of
  one and two racing writers both land, one number apart. Withdrawing adds a
  `withdrawn.toml`, written the same way; that's the only later change.
- **Preview sets the live scene aside rather than copying the option over
  it.** The scene, its whole `History` and its save point are swapped out
  (`app_collab::Preview`) and the option comes up with a fresh history. So
  previewing touches neither undo nor the dirty bit (`is_dirty` asks the
  set-aside history), and any edit made while previewing is a **tweak**:
  undoable inside the preview, kept when you leave, shown on the card, and
  sent with Send back. **Ctrl+S is refused while previewing** — it would write
  the option over the scene's file — and opening a scene leaves the preview
  first. Esc leaves it. *Hold to compare* shows the live scene while held; the
  cloud re-forms each way, since the two are different IFSs.
- **Apply is `scene_diff::merge`**, base → option laid over the live scene, so
  edits made since the request survive. Where both changed a value, a dialog
  lists yours beside theirs, defaulting to yours (as `--diff --base` does).
  One `commit_edit("Agent: …")`, so one Ctrl+Z, then a response saying
  "applied". The merge is of the live scene *as a save would write it*
  (`as_saved`), so the values that only reach the file on a save (point size,
  falloff, contrast, haze, exposure) conflict like any other: a haze you
  changed since the request is listed, not overwritten. An option that moves
  the framing moves the live camera too, or the next Ctrl+S, which writes the
  live framing, would quietly undo it. A tweak carries those values as well.
- **What goes out is what the person sees, except where they're standing**: a
  request's or proposal's scene carries those live values (`scene_to_send`)
  but keeps its own framing, because orbiting to look at a 3D scene is not a
  change to propose. "with this view" sends the live framing as the scene's
  camera, and "with a render" implies it — the `S` screenshot of the view,
  written beside the message (`App::screenshot_to`) before it's posted, is of
  a view, and the scene has to say which. The MCP `get_state` gives the scene
  with its own framing and the live one beside it, labelled as such.
- **Keep** copies an option (or its tweak) to a new `scenes/<thread>-<n><id>.toml`,
  never over a file: the one way a thread's scene reaches the project.

Not yet: a re-render of a tweak with the proposal's settings (a sent-back
tweak goes as a scene, without a picture); a thread picker for other scenes' threads (the window
shows the scene on screen's); the hold-to-blink *key* (a held button for now —
the key is Hazel's to choose); and tweaks surviving a restart.

## The agent connection: `fracturize mcp`, and the backends

Phases 3–5 of `AGENT-COLLAB-PLAN.md`. The thread (above) is the workflow; this
is what gets a message to an agent the moment it's sent, and back.

```
 Claude Code ──stdio MCP──▶ fracturize mcp ──JSON lines──▶ the window
   (a session)               (src/mcp.rs)    $XDG_RUNTIME_DIR/fracturize/gui.sock (src/bridge.rs)
 llama-server ◀──HTTP── the window's own loop (src/agent_local.rs)
```

```sh
claude mcp add --scope project fracturize -- fracturize mcp      # once
claude -n fracturize --dangerously-load-development-channels server:fracturize
```

Who answers is `[agent] backend` in prefs, picked in Agent settings (the
Collaborate window's header): `claude-channel` (default: a session you
started; nothing is spawned), `claude-headless` (the window runs `claude -p
--resume <id>` per message, with the same bridge), `local` (a model on
llama-server, started through with-llm's preset if nothing answers), or
`none`. The header's light says who's connected or working, with Stop.

Things it rests on:

- **The socket carries only what needs the live window**: `hello`,
  `get_state` (the scene as `scene_to_send` makes it), `get_render` (the `S`
  screenshot path), and three courtesies: `posted` (rescan now), `delivered`
  and `seen` (message states, kept in memory because a message is written
  once). Every thread operation the bridge does itself, on files, through
  `thread.rs`, like the CLI. So the MCP tools work with no window (only
  `get_state`/`get_render` say they need one), and the bridge `chdir`s to the
  window's working directory on `hello` so relative paths agree.
- **One bridge at a time.** A second `hello` is refused naming the first. The
  socket is taken over only if nobody answers on it (a window that died).
- **The push is a file watcher**, in the bridge: once a second it reads every
  thread and sends each new message for it (`for_me`: not from it, and `to`
  it, `agent`, or nobody) as `notifications/claude/channel`, meta keys being
  identifiers (`kind`, `thread`, `message`, `from`, `scene`). Messages already
  there at connect are not replayed, but what's waiting is said once.
  Channels need the interactive session; `-p` ignores the dev flag, which is
  why headless is told about the message in its prompt instead.
- **A new message kind, `note`**: text that wants no answer (progress, a
  report). It never waits and doesn't count as answering what it replies to,
  so a progress note leaves a request "waiting".
- **Dispatch** (`app_agent.rs`): with headless or local chosen and no session
  connected, each new message from the person (request, own proposal, a
  send-back) starts the backend, one at a time — only messages since the
  window opened or the backend was chosen. Options without a render get one
  from this binary (`mcp::render_option`, a small still in the window's
  renderer, splat without a window).
- **The local loop is `tools/llm-author/author.py`'s protocol and welfare
  rules, unchanged**, reading its prompts from there (plus `sendback.md` and
  `report.md`): each scene a card at once, `max_turns` replies, then an
  optional report as a note; DONE/STOP honoured on the spot, STOP never
  questioned; two repairs a turn; a full-budget reply without a scene ends
  it; a repeat asked about once. Transcripts in llm-author's format under the
  thread's `transcripts/`, so `review.sh` reads them. The conversation carries
  on into the next message in that thread until the window closes.
- **VRAM**: `with-llm --fits -m <preset>` is with-llm's own rule
  (weights + projector + 1200 MiB), shown in Agent settings; with-llm refuses
  to start one that doesn't fit and the window says why. The server is held
  up by `with-llm … -- cat` on a pipe, so closing the window stops it (if
  with-llm started it).

Not yet: the local backend's tools mode (the setting exists; whole-scene
runs), a re-render of a tweak, and a thread picker. The channel flag is a
research preview's and may change; headless is the way round it.

## The scene browser

The Scenes window is our own file picker, on purpose: a system dialog can't
show a fractal, and XFCE's default is GNOME's. Left: a folder tree, always
rooted at `~` (or `/` outside it) so it is one fixed map with where you are
highlighted in it — the branch to the shown folder held open, a folder opening
when you arrive in it. Rooting it at the folder you'd clicked was tried first
and read as Finder without Finder's sense of motion: every click a new root,
and no way back inside the tree. Under the tree, pinned, **places** — `scenes/`, `scenes/local-llm/` (where
`tools/llm-author` puts each run's final scene), and any folder added with
"Add this folder" (`prefs.browser_places`; right-click one to remove it).
Right: the folder's scenes, three lines a row — name, author, file and age —
beside a 16:9 thumbnail. Sort by name or newest. The folder you were in and
the sort persist to prefs. A render with a scene in it is listed too, marked
`PNG`; a `.render.toml` sidecar and an ordinary picture are not. A scene
that fails to parse is listed with its error rather than left out.

**Thumbnails are live renders, not cached PNGs** (`src/gpu/thumbs.rs`). Each
is a `PointCompute` with 256 walkers and an 8k ring, drawn by an ordinary
`SplatRenderer`, framed exactly as `--render` frames it (`offline::frame_setup`)
— the scene's own camera, never reframed, because that is authorial intent
and a scene with a poor opening shot should look like one. Measured on the
GTX 1080 at 96px: ~0.7 ms to build, converge and draw one, 0.06 ms per
re-draw, and parsing 0.1-1 ms a scene (9 ms for all of `scenes/`). 8k points
is where nearly every scene reads at that size, and 2k only the compact ones;
`cargo test --release thumbnail_study -- --ignored --nocapture` re-measures
and writes the thumbnails out to look at.

Three things make it cheap, and each is easy to undo by accident:

- **One compile.** `ChaosPipelines` and `SplatPipelines` hold the compiled
  shaders; `with_pipelines` shares them. Compiling `chaos.wgsl` is ~6 ms (230
  cold, first run in a process with the splat set), and every instance used to
  pay it — including the main view on every transform add, which now doesn't.
- **Converge once, then it's a still.** A burn-in submit and then exactly one
  lap. Two submits, not one dispatch: the ring is walker-major, so a dispatch
  writing more than a lap lets one walker's burn-in land on another's finished
  points. The chaos game has no fuse otherwise; the interactive ring hides the
  dust by turning over for thirteen seconds.
- **Only the row under the pointer moves**, flying its camera path (the
  default orbit if it authors none). Everything else stays still, which is a
  design rule rather than a budget: a list where everything moves is one
  nobody can look at.

Rows ask `ThumbCache::get` while egui lays them out; `App::render` builds
what was asked (4 ms budget a frame, always at least one) and registers the
textures before egui paints, so a thumbnail lands the frame after its row. The
scene's background colour stands in until then. A file's thumbnail rebuilds
when its modified time changes. Deferred: an author-set thumbnail view for
scenes whose opening shot is mid-flight; per-scene renderer choice (waits on
the renderer rework); reading folders off the UI thread, which 9 ms doesn't
yet need.

## Starting From Nothing

`--blank`, and File → New blank scene (Ctrl+N), give you `Scene::blank()`:
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
cutout. The live window is opaque unless launched with `--transparent-window`
(below). **Not supported for animation**: frames are converted to YUV from
r/g/b only, and neither AV1 nor H.264 carries an alpha plane here, so
`--transparent` with an `.avif` or `.mp4` output errors rather than quietly
producing opaque video.

**`--transparent-window`** makes the live view itself see-through: the desktop
shows wherever the fractal isn't, for composing it over other windows or
capturing it with OBS. Needs a compositing window manager (xfwm4's compositor
is on here); without one empty pixels come out black. The scene background is
not drawn. Three parts, all in the window pass only — screenshots and render
jobs from such a window still follow the `--transparent` checkbox:

- **`gpu::buffers::Alpha`** is what a pass does with alpha: `Opaque`,
  `Straight` (a file) or `Premultiplied` (this window). It is the
  `transparent` uniform's value, 0/1/2, in both point shaders and
  `SplatParams`; `CameraUniforms::with_alpha` and `upload_params` take it.
- **Premultiplied in sRGB code values, not linear light**
  (`premultiply_encoded`, in both shaders). An X11 compositor blends a
  32-bit window's bytes as `encoded + desktop·(1−a)`; multiplying in linear
  and letting the sRGB target encode the product gives `encode(c·a)`, far
  darker than `encode(c)·a` at low alpha, and dusty edges go to soot.
- **The surface prefers `PreMultiplied`, then `Inherit`**, and falls back to
  whatever is first. The GTX 1080 on X11 offers only `Opaque`, and presents
  the alpha anyway — verified by screenshotting the desktop with all three
  renderers (`import -window root`; capturing the app's own window would show
  nothing, and Xephyr has no compositor, so this one test runs on the real
  display — it sends no input, which is what the Xephyr rule protects).
  `RUST_LOG=info` logs the mode chosen. The T490 (Mesa) is untested.

The overlay composite and egui already blend premultiplied, so gizmos and
panels needed nothing; the panels stay opaque. The emphasis glow writes colour
only, which over a premultiplied zero reads as added light — glow on glass.

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
  `CameraUniforms.transparent` is how the shader knows which it's doing; a
  `--transparent-window` spends haze as transparency too, premultiplied.

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

**A render is a scene file too.** Every still carries its scene in a
`fracturize:scene` chunk (see "Render records"), so anything that takes a scene
path takes a `.png`: `-s render.png --info`, `-s render.png -r again.png`, the
in-app loader, a drop. `scene_source::read` is the one reader — it sniffs the
content, returns the TOML text and whether it came from a `.toml` or a PNG, and
reads a PNG only up to its pixels, so it is cheap enough to call on every file
in a folder. `file =` references resolve beside the PNG, and a miss says the
scene came from one. A scene opened from a PNG has no `.toml` behind it:
`scene_path` is `None`, `App::scene_png` remembers the picture, the title and
toolbar say "from" it, and Ctrl+S writes a new `scenes/<png-stem>.toml` (with a
timestamp if that exists) — the untitled rule, better named. `Scene::save`
refuses a PNG outright as a backstop.

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
# fov = 45                # optional: vertical field of view in DEGREES, 10-150
                          # (the one camera angle written in degrees, because
                          # it's the one people quote). Omit it — and almost
                          # every scene should — and the lens is the person's
                          # preference: `fov` in prefs.toml, set by the slider
                          # in the Controls window, defaulting to 45. State one
                          # only when a scene is *composed* against a
                          # particular lens; it then wins over that preference,
                          # and only `--fov` beats it. Saving from the app
                          # never writes this key and never removes it.
# rotvec = [x, y, z]      # optional: the exact framing, as a rotation vector
                          # in radians. Wins over yaw/pitch/roll, and is what
                          # gets written for a framing at the poles — where
                          # yaw and roll are the same control and the three
                          # angles stop naming it. Ordinary scenes never see
                          # this; it exists because the camera can now be
                          # pointed straight up.
# legacy: offset = [x,y,z] (eye displacement) still loads, but is folded
# into the framing/distance and never written back
# The camera's motion is its track: `[[track]] target = "camera"`, below with
# the others.

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

# Xaos: which map may follow which (see "Xaos"). Either per-transform rows —
#   [[transform]] ... xaos = { petal = 0, spike = 2.5, "*" = 1 }
# keyed by target name (or index as a string), "*" for every map not listed —
# or ONE strict matrix per layer, never both:
[xaos]
maps = ["whorl", "petal"]       # every map of the layer, once: rows and columns
matrix = [
  [1.0, 0.0],                   # from whorl: whorl yes, petal never
  [1.0, 2.0],                   # from petal
]

# Animation: any -S path over time (see "Tracks and the timeline"). Last in
# the file, because they name what's above.
#
# The camera's track comes first. Its keys are VIEWS AT MOMENTS: a framing —
# yaw/pitch/roll (radians) or `rotvec`, distance, focus, every one optional
# and inherited from [camera] — at `t` seconds. The camera is on each view at
# its time, and in between flies a cumulative quaternion Catmull-Rom (see
# src/path.rs): it splines framings rather than angles, distance in log space
# (constant-rate zooms), focus on its own spline. Uneven times are fine — the
# tangents are per second, so speed runs smoothly through a key.
[[track]]
target = "camera"
extrapolate = "repeat"    # what happens after the last key:
                          #   "hold"     stop there (the default)
                          #   "pingpong" fly back to the first key, and again
                          #   "repeat"   loop back to key 1 (seamless); needs
                          #   period = <s>, the lap back round to key 1, so the
                          #              way home is whatever the last key leaves
                          #   "offset"   close under the [zoom] symmetry, so it
                          #              loops as an endless zoom;
                          #              zoom_periods = N periods a lap
period = 14.0
# ease = true             # come to rest at the first and last key; default on
                          # for hold and pingpong, meaningless on the loops

# ROUTES. Which way round a segment goes is data, not re-derived from its
# endpoints — that is what used to make a 1° change swing 359° the wrong way,
# and what lets a path spiral down and back up through whole turns. Normally
# the yaw column says it: keys at 0, 3.14, 6.28 author a full turn, read once
# at load and kept, and saving writes the column chained, never wrapped. A key
# can also carry `turns = N` (extra whole turns on the segment leaving it),
# written only where the yaw column can't say it (a loop's way home, a key
# written as `rotvec`), or `route = [x, y, z]`, the journey as a rotation
# vector, for routes no integer names: between two equal framings no axis is
# implied, so "pitch three turns and come back" needs it. A route is checked
# on load — it must land on the next key within half a degree. Whatever the
# route says, the keys themselves are hit exactly.
#
# EVERY SCENE HAS A CAMERA TRACK. Without one the camera flies the default:
# a seamless orbit of the current framing at 0.18 rad/s, four keys a quarter
# turn apart on a repeat — the "turntable". It draws, plays and renders
# through the same code (`path::resolve`, used by `App::camera_path` and
# src/offline.rs). Editing it is what makes it scene data; until then Ctrl+S
# writes no camera track. Paths are drawn in the viewport with gizmos on (G),
# except while playing, when the camera is standing on the line.
[[track.keys]]
t = 0.0
yaw = 0.0
pitch = 0.9
distance = 5.5

[[track.keys]]
t = 6.0
yaw = 3.14                # half a turn later...
pitch = 0.1
distance = 1.6            # ...swooped in close
focus = [0.0, -0.5, 0.0]  # looking lower
roll = 0.4                # ...and tilted

[[track]]
target = "transform.whorl.rotation.y"   # an -S path; not camera.* (the camera track is)
keys = [[0.0, 0.0], [4.0, 90.0]]        # [seconds, value]; strings/bools step
interpolate = "smooth"                  # step | linear | smooth | spline
extrapolate = "pingpong"                # hold | repeat | pingpong | offset
# rate = 0.05      # ...or units a second from the authored value (or `from`)
# wave = "sine"    # ...or a cycle: sine | triangle | saw | square,
# period = 8.0     #    period seconds, amplitude either side of the
# amplitude = 15.0 #    authored value (or `centre`), phase in periods
# mute = true      # kept, drawn, not applied

# A map's WHOLE rotation keys poses, not one Euler angle, and turns between
# them as a rotation (the camera's spline and routes): no wrong way round, no
# gimbal lock, no shear. Change one angle from the key before and it turns by
# exactly that much about that angle's axis — y 0 → 720 is two whole turns;
# otherwise the short way, or say `turns = N` / `route = [x, y, z]`. Linear is
# a constant rate, smooth eases each key, spline flies through all of them;
# `offset` keeps turning. Applied as `rotvec`, so no Euler branch can flip it.
[[track]]
target = "transform.petal.rotation"     # or subscene.<name>.rotation
interpolate = "linear"
extrapolate = "offset"                  # a quarter turn every 2 s, forever
[[track.keys]]
t = 0.0
rotation = [0.0, 0.0, 0.0]              # degrees, XYZ; or rotvec = [...] (radians)
[[track.keys]]
t = 2.0
rotation = [0.0, 90.0, 0.0]

# Composition: another attractor beside this one (see "Subscenes"). Each keeps
# its own walkers, so their maps never mix. Repeatable, up to 15.
[[subscene]]
name = "smoke"            # how -S subscene.<name>… and --info refer to it
                          # (unnamed: the file's stem)
file = "dragon_smoke.toml"  # borrow another scene's maps and colour, by
                          # reference, relative to this file — or give the
                          # block its own [[subscene.transform]]s instead
share = 1.0               # relative claim on the point budget ([meta] share
                          # is the scene's own maps' claim)
visible = true
translation = [0.0, 1.0, 0.0]   # placement: a similarity, so...
rotation = [0.0, 0.0, -8.0]     # XYZ degrees (or rotvec = [...])
scale = 0.8                     # ...uniform only
# color_mode / color_speed / color_falloff / color_contrast, and a
# [subscene.palette] table, override the colour the maps came with.

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
color = [1.0, 0.2, 0.2]        # contributes to the cyclic colormap. Optional:
                               # omitted, maps take hues spaced by index
                               # and stay omitted on save (see "Colour")
                               # (palette mode never renders it anyway)
weight = 1.0                   # selection probability
color_value = 0.25             # optional explicit colormap index (0-1)
color_speed = 0.5              # optional per-transform override (wins over color_falloff)
# Nonlinear variation blend; omit for classic affine ({ linear = 1.0 })
variations = { swirl = 0.35, linear = 0.65 }
# Optional post-affine, applied after the variation blend (and before any
# symmetry group). Omit all three for identity. Same T·R·S order as above.
post_translation = [0.0, 0.0, 0.0]
post_scale = 1.0               # uniform or per-axis, like `scale`
post_rotation = [0, 0, 0]      # Euler degrees (XYZ); no rotvec form
```

The post-affine is what lets a variation act about a point other than the
origin: the affine carries that point to the origin, the variation acts, and
the post-affine carries it back. `scenes/flame_of_flames_opus55.toml` centres
`swirl` on each lick's root this way.

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
  (stellate shipped with 0.0025 and needed 0.0012). That bound is written for
  the default 45° lens; a point holds its world size as the FoV changes, so a
  wider one shrinks it on screen and the bound relaxes by
  `tan(22.5°)/tan(fov/2)` (0.41 at 90°). The Render window's `≈N px` readout
  is always the truth of it.
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

## Subscenes

A scene can **compose** other IFS attractors: `[[subscene]]` blocks, each its
own set of maps with its own colour, placed in the scene by a similarity. The
model and the reasoning are in `src/subscene.rs`; `scenes/censer.toml` is the
worked example (a Menger sponge with dragon smoke rising off it).

**A composite is the union of attractors, not one bigger IFS.** Concatenating
two map lists gives a third attractor that mixes them — a walker that takes a
fern map and then a lattice map lands somewhere neither contains. So each
subscene is a *layer* of the one chaos pass: every map carries
`TransformSpec::layer` (0 = the scene's own, `k` = `subscenes[k-1]`), the 16,384
walkers are dealt between the visible layers by `share`, and a walker only ever
draws its own layer's maps.

- **The point budget divides exactly as the walkers do.** Walkers write a fixed
  stride of the ring, so a share is a fraction of the buffer, not a hint.
  `subscene::allocate_walkers` is largest-remainder with a floor of one, and a
  hidden layer, or one whose maps are all disabled, gets none — its points go
  to the rest. Under the log tonemap share is a gentle brightness lever (0.3 →
  1.0 moved the smoke's mean luminance 64 → 76 on `censer`); it is mainly a
  density and grain lever.
- **Placement is applied as a point is written, never to the walker**, so a
  subscene moves, turns and resizes without re-deriving one of its maps.
  **Uniform scale only**: a similarity composed with a TRS map is TRS, so every
  map inside a placed subscene still decomposes, saves and drags. Per-axis
  placement would introduce shear the file can't write.
- **Colour is resolved in the chaos pass for a composite.** A point stores an
  8-bit colormap index, which can't also say *which layer's* colormap; so a
  composite looks the index up in its layer's colormap (with that layer's
  contrast) as it writes, and stores packed RGB — what `mix` mode already
  stores. Every renderer then draws a composite as a mix-mode scene
  (`Scene::packs_rgb`) and none of them changed. The price: a colour edit in a
  composite re-forms the cloud instead of recolouring it in place.
- **An ordinary scene is one layer with every map and every walker, and renders
  byte-identically to before.** `ComputeParams::composite == 0` skips placement
  and colour resolution, and one layer's cumulative weights are the old ones.
  Verified on four scenes × both renderers against a pre-change binary, and
  `--info` is unchanged (the golden files still pass).

**In the file.** `file = "other.toml"` borrows another scene's maps, symmetry
and colour by reference (its camera, zoom, haze and the rest are ignored);
otherwise the block carries its own `[[subscene.transform]]` and
`[[subscene.symmetry]]`. `[meta] share` / `visible` are the scene's own maps'
equivalents. A pure composite needs no `[[transform]]` of its own.

**Saving** keeps an unedited import a reference — four lines, carrying only the
colour keys that differ from the file's — and writes an *edited* one inline
with `source = "..."` recording where it came from. The referenced file is
someone's artwork and is never written to (`subscene::FileBase` is the
fingerprint that tells the two apart). References are respelled relative to
wherever the composite is saved, so Save-As into another directory keeps them
pointing at the same scene. `[[subscene]]` blocks are rebuilt wholesale on save,
like `[[symmetry]]`, so comments *inside* one are lost.

**Refused, by name, at load:** `file =` plus inline maps; a subscene with no
maps; two subscenes with one name; a non-positive scale; a subscene that is
itself a composite (nesting — deferred); `[zoom]` together with subscenes
(infinite zoom renormalizes about one map's fixed point, which knows nothing of
a placement — deferred); more than 15; every layer hidden.
`SubsceneDef` is `deny_unknown_fields`, so a typo'd key is an error too.

**CLI, for authoring by agent:**

- `--info` gains a `layers` section: per layer its share and what that comes to
  in points, placement, colour, provenance and credit, measured centre and
  radius *once placed*, similarity dimension, and **density** (points per unit
  area, relative) — plus how each pair sits (apart, overlapping by N%). Notes
  flag layers so far apart that framing both makes each small, and a density
  imbalance over 4x, with the `-S …share=` values that would even it. `shape`
  measures the union of the visible layers (`trace::measure_layers`), and
  dimension and lacunarity move to per layer, because two attractors' maps have
  no joint dimension.
- `-S subscene.<name>.<field>` sets placement, share, visibility or colour, and
  `-S subscene.<name>.transform.<ref>.<field>` reaches an inline subscene's
  maps — so `--sweep` works on subscenes too. A subscene is named by its `name`,
  or its file's stem, or its index. A borrowed subscene's maps aren't in this
  file and `-S` says so rather than doing nothing.
- `--solo <name|index>` draws one layer alone (with every point), placed where
  the composite puts it. Works with `--info`.

**In the app**: the Subscenes window (toolbar, next to Transforms) lists the
layers. Clicking a subscene's row selects it **as a whole**: the viewport then
shows one gizmo — its placement — and dragging it moves, turns and resizes the
whole attractor (tips resize uniformly; alt+scroll over it sets its share). The
Transforms window lists one layer at a time (a picker at the top of its rail),
and a map's gizmo is drawn and dragged in scene space while its inspector
fields stay in its subscene's own space. Import picks a scene file and adds it
as a reference with an equal share. Show/hide, share, placement, colour source,
rename, import and remove are all ordinary undoable edits.

Things that interact with it:

- **The drawn-point budget is bypassed** for a composite: it draws a *prefix* of
  the ring, and each stretch of the ring belongs to one layer's walkers.
- **Traces** walk one layer each, dealt by share, placed, coloured through their
  own layer's colormap.
- **The status bar's `d` and `Λ`** describe the layer the Transforms window is
  listing.
- **Emphasis** maps the selected transform to its GPU slot (the buffer is in
  layer order); a placed layer scales the sensitivity by its placement.
- **The layer table is one fact in two languages** — `GpuLayer` (96 bytes) and
  `Layer` in `points/chaos.wgsl`, with `MAX_LAYERS` = 16 in both;
  `the_shader_agrees_about_the_layer_table` guards it.

Deferred on purpose: nesting (a composite as a subscene); infinite zoom in a
composite; editing a subscene's palette stops in the GUI (its colour *source* is
editable; the gradient is edited in its file or `[subscene.palette]`); and
per-axis placement, for the shear reason above.

## Xaos

**Which map may follow which.** Each map carries a row of multipliers, and the
step after map `i` draws `j` with odds `wⱼ·xᵢⱼ` — 1 as usual, 0 never, 3 three
times as likely. The walk becomes a Markov chain and the scene a
**graph-directed IFS**. `XAOS-PLAN.md` is the design, `src/xaos.rs` the model,
`scenes/parasol.toml` the worked example (two cuts keep a defect at the heart
and arch the petals into domes).

Four facts drive every view of it, and are worth teaching anyone who uses it:

1. **Only zeros change the shape.** A cut removes every point whose address
   has that pair in it; a positive multiplier only moves density, like a weight.
   `parasol`'s sweep shows it: at 1, 0.3 and 0.1 the silhouette is identical.
2. **Weights stop meaning "how often".** The chain's stationary distribution —
   the **walk share** — does. A weight-1 map can be starved to nothing, and a
   cut can make a class absorbing and drain everything else (cut `spine`'s two
   other exits in `parasol` and the whole picture collapses onto the spine).
   So walk share sits beside weight share in `--info`, the inspector, the
   rail's weight bars (a tick) and the matrix window.
3. **Dimension changes formula** to Mauldin–Williams — the `d` where the
   spectral radius of `[i→j allowed]·Σ(sⱼe)ᵈ` is 1 — over the maps the walk
   actually visits. `--info` prints it beside the dimension without the cuts.
4. **Xaos stays inside a layer.** A subscene's walkers never draw another
   layer's maps; a key naming one is a load error saying so.

**The model.** A row is a `rest` (`"*"`, default 1) plus the exceptions that
differ from it, keyed by the target's position *within its layer*
(`TransformSpec::xaos: xaos::Xaos`). Never N², so "no self-loops" on a 40k-map
L-system port is 40k one-entry rows. Structural edits go through
`Scene::insert_transform` / `remove_transform`, the one place a map enters or
leaves a scene, which keep every row pointing at the maps it pointed at
(duplicate copies the row *and* the column: a second copy of the same map).

**Sampling is exact, and an ordinary scene is byte-identical.** The GPU draws
from a row in two binary searches over the layer's own cumulative weights
(`select_xaos` in `chaos.wgsl`, transcribed line for line as `xaos::sample` for
the CPU walkers — keep them in step), with the walker's last map in what was
`WalkerState`'s padding and each map's row offset in `GpuTransform`'s. One
builder (`xaos::build_rows`) feeds both. A layer with no row takes the old
path. Proven two ways against a baseline binary: ordinary scenes in both
renderers, and with `FRACTURIZE_XAOS_FORCE=1` — every map through the new
sampler with all rows at 1 — also byte-identical. A cut is *never* drawn, not
almost never: trailing zeros are pinned at 1.0 and a rounding guard steps out
of squeezed intervals (`a_forced_default_row_draws_exactly_the_plain_map`,
`the_sampler_draws_the_formula`).

**Edges of the model.** The first step and the step after a re-seed draw from
the plain weights. A **dead end** (a row with nowhere to go) restarts from the
plain weights and is *reported*, never refused — random rolls and live edits
reach one constantly. Symmetry is untouched: the group element is still drawn
independently after the map. **Infinite zoom** needs every map that fires to
allow the zoom map next, and a scene that breaks that is a load error naming
the edge (live: `zoom_error`). The dimension lock stands down under xaos —
no single factor balances a spectral radius — and its button says so.

**CLI, for agents:**

- `-S transform.a.xaos.b=0`, `xaos.*=0` (quote it), `xaos={}` (reset), and
  `xaos_p.b=0.4`, which solves for the multiplier that gives that probability
  with the rest of the row held (`=1` is "only this exit"). The same spellings
  rewrite the cell in a matrix-form scene. `--sweep transform.a.xaos.b=1,0.1,0`
  morphs between sub-attractors.
- `--xaos no-self-loops|cycle|blocks:N|reset` expands into whole-row `-S`
  edits *ahead of* your own, so a `-S` beside it edits the preset's rows. The
  rows are printed to stderr, ready to paste.
- `--info` gains an `xaos` section (only when a row is authored, so no golden
  moved): per map weight share against walk share and edges in/out; dimension
  with and without the cuts; for layers up to 12 maps a multiplier grid and a
  probability grid. Notes, each with the flag that fixes it: dead ends, drained
  maps, starved ones, a chain split into closed groups, slow mixing.
- `--isolate 'from>to'` draws only the points `to` placed straight after
  `from` — one edge's own material, the first thing a cut there removes.
  `--edge-grid` is the n×n sheet of every edge (up to 6 maps), cut ones empty.

**In the app.** Three views of one graph, every edit through the same `App`
methods (`set_xaos_edge(s)`, `set_xaos_rest`, `set_xaos_probability`,
`apply_xaos_preset`, `insert_linked_transform`), coalesced per edge:

- the inspector's **Xaos block** under Behaviour: walk share beside weight
  share, a goes-to/comes-from radio (Apophysis 7X's), a row per target with
  multiplier, probability and cut/restore (past 12 maps: exceptions and a
  filter), the rest of the row, and Reset / No self-loop / Only to… /
  **Linked map** — a new identity map that runs straight after this one and
  only there, Apophysis's pre-final-transform trick;
- the **Xaos window**, the whole layer as a grid;
- the **viewport overlay** (the toggle beside the emphasis bulb, edit mode
  only): the selected map's exits as arrows, width by probability, cuts as
  stubs with an ×; with nothing selected, every edge that isn't 1 in a layer of
  up to 12. `ui::xaos::overlay_edges` is the one geometry the painting and the
  picking both use. Viewport input reaches it through `app.rs`, like the gizmos.

Random flames and mutation draw xaos too (see "The dice"). **Not done yet**
(plan §8): coupling between subscenes; "hold shares" (re-solve weights so a xaos edit
leaves walk shares put). And a GUI trap worth knowing: **U+2192 `→` is in
none of the app's text fonts** and renders as tofu; strings use Phosphor's
arrow-right (`\u{E06C}`, `icons::ARROW`).

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

**Implied colours stay implied.** A transform whose file leaves out `color`
(or `color_value`) takes the one its position gives, and saving no longer
writes that value down: it is left out again, and re-spaced when a map is
added or removed beside it (`Scene::implied_colours`). So a scene that never
chose its colours keeps not choosing them, a small model can write a valid
scene without inventing RGB triples, and a diff of adding a transform is one
line. Implied needs both halves: the file left it out *and* the value is still
the positional one, so any edit makes it authored without anything having to
remember to say so, and an authored `color_value = 0.5` that happens to sit in
the middle stays authored. A fresh map (Shift+A) in a layer whose colours are
all implied is implied too; otherwise it is the usual pale grey. Subscene maps
are always written out, as before.

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

**Zoom into this** stays on a transform's row and gizmo context menus, and in
the inspector's action row, as shortcuts for the map you already have selected.

From the CLI, on any scene, without editing it:

```
# make an existing scene scale-invariant and look at it
fracturize --scene scenes/sierpinski.toml --zoom 0

# tune the band; --zoom-levels / --zoom-radius / --zoom-guard / --zoom-falloff
# all need --zoom
fracturize --scene scenes/lsys_kelp.toml --zoom trunk --zoom-levels 16 \
  --render /tmp/t.png --effort small --width 640 --height 400

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
approximately, because the scene has a symmetry. A camera track with
`extrapolate = "offset"` closes under that symmetry instead of by returning to
the first key: one lap descends `zoom_periods` zoom periods (default 1) and
ends on a frame **identical** to the one it
started, since scaling by `sᴺ` about the fixed point and turning by the map's
rotation leaves the rendered set unchanged. Played on a loop, a fourteen-second
file falls forever.

```toml
[[track]]
target = "camera"
extrapolate = "offset"   # the zoom loop
period = 14.0

[[track.keys]]           # one key is enough — and is better than two
t = 0.0
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

- The zoom loop needs a `[zoom]` map, and says so if there isn't one.
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

A camera key whose `distance` is many periods below the first authors a
descent of exactly that many periods — distance interpolates in log space, so
that's a constant-rate zoom, and the per-frame wrap folds it back into one
period however deep it goes. `scenes/wellspiral.toml` descends nine:

```toml
[[track.keys]]
t = 0.0
distance = 3.6
[[track.keys]]
t = 14.0
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

## Tracks and the timeline

FEATURE-PLANS.md items 9, 10 and a first cut of 8. A **track** is an `-S`
path, a source of values over time, and what happens past its ends
(`src/track.rs` has the model; `scenes/parasol_tide_opus55.toml` is the worked
example: Parasol's cut xaos edge opening and closing on a camera loop).

- **Sources**: `keys` (`[[t, v], ...]`, interpolated step / linear / smooth /
  spline; smooth is the default and eases every key, spline is non-uniform
  Catmull-Rom through them), `rate` (units a second) and a cycle (`wave`,
  `period`, `amplitude`, `phase`). **Rates and cycles move about the authored
  value** (or `from` / `centre`), so editing the base in the inspector moves
  the whole motion; keys *are* the value. String and boolean keys
  (`palette.name`, `palette.reverse`) step.
- **Extrapolation** past the last key: `hold` (default), `repeat`, `pingpong`,
  `offset` (each lap starts where the last ended, Blender's cycles with
  offset). **The camera is a track** (`target = "camera"`, see "Scene Files"):
  its keys are views at moments and its loop is one of these, the zoom loop
  being `offset` whose offset is the zoom symmetry. Only its interpolation is
  its own — framings are rotations, interpolated by `path.rs`'s cumulative
  quaternion spline with routes, so a path can spiral through whole turns.
  `CameraPath::sample` folds time by the same rule
  (`the_camera_folds_time_as_tracks_do` holds the two together). It is held
  as `Scene::camera_path`, not in `Scene::tracks`, because it is the one
  track every scene has (the default orbit) and it drives the view rather
  than an `-S` path; it is written first among the `[[track]]`s.
- **One clock, in seconds.** A camera key at `t` is passed at `t`.
  The **timeline's length** (`track::timeline_length`) is the scene's own
  path or its tracks, whichever runs longer; a scene with tracks and no path
  runs as long as its tracks while the default orbit keeps turning through
  them. An animation render plays exactly that; `--seconds` stretches it to
  fit, as it always did a path. It loops (drops its last frame) only if every
  track comes back too (`track::loops_over`).

**The scene at time t is the scene with one `-S` per track applied**
(`Animator`): the TOML without its `[[track]]`s, overrides applied to a
cached `toml_edit` document, parsed. ~0.08 ms a frame, so the window does it
every frame it plays. One address language for the CLI, sweeps and
animation, and anything `-S` reaches a track can animate.

**Classes** (`track::Class`) say what a frame costs: *view* (point size,
exposure, haze, background: re-render), *colour* (palette, transform colours:
recolour), *placement* (subscene placement) and *structure* (everything that
moves the attractor). Past view, an animation **refills the point buffer every
frame** — in place (`offline::refill_points`), same seed, so the grain is
coherent from frame to frame — and the window churns its ring at the fill rate
while one plays.

Things it rests on, each found the hard way:

- **The animated scene is never `App::scene`.** That is the document — what
  the inspector edits, history snapshots and Ctrl+S writes — and an animated
  value in it would be saved as the authored one. `App::refresh_animation`
  builds the frame aside, swaps it in only for the GPU upload (so the gizmos
  follow it), and holds view values in `App::anim_view`, swapped in only
  around the frame's drawing (`App::render`). Keyed off the history position,
  so a paused timeline costs nothing.
- **A track that doesn't resolve is reported, never refused**, like an xaos
  dead end: delete the map a track moves and it points at nothing, and a scene
  the app saved must open again. `Animator` skips it and says why — `--info`
  notes, stderr from a render, the Timeline. Renaming a map renames it in its
  tracks (`track::rename_map`). Refused at load are only malformed tracks:
  camera targets (the path is the camera's track), two tracks on one target,
  interpolated strings, mixed sources.
- **One Euler angle is only meaningful in its triple.** A track on
  `transform.x.rotation.y` sets one angle of the triple the scene *writes*,
  and a saved scene respells a rotation with |y| > 90° on the other branch;
  the animator warns when that is in play. **Animate a map's turn with its
  whole rotation instead** (`transform.x.rotation`, the picker's "whole
  (poses)" leaf): its keys are poses (`KeyValue::Rotation`, `track::RotKey`),
  interpolated as rotations along routes read from the Euler column
  (`RotKey::route_to`: one angle changing is that angle's axis, by exactly that
  much), and each frame sets `rotvec`, which no save respells. A whole-rotation
  track beside one of its own angles is refused, since the whole would win.
- **The transport is a clock plus "is the camera following".** `path_t` is
  gone. Grabbing the camera stops it following and, in a scene with tracks,
  leaves the clock running: you can orbit an animation while it plays. Play
  resumes from the playhead; from the end of a one-shot it starts over. The
  default orbit carries its own epoch, so it always starts from where you are
  and is carried across a timeline loop rather than snapping back.
- **Ordinary scenes are byte-identical**: stills in both renderers,
  `--path-t`, and animations including zoom and ping-pong loops, against the
  build before tracks.

**CLI, for agents:**

- `--time T` — the frame at T seconds, for a still, a sheet or `--info`
  (whose `tracks` section then lists the `-S` flags that moment comes to —
  the frame said as a command). A still of a scene with tracks without
  `--time` is t = 0, and says so.
- `--time-grid 4x3` — a sheet of moments across the timeline, each tile its
  time's `-S` flags and framed where the camera is then: how to read an
  animation without watching it.
- `--info` gains a `tracks` section (only for a scene with tracks, so no
  golden moved): each track's target, source and class, the timeline's
  length, what an animation costs, and notes for skipped or constant tracks.
- `.avif`/`.mp4` render the timeline; the summary line says the class and
  whether it loops.

**In the app**: the Timeline window (toolbar, film strip).

- **The transport is a clock, and the camera is attached to it or not.** The
  camera row's link attaches it (it flies its path on the clock, playing and
  scrubbing) or detaches it (it stays where you put it while the tracks play).
  Taking the camera in the viewport detaches it. O/Z/Space and the toolbar's
  Play *fly the camera*: attach and play. The Timeline's own Play runs the
  clock and leaves the camera as it is. Attaching isn't an edit.
- **Only the ruler scrubs.** A press on a row selects it and moves nothing
  else, so picking a track never shifts the camera.
- **People never type `-S` paths.** "Animate a parameter…" opens
  `ui::param_picker` over `params::catalogue`: the scene's parameters as a
  tree (Scene › Haze, Transforms › petal › Rotation › y, Subscenes › smoke ›
  Share…) to browse with the mouse, or search by any words in any order, each
  row showing its current value; already-animated ones greyed with the reason.
  Every listed path is tried on the scene's own text first, so the tree never
  offers something `-S` would refuse. Rows and the detail header use the same
  names; the path is only in tooltips. The picker is generic — it returns a
  `Param` and the caller decides what choosing means — and is there for any
  other "point at a value in this scene" need.
- **Values are clamped to what the scene can take** (`params::range_of`): a
  spline overshoots and a cycle swings past zero, and the loader refuses a
  negative xaos multiplier, share or subscene scale and draws nonsense from a
  negative weight or point size. What still can't be used (a step key naming a
  mode that doesn't exist) **holds the last good frame** and is reported **once
  per stretch, naming the track** — the value changes every frame, so a message
  carrying it was a new message, and a new log line, every frame. Offline
  renders do the same, on stderr.

Select a row, then edit it in the column on the right; drag a diamond to retime its key (the ruler holds still
while you do — it pages by the playhead and its length follows the last key,
and the two fed back into the drag); "Key at playhead" adds a key holding the
curve's value there; a new track on any `-S` path starts with one key at the
playhead holding the scene's value, so it changes nothing until keyed again.
All of those, and mute and delete, are undoable edits; scrubbing is view
state, like orbiting. **No auto-key**: editing a tracked value in the
inspector edits the scene's own value, which a rate or cycle moves about and
keys override — key values are edited in the Timeline. A Render job still renders the
frame at the playhead; an animation's length defaults to the timeline's, and
its estimate charges a refill per frame when the attractor moves.

**Not yet, on purpose** (the model first, FEATURE-PLANS order 9 → 8 → 10):
keying from the inspector (and with it the auto-key question, which is
Hazel's; see `TIMELINE-PLAN.md`), zooming the ruler, the curve editor proper,
spin/turn rate tracks and post-rotation poses (TIMELINE-PLAN §3 C), walkers carried between frames instead of a refill, motion blur, an
`--key`/`--plot` CLI, and tracks in random rolls and mutations.

## Two machines, one repo: how to not make a mess

Hazel works on a desktop (GTX 1080) and a T490, and agents work in the desktop
checkout **at the same time she does**.

**Hazel wants a fairly linear history and does not use branches much.** The
agent-created branches in this repo are artefacts of prompting for two things
at once, not a workflow anyone chose. So the default is: **commit to `main`**.
Branch only when work genuinely runs in parallel with something else, and merge
it back as soon as it lands rather than letting it live.

Two things that have already bitten:

**The working tree can change under you.** If files you committed appear to
have reverted, check `git log --oneline -1` and `git branch -vv` *before*
concluding anything was lost — a checkout of another branch makes your own
commits look rolled back, because that branch legitimately predates them. This
happened: `main` had absorbed the feature branch at one commit and taken laptop
work on top, so the tree showed that commit's version of every file. Nothing
was lost, and the commit was safe throughout because it had been pushed.

**Push agent work when it lands.** Not for an audience — the GitHub repo is
public but has no users yet, and Hazel has left it unpushed for long stretches
because it is only doing cross-device sync. The reason here is narrower: a
commit that exists only in the desktop checkout is invisible to the laptop and
to every other session, and it is the only copy. Pushing is what made today's
apparent revert a non-event. Cheap, so take it — but it is sync, not release.

If a branch does exist, keep it a *superset* of `main` — merge `main` into it
whenever `main` moves, so the merge back is a fast-forward and never conflicts.
`git log --oneline <branch>..main | wc -l` returning zero means exactly that.
In practice the file sets barely overlap anyway: laptop work is scenes, notes
and README; agent work is `src/`, `shaders/`, `tools/` and docs.

Note `renders/` is gitignored, so survey sheets and test renders never need
cleaning up — but the repo *root* is not, and `-r out.png` from the root leaves
`out.png`, `out.fgrade` and `out.render.toml` behind. Render into `renders/`.

## Render Jobs

`P`, and the Camera window's "Render job…", open the dialog
(`src/ui/render_job.rs`). There is no separate one-click "HQ render" — that
button and its keybind were folded into this dialog, which does the same job
with its parameters visible, an estimate, and a way to stop it. The model is
`src/render_job.rs`; the work runs on a second wgpu device, so the app stays at
full framerate while a job goes (measured: 119 FPS with a 240-frame animation
rendering).

- **Three stages, one window**: setup, running, finished. Finished is the
  running view with the bar replaced by the result, so the log stays where it
  was being read; **New render** goes back to the same settings with a fresh
  filename. It used to drop straight back to the form with a small notice
  above Start, which put "did it work, and where is it?" among the next job's
  settings. The app keeps the ended job whole (`render_job::FinishedJob`:
  result, params, working time, log, `seen`), because the dialog is often
  closed while a job runs; until the result has been shown, the status bar
  says "render done" (or stopped early, or failed), and clicking it opens it.
- **The result opens with one click.** The result panel is a button: it opens
  the file in the system's own viewer, through the `open` crate (ShellExecute
  on Windows, `open` on macOS, xdg-open with gio/kde fallbacks and WSL on
  Linux), detached. Beside it, Open and Show folder (the folder, not the file
  selected in it: that needs a different call on every desktop).
- **The setup form is in sections**, each a house table (`ui::table`) under a
  folding heading (`ui::section`, shared with Random settings), ordered by the
  questions people ask: Output (kind, filename, 16-bit and transparency, which
  describe the file), Size (with the aspect ratio, and how it compares with
  the window: a render keeps the window's *vertical* view, so a wider shape
  shows more at the sides; "the window's shape" is in the common sizes),
  Quality (renderer, samples, points, density estimation, then supersampling
  and its filter), Animation, then the two that aren't chosen here: **Look**
  (exposure, the inherited grade; read from the window each time the dialog
  opens) and **This machine** (GPU share, encoder threads; remembered in
  prefs). The renderer is in Quality, not Look, because it decides what the
  sample choice under it can be. A preset bar is pinned above and the cost
  estimate and Start below, so the cost sits beside the button that commits
  to it however far the form is scrolled.

  **Toggling a control never moves the form.** Nothing appears or vanishes
  with a choice: the line under Quality is laps and histogram when
  accumulating and the samples/px a pass comes to when not, Animation is
  there for a still too (greyed), the grade line doesn't depend on the
  renderer. **And what switches what is on the page**, not only on hover: a
  row that depends on the one above is indented under it, and a row that
  can't be used shows what it needs ("needs Samples: accumulate") in place of
  its control, naming the row that would free it. Switch rows, whose column
  is too narrow for that, carry the note beside their name.
- **Presets** (`src/render_preset.rs`) name a job's size and quality, as
  Apophysis's `Render presets.av` does: output kind, size, samples (accumulate
  to `spp`, or one pass with `points`/`rounds`), supersample, filter and
  radius, density estimation, bit depth, and a clip's fps and quality. Never
  the look and never the machine: a preset that switched a splat piece to
  points would change the picture under the name of rendering it bigger.
  Every key is optional and one left out leaves the form's value. Built-ins
  are compiled from `src/render_presets/`, cheapest first: `quick-look`,
  `preview` (the dialog's long-standing defaults, and the first one used),
  `hd`, `4k`, `4k-fine`, `print-a3`, `print-a2`, `clip-preview`, `clip-hd`;
  their descriptions give sizes and densities, never durations. The person's
  own are `~/.config/fracturize/render-presets/<name>.toml`. The picker groups
  Built in / Yours with descriptions on hover; the name carries a `*` while
  the settings differ from what the preset says (only the keys it says: a
  built-in leaves points to the dialog); Save, Save as, Revert, and Delete
  (armed, `danger_button`). The last preset chosen is remembered in prefs
  (`render_preset`) and the session's first opening applies it; after that the
  form keeps its settings between openings. `--render-preset` is the same
  thing on the command line (see "View Files & Offline Rendering").
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
  already chunked and the job's device is its own. Stop is a
  `danger_button`, and keeps the work (below).
- **Cancelling keeps the work.** The chaos game is an *anytime* algorithm: a
  buffer stopped at 60% is the same picture as one stopped at 100%, just
  noisier. So stopping a job renders and saves what it has, and every render
  entry point returns `render_job::Outcome` — `Complete` or `Partial` — which
  rides on `JobEvent::Done` and into `App::job_done`. It used to return
  `Err(CANCELLED)` from `fill_points` and propagate through `render()`'s `?`,
  so stopping at 99% got you nothing at all.

  `Outcome` is an enum rather than a bool or a second string sentinel because
  **every display site must handle both arms**: a partial render gets the
  warning colour, its own heading, no check mark, and no frame count (the one
  the job asked for is not the one it produced). A noisier-than-asked-for file
  presented in the green "Render done" panel is indistinguishable, a week
  later, from the render you meant to make.

  Two paths deliberately differ. An **animation cancelled during the fill**
  writes nothing and still returns `Err(CANCELLED)`: a sparse cloud drawn
  across every frame is a full-cost job at reduced quality, not a partial
  result. Cancelled during the *frame* loop it muxes what has been encoded — a
  shorter clip is a real partial result — provided at least two frames landed,
  since the sample table cannot describe a clip with fewer.
- **Threads are a machine setting.** One job-scoped value (`JobParams.threads`),
  defaulting to `render_job::default_threads()` = one less than the machine
  has, exposed as `--threads N` and as This machine's encoder threads in the
  dialog. Both encoders read *that* value; they used to call
  `available_parallelism()` each and hand the codec every core, which on the
  4-core no-SMT desktop left nothing for the desktop itself. It lives in prefs
  and on the command line and **never** in a scene or view — a sidecar may
  record what was used, as information, but nothing should replay it, because
  `threads = 16` is wrong advice on the T490. The dialog's control is greyed
  for a still, because encoding is the only thing it steers today; a still's
  PNG deflate is one short single-threaded pass.
- The time estimate runs on *working* time, not wall clock: pauses are
  subtracted, or the projected remaining time climbs while progress is frozen,
  which is a countdown that goes up.
- One job at a time. A queue was deferred in the first plan and stays deferred.

The CLI paths pass `control: None` and are unchanged.

## Supersampling and the reconstruction filter

Offline renders accumulate at `N x` output resolution and filter down
(`--supersample N`, 1–4, **default 2**; `--filter box|triangle|gaussian|mitchell|lanczos`,
default gaussian; `--filter-radius PX` in output pixels, default 0.5). This is
flam3's `oversample` + `filter_radius`, and it is the largest visible quality
win this renderer has.

**Why it beats simply adding points.** Every point was deposited into exactly
one pixel with no kernel, so the image was a raw histogram read out through a
log curve. Measured: past ~33 samples/pixel it stops getting visibly better,
and what remains is *not* shot noise — it is pixel quantization. Rendering 4x
and filtering down beat native rendering at the **same total sample count**,
decisively, for +40% wall clock. More samples fix noise; filtering fixes
aliasing, and only one of those was missing. See `RENDER-QUALITY-BASELINE.md`.

Default 2 rather than 1 is a deliberate change to what every `--render` writes.
`--supersample 1` restores the old behaviour, and is byte-identical to it.

**The filter runs on linear values, before any log tonemap.** For splat that
means between accumulate and tonemap, on the `(colour*weight, weight)`
accumulation — `SplatRenderer` owns its own supersampling for exactly that
reason, since nothing outside it can reach that point in the pipeline. For the
points renderer it is an sRGB texture, where `textureLoad`'s decode and the
render target's encode put the averaging in linear light for free (and straight
alpha is premultiplied across the filter and unpremultiplied after). Filtering
after the log would blur in a perceptually compressed space and muddy bright
cores.

Gaussian is the default; **not lanczos**, whose negative lobes ring around
exactly the small bright cores a flame image is made of. The downsample shader
clamps filtered density at zero for the same reason — a negative density is a
NaN once the tonemap takes its log, not a darker pixel.

**Three things are measured in render-target pixels and must agree**, which is
what `offline::Sampling` exists to guarantee:

- the camera's `screen_height`, which is the *accumulation* height;
- `CameraUniforms::max_point_pixels`, the near-field size cap. It means 12
  *output* pixels, so it scales with N. It was the literal `12.0` in both point
  shaders, which was right only while the render target was always the output;
  left alone, close motes silently shrink to 12/N output pixels as quality goes
  up. It occupies what used to be the struct's tail padding, so every WGSL
  mirror of `CameraUniforms` names it;
- `use_point_primitives`, the subpixel test. **This is the one that would make
  the feature look like it did nothing.** Compared against output height, a
  point that is subpixel at output but not at accumulation resolution keeps
  taking the unfiltered 1px path — which is precisely the finest, most
  alias-prone material in the picture.

The size *floors* deliberately do **not** scale with N (the splat radius's
`1.0`, and `min_point_pixels`). One accumulation texel is the finest thing the
target can represent, and letting a splat be that small is what supersampling
buys: at 4x, a point a quarter of an output pixel across stays a quarter of an
output pixel instead of being inflated to a whole one. Scaling the floors would
undo the feature for the smallest material there is. (`RENDER-QUALITY-PLAN.md`
says to scale both bounds; that is right for the cap and wrong for the floor.)

**Brightness is invariant to the factor**, and the arithmetic is exact rather
than approximate. The filter takes a weighted *mean* over each N x N block,
which divides density by N²; `exposure_scale` is `∝ screen_height²` and gets
the supersampled height, which is N² larger. They cancel. Measured across
1x/2x/4x on `nautilus`: mean 60.7 / 60.9 / 60.3. Passing the *output* height to
`upload_params` would darken every supersampled render by N².

Live window only ever runs at 1x. The point cloud's aliasing *is* the look
there (see the MSAA note above), and N x supersampling costs N² fill.

### 16-bit PNG

`--bit-depth 16` (and the dialog's checkbox) writes 16 bits per channel. Worth
having because supersampling produces exactly what 8 bits bands: smooth wide
gradients over a large area, where the step between adjacent codes shows as a
contour. It is an **output-format** choice and not a quality one — the render is
identical either way, only the quantization of the file differs — so the default
stays at 8, since a 16-bit PNG is twice the size and most renders are a check on
the framing rather than a keeper. Stills only; both video codecs take 8-bit
frames.

Three things make it work, and the first is the one that matters:

- **8-bit keeps the `Rgba8UnormSrgb` target**, so the hardware still does the
  sRGB encode on store and an 8-bit render is **byte-identical** to every render
  made before this existed (verified against a pre-change binary). A float
  target with a CPU encode everywhere would round differently in the last bit
  and silently churn every image in the project. 16-bit renders into
  `Rgba16Float` and `offline::linear_to_srgb` does the encode at readback —
  measured to agree with the hardware to under one 8-bit code.
- **The contact sheet is `Vec<u16>` unconditionally**, and `glyphs::draw_label`
  writes into it. An 8-bit channel is widened by `* 257` — not `<< 8`, which
  would map 255 to 65280 rather than to white — and that is exactly reversible
  by `/ 257`, which is what lets one sheet type serve both depths without the
  8-bit save writing anything different.
- **`offline::f16_to_f32`** decodes the float target. Twelve lines rather than a
  dependency, and tested against all 65536 inputs rather than a handful — which
  is how its subnormal branch was caught being off by one exponent.

## Render records: what a picture was made from

Every still and every animation now writes a **record** (`src/record.rs`) —
twice, to two audiences:

- **into the PNG**, as `iTXt` chunks, so the picture carries its own recipe
  wherever it goes. `Software` (= `fracturize α-0.5 (15698f9, 2026-09-27)`: the release from `version.txt`, then the git commit — `-dirty` if the compiled-in source had uncommitted changes — and that commit's date, all stamped by `build.rs`; `--version` prints the same), `Creation Time`,
  `fracturize:scene` (the full scene TOML, verbatim), `fracturize:render` (the
  same block as the sidecar) and `fracturize:scene_sha256`;
- **beside it**, as `renders/<name>.render.toml`, following the pattern
  `views/` established, so it can be read without a PNG parser. Animations get
  the sidecar only — the muxer is hand-rolled and emits `ftyp`/`mdat`/`moov`,
  and adding a `udta`/`meta` box is real work in a format this project already
  treats gingerly for upload-pipeline compatibility.

Five things about it are load-bearing:

- **It never goes into the scene file.** `point_count` and its neighbours are
  deliberately not scene data, precisely so a 100M batch and a 6M exploration
  session can't clobber each other; a `[last_render]` block would put exactly
  that back, as a second source of truth that disagrees with whichever render
  actually ran last, plus git-diff noise and a race between concurrent
  sessions. `renders/` is gitignored. A useful consequence is that
  `Scene::save`'s comment-preserving `toml_edit` merge never has to learn about
  render data, so there is no new way for scene round-tripping to drift.
- **`iTXt`, not `tEXt`.** tEXt is Latin-1; the version string alone is `α-0.5`,
  and scene and author names are UTF-8 in general. iTXt is the chunk PNG
  defines for UTF-8.
- **The scene is embedded as rendered, not as the file on disk.** By the time a
  render runs the scene may have been through `-S` overrides, a `--palette`, a
  mutation or a `--zoom` the file knows nothing about.
  `Scene::to_toml_string` is what `Scene::save` already used internally, so
  they can't drift. The sha256 is what tells "the scene as rendered" from "the
  scene as it is now".
- **The camera comes from `CameraOverride::describe`**, the one function that
  knows when the yaw/pitch/roll chart can say a framing and when it has to fall
  back to an exact `rotvec`. A second spelling would be a second thing to get
  wrong at the poles.
- **`[machine]` is informational and says so in the file.** Nothing in it
  should ever be replayed — `threads = 16` is a fact about the desktop and
  wrong advice on the T490. It explains a timing; it is not an input.

Mutation and sweep sheets deliberately get **no** record: they are many
different scenes, and a `fracturize:scene` chunk claiming one of them would be
a confident lie about the other tiles. Their per-tile mapping is printed, and
mutation variants are already written out as `<out>.mutN.toml`.

Writing the record never fails a render. The picture exists either way, and
reporting a successful render as failed because a receipt couldn't be filed is
the wrong trade — it warns instead.

Not built yet: **`--reproduce render.png`** (read the chunks back, write the
scene to a *new* file — never into `scenes/` — and re-invoke with the recorded
flags). The plumbing it needs is now all here. It should promise "same recipe",
not "byte-identical across machines": GPU float non-associativity across
vendors is a real caveat, though the chaos game itself is deterministic.

## GPU timing, and the batch size that isn't

`--gpu-timing` reports GPU-busy time per chaos dispatch (`src/gpu/timing.rs`).
It is measurement, not a render setting, and it silently does nothing on a
device without `Features::TIMESTAMP_QUERY` — which is requested when the
adapter offers it and never required.

It exists because it settled a question in the opposite direction to the one
everyone expected, and that answer is worth not re-deriving:

**Bigger chaos dispatches do not help. They are slower.** The reasoning that
said they should is sound as far as it goes — `points_per_frame` is
`buffer_capacity / 800`, a constant chosen for the *interactive* ring, and an
offline job has no ring to cycle — and there really is ~0.031 ms of fixed
per-dispatch overhead, 44-64% of each dispatch's wall clock at the interactive
rate. But GPU-busy time turned out to be linear in the batch with **no fixed
GPU-side term at all**, and slightly superlinear at that. Batching to a 2 ms GPU
budget made the chaos fill **19% slower** end to end. Swept at 1920x1080 / 100M
points / accumulate 16384: 1.47 s at the existing 114,688-point rate, rising
monotonically to 1.78 s and flattening. The batching was reverted; the
instrument that caught it was kept.

The likely mechanism is locality: 16,384 walkers is a fixed amount of
parallelism whatever the iteration count, so a longer dispatch exposes no more
of it while the points it streams stop fitting in cache. Which points at the
untested follow-on — if there is a lever here it is **more walkers**, not more
iterations per walker. `num_workgroups` is a hardcoded 64. Changing it changes
walker seeding and therefore the image, so it is not a free experiment.

`RENDER-QUALITY-PLAN.md` calls dispatch batching "the real GPU-side lever".
That was the right thing to test and the wrong answer; this is the note saying
so.

## Size tiers: `--effort tiny|small|medium|large|huge`

Named for **size and nothing else**. The tiers used to be `draft`/`ultra` plus
an accumulating one that wanted calling `overnight` or `converged`, and both of
those are promises the program cannot keep: a duration depends on the machine
(this runs on a GTX 1080 *and* a T490), and "converged" asserts an outcome the
user may have their own reasons to push past.

A tier sets **target samples per output pixel** — 1, 10, 100, 1000, 10000, four
orders of magnitude across five names. Samples *per pixel* because it is
resolution-independent: the same tier means the same density at 720p and 4K,
which a raw point count cannot do. `--spp` goes past `huge`.

For `--splat` a tier **accumulates**, because that is the only way to actually
deliver a sample density. The points renderer has no histogram, so there a tier
degrades to the nearest point buffer and says so on stderr rather than quietly
under-rendering. Note one lap of the ring is the granularity, so small tiers
overshoot — `--effort tiny` at 400x300 reports 8 spp, not 1, and reports it
truthfully.

Accumulation makes **one still**, so anything that renders several images — an
orbit/move grid, a mutation or sweep sheet, an animation — takes a tier the way
the points renderer does: as the nearest point buffer, per image, with a note.
It used to convert the tier to `--spp` first and then refuse it, so the one
command CRAFT tells you to run in its first paragraph (`--effort … --splat` on a
sheet) failed, blaming a flag nobody had typed. An explicit `--spp` on a sheet
is still refused; that one was asked for.

## Accumulation: `--spp`, and the sample ceiling it removes

The ordinary render splats the point ring **once**, so the distinct samples in
a finished image equal the ring's capacity and nothing more. `--accumulate`
changes *which* samples survive, never how many. That is the ceiling: a 1.45 s
run generates 2.1e9 samples and throws away 95% of them, and `--effort ultra`
is the largest buffer worth allocating rather than the best picture this
renderer can make.

`--spp N` (splat only, stills only) turns the ring into a streaming working
set. Each time the chaos game has replaced every point in it — one **lap** —
the whole buffer is splatted into a transient texture and folded into a
persistent 64-bit fixed-point histogram, and the samples are then free to be
overwritten. Quality is bounded by time, not by VRAM.

```
fracturize -s scenes/blossom.toml --splat --spp 8000 -r out.png
fracturize -s scenes/blossom.toml --splat --spp 20000 -r out.png
```

**It works, and here is the evidence.** Two renders differing only in
`--chaos-seed` differ by sampling noise and nothing else, since everything
deterministic cancels. blossom, 960x540, `--supersample 2`, 16-bit:

| spp  | mean abs. difference |
|------|----------------------|
| 100  | 2.9e-4               |
| 500  | 1.4e-4               |
| 2000 | 7.0e-5               |
| 8000 | 3.5e-5               |

Textbook 1/sqrt(N) — every 4x in samples halves the noise, with no plateau
anywhere in range. 8000 spp (the old `converged` tier; `huge` is 10000 now) is
~100x below one 8-bit code (1/255 = 3.9e-3), ~35 s at 1080p on the reference
desktop. No tier is called "overnight"; the measurement says the hardware does
not need a night. `--spp` is there for anyone who wants to spend one anyway.

### Things that will bite

* **`--spp` counts against *output* pixels**, not accumulation texels, so
  turning on `--supersample` costs N² more fill per lap but does not silently
  demand N² more laps.
* **Exposure is normalized by the laps actually completed**, not the ones
  asked for. That is what makes cancelling produce the same picture, noisier,
  rather than one darkened in proportion to how early it stopped. It is also
  why `upload_params` takes `f64`: an accumulating render passes 2^32 samples
  in minutes.
* **`--points` no longer sets quality here, it sets *batch size*.** Under
  accumulation the buffer's only job is keeping the GPU busy between folds, so
  `converged` deliberately uses a *smaller* buffer than `ultra` — the memory is
  better spent on the histogram, which is 32 bytes a texel and is what decides
  whether a large supersampled render runs at all.
* **Splat only, stills only, and it says so** rather than quietly rendering at
  the ring-buffer sample count. A contact sheet or an animation would be one
  full accumulation run per tile or frame, which is a fine thing to want and a
  terrible thing to get by accident.
* The histogram is **64-bit fixed point** and the reasons are in
  `src/gpu/points/accumulate.rs`. Briefly: f32 *stalls* — once a hot texel's
  running sum exceeds an increment by 2^24 the increment stops registering, and
  the sum-to-increment ratio is just the lap count — while 32 bits cannot
  simultaneously resolve a large gaussian's faint tail and hold a bright core.
* Folding **per lap, not per dispatch**, is what makes it affordable: the fold
  is a compute pass over every texel of the accumulation, so it wants
  amortizing over a whole buffer of points rather than an eightieth of one.

### The fp16 stall, and why the accumulation format is per-device

**This was a real correctness bug in the first cut of slice 4b.** `Rgba16Float`
carries 11 bits of integer precision, so a texel that reaches **2048** in one
pass stops registering unit increments — a hard stall, not a rounding error.
In the ring-buffer path that only flattens the very hottest cores. Under
accumulation it was much worse: *each lap's batch* clipped at 2048, so the
finished picture's brightest material depended on how many laps the ring took.

Measured on blossom at a fixed ~65M samples, varying only `--points`:

| ring | laps | peak density |
|---|---|---|
| 1M | 62 | 126,976 |
| 4M | 16 | 32,768 |
| 20M | 4 | 8,192 |

Exactly `2048 x laps` every time. `--points` was silently changing the picture,
which it must never do.

Fixed by accumulating into `Rgba32Float` wherever the device will blend into it
(`Features::FLOAT32_BLENDABLE`, requested when offered and never required).
After the fix, peak density per sample is 0.01735 / 0.01736 / 0.01734 across
those three ring sizes — a scene property again. `accumulation_is_exact()`
reports which format a device got, because on hardware without the feature the
old behaviour still applies and a caller that says nothing leaves the user
comparing renders that were never comparable.

The shader change that had to come with it: `u32(x * scale)` overflows once `x`
passes 65536, which an fp32 batch reaches easily. `accumulate.wgsl` now splits
the scaled value across the word boundary instead of clamping at the fp16
ceiling — that clamp was only ever safe because the batch had already stalled.

### Two panics fixed on the way

Both predate accumulation and both used to die inside wgpu quoting a number the
user never typed:

* `--supersample 4` at 4K wants a 15360 px texture against a limit of 8192.
  Now `Sampling::check_fits`, called after `create_device` in all five offline
  entry points. The 8192 was `wgpu::Limits::default()`, not the hardware — the
  GTX 1080 reports 32768 — so `create_device` now asks for the adapter's
  `max_texture_dimension_2d` and 4x at 4K works.
* `--effort ultra` on a **zoom scene** dispatched 390,625 workgroups against a
  limit of 65,535, because `rewrap` runs one thread per point in 1D. Now a 2D
  dispatch with the row stride in `ComputeParams::rewrap_stride`, which took a
  word of existing padding and so costs nothing.

## The tonemap grade: `--gamma`, `--gamma-threshold`, `--vibrancy`

Before these, the tonemap had exactly two knobs: exposure, and a hardcoded
`GAIN = 0.25`. There was no gamma curve at all, and gamma is most of what
people mean by the Apophysis look.

```
fracturize -s scenes/blossom.toml --splat --gamma 2.5 --gamma-threshold 0.35 -r out.png
```

**The neutral grade is the exact identity, not merely close.** At `gamma = 1`
the curve is `x^1`, so both of the shader's vibrancy routes collapse to
`mean_color * coverage` and the blend between them is inert whatever
`vibrancy` says — which is why the neutral vibrancy can be 1 without that
being a choice. A render with no grade flags is byte-identical to one from
before grading existed, and `grade_tests` pins it.

* **`--gamma`** lifts the dim end. It also lifts the single-sample speckle
  scattered over the background, which is what the threshold is for.
* **`--gamma-threshold`** blends the curve toward a straight line below the
  threshold, so near-zero coverage stays near-zero. **Its units are not
  flam3's.** flam3's `linrange` is in raw density; this one is in post-log
  *coverage*, whose whole scale is 0-1. Measured useful range **0.05-0.5**.
  On blossom at `--gamma 2.5`, threshold 0.5 recovers 80% of the lifted
  background for 5% of the bright detail. Gamma and vibrancy do mean the same
  thing in both programs; only the threshold's scale differs.
* **`--vibrancy`** picks how gamma reaches colour. At 1 it goes through
  coverage, so hue is untouched and bright cores stay saturated. At 0 it goes
  through each channel of the emitted colour, lifting a bright pixel's dim
  channels further than its strong one — a highlight rolling off toward white,
  the way film does it.

Recorded in the sidecar and the PNG only when non-neutral, so every record
made before grading existed is byte-identical and an absent block reads as
"the tonemap this program has always had".

### A grade belongs to a view, not a scene

`gamma`, `gamma_threshold` and `vibrancy` are **view** keys, alongside
`exposure`. A scene is the three-dimensional thing you explore; a view is
"save the state I want this re-rendered from", and a grade is exactly that
kind of state — it changes nothing about what the attractor *is*, only how its
density becomes a picture. Keeping it out of the scene also means a grade
found while rendering can never clobber a scene file mid-exploration.

```toml
# views/whatever.toml
renderer = "splat"
exposure = 1.0
gamma = 2.5
gamma_threshold = 0.35
```

Resolution is **flags over view over neutral, field by field** — the same
ladder `exposure` uses, and per-field so a view setting only `gamma` gets the
neutral threshold and vibrancy rather than being treated as ungraded.
Verified byte-identical either route.

A view with no grade keys loads as neutral, and saving an ungraded view writes
no grade keys, so every view already on disk round-trips unchanged. Three
tests pin that.

**Still to do:** the GUI has no grade controls, so `current_view()` writes
`None` for all three and a `V`-key save carries no grade. That is deferred to
a renderer-dialog pass once the rest of the render-quality work is done.

## Re-grading: `--grade-out`, `--retonemap`, `--grade-sweep`

The tonemap is a **pure function** of one texture plus a few scalars: it reads
output-sized linear density and writes pixels. It runs *after* the
reconstruction filter, so that filtered buffer is exactly its input — and
saving it means the grade can be redone without re-rendering.

```
# render once, keeping the linear buffer (bare --grade-out writes <render>.fgrade)
fracturize -s scenes/blossom.toml --splat --spp 500 --grade-out -r out.png

# grade it nine ways into one contact sheet
fracturize --retonemap out.fgrade --grade-sweep gamma --grade-range 1:4 \
           --sweep-steps 9 -r sheet.png

# or a single re-grade
fracturize --retonemap out.fgrade --gamma 2.5 --gamma-threshold 0.35 -r graded.png
```

Measured: a 1080p `--spp 500` render is **3.05 s**; re-grading it is **0.33 s**,
most of which is GPU device creation. Nine tiles of a 320x200 sweep is 0.21 s
*total*. That is the difference between answering a tonemap question by
looking and answering it by arguing, which is what this slice is for.

**Re-grading is exact, not an approximation of a re-render.** `--retonemap`
with no overrides reproduces the source render **byte for byte** — verified
across all three paths (1x, `--supersample 2`, and accumulating with a
non-neutral gamma carried forward). The buffer's own exposure and grade are
the starting point, so a bare re-grade is a no-op by construction.

### Sizes, and why not the histogram

| Artifact | 1080p @ 2x | Buys you |
|---|---|---|
| `.fgrade` (this) | **32 MB** | exposure, gamma, threshold, vibrancy, background |
| histogram checkpoint | 265 MB | also re-filter, re-supersample, resume accumulating |

16 bytes a pixel regardless of how long the render took or how much
supersampling it used — the buffer is always *output*-sized. The histogram
checkpoint is slice 5b and opt-in.

### Things that will bite

* **Two float formats reach the readback and they are not interchangeable.**
  The accumulating path resolves into `Rgba32Float`; the ordinary ring-buffer
  path's accumulation is `Rgba16Float`. Reading fp16 pairs as f32 produces
  confident garbage, which is what it did the first time — `read_float_texture`
  now branches on `texture.format()` and panics on anything else rather than
  guessing.
* **Splat, single tile.** A contact sheet has a different linear buffer behind
  every tile and there is no honest single file to write, so `--grade-out` says
  so instead of silently writing whichever tile was last. Animations, variant
  sheets and sweep sheets reject it outright.
* **The `.fgrade` carries the whole render record** in its TOML header, so
  provenance survives. The re-graded PNG does **not** yet carry those chunks
  forward — a natural follow-up, not done.
* `GradeAxis`'s CLI name and its `label()` differ on purpose: clap kebab-cases
  to match the `--gamma-threshold` flag, `label()` is the TOML key. A test
  checks they agree modulo the separator.

## Settling decision 5: `tools/exposure_survey.py`

Decision 5 is whether exposure should be normalized per image from that image's
own density (Apophysis-style) or stay the scene-independent constant it is
today. A gamma sweep cannot answer it — the question is about whether
*different scenes* land comfortably under **one** exposure.

```
tools/exposure_survey.py                       # four contrasting scenes
tools/exposure_survey.py --effort medium scenes/a.toml scenes/b.toml
```

It renders each scene with `--grade-out`, reads the linear density straight
out of the `.fgrade`, and asks what exposure would put each scene's material at
a target coverage — under **several anchoring rules**, because which part of
the histogram you anchor to turns out to matter more than the scenes do.

Measured on blossom / galaxy / fern_3d / glasshouse at `--effort medium`:

| anchoring rule | spread across the four scenes |
|---|---|
| p99.5 | 63.8x |
| p99 | 46.3x |
| p95 | 17.8x |
| **median lit** | **2.4x** |
| mean over all | 14.4x |

**Settled 2026-08-14: exposure stays fixed and scene-independent.** Hazel's
call, on this table plus the comparison sheet.

The scenes agree about typical brightness to within 2.4x and disagree about
*peaks* by 64x. That is a difference in dynamic range, not in exposure — and
dynamic range is what `--gamma` and `--gamma-threshold` are for.

The 2.4x is the whole case on its own: if the per-scene correction is that
small, an "adaptive" exposure is a **global constant** plus noise, and a global
constant is what the fixed default already is. You would be changing the
default, not adapting to anything.

Both anchors also look wrong, in opposite directions, which is what the sheet
showed:

* **median-lit** wants exposure 9.7–22.8 against a fixed 1.0, and **blows out**
  most of the scenes.
* **p99.5** wants 0.016–1.04 and goes dark, blossom nearly vanishing.

The absolute level there is a `--target` artifact rather than evidence about
per-scene adaptation — only the *spread* was ever that. But it does mean
neither anchor is a tonemap anyone would ship unattended.

Two things worth knowing before re-running it:

* The spread is **independent of `--target`**. The `(2^(target/gain) - 1)`
  factor is common to every scene and cancels out of the ratio. Only the
  *anchor* moves it, which is why the tool sweeps anchors and not targets.
* It reads `EXPOSURE_K` and `GAIN` out of `splat.rs` rather than restating
  them, so it cannot quietly describe a tonemap that no longer exists.

The first run of this survey was **wrong**, and usefully so: it reported 48.9x
at one tier and 13.0x at another, which is what exposed the fp16 stall above.
A number that moves when it shouldn't is worth more than a number that looks
plausible.

## Checkpoints: `--checkpoint`, `--resume`, and a Ctrl-C that keeps the work

The grade buffer re-grades; a **checkpoint** resumes. It is the accumulation
histogram itself — supersampled size, 32 bytes a texel, so 8x larger than a
`.fgrade` at `--supersample 2` — and it is the state the chaos game was adding
into, which is why it can be added to further.

```
fracturize -s scenes/blossom.toml --splat --spp 8000 --checkpoint -r out.png
# ...Ctrl-C at any point; out.png and out.fhist are both written...
fracturize -s scenes/blossom.toml --splat --spp 20000 --resume out.fhist -r out.png
```

**Ctrl-C keeps the render.** First interrupt finishes the current lap, saves,
and reports the run partial; second interrupt exits immediately, because a
program that ignores a second Ctrl-C is one you have to kill from another
terminal. Measured: interrupting a 14,400-lap run at 6 seconds stopped at lap
1341, kept 5,588 samples/px, and wrote both the PNG and a 61 MB checkpoint.
Before this, `--checkpoint`'s promise of writing on abort was only reachable
from the render-job dialog — from a terminal the process died first, which is
precisely the case the feature exists for.

**An interrupted render exits 130**, the conventional "terminated by SIGINT"
status; a completed one exits 0. The files are written either way, before the
status is decided. This is the only signal a shell gets: the partial PNG is a
real, useful file and says nothing about its own completeness, so without a
distinct status `fracturize ... && upload out.png` would publish a render that
was interrupted at 3% as though it had finished.

**`--spp` is a total, not an increment.** Resuming to a figure you already have
does nothing rather than doubling it. A bare `--resume` with no `--spp` runs one
more lap.

### A resume must deal a different hand

**This was a bug, and a nasty one.** The chaos game is deterministic, so
resuming with the original seed replays the identical walker stream and every
"new" lap deposits a perfect copy of samples the histogram already holds.
Measured: a 20 spp render checkpointed and resumed to 40 spp came out
**byte-identical** to the 20 spp one. The sample count doubles, the noise does
not fall at all, and the render reports a quality it did not deliver.

`resume_seed(seed, prior_laps)` mixes the lap count in, which deals an
independent stream while staying deterministic — resuming the same checkpoint
twice gives the same picture, and `prior_laps == 0` returns the seed untouched
so a fresh render is unchanged. Verified by convergence: independent-run
difference falls 1.39x from 20 to 40 spp when resumed, against 1.38x
straight-through and a √2 = 1.41 ideal.

### What is refused, and why each would fail silently

A checkpoint is only meaningful under the geometry and the scene that made it,
so `check_compatible` refuses by name rather than producing a quiet mess:

* **different scene** — adding another attractor's samples into the histogram
  is a double exposure, at whatever ratio the sample counts landed on.
* **different size** — reinterprets the buffer as a differently-shaped image.
* **different `--supersample`** — the histogram is stored at the *supersampled*
  size, so the two cannot be added at all.

## Density estimation: `--density-estimation`

The variable-width blur — wide kernels where the histogram is sparse and noisy,
narrow where it is dense and detailed. The plan calls it the thing that most
distinguishes an Apophysis render, and it is **not substitutable by more
samples**: more samples improve the whole image proportionally, while this
targets exactly the regions still noisy at a finite budget.

```
fracturize -s scenes/blossom.toml --splat --spp 300 --density-estimation 0.5 -r out.png
```

One amount, 0-1, with the internals derived — the `haze.rs` precedent. Needs
`--splat` and an accumulating render; the ring path has no
accumulate → filter → tonemap chain for it to sit in and says so.

The amount is a **strength mix**, not a radius scale: the kernel is identical
at every setting and the amount is how far the estimate is blended over the
unsmoothed image. So 0.3 is three tenths of the effect, and the control is
linear from end to end.

### Four things that were wrong before they were right

**A summed-area table does not survive f32.** The plan proposes one: 4 taps for
any radius. The arithmetic is right and the precision is not — a SAT
accumulates the whole image into its far corner and a box query is the
*difference* of two large nearly-equal sums. At 1600x1200 with mean density
~2e4 the corner reaches ~4e10, where f32 resolves steps of ~2.3e3, against tail
texels worth ~3. **715x the signal**, worst exactly where DE matters. A mip
pyramid avoids the problem instead of paying for it: every level is an
*average*, so magnitudes stay bounded.

**A 2x2 box reduction looks square.** Repeated box reduction keeps the kernel
square, and at level 3 whole flowers came out as visible rectangles. The
reduction is a 4x4 binomial tent (1-3-3-1 per axis), one step of a gaussian
approximation, so stacking levels converges toward round.

**The radius must ask its neighbourhood, or DE is just bloom.** flam3's DE is a
*scatter*: each bucket spreads with a width set by its own density, so a dense
filament has a narrow kernel and stays put. A GPU gather inverts that — an
empty texel beside a bright filament has near-zero density, takes the widest
radius on offer, and reaches *in*, dragging the filament outward.

The symptom was unmistakable once measured: **DE's effect did not diminish at
all between 30 and 3000 samples per pixel** (3.166 → 3.044), because empty
texels stay empty however long you render and they surround everything. One
extra tap fixes it — take a second density reading from the neighbourhood at
the scale the first guess would have blurred over, and use whichever is denser.
Effect now falls **13.6x** across that range (0.396 → 0.029), which is what
"DE retreats as a render converges" has to look like.

**The amount fought the probe, and the probe won.** Found by live-testing the
finished feature rather than by any test: `amount` originally scaled both
`TARGET_DENSITY` and `MAX_RADIUS_PX`, which reads like the obvious meaning of
"more DE". But the probe's level is derived from the first radius guess, which
comes from the target — so a bigger amount widened the probe, the wider probe
found more structure, and the radius collapsed back to about where it started.
Raising the amount mostly bought a better reason *not* to blur.

Mean luminance on ammonite at 10 spp, against the amount:

| amount | 0 | 0.2 | 0.4 | 0.6 | 0.8 | 1.0 |
|---|---|---|---|---|---|---|
| scaled | 48.0 | 51.4 | 52.0 | 51.7 | 52.2 | 51.8 |
| mix | 48.0 | 49.1 | 49.9 | 50.7 | 51.3 | 51.8 |

The scaled row is flat past 0.2 and not even monotonic — a 0-1 control with one
usable position on it, while the help text promised "try 0.3 before 1.0, it is
a strong effect". Disabling the probe restored monotonicity, which is what
identified the cancellation rather than some other cause. The fix is to leave
the internals at their calibrated values and apply the amount as a final mix.
Note where the rows meet: at 1.0 they are the same render, because full
strength was always the calibrated kernel. Nothing about DE at full strength
changed — everything below it became reachable.

### The radius law

`radius = sqrt(target / density)`, derived rather than tuned. Sampling noise
falls as 1/sqrt(N), so bringing a texel holding `density` samples up to the
quality of one holding `target` means averaging over `target / density` texels
— an area, so the radius is its square root. Anything already past `target`
gets a radius below 1 and is returned **bit-identical**.

Fixing the exponent at what the noise model implies is what lets this be one
control instead of flam3's three. The first attempt used flam3's free
`estimator_curve` of 0.4 and blurred filaments and voids nearly equally — the
exponent was doing the work, and it was the wrong exponent.

`TARGET_DENSITY = 16` is calibrated against measured densities, not guessed:
blossom's lit accumulation texels run p50 ≈ 7.9, p90 ≈ 162, p99 ≈ 2284. A first
try at 256 put the median lit texel at a 2.4-texel radius and smeared the
structure along with the noise.

### Interactions

* Runs **between the histogram resolve and the reconstruction filter** — on
  linear density, at accumulation resolution, before the log.
* A **checkpoint stores the un-estimated histogram**, so DE is applied fresh on
  each resolve. You can change `--density-estimation` on a `--resume` freely.
* A **`.fgrade` is post-DE**, so `--retonemap` cannot change it. That is the
  line between the two files: grade buffers re-grade, checkpoints re-render.
* Recorded in the sidecar only when non-zero, so earlier records are unchanged.

## What each quality knob actually buys

Measured on `scenes/lacewing.toml` at 960x540, gamma 2.3 / threshold 0.32.
Noise is the RMSE between two renders that differ only in `--chaos-seed`, which
isolates **sampling noise** from fractal structure — the structure is identical
in both, so whatever remains is the sampling. This is the metric to use for any
"is it less noisy?" question; a plain standard deviation cannot tell grain from
filaments and will happily report a blurred image as an improved one.

| configuration | noise | vs baseline |
|---|---|---|
| 100 spp, `--supersample 1` | 0.03462 | — |
| 100 spp, `--supersample 2` | 0.03462 | 1.00x |
| 100 spp, ss2, `--density-estimation 0.6` | 0.02280 | 1.52x |
| 1000 spp, ss2 | 0.01194 | 2.90x |
| 1000 spp, ss2, `--density-estimation 0.6` | 0.01104 | 3.14x |

Three things worth reading off it:

**Supersampling is not a denoiser.** It changes sampling noise by 0.008% — the
two top rows are the same number. It fixes *aliasing*, which is a different
defect that a noise metric cannot see: mean and standard deviation come out
bit-identical between ss1 and ss2 while 5.9% of pixels differ, because
supersampling redistributes energy locally and conserves it exactly. Judge it
by looking at edges, never by a noise figure.

**More samples follow 1/sqrt(N), with no plateau.** 10x the samples buys 2.90x
here, against the 3.16x the law predicts — and that shortfall was **the 8-bit
PNG, not the renderer.** It was blamed on the gamma curve for a while, on the
theory that RMSE measured after a nonlinear tonemap understates. That was
wrong. Re-measured at `--bit-depth 16` on `lacewing` at 1080p:

| `--spp` | noise, 16-bit | per decade | noise, 8-bit |
|---|---|---|---|
| 1,000 | 0.00665 | — | 0.00685 |
| 10,000 | 0.00211 | **3.157x** | 0.00261 |
| 100,000 | 0.000667 | **3.155x** | 0.00138 |

3.16 to three digits, twice. The convergence is textbook and the instrument was
the problem.

**So a noise measurement must be made at 16-bit.** An 8-bit file's own
quantisation contributes about `q/sqrt(12) * sqrt(2)` = 0.0016 RMSE between two
independently quantised renders, which is a *floor* the measurement cannot see
past. It inflates the figure by 24% at 10,000 spp and by 108% at 100,000, where
the render is genuinely finer than the file can hold.

That floor has a practical edge, not just a methodological one: sampling noise
crosses below 8-bit quantisation noise at roughly **30,000 spp**. Past there an
8-bit output is the limiting factor and more samples buy nothing you can save —
which is one of the arguments for 16-bit TIFF in `RENDER-SCALE-PLAN.md`.

**DE retreats as a render converges.** It buys 34% at 100 spp and 7.5% at 1000
spp, because it targets exactly the texels that are still noisy and there are
fewer of them every lap. That falloff is the property that distinguishes DE
from a blur, and it is what the neighbourhood probe exists to produce — see
"the radius must ask its neighbourhood" above.

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
| `shape` | where the attractor lands, its box, where it falls in the frame, and the two flags that would fill it |
| `maps` | four fixed lines per transform |
| `render` | point size, count, haze, decay, exposure, background |
| `colour` | mode, the three dials, luminance, the ramp |
| `camera` | the framing that would render, its screen axes in world space, and the scene's own under it |
| `path` | every keypoint, not just the count |
| `zoom` | the band as rows, or every eligible map as a `--zoom` command |

**Read `notes` first.** It is one block, second, listing every diagnostic the
report found with the section each is about, and a count on its first line —
so "is this scene sound?" is one line to read rather than fifty. `notes none`
is the cheapest possible signal that nothing is wrong. It catches, among
others: a `point_size` over the crisp bound, a framing far from the one that
fills the frame, a map that never fires, an expanding plain map (and whether
the maps still contract on average), a walk that escapes and keeps being
re-seeded, a flat gradient, a `--view` framed against a different scene, an
`edge_guard` ramp reaching into the picture.

**Where the report has computed a number you will act on, it emits the action.**
`shape` prints `-S camera.distance=1.42`, not `~1.42`; the zoom eligibility
list prints `--zoom descent`. A suggestion you have to reassemble into a flag
is a suggestion half-given, and the reader who has the flags memorised is the
smallest slice of who reads this.

`shape` is the part a TOML cannot answer: `point_size` and `camera distance`
are the two things most often wrong in a hand-authored scene, and neither can
be checked by looking at the file. In `maps`, contraction is the **signed** cube
root of the affine parts' determinant, pre times post — negative means the map
reflects, at or past 1.0 means it expands — and rotations are re-derived from
the matrix, so an authored `(-26, 138, 0)` can print as `(154, 42, -180)`: same
rotation, other euler branch.

**That number is the whole map only for a plain map** (`linear = 1`, nothing
else). A map with variations prints it as `affine`, because the variations
rescale space between the two affine halves: an inversion (affine,
`spherical`, post-affine) has affine halves multiplying to exactly 1 and still
contracts, and a fold's scale says nothing about where it sends a point. So
`--info` doesn't judge settling from the matrices. It *counts*: `shape` notes
the share of steps on which the CPU walk escaped and was re-seeded
(`trace::escape_fraction`), which is the measured answer, and is something the
box can't show, since a re-seeded walker lands back in the unit cube and looks
like part of the cloud. An expanding plain map alone doesn't stop the walk
settling either: the chaos game converges when the maps contract *on average*,
and the note says which side of that the scene is on.

`shape` also says **where the walk falls in the frame** that would render —
`on screen x 30% to 74%, y 10% to 83%`, 0% to 100% being the frame left to
right and top to bottom at `--width`/`--height`'s aspect, and `in frame` as the
share of the walk inside it — beside a world-space `box`. Both hold the middle
95% of the walk, like the radius. They are facts with no verdict attached, on
purpose: a compact attractor centred with a margin is the common framing, not
the only good one, and a close-up or a form that bursts past every edge is a
choice the report cannot tell from a mistake.

**Balance is the one framing it does flag**, because filling the frame is a
choice and crowding one side of an empty frame rarely is — it was the
commonest slip in small models' scenes, a focus left at the origin with the
attractor off to one side. `silhouette` is where the material *shows*: the lit
cells of a 32-column grid reaching half a frame past each edge, lit at a
hundredth of a dense cell (the log tonemap's view, so a sparse wing counts and
stray dust doesn't), from a 40k-step walk. `balance` per axis is
`(before − after) / (before + after)` over its margins: 0 even, ±1 flush to an
edge, and `fills` when under a quarter of the axis is empty — a close-up has
no margins to balance, and is never flagged. `to centre it` is the
`-S camera.focus=` pan (turn and distance kept, solved at the walk's depth
and refined by reprojecting) and `centred, it shows` where that lands. `notes`
speaks when an axis has room and one margin is four times the other (|balance|
≥ 0.6; rule-of-thirds is about 0.5). Infinite zoom says n/a: it renders the
band, not the bounded walk.

`camera` gives the framing as directions too: `forward`, `right` and `up` in
world space, then `axes`, which says where each world axis points on screen —
the app's corner axis cross in words (`+x right +y up +z toward you`). Moving
the focus along `right` slides the picture left; which way `+x` does that
depends entirely on where the camera is, and this is where it says.

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

### Deriving a map: `--derive`

`--derive <map>` prints a new `[[transform]]` block built from an existing one
and exits; it edits nothing. `--turn AXIS:DEG` turns the whole map about a world
axis through the origin (translation included — "k/n of the way round its
group"), `--spin AXIS:DEG` spins it about its own axis (translation kept, scale
spun with it). Both repeat; turns compose on the left and spins on the right, so
only the order within each list matters. `-S` applies first, so a derived map
can also be squashed or recoloured in the same command. The block is written by
`Scene::transform_toml`, the save path's own serializer: degrees where they
reproduce the matrix, `rotvec` where they don't.

It prints rather than adding a scene key on purpose. A load-time `turn` key
would have to survive every save the GUI makes — baked away it loses what the
author wrote, kept it means un-turning every edit before writing — and a pasted
block is just a transform.

### What changed: `--diff`, and three-way merges

`--diff A B` prints the changes from scene A to scene B (`src/scene_diff.rs`),
and is the shared core of agent collaboration (`AGENT-COLLAB-PLAN.md` §3).
Each value change is printed as a line `-S` accepts, so applying them to A gives
B. That holds over every scene in `scenes/` × its mutations
(`set_ops_rebuild_the_other_scene`). What `-S` can't say is listed after it: a
transform added, removed or reordered, a key back to its default, a block
(symmetry, a camera path, an xaos matrix) rewritten. Exits 0 when the scenes
are the same and 1 when they differ, like diff(1).

`--diff MINE THEIRS --base BASE` merges three ways: THEIRS' changes from BASE
laid over MINE, so edits made since BASE survive. A value both sides changed
differently is a **conflict**: listed, MINE kept unless `--take-theirs <path>`
names it. A transform one side removed and the other edited is one conflict on
the whole transform. `--merged FILE` writes the result. This is what the GUI's
Apply will do to a proposal.

```
fracturize --diff scenes/koru.toml renders/koru.mut3.toml
fracturize --diff live.toml option-B.toml --base sent.toml --merged out.toml
```

Three things it depends on:

- **Both sides are compared as canonical TOML, read back and written twice**
  (`canonical_text`). A scene in memory can hold a rotation the writer spells
  on one euler branch while the same matrix, read back, spells on the other;
  one pass made a merge of nothing look like a changed rotation.
- **Transforms and subscenes are matched by name**, and by position where they
  are unnamed or share a name, which goes wrong as soon as one is inserted
  before them.
- **Colours a file left out stay out** (see "Colour"), so adding a transform
  to such a scene is one `+ transform` line. Colours that *are* written down
  don't move when a transform is added, so they don't show up either.

### Reading a picture: `--stats`

`--stats <png>` reports on a rendered image: luminance and clipping, how much of
the frame is **empty ground**, and the lit pixels' hues grouped into families
(circular runs of occupied bins in a 16-bin histogram). Two things it does that
a pixel census wouldn't:

- **empty is measured against the background**, read from the scene the render
  embedded in its PNG (`fracturize:scene`), else the commonest border colour —
  and it says which. Counting pure black reported 0% empty on any scene with a
  chosen background, which is most of them (CRAFT §2.5). Colour statistics are
  taken over lit pixels only, so the ground's tint isn't counted as a hue.
- **the monochrome note fires on one narrow family**, which is what a pinned
  colour index (CRAFT §2.3) looks like. Two separated families — a base and an
  accent — are a two-colour picture, not a defect.

### Framing from the command line

`--yaw --pitch --distance --roll --focus x,y,z` override the camera, winning
over both the scene and any `--view`. They exist so that trying a framing
doesn't require authoring a view file for it. When any of them is given,
`--render` prints the `[camera]` block it settled on, ready to paste into a
scene:

```
fracturize --scene scenes/octahedron.toml --distance 4 --pitch 0.4   --render /tmp/o.png --effort small --width 400 --height 300
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

- `--effort tiny|small|medium|large|huge` — a target of 1/10/100/1000/10000
  samples per output pixel (see "Size tiers"). Under `--splat` a single still
  accumulates to it; anything rendering several images (grids, mutation and
  sweep sheets, animations) and the points renderer get the point buffer
  nearest it instead, with a note on stderr. The pre-rename names
  (`draft|low|high|ultra|converged`) are refused with the nearest new tier.
  Explicit `--points N` / `--spp N` override the tier.
  Point count is a *render* property, not a scene property: the scene's
  `point_count` is just the interactive default (16 bytes/point of GPU memory).
- `--width/--height` (default 1920x1080) — per tile when a grid mode is used.
- `--render-preset NAME|FILE` — a render preset's size and quality (see
  "Render Jobs"), filling each of `--width --height --spp --points
  --accumulate --supersample --filter --filter-radius --density-estimation
  --bit-depth --fps --quality` only where it wasn't typed; it prints the flags
  it came to, and `(yours kept: …)`. Its `spp` is a density target exactly as
  an `--effort` tier is, and degrades the same way (accumulates only for a
  single `--splat` still, otherwise the nearest point buffer, with a note);
  `--effort` wins over it. Its density estimation is dropped, with a note,
  where nothing accumulates, and a preset made for the other kind of file
  (a clip preset on a `.png`) lends its size and quality and says so. It
  never sets `--splat`, exposure or the grade. `--render-presets` lists every
  preset with its description and the exact flags it comes to;
  `--save-render-preset NAME` writes what this command line comes to as one
  of the person's presets (the dialog lists it under Yours), never over an
  existing one, and without `--render` saves and exits. So an agent can hand
  a person a render recipe in the form their dropdown takes:

  ```
  fracturize --render-presets
  fracturize -s s.toml --splat --render-preset hd -r renders/s.png
  fracturize --render-preset print-a3 --density-estimation 0.3 --save-render-preset a3-soft
  ```
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
- `--volume [--opacity X]` — the volume renderer (see "Rendering Approach"):
  splat with absorption, opacity default 2, 0 is plain splat. One-pass stills
  only; it refuses `--spp`, accumulating `--effort` tiers, grids, sheets and
  animations by name, and `--opacity` without `--volume`. A view's
  `renderer = "volume"` selects it too, at the default opacity.
- `--splat [--exposure X]` — render with the splat renderer (also implied by a
  view saved in splat mode; explicit flags win). Works with all grid modes.
  Exposure is capacity-normalized, so the same value looks the same at every
  effort level; raise it (1.5-3) to brighten thin filaments, lower it to
  recover detail in hot cores.
- `--supersample N` (1-4, **default 2**), `--filter box|triangle|gaussian|mitchell|lanczos`
  (default gaussian), `--filter-radius PX` (default 0.5) — render the histogram
  N x larger and filter it down. The largest visible quality win here, and note
  that the default is 2 rather than 1, so it changes what every `--render`
  writes; `--supersample 1` is byte-identical to the old renderer. See
  "Supersampling and the reconstruction filter".
- `--bit-depth 8|16` (default 8) — bits per channel in the PNG. The render is
  identical either way; only the file's quantization differs.
- `--threads N` (default: one less than the machine has) — CPU threads for
  encoding an animation. A machine setting, never scene or view data; see
  "Threads are a machine setting" under Render Jobs.

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

**Which map made that?** `--isolate <map>` draws only the points that map
placed and leaves the rest of the frame bare. A point belongs to the map that
moved it last, so the maps' parts partition the picture: each is exactly the
pixels the full render has there, and together they add up to it. Under
symmetry a motif's part includes its whole orbit. `--map-grid` is the sheet
form: one tile per map (up to 36), labelled with the map's name and its share
of the points, with each tile's `--isolate` flag on stdout.

It is a filter in the chaos pass, not a renderer: the walk runs unchanged, and
a point from any other map is written at `ISOLATE_DROPPED` (`chaos.wgsl`),
which both point shaders clip in every vertex entry point —
`every_point_shader_clips_what_isolate_drops` holds the three files together.
So it works on every render path, accumulation, transparency and animation
included. Exposure is still normalized by every point the walk made, which is
what keeps a map's part at the brightness it has in the full picture; the
`samples/px` a render reports counts those too, kept or not.

This is deliberately **not** the app's selected-transform glow. That is tuned
for a person with a mouse in hand: it brightens and dims by degrees, which
reads in motion and is a judgement call in a still. An agent with weak vision
needs the answer to be present or absent.

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
  twice for a 2D grid: the first varies across columns, the second down rows,
  and `--sweep-steps` applies to each, so two sweeps of 4 make a 4x4 sheet of
  sixteen tiles. A third is refused.
  Composes with `--set`, which sets the base every tile starts from.
- Sweeps need `--scene`, and **each tile refills the point buffer** (the swept
  parameter changes the IFS), so prefer `--effort tiny|small`. Unlike mutation
  sheets no variant files are written: a tile is fully described by one flag,
  and that flag is printed per tile, ready to paste.

**Tile labels.** Every sheet — orbit, move, mutation, sweep — draws its
per-tile parameters into the tile in amber (`src/glyphs.rs`, a 5x7 bitmap font;
the same colour the app uses for world-anchored transform names). Sheets print
to stdout too, but stdout isn't in the PNG, and an agent reads the PNG. A label
too long for its tile is truncated with a trailing `>` rather than running into
its neighbour. `--no-labels` turns them off.

**Animation** (`--render <out.avif|out.mp4>`): the camera flying
the scene's camera track — its views and the spline through them, or the default full-turn orbit
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

- `--fps N` (default 30), `--seconds S` (default: the timeline's length — the
  camera track's keys, or its lap — but the *default* orbit's lap is ~35s,
  one turn at 0.18 rad/s, so a scene without a camera track animates
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
  `--width 480 --height 270 --fps 12 --effort tiny`.
  Verify output with `ffprobe` / extract frames with `ffmpeg`.

**Mutation sheets** (evolutionary exploration): `--mutations N` renders the
scene plus N random variants (tile 0 = original, near-square grid). Each
variant is saved as `<out>.mutN.toml` and its operator list printed per tile
("T2 rotate 21° about (...)", "T1 +swirl 0.18", ...), so you can look at the
sheet, pick a tile, and load/iterate on its TOML. `--seed` reproduces a
sheet (the used seed is always printed), `--random-config` picks the dice
profile that says what may change, and `--mutation-strength` overrides its
strength. Every tile went through the gate, judged against the original (see
"The dice"); one where nothing passed is left empty and says why.
`--random-sheet N` is the same sheet of fresh rolls instead. Unlike camera grids, each tile refills the point
buffer, so prefer `--effort tiny|small`. The same operators run in-app on `U`.

```
# quick look at a new scene from all sides (fast: use tiny + small tiles)
fracturize --scene scenes/koru.toml --render /tmp/koru_orbit.png \
  --effort tiny --orbit-grid 4x2 --width 480 --height 270

# test a slight camera move around a saved framing
fracturize --scene scenes/koru.toml --view views/koru-123.toml \
  --render /tmp/koru_move.png --effort small --move-grid 3x3 --width 320 --height 180

# final frame
fracturize --scene scenes/glasshouse.toml --view views/glasshouse-123.toml \
  --render renders/glasshouse_4k.png --width 3840 --height 2160 --effort large

# evolve: original + 8 mutations, reproducible
fracturize --scene scenes/koru.toml --render /tmp/koru_evo.png \
  --effort tiny --mutations 8 --seed 42 --width 320 --height 180

# roll random flames offline; --seed makes any roll reproducible, and the
# seed used is always printed so a good one can be recovered
fracturize --random --render /tmp/roll.png --effort small --width 480 --height 270
fracturize --random --seed 7 --render /tmp/roll7.png --effort large
# ...under the app's Random settings (variations, bounds), exactly as it rolls
fracturize --random --seed 7 --random-config ~/.config/fracturize/prefs.toml --info

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

### Driving the running app: use a nested X server, always

`--screenshot` covers the point cloud, and view files cover framing. When you
genuinely need the *live* app — anything about input handling, or anything
whose symptom only exists while the camera is moving — drive it inside
**Xephyr**, on a display of its own, and never on `:0`.

This is not tidiness. XTest events do not go to the process you have in mind;
they go to whatever the pointer is over and whatever holds focus. Synthetic
scroll aimed at this app has landed in Hazel's other windows and scrolled them.
There is no targeting flag that fixes that — the isolation is the fix.

```python
xe = subprocess.Popen(['Xephyr', ':79', '-screen', '1800x1000', '-ac', '-noreset'])
env = dict(os.environ, DISPLAY=':79')
app = subprocess.Popen(['./target/release/fracturize', '--scene', scene], env=env)
d = display.Display(':79')                      # python-xlib; there is no xdotool
```

Five things that will each cost you a run:

- **Capture the Xephyr *root*, not the app's window.** `get_image` on the app's
  own window returns blank — it is a GL surface and its contents are not in the
  X server. Find the window for its geometry, then read those coordinates out
  of the root.
- **Make the Xephyr screen bigger than the window.** The app opens larger than
  you expect; a read that runs past the root's edge fails with `BadMatch`, not
  with a short read. Clamp the request to the root geometry as well.
- **Give it time to come up.** The point buffer fills over several seconds and
  a capture taken early is of a half-built picture, which reads exactly like
  the artifact you are hunting.
- **Keystrokes don't arrive.** Xephyr runs no window manager, so nothing gives
  the app keyboard focus and XTest key events go nowhere — silently, with no
  log line. Pointer events are fine. Drive an action through its button (Undo
  and Redo are on the toolbar), or `set_input_focus` on the app's window first.
- **To hold the camera still, load a view with `-v`, not a keystroke.** Applying
  a view pauses the orbit as a documented side effect. Toggling camera motion
  with `o` depends on focus having landed where you assumed, and when it
  silently doesn't you get a moving camera that you believe is stationary.

That last one is worth stating as a rule of its own, because it produced a
confident wrong answer during the zoom investigation: **frame-to-frame
differencing measures flicker only if the camera is provably stationary.**
Verify the camera is still *before* attributing anything to the renderer —
compare a frame against one many frames later, and check the ratio to the
adjacent-frame difference. A scene that is merely moving shows a large lag-1
difference and a much larger lag-N one; a scene that is genuinely resampling
every frame shows the two nearly equal. Reporting the first as the second wastes
a bisect. Pinned properly, an ordinary scene sits at ~0.00% frame-to-frame and
a zoom scene at ~0.16%.

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
