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

/// Transforms window toggle
pub const LIST: &str = "\u{E2F0}";
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
/// Camera transport: motion is stopped, click to start
pub const PLAY: &str = "\u{E3D0}";
/// Camera transport: motion is running, click to stop
pub const PAUSE: &str = "\u{E39E}";
