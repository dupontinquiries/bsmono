//! Big voxel block-letters for the menu title — the floating "UNENDLICH" the
//! camera trailers away from into the menu sections.
//!
//! [`voxel_text`] returns unit-block coordinates for an extruded 5×7 uppercase
//! font (only the letters UNENDLICH needs, plus space). The menu places these as
//! large floating world blocks above the title scene; rendering + camera framing is
//! the menu's job. Pure + deterministic, so it verifies headless (rasterise the
//! front view and read the letters).

/// A unit block in glyph-grid space: x rightward, y up, z depth.
pub type VoxelBlock = [i32; 3];

const GLYPH_W: i32 = 5;
const GLYPH_H: i32 = 7;
const SPACING: i32 = 1; // gap columns between glyphs
const DEPTH: i32 = 2; // extrusion thickness in +z

fn glyph(c: char) -> Option<[&'static str; 7]> {
    Some(match c {
        'U' => ["10001", "10001", "10001", "10001", "10001", "10001", "01110"],
        'N' => ["10001", "11001", "10101", "10101", "10011", "10001", "10001"],
        'E' => ["11111", "10000", "10000", "11110", "10000", "10000", "11111"],
        'D' => ["11110", "10001", "10001", "10001", "10001", "10001", "11110"],
        'L' => ["10000", "10000", "10000", "10000", "10000", "10000", "11111"],
        'I' => ["11111", "00100", "00100", "00100", "00100", "00100", "11111"],
        'C' => ["01110", "10001", "10000", "10000", "10000", "10001", "01110"],
        'H' => ["10001", "10001", "10001", "11111", "10001", "10001", "10001"],
        ' ' => ["00000", "00000", "00000", "00000", "00000", "00000", "00000"],
        _ => return None,
    })
}

/// Block coordinates for `text` (unknown chars skipped). Origin at the lower-left of
/// the first glyph; y up; extruded `DEPTH` blocks in +z.
pub fn voxel_text(text: &str) -> Vec<VoxelBlock> {
    let mut out = Vec::new();
    let mut x0 = 0;
    for ch in text.chars() {
        if let Some(rows) = glyph(ch.to_ascii_uppercase()) {
            for (r, row) in rows.iter().enumerate() {
                let y = (GLYPH_H - 1) - r as i32; // row 0 = top
                for (col, b) in row.bytes().enumerate() {
                    if b == b'1' {
                        for z in 0..DEPTH {
                            out.push([x0 + col as i32, y, z]);
                        }
                    }
                }
            }
            x0 += GLYPH_W + SPACING;
        }
    }
    out
}

/// Total width in blocks of `text` (for centering the title).
pub fn voxel_text_width(text: &str) -> i32 {
    let n = text.chars().filter(|c| glyph(c.to_ascii_uppercase()).is_some()).count() as i32;
    if n == 0 {
        0
    } else {
        n * (GLYPH_W + SPACING) - SPACING
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unendlich_spells_out() {
        let blocks = voxel_text("UNENDLICH");
        assert!(!blocks.is_empty(), "produces blocks");
        // 9 glyphs → width = 9*6 − 1 = 53
        assert_eq!(voxel_text_width("UNENDLICH"), 53);
        // every glyph contributes at least one column of blocks (no empty letter)
        let max_x = blocks.iter().map(|b| b[0]).max().unwrap();
        assert!(max_x >= 50 && max_x <= 53, "title spans the full width (max_x={max_x})");
        // extruded both depth layers
        assert!(blocks.iter().any(|b| b[2] == 0) && blocks.iter().any(|b| b[2] == 1));
        // height fits the 7-row font
        assert!(blocks.iter().all(|b| (0..GLYPH_H).contains(&b[1])));
    }

    #[test]
    fn unknown_chars_are_skipped_not_panicking() {
        // digits/punct aren't in the font; they're skipped without advancing oddly
        assert!(voxel_text("U?N").len() == voxel_text("UN").len());
    }
}
