//! Saved view parameter files (TOML)
//!
//! A view captures everything about how the camera sees the scene — orbit
//! angle, distance, focus, offset — plus point size and fog, so a framing
//! found interactively can be reproduced exactly (press V to save, load
//! with --view, render offline with --render).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct View {
    /// Scene file this view was captured from (provenance only)
    pub scene: Option<String>,
    /// Orbit angle in radians
    pub rotation: f32,
    /// Orbit radius
    pub distance: f32,
    /// Orbit center / look-at point
    pub focus: [f32; 3],
    /// Added to orbital camera position
    pub offset: [f32; 3],
    pub point_size: f32,
    pub fog_near: f32,
    pub fog_far: f32,
    pub fog_brightness: f32,
    pub fog_saturation: f32,
}

impl View {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let content = fs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read view file: {}", e))?;
        toml::from_str(&content).map_err(|e| format!("Failed to parse view file: {}", e))
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize view: {}", e))?;
        if let Some(dir) = path.as_ref().parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir).map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
            }
        }
        fs::write(path.as_ref(), content)
            .map_err(|e| format!("Failed to write view file: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_roundtrip() {
        let view = View {
            scene: Some("scenes/glasshouse.toml".to_string()),
            rotation: 1.25,
            distance: 4.0,
            focus: [0.1, 0.2, 0.3],
            offset: [0.0, 1.0, 0.0],
            point_size: 0.002,
            fog_near: 3.0,
            fog_far: 4.5,
            fog_brightness: 0.4,
            fog_saturation: 0.3,
        };
        let dir = std::env::temp_dir().join("fracturize_view_test");
        let path = dir.join("roundtrip.toml");
        view.save(&path).unwrap();
        let loaded = View::load(&path).unwrap();
        assert_eq!(loaded.rotation, view.rotation);
        assert_eq!(loaded.focus, view.focus);
        assert_eq!(loaded.scene, view.scene);
        std::fs::remove_dir_all(&dir).ok();
    }
}
