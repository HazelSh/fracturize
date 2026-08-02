//! Animated AVIF output: rav1e AV1 encoding, muxed by `src/video.rs`.
//!
//! An animated AVIF is an AV1 image sequence ('avis' brand): a tiny MP4-style
//! container with one 'pict' track whose samples are AV1 temporal units.
//! rav1e is already in our dependency tree (image -> ravif uses it for still
//! AVIFs), so encoding costs no new dependencies.
//!
//! The container, and the RGBA -> BT.709 limited-range 4:2:0 conversion that
//! feeds it, live in `src/video.rs` and are shared with the H.264/MP4 path in
//! `src/h264.rs`. What's left here is the AV1 part: rav1e's configuration, and
//! splitting its temporal units into the pieces the sample table wants.
//!
//! Encoding runs low-latency (no frame reordering) so decode order ==
//! presentation order and the sample table needs no composition-time offsets.

use std::path::Path;

use rav1e::config::SpeedSettings;
use rav1e::prelude::*;

use crate::video::{self, Format, Sample};

pub struct Av1Encoder {
    ctx: Context<u8>,
    width: u32,
    height: u32,
    fps: u32,
    samples: Vec<Sample>,
    frames_sent: u64,
}

impl Av1Encoder {
    /// `quality` is 0-100 (higher = better; ~60 is a good default), `speed` is
    /// rav1e's 0-10 preset (higher = faster). Size and fps are validated by
    /// `video::AnimationEncoder::new` before this is reached.
    pub fn new(width: u32, height: u32, fps: u32, quality: u8, speed: u8) -> Result<Self, String> {
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
        let yuv = video::rgba_to_yuv420(rgba, w, h);

        let mut frame = self.ctx.new_frame();
        frame.planes[0].copy_from_raw_u8(&yuv.y, w, 1);
        frame.planes[1].copy_from_raw_u8(&yuv.u, w / 2, 1);
        frame.planes[2].copy_from_raw_u8(&yuv.v, w / 2, 1);
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

        let file =
            video::mux(Format::Avif, self.width, self.height, self.fps, &av1c, &self.samples);
        video::write_out(path.as_ref(), file)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::tests::top_level_boxes;

    /// Encode a tiny gradient animation and validate the container
    /// structure; if ffprobe is installed, cross-check that it decodes.
    #[test]
    fn animated_avif_roundtrip() {
        let (w, h, fps, frames) = (64u32, 48u32, 10u32, 12u32);
        let mut enc = Av1Encoder::new(w, h, fps, 60, 10).unwrap();
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
        // ftyp brand, then a top-level box walk: ftyp, mdat, moov, nothing else
        assert_eq!(&bytes[4..8], b"ftyp");
        assert_eq!(&bytes[8..12], b"avis");
        assert_eq!(top_level_boxes(&bytes), ["ftyp", "mdat", "moov"]);

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
