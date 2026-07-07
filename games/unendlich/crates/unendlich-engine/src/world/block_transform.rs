//! Environmental block-*type* transformation — blocks that turn into other
//! blocks when water acts on them, the way reality wears ground down.
//!
//! Distinct from `weathering`/`erosion`, which track a *visual* intensity (how
//! bumpy a block looks). This changes the block's identity:
//!   * **Drowning** (instant): grass sealed under water suffocates into dirt.
//!   * **Erosion** (gradual): flowing water beside loose soil dissociates it a
//!     grade at a time — grass strips to dirt, dirt loosens to gravel, gravel
//!     washes to sand. Sand is terminal; stone and the like don't dissociate.
//!
//! Erosion is staged: each block carries a `wear` accumulator. Flowing water
//! adds wear each step; once wear crosses the block's resistance it transforms
//! to its product and wear resets. Without flow, wear recovers, so a brief
//! splash leaves no permanent scar. Pure + deterministic.

use crate::world::blocks::BlockId;

/// Wear added per step by adjacent flowing water.
pub const FLOW_WEAR: u16 = 1;
/// Wear recovered per step when no flowing water touches the block.
pub const DRY_RECOVER: u16 = 1;

/// What water is doing to a block this step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Conditions {
    /// A water block sits on top of / encloses this block (it's submerged).
    pub water_logged: bool,
    /// Flowing (not still) water is adjacent — the eroding agent.
    pub flowing: bool,
}

/// The block a given block dissociates *into*, one grade at a time. `None` =
/// inert, or already terminal (sand).
pub fn erosion_product(block: BlockId) -> Option<BlockId> {
    match block {
        BlockId::Grass => Some(BlockId::Dirt),   // stripped to bare soil
        BlockId::Dirt => Some(BlockId::Gravel),  // loosened
        BlockId::Gravel => Some(BlockId::Sand),  // washed fine
        _ => None,
    }
}

/// How much wear a block tolerates before it gives way. Only meaningful for
/// blocks that have an erosion product.
pub fn erosion_resistance(block: BlockId) -> u16 {
    match block {
        BlockId::Grass => 2,
        BlockId::Dirt => 3,
        BlockId::Gravel => 6,
        _ => u16::MAX,
    }
}

/// Result of one transformation step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transformed {
    pub block: BlockId,
    pub wear: u16,
    /// True if the block id changed this step.
    pub changed: bool,
}

/// Advance one block one step under the given water conditions, carrying its
/// accumulated `wear`. Drowning is instant; erosion accrues.
pub fn transform(block: BlockId, cond: Conditions, wear: u16) -> Transformed {
    // Grass sealed under water suffocates immediately into dirt.
    if block == BlockId::Grass && cond.water_logged {
        return Transformed { block: BlockId::Dirt, wear: 0, changed: true };
    }

    match erosion_product(block) {
        Some(product) if cond.flowing => {
            let next = wear.saturating_add(FLOW_WEAR);
            if next >= erosion_resistance(block) {
                Transformed { block: product, wear: 0, changed: true }
            } else {
                Transformed { block, wear: next, changed: false }
            }
        }
        // No flow (or inert block): wear recovers, block unchanged.
        _ => Transformed { block, wear: wear.saturating_sub(DRY_RECOVER), changed: false },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flowing() -> Conditions { Conditions { water_logged: false, flowing: true } }

    #[test]
    fn grass_drowns_into_dirt_when_submerged() {
        let out = transform(BlockId::Grass, Conditions { water_logged: true, flowing: false }, 0);
        assert_eq!(out.block, BlockId::Dirt);
        assert!(out.changed);
    }

    #[test]
    fn dry_grass_is_left_alone() {
        let out = transform(BlockId::Grass, Conditions::default(), 0);
        assert_eq!(out.block, BlockId::Grass);
        assert!(!out.changed);
    }

    #[test]
    fn flowing_water_dissociates_dirt_to_gravel_then_sand() {
        let mut b = BlockId::Dirt;
        let mut w = 0;
        for _ in 0..10 {
            let out = transform(b, flowing(), w);
            b = out.block; w = out.wear;
            if out.changed { break; }
        }
        assert_eq!(b, BlockId::Gravel, "dirt should loosen to gravel under flow");

        for _ in 0..10 {
            let out = transform(b, flowing(), w);
            b = out.block; w = out.wear;
            if out.changed { break; }
        }
        assert_eq!(b, BlockId::Sand, "gravel should wash to sand");
    }

    #[test]
    fn sand_is_terminal() {
        let out = transform(BlockId::Sand, flowing(), 100);
        assert_eq!(out.block, BlockId::Sand);
        assert!(!out.changed);
        assert!(erosion_product(BlockId::Sand).is_none());
    }

    #[test]
    fn stone_does_not_dissociate() {
        let out = transform(BlockId::Stone, flowing(), 5);
        assert_eq!(out.block, BlockId::Stone);
        assert!(!out.changed);
    }

    #[test]
    fn wear_accrues_below_threshold_then_recovers_when_flow_stops() {
        let step1 = transform(BlockId::Dirt, flowing(), 0);
        assert!(!step1.changed);
        assert_eq!(step1.wear, FLOW_WEAR);
        let step2 = transform(BlockId::Dirt, Conditions::default(), step1.wear);
        assert!(step2.wear < step1.wear);
        assert!(!step2.changed);
    }
}
