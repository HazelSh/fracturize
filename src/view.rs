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
    /// Orbit angle in radians.
    ///
    /// Called `rotation` here and `yaw` everywhere else — the scene format,
    /// the CLI flags, `OrbitCamera`. That is a trap worth only one person
    /// falling into, so `yaw` is accepted as an alias. Saving still writes
    /// `rotation`, so every view already on disk round-trips unchanged.
    #[serde(alias = "yaw")]
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
    /// Added to orbital camera position. Legacy: folded into yaw/pitch/
    /// distance at load (`OrbitCamera::from_legacy`) and zero in anything
    /// saved since. Defaulted so a hand-written view needn't carry it.
    #[serde(default)]
    pub offset: [f32; 3],
    #[serde(default = "default_view_point_size")]
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
    #[serde(alias = "fog_near", default = "default_haze_near")]
    pub haze_near: f32,
    #[serde(alias = "fog_far", default = "default_haze_far")]
    pub haze_far: f32,
    #[serde(alias = "fog_brightness", default = "default_haze_transmittance")]
    pub haze_transmittance: f32,
    #[serde(alias = "fog_saturation", default = "default_haze_saturation")]
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
    /// Tonemap grade, alongside `exposure` and for the same reason.
    ///
    /// These live on the **view** and not on the scene deliberately. A scene is
    /// the three-dimensional thing you explore; a view is "save the state I
    /// want this re-rendered from", and a grade is exactly that kind of state —
    /// it changes nothing about what the attractor *is*, only how its density
    /// becomes a picture. Putting it in the scene would also mean a grade
    /// found while rendering could clobber a scene file mid-exploration.
    ///
    /// `None` means "unset", which resolves to [`Grade::NEUTRAL`] — so every
    /// view already on disk keeps rendering exactly as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamma: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamma_threshold: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vibrancy: Option<f32>,
}

fn default_view_point_size() -> f32 {
    0.002
}

// A saved view always writes the haze block; these defaults exist only so a
// view written by *hand* — which is now a reasonable thing to do, see the note
// on `rotation` — can be four lines of camera and nothing else.
fn default_haze_near() -> f32 {
    3.0
}
fn default_haze_far() -> f32 {
    4.5
}
fn default_haze_transmittance() -> f32 {
    1.0
}
fn default_haze_saturation() -> f32 {
    1.0
}

impl View {
    /// Whether this view was captured with the splat renderer
    pub fn is_splat(&self) -> bool {
        self.renderer.as_deref() == Some("splat")
    }

    /// The grade this view asks for, field by field, falling back to neutral.
    ///
    /// Per field rather than all-or-nothing: a hand-written view that sets only
    /// `gamma` should get the neutral threshold and vibrancy, not be treated as
    /// having no grade at all.
    pub fn grade(&self) -> crate::gpu::points::splat::Grade {
        let n = crate::gpu::points::splat::Grade::NEUTRAL;
        crate::gpu::points::splat::Grade {
            gamma: self.gamma.unwrap_or(n.gamma),
            gamma_threshold: self.gamma_threshold.unwrap_or(n.gamma_threshold),
            vibrancy: self.vibrancy.unwrap_or(n.vibrancy),
        }
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

    /// A view with no grade keys must load as neutral, not as anything else:
    /// every view already on disk predates grading, and they have to keep
    /// rendering exactly as they did.
    #[test]
    fn a_view_without_a_grade_is_neutral() {
        let bare = "rotation = 1.0\ndistance = 3.0\nfocus = [0.0, 0.0, 0.0]\n";
        let v: View = toml::from_str(bare).expect("a four-line view is valid");
        assert_eq!(v.grade(), crate::gpu::points::splat::Grade::NEUTRAL);
    }

    /// Per-field fallback: setting one knob by hand must not drag the others
    /// away from neutral.
    #[test]
    fn a_partial_grade_fills_the_rest_from_neutral() {
        let one = "rotation = 1.0\ndistance = 3.0\nfocus = [0.0, 0.0, 0.0]\ngamma = 2.5\n";
        let v: View = toml::from_str(one).expect("valid");
        let n = crate::gpu::points::splat::Grade::NEUTRAL;
        assert_eq!(v.grade().gamma, 2.5);
        assert_eq!(v.grade().gamma_threshold, n.gamma_threshold);
        assert_eq!(v.grade().vibrancy, n.vibrancy);
    }

    /// An ungraded view must not gain grade keys when it is saved, or every
    /// existing view file would grow three lines on its next round trip.
    #[test]
    fn saving_an_ungraded_view_writes_no_grade_keys() {
        let bare = "rotation = 1.0\ndistance = 3.0\nfocus = [0.0, 0.0, 0.0]\n";
        let v: View = toml::from_str(bare).unwrap();
        let out = toml::to_string_pretty(&v).unwrap();
        for key in ["gamma", "gamma_threshold", "vibrancy"] {
            assert!(!out.contains(key), "`{}` leaked into an ungraded view:\n{}", key, out);
        }
    }

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
            gamma: Some(2.5),
            gamma_threshold: Some(0.35),
            vibrancy: Some(0.8),
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

    /// The minimum a person can reasonably type: four lines of camera, with
    /// the scene-format spelling of yaw. Everything else defaults.
    #[test]
    fn a_hand_written_view_is_four_lines() {
        let dir = std::env::temp_dir().join("fracturize_view_minimal_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("minimal.toml");
        std::fs::write(
            &path,
            "yaw = 1.2\npitch = 0.3\ndistance = 2.5\nfocus = [0.0, 0.5, 0.0]\n",
        )
        .unwrap();
        let v = View::load(&path).unwrap();
        assert_eq!(v.rotation, 1.2);
        assert_eq!(v.focus, [0.0, 0.5, 0.0]);
        assert_eq!(v.offset, [0.0; 3]);
        // No haze asked for, so none applied
        assert_eq!(v.haze_transmittance, 1.0);
        assert_eq!(v.haze_saturation, 1.0);
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
