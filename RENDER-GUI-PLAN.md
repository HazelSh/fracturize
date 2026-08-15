# Render GUI plan — surfacing the render-quality work

The render-quality slices landed CLI-first. Nine flags exist that the GUI has
never heard of, and `src/app.rs` around line 3229 is a column of apologies
saying so: `grade: NEUTRAL`, `grade_out: None`, `checkpoint_out: None`,
`resume_from: None`, `density_estimation: Default::default()`, `spp: None`.

This plan decides what to surface, where, and on what kind of control.

All wording below is **proposed**, not settled — GUI text is Hazel's.

---

## 1. What exists that the GUI can't reach

| CLI | What it is | Live-previewable? |
|---|---|---|
| `--gamma`, `--gamma-threshold`, `--vibrancy` | the tonemap grade | **yes, exactly** |
| `--spp` / `--effort` | accumulate to a sample target | no |
| `--density-estimation` | variable-width blur | no (see §3) |
| `--grade-out` | save the linear buffer | n/a — a file |
| `--checkpoint` / `--resume` | save/continue the histogram | n/a — a file |
| `--retonemap`, `--grade-sweep` | re-grade a saved buffer | n/a — an investigation |
| `--chaos-seed` | independent deal of the same attractor | n/a — a measurement |

The right-hand column is the whole design. It is the line the GUI should be
organised along, and it is already the line this app uses without having named
it: `exposure` is in the Render window because you can watch it, `supersample`
is in the render-job dialog because you cannot.

---

## 2. The one structural decision: the grade is not a render setting

`SplatRenderer::upload_params` has taken a `Grade` since slice 6a.
Both live call sites pass `Grade::NEUTRAL` — the window (`app.rs:5487`) and the
screenshot path (`app.rs:5197`) — as does the job's `JobParams`. The plumbing
for a live grade is already built and switched off.

That matters because the grade is a **pure function of the accumulated
density**. Given the same histogram, gamma 2.4 in the viewport and gamma 2.4 in
a render are the same arithmetic — the viewport's version is computed from
fewer samples, but it is not an approximation of the render's, it is the same
curve on a noisier input. So the viewport can show it *exactly*.

Putting gamma and vibrancy in the render-job dialog would be the mistake of
putting colour correction in a print dialog: you would type a number into a
modal, click Start, wait, open a PNG, and decide whether you wanted 2.2 or 2.6.
Live, it is a slider you drag while looking at the thing.

**So: the grade goes in the Render window, and the job inherits it** — exactly
as `exposure` already does at `render_job::open`.

### Persistence class

`src/ui/render_panel.rs` groups by persistence class under headings that say so
("Scene — saved with the scene by Ctrl+S"; "Preference — follows you across
scenes"). The grade fits neither. `src/view.rs` puts it on the **view**, with a
doc that argues the case: a grade "changes nothing about what the attractor
*is*, only how its density becomes a picture", and putting it in the scene
would let a grade found while rendering clobber the scene file.

So this needs the scheme's third heading, which the scheme was always going to
need:

> **View** — saved with a view file and written into render records; never
> written to the scene.

That is the panel's own stated purpose working as intended, not an exception to
it. `App::current_view()` currently writes `None` for all three grade fields;
it starts writing the live values.

---

## 3. Why density estimation is *not* live

DE is technically capable of running live — `DensityEstimator::pass_over` takes
any texture view of matching format and size, and the interactive splat
renderer has an accumulation target it would accept.

It should not, because **the radius law is calibrated in raw accumulated
units**. `TARGET_DENSITY = 16` and the radius is `sqrt(target / density)`, so
the kernel width at a given texel depends on how many samples landed there. The
viewport's ring-buffer accumulation and a 1000-spp histogram are at completely
different scales, so a live preview would pick systematically wrong radii and
show you a different picture from the one you are about to render.

This is not a "close enough" gap. The measured benefit of DE falls from 34% at
100 spp to 7.5% at 1000 spp (`AGENTS.md`) — DE is *a function of sample count*,
which is precisely what a preview cannot match. A preview that lies about the
one knob nobody can predict is worse than no preview.

So DE is a render-job control, and its tooltip should say why there is no
preview rather than leaving it to be discovered.

*(If a live DE is ever wanted, the honest route is normalising the estimator's
input by sample count so `TARGET_DENSITY` means the same thing at both
densities. Worth a measurement, not worth blocking this plan.)*

---

## 4. Render window — the new **View** group

Drawn after the Scene group, before Preference. Greyed entirely under the
points renderer, with the reason on hover, exactly as `exposure` already is:
there is no tonemap to grade.

```
── View ── (saved with a view file and with renders; not part of the scene)
   gamma                                2.40        [neutral]
   ────────────────────●─────────────────────────
   gamma threshold                      0.30
   ────────●─────────────────────────────────────
   vibrancy                             1.00
   ──────────────────────────────────────────────●
```

**Dependency, drawn.** `Grade::NEUTRAL` has gamma 1, and at gamma 1 the curve
is `x^1` — both vibrancy routes collapse and the threshold has nothing to flatten.
The type's own doc says so, and `--vibrancy`'s help says "inert at `--gamma 1`".
So **toe and vibrancy grey out at gamma 1**, with a hint saying they need a
gamma curve to act on. Two-thirds of the group teaching its own dependency for
free, and consistent with the house rule: grey and explain, never hide.

**A `neutral` button.** Three coupled sliders is a state you can get lost in,
and `NEUTRAL` is a named, meaningful point — not merely "the defaults". One
click back to the tonemap the window has always had.

**Naming: `gamma threshold`, in full.** An earlier draft proposed "toe" because
the panel had no room for the longer label. That was the layout constraining the
name, which is backwards — the name is what a person has to carry between this
window, `--gamma-threshold`, and Apophysis, and the panel is a few lines of
egui. "knee"/"toe" only land for someone who already pictures the curve, and
this window does not graph it at them.

So the label wins and **the layout moves.** `egui::Slider`'s text sits to the
right of the widget and squeezes it, so the fix is to stop using the slider's
own `.text()` for this group and put the labels above their sliders, full-width
below. That also buys room for the value readouts and makes the three grade rows
scan as a group rather than as three unrelated sliders. If the window ends up
wanting to be wider, it gets wider.

---

## 5. Render-job dialog — regrouped

Today `draw_quality` is a flat stack of eleven controls: size, points,
accumulate, supersample+filter, splat+exposure, bit depth, transparent, then
the animation block. Adding spp, DE, and two sidecar files to that stack makes
it a wall.

Proposed: **four groups, each one question, in pipeline order.**

```
Output    [ still | avif | mp4 | view ]
          renders/lacewing-1755…​.png
──────────────────────────────────────────────
How big   [SD] [480p] [720p] [1080p] [1440p] [4K]     w 1920  h 1080
──────────────────────────────────────────────
Samples   [ one pass | accumulate ]        ← accumulate is the default
          ┌ accumulate ───────────────────────
          │ effort    [tiny][small][ medium ][large][huge]
          │ target    ──────●───    100 samples/px
          │ buffer    ───●──────    20.0M     (working set)
          └ one pass ─────────────────────────
            points     ──────●───   20.0M
            accumulate ────●─────   96
──────────────────────────────────────────────
Pixels    supersample 2x   [gaussian]  r 0.50
          density estimation  ──●──────  0.30
──────────────────────────────────────────────
Files     [x] 16-bit PNG      [ ] transparent background
          [ ] grade buffer  (.fgrade — re-grade without re-rendering)
          [ ] checkpoint    (.fhist — continue this render later)
──────────────────────────────────────────────
grade: gamma 2.40 · toe 0.30 — from the Render window
GPU memory: 1.1 GB (320 MB of it points), limit 2.0 GB
Estimated time: 3–7 min (1 frame)
```

### Why "Samples" is a radio and not another slider

Under `--spp`, `--accumulate` is **ignored outright** (`offline.rs` prints
"--accumulate is ignored here" and moves on) and `points` stops being the
quality knob and becomes a working set that keeps the GPU busy between folds.
That is not one more setting — it is a different machine, and it should be
drawn as one. The app already has the idiom (`ui::radio`: exactly one is always
in force, "neither" is not a state).

The radio also **prevents the jobs the CLI would reject**, rather than
reporting them at Start. `accumulate` greys out unless the output is a still
and splat is on, with the hint carrying the reason I just reworded into the CLI
— a contact sheet or an animation would be one full accumulation run per tile
or frame. The GUI should not be able to construct a job the CLI refuses.

### Why effort tiers *and* an spp number

The tiers are the mental model the CLI teaches: four orders of magnitude across
five named sizes, named for size deliberately — not duration (machine-
dependent) and not outcome ("converged" assumes you have no reason to go
further). The segmented radio is the primary control; the spp slider under it
is the exact figure, and moving it lights no tier. `--effort` also goes no
further than `huge` on purpose, and the slider is how you get past it — the
same escape hatch, in the same place.

### Why DE sits with supersample and the filter

They are one family: **given the samples, how does a pixel get made?**
Supersampling gathers at N×, the filter reconstructs down, DE widens the kernel
where the histogram is sparse. All three are reconstruction. None of them
changes what the picture *is* or how many samples went into it — that is the
group above. This is a better home than "somewhere near the bottom", and it
puts the two things that interact (DE's radius scales with supersample —
`max_radius(supersample)`) next to each other.

### Why the sidecars are "Files"

The bit-depth control already carries the comment that it and transparency
"describe the *file* that leaves the app, not how the picture is drawn".
`.fgrade` and `.fhist` are exactly that and nothing else. The heading makes the
existing distinction visible instead of implied.

### The estimate has to be rewritten

`estimate_secs` models `points × (accumulate + 8) / throughput`. That is the
ring path and it is wrong by orders of magnitude for an accumulating render,
which is `laps = ceil(spp × pixels / capacity)` laps of a full buffer — so:

```
accumulating_seconds ≈ spp × pixels / throughput
```

Near enough independent of the point count, which is a genuinely useful thing
the dialog can *say*: under `accumulate`, the buffer slider changes how fast it
runs, not how long it runs for.

`total_bytes` also has to grow a histogram term — 32 bytes a texel at N², which
is 265 MB at 1080p and 2×, and over a gigabyte at 4K. That is not a rounding
error, and `rejection()` is checked before anything is allocated precisely so
this class of thing arrives as a sentence rather than a device-lost panic.

---

## 6. Ranges — the 20–80% arithmetic

Hazel's constraint: the usable artistic range should occupy the middle 20–80%
of the control's travel.

| Control | Range | Scale | Usable band | Lands at |
|---|---|---|---|---|
| gamma | 0.5 – 6.0 | **log** | 1.0 – 4.0 | **28% – 84%** |
| toe | 0.0 – 0.6 | linear | 0.05 – 0.5 | **8% – 83%** |
| vibrancy | 0.0 – 1.0 | linear | all of it | 0 – 100% |
| density estimation | 0.0 – 1.0 | linear | all of it | 0 – 100% |
| spp | 1 – 10,000 | **log** | 10 – 1,000 | **25% – 75%** |

**gamma** is logarithmic because it is a ratio knob — 2.0 vs 2.2 matters as
much as 4.0 vs 4.4, and the shader uses `1/gamma`. Position is
`log(v/0.5) / log(12)`: neutral 1.0 sits at 28%, the sweep default's top end
4.0 at 84%, and the measured example from `AGENTS.md` (gamma 2.5) at 65%.
Keeping a little of the sub-1 range costs nothing and gives neutral room to
breathe off the left edge. The clamp is 0.1–10; the slider deliberately does
not offer all of it.

**toe** is capped at 0.6 rather than its full 1.0 clamp. The measured useful
range is 0.05–0.5 (and note it is in post-log *coverage*, not flam3's raw
density units — the two knobs share a name and not a scale). Full 0–1 would put
everything usable in the bottom half. 0.6 puts it at 8–83%, and the top of the
travel reading as "further than you want" is correct: past ~0.5 the toe starts
eating real detail.

**vibrancy** and **density estimation** are the deliberate exceptions, and they
are exceptions for the same reason: the range *is* the phenomenon. Vibrancy is
a blend between two routes through the gamma curve, and DE — after the slice-7
fix — is a literal mix, linear end to end by construction, where 0.3 means
three tenths of the effect. Restricting either would be inventing a limit.

**spp** at `log(v/1)/log(10000)`: 10 → 25%, 100 → 50%, 1000 → 75%. The decades
land on quarters, which is what makes the tier radio and the slider read as the
same scale.

---

## 7. Deliberately not surfaced

- **`--resume`.** The CLI needs checkpoints because a process exit loses
  everything. The GUI process persists, and the dialog already has **Pause**,
  which frees the GPU between frames without losing the job. Resume in the GUI
  would be a file picker plus a compatibility check (scene, size, supersample
  must match) for a case Pause already covers. Surface `--checkpoint` as an
  *output* — so a GUI render can be continued by the CLI, or on another machine
  — and build no resume UI until something wants one.

  Note this is what makes the cancel prompt (§9) coherent rather than cruel: a
  `.fhist` the GUI cannot itself reload is still worth writing, because the CLI
  can. If that starts to feel like a dead end in practice, the answer is a
  resume UI, not a quieter cancel.
- **`--chaos-seed`.** Its use is rendering the same picture twice with
  independent noise to compare them. That is a measurement, not a decision
  anyone makes about a picture.
- **`--retonemap` / `--grade-sweep`.** Contact-sheet investigations. The GUI's
  answer to "which gamma?" is the live slider in §4, which is better than a
  sheet — and once `.fgrade` files are being written from the dialog, the CLI
  sweep is still there for the case where you want the sixteen side by side.
- **`--gpu-timing`.** Its own doc says it is an investigation, not part of a
  render.

---

## 8. Slices

**1 — Live grade.** `Grade` on `App`, both `upload_params` call sites, the
View group in `render_panel.rs`, `current_view()` writing the three fields.
Self-contained, and the highest value in the plan: gamma is most of what people
mean by the Apophysis look and the GUI currently cannot reach it at all.

**2 — The job inherits it.** `grade: app.grade` in `start_job`, and the weak
"from the Render window" line so an inherited setting is not an invisible one.
Retires the first of the six apology comments at `app.rs:3226`.

**3 — The estimator, before any control that depends on it.** `estimate_secs`
and `total_bytes` learn the accumulating path. This leads rather than follows:
the dialog's contract is that it tells you what a job costs *before* you agree
to it, and shipping a control whose cost line is wrong by 40× would break the
one promise the module doc makes ("Estimates don't lie"). It is also testable on
its own — the measurements in §9 are the fixture, and the estimate should land
inside its own ±40% band on every row of that table.

**4 — Samples radio + effort/spp,** defaulting to `accumulate`/`medium`, with
the accumulate option greyed on non-still/non-splat carrying the CLI's reason.
Now safe, because slice 3 can already price it.

**5 — DE slider** in the Pixels group.

**6 — Sidecar files:** `.fgrade` and `.fhist` checkboxes, defaulting their
paths off the render name the way the CLI's bare-flag forms do.

**7 — Cancel stops compute, then offers the checkpoint** (§9). Last because it
needs slice 6's `.fhist` writer, and because it changes what a red button
promises — worth landing on its own where it can be looked at.

Slices 1 and 2 are worth landing before 4 is designed in detail — they are
small, they are visible, and they will teach us whether the View heading reads.

---

## 9. Decisions, and what measured them

### The default is `accumulate` at `medium`

Measured on the GTX 1080 desktop, `scenes/lacewing.toml`, splat, 1× supersample.
Cost excludes setup (shader compilation, ~0.15s) so the terms that scale are
visible:

| output | ring 20M/acc 96 *(old default)* | `small` | `medium` | `large` |
|---|---|---|---|---|
| 720p | **21.7** spp · 0.26s | 10 spp · 0.27s | 109 spp · 0.63s | — |
| 1080p | **9.6** spp · 0.44s | 19 spp · 0.54s | 106 spp · 1.27s | 1003 spp · 5.09s |
| 4K | **2.4** spp · 1.57s | 12 spp · 2.47s | 101 spp · 6.49s | 1001 spp · 33.4s |

The ring's density **falls as the output grows** — 21.7 → 9.6 → 2.4 — because
the point buffer is a fixed 20M while the pixel count is not. The old default
therefore gets *worse* the bigger you render, which is backwards and is the
whole flaw `--effort` was built to fix. A 4K still at 2.4 samples/px is noise.

`small` is nearly free but **regresses at 720p** (10 spp against the ring's
21.7), and 720p is the dialog's default size — a new default that makes the
common case worse is not a good default however cheap it is.

`medium` is the lowest tier that beats the old default at *every* size, and it
costs under 1.3s at 1080p and 6.5s at 4K here. On a much slower GPU that grows,
but the dialog quotes an estimate before you commit — protecting a slow machine
is the estimate's job, not the default's.

### Cancel drops the compute, then offers the checkpoint

Today Cancel throws everything away ("Cancelled — nothing was written"), which
makes the GUI **more destructive than Ctrl-C**: the CLI finishes its lap, writes
the checkpoint, writes the partial PNG and exits 130. So:

1. Cancel stops the chaos game immediately — that is the resource you clicked
   the button to get back.
2. *Then* a small prompt offers the histogram as a `.fhist`.

One caveat to design against: stopping compute does not free the **histogram**,
which is a texture — 265 MB at 1080p×2, over a gigabyte at 4K×2. So the prompt
has to be a prompt, answered and gone, not a state the dialog can sit in
holding a gigabyte of VRAM while nobody is looking at it.

### Still open

- **`--accumulate` may not cost what the GUI says it does.** Its slider hint
  promises "proportionally more time"; measured at 1080p, `--accumulate` 1 → 512
  moved wall time by about 0.02s while visibly changing the image (0.33M pixels
  differ at 32, 1.15M at 512). It advances the chaos game without adding
  samples, so it is a *convergence* knob and close to free — not a density knob
  that costs time. The hint is wrong either way; whether the flag is also
  underperforming is worth its own look. Low priority now that `accumulate`
  lives in the non-default "one pass" branch.
