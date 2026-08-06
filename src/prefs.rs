//! Persistent user preferences (not scene data): things like pitch
//! inversion that follow the person, not the artwork.
//!
//! Stored at $XDG_CONFIG_HOME/fracturize/prefs.toml (~/.config fallback).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::camera::OrbitStyle;

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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Prefs {
    /// Flightsim-style: drag down to tilt the scene's top toward you
    #[serde(default)]
    pub invert_pitch: bool,
    /// Which axis a horizontal orbit drag yaws about. Defaults to
    /// [`OrbitStyle::Trackball`] — screen-relative, so the controls feel the
    /// same wherever the camera has been; `turntable` restores the world-Y
    /// orbit for anyone who wants the horizon held level instead.
    ///
    /// [`OrbitStyle::Trackball`]: crate::camera::OrbitStyle::Trackball
    #[serde(default)]
    pub orbit_style: OrbitStyle,
    /// Open/closed state of the Phase 2 panel windows
    #[serde(default)]
    pub panels: PanelPrefs,
    /// Strength multiplier for U's random mutation (Explore window slider)
    #[serde(default = "default_mutate_strength")]
    pub mutate_strength: f32,
    /// Who to credit on scenes made in-app. Scene files carry their own
    /// author; this is only the default for one that doesn't have one yet (a
    /// blank canvas, a random flame), so a person doesn't have to type their
    /// name into every scene they start. `None` = never set one, leave the
    /// author blank.
    #[serde(default)]
    pub author: Option<String>,
    /// Point-buffer capacity chosen in the Render window. A performance
    /// setting that follows the person rather than the artwork, so it wins
    /// over a scene file's `point_count` at startup — but loses to an
    /// explicit `--points`. `None` = never set one, defer to the scene.
    #[serde(default)]
    pub point_count: Option<usize>,
    /// Where each panel window was left, keyed by its `ui::WindowKey` name:
    /// `[x, y, width, height]` in egui points. Panels you've arranged stay
    /// arranged across launches; anything absent falls back to the
    /// screen-relative default layout in `ui::default_layout`.
    #[serde(default)]
    pub window_geometry: BTreeMap<String, [f32; 4]>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            invert_pitch: false,
            orbit_style: OrbitStyle::default(),
            panels: PanelPrefs::default(),
            mutate_strength: default_mutate_strength(),
            author: None,
            point_count: None,
            window_geometry: BTreeMap::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefs_file_written_before_the_orbit_style_still_loads() {
        // [`Prefs::load`] falls back to `Default` on *any* parse error, so a
        // field that can't cope with an older file doesn't merely lose its own
        // value — it silently resets every other preference the person has.
        let old = "invert_pitch = true\nmutate_strength = 2.5\n";
        let p: Prefs = toml::from_str(old).expect("an older prefs file must still parse");
        assert!(p.invert_pitch, "the settings that were there must survive");
        assert_eq!(p.orbit_style, OrbitStyle::Trackball, "and the new one takes its default");
    }

    #[test]
    fn the_orbit_style_round_trips_under_the_name_it_is_written_by() {
        let p = Prefs { orbit_style: OrbitStyle::Turntable, ..Default::default() };
        let s = toml::to_string(&p).expect("prefs must serialize");
        assert!(s.contains(r#"orbit_style = "turntable""#), "{}", s);
        assert_eq!(toml::from_str::<Prefs>(&s).unwrap().orbit_style, OrbitStyle::Turntable);
    }
}
