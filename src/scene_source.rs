//! Where a scene's TOML comes from: a `.toml` file, or the `fracturize:scene`
//! chunk every still render embeds (`record.rs`).
//!
//! One reader for both, so anything that takes a scene path — `-s`, the
//! Scenes browser, a file dropped on the window, `--stats` — takes a render
//! too, and the human and command-line interfaces can't disagree about which
//! files are scenes.
//!
//! Cheap by design: a PNG is read up to its first image-data chunk and never
//! decoded, since the renderer writes its text chunks ahead of the pixels. A
//! folder of renders can be listed by name and author without paying for a
//! single image.

use std::io::Read;
use std::path::Path;

/// What kind of file the scene text came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// A scene file: the text is the file.
    Toml,
    /// A render: the text is its embedded `fracturize:scene` chunk. There is
    /// no `.toml` behind it, so nothing should ever be saved back to it.
    Png,
}

/// A scene's TOML text, and where it came from.
#[derive(Clone, Debug)]
pub struct SceneSource {
    pub text: String,
    pub kind: SourceKind,
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// Whether `path` is a PNG, by its first eight bytes rather than its name —
/// the check that stops a save writing TOML over a render.
pub fn is_png(path: impl AsRef<Path>) -> bool {
    let mut head = [0u8; 8];
    std::fs::File::open(path.as_ref())
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| head == PNG_SIGNATURE)
}

/// Read the scene TOML at `path`: the file itself, or a render's embedded
/// scene. Sniffs the content, not the extension, so a render renamed to
/// anything still opens. Doesn't parse the TOML — that's `Scene::load`'s job.
pub fn read(path: impl AsRef<Path>) -> Result<SceneSource, String> {
    let path = path.as_ref();
    let open = || std::fs::File::open(path).map_err(|e| format!("Failed to read scene file: {}", e));
    if is_png(path) {
        // Streamed, not slurped: a poster-sized render is hundreds of
        // megabytes of pixels behind a few kilobytes of text.
        return read_png(path, std::io::BufReader::new(open()?))
            .map(|text| SceneSource { text, kind: SourceKind::Png });
    }
    let mut bytes = Vec::new();
    open()?
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read scene file: {}", e))?;
    let text = String::from_utf8(bytes).map_err(|_| {
        format!("{} isn't a scene: it is neither a TOML file nor a PNG render", path.display())
    })?;
    Ok(SceneSource { text, kind: SourceKind::Toml })
}

fn read_png(path: &Path, file: impl std::io::BufRead + std::io::Seek) -> Result<String, String> {
    // `read_info` stops at the first IDAT, which is past every chunk the
    // renderer writes, so this reads the header and the text and nothing else.
    let reader = png::Decoder::new(file)
        .read_info()
        .map_err(|e| format!("{}: not a readable PNG: {}", path.display(), e))?;
    let chunk = reader
        .info()
        .utf8_text
        .iter()
        .find(|c| c.keyword == crate::record::KEY_SCENE)
        .ok_or_else(|| {
            format!(
                "{} has no scene in it. Only Fracturize renders carry one \
                 (not mutation or sweep sheets, and not images from elsewhere)",
                path.display()
            )
        })?;
    chunk
        .get_text()
        .map_err(|e| format!("{}: the embedded scene is unreadable: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("fracturize-source-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_png(path: &Path, scene: Option<&str>) {
        let file = std::fs::File::create(path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 2, 2);
        enc.set_color(png::ColorType::Rgba);
        if let Some(s) = scene {
            enc.add_itxt_chunk(crate::record::KEY_SCENE.to_string(), s.to_string()).unwrap();
        }
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[0u8; 16]).unwrap();
    }

    const SCENE: &str = "[meta]\nname = \"From a render\"\nauthor = \"Test\"\n\
                         [[transform]]\ntranslation = [0.0, 0.0, 0.0]\nscale = 0.5\ncolor = [1.0, 1.0, 1.0]\n";

    #[test]
    fn a_toml_file_is_its_own_text() {
        let d = dir("toml");
        let p = d.join("s.toml");
        std::fs::write(&p, SCENE).unwrap();
        let src = read(&p).unwrap();
        assert_eq!(src.kind, SourceKind::Toml);
        assert_eq!(src.text, SCENE);
        assert!(!is_png(&p));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_render_gives_up_its_scene_and_loads() {
        let d = dir("png");
        // Named without an extension on purpose: the content decides.
        let p = d.join("render");
        write_png(&p, Some(SCENE));
        let src = read(&p).unwrap();
        assert_eq!(src.kind, SourceKind::Png);
        assert_eq!(src.text, SCENE);
        assert!(is_png(&p));
        let scene = crate::scene::Scene::load(&p).unwrap();
        assert_eq!(scene.name, "From a render");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_png_without_a_scene_says_so() {
        let d = dir("bare");
        let p = d.join("photo.png");
        write_png(&p, None);
        let e = read(&p).unwrap_err();
        assert!(e.contains("has no scene"), "{e}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn anything_else_is_refused() {
        let d = dir("other");
        let p = d.join("x.jpg");
        std::fs::write(&p, [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x80, 0x81]).unwrap();
        let e = read(&p).unwrap_err();
        assert!(e.contains("neither a TOML file nor a PNG"), "{e}");
        assert!(read(d.join("missing.toml")).is_err());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_render_resolves_references_beside_itself() {
        let d = dir("refs");
        let composite = "[meta]\nname = \"c\"\n[[subscene]]\nfile = \"smoke.toml\"\n";
        let p = d.join("render.png");
        write_png(&p, Some(composite));
        let e = crate::scene::Scene::load(&p).err().expect("the reference is missing");
        assert!(e.contains("smoke.toml") && e.contains("came from the PNG"), "{e}");
        std::fs::write(d.join("smoke.toml"), SCENE).unwrap();
        assert!(crate::scene::Scene::load(&p).is_ok());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn saving_never_writes_over_a_render() {
        let d = dir("save");
        let p = d.join("render.png");
        write_png(&p, Some(SCENE));
        let scene = crate::scene::Scene::load(&p).unwrap();
        assert!(scene.save(&p).is_err());
        assert!(read(&p).is_ok_and(|s| s.kind == SourceKind::Png));
        std::fs::remove_dir_all(&d).ok();
    }
}
