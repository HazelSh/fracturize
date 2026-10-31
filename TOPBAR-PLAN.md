# The top bar: a plan

*Status: built on branch `topbar` (slices 1-4), 2026-10-31, awaiting Hazel's
try-out. Revised the same day with her answers. First written 2026-10-30 by Claude Opus 5.5 against `main`
at 02669a1; revised against f7f3bd9. Where it changes a rule in
`UI-GUIDELINES.md` or AGENTS.md it says so (§7).*

## 1. What's wrong now

The toolbar is one row trying to hold four kinds of thing: two menus, seven
window toggles, quick controls and the transport, plus the scene's name on
the right. It stopped fitting when Xaos, Collaborate and Timeline were added,
and the fix at the time (`toolbar_compact`) drops every window toggle's caption
together so only the icons are left. That's the wrong trade:

- **Seven bare icons can't be told apart** without hovering each one. Shapes,
  unite, graph, chats, video camera, film strip and sliders mean nothing on
  their own to Pip, and Rook shouldn't have to learn them.
- **The scene's name and author get squeezed** to almost nothing, because the
  right-hand block is laid out last into whatever space is left. The one
  piece of identity on screen is what gets cut.
- **Several toggles don't earn a place on the bar.** Subscenes and the Xaos
  grid are used now and then, from a particular point in a particular task.
  Camera has been mostly emptied out by the Timeline. Collaborate is a
  conversation, not a working panel.
- **The Controls window (Help) does three jobs**: about, settings and the
  keybind table.
- **Timeline is away from the transport**, though the transport is how you
  use it.

The aim: every item on the bar has a caption at the widths people actually
use (1440 on the desktop, 1280 on the T490), and every window opens from
where you'd be when you want it.

## 2. The new bar

```
 File  Explore │ Transforms │ Undo ↷ │ Splat · 6.0M ▾ │ Edit Mode 💡 ⇢  (•)□ ( )⊞ ( )⊟ │ ⏮ ▶ Play ⏺ ⟲  12.50 s 375 f  Timeline │   *Koru — Hazel │ ¹Collaborate  Help ▾
```

| Group | Contents | Change |
|---|---|---|
| Where a scene comes from | **File**, **Explore** | File gains Subscenes, Import as subscene, Views and Settings rows (§3) |
| What I'm working on | **Transforms** | the only window toggle left on the left |
| Undo | **Undo** with caption, **↷** redo with no caption | redo loses its word |
| How it's drawn | **`Splat · 6.0M ▾`**: one button opening one popup | replaces the renderer radio, the point count button and the Render toggle |
| Edit tools | **Edit Mode**, then emphasis and the xaos overlay *to its right* | they used to sit on its left; Edit Mode is the control they depend on, so it comes first |
| Time | ⏺ record, ⏮ to start, ⟲ loop, **▶ Play**, the time, **Timeline** | loop moves next to record; the Timeline toggle joins the transport |
| The document | `*` **name — author** | measured width, laid out before the rest gives way (§4) |
| Talk, and help | **Collaborate** (with its count), **Help ▾** | Collaborate moves right; Help becomes a menu |

Every remaining item keeps its caption. The only captionless buttons are
redo, record, to start, loop and the two overlay toggles. Each sits right
next to something that explains it (Undo, Play, Edit Mode), and their glyphs
are standard.

**Widths are measured, never written down in pixels.** Every fixed width on
the bar is the measured width of the widest text that control can show
(`num::icon_button_width`, `num::text_button_width`, `num::mono_width`), so
it follows the font and the panel scale setting. The squeeze ladder (§4)
works from last frame's measured widths, not from estimates.

## 3. Where every window lives

Nothing loses its door. A window that leaves the bar opens from the place you
are when you want it, and usually from a menu row too.

| Window | Opened from | Notes |
|---|---|---|
| **Transforms** | toolbar | unchanged |
| **Subscenes** | File → *Subscenes…*; File → *Import as subscene ▸*; a button beside the layer picker at the top of the Transforms rail (composites only, since that's when the picker shows) | Import is a document-level action, as in every editor |
| **Xaos** (the grid) | a **Whole grid…** button in the inspector's Xaos block; the xaos edge's right-click menu | no menu row, on purpose (Hazel, 2026-10-31): you reach it from one map's xaos |
| **Look** (was *Render*) | the **Look…** row at the foot of the drawing popup | renamed. *Look* is a working name: Hazel finds it ambiguous and it may change again |
| **Timeline** | toolbar, in the transport group | moved |
| **Views** (what's left of *Camera*) | File → *Views…* | see §5 |
| **Collaborate** | toolbar, right, beside Help | moved; the count stays on it |
| **Agent settings** | Collaborate's header | unchanged |
| **Controls** (keys and mouse) | Help → *Controls*, H, F1 | keybind table only |
| **Settings** | File → *Settings…* | new: the preferences now in Controls and Camera |
| **About Fracturize** | Help → *About Fracturize* | new: what was the top of Controls |
| Scenes, History, Mutate, Random flames, Random settings, Render job | File and Explore, as now | unchanged |

### The drawing popup: `Splat · 6.0M ▾`

One button that shows the current state, and one popup holding how the
picture is drawn:

```
 ┌ Session ──────────────────────────┐
 │ (•) points  ( ) splat  ( ) volume │   render_panel::render_mode
 │ opacity  ━━━━━●━━━━  2.0          │   greyed unless volume
 ├ Preference ───────────────────────┤
 │ points   ━━━━━━━●━━━━━  6.0M       │   render_panel::point_count
 ├───────────────────────────────────┤
 │ Look…                             │   opens the Look window
 └───────────────────────────────────┘
```

- **Renderer and point count trade picture for speed**, and you reach for
  both when the machine starts to struggle.
- **Point size and exposure stay in the Look window**, which the popup's
  *Look…* row opens (Hazel, 2026-10-31: they're "our look"). An earlier
  revision of this plan put them in the popup by misreading her.
- **Headings are the persistence classes**, the same words the Look window
  uses, so the popup says what will be in your file tomorrow.
- **The radio stays a radio inside the popup**: it's one-of-three with no off
  state. The bar shows only the current choice.
- **The button's width is pinned** to its widest caption, so changing renderer
  or dragging the count doesn't move anything beside it.
- **Opacity moves here from the Look window.** It's a setting of the renderer,
  and session state like the mode.
- Every widget is the panels' own function, so rate limiting and hints are
  shared.

### What's left in the Look window

Point size and exposure first, then colour (falloff, the gradient and
contrast), background and transparency, infinite zoom, and the grade. **Haze is sidelined** to an icon button on the
background row that opens the haze slider and band in a popup (Hazel,
2026-10-31: it has never quite done what she wants visually, and can step
back for a while). The button is lit while haze is on, so a scene with haze
says so without being opened. F / Shift+F still work.

### The Help menu, split three ways

```
 Help ▾
   Controls             H
   About Fracturize
```

and, in File, just above Quit, **Settings…**:

- **Controls** is the keybind table and mouse gestures, as now, without the
  about block or the settings above it.
- **Settings** holds the person's preferences: orbit style, invert pitch,
  field of view, panel scale. Each follows the person, not the scene, so it
  needs no class heading. Machine settings that belong to one dialog (Render
  job's GPU share and threads, Agent settings) stay in that dialog.
- **Why File for Settings:** Help is for finding things out, and Settings
  changes things. With no Edit menu, File is where desktop programs put
  Preferences.
- **About** is a small window: logo slot, version line, what Fracturize is,
  the repository link and the pre-release warning.

## 4. The document name, and what gives way first

The name is what tells you which document you're editing, so it must not be
what's left over.

- **It gets a capped width**: room for about 28 characters of the button
  font, measured. A longer one is cut with `…` and shown whole in the
  tooltip.
- **The `*` always shows**, in its own fixed cell outside the cut text, so a
  long name can't hide that the scene is unsaved, and the bar doesn't shift
  when it appears.
- **Clicking it still opens the name and author fields.**

When the window is too narrow, things give way **one at a time and always in
the same order**:

1. the time readout drops its frame count;
2. the author drops out of the name (still in the tooltip);
3. *Edit Mode* / *View Mode* drops to its icon;
4. *Timeline* drops to its icon;
5. *Collaborate* drops to its icon and its count;
6. the drawing button drops the renderer name (`6.0M ▾`).

File, Explore, Transforms, Undo, Play and Help never lose their captions.
Below the last step, the right block is pushed past the window edge rather
than squeezing the left.

**How the step is chosen.** Each step's two forms are pinned to fixed
widths, so each step saves a known, measured amount. Last frame's measured
width of the whole bar, plus or minus those savings, says exactly which step
fits this frame; there's no stepping one at a time across frames. Going back
to a roomier step needs a margin of spare width, so a window edge sitting on
a boundary doesn't flicker.

## 5. Dropping the Camera window

| Part | Goes to |
|---|---|
| The camera path (play, length, loop, keys) | already in the Timeline's camera column |
| *Reset* to the default orbit | the Timeline's camera column (it was only in Camera) |
| *+ Add key* | Y, and the Timeline's *Key the camera at the playhead* |
| Saved views (list, apply, rename, save current) | the **Views** window |
| Distance and roll fields, the yaw · pitch · focus readout, *level* | the Views window's top block, *This view* |
| FoV | Settings (it's a preference) |

**The Views window** is small: *This view* (distance, roll, level, the
readout: what Theo wants written down), then *Saved views*: the list, click
to apply, right-click to rename, and *Save current* (V). It opens from File →
Views…, since views are files under `views/` beside screenshots.

**Level goes by the axis cross** (Hazel asked for this, and it's the only
thing that goes there). A tilted horizon is something you notice by looking
at the cross, so that's where your eye already is. It is greyed when the
camera is already level, not hidden, with the tooltip saying so. Like the
cross, it is shown only in edit mode.

## 6. Who this serves, and who it costs

- **Pip**: gains the most. Every item on the bar has a word on it, and Help
  is a menu with *Controls* in it, not a wall of keys, settings and a logo.
- **Rook**: gains Xaos and Subscenes opening from where the work is, and
  renderer and points in one popup. Pays one extra click to reach the Look
  window (point size, exposure, colour, zoom), and two for haze.
- **Jay**: gains the Timeline next to Play. Pays the Camera window's habit;
  the path was already on the Timeline.
- **Theo**: keeps the framing numbers, now in Views → This view.
- **Remy, Atlas, Mote**: unaffected. None of this reaches the CLI or the
  scene format.
- **Abe** would want Apophysis's full menu bar. We still don't give him one.

## 7. What changes in the guidelines

- **UI-GUIDELINES §2 "The toolbar"**: the table and the ASCII screen change.
  "File and Explore are the only menus" becomes: *File, Explore and Help are
  menus; the drawing button opens a popup.* There's still no menu bar,
  because Edit and View would be near-empty copies of the bar.
- **"When space runs out, window toggles drop their captions"** is replaced by
  the ladder in §4.
- **§6.12 icons**: an icon may stand alone only next to something that
  explains it, or with a glyph everyone already knows (redo, record, loop).
- **AGENTS.md**: the toolbar line, the window table (Camera → Views, Render →
  Look, Settings and About), and the mention of the Camera window's level
  button.

## 8. Slices

Resliced 2026-10-31 so the new ways in exist before anything leaves the bar,
and the bar is rebuilt once, when everything that is leaving has somewhere
to go. Each is a working build.

1. **New doors and windows.** Help becomes a menu (Controls, About). The
   Settings and About windows. Camera → Views, with level by the axis cross
   and Reset moved to the Timeline. File rows for Subscenes, Import as
   subscene, Views and Settings. *Whole grid…* in the Xaos block and edge
   menu, the Subscenes button by the layer picker. The old toolbar toggles
   are still there.
2. **The drawing popup and the Look window.** Renderer, opacity and points in
   the popup; Render renamed Look; haze to an icon button.
3. **The bar.** The Subscenes, Xaos, Camera and Render toggles come off;
   Timeline joins the transport; Collaborate moves right; redo loses its
   word; Edit Mode before its companions; the name's capped width and `*`
   cell; the §4 ladder replaces `toolbar_compact`.
4. **Docs** (§7).

Prefs keep the open state of every window. Renamed open flags read their old
key (`serde(alias)`), and the Look window keeps the Render window's geometry
key, so nobody's layout is lost.

## 9. Further ideas, not in this plan

- **A command search** (Ctrl+K or F3, like Blender): type a few letters of any
  command or window and run it. It would read from the same tables the menus
  and the Controls window are built from.
- **A Window list** (which windows are open, close all, reset layout) in the
  Help menu, for panels dragged off screen or forgotten.
- **A better name than "Look"** for the Look window.
- **When volume replaces the other renderers**, the popup loses its radio and
  the button reads `6.0M ▾`.

## 10. Round 2: after the first try-out (2026-11-01)

Hazel's verdict on the first build was *needs work*. What she asked, whether
each ask holds up, and what changed:

- **Transport order: to start, Play, record, loop.** Sound: it is the order
  of Logic and Reaper (to start, play, record, cycle), so a DAW hand finds
  Play where it expects. Done. The plan's earlier "loop next to record" still
  holds.
- **The time as a click-drag control.** Sound: Blender's current-frame field
  and Logic's position display both scrub on drag, and finding a moment by
  eye is a relative act. It is a `num::Drag` now: drag moves the playhead a
  frame per two points of travel, click types `12.5`, `300f` or `1:02.5` as
  before. It drags while playing too, which scrubs.
- **Toggles must not move anything.** Sound, and a house rule that was being
  broken. The cause: every pinned button puts `Atom::grow()` after its
  caption, egui adds a gap before that atom, and `icon_button_width` didn't
  count it, so the widest state (Pause, View Mode) ran 4 points past its pin.
  The helper she remembered was `num::icon_button_width`; it measured
  correctly, and the call sites built the button around it wrongly. Fixed
  once, in `num::Pinned`, which builds the button and its width together;
  every changing-caption button uses it, and a test draws every state.
  This also matters to the ladder: with 16 points to spare at 1440, a 4-point
  wobble on play/pause could tip a step and move the whole bar.
- **Collaborate far wider than its caption.** Sound. It was pinned to
  "Collaborate (99)". The count is now a warn-orange badge on its icon (the
  colour for "waiting on you", with the number in the tooltip in words), so
  the button fits "Collaborate" and a message arriving moves nothing.

Two buttons from other branches have places: **multiview's layout radio**
(on `main`) sits after the edit tools with ordinary spacing, its own control
and not joined to them; **warp-view's grid toggle** (not merged yet) goes
after the xaos overlay as Edit Mode's third companion, and the code marks the
spot. With the radio added, 1440 lands on step 2 (no frame count, no author).

### Review against the personas, other tools and Apophysis

Apophysis AV, run for comparison: a full menu bar over one strip of ~24
icon-only buttons (file, random/mutate, undo/redo, the editor windows, a
render-density box, play/stop for scripts, pan/rotate/zoom tools); the
flame's name is in the status bar and the title bar, not the toolbar. Our bar
keeps what Rook will look for (window doors one click away, density mid-bar)
and gives Pip the words Apophysis never had.

Things found that are **not** fixed here, each Hazel's call:

1. **Two buttons say "Play" and do different things.** The toolbar's flies
   the camera (attach and play); the Timeline window's runs the clock and
   leaves the camera alone. Jay will press one expecting the other. Options:
   call the Timeline's "Play tracks", or drop it and let the camera row's
   attach link carry the difference.
2. **The name duplicates the title bar.** It is what gives way second, and
   it costs 150–250 points. Apophysis keeps it in the status bar. Moving it
   there (or to the title only) would give 1440 back every caption with the
   layout radio in place.
3. **Help goes off the edge below ~900 wide.** A seventh step that hides the
   name entirely before Help moves would keep Help on screen to about 750.
4. **The layout radio is the widest thing per glyph on the bar** (about 125
   points for three icons) because the house radio draws a ring-and-dot per
   segment. That rule is right in a panel; on a toolbar of icon segments a
   lit segment already says "chosen". A toolbar-only segmented style would
   halve it. It's the multiview agent's control and the house radio's rule,
   so it's flagged rather than changed.

## 11. Round 3: Hazel's answers (2026-11-01)

- **One Play, and it plays the scene.** Playing runs the scene's clock; an
  attached camera moves with it, along its track or the default orbit when it
  has none. Detaching is a separate, lasting choice. The Timeline window's
  own Play and the camera column's Play/Stop are gone; the toolbar's Play is
  the only one, lit in the selection colour while playing. (The model was
  investigated and fixed in `App` by a subagent; see the commit.)
- **The view layout** (one, four, one big and three small) left the toolbar
  for the status bar's far right: one button showing the current layout,
  opening a menu upward. Radios cost too much width for something chosen now
  and then.
- **Level** left the axis cross, where it hung in space, for an icon beside
  the layout button. That corner, next to the point count and frame rate, is
  where small view and camera controls collect.
- **Look** is back on the bar, beside the drawing button; the popup's Look…
  row went.
- **Edit Mode / View Mode always shows its word.** It situates the two
  overlay buttons beside it and is one of the key controls, so it came off
  the ladder, which is now five steps: frame count, author, Timeline's
  word, Collaborate's word, the renderer's name.
