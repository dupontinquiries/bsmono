//! qr — render a player's id as a real, standards-compliant QR code (any phone's stock camera can
//! scan it — unlike `apps/snapclone`'s `QrCodeRenderer.cpp`, which by its own doc comment draws a
//! custom, non-standard module pattern only its own paired scanner understands). Pure Rust via the
//! `qrcode` crate; rasterized to a plain grayscale buffer so the caller can hand it to
//! `egui::ColorImage::from_gray` with no other imaging dependency.

use qrcode::QrCode;

/// A rasterized QR code: `pixels` is row-major grayscale (0 = black module, 255 = white), each
/// module drawn as a `scale`x`scale` block plus a quiet-zone border, `side` pixels square.
pub struct QrImage {
    pub side: usize,
    pub pixels: Vec<u8>,
}

/// Render `payload` (e.g. `"bsgames:<id>"`) as a QR code, each module scaled up by `scale` pixels
/// and padded with a 4-module quiet zone (the standard minimum), so it stays scannable at small
/// display sizes. Returns `None` only if the payload is too long to fit any QR version (extremely
/// unlikely for a short id).
pub fn render(payload: &str, scale: usize) -> Option<QrImage> {
    let code = QrCode::new(payload.as_bytes()).ok()?;
    let modules_per_side = code.width();
    let quiet = 4;
    let side_modules = modules_per_side + quiet * 2;
    let side = side_modules * scale;
    let mut pixels = vec![255u8; side * side];

    let colors = code.to_colors();
    for (i, c) in colors.iter().enumerate() {
        if *c == qrcode::Color::Light {
            continue;
        }
        let row = i / modules_per_side;
        let col = i % modules_per_side;
        let (py0, px0) = ((row + quiet) * scale, (col + quiet) * scale);
        for py in py0..py0 + scale {
            let row_off = py * side;
            for px in px0..px0 + scale {
                pixels[row_off + px] = 0;
            }
        }
    }
    Some(QrImage { side, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_square_image_with_the_expected_quiet_zone() {
        let img = render("bsgames:abc123", 3).expect("short payload should always encode");
        assert_eq!(img.pixels.len(), img.side * img.side);
        // the border quiet zone (first few rows/cols) must be entirely white
        for x in 0..img.side {
            assert_eq!(img.pixels[x], 255, "top row should be quiet-zone white");
        }
    }

    #[test]
    fn different_payloads_render_different_images() {
        let a = render("bsgames:aaaaaaaaaa", 2).unwrap();
        let b = render("bsgames:bbbbbbbbbb", 2).unwrap();
        assert_ne!(a.pixels, b.pixels);
    }

    #[test]
    fn contains_some_dark_modules_not_just_the_quiet_zone() {
        let img = render("bsgames:abc123", 2).unwrap();
        assert!(img.pixels.iter().any(|&p| p == 0), "a real QR must have dark modules");
    }
}
