//! The built-in gradient collection.
//!
//! A palette mode is only as good as its palettes: flam3 ships ~700 and that
//! library is a large part of why Apophysis output looks coherent even from
//! beginners. This is a curated ~20 rather than a scrape — flam3's own
//! `flam3-palettes.xml` is GPL'd and this project's licence isn't stated, so
//! it isn't vendored. `palette::import` reads Apophysis `.ugr` / `.gradient`
//! and `.flame` files, so anyone with a collection can bring it.
//!
//! Two rules every entry here follows, both from hard-won experience with
//! random palettes (see `palette::random`):
//!
//! - **Luminance goes somewhere.** The renderer has no lights, so the palette
//!   *is* the shading. A gradient that sits at one brightness renders flat.
//! - **Luminance rises and falls once.** The colormap is cyclic, so a
//!   monotone dark→bright ramp puts a hard seam at index 0 where white meets
//!   black. Every entry returns to roughly where it started.
//!
//! Colours are authored as sRGB bytes — the space you'd read off a colour
//! picker — and converted to the renderer's linear RGB on the way out.

use super::{from_srgb8, Palette, Stop};

/// One library gradient: authored stops, plus a line about what it's for.
struct Def {
    name: &'static str,
    blurb: &'static str,
    /// (position, sRGB bytes)
    stops: &'static [(f32, [u8; 3])],
}

/// The collection. Ordered roughly cool → warm → neutral, because that's how
/// you scan a list looking for one.
const LIBRARY: &[Def] = &[
    Def {
        name: "ember",
        blurb: "banked fire: maroon through orange to pale ash",
        stops: &[
            (0.00, [12, 4, 10]),
            (0.18, [70, 14, 20]),
            (0.38, [178, 56, 24]),
            (0.55, [240, 138, 42]),
            (0.70, [252, 222, 158]),
            (0.85, [120, 46, 44]),
        ],
    },
    Def {
        name: "lava",
        blurb: "black through violet and red to a white-hot core",
        stops: &[
            (0.00, [4, 2, 12]),
            (0.16, [48, 12, 76]),
            (0.34, [140, 22, 84]),
            (0.50, [222, 74, 40]),
            (0.64, [252, 176, 60]),
            (0.76, [255, 246, 210]),
            (0.90, [58, 16, 52]),
        ],
    },
    Def {
        name: "sunset",
        blurb: "indigo, violet, coral, gold — the wide warm sweep",
        stops: &[
            (0.00, [18, 16, 52]),
            (0.20, [76, 42, 118]),
            (0.40, [178, 78, 122]),
            (0.58, [242, 132, 92]),
            (0.74, [252, 208, 122]),
            (0.88, [64, 40, 86]),
        ],
    },
    Def {
        name: "copper",
        blurb: "oxidised metal: brown, rust, a bright rim, verdigris",
        stops: &[
            (0.00, [26, 14, 10]),
            (0.22, [96, 44, 22]),
            (0.42, [186, 104, 48]),
            (0.58, [246, 196, 138]),
            (0.72, [128, 148, 122]),
            (0.88, [42, 46, 40]),
        ],
    },
    Def {
        name: "autumn",
        blurb: "leaf litter: olive, amber, scarlet, bark",
        stops: &[
            (0.00, [22, 20, 10]),
            (0.20, [92, 88, 30]),
            (0.38, [200, 148, 40]),
            (0.54, [214, 88, 34]),
            (0.70, [244, 214, 160]),
            (0.86, [56, 34, 22]),
        ],
    },
    Def {
        name: "sodium",
        blurb: "streetlight monochrome — one hue, all the shading",
        stops: &[
            (0.00, [8, 4, 0]),
            (0.25, [86, 38, 4]),
            (0.45, [188, 106, 12]),
            (0.62, [252, 190, 90]),
            (0.78, [255, 238, 194]),
            (0.92, [60, 26, 4]),
        ],
    },
    Def {
        name: "ocean",
        blurb: "navy to teal to foam, the deep-water ramp",
        stops: &[
            (0.00, [4, 10, 34]),
            (0.20, [10, 48, 88]),
            (0.40, [16, 108, 128]),
            (0.58, [64, 176, 168]),
            (0.74, [206, 240, 232]),
            (0.90, [12, 40, 70]),
        ],
    },
    Def {
        name: "ice",
        blurb: "cold and bright: deep blue through cyan to white",
        stops: &[
            (0.00, [6, 12, 40]),
            (0.22, [24, 62, 130]),
            (0.44, [72, 148, 216]),
            (0.60, [160, 216, 244]),
            (0.74, [248, 252, 255]),
            (0.90, [20, 40, 92]),
        ],
    },
    Def {
        name: "cobalt",
        blurb: "saturated blue with a hot white centre",
        stops: &[
            (0.00, [2, 2, 18]),
            (0.24, [16, 24, 108]),
            (0.46, [40, 78, 208]),
            (0.60, [128, 176, 250]),
            (0.72, [236, 244, 255]),
            (0.88, [10, 14, 62]),
        ],
    },
    Def {
        name: "jade",
        blurb: "stone green: dark, mineral, one pale highlight",
        stops: &[
            (0.00, [6, 18, 14]),
            (0.22, [18, 62, 48]),
            (0.44, [46, 124, 96]),
            (0.60, [128, 194, 156]),
            (0.74, [232, 246, 232]),
            (0.90, [14, 44, 36]),
        ],
    },
    Def {
        name: "fern",
        blurb: "undergrowth: deep green to yellow-green to cream",
        stops: &[
            (0.00, [10, 20, 12]),
            (0.22, [30, 74, 34]),
            (0.42, [96, 140, 44]),
            (0.58, [186, 198, 78]),
            (0.74, [244, 246, 206]),
            (0.90, [24, 48, 26]),
        ],
    },
    Def {
        name: "amethyst",
        blurb: "violet through orchid to a chalk highlight",
        stops: &[
            (0.00, [12, 6, 24]),
            (0.22, [56, 24, 92]),
            (0.44, [122, 62, 168]),
            (0.60, [190, 138, 220]),
            (0.74, [242, 232, 250]),
            (0.90, [34, 16, 56]),
        ],
    },
    Def {
        name: "bloom",
        blurb: "pink and green, complementary, deliberately loud",
        stops: &[
            (0.00, [24, 10, 26]),
            (0.18, [128, 26, 88]),
            (0.34, [232, 96, 150]),
            (0.50, [252, 218, 226]),
            (0.66, [124, 190, 122]),
            (0.82, [28, 78, 52]),
        ],
    },
    Def {
        name: "peacock",
        blurb: "teal to gold across the wheel, jewelled",
        stops: &[
            (0.00, [6, 20, 28]),
            (0.18, [12, 82, 96]),
            (0.36, [26, 150, 148]),
            (0.52, [162, 208, 154]),
            (0.66, [238, 194, 88]),
            (0.82, [116, 58, 32]),
        ],
    },
    Def {
        name: "neon",
        blurb: "black, magenta, cyan, white — maximum chroma",
        stops: &[
            (0.00, [4, 0, 10]),
            (0.18, [126, 0, 128]),
            (0.34, [240, 40, 200]),
            (0.50, [255, 255, 255]),
            (0.66, [40, 224, 240]),
            (0.82, [0, 48, 96]),
        ],
    },
    Def {
        name: "spectrum",
        blurb: "the full hue circle — honest about being cyclic",
        stops: &[
            (0.000, [214, 44, 44]),
            (0.167, [220, 156, 36]),
            (0.333, [188, 210, 48]),
            (0.500, [48, 190, 120]),
            (0.667, [44, 128, 214]),
            (0.833, [140, 62, 190]),
        ],
    },
    Def {
        name: "ash",
        blurb: "neutral greyscale: read structure with no hue at all",
        stops: &[
            (0.00, [8, 8, 10]),
            (0.25, [64, 64, 70]),
            (0.45, [140, 140, 148]),
            (0.62, [220, 220, 226]),
            (0.78, [252, 252, 254]),
            (0.92, [40, 40, 46]),
        ],
    },
    Def {
        name: "cyanotype",
        blurb: "blueprint duotone: Prussian blue and paper",
        stops: &[
            (0.00, [4, 12, 30]),
            (0.28, [18, 52, 104]),
            (0.50, [58, 108, 164]),
            (0.68, [166, 196, 220]),
            (0.82, [238, 240, 232]),
            (0.94, [12, 30, 66]),
        ],
    },
    Def {
        name: "sepia",
        blurb: "old print: warm brown-black through paper white",
        stops: &[
            (0.00, [18, 12, 8]),
            (0.26, [76, 52, 32]),
            (0.48, [150, 116, 78]),
            (0.66, [216, 190, 152]),
            (0.80, [248, 240, 224]),
            (0.92, [50, 34, 22]),
        ],
    },
    Def {
        name: "abyss",
        blurb: "almost all dark, one narrow bioluminescent band",
        stops: &[
            (0.00, [2, 3, 8]),
            (0.30, [6, 14, 28]),
            (0.46, [16, 52, 74]),
            (0.56, [92, 206, 198]),
            (0.64, [222, 252, 246]),
            (0.76, [14, 40, 60]),
            (0.90, [3, 5, 12]),
        ],
    },
];

/// Every library palette, in list order.
pub fn all() -> Vec<Palette> {
    LIBRARY.iter().map(build).collect()
}

/// Names, in list order.
pub fn names() -> Vec<&'static str> {
    LIBRARY.iter().map(|d| d.name).collect()
}

/// One-line description of a library palette.
pub fn blurb(name: &str) -> Option<&'static str> {
    LIBRARY.iter().find(|d| d.name == name).map(|d| d.blurb)
}

/// Look a palette up by name (case-insensitive).
pub fn get(name: &str) -> Option<Palette> {
    LIBRARY
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(name))
        .map(build)
}

/// Number of palettes in the library.
pub fn len() -> usize {
    LIBRARY.len()
}

fn build(def: &Def) -> Palette {
    let mut p = Palette::from_stops(
        def.stops
            .iter()
            .map(|&(at, rgb)| Stop { at, color: from_srgb8(rgb) })
            .collect(),
    );
    p.name = Some(def.name.to_string());
    p
}

/// A random library palette, for `--random-palette` and mutation.
pub fn random(rng: &mut impl rand::Rng) -> Palette {
    build(&LIBRARY[rng.gen_range(0..LIBRARY.len())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_resolvable() {
        let mut seen = std::collections::HashSet::new();
        for name in names() {
            assert!(seen.insert(name), "duplicate library palette '{name}'");
            assert!(get(name).is_some(), "'{name}' does not resolve");
            assert!(get(&name.to_uppercase()).is_some(), "'{name}' lookup is case-sensitive");
            assert!(blurb(name).is_some(), "'{name}' has no blurb");
        }
        assert!(get("no-such-palette").is_none());
    }

    #[test]
    fn stops_are_sorted_and_in_range() {
        for def in LIBRARY {
            assert!(def.stops.len() >= 2, "'{}' needs at least two stops", def.name);
            for w in def.stops.windows(2) {
                assert!(w[0].0 < w[1].0, "'{}' stops are out of order", def.name);
            }
            for &(at, _) in def.stops {
                assert!((0.0..1.0).contains(&at), "'{}' stop at {at} is out of range", def.name);
            }
        }
    }

    /// The two rules in the module docs, enforced rather than hoped for.
    #[test]
    fn every_palette_has_a_luminance_sweep_and_no_seam() {
        for p in all() {
            let name = p.name.clone().unwrap();
            let (_, swing) = p.luminance_profile();
            assert!(swing > 0.15, "'{name}' is too flat to shade with (swing {swing:.3})");

            // Cyclic: the wrap from index 255 back to 0 must not be a visibly
            // bigger jump than the gradient makes step to step elsewhere. The
            // seam step is one of the 256, so compare it to the mean of the
            // rest rather than to a max that includes it.
            let steps: Vec<f32> = (0..256)
                .map(|i| (p.sample(i as f32 / 256.0) - p.sample((i + 1) as f32 / 256.0)).length())
                .collect();
            let seam = steps[255];
            let elsewhere = steps[..255].iter().sum::<f32>() / 255.0;
            assert!(
                seam <= elsewhere * 4.0 + 1e-4,
                "'{name}' has a seam at index 0 ({seam:.4} vs {elsewhere:.4} typical)"
            );
        }
    }
}
