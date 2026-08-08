# Plan: what the app is like for a stranger, and what to move

*Claude Opus 5, 2026-08-06. A review of the GUI as it stands at `4adca2d`,
read from source — every claim below cites the line it came from. Nothing
here is implemented; §11 is the worklist.*

> **Method.** Task-centred, in the Lewis & Rieman sense: name real tasks in
> the user's own words, walk each one through the interface that exists, and
> count the steps, the guesses, and the places where the user has to already
> know the answer. Conventions are judged against what a person arriving from
> Apophysis / Apophysis AV or from Blender will already have in their fingers.
> I did not launch the app — this is a code review of an interface, and the
> things it finds are structural rather than cosmetic. A live session (§11,
> last item) would add the cosmetic layer.

---

## 0. The headline

The craft in this UI is unusually high and mostly in the right places. Three
things in particular are better than the programs it will be compared to: the
**hint system** (every widget carries a tooltip *and* a status-bar line, and
disabled controls still speak — `ui/hints.rs:38-46`), the **segmented radio**
(`ui/radio.rs`, one-of-n drawn as one control instead of n toggles), and the
**live logarithmic point-count slider** (`ui/render_panel.rs:131`), which lets
you feel the machine load up under your hand instead of committing blind to a
number. Those are the answers to questions most tools get wrong.

**The organising idea, arrived at late but load-bearing throughout: a scene is
a document.** A unit you open, edit, save and save-as. It is the one model
every user already has, and most of what follows is a consequence of committing
to it — where file operations belong (§7.1, §9), what counts as an undoable
edit (§6.9), and which controls are part of the document at all (§10.1). The
places this UI confuses people are, almost without exception, the places that
question has no visible answer.

What's wrong is almost entirely **shelving and safety**, not drawing:

1. There is no dirty state and no save prompt, so unsaved edits are discarded
   without being mentioned — and **Escape quits** (`main.rs:989-1002`), while
   `CloseRequested` exits unconditionally (`:945`) and B → Enter replaces the
   scene outright. Two keystrokes lose an hour of work.
2. File operations live at the bottom of the **Camera** window
   (`ui/camera_panel.rs:549`), which is not where anybody will look for them.
3. **Up/Down means three different things** depending on state you can't see,
   and after any scene load it is permanently stuck on one of them, because
   `select_transform(None)` is never called anywhere in the codebase — there
   is no way to deselect.
4. The window title is a frame-rate readout (`app.rs:3703`) rather than the
   document name, so the taskbar can't tell you which scene you have open.
5. **Numeric readouts change width as their values change**, so rows of them
   re-lay-out every frame and physically vibrate. Invisible in screenshots;
   very much not invisible to a person. (§6.7)
6. **The one destructive-action guard that exists is defeated by a
   double-click** — render-cancel arms on click 1 and fires on click 2 with no
   minimum delay (`app.rs:86-100`). (§6.8)

None of those are hard. Items 1, 3 and 6 are the ones that lose work; item 5
is the one that makes the app feel unwell without anyone being able to say
why.

---

## 1. The shape of the thing

A full-surface 3D viewport, never shrunk by chrome. Over it: a thin top
toolbar of icon+label toggles, up to six floating `egui::Window` panels whose
geometry persists, and an Inkscape-style bottom status bar — hover hints on
the left, a performance instrument (FPS · mean · p99 · sparkline · point
stats) on the right. Gizmos draw into the viewport as tetrahedral cells with
world-anchored name labels.

This is the right macro-shape, and it is a *modern* one. Apophysis scatters
its Editor / Adjust / Gradient / Mutation across separate OS-level windows
that get lost behind the main one; Fracturize keeps everything in one surface
with the viewport always whole. Keep that.

**First run**, with no prefs file: gizmos on (`app.rs:755`), help off
(`:757`), browser off (`:801`), and all four toggle panels closed
(`prefs.rs:16-25`, all `bool` defaults). So a stranger's first screen is a
fractal, a row of seven buttons, and a status bar reading `drag: orbit ·
shift/middle-drag: pan · right-drag: roll · scroll: zoom`.

That's a defensible cold open — the status line is doing real work — but the
one window that explains the whole app (Keybinds, 47 rows, clickable) is off,
and unlike the other four its open state doesn't persist at all: `show_help`
and `show_browser` live on `App` rather than in `PanelPrefs`. **Recommend:
default Transforms + Explore open on a fresh prefs file, and move `show_help`
/ `show_browser` into `PanelPrefs` so all six behave alike.**

---

## 2. Eight tasks, walked

Named the way a person would say them. Step counts assume you already know
where things are; the "guesses" column is what you'd have to *discover*.

### T1 — "Show me something I didn't make"
Toolbar → Explore → **Random flame**. Two clicks, quality-checked, one Ctrl+Z
away from what you had. **This is excellent** and the tooltip says all three
of those things. No complaints.

### T2 — "Now make it mine"
Mutate + strength slider + undo/redo + a clickable history list that jumps N
steps in one rebuild (`ui/explore.rs`). Solid. The gap is that **mutation is
blind**: you press U, the picture changes, you judge, you undo. Apophysis's
signature exploration UI — the 3×3 grid of mutation thumbnails around the
current flame — shows you nine consequences *before* you commit to one. It is
the single biggest feature difference in exploration workflow between the two
programs, and it is already on `todo.txt`. See §3.

### T3 — "Push that arm out a bit"
Hover a gizmo (it glows, grows, cursor becomes a grab hand), drag its origin
dot; the fractal re-forms live; release commits **one** history entry
(`app.rs:1924-1941`). This is the best path in the app and it's better than
Apophysis's 2D triangle editor because it's honestly 3D. No complaints.

### T4 — "Make it that colour"
Transforms → swatch, or the gradient strip with its second "what the fractal
actually indexes" bar (`ui/gradient.rs:9-14`). The stretched-strip idea — show
what contrast is *doing* to your palette, directly under the control that did
it — is a genuinely good invention. Keep.

### T5 — "Get an angle I like and keep it"
Here the vocabulary starts costing. There are three persistent framings with
similar names: the **scene's camera** (written by Ctrl+S), a **saved view**
(V → `views/`), and a **path keypoint** (Y). A newcomer cannot tell from the
names which one they want, and two of them live in the Camera window under
headings ("Saved views", "Camera path") that don't distinguish *permanence*
from *purpose*. Not a bug; a naming pass. Suggest the panel say what each is
*for* in one line: framing = where the scene opens, view = a bookmark,
keypoint = a frame of the shot.

### T6 — "Make it zoom forever" ← the worst path, and the flagship feature
1. Transforms window → select a transform → **"Zoom about this"**
   (`ui/transforms.rs:523`, also in the right-click menu at `:389`).
   You must already know *which* transform, and what the sentence means.
2. Camera window → Loop → **zoom** segment (greyed until step 1 is done —
   correctly, with a tooltip saying how to un-grey it).
3. Render window → **infinite zoom** disclosure (collapsed) → **edge guard**.
4. Camera → Render job… → Animation.

Four windows, three of them needing you to know the vocabulary before the
control will mean anything. Two specific fixable things:

- **The app already knows which transforms qualify.** `zoom_action()`
  (`ui/transforms.rs:421`) runs `Renorm::build` per transform and produces
  both an enabled flag and a sentence explaining the failure. So step 1 does
  not have to be a guess: the Render window's infinite-zoom section should
  carry a **map picker** listing every transform, qualifying ones selectable
  and the rest greyed with their reason on hover. That turns "know the theory"
  into "read the list", using code that already exists.
- **`render_panel.rs:351` gives stale directions**: it says *"Transforms
  window → right-click a map → Zoom about this"*, but the same command has
  been a plain visible button in the detail pane since the action-row change
  (`transforms.rs:474-483` explains why it was added). Pointing a newcomer at
  the right-click is pointing them at the harder path.

- **`edge_guard` is two disclosures deep and closed by default**
  (`render_panel.rs:340, 371`). It is the headline control of the most recent
  plan. Once a scene *has* a zoom map, that section should be open.
  `todo.txt` already says "no hidden bits in render … collapsable infzoom
  bits", so this matches Hazel's instinct.

### T7 — "Save my work"
Ctrl+S, or the Camera window's bottom row (`ui/camera_panel.rs:549`), which
holds **Render job… / Screenshot / Save scene / Save as…**. Three of those
four have nothing to do with the camera. `todo.txt` has already spotted it
("break out save, save as, etc into scene browser, rename to files/open?").
Agreed: they belong in a **File** menu (§7.1), with the Scenes window as what
`Open…` shows you (§9, Move 1).

### T8 — "Render a 4K still"
P, or Camera → Render job…. The dialog asks what you want, estimates the cost
*before* you agree, shows progress, and takes two clicks to abandon
(`ui/render_job.rs:1-10`). This is better than Apophysis's render dialog and
better than most. No complaints.

---

## 3. What Apophysis / Apophysis AV users bring

**Expectations that are already met.** Negative variation weights (kept, with
a row that stays put at zero so a drag can pass through — `transforms.rs:1060`);
per-transform weights as the emphasis lever; a gradient editor; a batch render
with quality settings; direct manipulation of the transforms themselves.

**Expectations that are met better.** Fracturize shows only the variations
that carry weight plus a combo to add more, rather than Apophysis's full list
of every variation with a mostly-zero column. At ~20 variations that's the
right call and it scales. The render dialog's estimates and pause/stop are a
straight improvement. One window instead of five is a straight improvement.

**The one big thing that's missing: the mutation grid.** Apophysis's Mutation
window shows a 3×3 of thumbnails — the current flame in the centre, eight
perturbations around it — and clicking one recentres the grid on it. It turns
mutation from *roll, judge, undo* into *look, choose*. Fracturize has all the
machinery: `mutate.rs` generates the perturbation, `render_job.rs` /
`offline.rs` can already render offscreen at arbitrary point counts. Eight
low-point-count thumbnails at ~200×200 is cheap next to a 50M-point live view.
This is the highest-value single addition in the document and it's already on
`todo.txt`.

**One collision to fix.** Apophysis is a Windows app, so **Ctrl+Y is Redo**
there. In Fracturize, Ctrl+Y toggles the camera path loop (`main.rs:1091`) —
which routes through `set_path_loop`, one of the five path operations that
aren't history entries (§6.9), so Ctrl+Z won't undo what just happened. An Apophysis refugee reaching for redo silently changes their
shot and cannot take it back. Recommend: **Ctrl+Y becomes an alias for Redo**;
the loop toggle is already fully served by the Camera window's four-way radio,
which is a strictly better control than a keystroke that can only reach one of
the four states.

**One thing not to copy.** Apophysis's modal-dialog habit. Fracturize's Save
As is already non-modal (`ui/save_as.rs:84-90`) — though see §6.5, because
non-modal-but-looks-modal is its own problem.

---

## 4. What Blender (and Maya / Fusion / Figma) users bring

**Met.** Single-key modeless shortcuts over a viewport. Drag-to-change /
click-to-type numeric fields. Global undo with a visible list. Gizmo parts
that constrain the axis by *which part you grab*, which is the Maya/Unity
convention and correct for a 3D scene. A status bar that says what the mouse
buttons do right now.

**Not met, and worth meeting:**

- **No orientation indicator.** Blender, Fusion, Onshape and everything after
  them put a clickable axis gizmo in a viewport corner. Fracturize has nothing
  — and it needs one *more* than they do, because the default trackball orbit
  deliberately accumulates roll (`CAMERA-FEEL-PLAN.md` §2, and the price is
  stated honestly in the Camera panel's own tooltip). The only thing on screen
  that says which way is up is a numeric readout in a panel that is closed by
  default. A corner axis widget pays directly for the trackball decision and
  makes "level" discoverable as a gesture rather than a button in a panel.
- **No axis-aligned views.** Blender's numpad 1/3/7. For an IFS these aren't a
  nicety — authoring rotational symmetry is much easier when you can get
  exactly down an axis. Cheap: make the corner axis widget clickable and you
  get both features from one control.
- **No "frame selected".** Blender's numpad-`.`, and everyone's Home key.
  "Frame all" is ill-defined for an object with no largest feature, but
  **"put the camera on the selected transform's fixed point"** is exactly
  defined and uses maths `renorm.rs` already has. High value, especially at
  depth.
- **No smoothing on camera jumps.** Loading a saved view (`camera_panel.rs:222`)
  and pressing `level` (`:157`) both teleport. Blender smooths view changes
  over ~200ms by default and it is the single most-noticed "this feels
  expensive" detail in a 3D app. Cheap to add, large feel return.
- **Shift is unused during gizmo drags.** In every drawing and modelling tool
  in the world Shift-during-drag means *precision / fine*. `try_grab_gizmo`
  never consults it, so it's free. Take it.
- **No numeric entry mid-drag** (Blender: `G`, then type `2.5`, Enter). This
  is a deep Blender idiom, and the fallback here — drag roughly, then type
  exactly into the inspector — is acceptable. Defer.
- **No multi-select.** One transform at a time. Moving three arms together
  isn't possible. Real gap for the modelling workflow; deliberately deferred
  is fine, but it should be a stated deferral rather than an absence.

**A fork in the road, correctly taken.** Blender users expect left-drag to
*select* and middle-drag to *navigate*. Fracturize gives left-drag to orbit,
because there is nothing to box-select in a point cloud. That's the right
call — it matches Apophysis and every casual 3D viewer — but it has a
consequence the app hasn't followed through on: see §6.3.

---

## 5. Conventions already kept (protect these through any restructure)

- **Every interactive widget goes through `hinted()`** — tooltip *and* status
  hint, with the disabled path handled explicitly because egui silently drops
  `on_hover_text` on greyed widgets (`hints.rs:42-46`). That bug is subtle and
  it's already fixed; don't regress it.
- **Disabled, not hidden — with the tooltip saying how to un-disable.** The
  zoom-loop segment, the delete-last-transform button, the roll field at a
  pole. This is a top-decile convention that most apps get backwards.
- **One-of-n is a radio, not n toggles** (`ui/radio.rs`).
- **Keyboard and mouse funnel through the same `App` methods**, so they can't
  drift. The Keybinds window's rows *execute* their binding, which quietly
  makes it a command palette.
- **A whole drag, or a held key, is one undo step** (`commit_edit` coalescing).
- **Panel geometry persists, with deferred writes** so dragging a window
  doesn't rewrite prefs every frame.
- **Window minimum sizes are load-bearing**, not cosmetic — the writeup at
  `ui/mod.rs:160-183` documents a genuinely nasty egui failure mode. Keep the
  comment with the code.
- **Symbols are drawn, not typed**, because the font stack has no `◉` or `✕`
  and tofu in the middle of a control is worse than a slightly larger diff.

---

## 6. Nine convention breaks that should be fixed

Ordered by what they cost the user.

### 6.1 Escape quits, and nothing tracks unsaved work
`main.rs:989-1002` — Escape dismisses the transform menu, then Save-As, then
the browser, then **exits the event loop**. `CloseRequested` exits
unconditionally (`:945`). There is no scene dirty flag anywhere in the
codebase (the only `dirty` is `prefs_dirty_since`). And `load_scene_file`
calls `self.history.clear()` (`app.rs:2198`), so B → Enter also throws the
work away, without asking.

In every desktop program written this century Escape means *cancel the thing
in front of me*. Making it the quit key means the reflex that closes a popup
closes the app. Recommend, in order:

1. Add a `Scene::is_dirty` — cheapest honest version is a "history is
   non-empty since the last save" flag flipped by `commit_edit` and cleared by
   `save_scene` / `save_scene_as` / `load_scene_file`.
2. **Escape stops quitting.** It cancels: menu → dialog → browser →
   *deselect the transform* (see 6.3) → nothing. Quit becomes Ctrl+Q and the
   window close button.
3. Both quit paths, and `load_scene_file`, check dirty and put up a
   three-button *Save / Discard / Cancel*. This is the one place a genuinely
   modal dialog is correct.

### 6.2 The window title is a debug HUD
`app.rs:3703-3710` sets `Fracturize | 60 FPS | 16.6ms | 500k points`. All of
that is already in the status bar, with more detail and a sparkline. Meanwhile
the title — the thing the taskbar, the alt-tab list and the window switcher
show — cannot tell you which scene you have open. Convention is
`document — application`, with a marker when dirty. Recommend
`wellspiral.toml — Fracturize`, and `*wellspiral.toml — Fracturize` once 6.1
gives us a dirty bit. Keep the FPS behind `RUST_LOG` where it already goes.

### 6.3 Up/Down means three things, and gets stuck on one
`main.rs:1004-1023`: Up/Down is *browser navigation* if the browser is open,
else *step transform selection* if a transform is selected, else *zoom*.

Three meanings on one key with no on-screen indicator is already awkward. What
makes it a trap is that **`select_transform(None)` is never called from
anywhere** — I grepped the whole tree — and `load_scene_file` sets
`selected_transform = Some(0)` (`app.rs:2191`). So from the moment you open
any scene, Up/Down can never zoom again for the rest of the session. The
keybind table and the Keybinds window both advertise "Up / Down — zoom in /
out", so the app documents a behaviour you cannot reach.

Recommend: **clicking empty viewport space deselects** (the universal editor
convention, and it's what Escape should fall through to), which fixes the trap
by giving the state a visible exit. Then either accept the overload or move
transform stepping onto its own keys and let the arrows always zoom.

### 6.4 A navigation gesture edits the artwork
`app.rs:1740-1757`: scroll over a hovered gizmo changes that transform's chaos
weight instead of zooming. It's a lovely lever and the hint text advertises it
— but scroll is the *navigation* gesture, the one people use continuously and
without looking, and `todo.txt` already records the failure mode: *"zooming
breaks horribly as things move under the scrollwheel without visibility"*,
alongside the note that the fractal fully occludes gizmos while they keep
taking input.

Recommend: **plain scroll always zooms; weight moves to Alt+scroll** (or
Ctrl+scroll). It stays exactly as reachable — the hint line is right there —
and a navigation gesture can no longer silently edit the artwork. The deeper
fix (depth-aware gizmo picking so an occluded gizmo isn't hoverable) is worth
doing too, but it's a bigger job and this one closes the hole today.

### 6.5 Dialogs that look modal and aren't
`ui/save_as.rs:84-90` anchors Save-As dead centre, non-collapsible,
non-resizable — every visual signal of a modal — but it doesn't block the
panels behind it, which stay live and draggable. Same for the render-job
dialog. Either dim and block (egui has `Modal` now) or stop centring them.
Recommend blocking: both are commit-shaped decisions.

### 6.6 The empty Ctrl-space, and two toolbar groups that behave differently
The single-letter scheme is full and that's fine — it's the Blender approach
and it's fast. But Ctrl is nearly empty: only S, Shift+S, Z, Shift+Z, Y. Free
and expected: **Ctrl+O** (scenes), **Ctrl+N** (blank scene), **Ctrl+Q**
(quit), **F1** (keybinds). Nothing collides.

And in the toolbar, the first four buttons are `toggle_value` bound to
persisted prefs, while Edit / Keybind Help / Scene Browser are
`selectable_label`s bound to plain `App` bools that don't persist
(`ui/toolbar.rs:22-81`). They look identical and behave differently. Unify.
While there: **"Edit" is the wrong label for the gizmo toggle** — next to a row
of window toggles it reads as an Edit *menu*, and to anyone from Blender it
reads as Edit *Mode*, which is a large concept that doesn't exist here. Call it
"Gizmos".

### 6.7 Numeric readouts change width, so the UI vibrates
*Hazel's, and the one thing in this document that no screenshot can show.*

Every numeric readout in the app is formatted with `{:.N}` into a
proportionally-sized label or a content-sized widget. Both halves of that are
unstable: the **string** changes length as the value changes, and the
**widget** is sized to the string. So a value dithering around a boundary
re-lays-out its whole row, every frame, at up to 120fps. It reads as a
physical vibration, it is deeply unpleasant, and it is invisible in any static
capture — which is why it survives review.

Three separate width-changing events, in order of how often they fire:

1. **The sign.** `format!("{:.2}", -0.004)` is `"-0.00"`. A value jittering
   either side of zero — which is *most* of them, since focus, yaw, pitch and
   position all pass through zero constantly while you orbit — flicks one
   character in and out. Note this is not IEEE negative zero: `-0.0 == 0.0` is
   true, so a `v == 0.0` guard doesn't catch it. **The value has to be rounded
   to display precision first, and the sign dropped if the rounded result is
   zero.**
2. **Crossing a power of ten.** `9.99` → `10.00`. Hits the FPS and frametime
   readouts continuously.
3. **Conditional fragments appearing and disappearing** — `" (warming)"`,
   `"(drawing X.XM)"`, the whole zoom and job clusters.

Where it bites, worst first:

- **`status_bar.rs:114-121`**, the `FPS · ms · p99 · ui · wait` line, sits
  inside a `right_to_left` layout. In that layout widgets are placed from the
  right edge, so a width change in *any* of those five numbers shifts
  everything to its **left** — the sparkline, the zoom counter, the point
  stats, the variation readout. Five live numbers, five times a second at
  least, permanently shoving four other elements. This is the continuous
  source.
- **`camera_panel.rs:163-172`**, `yaw · pitch · focus (x, y, z)` — five signed
  values crossing zero while you orbit. Most likely the one you saw.
- **`transforms.rs:730-743`**, `drag_row`. Three bare `ui.add(DragValue)` in a
  horizontal, each sized to its own content. Dragging **x** through zero
  changes x's width and shoves **y** and **z** sideways *while your pointer is
  on the control*. This is the most hostile instance in the app: it moves the
  thing you are currently using, mid-gesture.
- Also `camera_panel.rs:329-336` (path key rows), `transforms.rs:791`
  (contraction), and the tooltip summaries — lower priority, less frequent.

**The fix is a house rule, applied everywhere, not per-site patches.** Add
`src/ui/num.rs`:

```rust
/// Round to `decimals` FIRST, then drop a sign that survived only as -0.00.
pub fn fixed(v: f32, decimals: usize) -> String;
/// `fixed`, right-aligned into an explicit character budget.
pub fn cell(v: f32, decimals: usize, width: usize) -> String;
/// A monospace, right-aligned, fixed-width readout label.
pub fn readout(ui: &mut Ui, v: f32, decimals: usize, width: usize) -> Response;
```

with four rules:

1. **Round before testing the sign.** Kills both `-0.0` and `-0.004`.
2. **Monospace family** for anything numeric, so digit advances are equal.
   Envy Code R when fontconfig resolves it, egui's built-in otherwise — the
   fallback is already wired (`ui/mod.rs:404-420`).
3. **A declared character budget per readout**, right-aligned, that the value
   is clamped into rather than allowed to grow past. This is the "defined
   character limit" — pick it from the range the value can actually take
   (`distance` is `0.05..=100.0`, so five characters covers it) and let
   anything beyond saturate rather than reflow.
4. **Size the widget, not just the string** — `ui.add_sized` around every
   `DragValue`. A short string in a content-sized widget still shrinks the
   widget; the cell has to be fixed independently of what's in it.

Rule 4 is what fixes `drag_row`, and it is the one that can't be done by
formatting alone.

### 6.8 Destructive actions guard against the wrong error
*Also Hazel's.* Render-cancel is currently a two-click arm: first click sets
`cancel_armed_at`, a second click within `CANCEL_ARM_WINDOW` (4 seconds,
`app.rs:57`) confirms (`app.rs:86-100`), with the button relabelling itself to
"Cancel? click again". The intent is right and the labelling is good.

But **there is no minimum delay**, so the two clicks can arrive 20ms apart.
That means the guard is defeated by a double-click — the single most common
accidental mouse input there is. It converts "one stray click destroys an
hour of GPU time" into "one stray *double*-click destroys an hour of GPU
time", which is a much smaller reduction in risk than it looks, because a
double-click is exactly what a hesitant or impatient user produces on a button
that appears not to have responded. And the relabel *invites* a second click.

**The pattern to adopt is click-wait-click** — arm on the first click, accept
the second only after a wall-clock minimum has elapsed. This is Hazel's call
and it is better reasoned than the hold-to-confirm I first proposed here, for
two reasons:

1. **Familiarity.** It is what web download dialogs and other confirmations
   that consider themselves important already use, so the gesture arrives
   pre-learned.
2. **Resilience, which is decisive.** The commonest reason to cancel a render
   job is that *the box has run out of resources and the desktop is crawling*.
   Hold-to-confirm's entire feedback channel is a smooth fill animation and its
   input model is a sustained press — both degrade precisely when you need
   them most. Click-wait-click needs only a timer and two discrete events,
   which survive a compositor running at one frame per second, or per several
   seconds.

That inverts my earlier objection — "a click that does nothing reads as a bug".
The answer isn't a different gesture, it's that the wait must be **shown**, and
shown *discretely*. Four implementation notes follow from the resilience
argument, and all four matter:

- **Wall-clock, evaluated at click time.** Test `armed_at.elapsed() >=
  MIN_WAIT` when the second click arrives. Never a frame counter, and never
  "is the button currently *drawn* as enabled" — at 0.2fps the repaint that
  enables it may not have happened yet even though the timer has long since
  fired. The click must be *accepted on its own merits*, without requiring the
  UI to have caught up.
- **The arm window must be generous.** Today's 4-second expiry (`app.rs:57`)
  is a bound in the wrong direction for the degraded case: at five seconds per
  frame it can close before the armed state has been drawn even once. Make it
  ~30s, or hold it until the pointer leaves or something else is clicked.
- **State changes must be legible without animation.** A discrete label
  sequence — `Cancel` → `Cancel? wait…` → `Cancel? click again` — reads
  correctly at 1fps. A fill wipe does not. A whole-second countdown is fine; a
  smooth progress bar is not.
- **~1 second minimum.** Long enough that no double-click can span it, short
  enough not to feel punitive.

Apply it to **render-job cancel** and to the **Discard** button in the
unsaved-changes dialog from §6.1. *Don't* apply it to Save-As's overwrite
acknowledgement — that one is already double-click-safe by accident, and it's
worth understanding why: a checkbox cannot be double-clicked into a dangerous
state, because the second click of a double-click toggles it back off.
Confirmation-as-a-separate-checkbox is inherently safe in a way that
confirmation-as-a-second-click is not.

### 6.9 Undo is inconsistent about what counts as an edit

Treating a scene as a **document** gives a rule that settles every case:
**if it changes what Ctrl+S writes, it is an undoable edit.**

One exception, and it's conventional: **continuous view state**. Nobody
expects Ctrl+Z to un-orbit a camera, and no 3D application offers it — even
though Ctrl+S does bake the current framing into the scene. Drags produce
thousands of intermediate states and none of them are decisions.

**Camera paths are on the wrong side of that line.** Adding a keypoint is a
discrete, deliberate authoring act — exactly as much an edit as adding a
transform — and it is written to the file. But of six path operations, exactly
one is undoable:

| Operation | Undoable? |
|---|---|
| `add_path_key` | no |
| `remove_path_key_at` | no |
| `set_path_loop` | no |
| `set_path_seconds` | no |
| `set_path_zoom_periods` | no |
| `reset_path_to_default` | **yes** |

Note which one it is. **Reset — the operation that throws away every keypoint
you have — is the recoverable one, while deleting a single keypoint is not.**
That is exactly backwards, and it means what Ctrl+Z does after a path edit
depends on which path edit you happened to make. That is the definition of
surprising.

Fix: route all six through `commit_edit`, coalescing the draggy one
(`set_path_seconds`) the way the inspector fields already do.

**Then the three ways history itself loses data** — one of which, on this
model, turns out not to be a bug at all:

1. **`load_scene_file` calls `history.clear()`** (`app.rs:2198`). Under the
   document model **this is correct**: opening a document gives you a fresh
   undo stack in every editor ever written. The surprise isn't the clear, it's
   that it happens without asking about unsaved work. §6.1's prompt is the
   entire fix — don't try to preserve history across loads.
2. **Silent eviction.** `History::evict` (`history.rs:216-223`) drops the
   oldest entry past 64 entries *or* 128MB (`history.rs:31-34`), and snapshots
   are **whole scenes**. On the L-system scenes the transform rail is
   virtualized for, one snapshot runs to megabytes, so the byte cap binds long
   before the entry cap and can grind the stack toward its floor of one — with
   nothing on screen saying so. Fix with a dimmed `… 12 older edits dropped`
   row at the foot of the list, and a floor of ~10 entries regardless of bytes.
   The root cause (whole-scene snapshots at 50k transforms) is a representation
   problem for a perf pass, not this document.
3. **Committing after undoing clears redo** (`history.rs:109`). Universal, and
   correct. The wrinkle is local: this app *draws* redo entries as clickable
   rows (`ui/explore.rs:123-137`), and things you can see and click read as
   durable. Jump back twenty steps, nudge one slider, and twenty visible rows
   vanish. A hover warning on the redo rows is the cheap answer.

---

## 7. Conventions: one to adopt, three to break on purpose

*(Defend all four in writing — the breaks especially, since an undefended
break reads as an oversight.)*

1. **A File menu, but not a menu bar.** *This revises an earlier draft of this
   section, which defended having neither.* The argument that changed it is
   Hazel's: **the scene is a document**, and the whole value of that abstraction
   is that people can port intuition from every file-editing program they have
   ever used. But intuition needs furniture to attach to — if you want the
   document model, you have to show the affordance that signals it, and that
   affordance is a File menu with the conventional contents in the conventional
   order.

   So: a **File** entry at the left end of the toolbar, opening
   New / Open… / Save / Save As… / — / Screenshot / Render job…, with the
   conventional accelerators (§6.6). What I would *not* add is a full
   File/Edit/View/Help bar: Edit and View would be near-empty duplicates of the
   toolbar toggles and the Explore window, and a menu bar that is mostly empty
   teaches people that menus here aren't worth opening. Undo and redo instead
   get *visible buttons* in the toolbar (§10.8) — they're frequent enough to
   deserve one click, which is exactly the trade a menu is wrong for. File
   operations are numerous, conventional and infrequent, which is exactly the
   trade a menu is right for.

2. **Left-drag orbits.** Modelling tools reserve left for selection. There's
   nothing to marquee-select here, and Apophysis users expect direct
   manipulation. Correct break. Pay for it with 6.3's click-to-deselect.
3. **No modes.** No Object/Edit/Sculpt. A single-mode tool where every
   keystroke means one thing is a real usability advantage and worth
   protecting as the app grows.
4. **Long tooltips.** Convention says one short line. Several here run four or
   five lines of real explanation (the zoom-loop tooltip, the edge guard, the
   point-count slider). Given the density of genuinely unfamiliar concepts —
   renormalization, octaves per period, negative variation weights — this is
   the right break. Make the existing informal shape a stated rule: **first
   line short and imperative, blank line, then the theory.** Most already do
   this; a few bury the actionable sentence at the end.

---

## 8. Mousefeel: the gesture layer

Concrete, cheap, and all in the "tiny details users notice" category.

- **Drag threshold.** `on_mouse_press` sets `Drag::Orbit` the instant the
  button goes down (`app.rs:1892`), so a one-pixel twitch during a click
  rotates the view. Every drag-capable surface in the world has a 3–4px dead
  zone before a drag begins. Add one — it's also the prerequisite for 6.3's
  click-to-deselect, since you need to distinguish a click from a drag.
- **Cursor feedback per gesture.** Already on `todo.txt`. Right now the cursor
  only changes over gizmos (`Grab` → `Grabbing`, `app.rs:1855`). Orbit, pan
  and roll drags leave it as the arrow, so the viewport never confirms which
  mode you're in. Minimum: `Grabbing` for orbit, `Move`/`AllScroll` for pan,
  something distinct for roll. `winit` has no trackball cursor, so roll may
  want a custom bitmap or a status-bar-only cue.
- **Shift = fine during gizmo drags.** Free (unused in the gizmo path),
  universal, and this app's drags are high-gain because they run the chaos game
  live.
- **Rotation snapping.** IFS aesthetics live on clean rotational symmetry, so
  a 15° snap during a rot-edge drag is worth more here than in a general 3D
  tool. Ctrl is taken by uniform scale, so put it on Alt, or on a small
  snap toggle in the Transforms window.
- **A documented deviation, not a fix.** Ctrl-for-uniform-scale is backwards
  from the wider world (Illustrator/Figma/Blender all use Shift to constrain
  proportion). But "ctrl+drag *any* gizmo part = uniform scale" is a clean,
  memorable rule and it's in muscle memory and in AGENTS.md. Recommend adding
  Shift as a *second* binding for uniform scale rather than moving it, and
  noting the deviation.
- **Double-click a transform tab to rename.** Universal — Blender's outliner,
  every file manager, every editor's tab bar. Currently rename is a
  context-menu item or a button (`transforms.rs:377`, `:505`). Cheap.
- **Drag the weight bar on the tab.** Each tab already paints a relative-weight
  bar along its bottom edge (`transforms.rs:302-316`). Making it draggable
  turns a readout into a control and puts the most-adjusted per-transform value
  under the pointer that's already there.
- **A filter box on the transform rail.** The rail is virtualized because
  L-system scenes reach tens of thousands of transforms (`transforms.rs:136`) —
  but scrolling 10,000 tabs to find one is not a workflow. A one-line filter
  makes the virtualization pay off.
- **Alt-click the eye to solo.** Photoshop/Blender convention for layer-ish
  lists, and "show me only this transform's contribution" is a real question
  about an IFS.

---

## 9. A restructure that follows the tasks

The current windows are named after *implementation areas*. The tasks want
them named after **what you're working on**. Three moves, in order of payoff:

**Move 1 — a File menu, and Scenes becomes its Open browser.**
Open / Save / Save as / recent, plus Screenshot and Render job…, move out of
the Camera window's bottom row into a **File** menu at the left end of the
toolbar (§7.1); the Scenes window stays as what `Open…` shows you. The Camera
window keeps framing, saved views, the path and its transport — one coherent
subject. This is exactly what `todo.txt` proposes and it's right: the reason
those buttons ended up in Camera is that Camera was the window with a bottom
panel free, which is an implementation reason, not a task one.

**Move 2 — Infinite zoom gets one home.**
The Render window's `infinite zoom` section becomes the complete control
surface: a **map picker** (all transforms, non-qualifying ones greyed with
their reason — the data already exists in `zoom_action()`), the edge guard,
and the band. Open by default once a scene has a map. The Transforms
context-menu and detail-pane items stay as shortcuts. Fix the stale
right-click direction at `render_panel.rs:351`.

**Move 3 — Toolbar ordering by task, not by module.**
Current order is Transforms, Explore, Camera, Render | Edit, Keybind Help,
Scene Browser | quick controls | scene name. Task order runs from *what am I
working on* to *how am I looking at it* to *help*:

```
File | Transforms  Gizmos | Explore | Camera  Render | Undo Redo  quick controls … name  Help
```

with Keybind Help pushed to the right end where help lives in every toolbar
ever made, and Gizmos sitting next to Transforms because it's a view of the
same object rather than a peer of the panel toggles.

**Not recommended:** collapsing the Transforms inspector into disclosure
sections. It's ~15 controls in one pane, which is at the edge — but `todo.txt`
is explicit that hidden bits in Render are the problem, not the solution, and
that instinct is right. If it needs relief later, separate with rules and
headings rather than closing things.

---

## 10. Grouping, layout, and what lives where

§9 proposed three moves. This section is the reasoning underneath them, and
the rest of what that reasoning turns up.

**The organising abstraction is the scene-as-document.** A scene is a unit you
open, edit, save, and save-as — and that is worth committing to explicitly,
because it is the one model users already have. Everything below follows from
asking, of each control: *is this part of the document, or isn't it?* The
places the current UI is confusing are, almost without exception, the places
that question has no visible answer.

**The core claim: three different axes are conflated, and only one is drawn.**

- **(a) Stage of work** — explore, edit, frame, output. This is what the
  windows are *named* for.
- **(b) Scope** — does this control act on one transform, the whole scene, the
  view, or the application?
- **(c) Persistence** — does this value travel with the artwork (scene TOML,
  undoable), follow the person (prefs), or evaporate at exit?

Windows are organised by (a). (b) and (c) are invisible. And where (a) and (b)
disagree, the code follows (b) while the UI follows (a) — which is exactly how
Save-scene ended up in the Camera window.

### 10.1 Persistence class is invisible, and it has already cost something real

The Render window stacks, with nothing between them saying so:

| Control | Class |
|---|---|
| renderer mode, exposure | **session** — gone at exit |
| point count | **preference** — follows you across scenes |
| point size, colour falloff, gradient, haze, background | **scene** — saved, undoable |
| infinite-zoom band | **scene**, and arguably not "render" at all |

`render_panel.rs:6-8` knows this and says so in a comment. The user gets no
such comment. Nothing on screen distinguishes a slider whose value will be in
your file tomorrow from one that won't.

**And it has already bitten.** `exposure` does not exist in `scene.rs` — it is
an `App` field (`app.rs:558`) fed by `--exposure`. So the splat renderer's
primary brightness control is **never saved with the scene**: tune it, Ctrl+S,
reopen, and it is back to the default with the picture wrong. The shipped
scenes work around this by recording the flags in a **comment**
(`scenes/ammonite.toml:18`: `# fracturize --scene … --splat --exposure 1.8`).
When a file format's workaround for a missing field is a comment telling you
what to type on the command line, the field wants to exist.

**Opinion: don't split windows by persistence class — group *within* them, with
headings that say it.** Splitting would scatter things that belong together by
task. A "Scene" / "Preference" heading, or a small marker glyph on
preference-class rows, costs one line each and answers a question users
currently can't ask. Then fix `exposure` by making it scene data.

### 10.2 The Camera window is five windows in a trenchcoat

It currently holds: **how the mouse behaves** (orbit style, invert pitch),
**where the camera is** (distance, roll, level, readout), **bookmarks** (saved
views), **the shot** (path, keypoints, transport, loop, duration), and **file
output** (render job, screenshot, save, save as).

The structural symptom is already documented in the codebase: `ui/mod.rs:160-183`
records that this window needs ~290px of *fixed furniture* before its list gets
a single pixel, and it is the only window that needed a load-bearing minimum
height. **A window that can't fit its own chrome is a window doing too much** —
that comment is a bug report about information architecture wearing a layout
comment's clothes.

Two things don't belong on scope grounds:

- **Orbit style and invert pitch are input preferences, not camera state.**
  They don't move the camera; they change how the *next drag is interpreted*,
  and `set_orbit_style` says so explicitly (`app.rs:1774-1784`). They belong
  wherever control preferences live — which argues for renaming the Keybinds
  window to **Controls** and giving it the two prefs plus, eventually, UI scale.
  That also gives the one window a stranger opens first something to be besides
  a table.
- **The output row belongs in Files** (§9, Move 1).

What's left — framing, views, path — is one coherent subject and fits.

### 10.3 Choosing the zoom map is a one-of-n drawn as n toggles

`ui/radio.rs`'s own module doc makes the argument: *"a lone toggle button says
'this is on or off', and three of them side by side say 'three independent
toggles'."* Picking the scene's zoom map is a **choose-one-of-n over
transforms** — only one map can carry the scale symmetry — but it's drawn as a
`Button::selected` on whichever transform you have selected
(`transforms.rs:525`), with the other n−1 options not on screen at all.

So it has the exact defect the radio module was written to eliminate, spread
across *time* instead of space: you can't see the alternatives, and nothing
says only one can be lit. This is a second, independent argument for the map
picker in §9 Move 2 — not just discoverability, but drawing the true shape of
the choice.

### 10.4 The Transforms inspector is a flat stack, and there are four ways to rename

The pane runs: header, actions, position, rotation, scale, `───`, weight,
colour, colour value, speed override, name, `───`, variations. Fifteen-odd
controls with two bare separators doing all the grouping work.

The natural grouping is four blocks, and the current order cuts across it:

1. **Shape** — position, rotation, scale ✓ already together
2. **Behaviour** — weight *and* variations: what this map does in the chaos
   game. These are the two controls that most define the transform, and they
   currently sit at **opposite ends** of the pane with the entire colour block
   between them.
3. **Appearance** — colour, colour value, speed override
4. **Identity** — name

Moving weight down to sit with variations, and giving the four blocks headings
instead of anonymous rules, is a small change that makes the pane scannable.

**Separately: renaming has four affordances**, two of them in the same window
behaving differently. The rail row has an inline editor; the pane has a `Name`
text field; the context menu has `Rename`; and the action row has a `Rename`
button — which opens the editor **over in the rail** rather than focusing the
field sitting six rows below it (`transforms.rs:505-521`). Two visible rename
controls in one window that put your caret in different places is worse than
either alone. Keep the rail inline editor (plus double-click, §8) and the pane
field; make the action-row button focus the pane field, or drop it.

The same duplication is milder on enable/disable: the eye icon on the tab and
the Enable/Disable button in the action row. That pair is defensible — one is
per-row in a list, one is for the thing you're inspecting — but it's worth
knowing it's there.

### 10.5 Two adjacent lists that look alike and mean different things

In the Camera window, **Saved views** (scrolling list of clickable rows,
`max_height 110`) sits directly above **Camera path** (scrolling list of rows
with ✕ buttons). They render nearly identically. One is a bookmark you jump
to; the other is a frame of an animation. They also differ in persistence —
views are separate files under `views/`, keypoints are scene data.

Adjacent same-shaped lists with different semantics is a reliable confusion
generator. If they stay in one window they need to look different — an icon
per row, or the views as chips rather than a list.

### 10.6 A disclosure should hide elaboration, never the headline

Two `collapsing` sections, one right and one wrong, which makes the rule easy
to state:

- **Haze — correct.** The `haze` amount slider is always visible; `haze band`
  hides the elaboration (pin, near, far) that most people never touch.
- **Infinite zoom — incorrect.** The *entire feature* is behind a closed
  disclosure, with `edge guard` inside it and `band size` nested inside that.
  Two levels deep for the headline control of the most recent plan.

Rule: **if a section is closed and the feature is on, the disclosure is
hiding the wrong thing.** Open the infinite-zoom section whenever the scene has
a zoom map.

### 10.7 The containers themselves are fine — it's what's in them

The window/popup/menu *selection* is, with one exception, well judged:

- **Point-count popup from the toolbar** (`toolbar.rs:187-192`) — right call.
  The toolbar has no room for a draggable log slider, the popup gets the real
  widget, `CloseOnClickOutside` is correctly set so grabbing the slider doesn't
  dismiss it, and it delegates to the same function as the panel so the two
  can't drift. This is the pattern to copy elsewhere.
- **Scene identity popup** — a readout that opens an editor for itself is a
  good pattern and the right size of container for two strings.
- **Gizmo context menu as a hand-rolled `Area`** — justified in the code
  (`ui/mod.rs:322-328`): there is no egui widget under the pointer, because the
  thing clicked is a tetrahedron. It behaves like a menu in every other
  respect. Correct.
- **Render-job dialog dismissible while the job runs** — right, because once
  started it stops being a form and becomes a monitor, and the status bar
  already carries progress.
- **Save As** is the exception: dressed as a modal, isn't one (§6.5).

So I'd change almost nothing about *which kind of box* things live in. The
problems are all about which things are in which box.

### 10.8 Toolbar quick controls: right principle, one omission

The stated criterion — "reached for often enough that opening a panel is
friction" — is the right one, and mode / point count / play-pause all pass it.
The omission is **undo**. It's the one control people reach for with the mouse
*even when they know the shortcut*, because it gets used at exactly the moment
confidence is low and something has just gone wrong. A creative tool with no
undo button in its toolbar is unusual. **Mutate** has a weaker claim but a real
one: mutate → judge → undo → mutate is the app's core exploration loop, and two
thirds of it currently needs the Explore window open.

---

## 11. Worklist, in the order I'd do it

**Safety — do these first, they're the ones that lose work.**
1. Scene dirty flag; Escape stops quitting and falls through cancel → deselect;
   Ctrl+Q and window-close both prompt; `load_scene_file` prompts. (§6.1)
2. Click-wait-click confirm widget — wall-clock, evaluated at click time,
   ~1s minimum, ~30s arm window, discrete label states. Render-cancel and the
   dialog's Discard adopt it. (§6.8)
3. Click empty space deselects; add the 3–4px drag threshold. (§6.3, §8)
4. Plain scroll always zooms; weight moves to Alt+scroll. (§6.4)
5. Route all six camera-path operations through `commit_edit`, so deleting a
   keypoint is as undoable as resetting the whole path. (§6.9)
6. History: floor of ~10 entries under the byte cap, and say in the Explore
   list when older edits were dropped. (§6.9)

**Typographic stability — one module, then a sweep.**
7. `src/ui/num.rs` with round-then-drop-sign, monospace, an explicit character
   budget, and `add_sized` cells. Then convert, worst first: the status bar's
   right-to-left cluster, `drag_row`'s three DragValues, the camera panel's
   yaw/pitch/focus line, the path key rows. (§6.7)

**Polish that costs almost nothing.**
8. Window title becomes `scene — Fracturize`, with a dirty marker. (§6.2)
9. Ctrl+O / Ctrl+N / Ctrl+Q / F1; Ctrl+Y aliases Redo. (§6.6, §3)
10. Cursor per drag mode. (§8)
11. Shift = fine during gizmo drags; double-click a tab to rename. (§8)
12. Smooth the camera on view-load and `level` (~200ms). (§4)
13. Rename "Edit" → "Gizmos"; unify the two toolbar button kinds; move
    `show_help`/`show_browser` into `PanelPrefs`. (§6.6, §1)

**Structure.**
14. Move 1 — File menu takes the file operations; Scenes becomes its Open
    browser. (§9, §7.1)
15. Move 2 — infinite zoom gets a map picker and one home; fix the stale
    right-click hint. (§9, §2 T6)
16. Move 3 — toolbar reordering, plus an undo button. (§9, §10.8)
17. `exposure` becomes scene data; group each panel's controls by persistence
    class with headings. (§10.1)
18. Orbit style + invert pitch move to a renamed **Controls** window; the
    Camera window keeps framing, views and path. (§10.2)
19. Transforms inspector regrouped Shape / Behaviour / Appearance / Identity,
    with weight moved next to variations; the duplicate rename affordance
    resolved. (§10.4)

**Features, in value order.**
20. Corner axis widget, clickable for axis-aligned views. (§4)
21. "Frame the selected transform's fixed point". (§4)
22. The 3×3 mutation grid. (§3) — biggest single win, biggest single job.
23. Transform rail filter box; draggable weight bars; alt-click solo. (§8)
24. Rotation snapping. (§8)

**Deferred, deliberately, and worth saying so in AGENTS.md:** multi-select,
numeric entry mid-drag, transform reordering (meaningless for an IFS — order
doesn't affect the attractor, weights do), and a menu bar.

**Not in this document:** anything that needs the app running. Hover states,
the actual feel of the drag gains, whether the Phosphor icons read at a
glance, whether a light-mode desktop gets a coherently themed app (egui gets
`window.theme()` at `ui/mod.rs:479` but the context isn't told to follow it),
and whether the panels are legible on a 4K display — egui takes the window
scale factor but there's no UI-scale preference, which is a one-line
`ctx.set_zoom_factor` and an expected setting. Those want a live session.
