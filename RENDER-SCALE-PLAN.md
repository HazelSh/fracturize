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
   many cores are free.
2. **It needs the whole image in memory**, which a tiled render otherwise never
   has to have.
3. **139 Mpx PNG is an awkward handoff** for print tooling.

**Write tiled 16-bit TIFF.** TIFF has native tiled storage, so render tiles map
onto file tiles one-to-one: each is written the moment it finishes, each is
compressed independently and therefore *in parallel across `--threads`*, and
the full image is never resident. 16-bit TIFF is also the format a print shop
actually wants. PNG stays the default at ordinary sizes.

This also gives the save phase honest progress for free: tiles written of tiles
total.

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

**0 — Finish `RENDER-GUI-PLAN.md` slices 3-5 first.** The estimator, the
samples radio defaulting to accumulate, and the DE slider. This is where the
grain actually gets fixed (§1), it needs none of the machinery below, and
without it the tiled renderer would ship with the same 9.6-samples/px default
that caused the complaint.

**1 — `TilePlan`**: pure geometry. Given output, N, binding limit, texture
limit and a VRAM budget, produce tiles, halos, sub-frustums and pass grouping.
No GPU. Fully testable, and every seam bug in §4 is a test on this type.

**2 — Tiled accumulating still**, single pass, halos, sub-frustum cameras,
assembled into the existing in-memory image. Correctness first: a 1-tile render
and a 4-tile render of the same scene must agree to within sampling noise, and
that is the acceptance test.

**3 — Multi-pass**, with resident-tile grouping.

**4 — Tiled TIFF writer** (§6), streaming, parallel per-tile compression.

**5 — `MAX_SUPERSAMPLE` to 16** (§5), once 1-4 make it safe.

**6 — The progress ledger and the shared phase enum** (§7).

**7 — Per-pass checkpoint and the resume UI** (§8).

**8 — Compact histograms** (§4), if the memory is still the binding constraint.

---

## 10. Open questions

1. **Is 600 ppi firm?** A2 at 300 ppi fits today at 1x and needs three passes at
   2x. It changes the size of this project a lot (§3).
2. **Colour handoff for print.** Everything here writes linear or sRGB RGB. A
   print shop may want an ICC profile embedded, or CMYK separation. Out of
   scope as written, but worth knowing before a poster gets sent.
3. **Is the GTX 1080 the render machine?** Every number here is measured on it.
   The binding limit in particular is a per-adapter property, and a card with a
   4 GB binding limit would halve every pass count in §4.
