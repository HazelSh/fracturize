# Agent collaboration — plan

*Status: plan, 2026-10-24, revised 2026-10-25 (§4 rewritten, §7–§8 redone,
§10 updated). Phase 1: the diff and the three-way merge are built
(`src/scene_diff.rs`, `--diff`); the GUI-callable recorded render is not. Written by Claude Opus 5.5 from Hazel's brief and
two rounds of answers (§10).*

*One collaborative flow for both of the project's users. Either one, human or
agent, can **request** a change or **propose** one. Proposals come with
rendered options. The other side can **preview** them, **tweak** one,
**apply** it, **reject** it, or **send it back** for another round. It is all
one thread per scene (§4), on files first (§7), with a live bridge to a
running Claude Code session and a local-model backend on top (§5, §6).*

*GUI wording in here is placeholder; it is Hazel's to set. Item 455 in
`todo.txt` ("needs an AI button! 'LLM edit'") is §4.*

---

## 1. What already exists, and what not to rebuild

- **The CLI is the agent interface already.** `-S/--set`, `--info`, `--stats`,
  `--render`, grids, sweeps and render records cover "read a scene, change it,
  see it". Claude Code drives all of it through Bash today. Nothing here
  wraps the CLI in a protocol: an agent that wants to render five variants
  still runs `fracturize` five times. **The new surface is only what needs the
  live GUI or a human.**
- **`tools/llm-author/`** already runs local models against llama-server's
  OpenAI-compatible API (via `~/code/lll/with-llm`). It has the 2k-token
  `primer.md`, the `scene.gbnf` grammar, the whole-scene round loop, the
  DONE/STOP exits, the repeat check, the debrief and the run directories.
  The local backend (§6) reuses those files rather than copying them.
- **`App::commit_edit`** is the single door every edit goes through, so "an
  accepted proposal is one undo step" comes for free.
- **`/renders` is gitignored.** Threads live under it (§4.2), so they never
  reach history, nor the public repo.
- **Claude Code ≥ 2.1 has channels.** An MCP server declared as a channel can
  push events into a running *interactive* session, and that session answers
  through the server's tools. This is the supported form of "open a Claude
  Code context, ask it to connect, brief it, then talk to it from the GUI".
  It runs on Hazel's claude.ai login; no API key is involved. Channels are a
  research preview: a custom server needs
  `--dangerously-load-development-channels server:fracturize`, and the flag
  may change. §5.3 has the fallback if it does.
- **Not present:** any IPC in the GUI, an HTTP client, `serde_json`, an async
  runtime. The plan adds `serde_json` and a small blocking HTTP client
  (`ureq`), and no tokio.

## 2. Shape

```
 Claude Code session ──stdio MCP──▶ fracturize mcp ──unix socket, JSON lines──▶ running GUI
   (interactive, auto mode)          (bridge; same binary)     $XDG_RUNTIME_DIR/fracturize/gui.sock
        ▲                                   │
        └──── channel events ◀──────────────┘   "Hazel asked …", "Hazel answered"

 Local LLM (llama-server :8080) ◀──HTTP── the GUI's own agent loop (§6; no MCP)

 Any agent, GUI or not ──files──▶ renders/collab/<scene>/   +   fracturize collab …   (§7)
```

**MCP rather than A2A.** A2A is for peer agents that discover each other over
a network. Here there is one human, one GUI and one agent at a time, on the
same machine. Claude Code speaks MCP natively and channels give it push;
nothing here speaks A2A. If a second agent ever needs to join, the socket
protocol in §5.1 is the place it attaches, not a new protocol.

**The GUI owns the socket; `fracturize mcp` is a stdio bridge.** Claude Code
launches MCP servers itself, over stdio, while the GUI is a long-lived process
started by Hazel. So the GUI listens, and `fracturize mcp` (a subcommand of
the same binary, so it shares scene code) connects to it, forwards tool calls
and turns GUI events into channel notifications. The GUI side is a listener
thread plus a `winit::EventLoopProxy` user event to wake the loop. It never
blocks a frame.

**The MCP JSON-RPC is hand-rolled.** The subset needed is `initialize`,
`tools/list`, `tools/call`, `notifications/claude/channel` and the
`experimental.claude/channel` capability: a few hundred lines with
`serde_json`. `rmcp` would bring tokio into a binary that doesn't otherwise
want it.

## 3. The shared core: one diff format

Proposal cards, tweaks, Send back and Apply all need to say *what changed
between two scenes*. One function, written first:

```
fracturize --diff a.toml b.toml          # also: scene_diff(&a, &b) in src/scene_diff.rs
```

prints:

```
# set-ops: each line is a valid -S argument
transform.facet-1.weight=0.62
transform.facet-1.variations.absfold=0.15
meta.color_falloff=0.3
# structural: not expressible as -S
+ transform "seam-3"        (added; full table in the scene)
- transform "spike"         (removed)
~ order: facet-1, seam-3, facet-2
```

- **Paths are the ones `--set` accepts.** Transforms are addressed by name
  where they have one, by index otherwise. The test that has to hold: for
  any two scenes with no structural changes, applying the set-ops to A with
  `-S` parses to the same scene as B. Run it over every scene in `scenes/`
  against a mutated copy (`mutate.rs` provides the copies).
- **Structural changes are listed, not encoded.** `--set` sets values; it
  can't add, remove or reorder a transform, and growing it to do so isn't
  needed for this plan. So the full scene TOML is always the source of truth
  in a proposal or a response. The diff is the readable summary that comes
  with it.
- **The comparison is on the parsed scene, not the text**, so reformatting
  and comments aren't changes. The value printed is the canonical one (the
  one `save_scene` would write).

**Three ways, for Apply.** `scene_diff::merge(base, mine, theirs)` takes the
changes from `base` to `theirs` and applies them to `mine`. A value changed on
only one side takes that side. A value both sides changed to the same thing is
no conflict. A value they changed differently is a **conflict**, returned as
`{path, base, mine, theirs}` for the GUI to show (§4.4) and resolved per row.
Structural changes go by transform name: an added transform is added unless
`mine` already has one of that name, a removed one is removed unless `mine`
changed it. Either refusal is a conflict too. The CLI form is
`fracturize --diff mine.toml theirs.toml --base base.toml`, which prints the
merge and its conflicts. The test that has to hold: `merge(a, a, b) == b`,
`merge(a, b, a) == b`, and edits to disjoint values from both sides both land.

The third shared piece: **render a scene with recorded settings, from inside
the GUI**, using the same view, size, effort and grade as a given render record
(the render-records section of AGENTS.md). Tweaks need it so that a human's
edit renders the same way as the agent's options. The GUI already renders
offline for render jobs, so this exposes that path; it isn't a new renderer.

## 4. One flow: threads of proposals

*Revised 2026-10-25. The first draft had two systems that met in the middle: an
Ask dialog where Hazel requested and the agent proposed, and review packets
where the agent asked and Hazel picked. Each had its own files, window and
verbs, and neither let anyone tweak a suggestion and hand it back. This
section replaces both.*

### 4.1 The model

Everything is a **message in a thread**, and a thread belongs to a scene.
Either side can send any kind of message. There are three kinds:

| kind | carries | e.g. |
|---|---|---|
| **request** | text, optionally pinned to transforms or a region of the view | "make the seams thinner", or from the agent: "frame it the way you'd hang it and send it back" |
| **proposal** | text, 1–9 **options** (a scene, a render, a note each), an `ask` (`pick-one`, `pick-any`, `free`), and the **base** the options were made from | three seam patterns; one reframing; Hazel's own edit, "what would you do with this?" |
| **response** | a verdict, and per option: applied, rejected, sent back, or a comment, plus the scene and diff of any tweak | "B, but thinner. My tweak's attached, can you push it further?" |

A response that sends an option back is itself a request for a revision. The
next proposal names what it revises (`revises = "<message>/<option>"`), so the
thread holds the lineage: B, Hazel's tweak of B, the agent's B2. A card can
show the diff against the option it revises as well as against the scene.

**Requests and proposals are symmetric; the live scene is not.** Either side
can propose, and either side can answer. But only the human's GUI has a live
scene, and **only a human action ever changes it**. An agent "applying" a
proposal means agreeing, or basing its next proposal on it. The rule from the
first draft stands: agents never edit the live scene.

### 4.2 On disk

```
renders/collab/<scene-stem>/
  thread.toml                  # scene path, participants, the message order
  0001-request/   message.toml
  0002-proposal/  message.toml  base.toml  A.toml A.png  B.toml B.png
  0003-response/  message.toml  B.tweak.toml B.tweak.png
  0004-proposal/  message.toml  base.toml  B2.toml B2.png
```

Under the gitignored `renders/`, as decided (§10.2). One directory per message
so a message is written atomically: write to `.tmp-…`, rename. A participant
that only has files, such as a headless script, `llm-author`, or a Claude Code
session with no bridge, can take part with nothing but the CLI (§7).

`message.toml` for a proposal:

```toml
kind    = "proposal"
from    = "claude-opus-5-5"         # or the author name from prefs
to      = "hazel"
replying_to = "0001-request"        # optional; a proposal can start a thread
revises = "0003-response/B"         # optional: the lineage
ask     = "pick-one"
message = """
Thinner seams, keeping your rotation. B2 also pulls the gold toward the rim.
"""
base_sha256 = "…"                   # of base.toml, the scene the options start from

[render]
record = "A.png"                    # view, size, effort, grade for re-renders

[[option]]
id     = "B2"
scene  = "B2.toml"
render = "B2.png"
note   = "seam scale 0.04 → 0.025"
```

`base.toml` is what makes Apply honest (§4.4). It is the scene as the proposer
saw it: the snapshot sent with the request, or the scene file the agent read.

### 4.3 The Collaborate window

One dockable window replaces both the Ask window and the Reviews list. It shows
the current scene's thread, and has a thread picker for the others, with a
badge counting messages waiting for an answer.

- **The thread**, oldest first: requests as speech, proposals as a row of
  option cards (render, id, note, a one-line diff count such as "4 values, +1
  transform"), responses as a compact line under the proposal they answer.
- **The composer** at the bottom. One text box and the send controls:
  - **Send**: a request. It attaches the scene (saved or not) as `base.toml`, a
    render of the current view, and the selection. Toggles under the box turn
    the render and the selection off.
  - **Propose my edit**: a proposal with one option, the live scene, against
    a base Hazel picks: the last save, the last applied proposal, or the scene
    when the thread began. This is how Hazel asks "what would you do with
    this?" instead of describing it.
  - **Send current scene**: answers an agent's request ("frame it the way you'd
    hang it") with a proposal of the live scene.
- **The backend picker and connection light** from the first draft (§4.6) sit
  in the header. With no backend connected the window still works: messages
  are written to disk and wait there for whichever agent reads them.
- **Every message shows its state**: *written*, *delivered*, *seen*,
  *working*, *answered*, *withdrawn*. Silence should never be ambiguous.

### 4.4 The verbs on an option

The same five whoever sent it. For a proposal addressed to Hazel:

1. **Preview.** Click a card. The viewport shows the option, and the toolbar
   shows *Previewing B2* where the dirty marker goes. **Hold a key** (to be
   chosen) to flip back to the live scene while it's held, so the difference
   reads as a blink, the way people compare two renders anyway. Previewing
   touches neither undo nor the dirty bit, and Esc leaves.
2. **Tweak.** Any edit made while previewing turns the preview into a
   **sandbox**: a copy of the option, with its own undo stack. The card is
   marked *tweaked* and shows the diff against the agent's version. Hazel's
   live scene is untouched underneath. **Re-render** renders the tweak with
   the proposal's `[render]` settings, so it sits beside the others like for
   like.
3. **Apply** (the option, or the tweak of it). One
   `commit_edit("Agent: <first line>")`, so one Ctrl+Z, and a row in History.
   It is a **three-way apply**: the changes from `base.toml` to the option
   are applied on top of the live scene, not the option written over it. So
   edits Hazel made after sending the request survive. Where both sides changed
   the same value, a small dialog lists those rows, *yours* beside *theirs*, and
   she picks per row. Nothing is overwritten without being shown.
4. **Send back.** Text, and the tweak if there is one. It writes a response
   with this option marked *sent back*, which the agent reads as a request for
   a revision. This is "tweak them further first", and the step that makes it a
   conversation rather than a vending machine.
5. **Reject**, with an optional one-line reason.

`pick-one` and `pick-any` decide how many options can be applied or picked;
`free` asks only for comments. **Keep** copies an option to `scenes/` through
the Save-as flow. It's the one way anything under `renders/collab/` reaches
the project.

For a proposal Hazel sent, the agent does the same things through tools or
the CLI: respond with a verdict, comment per option, and propose a revision.
It can't apply anything to the live scene, so it has no Apply.

### 4.5 Notifications

A new message addressed to Hazel badges the window's toolbar button and fires
`notify-send` if the window isn't focused. It never opens a window over her
work. A new message for an agent is pushed over the channel (§5) where there
is one. Otherwise it waits for `fracturize collab wait`.

### 4.6 Settings

These live in `prefs.toml` under `[agent]`, and in an **Agent settings**
dialog:

```toml
[agent]
backend = "claude-channel"        # claude-channel | claude-headless | local | none

[agent.claude]
session_name  = "fracturize"      # for headless --resume, and the connect hint
headless_args = []                # extra args for `claude -p`

[agent.local]
base_url  = "http://127.0.0.1:8080/v1"
model     = ""                    # empty: whatever /v1/models reports
preset    = "gemma4-e4b"          # with-llm preset to start if nothing is on :8080; "" = never start
vision    = "auto"                # auto (from /props) | on | off
thinking  = true
grammar   = false
mode      = "whole-scene"         # whole-scene | tools
max_turns = 6                     # §6.2
```

`none` is a real setting: the thread is file-only, for an agent that reads it
with the CLI. The dialog has a **Test** button for each backend: connect, ask
for one word, and show what came back, or the error.

## 5. Claude Code backends

### 5.1 The socket protocol (GUI ⇄ bridge)

JSON lines over `$XDG_RUNTIME_DIR/fracturize/gui.sock`, with
`{"id", "method", "params"}` in each direction. The GUI accepts **one bridge at
a time**. A second one gets a refusal that says which session already holds
it, and the Collaborate window shows that session's name.

### 5.2 MCP tools (`fracturize mcp`)

| tool | what it does |
|---|---|
| `get_state` | scene TOML, path, dirty flag, selection, view, render-panel settings |
| `get_render` | a PNG of the current view (existing screenshot path), returned as a file path *and* as image content; optional size |
| `thread` | `{thread, since?}`: the thread's messages, diffs included, as `collab show` prints them |
| `request` | `{thread, text, transforms?}`: ask Hazel for something |
| `propose` | `{thread, replying_to?, revises?, message, ask, options: [{id, scene_toml, note}], base_toml?}`: the GUI renders any option without a render, computes the diffs and shows the cards (§4.4) |
| `respond` | `{thread, message, verdict, picks?, comments?, send_back?: [{option, note, scene_toml?}]}`: answer a proposal of Hazel's |
| `reply` | `{thread, text}`: a message with no options, such as progress or a question |
| `withdraw` | `{thread, message, reason}` |

Channel events the bridge pushes into the session:

- `<channel source="fracturize" kind="request" thread=… message=…>`: Hazel's
  text, the scene path, the paths of the `base.toml` and PNG snapshots, and the
  selection;
- `kind="proposal"`: Hazel proposed her edit, with the message directory;
- `kind="response"`: Hazel answered a proposal of the agent's, with what was
  applied, rejected and sent back, and the paths of any tweaks.

Tool descriptions say to use the CLI for anything else.

### 5.3 Connecting

**Channel (primary).** Once:

```sh
claude mcp add --scope project fracturize -- fracturize mcp
```

Then, per working session:

```sh
claude -n fracturize --dangerously-load-development-channels server:fracturize
```

Brief it however suits: "you're connected to the fracturize GUI; read
AGENTS.md and CRAFT.md; requests arrive as channel events". The bridge
registers with the GUI when it starts, and the light goes green. Hazel uses
auto mode, so permission prompts don't stall a session she isn't watching.
Even so, the Collaborate window's amber light times out to a line saying "no reply in
N min; check the terminal".

**Headless (fallback, no terminal needed).** Per request, the GUI spawns:

```sh
claude -p --resume <saved session id> --mcp-config <generated> \
       --output-format stream-json "<the request, as the channel event would have said it>"
```

It streams the session's text into the thread as it arrives. The same tools
are reached through the same bridge. It runs on the same login. The first
request starts a session and saves its id under `[agent.claude]`; later ones
resume it, so the briefing carries over. The cost is a cold start for each
request. This is also the escape route if channels change shape before they
leave preview.

## 6. Local LLM backend

### 6.1 Two modes

- **whole-scene (default).** This is `author.py`'s protocol, moved into the
  GUI: the system prompt plus `primer.md`, then Hazel's request, the scene,
  the `--info` report and, with vision, the render. The model replies with
  prose and one fenced scene (with the grammar optionally enforced), or DONE
  or STOP. Each scene it returns becomes a proposal card. Small models have
  done this reliably in llm-author runs; tool loops they mostly have not.
- **tools.** The §5.2 tools, offered as OpenAI function calls, plus `render`
  and `info` (which in this mode the GUI runs for the model, because a local
  model has no shell). It's for larger or newer models, chosen in settings.

`primer.md`, `scene.gbnf` and `prompts/` stay in `tools/llm-author/`, and the
GUI reads them from there, so the script and the GUI can't drift apart. A
missing file is a clear error in the Collaborate window, not a silent fallback.

### 6.2 Turn limit, then report back

The model is never left looping on its own. Each request gets at most
`max_turns` model replies (default 6). A turn is one reply, whether it holds a
scene, a tool call or prose.

- **On reaching the limit**, the GUI makes one last call, using the existing
  `interview.md` debrief shape, cut down to *what you tried, where it stands,
  what you'd do next*. It posts the answer in the thread and ends the request.
  Proposals already made stay on the board. Continuing is Hazel's choice, by
  replying in the thread, and her reply starts a fresh budget.
- **Proposals don't wait.** Each scene appears as a card when it arrives, so
  Hazel can accept turn 2's while turns 3–6 are still running.
- **A Stop button** in the Collaborate window ends the request at once, between turns.

### 6.3 Welfare: the llm-author rules, unchanged

Carried over as they are, until Hazel has a better idea of what local-model
welfare should look like:

- DONE or STOP on its own line ends the request immediately. STOP needs no
  reason and is never framed as failure. A model that chose STOP is not
  debriefed.
- Feedback is the program's report, not a grade. A load error is "common and
  easy to fix", with STOP offered again. After `--repairs` (2) failed fixes
  in one turn, the request ends.
- A reply that uses its whole token budget without producing a scene ends the
  request.
- A repeated scene is asked about once (`prompts/same.md`); a second repeat is
  taken as DONE.
- One request at a time, per GUI.
- The full transcript, reasoning included, is written to
  `renders/collab/<scene-stem>/transcripts/<time>-<model>/` in llm-author's format, so
  `review.sh` works on it unchanged.

### 6.4 VRAM

The GUI renders while the model runs, on the same 8 GB GTX 1080. The settings
dialog shows the preset's weight size against free VRAM, using `with-llm`'s
1.2 GB headroom rule. Starting a preset that doesn't fit is refused, with that
reason. The headroom number is shared with `with-llm`, not copied.

## 7. The CLI side of a thread

For agents without the bridge (headless scripts, `llm-author`, a Claude Code
session told to poll), and for the bridge to call underneath:

```sh
fracturize collab list [--scene s.toml] [--waiting]   # threads, and what's waiting on whom
fracturize collab show <thread> [--since <message>]   # the thread as text, diffs included
fracturize collab request <thread> "text"             # a request
fracturize collab propose <thread> <dir>              # validate a proposal dir and post it
fracturize collab respond <thread> <message> --verdict "…" \
    [--pick B] [--reject C --reason "…"] [--send-back B --note "…" --scene tweak.toml]
fracturize collab wait <thread> [--timeout 30m]       # block until a message for me arrives;
                                                      # print it; exit 0, 3 timed out, 4 withdrawn
fracturize collab withdraw <thread> <message> --reason "…"
```

`propose` validates: every option parses, every render exists, `ask` is one of
the three, there are at most 9 options, and `base.toml` is present. It
computes the diffs itself, so an agent never has to write one by hand.

`collab` and `mcp` would be the CLI's first subcommands. Today everything is a
flag on one command and the scene comes in through `-s`, so nothing positional
collides. An optional clap subcommand alongside the existing flags keeps every
current invocation working. **Hazel's call**; the plan assumes subcommands.

**Withdraw:** a sender can withdraw a message that hasn't been answered. It
stays in the thread, greyed out with the reason, so nobody answers into
nothing.

## 8. Phases

Each phase ends with a check per surface it touches, in the
`ADDING-A-SCENE-FEATURE.md` spirit, and an AGENTS.md section. CRAFT.md gains a
short "proposing to a human" note at phase 2.

1. **Diff, three-way apply, recorded render.** `src/scene_diff.rs`, `--diff`
   (with `--base` for three ways), the round-trip test over `scenes/` ×
   mutations, the merge and its conflicts as a value the GUI can show, and the
   GUI-callable recorded render.
2. **Threads on files.** The message formats, `fracturize collab …`, and the
   Collaborate window: thread, composer, cards, Preview with the hold-to-blink,
   Tweak's sandbox, Apply with the conflict dialog, Send back, Reject, Keep. No
   IPC yet, so a Claude Code session joins by running `collab wait` in a loop.
   This ships first because it is the whole workflow, end to end, for any agent
   with a shell. Everything after makes it faster, not different.
3. **Socket and bridge.** The GUI listener and `EventLoopProxy`,
   `fracturize mcp`, the tools, channel push, the one-bridge rule. Checked with
   a real channel session and with `claude -p`.
4. **Backends in the composer.** The connection light, Agent settings, the
   channel backend, then the headless one.
5. **Local backend.** Whole-scene mode first, then tools mode, the turn limit
   and report-back, the welfare rules, transcripts, the VRAM gate.

## 9. Later, on purpose

- **Autopilot / shared editing.** An agent editing the live scene in the same
  workspace, with Hazel watching or editing alongside. Not now: proposals and
  Accept only. The hard part is timescale: an agent turn takes tens of
  seconds to minutes, a drag takes milliseconds, so a naive "both edit" means
  the agent is always editing a scene that no longer exists. Possible shapes
  for when it's tried:
  - the agent takes a lease on named transforms;
  - its edits land as their own coalesced history entries that Hazel can
    revert in one click;
  - the three-way apply (§3, §4.4) becomes a live rebase.

  Worth a test bed once phase 4 exists, since the bridge and the diff are
  already the pieces it needs.
- **Agent-to-agent threads.** A second agent answering a proposal. The format
  allows it (`from` and `to` are free text, and a response is just a file).
  The GUI doesn't need to know.
- **An API-key backend.** It would be straightforward to add beside the
  others if ever wanted. Nothing needs it.

## 10. Decisions taken

Hazel's answers, 2026-10-23/24:

1. **Proposals and acceptance only**, for now. Autopilot and shared editing are
   §9.
2. **Threads are not committed.** They live under the gitignored `/renders`.
   The project is public, and threads aren't history material. Keep (§4.4) is
   the only way a proposed scene enters `scenes/`.
3. **Permissions:** Claude Code runs in auto mode, so there's no
   permission-prompt work. The amber-light timeout (§5.3) stays as the
   general "agent went quiet" signal.
4. **The local model gets a turn limit, then reports back** (§6.2), and the
   llm-author welfare approach carries over unchanged (§6.3).
6. **One flow, not two** (2026-10-25): requests, proposals and responses
   from either side, with preview, tweak, apply, reject and send back. The
   Ask dialog and review packets of the first draft merge into §4.
7. **MCP over A2A** and **the GUI owns the socket with a stdio bridge** (§2):
   Claude's recommendation, taken with the plan.

Still open, for when the phase reaches them:

- the Collaborate window's keybind, the hold-to-blink key, and all GUI wording;
- the default `max_turns` (6 is a guess; llm-author runs used 4–6 rounds);
- subcommands vs flags for `mcp` and `collab` (§7);
- whether the headless backend's resumed session should be the *same* one as
  the channel session, so that one Claude remembers both, or a separate one.
