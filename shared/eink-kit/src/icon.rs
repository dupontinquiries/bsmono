//! Launcher icons for apps that have none.
//!
//! AppLoad shows `<app dir>/icon.png` in xochitl's sidebar and falls back to
//! a generic glyph. None of the monorepo's apps ship an icon, so this draws
//! a letter tile (the same fallback hatch uses): ink square, paper letters,
//! rendered with eink-kit's own rasterizer so it looks exactly as the panel
//! would draw it, and encoded as a PNG with no image dependency.

use crate::format::PixelFormat;
use crate::raster::{render, Target, Textures};

/// Up to two initials: "eink-demo" → "ED", "tomescope" → "TO".
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| c == '-' || c == '_' || c == ' ').filter(|w| !w.is_empty()).collect();
    let s: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
    };
    s.to_uppercase()
}

/// A `size`×`size` letter-tile PNG for `name`.
pub fn letter_tile_png(name: &str, size: u32) -> Vec<u8> {
    let ctx = egui::Context::default();
    let text = initials(name);
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size as f32, size as f32))),
        ..Default::default()
    };
    raw.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { native_pixels_per_point: Some(1.0), ..Default::default() });
    let mut textures = Textures::default();
    let mut last = None;
    // Two passes: the font atlas is uploaded on the first.
    for _ in 0..2 {
        let out = ctx.run_ui(raw.clone(), |ui| {
            let r = ui.ctx().content_rect().shrink(size as f32 * 0.04);
            let p = ui.painter();
            p.rect_filled(r, size as f32 * 0.18, egui::Color32::BLACK);
            let font = egui::FontId::proportional(size as f32 * if text.chars().count() > 1 { 0.42 } else { 0.55 });
            p.text(r.center(), egui::Align2::CENTER_CENTER, &text, font, egui::Color32::WHITE);
        });
        textures.apply_set(&out.textures_delta);
        last = Some(out);
    }
    let out = last.unwrap();
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    let mut buf = vec![0u8; (size * size * 3) as usize];
    render(&prims, out.pixels_per_point, &textures,
        Target { buf: &mut buf, stride: size as usize * 3, width: size, height: size, format: PixelFormat::Rgb888 },
        [255, 255, 255, 255], 1, true, None);
    encode_png_rgb(size, size, &buf)
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// RGB8 PNG, zlib with stored (uncompressed) blocks. Icons are small; not
/// compressing keeps this 40 lines and dependency-free.
pub fn encode_png_rgb(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w as usize * 3 + 1) * h as usize);
    for row in rgb.chunks(w as usize * 3) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(if chunks.peek().is_none() { 1 } else { 0 });
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |ty: &[u8], data: &[u8]| {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = ty.to_vec();
        body.extend_from_slice(data);
        png.extend_from_slice(&body);
        png.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolour
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_are_sensible() {
        assert_eq!(initials("eink-demo"), "ED");
        assert_eq!(initials("tomescope"), "TO");
        assert_eq!(initials("x"), "X");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn crc_matches_the_png_spec_example() {
        // CRC of "IEND" with no data is the constant every PNG ends with
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
    }

    /// Decode our own output (stored blocks only) and check the pixels:
    /// ink tile, paper background at the corner, paper letters inside.
    #[test]
    fn the_tile_decodes_and_has_ink_and_letters() {
        let size = 96u32;
        let png = letter_tile_png("eink-demo", size);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let idat_len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let z = &png[41..41 + idat_len];
        let (mut i, mut raw) = (2usize, Vec::new());
        loop {
            let last = z[i] & 1;
            let len = u16::from_le_bytes([z[i + 1], z[i + 2]]) as usize;
            raw.extend_from_slice(&z[i + 5..i + 5 + len]);
            i += 5 + len;
            if last == 1 { break; }
        }
        assert_eq!(u32::from_be_bytes(z[i..i + 4].try_into().unwrap()), adler32(&raw));
        let px = |x: u32, y: u32| { let o = (y * (size * 3 + 1) + 1 + x * 3) as usize; raw[o] };
        assert_eq!(px(0, 0), 255, "paper outside the rounded tile");
        assert_eq!(px(size / 2, size / 8), 0, "ink inside the tile");
        let letter_px = (size / 4..size * 3 / 4).flat_map(|y| (size / 4..size * 3 / 4).map(move |x| (x, y)))
            .filter(|&(x, y)| px(x, y) > 200).count();
        assert!(letter_px > 50, "letters should leave paper-white pixels, found {letter_px}");
    }
}
