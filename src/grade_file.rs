//! The grade buffer: a finished render's *linear density*, saved so the
//! tonemap can be redone without re-rendering.
//!
//! ## Why this and not the histogram
//!
//! The tonemap is a pure function of one texture plus a handful of scalars: it
//! reads output-sized linear density and writes pixels. It runs *after* the
//! reconstruction filter, so the filtered, output-sized buffer is exactly its
//! input — and that buffer is 8x smaller than the supersampled histogram
//! behind it (33 MB at 1080p against 265 MB at 2x supersampling; 1.06 GB at 4K).
//!
//! So there are two artifacts for two jobs, and this is the small one:
//!
//! * **This file** re-grades: exposure, gamma, threshold, vibrancy, background.
//!   Exact, because it is bit-for-bit the tonemap's input.
//! * A **histogram checkpoint** would additionally re-filter, change the
//!   supersampling, or resume accumulating. Much larger, and opt-in.
//!
//! ## Format
//!
//! ```text
//! fracturize-grade 1\n     magic and format version
//! <decimal length>\n       bytes of TOML header that follow
//! <TOML header>            [grade_buffer] + the whole render record
//! <raw pixels>             width*height*4 f32, little-endian, RGBA
//! ```
//!
//! Private rather than OpenEXR, deliberately: this is a *checkpoint*, not a
//! deliverable, and a private format costs no dependency and can carry the
//! render record verbatim. EXR export is on the roadmap for grading outside
//! fracturize, where interop is the whole point.
//!
//! The header is TOML and comes before the pixels so `head -c 2000` on one of
//! these tells you what it is — the same reason the sidecar exists.

use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::gpu::points::splat::Grade;

/// Magic line. The trailing version is the *format* version, bumped only when
/// the layout changes — not the program version, which is in the header.
const MAGIC: &str = "fracturize-grade 1";

/// A cap on the declared header length, so a corrupt or hostile file cannot
/// make this allocate arbitrarily before a single byte is validated.
const MAX_HEADER_BYTES: usize = 1 << 20;

/// Everything the tonemap needs that is not the pixels.
///
/// `samples` and `screen_height` are here because `exposure_scale` is derived
/// from them — `exposure * K * screen_height^2 / samples` — and re-grading at a
/// *different exposure* has to recompute it. Storing the derived scale instead
/// would freeze the exposure at whatever it was, which defeats the point.
#[derive(Clone, Debug, PartialEq)]
pub struct GradeBuffer {
    pub width: u32,
    pub height: u32,
    /// Samples accumulated behind this buffer — the ring's point count, or the
    /// accumulating path's total.
    pub samples: f64,
    /// The *supersampled* target height the render used. Exposure is
    /// normalized by its square, so an unsupersampled reading of it would
    /// misgrade a supersampled render by N².
    pub screen_height: f32,
    /// Linear RGB the tonemap composites over.
    pub background: [f32; 3],
    pub transparent: bool,
    /// The exposure and grade the buffer was *rendered* with. Not applied on
    /// load — the pixels are pre-tonemap and carry no grade — but recorded so
    /// a re-grade can start from where the render left off rather than from
    /// the defaults.
    pub exposure: f32,
    pub grade: Grade,
    /// RGBA f32, `width * height * 4` long, row-major from the top.
    pub pixels: Vec<f32>,
}

impl GradeBuffer {
    /// The conventional path beside a render: `out.png` -> `out.fgrade`.
    pub fn path_for(png: &Path) -> PathBuf {
        png.with_extension("fgrade")
    }

    pub fn write(&self, path: &Path, record: Option<&crate::record::RenderRecord>) -> Result<(), String> {
        let expected = self.width as usize * self.height as usize * 4;
        if self.pixels.len() != expected {
            return Err(format!(
                "grade buffer is {}x{} but holds {} floats, not {}",
                self.width, self.height, self.pixels.len(), expected
            ));
        }
        let header = self.header_toml(record);
        let file = std::fs::File::create(path)
            .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
        let mut w = BufWriter::new(file);
        let write = |w: &mut BufWriter<std::fs::File>, b: &[u8]| -> Result<(), String> {
            w.write_all(b).map_err(|e| format!("writing {}: {}", path.display(), e))
        };
        write(&mut w, MAGIC.as_bytes())?;
        write(&mut w, b"\n")?;
        write(&mut w, format!("{}\n", header.len()).as_bytes())?;
        write(&mut w, header.as_bytes())?;
        // Little-endian f32, which is every platform this runs on; stated in
        // the format doc rather than assumed silently.
        let mut bytes = Vec::with_capacity(self.pixels.len() * 4);
        for v in &self.pixels {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        write(&mut w, &bytes)?;
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
            .ok_or_else(|| {
                format!("{} is not a fracturize grade buffer", path.display())
            })?;
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
        let after_len = &rest[nl + 1..];
        if after_len.len() < len {
            return Err(format!("{}: truncated header", path.display()));
        }
        let header = std::str::from_utf8(&after_len[..len])
            .map_err(|_| format!("{}: header is not UTF-8", path.display()))?;
        let body = &after_len[len..];

        let mut b = Self::from_header(header, path)?;
        let expected = b.width as usize * b.height as usize * 4;
        if body.len() != expected * 4 {
            return Err(format!(
                "{}: header says {}x{} ({} bytes of pixels) but the file has {}",
                path.display(),
                b.width,
                b.height,
                expected * 4,
                body.len()
            ));
        }
        b.pixels = body
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        Ok(b)
    }

    fn header_toml(&self, record: Option<&crate::record::RenderRecord>) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(s, "# fracturize grade buffer — linear density, pre-tonemap.");
        let _ = writeln!(s, "# Re-grade it with `fracturize --retonemap <this file> -r out.png`.");
        let _ = writeln!(s, "# The pixels after this header are {} f32 RGBA, little-endian.",
            self.width as usize * self.height as usize * 4);
        let _ = writeln!(s);
        let _ = writeln!(s, "[grade_buffer]");
        let _ = writeln!(s, "width = {}", self.width);
        let _ = writeln!(s, "height = {}", self.height);
        // f64 with full precision: exposure is divided by this, so a rounded
        // value would re-grade at a subtly different brightness.
        let _ = writeln!(s, "samples = {:?}", self.samples);
        let _ = writeln!(s, "screen_height = {:?}", self.screen_height);
        let _ = writeln!(
            s,
            "background = [{:?}, {:?}, {:?}]",
            self.background[0], self.background[1], self.background[2]
        );
        let _ = writeln!(s, "transparent = {}", self.transparent);
        let _ = writeln!(s, "exposure = {:?}", self.exposure);
        let _ = writeln!(s, "gamma = {:?}", self.grade.gamma);
        let _ = writeln!(s, "gamma_threshold = {:?}", self.grade.gamma_threshold);
        let _ = writeln!(s, "vibrancy = {:?}", self.grade.vibrancy);
        if let Some(r) = record {
            let _ = writeln!(s);
            let _ = writeln!(s, "{}", r.to_toml());
        }
        s
    }

    /// Parse the `[grade_buffer]` block.
    ///
    /// Hand-rolled rather than through a TOML crate because the header is also
    /// allowed to carry a whole render record after it, and this only wants the
    /// one block — the record is there for a human and for whatever writes the
    /// re-graded PNG's provenance, not for this parser.
    fn from_header(header: &str, path: &Path) -> Result<Self, String> {
        let mut in_block = false;
        let mut f = std::collections::HashMap::new();
        for line in header.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_block = line == "[grade_buffer]";
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
        let bg = need("background")?;
        let parts: Vec<f32> = bg
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        if parts.len() != 3 {
            return Err(format!("{}: `background` is not three numbers", path.display()));
        }
        Ok(GradeBuffer {
            width: num("width")? as u32,
            height: num("height")? as u32,
            samples: num("samples")?,
            screen_height: num("screen_height")? as f32,
            background: [parts[0], parts[1], parts[2]],
            transparent: need("transparent")? == "true",
            exposure: num("exposure")? as f32,
            grade: Grade {
                gamma: num("gamma")? as f32,
                gamma_threshold: num("gamma_threshold")? as f32,
                vibrancy: num("vibrancy")? as f32,
            },
            pixels: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> GradeBuffer {
        GradeBuffer {
            width: 3,
            height: 2,
            samples: 1.234e10,
            screen_height: 2160.0,
            background: [0.01, 0.02, 0.03],
            transparent: false,
            exposure: 1.5,
            grade: Grade { gamma: 2.5, gamma_threshold: 0.35, vibrancy: 0.8 },
            pixels: (0..24).map(|i| i as f32 * 0.125).collect(),
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("fracturize-grade-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("b.fgrade")
    }

    #[test]
    fn a_grade_buffer_round_trips() {
        let b = sample();
        let p = tmp("roundtrip");
        b.write(&p, None).unwrap();
        let back = GradeBuffer::read(&p).unwrap();
        assert_eq!(back, b);
        let _ = std::fs::remove_file(&p);
    }

    /// `samples` divides the exposure, so a rounded round-trip would re-grade
    /// at a subtly different brightness than the render it came from — the
    /// kind of drift nobody would trace back to a text format.
    #[test]
    fn the_sample_count_survives_exactly() {
        let mut b = sample();
        b.samples = 4_123_456_789_012.0 + 0.5;
        let p = tmp("samples");
        b.write(&p, None).unwrap();
        assert_eq!(GradeBuffer::read(&p).unwrap().samples, b.samples);
        let _ = std::fs::remove_file(&p);
    }

    /// The header is meant to be readable with `head`, which is half the
    /// reason it is TOML and comes first.
    #[test]
    fn the_header_leads_the_file_and_says_what_it_is() {
        let b = sample();
        let p = tmp("header");
        b.write(&p, None).unwrap();
        let raw = std::fs::read(&p).unwrap();
        // The claim in the module doc is `head -c 2000`, so that is the
        // window this checks — not a tighter one that would pass by luck.
        let head = String::from_utf8_lossy(&raw[..2000.min(raw.len())]).to_string();
        assert!(head.starts_with("fracturize-grade 1\n"));
        assert!(head.contains("[grade_buffer]"));
        assert!(head.contains("--retonemap"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_file_that_is_not_one_is_refused_by_name() {
        let p = tmp("bogus");
        std::fs::write(&p, b"\x89PNG\r\n\x1a\n and then some").unwrap();
        let e = GradeBuffer::read(&p).unwrap_err();
        assert!(e.contains("not a fracturize grade buffer"), "{}", e);
        let _ = std::fs::remove_file(&p);
    }

    /// A truncated file must say so rather than hand back a short buffer that
    /// would tonemap into a garbled image.
    #[test]
    fn a_truncated_file_is_caught() {
        let b = sample();
        let p = tmp("truncated");
        b.write(&p, None).unwrap();
        let mut raw = std::fs::read(&p).unwrap();
        raw.truncate(raw.len() - 8);
        std::fs::write(&p, &raw).unwrap();
        let e = GradeBuffer::read(&p).unwrap_err();
        assert!(e.contains("but the file has"), "{}", e);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn the_conventional_path_sits_beside_the_png() {
        assert_eq!(
            GradeBuffer::path_for(Path::new("renders/a.png")),
            PathBuf::from("renders/a.fgrade")
        );
    }
}
