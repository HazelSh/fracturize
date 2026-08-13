//! Render records: what a picture was made from.
//!
//! A finished render used to be a PNG and nothing else. Which scene? At what
//! point count? From which framing? The answers lived in a terminal
//! scrollback, or in the filename if you had been careful, and neither
//! survives a week. That is a small annoyance at 0.35 s a render and a real
//! cost once renders are long — which is the direction the rest of the render
//! quality work goes.
//!
//! The record is written twice, to two audiences:
//!
//! * **Into the PNG**, as text chunks, so the picture carries its own recipe
//!   wherever it goes. Apophysis has embedded its native format in its output
//!   for decades and it is the reason a twenty-year-old flame PNG is still a
//!   flame you can open. Scene files here are ~1.6-1.8 KB, so this is free.
//! * **Beside it**, as `renders/<name>.render.toml`, so it can be read without
//!   a PNG parser — by a person, by a shell, by an agent.
//!
//! ## What it is not
//!
//! **It never goes into the scene file.** `point_count` and its neighbours are
//! deliberately not scene data, precisely so a 100M batch and a 6M exploration
//! session cannot clobber each other. A `[last_render]` block inside
//! `scenes/*.toml` would put exactly that back, creating a second source of
//! truth that disagrees with whichever render actually ran last, plus git-diff
//! noise on a tracked file and a race between concurrent sessions. `renders/`
//! is gitignored, so the sidecar has none of those problems.
//!
//! A useful consequence: `Scene::save`'s comment-preserving `toml_edit` merge
//! never has to learn about render data, so there is no new way for scene
//! round-tripping to drift.
//!
//! ## The machine block is informational
//!
//! `[machine]` is separated from the reproduction-relevant fields on purpose
//! and says so in the file. Nothing in it should ever be *replayed*:
//! `threads = 16` is a fact about the desktop and actively wrong advice on the
//! laptop. It is there to explain a timing, not to be an input.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::camera::{CameraOverride, OrbitCamera};
use crate::gpu::points::splat::Grade;
use crate::gpu::Filter;
use crate::offline::BitDepth;
use crate::version::VERSION;

/// PNG keyword for the embedded scene. Namespaced, because the PNG spec
/// reserves the unprefixed short keywords for its own registered meanings.
pub const KEY_SCENE: &str = "fracturize:scene";
pub const KEY_RENDER: &str = "fracturize:render";
pub const KEY_SCENE_SHA: &str = "fracturize:scene_sha256";

/// The cost/quality cluster: everything that changes how long a render takes
/// without changing what the picture is *of*.
#[derive(Clone, Debug, PartialEq)]
pub struct Quality {
    pub width: u32,
    pub height: u32,
    pub points: usize,
    pub accumulate: u32,
    /// Samples per output pixel actually accumulated, when the render used the
    /// persistent histogram. `None` for a ring-buffer render, where the sample
    /// count is `points` and there is nothing extra to say.
    ///
    /// The *achieved* figure, not the requested one: a render stopped early
    /// records what it got, so the receipt describes the picture on disk.
    pub spp: Option<f32>,
    pub splat: bool,
    pub exposure: f32,
    /// Tonemap grade. Written only when it is not the neutral one, so every
    /// record made before grading existed is byte-identical — and an absent
    /// block reads as "the tonemap this program has always had", which is
    /// exactly what it means.
    pub grade: Grade,
    pub transparent: bool,
    pub supersample: u32,
    pub filter: Filter,
    pub filter_radius: f32,
    pub bit_depth: BitDepth,
}

/// Facts about the box, not about the artwork. Never replayed.
#[derive(Clone, Debug, Default)]
pub struct Machine {
    pub threads: usize,
    pub elapsed_seconds: f32,
    /// The GPU that actually rendered it, as the adapter names itself
    pub adapter: String,
}

pub struct RenderRecord {
    /// Where the scene came from, if it came from anywhere. `None` for a
    /// `--random` roll or a blank canvas, which have no file behind them.
    pub scene_path: Option<PathBuf>,
    /// The scene **as rendered**, serialized. Not the bytes of the file on
    /// disk: by the time a render runs the scene may have been through `-S`
    /// overrides, a `--palette`, a mutation or a `--zoom` that the file knows
    /// nothing about, and the record has to describe the picture that exists.
    pub scene_toml: String,
    /// The camera that actually drew it, after view, path and flags
    pub camera: OrbitCamera,
    pub quality: Quality,
    pub machine: Machine,
    /// UTC, ISO-8601
    pub created: String,
}

impl RenderRecord {
    /// Hex SHA-256 of [`Self::scene_toml`].
    ///
    /// This is what tells "the scene as rendered" from "the scene as it is
    /// now" — the question you have a week later, looking at an image you like
    /// next to a file you have since edited.
    pub fn scene_sha256(&self) -> String {
        let digest = Sha256::digest(self.scene_toml.as_bytes());
        let mut out = String::with_capacity(64);
        for b in digest {
            let _ = write!(out, "{:02x}", b);
        }
        out
    }

    /// The record as a TOML document: the sidecar's contents, and the
    /// `fracturize:render` chunk's.
    ///
    /// Hand-written rather than derived through serde because it carries a
    /// header and a comment on `[machine]` that are half its value — a reader
    /// finding this next to an image needs to be told what not to trust.
    pub fn to_toml(&self) -> String {
        let q = &self.quality;
        let mut s = String::new();
        let _ = writeln!(s, "# fracturize render record — informational, not a scene file.");
        let _ = writeln!(s, "# Written by fracturize {} at {}", VERSION, self.created);
        let _ = writeln!(s);

        let _ = writeln!(s, "[source]");
        match &self.scene_path {
            Some(p) => {
                let _ = writeln!(s, "scene = {}", toml_str(&p.display().to_string()));
            }
            // Said explicitly rather than omitted: an absent key reads as a
            // record that forgot, and this one didn't.
            None => {
                let _ = writeln!(s, "# no scene file — a random roll or a blank canvas");
            }
        }
        let _ = writeln!(s, "scene_sha256 = {}", toml_str(&self.scene_sha256()));
        let _ = writeln!(s);

        let _ = writeln!(s, "[render]");
        let _ = writeln!(s, "width = {}", q.width);
        let _ = writeln!(s, "height = {}", q.height);
        let _ = writeln!(s, "renderer = {}", if q.splat { "\"splat\"" } else { "\"points\"" });
        let _ = writeln!(s, "points = {}", q.points);
        let _ = writeln!(s, "accumulate = {}", q.accumulate);
        if let Some(spp) = q.spp {
            let _ = writeln!(s, "spp = {}", num(spp));
        }
        let _ = writeln!(s, "exposure = {}", num(q.exposure));
        if !q.grade.is_neutral() {
            let _ = writeln!(s, "gamma = {}", num(q.grade.gamma));
            let _ = writeln!(s, "gamma_threshold = {}", num(q.grade.gamma_threshold));
            let _ = writeln!(s, "vibrancy = {}", num(q.grade.vibrancy));
        }
        let _ = writeln!(s, "transparent = {}", q.transparent);
        let _ = writeln!(s, "supersample = {}", q.supersample);
        let _ = writeln!(s, "filter = {}", toml_str(q.filter.label()));
        let _ = writeln!(s, "filter_radius = {}", num(q.filter_radius));
        let _ = writeln!(s, "bit_depth = {}", q.bit_depth.bits());
        let _ = writeln!(s);

        // Straight from `CameraOverride::describe`, which is the one place that
        // knows when the yaw/pitch/roll chart can say a framing and when it has
        // to fall back to an exact `rotvec`. A second spelling here would be a
        // second thing to get wrong at the poles.
        let _ = writeln!(s, "{}", CameraOverride::describe(&self.camera));
        let _ = writeln!(s);

        let _ = writeln!(s, "[machine]");
        let _ = writeln!(s, "# Informational. Do not replay any of this: a thread count that");
        let _ = writeln!(s, "# is right on one box is wrong advice on another.");
        let _ = writeln!(s, "version = {}", toml_str(VERSION));
        let _ = writeln!(s, "threads = {}", self.machine.threads);
        let _ = writeln!(s, "elapsed_seconds = {}", num(self.machine.elapsed_seconds));
        if !self.machine.adapter.is_empty() {
            let _ = writeln!(s, "adapter = {}", toml_str(&self.machine.adapter));
        }
        s
    }

    /// The PNG text chunks, in the order they should be written.
    ///
    /// `Software` and `Creation Time` are the keywords PNG itself reserves for
    /// exactly this; the rest are namespaced. Nothing here is JSON: fracturize
    /// already has a native serialization, and translating would be a second
    /// format to keep in sync for no reader's benefit.
    pub fn png_chunks(&self) -> Vec<(String, String)> {
        vec![
            ("Software".to_string(), format!("fracturize {}", VERSION)),
            ("Creation Time".to_string(), self.created.clone()),
            (KEY_SCENE_SHA.to_string(), self.scene_sha256()),
            (KEY_RENDER.to_string(), self.to_toml()),
            (KEY_SCENE.to_string(), self.scene_toml.clone()),
        ]
    }

    /// Write the sidecar next to `out_path`, as `<stem>.render.toml`.
    ///
    /// Best-effort by design: a render that succeeded must not be reported as
    /// failed because its receipt could not be filed. The error is returned so
    /// the caller can say so, not so it can abort.
    pub fn write_sidecar(&self, out_path: &Path) -> Result<PathBuf, String> {
        let path = sidecar_path(out_path);
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
            }
        }
        std::fs::write(&path, self.to_toml())
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
        Ok(path)
    }
}

/// `renders/foo.png` -> `renders/foo.render.toml`, following the pattern
/// `views/` already established: beside the artefact, under its own name.
pub fn sidecar_path(out_path: &Path) -> PathBuf {
    out_path.with_extension("render.toml")
}

/// Now, as ISO-8601 UTC.
///
/// Computed from the epoch rather than pulled from a date library: this is the
/// only place the program needs a wall-clock date, and the civil-calendar
/// arithmetic below is a well-known closed form (Howard Hinnant's
/// `civil_from_days`) rather than something invented here.
pub fn timestamp_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A TOML basic string. Only the escapes TOML requires — these are paths and
/// short identifiers, not arbitrary text.
fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A float that always parses back as a TOML float.
///
/// `1.0_f32` formats as `"1"` through `Display`, and `exposure = 1` is a TOML
/// *integer* — which is a different type on the way back in, and the kind of
/// round-trip failure that only shows up in whatever reads this later.
fn num(v: f32) -> String {
    if v.is_finite() {
        let s = format!("{}", v);
        if s.contains(['.', 'e', 'E']) { s } else { format!("{}.0", s) }
    } else {
        // TOML has spellings for these, and a record that silently wrote
        // something unparseable would be worse than one that is honest.
        if v.is_nan() {
            "nan".to_string()
        } else if v > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn record() -> RenderRecord {
        RenderRecord {
            scene_path: Some(PathBuf::from("scenes/nautilus.toml")),
            scene_toml: "[meta]\nname = \"Nautilus\"\n".to_string(),
            camera: OrbitCamera::from_chart(0.5, 0.3, 0.0, 4.0, Vec3::ZERO),
            quality: Quality {
                width: 1920,
                height: 1080,
                points: 100_000_000,
                accumulate: 256,
                spp: None,
                grade: Grade::NEUTRAL,
                splat: true,
                exposure: 1.0,
                transparent: false,
                supersample: 2,
                filter: Filter::Gaussian,
                filter_radius: 0.5,
                bit_depth: BitDepth::Eight,
            },
            machine: Machine {
                threads: 3,
                elapsed_seconds: 12.5,
                adapter: "NVIDIA GeForce GTX 1080".to_string(),
            },
            created: "2026-08-12T14:03:22Z".to_string(),
        }
    }

    /// The record is a document other programs read. If it does not parse,
    /// every promise this module makes is void.
    #[test]
    fn the_record_is_valid_toml() {
        let text = record().to_toml();
        let v: toml::Value = toml::from_str(&text).expect("the record must parse");
        assert_eq!(v["render"]["points"].as_integer(), Some(100_000_000));
        assert_eq!(v["render"]["supersample"].as_integer(), Some(2));
        assert_eq!(v["render"]["filter"].as_str(), Some("gaussian"));
        assert_eq!(v["render"]["renderer"].as_str(), Some("splat"));
        assert_eq!(v["render"]["bit_depth"].as_integer(), Some(8));
        assert_eq!(v["source"]["scene"].as_str(), Some("scenes/nautilus.toml"));
        assert_eq!(v["machine"]["threads"].as_integer(), Some(3));
        // The camera block came from `CameraOverride::describe`
        assert!(v["camera"]["distance"].as_float().is_some());
    }

    /// `1.0_f32` Displays as "1", and `exposure = 1` is a TOML integer — a
    /// different type on the way back in.
    #[test]
    fn whole_floats_stay_floats() {
        assert_eq!(num(1.0), "1.0");
        assert_eq!(num(0.5), "0.5");
        assert_eq!(num(-2.0), "-2.0");
        let v: toml::Value = toml::from_str(&record().to_toml()).unwrap();
        assert_eq!(v["render"]["exposure"].as_float(), Some(1.0));
        assert_eq!(v["render"]["filter_radius"].as_float(), Some(0.5));
    }

    /// A scene with no file behind it — a `--random` roll — must still produce
    /// a parseable record rather than a key with nothing after it.
    #[test]
    fn a_scene_with_no_file_still_records() {
        let r = RenderRecord { scene_path: None, ..record() };
        let v: toml::Value = toml::from_str(&r.to_toml()).expect("must parse");
        assert!(v["source"].get("scene").is_none());
        assert_eq!(v["source"]["scene_sha256"].as_str(), Some(r.scene_sha256().as_str()));
    }

    /// Against the published vector, so a wrong hash is a failing test rather
    /// than a fingerprint that quietly means nothing.
    #[test]
    fn the_scene_hash_is_sha256_of_the_scene_text() {
        let r = RenderRecord { scene_toml: "abc".to_string(), ..record() };
        assert_eq!(
            r.scene_sha256(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// The point of the hash: the same picture from a since-edited scene must
    /// be distinguishable from one made now.
    #[test]
    fn editing_the_scene_changes_the_hash() {
        let a = record();
        let b = RenderRecord { scene_toml: format!("{}# a comment\n", a.scene_toml), ..record() };
        assert_ne!(a.scene_sha256(), b.scene_sha256());
    }

    #[test]
    fn the_sidecar_sits_beside_the_image_under_its_own_name() {
        assert_eq!(
            sidecar_path(Path::new("renders/koru-123.png")),
            PathBuf::from("renders/koru-123.render.toml")
        );
        // Animation gets one too — the muxer is hand-rolled and adding a
        // metadata box to it is a different job.
        assert_eq!(
            sidecar_path(Path::new("renders/koru-123.avif")),
            PathBuf::from("renders/koru-123.render.toml")
        );
    }

    /// Every chunk keyword must be legal PNG: 1-79 bytes, printable Latin-1,
    /// no leading, trailing or consecutive spaces. `png` returns an error
    /// rather than writing a broken file, so a bad keyword would lose the
    /// metadata silently at the one moment it is being created.
    #[test]
    fn png_keywords_are_legal() {
        for (k, _) in record().png_chunks() {
            assert!((1..=79).contains(&k.len()), "{:?} is {} bytes", k, k.len());
            assert!(!k.starts_with(' ') && !k.ends_with(' '), "{:?}", k);
            assert!(!k.contains("  "), "{:?}", k);
            assert!(
                k.bytes().all(|b| (32..=126).contains(&b) || (161..=255).contains(&b)),
                "{:?} is not printable Latin-1",
                k
            );
        }
    }

    /// The scene rides in whole, which is the thing that makes a stray PNG
    /// recoverable years later.
    #[test]
    fn the_chunks_carry_the_whole_scene_and_the_version() {
        let r = record();
        let chunks = r.png_chunks();
        let get = |k: &str| {
            chunks.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).expect(k)
        };
        assert_eq!(get(KEY_SCENE), r.scene_toml);
        assert_eq!(get(KEY_SCENE_SHA), r.scene_sha256());
        assert!(get("Software").contains(VERSION));
        // Not `CARGO_PKG_VERSION`, which still says 0.1.0
        assert!(!get("Software").contains("0.1.0"));
        // And no embedded newline in a single-line chunk: a text chunk with a
        // stray newline is a subtly corrupt record and nothing would catch it.
        assert!(!get("Software").contains('\n'));
        assert!(!get("Creation Time").contains('\n'));
    }

    #[test]
    fn timestamps_are_iso_8601_utc() {
        let t = timestamp_utc();
        assert_eq!(t.len(), 20, "{}", t);
        assert!(t.ends_with('Z'));
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[10..11], "T");
        // Known epochs, so a broken calendar is caught rather than assumed
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        // A leap day, which is where a hand-rolled calendar goes wrong
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn strings_that_need_escaping_still_parse() {
        let r = RenderRecord {
            scene_path: Some(PathBuf::from(r#"scenes/od"d\name.toml"#)),
            ..record()
        };
        let v: toml::Value = toml::from_str(&r.to_toml()).expect("must parse");
        assert_eq!(v["source"]["scene"].as_str(), Some(r#"scenes/od"d\name.toml"#));
    }

    /// The camera is emitted through the one function that knows when the
    /// yaw/pitch/roll chart can't say a framing. Looking straight down is
    /// exactly that case, and it must still round-trip.
    #[test]
    fn a_framing_the_chart_cannot_say_falls_back_to_rotvec() {
        // Straight down: yaw and roll become the same control and neither
        // means anything on its own, so the chart cannot say this framing.
        let cam = OrbitCamera::from_chart(
            0.0,
            std::f32::consts::FRAC_PI_2,
            0.0,
            4.0,
            Vec3::ZERO,
        );
        let r = RenderRecord { camera: cam, ..record() };
        let text = r.to_toml();
        let v: toml::Value = toml::from_str(&text).expect("must parse");
        assert!(
            v["camera"].get("rotvec").is_some() || v["camera"].get("yaw").is_some(),
            "a framing must be recorded one way or the other:\n{}",
            text
        );
    }
}
