# Fit optimizer: what was measured, and what to do next

Continuation plan for `src/fit/`. LIKENESS-PLAN.md is still the governing design
and its aesthetic framing is unchanged; this is about the solver and the
objective underneath it. Read IFS-INVERSE-BACKGROUND.md first if you have not —
several decisions below are downstream of theorems in it, and re-deriving them
by experiment is expensive.

## 0. State of play

Everything below was committed on 2026-09-26, having sat in the working tree
since 2026-08-19/20 while other sessions used the checkout. Part of it (the
`mod.rs` / `main.rs` half: the ladder and `--fit-perceptual`) had already been
swept into the unrelated FoV commit `3aef0fa`, which left every commit from
there to the fit commit unbuildable on its own — worth knowing if you bisect
through that range. What landed:

- `src/fit/loss.rs` — block-SSIM term over the density pyramid (`ssim_distance`),
  a mean-1 `raw` density level alongside the existing `√p` levels, and a
  `perceptual` share on `Target`.
- `src/fit/solve.rs` — `fit()` takes a coarse-to-fine ladder `&[Target]` instead
  of one target; adaptive proposal step; `accept_rate` on `Outcome`.
- `src/fit/mod.rs` — builds the ladder, logs it and the acceptance rate.
- `src/main.rs` — `--fit-perceptual`, default 0.4.

All 10 `fit::` tests pass. `--fit-perceptual 0` reproduces the old objective
exactly, which is the A/B switch for everything below.

**Added 2026-08-19 (§4.1, the instrument):**

- `src/fit/mod.rs` — contact-sheet cells carry a `SEED n LOSS 0.xxxx` label
  drawn with the existing `src/glyphs.rs` font; sheet and terminal share one
  ranked order; `Options` gained `order` and `cell`. Seeds now run on a
  bounded worker pool sized to the core count instead of one thread each, and
  print `[k/n] seed s loss L` as they land.
- `src/main.rs` — `--fit-sort loss|seed` (default `loss`), `--fit-cell PX`
  (default 480).
- `tools/lace_score.py` — scores a sheet's cells for lace-vs-fog. New; see
  §1.1 for what it is and what it is not.
- `fit-runs/2026-08-19-perceptual-n20/` — the two sheets and the log behind
  §1.1, kept because they are the first n=20 evidence in this document. Only
  the logs and scripts under `fit-runs/` are in git; the sheets (~65 MB of PNG
  across all three runs) are on the desktop only, and are what §5 regenerates.

11 `fit::` tests pass (one new: every cell is labelled, inside its own square).

**Added 2026-08-20 (§4.3a, overlap):**

- `src/fit/solve.rs` — `Walk` carries a `piece` tag (which map produced each
  point); `piece_overlap` measures open-set-condition violation as redundancy
  of mass; `Outcome::overlap`, reported per fit as `overlap NN%`.
- `src/fit/mod.rs` — overlap in the ranked table.
- `fit-runs/2026-08-20-overlap/` — the 60 fits behind §4.3a.

12 `fit::` tests pass, 550 overall (one new: overlap reads 0.003 on an exact
tiling and exactly 0.50 on two-of-three stacked).

## 1. The central finding

**Driving the fit loss down made the pictures worse, reliably.** Best-scoring
run of eight (6 maps, 64k iters, 16384 points, loss 0.0775) was the haziest
picture in the set; the best-looking one had mediocre loss. The solver was not
failing — it was succeeding at the wrong question.

Both halves of the old objective were *distributional*. Sliced Wasserstein
compares one-dimensional marginals, and a diffuse cloud matches a thin edge
target's marginals almost exactly. The Hellinger grid term averages over
`PYRAMID = [8,4,2,1]`, so at `fine = 32` half its weight sat on 4×4 and 8×8
grids, which fog also matches. Nothing in the objective measured *local
contrast*, which is the only thing that separates lace from haze.

The fix is a block-SSIM term over the same pyramid, blended rather than
substituted. **Both ends of the blend fail, differently, and the useful range is
interior** — this is the single most important empirical fact in this document:

| `--fit-perceptual` | behaviour |
|---|---|
| 0.0 | pure distributional. Minimiser is fog. Loss falls a long way, picture degrades. |
| 0.2 | mostly fog. |
| 0.4 | current default. Best pictures seen so far. |
| 0.6–0.8 | structure returns but weaker than 0.4. |
| 1.0 | anneal barely moves (0.3069 → 0.3341). Pure SSIM has almost no long-range signal: two structures that do not already overlap score alike however far apart they are. |

0.4 was chosen **by eye, on one reference, from one seed**. Treat it as a
starting point, not a result.

> **Update 2026-08-19:** it has since been swept properly — six values, 20
> paired seeds — and 0.4 came out as the knee of a real likeness/structure
> trade-off rather than a lucky guess. The qualitative shape of this table
> holds, including that 1.0 is degenerate. See §4.2a for the numbers, and for
> the metric trap that makes 1.0 look *good* if you rank on structure alone.

## 1.1 The n=20 re-test — read this before acting on §1

The table above rests on n ≤ 4. It has now been re-run at n = 20: the Clarent
card, 16000 iters, 8192 points, the same twenty seeds under
`--fit-perceptual 0` and `0.4`, scored with `tools/lace_score.py`.

That script measures **lace-vs-fog and nothing else** — a local coefficient of
variation, `mean(local sd / local mean)` over occupied pixels. Fog is smooth
and scores low; filaments, holes and lattices score high. It is not the SSIM
term the fit optimises, because scoring the objective with itself cannot answer
whether that objective picks good pictures. It is emphatically **not
likeness**: the single laciest cell in the whole sweep (0.4, seed 15, score
1.831) is a lattice of straight lines with no resemblance to the card at all. A
high score means "worth looking at", not "good".

| | `perceptual 0.0` | `perceptual 0.4` |
|---|---|---|
| lace, mean | 1.267 | 1.327 |
| lace, median | 1.249 | 1.313 |
| lace, max | 1.458 | **1.831** |
| lace, min | 0.936 | 1.026 |
| corr(loss, lace) | +0.043 | −0.123 |

Two findings, and the first one revises §1 rather than confirming it.

**1. The loss is not anti-correlated with structure. It is uninformative about
it.** `r = +0.04` and `−0.12`; at n = 20 significance needs about `|r| = 0.44`,
so both are indistinguishable from zero. The lace-ranks of the five
best-scoring fits were [7, 6, 13, 11, 8] at 0.0 and [4, 6, 9, 14, 7] at 0.4 —
scattered through the middle of the field in both. Ranking a sweep by loss is
close to ranking it at random with respect to whether you get lace or fog. That
is a weaker claim than "driving the loss down makes it worse", and a more
useful one, because it says the loss is not a judge rather than that it is an
inverted judge. (It does not test §1's actual claim, which was about driving
the loss down *within* one run by spending more budget. That is still untested.)

**2. The perceptual term raises the ceiling more than the average.** Paired
over the same seeds, the mean lace change is +0.060 (sd 0.194, se 0.043),
`t = 1.38` on 19 df — **not significant**, with only 12 of 20 seeds improving.
But the best-of-20 went 1.458 → 1.831, a 26% rise against a 5% rise in the
mean. So 0.4 is not reliably better *per fit*; it is better at the top of the
distribution.

**The methodological consequence is the important part: compare conditions by
their best-of-N, not by their mean.** The workflow is "roll twenty and pick
one", so the statistic that matters is the best cell a condition produces, and
a mean over a distribution dominated by seed variance will call a real ceiling
improvement insignificant — as it just did.

## 2. What is measured and what is assumed

The evidence base is thin and the next agent should know exactly how thin.

**Measured:**
- Loss falls monotonically with iterations, no plateau: 4k → 0.1259, 16k →
  0.1093, 64k → 0.0880 (6 maps, 4096 points, same seed). The optimizer is not
  stuck; budget is a real lever.
- Default budget is ~22 s/seed single-core. An hour on 4 cores is ~650×.
- Grid resolution is `sqrt(points/4)` clamped to `[8, 64]`. Default 4096 points
  gives a **32×32** finest grid; 16384 gives 64×64, which is the hard ceiling.
- Acceptance sits at 57–60% against the 30% the adaptive step targets.
- At 16384 points, more maps scored *worse* at fixed iterations: 6 maps 0.0775,
  16 maps 0.1040, 24 maps 0.1413. Capacity without budget to train it is a
  losing trade.

- **The loss does not rank a sweep by structure** (§1.1, n = 20 × 2
  conditions). This is now measured rather than inferred from one sheet.
- **The perceptual term's benefit is in the tail, not the mean** (§1.1). Its
  effect on the average fit is within seed noise at n = 20.
- The reported `dimension` is untrustworthy, visibly and at scale: across the
  20 pure-distributional fits it read 0.68–1.26 — "lacy" by the Moran
  equation — on cells that are plainly solid fog. Rule 4 below, now with 20
  witnesses instead of one.

**Assumed, not measured:**
- That 0.4 is near-optimal for anything other than the Clarent card.
- That the perceptual term helps on subjects other than this one.
- That the ladder's 3 rungs and squared-progress schedule are well chosen.
- That block-SSIM is better than sliding-window MS-SSIM here.
- That spending more budget *within* a run degrades the picture — §1's original
  claim, which §1.1 did not test and which the monotonic loss curve above makes
  worth re-checking now that there is an instrument for it.

**Everything visual rests on n ≤ 4 seeds.** Seed variance visibly dominates
every effect measured. In the 4-seed default run only seed 0 produced the ring
structure, colour separation and sword-line; seeds 1–3 stayed hazy. Hazel
reproduced this exactly, so it is deterministic and seed-specific, not luck in
one run.

## 3. Rules for evaluating anything here

These have already caused wrong conclusions once each.

1. **Never judge a fit by its loss number.** Render the sheet and look. The
   whole finding in §1 is that the two are anti-correlated over part of the
   range.
2. **Losses are not comparable across `--fit-points`.** Finer grids score higher
   intrinsically. Only compare within a fixed point count.
3. **Losses are not comparable across `--fit-perceptual`.** Different objective.
4. **`similarity_dimension` silently lies when maps overlap.** It solves
   `Σsᵢᵈ = 1`, which assumes the open set condition. A fit reported d = 1.65
   while rendering as fog. **Confirmed 2026-08-20**: fitted attractors carry
   0.12–0.18 overlap against 0.003 for an exact tiling (§4.3a), so the premise
   is materially violated and the readout is not an instrument. It is now
   printed next to `overlap` so the two can be read together — a dimension
   quoted beside a high overlap is nonsense, and now visibly so.
5. `initial_loss` is now re-measured at the finest rung for reporting. If you
   change the ladder, keep that or the "N× better" figure becomes meaningless.

## 4. The work, in order

The ordering matters and is not the order these were thought of. Two items are
deliberately early because they are *instruments*: until they exist, the other
items cannot be evaluated.

### 4.1 Seed sweeps as the measurement instrument — **done 2026-08-19**

Landed: labelled cells, one shared ranked order for sheet and terminal,
`--fit-sort`, `--fit-cell`, a bounded worker pool, live progress, and
`tools/lace_score.py`. See §0 and §1.1.

What it bought immediately: §1's central finding turned out to be *wrong in its
strong form* the moment it was measured at n = 20 (§1.1), and the dimension
readout was caught lying at scale. Both of those were single-sheet impressions
before. Keep using it that way — every item below should end in a sheet and a
`lace_score` run, compared **best-of-N rather than mean**.

### 4.2a The perceptual ratio, swept — **done 2026-08-19, on one reference**

`0.0 0.2 0.4 0.6 0.8 1.0`, 20 seeds each, the *same* 20 seeds in every
condition so all comparisons are paired. Clarent card, 16000 iters, 8192
points. Artifacts in `fit-runs/2026-08-19-perceptual-sweep/`.

| perceptual | lace mean | likeness mean | coverage mean | coverage min | anneal gain |
|---|---|---|---|---|---|
| 0.0 | 1.267 | 0.709 | 0.348 | 0.219 | 3.48× |
| 0.2 | 1.279 | 0.701 | 0.340 | 0.185 | 2.98× |
| 0.4 | 1.327 | 0.704 | 0.327 | 0.197 | 2.54× |
| 0.6 | 1.325 | 0.692 | 0.326 | 0.201 | 2.17× |
| 0.8 | 1.331 | 0.677 | 0.314 | 0.145 | 2.25× |
| 1.0 | 1.394 | 0.638 | 0.276 | **0.018** | 1.91× |

Within-seed trend tests across the range (slope per unit perceptual, fitted per
seed, then tested over the 20 slopes):

- **likeness −0.0625 (t = −3.35, significant).** Raising the ratio makes fits
  less like the subject, reliably.
- **anneal gain falls monotonically, 3.48× → 1.91×.** The 1.0 end has almost no
  long-range signal, exactly as §1 said. This is a property of the optimiser
  and — unlike raw loss — is comparable across conditions.
- **coverage falls, and its floor collapses.** One fit at 1.0 came out at 1.8%
  coverage: a handful of line segments.
- lace +0.113 (t = +2.25, significant) — **but see the trap below.**

**The trap, which caught me first.** `lace` rises monotonically and peaks at
1.0, which reads as "more perceptual is better" and is wrong. A nearly-empty
cell has enormous local contrast, so a collapsed fit scores at the *top* of
that metric: corr(lace, coverage) = **−0.54 at 0.4 and −0.87 at 1.0**. At the
sparse end `lace` is mostly a readout of emptiness. The 1.0 sweep's laciest
cell is its collapsed one. `tools/lace_score.py` now prints coverage beside
lace and flags cells under 10% for this reason — never rank on lace alone.

**Where the knee is.** Marginal trade per step, averaged within seeds:

| step | Δ lace | Δ likeness | |
|---|---|---|---|
| 0.0→0.2 | +0.012 | −0.008 | bad trade |
| 0.2→0.4 | +0.048 | +0.002 | **structure essentially free** |
| 0.4→0.6 | −0.001 | −0.012 | bad trade |
| 0.6→0.8 | +0.006 | −0.015 | bad trade |
| 0.8→1.0 | +0.062 | −0.039 | lace bought with likeness (and coverage) |

`0.2→0.4` is the only step that buys structure without paying likeness for it.
**So 0.4 survives its own arbitrariness — it is the knee, not a guess that
happened to stick.** The defensible band is 0.2–0.6; above 0.6 all three honest
indicators (likeness, coverage, anneal gain) degrade together.

**What this does not establish.** One reference. The likeness figure is a crude
proxy — 32×32 Pearson between tone-mapped renders — chosen to be independent of
the three terms the fit optimises, not because it is good. Differences *within*
0.0–0.6 are small and mostly not individually significant; what is significant
is the trend across the whole range. Subject-dependence and the interaction
with map count are still untested, which is what 4.2b is for.

### 4.2b Per-target sweeps of the perceptual ratio

Before the temperature work, not after. A better optimizer descends the loss
faster, and if the loss is still partly fog-seeking you will buy fog more
efficiently — the exact trap §1 describes. Settle what is being descended first.

Sweep `--fit-perceptual` across several *different* references (the card, the
powercell token, the inert sword) at ~20 seeds each. The open questions: is
there one good default, or is it subject-dependent? Does the optimum move with
map count?

Now that 4.2a has done the Clarent card, what is left here is the part that
actually needed several references: **is 0.4 subject-dependent?** Run the same
six-point sweep on the powercell token and the inert sword, and check whether
the knee moves. Also whether it moves with map count.

Two method notes carried over, both learned the hard way:

- Judge a condition by its **best cell**, not its mean — the workflow is "roll
  twenty and pick one", and a mean over seed-variance-dominated data will call
  a real ceiling difference insignificant. But bootstrap the ceiling; it is a
  noisy order statistic and its CIs overlapped heavily across conditions here.
- Never rank on a single metric. Every one of `lace`, `likeness` and `loss`
  fails somewhere in this range, and they fail in different places, which is
  the only reason the knee was locatable at all.

### 4.3a Overlap, measured — **done 2026-08-20. The premise did not survive.**

`solve::piece_overlap` now measures the open set condition directly. The chaos
game already knows which map produced each point (`Walker::step` returns the
index), so the walk carries a `piece` tag and the pieces `fᵢ(A)` can be
rasterised separately. The statistic is redundancy of mass — what fraction of
the picture is covered by more than one piece — rescaled by its `1 − 1/N`
maximum so map counts compare. Reported per fit as `overlap NN%`.

Calibrated at both ends by test:

| | overlap |
|---|---|
| Sierpinski — exact tiling, OSC holds | **0.003** |
| two of three maps perfectly stacked | **0.498** (exactly ½, as the arithmetic requires) |
| fitted attractors, 60 fits | mean 0.12–0.18, **max 0.31** |

**No fit anywhere near collapse.** The pathology this item was meant to fix —
IFS-INVERSE-BACKGROUND.md §3.5, maps stacking until capacity vanishes — is not
what these fits are doing. Over 60 fits (perceptual 0.0/0.4/1.0, 20 seeds each,
`fit-runs/2026-08-20-overlap/`):

| | r with overlap | 95% CI |
|---|---|---|
| lace | −0.260 | [−0.482, −0.007] |
| coverage | **+0.573** | [+0.374, +0.722] |
| likeness | **+0.602** | [+0.411, +0.742] |
| loss | −0.522 | [−0.685, −0.309] |

Consistent in sign within every condition separately, so it is not an artifact
of pooling across conditions.

**Overlap buys likeness and coverage.** Penalising it would trade both away for
lace — which is the *same* axis §4.2a already showed the perceptual ratio
controls, so it is not the independent lever this item assumed. And the
direction of the observed failure is the opposite one: the single degenerate
fit in the whole sweep (1.8% coverage, three line segments) has the **lowest**
overlap in its condition at 0.02. Here the danger sign is too little overlap,
not too much.

This is exactly what §3.3 of the background doc predicts and the plan did not
carry through: covering a 2D extent needs the maps to tile nearly exactly, and
generic optimised maps cannot, so sloppy overlap is *how* six maps reach the
subject's extent at all. Remove it and you get dust.

**Recommendation: do not add an unconditional overlap penalty.** What is worth
keeping is the measurement, for two jobs it does well:

- It says when `similarity_dimension` is meaningful. At 0.15 overlap the Moran
  equation's premise is materially violated — 50× a clean tiling — so the
  d = 0.68–1.26 "dust/lace" readings on visibly solid fog are confirmed
  nonsense rather than suspected nonsense (rule 4).
- Low overlap *with* low coverage identifies a collapsed fit, which is a real
  failure the sweep produces occasionally and nothing else currently catches.

**What would still settle it properly.** The above is observational, on one
reference: overlap and likeness may both be driven by how far the attractor
spreads. The intervention — a one-sided penalty with a swept weight, watching
coverage as the guard rail — is the only way to know whether pushing overlap
down finds better *tilings* (§3.3's knife edge) or just dust. Worth doing only
if the trade-off curve in §4.2a stops being the binding constraint, and worth
doing as a sweep from the start rather than a tuned constant.

Missing calibration: what a *hand-authored* scene that looks good scores. That
would say whether 0.15 is high or normal, and it is cheap — surfacing overlap
next to dimension in `--info` would do it, at the cost of updating three golden
layouts.

### 4.3b Overlap / collapse penalty — superseded, see 4.3a

Missing entirely, and it buys two things at once. There is no open-set-condition
constraint anywhere, so maps freely stack — which is both a fog mechanism in its
own right (§3.3 of the background doc) and the reason the dimension readout is
untrustworthy (rule 4 above). A penalty on pairwise map overlap makes the
diagnostic honest and adds a prior that pushes away from haze.

### 4.4 Temperature schedule

Well-motivated by measurement: acceptance sits at 57–60% against a 30% target,
and the adaptive step controller *saturates* without reaching it, because
temperature rather than step size is setting the acceptance floor. `temp =
cur_loss × lerp(0.12, 0.001, t)` is generous throughout. The step controller
cannot fix this on its own; the schedule itself is the lever.

### 4.5 Measure the target's real resolution — *then* decide on a ladder rung

Do not raise the `[8, 64]` clamp on faith. The target is a heavily filtered
segmentation, and Hazel's own read is that it survives 4× downsampling — which
puts its real information content around 112×94. If so, 128×128 is already past
it and the rung buys nothing but 4× cost on the most expensive leg.

Cheap check first: downsample the target grid progressively and find where the
structure term stops changing. Let that number decide whether to build the rung.

### 4.6 More advanced perceptual metrics

Split this by where the metric runs, because the economics are completely
different:

- **In the inner loop** (~64k evaluations per fit) only cheap metrics are
  viable. The obvious next one is a gradient-orientation histogram: the target
  genuinely *is* an edge map, so orientation is the signal actually present and
  nothing currently reads it. Sliding-window MS-SSIM is the conservative
  alternative.
- **As a re-ranker** over a seed sweep, an expensive learned metric is entirely
  affordable — 100 seeds is 100 inferences. This is probably the highest-leverage
  use of a local model (a Gemma-class VLM, or LPIPS): not to drive the anneal,
  but to replace the human eye at the "which of these hundred is good" step,
  which is the part of the workflow that does not scale.

Reach for this only if 4.1–4.4 leave the pictures short. Cheap in-loop upgrade
before expensive one.

**§1.1 promotes the re-ranker half of this item.** Selection is now a *measured*
bottleneck rather than a suspected one: the loss ranks a sweep essentially at
random with respect to structure (`r ≈ 0`, both conditions, n = 20), so the
twenty scenes on disk contain more than the loss can find, and every sweep from
here is bottlenecked on a human looking at cells. `tools/lace_score.py` is a
first, deliberately crude stand-in — it separates lace from fog and knows
nothing about likeness, so it can shortlist but not choose. Replacing it with
something that reads likeness (LPIPS, or a Gemma-class VLM asked to rank cells
against the reference) is affordable at 100 inferences per sweep and is
probably the highest-leverage item left in this document. The in-loop metrics
above are still gated behind 4.3 and 4.4.

## 5. Reproduction

```
cargo build --release
./target/release/fracturize --fit clarent-reimagined-prxy.png --fit-crop card \
  --fit-seeds 4 --fit-iters 64000 --fit-points 16384 \
  --fit-out out.toml --fit-preview out.png
```

Four seeds at this budget is roughly a coffee on 4 cores. Seed 0 is the one with
the rings, the magenta/cyan split and the diagonal sword-line; seeds 1–3 are
hazy. `--fit-perceptual 0` on the same command gives the pre-change objective
for comparison.

The §1.1 sweep, which is the shape every measurement below should take — two
conditions over the same seeds, then scored:

```
for P in 0.0 0.4; do
  ./target/release/fracturize --fit clarent-reimagined-prxy.png --fit-crop card \
    --fit-seeds 20 --fit-iters 16000 --fit-points 8192 --fit-perceptual $P \
    --fit-out p$P.toml --fit-preview p$P.png
done | tee sweep.log
tools/lace_score.py p0.0.png --log sweep.log --run 0
tools/lace_score.py p0.4.png --log sweep.log --run 1
```

20 seeds at that budget is 6–8 min per condition on 4 cores. Note the reduced
budget against the command above — it is deliberate, because 20 seeds at 64000
iters and 16384 points is about 80 min per condition and the comparison did not
need it. Losses from this budget are not comparable with those from that one.

Effort for the items above is measured in sweeps: 4.1 and 4.2 are sweep-bound
rather than code-bound, 4.3 and 4.4 are small code changes each followed by a
sweep to see whether they did anything, 4.5 is a measurement before it is a
change, and 4.6 is a real piece of work that should not start until the earlier
items have run.
