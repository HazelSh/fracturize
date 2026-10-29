//! Render-job presets: how big and how good a render is, by name.
//!
//! Apophysis's render dialog has a preset list (`Render presets.av`: a name,
//! the size, the quality, the filter radius, the oversample and the file
//! format), and anyone who rendered with it has a handful they reach for —
//! a quick check, a wallpaper, a print. This is that, in this program's terms:
//! the output kind, the size, the samples, supersampling and its filter,
//! density estimation, the PNG's depth, and a clip's frame rate and quality.
//!
//! **What a preset leaves alone is as deliberate as what it sets.** It never
//! carries the *look* — the renderer, exposure, the grade — because those are
//! the artwork's, set in the window and inherited by the job; nor the machine
//! settings (GPU share, threads), which describe the box and follow it. A
//! preset that switched a splat piece to the points renderer would be
//! changing the picture under the name of rendering it bigger.
//!
//! Every key is optional, and one left out leaves the dialog's value as it is.
//! A preset saved from the dialog writes them all; the built-ins write all
//! but `points`, which they leave to the dialog's seeding from the window.
//!
//! Built-ins are compiled from `src/render_presets/`; the person's own live in
//! `$XDG_CONFIG_HOME/fracturize/render-presets/<name>.toml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The preset a first render starts from: the dialog's long-standing default.
pub const DEFAULT: &str = "preview";

/// The built-ins, in the order the picker lists them, cheapest first.
pub const BUILTINS: &[(&str, &str)] = &[
    ("quick-look", include_str!("render_presets/quick-look.toml")),
    ("preview", include_str!("render_presets/preview.toml")),
    ("hd", include_str!("render_presets/hd.toml")),
    ("4k", include_str!("render_presets/4k.toml")),
    ("4k-fine", include_str!("render_presets/4k-fine.toml")),
    ("print-a3", include_str!("render_presets/print-a3.toml")),
    ("print-a2", include_str!("render_presets/print-a2.toml")),
    ("clip-preview", include_str!("render_presets/clip-preview.toml")),
    ("clip-hd", include_str!("render_presets/clip-hd.toml")),
];

/// What a preset renders. A view file has no settings, so it isn't one.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Output {
    Still,
    Avif,
    Mp4,
}

/// How a still gathers its samples: the dialog's accumulate / one pass.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Samples {
    Accumulate,
    OnePass,
}

/// One preset, as its file says it. See the module doc for what is left out.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    /// One line, shown under the picker and on hover.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Output>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samples: Option<Samples>,
    /// Samples per output pixel, when accumulating.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spp: Option<u32>,
    /// Extra rounds of the chaos game, for one pass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rounds: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersample: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<crate::gpu::Filter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_radius: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub density_estimation: Option<f64>,
    /// 8 or 16.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    /// The codec's quality, 0-100.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<u8>,
}

impl Preset {
    /// Values the dialog could not take, named. Checked on every read, so a
    /// hand-edited file says what is wrong with it instead of loading a size
    /// the dialog would refuse.
    fn check(&self) -> Result<(), String> {
        use crate::gpu::points::downsample::{MAX_FILTER_RADIUS, MAX_SUPERSAMPLE, MIN_FILTER_RADIUS};
        let within = |name: &str, v: f64, lo: f64, hi: f64| {
            if (lo..=hi).contains(&v) { Ok(()) } else { Err(format!("{name} = {v} is outside {lo} to {hi}")) }
        };
        let opt = |name: &str, v: Option<f64>, lo: f64, hi: f64| v.map_or(Ok(()), |v| within(name, v, lo, hi));
        opt("width", self.width.map(f64::from), 16.0, 16384.0)?;
        opt("height", self.height.map(f64::from), 16.0, 16384.0)?;
        opt("spp", self.spp.map(f64::from), 1.0, 1e7)?;
        opt("rounds", self.rounds.map(f64::from), 1.0, 512.0)?;
        opt("points", self.points.map(|p| p as f64), 100_000.0, 4e9)?;
        opt("supersample", self.supersample.map(f64::from), 1.0, MAX_SUPERSAMPLE as f64)?;
        opt("filter_radius", self.filter_radius, MIN_FILTER_RADIUS as f64, MAX_FILTER_RADIUS as f64)?;
        opt("density_estimation", self.density_estimation, 0.0, 1.0)?;
        opt("fps", self.fps.map(f64::from), 1.0, 120.0)?;
        opt("quality", self.quality.map(f64::from), 0.0, 100.0)?;
        match self.bit_depth {
            None | Some(8) | Some(16) => Ok(()),
            Some(b) => Err(format!("bit_depth = {b}: it is 8 or 16")),
        }
    }
}

/// Read a preset file's text.
pub fn parse(text: &str) -> Result<Preset, String> {
    let preset: Preset = toml::from_str(text).map_err(|e| e.message().to_string())?;
    preset.check()?;
    Ok(preset)
}

const HEADER: &str = "\
# A Fracturize render-job preset: how big and how good a render is.
# Every key is optional; one left out leaves the dialog's value alone.
# output: still | avif | mp4. samples: accumulate (to spp per pixel) or
# one-pass (points, plus `rounds` extra rounds). bit_depth: 8 or 16 (stills).
";

/// A preset as a file.
pub fn to_toml_string(preset: &Preset) -> String {
    let body = toml::to_string(preset).expect("a preset is plain values");
    format!("{HEADER}\n{body}")
}

/// The person's own presets: `$XDG_CONFIG_HOME/fracturize/render-presets`.
pub fn user_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("fracturize").join("render-presets"))
}

/// A preset that can be loaded by name.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    /// `None` for a built-in.
    pub path: Option<PathBuf>,
}

impl Entry {
    pub fn is_builtin(&self) -> bool {
        self.path.is_none()
    }
}

pub fn is_builtin(name: &str) -> bool {
    BUILTINS.iter().any(|(n, _)| *n == name)
}

/// Every preset: the built-ins in their order, then the person's, sorted.
pub fn list() -> Vec<Entry> {
    let mut out: Vec<Entry> = BUILTINS.iter().map(|(n, _)| Entry { name: (*n).into(), path: None }).collect();
    if let Some(Ok(read)) = user_dir().map(std::fs::read_dir) {
        let mut own: Vec<Entry> = read
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .filter_map(|p| Some(Entry { name: p.file_stem()?.to_str()?.to_string(), path: Some(p) }))
            .filter(|e| !is_builtin(&e.name))
            .collect();
        own.sort_by(|a, b| a.name.cmp(&b.name));
        out.extend(own);
    }
    out
}

/// The preset called `name`, if there is one.
pub fn find(name: &str) -> Option<Entry> {
    list().into_iter().find(|e| e.name == name)
}

pub fn load(entry: &Entry) -> Result<Preset, String> {
    match &entry.path {
        None => {
            let text = BUILTINS
                .iter()
                .find(|(n, _)| *n == entry.name)
                .map(|(_, t)| *t)
                .ok_or_else(|| format!("no built-in preset '{}'", entry.name))?;
            parse(text).map_err(|e| format!("built-in {}: {e}", entry.name))
        }
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            parse(&text).map_err(|e| format!("{}: {e}", path.display()))
        }
    }
}

/// A name a preset can be saved under, tidied (spaces become dashes), or why
/// not. The name is the file's, so it's what can be in one.
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim().replace(' ', "-");
    if name.is_empty() {
        return Err("a preset needs a name".into());
    }
    if is_builtin(&name) {
        return Err(format!("'{name}' is a built-in preset; pick another name"));
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.') || name.starts_with('.') {
        return Err("letters, digits, '-', '_' and '.' only".into());
    }
    Ok(name)
}

/// Write `preset` as the person's own preset called `name`, over any there.
pub fn save(name: &str, preset: &Preset) -> Result<Entry, String> {
    let name = check_name(name)?;
    let dir = user_dir().ok_or("no config folder: neither XDG_CONFIG_HOME nor HOME is set")?;
    save_in(&dir, &name, preset)
}

fn save_in(dir: &Path, name: &str, preset: &Preset) -> Result<Entry, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.toml"));
    std::fs::write(&path, to_toml_string(preset)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Entry { name: name.into(), path: Some(path) })
}

/// Remove one of the person's presets. A built-in can't be.
pub fn delete(entry: &Entry) -> Result<(), String> {
    let path = entry.path.as_ref().ok_or("a built-in preset can't be deleted")?;
    std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A built-in that doesn't read would show up as an error in the picker,
    /// the first time someone opened it.
    #[test]
    fn every_builtin_reads_and_says_what_it_is() {
        for (name, text) in BUILTINS {
            let p = parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!p.description.is_empty(), "{name} has no description");
            assert!(p.output.is_some() && p.width.is_some() && p.height.is_some(), "{name} is a whole recipe");
            assert!(check_name(name).is_err(), "{name} can't be saved over");
        }
        assert!(is_builtin(DEFAULT));
    }

    #[test]
    fn a_saved_preset_reads_back_the_same() {
        let p = Preset {
            description: "mine".into(),
            output: Some(Output::Mp4),
            width: Some(1080),
            height: Some(1350),
            samples: Some(Samples::OnePass),
            spp: Some(250),
            rounds: Some(96),
            points: Some(30_000_000),
            supersample: Some(3),
            filter: Some(crate::gpu::Filter::Mitchell),
            filter_radius: Some(0.75),
            density_estimation: Some(0.3),
            bit_depth: Some(16),
            fps: Some(24),
            quality: Some(70),
        };
        let dir = std::env::temp_dir().join(format!("fracturize-presets-{}", std::process::id()));
        let entry = save_in(&dir, "mine", &p).unwrap();
        assert_eq!(load(&entry).unwrap(), p);
        delete(&entry).unwrap();
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn a_bad_file_says_what_is_wrong() {
        assert!(parse("widht = 100").unwrap_err().contains("widht"));
        assert!(parse("bit_depth = 12").unwrap_err().contains("8 or 16"));
        assert!(parse("supersample = 99").unwrap_err().contains("supersample"));
        assert_eq!(parse("").unwrap(), Preset::default());
    }
}
