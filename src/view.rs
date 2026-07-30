//! Saved view parameter files (TOML)
//!
//! A view captures everything about how the camera sees the scene — orbit
//! angle, distance, focus, offset — plus point size and haze, so a framing
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
    /// Orbit elevation angle in radians (0 = level with the focus)
    #[serde(default)]
    pub pitch: f32,
    /// Roll about the view axis in radians. `#[serde(default)]` so views
    /// written before roll existed load as level rather than failing.
    #[serde(default)]
    pub roll: f32,
    /// Orbit radius
    pub distance: f32,
    /// Orbit center / look-at point
    pub focus: [f32; 3],
    /// Added to orbital camera position
    pub offset: [f32; 3],
    pub point_size: f32,
    /// The four values the shader wants, always written. They are derived
    /// from `haze` below when a view is saved; they remain the wire format
    /// because the offline renderer consumes them directly and because views
    /// written before the single-control rework carry nothing else.
    ///
    /// The `fog_*` aliases are what these were called when the effect
    /// darkened distant material rather than thinning it (see `src/haze.rs`);
    /// old view files still load. `haze_transmittance` in particular *means*
    /// something different from the `fog_brightness` it reads, but it ran over
    /// the same range from the same control, so the framing is recovered.
    #[serde(alias = "fog_near")]
    pub haze_near: f32,
    #[serde(alias = "fog_far")]
    pub haze_far: f32,
    #[serde(alias = "fog_brightness")]
    pub haze_transmittance: f32,
    #[serde(alias = "fog_saturation")]
    pub haze_saturation: f32,
    /// Haze amount 0–1 (see `crate::haze`). `None` on views saved before the
    /// single-control rework — `App::apply_view` recovers an equivalent
    /// amount from `haze_transmittance` in that case.
    #[serde(default, alias = "fog", skip_serializing_if = "Option::is_none")]
    pub haze: Option<f32>,
    /// Whether the haze band above was pinned by hand rather than auto-ranged
    /// off the camera distance. Only meaningful alongside `haze`.
    #[serde(default, alias = "fog_band_pinned", skip_serializing_if = "std::ops::Not::not")]
    pub haze_band_pinned: bool,
    /// Scale-aware color accumulation exponent; None = use the scene's value
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_falloff: Option<f32>,
    /// Cyclic colormap contrast stretch; None = use the scene's value
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_contrast: Option<f32>,
    /// Renderer this view was captured with: "points" (default) or "splat"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<String>,
    /// Splat-renderer exposure (only meaningful with renderer = "splat")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure: Option<f32>,
}

impl View {
    /// Whether this view was captured with the splat renderer
    pub fn is_splat(&self) -> bool {
        self.renderer.as_deref() == Some("splat")
    }
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
            pitch: 0.35,
            roll: -0.4,
            distance: 4.0,
            focus: [0.1, 0.2, 0.3],
            offset: [0.0, 1.0, 0.0],
            point_size: 0.002,
            haze_near: 3.0,
            haze_far: 4.5,
            haze_transmittance: 0.4,
            haze_saturation: 0.3,
            haze: Some(0.68),
            haze_band_pinned: false,
            color_falloff: Some(0.6),
            color_contrast: Some(2.0),
            renderer: Some("splat".to_string()),
            exposure: Some(1.5),
        };
        let dir = std::env::temp_dir().join("fracturize_view_test");
        let path = dir.join("roundtrip.toml");
        view.save(&path).unwrap();
        let loaded = View::load(&path).unwrap();
        assert_eq!(loaded.rotation, view.rotation);
        assert_eq!(loaded.pitch, view.pitch);
        assert_eq!(loaded.roll, view.roll);
        assert_eq!(loaded.focus, view.focus);
        assert_eq!(loaded.scene, view.scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A view written before roll and before the haze rework must still
    /// load: level, and reading its `fog_*` keys into the `haze_*` fields.
    #[test]
    fn pre_roll_view_loads_level() {
        let legacy = r#"
rotation = 1.0
distance = 3.0
focus = [0.0, 0.0, 0.0]
offset = [0.0, 0.0, 0.0]
point_size = 0.002
fog_near = 3.0
fog_far = 4.5
fog_brightness = 0.4
fog_saturation = 0.3
"#;
        let dir = std::env::temp_dir().join("fracturize_view_legacy_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy.toml");
        std::fs::write(&path, legacy).unwrap();
        let v = View::load(&path).unwrap();
        assert_eq!(v.roll, 0.0);
        assert_eq!(v.pitch, 0.0);
        assert_eq!(v.haze, None);
        // The legacy key names still land in the right fields.
        assert_eq!(v.haze_near, 3.0);
        assert_eq!(v.haze_transmittance, 0.4);
        std::fs::remove_dir_all(&dir).ok();
    }
}
