//! Persistent user preferences (not scene data): things like pitch
//! inversion that follow the person, not the artwork.
//!
//! Stored at $XDG_CONFIG_HOME/fracturize/prefs.toml (~/.config fallback).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug)]
pub struct Prefs {
    /// Flightsim-style: drag down to tilt the scene's top toward you
    #[serde(default)]
    pub invert_pitch: bool,
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
