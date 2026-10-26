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
    /// History: every edit, clickable to jump to.
    #[serde(default)]
    pub history_open: bool,
    /// Mutate: the profile, the strength, and the grid of mutations.
    #[serde(default)]
    pub mutate_open: bool,
    /// The Random flames list down the left of the screen.
    #[serde(default)]
    pub random_open: bool,
    /// What the dice may roll (`dice::RandomSettings`).
    #[serde(default)]
    pub random_settings_open: bool,
    #[serde(default)]
    pub camera_open: bool,
    #[serde(default)]
    pub render_open: bool,
    /// The Keybinds window. Lives here with the rest since the UI review: it
    /// was the one window that explains the whole app, and the only one whose
    /// open state evaporated at exit — so a person who found it, used it and
    /// left it open came back to a toolbar of seven unexplained icons.
    #[serde(default)]
    pub help_open: bool,
    /// The scene browser. Same reasoning.
    #[serde(default)]
    pub browser_open: bool,
    /// The Subscenes window (composing several attractors into one scene).
    #[serde(default)]
    pub subscenes_open: bool,
    /// The Xaos window: a layer's whole transition matrix.
    #[serde(default)]
    pub xaos_open: bool,
    /// Collaborate: the scene's thread with an agent.
    #[serde(default)]
    pub collab_open: bool,
    /// The Timeline: the camera and every track on one clock.
    #[serde(default)]
    pub timeline_open: bool,
}

fn default_mutation_grid() -> u32 {
    3
}

fn default_ui_scale() -> f32 {
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
    /// Vertical field of view in degrees, 10-150. `None` = never set one,
    /// take [`crate::camera::DEFAULT_FOV_DEGREES`].
    ///
    /// A preference and not scene data because the right answer depends on
    /// the desk: geometrically it is the angle your screen subtends at your
    /// eye (about 30 degrees at arm's length from a laptop), and people's
    /// taste runs much wider than that — 90 is a common games setting.
    /// Neither number is a property of anybody's artwork.
    ///
    /// Read by the window only. A headless `--render` never consults it — the
    /// same rule [`Self::threads`] follows, and for the same reason: a render
    /// has to be reproducible from flags plus scene, and a lens changes the
    /// picture. `--fov` is how the command line says it deliberately.
    ///
    /// Loses to a scene that states its own `[camera] fov`, which is the
    /// opposite way round from [`Self::point_count`] and deliberately so:
    /// `point_count` describes what *this box* can afford, so the box must
    /// win, while a FoV changes the picture, so a scene that was composed
    /// against one keeps it. `--fov` wins over everything.
    #[serde(default)]
    pub fov: Option<f32>,
    /// Open/closed state of the Phase 2 panel windows
    #[serde(default)]
    pub panels: PanelPrefs,
    /// Where mutation strength lived before it moved into the dice profile
    /// (`random.mutation.strength`). Read once, to carry an old setting over,
    /// and never written again.
    #[serde(default, skip_serializing)]
    pub mutate_strength: Option<f32>,
    /// Multiplier on every panel's size, over and above the window's own scale
    /// factor. egui takes the display's scale factor already; this is the
    /// separate, expected setting for "the panels are too small on this
    /// monitor" — which on a 4K display is a real complaint and had no answer.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    /// What the dice may roll and a mutation may change — the Random
    /// settings window. Follows the person rather than any scene. `--random`
    /// never reads it (a render must be reproducible from flags plus scene);
    /// `--random-config` can point at this file to borrow it deliberately, or
    /// name the profile it was loaded from.
    #[serde(default)]
    pub random: crate::dice::RandomSettings,
    /// The dice profile file `random` was loaded from or last saved to, which
    /// the window compares against to mark it changed. `None` = a built-in
    /// profile, named by `random_profile_builtin`.
    #[serde(default)]
    pub random_profile: Option<PathBuf>,
    /// The built-in profile `random` came from, when `random_profile` is
    /// `None`. `None` here too = `default`.
    #[serde(default)]
    pub random_profile_builtin: Option<String>,
    /// The Mutate window's grid: 3 (eight mutations around the scene) or 5
    /// (twenty-four, the outer ring at twice the strength).
    #[serde(default = "default_mutation_grid")]
    pub mutation_grid: u32,
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
    /// CPU threads a render job's encoders may use. Describes the box, not the
    /// artwork, so it lives here and on the command line and is never scene or
    /// view data — `threads = 16` is actively wrong advice on the laptop, so
    /// nothing should ever *replay* one. `None` = never set one, take
    /// [`crate::render_job::default_threads`] (one less than the machine has).
    #[serde(default)]
    pub threads: Option<usize>,
    /// Fraction of the GPU a render job may take, 0.05-1.0.
    ///
    /// A machine setting for the same reason `threads` is: it describes how
    /// much of *this box* a render is allowed to eat while you keep using it,
    /// which is a fact about the desk it sits on rather than about the artwork.
    ///
    /// `#[serde(default)]` like every field here, and not as a formality:
    /// [`Prefs::load`] falls back to `Default` on *any* parse error, so a field
    /// that cannot cope with an older file does not merely lose its own value
    /// — it silently resets every preference the person has.
    #[serde(default)]
    pub gpu_share: Option<f32>,
    /// Folders added to the scene browser's places, after the built-in ones
    /// (`scene_browser::builtin_places`).
    #[serde(default)]
    pub browser_places: Vec<PathBuf>,
    /// The folder the scene browser was last showing; it reopens there.
    /// `None` = never opened, start in `scenes/`.
    #[serde(default)]
    pub browser_folder: Option<PathBuf>,
    /// How the scene browser orders its list.
    #[serde(default)]
    pub browser_sort: crate::scene_browser::Sort,
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
            fov: None,
            panels: PanelPrefs::default(),
            mutate_strength: None,
            ui_scale: default_ui_scale(),
            random: Default::default(),
            random_profile: None,
            random_profile_builtin: None,
            mutation_grid: default_mutation_grid(),
            author: None,
            point_count: None,
            threads: None,
            gpu_share: None,
            browser_places: Vec::new(),
            browser_folder: None,
            browser_sort: Default::default(),
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
            .and_then(|s| toml::from_str::<Self>(&s).ok())
            .map(Self::migrated)
            .unwrap_or_default()
    }

    /// Prefs as read, brought up to date: hand-edited ranges in order and in
    /// bounds, and a mutation strength from before the dice profile held it
    /// carried into the profile — unless the profile already says otherwise.
    fn migrated(mut self) -> Self {
        if let Some(s) = self.mutate_strength.take() {
            if self.random.mutation.strength == crate::dice::MutationSettings::default().strength {
                self.random.mutation.strength = s;
            }
        }
        self.random = self.random.normalized();
        self
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
        assert_eq!(p.gpu_share, None, "and so does one added later still");
    }

    #[test]
    fn the_fov_preference_is_absent_until_someone_sets_one() {
        // The whole point of `Option` here: an unset preference must not
        // shadow a scene's own `[camera] fov`, and only `None` can express
        // "no opinion" — 45.0 would be an opinion that happens to match the
        // default, and would start winning the moment the default moved.
        assert_eq!(Prefs::default().fov, None);
        let s = toml::to_string(&Prefs::default()).expect("prefs must serialize");
        assert!(!s.contains("fov"), "an unset FoV must not be written: {}", s);

        let p = Prefs { fov: Some(90.0), ..Default::default() };
        let s = toml::to_string(&p).expect("prefs must serialize");
        assert_eq!(toml::from_str::<Prefs>(&s).unwrap().fov, Some(90.0));
    }

    #[test]
    fn the_orbit_style_round_trips_under_the_name_it_is_written_by() {
        let p = Prefs { orbit_style: OrbitStyle::Turntable, ..Default::default() };
        let s = toml::to_string(&p).expect("prefs must serialize");
        assert!(s.contains(r#"orbit_style = "turntable""#), "{}", s);
        assert_eq!(toml::from_str::<Prefs>(&s).unwrap().orbit_style, OrbitStyle::Turntable);
    }

    #[test]
    fn random_settings_round_trip_and_feed_random_config() {
        let mut p = Prefs::default();
        p.random.transforms = [3, 7];
        p.random.lacunarity.on = true;
        p.random.variations.get_mut("spherical").unwrap().enabled = true;
        let s = toml::to_string(&p).expect("prefs must serialize");
        let back: Prefs = toml::from_str(&s).unwrap();
        assert_eq!(back.random, p.random);
        // The prefs file is a valid --random-config.
        let from_file = crate::dice::profile::parse(&s).unwrap();
        assert_eq!(from_file.settings, p.random);
        assert!(from_file.warnings.is_empty(), "{:?}", from_file.warnings);
    }

    /// Mutation strength moved from prefs into the dice profile; an old
    /// setting is carried over once, and then never written back.
    #[test]
    fn an_old_mutation_strength_moves_into_the_profile() {
        let old = "invert_pitch = true\nmutate_strength = 2.5\n";
        let p = toml::from_str::<Prefs>(old).unwrap().migrated();
        assert_eq!(p.random.mutation.strength, 2.5);
        let s = toml::to_string(&p).unwrap();
        assert!(!s.contains("\nmutate_strength"), "the old key must not be written: {s}");
        // A profile that already states a strength keeps it.
        let both = "mutate_strength = 2.5\n[random.mutation]\nstrength = 0.5\n";
        assert_eq!(toml::from_str::<Prefs>(both).unwrap().migrated().random.mutation.strength, 0.5);
    }

}
