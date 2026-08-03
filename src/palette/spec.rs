//! The `[palette]` table: how a gradient is written down in a scene file.
//!
//! Kept apart from `Palette` itself so the runtime type stays free of serde
//! and of the file format's compatibility shims.
//!
//! ```toml
//! [palette]
//! name = "ember"                 # a library palette, taken as-is...
//!
//! # ...or authored here. Any of `stops`, `cosine` or `entries`; the first
//! # one present wins, and all three override `name`.
//! cyclic = true
//! interpolate = "rgb"            # or "oklab"
//! stops = [
//!   { at = 0.00, color = "#0c040a" },
//!   { at = 0.38, color = "#b23818" },
//!   { at = 0.70, color = "#fcde9e" },
//! ]
//! # cosine = { a = [...], b = [...], c = [...], d = [...] }
//!
//! rotate = 0.0                   # shift the gradient along the index
//! reverse = false                # both apply on top of a library palette,
//!                                # so it can be tuned without forking it
//! enabled = true                 # false keeps the palette but renders the
//!                                # per-transform ring (the A/B toggle)
//! ```
//!
//! **A stop's `color` is sRGB hex or linear floats.** `"#b23818"` is the
//! value a colour picker shows you; `[0.44, 0.04, 0.01]` is the linear triple
//! the GPU receives, matching per-transform `color` and `background`. Both
//! parse; saving writes hex, because a palette is a thing you look at.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use super::{from_srgb8, library, to_srgb8, Body, Cosine, Interpolate, Palette, Stop};

/// A colour in a palette: `"#rrggbb"` (sRGB) or `[r, g, b]` (linear).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum ColorDef {
    Hex(String),
    Linear([f64; 3]),
}

impl ColorDef {
    pub fn to_linear(&self) -> Result<Vec3, String> {
        match self {
            ColorDef::Linear(c) => Ok(Vec3::from(c.map(|v| v as f32))),
            ColorDef::Hex(s) => parse_hex(s).map(from_srgb8),
        }
    }

    pub fn from_linear(c: Vec3) -> Self {
        let [r, g, b] = to_srgb8(c);
        ColorDef::Hex(format!("#{r:02x}{g:02x}{b:02x}"))
    }
}

/// `#rrggbb` / `rrggbb` (and the 3-digit short form) → sRGB bytes.
pub fn parse_hex(s: &str) -> Result<[u8; 3], String> {
    let h = s.trim().trim_start_matches('#');
    let byte = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("bad hex colour '{s}'"))
    };
    match h.len() {
        6 => Ok([byte(0)?, byte(2)?, byte(4)?]),
        3 => {
            let nib = |i: usize| {
                u8::from_str_radix(&h[i..i + 1], 16)
                    .map(|v| v * 17)
                    .map_err(|_| format!("bad hex colour '{s}'"))
            };
            Ok([nib(0)?, nib(1)?, nib(2)?])
        }
        _ => Err(format!("bad hex colour '{s}': expected #rrggbb")),
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StopDef {
    pub at: f64,
    pub color: ColorDef,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CosineDef {
    pub a: [f64; 3],
    pub b: [f64; 3],
    pub c: [f64; 3],
    pub d: [f64; 3],
}

/// The `[palette]` table as it appears in a scene (or a standalone palette
/// file, which is the same table at the top level).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PaletteDef {
    /// Library palette to start from, or just the name of an authored one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether this palette is the colour source. `false` keeps the gradient
    /// in the file while rendering the per-transform ring — the A/B toggle,
    /// which would otherwise be lost on every save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cyclic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpolate: Option<Interpolate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stops: Option<Vec<StopDef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cosine: Option<CosineDef>,
    /// 256 sRGB hex entries, for a palette imported verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<String>>,
}

impl PaletteDef {
    /// Resolve to a usable gradient. Errors name the problem rather than
    /// falling back to something that renders but isn't what was asked for.
    pub fn resolve(&self) -> Result<Palette, String> {
        // A body given here always wins over `name`, so a scene that started
        // from a library palette and then edited its stops keeps the edits
        // (and the name, as provenance).
        let body = if let Some(stops) = &self.stops {
            let mut v: Vec<Stop> = stops
                .iter()
                .map(|s| Ok(Stop { at: s.at as f32, color: s.color.to_linear()? }))
                .collect::<Result<_, String>>()?;
            v.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
            Some(Body::Stops(v))
        } else if let Some(c) = &self.cosine {
            Some(Body::Cosine(Cosine {
                a: Vec3::from(c.a.map(|v| v as f32)),
                b: Vec3::from(c.b.map(|v| v as f32)),
                c: Vec3::from(c.c.map(|v| v as f32)),
                d: Vec3::from(c.d.map(|v| v as f32)),
            }))
        } else if let Some(e) = &self.entries {
            if e.len() != 256 {
                return Err(format!("palette `entries` needs exactly 256 colours, got {}", e.len()));
            }
            let mut arr = [Vec3::ZERO; 256];
            for (slot, hex) in arr.iter_mut().zip(e) {
                *slot = from_srgb8(parse_hex(hex)?);
            }
            Some(Body::Entries(Box::new(arr)))
        } else {
            None
        };

        let mut palette = match (body, &self.name) {
            (Some(body), _) => Palette { body, ..Palette::default() },
            (None, Some(name)) => library::get(name).ok_or_else(|| {
                format!(
                    "unknown palette '{}'. Available: {}",
                    name,
                    library::names().join(", ")
                )
            })?,
            (None, None) => {
                return Err(
                    "[palette] needs `name`, `stops`, `cosine` or `entries`".to_string()
                )
            }
        };

        palette.name = self.name.clone().or(palette.name);
        if let Some(v) = self.cyclic {
            palette.cyclic = v;
        }
        if let Some(v) = self.interpolate {
            palette.interpolate = v;
        }
        palette.rotate = self.rotate.unwrap_or(0.0) as f32;
        palette.reverse = self.reverse.unwrap_or(false);
        Ok(palette)
    }

    /// Serialize a gradient back out. `enabled` is written only when false,
    /// and defaults are omitted, so the common case is a small table.
    pub fn from_palette(p: &Palette, enabled: bool) -> Self {
        let mut def = PaletteDef {
            name: p.name.clone(),
            enabled: (!enabled).then_some(false),
            cyclic: (!p.cyclic).then_some(false),
            interpolate: (p.interpolate != Interpolate::default()).then_some(p.interpolate),
            rotate: (p.rotate != 0.0).then(|| tidy(p.rotate)),
            reverse: p.reverse.then_some(true),
            ..Self::default()
        };
        // A library palette that hasn't been edited is written as just its
        // name; anything else carries its colours, because the library is
        // allowed to change and a scene is not.
        let untouched_library = p
            .name
            .as_deref()
            .and_then(library::get)
            .is_some_and(|lib| lib.body == p.body);
        if !untouched_library {
            match &p.body {
                Body::Stops(stops) => {
                    def.stops = Some(
                        stops
                            .iter()
                            .map(|s| StopDef {
                                at: tidy(s.at),
                                color: ColorDef::from_linear(s.color),
                            })
                            .collect(),
                    )
                }
                Body::Cosine(c) => {
                    def.cosine = Some(CosineDef {
                        a: c.a.to_array().map(tidy),
                        b: c.b.to_array().map(tidy),
                        c: c.c.to_array().map(tidy),
                        d: c.d.to_array().map(tidy),
                    })
                }
                Body::Entries(e) => {
                    def.entries = Some(
                        e.iter()
                            .map(|&c| {
                                let [r, g, b] = to_srgb8(c);
                                format!("#{r:02x}{g:02x}{b:02x}")
                            })
                            .collect(),
                    )
                }
            }
        }
        def
    }

    /// The `[palette]` table as toml_edit builds it — inline stops one per
    /// line, an inline `cosine`, entries wrapped eight to a row.
    ///
    /// Hand-built rather than left to serde because serde renders nested
    /// structs as *sub-tables*: `stops` would come out as a run of
    /// `[[palette.stops]]` headers and `cosine` as `[palette.cosine]`, which
    /// is valid TOML but neither readable nor safely pasteable — a fragment
    /// starting `[palette]` followed by `[cosine]` reparses as a top-level
    /// `cosine` table and silently loses the gradient.
    pub fn to_table(&self) -> toml_edit::Table {
        let mut table = toml_edit::Table::new();
        if let Some(name) = &self.name {
            table["name"] = toml_edit::value(name.as_str());
        }
        if let Some(v) = self.enabled {
            table["enabled"] = toml_edit::value(v);
        }
        if let Some(v) = self.cyclic {
            table["cyclic"] = toml_edit::value(v);
        }
        if let Some(v) = self.interpolate {
            table["interpolate"] = toml_edit::value(v.name());
        }
        if let Some(v) = self.rotate {
            table["rotate"] = toml_edit::value(v);
        }
        if let Some(v) = self.reverse {
            table["reverse"] = toml_edit::value(v);
        }
        if let Some(stops) = &self.stops {
            let mut arr = toml_edit::Array::new();
            for s in stops {
                let mut it = toml_edit::InlineTable::new();
                it.insert("at", s.at.into());
                it.insert(
                    "color",
                    match &s.color {
                        ColorDef::Hex(h) => h.as_str().into(),
                        ColorDef::Linear(c) => {
                            let mut a = toml_edit::Array::new();
                            c.iter().for_each(|&v| a.push(v));
                            toml_edit::Value::Array(a)
                        }
                    },
                );
                arr.push(toml_edit::Value::InlineTable(it));
                // One stop per line: a gradient is read down the page, and
                // eight stops on one line is unreviewable in a diff.
                if let Some(last) = arr.iter_mut().last() {
                    last.decor_mut().set_prefix("\n    ");
                }
            }
            arr.set_trailing("\n");
            table["stops"] = toml_edit::Item::Value(toml_edit::Value::Array(arr));
        }
        if let Some(c) = &self.cosine {
            let mut it = toml_edit::InlineTable::new();
            for (key, vals) in [("a", c.a), ("b", c.b), ("c", c.c), ("d", c.d)] {
                let mut a = toml_edit::Array::new();
                vals.iter().for_each(|&v| a.push(v));
                it.insert(key, toml_edit::Value::Array(a));
            }
            table["cosine"] = toml_edit::value(it);
        }
        if let Some(entries) = &self.entries {
            let mut arr = toml_edit::Array::new();
            for (i, e) in entries.iter().enumerate() {
                arr.push(e.as_str());
                // 256 hex strings on one line is a 2 KB line; eight per row
                // is still a blob, but a diffable one.
                if i % 8 == 0 {
                    if let Some(last) = arr.iter_mut().last() {
                        last.decor_mut().set_prefix("\n    ");
                    }
                }
            }
            arr.set_trailing("\n");
            table["entries"] = toml_edit::Item::Value(toml_edit::Value::Array(arr));
        }
        table
    }

    /// The table as a pasteable TOML fragment, for `--random-palette` — the
    /// same convention as `--random` printing its seed, so a good roll can be
    /// kept without re-deriving it.
    pub fn to_toml_fragment(&self) -> String {
        // A bare `Table`'s Display has no leading newline of its own, so the
        // header and the first key would share a line and not parse.
        format!("[palette]\n{}", self.to_table().to_string().trim_start_matches('\n'))
    }
}

/// Round in f64 space, matching `scene::tidy`, so palettes don't litter files
/// with 0.3000000119209290.
fn tidy(v: f32) -> f64 {
    (v as f64 * 10_000.0).round() / 10_000.0 + 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_forms_parse() {
        assert_eq!(parse_hex("#ff8000").unwrap(), [255, 128, 0]);
        assert_eq!(parse_hex("ff8000").unwrap(), [255, 128, 0]);
        assert_eq!(parse_hex("#f80").unwrap(), [255, 136, 0]);
        assert!(parse_hex("#gg0000").is_err());
        assert!(parse_hex("#ff00").is_err());
    }

    #[test]
    fn hex_and_linear_stops_agree() {
        let hex: PaletteDef = toml::from_str(
            r##"stops = [{ at = 0.0, color = "#ff8000" }, { at = 0.5, color = "#0080ff" }]"##,
        )
        .unwrap();
        let lin = PaletteDef {
            stops: Some(vec![
                StopDef {
                    at: 0.0,
                    color: ColorDef::Linear(from_srgb8([255, 128, 0]).to_array().map(|v| v as f64)),
                },
                StopDef {
                    at: 0.5,
                    color: ColorDef::Linear(from_srgb8([0, 128, 255]).to_array().map(|v| v as f64)),
                },
            ]),
            ..Default::default()
        };
        let (a, b) = (hex.resolve().unwrap(), lin.resolve().unwrap());
        for i in 0..256 {
            let t = i as f32 / 256.0;
            assert!((a.sample(t) - b.sample(t)).length() < 1e-5);
        }
    }

    #[test]
    fn library_palettes_round_trip_by_name_alone() {
        for name in library::names() {
            let p = library::get(name).unwrap();
            let def = PaletteDef::from_palette(&p, true);
            assert!(def.stops.is_none(), "'{name}' should serialize as just a name");
            assert_eq!(def.name.as_deref(), Some(name));
            assert_eq!(def.resolve().unwrap(), p);
        }
    }

    #[test]
    fn an_edited_library_palette_carries_its_colours() {
        let mut p = library::get("ember").unwrap();
        p.stops_mut().unwrap()[0].color = Vec3::new(1.0, 0.0, 0.0);
        let def = PaletteDef::from_palette(&p, true);
        assert!(def.stops.is_some(), "edited stops must be written out");
        let back = def.resolve().unwrap();
        assert_eq!(back.stops().unwrap()[0].color, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(back.name.as_deref(), Some("ember"), "provenance survives an edit");
    }

    #[test]
    fn every_body_form_round_trips() {
        let cases = vec![
            library::get("ocean").unwrap().to_stops(9),
            Palette::from_cosine(Cosine {
                a: Vec3::splat(0.5),
                b: Vec3::splat(0.4),
                c: Vec3::new(1.0, 1.0, 1.0),
                d: Vec3::new(0.0, 0.1, 0.2),
            }),
            Palette::from_entries(std::array::from_fn(|i| {
                from_srgb8([i as u8, (255 - i) as u8, 128])
            })),
        ];
        for mut p in cases {
            p.cyclic = false;
            p.interpolate = Interpolate::Oklab;
            p.rotate = 0.25;
            p.reverse = true;
            let def = PaletteDef::from_palette(&p, true);
            let text = toml::to_string(&def).unwrap();
            let parsed: PaletteDef = toml::from_str(&text).unwrap();
            let back = parsed.resolve().unwrap();
            for i in 0..256 {
                let t = i as f32 / 256.0;
                assert!(
                    (back.sample(t) - p.sample(t)).length() < 2e-3,
                    "{:?} drifted at t = {t}",
                    std::mem::discriminant(&p.body)
                );
            }
            assert_eq!(back.cyclic, false);
            assert_eq!(back.interpolate, Interpolate::Oklab);
            assert_eq!(back.reverse, true);
            assert!((back.rotate - 0.25).abs() < 1e-6);
        }
    }

    /// `--random-palette` prints this so a roll can be kept. If it doesn't
    /// reparse into the same gradient it is worse than useless.
    #[test]
    fn the_printed_fragment_pastes_back_into_a_scene() {
        #[derive(serde::Deserialize)]
        struct Wrapper {
            palette: PaletteDef,
        }
        let cases = vec![
            library::get("peacock").unwrap().to_stops(6),
            Palette::from_cosine(Cosine {
                a: Vec3::splat(0.5),
                b: Vec3::splat(0.45),
                c: Vec3::ONE,
                d: Vec3::new(0.1, 0.4, 0.7),
            }),
            library::get("ash").unwrap(),
        ];
        for p in cases {
            let text = PaletteDef::from_palette(&p, true).to_toml_fragment();
            assert!(text.starts_with("[palette]"), "{text}");
            // The trap this guards: serde would emit `[cosine]`, which
            // reparses as a *top-level* table and loses the palette entirely.
            assert!(!text.contains("\n[cosine]"), "sub-table leaked into the fragment:\n{text}");
            assert!(!text.contains("[[stops]]"), "sub-table leaked into the fragment:\n{text}");

            let w: Wrapper = toml::from_str(&text)
                .unwrap_or_else(|e| panic!("fragment does not parse: {e}\n{text}"));
            // Not float-identical: positions round to four decimals and
            // colours through 8-bit hex, which is the readable form the file
            // format wants. What has to survive is the gradient itself.
            let back = w.palette.resolve().unwrap();
            assert_eq!(back.name, p.name);
            for i in 0..256 {
                let t = i as f32 / 256.0;
                assert!(
                    (back.sample(t) - p.sample(t)).length() < 5e-3,
                    "drifted at t = {t}\n{text}"
                );
            }
        }
    }

    #[test]
    fn enabled_false_survives_a_round_trip() {
        let p = library::get("ash").unwrap();
        let def = PaletteDef::from_palette(&p, false);
        assert_eq!(def.enabled, Some(false));
        let text = toml::to_string(&def).unwrap();
        let parsed: PaletteDef = toml::from_str(&text).unwrap();
        assert_eq!(parsed.enabled, Some(false));
        // ...and the common case stays out of the file
        assert!(PaletteDef::from_palette(&p, true).enabled.is_none());
    }

    #[test]
    fn unknown_names_and_bad_bodies_are_errors() {
        let def = PaletteDef { name: Some("nope".into()), ..Default::default() };
        assert!(def.resolve().unwrap_err().contains("unknown palette"));

        assert!(PaletteDef::default().resolve().unwrap_err().contains("needs"));

        let short = PaletteDef { entries: Some(vec!["#000000".into(); 3]), ..Default::default() };
        assert!(short.resolve().unwrap_err().contains("256"));
    }
}
