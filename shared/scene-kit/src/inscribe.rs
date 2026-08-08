//! inscribe — turns styled text lines into the RGBA page that `u_page` samples.
//!
//! This is the CPU half of the inscription pipeline: `textsurface` decides
//! *which shape* carries the words and how many fit; this module *rasterizes*
//! them; `mesh3d::panel_uv` + `gpuscene::MAT_INSCRIPTION` put the pixels on
//! the face. The font is font8x8 (public-domain 8×8 bitmap) scaled by an
//! integer factor — deliberately chunky: on a wooden lectern the text should
//! read as engraved, not as a floating VS Code. Glyph pixels get span color
//! at full alpha; everything else is transparent, so the parchment/wood grain
//! shows through between the letters (the shader mixes by alpha).
//!
//! Byte-range spans, matching aperture-core's lexer output. Non-ASCII lands
//! as `·` (font8x8 BASIC covers 0..=0x7f). Tabs are the caller's to expand —
//! a rasterizer should not have opinions about tab stops.

use font8x8::legacy::BASIC_LEGACY;
use std::ops::Range;

pub const GLYPH: usize = 8;

/// One text row and its colored byte ranges (unstyled bytes take `ink`).
#[derive(Clone, Debug, Default)]
pub struct StyledLine {
    pub text: String,
    pub spans: Vec<(Range<usize>, [u8; 3])>,
}

/// A finished page raster: tightly-packed RGBA, top row first (`panel_uv`'s V axis).
#[derive(Clone, Debug)]
pub struct Page {
    pub w: i32,
    pub h: i32,
    pub rgba: Vec<u8>,
}

/// Rasterize `lines` into a `cols`-wide page at `scale`× (pixel size =
/// `cols*8*scale` × `lines.len()*8*scale`... plus one glyph of margin all
/// around, so ink never kisses the panel's edge). Lines longer than `cols`
/// are clipped — horizontal scroll is a viewer concern.
pub fn render_page(lines: &[StyledLine], cols: usize, scale: usize, ink: [u8; 3]) -> Page {
    let scale = scale.max(1);
    let cell = GLYPH * scale;
    let margin = cell;
    let w = cols * cell + margin * 2;
    let h = lines.len().max(1) * cell + margin * 2;
    let mut rgba = vec![0u8; w * h * 4];

    for (row, line) in lines.iter().enumerate() {
        let y0 = margin + row * cell;
        for (col, (byte, ch)) in line.text.char_indices().take(cols).enumerate() {
            let code = ch as u32;
            let glyph = if code < 0x80 { BASIC_LEGACY[code as usize] } else { BASIC_LEGACY[b'.' as usize] };
            // Color for this char: last span covering its byte offset wins.
            let mut color = ink;
            for (r, c) in &line.spans {
                if r.contains(&byte) {
                    color = *c;
                }
            }
            let x0 = margin + col * cell;
            for (gy, bits) in glyph.iter().enumerate() {
                for gx in 0..GLYPH {
                    if bits & (1 << gx) == 0 {
                        continue;
                    }
                    for sy in 0..scale {
                        let py = y0 + gy * scale + sy;
                        let base_row = (py * w + x0 + gx * scale) * 4;
                        for sx in 0..scale {
                            let i = base_row + sx * 4;
                            rgba[i] = color[0];
                            rgba[i + 1] = color[1];
                            rgba[i + 2] = color[2];
                            rgba[i + 3] = 255;
                        }
                    }
                }
            }
        }
    }
    Page { w: w as i32, h: h as i32, rgba }
}

/// How many `cols`×`rows` at 8px/glyph a given texture budget allows —
/// the bridge to `textsurface::Surface::char_budget`, in pixels.
pub fn page_px(cols: usize, rows: usize, scale: usize) -> (usize, usize) {
    let cell = GLYPH * scale.max(1);
    (cols * cell + cell * 2, rows * cell + cell * 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(t: &str) -> StyledLine {
        StyledLine { text: t.into(), spans: vec![] }
    }

    #[test]
    fn glyphs_land_and_blanks_stay_transparent() {
        let p = render_page(&[line("A "), line("  ")], 2, 1, [10, 20, 30]);
        assert_eq!((p.w, p.h), (2 * 8 + 16, 2 * 8 + 16));
        let ink: usize = p.rgba.chunks(4).filter(|px| px[3] == 255).count();
        assert!(ink > 8 && ink < 40, "an 'A' is some pixels, not none, not a block: {ink}");
        // Every inked pixel carries the ink color.
        assert!(p.rgba.chunks(4).filter(|px| px[3] == 255).all(|px| px[..3] == [10, 20, 30]));
        // Row 2 ("  ") is fully transparent.
        let row2_start = ((8 + 8) * p.w as usize) * 4;
        assert!(p.rgba[row2_start..row2_start + (p.w as usize * 4)].chunks(4).all(|px| px[3] == 0));
    }

    #[test]
    fn spans_color_their_bytes_and_scale_scales() {
        let styled = StyledLine { text: "ab".into(), spans: vec![(0..1, [200, 0, 0])] };
        let p = render_page(&[styled.clone()], 2, 1, [0, 0, 200]);
        let reds = p.rgba.chunks(4).filter(|px| px[3] == 255 && px[0] == 200).count();
        let blues = p.rgba.chunks(4).filter(|px| px[3] == 255 && px[2] == 200).count();
        assert!(reds > 0 && blues > 0, "both the span color and the ink appear");
        let p2 = render_page(&[styled], 2, 2, [0, 0, 200]);
        let ink1 = p.rgba.chunks(4).filter(|px| px[3] == 255).count();
        let ink2 = p2.rgba.chunks(4).filter(|px| px[3] == 255).count();
        assert_eq!(ink2, ink1 * 4, "2× scale quadruples inked pixels");
    }

    #[test]
    fn long_lines_clip_and_non_ascii_degrades() {
        let p = render_page(&[line("xxxxxxxxxx")], 3, 1, [255, 255, 255]);
        assert_eq!(p.w, 3 * 8 + 16, "width is cols, not text length");
        let q = render_page(&[line("é")], 1, 1, [255, 255, 255]);
        assert!(q.rgba.chunks(4).any(|px| px[3] == 255), "replacement glyph still inks");
    }
}
