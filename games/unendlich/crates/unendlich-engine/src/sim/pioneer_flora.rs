//! `sim::pioneer_flora` (r455) — climate-driven "pioneer" ground decorations.
//!
//! Adds plant VARIETY at runtime using blocks that already render, no worldgen/shader change: given a
//! bare cell's local climate (temperature °C, moisture 0..1) and the block it sits on, pick an
//! appropriate decoration to colonise it — dead bushes on arid sand, lichen on cold exposed rock,
//! moss on damp rock, ferns in warm humid undergrowth — or `None` where nothing fits. Pure +
//! testable; the caller owns finding bare cells + placing the block.
//!
//! All outputs are non-solid decoration [`BlockId`]s the mesher already colours (DeadBush, Lichen,
//! Moss, Fern), so this is safe to drive from a runtime tick (recoverable, no new block/atlas/shader).

use crate::world::blocks::BlockId;

/// The decoration that should colonise a bare cell on `ground` under the local climate, or `None`.
pub fn pioneer_for(temp_c: f32, moisture01: f32, ground: BlockId) -> Option<BlockId> {
    use BlockId::*;
    Some(match ground {
        // arid dunes: dead bushes
        Sand if temp_c > 22.0 && moisture01 < 0.35 => DeadBush,
        // cold exposed rock: lichen
        Stone | Andesite | Granite | Slate | Basalt | Tuff if temp_c < 8.0 => Lichen,
        // damp rock: moss
        Stone | Cobblestone | Andesite if moisture01 > 0.6 => Moss,
        // warm humid undergrowth: ferns
        Grass | Dirt if temp_c > 18.0 && moisture01 > 0.7 => Fern,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arid_sand_gets_deadbush() {
        assert_eq!(pioneer_for(30.0, 0.1, BlockId::Sand), Some(BlockId::DeadBush));
        assert_eq!(pioneer_for(30.0, 0.9, BlockId::Sand), None);
        assert_eq!(pioneer_for(10.0, 0.1, BlockId::Sand), None);
    }

    #[test]
    fn cold_rock_gets_lichen() {
        assert_eq!(pioneer_for(2.0, 0.4, BlockId::Stone), Some(BlockId::Lichen));
        assert_eq!(pioneer_for(2.0, 0.4, BlockId::Granite), Some(BlockId::Lichen));
    }

    #[test]
    fn damp_warm_rock_gets_moss() {
        assert_eq!(pioneer_for(20.0, 0.8, BlockId::Stone), Some(BlockId::Moss));
    }

    #[test]
    fn warm_humid_dirt_gets_fern() {
        assert_eq!(pioneer_for(22.0, 0.8, BlockId::Dirt), Some(BlockId::Fern));
    }

    #[test]
    fn temperate_dry_gives_nothing() {
        assert_eq!(pioneer_for(15.0, 0.4, BlockId::Grass), None);
        assert_eq!(pioneer_for(15.0, 0.4, BlockId::Dirt), None);
    }
}
