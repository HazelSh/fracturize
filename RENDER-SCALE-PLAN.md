# Render scale plan — posters, 16x, and hours of compute

Three asks: render at 8K and beyond (A2 at 600 ppi), supersample at 16x, and
have somewhere useful to spend hours of GPU time. Plus: 4K render time is
dominated by saving, and every phase needs to report progress.

One of those four is a misdiagnosis, one is blocked by a single hard number,
and two are straightforwardly buildable. Taking them in that order.

---

## 1. Supersampling will not fix the grain

This is the important correction, and everything else in the plan is easier
once it is out of the way.

**Supersampling is an anti-aliasing tool with exact brightness invariance.** It
was measured during the render-quality work at **0.008%** change in noise —
which is not "a small improvement", it is zero to within measurement error, and
it is zero *for a reason*. Depositing S samples into one pixel and depositing S
samples across N² sub-texels that then get averaged are the same sum over the
same samples. The relative noise of a Poisson count is 1/sqrt(S) either way.
Supersampling moves *where* the samples are recorded, never how many there are.

What fixes grain is samples. Measured on `lacewing` at 1080p, by
independent-seed differencing — rendering the same frame twice with
`--chaos-seed 0` and `12345` and taking the RMSE between them, which is the
only metric that separates sampling noise from real fine structure:

| `--spp` | noise (RMSE) | chaos time | vs. the dialog's default |
|---|---|---|---|
| ~9.6 *(render-job dialog default at 1080p)* | ~0.067 *(extrapolated)* | 0.04s | — |
| 100 | 0.0206 | 0.37s | 3.2x cleaner |
| 1,000 | 0.00685 | 3.40s | 9.7x cleaner |
| 10,000 | 0.00261 | 31.6s | **25x cleaner** |

Time is exactly linear in `--spp`. Noise falls 3.01x then 2.62x per decade,
against the 3.16x of ideal 1/sqrt(N) — the shortfall is measuring RMSE after a
nonlinear tonemap, not a defect in the accumulation.

**So the grain is not a missing feature.** `--spp` has had no ceiling since
slice 4b. The reasons it is not being used are all interface:

1. The render-job dialog has no accumulation control at all — its default
   works out to **9.6 samples/px at 1080p and 2.4 at 4K**, because the ring
   path's density *falls* as the output grows (measured in
   `RENDER-GUI-PLAN.md` §9). That is the single biggest source of the grain.
2. `--effort` stops at `huge` = 10,000 spp, which at 4K is about five minutes.
   Nothing in the interface suggests that 100,000 is a thing you may type, and
   it is.
3. Nothing anywhere says what a sample count buys. The table above is the
   first time that has been written down in user-facing terms.

Fixing 1 is `RENDER-GUI-PLAN.md` slice 4, already planned. That plan's remaining
slices are now **the highest-value work in this document**, ahead of everything
below, because they are what turn "renders finish in seconds" into "renders can
be asked for real work".

**Where supersampling does earn its keep** is the finest filaments — the `rib`
transform in `lacewing` is exactly the material that aliases. It is worth
raising the cap. It is not worth raising it to fix grain, and at print
resolution it is worth very little (§3).

---

## 2. The wall: one number

The histogram is a **storage buffer**, not a texture, at **32 bytes a texel**.
So the binding limit governs, and on the GTX 1080 it is:

```
max_storage_buffer_binding_size:  2.15 GB   ->  67.1 M texels, total
max_texture_dimension_2d:        32768
```

67.1 M texels is the entire budget for one accumulating render. Divided by the
supersample factor's N², it is the largest picture that can be rendered at all:

| supersample | texels/px | largest output | what that is |
|---|---|---|---|
| 1x | 1 | 67.1 Mpx | 8K fits; A2@600ppi does not |
| 2x | 4 | 16.8 Mpx | 4K fits; 8K does not |
| 4x | 16 | 4.19 Mpx | 1440p fits; 4K does not |
| 8x | 64 | 1.05 Mpx | about 1280x820 |
| **16x** | 256 | **262 kpx** | **about 512x512** |

That last row is the whole answer to "why can't I have 16x". Today 16x is a
thumbnail feature — and `MAX_SUPERSAMPLE` is 4 anyway, so it is not reachable
at any size.

**A2 at 600 ppi** is 9921 x 14032 = **139.2 Mpx**, which needs 4.45 GB at 1x —
over the binding limit by 2x before any supersampling at all.

| target | 1x | 2x | 4x | 16x |
|---|---|---|---|---|
| 4K | 0.27 GB | 1.06 GB | 4.25 GB | 68 GB |
| 8K | 1.06 GB | 4.25 GB | 17.0 GB | 272 GB |
| A2 @ 600ppi | 4.45 GB | 17.8 GB | 71.3 GB | 1.14 TB |

Both edges of that table were checked against the machine rather than
calculated and trusted. **8K at 1x renders today** — 7680x4320, 11.0s wall. And
4K at 4x refuses, with the arithmetic already in the message:

```
accumulation histogram needs 4.2 GB (15360x8640 texels x 32 bytes)
but this GPU binds at most 2.1 GB — render smaller, or lower --supersample
```

So the ceiling is real, it is where the arithmetic says, and it already
announces itself properly. What it cannot do is offer a way through, because
there isn't one yet.

So: **tiling is not an optimisation here, it is the only way any of this
happens.** And note the bottom-right corner — 16x at poster size is 35.6
*billion* texels. That is not a thing to build toward; see §3.

---

## 3. What resolution and supersampling are each for

Worth stating plainly because the two asks overlap more than they look.

At **600 ppi a pixel is 42 microns**, and the eye resolves about 100 microns at
reading distance. A 600 ppi print is already sampling ~2.4x finer than anyone
can see. Supersampling on top of that buys spatial detail the paper cannot
show and the eye could not find if it did — 16x at A2/600 would be sampling at
an effective 9600 ppi.

So the two asks want different answers, and it is worth being explicit:

* **Poster at 600 ppi** — tile it, supersample **1x or 2x**, and put every
  spare minute into `--spp`. The grain is the real enemy at this size and
  samples are the only thing that touch it.
* **16x supersampling** — worth having, and worth having at screen and
  moderate-print sizes where an output pixel is genuinely bigger than the
  structure. Tiling makes 1080p and 4K reachable at 16x.

Also worth checking before committing to the larger job: **A2 at 300 ppi is
34.8 Mpx and fits the histogram *today* at 1x.** 300 ppi is the normal
fine-art print standard; 600 is a line-art number. If 300 is acceptable for a
given piece, that piece needs none of this.

---

## 4. Tiling

### The plan

Split the *output* into tiles, each with its own histogram sized under the
binding limit. Per tile: accumulate, density-estimate, downsample, tonemap,
encode, write. Assemble by writing tiles straight into the output file (§6),
never materialising the whole image.

Two constraints set the tile size, and both are per-tile once tiling exists:

* `tile_w * tile_h * N² * 32 <= binding limit`
* `tile_w * N <= 32768`, same for height

### Passes, not tiles, is the cost

The binding limit is **per binding**, and this GPU has 8 GB of VRAM. So roughly
**three** full-size histograms can be resident at once, and the thing that
actually costs time is:

```
passes = ceil(total histogram bytes / (binding limit * resident tiles))
```

| target | N | histogram | passes @ 3 resident |
|---|---|---|---|
| 4K | 4 | 4.25 GB | 1 |
| 4K | 16 | 68 GB | 11 |
| 8K | 2 | 4.25 GB | 1 |
| 8K | 4 | 17.0 GB | 3 |
| A2 @ 600 | 1 | 4.45 GB | 1 |
| A2 @ 600 | 2 | 17.8 GB | 3 |
| A2 @ 600 | 4 | 71.3 GB | 12 |

### Why each pass re-runs the chaos game

A single chaos run at `--spp S` deposits S samples per pixel *everywhere* — the
samples distribute themselves spatially, so one run feeds every tile at once
and is exactly the right amount of work for all of them. Only histogram
residency stops us using it that way. The obvious fix is to bin the points once
and route each to its tile, so the chaos game runs once instead of `passes`
times.

**That trade loses, and it is worth writing down why.** The point spool is
`spp * pixels * 16` bytes, against a histogram of `pixels * N² * 32` — so
spooling is bigger whenever `spp > 2N²`, which is essentially always at the
sample counts §1 is arguing for. A2 at 1000 spp spools **2.2 TB** where the
histogram is 17.8 GB. Streaming histogram tiles instead is worse again: it
moves the whole 17.8 GB per lap, thousands of times over.

Meanwhile the chaos game generates ~270-660 M samples/s (measured; it falls as
the histogram grows and splatting turns memory-bound). Generating a sample is
*cheaper than moving one from disk*. So:

> Re-running the chaos game per pass is not the naive option that a later
> version replaces. It is the fast one, and the simple one, and they are the
> same one.

A2 at 1000 spp, 2x supersampled: 139 G samples per pass, three passes, call it
**25-40 minutes**. At 10,000 spp it is a few hours — which is the ask.

### Two things that will produce visible seams if missed

1. **Halos.** Density estimation reads up to `MAX_RADIUS_PX * N` texels away and
   builds a mip pyramid with the same footprint; the downsample filter reads
   `filter_radius * N`. A tile that renders only its own texels gets wrong
   values along every edge — a grid of seams across the poster, which is the
   worst possible failure at this size. Tiles need a halo of
   `MAX_RADIUS_PX + filter_radius` — and since both scale with N, that is
   **~8 *output* pixels per side regardless of supersampling.** Negligible
   overhead, fatal if forgotten.
2. **One camera, sub-frustums.** Every tile is a window onto the *same* camera,
   not its own camera — unlike the contact sheets, which really do move the
   camera per tile. Critically, `Sampling` must keep the **full output height**,
   not the tile's: its own doc already warns that when `screen_height`, the
   near-field size cap and `use_point_primitives` disagree about the target,
   the feature silently cancels for part of the picture. Per-tile point sizes
   would be exactly that bug, made visible as tiles of differing texture.

### A free 2x: compact histograms

32 bytes a texel is `4 x u64`, chosen for exactness. `4 x f32` at 16 bytes
holds sample counts to ~1.7e7 per texel before the mantissa stalls — well past
anything but the most extreme `--spp`. A `compact` histogram mode halves the
memory, halves the passes, and nearly halves the wall clock for large renders.
Worth offering with `exact` as the default and a clear statement of where
compact stops being safe.

---

## 5. Raising `MAX_SUPERSAMPLE` to 16

A constant change, but only safe once tiling exists — both guards it would blow
past (`check_fits` on the texture limit, `Accumulator::new` on the binding
limit) become per-tile checks.

Two things to verify rather than assume:

* `MAX_LEVELS` is 7, and DE wants `log2(6N)` levels — at N=16 that is ~6.6, so
  7 is just enough. At 32x it would not be. Worth an assertion rather than a
  silent truncation of the pyramid.
* The N² fill cost is real: 16x is **64x the fill** of 4x. At poster sizes that
  is the difference between hours and weeks, which is the practical reason §3
  argues for 1-2x there.

---

## 6. Saving

Measured, and it is worse than "dominant" — at 8K it is nearly the whole job:

| output | chaos | save | save as % of wall |
|---|---|---|---|
| 4K, `--effort medium` | 3.10s | 3.26s | 49% |
| **8K, `--spp 10`** | **1.54s** | **8.88s** | **82%** |

That is 0.27 s/Mpx, linear in pixels, and independent of how long the render
itself took — so it gets *proportionally worse* the cheaper the render. At A2
it is ~37s and **1.11 GB of image held in RAM**.

Three separate problems, one answer:

1. **PNG deflate is a single serial stream.** It cannot use `--threads` however
   many cores are free — this is the big one, and it is what makes saving 82%
   of an 8K render.
2. **It needs the whole image in memory**, which a tiled render otherwise never
   has to have.
3. **139 Mpx PNG is an awkward handoff** for print tooling.

**Not on that list: bit depth.** An earlier draft argued for TIFF partly on
16-bit grounds, which was wrong — `--bit-depth 16` already writes 16-bit PNG
and has for some time. TIFF earns its place on parallelism and streaming alone,
and the case is strong enough without the bad argument.

**Write tiled TIFF.** TIFF has native tiled storage, so render tiles map onto
file tiles one-to-one: each is written the moment it finishes, each is
compressed independently and therefore *in parallel across `--threads`*, and
the full image is never resident. It is also what a print shop wants. PNG stays
the default at ordinary sizes, at either depth.

(A parallel PNG encoder is possible in principle — deflate permits
independently compressed blocks concatenated into one stream, which is how
`pigz` works. It would fix problem 1 but not 2, and TIFF gives both for less
work. Worth knowing the option exists so the choice is an informed one.)

This also gives the save phase honest progress for free: tiles written of tiles
total.

---

## 6b. Dither, because a converged render is what bands

8-bit output bands, and it is measurable without any reference image. On the
100,000-spp `lacewing`:

* a 300x200 patch of smooth material carries **1,134 distinct colours at 8-bit
  against 56,747 at 16-bit** — a 50x collapse of the tonal range that is
  actually there;
* a 480 px scanline through that material uses **30 codes**, with runs of up to
  **14 identical pixels**. A 14-pixel run of one code across continuously
  varying material is a band, by definition.

**Here is the part worth noticing: this is a symptom of converging.** Sampling
noise is itself a dither. At 1,000 spp the render's own grain is several
quantisation steps wide, so it randomises every rounding decision for free and
no banding is possible. As `--spp` climbs, that self-dither shrinks below one
step — around the 30,000 spp crossover in §11 — and the quantisation structure
it was hiding emerges. **The banding appears exactly when the render finally
gets good.** Which is precisely the regime this whole plan exists to reach.

### The design

**Dither has to live in the tonemap shader.** For `--bit-depth 8` the tonemap
writes into an `Rgba8UnormSrgb` render target and the *hardware* does the
rounding — the CPU-side path only ever sees bytes that have already been
quantised, so there is no later point at which to intervene. One added term
before the write, and a noise source.

**Triangular PDF, one LSB.** Uniform (RPDF) dither decorrelates the error from
the signal but leaves the noise floor signal-dependent, which is audible as
"pumping" in audio and visible as breathing in a gradient. TPDF — the sum of
two uniform draws, spanning ±1 LSB — decorrelates *and* removes that
modulation, and is the standard answer.

**Be honest about the trade: dither does not reduce error, it changes its
character.** Round-to-nearest has an RMS error of 0.289q; TPDF has 0.5q. The
error gets *larger* and the picture gets *better*, because structured error at
0.289q is a visible contour and unstructured error at 0.5q is invisible grain
below the noise the eye brings to it. Nothing here recovers information the
file cannot hold — §11's crossover is unchanged as a statement about
information. What changes is that past it the output degrades gracefully
instead of into contours.

**White noise is enough for print, and blue noise is a screen refinement.** A
hash of pixel coordinate and render seed gives white TPDF in one line with no
asset. A void-and-cluster blue-noise mask pushes the error into high spatial
frequencies where the eye is least sensitive, which is meaningfully better on a
monitor at 1:1 — and at 600 ppi is beside the point, since a dither grain is 42
microns and invisible whatever its spectrum. Start white; add blue noise when
the 1:1 preview (§13) makes it worth judging.

**Two things not to dither.** The 16-bit path, where one step is 1/65535 and
below anything the render or the eye contains — dithering it would add noise to
buy nothing. And alpha, which is coverage rather than colour, matching what the
existing 16-bit path already does by leaving alpha out of the sRGB encode.

---

## 7. Progress

The current model has two bars and four phase strings matched by literal text
(`stage_of` in `ui/render_job.rs`). A tiled accumulating render has a phase
tree, not a phase list: passes contain tiles contain laps, and then a
per-tile finish and write.

Proposed: **a weighted work ledger.** Every phase declares an estimated cost in
the same measured units the estimator already uses (§4's throughput,
`*_SECS_PER_PIXEL`, a disk rate). Overall progress is
`sum(done weights) / sum(all weights)` — one bar, monotonic, never resets, and
correct even though the phases are wildly unequal. Under it, a line naming what
is happening now: `pass 2/3 · tile 7/12 · lap 340/1160`.

This replaces `stage_of`'s string matching, which is coupling
`ui/render_job.rs` to literal strings in `offline.rs` across a module boundary
with no shared type — and which would need a new arm per phase added here. A
shared enum is the fix, and it makes a missed phase a build error rather than a
bar that silently reads "not started".

**The still path must grow an encode bar.** Its absence is currently justified
in a comment — "a still has no encode phase — the PNG write is fast and never
reports progress at all" — and at 139 Mpx that is simply false. It is a minute
of unreported work at the end of a multi-hour job.

---

## 8. Reversal: checkpoint and resume are now required

`RENDER-GUI-PLAN.md` §7 argued for surfacing `--checkpoint` but building no
resume UI, on the grounds that the GUI process persists and Pause already
frees the GPU. **That reasoning was sound for renders that take minutes and
does not survive renders that take hours.** A multi-hour render that cannot
survive a crash, a driver reset, or a reboot is not a feature anyone can rely
on.

Tiling also gives checkpointing a natural granularity it did not have:
**checkpoint after each pass**, and a resume skips completed tiles outright
rather than re-accumulating them. That is cheaper *and* simpler than the
histogram-level resume the CLI does today.

So: build the resume UI, and treat "can this job be interrupted and continued"
as a requirement of the tiled path rather than a nicety.

---

## 9. Slices

**0 — Finish `RENDER-GUI-PLAN.md` slices 3-5 first.** *(done: `9be7a9b`, `523089e`, `4adb407`)* The estimator, the
samples radio defaulting to accumulate, and the DE slider. This is where the
grain actually gets fixed (§1), it needs none of the machinery below, and
without it the tiled renderer would ship with the same 9.6-samples/px default
that caused the complaint.

**1 — `TilePlan`**: pure geometry. Given output, N, binding limit, texture
limit and a VRAM budget, produce tiles, halos (sized from the actual settings,
§10), sub-frustums and pass grouping. No GPU. Fully testable, and every seam
bug in §4 is a test on this type — as is every row of §10's overhead table.

**2 — Tiled accumulating still**, single pass, halos, sub-frustum cameras,
assembled into the existing in-memory image. Correctness first: a 1-tile render
and a 4-tile render of the same scene must agree to within sampling noise, and
that is the acceptance test.

**3 — Multi-pass**, with resident-tile grouping.

**4 — Compact histograms** (§4). Promoted from last to here: it halves the
memory, and therefore the passes, and therefore the wall clock — §11's
six-hour poster becomes four. It is the cheapest large win in the plan and
everything after it is measured on top of it.

**4b — TPDF dither on the 8-bit path** (§6b). Out of order because it is small,
independent of every other slice, and fixes a defect that is *already visible*
in renders being made today — it does not need tiling, TIFF or anything else to
land. One term in the tonemap shader.

**5 — Tiled TIFF writer** (§6), streaming, parallel per-tile compression.

**6 — Preview writes** (§13). Small, and it is what makes a multi-hour render
something a person can live with rather than bet on.

**6b — The two-scale preview in the dialog** (§13): fit view, 1:1 loupe, and
the fit view as the loupe's picker. Promoted next to the preview writes it
shares its readback with, rather than left to slice 10.

**7 — The progress ledger and the shared phase enum** (§7).

**8 — Per-pass checkpoint and the GUI resume** (§8, §13).

**9 — `MAX_SUPERSAMPLE` to 16** (§5), once the rest makes it safe. Late
deliberately: §3 and §11 both say it is not what the poster needs, so it should
not block the poster.

**10 — Blue-noise dither mask** (§6b), if the 1:1 preview from 6b shows white
dither is worth improving on. Deliberately last and deliberately conditional:
at print resolution it changes nothing, so it should be judged on a monitor
against the thing it is supposed to improve.

Then, as its own investigation rather than a slice: **denoising the grade
buffer** (§12a), starting with a 100,000-spp ground-truth render of `lacewing`
to score against.

---

## 10. The halo, and how small a machine can tile

The halo is a **fixed cost in output pixels** (~8 per side) while the tile it
sits around shrinks as **N²** for a given memory budget. So the overhead is
negligible everywhere except one corner — high supersampling on a small
binding limit — where it becomes the dominant cost:

| budget | 1x | 2x | 4x | 16x |
|---|---|---|---|---|
| 2.1 GB *(GTX 1080)* | 0.4% | 0.8% | 1.6% | 6.6% |
| 512 MB | 0.8% | 1.6% | 3.3% | 14.1% |
| 128 MB | 1.6% | 3.3% | 6.7% | **31.5%** |
| 32 MB | 3.3% | 6.7% | 14.1% | **80.7%** |

At 32 MB and 16x the tile is 46 output pixels inside a 62-pixel rendered
square: nearly half the work is halo. That is the failure mode to design
against.

**The fix is to size the halo from the actual settings, not the maximum.** The
6 px is entirely density estimation's reach; the filter contributes 0.5-2. With
DE off the halo is ~2 px, and the same table becomes:

| budget | 1x | 2x | 4x | 16x |
|---|---|---|---|---|
| 128 MB | 0.4% | 0.8% | 1.6% | 6.7% |
| 32 MB | 0.8% | 1.6% | 3.3% | 14.1% |

80.7% → 14.1% for a change with no downside: a halo wide enough for a pass that
is not running is pure waste. So `halo_px = (de.is_off() ? 0 : MAX_RADIUS_PX) +
filter_radius`, and `TilePlan` reports it so the cost is visible rather than
discovered.

**Full-width strips** are the other lever, and they compose with tiling rather
than replacing it: a strip that spans the image needs no left or right halo,
because those edges are the picture's own. Overhead becomes linear in the
strip's height instead of quadratic in a tile's side. The catch is that the
downsample and DE intermediates are *textures*, capped at 32768, so a strip
only works while `width * N <= 32768` — at A2 that is 1x and 2x, and not 4x.
Which is fine, because 1x and 2x is what §3 argues for at poster size anyway.

---

## 11. spp or pixels? — spp, and the arithmetic is not close

Your instinct is right, and it is worth pinning down because it decides where
every marginal hour goes.

At **600 ppi a pixel is 42 microns**, against roughly 87 microns for one arcmin
of visual acuity at reading distance. The print is already sampling ~2x finer
than the eye can resolve. Doubling to 1200 ppi quadruples the pixel count for
detail that physically cannot be seen. Doubling `--spp` divides the noise by
1.41, and the noise is the thing you are actually looking at.

At a fixed compute budget the total is `spp x pixels`, so this is a genuine
trade and it is lopsided: **pixels are already past the eye, noise is not.**

So the target is not "more pixels than A2 at 600". It is **A2 at 600 with
enough samples**, and that is a number we can name:

| `--spp` at A2 600ppi | noise (RMSE) | in 8-bit levels | passes @2x | wall |
|---|---|---|---|---|
| 1,000 | 0.00665 | 1.70 | 3 | ~40 min |
| 10,000 | 0.00211 | **0.54** | 3 | ~6 hours |
| 100,000 | 0.000667 | 0.17 | 3 | ~60 hours |

Measured, not extrapolated — including the 100,000 row, which is a real
five-minute render at 1080p (17,280 laps, and the accumulation holds up
exactly: 3.157x then 3.155x per decade, against 1/sqrt(10) = 3.162).

The useful ceiling has a sharp edge. **8-bit quantisation contributes 1.0 level
of its own noise**, so sampling noise crosses below the file's own noise at
about **30,000 spp** — past there an 8-bit output cannot hold what the render
has, and more samples buy nothing you can save. 10,000 spp is therefore the
right target for a print: about six hours, half a quantisation step of noise,
and the last point on the curve where more time still shows up in the file.

Going further needs 16 bits — which `--bit-depth 16` already writes, so this is
a reason to *use* that flag rather than a reason for any new format. And it is
now mandatory for *measuring* noise at all, since an 8-bit file inflated these
very figures by 24% at 10,000 spp and 108% at 100,000. See `AGENTS.md`, where
that correction is recorded.

Note this crossover is about *information*, and §6b's dither does not move it.
What dither changes is that past the crossover an 8-bit file degrades into
invisible grain rather than into visible contours — so 10,000 spp stays the
right target for an 8-bit deliverable, and 16-bit is what makes going past it
worth the hours.

(`--spp` is per *output* pixel, so these figures carry over from the 1080p
measurements in §1 unchanged — which is exactly why the tier system was built
on samples/px instead of a point count.)

**Compact histograms (§4) move this from 3 passes to 2**, taking the six-hour
job to about four. That makes it the highest-leverage single change in the
plan after tiling itself.

---

## 12. Past brute force: where "better" comes from after that

1/sqrt(N) is a hard wall. Past 10,000 spp, ten times cleaner costs a hundred
times the time — 60 hours buys 3.16x, 600 hours buys 10x. If "bigger, better,
more beautiful" is to keep going after §11, it does not go there.

Three directions, honestly ranked.

### a) Denoise the grade buffer — the big one

`--grade-out` already writes exactly the right input: the pre-tonemap linear
density. `--retonemap` already re-processes it without re-rendering. A denoise
pass slots into that seam with no change to the renderer at all, and inherits
the property that makes the grade sweep useful — **render once, try sixteen
settings.**

Production renderers get the equivalent of 10-100x more samples this way, which
is one to two decades of the table in §11 for free. The risk is specific and
real: a flame image has genuine structure at the same scale as its noise, and
an aggressive denoiser eats filaments — precisely the `rib` material `lacewing`
was built to expose.

But that risk is *measurable here in a way it usually isn't*, because §11 says
a ground-truth render is affordable. Render `lacewing` once at 100,000 spp,
keep it as the reference, and then any denoiser applied to a 1,000-spp buffer
can be scored against it directly — with the same independent-seed method that
produced every number in this document. A denoiser that eats filaments will
show it immediately as error against the reference, not as an argument.

Start with a guided/edge-aware filter written here (no dependency, fully
understood) and only reach for something like OIDN if the simple thing plateaus.

**The reference already exists**, and it is cheap. `lacewing` at 1080p, 100,000
spp, two independent seeds — five minutes each, noise 0.000667, a sixth of a
quantisation step. Any denoiser applied to the 1,000-spp buffer can be scored
against it directly, and "did it eat the `rib` filaments" becomes a number
rather than an argument.

Regenerating it (`reference/` is gitignored — five minutes is cheaper than 22 MB
of history):

```sh
for seed in 0 12345; do
  fracturize -s scenes/lacewing.toml --splat --width 1920 --height 1080 \
    --spp 100000 --chaos-seed $seed --supersample 1 --bit-depth 16 \
    --grade-out reference/lacewing-1080p-100k-$seed.fgrade \
    -r reference/lacewing-1080p-100k-$seed.png
done
```

Keep the `.fgrade` buffers: they are the linear, pre-tonemap density, which is
what a denoiser should operate on and what `--retonemap` re-grades in
milliseconds.

### b) Stratified or low-discrepancy transform selection

The chaos game picks its next transform from a uniform random draw. A
low-discrepancy sequence can do better than 1/sqrt(N) in some regimes — call it
1/N^0.6 — which is modest but compounds over a six-hour render. Cheap to try,
cheap to abandon, and measurable with the existing tooling.

### c) Importance-sampled chaos — research, not a plan item

The grain is worst in the sparse regions *by construction*: the chaos game
visits in proportion to the attractor's natural measure, so low-measure regions
get few samples and no amount of uniform sampling changes their relative share.
Sampling with modified transform probabilities and carrying a compensating
weight would let samples be steered — and is mathematically delicate, since
weights can explode and a bad weight is a bright wrong pixel rather than a
noisy one. Worth knowing it is the principled answer; not worth committing to.

**Note that density estimation is not on this list.** DE is the denoiser we
already have, and it is a *low-spp* tool: measured 34% benefit at 100 spp
falling to 7.5% at 1000. At the sample counts §11 argues for it will be doing
almost nothing, which is a reason to finish its GUI slice for ordinary renders
and not to expect it to carry a poster.

---

## 13. Living with a six-hour render

An hours-long render is a different kind of object from a minutes-long one, and
three things follow.

**Preview as it goes.** The accumulating render is an anytime algorithm —
exposure is normalised by what has actually accumulated, so the histogram at
40% is the same picture, noisier. That means a preview PNG can be written every
pass (or every few minutes) at essentially no cost, and it changes the
experience completely: you can look at hour one and decide whether hour six is
worth having. Without it, a six-hour render is a six-hour bet.

**Watch it in the dialog — at two scales, and both are load-bearing.**

A fit-to-pane view of an A2 render is a **25x downscale**, and downscaling
averages pixels together, which is precisely the operation that *destroys the
evidence of grain*. A noisy render and a converged one look identical in the
fit view. So the fit view can answer "is the composition right, is the exposure
right, has it got as far as the left edge yet" and it structurally cannot
answer "is it still grainy".

A 1:1 view answers that and nothing else: it shows 0.3% of an A2 frame.

So neither is a nice-to-have on top of the other — they answer disjoint
questions, and a render this long needs both answered. The pairing is the
standard loupe interaction: **the fit view is also the picker**, and clicking or
dragging on it moves the 1:1 inspection point, with a rectangle on the fit view
showing where the loupe is.

Data flow fits the existing `JobEvent` channel without straining it. Per
preview the job sends two small readbacks: a thumbnail (say 512 px wide) and a
1:1 crop (say 512x512) around a point the UI last asked for. Both are tiny next
to the histogram, and the crop centre travels the other way as a request. No
new plumbing shape — the job already streams phase, progress and log lines this
way.

**And for a tiled render the preview is the progress bar.** Passes complete
whole tiles, so the image fills in as a jigsaw: which tiles are done, and how
they look, in one glance. That is more information than a percentage, and it is
free — the tiles are being written anyway.

A cadence of once per pass, or every few minutes for a long single pass,
whichever is less frequent. The point is to be able to walk past the machine
and know, not to animate.

**Survive the machine.** Per-pass checkpointing and the GUI resume from §8,
which you have approved. At six hours the relevant failure is not a crash but a
reboot, and a render that cannot cross one is not really a six-hour feature.

---

## 14. Open questions

**Settled:** scaling up is firm, with A2 at 600 ppi as the anchoring real use
case. Tiled TIFF is the output format. The GUI gets a resume. The 8 px halo is
accepted, with §10 planning for how it scales down.

Still open:

1. **Colour handoff for print.** Everything here writes linear or sRGB RGB. A
   print shop may want an ICC profile embedded, or CMYK separation. Out of
   scope as written, but worth knowing before a poster is actually sent — it is
   the one thing in this document that could invalidate a six-hour render after
   the fact.
2. **Is the GTX 1080 the render machine?** Every number here is measured on it.
   The binding limit is a per-adapter property, and a card with a 4 GB binding
   limit would halve every pass count in §4 and §11.
3. **How small a machine has to tile at all?** §10 says the halo stays cheap
   down to a 32 MB budget except at 16x. Worth knowing whether the T490 is
   expected to render posters or only to explore, because "explore here, render
   there" makes the small-budget corner of that table irrelevant.
