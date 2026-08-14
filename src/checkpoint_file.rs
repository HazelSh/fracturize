//! The accumulation checkpoint: a render's histogram, saved so a long run can
//! be resumed or extended instead of restarted.
//!
//! ## How this differs from `grade_file`
//!
//! Two artifacts, two jobs, and they are not interchangeable:
//!
//! * A **grade buffer** (`src/grade_file.rs`) is the tonemap's *input* —
//!   output-sized, post-filter, 16 bytes a pixel. It re-grades exactly and
//!   cheaply, and that is all it can do.
//! * A **checkpoint** is the accumulation histogram itself — *supersampled*
//!   size, 32 bytes a texel, so 8x larger at `--supersample 2`. It can do
//!   everything the grade buffer can and also **keep accumulating**, because
//!   it is the state the chaos game was adding into.
//!
//! Opt-in for that reason. 265 MB at 1080p / 2x is not a thing to write after
//! every render, but it is a very reasonable thing to write after an hour of
//! one.
//!
//! ## Written on abort, deliberately
//!
//! Hazel's call: aborting a `--checkpoint` render **commits the histogram to
//! disk** rather than discarding it. That is the whole point of the feature —
//! an interrupted render you cannot resume is just a slower failure. So the
//! write happens however the run ends: completion, `--spp` target reached, or
//! cancellation partway through.
//!
//! ## Format
//!
//! ```text
//! fracturize-checkpoint 1\n   magic and format version
//! <decimal length>\n          bytes of TOML header that follow
//! <TOML header>               [checkpoint] + the whole render record
//! <raw histogram>             width*height*8 u32, little-endian
//! ```
//!
//! Same shape as `.fgrade` on purpose — one idiom, one parser to reason about,
//! and `head -c 2000` tells you what a file is either way.
//!
//! ## What must match to resume
//!
//! A checkpoint is only meaningful under the geometry and the scene that
//! produced it: adding samples of a *different* attractor into an existing
//! histogram silently blends two pictures. So the header carries the scene
//! hash and the render geometry, and [`Checkpoint::check_compatible`] refuses
//! a mismatch by name rather than producing a quiet double exposure.

use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// Magic line. The trailing number is the *format* version.
const MAGIC: &str = "fracturize-checkpoint 1";

/// Cap on the declared header length, so a corrupt file cannot make this
/// allocate wildly before anything is validated.
const MAX_HEADER_BYTES: usize = 1 << 20;

/// A saved accumulation histogram plus what it takes to keep adding to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    /// **Accumulation** width and height — the supersampled size, not the
    /// output size. Getting this wrong is the difference between resuming and
    /// reinterpreting the buffer.
    pub width: u32,
    pub height: u32,
    /// Output size, so a resume can rebuild the same targets without guessing.
    pub out_width: u32,
    pub out_height: u32,
    pub supersample: u32,
    /// Samples already folded in. Exposure is normalized by this, so it has to
    /// survive the round trip exactly or a resumed render changes brightness
    /// at the join.
    pub samples: f64,
    /// Laps already run, for reporting.
    pub laps: u32,
    /// SHA-256 of the scene as rendered. A resume against a different scene is
    /// a silent double exposure, so this is checked, not decorative.
    pub scene_sha256: String,
    /// The raw histogram: `width * height * 8` u32s, little-endian lo/hi pairs.
    pub words: Vec<u32>,
}

impl Checkpoint {
    /// The conventional path beside a render: `out.png` -> `out.fhist`.
    pub fn path_for(png: &Path) -> PathBuf {
        png.with_extension("fhist")
    }

    /// Words per texel, mirroring `WORDS_PER_TEXEL` in accumulate.wgsl.
    pub const WORDS_PER_TEXEL: usize = 8;

    pub fn expected_words(&self) -> usize {
        self.width as usize * self.height as usize * Self::WORDS_PER_TEXEL
    }

    /// Refuse to resume into something that is not the same render.
    ///
    /// Each of these would otherwise fail silently and plausibly: a geometry
    /// mismatch reinterprets the buffer as a differently-shaped image, and a
    /// scene mismatch blends two attractors into one histogram at whatever
    /// ratio the sample counts happened to land on.
    pub fn check_compatible(
        &self,
        out_width: u32,
        out_height: u32,
        supersample: u32,
        scene_sha256: &str,
    ) -> Result<(), String> {
        if (self.out_width, self.out_height) != (out_width, out_height) {
            return Err(format!(
                "checkpoint is {}x{} but this render is {}x{} — resume at the original size",
                self.out_width, self.out_height, out_width, out_height
            ));
        }
        if self.supersample != supersample {
            return Err(format!(
                "checkpoint used --supersample {} but this render uses {} — the histogram \
                 is stored at the supersampled size, so the two cannot be added",
                self.supersample, supersample
            ));
        }
        if self.scene_sha256 != scene_sha256 {
            return Err(format!(
                "checkpoint was accumulated from a different scene (sha {}… vs {}…). \
                 Resuming would blend two attractors into one histogram.",
                &self.scene_sha256[..8.min(self.scene_sha256.len())],
                &scene_sha256[..8.min(scene_sha256.len())],
            ));
        }
        Ok(())
    }

    pub fn write(
        &self,
        path: &Path,
        record: Option<&crate::record::RenderRecord>,
    ) -> Result<(), String> {
        if self.words.len() != self.expected_words() {
            return Err(format!(
                "checkpoint is {}x{} but holds {} words, not {}",
                self.width,
                self.height,
                self.words.len(),
                self.expected_words()
            ));
        }
        let header = self.header_toml(record);
        let file = std::fs::File::create(path)
            .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
        let mut w = BufWriter::new(file);
        let mut put = |b: &[u8]| -> Result<(), String> {
            w.write_all(b).map_err(|e| format!("writing {}: {}", path.display(), e))
        };
        put(MAGIC.as_bytes())?;
        put(b"\n")?;
        put(format!("{}\n", header.len()).as_bytes())?;
        put(header.as_bytes())?;
        // Chunked rather than one big Vec: at 2x supersampled 1080p this body
        // is 265 MB, and building a second copy of it in memory to write it
        // would be the largest allocation in the program for no reason.
        let mut buf = Vec::with_capacity(1 << 16);
        for chunk in self.words.chunks(1 << 14) {
            buf.clear();
            for v in chunk {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            put(&buf)?;
        }
        w.flush().map_err(|e| format!("writing {}: {}", path.display(), e))?;
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let mut file = std::fs::File::open(path)
            .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        let mut all = Vec::new();
        file.read_to_end(&mut all)
            .map_err(|e| format!("reading {}: {}", path.display(), e))?;

        let rest = all
            .strip_prefix(MAGIC.as_bytes())
            .and_then(|r| r.strip_prefix(b"\n"))
            .ok_or_else(|| format!("{} is not a fracturize checkpoint", path.display()))?;
        let nl = rest
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| format!("{}: truncated header length", path.display()))?;
        let len: usize = std::str::from_utf8(&rest[..nl])
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| format!("{}: unreadable header length", path.display()))?;
        if len > MAX_HEADER_BYTES {
            return Err(format!("{}: header claims {} bytes", path.display(), len));
        }
        let after = &rest[nl + 1..];
        if after.len() < len {
            return Err(format!("{}: truncated header", path.display()));
        }
        let header = std::str::from_utf8(&after[..len])
            .map_err(|_| format!("{}: header is not UTF-8", path.display()))?;
        let body = &after[len..];

        let mut c = Self::from_header(header, path)?;
        if body.len() != c.expected_words() * 4 {
            return Err(format!(
                "{}: header says {}x{} ({} bytes of histogram) but the file has {}",
                path.display(),
                c.width,
                c.height,
                c.expected_words() * 4,
                body.len()
            ));
        }
        c.words = body
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        Ok(c)
    }

    fn header_toml(&self, record: Option<&crate::record::RenderRecord>) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(s, "# fracturize accumulation checkpoint — a render's histogram.");
        let _ = writeln!(
            s,
            "# Keep accumulating with `fracturize -s <scene> --splat --resume <this file> \\"
        );
        let _ = writeln!(s, "#   --spp <more> -r out.png`.");
        let _ = writeln!(
            s,
            "# After this header are {} u32 little-endian, {} words per texel.",
            self.expected_words(),
            Self::WORDS_PER_TEXEL
        );
        let _ = writeln!(s);
        let _ = writeln!(s, "[checkpoint]");
        let _ = writeln!(s, "width = {}", self.width);
        let _ = writeln!(s, "height = {}", self.height);
        let _ = writeln!(s, "out_width = {}", self.out_width);
        let _ = writeln!(s, "out_height = {}", self.out_height);
        let _ = writeln!(s, "supersample = {}", self.supersample);
        // Full precision: exposure divides by this, so a rounded round trip
        // would change brightness at the join of a resumed render.
        let _ = writeln!(s, "samples = {:?}", self.samples);
        let _ = writeln!(s, "laps = {}", self.laps);
        let _ = writeln!(s, "scene_sha256 = \"{}\"", self.scene_sha256);
        if let Some(r) = record {
            let _ = writeln!(s);
            let _ = writeln!(s, "{}", r.to_toml());
        }
        s
    }

    fn from_header(header: &str, path: &Path) -> Result<Self, String> {
        let mut in_block = false;
        let mut f = std::collections::HashMap::new();
        for line in header.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_block = line == "[checkpoint]";
                continue;
            }
            if !in_block || line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                f.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        let need = |key: &str| -> Result<String, String> {
            f.get(key)
                .cloned()
                .ok_or_else(|| format!("{}: header has no `{}`", path.display(), key))
        };
        let num = |key: &str| -> Result<f64, String> {
            need(key)?
                .parse::<f64>()
                .map_err(|_| format!("{}: `{}` is not a number", path.display(), key))
        };
        Ok(Checkpoint {
            width: num("width")? as u32,
            height: num("height")? as u32,
            out_width: num("out_width")? as u32,
            out_height: num("out_height")? as u32,
            supersample: num("supersample")? as u32,
            samples: num("samples")?,
            laps: num("laps")? as u32,
            scene_sha256: need("scene_sha256")?.trim_matches('"').to_string(),
            words: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Checkpoint {
        Checkpoint {
            width: 6,
            height: 4,
            out_width: 3,
            out_height: 2,
            supersample: 2,
            samples: 9.87654321e12,
            laps: 137,
            scene_sha256: "abc123def456".into(),
            words: (0..6 * 4 * Checkpoint::WORDS_PER_TEXEL).map(|i| i as u32 * 7).collect(),
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("fracturize-ckpt-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("c.fhist")
    }

    #[test]
    fn a_checkpoint_round_trips() {
        let c = sample();
        let p = tmp("roundtrip");
        c.write(&p, None).unwrap();
        assert_eq!(Checkpoint::read(&p).unwrap(), c);
        let _ = std::fs::remove_file(&p);
    }

    /// Exposure divides by the sample count, so a rounded round trip would
    /// shift brightness at the join of a resumed render — the kind of seam
    /// nobody would trace back to a text header.
    #[test]
    fn the_sample_count_survives_exactly() {
        let mut c = sample();
        c.samples = 8_123_456_789_012.0 + 0.25;
        let p = tmp("samples");
        c.write(&p, None).unwrap();
        assert_eq!(Checkpoint::read(&p).unwrap().samples, c.samples);
        let _ = std::fs::remove_file(&p);
    }

    /// Every mismatch here is one that would otherwise fail silently and
    /// plausibly, so each gets named rather than lumped together.
    #[test]
    fn resuming_into_a_different_render_is_refused() {
        let c = sample();
        assert!(c.check_compatible(3, 2, 2, "abc123def456").is_ok());

        let size = c.check_compatible(4, 2, 2, "abc123def456").unwrap_err();
        assert!(size.contains("resume at the original size"), "{}", size);

        let ss = c.check_compatible(3, 2, 4, "abc123def456").unwrap_err();
        assert!(ss.contains("supersample"), "{}", ss);

        let scene = c.check_compatible(3, 2, 2, "999999999999").unwrap_err();
        assert!(scene.contains("different scene"), "{}", scene);
        // And it must say *why* that matters, not just that it differs.
        assert!(scene.contains("blend two attractors"), "{}", scene);
    }

    #[test]
    fn a_file_that_is_not_one_is_refused_by_name() {
        let p = tmp("bogus");
        std::fs::write(&p, b"fracturize-grade 1\n12\nnot this one").unwrap();
        let e = Checkpoint::read(&p).unwrap_err();
        assert!(e.contains("not a fracturize checkpoint"), "{}", e);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_truncated_file_is_caught() {
        let c = sample();
        let p = tmp("truncated");
        c.write(&p, None).unwrap();
        let mut raw = std::fs::read(&p).unwrap();
        raw.truncate(raw.len() - 12);
        std::fs::write(&p, &raw).unwrap();
        let e = Checkpoint::read(&p).unwrap_err();
        assert!(e.contains("but the file has"), "{}", e);
        let _ = std::fs::remove_file(&p);
    }

    /// The header leads the file so `head` describes it, same as `.fgrade`.
    #[test]
    fn the_header_leads_the_file_and_says_how_to_resume() {
        let c = sample();
        let p = tmp("header");
        c.write(&p, None).unwrap();
        let raw = std::fs::read(&p).unwrap();
        let head = String::from_utf8_lossy(&raw[..2000.min(raw.len())]).to_string();
        assert!(head.starts_with("fracturize-checkpoint 1\n"));
        assert!(head.contains("[checkpoint]"));
        assert!(head.contains("--resume"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn the_conventional_path_sits_beside_the_png() {
        assert_eq!(
            Checkpoint::path_for(Path::new("renders/a.png")),
            PathBuf::from("renders/a.fhist")
        );
    }

    /// The stride is one fact in two languages and WGSL is opaque to the
    /// compiler, the same arrangement the kernel numbering uses.
    #[test]
    fn the_shader_agrees_about_the_texel_stride() {
        let wgsl = include_str!("../shaders/points/accumulate.wgsl");
        assert!(
            wgsl.contains(&format!(
                "const WORDS_PER_TEXEL: u32 = {}u;",
                Checkpoint::WORDS_PER_TEXEL
            )),
            "accumulate.wgsl must declare WORDS_PER_TEXEL = {}",
            Checkpoint::WORDS_PER_TEXEL
        );
    }
}
