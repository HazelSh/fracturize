//! Reading gradients written by other tools.
//!
//! The library here is deliberately small and hand-authored (see
//! `palette::library` for why flam3's ~700 aren't vendored), so the way to get
//! a big collection is to bring one. Three formats, all of which a fractal
//! flame user is likely to already have on disk:
//!
//! - **`.ugr` / `.gradient`** — UltraFractal gradients, which is what
//!   Apophysis's gradient browser eats. One file holds many named gradients;
//!   colours are `index=N color=M` with `M` packed **BGR** (`r + g<<8 + b<<16`)
//!   and indices running 0..399, not 0..255.
//! - **`.flame`** — an Apophysis flame's `<palette>` element: a hex blob of
//!   256 RGB triples. Read *only* for its palette; the rest of a `.flame` is a
//!   different project.
//! - **`.toml`** — fracturize's own `[palette]` table, standalone.
//!
//! Imported colours are display sRGB and are converted to the renderer's
//! linear RGB on the way in, so an Apophysis gradient looks here like it
//! looked there.

use std::path::Path;

use glam::Vec3;

use super::spec::PaletteDef;
use super::{from_srgb8, Interpolate, Palette, Stop};

/// Load every gradient in a file. Most formats hold one; `.ugr` holds many.
pub fn load(path: impl AsRef<Path>) -> Result<Vec<Palette>, String> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("imported");

    let found = match ext.as_str() {
        "ugr" | "gradient" | "uxf" => parse_ugr(&text),
        "flame" | "xml" => parse_flame(&text, stem),
        "toml" => {
            let def: PaletteDef = parse_palette_toml(&text)?;
            vec![def.resolve()?]
        }
        // No extension we know: sniff. A UGR body always has `index=`, a
        // flame is XML.
        _ if text.contains("<palette") || text.contains("<flame") => parse_flame(&text, stem),
        _ if text.contains("index=") => parse_ugr(&text),
        _ => return Err(format!("{}: unrecognised palette format", path.display())),
    };

    if found.is_empty() {
        return Err(format!("{}: no palettes found", path.display()));
    }
    Ok(found)
}

/// Load exactly one gradient — the first in the file, or the one whose name
/// matches `#name` appended to the path (`gradients.ugr#twilight`).
pub fn load_one(spec: &str) -> Result<Palette, String> {
    let (path, wanted) = match spec.rsplit_once('#') {
        Some((p, n)) if !p.is_empty() => (p, Some(n)),
        _ => (spec, None),
    };
    let found = load(path)?;
    match wanted {
        None => Ok(found.into_iter().next().expect("load rejects empty files")),
        Some(name) => found
            .iter()
            .find(|p| p.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "no gradient named '{}' in {} (found: {})",
                    name,
                    path,
                    found
                        .iter()
                        .filter_map(|p| p.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
    }
}

/// A standalone palette TOML: either a bare table or one wrapped in
/// `[palette]`, so a fragment cut out of a scene file pastes in directly.
fn parse_palette_toml(text: &str) -> Result<PaletteDef, String> {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        palette: PaletteDef,
    }
    if let Ok(w) = toml::from_str::<Wrapper>(text) {
        return Ok(w.palette);
    }
    toml::from_str::<PaletteDef>(text).map_err(|e| format!("not a palette file: {e}"))
}

/// UltraFractal / Apophysis `.ugr`. Each gradient is `name { ... }` with
/// `index=N color=M` pairs inside.
fn parse_ugr(text: &str) -> Vec<Palette> {
    let mut out = Vec::new();
    let mut rest = text;

    while let Some(open) = rest.find('{') {
        // The name is the last token before the brace — UGR files put a
        // comment block above each gradient often enough that taking the
        // whole preceding line would pick up junk.
        let name = rest[..open]
            .split_whitespace()
            .next_back()
            .unwrap_or("imported")
            .to_string();
        let Some(close) = rest[open..].find('}') else { break };
        let body = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];

        // `opacity:` starts a second index list we don't want to mix in.
        let body = match body.find("opacity:") {
            Some(i) => &body[..i],
            None => body,
        };

        let mut points: Vec<(u32, Vec3)> = Vec::new();
        for chunk in body.split("index=").skip(1) {
            let mut it = chunk.split_whitespace();
            let Some(index) = it.next().and_then(|v| v.trim_matches('"').parse::<u32>().ok())
            else {
                continue;
            };
            let Some(color) = chunk
                .split("color=")
                .nth(1)
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.trim_matches('"').parse::<i64>().ok())
            else {
                continue;
            };
            // BGR-packed, which is the one thing about this format that will
            // silently produce a plausible-but-wrong palette if you get it
            // backwards.
            let c = color.max(0) as u32;
            let rgb = [(c & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, ((c >> 16) & 0xFF) as u8];
            points.push((index, from_srgb8(rgb)));
        }

        if points.len() < 2 {
            continue;
        }
        points.sort_by_key(|&(i, _)| i);
        points.dedup_by_key(|&mut (i, _)| i);

        // UF indexes gradients 0..399; some writers use 0..255. Pick the
        // scale from what's actually in the file rather than guessing.
        let max = points.last().map(|&(i, _)| i).unwrap_or(0);
        let span = if max > 255 { 400.0 } else { 256.0 };

        let mut p = Palette::from_stops(
            points
                .into_iter()
                .map(|(i, color)| Stop { at: (i as f32 / span).clamp(0.0, 1.0), color })
                .collect(),
        );
        p.name = Some(name);
        p.interpolate = Interpolate::Rgb;
        out.push(p);
    }
    out
}

/// An Apophysis `.flame`'s `<palette>` blob, or a flam3 palette library's
/// `data=` attribute. Both are hex, 6 or 8 characters per entry (the 8-char
/// form carries a leading pad byte).
fn parse_flame(text: &str, stem: &str) -> Vec<Palette> {
    let mut out = Vec::new();

    // Element form: <palette ...>HEX</palette>
    for (i, chunk) in text.split("<palette").skip(1).enumerate() {
        let name = attr(chunk, "name")
            .unwrap_or_else(|| if i == 0 { stem.to_string() } else { format!("{stem}-{i}") });
        // Either the attribute or the element body carries the hex.
        let hex = attr(chunk, "data").unwrap_or_else(|| {
            chunk
                .split_once('>')
                .map(|(_, body)| body.split('<').next().unwrap_or("").to_string())
                .unwrap_or_default()
        });
        if let Some(entries) = decode_hex_entries(&hex) {
            let mut p = Palette::from_entries(entries);
            p.name = Some(name);
            out.push(p);
        }
    }
    if !out.is_empty() {
        return out;
    }

    // Older per-colour form: <color index="0" rgb="12 30 44"/>
    let mut points: Vec<(u32, Vec3)> = Vec::new();
    for chunk in text.split("<color").skip(1) {
        let (Some(index), Some(rgb)) = (attr(chunk, "index"), attr(chunk, "rgb")) else {
            continue;
        };
        let Ok(index) = index.trim().parse::<u32>() else { continue };
        let vals: Vec<u8> = rgb
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.trim().parse::<f32>().ok())
            .map(|v| v.clamp(0.0, 255.0) as u8)
            .collect();
        if vals.len() >= 3 {
            points.push((index, from_srgb8([vals[0], vals[1], vals[2]])));
        }
    }
    if points.len() >= 2 {
        points.sort_by_key(|&(i, _)| i);
        let mut p = Palette::from_stops(
            points
                .into_iter()
                .map(|(i, color)| Stop { at: (i as f32 / 256.0).clamp(0.0, 1.0), color })
                .collect(),
        );
        p.name = Some(stem.to_string());
        out.push(p);
    }
    out
}

/// `key="value"` out of an XML-ish fragment.
fn attr(chunk: &str, key: &str) -> Option<String> {
    let at = chunk.find(&format!("{key}=\""))? + key.len() + 2;
    let end = chunk[at..].find('"')? + at;
    Some(chunk[at..end].to_string())
}

/// A hex blob → 256 linear colours. Accepts 6 or 8 hex characters per entry;
/// the 8-character form is flam3's, whose leading byte is padding.
fn decode_hex_entries(hex: &str) -> Option<[Vec3; 256]> {
    let digits: Vec<u8> = hex
        .bytes()
        .filter(|b| b.is_ascii_hexdigit())
        .map(|b| (b as char).to_digit(16).unwrap() as u8)
        .collect();
    let per = match digits.len() {
        n if n == 256 * 6 => 6,
        n if n == 256 * 8 => 8,
        _ => return None,
    };
    let skip = per - 6;
    let byte = |d: &[u8], i: usize| d[i] * 16 + d[i + 1];
    Some(std::array::from_fn(|i| {
        let base = i * per + skip;
        from_srgb8([
            byte(&digits, base),
            byte(&digits, base + 2),
            byte(&digits, base + 4),
        ])
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{library, to_srgb8, Body};

    #[test]
    fn ugr_reads_names_colours_and_the_400_index_scale() {
        // color= is BGR-packed: 0xFF0000 is *blue*, not red.
        let text = r#"
twilight {
gradient:
 title="twilight" smooth=no
 index=0 color=0
 index=200 color=16711680
 index=399 color=255
opacity:
 index=0 opacity=255
}
dawn {
gradient:
 index=0 color=65280
 index=399 color=16777215
}
"#;
        let found = parse_ugr(text);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name.as_deref(), Some("twilight"));
        assert_eq!(found[1].name.as_deref(), Some("dawn"));

        let stops = found[0].stops().unwrap();
        assert_eq!(stops.len(), 3);
        assert_eq!(to_srgb8(stops[0].color), [0, 0, 0]);
        assert_eq!(to_srgb8(stops[1].color), [0, 0, 255], "0xFF0000 is blue in BGR");
        assert_eq!(to_srgb8(stops[2].color), [255, 0, 0], "0x0000FF is red in BGR");
        // 0..399 indexing, not 0..255
        assert!((stops[1].at - 0.5).abs() < 0.01, "index 200 of 399 should land mid-gradient");
        assert!(stops[2].at > 0.99);

        // Green through the middle of the second one
        assert_eq!(to_srgb8(found[1].stops().unwrap()[0].color), [0, 255, 0]);
    }

    #[test]
    fn ugr_ignores_the_opacity_block() {
        let text = "g {\ngradient:\n index=0 color=0\n index=255 color=16777215\nopacity:\n index=0 opacity=255\n index=128 opacity=0\n}";
        let found = parse_ugr(text);
        assert_eq!(found[0].stops().unwrap().len(), 2, "opacity indices must not become stops");
    }

    #[test]
    fn flame_palette_blob_decodes() {
        // 256 entries, RGB, ramping red 0..255 with fixed green/blue.
        let hex: String = (0..256).map(|i| format!("{i:02X}8040")).collect();
        let text = format!("<flame><palette count=\"256\" format=\"RGB\">{hex}</palette></flame>");
        let found = parse_flame(&text, "scene");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name.as_deref(), Some("scene"));
        let Body::Entries(e) = &found[0].body else { panic!("expected verbatim entries") };
        assert_eq!(to_srgb8(e[0]), [0, 128, 64]);
        assert_eq!(to_srgb8(e[255]), [255, 128, 64]);
    }

    #[test]
    fn flam3_style_eight_digit_entries_drop_the_pad_byte() {
        let hex: String = (0..256).map(|i| format!("00{i:02X}8040")).collect();
        let found = parse_flame(&format!("<palette data=\"{hex}\" name=\"lib\"/>"), "x");
        let Body::Entries(e) = &found[0].body else { panic!("expected verbatim entries") };
        assert_eq!(found[0].name.as_deref(), Some("lib"));
        assert_eq!(to_srgb8(e[7]), [7, 128, 64]);
    }

    #[test]
    fn flame_per_colour_elements_decode() {
        let text = "<flame><color index=\"0\" rgb=\"0 0 0\"/><color index=\"255\" rgb=\"255 128 0\"/></flame>";
        let found = parse_flame(text, "old");
        assert_eq!(found.len(), 1);
        let stops = found[0].stops().unwrap();
        assert_eq!(to_srgb8(stops[1].color), [255, 128, 0]);
    }

    #[test]
    fn palette_toml_parses_wrapped_and_bare() {
        let bare = r#"name = "ember""#;
        let wrapped = format!("[palette]\n{bare}\n");
        let a = parse_palette_toml(bare).unwrap().resolve().unwrap();
        let b = parse_palette_toml(&wrapped).unwrap().resolve().unwrap();
        assert_eq!(a, b);
        assert_eq!(a, library::get("ember").unwrap());
    }

    #[test]
    fn load_one_selects_by_fragment() {
        let dir = std::env::temp_dir().join(format!("fracturize-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gradients.ugr");
        std::fs::write(
            &path,
            "twilight {\ngradient:\n index=0 color=0\n index=399 color=16711680\n}\n\
             dawn {\ngradient:\n index=0 color=0\n index=399 color=255\n}\n",
        )
        .unwrap();

        let first = load_one(path.to_str().unwrap()).unwrap();
        assert_eq!(first.name.as_deref(), Some("twilight"));

        let picked = load_one(&format!("{}#dawn", path.display())).unwrap();
        assert_eq!(picked.name.as_deref(), Some("dawn"));

        let err = load_one(&format!("{}#nope", path.display())).unwrap_err();
        assert!(err.contains("twilight") && err.contains("dawn"), "error lists what is there: {err}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
