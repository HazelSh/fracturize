//! Animated output: the container both video formats share, the colour
//! conversion they both feed, and the choice between them.
//!
//! Two formats come out of here, and the difference that matters is the codec,
//! not the box layout:
//!
//! * **`.avif`** — AV1 in an ISOBMFF image sequence (`avis` brand). The
//!   "modern GIF": one file, loops in a browser, no audio track. Encoded by
//!   rav1e in `src/avif.rs`.
//! * **`.mp4`** — H.264 in an ordinary MP4 (`isom` brand). Encoded by
//!   openh264 in `src/h264.rs`.
//!
//! The second exists because AVIF is the wrong answer for *posting*. Platforms
//! that loop short clips overwhelmingly ingest H.264, and several reject AV1
//! outright; muxing our existing AV1 into `.mp4` would have been nearly free
//! and would have produced a file that looks right locally and fails on
//! upload. So `.mp4` means H.264 here deliberately, not "the same video with a
//! different extension".
//!
//! What they genuinely share is this module: an animated AVIF *is* an
//! MP4-shaped file, so one muxer writes both, parameterised by the handful of
//! places they differ (brands, handler type, sample entry, and whether `moov`
//! leads). Frames arrive as RGBA8 — the offline renderer's readback format —
//! and are converted once, here, to BT.709 limited-range 4:2:0, so the two
//! formats are colour-identical by construction rather than by coincidence.

use std::path::Path;

/// Which animated file to write.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Format {
    /// Animated AVIF (AV1). Loops like a GIF, at a fraction of the size.
    #[default]
    Avif,
    /// MP4 (H.264). What upload pipelines actually accept.
    Mp4,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Avif => "avif",
            Format::Mp4 => "mp4",
        }
    }

    /// The codec name people will see in ffprobe, for logs and tooltips
    pub fn codec_label(self) -> &'static str {
        match self {
            Format::Avif => "AV1",
            Format::Mp4 => "H.264",
        }
    }

    /// Pick the format from an output path's extension.
    ///
    /// `None` means "not an animation" — the caller renders a still instead,
    /// which is how `--render` has always decided what it was being asked for.
    pub fn from_path(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?;
        if ext.eq_ignore_ascii_case("avif") {
            Some(Format::Avif)
        } else if ext.eq_ignore_ascii_case("mp4") {
            Some(Format::Mp4)
        } else {
            None
        }
    }

    /// `moov` before `mdat`, so a player can start without reading to the end
    /// of the file ("faststart").
    ///
    /// On for MP4 because that is what upload pipelines and progressive
    /// download expect. Left off for AVIF: the existing output is
    /// `ftyp/mdat/moov`, nothing reads AVIFs progressively, and rearranging
    /// bytes that already work buys nothing.
    fn faststart(self) -> bool {
        matches!(self, Format::Mp4)
    }
}

/// One encoded sample — an AV1 temporal unit, or a frame's H.264 NAL units —
/// ready for the container
pub struct Sample {
    pub data: Vec<u8>,
    pub sync: bool,
}

/// A frame in planar 8-bit 4:2:0, which is what both encoders take
pub struct Yuv420 {
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// RGBA8 -> BT.709 limited-range 4:2:0. Chroma is averaged over each 2x2 block.
///
/// Both encoders are configured to *declare* BT.709 limited — rav1e's
/// `color_description`, openh264's VUI — so this is the one place the actual
/// numbers are produced, and the two formats cannot drift apart.
pub fn rgba_to_yuv420(rgba: &[u8], width: usize, height: usize) -> Yuv420 {
    let (w, h) = (width, height);
    assert_eq!(rgba.len(), w * h * 4, "frame size mismatch");
    let mut yp = vec![0u8; w * h];
    let mut up = vec![0u8; (w / 2) * (h / 2)];
    let mut vp = vec![0u8; (w / 2) * (h / 2)];
    for by in 0..h / 2 {
        for bx in 0..w / 2 {
            let (mut cb_acc, mut cr_acc) = (0.0f32, 0.0f32);
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let (x, y) = (bx * 2 + dx, by * 2 + dy);
                let i = (y * w + x) * 4;
                let (r, g, b) = (rgba[i] as f32, rgba[i + 1] as f32, rgba[i + 2] as f32);
                let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                yp[y * w + x] = (16.0 + luma * (219.0 / 255.0)).round().clamp(16.0, 235.0) as u8;
                cb_acc += (b - luma) / 1.8556;
                cr_acc += (r - luma) / 1.5748;
            }
            let c = by * (w / 2) + bx;
            up[c] = (128.0 + cb_acc * 0.25 * (224.0 / 255.0)).round().clamp(16.0, 240.0) as u8;
            vp[c] = (128.0 + cr_acc * 0.25 * (224.0 / 255.0)).round().clamp(16.0, 240.0) as u8;
        }
    }
    Yuv420 { y: yp, u: up, v: vp, width: w, height: h }
}

/// One encoder behind one interface, chosen by [`Format`].
///
/// An enum rather than a trait object: there are exactly two, the choice is
/// made once at the top of a render, and the codec-specific setup (quantizer
/// scales, keyframe intervals) has nothing left worth sharing once the colour
/// conversion and the muxer are factored out above.
pub enum AnimationEncoder {
    Av1(crate::avif::Av1Encoder),
    H264(crate::h264::H264Encoder),
}

impl AnimationEncoder {
    /// `quality` is 0-100 (higher = better; ~60 is a good default). `speed` is
    /// rav1e's 0-10 preset, and is ignored for H.264, which has its own
    /// complexity setting.
    pub fn new(
        format: Format,
        width: u32,
        height: u32,
        fps: u32,
        quality: u8,
        speed: u8,
    ) -> Result<Self, String> {
        // Checked here rather than in each backend: both need it, and the
        // caller's fix ("render an even size") is the same either way.
        if width % 2 != 0 || height % 2 != 0 {
            return Err(format!(
                "animation size must be even for 4:2:0 chroma (got {}x{})",
                width, height
            ));
        }
        if fps == 0 {
            return Err("fps must be positive".to_string());
        }
        Ok(match format {
            Format::Avif => AnimationEncoder::Av1(crate::avif::Av1Encoder::new(
                width, height, fps, quality, speed,
            )?),
            Format::Mp4 => {
                AnimationEncoder::H264(crate::h264::H264Encoder::new(width, height, fps, quality)?)
            }
        })
    }

    /// Push one RGBA8 frame (tightly packed, width*height*4 bytes)
    pub fn push_frame(&mut self, rgba: &[u8]) -> Result<(), String> {
        match self {
            AnimationEncoder::Av1(e) => e.push_frame(rgba),
            AnimationEncoder::H264(e) => e.push_frame(rgba),
        }
    }

    /// Flush the encoder and write the file
    pub fn finish<P: AsRef<Path>>(self, path: P) -> Result<(), String> {
        match self {
            AnimationEncoder::Av1(e) => e.finish(path),
            AnimationEncoder::H264(e) => e.finish(path),
        }
    }
}

/// Write `bytes` to `path`, creating the directory if it isn't there
pub(crate) fn write_out(path: &Path, bytes: Vec<u8>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
        }
    }
    std::fs::write(path, bytes).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

// === ISOBMFF muxing ===

fn bx(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(payload.len() + 8);
    b.extend_from_slice(&(payload.len() as u32 + 8).to_be_bytes());
    b.extend_from_slice(name);
    b.extend_from_slice(payload);
    b
}

/// A "full box": version byte + 24-bit flags before the payload
fn full(name: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(payload.len() + 4);
    p.push(version);
    p.extend_from_slice(&flags.to_be_bytes()[1..]);
    p.extend_from_slice(payload);
    bx(name, &p)
}

struct W(Vec<u8>);

impl W {
    fn new() -> Self {
        W(Vec::new())
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
    fn zeros(&mut self, n: usize) -> &mut Self {
        self.0.resize(self.0.len() + n, 0);
        self
    }
}

/// The identity transformation matrix used by tkhd/mvhd
fn unity_matrix(w: &mut W) {
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        w.u32(v);
    }
}

/// Assemble the complete file: `ftyp + mdat + moov`, or `ftyp + moov + mdat`
/// when the format wants faststart.
///
/// `config` is the codec configuration box payload — an AV1CodecConfigurationRecord
/// for AV1, an AVCDecoderConfigurationRecord for H.264 — and is the only
/// codec-specific run of bytes this function doesn't build itself.
pub(crate) fn mux(
    format: Format,
    width: u32,
    height: u32,
    fps: u32,
    config: &[u8],
    samples: &[Sample],
) -> Vec<u8> {
    let n = samples.len() as u32;
    let movie_timescale = 1000u32;
    let movie_duration = (n as u64 * movie_timescale as u64).div_ceil(fps as u64) as u32;

    let ftyp = {
        let mut w = W::new();
        match format {
            Format::Avif => {
                w.raw(b"avis").u32(0);
                for brand in [b"avis", b"msf1", b"iso8", b"miaf"] {
                    w.raw(brand);
                }
            }
            // The brand set every muxer writes for H.264 in MP4. Minor version
            // 512 is the conventional value and some older parsers look for it.
            Format::Mp4 => {
                w.raw(b"isom").u32(512);
                for brand in [b"isom", b"iso2", b"avc1", b"mp41"] {
                    w.raw(brand);
                }
            }
        }
        bx(b"ftyp", &w.0)
    };

    let mdat_payload: Vec<u8> = samples.iter().flat_map(|s| s.data.iter().copied()).collect();
    let mdat = bx(b"mdat", &mdat_payload);

    let mvhd = {
        let mut w = W::new();
        w.u32(0).u32(0); // creation/modification time
        w.u32(movie_timescale).u32(movie_duration);
        w.u32(0x0001_0000).u16(0x0100).u16(0); // rate 1.0, volume, reserved
        w.zeros(8);
        unity_matrix(&mut w);
        w.zeros(24); // pre_defined
        w.u32(2); // next_track_ID
        full(b"mvhd", 0, 0, &w.0)
    };

    let tkhd = {
        let mut w = W::new();
        w.u32(0).u32(0); // creation/modification time
        w.u32(1).u32(0); // track_ID, reserved
        w.u32(movie_duration);
        w.zeros(8);
        w.u16(0).u16(0).u16(0).u16(0); // layer, alternate_group, volume, reserved
        unity_matrix(&mut w);
        w.u32(width << 16).u32(height << 16); // 16.16 fixed point
        full(b"tkhd", 0, 3, &w.0) // flags: enabled + in_movie
    };

    let mdhd = {
        let mut w = W::new();
        w.u32(0).u32(0);
        w.u32(fps).u32(n); // media timescale = fps, 1 tick per sample
        w.u16(0x55C4).u16(0); // language "und"
        full(b"mdhd", 0, 0, &w.0)
    };

    let hdlr = {
        // AVIF carries a picture sequence; MP4 carries video. Players key off
        // this, so it is not cosmetic.
        let (kind, name): (&[u8; 4], &[u8]) = match format {
            Format::Avif => (b"pict", b"fracturize\0"),
            Format::Mp4 => (b"vide", b"VideoHandler\0"),
        };
        let mut w = W::new();
        w.u32(0).raw(kind).zeros(12).raw(name);
        full(b"hdlr", 0, 0, &w.0)
    };

    let stsd = {
        let (entry, config_box): (&[u8; 4], &[u8; 4]) = match format {
            Format::Avif => (b"av01", b"av1C"),
            Format::Mp4 => (b"avc1", b"avcC"),
        };
        let visual = {
            let mut w = W::new();
            w.zeros(6).u16(1); // reserved, data_reference_index
            w.zeros(16); // pre_defined/reserved
            w.u16(width as u16).u16(height as u16);
            w.u32(0x0048_0000).u32(0x0048_0000); // 72 dpi
            w.u32(0).u16(1); // reserved, frame_count
            w.zeros(32); // compressorname
            w.u16(0x0018).u16(0xFFFF); // depth 24, pre_defined -1
            w.raw(&bx(config_box, config));
            // nclx colour info matching the encode: BT.709, limited range
            let mut c = W::new();
            c.raw(b"nclx").u16(1).u16(1).u16(1).raw(&[0]);
            w.raw(&bx(b"colr", &c.0));
            bx(entry, &w.0)
        };
        let mut w = W::new();
        w.u32(1).raw(&visual);
        full(b"stsd", 0, 0, &w.0)
    };

    let stts = {
        let mut w = W::new();
        w.u32(1).u32(n).u32(1); // all n samples last 1 tick
        full(b"stts", 0, 0, &w.0)
    };

    // Sync-sample table; omitted when every sample is a keyframe
    let stss = if samples.iter().all(|s| s.sync) {
        Vec::new()
    } else {
        let syncs: Vec<u32> = samples
            .iter()
            .enumerate()
            .filter(|(_, s)| s.sync)
            .map(|(i, _)| i as u32 + 1)
            .collect();
        let mut w = W::new();
        w.u32(syncs.len() as u32);
        for s in &syncs {
            w.u32(*s);
        }
        full(b"stss", 0, 0, &w.0)
    };

    let stsc = {
        let mut w = W::new();
        w.u32(1).u32(1).u32(n).u32(1); // one chunk holding every sample
        full(b"stsc", 0, 0, &w.0)
    };

    let stsz = {
        let mut w = W::new();
        w.u32(0).u32(n);
        for s in samples {
            w.u32(s.data.len() as u32);
        }
        full(b"stsz", 0, 0, &w.0)
    };

    // `moov` has to say where the samples are, and under faststart the samples
    // sit *after* `moov` — so build it once to learn its size, then again with
    // the real offset. The rebuild is the same length by construction: `stco`
    // holds a fixed-width u32 whatever the value.
    let build_moov = |chunk_offset: u32| {
        let stco = {
            let mut w = W::new();
            w.u32(1).u32(chunk_offset);
            full(b"stco", 0, 0, &w.0)
        };
        let stbl = {
            let mut p = stsd.clone();
            p.extend(stts.clone());
            p.extend(stss.clone());
            p.extend(stsc.clone());
            p.extend(stsz.clone());
            p.extend(stco);
            bx(b"stbl", &p)
        };
        let minf = {
            let vmhd = full(b"vmhd", 0, 1, &[0u8; 8]);
            let dinf = {
                let url = full(b"url ", 0, 1, &[]); // self-contained
                let mut w = W::new();
                w.u32(1).raw(&url);
                bx(b"dinf", &full(b"dref", 0, 0, &w.0))
            };
            let mut p = vmhd;
            p.extend(dinf);
            p.extend(stbl);
            bx(b"minf", &p)
        };
        let mdia = {
            let mut p = mdhd.clone();
            p.extend(hdlr.clone());
            p.extend(minf);
            bx(b"mdia", &p)
        };
        let trak = {
            let mut p = tkhd.clone();
            p.extend(mdia);
            bx(b"trak", &p)
        };
        let mut p = mvhd.clone();
        p.extend(trak);
        bx(b"moov", &p)
    };

    if format.faststart() {
        let moov_len = build_moov(0).len();
        // ftyp, then moov, then the mdat header: that's where sample 1 starts.
        let moov = build_moov((ftyp.len() + moov_len + 8) as u32);
        debug_assert_eq!(moov.len(), moov_len, "moov changed size with the real offset");
        let mut file = ftyp;
        file.extend(moov);
        file.extend(mdat);
        file
    } else {
        let moov = build_moov((ftyp.len() + 8) as u32);
        let mut file = ftyp;
        file.extend(mdat);
        file.extend(moov);
        file
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Walk the top-level boxes, checking each one's size lands exactly on the
    /// start of the next. A muxer that gets this wrong writes a file players
    /// refuse, and the refusal never says which box was short.
    pub(crate) fn top_level_boxes(bytes: &[u8]) -> Vec<String> {
        let mut names = Vec::new();
        let mut i = 0usize;
        while i + 8 <= bytes.len() {
            let size = u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
            names.push(String::from_utf8_lossy(&bytes[i + 4..i + 8]).into_owned());
            assert!(size >= 8 && i + size <= bytes.len(), "box overruns file");
            i += size;
        }
        assert_eq!(i, bytes.len(), "boxes don't tile the file");
        names
    }

    fn samples(n: usize) -> Vec<Sample> {
        (0..n)
            .map(|i| Sample { data: vec![i as u8; 10 + i], sync: i == 0 })
            .collect()
    }

    #[test]
    fn avif_keeps_its_layout_and_brand() {
        let out = mux(Format::Avif, 64, 48, 10, &[1, 2, 3, 4], &samples(4));
        assert_eq!(&out[4..8], b"ftyp");
        assert_eq!(&out[8..12], b"avis");
        assert_eq!(top_level_boxes(&out), ["ftyp", "mdat", "moov"]);
    }

    #[test]
    fn mp4_is_isom_and_leads_with_moov() {
        let out = mux(Format::Mp4, 64, 48, 10, &[1, 2, 3, 4], &samples(4));
        assert_eq!(&out[8..12], b"isom");
        // faststart: a player must reach the sample table before the payload
        assert_eq!(top_level_boxes(&out), ["ftyp", "moov", "mdat"]);
    }

    /// The faststart offset has to point at the first sample byte, or every
    /// frame decodes as garbage. Check it against the payload we put in.
    #[test]
    fn faststart_chunk_offset_lands_on_the_first_sample() {
        let samples = samples(3);
        let first = samples[0].data.clone();
        let out = mux(Format::Mp4, 64, 48, 10, &[1, 2, 3, 4], &samples);
        let stco = out.windows(4).position(|w| w == b"stco").expect("stco");
        // full box: name, then 4 bytes version/flags, then entry_count, then
        // the offset itself
        let off = u32::from_be_bytes(out[stco + 12..stco + 16].try_into().unwrap()) as usize;
        assert_eq!(&out[off..off + first.len()], &first[..], "offset misses sample 1");
    }

    #[test]
    fn sample_sizes_round_trip_through_stsz() {
        let samples = samples(5);
        let out = mux(Format::Mp4, 64, 48, 10, &[1, 2, 3, 4], &samples);
        let stsz = out.windows(4).position(|w| w == b"stsz").expect("stsz");
        let count = u32::from_be_bytes(out[stsz + 12..stsz + 16].try_into().unwrap());
        assert_eq!(count as usize, samples.len());
        for (i, s) in samples.iter().enumerate() {
            let at = stsz + 16 + i * 4;
            let size = u32::from_be_bytes(out[at..at + 4].try_into().unwrap());
            assert_eq!(size as usize, s.data.len());
        }
    }

    #[test]
    fn formats_map_to_extensions_both_ways() {
        assert_eq!(Format::Avif.extension(), "avif");
        assert_eq!(Format::Mp4.extension(), "mp4");
        assert_eq!(Format::from_path(Path::new("a/b.MP4")), Some(Format::Mp4));
        assert_eq!(Format::from_path(Path::new("a/b.avif")), Some(Format::Avif));
        assert_eq!(Format::from_path(Path::new("a/b.png")), None);
        assert_eq!(Format::from_path(Path::new("noext")), None);
    }

    /// Grey in, grey out: a neutral RGB must land on chroma 128 exactly, or
    /// every render picks up a colour cast.
    #[test]
    fn neutral_colour_stays_neutral() {
        let rgba = vec![128u8; 4 * 4 * 4];
        let yuv = rgba_to_yuv420(&rgba, 4, 4);
        assert!(yuv.u.iter().all(|&c| c == 128), "cb drifted: {:?}", yuv.u);
        assert!(yuv.v.iter().all(|&c| c == 128), "cr drifted: {:?}", yuv.v);
        // 128 carried through the limited-range luma scale
        let expect = (16.0f32 + 128.0 * (219.0 / 255.0)).round() as u8;
        assert!(yuv.y.iter().all(|&y| y == expect));
    }

    #[test]
    fn luma_stays_inside_the_limited_range() {
        for v in [0u8, 255u8] {
            let rgba = vec![v; 2 * 2 * 4];
            let yuv = rgba_to_yuv420(&rgba, 2, 2);
            assert!(yuv.y.iter().all(|&y| (16..=235).contains(&y)), "y out of range for {}", v);
        }
    }

    #[test]
    fn odd_sizes_are_refused_before_anything_encodes() {
        let e = AnimationEncoder::new(Format::Mp4, 65, 48, 24, 60, 8);
        assert!(e.err().is_some_and(|m| m.contains("even")));
        let e = AnimationEncoder::new(Format::Avif, 64, 48, 0, 60, 8);
        assert!(e.err().is_some_and(|m| m.contains("fps")));
    }
}
