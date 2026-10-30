# Fracturize GUI guidelines

*Status: first draft, 2026-10-30. Written by Claude Opus 5.5 from Hazel's
brief, with every fact checked against the code on `main` at f1be6fc. The
principles in §1 are Hazel's; the rest describes how the program works now,
plus a list of known gaps (§12). Edit freely.*

**What this is for.** It lets you make a GUI change that looks and behaves
like the rest of Fracturize after reading one document. AGENTS.md says what
each part of the program *is* and why. This says how the interface is built:
what to reach for, what it should look like, and which things you must not
undo. Where AGENTS.md has the full reasoning, this links to it.

**Who it's for.** Any Claude doing UI work, and Hazel. Plans name the people
they serve and the people they cost, using the names in `USERS.md` (Rook,
Pip, Jay, Theo, Remy, Atlas, Mote, and the anti-personas Abe, Silk, Socket).

**The CLI has its own conventions** (fixed-schema `--info`, one-line help
summaries, `[bracket]` defaults, rationed short flags). They're more settled
than the GUI's and are written up in AGENTS.md under "Reading a scene without
rendering it" and "Coding Conventions". A separate CLI guideline is still to
do (§13).

Contents:

1. Principles
2. The screen
3. Windows and how information is laid out
4. Type
5. Colour
6. The widget library
7. Help text: tooltips, status-bar hints, inline notes, notices
8. Input: pointer, keyboard, modes
9. Edits, history, documents and files
10. Performance, and sharing the machine
11. Humans and agents as equals
12. Known gaps
13. Later
14. Checklist for a UI change

---

## 1. Principles

These come first because every rule below follows from one of them. When a
rule doesn't cover your case, reason from these.

**1. The picture is the program.** The fractal fills the window and nothing
covers it permanently. The toolbar and status bar are thin, panels float, and
nothing shrinks the drawable area. The chrome is dark so the art stays the
brightest thing on screen, and the art is the only thing on screen that moves
on its own.

**2. Gritty and point-focused, on purpose.** The live view draws points, not
a smoothed image. Aliasing and grain are the look, not a defect: the
in-world UI is multisampled and the artwork is not, so the gizmos are clean
while the point cloud keeps its grit. The point ring is always turning over
(a full refresh about every 13 seconds, about 1 second while a timeline moves
the attractor or the emphasis is filling), so the cloud *fizzes*: it is
always being redrawn from new samples, which tells you it's alive and lets
you see its density. Don't hide that by making the live view converge to a
still, smooth image. Converged, smooth renders are something you choose in a
render job. (Silk is the anti-persona this protects against.)

**3. Apophysis got a lot right.** Fracturize treats Apophysis as a good
program to learn from, not a legacy to escape. Instant feedback on every
edit, a gritty preview, a gradient you can see, xaos with its goes-to /
comes-from view, linked transforms, named render presets and Ctrl+Y all come
from it on purpose. A lot of what came after it, especially smooth continuous
rendering that converges in the live view, was a step backwards for making
art. When we differ from Apophysis it's for a reason that protects the
artwork (scroll never edits; ctrl snaps instead of alt), and we say so once,
plainly (Rook). We don't copy it exactly (Abe).

**4. Live, everywhere.** Every edit takes effect on screen right away: a drag
re-forms the attractor while you drag, a gradient handle recolours the cloud
as it moves, the point count slider reallocates under your hand
(rate-limited, but live). There is no Apply button for anything that can be
shown live. A commit button is only for actions that are expensive or that
leave the program (render, send, save).

**5. Performance is a feature.** The target is 120 FPS on Hazel's desktop
(GTX 1080) and 60 FPS on the T490 laptop (Intel UHD 620). The way to reach
that on any machine is the point count and the renderer (points, splat,
volume), both of which you can change live, so a weak machine can stay
responsive and a strong one can draw a denser picture. Chrome has a budget
too: the status bar shows `ui Xms` and p99. Watch p99, not the mean.

**6. Zero animation in the chrome.** No fades, no easing, no smooth
scrolling, no tweened panels. Every click shows its full effect on the next
frame. This is enforced once, in `EguiLayer::new`, and is not to be undone
per widget. There are no exceptions: the one there was, a pulsing encode
bar, went with egui's `ProgressBar` (§6.10a).

**7. The chrome doesn't move.** Numbers don't change width as they change
value, buttons don't resize when their caption changes, and toggling a
control doesn't make the form below it jump. A control that's not available
is greyed in place, not removed.

**8. Nothing is lost.** Every change to the artwork can be undone with
Ctrl+Z, and the History window can jump to any earlier state. Destructive
actions are armed, not confirmed by a quick second click. Quitting, opening
another scene and starting a new one all ask first when there are unsaved
edits. A stopped render keeps what it has. Escape cancels; it never quits.
This is what lets Pip click without fear and Jay wander for hours.

**9. Two kinds of mode only: edit and view.** View mode is for looking: no
gizmos, and the pointer hides after two seconds of stillness. Edit mode shows
the gizmos and the pointer always stays. Separately, the camera is either
*attached* to the timeline (it flies its path when you play) or *detached*
(it stays where you put it while tracks play). Grabbing the camera from
anywhere in the viewport detaches it. Play attaches it. You can grab the
camera at any time, in either mode, while anything is playing.

**10. A scene is a document, and TOML is the format.** Scenes, views, dice
profiles, render presets, prefs, threads and render records are all TOML that
a person can read and edit by hand. A save keeps comments and formatting. If a
change alters what Ctrl+S writes, it's an undoable edit; moving the camera is
the one conventional exception.

**11. The CLI is an equal path, not a fallback.** Everything you can author
in the GUI can be authored from a terminal, and the reverse. Both go through
the same `App` methods or the same scene format, so they can't drift apart.
Agents (Atlas, Mote) and terminal-first people use the CLI. Pip uses the
window. Remy uses both. Collaboration between a person in the window and an
agent at the CLI is a core feature, not an add-on (threads, the MCP bridge).

**12. Be a good resident of the computer.** Hazel's desktop may be running
her session, two Fracturize windows, a local LLM on llama-server, a render
job and an agent, all at once. Background work runs on its own device and
threads and leaves a core free. GPU benchmarks are throttled. Prefs don't
clash between processes. The window never grabs focus or input from other
windows. (§10.)

**13. Disabled, not hidden, and say why.** A control that disappears can't
tell anyone the feature exists. Grey it where it is and give it a tooltip
that says how to make it available.

**14. Plain words.** Interface text describes what something does, not how
it's implemented. "Makes the far parts fade", not "aerial perspective
transmittance" (Pip). Numbers stay available one step down (Theo). §7 has the
details.

**15. Boxes with edges, not pills.** Controls are squares or rounded squares
with a border you can see, grouped into sections you can see the edges of.
Information goes in tables with zebra stripes. The current fashion of
borderless, fully-rounded blocks of flat colour floating wherever there's a
control (a touch-screen look) is what this interface doesn't do. §3 "Shape
and surface" has the details.

---

## 2. The screen

```
┌──────────────────────────────────────────────────────────────────────────┐
│ File Explore │ Transforms Subscenes Xaos Collab │ Camera Timeline Render │ │  toolbar
│   │ Undo Redo │ quick controls │ transport          scene name* │ Help   │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│   floating windows over the viewport          world-anchored labels      │
│   (Transforms docked left, Render right…)     gizmos, traces, indicators │
│                                                                          │
│                                                        axis cross  (o)   │
│   Timeline (wide and low, above the status bar)                          │
├──────────────────────────────────────────────────────────────────────────┤
│ hint for whatever is under the pointer     d Λ · pts · FPS ms p99 ~~~~  │  status bar
└──────────────────────────────────────────────────────────────────────────┘
```

### The viewport

Everything between the two bars, always the whole window, whatever panels are
open, so aspect ratio and picking math never depend on the layout. Things
drawn in the viewport, from back to front:

- the point cloud, in one of three renderers;
- the in-world overlay (gizmos, traces, camera path, indicators), in its own
  multisampled target, composited over the cloud and occluded by it;
- world-anchored egui painting on the `Background` layer, under every window:
  transform name labels, the roll ring, xaos edge arrows;
- the corner axis cross (bottom right; click a ball to look down that axis).

Anything you add to the viewport must stay legible over *any* attractor, from
black to near white. That's why labels carry a 1px black shadow and origin
dots have a rim. White text with no backing isn't legible over a pale flame.

**Dividing the main screen** is allowed when a task really needs two views at
once (multiview panes are planned for the mutation grid). If you do it: each
pane is a full viewport with its own camera and nothing else in it; the
toolbar and status bar stay full width; picking and aspect ratio are computed
per pane; and the divider is draggable and saved to prefs like a window's
geometry. Don't split the screen to make room for chrome. Chrome floats.

### The toolbar (top, `ui/toolbar.rs`)

From left to right, ordered by task:

| Group | Contents | Kind of control |
|---|---|---|
| Where a scene comes from | **File**, **Explore** | menu buttons (§6.6) |
| What I'm working on | Transforms, Subscenes, Xaos, Collaborate | window toggles |
| How I'm looking at it | Camera, Timeline, Render | window toggles |
| | Undo, Redo | command buttons |
| Quick controls | renderer radio, point count button (opens a slider popup), Edit/View mode, emphasis | each delegates to its panel's own widget |
| | transport: record, to start, play/pause, time, loop | `ui/transport.rs` |
| (right) | scene name and author (click to edit), `*` when unsaved; **Help** | |

Rules:

- **File and Explore are the only menus.** There's no Edit/View menu bar,
  because it would be mostly empty duplicates of the toggles. Don't add a
  third menu without a group of commands that is conventional, numerous and
  infrequent (that's the trade-off a menu is right for).
- **A quick control goes on the toolbar only if it's reached for constantly
  and the panel version is friction.** It must call the panel's own widget
  function, never a copy of it.
- **When space runs out, window toggles drop their captions and keep their
  icons** (`toolbar_compact`, with hysteresis so it doesn't flicker). Plan
  for it: every toggle's icon has to identify it alone, and its tooltip says
  the name.
- **The transport is always visible**, whether or not the Timeline is open,
  because a hand reaches for play without looking.

### The status bar (bottom, `ui/status_bar.rs`, 22px)

- **Left: the hint.** What the thing under the pointer does, as
  `verb: action · verb: action`. Precedence: a file being dragged over the
  window → a hovered `hinted()` widget → a hovered gizmo part → the viewport
  default (`drag: orbit · shift/middle-drag: pan · right-drag: roll · scroll:
  zoom`).
- **Notices** (`App::notice`, with `Tone::Note / Success / Error`) show here
  until clicked away or replaced, never on a timer: "Couldn't open x.toml:
  …", "render done". If a notice is clickable, clicking it opens the thing
  it's about.
- **Right: the instruments.** Points (`valid/capacity`, `(warming)`), the
  drawing budget when it's in effect, `zoom +N`, the render job's progress,
  the dimension `d` and lacunarity `Λ` of the layer being listed, and the
  performance cluster: FPS, mean ms, p99 ms, `ui` ms, present wait, and a
  120-sample sparkline. Every readout has a fixed character budget (§6.2).
- The status bar describes state and the hint. It doesn't hold controls,
  except for the dimension lock and clickable notices.

### Windows

Every other control lives in a floating `egui::Window` opened from the
toolbar or a menu. See §3.

---

## 3. Windows and how information is laid out

### What every window has in common

- **Built with `ui::window(ctx, app, WindowKey, title)`.** That gives it a
  stored position and size (in `prefs.window_geometry`, saved lazily), a
  default from `ui::default_layout` as a function of screen size, a minimum
  size, and resizability. Call `ui::remember` right after showing it.
- **A minimum height that fits its fixed controls** (`WindowKey::min_size`).
  This is required, not cosmetic. A window that's too short gives its
  scrolling middle a negative height, and that middle then draws over the
  bottom-pinned rows *and takes their clicks*. The buried controls still
  highlight and show tooltips, so it looks like a broken widget. Widths are
  only a legibility floor.
- **Its open state is saved to prefs**, and its toolbar toggle or menu row is
  lit while it's open.
- **It isn't modal.** The scene keeps running and can be orbited behind it.
  The one true modal (`egui::Modal`) is the unsaved-changes dialog, because it
  stands between you and losing work. Save as and the merge conflict list are
  small dialogs in front of the work, closed by Esc.
- **Esc closes the frontmost dialog** (the order is a menu, then a dialog,
  then the browser, then the selection).
- **Pinned top and bottom, scrolling middle.** Things that apply to the whole
  window (a profile picker, a preset bar) are pinned at the top with
  `Panel::top`. Commit actions and the cost of committing (Start and its
  estimate; Roll one; Mutate) are pinned at the bottom. Everything else
  scrolls between them.
- **The window title is a noun** (Transforms, Render, Random settings). No
  "settings for…", and no window titled by an action.

### Sections

A settings window is split into folding sections with
`ui::section(ui, window, title, |ui| …)`: a strong heading that collapses,
open by default and remembered once folded. Order the sections by the
questions people ask, not by how the code is organised. Example: Render job is
Output → Size → Quality → Animation → Look → This machine.

**A disclosure hides elaboration, never the headline.** Haze's amount is
always visible and only the band's pin/near/far fold away. If a section can
be closed while its feature is *on*, the fold is in the wrong place.

### Group by how long a value lasts

There are three kinds of value. Group them under headings that say which
kind:

| Class | Lasts | Examples | Heading wording |
|---|---|---|---|
| Scene | in the file, undoable | haze, background, exposure, palette | (the window's main body) |
| Preference | follows the person to every scene, in prefs.toml | orbit style, FoV, invert pitch, UI scale, point count | "Preferences", "This machine" |
| Session | ends when the program closes | volume opacity, loop region, preview state | "Session" |

Group them *within* a window, not across windows: the task is why the window
was opened, and splitting by persistence would scatter it. Before this rule,
nothing showed which sliders would end up in your file, which is how
`exposure` went unsaved for so long.

### Order inside an inspector

Order follows how people use it, not how the code runs. The Transforms
inspector is Weight → Appearance → Shape → Variations → Symmetry →
Post-affine → Xaos (roughly Apophysis's order, not the order the map runs
in). The most-edited property, a transform's name, is edited in the header
where it's displayed, not in a section of its own.

### Shape and surface

Hazel's references, in her words: Windows 7's clear squares with highlight
outlines, and Windows 3.1's large, clear borders, are the good end. Windows
10's flat glass materials are acceptable. Not XP's big soft skeuomorphism,
not 95/98's fake-3D grey on grey, and not Vista's glossy shine. Above all,
not the current style of disconnected, unmarked pills: block colour with no
gradient and no border, where only the shape says there's a control.

What that means in practice:

- **Corners are slightly rounded, never fully.** Use egui's radii: windows
  and menus 6, widgets 2–3 (`window_corner_radius`, `widgets.*.corner_radius`).
  A bar, a box or a cell is a rounded rectangle; nothing is a capsule. This
  is why the render progress bar is drawn by `ui::progress`, not by egui's
  `ProgressBar`, which is a borderless pill.
- **Edges are visible.** A trough, a group, a bar and a selected thing each
  have a 1px border (`widgets.noninteractive.bg_stroke` for structure, the
  selection colour for what's selected). If a control would only be
  recognisable by its fill colour, give it an edge.
- **Groups are drawn as groups.** A set of options is one connected shape
  (`ui::radio`); a section has a heading and a rule (`ui::section`); a table
  has stripes and rules (`ui::table`). Don't scatter separate floating
  buttons where one grouped control would say what belongs together.
- **Tables and zebra stripes** for anything that lines up (§3 "Tables versus
  grids").
- **Flat, with no gloss or bevels.** No gradients on controls, no drop
  shadows inside windows, no fake 3D. Depth comes from fills (recessed
  troughs are darker) and edges, not from lighting effects.

### Selector and detail

When you choose one of a list and then edit it, use a **tab rail plus a
detail pane** (the Transforms window). The selected tab is **filled with the
selection colour, like the chosen segment of a radio**, with its text in the
selection's text colour, because choosing one transform is a choose-one like
any other. It runs past the rail's edge into the pane, so it points at the
fields it owns. The rail is for choosing, and nothing else on a tab takes a click:
swatch, name, eye, mute and solo, nothing more. Rows are a fixed `TAB_HEIGHT` because the
list is virtualised, and the list takes `ui.available_height()`. Add,
duplicate and delete go *above* the list, at the end that doesn't move.

### Tables versus grids

- **A table** (`ui::table`) is for values that line up: the same kind of
  value down a column, with a header that says what it is. Used in the dice
  profile, the render job form and the xaos targets.
- **`egui::Grid`** is fine for a label-and-control form of a few rows where
  no column holds one kind of value.
- **If you're tempted to put different kinds of value in one grid's
  columns** (chances beside low/high pairs), make two tables. The Random
  settings window used to do that and read as a scatter of fields.

### Showing numbers to Pip and to Theo

Layer it: the effect in words on top, the number one step down (in the field,
the tooltip, or a folded section). Never remove the numbers, because Theo and
Rook need them. Never lead with them, because Pip leaves.

---

## 4. Type

### Fonts

| Family | Font | Used for |
|---|---|---|
| Proportional | egui's built-in (Ubuntu-Light, with NotoEmoji and emoji-icon-font as fallbacks) | all words: labels, buttons, headings, tooltips |
| Monospace | **Envy Code R**, found through fontconfig; egui's built-in Hack when it isn't installed | **every number that changes** (§6.2), typed time and path fields, `--info`-style text |
| Icons | **Phosphor** (regular), vendored at `assets/fonts/Phosphor.ttf`, registered as a fallback in *both* families | toolbar, buttons, menu rows |

`ui::install_fonts` sets all of this up. Envy Code R is never required.

**Check a glyph exists before you type it.** Several obvious symbols are in
none of these fonts and render as tofu (a blank box): `→` (U+2192; use
`icons::ARROW`), `⭘` and `◉` (the radio draws its own circles). Check with
`fc-list` or fontTools, and remember that a glyph only in Hack works in
monospace text and nowhere else.

### Sizes

Sizes are egui 0.35's defaults, which we don't override. All of them scale
with the **UI scale** preference (Controls window, 0.7–2.0, applied as egui's
zoom factor):

| Style | Size | Use |
|---|---|---|
| `Heading` | 18 | rarely; a window title is enough heading |
| `Body`, `Button` | 13 | almost everything |
| `Monospace` | 13 | numbers, typed values |
| `Small` | 9 | secondary lines under a row (author and age in the browser) |
| `.strong()` Body | 13 | section headings (`ui::section`), table headers, danger labels |
| `.weak()` / `.small().weak()` | 13 / 9 | inline notes, units, "unset", secondary info |

Painted text uses fixed sizes, all between 9 and 13: transform labels 12,
thumbnail captions 11, timeline ruler and lane names 10–13 (ruler numbers in
monospace 10), the axis cross's letters monospace 9, the browser's `PNG` chip
10. **For new painted text, use `TextStyle::…resolve(ui.style())` rather than
a literal size**, so it follows the UI scale and the defaults. (The existing
literals are a gap; see §12.)

Emphasis comes from weight (`.strong()`) and dimming (`.weak()`), not from
size. A window shouldn't need more than two sizes.

---

## 5. Colour

### The chrome is dark

The UI is egui's dark theme with its defaults: panels and windows at grey 27,
text at grey 140–240 depending on state, text fields at grey 10, and so on.
Dark chrome keeps the fractal the brightest thing on screen and doesn't
fight a flame's colours. **There's no light theme**: `EguiLayer::new` sets
`egui::Theme::Dark` whatever the desktop's theme is, because every colour
painted by hand here was chosen against dark panels.

### The palette, by meaning

| Colour | Value | Means | Used for | Defined in |
|---|---|---|---|---|
| **Selection, fill** | default `#005C80` (0, 92, 128); **a setting** | *chosen / selected / lit* | the chosen segment of a radio; a lit toggle (open window, Edit Mode, the emphasis bulb, Snap); the selected Transforms tab; the selected timeline track's name; the filled part of a progress bar; the loop region | egui `visuals.selection.bg_fill`, read through `ui::theme` |
| **Selection, text and edge** | default `#C0DEFF` (192, 222, 255) | *this one* | text on a selection fill; the playhead; a selected key; the chosen mutation tile's border | egui `visuals.selection.stroke` |
| Selection tint + edge | derived | *this row is selected*, under content that has to stay readable | selected rows in the Scenes browser and Random flames; the previewed Collaborate card; the xaos matrix's selected row and column; the selected gradient stop | `theme::selected_tint`, `selected_edge`, `paint_selected` |
| Hyperlink blue | `#5AAAFF` (90, 170, 255) | a link | egui's own; we have almost no links | egui |
| **Success green** | `#5ADC78` (90, 220, 120) | *done, connected, under budget* | "Render done" and "View saved", "sent"/"kept" notices, a connected agent light, a frametime that makes 60 FPS | `theme::SUCCESS` |
| **Caution amber** | `#E6AF3C` (230, 175, 60) | *a caveat on a value*, not a problem | a sheared matrix, a frametime over budget | `theme::CAUTION` |
| **Warn orange** | `#FF8F00` (255, 143, 0) | *wants a look* | waiting on you; render stopped early; paused and stopping (text and progress bar); "will be overwritten"; a note-toned notice; xaos and timeline warnings | `theme::warn(v)` = egui `warn_fg_color` |
| **Error red** | `#FF0000` (255, 0, 0) | *failed or impossible* | open, save and preset failures; render failed; "that's a folder"; a render the GPU can't hold; an armed danger button's fill | `theme::error(v)` = egui `error_fg_color` |
| Record red | the error red | *recording* | the record button's dot while armed, and nothing else | `theme::recording(v)` |
| White, black shadow | | *legible over any picture* | world labels, the held roll ring, gradient handles | painted |

**The selection colour.** It means *chosen* and nothing else:

- It marks which of several things is current: the chosen option, the open
  window, the selected transform, track, row or key, where the playhead is,
  how far a render has got.
- It's never decoration, never a heading colour, never "important", never a
  status. A blue word that isn't selected is a link, and we have almost none.
- **It's a preference** (Settings → "highlight colour", `selection_colour` in
  prefs.toml; Default puts egui's blue back). So never write it as a
  literal: read `ui.visuals().selection` or the `ui::theme` helpers, or the
  person's colour won't reach your widget. `theme::selection_for` derives
  the text/edge colour from the fill.
- **Two strengths, by surface.** A *control* that is chosen (a radio
  segment, a toggle, a tab, a track's name) is **filled** with it, the way
  egui fills a selectable button. A *row of content* that is selected (a
  scene in a list, a card, a matrix row) gets `paint_selected`: half-strength
  tint and a 1px edge, so its thumbnail and text stay readable. A **drawing
  surface** (timeline lanes, the viewport) is never tinted: mark the thing's
  label, or the thing itself (a key's ring), not the canvas it sits on.
- One colour. Don't add a second blue with another meaning. The status bar's
  dimension lock (`#64C8FF`) still does, and is listed as a gap (§12).

**The status colours: one meaning, one colour, one place.** Every success,
caution, warning, error and record colour comes from `ui::theme`. Nothing
reads `visuals.warn_fg_color` or `error_fg_color` directly, and nothing
writes its own green or red. A finished render, a sent message and a
connected agent are the same green on purpose. Choosing one:

| Situation | Colour | Example |
|---|---|---|
| It worked, it's done, it's connected | success | "Render done" |
| A value has a caveat, nothing's wrong yet | caution | "sheared" beside a matrix |
| It needs you to look, or it's in a state you should know about | warn | "This file exists and will be overwritten", "paused", "Stopped early" |
| It failed, or can't happen | error | "That's a folder. Add a filename to the end." |
| Neutral, or it's over and nothing's wrong | weak text | "Stopped before any frames were rendered" |

Text, not colour alone, carries the message: every coloured state also says
what it is in words, because colour alone fails anyone colour-blind and any
agent reading a screenshot (Mote).

### Domain colours (about the scene, not the chrome)

These belong to what they draw, live beside it, and are not part of the
theme. Never reuse them to mean a chrome state.

| Where | Colours | Meaning |
|---|---|---|
| A transform's swatch, labels, gizmo | the transform's own RGB, or its gradient position | identity of the transform. This is how transforms are told apart: by colour and name, never by an icon |
| Gradient strip | the colormap, sRGB-encoded | what renders. Handles are white with a black outline |
| Timeline track class | view `#78B4E6`, colour `#E1BE5A`, placement `#96D28C`, structure `#E67878` | what a frame costs (re-render, recolour, re-place, refill) |
| Xaos heatmap | grey (multiplier 1) → orange `#D6A046` by probability; above 1 toward `#D67834`, below toward `#4078C8` | how likely an edge is. A cut is drawn as **×**, not as a shade |
| Xaos overlay | grey edges; cut edges `#E65A50` with ×; the held edge with a white halo | the selected transform's exits |
| Thumbnails before they arrive | the scene's own background colour | a placeholder that matches |
| Scene background | scene data, linear RGB | the artwork's ground |

### Picking colours inside a feature

If a feature needs its own colour scale (like the timeline classes), choose
colours that are distinct from the selection colour and the status colours, at
about the same lightness as each other, readable on grey 27, and named by
what they mean in a `const` or function next to the drawing code, with a
comment saying so.

---

## 6. The widget library

Look for the module before you build a widget that resembles one. Each one
here is a precedent that encodes rules, not just a convenience.

### 6.1 `hints::hinted`: every interactive widget

```rust
let resp = hinted(resp, &mut app.ui_state, "Turn this transform back on (Enter)",
                  "click: enable · alt+click: show only this one");
```

Attaches the tooltip and sets the status-bar hint in one call, and it works
on **disabled** widgets (egui's `on_hover_text` silently shows nothing on a
disabled widget). A widget without `hinted()` is a widget nobody can learn
about. The only exception is a bare drag on empty viewport space, which falls
back to `HINT_VIEWPORT`. Gizmo parts have their own `HINT_*` constants.

### 6.2 Numbers: `ui::num`

**Every number box is `num::Drag`, never a bare `egui::DragValue`.** It
uses the same builder methods; the difference is the drag. egui's DragValue
snaps to the roundest nearby number each frame, so values stick and jump.
`Drag` adds up movement exactly, at half the call site's speed
(`num::DRAG_GAIN` = 0.5): every pixel moves the value by the same step,
reversing at a limit responds on the first pixel, and the output is rounded to
a grid ten times finer than a pixel's step so saved scenes don't fill with
`0.12345679`. Click to type an exact value, Shift for fine: those are
egui's.

**Every numeric readout goes through `num`**: `fixed` (round before testing
the sign, so `-0.004` shows `0.00`, not `-0.00`), `cell` / `text_cell` (a
declared character budget, right-aligned, monospace), and `readout` /
`drag` (the *widget* has a fixed size, not just the string). Choose the
budget from the range the value can actually take. A bare `format!("{:.2}",
v)` in a label that sizes to its content is a regression: a value hovering
around a boundary re-lays out its row every frame, and the interface looks
like it's vibrating. No screenshot shows this, which is why it's a house
rule.

**Buttons whose caption changes** (Edit Mode / View Mode, Play / Pause, the
point count): pin the width with `num::icon_button_width` /
`text_button_width`, then pin the caption to the left with an
`egui::Atom::grow()` after it, so neither the neighbours nor the button's own
icon move.

### 6.3 Sliders and drag fields

| Control | When | Example |
|---|---|---|
| `num::Drag` | an exact value with no natural range, or one you'll type: positions, angles, scales, weights, seeds | transform fields, keys' times |
| `egui::Slider` with a value box | a bounded amount you set by feel, where seeing the position in the range helps | haze amount, colour contrast, gradient rotate, mutation strength, FoV, UI scale |
| a log slider | a range spanning orders of magnitude | point count (applies live, at most one reallocation per 250 ms) |
| a custom bar | a *share* of a whole | the Weight share bar under the Transforms list |

**The Weight share bar** (`transforms::draw_weight_share`) is one segment per
enabled transform, the selected one first in its colour, the rest dimmed.
Each segment carries the transform's name in small text, black or white by
the segment's brightness, clipped at the segment's edge rather than
shortened. A segment narrower than `SHARE_NAME_MIN_PX` (14px, two or three
letters) shows colour only, and past `SHARE_MAX_SEGMENTS` the others merge
into one grey segment. Drag along it to set the selected transform's share.

Rules:

- Both kinds of control are **edit and drag in one**: drag to change, click
  to type. Never a separate text box beside a slider.
- **The value applies live while dragging.** A drag is one undo step (edits
  with the same key are merged within one second).
- **Fixed width.** A slider's value box is a `num` cell.
- **Signed values pass through zero smoothly.** Variation weights can be
  negative, and a row stays where it is at 0 so a drag can pass through.
- **Clamp to what the scene can accept** (`params::range_of`), and use
  `SliderClamping::Always` where going out of range has no meaning.

### 6.4 Toggles, radios, checkboxes

- **Toggle button** (`ui.toggle_value`, lit in the selection colour when on): a mode
  or a window that is on or off and that lives in a strip of buttons — the
  toolbar's window toggles, Edit Mode, emphasis, record.
- **Checkbox**: an on/off setting in a form or table. In a table it goes in
  the **On** column, flush right (`Col::tick`), so ticks line up from one
  table to the next. Checkboxes are also the right way to confirm something
  safely (Save as's "overwrite" tick), because the second click of a
  double-click turns a checkbox back off.
- **Segmented radio** (`ui::radio`): choose exactly one of a few options,
  with no off state — renderer, colour source, camera loop. One connected
  shape, the chosen segment filled in the selection colour, a drawn ring and dot on
  each segment. Each segment has its own `hinted()`. A segment that's not
  available is greyed in place, with the reason in its tooltip. **Never draw
  a choose-one setting as a row of toggle buttons**: that suggests they could
  all be off, or all on.
- **Choose one of many** (more than about five options, or a list that grows,
  like transforms or presets): a combo box or a picker list where only one row
  can be lit (the zoom map picker), not a radio.

### 6.5 Buttons

| Button | Looks like | Example |
|---|---|---|
| **Command** | icon + verb, plain button | Undo, Roll one, Duplicate |
| **Opens a dialog or asks for more input** | caption ends in **`…`** | `Open…`, `Save as…`, `Render job…`, `Mutate…`, `Animate a parameter…`, `Only to…` |
| **Opens a window** | a toggle (lit while the window's open) on the toolbar; a `…` row in a menu, also lit while open | Transforms; Explore → History… |
| **Shows a menu** | caption names the menu, no `…`; the menu opens below | File, Explore; a point-count button that opens a popup |
| **Destructive** | `confirm::danger_button` (§6.9) | Delete preset, Stop render, Discard edits |
| **Commit / start** | pinned at the bottom of its window, beside its cost | Start (render job), Send request |

The `…` is a promise that the click asks for more before it acts. A button
that acts at once doesn't get one, and one that opens something does.

### 6.6 Menus

- **Menu rows are `menu_item`**: the whole row is one target, label on the
  left, the shortcut greyed on the right (`Button::shortcut_text`), an icon
  leading the label, and `hinted()` with a tooltip that says what it does.
- **The order is the conventional one** wherever there is a convention
  (File: New, Open, Save, Save as | Screenshot, Render job | Quit), because
  people bring that habit from other programs.
- **A command that's unavailable is greyed in the menu**, never removed.
- **Context menus** (right-click) offer the same actions in every place the
  object appears. A transform has one menu, the same from its gizmo and from
  its row. Timeline lanes, keys and xaos edges each have one.

### 6.7 Tables: `ui::table`

`egui_extras::TableBuilder` in the house style, which Hazel approved ("very
clean"): strong row stripes, faint column rules (you read along a row), an
underlined header, a `hinted()` title on each column, the name column taking
the spare width, exact widths otherwise, and the switch in an **On** column
flush right. A table never scrolls by itself; it sits in its window's scroll
area. A row whose part is off greys out. A row that can't be used shows what
it needs *in place of its control* ("needs Samples: accumulate"), naming the
row that would make it available.

### 6.8 The colour picker

egui's `color_edit_button` (a swatch that opens egui's HSV picker), used for
the transform colour, gradient stops and background. Two rules:

- **Use the right space.** Scene colours are stored in **linear** RGB.
  `color_edit_button_rgb` takes linear values (the background); the
  transform and gradient swatches convert to sRGB and use
  `color_edit_button_srgb`. Getting this wrong makes everything look washed
  out or too dark.
- **It's live.** Dragging in the picker recolours the cloud each frame
  (`App::push_colormap` re-uploads 256 entries; the points keep their index,
  so the chaos game doesn't re-run). A pick is one undo step.

Anything that *displays* a palette encodes to sRGB first, which is why the
gradient strip reads brighter than the transform swatches beside it. The
strip is correct; it's what renders.

### 6.9 Destructive actions: `confirm::danger_button`

Click, then it's **disabled and greyed with a countdown** (`waiting (3)`),
then it's **enabled in danger colours with a more explicit caption** (`Stop`
→ `Stop render`), it **stays that way** until clicked or until you click
anywhere else, which puts it back. It's drawn in one fixed-width cell, and the
icon leads every state. Use it for every new destructive action; don't build
an "are you sure?" popup. A plain two-click guard is beaten by a
double-click, ignoring clicks during the wait looks broken, and a timeout
guesses wrong in the dangerous direction. The full reasoning is in
`confirm.rs`.

The one real modal is the unsaved-changes dialog (`confirm::draw`), because it
stands between you and losing work.

### 6.10 Timeline controls (`ui/timeline.rs`, `ui/transport.rs`)

- **Transport** (toolbar): record, to start, play/pause, the time (seconds
  *and* frames at the Render job's fps; type `12.5`, `300f` or `1:02.5`
  while paused), loop. Play flies the camera (attach and play). O, Z and
  Space all play.
- **The Timeline window**: its own Play (clock only; it leaves the camera
  where it is), Snap (the magnet, on by default; Ctrl flips it for one drag),
  Whole (zoom to fit).
- **The ruler is fixed; the lanes zoom.** Only the ruler scrubs. Clicking a
  row selects it and moves nothing else. Ctrl+scroll zooms around the
  pointer; Shift+scroll or a middle-drag moves along.
- **Keys are diamonds.** Drag one to change its time (it snaps to keys, the
  playhead, the loop ends and ruler ticks within `SNAP_PX`). Right-click for
  copy, paste, duplicate and delete. The selected key has an accent ring.
- **The loop region** is in a strip along the top of the ruler: drag to make
  one, drag its ends or middle to move it.
- **Rows are named for people** (Transforms › petal › Rotation), never as
  `-S` paths, which only appear in tooltips. New tracks come from the
  parameter picker (`ui::param_picker`), never from typing a path.
- **The column on the right edits the selection.** The lanes are for timing,
  the column for values.
- A track's class colour (§5) is shown as a band on its row.
- **The selected track is marked on its name, not its lane.** Its name cell
  is filled with the selection colour like a chosen radio segment; the lane
  stays clear, because the lanes are a drawing surface for keys and curves
  and a tint over one sits on top of what you're editing. A selected key is
  drawn in the selection's text colour.

### 6.10a Progress bars: `ui::progress`

`progress::bar(ui, fraction, text, tone)`: a rounded rectangle at the
window's corner radius, a recessed trough with a 1px border, the done part
filled and clipped to the same shape, and the caption centred on it in the
strong text colour. `Tone::Running` fills in the selection colour;
`Tone::Held` (paused, stopping) fills in the warn colour at 60%. The caption
comes from a fixed-width source (`num`), two decimals, because a whole
percent sits still for minutes on a long render and looks hung. The render
job window uses it, one bar for the whole job.

Don't use egui's `ProgressBar`: it's a borderless pill (§3 "Shape and
surface"), and its `.animate(true)` pulse bypasses the no-animation rule. A
bar with no fraction to show says so in its caption ("working…") and
doesn't move. (The old encode-bar pulse, the one documented exception to the
rule, went with it: nothing drew it any more.)

### 6.11 Pickers and lists of things

- **Parameter picker** (`ui::param_picker`): a browsable tree plus search by
  any words in any order, each row showing its current value; ones that can't
  be chosen are greyed with the reason. It's generic: reuse it whenever the
  question is "point at a value in this scene".
- **Profile picker** (`random_settings::profile_picker`): built-ins under
  their own heading, then the person's own; description on hover; `*` while
  edited. Render presets use the same pattern.
- **Thumbnail lists** (Scenes, Random flames, Mutate): live renders, never
  cached PNGs, framed with the scene's own camera, never reframed. **Only the
  row under the pointer moves** (flies its camera). A list where everything
  moves is a list nobody can look at.

### 6.12 Icons: `ui/icons.rs`

- Phosphor, regular. Codepoints are copied by hand into `icons.rs` (there's
  no egui-0.35 `egui-phosphor`). Add a constant there; never put a raw
  `\u{…}` in a call site.
- **Icons name actions and windows, never domain objects.** A transform is a
  colour swatch and a name. A variation is a name.
- An icon leads its caption, consistently, so a column of buttons lines up
  by icon.
- An icon on its own must still say what it is, because the toolbar can drop
  captions; its tooltip gives the name.

### 6.13 Things painted in the viewport

- Gizmos: only the selected transform can be manipulated; an unselected one
  offers only its origin dot, and that dot only selects. Hovering makes a
  part glow and grow; *held* is a different state (its own colour at full
  strength, without the white mix).
- Anything painted in screen space that you can also grab has **one
  function** that defines its geometry, used by both drawing and picking
  (`pick::roll_ring`, `ui::xaos::overlay_edges`). A control drawn in one
  place and grabbed in another is worse than none.
- World labels: the selected or hovered one gets a solid backing plate;
  others get a 1px black shadow (lay the galley out in
  `Color32::PLACEHOLDER` so the shadow takes its own colour).

---

## 7. Help text: tooltips, status-bar hints, inline notes, notices

Four channels, each with one job. When unsure, ask *when does the person need
this?*

| Channel | Answers | When it's shown | Form |
|---|---|---|---|
| **Tooltip** | *What does this do?* | while hovering a widget | one or two plain sentences, then the shortcut in brackets |
| **Status-bar hint** | *What can I do with my hands here?* | while hovering anything, including gizmos and empty viewport | `verb: action · verb: action`, lower case |
| **Inline note** | *Why does this look the way it does right now?* | always, beside or in place of a control | a few weak words |
| **Notice** | *What just happened?* | after an action, until dismissed | one sentence in its tone's colour |

### Tooltips

Say the effect in plain words, then the shortcut:

- ✅ "Turn this transform back on (Enter)"
- ✅ "Makes the far parts of the picture fade into the background. 0 is off."
- ❌ "Toggles `transform_enabled[i]`." (the code, not the effect)
- ❌ "Aerial-perspective transmittance falloff." (jargon first, Pip gone)
- ❌ "Haze" (the label again; it tells nobody anything)

For a disabled control, the tooltip says **how to make it available**:

- ✅ "Needs a zoom map. Pick one under Render → infinite zoom."
- ❌ "Unavailable."

Keep the implementation's reasons in code comments. Join sentences with full
stops, not dashes. Check every claim against the code: the 2026-09-26 text
pass found tooltips that were simply false.

### Status-bar hints

Gestures, not explanations. They're read in the corner of the eye mid-drag:

- ✅ `drag: rotate (shift: fine, ctrl: 15° steps) · alt+scroll: weight`
- ✅ `click: open the point count slider`
- ❌ `This edge rotates the transform about its third local axis.` (that's a
  tooltip)

Every hover gets *some* hint. Silence in the status bar reads as "nothing
here".

### Inline notes

Use one when the state of a control needs explaining *without* hovering,
because someone scanning the window would otherwise misread it:

- ✅ A greyed table row showing "needs Samples: accumulate" in place of its
  control.
- ✅ "repeats, not symmetric" on a symmetry badge, because the difference
  changes how you use it.
- ✅ "Session" as a heading over values that won't be saved.
- ✅ "Add a transform outside this group" beside the symmetry picker, when
  every transform is in the group (the picture would be wallpaper).
- ❌ A note repeating what the tooltip says, on every row.
- ❌ A paragraph of help text in a window. If it needs a paragraph, it needs
  a better control, or a section in CRAFT.md.
- ❌ A note that appears and disappears with state and pushes the form around
  (§1.7). Reserve its space, or put it in place of the control.

### Notices

For results and failures that happened *because of what you did*, in the
status bar, coloured by tone (`Tone::Note`, `Success`, `Error`), staying until
clicked or replaced. Never on a timer: a timer guesses how long you need to
read it. A failure says what failed and why, in words: "Couldn't open
scenes/koru.toml: " followed by the parser's own reason. When the result has a place to go
(a finished render), clicking the notice goes there.

No pop-up alerts, no toasts that fade, no sounds.

### Voice

Plain, British spelling in visible text ("colour"), "transform" not "map",
no `+` for "and", plain words over idioms ("show only this one", not "solo").
Name things the way the people in USERS.md would: effects for Pip, the
Apophysis term alongside ours for Rook where they differ.

---

## 8. Input: pointer, keyboard, modes

The full tables are in AGENTS.md ("Mouse Controls", "Keybinds"). The rules
behind them:

### Pointer

- **Left-drag on empty space orbits, from the first pixel.** There's no dead
  zone on the camera, because a dead zone on a constant gesture is felt as
  stickiness every time. Gizmo drags wait for a 4px threshold, because they
  write to the artwork.
- **You can grab the camera from anywhere**, in either mode, while anything
  is playing. Grabbing it detaches it from the timeline; the clock keeps
  running.
- **Scroll zooms, always.** A navigation gesture must never change the
  artwork. Alt+scroll is the one deliberate exception (a transform's weight,
  an xaos edge's multiplier).
- **Click on empty space deselects.**
- **Modifiers mean the same thing everywhere.** Shift = fine (a fifth of the
  movement, without a jump when pressed mid-drag). Ctrl = snap where there's
  something to snap to, uniform scale otherwise. Alt is kept as an alias but
  never relied on for a drag, because XFCE takes alt+drag to move windows.
- **The pointer shows the gesture:** a grab hand over a grabbable part,
  grabbing while orbiting, move for pan, horizontal-resize for roll.

### Keyboard

- **Every shortcut has a mouse path**, and both call the same `App` method.
- **Every shortcut appears in three places**: the Controls window (H / F1),
  the tooltip of the widget it triggers (in brackets), and the menu row's
  shortcut column. The Controls window rows are clickable.
- **Single letters for frequent actions on the scene or view; Shift reverses
  or does the opposite** (U / Shift+U, W / Shift+W, X / Shift+X). **Ctrl for
  document operations, with the conventional bindings** (Ctrl+S, Ctrl+O,
  Ctrl+N, Ctrl+Z, Ctrl+Shift+Z *and* Ctrl+Y, Ctrl+Q).
- **Esc cancels and never quits.** It works through: menu → dialog → browser
  → selection → nothing.
- **Hazel types Dvorak.** Choose letters for what they mean, not for where
  they sit on a QWERTY keyboard.
- **Text fields take the keyboard.** While a field has focus, single-letter
  shortcuts don't fire.
- Adding a shortcut takes a letter from everything after it. Check AGENTS.md's
  table and the Controls window before taking one, and add yours to both.

### Edit mode and view mode

| | View mode | Edit mode |
|---|---|---|
| For | looking, presenting, recording | building |
| Gizmos, labels | hidden | shown |
| Emphasis | off | on the selected transform (M) |
| Pointer | hides after 2 s still over the art | always shown |
| Xaos overlay | off | available |

The toolbar's Edit Mode / View Mode button, G and Tab all switch between them
(it's one flag, `App::show_gizmos`). Don't add a
third mode. A new tool that needs the viewport becomes part of edit mode, or
an overlay toggled within it.

---

## 9. Edits, history, documents and files

- **Every edit goes through `App::commit_edit`** with a label people can read
  ("Grid 4: rotate petal"). Edits with the same key within 1 s merge into one
  step, so a held key or a whole drag is one undo.
- **The dirty marker is a position in the history**, not a flag: undo back to
  the saved state and the `*` goes away.
- **Camera moves aren't history entries; camera paths are.**
- **The document is never the animated frame.** While a timeline plays, the
  scene being drawn is built separately; what the inspector edits and Ctrl+S
  writes is the authored scene.
- **Save keeps comments.** Scene files are edited in place with `toml_edit`;
  only changed values are rewritten.
- **New file kinds are TOML**, with every key optional, defaults at every
  level, and unknown keys reported rather than ignored. Built-ins are compiled
  in; the person's own go under `~/.config/fracturize/<kind>/`.
- **Opening a file goes through one door**: `App::request_load_scene`,
  whether from the browser, File → Open or a drop. Dropping a `.toml` or a
  `.png` with a scene in it opens it.
- **Never overwrite silently.** Screenshots get a `-N` suffix; Save as needs
  a tick to overwrite; Keep copies to a new file.

---

## 10. Performance, and sharing the machine

### Frame budget

- **120 FPS on the desktop, 60 on the T490**, at the default point count with
  the panels you'd normally have open. Measure with a **release** build; a
  debug build runs at a fraction of that and misleads. `tryout stage`
  refuses an unoptimised build.
- **Chrome costs little**: build and tessellate is `ui Xms` in the status
  bar. For a breakdown per panel, run with
  `FRACTURIZE_UI_PROFILE=1 RUST_LOG=info`. Don't do work per frame that only
  needs doing when something changes. Key caches off the history position.
  Rate-limit reallocations.
- **Point count and renderer are how the program scales**, and both are live.
  Give a new expensive feature a way to scale down too, and say its cost in
  the UI (the render job's estimate is the model).
- **The window only draws what it needs to**: the collapsed-attractor budget,
  the thumbnail budget (4 ms a frame), only the hovered thumbnail animating.

### Background work

- **Render jobs run on their own wgpu device** and the window stays at full
  frame rate while they run. One job at a time. Pause, Stop (which keeps the
  work), an estimate before starting, progress during, the result one click
  away after.
- **Threads: one less than the machine has**, by default, and set per machine
  (prefs and `--threads`), never per scene.
- **GPU share** for a render job is a per-machine setting. Agents running GPU
  benchmarks throttle to ≤80% duty with short submissions, one GPU job at a
  time. The GPU is shared with Hazel's desktop.
- **Slow work never blocks a frame**: random rolls and mutations run on worker
  threads; thumbnails are built within a time budget; anything slower than a
  frame belongs on a worker.

### Living alongside other things

- **Several Fracturize windows can run at once.** Prefs are written lazily
  and only when changed (`App::flush_dirty_prefs`), so dragging a window
  doesn't rewrite prefs.toml every frame. Staged builds get their own copy of prefs.
- **One agent bridge at a time.** A second `hello` is refused, naming the
  first. A socket is only taken over if nobody answers on it.
- **Local LLMs**: `with-llm --fits` checks VRAM before starting a model, and
  the window says why when it won't fit. A server the window started stops
  when the window closes.
- **Never take input from other windows.** Testing that drives the live app
  runs in Xephyr, never on `:0` (AGENTS.md, "Driving the running app").

---

## 11. Humans and agents as equals

- **What a person can do in the window, an agent can do from the CLI**, and
  the reverse. If you add a GUI control for scene data, it must also be
  reachable with `-S` (the parameter catalogue makes that automatic for most
  values), shown in `--info`, and saved to TOML. `ADDING-A-SCENE-FEATURE.md`
  is the checklist.
- **Agent-facing visuals are unmistakable.** Show it or don't, with labels,
  in amber on sheets. Subtle views (the emphasis glow) are for people with a
  mouse in their hand; agents get `--isolate`, `--map-grid` and labelled
  sheets. Mote's eyesight is the floor.
- **Collaboration has its own UI and is not a chat window bolted on**: option
  cards with Preview (a sandbox that never touches your history), Apply (a
  three-way merge, conflicts listed beside each other, yours kept by default),
  Send back, Keep. Proposals and requests are files on disk, so either side
  can take part from anywhere.
- **The person always has the last word on their scene.** An agent's change
  arrives as a proposal, never as a silent edit, and applying it is one undo
  step.

---

## 12. Known gaps

Things the code doesn't yet do that these guidelines say it should. Fix them
when you're nearby, or ask Hazel.

1. **A second blue.** The status bar's dimension lock uses its own
   `#64C8FF`, which reads as the selection colour but means "lock is on", and
   doesn't follow the highlight-colour setting. Either use the selection
   colour (it *is* a lit toggle) or a non-blue.
2. **Painted text uses literal sizes** (9–13 in timeline, labels, browser,
   mutate, xaos) rather than text styles. They don't follow the UI scale in
   the same way as the rest. Move them to `TextStyle` sizes, or derive them
   from one.
3. **Error red is pure `#FF0000`**, egui's default: harsh on grey 27. If it's
   changed, change it in `ui::theme` through the visuals, not at call sites.
4. **No grade controls in the GUI** (gamma, threshold, vibrancy are CLI and
   view-file only).
5. **The FPS figures above are targets**, not a regression test. Nothing
   measures the T490 automatically.
6. **Other pill-shaped widgets.** egui's own buttons and value boxes are
   rounded rectangles already, but check any new egui widget against §3
   "Shape and surface" before using it as-is.

---

## 13. Later

- **CLI guidelines.** The CLI seems more consistent already; collect its
  conventions (help layout, value names, `[bracket]` defaults, fixed-schema
  reports, printing the action rather than describing it, exit codes,
  colour opt-in, 78 columns) into a sibling document.
- A screenshot gallery of each widget in its states, for checking a change by
  eye.

---

## 14. Checklist for a UI change

Before you hand it over (`tryout stage --notes`, release build):

- [ ] Who it serves and who it costs, by persona name, in the notes.
- [ ] Every interactive widget goes through `hinted()`: tooltip in plain
      words with the shortcut, status hint in `verb: action` form.
- [ ] Every number is a `num::Drag` or a `num` readout with a fixed width.
      Drag a value through zero and through a power of ten: nothing beside
      it moves.
- [ ] Choose-one is a `radio`; destructive is a `danger_button`; tables are
      `ui::table`; windows come from `ui::window` with a minimum size.
- [ ] The selection colour only for chosen/selected, read from the visuals
      or `ui::theme`, never a literal (it's a setting: try a green one);
      filled on a control, tint + edge on a content row, never on a drawing
      surface. Status colours only from `ui::theme`; any new domain colours
      named beside their drawing code.
- [ ] Boxes with edges: rounded squares with visible borders, grouped
      controls drawn as one shape, no borderless pills (§3 "Shape and
      surface").
- [ ] Nothing animates. Nothing appears or disappears and shifts the layout.
      Unavailable means greyed with a reason.
- [ ] Every edit to the artwork is live and goes through `commit_edit`; one
      drag is one undo step; Ctrl+S writes it; it reloads.
- [ ] Scene data is reachable with `-S` and shows in `--info` (see
      ADDING-A-SCENE-FEATURE.md).
- [ ] Any new shortcut is in the Controls window, AGENTS.md's table and the
      tooltip.
- [ ] No new glyph that isn't in the fonts (check with `fc-list`).
- [ ] FPS and p99 checked in a release build with the new UI open.
- [ ] If it touches prefs, tested with an isolated `XDG_CONFIG_HOME`.
- [ ] Interface text: British spelling, "transform", full stops not dashes,
      effects not mechanisms. The wording is Hazel's call: say what you
      changed.
