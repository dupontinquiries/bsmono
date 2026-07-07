//! `mineral_hardness` — mine time from block hardness × tool tier.
//!
//! Worker-03 / WORLD-PERF pure core, round 10. Given a block id and a tool tier,
//! returns how long the block takes to mine, or `None` if the tool is too weak
//! to harvest it at all. Harder blocks take longer; higher tiers mine faster.
//! Pairs with `ore_distribution` (deep ores tend to need better tools).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

// Sample block ids.
pub const DIRT: u8 = 0;
pub const STONE: u8 = 1;
pub const IRON_ORE: u8 = 2;
pub const DIAMOND_ORE: u8 = 3;
pub const OBSIDIAN: u8 = 4;

/// Base hardness of a block (arbitrary unknowns default to a mid value).
pub fn block_hardness(block: u8) -> f32 {
    match block {
        DIRT => 0.5,
        STONE => 1.5,
        IRON_ORE => 3.0,
        DIAMOND_ORE => 3.0,
        OBSIDIAN => 50.0,
        _ => 1.0,
    }
}

/// Minimum tool tier required to harvest a block at all.
pub fn required_tier(block: u8) -> u8 {
    match block {
        DIRT => 0,
        STONE => 0,
        IRON_ORE => 1,
        DIAMOND_ORE => 2,
        OBSIDIAN => 3,
        _ => 0,
    }
}

/// Time to mine `block` with a tool of `tool_tier`. `None` if the tool is below
/// the block's required tier. Otherwise `hardness / (tier + 1)` — strictly
/// positive, longer for harder blocks, shorter for higher tiers.
pub fn mine_time(block: u8, tool_tier: u8) -> Option<f32> {
    if tool_tier < required_tier(block) {
        return None;
    }
    let speed = tool_tier as f32 + 1.0;
    Some(block_hardness(block) / speed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harder_block_takes_longer() {
        // Same tier, compare hardness (both harvestable at tier 3).
        let stone = mine_time(STONE, 3).unwrap();
        let obsidian = mine_time(OBSIDIAN, 3).unwrap();
        assert!(obsidian > stone);
    }

    #[test]
    fn higher_tier_mines_faster() {
        let slow = mine_time(STONE, 0).unwrap();
        let fast = mine_time(STONE, 5).unwrap();
        assert!(fast < slow);
    }

    #[test]
    fn too_weak_tool_is_none() {
        assert!(mine_time(DIAMOND_ORE, 0).is_none());
        assert!(mine_time(DIAMOND_ORE, 1).is_none());
        assert!(mine_time(OBSIDIAN, 2).is_none());
        assert!(mine_time(OBSIDIAN, 3).is_some());
    }

    #[test]
    fn time_positive_when_minable() {
        for block in [DIRT, STONE, IRON_ORE, DIAMOND_ORE, OBSIDIAN, 200] {
            for tier in 0..8u8 {
                if let Some(t) = mine_time(block, tier) {
                    assert!(t > 0.0, "non-positive mine time");
                }
            }
        }
    }

    #[test]
    fn monotone_decreasing_in_tier() {
        // For a fixed block, more tier never increases time (over valid tiers).
        let block = IRON_ORE;
        let req = required_tier(block);
        let mut prev = f32::INFINITY;
        for tier in req..12u8 {
            let t = mine_time(block, tier).unwrap();
            assert!(t <= prev + 1e-6, "time rose with tier");
            prev = t;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(mine_time(OBSIDIAN, 4), mine_time(OBSIDIAN, 4));
        assert_eq!(mine_time(DIRT, 0), mine_time(DIRT, 0));
    }

    #[test]
    fn unknown_block_is_minable_with_default_hardness() {
        let t = mine_time(222, 0).unwrap();
        assert!((t - 1.0).abs() < 1e-6); // default hardness 1.0 / (0+1)
    }
}
