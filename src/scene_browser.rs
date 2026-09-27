//! The scene browser's model: places, the folder being shown, and the scenes
//! in it. No UI and no GPU — `ui/browser.rs` draws this, and the thumbnails
//! are `gpu::thumbs`.
//!
//! **A folder is read whole, and every scene in it is parsed.** That is what
//! lets a row show a scene's name and author rather than its filename, and it
//! is what a thumbnail is built from. It is cheap: parsing measured 0.1–1 ms a
//! scene (the slow end is a composite pulling in another file), and a render's
//! embedded scene is read without decoding a pixel (`scene_source`).
//!
//! What counts as a scene is `scene_source`'s call, so the browser, `-s` and a
//! dropped file agree: a `.toml`, or a PNG carrying a `fracturize:scene` chunk.
//! A PNG without one is an ordinary picture and is passed over. A `.toml` that
//! fails to parse is *listed*, with its error, because a file that silently
//! isn't there is harder to fix than one that says what is wrong with it. The
//! exception is a render's `.render.toml` sidecar, which is never a scene.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::scene::Scene;
use crate::scene_source::{self, SourceKind};

/// How the list is ordered. Persisted in prefs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    /// By the scene's name, as the row shows it.
    #[default]
    Name,
    /// Newest first — for "what did the models make overnight".
    Modified,
}

/// A folder listed on the left, under the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
    /// Built in (always listed, can't be removed) rather than added by hand.
    pub builtin: bool,
}

/// The places every install has. `scenes/local-llm/` is where
/// `tools/llm-author` puts each run's final scene.
pub fn builtin_places() -> Vec<Place> {
    vec![
        Place { label: "Scenes".into(), path: PathBuf::from("scenes"), builtin: true },
        Place {
            label: "Local models".into(),
            path: PathBuf::from("scenes/local-llm"),
            builtin: true,
        },
    ]
}

/// Built-in places followed by the ones added by hand, without duplicates.
pub fn places(added: &[PathBuf]) -> Vec<Place> {
    let mut all = builtin_places();
    for path in added {
        if all.iter().any(|p| same_path(&p.path, path)) {
            continue;
        }
        all.push(Place { label: folder_label(path), path: path.clone(), builtin: false });
    }
    all
}

/// A folder's name as a label: its last component, or the whole path when it
/// has none (`/`, `.`).
pub fn folder_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Whether two paths name one file or directory, however they were spelled.
pub fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// One scene in the folder.
pub struct Entry {
    pub path: PathBuf,
    pub kind: SourceKind,
    pub modified: Option<SystemTime>,
    /// The parsed scene, or why it didn't parse.
    pub scene: Result<Scene, String>,
}

impl Entry {
    /// What the row calls it: the scene's own name, else the file's.
    pub fn title(&self) -> String {
        match &self.scene {
            Ok(s) if !s.name.trim().is_empty() => s.name.clone(),
            _ => self.file_name(),
        }
    }

    pub fn author(&self) -> Option<&str> {
        self.scene.as_ref().ok().map(|s| s.author.trim()).filter(|a| !a.is_empty())
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// A folder as the browser shows it.
pub struct Folder {
    pub entries: Vec<Entry>,
}

impl Folder {
    /// Read every scene in `path`, parsed. Subfolders are the tree's business
    /// ([`Folder::subdirs_of`]), read as its nodes open.
    pub fn read(path: &Path, sort: Sort) -> Result<Self, String> {
        let listing = std::fs::read_dir(path)
            .map_err(|e| format!("Couldn't read {}: {}", path.display(), e))?;
        let mut entries = Vec::new();
        for item in listing.filter_map(|e| e.ok()) {
            let Ok(meta) = item.metadata() else { continue };
            if meta.is_file()
                && let Some(entry) = read_entry(&item.path(), meta.modified().ok())
            {
                entries.push(entry);
            }
        }
        let mut folder = Self { entries };
        folder.sort(sort);
        Ok(folder)
    }

    pub fn sort(&mut self, sort: Sort) {
        match sort {
            Sort::Name => self.entries.sort_by(|a, b| {
                a.title()
                    .to_lowercase()
                    .cmp(&b.title().to_lowercase())
                    .then_with(|| a.path.cmp(&b.path))
            }),
            // Unknown times last; ties by name so the order is stable.
            Sort::Modified => self.entries.sort_by(|a, b| {
                b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path))
            }),
        }
    }

    /// Subfolders of `path`, for the tree, which reads them only when a node
    /// is opened.
    pub fn subdirs_of(path: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        dirs.sort_by_key(|d| folder_label(d).to_lowercase());
        dirs
    }
}

/// A file as a browser entry, or `None` when it isn't a scene at all.
fn read_entry(path: &Path, modified: Option<SystemTime>) -> Option<Entry> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let is_toml = name.ends_with(".toml") && !name.ends_with(".render.toml");
    let is_png = name.ends_with(".png");
    if !is_toml && !is_png {
        return None;
    }
    let kind = match scene_source::read_if_scene(path) {
        Ok(Some(source)) => source.kind,
        // A picture that isn't a render: not a scene, not an error.
        Ok(None) => return None,
        Err(e) => {
            return Some(Entry { path: path.to_path_buf(), kind: SourceKind::Toml, modified, scene: Err(e) });
        }
    };
    let scene = Scene::load(path);
    Some(Entry { path: path.to_path_buf(), kind, modified, scene })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir()
            .join(format!("fracturize-browser-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const SCENE: &str = "[meta]\nname = \"NAME\"\nauthor = \"AUTHOR\"\n\n[[transform]]\n\
                         translation = [0.5, 0.0, 0.0]\nscale = 0.5\ncolor = [1.0, 0.5, 0.2]\n\n[[transform]]\n\
                         translation = [-0.5, 0.0, 0.0]\nscale = 0.5\ncolor = [0.2, 0.5, 1.0]\n";

    fn scene(name: &str, author: &str) -> String {
        SCENE.replace("NAME", name).replace("AUTHOR", author)
    }

    #[test]
    fn lists_scenes_and_subfolders_and_passes_over_the_rest() {
        let d = dir("list");
        std::fs::write(d.join("b.toml"), scene("Beta", "Someone")).unwrap();
        std::fs::write(d.join("a.toml"), scene("Alpha", "")).unwrap();
        std::fs::write(d.join("broken.toml"), "[[transform]\n").unwrap();
        std::fs::write(d.join("x.render.toml"), "[render]\n").unwrap();
        std::fs::write(d.join("notes.txt"), "hi").unwrap();
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::create_dir(d.join(".hidden")).unwrap();

        let f = Folder::read(&d, Sort::Name).unwrap();
        let titles: Vec<String> = f.entries.iter().map(|e| e.title()).collect();
        assert_eq!(titles, ["Alpha", "Beta", "broken.toml"]);
        assert!(f.entries[2].scene.is_err(), "a broken scene is listed with its error");
        assert_eq!(f.entries[0].author(), None, "an empty author is no author");
        assert_eq!(f.entries[1].author(), Some("Someone"));
        assert_eq!(Folder::subdirs_of(&d), [d.join("sub")], "hidden folders are left out");
    }

    #[test]
    fn a_render_is_a_scene_and_a_picture_is_not() {
        let d = dir("png");
        let write_png = |path: &Path, text: Option<&str>| {
            let file = std::fs::File::create(path).unwrap();
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 2, 2);
            enc.set_color(png::ColorType::Rgba);
            if let Some(t) = text {
                enc.add_itxt_chunk(crate::record::KEY_SCENE.to_string(), t.to_string()).unwrap();
            }
            enc.write_header().unwrap().write_image_data(&[0u8; 16]).unwrap();
        };
        write_png(&d.join("render.png"), Some(&scene("From A Render", "")));
        write_png(&d.join("photo.png"), None);

        let f = Folder::read(&d, Sort::Name).unwrap();
        assert_eq!(f.entries.len(), 1);
        assert_eq!(f.entries[0].kind, SourceKind::Png);
        assert_eq!(f.entries[0].title(), "From A Render");
    }

    /// How long reading a real folder takes. `cargo test --release
    /// folder_read_timing -- --ignored --nocapture`
    #[test]
    #[ignore = "reads the repo's scenes/; a measurement, not a check"]
    fn folder_read_timing() {
        let t = std::time::Instant::now();
        let f = Folder::read(Path::new("scenes"), Sort::Name).unwrap();
        println!("scenes/: {} entries in {:.1} ms", f.entries.len(), t.elapsed().as_secs_f64() * 1e3);
    }

    #[test]
    fn places_put_the_builtins_first_and_skip_duplicates() {
        let p = places(&[PathBuf::from("scenes"), PathBuf::from("/tmp/elsewhere")]);
        assert_eq!(p.len(), 3);
        assert!(p[0].builtin && p[1].builtin && !p[2].builtin);
        assert_eq!(p[2].label, "elsewhere");
    }
}
