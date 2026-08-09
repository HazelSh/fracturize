//! Phosphor icon codepoints used by the Phase 2 toolbar/status bar.
//!
//! `egui-phosphor` has no release compatible with egui 0.35 (latest pins
//! egui ^0.34 — see the GUI upgrade plan / AGENTS.md), so the font itself is
//! vendored at `assets/fonts/Phosphor.ttf` (regular variant, MIT-licensed;
//! see `assets/fonts/LICENSE-phosphor.txt`) and registered as a fallback
//! font in `ui::install_fonts`. These codepoints were copied by hand from
//! egui-phosphor's generated `src/variants/regular.rs`
//! (github.com/amPerl/egui-phosphor) — only the glyphs this phase needs.
//!
//! Domain objects (transforms, variations) are identified by palette color +
//! text name elsewhere in the UI, never by icon — icons are for actions/
//! windows only.

/// General menu toggle
pub const LIST: &str = "\u{E2F0}";
/// Transforms window toggle
pub const SHAPES: &str = "\u{EC5E}";
/// Explore window toggle (mutate / undo / random-flame exploration)
pub const FLASK: &str = "\u{E79E}";
/// Camera window toggle
pub const VIDEO_CAMERA: &str = "\u{E4DA}";
/// Render window toggle
pub const SLIDERS: &str = "\u{E432}";
/// Shortcuts (legacy keybind help overlay) toggle
pub const KEYBOARD: &str = "\u{E2D8}";
/// Browser (legacy scene browser overlay) toggle
pub const FOLDER_OPEN: &str = "\u{E256}";
/// Transform row eye toggle: enabled
pub const EYE: &str = "\u{E220}";
/// Transform row eye toggle: disabled
pub const EYE_SLASH: &str = "\u{E224}";
/// Remove / dismiss (inspector variation rows). The obvious "✕" U+2715 isn't
/// in Envy Code R or egui's built-in fonts and renders as tofu, so use
/// Phosphor's.
pub const X: &str = "\u{E4F6}";
/// Roll a new random flame (Explore window) — Phosphor `dice-five`
pub const DICE: &str = "\u{E1EE}";
/// Start a blank scene (Explore window) — Phosphor `file`, the empty page
pub const FILE: &str = "\u{E230}";
/// Camera transport: motion is stopped, click to start
pub const PLAY: &str = "\u{E3D0}";
/// Camera transport: motion is running, click to stop
pub const PAUSE: &str = "\u{E39E}";
/// Transform gizmos + labels toggle (G) — Phosphor `cube`, for the tetrahedral
/// cells it draws. Verified by rendering, not guessed: this font's codepoints
/// are roughly alphabetical but not reliably so, and `cube`'s neighbours in the
/// obvious place turned out to be `circle`.
pub const CUBE: &str = "\u{E1DA}";

// -- Added for the "text buttons need icons too" pass. Every codepoint below
// was located by rendering the vendored font's whole PUA range (E000-EE83, via
// `fontTools.ttLib.TTFont.getBestCmap`) to contact sheets with Pillow and
// reading the glyph off the page — not guessed and not carried over from
// egui-phosphor's source, which doesn't build against this egui version (see
// the module doc above). Existing constants above were trusted as already
// verified by whoever added them.

/// Save (scene, view) — Phosphor `floppy-disk`
pub const SAVE: &str = "\u{E248}";
/// Discard — Phosphor `trash`
pub const TRASH: &str = "\u{E4A6}";
/// Quit the app — Phosphor `power`
pub const QUIT: &str = "\u{E3DA}";
/// Screenshot. Deliberately not `VIDEO_CAMERA`: that icon means "the Camera
/// window" elsewhere in this UI, and a screenshot isn't that — Phosphor
/// `camera`, the plain stills camera.
pub const CAMERA: &str = "\u{E10E}";
/// Render job… (batch render dialog) — Phosphor `stack`, distinct from
/// `SLIDERS` (the Render *window*, a different thing you open).
pub const STACK: &str = "\u{E466}";
/// Undo — Phosphor `arrow-u-up-left`, the hooked "undo" arrow
pub const UNDO: &str = "\u{E018}";
/// Redo — Phosphor `arrow-u-up-right`, mirror of `UNDO`
pub const REDO: &str = "\u{E01A}";
/// Reset to a default (camera path "Reset") — Phosphor `arrow-counter-clockwise`,
/// a full-circle revert, deliberately different from `UNDO`/`REDO`: this isn't
/// undoing an edit, it's throwing the path away and going back to the default.
pub const RESET: &str = "\u{E038}";
/// Level the camera — Phosphor `crosshair-simple`, read as "centre this back up"
pub const TARGET: &str = "\u{E1D6}";
/// Mutate the scene — Phosphor `shuffle`, distinct from `DICE` (a fresh random
/// flame from nothing; this perturbs the one already on screen)
pub const SHUFFLE: &str = "\u{E424}";
/// Dimension-lock toggle: locked — Phosphor `lock`. Verified the same way as
/// the block above (contact sheet over the vendored font's PUA range,
/// rendered with Pillow and read off the page): the lock family sits at
/// E2FA–E30A, several closed/open pairs deep (`lock-simple`, `lock-key`,
/// `lock-laminated`, plain `lock`); this is the plain undecorated pair, the
/// most legible at status-bar size.
pub const LOCK: &str = "\u{E308}";
/// Dimension-lock toggle: unlocked — Phosphor `lock-open`, mirror of `LOCK`.
pub const LOCK_OPEN: &str = "\u{E30A}";

/// Add a new thing to a list (Transforms rail) — Phosphor `plus`. Located the
/// same way as the block above: `play` (E3D0, already trusted here) anchors
/// that stretch of the sheet, and `plus` sits two even codepoints along.
///
/// Note this font's PUA is *even*-codepointed — the odd slots between these
/// are unmapped — so a neighbouring icon is +2, not +1.
pub const PLUS: &str = "\u{E3D4}";
/// Duplicate the selected thing (Transforms rail) — Phosphor `copy`, the two
/// overlapping sheets. Picked over `copy-simple` (E1CC, a single sheet), which
/// at button size is hard to tell from `file`. `cube` (E1DA) anchors this
/// stretch of the sheet.
pub const COPY: &str = "\u{E1CA}";
