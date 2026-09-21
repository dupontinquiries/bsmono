//! export — write frames out as a `.fao` file.
//!
//! ## Why this uses the v1 DCT path and not the V2 encoder
//!
//! The Sequence Lab encodes with `v2_encoder::V2Encoder`, and its result type,
//! `fao_codec_lib::FrameResult`, carries NO BITSTREAM. It reports
//! `compressed_size` -- a number -- alongside PSNR and timings, and writes the
//! reconstruction into a caller buffer, but the encoded bytes themselves never
//! leave the encoder. So a V2 encode cannot be saved from here at all. That is a
//! library API gap, not a missing button, and it is the real reason "encode then
//! export" had no path in this frontend.
//!
//! The v1 DCT path does hand back its bytes -- `compress_frame_dct` returns the
//! payload -- and `fao_codec_lib::write_fao_container` wraps payloads in the
//! container. It is also exactly what this frontend's own decoder
//! (`decode::FaoFile`) reads, so an export here is one this app can reopen.
//!
//! When V2 grows a way to return its bitstream, this is where a second writer
//! goes.

use crate::decode::rgb_to_yuv420p;
use fao_codec_lib as fao_v1;

/// Why an export could not be written.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportError {
    /// Nothing to write. An empty `.fao` is a valid-looking file that plays as
    /// nothing, which is worse than refusing.
    NoFrames,
    /// A frame was not `w * h * 3` bytes of RGB. Refused rather than padded or
    /// truncated -- either would write a picture that is not the one given.
    WrongSize { frame: usize, got: usize, want: usize },
    /// Width and height must both be even: the chroma planes are half size in
    /// each direction, and an odd edge has no half.
    OddDimensions { w: usize, h: usize },
    /// The container writer refused.
    Write(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::NoFrames => write!(f, "nothing to export"),
            ExportError::WrongSize { frame, got, want } => {
                write!(f, "frame {frame} is {got} bytes, expected {want}")
            }
            ExportError::OddDimensions { w, h } => {
                write!(f, "{w}x{h}: both sides must be even for 4:2:0")
            }
            ExportError::Write(e) => write!(f, "could not write: {e}"),
        }
    }
}

/// Encode RGB frames as DCT I-frames and write a `.fao` container.
///
/// `quality` is the DCT quality, 1..=100. Every frame is an I-frame: this path
/// has no motion search, so the file is larger than a temporal encode would be
/// -- but every frame decodes on its own, which is what makes it safe to seek.
pub fn write_dct_fao(
    frames: &[Vec<u8>],
    w: usize,
    h: usize,
    fps: f64,
    quality: u8,
) -> Result<Vec<u8>, ExportError> {
    if frames.is_empty() {
        return Err(ExportError::NoFrames);
    }
    if w % 2 != 0 || h % 2 != 0 || w == 0 || h == 0 {
        return Err(ExportError::OddDimensions { w, h });
    }
    let want = w * h * 3;
    let q = quality.clamp(1, 100);
    let mut entries = Vec::with_capacity(frames.len());
    for (i, rgb) in frames.iter().enumerate() {
        if rgb.len() != want {
            return Err(ExportError::WrongSize { frame: i, got: rgb.len(), want });
        }
        let yuv = rgb_to_yuv420p(rgb, w, h);
        let payload = fao_v1::compress_frame_dct(&yuv, w, h, q);
        let n = payload.len();
        // (orig, recon, ftype, enc_bytes, psnr, skip_pct). `orig` empty and the
        // payload in `recon` is how the library's own round-trip test writes it.
        entries.push((Vec::new(), payload, 0u8, n, 0.0f64, 0.0f64));
    }
    let mut out = Vec::new();
    fao_v1::write_fao_container(&mut out, w, h, fps, q as f32 / 100.0, &entries)
        .map_err(|e| ExportError::Write(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{yuv_to_rgb, FaoFile};

    fn gradient(w: usize, h: usize, phase: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                v.push((x * 255 / w.max(1)) as u8 ^ phase);
                v.push((y * 255 / h.max(1)) as u8);
                v.push(phase);
            }
        }
        v
    }

    /// THE ROUND TRIP. What is exported must reopen in this app's own decoder,
    /// with the right shape, and every frame must decode to exactly what the
    /// codec produces for it -- not approximately, since both sides run the
    /// same deterministic DCT.
    #[test]
    fn an_export_reopens_and_decodes_to_the_codecs_answer() {
        let (w, h) = (32usize, 16usize);
        let frames = vec![gradient(w, h, 10), gradient(w, h, 90), gradient(w, h, 200)];
        let bytes = write_dct_fao(&frames, w, h, 30.0, 80).expect("writes");
        let f = FaoFile::parse(bytes).expect("reopens");
        assert_eq!((f.width(), f.height(), f.frame_count()), (w, h, 3));
        for (i, rgb) in frames.iter().enumerate() {
            let yuv = rgb_to_yuv420p(rgb, w, h);
            let payload = fao_v1::compress_frame_dct(&yuv, w, h, 80);
            let want = yuv_to_rgb(&fao_v1::decompress_frame_dct(&payload, w, h).unwrap(), w, h);
            assert_eq!(f.decode_rgb(i).unwrap(), want, "frame {i}");
        }
    }

    /// Frames stay IN ORDER and DISTINCT -- an exporter that wrote the first
    /// frame three times would pass a shape check.
    #[test]
    fn frames_keep_their_order() {
        let (w, h) = (32usize, 16usize);
        let frames = vec![gradient(w, h, 0), gradient(w, h, 255)];
        let f = FaoFile::parse(write_dct_fao(&frames, w, h, 24.0, 90).unwrap()).unwrap();
        assert_ne!(f.decode_rgb(0).unwrap(), f.decode_rgb(1).unwrap());
    }

    /// Higher quality must actually mean closer to the source.
    #[test]
    fn quality_changes_fidelity() {
        let (w, h) = (32usize, 16usize);
        let src = gradient(w, h, 77);
        let err = |q: u8| -> u64 {
            let f = FaoFile::parse(write_dct_fao(&[src.clone()], w, h, 30.0, q).unwrap()).unwrap();
            f.decode_rgb(0).unwrap().iter().zip(&src)
                .map(|(a, b)| (*a as i64 - *b as i64).unsigned_abs()).sum()
        };
        assert!(err(95) < err(10), "q95 {} should beat q10 {}", err(95), err(10));
    }

    #[test]
    fn empty_and_malformed_input_is_refused() {
        assert_eq!(write_dct_fao(&[], 32, 16, 30.0, 80), Err(ExportError::NoFrames));
        assert!(matches!(write_dct_fao(&[vec![0; 10]], 32, 16, 30.0, 80),
                         Err(ExportError::WrongSize { frame: 0, got: 10, .. })));
        assert!(matches!(write_dct_fao(&[vec![0; 33 * 16 * 3]], 33, 16, 30.0, 80),
                         Err(ExportError::OddDimensions { .. })));
    }
}
