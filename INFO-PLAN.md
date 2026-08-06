# Plan: `--info` for the four people who read it

*Claude Opus 5, 2026-08-06. A review of `--info` as it stands at `007a9d8`,
read from source and from ~20 live invocations. Every byte count below was
measured in this checkout; token counts are estimates and are labelled as
such. §9 is the worklist.*

> **Status: landed 2026-08-06**, Tier 1 and Tier 2 (items 1-15). Only item 16,
> `--info --json`, is outstanding — deliberately, per §6: it is worth nothing
> until there is an open release and real money the day there is one.
>
> Measured on blossom against `7a9dcde`: **33 lines → 67, 2837 B → 3356 B, and
> about 20% *fewer* tokens** — the swatch leaving pays for everything added.
> The plan guessed 45 lines and −40%; both were optimistic, because the
> per-transform colour, the `set` and `notes` blocks, the path keypoints and
> the zoom rows are more content than the mockup showed. Three golden files in
> `tests/golden/` hold the layout, and `no_line_is_wider_than_the_margin`
> holds rule 5.

> **Method.** `--info` has four consumers with genuinely different needs, and
> the plan is organised around them rather than around the code. I ran the
> real binary over blossom / rimefall / koru / jspace / hazels-default /
> `--random` / `--blank`, with and without `--view`, `-S` and `--zoom`, and
> measured the output. Where the report and `AGENTS.md` disagree with each
> other, or with what the terminal actually does, I say so.

---

## 0. The headline

`src/info.rs` is a good piece of work and its two stated conventions — fixed
schema, one writer per quantity — are the right ones. The problems are that
**neither convention is actually applied outside the `view:` block**, that
**a third of the output is a decoration its most frequent reader cannot
see**, and that **one of the three levers `CRAFT.md` says the craft consists
of is not reported at all**.

Six findings, in the order they cost something:

1. **The ANSI swatch is 31–33% of the report's bytes and roughly half its
   tokens, and no text-based agent can see it.** `info.rs:214` emits 48
   24-bit background escapes unconditionally, "including when stdout is a
   pipe", on the reasoning (`AGENTS.md:833-836`) that "an agent that can see
   the gradient makes better decisions than one imagining it from floats."
   An agent reading Bash output cannot see it — it receives the escape bytes
   as literal text. Measured: 898 B of 2837 B on blossom, 967 B of 2898 B on
   jspace. The hex ramp on the next line is the channel that actually works.
2. **Per-transform colour is missing entirely.** `CRAFT.md:48-52` names three
   levers — form, density, colour — and says "the gap between a competent
   flame and a good one is almost always density and colour". `--info`
   reports form completely, density mostly, and of colour reports only the
   scene-global gradient. `color_value`, the per-map RGB, and the resolved
   per-map `color_speed` never appear (`info.rs:285-314`). Blossom's maps sit
   at 0.12 / 0.60 / 0.72 / 0.84 in its gradient; the report cannot tell you
   that.
3. **`contraction()` hides reflection and expansion** (`scene.rs:142-144`:
   `.abs().powf(1/3).clamp(0.05, 0.95)`). A mirrored map and a plain one
   print identically; an *expanding* map prints `0.950`. So when the report
   says "the chaos game does not converge" (`info.rs:347`) it has already
   thrown away the one number that would say which map did it.
4. **`-S/--set` is applied silently.** `main.rs:1341-1343` patches the scene
   before the report; the report shows the result and never says the command
   line moved anything. `--view` gets a whole block saying what each value
   replaced (`info.rs:140-204`) — the same question about `-S` is
   unanswerable. Verified: `-S meta.haze=0.9` prints `haze 0.90` with no
   trace of the file's `0.50`.
5. **The fixed-schema convention is broken in the block read most.** The
   variation line is skipped when a map is plain linear (`info.rs:310-313`),
   so transforms are two lines or three depending on content, and two
   reports no longer diff row for row — which is the entire stated reason
   the convention exists (`AGENTS.md:1286-1290`).
6. **Sections are unlabelled and unordered by value.** Four of the nine
   sections have no header at all; the two that matter most to a reader
   arriving cold — the measurement, and the problems — are at position 3 and
   scattered inline respectively.

Two smaller bugs found on the way, noted here and out of scope for this
plan: `-S` is a **silent no-op** on `--blank` and `--random` (it patches TOML
text before parsing, `src/set.rs`, and generated scenes have no text —
verified: `--random -S meta.haze=0.99` still reports `haze 0.35`), and an
unknown *key* in a known table (`-S meta.nonsense=5`) exits 0 rather than
erroring as `AGENTS.md:1365` promises. Both are `set.rs` problems.

---

## 1. Who reads this, and what each one needs

With Hazel's measured weights. **Today: 95 / 5 / 0 / 0.** Expected after an
open release: **50 / 40 / 10**, near-frontier-in-a-harness / human / small
local model. Two things follow, and they pull in opposite directions only if
you let them:

- The **default** output must be tuned for C1, because C1 is 95% of calls
  today and ~50% forever. Anything a human wants that a model cannot use is
  opt-in, always.
- **Eyeball quality is not a rounding error** — it is 5% now and 40% later,
  so it is worth building for now and cheap to keep. §5 argues it is very
  nearly the *same* build.

### C1 — Claude Code agents (Opus 5 / Fable 5) — 95% today, ~50% later

Reads the report as Bash tool output: plain text, no TTY, no colour. Pays
tokens per byte, and calls `--info` many times in one session — the explore
loop is one call per candidate. What it needs, in order:

- **The next command.** The zoom section already does this perfectly:
  `--zoom descent` is a copy-pasteable fragment. Nothing else in the report
  does. `suggests: camera distance ~1.42` is a number the agent must then
  hand-assemble into `-S camera.distance=1.42`.
- **A verdict it can branch on.** "Is this scene worth rendering, or is
  something wrong with it?" — currently answerable only by reading all 33
  lines and noticing the `NOTE:` that may or may not be there.
- **Stable line shapes**, so `grep`/`sed` over the output is reliable.
- **No decoration.** See finding 1.

### C2 — Hazel, XFCE terminal — 5% today

Lives in the GUI; pokes the CLI for fun. That frequency matters for one
reason: **an occasional reader has no memorised layout**, so the report has
to re-teach its own shape every time it is read. It cannot rely on "you'll
learn where things are" the way a daily tool can. Wants the headline before
the data, columns that line up under the eye, and the swatch — which is
genuinely the best thing in the report for this reader.

### C3 — other humans, CLI-first, after an open release — 0% today, ~40% later

C2 without the priors, and the largest single growth in the mix. Note that
C2 and C3 merge into one design target: an infrequent human reader with no
memorised layout. Optimising for C2's *rarity* is what serves C3's
*inexperience*.

This reader doesn't know what "contraction", "occupancy",
"octaves/period", or "95th pct" mean. Needs the report to be *self-locating*
— terse enough not to lecture, but with terms spelled consistently enough
that `grep contraction AGENTS.md` lands. Does **not** need an inline
glossary; that would tax C1 on every call for a one-time cost.

### C4 — small local models, other harnesses — 0% today, ~10% later

The hard one, and the least frequent in both the present and projected mix. Limited
context, unusual tokenizers, poor generalisation. Will not reliably extract
`~1.42` from *"suggests: camera distance ~1.42 (fills the frame),
point_size ≤ 0.0004 (stays on the crisp 1px path at 1080p)"* — that sentence
has an em-dash-adjacent tilde, a `≤`, two parentheticals, and a unit implied
three words away. It *will* read `camera.distance.suggested  1.42`.

Serving C4 inside the prose report means flattening the prose, which costs
C2 and C3 the thing they like about it. **So C4 gets a separate emitter, not
a compromise** (§6). This is the one place a second output format earns its
keep.

---

## 2. What `--info` is *for* — the two jobs, and the one it does badly

Reading the authoring loop in `CRAFT.md` and
`memory/fracturize-render-workflow.md`, `--info` is called at exactly two
moments:

**Orient.** A scene you have never seen — a `--random` roll, a `.mutN.toml`
the mutation sheet produced, someone else's file. "What is this, is it
sound, and what should I do to it next?" `CRAFT.md:43` puts it plainly:
*"You cannot read a TOML and know what it looks like. Render it. This is why
`--info`'s `measured:` block exists and why you should read it first."*
The report is good at this and the layout disagrees with the doc — the block
you should read first is third.

**Verify.** You changed something — a hand edit, a `-S`, a `--view` — and
you want to know it landed and what else it moved. This is a **diff** job:
`diff <(fracturize -s a.toml --info) <(fracturize -s b.toml --info)`. The
report is poor at this, for three separate reasons: `-S` is not attributed
at all (finding 4), the transform block has variable line counts (finding
5), and `point_size` is printed with a raw `{}` at `info.rs:356` instead of
the `size()` helper — so a file scene prints `0.0018` and a `--random` scene
prints `0.001209969`, and the column moves.

A third job is *latent* and worth making explicit: **decide**. An agent
running the breed-and-select loop wants one line it can branch on before
spending a render. That is what §4's `notes` block is for.

---

## 3. The colour decision, and the token arithmetic

This is the finding with a number attached, and it answers Hazel's framing
question — where the line between "eyeball-friendly alignment" and "wasted
tokens" actually sits.

Measured on blossom's 2837-byte report:

| | bytes | share | est. tokens |
|---|---|---|---|
| the 48-block ANSI swatch (one line) | 898 | 31.6% | ~640 |
| everything else (32 lines) | 1939 | 68.4% | ~500 |

The token column is an estimate and should be read as an order of
magnitude, but the direction is not in doubt: escape sequences are runs of
digits and punctuation, which BPE tokenizers split near one token per one or
two characters, while English prose runs nearer one per four. **The swatch is
probably a little over half the tokens of the entire report**, for a line
that renders as grey blocks in an agent's context.

The corollary matters just as much and points the other way. A run of
spaces merges into one or two tokens in any BPE tokenizer — it does not cost
one token per space. Padding all 33 lines out to aligned columns costs on the
order of 260 bytes and *tens* of tokens. So:

> **Alignment is nearly free; the swatch is not.** `AGENTS.md:1296-1297`
> currently says "padding out to a grid just burns another agent's context",
> which is measurably backwards relative to what is actually burning it. Buy
> the alignment. Sell the escape codes.

**Recommendation: a plain opt-in `--color` flag, default off.** Not
`isatty` auto-detection, which was the first instinct and is worse:

- **The default should serve 95% of calls, and that majority is a pipe.**
  Auto-detection gets the same answer nine times in ten but arrives at it
  non-deterministically — the same command emits different bytes depending
  on how the harness spawned it. A golden test then has to pin the
  environment, and an agent in a pty-allocating harness silently pays the
  900 bytes with no way to know why.
- **Opt-in is honest about who colour is for.** It is a human affordance.
  A flag says so; a heuristic pretends the tool can tell.
- One rule, everywhere: **ANSI escapes appear if and only if `--color` was
  passed.** That governs `swatch()` (`info.rs:214`), `color_blocks()`
  (`:226`) and `--palettes` (`main.rs:698`). `--palettes` without it prints
  names plus hex ramps, which is *more* useful to the 95% reader than names
  plus invisible blocks.
- No short flag — `AGENTS.md:1526` rations those, and adding one costs the
  next flag its obvious letter.

**Colour is additive, never substitutive.** `--color` must not *replace* the
hex ramp with a swatch; it *adds* the swatch line, and tints each hex stop
and each per-map colour in place. So the two outputs differ by exactly one
line, the coloured report is a strict superset of the default, and the
golden tests test what 95% of callers actually receive. Hazel gets the
continuous ramp — which shows where a gradient ramps and where it goes flat,
something eight discrete stops genuinely cannot — sitting directly under the
numbers it describes.

Independently of the flag, widen the hex ramp from 8 stops to 12
(`info.rs:405`, ~40 B). Hazel's point stands on its own: a model reads RGB
triples better as plain text than as the same triples wrapped in escape
codes, so the plaintext channel should carry the resolution.

Fix the claim at `AGENTS.md:833-836` at the same time — leaving a false
rationale in the doc is how it gets re-implemented later.

---

## 4. `notes:` — one block an agent can branch on

The report already emits diagnostics; it just scatters them. Today they live
at `info.rs:344` (framing mismatch), `:347` (does not converge), `:414`
(flat palette), `:417` (scene also carries a palette), `:419-425`
(color_contrast stretch), `:508` (zoom broken), and inside
`Renorm::summary()` (the edge-guard warning) — six sites, no common shape,
no way to know from the top whether any fired.

**Proposal.** A `notes` section, second, listing every diagnostic with a
one-word section tag, and a count. When nothing fired it prints `notes
none` — one line, and the cheapest possible "this scene is sound" signal for
C1.

The note text lives **only** in this block. The inline sites keep the
*numbers* (the `shape` section still prints the suggested distance and the
scene's actual); the *judgement* moves up. No duplication, so the block is
close to free.

Notes to carry over: framing mismatch, non-convergence, flat palette
(swing < 0.15), unreachable palette range from `color_contrast`, dormant
`[palette]` in transforms mode, zoom broken, edge-guard ramp in view.

Notes to add, each of which is a real failure mode with no current signal:

- **`point_size` above the crisp bound.** The bound is computed at
  `info.rs:337` and compared against nothing. This is the single most common
  hand-authoring error per the workflow memory ("stellate's fat points bug").
- **A map that expands or reflects** (finding 3) — and when the walk
  diverges, name the map.
- **A zero-weight transform**: it is in the file, in the report at `0.0%`,
  and never fires.
- **`--view` loaded against a different scene than its `of scene` names.**
  Verified live: loading `views/nautilus-*.toml` over `hazels-default.toml`
  prints `of scene  scenes/nautilus.toml` next to a completely different
  attractor, with no warning at all.
- **Camera focus far from the measured centre** — the report has both
  numbers (`info.rs:323` and `:442`) and never compares them.

---

## 5. Layout: the vision

### 5.1 One page, not two

The thing that makes this tractable: **a near-frontier model reading text and
a human skimming a terminal want very nearly the same page.** Both segment on
whitespace, both use position as a proxy for importance, both are hurt by
prose that hides numbers inside sentences and by arithmetic left for the
reader to do. Where they differ, they differ narrowly:

| | the eye | frontier model | small model |
|---|---|---|---|
| ANSI colour | the payload | invisible noise | invisible noise |
| long lines | costly — the eye loses its place | cheap | costly |
| a row that wraps | recoverable | alignment destroyed | destroyed |
| a column header 12 rows up | tracked visually, free | weaker | fails |
| repeated labels | reads as noise | harmless | load-bearing |
| total length | scrolling | tokens | context |
| implied units | inferred from context | usually inferred | not inferred |

So the design rule is one sentence:

> **Design the page for the eye, subtract the colour, and cap the line
> length. That page is also the agent's page.**

Every row of that table is satisfied by the same artifact except colour
(§3's flag) and header-distance (Rule 3 below). There is no format
compromise to negotiate, which is why I am not proposing a `--brief` or an
agent-specific mode. The 95:5 present and the 50:40 future want the same
thing.

The target genre is **instrument panel** — `git status`, `df -h`,
`systemctl status`. Not prose-with-numbers, which is what the report is now,
and not a spreadsheet, which is unreadable at this width. Labelled values,
grouped, verdict at the top, and sentences only where a sentence is
genuinely the payload.

### 5.2 Seven rules

**Rule 1 — a section keyword in a fixed left gutter.** Eight columns,
lowercase, one word, continuation lines indented into it. This is the
hanging-indent definition list — the most scannable layout in text, which is
why dictionaries, man pages and `git status` all converge on it. For a model
it is a flat key namespace; for a small model it is `^(\w+)\s+`, trivially
parseable. One device, all four readers, about eight tokens for the whole
report.

**Rule 2 — importance by position, in three depths.** Line 1–2: what is
this. Lines 3–8: is it sound, what do I do next. The rest: detail. A human
stops reading when satisfied; a model receives it all but attends hardest to
the top. This is the single decision that serves the 95% case hardest, and
it is why `notes` goes second (§4).

**Rule 3 — short blocks label in the header; long blocks label in the row.**
This corrects the flat "say the unit once" rule. Within a block the eye can
take in at a glance — six rows or so — a header carries the vocabulary and
the rows carry bare numbers, which is how `render` and `colour` should look.
But the transform list is fifteen rows, and by map 5 the header is twelve
lines up: the eye tracks that column for free, a frontier model less
reliably, a small model not at all. So the long block self-labels
(`contraction 0.600`, not a bare `0.600`). It costs about two tokens a row
and it is robust at any distance. **"Say it once" is a rule about a section,
not about a report.**

**Rule 4 — decimal points line up, and the sign gets its own column.** What
makes a numeric column readable is the decimal point, not the digits;
`info.rs` already has `{:>8.3}` helpers that do this and uses them in
exactly one block. Extend them everywhere. Reserve the sign column too —
otherwise every negative value shifts its row one character and the eye
loses the edge it was following. That is one space, and it is the same fix
`UI-REVIEW-PLAN.md §6.7` wants for the GUI's DragValues.

**Rule 5 — 78 columns, hard.** Two constraints coincide here: an 80-column
terminal is still the human floor, and a wrapped row is *worse than no
table* for a model, because wrapping destroys exactly the alignment the
table was built to provide. This constraint is what rules out the tempting
one-line-per-transform layout, which needs about 120 columns.

**Rule 6 — spend blank lines freely.** A blank line costs one token and is
the strongest segmentation signal available to both readers. The current
report has 5 in 33 lines; the target is roughly one per section. This is the
cheapest eyeball-friendliness in the entire document.

**Rule 7 — every row prints every time, and footnotes go last.** Finish the
convention `AGENTS.md:1286` already states: the variation line prints
`linear` rather than vanishing, `point_size` goes through `size()`, the
transform block is a fixed three lines per map. And where prose genuinely
earns its place — the euler-branch caveat, "a view sets only the rows above"
— it belongs at the *end* of its section. The euler caveat is currently
welded to the section header (`info.rs:281-284`), which is the highest-value
real estate in the block, spent on a footnote.

### 5.3 What this costs

Longer in lines, shorter in bytes: roughly 45 lines against today's 33,
against a ~40% drop in tokens once the swatch is opt-in. That is the right
trade for this mix — line count is a human cost (scrolling) and byte count
is the agent cost, and the agents are 95% today and 50% later. 45 lines
still fits a maximised terminal; it does not fit an 80×24, but neither does
the current 33, so nothing is lost that was there.

If a genuinely short form is ever wanted, it should be a separate flag
rather than a compromise in this one — and the material for it already
exists as the two-line `scene` header plus `notes`. **Not now:** it is
speculative, and the whole argument above is that one page serves everybody.

### 5.4 The mockup

Applied to blossom, with the real numbers from this checkout (per-map
colours are the file's `color_value`s: 0.12 / 0.60 / 0.72 / 0.84; the hex
shown is the palette sampled *at* that index, i.e. the colour the map
actually renders as, which is the thing you cannot get from the file):

```
scene    Blossom                                    scenes/blossom.toml
         Claude Opus 5 · 5 maps · palette · zoom on · 8.0M points

notes    1
         zoom   edge-guard ramp reaches into the view; raise zoom.radius

shape    centre     ( 0.184,  0.026,  0.140)
         spread     ( 0.231,  0.039,  0.187)   occupancy   4.5%
         radius        0.592  (95th pct)
         fills frame at distance 1.42, point_size <= 0.0020
         scene has              2.040                0.0018

maps     5   share of the walk · contraction · colour at its palette index
             (rotations re-derived from the matrix; euler branch may differ)
  [0] bough        50.5%   contraction 0.600   #26202b @0.12
      scale  0.600                  rot  (  -0,  32,  -0)°
      move  ( 0.000,  0.000,  0.000)                   linear
  [1] floret-1     16.0%   contraction 0.450   #d18ea0 @0.60
      scale  0.450                  rot  ( -24,  16,  -0)°
      move  (-0.320, -0.300, -0.780)   absfold 0.55 + linear 0.45
  ...

render   point_size    0.0018   point_count   8000000
         haze            0.50   background    (0.010, 0.008, 0.016) linear

colour   mode        palette   7 stops, oklab
         speed          0.50   falloff  0.60   contrast  1.00
         luminance   mean 0.25   swing 0.91
         #0d0a10 #28212a #4c3842 #76505c #b16c80 #db94a7 #f5c4d0 #d6cace
         <swatch, TTY only>

camera   yaw   0.5585   pitch  1.1200   roll  0.0000
         distance      2.040   focus  ( 0.000,  0.000,  0.000)
         from the scene · infinite zoom, canonical period
path     none authored (the default full-turn turntable applies)

zoom     ON   map [0] bough   fixed point ( 0.000,  0.000,  0.000)
         scale         0.600   0.74 octaves/period, 32° twist
         radius         1.60   levels  14.0   octave_falloff  1.00
         edge_guard     1.00   octaves (1.60x-3.20x the eye distance)
         rendered   19 periods (14.0 octaves)
```

Notes on the mockup:

- The `zoom` section is the change with the most content behind it. Today
  every one of those numbers is welded into one 240-character prose sentence
  from `Renorm::summary()` (`info.rs:504-507`) — unparseable by C4, hard to
  diff, and it hides that `radius`, `levels`, `octave_falloff` and
  `edge_guard` are the four things you would actually *set*. Rows, and
  `Renorm::summary()` stays for the GUI.
- The zoom-**off** eligibility list keeps its current shape exactly. It is
  the best thing in the report: `--zoom descent` is a command you can run.
  Extend the idea rather than change it — `shape` should print
  `-S camera.distance=1.42` rather than `~1.42`, and the notes block should
  name the settable path (`zoom.radius`) as it does above.
- Numbers are right-aligned in fixed-width fields so a column of values has
  one edge. Signed values reserve the sign column, which is why `( 0.184,`
  has a leading space — this is the same fix the GUI plan calls for at
  `UI-REVIEW-PLAN.md §6.7`, and for the same reason.
- Order follows §2: identity, verdict, measurement, then detail. `shape`
  moves from third to third-but-shorter and `maps` moves down, so the doc
  and the layout finally agree about what to read first.

Estimated cost of the whole rework for C1, versus today: **down**, on the
order of 40%, dominated by the swatch leaving. The added colour, note and
zoom rows are worth perhaps 15 lines of short numeric rows; the alignment is
noise against that.

---

## 6. `--info --json` for C4, and for scripting

One emitter, one data model, two renderers. Build the report as a value —
sections of typed rows — and render it as text or as JSON.

What it buys, honestly stated:

- **C4 completely.** A 3B model with an odd tokenizer parsing
  `{"shape":{"radius":0.592,"suggested_distance":1.42}}` is doing a task it
  is reliable at, instead of one it is not.
- **C1's scripting.** `--info --json | jq '.maps[].share'` makes a
  breed-and-select gate a shell condition. Today that needs `sed` against
  prose.
- **Drift protection.** Once the text renderer is purely presentational, a
  new field cannot be added to one output and forgotten in the other. The
  same argument that made `offline::effective_camera` shared between
  `--info` and `--render` (`info.rs:255`).

What it does **not** buy: tokens. JSON of the same data is *more* bytes than
the text report — braces, quotes, repeated keys. It is a reliability and
scripting feature, not an efficiency one, and C1 should keep using the text
form.

**Tier 3, and specifically a pre-release job.** C4 is 0% of calls today and
~10% after an open release, so this is worth exactly nothing until there is
a release and worth real money at it. It also has to come after §3–§5: the
value of a shared data model is that the two renderers cannot drift, and
that guarantee is worthless if it is locked in around a text layout still
being changed.

One consequence worth planning for now, though, at no cost: **build the
report as a value from the start, even while there is only one renderer.**
Sections of typed rows, rendered to text at the end. That is a better shape
than today's `String`-append (`info.rs:262-266`) regardless of JSON, it is
what makes the `notes` block collectable rather than scattered (§4), and it
means the eventual JSON emitter is an afternoon rather than a rewrite.

---

## 7. What stays exactly as it is

Worth writing down so a later pass doesn't "improve" them:

- The **zoom eligibility list with its per-map reason** (`info.rs:519-543`).
  Best-designed part of the report.
- The **measurement block existing at all** — running CPU walkers to report
  what a file cannot say is the whole justification for the command.
- The **`view:` block's `what it replaced` notes** (`info.rs:155-199`). This
  is the pattern §4's `set:` block should copy, not replace.
- **Radians with degrees alongside, never one alone** (`info.rs:71-73`).
- The **euler-branch caveat**. It is four tokens of self-defence against a
  bug report and it stays.
- **No inline glossary.** C3's needs are met by consistent terminology plus
  `AGENTS.md`, not by taxing C1 on every call.

---

## 8. The one question, and why the weights answer it

I had this open: **should `shape` print settable paths
(`-S camera.distance=1.42`) or bare numbers (`~1.42`)?** The settable form
costs about six characters and hands C1 and C4 a runnable command; the bare
form reads slightly better to an eye that already knows the flag.

The 95:5 / 50:40 weights settle it: **settable.** The reader who has the
flags memorised is the smallest slice of the mix in both the present and the
projected one, and the occasional human of §1 — who has *not* memorised them
— is served by the settable form too, because it is the rare reader who most
needs to be told the flag rather than reminded of it. The zoom eligibility
list has been quietly proving this for months; it is the best feature in the
report precisely because it emits commands rather than facts.

The general principle, worth keeping: **where the report has computed a
number the reader will act on, emit the action, not just the number.** It
applies to the suggested distance and point size, to `zoom.radius` in the
notes block, and to any check added later.

§5.4's mockup is written that way. Nothing here blocks the worklist.

---

## 9. Worklist

**Tier 1 — the report currently misleads, or omits a lever.**

1. `--color`, opt-in, default off, governing `--info` and `--palettes`
   alike; additive (adds the swatch, tints hexes in place, replaces
   nothing). Widen the hex ramp to 12 stops unconditionally. Fix
   `AGENTS.md:833-836`. (§3) — *biggest single win, smallest single change.*
2. Per-transform colour: `color_value`, and the palette sampled at it (or
   the map's own RGB in transforms/mix mode). (§0.2, §5)
3. `contraction` stops hiding reflection and expansion; when the walk
   diverges, name the map that did it. (§0.3)
4. A `set:` block attributing each applied `-S`, in the shape the `view:`
   block already uses. (§0.4)
5. Fix the schema breaks: the variation line always prints; `point_size`
   goes through `size()`. (§0.5, §2)
6. The `notes` block, with the seven existing diagnostics moved into it and
   the five new ones added. (§4)

**Tier 2 — completeness and layout.**

7. The report becomes a value — sections of typed rows — with a single text
   renderer over it. Precondition for 6, 8 and 14, and better shape than
   `String`-append regardless. (§6)
8. The seven layout rules: gutter sections, three-depth ordering, short
   blocks labelled in the header and long ones in the row, decimal and sign
   alignment, the 78-column cap, blank lines per section, footnotes last.
   (§5.2)
9. `zoom` ON becomes rows — `radius` / `levels` / `octave_falloff` /
   `edge_guard` visible as the settable things they are. (§5)
10. Suggestions become settable paths — `-S camera.distance=1.42`, and
    `zoom.radius` named in the notes. (§8)
11. Per-transform `color_speed` when it differs from the global — explicit,
    or resolved through `color_falloff`. (§0.2)
12. Camera path keypoints, not just the count and duration. Path authoring
    is a documented workflow and the keys are its subject.
13. Default-vs-file provenance where a default is silently substituted
    (`point_count` → 500 000).
14. `--random`'s reproduce line inside the report, not only on stderr.
15. Golden-file tests, landing **with** item 8 rather than after it — a
    layout rework is exactly when the fixed-schema convention needs
    something checking it, and diff-stability is half of what the report is
    for (§2). Blossom (palette + zoom on), a `--random` roll (mix mode),
    `--blank` (transforms mode, zoom off), and one with `--view` + `-S`
    layered. Default output only, which §3's additive colour rule makes
    possible.

**Tier 3 — before an open release, not before that.**

16. `--info --json` over the item-7 data model. Worth nothing until C4
    exists; worth real money the day it does. (§6)

**Out of scope, but found here and worth filing:** `-S` silently no-ops on
`--blank` / `--random`, and an unknown key in a known table exits 0 rather
than erroring as `AGENTS.md:1365` promises. Both are `src/set.rs`.
