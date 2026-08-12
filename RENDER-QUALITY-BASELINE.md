# Render quality: the measured baseline

Measurements taken 2026-08-10 on the desktop (GTX 1080, 8 GB VRAM, driver 580.173.02;
15 GB system RAM, 4 cores), against `scenes/nautilus.toml` at 1920x1080, splat renderer,
from the release binary. Every number below is measured, not estimated, except where
marked.

## The findings, in three lines

1. **The GPU already generates far more samples than the image keeps.** The chaos game
   runs at ~1.6 billion samples/second; the ring buffer throws all but the last ~100M away.
2. **But raw sample count is not the main thing wrong with the picture.** Past ~30
   samples/pixel the image stops getting visibly better, and what remains is not shot
   noise — it is pixel quantization, because every point deposits into exactly one pixel
   with no reconstruction filter.
3. **Supersampling plus a downsample filter is the single biggest visible win, and it is
   nearly free.** Measured: 0.50 s vs 0.35 s for a dramatically better image at the same
   sample count.

The two findings are complementary, not competing: supersampling multiplies the number of
buckets by N², which is exactly what makes the sample ceiling start to bite. See
"How the two fit together" at the end.

## What `--effort ultra` actually is

`Effort::Ultra` (src/main.rs:1049) is `(100_000_000 points, 256 accumulate frames)`.

```
$ fracturize --scene scenes/nautilus.toml --render out.png \
    --width 1920 --height 1080 --effort ultra --splat
Timing: setup 0.15s | chaos fill 0.15s | render 0.05s | encode+save 0.01s | total 0.35s
```

The highest quality setting the renderer offers completes in **0.35 seconds**. Not because
it is efficient — because it stops early. It is not time-limited, it is storage-limited.

## Why `--accumulate` does not accumulate

The chaos game writes into a **circular** point buffer.
`PointCompute::valid_point_count()` (src/gpu/points/compute.rs:416) clamps to
`buffer_capacity`. Accumulate frames run *after* warmup, so they overwrite points already
in the ring. The render pass then draws the buffer once.

So for a still image:

> total distinct samples contributing to the picture == point buffer capacity.
> `--accumulate` changes which samples survive, never how many.

At `ultra` that ceiling is 100M samples, costing a 1.6 GB point buffer
(16 bytes/point) — already pressed against the per-buffer allocation limit. The ceiling
cannot be raised by turning the existing dials up; the dials are not attached to it.

Measured directly — accumulate frames cost real time and change nothing that survives:

| accumulate | chaos fill | samples generated | samples kept |
|-----------:|-----------:|------------------:|-------------:|
|        256 |      0.14s |             1.1e8 |         1e8  |
|       4096 |      0.45s |             6.4e8 |         1e8  |
|      16384 |      1.45s |             2.1e9 |         1e8  |

The last row computes 2.1 billion samples in 1.45 s and discards 95% of them.

## Measured throughput

Marginal chaos-game rate, from the 4096 → 16384 delta (12288 frames x 131072 points
per frame in 1.00 s):

- **chaos game: ~1.6e9 samples/sec**

Splat rasterization, from the point-count sweep (render column):

| points | render |
|-------:|-------:|
|     4M |  0.02s |
|    25M |  0.03s |
|   100M |  0.06s |

- **splat raster: ~2e9 points/sec** above a ~15 ms fixed pass cost

Pipelined (generate *and* splat every sample), expect a combined **~0.9e9 samples/sec**.

## The gap to Apophysis, quantified

flam3/Apophysis "density" (quality) is samples per output pixel. At 1920x1080
(2.07e6 pixels):

| density | samples needed | time at 0.9e9/s | fracturize today |
|--------:|---------------:|----------------:|------------------|
|      48 |          1.0e8 |           0.1 s | **the ultra ceiling** |
|     500 |          1.0e9 |           1.2 s | out of reach |
|   1 000 |          2.1e9 |           2.3 s | out of reach |
|  10 000 |          2.1e10 |            23 s | out of reach |
| 100 000 |          2.1e11 |           230 s | out of reach |

**`--effort ultra` is equivalent to Apophysis density ~48.** Hazel's read that it "is not
doing what 100k density does" is correct, and the shortfall is a factor of about **2000x**.

The encouraging half: density 100k is roughly **four minutes** of work on this GPU. The
hardware is not the constraint and never was. The samples are being computed and thrown
away.

## More samples stop helping sooner than expected

A point-count sweep at 800x600, same scene, same everything else. "spp" is samples per
output pixel. Crops are in `renders/quality-study/samples-1M-4M-16M-100M.png`.

| points | spp | what it looks like |
|-------:|----:|--------------------|
|     1M | 2.1 | heavy speckle, obviously undersampled |
|     4M | 8.3 | speckle largely gone |
|    16M | 33  | smooth; more fine filaments resolved |
|   100M | 208 | **barely distinguishable from 16M** |

Between 33 and 208 samples/pixel there is very little visible improvement. Whatever is
still wrong with the image at 208 spp is not shot noise, and no amount of extra sampling
fixes it.

## What is actually wrong: no reconstruction filter

At these settings every point is deposited by the native 1px point-primitive path — one
point, one pixel, no kernel, no filtering (`use_point_primitives`, decided at
offline.rs:723 and three sibling sites). The image is a raw histogram read out through a
log curve. That is why it stays hard-edged and crunchy however many samples land in it.

Test, done today with no code changes: render the same scene at **3200x2400** with the
same 100M points, then average each 4x4 block in linear light down to 800x600. That is
exactly what a 4x supersample plus a box reconstruction filter would do.

| | samples/output px | wall clock | result |
|---|---:|---:|---|
| native 800x600 | 208 | 0.35 s | crunchy, aliased |
| 3200x2400 -> 4x down | 208 | **0.50 s** | dramatically smoother, *more* legible detail |

Side by side in `renders/quality-study/native-vs-4x-supersampled.png` (left native, middle
4x box-downsampled, right with an additional 0.4px gaussian).

Note what this rules out: the supersampled version has the **same total sample count**,
and only 13 samples per *accumulation bucket* versus 208. It still wins, decisively. So in
this regime reconstruction matters more than sample count, and it costs 0.15 s.

## Adapter limits, measured

From `vulkaninfo` on the GTX 1080 — these correct the values usually assumed:

| limit | value |
|---|---|
| `maxStorageBufferRange` | 4294967295 (**4 GiB - 1**, not the common 2 GiB) |
| `maxMemoryAllocationSize` | 0xffe00000 (**~3.998 GiB**) |
| `maxImageDimension2D` | **32768** |

So a single storage binding can hold ~4 GB, and a 16384x16384 accumulation texture is
well within the texture-dimension limit. The binding cap only starts to bite at
4096x4096 output with 4x supersampling (268M texels x 16 B = 4.30 GB), which is also
just past `maxMemoryAllocationSize` — that is the point where the accumulator must be
split across bindings, and it is one step further out than a 2 GiB assumption suggests.

## The chaos game has no seed

`WalkerState::new` (compute.rs:239) derives every walker's RNG state from its **index
alone** — no clock, no entropy, no user seed. Verified: two separate runs of the same
command produce byte-identical PNGs.

Good news for reproducibility, but it has a sharp consequence for any accumulation
design: batches must **not** re-seed or reset between passes, or every pass replays the
identical walker trajectory and the histogram just re-counts the same samples — getting
brighter, not better. The safe construction is to never reset and let the existing
walkers keep advancing, which is already what the interactive renderer does every frame.
Separately, there is currently no way to ask for an *independent* sample stream at all.

## What this implies for the plan

1. **Supersampling with a real downsample filter is the first thing to build.** It is the
   largest visible improvement per unit of effort by a wide margin, it needs no new
   accumulation architecture, and the measurement above says it costs ~40% more wall
   clock at 4x. Two existing details become wrong under it and must be fixed with it: the
   splat radius `clamp(base_radius, 1.0, 12.0)` at shaders/points/splat.wgsl:183 is in
   render-target pixels (so the 12px cap tightens to 12/N output px), and the
   `use_point_primitives` test compares against *output* height at offline.rs:723, 971,
   1076 and 1219 — under supersampling it must compare against N x height, or the
   filtered path never engages and the whole benefit is silently lost.
2. The **persistent accumulation histogram** is the second thing, and supersampling is
   what makes it necessary rather than merely nice: N x supersampling multiplies bucket
   count by N², so it divides samples-per-bucket by N². At 3840x2160 with 4x
   supersampling there are 1.4e8 buckets, and even 100 samples per bucket needs 1.4e10
   samples — 140x past today's hard ceiling. Supersampling is what spends the sample
   budget; accumulation is what supplies it.
3. Once samples are unbounded, the point buffer should get *smaller*, not larger. It
   becomes a working set streamed through the histogram, not the image's storage. The
   1.6 GB allocation and the per-buffer limit stop mattering.
4. Accumulator precision becomes the live constraint. At 1e11 samples an rgba16float
   target is hopeless — fp16 represents integers exactly only to 2048. The clean answer
   is a persistent fixed-point `u32 x 4` histogram written by a compute pass with one
   thread per texel, which needs no atomics at all because each thread owns its texel.
5. CPU sample generation is worth roughly 0.5% of what this GPU already produces and
   throws away. It should be judged against that number, not against zero.
6. Only the **splat** path can accumulate this way. The depth-tested point renderer
   cannot: each pixel shows its single nearest point, so there is no density to sum.
   Any accumulation UI must say so rather than leaving the knob wired to a mode it
   cannot affect.

## How the two fit together

They are not competing proposals, and the ordering matters:

- **Supersampling spends samples; accumulation supplies them.** Supersampling at N x
  divides samples-per-bucket by N², so turning it on at print sizes is precisely what
  exhausts the current ceiling.
- Today, at 800x600, there are enough samples for 4x supersampling to be a free win.
  That is why it should ship first and can ship alone.
- At 4K with 3-4x supersampling, there are not — and that is the render Hazel actually
  wants. Accumulation is what makes that size reachable at all.
- Density estimation (a variable-width blur, wide where the histogram is sparse and
  narrow where it is dense) is the third leg, and it is what buys smooth sparse regions
  without smearing the bright filaments a fixed filter would have to soften too.

A useful way to hold it: more samples fix *noise*, filtering fixes *aliasing*, and
density estimation fixes *the noise you could not afford to sample away*. Today
fracturize does none of the three, and the measurements say the cheapest one is missing
the most.

## Reproducing these numbers

```sh
cargo build --release -j3

# throughput: the marginal rate is the 4096 -> 16384 delta
for A in 256 4096 16384; do
  ./target/release/fracturize --scene scenes/nautilus.toml --render /tmp/acc$A.png \
    --width 1920 --height 1080 --points 100000000 --accumulate $A --splat
done

# the sample-count plateau
for P in 1000000 4000000 16000000 64000000 100000000; do
  ./target/release/fracturize --scene scenes/nautilus.toml --render /tmp/sw_$P.png \
    --width 800 --height 600 --points $P --accumulate 8 --splat
done

# the supersampling result: render 4x, average 4x4 blocks in linear light
./target/release/fracturize --scene scenes/nautilus.toml --render /tmp/ss_4x.png \
  --width 3200 --height 2400 --points 100000000 --accumulate 8 --splat

# determinism: these two are byte-identical
./target/release/fracturize --scene scenes/nautilus.toml --render /tmp/d1.png \
  --width 640 --height 480 --points 4000000 --accumulate 8 --splat
./target/release/fracturize --scene scenes/nautilus.toml --render /tmp/d2.png \
  --width 640 --height 480 --points 4000000 --accumulate 8 --splat
md5sum /tmp/d1.png /tmp/d2.png
```

Comparison crops are in `renders/quality-study/` (gitignored, so they will not follow
this file into a commit).
