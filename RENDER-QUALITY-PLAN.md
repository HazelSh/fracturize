# Render quality: the plan

This is the design that came out of measuring the current renderer and a four-way
brainstorm. The measurements it rests on are in `RENDER-QUALITY-BASELINE.md` — read that
first; several of the conclusions below are counter-intuitive and only make sense with
the numbers attached.

## Status

**Slices 0 and 1 are implemented**, on branch `render-quality`, untested by Hazel.

- **Slice 0** (`5fdec96`) — encoder thread cap + `--threads` and a dialog control;
  partial-result-on-cancel via a `render_job::Outcome` enum; the shared const-trimmed
  `VERSION` in `src/version.rs`.
- **Slice 1** (`2c3f86b`, `874866b`) — `--supersample`, `--filter`, `--filter-radius`,
  the two clamp/threshold fixes, and `--bit-depth 16`.

Two deviations from what is written below, both deliberate and both argued at the
point they occur:

1. **The size *floors* do not scale with N.** The text below says both bounds of the
   splat radius clamp need scaling; that is right for the 12px cap and wrong for the
   1px floor. One accumulation texel is the finest thing the target can represent, and
   letting a splat be that small is exactly what supersampling buys — scaling the floor
   would undo the feature for the smallest material in the picture. Only the cap scales.
2. **16-bit PNG was not "nearly free".** It needed the render target format switched to
   `Rgba16Float` with the sRGB encode moved to the CPU readback, and the contact sheet
   and `glyphs::draw_label` widened to `u16`. Done, and 8-bit output is byte-identical
   to before — but it is a slice of its own, not a line.

Also note **`--supersample` defaults to 2**, so every `--render` writes a different
(better) image than it did. `--supersample 1` is byte-identical to the old renderer.

Remaining: slices 2–7 below. Decision 5 is still open by design — it is meant to be
settled by looking, once slice 5 exists.

---

## The short version

Hazel asked for a massive increase in achievable render quality, and was willing to trade
time for it. The surprise from measuring is that **time is not what is being traded**.
`--effort ultra` at 1920x1080 finishes in 0.35 seconds. The renderer is not slow; it stops
early, and it stops early because of an architectural limit nobody has to accept.

Three things are missing, in the order they should be built:

| # | Thing | Why | Cost |
|---|-------|-----|------|
| 1 | **Supersampling + downsample filter** | Biggest visible win. Measured: better image for +40% wall clock at 4x | Small |
| 2 | **Persistent accumulation histogram** | Removes the 1e8 sample ceiling entirely. Supersampling at 4K is what makes this necessary | Medium |
| 3 | **Density estimation** | The variable-width blur that gives Apophysis its smooth voids next to crisp filaments | Large |

Then a fourth strand that is not about the image at all: **provenance** — saving render
parameters into the PNG and alongside the scene.

And before any of it, two small bugs worth fixing on their own merits — see
"Fix these first".

---

## Fix these first (small, independent, worth doing regardless)

**1. The encoders take every core.** `src/avif.rs:58` and `src/h264.rs:74` both do
`available_parallelism().unwrap_or(4)` and hand the result straight to the encoder
(`with_threads` / `num_threads`). On this desktop — an i5-6600, **4 cores, no SMT** —
that is all four cores with nothing held back.

To be accurate about the impact: Hazel reports this does *not* cause bad lockups from
fracturize today, because renders don't last long enough for it to bite — she has seen the
symptom from other apps. So this is a latent problem, not a present pain. It becomes a
real one exactly when renders get long, which is what the rest of this plan is for:
`render_job.rs`'s own comments put the AV1 flush at ~75x the cost of rendering a frame, so
on a long animation job this would be the majority of wall-clock time running fully
saturated.

**Thread count is a required parameter, not just a better default** (Hazel's call): it
needs a real control in **both** the GUI and the CLI. See "Thread-count control" in leg 6.
Capping the encoders to `cores - 1` is the sane default underneath it.

**2. Cancelling a render throws away everything.** `fill_points` returns
`Err(CANCELLED)` (`offline.rs:258`, and at 741, 927, 1243), which propagates up through
`render()`'s `?`. Stop a job at 99% and you get nothing at all.

That is tolerable when a render takes 0.35 s. It is not tolerable once `overnight` exists
— and a partial accumulation is a genuinely usable image, just noisier, which is the
whole point of an anytime algorithm. On cancel, fall through to render-and-save with
whatever the histogram holds and report "stopped, partial". The downstream save path
already works from a smaller-than-target point count; that is what a mid-warmup buffer
already is.

---

## Where we are, measured

- Chaos game: **~1.6e9 samples/sec** on the GTX 1080. Splat raster: **~2e9 points/sec**.
- `--effort ultra` = 100M points, 256 accumulate frames, **0.35 s** at 1080p.
- The point buffer is a **ring**. `--accumulate` frames overwrite it. Total distinct
  samples in a finished image **equals the buffer capacity** and nothing more.
- A 1.45 s run generates 2.1e9 samples and **discards 95% of them**.
- `ultra`'s 1e8 samples is Apophysis **density ~48** at 1080p. Density 100k would be
  2.1e11 samples — about **230 seconds** of GPU time that the architecture cannot spend.
- But: past ~33 samples/pixel the image **stops visibly improving**. The remaining
  harshness is pixel quantization, not shot noise.
- Rendering at 4x resolution and filtering down beats native at the same sample count,
  decisively, for +0.15 s.

The last two points are the ones that reorder everything.

---

## Leg 1 — Supersampling and the downsample filter

Render the histogram at `N x` output resolution, then filter down. This is flam3's
`oversample` + `filter_radius`, and it is the single best value-per-effort change
available.

**Pipeline shape.** Insert one pass: accumulate at `N·W x N·H` (existing shaders
unchanged) -> **filter + downsample** to `W x H` -> tonemap (existing, unchanged). The
downsample must happen on **linear additive density, before the log tonemap**. Filtering
after the log would blur in a perceptually compressed space and visibly muddy bright
cores.

**Controls.** `--supersample N` (1-4, default 2), `--filter box|triangle|gaussian|mitchell|lanczos`,
`--filter-radius PX` in *output* pixels (default ~0.5). Internally the tap radius is
`filter_radius * N`, which is how the two compose in flam3.

Default kernel: gaussian (what this lineage expects by name), with mitchell worth an A/B
as a possibly-better default. **Not lanczos by default** — its negative lobes ring around
exactly the small bright hot cores flame images are full of.

**Two existing details become bugs under supersampling, and must be fixed with it:**

- `shaders/points/splat.wgsl:183` — `clamp(base_radius, 1.0, 12.0)` is in *render-target*
  pixels. Under N x supersampling the 12px near-field cap silently becomes 12/N output
  pixels, so close motes shrink. Both bounds need scaling by N.
- `src/offline.rs:723`, and the same expression at `971`, `1076`, `1219` —
  `use_point_primitives` compares `point_size * height / distance <= 1.5` against the
  **output** height. Under supersampling it must compare against `N * height`. Get this
  wrong and points that are subpixel at output but not at accumulation resolution keep
  taking the unfiltered 1px path — which disables the entire benefit for exactly the
  finest, most alias-prone material. This is the one that would make the feature look
  like it did nothing.

**Memory** is a non-issue at the sizes that matter: the accumulation texture at 1080p is
16.6 MB at 1x and 265 MB at 4x, against a 1.6 GB point buffer.

---

## Leg 2 — Persistent accumulation histogram

**Why it is needed even though leg 1 is the bigger visible win:** supersampling multiplies
buckets by N², so it divides samples-per-bucket by N². At 3840x2160 with 4x supersampling
there are 1.4e8 buckets; even 100 samples each needs 1.4e10 samples, which is 140x past
today's ceiling. Leg 1 spends the sample budget. Leg 2 supplies it.

**The design.** Per chaos-game frame:

1. `advance_frame` writes a slice of new points into the ring (unchanged).
2. Splat **only that slice** into a transient `rgba16float` batch texture. This needs no
   shader change — both splat entry points already index the point buffer by
   vertex/instance index, so drawing only the delta is a change to the **draw range**,
   `pass.draw(offset..offset+delta, ..)` (two calls when the slice wraps).
3. A compute pass with **one thread per texel** does `accum[i] += batch[i]` into a
   persistent fixed-point `u32 x 4` (r, g, b, density) storage buffer; the batch texture
   is cleared by the next pass's `LoadOp::Clear`.
4. Repeat until the sample target or time budget is met. Tonemap reads the persistent
   buffer.

**Why `u32 x 4` and one-thread-per-texel.** No atomics are needed — each thread owns its
texel exclusively, so there is no race however many points landed there. This sidesteps
every risky alternative: fp32 blending is not guaranteed exposed by wgpu; storage-texture
atomics are not reliably available; and non-atomic scattered writes from overlapping
fragment invocations are outright unsound (silently lost samples). Exactness ceiling is
4.29e9 per texel, extendable to 64-bit if ever needed.

The transient fp16 batch texture is fine because it only ever holds **one frame** of new
points before being folded in and cleared. The exception is a **collapsed attractor**,
where a single frame can put more than fp16's exact-integer limit of 2048 on one texel;
mitigate by not using the 10x warmup burst rate in this mode.

**The seeding trap — the sharpest correctness finding here.** `WalkerState::new`
(`compute.rs:239`) seeds each walker from its **index alone**. No clock, no entropy, no
user seed. Verified: two runs of the same command produce byte-identical PNGs. So if
"many batches" is implemented as "reset or rebuild `PointCompute` per batch", every batch
replays the *identical* walker trajectory, and the histogram re-counts the same samples —
the image gets **brighter, not better**, while the progress bar advances and the log
tonemap makes the result look plausible. The fix is also the simplest possible design:
**never reset between batches**; keep advancing the same walkers, which is already what
the interactive renderer does every frame. Then the whole run is reproducible from
(scene, seed, frame count).

Separately, there is no way to request an *independent* stream at all. Thread an explicit
`seed: u64` through `PointCompute::new`, defaulting to today's constant.

**Memory, with the real adapter limits** (measured via `vulkaninfo`, and these correct the
commonly assumed values): `maxStorageBufferRange` is **4 GiB−1**, not 2 GiB;
`maxMemoryAllocationSize` ~3.998 GiB; `maxImageDimension2D` **32768**.

| Output | SS | Texels | Persistent (16 B) | Fits one binding? |
|---|---|---:|---:|---|
| 1920x1080 | 4x | 33.2M | 531 MB | yes |
| 2560x1440 | 4x | 59.0M | 944 MB | yes |
| 4096x4096 | 2x | 67.1M | 1.07 GB | yes |
| 4096x4096 | 3x | 151.0M | 2.42 GB | yes (4 GiB cap) |
| 4096x4096 | 4x | 268.4M | 4.30 GB | **no — split across bindings** |

Everything Hazel is realistically rendering is comfortably inside one binding. Splitting,
when needed, is row-band partitioning with one dispatch per band — no in-shader
indirection.

**Do not build the host-RAM path.** With ~7 GB free system RAM against ~7.4 GiB free VRAM
and only 3 GB of swap, host RAM is not a larger reserve — it is a smaller one with a
catastrophic failure mode (swapping mid-way through a multi-hour render). Stay
VRAM-resident. If a target genuinely will not fit, **tile** instead; tiling costs roughly
N² more chaos-game work for an N x N grid because most samples miss each tile, which is
real and is the accepted historical price for otherwise-impossible print sizes. Last
lever, not the default.

**Once samples are unbounded, shrink the point buffer.** It stops being the image's
storage and becomes a streaming working set; 16-32M points (256-512 MB) is plenty, and
the VRAM comes back for the accumulator.

**Two honest scope limits:**

- **Splat only.** The opaque depth-tested point renderer shows the single nearest point
  per pixel. There is no density to sum, so accumulation cannot do anything for it. The
  UI must say so rather than leaving the knob wired to a mode it cannot affect.
- **Stills only.** Grids and animation exist on the opposite trade — one fill, many cheap
  reprojections. Accumulation is single-fixed-camera by construction. Forcing it through
  a grid means re-running the whole loop per tile.

---

## Leg 3 — Density estimation

The variable-width blur: wide kernels where the histogram is sparse and noisy, narrow
where it is dense and detailed. flam3 exposes it as `estimator` / `estimator_curve` /
`estimator_minimum`.

This is arguably what most distinguishes an Apophysis render from a fracturize one, and
it is **not substitutable by more samples**: more samples improve the whole image
proportionally, while DE targets exactly the regions still noisy at a finite budget. A
fixed filter (leg 1) has to trade detail for smoothness uniformly — crank it enough to
kill grain in a void and you have softened the filament beside it. DE is what lets both
coexist.

**GPU approach.** Rather than flam3's per-cell scatter, build a **summed-area table** of
the density and colour-weighted-density channels via a prefix-sum pass; then any texel's
box-blur of radius r is 4 taps, independent of r. That turns an O(pixels x r²) pass into a
bandwidth-bound O(pixels) one — the difference between tractable and not at supersampled
resolutions. Gaussian-shaped DE can be approximated by repeated box passes, though the
*variable*-radius version of that trick is the part I would validate before relying on it.

Expose **one amount, 0-1**, with the internals derived — the `haze.rs` precedent
("one amount, band and falloffs derived"). Sequence this last; it wants the
accumulate -> filter -> tonemap pipeline from legs 1-2 to already exist.

---

## Leg 4 — Tonemapping, bit depth, and colour

**Verified already correct, no action:** `fs_splat` writes `(color*w, w)` and the tonemap
divides `acc.rgb / acc.a` — the standard density-weighted mean. Colours do not wash out at
high density, and there is no double-gamma bug.

**Missing, in value order:**

1. **16-bit PNG output.** Nearly free — `image = "0.25"` already supports
   `ColorType::Rgba16`; output is currently hardcoded `Rgba8UnormSrgb` at `offline.rs`.
   Worth doing *with* leg 1, because smooth wide gradients are exactly what 8 bits bands.
2. **`gamma` + `gamma_threshold`.** There is no gamma curve at all today — just a fixed
   `GAIN = 0.25` after the log. Gamma is most of the "AV look"; the threshold is what
   stops near-zero-density pixels being lifted into a grey veil over the whole background.
3. **`vibrancy`.** Cheap once gamma exists; controls how far saturated colour survives
   into bright cores.
4. **Per-image adaptive exposure normalization** (flam3 derives its constants from the
   image's own max density). It would break the current documented invariant that a given
   exposure means the same thing across scenes and effort levels. **Still open — see
   decision 5.** The short version: build **retonemapping from a stored histogram** first,
   then decide this by looking at it rather than by argument.

EXR is not needed just to fix banding and would add a dependency; only worth it for
external regrading.

---

## Leg 5 — Controls, presets, and provenance

### Keep `--effort`, extend it

`--effort {draft,low,medium,high,ultra}` already exists and is documented throughout
`AGENTS.md`. Adding a parallel "draft/good/high/overnight" ladder would fork the mental
model. Instead: keep the names, grow `Effort::preset()` from returning `(points,
accumulate)` to a struct covering the whole new cluster, and add one tier —
**`overnight`**, meaning no fixed target: accumulate until cancelled. That tier is the one
that needs genuinely new plumbing (a job with no natural 100%).

### The primary dial should be samples per pixel

Apophysis users think in density, and it is resolution-independent — the same "good"
render needs different raw point counts at 720p and 4K, arithmetic nothing in the UI does
today. With accumulation, "keep going until N samples/pixel" is also the natural stopping
rule. So: expose **SPP** as the dial, derive points/accumulate/passes from
`SPP x width x height`, and keep `--points`/`--accumulate` as the advanced escape hatch.

`--accumulate` keeps meaning exactly what it means today, forever — no silent remapping of
a number someone's script already passes.

### Where each parameter lives

| Knob | Home | Why |
|---|---|---|
| tonemap curve, gamma, vibrancy | **scene** `[meta]` | changes what the picture looks like; same class as `haze` |
| density-estimation amount | **scene**, one float | changes character, like haze |
| supersample, filter kernel + radius, sample target | **job** | cost/quality at fixed artistic intent, like `points` today |
| threads, memory budget | **prefs + CLI**, never scene or view | describes the box, not the artwork |

The test to apply, from this codebase's own history: `exposure` got promoted to scene data
because the workaround for its absence was a comment in `ammonite.toml` telling you which
CLI flags to type. If a knob's absence would produce that comment, it belongs in the scene.

### Render records — sidecar (DECIDED)

**Hazel's call: sidecar only. Render parameters do not go into the scene file or the scene
format at all** — not as a data block, and not as the auto-managed comment line I had
offered as a middle option either; that is dropped. The
reasoning that persuaded her is kept here because it is the reason the boundary exists.

`point_count` and friends are *deliberately* not scene data, precisely so a 100M batch and
a 6M exploration session cannot clobber each other. A `[last_render]` block puts exactly
that data back inside `scenes/*.toml`, creating a second source of truth that disagrees
with whichever render actually ran last — plus git-diff noise on a tracked file, and a
race between concurrent sessions.

**The design:** a **sidecar** `renders/<slug>-<timestamp>.render.toml`, following the
pattern `views/` already established. `renders/` is gitignored, so: no diff noise, no
merge conflicts, no cross-session race. Contains source scene + sha256, the full quality
cluster, the camera actually rendered, and an informational `[machine]` block (threads,
elapsed) clearly separated from the reproduction-relevant fields.

A useful consequence of settling this: `Scene::save`'s comment-preserving `toml_edit`
merge never has to learn about render data, so there is no new way for scene round-tripping
to drift.

### PNG metadata

`image 0.25` pulls in `png 0.18` transitively, so adding `png` as a direct dependency is
free. `image`'s convenience wrapper has no text-chunk passthrough; the still-render write
path drives `png::Encoder` directly, which exposes `add_text_chunk` / `add_ztxt_chunk` /
`add_itxt_chunk`. One helper, called from the two current PNG write sites.

Chunks, following Apophysis's precedent of embedding the native format wholesale (scene
files are only ~1.6-1.8 KB):

- `Software` = `fracturize α-0.4` — the keyword PNG actually reserves for this. **Use
  `version.txt`, not `Cargo.toml`** (see below)
- `Creation Time` — ISO-8601, mirrored into `tIME`
- `fracturize:scene` — the **full scene TOML, verbatim**
- `fracturize:render` — the same block as the sidecar
- `fracturize:scene_sha256` — tells "the scene as rendered" from "the scene as it is now"

Not JSON: fracturize already has a native serialization, and translating would be a second
format to keep in sync for no reader's benefit.

### Which version string, and how (asked, and the codebase already answers it)

**Build time, and it already is.** `src/ui/shortcuts.rs:25` does
`const VERSION: &str = include_str!("../../version.txt")`, with a doc comment stating the
intent: "exactly one place to bump it and no file to find at runtime." No `build.rs`
exists or is needed for this.

Two things to get right when the PNG writer reuses it:

1. **`version.txt` is the real version, `Cargo.toml` is not.** `version.txt` holds
   `α-0.4`; `Cargo.toml` still says `0.1.0`. So `env!("CARGO_PKG_VERSION")` would silently
   embed a wrong version in every render. Use the file.
2. **Trim once, at compile time, not "at every use."** The current comment says "Trailing
   newline included, hence the `trim` at every use" — which is exactly the
   remember-to-do-it hazard that bites a *new* caller: a PNG text chunk with an embedded
   newline is a subtly corrupt record, and nothing would catch it. `str::trim_ascii_end`
   is const-stable (verified on this toolchain, rustc 1.95), so:

   ```rust
   pub const VERSION: &str = include_str!("../version.txt").trim_ascii_end();
   ```

   Promote that out of `src/ui/shortcuts.rs` into a shared location — the metadata writer,
   the sidecar, and the help window should all read one already-clean constant. Then the
   invariant is enforced by the compiler instead of by a comment.

A git commit hash is a separate, additive question: it *would* need a `build.rs` (there
isn't one). Worth doing eventually so a render can be tied to an exact tree, but it is new
infrastructure rather than a metadata-format decision — sequence it after the chunk
plumbing, and note that `α-0.4` alone does not distinguish two builds from the same
release name.

**Skip animation container metadata for now.** The muxer is hand-rolled and emits only
`ftyp`/`mdat`/`moov`; adding a `udta`/`meta` box is real work in a format the project
already treats gingerly for upload-pipeline compatibility. Give `.avif`/`.mp4` the same
sidecar instead.

### Progress reporting

With a known sample target and a measured rate, the ETA becomes genuinely accurate rather
than a guess — a real improvement on the deliberately-vague range `format_estimate` quotes
today, and worth saying so in the UI. For `overnight`, there is no percentage by design;
show elapsed, live density, and rate (`3h 12m · 1,840 samples/px · +6/min`) rather than
fabricating a fraction.

**The CLI currently prints nothing until the render ends.** That was fine at 0.35 s; it is
not fine once `overnight` exists, and a silent headless job is indistinguishable from a
hang to a script or an agent. Print periodic progress **to stderr**, keeping stdout's
parseable summary clean.

### `--reproduce render.png`

Worth building, and unusually honest here because the chaos game is already deterministic.
Read the chunks back, write the scene to a **new** file (never into `scenes/`), re-invoke
with the recorded flags. Promise "same recipe", not "byte-identical across machines" —
GPU float non-associativity across vendors is a real caveat. Sequence after the metadata
plumbing.

---

## Leg 6 — Machine resources: what the CPU should and should not do

**CPU sample generation: recommended against.** Two independent estimates, from the real
per-iteration work in `trace.rs` and from the measured GPU rate, put 3 threads of this
i5-6600 at **~1.5-3% of the GPU's 1.6e9 samples/sec** (~35M/s across 3 threads against
1.6e9/s). The gap is ~45-70x per thread.

Two details make it worse than a raw FLOPS comparison suggests, both visible in the code:
`atan2`/`sqrt` are computed **unconditionally every iteration** whether or not the active
variations need them (`trace.rs:25-27`, mirrored at `chaos.wgsl:150-152`) — GPUs have
special-function units for exactly this and CPUs pay full serial latency; and the CPU port
does a linear scan for transform selection where the shader does a binary search.

Hand-rolled AVX2 across walkers would give maybe 2-4x, not 8x — each lane can pick a
different transform and variation set, which is the same divergence problem the GPU
already handles better in hardware. That moves the CPU from ~2% to ~5-8%. Still not worth
it, and it would come with a permanent obligation to keep a second full copy of the
variation blend bit-compatible with the shader forever — `trace.rs` already carries that
burden and says so in its own doc comment.

**What the spare cores should do instead**, in order: video encoding (already happens —
just needs the cap above), progress/ETA and checkpoint writes, and *staying idle*. That
last one is a real answer, not a shrug: on a 4-core box with no SMT, 3 cores buying ~2%
more samples is a bad trade against Hazel being able to use her machine while a multi-hour
render runs. The render job already gets its own wgpu device so the window stays
responsive; leaving the CPU alone extends that same intent to the rest of the desktop.

**Keep histogram reduction on the GPU.** A readback-and-reduce path costs a PCIe round
trip (~12 GB/s) plus DDR4 bandwidth (~25-35 GB/s) against ~320 GB/s on-die. The CPU's only
legitimate touch is the single final readback at save time, which already happens.

**Dispatch batching is the real GPU-side lever.** The chaos loop runs **12,288
dispatches/sec** — ~82 µs each. `points_per_frame` is derived from `buffer_capacity / 800`
(`compute.rs:182-186`), a constant chosen so the *interactive* ring cycles smoothly over
800 frames. An offline job accumulating toward a sample target has no reason to honour it.
Bigger batches would amortize per-dispatch submission overhead.

Honest caveat: 82 µs is wall-clock around a submit-and-occasionally-poll cycle, so I
cannot yet separate fixed overhead from proportional GPU time. **Add
`Features::TIMESTAMP_QUERY` first** — neither device-creation path requests it and every
pass passes `timestamp_writes: None` — then tune batch size against real numbers rather
than guessing.

**Keep per-dispatch GPU-busy time in the low single-digit milliseconds.** On NVIDIA/X11 a
long dispatch can starve the compositor into visible input lag; on the T490's Mesa/i915
the hangcheck timeout and genuine display-pipe contention make the same rule bind harder.
One rule, two different reasons. Today's loop is already well-behaved by accident (many
small dispatches, a poll every 16 frames); a 20-50x bigger batch is still comfortably
inside the budget.

### Thread-count control (decided: build it)

One job-scoped value, exposed in **both** interfaces — a control in the render-job dialog
and a `--threads N` flag — applied to the encoders (`avif.rs`, `h264.rs`) and to anything
CPU-side the job spawns later. Default `available_parallelism() - 1`, so `j=3` on this box
is what you get without asking.

Two notes on shape:

- It is a **machine** setting, not artwork: prefs + CLI override, never scene or view
  data. A sidecar may record what was used, as information, but nothing should ever
  *replay* a thread count — `threads = 16` is actively wrong advice on the T490.
- The two encoder call sites currently each compute their own thread count independently.
  They should both read the one job value, so there is no second place to forget.

**Progress and ETA get honestly better.** `format_estimate` quotes a deliberately vague
range because throughput was extrapolated from a *different* workload — the interactive
renderer at a different point count and resolution. With a sample target and a rate
measured from this job's own first batches, that justification disappears: same scene,
same device, same workload. Replace the range with a converging point estimate (wide on
the first batch, tightening as batches land), computed from a rolling window so one stall
does not skew it, and carry the measured rate on `JobEvent::Progress` so the dialog and
the CLI do not each recompute it.

For automated callers, add a stable machine-parseable line to **stderr** (stdout keeps its
clean summary):

```
progress done=1234567890 total=2100000000 rate=1.6e9/s eta_s=142 phase=filling
```

## Suggested slice order

0. **The two fixes above** — encoder thread cap and partial-result-on-cancel — plus the
   **thread-count control in GUI and CLI**, and the shared trimmed `VERSION` constant.
   All tiny and independent.
1. **Supersample + filter + the two clamp/threshold fixes + 16-bit PNG.** Self-contained,
   biggest visible win, no new architecture. Ship and look at it.
2. **PNG metadata + render sidecar.** Independent of everything else; makes every
   subsequent experiment self-documenting, which is worth having *before* the long renders
   start. Depends on slice 0's `VERSION`.
3. **Timestamp queries**, then tune dispatch batch size against the numbers they give.
4. **Persistent accumulation histogram**, splat-only, stills-only, plus the explicit seed
   and the never-reset rule. Adds `overnight` and the converging ETA.
5. **Retonemap from the stored histogram** — cheap once slice 4 exists, and the thing that
   makes every remaining tonemap question answerable by looking instead of arguing.
6. **Tonemap gamma / threshold / vibrancy**, and settle decision 5 against slice 5.
7. **Density estimation** via the SAT approach.

Legs 1 and 2 are separable and 1 does not depend on 2 — which is the useful part, because
1 is small and immediately visible.

**One ordering question left.** I read "let's get that built first" as *before the CPU
work*, which is how it is written above. If you meant the accumulation histogram should
come before supersampling outright, say so and I will swap slices 1 and 4 — the two are
independent, so the order costs nothing either way.

My reason for putting supersampling first is only that it is the smaller change and the
one you can immediately look at and judge; accumulation is the more valuable change but
its payoff is invisible until you render something big enough to need it. If you'd rather
have the ceiling gone before anything else, that is an entirely reasonable call.

---

## Decisions

**1. Scene-file trace — SETTLED.** Sidecar only. Render parameters stay out of the scene
file and the scene format entirely.

**2. Thread count — SETTLED.** Needs a real parameter in both the GUI and the CLI, not
just a better default. See "Thread-count control" in leg 6.

**3. Version string for metadata — SETTLED.** `version.txt`, baked in at build time, which
is already the established mechanism. Promote the constant and trim it once, in const.

**4. CPU sample generation — SETTLED: not building it.** Build the GPU generate+accumulate
path first (leg 2). Hazel will then test on the **T490 laptop**, where the ratio is very
different — a UHD 620 is perhaps 1-2% of a GTX 1080, so three cores of *that* machine's
CPU are a materially larger fraction of its total, and a CPU path may earn its place as a
**fallback for GPU-poor machines**. Revisit with laptop measurements in hand; do not
design for it now. Hazel's standing ask stands, though: if there is genuinely parallel
work for the spare cores, use them — see leg 6 for the list (encoding, checkpoints,
progress), which is real but small.

**5. Adaptive per-image exposure normalization — STILL OPEN, leaning Apophysis-compatible.**

Hazel's instinct is to just do what Apophysis does, with the caveat that *either* way it
is hard to predict the effect before committing to a big render.

That caveat is the more interesting half, and I think it points at the actual fix. The
problem is not really which normalization rule is right — it is that there is currently no
way to see what a tonemap decision does without paying for the whole render. With the
accumulation architecture from leg 2, there is a cheap answer: **the tonemap is a pure
function of the finished histogram**, so re-tonemapping an existing accumulation is
essentially free, and a partial accumulation is already representative of the final one
for exposure purposes. That makes two things possible that would settle this properly:

- retonemap-without-re-rendering, so exposure/gamma/vibrancy can be judged interactively
  against a real (even partial) high-sample histogram;
- and, if checkpoints land, re-grading a finished render from its saved histogram rather
  than re-running it.

**Recommendation: defer the normalization choice until retonemapping exists, then decide
it by looking.** It is a one-line change either way at that point, and it stops being a
question anyone has to answer blind. If you'd rather not wait, Apophysis-compatible is the
right default — it is what your eye is trained on.

## Things I recommend against

- A `[last_render]` data block in scene files — *settled, not doing it.*
- Host-RAM accumulation on this machine — smaller than VRAM, with swap behind it.
- CPU chaos-game generation on the desktop, and therefore also CPU/GPU work-stealing,
  hand-rolled AVX2 across walkers, and f64 anywhere in this path. *Open again only as a
  laptop fallback, after leg 2 lands and the T490 is measured.*
- `env!("CARGO_PKG_VERSION")` anywhere in metadata — it says `0.1.0`, which is wrong.
- Lanczos as the default filter — rings around bright cores.
- ISOBMFF metadata boxes for `.avif`/`.mp4` right now.
- Occlusion in the high-quality path. `CRAFT.md` already states the renderer is pure
  emission and that haze is the only depth cue; occlusion would break the additive density
  model both supersampling and DE depend on. It is a different feature, not a smoothness
  lever.
