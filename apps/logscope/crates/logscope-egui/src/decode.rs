//! decode.rs — the one IMPURE seam in the media pipeline: turn `sd-cli`'s PNG output into RGBA8 pixels.
//!
//! Everything downstream is pure and already tested — [`crate::imprint::downsample`] wants `(rgba, w, h)`, and
//! the UI wraps the same buffer in an `egui::ColorImage`. This wrapper normalizes whatever the encoder emits
//! (RGB / RGBA / grayscale / palette, 8- or 16-bit) to straight RGBA8 so callers never branch on colour type.
//! Uses the pure-Rust `png` crate (already in the tree), so no system image libraries.

/// Decode a PNG byte buffer into `(rgba8, width, height)` with 4 bytes per pixel, row-major, top-down.
/// Palette + low-bit-depth are expanded and 16-bit is stripped to 8, then every colour type is mapped up to
/// RGBA8 (opaque where the source had no alpha). Returns a short error string on malformed input.
pub fn decode_png_rgba(bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    // EXPAND: palette→RGB, grayscale<8→8, tRNS→alpha. STRIP_16: 16-bit→8-bit. Leaves us at 8-bit
    // Gray / GrayA / Rgb / Rgba, which we lift to Rgba8 below.
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| format!("png header: {e}"))?;
    let obs = reader.output_buffer_size().ok_or("png output size overflow")?;
    let mut buf = vec![0u8; obs];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("png decode: {e}"))?;
    let (w, h) = (info.width as usize, info.height as usize);
    buf.truncate(info.buffer_size());

    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => expand(&buf, w * h, |px, s| {
            px.copy_from_slice(&[s[0], s[1], s[2], 255]);
        }, 3),
        png::ColorType::GrayscaleAlpha => expand(&buf, w * h, |px, s| {
            px.copy_from_slice(&[s[0], s[0], s[0], s[1]]);
        }, 2),
        png::ColorType::Grayscale => expand(&buf, w * h, |px, s| {
            px.copy_from_slice(&[s[0], s[0], s[0], 255]);
        }, 1),
        png::ColorType::Indexed => return Err("indexed png not expanded".to_string()), // EXPAND should prevent this
    };
    if rgba.len() != w * h * 4 {
        return Err(format!("decoded {} bytes, expected {}", rgba.len(), w * h * 4));
    }
    Ok((rgba, w, h))
}

/// Map a packed source (`src_channels` bytes/pixel) into a fresh RGBA8 buffer via `f(dest4, src)`.
fn expand(src: &[u8], pixels: usize, f: impl Fn(&mut [u8], &[u8]), src_channels: usize) -> Vec<u8> {
    let mut out = vec![0u8; pixels * 4];
    for i in 0..pixels {
        let s = &src[i * src_channels..i * src_channels + src_channels];
        f(&mut out[i * 4..i * 4 + 4], s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode `(rgba, w, h)` to an in-memory PNG of the given colour type, for round-trip tests.
    fn encode(rgba: &[u8], w: u32, h: u32, ct: png::ColorType) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(ct);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().unwrap();
            let data: Vec<u8> = match ct {
                png::ColorType::Rgba => rgba.to_vec(),
                png::ColorType::Rgb => rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect(),
                png::ColorType::Grayscale => rgba.chunks(4).map(|p| p[0]).collect(),
                _ => rgba.to_vec(),
            };
            writer.write_image_data(&data).unwrap();
        }
        out
    }

    #[test]
    fn round_trips_rgba_2x2() {
        // a known 2×2: red, green, blue, white
        let src = [
            255, 0, 0, 255, 0, 255, 0, 255,
            0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let png = encode(&src, 2, 2, png::ColorType::Rgba);
        let (rgba, w, h) = decode_png_rgba(&png).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgba, src);
    }

    #[test]
    fn rgb_source_gets_opaque_alpha() {
        let src = [10, 20, 30, 255, 40, 50, 60, 255]; // 2×1
        let png = encode(&src, 2, 1, png::ColorType::Rgb);
        let (rgba, w, h) = decode_png_rgba(&png).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgba, src, "RGB decodes to RGBA with alpha 255");
    }

    #[test]
    fn grayscale_source_expands_to_rgba() {
        let src = [128, 128, 128, 255, 200, 200, 200, 255]; // 2×1 grays as rgba
        let png = encode(&src, 2, 1, png::ColorType::Grayscale);
        let (rgba, _, _) = decode_png_rgba(&png).unwrap();
        assert_eq!(rgba, src, "gray value replicated across RGB, opaque alpha");
    }

    #[test]
    fn decode_then_downsample_end_to_end() {
        // the actual pipeline: PNG bytes -> rgba -> imprint mosaic
        let mut src = Vec::new();
        for _ in 0..64 { src.extend_from_slice(&[90, 140, 210, 255]); } // 8×8 solid
        let png = encode(&src, 8, 8, png::ColorType::Rgba);
        let (rgba, w, h) = decode_png_rgba(&png).unwrap();
        let grid = crate::imprint::downsample(&rgba, w, h, 4, 4);
        assert!(grid.iter().all(|&c| c == [90, 140, 210]), "solid image → uniform mosaic through the real decode");
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_png_rgba(&[1, 2, 3, 4]).is_err());
        assert!(decode_png_rgba(&[]).is_err());
    }
}
