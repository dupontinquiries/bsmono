//! Crystal blades (`cave_biomes.docx`, Crystal Growth Chambers, Survival
//! Hazards): "Crystal blade tips are sharp. Moving carelessly through dense
//! crystal fields causes minor laceration damage (1 HP per fast transit
//! through dense zones)."
//!
//! Pure. The game counts the crystal blocks around the player's body and
//! passes its speed; [`Blades::tick`] says when to cut. One transit is one
//! visit to a dense field: moving fast in it cuts once, and walking through
//! carefully does not cut at all. Leaving the field ends the transit.
use crate::world::blocks::BlockId;

/// Blocks that grow as blades.
pub fn is_blade(b: BlockId) -> bool {
    matches!(b, BlockId::Crystal | BlockId::Quartz | BlockId::Selenite | BlockId::GlowCrystal)
}

/// Cells the game samples around the body: 3 x 2 x 3 (feet and chest level).
pub const SAMPLED_CELLS: usize = 18;

/// A field is dense from this many blade cells around the body.
pub const DENSE_CELLS: usize = 4;

/// "Fast": above this horizontal speed (blocks/s). A walk (4.5) is careful
/// enough; a sprint (4.5 x 1.6 = 7.2) is not.
pub const FAST_SPEED: f32 = crate::physics::constants::WALK_SPEED * 1.2;

/// Vitality (0..1) one cut takes: the docx's 1 HP, on the scale where a
/// predator's bite takes 0.04.
pub const CUT: f32 = 0.02;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Blades {
    cut_this_transit: bool,
}

impl Blades {
    /// `blade_cells` crystal cells of the [`SAMPLED_CELLS`] around the body,
    /// moving at `speed`. Returns the vitality to take this tick, if any.
    pub fn tick(&mut self, blade_cells: usize, speed: f32) -> Option<f32> {
        if blade_cells < DENSE_CELLS {
            self.cut_this_transit = false;
            return None;
        }
        if speed > FAST_SPEED && !self.cut_this_transit {
            self.cut_this_transit = true;
            return Some(CUT);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_crystal_blocks_are_blades() {
        for b in [BlockId::Crystal, BlockId::Quartz, BlockId::Selenite, BlockId::GlowCrystal] {
            assert!(is_blade(b), "{b:?}");
        }
        for b in [BlockId::Stone, BlockId::Air, BlockId::Amber, BlockId::Obsidian, BlockId::Glowshroom] {
            assert!(!is_blade(b), "{b:?}");
        }
    }

    #[test]
    fn a_fast_transit_cuts_once_and_a_careful_one_never() {
        let mut b = Blades::default();
        // Run through: several ticks inside a dense field at speed.
        let cuts: Vec<_> = (0..20).filter_map(|_| b.tick(6, 7.2)).collect();
        assert_eq!(cuts, vec![CUT], "one cut per transit");
        // Out, and back in fast: a new transit, a new cut.
        assert_eq!(b.tick(0, 7.2), None);
        assert_eq!(b.tick(6, 7.2), Some(CUT));
        // A walk through a fresh field never cuts, even at full walking pace.
        let mut w = Blades::default();
        assert!((0..50).all(|_| w.tick(8, crate::physics::constants::WALK_SPEED).is_none()));
        // Breaking into a sprint halfway through that same field does.
        assert_eq!(w.tick(8, 7.2), Some(CUT));
    }

    #[test]
    fn a_sparse_field_is_safe_at_any_speed() {
        let mut b = Blades::default();
        assert!((0..50).all(|_| b.tick(DENSE_CELLS - 1, 9.0).is_none()));
        assert_eq!(b.tick(DENSE_CELLS, 9.0), Some(CUT), "dense from DENSE_CELLS");
    }
}
