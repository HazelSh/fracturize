//! Persistent user preferences (not scene data): things like pitch
//! inversion that follow the person, not the artwork.
//!
//! Stored at $XDG_CONFIG_HOME/fracturize/prefs.toml (~/.config fallback).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which of the Phase 2 `egui::Window` panels are open. Toggled from the top
/// toolbar or a window's own close button (the two are the same bool, so
/// they can never go out of sync); persisted the moment either changes.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq)]
pub struct PanelPrefs {
    #[serde(default)]
    pub transforms_open: bool,
    #[serde(default)]
    pub explore_open: bool,
    #[serde(default)]
    pub camera_open: bool,
    #[serde(default)]
    pub render_open: bool,
}

fn default_mutate_strength() -> f32 {
    1.0
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Prefs {
    /// Flightsim-style: drag down to tilt the scene's top toward you
    #[serde(default)]
    pub invert_pitch: bool,
    /// Open/closed state of the Phase 2 panel windows
    #[serde(default)]
    pub panels: PanelPrefs,
    /// Strength multiplier for U's random mutation (Explore window slider)
    #[serde(default = "default_mutate_strength")]
    pub mutate_strength: f32,
    /// Point-buffer capacity chosen in the Render window. A performance
    /// setting that follows the person rather than the artwork, so it wins
    /// over a scene file's `point_count` at startup — but loses to an
    /// explicit `--points`. `None` = never set one, defer to the scene.
    #[serde(default)]
    pub point_count: Option<usize>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            invert_pitch: false,
            panels: PanelPrefs::default(),
            mutate_strength: default_mutate_strength(),
            point_count: None,
        }
    }
}

fn prefs_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("fracturize").join("prefs.toml"))
}

impl Prefs {
    pub fn load() -> Self {
        prefs_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = prefs_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match toml::to_string(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&path, s) {
                    log::warn!("Failed to save prefs to {}: {}", path.display(), e);
                } else {
                    log::info!("Prefs saved to {}", path.display());
                }
            }
            Err(e) => log::warn!("Failed to serialize prefs: {}", e),
        }
    }
}
