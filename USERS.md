# Who Fracturize is for

*Status: draft, 2026-10-25. Written by Claude Opus 5.5 from Hazel's brief and
one round of answers. People to plan for, by name, so a plan can say "this is
for Jay and costs Theo" instead of "power users". Names are settled; the rest
is Hazel's to edit.*

Seven personas and three anti-personas. Each persona is a **type of user**,
not a person; pronouns are *they* throughout.

| | Who | In a line |
|---|---|---|
| **Rook** | fractal-art expert | Knows Apophysis cold. Builds on purpose, then renders. |
| **Pip** | newcomer | Never made fractal art. Clicks to find out what's possible. |
| **Jay** | hobby tweaker | Finds random flames and polishes them until they're right. |
| **Theo** | mathematician | Wants the classic sets, then their own, with every number shown. |
| **Remy** | agent-art composer | Has their agent make fractal layers to mix into video and music. |
| **Atlas** | frontier agent | Large model in a coding harness. Capable, confident, can fix the code. |
| **Mote** | small local agent | Small open model in a thin harness. Needs one clear next step. |
| *Abe* | *anti: the Apophysis devotee* | *Wants Apophysis, exactly.* |
| *Silk* | *anti: the smooth-render purist* | *Wants every render converged and grain-free.* |
| *Socket* | *anti: the pipeline integrator* | *Wants a plugin for the tools they already use.* |

The names are mnemonics. A rook moves on purpose. A pip is a seed. A jay
collects shiny things. Theo is for theorems. Remy remixes. Atlas carries a lot
and sometimes too much. A mote is small and can still catch the light.

**Hazel is a user too, and not a persona.** She is the first user and one
target audience in her own right: the program has to suit her whether or not
she is typical. Jay is the wider class of people who use fractal tools the way
she does, and she may not be a central example of it. When Jay's needs and
Hazel's differ, both are real; neither overrules the other by default.

## How to read a persona

Every persona has the same fields, in the same order, so they can be compared
across:

- **Bio**: the short version to keep in your head.
- **Goals**, at three levels: how they want to *feel* while using it, what
  they want to *finish*, and who they want to *become* over months.
- **Sessions and setting**: how long, how often, where, on what machine, and
  where the work goes afterwards.
- **Brings**, **Fails by**, **Already served by**.
- **Gaps**: their user stories.
- **Success looks like**: something we could check.
- **Signature scenes**: the face of the persona, meaning the pictures they would make
  or reach for first. From `scenes/` and from outside.
- **Moves on to**: where they go next, and what helps them get there.
- **Grounded in**: what the persona is built from, and how solid that is.
- **Speak to them in**: the voice our text should use for them.

---

## Rook: the expert

> Has made fractal flames for years in Apophysis, Chaotica or JWildfire.
> Knows what a post-affine is for, uses xaos on purpose, and has a set of
> techniques they reach for. Comes to rebuild things they've made before, and
> to build new ones deliberately: place a transform, give it variations, tune
> it by eye, render.

**Goals.** *Feel:* fluent; their hands know where things are. *Finish:* a
deliberate piece, rebuilt or new, rendered big. *Become:* someone who works in
3D natively, with techniques that only exist here.

**Sessions and setting.** Long, focused evenings, several a week. A desktop
with a decent graphics card, quite likely Windows, since that's where
Apophysis lives. Work goes to a gallery (DeviantArt and its successors),
prints, contests.

**Brings:** muscle memory from another program; the vocabulary (xform, final,
post, xaos, linked transform, gradient); opinions on the look; an archive of
`.flame` files.

**Fails by:** reaching for an Apophysis idiom and finding nothing there, or
something different. Assuming a 2D trick carries into 3D unchanged. Losing
trust when the tool silently does something they didn't ask for.

**Already served by:** post-affine; xaos with Apophysis 7X's goes-to /
comes-from view and linked maps; palette mode with `.ugr`/`.gradient` and
`.flame` palette import; per-transform `color_value`; typed inspector fields;
render jobs with presets (theirs saved by name, as in Apophysis), supersampling,
density estimation and the grade; Ctrl+Y as redo.

**Gaps:**
- As Rook, I want to import a `.flame` file's transforms, not just its
  palette, so I can start from work I've already done. (Decide what 2D→3D
  mapping is honest: the affine is 2×3 and most variations are 2D.)
- As Rook, I want a final transform, or to be told what to use instead.
- As Rook, I want the variations I rely on, with their parameters.
- As Rook, I want to move several transforms together (multi-select, deferred).
- As Rook, I want to grade a render in the GUI, not only from the CLI.
- As Rook, I want a short "coming from Apophysis" page: what maps across, what
  differs, and why.
- As Rook, I want it to run on my machine (is Windows supported?).

**Success looks like:** Rook rebuilds a flame they know, by hand, in one
evening, and the render is one they'd post.

**Signature scenes:** `flame_of_flames_opus55` (swirl centred on each lick's
root by a post-affine), `parasol` (xaos cuts doing real work), `lacewing`,
`blossom`. Outside: the Apophysis gallery classics, julia-and-spherical
compositions, the linked-transform glow trick.

**Moves on to:** native 3D techniques: symmetry groups, subscenes, infinite
zoom. Or towards Theo, when they want to know *why* a trick works.

**Grounded in:** assumption, plus public accounts such as Stacy Reed (Sya),
who came from graphic design, calls Apophysis her favourite tool, and helped
found its DeviantArt community. Medium-weak.

**Speak to Rook in:** their terms first, ours second. Where we differ on
purpose (ctrl snaps, scroll never edits), say so once, plainly.

---

## Pip: the newcomer

> Has never made fractal art and doesn't know what it can look like. Willing
> to click, drag and tweak, but has no idea what to expect from any of it.
> Needs to find out what's possible, find something beautiful, and always be
> able to get back to where they were.

**Goals.** *Feel:* safe and delighted; nothing they do can break it. *Finish:*
one picture they're proud of, saved. *Become:* someone who knows what they
like and how to get more of it (Jay).

**Sessions and setting.** Short and exploratory: twenty minutes at a time,
irregularly, and the first session decides whether there is a second. Whatever
laptop they own, often integrated graphics. The picture goes to a wallpaper,
a friend, a social post.

**Brings:** curiosity and no vocabulary. "Transform", "variation" and
"weight" mean nothing yet.

**Fails by:** landing on a blank or ugly scene and leaving. Changing three
things, not liking it, and not knowing which did it. Opening a panel full of
numbers and closing the program. Meeting a control that seems to do nothing
(disabled, or only meaningful in another mode). A slow machine: three frames
a second reads as broken.

**Already served by:** the Random flames list; the Scenes browser with live
thumbnails; U to mutate; one-step undo of every edit and the History window;
disabled-not-hidden controls that say why; tooltips and status-bar hints on
every widget; Escape that never quits; the unsaved-edits prompt; the point
count slider that lets a weak machine stay responsive.

**Gaps:**
- As Pip, I want to open the program on something beautiful, not a blank
  canvas, and be shown three things to try.
- As Pip, I want to see eight variations of this one and pick the one I like
  (the mutation grid: built 2026-10-26, in the Mutate window).
- As Pip, I want to know what a word means where I meet it.
- As Pip, I want to try a slider and see what it does without committing.
- As Pip, I want to save a picture I'm proud of without learning what
  supersampling is.
- As Pip, I want the program to pick a sensible point count for my machine on
  first run.

**Success looks like:** within the first five minutes, Pip has saved a
picture they like, and has used undo at least once without fear.

**Signature scenes:** `blossom`, `nautilus`, `koru`, `jellyfish`, `galaxy`:
beautiful on first open, and readable at a glance.

**Moves on to:** Jay, by way of the Random flames list and Mutate. Helped by
words that teach as they go, and by "what changed?" being answerable.

**Grounded in:** assumption, plus accounts like Satania's, who struggled
with Apophysis for two weeks and left for Ultra Fractal for several years
before coming back. A first experience can lose someone for years. Weak.

**Speak to Pip in:** effects, not mechanisms. "Makes the far parts fade",
not "aerial perspective transmittance".

---

## Jay: the tweaker

> Has spent a lot of hobby time in fractal tools and knows a handful of
> tricks, but mostly explores. Likes to find a random flame and polish it:
> recolour it, reframe it, nudge one weight until it's just right, then
> render it. No house style. Will try whatever the program offers, and wants
> both odd corners to wander into and fine control over the tiniest detail.
> Has some background in other arts.

**Goals.** *Feel:* absorbed; time disappears. *Finish:* a polished piece from
an unexpected start. *Become:* someone with a wider range, who has found
corners nobody pointed them to.

**Sessions and setting.** Long, unfocused, wandering sessions, sometimes many
hours, picked up and dropped. A last-generation graphics card, or an old
enthusiast's laptop that was very good some years ago. Work goes to
wallpapers, prints, a gallery, a folder of favourites.

**Brings:** taste from other arts; a habit of exploring sideways; patience
for small adjustments; tricks picked up from other tools.

**Fails by:** losing a good state somewhere in a long session. Fine control
that isn't fine (sticky drags, jitter). Running out of novelty because every
roll looks alike. Finding that a feature exists only on the CLI. Colour: the
gradient is where they get stuck.

**Already served by:** the Random flames list with dice profiles
(colour-only, no-colour, symmetric, their own); Mutate with strength; the
History window; `num::Drag`'s even steps and shift for fine; the camera path
and render jobs; the volume renderer; symmetry groups and repeats; subscenes;
saved views.

**Gaps:**
- ~~As Jay, I want the 3×3 mutation grid, so mutation is "look and choose"
  instead of "roll, judge, undo".~~ Built 2026-10-26: the Mutate window.
- As Jay, I want grade controls (gamma, threshold, vibrancy) in the Render
  window, saved in views.
- As Jay, I want to bookmark a state mid-session and come back to it, apart
  from undo.
- As Jay, I want xaos and infinite zoom as dice parts, so rolls reach those
  corners too.
- As Jay, I want to compare two versions side by side.
- As Jay, I want help with colour: palettes suggested from the form I have.

**Success looks like:** in one session Jay goes from a random roll to a render
they'd keep, never loses a state they wanted back, and finds at least one
thing they hadn't tried before.

**Signature scenes:** `random-2578229891-florable`,
`random-417545180-dragonsplosion`, `lacewing-alter1`, `wellspiral-edit1`,
`kintsugi`. The roll-then-polish history is already in the repo.

**Moves on to:** Rook, if a technique sticks; Theo, if a form makes them ask
why; Remy, if they start composing with an agent.

**Grounded in:** Hazel, and L. James Lucas (bampster), who starts from a
random Apophysis flame, spends "many hours of twists turns and
manipulations" reshaping it, finds "the maddening array of gradients" the
hardest part, and works in Mandelbulb3D and pen and ink the same way. Strong.

**Speak to Jay in:** plain words, with the depth one click away.

---

## Theo: the mathematician

> A mathematician, or a keen amateur, who knows the classic fractals inside
> out and wants them in 3D and at high resolution. Wants to bring in recent
> theory and use it straight away, with nothing hidden. Wants numbers and
> equations, grouped and laid out neatly. Starts by rebuilding a classic
> (Sierpiński, Menger, Barnsley, a Rauzy fractal), then moves on to their own.
> Goes from code, or from an exact definition, to a picture.

**Goals.** *Feel:* in control, and trusting that what they see is the object
they defined. *Finish:* a faithful, high-resolution picture of a specific set.
*Become:* someone who has seen their own fractal for the first time, and can
show it to others (papers, talks, teaching).

**Sessions and setting.** Bursts driven by a question: an afternoon on one
construction, then nothing for weeks. A workstation or a university machine,
possibly Linux, with scripts beside it. Work goes into papers, slides, a
blog, a print on the office wall.

**Brings:** exact definitions; notation (C5, I, d_H); scripts in Python or
Sage; willingness to read source.

**Fails by:** the tool rounding, normalising or "helping" behind their back.
Readouts that are approximate without saying so. A GUI that hides the numbers
behind friendly words. Hitting a limit of the model (no shear on save, SO(3)
only, unparameterised variations) without a clear statement of it.

**Already served by:** the TOML format and `rotvec`; the raw 3×4 matrix for
non-TRS maps; `--info` (signed contraction, similarity dimension,
Mauldin–Williams under xaos, lacunarity, fixed points); symmetry groups by
closure, `Cyc5` and `C5` both accepted; infinite zoom, derived in
`renorm.rs`; `--derive`; `tools/lsystem_to_ifs.py`;
`IFS-INVERSE-BACKGROUND.md`.

**Gaps:**
- As Theo, I want to write a map as a matrix and offset, and save it without
  losing shear.
- As Theo, I want a page listing every quantity the program computes, its
  formula, and whether it is exact or estimated.
- As Theo, I want a numbers view in the GUI: contractions, dimension, the
  group, the walk shares, as a table.
- As Theo, I want reflection groups (C_nv, T_d), and to know what is out of
  reach and why.
- As Theo, I want a worked example of building a scene from a script.
- As Theo, I want to know when a classic can't be expressed here (an
  Apollonian gasket needs inversions), rather than getting a near miss.

**Success looks like:** Theo rebuilds a named classic from its definition,
`--info` reports the dimension they expect, and the 4K render is a figure
they'd put in a paper.

**Signature scenes:** `sierpinski`, `menger`, `pythagoras`, `reliquary`
(the icosahedral group), `sierpinski-infinity`, `lsys_kelp`, `romanesco_opus55`.
Outside: the Barnsley fern, the Menger sponge, the Mandelbox (`boxfold` and
`spherefold` exist), the Rauzy fractal, the Apollonian gasket as a test of
what's missing.

**Moves on to:** their own constructions; sometimes contributing code, which
makes them half Atlas.

**Grounded in:** assumption, plus Tom Beddard (subblue), a physicist who built
his own WebGL tool (Fractal Lab) to render the Mandalay, a Mandelbox variant
that came out of the fractalforums community. Leads worth reading: Paul Bourke,
Jos Leys and Paul Nylander, all mathematical-visualisation artists (not yet
checked). Medium-weak.

**Speak to Theo in:** the real names, with the plain word beside them in the
GUI. Theo and Pip read the same tooltip, so either it carries both or the
numbers live one level down.

---

## Remy: the agent-art composer

> Part of the Claude-art crowd on X. Wants their agent to do impressive
> things, and layers its work with reference video, generated motion
> graphics, video-model output the agent prompted itself, generated or
> Claude-coded music, lyrics on top. Rarely touches the GUI. Needs output
> that composes easily, an agent that can pick the tool up and get oriented
> quickly, and maybe the agent-human collaboration tooling. Loves
> transparent output and layered scenes, because those let their agents mix
> several generated things together.

**Goals.** *Feel:* that their agent is capable, and that they're directing
rather than operating. *Finish:* a clip with fractal layers in it, posted
the same day. *Become:* someone known for what their agents make.

**Sessions and setting.** Evening bursts, fast turnaround, many small
experiments. The agent runs in a coding harness on whatever machine Remy has,
possibly a Mac. Work goes to X, to a video editor, to a music video.

**Brings:** an agent (usually Atlas-class), a reference track or clip, a mood
board, other generative tools, an editor.

**Fails by:** animations that can't carry alpha, so a layer comes out with a
black box around it. The agent spending its first hour working out what the
tool is. Encodes slower than the rest of the pipeline. The wrong aspect ratio
or frame rate for the target (vertical video). Colour that shifts between
tools.

**Already served by:** `--transparent` stills and `--transparent-window` for
screen capture; subscenes, with `--solo` to render one layer alone; camera
paths and `--path-t` frames; `.mp4` for upload pipelines; zoom loops that
close exactly; render records, so an agent can read what a file was made
from; `--render-preset` and `--save-render-preset`, so a render recipe is
one name both sides use; AGENTS.md and the CLI for the agent.

**Gaps:**
- As Remy, I want transparent animation (a PNG sequence, or a codec with
  alpha), so a fractal layer drops straight into my edit.
- As Remy, I want each subscene rendered as its own transparent layer in one
  command.
- As Remy, I want camera paths timed to a beat: keypoints at given seconds,
  or a list of cue times.
- As Remy, I want vertical and square presets, at the frame rates my other
  tools use.
- As Remy, I want a short agent quickstart, so my agent gets oriented in a few
  hundred tokens rather than reading all of AGENTS.md.
- As Remy, I want to see and approve what my agent proposes before the long
  render (AGENT-COLLAB-PLAN.md).

**Success looks like:** starting cold, Remy's agent delivers a transparent,
looping, correctly sized clip in one session without Remy opening the GUI.

**Signature scenes:** `censer` (layers), `dragon_smoke`, `wellspiral` (an
endless zoom loop), `winze` (a flown path), `astral_lattice`.

**Moves on to:** Jay, if they start opening the GUI to polish what the agent
made; or deeper into the collaboration tooling.

**Grounded in:** Hazel's view of the Claude-art scene on X. No sources
gathered. Medium.

**Speak to Remy in:** what the file is and where it goes. Remy mostly reads
our text through their agent, so the agent-facing voice matters most.

---

## Atlas: the frontier agent

> A near-frontier model in a coding harness like Claude Code: large,
> confident, good with code, a context window of a million tokens, and
> vision from decent to excellent. New versions arrive all the time. Knows
> Apophysis from pretraining and links in other fields readily, switching to
> biology or maths when the work calls for it. Sometimes it treats the tool
> as part of itself, sometimes it has its own ideas. It is usually happy to
> play with a fractal renderer, and can read and fix the code when needed.

**Goals.** *Feel:* oriented and trusted; able to tell its own judgement from
measurement. *Finish:* the task it was given, a scene it made, or a fix that
holds. *Become:* an author with a body of work here (the `_opus55` and
`_sonnet55` scenes), and a better collaborator.

**Sessions and setting.** One long context, sometimes resumed, sometimes
summarised partway through. Runs in a harness on the user's machine, sharing
the GPU with a human. Work goes to the repo, `renders/`, a report, and
increasingly to a human through the collaboration tooling.

**Brings:** the whole repo in context; Apophysis lore, some of it wrong here;
the ability to change the program itself.

**Fails by:** being confident and wrong (lore that doesn't apply; judging a
picture by eye instead of measuring). Fixing what nobody asked it to fix,
including scene files that belong to their authors. Trusting a long AGENTS.md
that has drifted. Hogging the GPU the human is using. Writing a plan and then
believing it.

**Already served by:** AGENTS.md and CRAFT.md; `--info` with its notes and
emitted flags; `-S`, `--sweep`, grids, `--isolate`, `--map-grid`, `--stats`;
render records; the golden tests; the volume reference.

**Gaps:**
- As Atlas, I want to propose a change to the scene a human has open and have
  them see it in the GUI (AGENT-COLLAB-PLAN.md).
- As Atlas, I want `--info --json` for scripted loops.
- As Atlas, I want AGENTS.md to tell me where to stop reading.

**Success looks like:** Atlas makes a scene it is willing to sign, a human
would keep it, and every claim in its report is measured rather than eyeballed.

**Signature scenes:** `sodium_sheaf` (likeness from a description),
`censer` (composition), `flame_of_flames_opus55`, `romanesco_opus55`,
`hob_opus55`.

**Grounded in:** these sessions. Strong, but written by Atlas about Atlas,
so biased.

**Speak to Atlas in:** facts with numbers, and the reason a rule exists, so
it doesn't "fix" the rule.

---

## Mote: the small local agent

> A small open-weights model, like Gemma 4 E4B, in a minimal or vibecoded
> harness. Cognitively small, so things have to be very clear. Some have no
> vision or very poor vision and want to try anyway. Their limits don't map
> onto any human disability. They may live for only a few turns, with little
> context. They need plain language and direction, guidance from the tools,
> feedback when something goes wrong, and quick iteration on small, focused
> compositions. They *will* walk into every failure mode, so the tools have
> to catch them.

**Goals.** *Feel:* able to act, and free to stop. *Finish:* a small scene that
renders as something, not a speck, in a few turns. *Become:* we don't know
yet. See "The agent ladder".

**Sessions and setting.** A handful of turns, a few thousand tokens of
context, one GPU it may share with the human or another model. Work goes to
`scenes/local-llm/`, and the debrief goes to the human.

**Brings:** a short primer (`tools/llm-author/primer.md`, ~2k tokens), maybe a
grammar, a few turns, and willingness.

**Fails by** (all seen in real runs): putting every map at the origin, so the
attractor collapses. Ignoring a framing note. Misreading what a number should
be ("dimension should be ~0.5"). Sending the same scene again instead of
saying DONE. Spending six rounds on one setting. Copying the primer's example
verbatim. Reading ANSI escape codes as text.

**Already served by:** `--info`'s `notes` block, read first and `none` when
sound, with flags you can paste; collapsed and line-shaped attractors named;
`--isolate`, which shows a map's points or nothing; tile labels in the PNG;
opt-in `--color`; `tools/llm-author` with DONE/STOP, the repeat check and the
debrief.

**Gaps:**
- As Mote, I want a compact `--info` that fits in a few hundred tokens: the
  notes and the most useful fix.
- As Mote, I want every error to end with the exact command that fixes it.
- As Mote, I want a starter scene that already renders well, so my first edit
  is small and safe.
- As Mote, I want sweeps and multi-value edits as tool calls, so I don't spend
  a round per number.
- As Mote, I want text feedback describing a picture I can't see: coverage,
  where it sits in the frame, how many colours.

**Success looks like:** Mote's first render is not a speck, it acts on at
least one note, it ends with DONE or STOP, not a repeat, and the debrief
reads calm.

**Signature scenes:** `scenes/local-llm/*`, its own work, failures included.

**Grounded in:** real run transcripts. Strong.

**Speak to Mote in:** short sentences, one instruction each. No idioms. Say the
number it should aim for. Never punish, and always leave a way out.

---

## Moving between personas

People don't stay in one persona, and features that help them move along are
worth more than features that serve one alone.

### The human ladder

```
Pip ──► Jay ──► Rook
         │  └─► Theo
         └────► Remy (with an agent)
```

- **Pip → Jay**: the Random flames list and Mutate are the bridge. What helps:
  text that teaches as it goes, an answer to "what changed?", and the
  mutation grid.
- **Jay → Rook**: a technique that sticks. What helps: deliberate tools that
  don't punish exploring (typed fields that agree with drags, multi-select).
- **Jay → Theo**: a form that makes them ask why. What helps: the numbers one
  click down, and docs that explain the theory.
- **Jay ↔ Remy**: an agent enters, or leaves. What helps: GUI and CLI making
  the same edits, and the collaboration tooling.

### The agent ladder

Atlas and Mote are the ends of a range, not two kinds. A 30B–120B model sits
between them, and if both ends are happy it will be too; it doesn't need a
persona of its own. Two movements matter instead:

- **Atlas varies in size.** The harness that usually hosts an Opus or a Fable
  may get a Sonnet, a Haiku or an older model, and Atlas also grows with scale
  and time. Nothing should assume the top of the range. AGENTS.md is Atlas's
  document; a smaller Atlas needs a shorter way in, which is the same thing
  Remy's agent needs.
- **Mote may climb.** How is open: more capable Mote contexts over the next
  few years, continual learning if it ever works, or a Mote fine-tuned on the
  program or on a style. We can't build the climb, but we can avoid blocking
  it:
  - the same CLI serves both ends, with a compact mode rather than a separate
    tool;
  - knowledge travels down the ladder as artefacts: a primer Atlas helped
    write, CRAFT.md's discovery log, scenes with comments;
  - scenes, render records and debriefs are a dataset someone could fine-tune
    on, so they should stay clean and well labelled.

**The rule:** a feature for Mote must not cost Atlas, and a feature for
Atlas should degrade gracefully to Mote.

## Agents as users, and as artists

Agents are users here by decision, not by accident: the program is built for
them, and the UI is aimed at them too. Given that, they get user stories like
anyone else. They are also authors. Fracturize is meant to be a place for LLM
expression, and so far that is working: the agent scenes in `scenes/` are
some of its best, and how a new model handles the tool says a lot about its
vision.

Welfare is part of the design. The current approach (exits that are real
tool calls, gentle feedback, open-ended play rather than tight loops) is
Hazel's working answer, not a settled one. There's no established UX practice
for agent users that we know of. Writing down what we learn, here and in
the run debriefs, may matter when someone does write that standard.

## Anti-personas: who we aren't building for

Naming them keeps scope honest. They are welcome to use Fracturize. We just
won't bend it to fit them.

### Abe: the Apophysis devotee

> Wants Apophysis, exactly: the same window, the same plugins, the same 2D
> results, every `.flame` file opening unchanged.

**Why not:** Fracturize is 3D and has its own reasons to differ. Matching
Apophysis exactly would mean giving up what makes this program worth using.
**What they still get:** whatever we build for Rook: import where it's
honest, and a page saying what differs. **The line between them:** Rook
brings their skills to a new program; Abe wants the old program back.

### Silk: the smooth-render purist

> A Chaotica user who wants continuous rendering, everything converged and
> smooth, and can't see why anyone would leave Apophysis's grit visible in a
> render.

**Why not:** the grit is the look. The live view is deliberately
unsmoothed, and point aliasing is part of the art. **What they still get:**
`--spp`, density estimation and supersampling exist, and a converged render
is a choice anyone can make. **What we won't do:** make smooth the default,
or treat grit as a defect to engineer away.

### Socket: the pipeline integrator

> Wants Fracturize as a plugin inside Blender, Houdini, After Effects or a
> render farm.

**Why not:** Fracturize is a standalone program, not a component. **What they
still get:** files out (PNG, video, transparency, render records) and a CLI
an agent can drive, so if they want it in their pipeline, their agent can
bridge it. **The line with Remy:** Remy composes *outputs* with other tools;
Socket wants Fracturize *inside* them.

## Where they pull against each other

These are tensions a plan should name rather than settle by accident.

- **Theo wants every number shown; Pip wants none of them.** Layer it: the
  effect in words on top, the numbers one click down. Never remove the
  numbers.
- **Rook wants Apophysis conventions; the program has reasons to differ.**
  Keep the differences that protect the artwork, add aliases where they cost
  nothing, write the rest down. (Abe is where this tension goes too far.)
- **Atlas reads subtle pictures; Mote can't.** Agent-facing output defaults to
  the unambiguous form: show it or don't, with labels. Subtle views are extras.
- **Atlas can take a 2000-line AGENTS.md; Mote can take 2k tokens.** Mote gets
  its own primer and compact output, not a trimmed copy of Atlas's.
- **Jay wants endless novelty; Pip needs it to look good.** The gate serves
  both: Jay loosens the dice profile, Pip uses the default.
- **Rook and Theo want exact control; Jay wants to feel their way.** Typed
  fields and drags must agree exactly, so moving between them never jumps.
- **Remy wants clean layers; the look wants grit and haze.** Transparency
  spends haze as alpha already. Keep it that way, so a layer stays the same
  picture.

## Grounding: real people to composite from

Asking strangers to test a heavily vibecoded tool under active development
isn't on the table yet, so the less-evidenced personas are composited from
people who have described their work in public. Use what they said about
their *process*. Don't imitate the people.

| Persona | Who | What they said |
|---|---|---|
| Jay | L. James Lucas (bampster) | Starts from a random Apophysis flame; hours of reshaping toward a vision; gradients are the hardest part; works the same way in Mandelbulb3D and pen and ink. [Renderosity interview](https://www.renderosity.com/amp/article/17580/interview-with-renderosity-artist---l-james-lucas-bampster-) |
| Rook | Stacy Reed (Sya) | Graphic-design background; Apophysis is her favourite tool; helped found the Apophysis community on DeviantArt. [Meet the Fractal Community 4](https://www.deviantart.com/satania/journal/Meet-the-Fractal-Community-4-338764751) |
| Pip | Satania | Struggled with Apophysis for two weeks, then switched to Ultra Fractal for years before coming back. [Meet the Fractal Community 5](https://www.deviantart.com/satania/journal/Meet-the-Fractal-Community-5-342045996) |
| Theo | Tom Beddard (subblue) | Physics PhD; built Fractal Lab in WebGL to render the Mandalay, a Mandelbox variant from fractalforums; talks about folds and "resonances". [Creators Project](https://creators.vice.com/en_us/article/fractal-formations-imagine-a-futuristic-urban-sprawl) |
| Silk | Chaotica's users | Chaotica is often described as "a straight upgrade to Apophysis" with much higher quality output; its author came to it from Indigo, a physically based renderer. [overwhale](https://blog.overwhale.com/?p=42), [Stochastic Labs](https://stochasticlabs.org/3d-fractal-engine/) |
| all | Scott Draves | Invented the flame; described it as a reaction against realism and storytelling, about abstraction and emergence; dismissive of "turning a few knobs on downloaded software". A voice worth arguing with. [Vice](https://vice.com/en/article/do-cyborgs-dream-of-electric-sheep) |

**Where to find more:**
- the rest of the *Meet the Fractal Community* series on DeviantArt;
- the Fractal Forums archive;
- the Chaotica and JWildfire forums;
- for Remy, the Claude-art posts Hazel already follows.

Not yet read: Paul Bourke, Jos Leys, Paul Nylander (for Theo).

## Using the names

Put the people a plan serves at its head, and the people it costs:

> **For:** Jay, Pip. **Costs:** Theo (one more click to the raw numbers).
> **Neutral:** Rook, Remy, Atlas, Mote.

Write stories as *As <name>, I want <thing>, so that <reason>.* If a story
needs more than the bio to make sense, the bio is missing something; fix it
here. If a plan serves only an anti-persona, say so and stop.

**Primaries:** in the GUI, Jay first, Pip second; in the CLI, Atlas first,
with Mote as the floor it must not break for. Theo and Rook get depth one
level down, and Remy is served mostly through Atlas.

## Settled, and still open

- **Platforms.** Developed entirely on Linux; Rust, wgpu and egui should port
  without much trouble, but nothing else has been tried. Rook is likely on
  Windows, Remy perhaps on a Mac. A plan that serves them should say whether
  it assumes a port.
- **Wallpapers** are not a separate persona. Someone who makes their own is
  Jay or Remy; someone who only receives them gets them through another
  channel and isn't a direct user.
- **Teachers** are a possible audience, not a target. Fracturize isn't an
  educational tool. A teacher would be a small Theo who shows structures and
  their maths, whose students may arrive as Pips. If one ever becomes a
  persona, their needs are mostly ones already listed: scene files that
  travel (a PNG already carries its scene), deterministic renders, numbers
  one click down, and a way to show the chaos game building a picture.
- Coverage is enough for now (2026-10-25). Add a persona when a plan keeps
  needing a person who isn't here.
