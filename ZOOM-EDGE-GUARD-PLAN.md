# Plan: a camera-relative edge guard, replacing the octave fade

Written after a review of the octave-fade work (OCTAVE-FADE-PLAN.md and what
landed from it). The arithmetic that shipped is correct — the CPU and GPU
copies of `octave_offset` agree, the taper hits its target distribution, the
tests prove what they claim. The *design* is what's wrong, and no tuning of it
can produce the thing actually asked for: old octaves leaving the picture
gradually, at a perceptually constant rate over the progress of the zoom.

## Why the current design cannot work

The octave fade is a **static, world-space density profile**: the outer
`fade_periods` shells of the band are dealt fewer points, ramping from 1/16 of
a share at the rim up to full (`renorm.rs` `octave_offset`, mirrored in
`points/chaos.wgsl`). That profile is baked into the point deal and never
moves.

Between wraps the camera moves continuously, so the image evolves
continuously. The wrap is an exact similarity jump, and it is invisible
precisely where the point density is scale-invariant — i.e. where the deal is
flat. Anywhere density varies with radius, the entire difference is delivered
**at the wrap instant** as a discontinuous step. A fade is, by definition,
density varying with radius. So each faded shell brightens gradually through
the period and then snaps by a factor of `fade_g` (≈ 0.40 for a 3-octave
fade — a 2.5× density step) at every wrap. `renorm.rs` says this itself:
*"This is also, exactly, how much the on-screen density of the faded region
changes at each wrap."*

The telescoping argument in `DEFAULT_OCTAVE_FADE`'s doc ("the fade cannot make
the step smaller, only wider") proved that the fade only redistributes the
change **over screen area**. The requirement is that it be spread **over
time**. No static profile can do that: the wrap is the only discontinuous
event, so a static profile concentrates 100% of its non-invariance there.
That is why the live spike went 35× → 10× the median frame step and stopped —
10× is the floor of this design, not a residual bug.

There is also a geometry problem making it worse at the defaults: with
`radius = 4.8` and a 3-octave fade, the faded region spans world radii
`[0.6, 4.8] × band`, but the visible field only needs material out to
`MIN_RADIUS ≈ 2.42 × band` and the haze *near* plane sits at `0.58 × d`. Two
of the three faded octaves are inside the visible field, part of them
completely un-hazed near mid-frame, stepping 2.5× per wrap with nothing
hiding it. `band_covers_the_view()` doesn't know about the fade, so no
warning fires. For the fade to sit wholly out of view you'd need
`radius ≥ 2.42 · 2^fade ≈ 19` — no scene has that.

## The design that does work

Fade at **render time**, in **camera-relative (scale-invariant) coordinates**:
weight every point by a guard

```text
    ρ = |pos − fixed_point| / d          d = |eye − fixed_point|, current frame

    G(ρ) = 1                             ρ ≤ ρ_start
         = 1 − smoothstep over ln ρ      ρ_start < ρ < ρ_end
         = 0                             ρ ≥ ρ_end
```

Two properties, both exact:

1. **The wrap step is identically zero.** `ρ` is invariant under the wrap
   similarity — the wrap scales `|pos − fp|` of the material at each pixel and
   `d` by the same factor. Not "small": zero, by construction, at every haze
   amount.

2. **Perceptually constant fade rate.** Zoom progress is linear in
   `ln d`, and for a fixed piece of material `ln ρ = ln r − ln d`, so a
   feature crosses the guard's log-space ramp at a constant rate per unit of
   zoom progress. The ramp **must** be a smoothstep in `ln ρ`, not in `ρ` —
   linear-space would fade fast at the near end of the ramp and slow at the
   far end.

This is "the last stretch of haze, made mandatory and taken all the way to
zero, in ratio space" — which is exactly why full-strength haze already hides
the edge today. The guard is pure **transmittance** (weight), fading material
into the background; it does not touch colour or saturation. That matches the
haze philosophy in `src/haze.rs`.

### Choosing the ramp

All ratios below are in band units (multiples of the reference eye distance),
so `spec.radius` is directly usable.

- `ρ_end = spec.radius`. The band's true outermost material reaches
  `R/√s` (the `round()` in `renormalize()` spreads the outer shell half a
  period past `R`), and its on-screen ratio is minimized at `d = band`, where
  it is `spec.radius/√s > ρ_end`. So a guard that is zero at `ρ_end` hides
  the hard edge at every phase, with margin.

- `ρ_start = ρ_end / 2^W`, where `W` is the guard width in octaves.
  Default `W = 1.0`. With the default radius 4.8 that puts the ramp over
  `[2.4, 4.8]`, which begins almost exactly at `MIN_RADIUS` — the ramp lives
  in the part of the field that full haze would have hidden anyway, and at
  lower haze it dims the far field by a **constant** amount, which is
  invisible in motion.

- Repurpose the existing `octave_fade` scene/CLI field as `W` (keep the name
  or rename to `edge_guard`; if renamed, keep parsing the old key). `0` should
  mean "default width 1.0", not "off" — the guard is what makes the edge
  lawful and there is no reason to run a zoom scene without it. Clamp `W` so
  `ρ_start ≥ MIN_RADIUS` when the radius allows it, i.e.
  `W ≤ log2(spec.radius / MIN_RADIUS)` when that is ≥ the default; when the
  band is too short to allow even the default width (authored radius near or
  below MIN_RADIUS), let the ramp eat inward and keep the existing BAND TOO
  SHORT warning — a dimmed-but-steady view beats a snap.

### Where it goes

The guard is evaluated per point in the vertex shaders, exactly where haze
already is:

- `shaders/points/splat.wgsl`: multiply into `out.weight` in **both**
  `vs_splat` (next to `haze_weight(depth)` at ~line 175) and
  `vs_splat_point` (~line 199).
- `shaders/points/render.wgsl`: same idea at its haze site (~lines 147–165);
  compute the guard in the vertex stage (world position is available there)
  and carry it to where transmittance is applied.

Uniforms — extend `CameraUniforms` (`src/gpu/buffers.rs:182`). The struct has
two floats of tail padding; the guard needs five (grow by 16 bytes, keep the
multiple-of-16 rule and the WGSL structs in all three point shaders in step):

```text
    guard_center: [f32; 3]   // the fixed point, world space
    guard_ln_near: f32       // ln(ρ_start · d)  — world units, this frame
    guard_inv_ln_width: f32  // 1 / (ln ρ_end − ln ρ_start); 0 disables
```

Shader side:

```wgsl
    // 0 when zoom is off; otherwise fades ln r over the guard ramp.
    fn guard_weight(pos: vec3<f32>) -> f32 {
        if camera.guard_inv_ln_width == 0.0 { return 1.0; }
        let r = length(pos - camera.guard_center);
        let t = (log(max(r, 1e-20)) - camera.guard_ln_near) * camera.guard_inv_ln_width;
        return 1.0 - smoothstep(0.0, 1.0, t);
    }
```

Per-frame plumbing: `d` must be the **current** eye-to-fixed-point distance,
recomputed every frame (it is what makes the guard track the zoom). Fill the
new fields at every `CameraUniforms::new` site with the scene's `Renorm` in
hand, zeros when zoom is off:

- live: `src/app.rs:3685` and `src/app.rs:3954`
- offline: `src/offline.rs:705`, `:903`, `:1017`, `:1151`

Prefer a small constructor helper (e.g. `CameraUniforms::with_zoom_guard(&renorm, eye)`)
over six hand-expanded copies. Put the ramp math (`ρ_start`, `ρ_end`,
`ln`/width) in one Rust function — `Renorm::guard_params(eye: Vec3) -> (f32, f32)`
or similar — so the six call sites and the tests share one derivation.

Cost: one `length` + one `log` per point per vertex invocation. Negligible
next to the matrix multiply already there; nothing measurable expected even
on the T490.

## Retire the deal-side taper

With the guard in place the static taper is not just redundant — leaving both
would double-fade the rim and reintroduce a (smaller) wrap step. Remove it:

- `renorm.rs`: `fade_periods`, `fade_g`, `FADE_DEPTH`, `SUGGESTED_OCTAVE_FADE`,
  and the taper branch of `octave_offset` (the `m1` piece). **Keep** `geo_sum`
  / `geo_pick` and `octave_q` — the falloff (`octave_falloff`, the
  stills-only clustering control) is untouched by all of this and its math is
  fine.
- `points/chaos.wgsl`: `FADE_DEPTH`, the taper branch in `octave_offset`,
  `zoom_octave_fade`, `zoom_fade_g` (leave pads in their place or renumber —
  whatever keeps the 192-byte layout honest with `buffers.rs`).
- `src/gpu/buffers.rs:327–328`: the two uploads.
- `Renorm::summary`: replace the "outer N octaves faded" clause with the
  guard's report (width in octaves, and where the ramp starts in band units).
- Tests in `renorm.rs`: drop the taper-distribution tests
  (`the_taper_matches_its_target_distribution`, `..._intended_depth...`,
  `..._moves_points_inward...`, `..._never_eats_more_than_half...`,
  `a_fade_wider_than_the_band_still_deals_every_point`,
  `the_fade_and_the_falloff_compose...`); `no_fade_is_the_flat_deal_exactly`
  becomes "the deal is flat whenever falloff is 0", unconditionally.
- While in `chaos.wgsl`: the comment block above `octave_offset`
  (~lines 289–297) still claims the taper "is switched off on the CPU" when
  the falloff is in use — that went stale at commit 8d65a88 and goes away
  with the taper anyway.

Scene compatibility: `scenes/octave-edge-test.toml` and
`octave-edge-visual.toml` set `octave_fade`; under the repurposed meaning
they get a 3-octave guard, which is legal (radius permitting — the clamp
handles it). Re-render them and update whatever numbers their comments quote.

Note for live A/B checks after this lands: removing the deal taper changes
the *deal*, and the point buffer turns over at 1/800th per frame
(`gpu/points/compute.rs:99`) — ~13 s at 60 fps before the cloud fully
reflects it. The guard itself is render-time and updates instantly.

## Fix the pinned haze band under zoom (separate, real bug)

`App::haze_range` (`src/app.rs:3221`) honours a pinned haze band whenever a
view pinned one — and legacy views (no `haze` amount) are treated as pinned
forever (`src/app.rs:848`). A band pinned in world units breaks the haze's
wrap-invariance: the offline renderer's doc (`src/offline.rs:286`) records
exactly this artifact — 12% drift across a loop undone by an 11% snap — and
the auto-ranging fix there does not protect the live app when a pinned view
is loaded.

Fix: when infinite zoom is enabled, ignore the pin and auto-range —
in `App::haze_range` and in offline's `Haze::band`. Surface it (status bar or
`--info`): "haze band auto-ranged (pinned band ignored under infinite zoom)".
A pinned band is a framing tool for stills; under a zoom it is a bug by
construction.

## Verification

Unit tests (in `renorm.rs`, against the shared Rust guard function):

1. **Wrap invariance**: for a camera anywhere in the band and any world
   point, the guard weight computed pre-wrap equals the weight computed
   post-wrap for the point's similarity partner (mirror the structure of
   `wrapping_the_camera_does_not_move_the_picture`, which should also gain a
   weight assertion so the seamlessness claim covers density, not just
   position).
2. **Temporal continuity**: for a fixed world point, `G` as a function of
   phase across `[0, 1]` plus the wrap is continuous and monotone.
3. **The edge is hidden**: `G = 0` at the band's true outermost material
   radius (`R/√s`) at every phase.
4. **Constant rate**: `ln G`'s smoothstep argument advances linearly in
   `ln d` — equal zoom steps cross equal fractions of the ramp.

End-to-end (the measurements that actually catch this class of bug —
remember the offline loop render structurally cannot: it wraps every frame,
so the seam always measures as one frame step):

- The two-still wrap check (render at `radius` and `radius · s`) must now
  come out at ratio **1.0000 with the guard on** — the guard is invariant, so
  it adds nothing to the wrap even on scenes where the old fade added 4–6%.
  Rerun the table in `DEFAULT_OCTAVE_FADE`'s doc (wellspiral,
  pythagoras-zoomy, octave-edge-test) and rewrite that doc around the guard.
- Screen-record the live app on `scenes/octave-edge-visual.toml` (the
  zoom-band-measurement workflow): the wrap spike was 35× median frame step
  hard-edged and 10× with the old fade; with the guard it should be
  indistinguishable from an ordinary frame (~1×). That single number is the
  acceptance test for the whole plan.
- One phase-sweep still set is worth keeping as a tool: render ~8 frames
  spaced through one period on the real (non-loop) camera and check per-pixel
  brightness is smooth in phase, including across the seam. It is the direct
  measurement of "spread over the progress of the zoom".

## What not to do

- **No phase-dependent deal in `chaos.wgsl`.** The circular buffer holds
  points dealt over the last ~800 frames; a deal that depends on the current
  camera phase would mix stale phases for 13 seconds at a time. Camera-
  dependent fading must live at render time, where every point is re-weighted
  every frame.
- **Don't push the old fade further out with a bigger radius.** Hiding a
  W-octave static fade costs `2^W` in radius and the same again in levels;
  the guard costs nothing.
- **Don't keep both fades on** "for safety" — the static taper's wrap step
  is the artifact this plan removes.
