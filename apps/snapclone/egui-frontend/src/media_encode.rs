//! Compressing a snap, once, for both the file and the wire.
//!
//! # What this replaces and why
//!
//! `frame_pump::ppm_bytes` is a twelve-byte header over raw RGB, and the same
//! bytes go to disk and to every recipient. On disk that was a defensible
//! first step — the capture arm says so: "no encoder dependency, and every
//! image tool opens it", explicitly "not the composer flow yet". On the wire it
//! is not:
//!
//! | frame | PPM | JPEG q85 |
//! |---|---|---|
//! | 720p | 2.8 MB | ~150 KB |
//! | 1080p | 6.2 MB | ~300 KB |
//! | 12 MP | ~36 MB | ~1.5 MB |
//!
//! For an app whose whole purpose is sending someone a photo, over a cellular
//! link, that is the difference between usable and not.
//!
//! # One encode, not two
//!
//! The property the old code states and gets right, kept verbatim here: the
//! bytes sent ARE the bytes on disk, "so what a peer receives is byte for byte
//! what the local file holds. A second encoding path is a second thing that can
//! differ subtly from the photo you looked at."
//!
//! So this is a replacement for the single existing encode, not an extra step
//! before the socket. The file extension moves with it — a `.jpg` on disk is
//! also what a user expects a photo to be, and `save_snap` names the file from
//! [`Encoded::ext`] rather than hard-coding one.
//!
//! # Why JPEG and not AV1
//!
//! AVIF at the same visual quality is roughly half the size again, and the
//! fleet already has AV1 machinery — dandere2x drives `libsvtav1` through
//! ffmpeg. But that is a native encoder invoked as a subprocess, and putting a
//! process spawn on the shutter path costs a hundred milliseconds and a
//! dependency on an ffmpeg build with the right encoder, which is exactly the
//! failure `d2x-job`'s codec preflight exists to catch.
//!
//! JPEG through the `image` crate is pure Rust, links nothing, and takes the
//! payload from 6.2 MB to 300 KB. The remaining 2x from AVIF is worth having
//! and is not worth blocking the first 20x on. See C14_TODO §13.

use crate::frame_pump::RawFrame;

/// The encoded picture, and what to call the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    /// Extension WITHOUT the dot, for naming the file on disk.
    pub ext: &'static str,
}

/// JPEG quality for a sent snap, 1..=100.
///
/// 85 is the usual photographic default: artefacts are not visible at arm's
/// length on a phone, and it sits at the knee of the size curve — 95 is roughly
/// twice the bytes for a difference you need to pixel-peep to find, and 70
/// starts showing on skin and sky, which is most of what this app carries.
pub const DEFAULT_QUALITY: u8 = 85;

/// Quality below which the result is not worth sending.
///
/// A guard, not a preference: a caller that computes a quality from a bandwidth
/// estimate can arrive at single digits, and a snap nobody can make out is
/// worse than a slow one.
pub const MIN_QUALITY: u8 = 40;

/// Clamp a requested quality into the range that produces a usable photo.
pub fn usable_quality(requested: u8) -> u8 {
    requested.clamp(MIN_QUALITY, 100)
}

/// Encode one captured frame.
///
/// Fails only if the encoder does — which for JPEG over a well-formed RGB
/// buffer means an allocation failure. Returned as a `Result` anyway rather
/// than unwrapped: this runs on the shutter press, and a panic there loses the
/// photo AND the app.
pub fn encode_still(f: &RawFrame, quality: u8) -> Result<Encoded, String> {
    // RGBA to RGB. JPEG has no alpha and the camera's is opaque, which is the
    // same conversion `ppm_bytes` did — kept here so the two cannot disagree
    // about channel order while both exist.
    let mut rgb = Vec::with_capacity(f.width as usize * f.height as usize * 3);
    for px in f.rgba.chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }
    let mut bytes = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
        std::io::Cursor::new(&mut bytes),
        usable_quality(quality),
    );
    enc.encode(&rgb, f.width, f.height, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("jpeg encode failed: {e}"))?;
    Ok(Encoded { bytes, ext: "jpg" })
}

/// How much smaller the encode came out, as a ratio of the raw RGB size.
///
/// For the log line on a send. A number the operator can see is the difference
/// between trusting that compression is happening and assuming it is — and this
/// path has already shipped once believing something it was not doing.
pub fn compression_ratio(f: &RawFrame, encoded_len: usize) -> f32 {
    let raw = f.width as usize * f.height as usize * 3;
    if encoded_len == 0 {
        return 0.0;
    }
    raw as f32 / encoded_len as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with STRUCTURE, not a flat colour.
    ///
    /// A solid block compresses to almost nothing at any quality, so a test
    /// built on one passes whatever the encoder does — including nothing. The
    /// gradient plus noise is what makes the size assertions mean something.
    fn frame(w: u32, h: u32) -> RawFrame {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let n = ((x * 7 + y * 13) % 61) as u8;
                rgba.extend_from_slice(&[
                    (x % 256) as u8,
                    (y % 256) as u8,
                    n.wrapping_mul(4),
                    255,
                ]);
            }
        }
        RawFrame { width: w, height: h, rgba: rgba.into(), captured_ms: 0, seq: 0 }
    }

    #[test]
    fn a_snap_is_far_smaller_than_the_raw_rgb_it_replaces() {
        let f = frame(320, 240);
        let raw = 320 * 240 * 3;
        let e = encode_still(&f, DEFAULT_QUALITY).expect("encode");
        assert!(
            e.bytes.len() < raw / 4,
            "encoded {} bytes against {raw} raw — that is not compression",
            e.bytes.len()
        );
        assert_eq!(e.ext, "jpg");
    }

    #[test]
    fn the_output_is_a_real_jpeg_that_decodes_to_the_right_size() {
        // Size alone would pass on a truncated or empty file, which is the
        // failure that matters: a send path that "compresses" to something no
        // recipient can open.
        let f = frame(64, 48);
        let e = encode_still(&f, DEFAULT_QUALITY).expect("encode");
        assert_eq!(&e.bytes[..2], &[0xFF, 0xD8], "no JPEG start-of-image marker");
        let img = image::load_from_memory(&e.bytes).expect("the encoded bytes do not decode");
        assert_eq!((img.width(), img.height()), (64, 48));
    }

    #[test]
    fn the_channels_survive_the_round_trip() {
        // Found by breaking the RGBA→RGB step and watching NOTHING fail.
        // Checking only dimensions passes on a swap, and so does any check
        // that the bytes decode — a decoder returns whatever order the encoder
        // put in. Only comparing VALUES catches it, and a swap here is a photo
        // with the colours wrong, which is the most visible bug this module
        // could ship.
        //
        // Flat colour on purpose, with three clearly different channels: a
        // gradient survives a swap looking plausible.
        let (w, h) = (16u32, 16u32);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            rgba.extend_from_slice(&[220, 120, 30, 255]);
        }
        let f = RawFrame { width: w, height: h, rgba: rgba.into(), captured_ms: 0, seq: 0 };
        let e = encode_still(&f, 95).expect("encode");
        let img = image::load_from_memory(&e.bytes).expect("decodes").to_rgb8();
        let px = img.get_pixel(w / 2, h / 2).0;
        // Generous, because JPEG is lossy; the point is red is red.
        assert!((px[0] as i32 - 220).abs() < 25, "red came back {}", px[0]);
        assert!((px[1] as i32 - 120).abs() < 25, "green came back {}", px[1]);
        assert!((px[2] as i32 - 30).abs() < 25,
                "blue came back {} — the channels are swapped", px[2]);
    }

    #[test]
    fn encode_still_clamps_the_quality_it_is_given() {
        // Also found by breaking it: `usable_quality` has its own test and
        // passes perfectly with `encode_still` ignoring it. The mechanism and
        // its use need separate assertions — the same split that caught the
        // IME hook and the unendlich consent seed.
        //
        // A quality of 0 produces a visibly destroyed image, so the clamped
        // encode must be materially bigger than an unclamped one would be.
        let f = frame(128, 128);
        let clamped = encode_still(&f, 0).expect("encode at 0").bytes.len();
        let floor = encode_still(&f, MIN_QUALITY).expect("encode at floor").bytes.len();
        assert_eq!(clamped, floor,
                   "quality 0 was honoured instead of clamped to {MIN_QUALITY}");
    }

    #[test]
    fn quality_moves_the_size_in_the_direction_it_should() {
        // Both directions, because a quality parameter that is ignored still
        // produces a valid small JPEG and passes every other test here.
        let f = frame(256, 256);
        let lo = encode_still(&f, MIN_QUALITY).expect("lo").bytes.len();
        let hi = encode_still(&f, 95).expect("hi").bytes.len();
        assert!(lo < hi, "quality {MIN_QUALITY} gave {lo} bytes, 95 gave {hi}");
    }

    #[test]
    fn an_unusable_quality_is_clamped_rather_than_honoured() {
        // A caller deriving quality from a bandwidth estimate can arrive at
        // single digits. A snap nobody can make out is worse than a slow one.
        assert_eq!(usable_quality(0), MIN_QUALITY);
        assert_eq!(usable_quality(1), MIN_QUALITY);
        assert_eq!(usable_quality(200), 100);
        // And a sensible request is passed through untouched, which a
        // clamp-everything implementation would also fail.
        assert_eq!(usable_quality(85), 85);
    }

    #[test]
    fn the_ratio_is_reported_against_raw_rgb_and_survives_an_empty_encode() {
        let f = frame(100, 100);
        assert_eq!(compression_ratio(&f, 30_000), 1.0);
        assert!(compression_ratio(&f, 1_000) > 29.0);
        // Zero would divide by zero and print `inf` in a log line that is
        // meant to reassure someone.
        assert_eq!(compression_ratio(&f, 0), 0.0);
    }

    #[test]
    fn a_frame_with_an_odd_size_still_encodes() {
        // Camera formats are not all even multiples, and a stride assumption
        // here would show up as a corrupt photo rather than an error.
        for (w, h) in [(1, 1), (3, 7), (17, 33)] {
            let e = encode_still(&frame(w, h), DEFAULT_QUALITY)
                .unwrap_or_else(|e| panic!("{w}x{h}: {e}"));
            let img = image::load_from_memory(&e.bytes).expect("decodes");
            assert_eq!((img.width(), img.height()), (w, h));
        }
    }
}
