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
//! fleet already has AV1 machinery — bscale drives `libsvtav1` through
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

/// Draw a composed snap's caption INTO the pixels: a translucent band across
/// the frame at `caption_y` (0 top .. 1 bottom), the text centred in `color`.
///
/// The Qt build does the same (`renderComposed`): the wire has no caption field,
/// and a caption the recipient never sees was being shown to the sender as part
/// of what they were sending. Rasterised with egui's own font stack (epaint:
/// skrifa + its glyph atlas, headless -- no context, no GPU) so no second font
/// renderer comes into the build. Wrapped to 90% of the width; sized to the
/// frame height so it reads the same on a 720p and a 4K capture.
pub fn bake_caption(rgb: &mut [u8], width: u32, height: u32, caption: &str, color: [u8; 3], caption_y: f32) {
    use egui::epaint::text::{FontDefinitions, Fonts, TextOptions};
    let caption = caption.trim();
    if caption.is_empty() || width == 0 || height == 0 || rgb.len() < (width * height * 3) as usize {
        return;
    }
    let (w, h) = (width as f32, height as f32);
    let size = (h * 0.055).clamp(12.0, 160.0);
    let mut fonts = Fonts::new(TextOptions::default(), FontDefinitions::default());
    let mut view = fonts.with_pixels_per_point(1.0);
    let galley = view.layout(caption.to_string(), egui::FontId::proportional(size),
                             egui::Color32::WHITE, w * 0.9);
    let atlas = view.image();
    let pad = size * 0.35;
    let band_h = galley.size().y + 2.0 * pad;
    let centre = caption_y.clamp(0.0, 1.0) * h;
    let top = (centre - band_h / 2.0).clamp(0.0, (h - band_h).max(0.0));
    let origin_x = (w - galley.size().x) / 2.0;
    let origin_y = top + pad;
    // The band: darken to 55%, the same legibility the viewer's overlay uses.
    let (y0, y1) = (top.max(0.0) as u32, ((top + band_h) as u32).min(height));
    for y in y0..y1 {
        for x in 0..width {
            let i = ((y * width + x) * 3) as usize;
            for c in 0..3 { rgb[i + c] = (rgb[i + c] as u32 * 45 / 100) as u8; }
        }
    }
    let [aw, _ah] = atlas.size;
    for row in &galley.rows {
        for g in &row.row.glyphs {
            let uv = g.uv_rect;
            if uv.is_nothing() { continue; }
            let lt_x = (origin_x + row.pos.x + g.pos.x + uv.offset.x).round() as i64;
            let lt_y = (origin_y + row.pos.y + g.pos.y + uv.offset.y).round() as i64;
            for ty in uv.min[1]..uv.max[1] {
                for tx in uv.min[0]..uv.max[0] {
                    let cov = atlas.pixels[ty as usize * aw + tx as usize].a() as u32;
                    if cov == 0 { continue; }
                    let x = lt_x + (tx - uv.min[0]) as i64;
                    let y = lt_y + (ty - uv.min[1]) as i64;
                    if x < 0 || y < 0 || x >= width as i64 || y >= height as i64 { continue; }
                    let i = ((y as u32 * width + x as u32) * 3) as usize;
                    for c in 0..3 {
                        rgb[i + c] = ((rgb[i + c] as u32 * (255 - cov) + color[c] as u32 * cov) / 255) as u8;
                    }
                }
            }
        }
    }
}

/// [`bake_caption`] on an already-encoded JPEG: decode, draw, re-encode at
/// `quality`. The send path holds the capture as JPEG bytes (`last_snap`), so
/// this is where a composition meets them.
pub fn bake_caption_jpeg(jpeg: &[u8], caption: &str, color: [u8; 3], caption_y: f32, quality: u8) -> Result<Vec<u8>, String> {
    if caption.trim().is_empty() {
        return Ok(jpeg.to_vec());
    }
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .map_err(|e| format!("jpeg decode failed: {e}"))?
        .to_rgb8();
    let (w, h) = img.dimensions();
    let mut rgb = img.into_raw();
    bake_caption(&mut rgb, w, h, caption, color, caption_y);
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::Cursor::new(&mut bytes), usable_quality(quality))
        .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("jpeg encode failed: {e}"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    /// The caption is really in the pixels: text-coloured pixels appear inside
    /// the band at caption_y and nowhere else, the band darkens only its rows,
    /// and an empty caption changes nothing.
    #[test]
    fn a_caption_is_drawn_into_the_frame_where_the_composer_put_it() {
        let (w, h) = (320u32, 240u32);
        let grey = vec![128u8; (w * h * 3) as usize];
        let mut img = grey.clone();
        bake_caption(&mut img, w, h, "  ", [255, 0, 0], 0.5);
        assert_eq!(img, grey, "an empty caption touched the frame");
        bake_caption(&mut img, w, h, "Hello snap", [255, 40, 40], 0.75);
        let px = |x: u32, y: u32| { let i = ((y * w + x) * 3) as usize; [img[i], img[i + 1], img[i + 2]] };
        let reddish = |p: [u8; 3]| p[0] > 150 && p[1] < 110;
        let rows_with_text: Vec<u32> = (0..h).filter(|&y| (0..w).any(|x| reddish(px(x, y)))).collect();
        assert!(!rows_with_text.is_empty(), "no caption pixels were drawn");
        let (lo, hi) = (*rows_with_text.first().unwrap(), *rows_with_text.last().unwrap());
        assert!(lo as f32 > h as f32 * 0.6 && (hi as f32) < h as f32 * 0.9, "text rows {lo}..{hi}, expected near 75%");
        assert_eq!(px(5, 10), [128, 128, 128], "the top of the frame was touched");
        assert!(px(2, (h as f32 * 0.75) as u32)[0] < 128, "the band did not darken its rows");
        let cols: Vec<u32> = (0..w).filter(|&x| (lo..=hi).any(|y| reddish(px(x, y)))).collect();
        let mid = (cols.first().unwrap() + cols.last().unwrap()) as f32 / 2.0;
        assert!((mid - w as f32 / 2.0).abs() < w as f32 * 0.08, "not centred: {mid}");
        // Through JPEG: decode, bake, encode still yields a valid, different JPEG.
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::Cursor::new(&mut jpg), 90)
            .encode(&grey, w, h, image::ExtendedColorType::Rgb8).unwrap();
        let baked = bake_caption_jpeg(&jpg, "Hi", [255, 255, 255], 0.5, 85).unwrap();
        assert_ne!(baked, jpg);
        assert!(image::load_from_memory(&baked).is_ok());
        assert_eq!(bake_caption_jpeg(&jpg, "", [0, 0, 0], 0.5, 85).unwrap(), jpg, "no caption, no re-encode");
    }

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
