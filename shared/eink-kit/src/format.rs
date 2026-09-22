//! Pixel formats the display servers accept, and conversion from the
//! rasterizer's premultiplied RGBA (always opaque after compositing over the
//! paper background, so premultiplied == straight here).

/// Output pixel format. Values chosen to be what qtfb/Blight actually take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 1 byte, luminance. Blight `Format_Grayscale8`; ideal for mono panels.
    Gray8,
    /// 2 bytes, native-endian r5g6b5 (QImage `Format_RGB16`). qtfb's
    /// RM2FB / RGB565 modes; half the memory of RGB888.
    Rgb565,
    /// 3 bytes R,G,B (QImage `Format_RGB888`).
    Rgb888,
    /// 4 bytes R,G,B,A (QImage `Format_RGBA8888`).
    Rgba8888,
}

impl PixelFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Gray8 => 1,
            PixelFormat::Rgb565 => 2,
            PixelFormat::Rgb888 => 3,
            PixelFormat::Rgba8888 => 4,
        }
    }
    pub fn parse(s: &str) -> Option<PixelFormat> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gray8" | "grey8" => Some(PixelFormat::Gray8),
            "rgb565" => Some(PixelFormat::Rgb565),
            "rgb888" => Some(PixelFormat::Rgb888),
            "rgba8888" => Some(PixelFormat::Rgba8888),
            _ => None,
        }
    }
}

/// Rec.601 luma on gamma-encoded values. Integer, no floats per pixel.
#[inline]
pub fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((r as u32 * 77 + g as u32 * 150 + b as u32 * 29 + 128) >> 8) as u8
}

/// A pixel counts as coloured if its channels spread by more than this.
/// Anti-aliased black-on-white text stays grey; a red button does not.
pub const CHROMA_THRESHOLD: u8 = 24;

#[inline]
pub fn is_chromatic(r: u8, g: u8, b: u8) -> bool {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    mx - mn > CHROMA_THRESHOLD
}

/// Encode one RGBA pixel into `out` (length = bytes_per_pixel).
#[inline]
pub fn encode(fmt: PixelFormat, rgba: [u8; 4], out: &mut [u8]) {
    let [r, g, b, _] = rgba;
    match fmt {
        PixelFormat::Gray8 => out[0] = luma(r, g, b),
        PixelFormat::Rgb565 => {
            // Round, don't truncate: truncation quantises the 5-bit and
            // 6-bit channels differently, so every grey came out tinted
            // (248,248,248 → 255,251,255: visibly pink on a colour panel).
            let r5 = (r as u16 * 31 + 127) / 255;
            let g6 = (g as u16 * 63 + 127) / 255;
            let b5 = (b as u16 * 31 + 127) / 255;
            let v: u16 = (r5 << 11) | (g6 << 5) | b5;
            out[..2].copy_from_slice(&v.to_ne_bytes());
        }
        PixelFormat::Rgb888 => out[..3].copy_from_slice(&[r, g, b]),
        PixelFormat::Rgba8888 => out[..4].copy_from_slice(&[r, g, b, 255]),
    }
}

/// Decode back to RGBA, for snapshots and tests.
pub fn decode(fmt: PixelFormat, px: &[u8]) -> [u8; 4] {
    match fmt {
        PixelFormat::Gray8 => [px[0], px[0], px[0], 255],
        PixelFormat::Rgb565 => {
            let v = u16::from_ne_bytes([px[0], px[1]]);
            let r = ((v >> 11) & 0x1f) as u8;
            let g = ((v >> 5) & 0x3f) as u8;
            let b = (v & 0x1f) as u8;
            [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2), 255]
        }
        PixelFormat::Rgb888 => [px[0], px[1], px[2], 255],
        PixelFormat::Rgba8888 => [px[0], px[1], px[2], 255],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrips_primaries_within_quantisation() {
        for fmt in [PixelFormat::Rgb565, PixelFormat::Rgb888, PixelFormat::Rgba8888] {
            for c in [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255], [0, 0, 0, 255]] {
                let mut buf = [0u8; 4];
                encode(fmt, c, &mut buf);
                assert_eq!(decode(fmt, &buf), c, "{fmt:?}");
            }
        }
    }
    #[test]
    fn rgb565_keeps_greys_grey() {
        for v in 0..=255u8 {
            let mut b = [0u8; 2];
            encode(PixelFormat::Rgb565, [v, v, v, 255], &mut b);
            let [r, g, bl, _] = decode(PixelFormat::Rgb565, &b);
            let spread = r.max(g).max(bl) - r.min(g).min(bl);
            assert!(spread <= 6, "grey {v} → ({r},{g},{bl})");
            assert!((r as i16 - v as i16).abs() <= 5, "grey {v} → {r}");
        }
    }

    #[test]
    fn grey_text_is_not_colour_but_red_is() {
        assert!(!is_chromatic(128, 128, 128));
        assert!(!is_chromatic(200, 205, 210));
        assert!(is_chromatic(220, 40, 40));
    }
}
