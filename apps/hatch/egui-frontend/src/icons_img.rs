//! icons_img — one function from icon file to [`egui::ColorImage`].
//!
//! The decode stack is the only heavy dependency this shell carries, so it is
//! quarantined here: `decode()` is the whole surface, callers never see
//! `image` or `resvg` types, and the `svg` feature can be dropped for a
//! min-size build without touching a call site (svg-only icons then fall back
//! to the mono letter tile, which is a degradation, not a break).

use std::path::Path;

/// Decode `path` to an RGBA image, rasterising svg at `target_px` square.
/// `None` covers every failure — missing file, bad data, unsupported format —
/// because the caller's answer to all of them is the same letter tile.
pub fn decode(path: &Path, target_px: u32) -> Option<egui::ColorImage> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let bytes = std::fs::read(path).ok()?;
    match ext {
        "svg" | "svgz" => decode_svg(&bytes, target_px),
        _ => decode_png(&bytes),
    }
}

fn decode_png(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        rgba.as_raw(),
    ))
}

#[cfg(feature = "svg")]
fn decode_svg(bytes: &[u8], target_px: u32) -> Option<egui::ColorImage> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &opt).ok()?;
    let size = tree.size();
    let (sw, sh) = (size.width().max(1.0), size.height().max(1.0));
    let scale = (target_px as f32 / sw).min(target_px as f32 / sh);
    let (w, h) = (((sw * scale).round() as u32).max(1), ((sh * scale).round() as u32).max(1));
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia gives premultiplied RGBA; egui wants unmultiplied.
    let mut data = pixmap.take();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            px[0] = (px[0] as u32 * 255 / a).min(255) as u8;
            px[1] = (px[1] as u32 * 255 / a).min(255) as u8;
            px[2] = (px[2] as u32 * 255 / a).min(255) as u8;
        }
    }
    Some(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &data))
}

#[cfg(not(feature = "svg"))]
fn decode_svg(_bytes: &[u8], _target_px: u32) -> Option<egui::ColorImage> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real png produced by the same `image` crate we decode with — an
    /// encode→decode round trip instead of inlined bytes, so the fixture can
    /// never be wrong by hand.
    fn red_px_png() -> Vec<u8> {
        let mut img = image::RgbaImage::new(1, 1);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("png encode");
        out.into_inner()
    }

    #[test]
    fn a_png_decodes_to_its_pixels() {
        let img = decode_png(&red_px_png()).expect("valid png");
        assert_eq!(img.size, [1, 1]);
        let p = img.pixels[0];
        assert_eq!((p.r(), p.a()), (255, 255));
    }

    #[cfg(feature = "svg")]
    #[test]
    fn an_svg_rasterises_at_the_requested_size() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#00ff00"/></svg>"##;
        let img = decode_svg(svg, 64).expect("valid svg");
        assert_eq!(img.size, [64, 64]);
        assert_eq!(img.pixels[0].g(), 255);
    }

    #[test]
    fn garbage_bytes_are_none_not_a_panic() {
        assert!(decode_png(b"not a png").is_none());
        assert!(decode_svg(b"<svg", 64).is_none() || cfg!(not(feature = "svg")));
    }
}
