//! H.264 encoding for `.mp4` output: openh264 + the shared muxer in
//! `src/video.rs`.
//!
//! This is the "post it somewhere" path. rav1e (see `src/avif.rs`) only speaks
//! AV1, and while AV1 muxes into MP4 perfectly well, upload pipelines for
//! short looping video overwhelmingly want H.264 — so the format that exists
//! to be posted uses the codec that gets accepted.
//!
//! Two conversions stand between openh264's output and an MP4 sample:
//!
//! * **Framing.** openh264 emits Annex-B: NAL units separated by `00 00 01`
//!   start codes. MP4 wants each NAL prefixed by its own 4-byte big-endian
//!   length. Every start code is rewritten on the way into a sample.
//! * **Parameter sets.** SPS and PPS describe the stream and belong in the
//!   `avcC` box in the sample table, not in the samples. They are lifted out of
//!   the first keyframe and dropped from the payload thereafter.
//!
//! Encoding pins the quantizer — min QP == max QP — so the quality slider
//! means the same thing in both formats: pick a fidelity and let the bitrate
//! land where it lands, rather than silently becoming a bitrate target in one
//! of them.

use std::path::Path;

use openh264::encoder::{
    Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Level, Profile, QpRange,
    RateControlMode, VuiConfig,
};
use openh264::formats::YUVSource;

use crate::video::{self, Format, Sample, Yuv420};

/// `Yuv420` is already planar 8-bit 4:2:0 with tight strides, which is exactly
/// what openh264 asks a source for.
struct Frame<'a>(&'a Yuv420);

impl YUVSource for Frame<'_> {
    fn dimensions(&self) -> (usize, usize) {
        (self.0.width, self.0.height)
    }
    fn strides(&self) -> (usize, usize, usize) {
        (self.0.width, self.0.width / 2, self.0.width / 2)
    }
    fn y(&self) -> &[u8] {
        &self.0.y
    }
    fn u(&self) -> &[u8] {
        &self.0.u
    }
    fn v(&self) -> &[u8] {
        &self.0.v
    }
}

pub struct H264Encoder {
    encoder: Encoder,
    width: u32,
    height: u32,
    fps: u32,
    samples: Vec<Sample>,
    /// SPS and PPS, lifted from the first keyframe for the `avcC` box
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl H264Encoder {
    /// `quality` is 0-100 (higher = better). Size and fps are validated by
    /// `video::AnimationEncoder::new` before this is reached.
    pub fn new(width: u32, height: u32, fps: u32, quality: u8) -> Result<Self, String> {
        // 0-100 (higher better) onto H.264's QP (0-51, lower better). Kept off
        // 0 at the top: QP 0 is near-lossless and produces files far larger
        // than anything the slider's wording promises.
        let qp = (51 - (quality.min(100) as u32 * 50) / 100).clamp(1, 51) as u8;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);

        // Left on the default `CameraVideoRealTime` usage type deliberately.
        // The non-realtime one looks like the right choice for an offline
        // render, and openh264 refuses to initialise with it — every other
        // setting below was accepted on its own, and that one returns native
        // error 1 (cmInitParaError) on the first encode.
        let config = EncoderConfig::new()
            // A rate-control mode with min QP == max QP, NOT
            // `RateControlMode::Off`. Off sounds like the way to ask for a
            // fixed quantizer and is a trap: this crate carries `qp` to
            // `iMinQp`/`iMaxQp`, which openh264 only consults *under* rate
            // control, so with Off every quality setting produced a
            // byte-identical file — measured at 699,001 bytes for QP 10, 25
            // and 40 alike. Under a real mode the same three give
            // 1.37 MB / 758 KB / 112 KB.
            //
            // Bufferbased specifically, out of the three modes that honour the
            // quantizer. They produce byte-identical output here, but Quality
            // and Timestamp both print a warning at init — that bitrate can't
            // be held without frame skipping, which is true and precisely what
            // we want, since QP is meant to be steering. The warning is
            // emitted inside `initialize_ext`, before the crate gets to apply
            // a trace level, so it cannot be silenced through the config; the
            // only way not to print it on every single render is not to
            // provoke it.
            .rate_control_mode(RateControlMode::Bufferbased)
            .qp(QpRange::new(qp, qp))
            // One sample per frame is a promise the sample table makes: `stts`
            // gives every sample one tick. A dropped frame would shorten the
            // clip silently and, worse, break a zoom loop — those close because
            // the last frame lands exactly one period on from the first, and a
            // hole anywhere in the run moves the seam.
            .skip_frames(false)
            .max_frame_rate(FrameRate::from_hz(fps as f32))
            // High profile is universally decodable in 2026 and costs nothing
            // over Baseline; Level 5.1 covers 4K, which the render dialog offers.
            .profile(Profile::High)
            .level(Level::Level_5_1)
            // Matches the BT.709 limited-range conversion in video.rs, and says
            // so in the bitstream rather than relying on the container's `colr`
            // box alone — players trust the VUI first.
            .vui(VuiConfig::bt709())
            .intra_frame_period(IntraFramePeriod::from_num_frames(300))
            .num_threads(threads as u16);

        let encoder = Encoder::with_api_config(openh264::OpenH264API::from_source(), config)
            .map_err(|e| format!("openh264 rejected the config: {}", e))?;
        Ok(Self {
            encoder,
            width,
            height,
            fps,
            samples: Vec::new(),
            sps: None,
            pps: None,
        })
    }

    /// Push one RGBA8 frame (tightly packed, width*height*4 bytes)
    pub fn push_frame(&mut self, rgba: &[u8]) -> Result<(), String> {
        let yuv = video::rgba_to_yuv420(rgba, self.width as usize, self.height as usize);
        let bitstream = self
            .encoder
            .encode(&Frame(&yuv))
            .map_err(|e| format!("openh264 encode: {}", e))?;

        // openh264 is one-frame-in, one-frame-out with no reordering, so a
        // packet belongs to the frame just pushed and decode order is
        // presentation order — the same property the AV1 path gets from
        // low-latency mode, and what lets the sample table skip composition
        // offsets.
        let sync = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I);
        let mut data = Vec::new();
        for layer in 0..bitstream.num_layers() {
            let Some(layer) = bitstream.layer(layer) else { continue };
            for i in 0..layer.nal_count() {
                let Some(nal) = layer.nal_unit(i) else { continue };
                let nal = strip_start_code(nal);
                if nal.is_empty() {
                    continue;
                }
                match nal[0] & 0x1F {
                    // SPS / PPS: configuration, not payload. Keep the first of
                    // each for `avcC` and drop them from every sample.
                    7 => {
                        self.sps.get_or_insert_with(|| nal.to_vec());
                    }
                    8 => {
                        self.pps.get_or_insert_with(|| nal.to_vec());
                    }
                    // Access unit delimiters carry nothing a muxed stream needs
                    9 => {}
                    _ => {
                        data.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                        data.extend_from_slice(nal);
                    }
                }
            }
        }
        if data.is_empty() {
            return Err(format!(
                "openh264 produced no slice data for frame {} (frame type {:?})",
                self.samples.len(),
                bitstream.frame_type(),
            ));
        }
        self.samples.push(Sample { data, sync });
        Ok(())
    }

    /// Write the MP4. openh264 has no deferred work, so there is nothing to
    /// flush — every frame was encoded on the way in.
    pub fn finish<P: AsRef<Path>>(self, path: P) -> Result<(), String> {
        if self.samples.is_empty() {
            return Err("no frames pushed".to_string());
        }
        let (Some(sps), Some(pps)) = (self.sps.as_ref(), self.pps.as_ref()) else {
            return Err("openh264 never emitted SPS/PPS; the stream would be undecodable".into())
        };
        let config = avc_decoder_config(sps, pps)?;
        let file = video::mux(Format::Mp4, self.width, self.height, self.fps, &config, &self.samples);
        video::write_out(path.as_ref(), file)
    }
}

/// Drop a leading Annex-B start code (`00 00 01` or `00 00 00 01`)
fn strip_start_code(nal: &[u8]) -> &[u8] {
    if nal.starts_with(&[0, 0, 0, 1]) {
        &nal[4..]
    } else if nal.starts_with(&[0, 0, 1]) {
        &nal[3..]
    } else {
        nal
    }
}

/// Build an AVCDecoderConfigurationRecord — the payload of the `avcC` box.
///
/// The profile/compatibility/level triple is read back out of the SPS rather
/// than from what we asked the encoder for, because the encoder is free to
/// give us something else and the record has to describe the actual stream.
fn avc_decoder_config(sps: &[u8], pps: &[u8]) -> Result<Vec<u8>, String> {
    if sps.len() < 4 {
        return Err(format!("SPS too short to describe a stream ({} bytes)", sps.len()));
    }
    let mut c = Vec::with_capacity(sps.len() + pps.len() + 16);
    c.push(1); // configurationVersion
    c.push(sps[1]); // AVCProfileIndication
    c.push(sps[2]); // profile_compatibility
    c.push(sps[3]); // AVCLevelIndication
    c.push(0xFF); // 6 reserved bits + lengthSizeMinusOne = 3 (4-byte lengths)
    c.push(0xE1); // 3 reserved bits + numOfSequenceParameterSets = 1
    c.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    c.extend_from_slice(sps);
    c.push(1); // numOfPictureParameterSets
    c.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    c.extend_from_slice(pps);
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::tests::top_level_boxes;

    #[test]
    fn start_codes_come_off_either_length() {
        assert_eq!(strip_start_code(&[0, 0, 0, 1, 0x67, 0x42]), &[0x67, 0x42]);
        assert_eq!(strip_start_code(&[0, 0, 1, 0x68]), &[0x68]);
        // Already bare: left alone rather than truncated
        assert_eq!(strip_start_code(&[0x65, 0x88]), &[0x65, 0x88]);
    }

    #[test]
    fn avcc_describes_the_stream_it_was_given() {
        let sps = [0x67, 0x64, 0x00, 0x33, 0xAC];
        let pps = [0x68, 0xEE, 0x3C];
        let c = avc_decoder_config(&sps, &pps).unwrap();
        assert_eq!(c[0], 1, "configurationVersion");
        // profile/compat/level lifted straight out of the SPS
        assert_eq!(&c[1..4], &[0x64, 0x00, 0x33]);
        assert_eq!(c[4], 0xFF, "4-byte NAL lengths");
        assert_eq!(u16::from_be_bytes([c[6], c[7]]) as usize, sps.len());
        assert_eq!(&c[8..8 + sps.len()], &sps);
        let after = 8 + sps.len();
        assert_eq!(c[after], 1, "one PPS");
        assert_eq!(u16::from_be_bytes([c[after + 1], c[after + 2]]) as usize, pps.len());
        assert_eq!(&c[after + 3..], &pps);
    }

    #[test]
    fn a_truncated_sps_is_refused_rather_than_indexed_into() {
        assert!(avc_decoder_config(&[0x67], &[0x68]).is_err());
    }

    /// The quality knob has to actually move the bitrate.
    ///
    /// This is here because the first version of this encoder used
    /// `RateControlMode::Off`, which looks like the right way to ask for a
    /// fixed quantizer and silently ignores it: every quality setting produced
    /// a byte-identical file, and every other test still passed. Container
    /// structure, frame counts and ffprobe all look perfect when the quality
    /// slider is a no-op, so only a size comparison catches it.
    #[test]
    fn quality_changes_the_bitrate() {
        // Noise, not a gradient: a gradient compresses to almost nothing at
        // every setting and the differences vanish into the noise floor.
        let (w, h) = (128u32, 96u32);
        let encoded = |quality: u8| {
            let mut enc = H264Encoder::new(w, h, 24, quality).unwrap();
            let mut seed = 12345u32;
            for _ in 0..15 {
                let mut rgba = vec![0u8; (w * h * 4) as usize];
                for px in rgba.chunks_exact_mut(4) {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    px[0] = (seed >> 24) as u8;
                    px[1] = (seed >> 16) as u8;
                    px[2] = (seed >> 8) as u8;
                    px[3] = 255;
                }
                enc.push_frame(&rgba).unwrap();
            }
            enc.samples.iter().map(|s| s.data.len()).sum::<usize>()
        };
        let (low, mid, high) = (encoded(20), encoded(50), encoded(80));
        assert!(low < mid, "q20 ({low}) should be smaller than q50 ({mid})");
        assert!(mid < high, "q50 ({mid}) should be smaller than q80 ({high})");
    }

    /// Encode a real gradient animation and validate the container; if ffprobe
    /// is installed, cross-check that it decodes as H.264 with every frame
    /// present. This is the test that would have caught Annex-B framing left
    /// in the samples, which no amount of box-walking notices.
    #[test]
    fn mp4_roundtrip() {
        let (w, h, fps, frames) = (64u32, 48u32, 10u32, 12u32);
        let mut enc = H264Encoder::new(w, h, fps, 60).unwrap();
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
        let dir = std::env::temp_dir().join("fracturize_mp4_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("anim.mp4");
        enc.finish(&path).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        assert_eq!(&bytes[8..12], b"isom");
        assert_eq!(top_level_boxes(&bytes), ["ftyp", "moov", "mdat"]);

        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v", "error", "-select_streams", "v:0",
                "-count_frames", "-show_entries",
                "stream=codec_name,nb_read_frames,width,height,pix_fmt",
                "-of", "csv=p=0",
            ])
            .arg(&path)
            .output();
        if let Ok(out) = probe {
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success() && text.contains("h264"),
                "ffprobe failed: {} / {}",
                text,
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(text.contains("yuv420p"), "not 4:2:0: {}", text);
            assert!(text.contains(&frames.to_string()), "frame count: {}", text);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
