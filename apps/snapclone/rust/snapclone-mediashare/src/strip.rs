//! strip.rs — remove privacy-sensitive metadata from images before sharing.
//!
//! Photos carry a surprising amount that you don't want to broadcast to a group:
//! GPS coordinates, capture timestamps, camera serial numbers, software, even
//! embedded thumbnails. This strips that, structurally, without decoding pixels:
//! it walks the container (JPEG marker segments / PNG chunks) and drops the
//! metadata segments, copying the image data through byte-for-byte. So it is
//! lossless for the picture and needs no image codec.
//!
//! Stripped: JPEG APPn markers (EXIF/GPS, XMP, IPTC, JFIF/JFXX thumbnails) and
//! comments; PNG tEXt/zTXt/iTXt/tIME/eXIf chunks. The ICC color profile is kept
//! only if `keep_color_profile` is set (it can fingerprint a device/editor).
//!
//! NOTE on orientation: a JPEG's display rotation lives in EXIF, which we remove,
//! so the capture path should bake orientation into the pixels before sharing
//! (most phones already store upright). WebP/HEIC aren't handled yet — unknown
//! formats pass through untouched.

/// Recognised container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Jpeg,
    Png,
    Unknown,
}

/// What the strip did.
#[derive(Debug, Clone, Copy)]
pub struct StripReport {
    pub format: ImageFormat,
    pub original_len: usize,
    pub stripped_len: usize,
    pub segments_removed: usize,
}

/// Tuning.
#[derive(Debug, Clone, Copy)]
pub struct StripOptions {
    /// Keep the ICC color profile (JPEG APP2 / PNG iCCP) for colour fidelity.
    /// Default false (a profile can identify the device/editor).
    pub keep_color_profile: bool,
}

impl Default for StripOptions {
    fn default() -> Self {
        Self { keep_color_profile: false }
    }
}

const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// Identify the container from its magic bytes.
pub fn detect_format(data: &[u8]) -> ImageFormat {
    if data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
        ImageFormat::Jpeg
    } else if data.len() >= 8 && data[0..8] == PNG_SIG {
        ImageFormat::Png
    } else {
        ImageFormat::Unknown
    }
}

/// Strip metadata. Returns the cleaned bytes and a report. An unrecognised format
/// is returned unchanged with `format = Unknown`.
pub fn strip_metadata(data: &[u8], opts: &StripOptions) -> (Vec<u8>, StripReport) {
    let format = detect_format(data);
    let (out, removed) = match format {
        ImageFormat::Jpeg => strip_jpeg(data, opts).unwrap_or_else(|| (data.to_vec(), 0)),
        ImageFormat::Png => strip_png(data, opts).unwrap_or_else(|| (data.to_vec(), 0)),
        ImageFormat::Unknown => (data.to_vec(), 0),
    };
    let report = StripReport {
        format,
        original_len: data.len(),
        stripped_len: out.len(),
        segments_removed: removed,
    };
    (out, report)
}

// ── JPEG ──────────────────────────────────────────────────────────────────────
// SOI, then marker segments. A marker is 0xFF (one or more) then a non-0xFF byte.
// Most segments are 0xFF, marker, 2-byte big-endian length (counting the length
// bytes), payload. Standalone markers (RST0-7, TEM) have no length. At the first
// SOS the rest of the file is entropy-coded data + EOI, copied verbatim — APPn /
// COM metadata only appears before it.

fn jpeg_strip_appn(marker: u8, opts: &StripOptions) -> bool {
    if marker == 0xE2 {
        // APP2 — treat as the ICC color profile
        !opts.keep_color_profile
    } else {
        // APP0 (JFIF/JFXX thumbnail), APP1 (EXIF/XMP), APP13 (IPTC), … → strip
        (0xE0..=0xEF).contains(&marker)
    }
}

fn strip_jpeg(data: &[u8], opts: &StripOptions) -> Option<(Vec<u8>, usize)> {
    if data.len() < 2 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&[0xFF, 0xD8]); // SOI
    let mut i = 2usize;
    let mut removed = 0usize;

    while i < data.len() {
        if data[i] != 0xFF {
            // not at a marker — copy the remainder and stop (malformed/odd)
            out.extend_from_slice(&data[i..]);
            break;
        }
        // skip fill 0xFFs to the marker byte
        let mut p = i;
        while p < data.len() && data[p] == 0xFF {
            p += 1;
        }
        if p >= data.len() {
            out.extend_from_slice(&data[i..]);
            break;
        }
        let marker = data[p];

        if marker == 0xD9 {
            out.extend_from_slice(&[0xFF, 0xD9]); // EOI
            break;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            out.extend_from_slice(&[0xFF, marker]); // standalone, no payload
            i = p + 1;
            continue;
        }
        // length-bearing marker
        if p + 2 >= data.len() {
            out.extend_from_slice(&data[i..]);
            break;
        }
        let len = ((data[p + 1] as usize) << 8) | (data[p + 2] as usize);
        let seg_end = p + 1 + len; // payload is data[p+3 .. seg_end]
        if len < 2 || seg_end > data.len() {
            out.extend_from_slice(&data[i..]);
            break;
        }

        if marker == 0xDA {
            // SOS: copy the marker + everything to the end (entropy data + EOI)
            out.push(0xFF);
            out.extend_from_slice(&data[p..]);
            break;
        }

        let strip = marker == 0xFE /* COM */ || jpeg_strip_appn(marker, opts);
        if strip {
            removed += 1;
        } else {
            out.push(0xFF);
            out.extend_from_slice(&data[p..seg_end]);
        }
        i = seg_end;
    }
    Some((out, removed))
}

// ── PNG ───────────────────────────────────────────────────────────────────────
// 8-byte signature, then chunks: length(4 BE) | type(4) | data | crc(4). Kept
// chunks are copied byte-for-byte (their CRC stays valid); metadata chunks are
// dropped.

fn png_strip_chunk(ctype: &[u8], opts: &StripOptions) -> bool {
    match ctype {
        b"tEXt" | b"zTXt" | b"iTXt" | b"tIME" | b"eXIf" => true,
        b"iCCP" => !opts.keep_color_profile,
        _ => false, // keep IHDR, PLTE, IDAT, IEND, gAMA, cHRM, sRGB, pHYs, …
    }
}

fn strip_png(data: &[u8], opts: &StripOptions) -> Option<(Vec<u8>, usize)> {
    if data.len() < 8 || data[0..8] != PNG_SIG {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&PNG_SIG);
    let mut i = 8usize;
    let mut removed = 0usize;

    while i + 8 <= data.len() {
        let len = u32::from_be_bytes(data[i..i + 4].try_into().ok()?) as usize;
        let ctype = &data[i + 4..i + 8];
        let chunk_end = i.checked_add(12)?.checked_add(len)?; // 4+4+len+4
        if chunk_end > data.len() {
            out.extend_from_slice(&data[i..]); // truncated tail — copy as-is
            break;
        }
        let is_iend = ctype == b"IEND";
        if png_strip_chunk(ctype, opts) {
            removed += 1;
        } else {
            out.extend_from_slice(&data[i..chunk_end]);
        }
        i = chunk_end;
        if is_iend {
            break;
        }
    }
    Some((out, removed))
}

// ─── FFI ─────────────────────────────────────────────────────────────────────

use std::os::raw::c_int;

fn format_code(f: ImageFormat) -> c_int {
    match f {
        ImageFormat::Unknown => 0,
        ImageFormat::Jpeg => 1,
        ImageFormat::Png => 2,
    }
}

/// Detect the container: 0 = unknown, 1 = JPEG, 2 = PNG.
///
/// # Safety
/// `data` valid for `len` (or null).
#[no_mangle]
pub unsafe extern "C" fn sc_image_detect_format(data: *const u8, len: usize) -> c_int {
    if data.is_null() {
        return 0;
    }
    format_code(detect_format(std::slice::from_raw_parts(data, len)))
}

/// Strip metadata into `out`. Returns 0 (JPEG/PNG cleaned) or 1 (unknown format —
/// `out` is a copy of the input); -1 (null), -2 (`out` too small — `out_len` =
/// required size). `out_removed` (segment count) and `out_format` (0/1/2) may be
/// null. Pass `keep_color_profile` != 0 to retain the ICC profile.
///
/// # Safety
/// `data` valid for `len`; `out` writable for `out_cap`; `out_len` valid; the
/// optional out pointers valid or null.
#[no_mangle]
pub unsafe extern "C" fn sc_image_strip_metadata(
    data: *const u8,
    len: usize,
    keep_color_profile: c_int,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
    out_removed: *mut usize,
    out_format: *mut c_int,
) -> c_int {
    if data.is_null() || out_len.is_null() {
        return -1;
    }
    let input = std::slice::from_raw_parts(data, len);
    let opts = StripOptions { keep_color_profile: keep_color_profile != 0 };
    let (stripped, report) = strip_metadata(input, &opts);
    *out_len = stripped.len();
    if !out_removed.is_null() {
        *out_removed = report.segments_removed;
    }
    if !out_format.is_null() {
        *out_format = format_code(report.format);
    }
    if out.is_null() || stripped.len() > out_cap {
        return -2;
    }
    std::ptr::copy_nonoverlapping(stripped.as_ptr(), out, stripped.len());
    if report.format == ImageFormat::Unknown {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jseg(marker: u8, payload: &[u8]) -> Vec<u8> {
        let len = payload.len() + 2;
        let mut v = vec![0xFF, marker, (len >> 8) as u8, (len & 0xFF) as u8];
        v.extend_from_slice(payload);
        v
    }

    fn sample_jpeg() -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8]; // SOI
        j.extend(jseg(0xE1, b"Exif\x00\x00GPS-SECRET-LOCATION")); // APP1 EXIF
        j.extend(jseg(0xE0, b"JFIF\x00thumbnail-bytes")); // APP0
        j.extend(jseg(0xE2, b"ICC_PROFILE\x00profile")); // APP2 ICC
        j.extend(jseg(0xFE, b"a private comment")); // COM
        j.extend(jseg(0xDB, &[1u8; 64])); // DQT (keep)
        j.extend(jseg(0xC0, &[2u8; 15])); // SOF0 (keep)
        j.extend(jseg(0xC4, &[3u8; 20])); // DHT (keep)
        j.extend(jseg(0xDA, &[4u8; 10])); // SOS header
        j.extend_from_slice(&[0x12, 0x34, 0xFF, 0x00, 0x56, 0x78]); // entropy (stuffed FF00)
        j.extend_from_slice(&[0xFF, 0xD9]); // EOI
        j
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn detects_formats() {
        assert_eq!(detect_format(&sample_jpeg()), ImageFormat::Jpeg);
        assert_eq!(detect_format(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]), ImageFormat::Png);
        assert_eq!(detect_format(b"not an image"), ImageFormat::Unknown);
        assert_eq!(detect_format(&[]), ImageFormat::Unknown);
    }

    #[test]
    fn jpeg_strips_exif_and_comments_keeps_image() {
        let jpeg = sample_jpeg();
        let (out, rep) = strip_metadata(&jpeg, &StripOptions::default());
        assert_eq!(rep.format, ImageFormat::Jpeg);
        // EXIF/GPS, JFIF thumbnail, ICC, comment all gone
        assert!(!contains(&out, b"GPS-SECRET-LOCATION"));
        assert!(!contains(&out, b"thumbnail-bytes"));
        assert!(!contains(&out, b"ICC_PROFILE"));
        assert!(!contains(&out, b"a private comment"));
        // APP1+APP0+APP2+COM = 4 segments removed
        assert_eq!(rep.segments_removed, 4);
        // image data preserved: SOI, DQT/SOF/DHT, SOS + entropy + EOI
        assert_eq!(&out[0..2], &[0xFF, 0xD8]);
        assert!(contains(&out, &[0x12, 0x34, 0xFF, 0x00, 0x56, 0x78])); // entropy intact
        assert_eq!(&out[out.len() - 2..], &[0xFF, 0xD9]); // EOI
        assert!(out.len() < jpeg.len());
    }

    #[test]
    fn jpeg_keeps_icc_when_requested() {
        let jpeg = sample_jpeg();
        let (out, rep) = strip_metadata(&jpeg, &StripOptions { keep_color_profile: true });
        assert!(contains(&out, b"ICC_PROFILE"), "ICC kept");
        assert!(!contains(&out, b"GPS-SECRET-LOCATION"), "EXIF still stripped");
        assert_eq!(rep.segments_removed, 3); // APP1 + APP0 + COM (APP2 kept)
    }

    #[test]
    fn jpeg_already_clean_removes_nothing() {
        let jpeg = sample_jpeg();
        let (once, _) = strip_metadata(&jpeg, &StripOptions::default());
        let (twice, rep) = strip_metadata(&once, &StripOptions::default());
        assert_eq!(rep.segments_removed, 0, "second pass finds nothing");
        assert_eq!(once, twice, "idempotent");
    }

    fn pchunk(ctype: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(data.len() as u32).to_be_bytes());
        v.extend_from_slice(ctype);
        v.extend_from_slice(data);
        v.extend_from_slice(&[0, 0, 0, 0]); // dummy CRC (stripper doesn't check)
        v
    }

    fn sample_png() -> Vec<u8> {
        let mut p = PNG_SIG.to_vec();
        p.extend(pchunk(b"IHDR", &[7u8; 13]));
        p.extend(pchunk(b"tEXt", b"Software:SecretEditor")); // strip
        p.extend(pchunk(b"iCCP", b"icc-profile-bytes")); // strip unless keep
        p.extend(pchunk(b"IDAT", &[9u8; 24])); // keep
        p.extend(pchunk(b"eXIf", b"GPS-SECRET")); // strip
        p.extend(pchunk(b"tIME", b"2026:01:01")); // strip
        p.extend(pchunk(b"IEND", &[])); // keep
        p
    }

    #[test]
    fn png_strips_text_exif_time_keeps_image() {
        let png = sample_png();
        let (out, rep) = strip_metadata(&png, &StripOptions::default());
        assert_eq!(rep.format, ImageFormat::Png);
        assert!(!contains(&out, b"SecretEditor"));
        assert!(!contains(&out, b"GPS-SECRET"));
        assert!(!contains(&out, b"2026:01:01"));
        assert!(!contains(&out, b"icc-profile-bytes"));
        assert_eq!(rep.segments_removed, 4); // tEXt + iCCP + eXIf + tIME
        // image structure preserved
        assert_eq!(&out[0..8], &PNG_SIG);
        assert!(contains(&out, b"IHDR"));
        assert!(contains(&out, b"IDAT"));
        assert!(contains(&out, b"IEND"));
        assert!(out.len() < png.len());
    }

    #[test]
    fn png_keeps_iccp_when_requested() {
        let png = sample_png();
        let (out, rep) = strip_metadata(&png, &StripOptions { keep_color_profile: true });
        assert!(contains(&out, b"icc-profile-bytes"));
        assert_eq!(rep.segments_removed, 3); // tEXt + eXIf + tIME (iCCP kept)
    }

    #[test]
    fn unknown_format_passes_through() {
        let blob = b"this is just some bytes, not an image at all".to_vec();
        let (out, rep) = strip_metadata(&blob, &StripOptions::default());
        assert_eq!(rep.format, ImageFormat::Unknown);
        assert_eq!(rep.segments_removed, 0);
        assert_eq!(out, blob);
    }

    #[test]
    fn ffi_strip_roundtrip() {
        unsafe {
            let jpeg = sample_jpeg();
            assert_eq!(sc_image_detect_format(jpeg.as_ptr(), jpeg.len()), 1);

            // size query
            let mut need = 0usize;
            let mut removed = 0usize;
            let mut fmt = -1;
            assert_eq!(
                sc_image_strip_metadata(jpeg.as_ptr(), jpeg.len(), 0,
                    std::ptr::null_mut(), 0, &mut need, &mut removed, &mut fmt),
                -2
            );
            assert!(need > 0 && need < jpeg.len());
            assert_eq!(removed, 4);
            assert_eq!(fmt, 1);

            // real strip
            let mut out = vec![0u8; need];
            let mut olen = 0usize;
            assert_eq!(
                sc_image_strip_metadata(jpeg.as_ptr(), jpeg.len(), 0,
                    out.as_mut_ptr(), out.len(), &mut olen, std::ptr::null_mut(), std::ptr::null_mut()),
                0
            );
            out.truncate(olen);
            assert!(!contains(&out, b"GPS-SECRET-LOCATION"));

            // unknown format → returns 1, passthrough
            let blob = b"xxxxxxxxxxxx".to_vec();
            let mut o2 = vec![0u8; blob.len()];
            let mut l2 = 0usize;
            assert_eq!(
                sc_image_strip_metadata(blob.as_ptr(), blob.len(), 0,
                    o2.as_mut_ptr(), o2.len(), &mut l2, std::ptr::null_mut(), std::ptr::null_mut()),
                1
            );
        }
    }
}
