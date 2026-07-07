//! `block_predicates` — narrow block predicates, centered on `is_opaque`.
//!
//! Worker-03 / WORLD pure core, round 7 (flagship). The overloaded `is_solid`
//! conflates three independent axes: does a block occlude light, is it solid to
//! bodies, and does it produce a mesh face. Light-shaft culling and leaf
//! transparency need `is_opaque` specifically — a leaf is collision-solid but
//! NOT opaque, so it must neither seal faces against other leaves nor block
//! light. This is just the pure predicate set; the mesher/light migration is a
//! later integration step (no call-site changes here).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// A small self-contained sample of block kinds (the engine maps its `BlockId`
/// onto these categories when wiring).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Air,
    Stone,
    Dirt,
    Leaf,
    Glass,
    Water,
    Wood,
}

/// Occludes light / fully hides what's behind it.
pub fn is_opaque(b: BlockKind) -> bool {
    matches!(b, BlockKind::Stone | BlockKind::Dirt | BlockKind::Wood)
}

/// Solid to bodies (collision). Independent of opacity: a leaf collides but is
/// not opaque; water is opaque-false AND collision-false.
pub fn blocks_collision(b: BlockKind) -> bool {
    matches!(
        b,
        BlockKind::Stone | BlockKind::Dirt | BlockKind::Wood | BlockKind::Leaf | BlockKind::Glass
    )
}

/// Whether this kind produces any mesh geometry at all (Air does not).
pub fn is_meshable_volume(b: BlockKind) -> bool {
    !matches!(b, BlockKind::Air)
}

/// Should `self_` draw a face on the boundary toward `neighbour`?
///
/// Asymmetric: an opaque block draws against a transparent neighbour, but a
/// transparent block's face toward an opaque neighbour is sealed (hidden behind
/// the opaque block). Two identical transparent blocks (leaf↔leaf, water↔water,
/// glass↔glass) do NOT draw an internal face between them (no double-draw).
pub fn produces_face_against(self_: BlockKind, neighbour: BlockKind) -> bool {
    // Air (and anything non-meshable) draws nothing.
    if !is_meshable_volume(self_) {
        return false;
    }
    // An opaque neighbour fully occludes this face.
    if is_opaque(neighbour) {
        return false;
    }
    // Neighbour is transparent: draw, except don't double-draw between two
    // identical transparent blocks.
    if self_ == neighbour && !is_opaque(self_) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [BlockKind; 7] = [
        BlockKind::Air,
        BlockKind::Stone,
        BlockKind::Dirt,
        BlockKind::Leaf,
        BlockKind::Glass,
        BlockKind::Water,
        BlockKind::Wood,
    ];

    #[test]
    fn leaf_is_solid_but_not_opaque() {
        assert!(blocks_collision(BlockKind::Leaf));
        assert!(!is_opaque(BlockKind::Leaf));
        assert!(is_meshable_volume(BlockKind::Leaf));
    }

    #[test]
    fn air_is_all_false() {
        assert!(!is_opaque(BlockKind::Air));
        assert!(!blocks_collision(BlockKind::Air));
        assert!(!is_meshable_volume(BlockKind::Air));
    }

    #[test]
    fn glass_non_opaque_but_meshable_and_solid() {
        assert!(!is_opaque(BlockKind::Glass));
        assert!(is_meshable_volume(BlockKind::Glass));
        assert!(blocks_collision(BlockKind::Glass));
    }

    #[test]
    fn water_not_collision_not_opaque_but_meshable() {
        assert!(!blocks_collision(BlockKind::Water));
        assert!(!is_opaque(BlockKind::Water));
        assert!(is_meshable_volume(BlockKind::Water));
    }

    #[test]
    fn face_asymmetry_opaque_vs_transparent() {
        // Opaque stone draws against transparent air / glass / water...
        assert!(produces_face_against(BlockKind::Stone, BlockKind::Air));
        assert!(produces_face_against(BlockKind::Stone, BlockKind::Glass));
        assert!(produces_face_against(BlockKind::Stone, BlockKind::Water));
        // ...but the transparent block's face toward opaque stone is sealed.
        assert!(!produces_face_against(BlockKind::Glass, BlockKind::Stone));
        assert!(!produces_face_against(BlockKind::Water, BlockKind::Stone));
        // Air never draws.
        assert!(!produces_face_against(BlockKind::Air, BlockKind::Stone));
    }

    #[test]
    fn no_self_face_between_identical_transparent_blocks() {
        assert!(!produces_face_against(BlockKind::Leaf, BlockKind::Leaf));
        assert!(!produces_face_against(BlockKind::Water, BlockKind::Water));
        assert!(!produces_face_against(BlockKind::Glass, BlockKind::Glass));
        // ...but two opaque of the same kind DO seal (also no face), and a
        // transparent block against a *different* transparent neighbour draws:
        assert!(produces_face_against(BlockKind::Leaf, BlockKind::Air));
        assert!(produces_face_against(BlockKind::Leaf, BlockKind::Glass));
    }

    #[test]
    fn leaf_against_opaque_is_sealed_against_air_draws() {
        assert!(!produces_face_against(BlockKind::Leaf, BlockKind::Stone)); // hidden behind stone
        assert!(produces_face_against(BlockKind::Leaf, BlockKind::Air)); // leaf surface to air
    }

    #[test]
    fn opacity_and_collision_are_independent_axes() {
        // Knowing is_opaque does not determine blocks_collision: a leaf is
        // collision-true / opaque-false, and water is collision-false /
        // opaque-false — same opacity, different collision.
        assert_ne!(is_opaque(BlockKind::Leaf), blocks_collision(BlockKind::Leaf));
        assert_eq!(is_opaque(BlockKind::Leaf), is_opaque(BlockKind::Water));
        assert_ne!(blocks_collision(BlockKind::Leaf), blocks_collision(BlockKind::Water));
    }

    #[test]
    fn every_kind_deterministic() {
        for &b in &ALL {
            assert_eq!(is_opaque(b), is_opaque(b));
            assert_eq!(blocks_collision(b), blocks_collision(b));
            assert_eq!(is_meshable_volume(b), is_meshable_volume(b));
            for &n in &ALL {
                assert_eq!(produces_face_against(b, n), produces_face_against(b, n));
            }
        }
    }
}
