//! Block texturing data — the pure mapping from a block (and which face) to an
//! atlas tile, plus the UV math for a tile. This is the data layer the *textured*
//! mesher and fragment shader will consume; the GPU atlas upload and the shader
//! sampling are wired into the render path during the live texturing session.
//!
//! Convention: a square atlas of [`ATLAS_COLS`]×[`ATLAS_COLS`] tiles, indexed in
//! row-major order from the top-left. The art fills these slots; the tile indices
//! in [`tile`] are the stable contract between art and code.

use crate::world::blocks::BlockId;

/// Tiles per atlas side (so `ATLAS_COLS² = 256` slots in a 16×16 sheet).
pub const ATLAS_COLS: u16 = 16;

/// Which atlas tile each of a block's faces samples. Most blocks are uniform
/// (all faces equal); grass-like blocks differ on top / side / bottom.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BlockTiles {
    pub top: u16,
    pub side: u16,
    pub bottom: u16,
}

impl BlockTiles {
    const fn uniform(t: u16) -> BlockTiles {
        BlockTiles { top: t, side: t, bottom: t }
    }
}

/// The atlas slots the art fills. Kept explicit and named so the mapping reads
/// clearly and the indices stay stable as art lands.
pub mod tile {
    pub const GRASS_TOP: u16 = 0;
    pub const GRASS_SIDE: u16 = 1;
    pub const DIRT: u16 = 2;
    pub const STONE: u16 = 3;
    pub const COBBLE: u16 = 4;
    pub const SAND: u16 = 5;
    pub const WATER: u16 = 6;
    pub const LAVA: u16 = 7;
    pub const BEDROCK: u16 = 8;
    /// Magenta/black checkerboard fallback for any unmapped block.
    pub const MISSING: u16 = 255;
}

/// The atlas tiles for a block's faces. Unmapped blocks fall back to the
/// `MISSING` checkerboard so a gap is obvious in-world rather than invisible.
pub fn tiles(b: BlockId) -> BlockTiles {
    use tile::*;
    match b {
        BlockId::Grass => BlockTiles { top: GRASS_TOP, side: GRASS_SIDE, bottom: DIRT },
        BlockId::Dirt => BlockTiles::uniform(DIRT),
        BlockId::Stone => BlockTiles::uniform(STONE),
        BlockId::Cobblestone => BlockTiles::uniform(COBBLE),
        BlockId::Sand => BlockTiles::uniform(SAND),
        BlockId::Water => BlockTiles::uniform(WATER),
        BlockId::Lava => BlockTiles::uniform(LAVA),
        BlockId::Bedrock => BlockTiles::uniform(BEDROCK),
        _ => BlockTiles::uniform(MISSING),
    }
}

/// The UV rectangle `(u0, v0, u1, v1)` of `tile` in the atlas — row-major from
/// the top-left, normalized to `[0,1]`. This is what the mesher writes onto the
/// face's four corners (scaled by the quad's footprint for tiling) and what the
/// fragment shader samples.
pub fn tile_uv(tile: u16) -> [f32; 4] {
    let cols = ATLAS_COLS;
    let col = (tile % cols) as f32;
    let row = (tile / cols) as f32;
    let s = 1.0 / cols as f32;
    [col * s, row * s, (col + 1.0) * s, (row + 1.0) * s]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grass_differs_by_face_others_uniform() {
        let g = tiles(BlockId::Grass);
        assert_ne!(g.top, g.side);
        assert_ne!(g.side, g.bottom);
        assert_eq!(g.bottom, tile::DIRT, "grass bottom is dirt");
        let s = tiles(BlockId::Stone);
        assert_eq!(s.top, s.side);
        assert_eq!(s.side, s.bottom);
    }

    #[test]
    fn unmapped_block_is_the_missing_checker() {
        // Air is never meshed, but it stands in for any unmapped id here.
        assert_eq!(tiles(BlockId::Air).top, tile::MISSING);
    }

    #[test]
    fn tile_uv_lays_out_row_major() {
        let s = 1.0 / ATLAS_COLS as f32;
        // tile 0 is the top-left cell.
        assert_eq!(tile_uv(0), [0.0, 0.0, s, s]);
        // tile 1 advances one column.
        assert_eq!(tile_uv(1), [s, 0.0, 2.0 * s, s]);
        // tile = ATLAS_COLS wraps to the next row, column 0.
        assert_eq!(tile_uv(ATLAS_COLS), [0.0, s, s, 2.0 * s]);
        // every tile is exactly one cell wide/tall.
        let uv = tile_uv(40);
        assert!((uv[2] - uv[0] - s).abs() < 1e-6 && (uv[3] - uv[1] - s).abs() < 1e-6);
    }
}

// r347 textures — the data path from block faces to atlas pixels.

impl BlockTiles {
    /// Tile for a face index (0..5) under the engine's FACE_NORMALS order
    /// (+X,-X,+Y,-Y,+Z,-Z): +Y => top, -Y => bottom, else side.
    pub fn face_tile(&self, face: usize) -> u16 {
        match face {
            2 => self.top,    // +Y
            3 => self.bottom, // -Y
            _ => self.side,
        }
    }
}

/// The per-vertex tile byte the mesher packs into the color alpha for a block
/// face. Mapped blocks → their tile (0..=254); unmapped/`MISSING` → 255, the
/// "no atlas, keep procedural shading" sentinel the fragment shader honours.
pub fn face_tile_byte(b: BlockId, face: usize) -> u8 {
    let t = tiles(b).face_tile(face);
    if t == tile::MISSING || t > 254 { 255 } else { t as u8 }
}

/// Atlas side length in pixels (16 px tiles × 16 tiles).
pub const ATLAS_PX: u32 = (ATLAS_COLS as u32) * 16;

/// Build the block atlas as RGBA8 (`ATLAS_PX² × 4` bytes), row-major tiles.
/// This is programmer-art (distinct per-tile patterns) so textures are visibly
/// distinguishable until real art replaces it; `flat_white` returns an all-white
/// sheet (sampling → white, so `albedo = white * vertex_tint` ≈ the untextured
/// look — the bisect toggle).
pub fn atlas_rgba(flat_white: bool) -> Vec<u8> {
    let px = ATLAS_PX as usize;
    let mut out = vec![0u8; px * px * 4];
    if flat_white {
        out.iter_mut().for_each(|b| *b = 255);
        return out;
    }
    // base color per named tile; pattern adds a little per-texel variation.
    let base = |t_id: u16| -> [u8; 3] {
        match t_id {
            tile::GRASS_TOP => [86, 150, 62],
            tile::GRASS_SIDE => [104, 88, 56],
            tile::DIRT => [120, 86, 58],
            tile::STONE => [128, 128, 132],
            tile::COBBLE => [112, 112, 116],
            tile::SAND => [214, 198, 144],
            tile::WATER => [54, 96, 170],
            tile::LAVA => [206, 96, 36],
            tile::BEDROCK => [42, 42, 46],
            _ => [200, 30, 200], // MISSING / unfilled → magenta
        }
    };
    let cols = ATLAS_COLS as usize;
    for ty in 0..cols {
        for tx in 0..cols {
            let tile = (ty * cols + tx) as u16;
            let c = base(tile);
            for py in 0..16 {
                for pxl in 0..16 {
                    // cheap deterministic grain so tiles aren't flat.
                    let n = ((px_hash(tx, ty, pxl, py) >> 4) & 0x1F) as i32 - 16;
                    let gx = ty * 16 + py; // image row
                    let gxp = tx * 16 + pxl; // image col
                    let idx = (gx * px + gxp) * 4;
                    out[idx] = (c[0] as i32 + n).clamp(0, 255) as u8;
                    out[idx + 1] = (c[1] as i32 + n).clamp(0, 255) as u8;
                    out[idx + 2] = (c[2] as i32 + n).clamp(0, 255) as u8;
                    out[idx + 3] = 255;
                }
            }
        }
    }
    out
}

#[inline]
fn px_hash(tx: usize, ty: usize, x: usize, y: usize) -> u32 {
    let mut h = (tx as u32).wrapping_mul(73856093)
        ^ (ty as u32).wrapping_mul(19349663)
        ^ (x as u32).wrapping_mul(83492791)
        ^ (y as u32).wrapping_mul(2654435761);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1e995);
    h ^ (h >> 15)
}

#[cfg(test)]
mod r347_atlas_tests {
    use super::*;
    #[test]
    fn atlas_dims_and_white_toggle() {
        let a = atlas_rgba(false);
        assert_eq!(a.len(), (ATLAS_PX * ATLAS_PX * 4) as usize);
        assert!(a.iter().any(|&b| b != 255), "procedural atlas isn't all white");
        let w = atlas_rgba(true);
        assert!(w.iter().all(|&b| b == 255), "flat_white is all white");
    }
    #[test]
    fn face_tile_byte_maps_known_and_sentinels_unknown() {
        assert_eq!(face_tile_byte(BlockId::Grass, 2), tile::GRASS_TOP as u8);
        assert_eq!(face_tile_byte(BlockId::Grass, 3), tile::DIRT as u8); // bottom
        assert_eq!(face_tile_byte(BlockId::Grass, 0), tile::GRASS_SIDE as u8);
        assert_eq!(face_tile_byte(BlockId::Air, 2), 255); // unmapped → sentinel
    }
}
