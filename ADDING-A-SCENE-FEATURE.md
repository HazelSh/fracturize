# Adding a feature to the scene format

A scene feature is anything that changes what a scene *is* — a new key, a new
table, a new kind of map, a new way of combining them. Symmetry, palettes,
infinite zoom and subscenes are all scene features. They share a shape: each
has to be **written down** (the TOML), **edited** (the GUI), **read by an
agent** (`--info`, `-S`), **drawn** (GPU, and the CPU walkers that mirror it)
and **shown off** (a scene that uses it). Miss one and the feature exists in
some of the program and silently not in the rest — which is how `[zoom]` edits
were once dropped on every save, and how `exposure` went years without being
scene data at all.

This is the checklist, in the order that works, with the traps that each step
has actually sprung. Subscenes (`src/subscene.rs`, 2026-09) are the worked
example throughout; `git log --stat` on that work shows every file this list
touches.

It is written for an agent doing the work, so effort is measured in what to
run and what to check, never in time.

---

## 0. Decide the model before writing any code

Four questions, each of which reshapes everything after it:

- **Whose property is it?** A transform's (symmetry: `TransformSpec::symmetry`),
  the scene's (`[meta] haze`), or something between (a subscene is a *layer*:
  a tag on each transform plus a table of layers). Put it where it is true, and
  let a file-format table be sugar over that — `[[symmetry]]` blocks resolve onto
  transforms at load and are regrouped at save.
- **What persistence class?** Scene data (Ctrl+S writes it → it is an undoable
  edit), view data (a grade, a framing), or a preference (prefs.toml, follows
  the person). AGENTS.md's rule: *if it changes what Ctrl+S writes, it is an
  undoable edit.*
- **Does the GPU need it, and can an ordinary scene stay byte-identical?** The
  answer should be yes: gate the new behaviour behind a flag that is zero for
  every scene that doesn't use it (subscenes: `ComputeParams::composite`). Then
  "nothing moved" is something you can *prove* (§4) rather than hope.
- **Which consumers treat the scene as one IFS, and are they still right?**
  `grep -n "scene.transforms" src/` and read every hit. For subscenes, four
  were silently wrong — the drawn-point budget, the trace overlay, `--info`'s
  shape measurement, and the status bar's dimension — and none would have
  crashed. A feature that changes what the scene *means* has to visit all of
  them.

Write the model down as the module doc of its own file before the code. It is
where the reasoning a later reader needs will live.

## 1. The file format — `src/scene.rs`

- **A `…Def` struct for the TOML, a runtime struct for everything else.** The
  loader resolves one into the other once, at the boundary. Use serde defaults
  so existing files load unchanged, `skip_serializing_if` so saving a scene
  that doesn't use the feature writes nothing new, and `deny_unknown_fields` on
  a *new* table (old tables can't take it without breaking files on disk).
- **Errors, not fallbacks.** A malformed value is a load error that names the
  problem and the fix. A scene that quietly rendered without the thing it asked
  for is far more expensive to diagnose.
- **Reuse the existing builders.** Subscenes needed transforms built exactly as
  top-level ones are, so the per-transform code in the loader moved into
  `build_transforms` / `resolve_colour` / `transform_def` and both paths call
  them. Two copies of "how a transform is read" would drift.
- **Saving has three places to reach, and missing one is silent:**
  1. `to_scene_file` — the serialization structs;
  2. `merge_scene_into_document` — the comment-preserving merge into an
     existing file. It only writes what it has a case for, so a feature it
     doesn't know about is *dropped on every save over an existing file*. For a
     table whose shape changes under editing, rebuild it wholesale from the
     fresh serialization (as `[[symmetry]]` and `[[subscene]]` do) rather than
     merging key by key;
  3. the text post-passes in `to_toml_string` (`inline_variation_tables`,
     `rewrite_palette_table`) — serde's raw output for nested tables is legal
     but unreadable, and a nested table is easy to lose in a text round trip
     (a subscene's palette stops were, until they were rewritten from the
     `PaletteDef` in hand rather than re-parsed).
- **Anything path-like has to survive Save-As.** Subscene `file =` references
  are relative to the file they are written in, so `save()` respells them for
  the target directory. The every-scene round-trip test caught this, because it
  saves every scene to a temp directory.
- **Never edit another scene file as a side effect.** A referenced or imported
  scene is someone's artwork; subscenes write edits inline and leave the source
  alone.

**Prove it:** unit tests that load, save, reload and compare (see
`scene::subscene_tests`), including a save to a *different* directory and a
file that must come back untouched. `every_scenes_transforms_survive_a_save`
and `a_saved_scene_flies_the_same_path` automatically cover any new scene you
add to `scenes/`.

## 2. `-S` / `--sweep` — `src/set.rs`

`-S` edits the TOML text before parsing, so every other part of the program
sees an ordinary scene. Three things to do:

- **Add the fields to `struct_fields!`.** It is an exhaustive destructuring
  pattern, so adding a field to a `…Def` struct *fails the build* until the
  list is updated — the compiler, not a reviewer, keeps `-S` validation in step.
  For a new table, add a new `struct_fields!` list and a `validate_path` arm.
- **Resolve names the way the loader does.** An unnamed subscene is called
  after its file's stem by the loader, so `set.rs` resolves the same stem;
  otherwise `--info` shows a name `-S` can't find. (File names can't be path
  segments anyway — the dots split them.)
- **Refuse no-ops.** A path that would change nothing (a borrowed subscene's
  maps aren't in this file; a `rotation` shadowed by a `rotvec`) is an error
  that says why. A sweep that renders nine identical tiles is the failure this
  module exists to prevent.

Sweeping the feature is then free: `--sweep subscene.smoke.share=0.3,0.6,1.0`.

## 3. `--info` — `src/info.rs`

The agent's view of a scene. Conventions (all enforced or golden-tested):

- **A section only when the feature is present** (`symmetry`, `layers`), so the
  forty scenes without it don't each pay lines for an absence.
- **Fixed schema inside it**: every row every time.
- **78 columns, hard.** Check with
  `fracturize -s x.toml --info | awk 'length > 78'`. The section key sits in a
  9-column gutter including its trailing space, so **a key is at most 8
  characters** — `subscenes` ran into its first row; the section is `layers`.
- **Emit the action.** Where a number is something to act on, print the flag:
  `-S subscene.smoke.share=0.412`, not "consider lowering the share".
- **Notes are for judgements** (tagged with the section they are about):
  imbalance, things that won't render, things far apart.
- Measure what is drawn, not what is stored. For subscenes that meant a new
  `trace::measure_layers` — each layer walked on its own maps, placed, pooled by
  share — because walking every map as one IFS measures a picture nothing draws.

Re-bless goldens only if an *ordinary* scene's report should change:
`FRACTURIZE_BLESS=1 cargo test golden`, and read the diff. For subscenes it
shouldn't have, and it didn't.

## 4. The renderer — GPU, offline, and the CPU mirrors

- **Structs are one fact in two languages.** A Rust `#[repr(C)]` struct in
  `buffers.rs` / `compute.rs` and its WGSL twin. Reuse padding words where they
  exist (subscenes took two of `ComputeParams`' three) and add a test that pins
  the size and checks the shader declares what Rust thinks it does
  (`the_shader_agrees_about_the_layer_table`).
- **Mind the binding budget.** Eight storage buffers per shader stage is the
  default limit; the chaos pass is at seven. A small fixed-size table can go in
  a uniform instead (the layer table does).
- **Build the GPU data in one place** from the `Scene` (`LayerTable::build`) and
  call it from every path: the live app, and each `PointCompute::new` in
  `offline.rs`. `offline.rs` has several render entry points; `grep -n
  "PointCompute::new\|packs_rgb\|resolve_color_speeds" src/` finds them all.
- **The CPU walkers (`trace.rs`) mirror the chaos pass.** They drive the trace
  overlay, `randomize.rs`'s quality gate, the drawn-point budget and `--info`.
  If the GPU walk changes, they change too.
- **Prove ordinary scenes are unchanged**, byte for byte:

  ```sh
  # A baseline binary from HEAD, built outside the shared tree.
  B=$CLAUDE_JOB_DIR/tmp/base; mkdir -p $B
  git archive HEAD | tar -x -C $B
  (cd $B && CARGO_TARGET_DIR=$CLAUDE_JOB_DIR/tmp/base-target cargo build --release)
  # Then, per scene and per renderer, render with both and compare pixels
  # (PIL's Image.tobytes() — the PNG files differ anyway, by timestamp).
  ```

  If HEAD doesn't build — another session's committed half-work, which happens
  in this checkout — copy their *uncommitted* files over the export and remove
  your own lines from them, so the baseline is "everything but your change".
  Render with `--effort small` at a small size, several scenes, both `--splat`
  and points; also diff `--info` output.

## 5. The app — `src/app.rs` and `src/ui/`

- **Every edit goes through `commit_edit`**, snapshot first. `EditSnapshot`
  clones the whole `Scene`, so new scene fields are undoable for free; a live
  copy kept on `App` (like `color_falloff`) has to be added to the snapshot.
- **State that points into the scene goes stale on undo.** An undo can remove
  the subscene that was selected; `after_scene_shape_change` is where such
  indices are revalidated.
- **Gizmos are an instance list fixed at construction.** Subscenes feed the
  gizmo renderer, picking, labels, the roll ring and the indicators from one
  function (`App::gizmo_specs`, scene space) and rebuild the renderer when the
  list's length changes. Several consumers each reading `scene.transforms` would
  have drifted apart.
- **UI conventions are a house rule** (AGENTS.md, "The chrome doesn't move"):
  every widget through `hints::hinted`, numbers through `ui::num`, one-of-n as
  `ui::radio`, disabled-not-hidden with the reason in the tooltip. A new window
  needs a `WindowKey` variant, a `default_layout` case, a `PanelPrefs` flag, a
  toolbar toggle and a line in the draw order in `ui::draw`.
- **Icons:** check the glyph exists in the vendored font before using it —
  Phosphor's names are GSUB ligatures, so a script over
  `fontTools.ttLib.TTFont('assets/fonts/Phosphor.ttf')['GSUB']` lists them — and
  check the icon doesn't already mean something else (`STACK` is the render
  job; subscenes use `UNITE`).
- **Test it in the running app, in Xephyr** (AGENTS.md, "Driving the running
  app"). Pre-open windows with an isolated `XDG_CONFIG_HOME` holding a
  `prefs.toml` (`[panels] subscenes_open = true`), pin the camera with `-v`,
  capture the Xephyr root, and read `RUST_LOG=info` output to confirm what
  happened. **Keys don't arrive in Xephyr** — there is no window manager to give
  the app focus — so drive keyboard-only actions through their buttons (Undo is
  on the toolbar) or set input focus explicitly.

## 6. A scene that uses it

A new file in `scenes/`, authored under your model's name, with a header comment
saying what it demonstrates and the commands to look at it. Tune it with the
tools you just built — for subscenes that was `--info`'s `layers` section and
`-S`/`--sweep` over `subscene.*` paths, at `--effort small`, then one
`--effort medium` render to judge. Render into `renders/` (gitignored), never
the repo root. Write down what you learned about the craft in CRAFT.md's
discovery log — `censer` found that a composite with a solid in it wants the
points renderer, which nothing predicted.

## 7. Docs

- **AGENTS.md**: a section for the feature (model, file format, CLI, app,
  interactions, deferred-on-purpose), the example in "Scene Files (TOML)", the
  architecture tree, the windows table, mouse/keybind rows.
- **CRAFT.md**: discovery-log entry if it changes how scenes are made.
- **This file**, if the feature taught a new trap.

AGENTS.md is shared by every session in the checkout and can lose an edit with
no error. Apply doc changes with assert-guarded scripts, grep for a phrase from
each edit before committing, and commit promptly.

## 8. Commit, in a shared tree

- Run the full suite once at the end (`cargo test --bin fracturize`; the one
  slow test is `fit::solve::…the_loss_has_a_basin…`, and `--skip
  the_loss_has_a_basin` is the fast loop while iterating).
- Stage only your own hunks. Files other sessions have uncommitted work in
  (`git status` at the start says which) get committed hunk by hunk: filter
  `git diff` to your hunks and `git apply --cached` them, then check
  `git diff --cached` contains nothing of theirs.
- Push when it lands — it is cross-device sync, and the only other copy.
