//! Remesh policy — decide whether a block edit actually changes what's drawn at
//! a chunk's current LOD, so the engine can *skip* the re-mesh when it wouldn't.
//!
//! ## Why this exists
//!
//! Plant lifecycle grows grass / flowers / ferns across thousands of loaded
//! chunks every tick. At far LOD those are sub-pixel — they aren't part of the
//! downsampled surface at all — yet a re-mesh fires anyway. The far GPU mesher
//! costs ~7ms per chunk (a near-fixed per-dispatch cost in the run logs), so
//! firing it ~150×/sec for invisible edits pins the mesher and starves the
//! LOD-upgrade meshes that would actually resolve the view. The world then
//! "never settles" even after generation finishes.
//!
//! The edit still *applies* to the chunk's block data — so when the chunk later
//! resolves to Near, the accumulated growth shows correctly. Only the pointless
//! far/mid re-mesh is skipped.
//!
//! ## Conservatism
//!
//! This only ever skips a remesh for *micro-detail* growth (small non-solid
//! plants) at a LOD too coarse to draw it. Any solid/terrain/foliage edit — or
//! a micro-detail edit at Near — always remeshes. So a structural change can
//! never be hidden; the worst case of a bug here is a missed grass blade at a
//! distance, not a missing wall.

use super::blocks::BlockId;
use super::lod::LodLevel;

/// Small decorative plants that occupy a fraction of a voxel and disappear below
/// Near LOD (they're not represented in the downsampled surface). These are the
/// blocks plant lifecycle churns most.
pub fn is_micro_detail(b: BlockId) -> bool {
    matches!(
        b,
        BlockId::Flower
            | BlockId::TallGrass
            | BlockId::Fern
            | BlockId::DeadBush
            | BlockId::Vine
            | BlockId::Cobweb
            | BlockId::Mushroom
            | BlockId::Lichen
    )
}

/// The coarsest LOD at which micro-detail is still drawn — only Near. (Mid and
/// beyond downsample past the point where a single grass blade survives.)
pub fn micro_detail_visible_at(lod: LodLevel) -> bool {
    lod == LodLevel::Near
}

/// Does placing `new_block` in a chunk currently meshed at `chunk_lod` change
/// what's visible? Conservative: returns `false` (skip the remesh) only for
/// micro-detail growth at a LOD too coarse to show it; everything else remeshes.
pub fn edit_requires_remesh(new_block: BlockId, chunk_lod: LodLevel) -> bool {
    if is_micro_detail(new_block) {
        micro_detail_visible_at(chunk_lod)
    } else {
        true
    }
}

/// Whole-chunk decision: remesh if ANY edited block requires it at this LOD.
/// (One structural edit in a batch of grass growth still forces the remesh.)
pub fn chunk_requires_remesh(edited_new_blocks: &[BlockId], chunk_lod: LodLevel) -> bool {
    edited_new_blocks
        .iter()
        .any(|&b| edit_requires_remesh(b, chunk_lod))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SAFETY INVARIANT (over the whole palette): the gate may only ever skip a
    /// remesh for NON-SOLID blocks. This is what guarantees the module's promise
    /// — "worst case is a missed grass blade, not a missing wall." If anyone adds
    /// a solid block to `is_micro_detail` (or flips one of these to solid in
    /// `is_solid`), this fails. Equivalently: every solid block remeshes at every
    /// LOD, so a structural edit can never be hidden.
    #[test]
    fn skip_set_is_a_subset_of_non_solid_blocks() {
        for b in BlockId::ALL {
            if is_micro_detail(b) {
                assert!(!b.is_solid(), "{b:?} is gate-skippable but solid — a structural edit could be hidden");
            }
            if b.is_solid() {
                for lod in [LodLevel::Near, LodLevel::Mid, LodLevel::Far, LodLevel::VeryFar] {
                    assert!(edit_requires_remesh(b, lod), "solid {b:?} must always remesh ({lod:?})");
                }
            }
        }
    }

    #[test]
    fn micro_detail_skips_remesh_beyond_near() {
        // Grass growing in a far chunk → invisible → skip.
        assert!(!edit_requires_remesh(BlockId::TallGrass, LodLevel::Far));
        assert!(!edit_requires_remesh(BlockId::TallGrass, LodLevel::Mid));
        assert!(!edit_requires_remesh(BlockId::TallGrass, LodLevel::VeryFar));
        // Same grass in a near chunk → visible → remesh.
        assert!(edit_requires_remesh(BlockId::TallGrass, LodLevel::Near));
        // The whole micro-detail set behaves the same.
        for b in [
            BlockId::Flower, BlockId::Fern, BlockId::DeadBush, BlockId::Vine,
            BlockId::Cobweb, BlockId::Mushroom, BlockId::Lichen,
        ] {
            assert!(is_micro_detail(b), "{b:?} should be micro-detail");
            assert!(!edit_requires_remesh(b, LodLevel::Far), "{b:?} skips at far");
            assert!(edit_requires_remesh(b, LodLevel::Near), "{b:?} remeshes at near");
        }
    }

    #[test]
    fn structural_edits_always_remesh() {
        // Terrain / building blocks are visible at every LOD → never skipped.
        for b in [BlockId::Stone, BlockId::Dirt, BlockId::OakLog, BlockId::Sand] {
            for lod in [LodLevel::Near, LodLevel::Mid, LodLevel::Far, LodLevel::VeryFar] {
                assert!(edit_requires_remesh(b, lod), "{b:?} must remesh at {lod:?}");
            }
            assert!(!is_micro_detail(b), "{b:?} is not micro-detail");
        }
    }

    #[test]
    fn foliage_volume_is_not_gated() {
        // Leaves form the canopy "blob" visible at distance, so they are NOT
        // micro-detail and must keep remeshing at far LOD.
        assert!(!is_micro_detail(BlockId::OakLeaves));
        assert!(!is_micro_detail(BlockId::BirchLeaves));
        assert!(edit_requires_remesh(BlockId::OakLeaves, LodLevel::Far));
    }

    #[test]
    fn one_structural_edit_forces_a_whole_chunk_remesh() {
        // A far chunk whose tick edits are all grass → skip.
        let grassy = [BlockId::TallGrass, BlockId::Flower, BlockId::Fern];
        assert!(!chunk_requires_remesh(&grassy, LodLevel::Far));
        // ...but add one stone placement and the chunk must remesh.
        let mixed = [BlockId::TallGrass, BlockId::Stone, BlockId::Flower];
        assert!(chunk_requires_remesh(&mixed, LodLevel::Far));
        // Empty edit set → nothing to do.
        assert!(!chunk_requires_remesh(&[], LodLevel::Far));
        // At Near everything remeshes.
        assert!(chunk_requires_remesh(&grassy, LodLevel::Near));
    }
}
