//! Animated AVIF output: rav1e AV1 encoding + a minimal ISOBMFF muxer
//!
//! An animated AVIF is an AV1 image sequence ('avis' brand): a tiny MP4-style
//! container with one 'pict' track whose samples are AV1 temporal units.
//! rav1e is already in our dependency tree (image -> ravif uses it for still
//! AVIFs), so encoding costs no new dependencies; the container is simple
//! enough to write by hand (ftyp + mdat + moov with an av01 sample table).
//!
//! Frames are pushed as RGBA8 (the offline renderer's readback format),
//! converted here to BT.709 limited-range 4:2:0. Encoding runs low-latency
//! (no frame reordering) so decode order == presentation order and the
//! sample table needs no composition-time offsets.

use std::path::Path;

use rav1e::config::SpeedSettings;
use rav1e::prelude::*;

/// One encoded AV1 sample (temporal unit) ready for the container
struct Sample {
    data: Vec<u8>,
    sync: bool,
}

pub struct AnimationEncoder {
    ctx: Context<u8>,
    width: u32,
    height: u32,
    fps: u32,
    samples: Vec<Sample>,
    frames_sent: u64,
}

impl AnimationEncoder {
    /// `quality` is 0-100 (higher = better; ~60 is a good default),
    /// `speed` is rav1e's 0-10 preset (higher = faster).
    pub fn new(width: u32, height: u32, fps: u32, quality: u8, speed: u8) -> Result<Self, String> {
        if width % 2 != 0 || height % 2 != 0 {
            return Err(format!(
                "animation size must be even for 4:2:0 chroma (got {}x{})",
                width, height
            ));
        }
        if fps == 0 {
            return Err("fps must be positive".to_string());
        }
        let enc = EncoderConfig {
            width: width as usize,
            height: height as usize,
            time_base: Rational { num: 1, den: fps as u64 },
            bit_depth: 8,
            chroma_sampling: ChromaSampling::Cs420,
            chroma_sample_position: ChromaSamplePosition::Unknown,
            pixel_range: PixelRange::Limited,
            color_description: Some(ColorDescription {
                color_primaries: ColorPrimaries::BT709,
                transfer_characteristics: TransferCharacteristics::BT709,
                matrix_coefficients: MatrixCoefficients::BT709,
            }),
            still_picture: false,
            // Decode order == presentation order: keeps the muxer trivial
            low_latency: true,
            max_key_frame_interval: 300,
            quantizer: (255 - (quality.min(100) as usize * 255) / 100).max(1),
            speed_settings: SpeedSettings::from_preset(speed.min(10)),
            ..Default::default()
        };
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let ctx = Config::new()
            .with_encoder_config(enc)
            .with_threads(threads)
            .new_context::<u8>()
            .map_err(|e| format!("rav1e config rejected: {}", e))?;
        Ok(Self { ctx, width, height, fps, samples: Vec::new(), frames_sent: 0 })
    }

    /// Push one RGBA8 frame (tightly packed, width*height*4 bytes)
    pub fn push_frame(&mut self, rgba: &[u8]) -> Result<(), String> {
        let (w, h) = (self.width as usize, self.height as usize);
        assert_eq!(rgba.len(), w * h * 4, "frame size mismatch");

        // RGB -> BT.709 limited-range YUV 4:2:0. Chroma is averaged over
        // each 2x2 block.
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

        let mut frame = self.ctx.new_frame();
        frame.planes[0].copy_from_raw_u8(&yp, w, 1);
        frame.planes[1].copy_from_raw_u8(&up, w / 2, 1);
        frame.planes[2].copy_from_raw_u8(&vp, w / 2, 1);
        self.ctx
            .send_frame(frame)
            .map_err(|e| format!("rav1e send_frame: {:?}", e))?;
        self.frames_sent += 1;
        self.drain(false)
    }

    fn drain(&mut self, flushing: bool) -> Result<(), String> {
        loop {
            match self.ctx.receive_packet() {
                Ok(pkt) => {
                    // A sample is the temporal unit minus any temporal
                    // delimiter OBUs (ISOBMFF carries framing itself)
                    let data: Vec<u8> = split_obus(&pkt.data)
                        .into_iter()
                        .filter(|(ty, _)| *ty != OBU_TEMPORAL_DELIMITER)
                        .flat_map(|(_, bytes)| bytes.iter().copied())
                        .collect();
                    self.samples.push(Sample { data, sync: pkt.frame_type == FrameType::KEY });
                }
                Err(EncoderStatus::Encoded) => continue,
                Err(EncoderStatus::NeedMoreData) => {
                    if flushing {
                        continue;
                    }
                    return Ok(());
                }
                Err(EncoderStatus::LimitReached) => return Ok(()),
                Err(e) => return Err(format!("rav1e receive_packet: {:?}", e)),
            }
        }
    }

    /// Flush the encoder and write the animated AVIF
    pub fn finish<P: AsRef<Path>>(mut self, path: P) -> Result<(), String> {
        self.ctx.flush();
        self.drain(true)?;
        if self.samples.len() as u64 != self.frames_sent {
            return Err(format!(
                "encoder produced {} packets for {} frames",
                self.samples.len(),
                self.frames_sent
            ));
        }
        if self.samples.is_empty() {
            return Err("no frames pushed".to_string());
        }

        // av1C payload: 4 config bytes from rav1e + the sequence header OBU
        // as configOBUs (some decoders want it before the first sample)
        let mut av1c = self.ctx.container_sequence_header();
        let first_key = &self.samples[0].data;
        if let Some((_, seq)) = split_obus(first_key)
            .into_iter()
            .find(|(ty, _)| *ty == OBU_SEQUENCE_HEADER)
        {
            av1c.extend_from_slice(seq);
        }

        let file = mux_avis(self.width, self.height, self.fps, &av1c, &self.samples);
        if let Some(dir) = path.as_ref().parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
            }
        }
        std::fs::write(path.as_ref(), file)
            .map_err(|e| format!("Failed to write {}: {}", path.as_ref().display(), e))
    }
}

// === AV1 OBU handling ===

const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_TEMPORAL_DELIMITER: u8 = 2;

/// Split an AV1 temporal unit into (obu_type, full obu bytes) pieces.
/// rav1e always writes the obu_has_size_field form; on anything malformed
/// the remainder is returned as one piece so nothing is silently dropped.
fn split_obus(data: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let start = i;
        let hdr = data[i];
        let obu_type = (hdr >> 3) & 0x0F;
        i += 1;
        if hdr & 0x04 != 0 {
            i += 1; // extension byte
        }
        if hdr & 0x02 == 0 {
            // No size field: OBU runs to the end of the temporal unit
            out.push((obu_type, &data[start..]));
            break;
        }
        // leb128 size
        let mut size = 0usize;
        let mut shift = 0u32;
        loop {
            let Some(&b) = data.get(i) else { return out };
            i += 1;
            size |= ((b & 0x7F) as usize) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                break;
            }
        }
        let end = (i + size).min(data.len());
        out.push((obu_type, &data[start..end]));
        i = end;
    }
    out
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

/// Assemble the complete 'avis' file: ftyp + mdat + moov
fn mux_avis(width: u32, height: u32, fps: u32, av1c: &[u8], samples: &[Sample]) -> Vec<u8> {
    let n = samples.len() as u32;
    let movie_timescale = 1000u32;
    let movie_duration = (n as u64 * movie_timescale as u64).div_ceil(fps as u64) as u32;

    let ftyp = {
        let mut w = W::new();
        w.raw(b"avis").u32(0);
        for brand in [b"avis", b"msf1", b"iso8", b"miaf"] {
            w.raw(brand);
        }
        bx(b"ftyp", &w.0)
    };

    let mdat_payload: Vec<u8> = samples.iter().flat_map(|s| s.data.iter().copied()).collect();
    let mdat = bx(b"mdat", &mdat_payload);
    // Samples start right after ftyp + the mdat header
    let chunk_offset = (ftyp.len() + 8) as u32;

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
        let mut w = W::new();
        w.u32(0).raw(b"pict").zeros(12).raw(b"fracturize\0");
        full(b"hdlr", 0, 0, &w.0)
    };

    let stsd = {
        // av01 VisualSampleEntry with the av1C configuration box inside
        let av01 = {
            let mut w = W::new();
            w.zeros(6).u16(1); // reserved, data_reference_index
            w.zeros(16); // pre_defined/reserved
            w.u16(width as u16).u16(height as u16);
            w.u32(0x0048_0000).u32(0x0048_0000); // 72 dpi
            w.u32(0).u16(1); // reserved, frame_count
            w.zeros(32); // compressorname
            w.u16(0x0018).u16(0xFFFF); // depth 24, pre_defined -1
            w.raw(&bx(b"av1C", av1c));
            // nclx color info matching the encode: BT.709, limited range
            let mut c = W::new();
            c.raw(b"nclx").u16(1).u16(1).u16(1).raw(&[0]);
            w.raw(&bx(b"colr", &c.0));
            bx(b"av01", &w.0)
        };
        let mut w = W::new();
        w.u32(1).raw(&av01);
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

    let stco = {
        let mut w = W::new();
        w.u32(1).u32(chunk_offset);
        full(b"stco", 0, 0, &w.0)
    };

    let stbl = {
        let mut p = stsd;
        p.extend(stts);
        p.extend(stss);
        p.extend(stsc);
        p.extend(stsz);
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
        let mut p = mdhd;
        p.extend(hdlr);
        p.extend(minf);
        bx(b"mdia", &p)
    };

    let trak = {
        let mut p = tkhd;
        p.extend(mdia);
        bx(b"trak", &p)
    };

    let moov = {
        let mut p = mvhd;
        p.extend(trak);
        bx(b"moov", &p)
    };

    let mut file = ftyp;
    file.extend(mdat);
    file.extend(moov);
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a tiny gradient animation and validate the container
    /// structure; if ffprobe is installed, cross-check that it decodes.
    #[test]
    fn animated_avif_roundtrip() {
        let (w, h, fps, frames) = (64u32, 48u32, 10u32, 12u32);
        let mut enc = AnimationEncoder::new(w, h, fps, 60, 10).unwrap();
        for f in 0..frames {
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    rgba[i] = (x * 4 + f * 8) as u8;
                    rgba[i + 1] = (y * 5) as u8;
                    rgba[i + 2] = (f * 20) as u8;
                    rgba[i + 3] = 255;
                }
            }
            enc.push_frame(&rgba).unwrap();
        }
        let dir = std::env::temp_dir().join("fracturize_avif_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("anim.avif");
        enc.finish(&path).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        // ftyp brand
        assert_eq!(&bytes[4..8], b"ftyp");
        assert_eq!(&bytes[8..12], b"avis");
        // Top-level box walk: ftyp, mdat, moov and nothing else
        let mut names = Vec::new();
        let mut i = 0usize;
        while i + 8 <= bytes.len() {
            let size = u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
            names.push(bytes[i + 4..i + 8].to_vec());
            assert!(size >= 8 && i + size <= bytes.len(), "box overruns file");
            i += size;
        }
        assert_eq!(i, bytes.len());
        assert_eq!(names, vec![b"ftyp".to_vec(), b"mdat".to_vec(), b"moov".to_vec()]);

        // ffprobe cross-check when available
        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v", "error", "-select_streams", "v:0",
                "-count_frames", "-show_entries",
                "stream=codec_name,nb_read_frames,width,height",
                "-of", "csv=p=0",
            ])
            .arg(&path)
            .output();
        if let Ok(out) = probe {
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success() && text.contains("av1"),
                "ffprobe failed: {} / {}",
                text,
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(text.contains(&frames.to_string()), "frame count: {}", text);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
