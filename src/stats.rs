//! `--stats <image>`: numbers about a rendered picture, for judging it without
//! looking at it.
//!
//! Two things this has to get right that a naive pixel census gets wrong, and
//! the first version did:
//!
//! - **"Empty" means the background, not black.** Scenes choose their
//!   background (CRAFT §2.5), and a warm charcoal ground has no pure-black
//!   pixels at all, so counting `#000000` reported 0% empty for a frame that
//!   was mostly empty. The background is read from the scene embedded in the
//!   PNG (every render carries one, `record.rs`) and, for an image made
//!   elsewhere, taken as the commonest colour around the border. Colour
//!   statistics are then taken over the *lit* pixels only, so the ground's own
//!   tint doesn't masquerade as a hue in the picture.
//! - **Two hues are not a monochrome trap.** The trap (CRAFT §2.3) is a colour
//!   index pinned in one place, which renders as *one narrow* hue. A picture
//!   whose colour sits in two separated families — jade and gold, say — is a
//!   two-colour picture, and flagging it sends the reader after a defect that
//!   isn't there. So the histogram is grouped into families (circular runs of
//!   occupied bins), and the note fires only on a single narrow one.

use image::RgbaImage;

/// Where the background colour came from, which the report says, because a
/// guessed background is a weaker basis for "empty" than a recorded one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BgSource {
    /// Read from the `fracturize:scene` chunk the renderer embedded
    Recorded,
    /// The commonest border colour — for images from elsewhere
    Border,
}

/// A hue family: a circular run of adjacent occupied histogram bins.
#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    /// Circular mean hue, degrees
    pub hue: f32,
    /// Bins the run spans (each 22.5°)
    pub width: usize,
    /// Share of the coloured lit pixels
    pub share: f64,
}

pub struct Stats {
    pub width: u32,
    pub height: u32,
    pub background: [u8; 3],
    pub bg_source: BgSource,
    pub mean_lum: f64,
    pub max_lum: f32,
    pub clipped_pct: f64,
    /// Background or fully transparent
    pub empty_pct: f64,
    pub lit: u64,
    /// Of the lit pixels, those with enough chroma to have a hue
    pub colored: u64,
    pub mean_sat: f64,
    pub hue_hist: [u64; BINS],
    pub families: Vec<Family>,
}

pub const BINS: usize = 16;
/// Per-channel distance, in 8-bit codes, still counted as the background:
/// one code of rounding either side, plus one for dithered encoders.
const BG_TOLERANCE: i32 = 2;
/// A bin holding less than this share of the coloured pixels is noise, not a
/// family. 3% keeps a deliberate accent (Kintsugi's gold is ~8%) and drops
/// the fringe where a hue family's edge spills into its neighbour.
const OCCUPIED: f64 = 0.03;
/// A single family this narrow or narrower (3 bins, 67.5°) is one hue.
const NARROW_BINS: usize = 3;

/// The background as the embedded scene records it, in 8-bit sRGB.
///
/// `None` for anything the renderer didn't write, or whose record doesn't
/// parse — the caller falls back to the border.
pub fn recorded_background(path: &std::path::Path) -> Option<[u8; 3]> {
    let source = crate::scene_source::read(path).ok()?;
    if source.kind != crate::scene_source::SourceKind::Png {
        return None;
    }
    let scene: crate::scene::SceneFile = toml::from_str(&source.text).ok()?;
    let b = scene.meta.background;
    Some([0, 1, 2].map(|i| crate::palette::linear_to_srgb8(b[i] as f32)))
}

/// The commonest colour along the image's border.
///
/// A framed attractor rarely touches the edge, so the border is mostly ground.
/// Exact-colour mode rather than a mean, because the few lit pixels that do
/// reach the edge would drag a mean off the ground and make it match nothing.
pub fn border_background(img: &RgbaImage) -> [u8; 3] {
    let (w, h) = img.dimensions();
    let mut counts = std::collections::HashMap::<[u8; 3], u32>::new();
    let mut add = |x: u32, y: u32| {
        let p = img.get_pixel(x, y).0;
        *counts.entry([p[0], p[1], p[2]]).or_default() += 1;
    };
    for x in 0..w {
        add(x, 0);
        add(x, h - 1);
    }
    for y in 0..h {
        add(0, y);
        add(w - 1, y);
    }
    counts
        .into_iter()
        .max_by_key(|&(c, n)| (n, c))
        .map(|(c, _)| c)
        .unwrap_or([0, 0, 0])
}

pub fn analyze(img: &RgbaImage, background: [u8; 3], bg_source: BgSource) -> Stats {
    let (width, height) = img.dimensions();
    let total = (width as u64 * height as u64).max(1) as f64;

    let mut sum_lum = 0.0f64;
    let mut max_lum = 0.0f32;
    let mut clipped = 0u64;
    let mut empty = 0u64;
    let mut lit = 0u64;
    let mut colored = 0u64;
    let mut sat_sum = 0.0f64;
    let mut hue_hist = [0u64; BINS];

    for p in img.pixels() {
        let [r, g, b, a] = p.0;
        let [rf, gf, bf] = [r, g, b].map(|c| c as f32 / 255.0);
        let lum = 0.2126 * rf + 0.7152 * gf + 0.0722 * bf;
        sum_lum += lum as f64;
        max_lum = max_lum.max(lum);
        if r == 255 && g == 255 && b == 255 {
            clipped += 1;
        }
        let near_bg = [r, g, b]
            .iter()
            .zip(background)
            .all(|(&c, bg)| (c as i32 - bg as i32).abs() <= BG_TOLERANCE);
        if a == 0 || near_bg {
            empty += 1;
            continue;
        }
        lit += 1;

        let max_c = rf.max(gf).max(bf);
        let min_c = rf.min(gf).min(bf);
        let delta = max_c - min_c;
        if delta > 0.05 {
            colored += 1;
            sat_sum += (delta / max_c.max(0.001)) as f64;
            let hue = if max_c == rf {
                60.0 * (((gf - bf) / delta).rem_euclid(6.0))
            } else if max_c == gf {
                60.0 * (((bf - rf) / delta) + 2.0)
            } else {
                60.0 * (((rf - gf) / delta) + 4.0)
            };
            let bin = ((hue / 360.0 * BINS as f32) as usize).min(BINS - 1);
            hue_hist[bin] += 1;
        }
    }

    Stats {
        width,
        height,
        background,
        bg_source,
        mean_lum: sum_lum / total,
        max_lum,
        clipped_pct: clipped as f64 / total * 100.0,
        empty_pct: empty as f64 / total * 100.0,
        lit,
        colored,
        mean_sat: if colored > 0 { sat_sum / colored as f64 } else { 0.0 },
        hue_hist,
        families: families(&hue_hist),
    }
}

/// Group the hue histogram into families: circular runs of occupied bins.
pub fn families(hist: &[u64; BINS]) -> Vec<Family> {
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return Vec::new();
    }
    let occupied: Vec<bool> =
        hist.iter().map(|&c| c as f64 / total as f64 >= OCCUPIED).collect();
    if occupied.iter().all(|&o| o) {
        // Every hue at once: one family with no centre worth naming.
        return vec![Family { hue: 0.0, width: BINS, share: 1.0 }];
    }
    // Start just after an unoccupied bin, so no run is split across the wrap.
    let start = (0..BINS).find(|&i| !occupied[i]).unwrap() + 1;
    let mut out = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    for k in 0..=BINS {
        let i = (start + k) % BINS;
        if k < BINS && occupied[i] {
            run.push(i);
        } else if !run.is_empty() {
            let n: u64 = run.iter().map(|&i| hist[i]).sum();
            let (mut sx, mut sy) = (0.0f64, 0.0f64);
            for &i in &run {
                let a = ((i as f64 + 0.5) / BINS as f64) * std::f64::consts::TAU;
                sx += a.cos() * hist[i] as f64;
                sy += a.sin() * hist[i] as f64;
            }
            let hue = (sy.atan2(sx).to_degrees().rem_euclid(360.0)) as f32;
            out.push(Family { hue, width: run.len(), share: n as f64 / total as f64 });
            run.clear();
        }
    }
    out.sort_by(|a, b| b.share.total_cmp(&a.share));
    out
}

/// The nearest of twelve hue names, 30° apart.
pub fn hue_name(deg: f32) -> &'static str {
    const NAMES: [&str; 12] = [
        "red", "orange", "yellow", "chartreuse", "green", "spring green", "cyan", "azure",
        "blue", "violet", "magenta", "rose",
    ];
    NAMES[((deg.rem_euclid(360.0) / 30.0).round() as usize) % 12]
}

impl Stats {
    /// The one narrow family the monochrome note is about, if that is what
    /// this picture has.
    pub fn monochrome(&self) -> bool {
        self.lit > 0
            && self.colored as f64 / self.lit as f64 > 0.1
            && self.families.len() == 1
            && self.families[0].width <= NARROW_BINS
    }
}

pub fn print(path: &std::path::Path) {
    let img = match image::open(path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("Failed to open '{}': {}", path.display(), e);
            std::process::exit(1);
        }
    };
    if img.width() == 0 || img.height() == 0 {
        eprintln!("Image has zero pixels");
        std::process::exit(1);
    }
    let (bg, src) = match recorded_background(path) {
        Some(bg) => (bg, BgSource::Recorded),
        None => (border_background(&img), BgSource::Border),
    };
    let s = analyze(&img, bg, src);
    let total = s.width as f64 * s.height as f64;

    println!("stats    {}", path.display());
    println!("         {}x{} ({:.1}M pixels)", s.width, s.height, total / 1e6);

    println!();
    println!(
        "ground   #{:02x}{:02x}{:02x}   {}",
        s.background[0],
        s.background[1],
        s.background[2],
        match s.bg_source {
            BgSource::Recorded => "from the scene embedded in the image",
            BgSource::Border => "guessed: the commonest border colour (no embedded scene)",
        }
    );
    println!(
        "         empty {:>5.1}%   lit {:>5.1}%",
        s.empty_pct,
        s.lit as f64 / total * 100.0
    );
    if s.empty_pct > 80.0 {
        println!("         note: >80% empty — attractor may be too small in frame");
    }

    println!();
    println!("lumin    mean {:.3}   max {:.3}", s.mean_lum, s.max_lum);
    println!("         clipped {:>5.1}%", s.clipped_pct);
    if s.clipped_pct > 5.0 {
        println!("         note: >5% clipped — consider lowering exposure");
    }

    println!();
    if s.colored == 0 {
        println!("color    no saturated lit pixels (grayscale or empty)");
        return;
    }
    println!(
        "color    mean saturation {:.2}   coloured {:>5.1}% of lit pixels",
        s.mean_sat,
        s.colored as f64 / s.lit.max(1) as f64 * 100.0
    );
    let fams: Vec<String> = s
        .families
        .iter()
        .map(|f| {
            if f.width == BINS {
                "every hue".to_string()
            } else {
                format!("{} {:.0}%", hue_name(f.hue), f.share * 100.0)
            }
        })
        .collect();
    println!(
        "         {} hue famil{}: {}",
        s.families.len(),
        if s.families.len() == 1 { "y" } else { "ies" },
        fams.join(", ")
    );
    if s.monochrome() {
        println!(
            "         note: one narrow hue — if this should be multicoloured, the colour \
             index may be pinned (CRAFT §2.3)"
        );
    }
    let max_bin = *s.hue_hist.iter().max().unwrap_or(&1);
    let sparkline: String = s
        .hue_hist
        .iter()
        .map(|&c| {
            let level = (c as f64 / max_bin as f64 * 7.0) as usize;
            ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][level.min(7)]
        })
        .collect();
    println!("         hue histogram: {}", sparkline);
    // Tick letters sit under the bin their hue actually lands in:
    // 16 bins of 22.5°, so red 0° -> 0, yellow 60° -> 2, green
    // 120° -> 5, cyan 180° -> 8, blue 240° -> 10, magenta 300° -> 13.
    let mut ticks = [b' '; BINS];
    for (hue_deg, letter) in
        [(0, b'R'), (60, b'Y'), (120, b'G'), (180, b'C'), (240, b'B'), (300, b'M')]
    {
        ticks[hue_deg * BINS / 360] = letter;
    }
    println!("                        {}", String::from_utf8_lossy(&ticks).trim_end());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist(pairs: &[(usize, u64)]) -> [u64; BINS] {
        let mut h = [0; BINS];
        for &(i, n) in pairs {
            h[i] = n;
        }
        h
    }

    #[test]
    fn two_separated_hues_are_two_families_and_not_monochrome() {
        // jade (bins 6-8) with a gold accent (bin 2): the Kintsugi shape
        let f = families(&hist(&[(2, 80), (6, 300), (7, 400), (8, 220)]));
        assert_eq!(f.len(), 2, "{f:?}");
        assert!((f[1].share - 0.08).abs() < 1e-9);
    }

    #[test]
    fn a_run_across_the_wrap_is_one_family() {
        let f = families(&hist(&[(15, 100), (0, 100), (1, 100)]));
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].width, 3);
        assert_eq!(hue_name(f[0].hue), "red");
    }

    #[test]
    fn a_trace_below_the_floor_is_not_a_family() {
        let f = families(&hist(&[(5, 1000), (11, 10)]));
        assert_eq!(f.len(), 1);
    }

    fn image(fill: [u8; 4], lit: &[(u32, [u8; 4])]) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(20, 10, image::Rgba(fill));
        for &(x, c) in lit {
            img.put_pixel(x, 5, image::Rgba(c));
        }
        img
    }

    #[test]
    fn empty_is_measured_against_the_background_not_black() {
        let bg = [0x26, 0x1f, 0x1c, 255];
        let img = image(bg, &[(3, [200, 60, 40, 255]), (4, [0x27, 0x1f, 0x1c, 255])]);
        assert_eq!(border_background(&img), [0x26, 0x1f, 0x1c]);
        let s = analyze(&img, [0x26, 0x1f, 0x1c], BgSource::Border);
        // one real pixel lit; the one-code-off neighbour is still ground
        assert_eq!(s.lit, 1);
        assert!((s.empty_pct - 199.0 / 2.0).abs() < 1e-9);
    }

    #[test]
    fn one_narrow_hue_is_flagged_and_two_are_not() {
        let bg = [10, 10, 10, 255];
        let blue = [40, 80, 220, 255];
        let gold = [230, 170, 40, 255];
        let mono = image(bg, &[(3, blue), (4, blue), (5, blue)]);
        assert!(analyze(&mono, [10, 10, 10], BgSource::Border).monochrome());
        let two = image(bg, &[(3, blue), (4, blue), (5, gold)]);
        assert!(!analyze(&two, [10, 10, 10], BgSource::Border).monochrome());
    }

    #[test]
    fn a_render_carries_its_background() {
        // A PNG with a scene chunk, as offline.rs writes one.
        let dir = std::env::temp_dir().join(format!("fracturize-stats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("r.png");
        let scene = "[meta]\nname = \"t\"\nbackground = [0.02, 0.014, 0.012]\n\
                     [[transform]]\ntranslation = [0.0, 0.0, 0.0]\ncolor = [1.0, 1.0, 1.0]\n";
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 2, 2);
        enc.set_color(png::ColorType::Rgba);
        enc.add_itxt_chunk(crate::record::KEY_SCENE.to_string(), scene.to_string()).unwrap();
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[0u8; 16]).unwrap();
        drop(w);
        let want = [0.02f32, 0.014, 0.012].map(crate::palette::linear_to_srgb8);
        assert_eq!(recorded_background(&path), Some(want));
        std::fs::remove_dir_all(&dir).ok();
    }
}
