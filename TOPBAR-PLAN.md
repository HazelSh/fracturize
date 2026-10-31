# The top bar: a plan

*Status: plan, 2026-10-30. Written by Claude Opus 5.5 from Hazel's brief
the same day, against `main` at 02669a1. Nothing here is built yet. Where
it changes a rule in `UI-GUIDELINES.md` or AGENTS.md it says so (§7).*

## 1. What's wrong now

The toolbar is one row trying to hold four kinds of thing: two menus, seven
window toggles, quick controls and the transport, plus the scene's name on
the right. It stopped fitting when Xaos, Collaborate and Timeline were added,
and the fix at the time (`toolbar_compact`) drops every window toggle's caption
together so only the icons are left. That's the wrong trade:

- **Seven bare icons can't be told apart** without hovering each one. Shapes,
  unite, graph, chats, video camera, film strip and sliders mean nothing on
  their own to Pip, and Rook shouldn't have to learn them. The guideline
  (§6.12) says an icon alone has to identify its window, and these don't.
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

The aim: every item on the bar has a caption, at the widths people actually
use (1440 on the desktop, 1280 on the T490), and every window opens from
where you'd be when you want it.

## 2. The new bar

```
 File  Explore │ Transforms │ Undo ↷ │ Splat · 6.0M ▾ │ Edit Mode 💡 ⇢ │ ⏺ ⏮ ⟲ ▶ Play  0:12.5 300f  Timeline │      Koru — Hazel *     │ Collaborate (2)  Help ▾
 └─ where a ──┘ └ what I'm ┘ └ undo ─┘ └ how it's ───┘ └ edit tools ──┘ └──────── time ──────────────────────┘ └ the document ──────┘ └ talk, and help ───────┘
   scene comes from  working on          drawn (popup)
```

From left to right:

| Group | Contents | Change |
|---|---|---|
| Where a scene comes from | **File**, **Explore** | File gains Subscenes and Views rows (§3) |
| What I'm working on | **Transforms** | the only window toggle left on the left |
| Undo | **Undo** with caption, **↷** redo with no caption | redo loses its word |
| How it's drawn | **`Splat · 6.0M ▾`**: one button opening one popup | replaces the renderer radio, the point count button and the Render toggle |
| Edit tools | Edit/View mode, emphasis, xaos overlay | unchanged |
| Time | ⏺ record, ⏮ to start, ⟲ loop, **▶ Play**, the time, **Timeline** | loop moves next to record; the Timeline toggle joins the transport |
| The document | **name — author `*`** | gets a reserved width (§4) |
| Talk, and help | **Collaborate** (with its count), **Help ▾** | Collaborate moves right; Help becomes a menu |

Every remaining item keeps its caption. The only captionless buttons are
redo, record, to start, loop and the two overlay toggles. Each of them sits
right next to something that explains it (Undo, Play, Edit Mode), and their
glyphs are standard.

**Width.** By estimate (14px text at about 7px a character, plus padding and
separators) the left part comes to about 1000px and the right about 370px.
So it fits at 1440 and doesn't quite fit at 1280. That's what the squeeze
ladder in §4 is for. Slice 1 measures the real widths and replaces these
numbers.

## 3. Where every window lives

Nothing loses its door. A window that leaves the bar opens from the place you
are when you want it, and also from a menu row, so Pip can find it by
browsing.

| Window | Opened from | Notes |
|---|---|---|
| **Transforms** | toolbar | unchanged |
| **Subscenes** | File → *Subscenes…*; File → *Import as subscene…*; a **Subscenes…** button beside the layer picker at the top of the Transforms rail | Composition is a document-level action, like Import in every editor, and the layer picker is where you notice a scene has layers |
| **Xaos** (the grid) | a **Whole grid…** button in the inspector's Xaos block; the xaos overlay's right-click menu | You go there from one map's row in the Xaos block. The overlay toggle stays on the bar |
| **Look** (was *Render*) | the **Look…** row at the foot of the `Splat · 6.0M ▾` popup | renamed, see below |
| **Timeline** | toolbar, in the transport group | moved |
| **Views** (what's left of *Camera*) | File → *Views…*; the views button by the axis cross (§5) | see §5 |
| **Collaborate** | toolbar, right, beside Help | moved; the count stays on it |
| **Agent settings** | Collaborate's header | unchanged |
| **Controls** (keys and mouse) | Help → *Controls*, H, F1 | keybind table only |
| **Settings** | File → *Settings…* | new: the preferences now in Controls |
| **About Fracturize** | Help → *About Fracturize* | new: logo, version, credits |
| Scenes, History, Mutate, Random flames, Random settings, Render job | File and Explore, as now | unchanged |

**"Render" becomes "Look".** The Render window holds point size, colour,
haze, background, the grade and infinite zoom: how the picture looks. It sits
next to *Render job…* (P) in the File menu, and the shared word suggests
they're the same thing at two sizes when they're not. Once the renderer
and point count move into the popup, what's left in that window really is
the look of the scene. *Appearance* would also work. *Look* is shorter and
it's the word people use for this ("the look").

### The `Splat · 6.0M ▾` popup

One button that shows the current state, and one popup that holds how the
picture is drawn:

```
 ┌ Renderer ─────────────────────────┐
 │ (•) points  ( ) splat  ( ) volume │   the existing radio (render_panel::render_mode)
 │ opacity  ━━━━━●━━━━  2.0          │   volume only; greyed under the other two
 ├ Points ───────────────────────────┤
 │ ━━━━━━━━●━━━━━━━━━━  6.0M          │   the existing log slider (render_panel::point_count)
 ├───────────────────────────────────┤
 │ Look…                             │   opens the Look window
 └───────────────────────────────────┘
```

- **These are the two settings that trade picture for speed** (Principle 5),
  and you reach for both when the machine starts to struggle. That's why they
  share a popup and stay one click away.
- **The radio stays a radio inside the popup**, because it's one-of-three
  with no off state. The bar shows only the current choice, since there's no
  room for three segments. When volume replaces the others, the renderer row
  goes, and the button just reads `6.0M ▾`.
- **The button's width is fixed** to its longest caption (`Volume · 999M`),
  so changing renderer or dragging the count doesn't move anything beside it.
- **Opacity moves here from the Look window.** It's a setting of the
  renderer, it's session state like the mode, and the Look window was
  already treating it as a special case.
- Both widgets are the panels' own functions, as the guideline says, so the
  rate limiting and hints are shared.

### The Help menu, split three ways

```
 Help ▾
   Controls             H
   About Fracturize
```

and, in File, just above Quit:

```
   Settings…
```

- **Controls** is the keybind table and mouse gestures, as now, without the
  about block or the settings above it. It's the window a stranger opens
  first, so it should just be a reference.
- **Settings** is a window for the person's preferences: orbit style, invert
  pitch, panel scale, field of view, the author name used for new scenes.
  Each is a preference by definition (it follows the person, not the scene),
  so nothing in it needs a class heading. The FoV slider moves here from the
  Camera window. Machine settings that belong to one dialog (Render job's GPU
  share and threads, Agent settings) stay in that dialog, because that's
  where they're read.
- **Why File and not Help for Settings:** Help is for finding things out, and
  Settings changes things. With no Edit menu, File is where desktop programs
  put Preferences/Options. A *Settings…* row in Help as well is cheap if
  Hazel wants both.
- **About** is a small window: the logo, the version line (`α-0.5 (commit,
  date)`, the same string as `--version`), what Fracturize is in a sentence,
  and credits (Phosphor's MIT licence, the fonts).

## 4. The document name, and what gives way first

The name is what tells you which document you're editing, so it must not be
what's left over.

- **It gets a reserved width**, about 180px: room for a 20-character name and
  a short author. A longer one is cut with `…` and shown whole in the
  tooltip. It's laid out before the left side takes the remaining space, not
  after.
- **The `*` always shows**, outside the cut text, so a long name can't hide
  that the scene is unsaved.
- **Clicking it still opens the name and author fields.**

When the window is too narrow, things give way **one at a time and always in
the same order**, each with hysteresis so a window edge sitting on a boundary
doesn't flicker (as `toolbar_compact` does now):

1. the time readout drops its frame count (`0:12.5`);
2. the author drops out of the name (still in the tooltip);
3. *Edit Mode* / *View Mode* drops to its icon;
4. *Timeline* drops to its icon;
5. *Collaborate* drops to its icon and its count;
6. the popup button drops the renderer name (`6.0M ▾`).

File, Explore, Transforms, Undo, Play and Help never lose their captions.
That's a much smaller set than the seven toggles that drop now, and every
step lands somewhere a person can still read. Below the last step, the right
block is allowed to overlap the window edge. It doesn't squeeze the left.

Two more rules follow from "the chrome doesn't move":

- each step's two forms are pinned to a fixed width, so moving between them
  is a jump in a known place, not a reflow;
- the Collaborate button is sized for a two-digit count, so a message
  arriving doesn't push Help along.

## 5. Dropping the Camera window

What's in the Camera window today and where each part goes:

| Part | Goes to |
|---|---|
| The camera path (keys, length, loop, play) | already in the Timeline's camera column; it just goes |
| Saved views (list, apply, rename, delete, save current) | the **Views** window |
| Distance and roll fields, the yaw · pitch · focus readout | the Views window's top block, *This view* |
| *level* | a button by the axis cross; also in *This view* |
| FoV | Settings (it's a preference) |
| Save as…, Render job… | already in File; drop any copies left here |

**The Views window** is small: *This view* (distance, roll, level, the
readout: what Theo wants written down), then *Saved views*: the list, click
to apply, rename, delete (armed), and *Save current* (V). It opens from File
→ Views…, since views are files under `views/` beside screenshots, and from
a views button by the axis cross.

**The axis cross becomes the camera's corner.** It already answers "which
way am I looking" and already takes clicks (look down an axis). Beside it go
two small buttons:

```
                    ⊕ level   ▤ views
                         (axis cross)
```

- **level** is greyed when the camera is already level, not hidden, with the
  tooltip saying so (Principle 13). It sits by the cross because a tilted
  horizon is something you notice by looking at the cross, so that's where
  your eye already is.
- **views** opens the Views window.

These go in the viewport rather than the status bar. The status bar holds
state and hints, not controls (§2 of the guidelines). The corner by the
cross is the one place in the viewport that already has chrome, so it costs
the picture nothing new. Like the cross, they're painted with a backing
that stays readable over any attractor.

## 6. Who this serves, and who it costs

- **Pip**: gains the most. Every item on the bar has a word on it. Every
  window can be found by opening a menu. Help is a menu with *Controls* in
  it, not a wall of keys, settings and a logo all at once.
- **Rook**: gains Xaos and Subscenes opening from where the work is (the
  Xaos block, the layer picker), and the drawing settings in one popup. Pays
  one extra click to reach the Look window, which used to be a toggle.
- **Jay**: gains the Timeline next to Play, and Views reachable from the
  corner while framing a shot. Pays the Camera window's habit.
- **Theo**: keeps the framing numbers, now in Views → This view. Nothing
  else changes.
- **Remy, Atlas, Mote**: unaffected. None of this reaches the CLI or the
  scene format. Collaborate keeps its count, now next to Help, where a
  person checks for messages.
- **Abe** would want Apophysis's full menu bar. We still don't give him one
  (see §7).

## 7. What changes in the guidelines

- **UI-GUIDELINES §2 "The toolbar"**: the table and the ASCII screen both
  change. The rule "File and Explore are the only menus" becomes: *File,
  Explore and Help are menus; the drawing popup is a popup.* The reason for
  having no menu bar still stands: Edit and View would still be near-empty
  copies of the bar. Help now has a real set of rows, and it's the one menu
  every program has.
- **"When space runs out, window toggles drop their captions"** is replaced
  by the ladder in §4.
- **§6.12 icons**: add that an icon may stand alone only next to something
  that explains it, or with a glyph everyone already knows (redo, record,
  loop).
- **AGENTS.md**: the toolbar line under "Human Interface" and the window
  table (Camera → Views, Render → Look, the new Settings and About), and the
  keybinds table (Help menu, H/F1 open Controls).

## 8. Slices

Each slice is a working build that can be staged with `tryout`, in this
order because each one frees the room the next needs:

1. **Measure and lay out the bar.** Measure the real caption widths at both
   reference widths and replace §2's estimate. Reserve width for the name.
   Swap `toolbar_compact` for the §4 ladder. Captionless redo. Moving
   Collaborate and Timeline. On its own this fixes the squashed name.
2. **The drawing popup**, with opacity moved into it. The Render window
   loses the renderer, opacity and point count, and is renamed Look.
3. **Help split**: Controls (keys only), Settings (new, with FoV from
   Camera), About.
4. **Camera → Views**: the Views window, level and views by the axis cross,
   the path list removed (the Timeline has it). Delete `camera_panel.rs`'s
   path code where the Timeline doesn't call it.
5. **Doors for Subscenes and Xaos**: File rows, the button by the layer
   picker, *Whole grid…* in the Xaos block. Remove their toolbar toggles last,
   once the new doors exist.
6. Docs: §7.

Prefs keep the open/closed state of every window. A window renamed in the
code keeps its `WindowKey`, or reads the old key once, so nobody's layout is
lost.

## 9. Further ideas, not in this plan

- **A command search** (Ctrl+K or F3, like Blender): type a few letters of
  any command or window and run it. Once some windows have no button on the
  bar, this is the fastest way to reach them for anyone who has learned the
  names, Rook especially. It would read from the same tables the menus and
  the Controls window are built from, so it can't get out of step with
  them.
- **A Window list** (which windows are open, close all, reset layout) in the
  Help menu, for when panels have been dragged off screen or forgotten.
  *Reset layout* restores `ui::default_layout`.
- **Explore keeps Undo, Redo and History**, which now also sit on the bar.
  That's fine: the menu rows are how you find History. But if Explore gets
  crowded, they're the first rows that could go.
- **When volume replaces the other renderers**, the popup shrinks to point
  count, opacity and Look…, and the button reads `6.0M ▾`. Nothing else in
  this plan depends on that happening.

## 10. Open questions for Hazel

1. **Look** or **Appearance** for the old Render window? Or keep "Render"?
2. **Settings in File** (recommended), in Help, or both?
3. **Level and views by the axis cross**, or would you rather they went
   somewhere else (status bar corner, Views window only)?
4. **Subscenes under File**: is Import-as-subscene the right reading, or
   does Subscenes feel more like part of Transforms (its layer picker
   only)?
