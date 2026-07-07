//! Texturing convention **ruleset** — the CPU reference implementations of every
//! convention the block shaders depend on. The GPU side (chunk.vert / chunk.frag /
//! the compute mesher) can't be compiled or run here, so the error-prone math
//! lives here as pure, tested Rust and the GLSL becomes a transcription of proven
//! formulas. [`super::shader_contract`] then statically checks the real shader
//! source against the constants pinned here.
//!
//! Nothing in this module touches Vulkan; it is all integer/float convention math
//! and verifiable in the fast lib path.

use crate::world::block_textures::{self, BlockTiles};
use crate::world::blocks::BlockId;

/// Chunk edge length in blocks. MUST equal `CHUNK_EXTENT` in chunk.vert — the
/// vertex shader scales the unorm-packed local position by this. Pinned by the
/// shader contract.
pub const CHUNK_EXTENT: f32 = 32.0;

/// The six axis-aligned cube faces. Voxel faces are always one of these, so the
/// fragment shader's axis-snapped normal collapses to exactly one `Face`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl Face {
    /// Outward unit normal.
    pub fn normal(self) -> [f32; 3] {
        match self {
            Face::PosX => [1.0, 0.0, 0.0],
            Face::NegX => [-1.0, 0.0, 0.0],
            Face::PosY => [0.0, 1.0, 0.0],
            Face::NegY => [0.0, -1.0, 0.0],
            Face::PosZ => [0.0, 0.0, 1.0],
            Face::NegZ => [0.0, 0.0, -1.0],
        }
    }
}

/// Snap a (possibly noisy) normal to its cardinal face. This is the exact
/// transcription of chunk.frag's axis-snap block: pick the axis with the largest
/// absolute component (strict `>`, ties falling through to Y then Z), then take
/// its sign. Keeping the reference here means the shader's snap can be regression-
/// checked and any future port (e.g. into the mesher) starts from a proven spec.
///
/// Divergence note: at an exact zero component GLSL `sign(0)=0`; here we treat
/// `>= 0` as the positive face. Real voxel faces never produce a zero dominant
/// component, so this only differs on degenerate input.
pub fn snap_normal(n: [f32; 3]) -> Face {
    let ax = n[0].abs();
    let ay = n[1].abs();
    let az = n[2].abs();
    if ax > ay && ax > az {
        if n[0] >= 0.0 { Face::PosX } else { Face::NegX }
    } else if ay > az {
        if n[1] >= 0.0 { Face::PosY } else { Face::NegY }
    } else if n[2] >= 0.0 {
        Face::PosZ
    } else {
        Face::NegZ
    }
}

/// Which atlas tile a block shows on a given face. `top`/`bottom` for the up/down
/// faces, `side` for the four lateral faces.
pub fn tile_for_face(b: BlockId, face: Face) -> u16 {
    let t: BlockTiles = block_textures::tiles(b);
    match face {
        Face::PosY => t.top,
        Face::NegY => t.bottom,
        _ => t.side,
    }
}

// ─── Packed-vertex round-trips ──────────────────────────────────────────────
//
// The GPU mesher emits an 8-byte packed vertex: position as
// A2B10G10R10_UNORM_PACK32 (x=R bits0-9, y=G 10-19, z=B 20-29, w=A 30-31) and
// color as R8G8B8A8_UNORM (r 0-7, g 8-15, b 16-23, a 24-31). These mirror that
// packing so we can prove what survives a round-trip — in particular that a
// per-vertex material/tile id can ride in the reserved color-alpha byte
// losslessly, i.e. WITHOUT growing the vertex or changing the pipeline's
// vertex-input descriptions.

/// `fract` matching GLSL semantics (`x - floor(x)`, always in `[0,1)`).
pub fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// Pack a chunk-local position (each axis in `[0, CHUNK_EXTENT]`) plus a 2-bit
/// `w` into A2B10G10R10_UNORM_PACK32.
pub fn pack_pos_unorm(local: [f32; 3], w2: u8) -> u32 {
    let q = |v: f32| -> u32 {
        let u = (v / CHUNK_EXTENT).clamp(0.0, 1.0);
        (u * 1023.0 + 0.5) as u32 & 0x3FF
    };
    q(local[0]) | (q(local[1]) << 10) | (q(local[2]) << 20) | (((w2 as u32) & 0x3) << 30)
}

/// Unpack the position (in blocks) from A2B10G10R10_UNORM_PACK32.
pub fn unpack_pos(p: u32) -> [f32; 3] {
    let d = |bits: u32| -> f32 { (bits & 0x3FF) as f32 / 1023.0 * CHUNK_EXTENT };
    [d(p), d(p >> 10), d(p >> 20)]
}

/// Pack flat-shaded rgb plus a material/tile id (the reserved alpha byte) into
/// R8G8B8A8_UNORM.
pub fn pack_color_id(rgb: [u8; 3], id: u8) -> u32 {
    (rgb[0] as u32) | ((rgb[1] as u32) << 8) | ((rgb[2] as u32) << 16) | ((id as u32) << 24)
}

/// Recover the material/tile id from the packed color word.
pub fn unpack_id(c: u32) -> u8 {
    ((c >> 24) & 0xFF) as u8
}

/// Recover the id the way the *shader* must: from the unorm alpha float
/// `in_color_unorm.a` in `[0,1]`. Rounds to the nearest byte. The shader line
/// this pins is `int(in_color_unorm.a * 255.0 + 0.5)`.
pub fn decode_id_from_unorm(a: f32) -> u8 {
    (a * 255.0 + 0.5) as u8
}

// ─── Per-face UV reconstruction (procedural / solid path) ────────────────────
//
// For the less-invasive texturing path, the fragment shader derives the in-face
// UV from the world position and the snapped face — no UV vertex attribute. Each
// face uses the two world axes tangent to it; `fract` gives seamless 1m tiling
// across block boundaries. Orientation (which tangent is U vs V, and any flip) is
// a visual convention to confirm on the machine; these are the axes and the
// tiling/range invariants the shader must satisfy.

/// In-face UV in `[0,1)` derived from world position, for the given face.
pub fn face_uv(world: [f32; 3], face: Face) -> [f32; 2] {
    let (a, b) = match face {
        Face::PosX | Face::NegX => (world[2], world[1]), // tangents Z,Y
        Face::PosY | Face::NegY => (world[0], world[2]), // tangents X,Z
        Face::PosZ | Face::NegZ => (world[0], world[1]), // tangents X,Y
    };
    [fract(a), fract(b)]
}

// ─── Atlas vs texture array ──────────────────────────────────────────────────
//
// Chosen path is a texture ARRAY (one layer per tile): no inter-tile bleed, clean
// per-tile mips, array_layer is just the tile index. The atlas mapping is kept as
// a fallback and to test the bleed-avoiding inset math.

/// Texture-array layer for a tile (identity — this is the chosen path).
pub fn array_layer(tile: u16) -> u32 {
    tile as u32
}

/// Map an in-tile UV in `[0,1]` into a tile's atlas rect, inset by `inset_texels`
/// (half a texel is the usual bleed guard) given the atlas dimension in texels.
/// Returns absolute atlas UV. Used only on the atlas fallback path.
pub fn atlas_uv(tile: u16, in_uv: [f32; 2], inset_texels: f32, atlas_px: f32) -> [f32; 2] {
    let r = block_textures::tile_uv(tile); // [u0,v0,u1,v1]
    let inset = inset_texels / atlas_px.max(1.0);
    let u = r[0] + inset + in_uv[0].clamp(0.0, 1.0) * ((r[2] - r[0]) - 2.0 * inset);
    let v = r[1] + inset + in_uv[1].clamp(0.0, 1.0) * ((r[3] - r[1]) - 2.0 * inset);
    [u, v]
}

// ─── Mip chain descriptor math ───────────────────────────────────────────────
//
// Not a shader concern, but the parameters passed to Vulkan image creation. A
// full chain is required (anti-flicker is the migration priority); getting the
// level count wrong is a real bug.

/// Number of mip levels for a `w×h` image: `floor(log2(max(w,h))) + 1`.
pub fn mip_levels(w: u32, h: u32) -> u32 {
    let mut s = w.max(h).max(1);
    let mut levels = 1;
    while s > 1 {
        s /= 2;
        levels += 1;
    }
    levels
}

/// Dimensions of mip `level` (each axis halved, floored, min 1).
pub fn mip_dim(w: u32, h: u32, level: u32) -> (u32, u32) {
    ((w >> level).max(1), (h >> level).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block_textures::tile;

    #[test]
    fn snap_normal_picks_dominant_axis_and_sign() {
        assert_eq!(snap_normal([0.9, 0.1, -0.05]), Face::PosX);
        assert_eq!(snap_normal([-0.9, 0.1, 0.05]), Face::NegX);
        assert_eq!(snap_normal([0.1, 0.95, 0.2]), Face::PosY);
        assert_eq!(snap_normal([0.1, -0.95, 0.2]), Face::NegY);
        assert_eq!(snap_normal([0.2, 0.1, 0.9]), Face::PosZ);
        assert_eq!(snap_normal([0.2, 0.1, -0.9]), Face::NegZ);
        // A clean cardinal normal round-trips through snap unchanged.
        for f in [Face::PosX, Face::NegX, Face::PosY, Face::NegY, Face::PosZ, Face::NegZ] {
            assert_eq!(snap_normal(f.normal()), f);
        }
    }

    #[test]
    fn grass_shows_three_different_tiles() {
        assert_eq!(tile_for_face(BlockId::Grass, Face::PosY), tile::GRASS_TOP);
        assert_eq!(tile_for_face(BlockId::Grass, Face::PosX), tile::GRASS_SIDE);
        assert_eq!(tile_for_face(BlockId::Grass, Face::NegZ), tile::GRASS_SIDE);
        assert_eq!(tile_for_face(BlockId::Grass, Face::NegY), tile::DIRT);
    }

    #[test]
    fn position_pack_round_trips_within_quantization() {
        // Quantization step is CHUNK_EXTENT/1023 ≈ 0.0313 blocks.
        let step = CHUNK_EXTENT / 1023.0;
        for &p in &[[0.0, 0.0, 0.0], [16.0, 8.0, 31.0], [32.0, 32.0, 32.0], [5.5, 12.25, 27.9]] {
            let back = unpack_pos(pack_pos_unorm(p, 0));
            for k in 0..3 {
                assert!((back[k] - p[k]).abs() <= step + 1e-4, "axis {k}: {} vs {}", back[k], p[k]);
            }
        }
    }

    #[test]
    fn material_id_rides_alpha_losslessly() {
        // The architecture-enabling fact: a per-vertex id 0..=255 survives in the
        // reserved color-alpha byte exactly, so no vertex-stride / vertex-input
        // change is needed to carry a material id. Both the integer unpack and the
        // shader's unorm decode must recover it.
        for id in 0u16..=255 {
            let id = id as u8;
            let word = pack_color_id([122, 184, 86], id);
            assert_eq!(unpack_id(word), id, "integer unpack");
            let a_unorm = id as f32 / 255.0;
            assert_eq!(decode_id_from_unorm(a_unorm), id, "shader unorm decode");
            // rgb must be untouched by stashing the id.
            assert_eq!(word & 0x00FF_FFFF, 122 | (184 << 8) | (86 << 16));
        }
    }

    #[test]
    fn every_tile_in_use_fits_the_alpha_id_channel() {
        // If any block's tile index exceeded 255 the alpha-id plan would break.
        let blocks = [
            BlockId::Stone, BlockId::Grass, BlockId::Dirt, BlockId::Sand,
            BlockId::Cobblestone, BlockId::Bedrock, BlockId::Water, BlockId::Lava,
        ];
        for b in blocks {
            let t = block_textures::tiles(b);
            for idx in [t.top, t.side, t.bottom] {
                assert!(idx <= 255, "tile {idx} exceeds the 8-bit id channel");
            }
        }
    }

    #[test]
    fn face_uv_tiles_seamlessly_and_stays_in_unit_range() {
        // Same fractional UV one block over → seamless 1m tiling.
        let a = face_uv([1.5, 4.0, 2.25], Face::PosY); // tangents x,z
        let b = face_uv([2.5, 4.0, 3.25], Face::PosY);
        assert!((a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5);
        // Range and correct tangent axes.
        for f in [Face::PosX, Face::NegX, Face::PosY, Face::NegY, Face::PosZ, Face::NegZ] {
            let uv = face_uv([3.7, 9.2, -4.1], f);
            assert!(uv[0] >= 0.0 && uv[0] < 1.0 && uv[1] >= 0.0 && uv[1] < 1.0);
        }
        // PosX uses Z,Y (not X): moving along X must not change its UV.
        let p0 = face_uv([0.3, 1.7, 2.9], Face::PosX);
        let p1 = face_uv([5.3, 1.7, 2.9], Face::PosX);
        assert_eq!(p0, p1);
    }

    #[test]
    fn atlas_uv_inset_keeps_sampling_inside_the_tile() {
        // Even at the UV extremes, the inset keeps us strictly within the tile
        // rect, so trilinear/aniso can't bleed into a neighbour tile.
        let r = block_textures::tile_uv(tile::STONE);
        for &corner in &[[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]] {
            let uv = atlas_uv(tile::STONE, corner, 0.5, 256.0);
            assert!(uv[0] >= r[0] && uv[0] <= r[2], "u within tile");
            assert!(uv[1] >= r[1] && uv[1] <= r[3], "v within tile");
        }
    }

    #[test]
    fn array_layer_is_the_tile_index() {
        assert_eq!(array_layer(tile::STONE), tile::STONE as u32);
        assert_eq!(array_layer(tile::LAVA), tile::LAVA as u32);
    }

    #[test]
    fn mip_chain_math_matches_known_sizes() {
        assert_eq!(mip_levels(256, 256), 9);
        assert_eq!(mip_levels(1, 1), 1);
        assert_eq!(mip_levels(1024, 1024), 11);
        assert_eq!(mip_levels(256, 64), 9); // driven by the larger axis
        assert_eq!(mip_dim(256, 256, 0), (256, 256));
        assert_eq!(mip_dim(256, 256, 8), (1, 1));
        assert_eq!(mip_dim(256, 64, 3), (32, 8));
        let _ = block_textures::ATLAS_COLS; // pinned constant referenced by the contract
    }
}
