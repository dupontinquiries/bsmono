//! `world::title_structure` — the UNENDLICH title as a monument of REAL voxel
//! blocks placed in the world (a floating landmark by spawn / the menu backdrop),
//! drawn by the actual block renderer — not UI text. Reuses `sim::voxel_text`'s
//! 5×7 block-letter geometry, mapped to world positions of a chosen block type.
//! Pure + deterministic: positions in, no IO/RNG.

use crate::sim::voxel_text::voxel_text;
use crate::world::blocks::BlockId;

/// Block the title letters are built from: pale-violet Crystal (slightly emissive),
/// so the monument reads against terrain and glows a little.
pub const TITLE_BLOCK: BlockId = BlockId::Crystal;

/// World-block positions for `text`, lower-left of the first glyph at `origin`.
/// Letters stand upright: x runs across, y up, z is the 2-block extrusion depth —
/// readable looking toward +Z (i.e. from −Z).
pub fn title_blocks(origin: [i32; 3], text: &str) -> Vec<[i32; 3]> {
    voxel_text(text)
        .into_iter()
        .map(|b| [origin[0] + b[0], origin[1] + b[1], origin[2] + b[2]])
        .collect()
}

/// The "UNENDLICH" monument at `origin`.
pub fn unendlich_blocks(origin: [i32; 3]) -> Vec<[i32; 3]> {
    title_blocks(origin, "UNENDLICH")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monument_is_offset_to_origin_and_upright() {
        let o = [100, 70, -40];
        let blocks = unendlich_blocks(o);
        assert!(!blocks.is_empty());
        // every block sits at or above the origin baseline, within a 7-tall band,
        // and within the 53-wide title span + 2-deep extrusion.
        let minx = blocks.iter().map(|b| b[0]).min().unwrap();
        let maxx = blocks.iter().map(|b| b[0]).max().unwrap();
        assert_eq!(minx, o[0], "first glyph starts at origin x");
        assert!(maxx - o[0] <= 53 && maxx - o[0] >= 50, "spans the title width");
        assert!(blocks.iter().all(|b| (o[1]..o[1] + 7).contains(&b[1])), "7 blocks tall, upright");
        assert!(blocks.iter().all(|b| (o[2]..o[2] + 2).contains(&b[2])), "2-block extrusion");
    }

    #[test]
    fn deterministic() {
        assert_eq!(unendlich_blocks([0, 0, 0]), unendlich_blocks([0, 0, 0]));
    }
}
