//! Texture atlas coordinate mechanism (pure, texture-agnostic).
//!
//! An atlas is a grid of equal-size tiles. This module turns a tile *index*
//! into the UV rectangle the mesher emits and the fragment shader samples. It
//! knows the grid *shape*, never the pixels — drop in any atlas with the same
//! layout and the coordinates still hold. The `BlockId -> tile index` mapping
//! is intentionally a separate concern (it depends on which blocks the art set
//! actually covers).

/// Shape of a tile atlas: square `tile_px` tiles in a `cols × rows` grid,
/// indexed row-major from the top-left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasLayout {
    pub tile_px: u32,
    pub cols: u32,
    pub rows: u32,
}

impl AtlasLayout {
    pub const fn new(tile_px: u32, cols: u32, rows: u32) -> Self {
        AtlasLayout { tile_px, cols, rows }
    }

    /// Number of tile slots.
    pub const fn len(&self) -> u32 {
        self.cols * self.rows
    }

    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `(col, row)` of a tile index in row-major order.
    pub const fn cell(&self, index: u32) -> (u32, u32) {
        (index % self.cols, index / self.cols)
    }

    /// UV rectangle `[u0, v0, u1, v1]` for a tile index, in `0..1` atlas space.
    /// `None` if the index is past the last tile.
    pub fn tile_uv(&self, index: u32) -> Option<[f32; 4]> {
        if index >= self.len() {
            return None;
        }
        let (c, r) = self.cell(index);
        let uw = 1.0 / self.cols as f32;
        let vh = 1.0 / self.rows as f32;
        let u0 = c as f32 * uw;
        let v0 = r as f32 * vh;
        Some([u0, v0, u0 + uw, v0 + vh])
    }

    /// Like [`tile_uv`](Self::tile_uv) but inset by half a texel on every edge,
    /// which stops neighbouring tiles bleeding in under bilinear filtering or
    /// mipmapping. Use this for the actual sampled coordinates.
    pub fn tile_uv_inset(&self, index: u32) -> Option<[f32; 4]> {
        let [u0, v0, u1, v1] = self.tile_uv(index)?;
        let hx = 0.5 / (self.cols * self.tile_px) as f32;
        let hy = 0.5 / (self.rows * self.tile_px) as f32;
        Some([u0 + hx, v0 + hy, u1 - hx, v1 - hy])
    }
}

/// The atlas bundled from the KPC block tiles (`assets/textures/kpc_atlas.png`):
/// 16 px tiles in a 4×4 grid. Layout only — the pixels live in the asset, and
/// the name→index order is recorded in `assets/textures/kpc_atlas.json`.
pub const KPC_ATLAS: AtlasLayout = AtlasLayout::new(16, 4, 4);

// ── Per-block, per-face mapping into the natural-mineral block atlas ──────────
//
// assets/textures/block_atlas.png is your KPC natural-mineral set packed by
// tools/pack_textures.py in sorted-name order. These indices mirror that order;
// re-run the packer and update them together if the tile set changes.

use crate::world::blocks::BlockId;

/// Layout of the bundled natural-mineral block atlas (12 tiles, 4×3 grid).
pub const BLOCK_ATLAS: AtlasLayout = AtlasLayout::new(16, 4, 3);

// Tile indices into BLOCK_ATLAS.
const T_DIAMOND_ORE: u32 = 0;
const T_DIRT: u32 = 1;
const T_LOG_SIDE_OAK: u32 = 2;
const T_LOG_SIDE_BIRCH: u32 = 3;
const T_LOG_TB_OAK: u32 = 5;
const T_LOG_TB_BIRCH: u32 = 6;
const T_SAND: u32 = 7;
const T_STONE_2: u32 = 10; // a second, blockier stone pattern
const T_STONE: u32 = 11;

/// Which atlas tile each face of a block uses. Most blocks are uniform; logs and
/// the like use a different tile on the end caps (top/bottom) vs the sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceTiles {
    pub top: u32,
    pub bottom: u32,
    pub side: u32,
}

impl FaceTiles {
    pub const fn uniform(t: u32) -> Self {
        FaceTiles { top: t, bottom: t, side: t }
    }
    /// A "log" layout: `tb` on top + bottom end caps, `side` around.
    pub const fn column(tb: u32, side: u32) -> Self {
        FaceTiles { top: tb, bottom: tb, side }
    }
    /// Pick the tile for a face given its (axis-aligned) normal.
    pub fn for_normal(&self, normal: [f32; 3]) -> u32 {
        if normal[1] > 0.5 {
            self.top
        } else if normal[1] < -0.5 {
            self.bottom
        } else {
            self.side
        }
    }
}

/// A block's appearance: which atlas tiles, plus a multiplicative `tint`. The
/// tint is what lets many BlockIds share one pattern (e.g. Stone / Andesite /
/// Granite / Slate all sample the same `stone_loose` tile, recoloured) — the
/// "one pattern, many blocks" approach. `[1,1,1]` means the tile is used as-is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockTexture {
    pub faces: FaceTiles,
    pub tint: [f32; 3],
}

impl BlockTexture {
    const fn plain(faces: FaceTiles) -> Self {
        BlockTexture { faces, tint: [1.0, 1.0, 1.0] }
    }
    const fn tinted(faces: FaceTiles, tint: [f32; 3]) -> Self {
        BlockTexture { faces, tint }
    }
}

/// Texture mapping for the blocks the current art set covers. `None` means
/// "no texture authored yet" (fall back to the flat material colour). Shows all
/// three patterns: shared-tile-with-tint (stone family), explicit deviation
/// (Cobblestone uses a different stone tile), and per-face (logs).
pub fn block_texture(block: BlockId) -> Option<BlockTexture> {
    use BlockId::*;
    Some(match block {
        Stone => BlockTexture::plain(FaceTiles::uniform(T_STONE)),
        Andesite => BlockTexture::tinted(FaceTiles::uniform(T_STONE), [0.80, 0.82, 0.86]),
        Granite => BlockTexture::tinted(FaceTiles::uniform(T_STONE), [0.92, 0.74, 0.70]),
        Slate => BlockTexture::tinted(FaceTiles::uniform(T_STONE), [0.62, 0.66, 0.78]),
        Cobblestone => BlockTexture::plain(FaceTiles::uniform(T_STONE_2)),
        Dirt => BlockTexture::plain(FaceTiles::uniform(T_DIRT)),
        Sand => BlockTexture::plain(FaceTiles::uniform(T_SAND)),
        DiamondOre => BlockTexture::plain(FaceTiles::uniform(T_DIAMOND_ORE)),
        OakLog => BlockTexture::plain(FaceTiles::column(T_LOG_TB_OAK, T_LOG_SIDE_OAK)),
        BirchLog => BlockTexture::tinted(
            FaceTiles::column(T_LOG_TB_BIRCH, T_LOG_SIDE_BIRCH),
            [0.95, 0.95, 0.92],
        ),
        // r381: foliage (leaves/bushes) no longer borrows the tinted-dirt tile —
        // leaves read better as their flat material colour + procedural shading
        // (and the dedicated leaf-light path) than as a green-tinted dirt texture.
        // They fall through to `None` (untextured) below.
        _ => return None,
    })
}

// ── GPU-uploadable per-block texture table ───────────────────────────────────
//
// The thin chunk vertex already carries block_id; rather than fatten it with UVs,
// the fragment looks the block up in this table (indexed by block_id), picks the
// face tile by normal, and samples the atlas. std140-friendly: a `uvec4` + `vec4`
// per block (32 bytes), array indexed by `block as u8`.

/// One block's entry for the GPU table. `tiles = [top, bottom, side, textured]`
/// (textured = 1 if this block has art, else 0 → fall back to material colour).
/// `tint = [r, g, b, 0]`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockTexGpu {
    pub tiles: [u32; 4],
    pub tint: [f32; 4],
}

impl BlockTexGpu {
    const UNTEXTURED: BlockTexGpu = BlockTexGpu { tiles: [0, 0, 0, 0], tint: [1.0, 1.0, 1.0, 0.0] };
}

/// Highest `BlockId` discriminant; the table is indexed directly by `block as u8`.
fn max_block_index() -> usize {
    BlockId::ALL.iter().map(|b| *b as usize).max().unwrap_or(0)
}

/// Build the per-block texture table, indexed by `block as u8`. Gaps (ids with no
/// variant, or blocks with no art yet) are `UNTEXTURED`.
pub fn block_texture_table() -> Vec<BlockTexGpu> {
    let mut table = vec![BlockTexGpu::UNTEXTURED; max_block_index() + 1];
    for b in BlockId::ALL {
        if let Some(t) = block_texture(b) {
            table[b as usize] = BlockTexGpu {
                tiles: [t.faces.top, t.faces.bottom, t.faces.side, 1],
                tint: [t.tint[0], t.tint[1], t.tint[2], 0.0],
            };
        }
    }
    table
}

/// The table flattened to bytes for a UBO/SSBO upload (32 bytes per entry).
pub fn block_texture_table_bytes() -> Vec<u8> {
    let table = block_texture_table();
    let mut out = Vec::with_capacity(table.len() * 32);
    for e in &table {
        for v in e.tiles {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in e.tint {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

// =============================================================================
// r336-textures — per-block texture *variation* (deterministic by position) +
// broader block coverage via tint-sharing. Additive; reuses `AtlasLayout`,
// `FaceTiles`, `BlockTexture` above. New *pixels* are an asset step — this ships
// the pure slot/UV/variation math only (see HANDOFF_textures.md).
// =============================================================================

/// A larger atlas reserved for texture *variants* and the broader block set:
/// 16 px tiles in an 8×8 grid (64 slots). The current `BLOCK_ATLAS` (12 tiles)
/// stays the shipped art; this layout documents where variant runs live once the
/// atlas image is expanded (asset step). Variant runs are contiguous tile indices.
pub const VARIANT_ATLAS: AtlasLayout = AtlasLayout::new(16, 8, 8);

/// A contiguous run of variant tiles for one block: `base_tile .. base_tile+count`
/// in some [`AtlasLayout`]. `count == 1` means "no variation" (one fixed tile).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VariantSet {
    pub base_tile: u32,
    pub count: u8,
}

impl VariantSet {
    pub const fn single(base_tile: u32) -> Self {
        VariantSet { base_tile, count: 1 }
    }
    pub const fn run(base_tile: u32, count: u8) -> Self {
        VariantSet { base_tile, count: if count == 0 { 1 } else { count } }
    }
    /// The tile index for a given variant, clamped into the run (so an
    /// out-of-range variant maps to the last tile rather than past the run).
    #[inline]
    pub fn tile(&self, variant: u8) -> u32 {
        let last = self.count.saturating_sub(1);
        self.base_tile + variant.min(last) as u32
    }
}

/// How many texture variants a block has (1 = no variation). Natural,
/// frequently-tiled surfaces get a few variants so large expanses don't show an
/// obvious repeat; manufactured/rare blocks stay single. Pixels for these runs
/// are an asset step — this only declares the intended counts.
pub fn variant_count_for(block: BlockId) -> u8 {
    use BlockId::*;
    match block {
        Stone | Cobblestone => 4,
        Dirt | Gravel | Sand => 3,
        Grass | Moss => 2,
        _ => 1,
    }
}

/// murmur3 fmix32 — a pure integer avalanche mixer (no RNG/IO/clock).
#[inline]
fn fmix32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    h
}

/// Deterministic, well-mixed hash of a block id + world position.
#[inline]
fn hash_block_pos(block: u8, pos: [i32; 3]) -> u32 {
    let mut h = 0x9e37_79b9u32 ^ block as u32;
    for &c in &pos {
        h = fmix32(h ^ (c as u32).wrapping_mul(0x9e37_79b9));
    }
    fmix32(h)
}

/// Pick a block's texture variant deterministically from its world position, in
/// `0..variant_count_for(block)`. Same `(block, pos)` always yields the same
/// variant (stable across frames and re-meshes); the murmur mix spreads variants
/// evenly over space so tiling isn't obvious. Blocks with a single variant
/// always return `0`.
pub fn variant_for(block: BlockId, pos: [i32; 3]) -> u8 {
    let count = variant_count_for(block);
    if count <= 1 {
        return 0;
    }
    (hash_block_pos(block as u8, pos) % count as u32) as u8
}

/// Resolve the (inset) UV rectangle for a given variant of a [`VariantSet`] in
/// `layout`. `None` if the variant's tile is past the atlas.
pub fn variant_uv(layout: &AtlasLayout, set: &VariantSet, variant: u8) -> Option<[f32; 4]> {
    layout.tile_uv_inset(set.tile(variant))
}

/// Convenience: pick the variant for `block` at `pos` and resolve its UV rect in
/// `layout`, given the block's variant run `set`. Pure end-to-end.
pub fn resolve_variant_uv(
    layout: &AtlasLayout,
    block: BlockId,
    set: &VariantSet,
    pos: [i32; 3],
) -> Option<[f32; 4]> {
    variant_uv(layout, set, variant_for(block, pos))
}

/// Broader block coverage with **no new pixels**: more `BlockId`s mapped onto the
/// existing tiles via tint (the "one pattern, many blocks" approach already used
/// for the stone family). Returns `None` for blocks left to the asset step.
/// `block_texture_full` tries the shipped `block_texture` first, then this.
pub fn block_texture_ext(block: BlockId) -> Option<BlockTexture> {
    use BlockId::*;
    Some(match block {
        // Extended stone family — all recolour the loose-stone tile.
        Limestone => BlockTexture::tinted(FaceTiles::uniform(T_STONE), [0.90, 0.90, 0.84]),
        Marble => BlockTexture::tinted(FaceTiles::uniform(T_STONE), [0.95, 0.95, 0.93]),
        Basalt => BlockTexture::tinted(FaceTiles::uniform(T_STONE_2), [0.42, 0.43, 0.47]),
        Tuff => BlockTexture::tinted(FaceTiles::uniform(T_STONE_2), [0.62, 0.61, 0.58]),
        Calcite => BlockTexture::tinted(FaceTiles::uniform(T_STONE), [0.88, 0.90, 0.92]),
        Schist => BlockTexture::tinted(FaceTiles::uniform(T_STONE), [0.50, 0.52, 0.55]),
        Quartz => BlockTexture::tinted(FaceTiles::uniform(T_STONE), [0.97, 0.96, 0.94]),
        Obsidian => BlockTexture::tinted(FaceTiles::uniform(T_STONE_2), [0.18, 0.16, 0.22]),
        Bedrock => BlockTexture::tinted(FaceTiles::uniform(T_STONE_2), [0.30, 0.30, 0.32]),
        StoneBricks => BlockTexture::tinted(FaceTiles::uniform(T_STONE_2), [0.78, 0.78, 0.80]),
        // Sediment — off the sand/dirt tiles.
        Sandstone => BlockTexture::tinted(FaceTiles::uniform(T_SAND), [0.96, 0.92, 0.78]),
        RedSandstone => BlockTexture::tinted(FaceTiles::uniform(T_SAND), [0.86, 0.52, 0.36]),
        Gravel => BlockTexture::tinted(FaceTiles::uniform(T_STONE_2), [0.70, 0.69, 0.67]),
        Clay => BlockTexture::tinted(FaceTiles::uniform(T_DIRT), [0.72, 0.74, 0.78]),
        Moss => BlockTexture::tinted(FaceTiles::uniform(T_DIRT), [0.40, 0.62, 0.30]),
        // Ores — recolour the speckled diamond-ore tile.
        GoldOre => BlockTexture::tinted(FaceTiles::uniform(T_DIAMOND_ORE), [1.00, 0.84, 0.30]),
        IronOre => BlockTexture::tinted(FaceTiles::uniform(T_DIAMOND_ORE), [0.86, 0.72, 0.60]),
        CoalOre => BlockTexture::tinted(FaceTiles::uniform(T_DIAMOND_ORE), [0.30, 0.30, 0.32]),
        CopperOre => BlockTexture::tinted(FaceTiles::uniform(T_DIAMOND_ORE), [0.86, 0.52, 0.34]),
        _ => return None,
    })
}

/// Block texture from the shipped set, falling back to the r336 tinted extension.
pub fn block_texture_full(block: BlockId) -> Option<BlockTexture> {
    block_texture(block).or_else(|| block_texture_ext(block))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_indexed_by_block_id_and_packs_to_32_bytes() {
        let table = block_texture_table();
        assert_eq!(table.len(), max_block_index() + 1);
        let bytes = block_texture_table_bytes();
        assert_eq!(bytes.len(), table.len() * 32);

        // Stone is textured with its tile + white tint.
        let stone = table[BlockId::Stone as usize];
        assert_eq!(stone.tiles[3], 1, "Stone textured");
        assert_eq!(stone.tiles[0], T_STONE);
        assert_eq!(stone.tint, [1.0, 1.0, 1.0, 0.0]);

        // Andesite shares the tile but is tinted.
        let andesite = table[BlockId::Andesite as usize];
        assert_eq!(andesite.tiles[0], stone.tiles[0]);
        assert_ne!(andesite.tint, stone.tint);

        // Air and other unauthored blocks are flagged untextured.
        assert_eq!(table[BlockId::Air as usize].tiles[3], 0);
        assert_eq!(table[BlockId::Water as usize].tiles[3], 0);
    }

    #[test]
    fn logs_carry_distinct_cap_and_side_tiles_in_the_table() {
        let log = block_texture_table()[BlockId::OakLog as usize];
        assert_eq!(log.tiles[3], 1);
        assert_eq!(log.tiles[0], log.tiles[1], "caps match");
        assert_ne!(log.tiles[0], log.tiles[2], "caps differ from bark");
    }

    #[test]
    fn block_texture_covers_the_authored_blocks_and_indices_are_in_range() {
        use BlockId::*;
        for b in [Stone, Andesite, Granite, Slate, Cobblestone, Dirt, Sand, DiamondOre, OakLog, BirchLog] {
            let t = block_texture(b).expect("authored");
            for idx in [t.faces.top, t.faces.bottom, t.faces.side] {
                assert!(idx < BLOCK_ATLAS.len(), "{b:?} tile {idx} >= atlas len");
                assert!(BLOCK_ATLAS.tile_uv(idx).is_some());
            }
        }
        assert_eq!(block_texture(BlockId::Water), None, "no art yet → None");
    }

    #[test]
    fn stone_family_shares_one_pattern_via_tint() {
        let stone = block_texture(BlockId::Stone).unwrap();
        let andesite = block_texture(BlockId::Andesite).unwrap();
        let granite = block_texture(BlockId::Granite).unwrap();
        // Same tile...
        assert_eq!(stone.faces, andesite.faces);
        assert_eq!(stone.faces, granite.faces);
        // ...recoloured.
        assert_eq!(stone.tint, [1.0, 1.0, 1.0]);
        assert_ne!(andesite.tint, stone.tint);
        assert_ne!(granite.tint, andesite.tint);
        // Cobblestone is an explicit deviation (different tile).
        assert_ne!(block_texture(BlockId::Cobblestone).unwrap().faces, stone.faces);
    }

    #[test]
    fn logs_are_per_face() {
        let log = block_texture(BlockId::OakLog).unwrap().faces;
        assert_eq!(log.top, log.bottom, "end caps match");
        assert_ne!(log.top, log.side, "caps differ from bark");
        // Normal-driven face pick.
        assert_eq!(log.for_normal([0.0, 1.0, 0.0]), log.top);
        assert_eq!(log.for_normal([0.0, -1.0, 0.0]), log.bottom);
        assert_eq!(log.for_normal([1.0, 0.0, 0.0]), log.side);
    }

    #[test]
    fn tile_uv_maps_grid_cells() {
        let a = AtlasLayout::new(16, 4, 4);
        assert_eq!(a.len(), 16);
        assert_eq!(a.tile_uv(0), Some([0.0, 0.0, 0.25, 0.25]));
        assert_eq!(a.cell(5), (1, 1));
        assert_eq!(a.tile_uv(5), Some([0.25, 0.25, 0.5, 0.5]));
        assert_eq!(a.tile_uv(15), Some([0.75, 0.75, 1.0, 1.0]));
        assert_eq!(a.tile_uv(16), None, "out of range");
    }

    #[test]
    fn every_uv_is_inside_the_atlas() {
        let a = KPC_ATLAS;
        for i in 0..a.len() {
            let uv = a.tile_uv(i).unwrap();
            for c in &uv {
                assert!((0.0..=1.0).contains(c), "uv {c} out of 0..1");
            }
            assert!(uv[0] < uv[2] && uv[1] < uv[3], "rect is non-degenerate");
        }
    }

    #[test]
    fn inset_shrinks_within_the_plain_rect() {
        let a = KPC_ATLAS;
        let [u0, v0, u1, v1] = a.tile_uv(6).unwrap();
        let [iu0, iv0, iu1, iv1] = a.tile_uv_inset(6).unwrap();
        assert!(iu0 > u0 && iv0 > v0 && iu1 < u1 && iv1 < v1, "inset is strictly inside");
        // ...but only by a fraction of a tile.
        assert!((iu0 - u0) < (u1 - u0) * 0.25);
    }
}

#[cfg(test)]
mod r336_textures_tests {
    use super::*;
    use crate::world::blocks::BlockId;

    // every slot in the variant atlas has in-bounds UVs
    #[test]
    fn variant_atlas_slots_all_in_bounds() {
        let l = VARIANT_ATLAS;
        for i in 0..l.len() {
            let uv = l.tile_uv(i).expect("slot in range");
            for c in uv {
                assert!((0.0..=1.0).contains(&c), "uv {c} out of [0,1] at slot {i}");
            }
            assert!(uv[0] < uv[2] && uv[1] < uv[3], "degenerate rect at {i}");
        }
        assert!(l.tile_uv(l.len()).is_none(), "past-last slot must be None");
    }

    // no two distinct slots' UV rects overlap (grid is a partition)
    #[test]
    fn no_two_slots_overlap() {
        let l = VARIANT_ATLAS;
        let n = l.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let a = l.tile_uv(i).unwrap();
                let b = l.tile_uv(j).unwrap();
                // disjoint iff separated on u or on v
                let disjoint = a[2] <= b[0] || b[2] <= a[0] || a[3] <= b[1] || b[3] <= a[1];
                assert!(disjoint, "slots {i},{j} overlap: {a:?} vs {b:?}");
            }
        }
    }

    // slot index -> UV -> slot index round-trips
    #[test]
    fn slot_uv_round_trip() {
        let l = VARIANT_ATLAS;
        for i in 0..l.len() {
            let uv = l.tile_uv(i).unwrap();
            let col = (uv[0] * l.cols as f32).round() as u32;
            let row = (uv[1] * l.rows as f32).round() as u32;
            assert_eq!(row * l.cols + col, i, "round-trip failed at {i}");
        }
    }

    // variant_for is in [0, count) and respects single-variant blocks
    #[test]
    fn variant_in_range_and_single_is_zero() {
        for b in BlockId::ALL {
            let count = variant_count_for(b);
            for p in [[0, 0, 0], [5, 64, -9], [1000, -7, 42], [-3, -3, -3]] {
                let v = variant_for(b, p);
                assert!((v as u8) < count.max(1), "{b:?} variant {v} >= count {count}");
                if count <= 1 {
                    assert_eq!(v, 0, "single-variant block must be 0");
                }
            }
        }
    }

    // deterministic: same (block,pos) -> same variant
    #[test]
    fn variant_deterministic() {
        let b = BlockId::Stone;
        let p = [12, 7, -33];
        let v = variant_for(b, p);
        for _ in 0..50 {
            assert_eq!(variant_for(b, p), v);
        }
    }

    // variant pick is roughly uniform over a grid of positions
    #[test]
    fn variant_distribution_is_uniformish() {
        let b = BlockId::Stone; // count 4
        let count = variant_count_for(b) as usize;
        let mut hist = vec![0usize; count];
        let mut n = 0usize;
        for x in 0..32i32 {
            for z in 0..32i32 {
                hist[variant_for(b, [x, 7, z]) as usize] += 1;
                n += 1;
            }
        }
        let expected = n as f64 / count as f64;
        for (i, &h) in hist.iter().enumerate() {
            let dev = (h as f64 - expected).abs() / expected;
            assert!(dev < 0.35, "variant {i} skewed: {h} vs ~{expected:.0} ({:.0}%)", dev * 100.0);
        }
    }

    // VariantSet::tile clamps an out-of-range variant into the run
    #[test]
    fn variant_set_tile_clamps() {
        let s = VariantSet::run(20, 3); // tiles 20,21,22
        assert_eq!(s.tile(0), 20);
        assert_eq!(s.tile(2), 22);
        assert_eq!(s.tile(7), 22, "past-run variant clamps to last tile");
        assert_eq!(VariantSet::run(5, 0).count, 1, "zero count floored to 1");
    }

    // resolve_variant_uv yields an in-bounds rect for a real run
    #[test]
    fn resolve_variant_uv_in_bounds() {
        let set = VariantSet::run(0, variant_count_for(BlockId::Stone)); // 4 tiles from 0
        for p in [[0, 0, 0], [3, 9, 12], [-5, 2, 7]] {
            let uv = resolve_variant_uv(&VARIANT_ATLAS, BlockId::Stone, &set, p).expect("in atlas");
            assert!(uv.iter().all(|c| (0.0..=1.0).contains(c)));
            assert!(uv[0] < uv[2] && uv[1] < uv[3]);
        }
    }

    // tinted coverage extension: more blocks now textured, originals unchanged
    #[test]
    fn ext_coverage_adds_blocks_without_changing_originals() {
        // originals still resolve identically through _full
        for b in [BlockId::Stone, BlockId::Dirt, BlockId::OakLog] {
            assert_eq!(block_texture_full(b), block_texture(b));
        }
        // newly covered blocks were previously None, now Some via _full
        for b in [BlockId::Basalt, BlockId::Gravel, BlockId::GoldOre, BlockId::Sandstone, BlockId::Moss] {
            assert!(block_texture(b).is_none(), "{b:?} expected uncovered in base");
            let t = block_texture_full(b).expect("ext should cover");
            // ext reuses real atlas tiles → tile indices are within BLOCK_ATLAS
            assert!(t.faces.top < BLOCK_ATLAS.len(), "ext tile in atlas");
            assert!(t.tint.iter().all(|c| c.is_finite()));
        }
    }

    #[test]
    fn ext_tiles_all_within_block_atlas() {
        for b in BlockId::ALL {
            if let Some(t) = block_texture_ext(b) {
                for tile in [t.faces.top, t.faces.bottom, t.faces.side] {
                    assert!(tile < BLOCK_ATLAS.len(), "{b:?} ext tile {tile} past atlas");
                }
            }
        }
    }
}
