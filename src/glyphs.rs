//! A 5x7 bitmap font, and label drawing straight into an RGBA contact sheet.
//!
//! Contact sheets print their per-tile parameters to stdout, which is right for
//! a human reading a terminal and wrong for anything reading the PNG — an agent
//! looking at a 3x3 sheet otherwise has to count tiles and cross-reference a
//! block of text. A sheet should say what it is.
//!
//! Why a hand-rolled font rather than `ab_glyph` and the vendored TTFs: the
//! offline path is pure wgpu and arithmetic with no text machinery in it at
//! all, and these labels are never longer than a line of ASCII. A real
//! rasteriser would be better typography and a new dependency plus a
//! font-loading path, for `ABSFOLD=0.30`. Keep it simple.
//!
//! Everything here is CPU pixel work on a plain buffer, so it is tested without
//! a GPU.

/// Colour of label text: the same amber the running app uses for its
/// world-anchored transform names (`src/ui/labels.rs`), so an annotation looks
/// like an annotation and not like part of the artwork.
pub const LABEL_RGB: [u8; 3] = [255, 199, 89];

const GLYPH_W: u32 = 5;
const GLYPH_H: u32 = 7;
/// Blank columns between glyphs, in font pixels.
const TRACKING: u32 = 1;

/// Rows of 5 bits, MSB-of-5 leftmost.
#[rustfmt::skip]
fn glyph(c: char) -> Option<[u8; 7]> {
    let g: [u8; 7] = match c.to_ascii_uppercase() {
        ' ' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        ',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b00100, 0b01000],
        ':' => [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '=' => [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000],
        '+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '_' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        '<' => [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010],
        '>' => [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000],
        '#' => [0b01010, 0b11111, 0b01010, 0b01010, 0b11111, 0b01010, 0b00000],
        '%' => [0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011],
        '*' => [0b00000, 0b10101, 0b01110, 0b11111, 0b01110, 0b10101, 0b00000],
        '°' => [0b01100, 0b10010, 0b01100, 0b00000, 0b00000, 0b00000, 0b00000],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        '"' => [0b01010, 0b01010, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
        '$' => [0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100],
        '&' => [0b01000, 0b10100, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101],
        '\'' => [0b00100, 0b00100, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
        ';' => [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b00100, 0b01000],
        '@' => [0b01110, 0b10001, 0b10111, 0b10101, 0b10111, 0b10000, 0b01110],
        '\\' => [0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001],
        '^' => [0b00100, 0b01010, 0b10001, 0b00000, 0b00000, 0b00000, 0b00000],
        '`' => [0b01000, 0b00100, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
        '{' => [0b00110, 0b01000, 0b01000, 0b11000, 0b01000, 0b01000, 0b00110],
        '|' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        '}' => [0b01100, 0b00010, 0b00010, 0b00011, 0b00010, 0b00010, 0b01100],
        '~' => [0b00000, 0b00000, 0b01000, 0b10101, 0b00010, 0b00000, 0b00000],
        // Non-ASCII that the label strings actually use: `T2 scale x1.26`
        // and `T1 rotate 17 deg about (...)` in src/mutate.rs, and the orbit
        // grid's `yaw 45.0 deg`.
        '\u{d7}' => [0b00000, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b00000],
        '?' => [0b01110, 0b10001, 0b00010, 0b00100, 0b00100, 0b00000, 0b00100],
        _ => return None,
    };
    Some(g)
}

/// Width in buffer pixels that `text` needs at `scale`.
pub fn text_width(text: &str, scale: u32) -> u32 {
    let n = text.chars().count() as u32;
    if n == 0 {
        return 0;
    }
    (n * (GLYPH_W + TRACKING) - TRACKING) * scale
}

/// Height in buffer pixels of one line at `scale`.
pub fn text_height(scale: u32) -> u32 {
    GLYPH_H * scale
}

/// How many leading characters of `text` fit in `avail` buffer pixels.
fn fits(text: &str, scale: u32, avail: u32) -> usize {
    let step = (GLYPH_W + TRACKING) * scale;
    if avail < GLYPH_W * scale {
        return 0;
    }
    // n glyphs occupy n*step - TRACKING*scale
    let n = ((avail + TRACKING * scale) / step) as usize;
    n.min(text.chars().count())
}

/// Draw `text` into an RGBA buffer at (`ox`, `oy`), on a dark plate so it stays
/// legible over both a near-black background and a bright core.
///
/// Clips to `max_w` (the tile width) and to the buffer; a label too long for
/// its tile is truncated rather than running into the neighbour. Label pixels
/// are written opaque, since an annotation that inherits the artwork's
/// transparency would be unreadable in exactly the case it is needed.
pub fn draw_label(
    buf: &mut [u8],
    buf_w: u32,
    buf_h: u32,
    ox: u32,
    oy: u32,
    text: &str,
    scale: u32,
    max_w: u32,
) {
    let scale = scale.max(1);
    let pad = 2 * scale;

    // Truncate to what the tile can hold, leaving room for the plate padding.
    let avail = max_w.saturating_sub(2 * pad);
    let keep = fits(text, scale, avail);
    if keep == 0 {
        return;
    }
    // Mark a cut, so a label that ran out of tile reads as truncated rather
    // than as a parameter that happens to end oddly. Mutation labels are long
    // enough that this happens often.
    let total = text.chars().count();
    let shown: String = if keep < total {
        text.chars().take(keep.saturating_sub(1)).chain(['>']).collect()
    } else {
        text.chars().collect()
    };

    let tw = text_width(&shown, scale);
    let th = text_height(scale);
    let plate_w = tw + 2 * pad;
    let plate_h = th + 2 * pad;

    // Plate: darken what is already there rather than painting flat black, so
    // a label over structure still shows the structure faintly under it.
    for py in 0..plate_h {
        for px in 0..plate_w {
            let (x, y) = (ox + px, oy + py);
            if x >= buf_w || y >= buf_h {
                continue;
            }
            let i = ((y * buf_w + x) * 4) as usize;
            for c in 0..3 {
                buf[i + c] = (buf[i + c] as u32 * 35 / 100) as u8;
            }
            buf[i + 3] = buf[i + 3].max(200);
        }
    }

    // Glyphs
    for (gi, ch) in shown.chars().enumerate() {
        let g = glyph(ch).or_else(|| glyph('?')).expect("'?' is in the font");
        let gx = ox + pad + gi as u32 * (GLYPH_W + TRACKING) * scale;
        let gy = oy + pad;
        for (row, bits) in g.iter().enumerate() {
            for col in 0..GLYPH_W {
                // bit 4 is the leftmost column
                if bits & (1 << (GLYPH_W - 1 - col)) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let x = gx + col * scale + sx;
                        let y = gy + row as u32 * scale + sy;
                        if x >= buf_w || y >= buf_h {
                            continue;
                        }
                        let i = ((y * buf_w + x) * 4) as usize;
                        buf[i] = LABEL_RGB[0];
                        buf[i + 1] = LABEL_RGB[1];
                        buf[i + 2] = LABEL_RGB[2];
                        buf[i + 3] = 255;
                    }
                }
            }
        }
    }
}

/// Label scale that suits a tile of this width: small tiles get 1x, big ones
/// more, so a 4K sheet's labels are not microscopic.
pub fn scale_for_tile(tile_w: u32) -> u32 {
    match tile_w {
        0..=319 => 1,
        320..=799 => 2,
        800..=1599 => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(w: u32, h: u32) -> Vec<u8> {
        vec![0u8; (w * h * 4) as usize]
    }

    fn count_label_px(buf: &[u8]) -> usize {
        buf.chunks(4)
            .filter(|p| p[0] == LABEL_RGB[0] && p[1] == LABEL_RGB[1] && p[2] == LABEL_RGB[2])
            .count()
    }

    #[test]
    fn draws_something_in_the_label_colour() {
        let (w, h) = (200, 40);
        let mut buf = blank(w, h);
        draw_label(&mut buf, w, h, 0, 0, "AB 12", 1, w);
        assert!(count_label_px(&buf) > 20, "expected glyph pixels");
    }

    #[test]
    fn writes_nothing_outside_the_plate() {
        let (w, h) = (200, 40);
        let mut buf = blank(w, h);
        draw_label(&mut buf, w, h, 0, 0, "X", 1, w);
        let plate_h = text_height(1) + 4;
        // rows below the plate are untouched
        for y in plate_h..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                assert_eq!(&buf[i..i + 4], &[0, 0, 0, 0], "row {y} col {x} was touched");
            }
        }
    }

    #[test]
    fn clips_at_the_buffer_edge_without_panicking() {
        let (w, h) = (40, 20);
        let mut buf = blank(w, h);
        // origin near the far corner: every write should be clipped away
        draw_label(&mut buf, w, h, w - 3, h - 3, "LONG TEXT HERE", 2, 200);
        // no panic is the assertion; buffer stayed the right size
        assert_eq!(buf.len(), (w * h * 4) as usize);
    }

    #[test]
    fn truncates_to_the_tile_width_instead_of_overrunning() {
        let (w, h) = (300, 40);
        let narrow = 40;
        let mut buf = blank(w, h);
        draw_label(&mut buf, w, h, 0, 0, "ABCDEFGHIJKLMNOP", 1, narrow);
        // nothing drawn beyond the tile it belongs to
        for y in 0..h {
            for x in narrow..w {
                let i = ((y * w + x) * 4) as usize;
                assert_eq!(&buf[i..i + 4], &[0, 0, 0, 0], "spilled at col {x}");
            }
        }
    }

    #[test]
    fn a_tile_too_narrow_for_one_glyph_draws_nothing() {
        let (w, h) = (300, 40);
        let mut buf = blank(w, h);
        draw_label(&mut buf, w, h, 0, 0, "ABC", 1, 4);
        assert_eq!(count_label_px(&buf), 0);
    }

    #[test]
    fn unknown_characters_fall_back_and_do_not_panic() {
        let (w, h) = (200, 40);
        let mut buf = blank(w, h);
        draw_label(&mut buf, w, h, 0, 0, "A\u{2603}B", 1, w);
        assert!(count_label_px(&buf) > 10);
    }

    #[test]
    fn the_degree_sign_has_a_glyph_since_orbit_grids_use_it() {
        assert!(glyph('°').is_some());
    }

    #[test]
    fn lowercase_maps_onto_uppercase() {
        assert_eq!(glyph('a'), glyph('A'));
    }

    #[test]
    fn text_width_matches_what_gets_drawn() {
        // one glyph at scale 1 is GLYPH_W wide, two are GLYPH_W*2 + TRACKING
        assert_eq!(text_width("A", 1), 5);
        assert_eq!(text_width("AB", 1), 11);
        assert_eq!(text_width("AB", 2), 22);
        assert_eq!(text_width("", 1), 0);
    }

    #[test]
    fn plate_darkens_existing_pixels_rather_than_erasing_them() {
        let (w, h) = (60, 20);
        let mut buf = vec![200u8; (w * h * 4) as usize];
        draw_label(&mut buf, w, h, 0, 0, " ", 1, w);
        // a plate pixel that isn't a glyph: darkened, not zeroed
        let i = 0;
        assert!(buf[i] > 0 && buf[i] < 200, "got {}", buf[i]);
    }

    #[test]
    fn scale_grows_with_tile_size() {
        assert_eq!(scale_for_tile(200), 1);
        assert_eq!(scale_for_tile(480), 2);
        assert_eq!(scale_for_tile(960), 3);
        assert!(scale_for_tile(3840) >= 4);
    }

    #[test]
    fn every_printable_ascii_has_a_glyph() {
        let missing: Vec<char> = (0x20u8..=0x7e)
            .map(|b| b as char)
            .filter(|c| glyph(*c).is_none())
            .collect();
        assert!(missing.is_empty(), "no glyph for {missing:?}");
    }

    #[test]
    fn the_non_ascii_the_labels_use_has_glyphs() {
        // src/mutate.rs writes "scale x1.26" with U+00D7 and "hue +17 deg"
        // with U+00B0; the orbit grid writes degrees too. A '?' box in the
        // middle of a label is the symptom that brought these in.
        for c in ['\u{d7}', '\u{b0}'] {
            assert!(glyph(c).is_some(), "no glyph for U+{:04X}", c as u32);
        }
    }

    #[test]
    fn a_truncated_label_is_marked_as_cut() {
        // narrow enough to hold ~5 glyphs
        let (w, h) = (200, 40);
        let mut buf = blank(w, h);
        draw_label(&mut buf, w, h, 0, 0, "ABCDEFGHIJ", 1, 40);
        // the '>' marker is drawn: compare against the same width of plain text,
        // which must differ
        let mut buf2 = blank(w, h);
        draw_label(&mut buf2, w, h, 0, 0, "ABCDE", 1, 40);
        assert_ne!(buf, buf2, "truncated label should not equal its plain prefix");
    }

    #[test]
    fn a_label_that_fits_is_not_marked() {
        let (w, h) = (200, 40);
        let mut a = blank(w, h);
        draw_label(&mut a, w, h, 0, 0, "ABC", 1, 200);
        let mut b = blank(w, h);
        draw_label(&mut b, w, h, 0, 0, "ABC", 1, 200);
        assert_eq!(a, b);
        // and it really drew all three glyphs, not two plus a marker
        let mut c = blank(w, h);
        draw_label(&mut c, w, h, 0, 0, "AB>", 1, 200);
        assert_ne!(a, c);
    }
}
