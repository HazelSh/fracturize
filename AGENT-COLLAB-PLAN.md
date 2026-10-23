# Agent collaboration — plan

*Status: plan, 2026-10-24. Not started. Written by Claude Opus 5.5 from Hazel's
brief and two rounds of answers (§10). Two directions across the CLI/GUI gap:*

- ***Human asks, agent proposes (§3–§6).*** *A dialog in the GUI sends the
  scene, a render and a typed request to Hazel's choice of LLM: her running
  Claude Code session, or a local model. What comes back is a proposal, which
  she accepts or rejects.*
- ***Agent asks, human reviews (§7).*** *An agent packages a short write-up and
  a set of rendered options. Hazel picks, comments, optionally edits one, and
  the agent gets back a verdict, a selection, a diff of her edits and a render
  of them.*

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
- **`/renders` is gitignored.** Reviews live under it (§7.1), so they never
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
        └──── channel events ◀──────────────┘   "Hazel asked …", "review answered"

 Local LLM (llama-server :8080) ◀──HTTP── the GUI's own agent loop (§6; no MCP)

 Any agent, GUI or not ──files──▶ renders/reviews/<id>/   +   fracturize review …   (§7)
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

Proposals (§4), reviews (§7) and the dialog's before/after all need to say
*what changed between two scenes*. One function, written first:

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

The second shared piece: **render a scene with recorded settings, from inside
the GUI**, using the same view, size, effort and grade as a given render record
(the render-records section of AGENTS.md). Reviews need it so that a human's
edit renders the same way as the agent's options. The GUI already renders
offline for render jobs, so this exposes that path; it isn't a new renderer.

## 4. Human-first: the Ask dialog

### 4.1 The window

A toolbar button (and a key, to be chosen against the keybind table) opens a
dockable **Ask** window:

- **The backend picker and a connection light** at the top: Claude Code
  (channel), Claude Code (headless) or Local LLM, plus a gear that opens §4.3.
  The light means: green, connected and idle; amber, working; grey, nothing
  connected, with one line saying how to connect.
- **The thread.** Hazel's requests, the agent's replies and proposal cards, in
  order. It is saved per scene path in `renders/ask/<scene-stem>/thread.jsonl`
  (gitignored), so reopening a scene shows its conversation.
- **The input box.** Send attaches, by default: the scene TOML (saved or not),
  a render of the current view, the selection, and the scene path. Small
  toggles under the box can turn the render or the selection off.
- **The state of each request**, as a line under it: *delivered*, *working*,
  *replied*, *ended*. A request sent while the agent is busy queues and says
  so. Silence should never be ambiguous.

### 4.2 Proposals, never edits

**Agents never change the live scene.** Everything comes back as a **proposal
card**:

- the agent's message;
- the §3 diff, collapsed to a one-line count ("4 values, +1 transform"),
  expandable to the full list;
- **Preview**: swaps the viewport to the proposed scene. The toolbar shows
  *Previewing proposal* in the place it shows the dirty marker, and Esc or
  clicking Preview again returns. Previewing touches neither undo nor the
  dirty flag;
- **Accept**: one `commit_edit("Agent: <first line of message>")`. It can be
  undone and shows in the History window like any other edit;
- **Reject**, with an optional one-line reason that goes back to the agent;
- **Stale**: if the scene changed after the request was sent, the card says
  so. Accept then applies the set-ops on top of the current scene where they
  still resolve, and refuses with the reason where they don't (a removed
  transform, say). It never silently overwrites Hazel's edits made in the
  meantime.

An agent may send several proposals for one request. They show side by side,
and each is accepted or rejected on its own.

### 4.3 Settings

These live in `prefs.toml` under `[agent]`, and in an **Agent settings** dialog:

```toml
[agent]
backend = "claude-channel"        # claude-channel | claude-headless | local

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

The dialog has a **Test** button for each backend: connect, ask for one word,
and show what came back, or the error.

## 5. Claude Code backends

### 5.1 The socket protocol (GUI ⇄ bridge)

JSON lines over `$XDG_RUNTIME_DIR/fracturize/gui.sock`, with
`{"id", "method", "params"}` in each direction. The GUI accepts **one bridge at
a time**. A second one gets a refusal that says which session already holds
it, and the Ask window shows that session's name.

### 5.2 MCP tools (`fracturize mcp`)

| tool | what it does |
|---|---|
| `get_state` | scene TOML, path, dirty flag, selection, view, render-panel settings |
| `get_render` | a PNG of the current view (existing screenshot path), returned as a file path *and* as image content; optional size |
| `propose` | `{request_id, message, scene_toml}`; the GUI computes the diff and shows the card (§4.2) |
| `reply` | `{request_id, text}`: a message in the thread, with no proposal |
| `end_request` | `{request_id, summary}`: the agent is done with this one |
| `request_review` | §7: a packet directory or inline fields; returns a review id at once |
| `review_status` | `{review_id}` → open / answered / withdrawn, plus the response when answered |

Channel events the bridge pushes into the session:

- `<channel source="fracturize" kind="ask" request_id=…>`: Hazel's text, the
  scene path, the paths of the TOML and PNG snapshots written for this
  request (under `renders/ask/…`), and the selection;
- `kind="verdict"`: a proposal was accepted or rejected, plus any reason;
- `kind="review"`: a review was answered, plus the path of `response.toml`.

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
Even so, the Ask window's amber light times out to a line saying "no reply in
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
missing file is a clear error in the Ask window, not a silent fallback.

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
- **A Stop button** in the Ask window ends the request at once, between turns.

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
  `renders/ask/<scene-stem>/<time>-<model>/` in llm-author's format, so
  `review.sh` works on it unchanged.

### 6.4 VRAM

The GUI renders while the model runs, on the same 8 GB GTX 1080. The settings
dialog shows the preset's weight size against free VRAM, using `with-llm`'s
1.2 GB headroom rule. Starting a preset that doesn't fit is refused, with that
reason. The headroom number is shared with `with-llm`, not copied.

## 7. Agent-first: reviews

### 7.1 The packet

A directory, so it works with no GUI, no MCP and no particular agent:

```
renders/reviews/2026-10-24-1530-kintsugi-seams/
  packet.toml
  A.toml  A.png
  B.toml  B.png
  C.toml  C.png
```

```toml
title   = "Kintsugi: which seam pattern?"
author  = "claude-opus-5-5"
message = """
Three takes on the gold seams. B is my pick: it keeps the bowl's silhouette.
C trades depth for clarity.
"""
ask     = "pick-one"            # pick-one | pick-any | free
base    = "scenes/kintsugi.toml"   # optional: what the options were derived from

[render]                        # how A–C were rendered; reused for Hazel's edits
record = "A.png"                # a render whose embedded record gives view, size, effort, grade

[[option]]
id     = "A"
scene  = "A.toml"
render = "A.png"
note   = "denser seams, d = 2.31"
```

`fracturize review new <dir>` validates a packet: every option must parse,
every render must exist, and `ask` must be one of the three. It prints the
review id. Up to 9 options; more is a sign the agent should narrow them down
before asking.

### 7.2 The Review window

- **Header:** title, author, the message, and a status of open, answered or
  withdrawn.
- **Option cards in a grid:** the render, the id, the note, and a select
  control (a radio for `pick-one`, a checkbox for `pick-any`, none for
  `free`), plus a one-line comment field per option.
- **Clicking a card's render opens that option in the viewport as a sandbox**:
  a copy, with its own undo stack, and a toolbar banner reading *Reviewing B*.
  Hazel's own scene is untouched underneath and comes back when she leaves.
  Edits in the sandbox mark the card *edited*, show the §3 diff against the
  agent's version, and a **Re-render** button renders the edit with the
  packet's `[render]` settings so it sits beside the others like for like.
- **Footer:** a verdict box (free text) and **Submit**. Submit renders any
  edited option that hasn't been re-rendered yet, then writes the response.
- **A Reviews list** (toolbar, with a count badge for open reviews) lists every
  packet under `renders/reviews/`, newest first. Reviews persist: an agent can
  ask while Hazel is away, and she answers whenever she next opens the GUI.
- **A new packet** fires `notify-send` and badges the toolbar. It never opens
  a window over her work by itself.

### 7.3 The response

`response.toml`, written to the same directory:

```toml
answered = 2026-10-24T16:02:11
verdict  = "B, but the seams want to be thinner — see my edit."
selected = ["B"]

[[option]]
id      = "B"
comment = "closest"
edited  = true
scene   = "B.edited.toml"
render  = "B.edited.png"
diff    = """
transform.seam-1.variations.linear=0.4
transform.seam-2.variations.linear=0.4
"""

[[option]]
id      = "C"
comment = "too flat"
```

An option Hazel neither selected, commented on nor edited is left out.
**Keep** on a card copies that option's scene (edited or not) to `scenes/`
through the existing Save-as flow, so a review outcome can enter the project
deliberately. Nothing else under `renders/reviews/` ever does.

### 7.4 Delivery

- **Over MCP:** `request_review` creates the packet (or takes a directory
  the agent wrote) and returns its id at once. When Hazel submits, the
  bridge pushes `kind="review"` into the session. The agent never sits
  blocked in a long tool call.
- **Over the CLI**, for agents without the bridge (headless scripts,
  `llm-author`, other tools):

  ```sh
  fracturize review new  renders/reviews/<dir>     # validate; prints the id
  fracturize review open <id>                      # starts the GUI in the Review window, or tells a running GUI to open it
  fracturize review wait <id> [--timeout 30m]      # blocks; prints response.toml; exit 0 answered, 3 timed out, 4 withdrawn
  fracturize review list [--open]
  ```

  `review` and `mcp` would be the CLI's first subcommands; today everything
  is a flag on one command, and the scene comes in through `-s`, so nothing
  positional collides. Clap's optional subcommand alongside the existing
  flags keeps every current invocation working. The alternative, `--review-new`
  and so on, matches house style but reads worse for a verb-shaped tool.
  **Hazel's call**; the plan assumes subcommands.
- **Withdraw:** an agent can withdraw an open review (`review withdraw <id>`,
  or the MCP equivalent). The card stays in the list, greyed out with the
  reason, so Hazel isn't left answering into nothing.

## 8. Phases

Each phase ends with a check per surface it touches, in the
`ADDING-A-SCENE-FEATURE.md` spirit, and an AGENTS.md section. CRAFT.md gains a
short "asking for review" note at phase 2.

1. **Diff and recorded render.** `src/scene_diff.rs`, `--diff`, the round-trip
   test over `scenes/` × mutations, and the GUI-callable recorded render.
2. **Reviews, file-based.** Packet and response formats, `fracturize review
   new|open|wait|list|withdraw`, the Review window, the sandbox, re-render,
   Keep. No IPC yet. `review open` with no GUI running just starts one. This
   ships first because it gives the most per line and puts §3 under real use.
3. **Socket and bridge.** The GUI listener and `EventLoopProxy`,
   `fracturize mcp`, the five live tools, channel push, the one-bridge rule,
   and `review open` reaching a running GUI. Checked with a real channel
   session and with `claude -p`.
4. **Ask dialog.** The window, the thread, proposal cards (preview, accept,
   reject, stale), Agent settings, the channel backend, then the headless
   backend.
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
  - the stale-proposal rule (§4.2) becomes a live rebase.

  Worth a test bed once phase 4 exists, since the bridge and the diff are
  already the pieces it needs.
- **Agent-to-agent review.** A second agent answering a packet. The packet
  format allows it (`author` is free text, and a response is just a file).
  The GUI doesn't need to know.
- **An API-key backend.** It would be straightforward to add beside the
  others if ever wanted. Nothing needs it.

## 10. Decisions taken

Hazel's answers, 2026-10-23/24:

1. **Proposals and acceptance only**, for now. Autopilot and shared editing are
   §9.
2. **Reviews are not committed.** They live under the gitignored `/renders`.
   The project is public, and review packets aren't history material. Keep
   (§7.3) is the only way a review's scene enters `scenes/`.
3. **Permissions:** Claude Code runs in auto mode, so there's no
   permission-prompt work. The amber-light timeout (§5.3) stays as the
   general "agent went quiet" signal.
4. **The local model gets a turn limit, then reports back** (§6.2), and the
   llm-author welfare approach carries over unchanged (§6.3).
5. **MCP over A2A** and **the GUI owns the socket with a stdio bridge** (§2):
   Claude's recommendation, taken with the plan.

Still open, for when the phase reaches them:

- the Ask and Reviews keybinds, and all GUI wording;
- the default `max_turns` (6 is a guess; llm-author runs used 4–6 rounds);
- subcommands vs flags for `mcp` and `review` (§7.4);
- whether the headless backend's resumed session should be the *same* one as
  the channel session, so that one Claude remembers both, or a separate one.
