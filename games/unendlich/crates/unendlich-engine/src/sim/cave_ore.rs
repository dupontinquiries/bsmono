//! `sim::cave_ore` — depth-weighted ore selection for the runtime cave-enrichment
//! pass (r516). The world generator runs on the GPU on this target, so adding ore
//! *there* is invisible without a shader change; instead a CPU pass seeds ore into
//! exposed cave stone near the player, and this module decides *which* ore.
//!
//! Pure + deterministic: given a depth below sea level and a uniform roll, return an
//! ore block (or `None`, the common case — ore is rare). Deeper stone trends toward
//! richer ores: coal/copper near the surface, iron/gold in the mid-depths, and
//! quartz / amber / crystal / diamond in the deeps. No clock, no IO, no randomness of
//! its own — the caller supplies the roll, so it's trivially testable and replayable.

use crate::world::blocks::BlockId;

/// Pick an ore to seed into exposed cave stone `depth` blocks below sea level, given
/// a uniform `roll01` in `[0, 1)`. `None` most of the time.
pub fn ore_for(depth: i32, roll01: f32) -> Option<BlockId> {
    if depth < 4 {
        return None; // surface rock stays plain
    }
    // Spawn chance climbs gently with depth, capped — caves get richer as you descend.
    let spawn = (0.04 + depth as f32 * 0.0015).min(0.12);
    if roll01 >= spawn {
        return None;
    }
    // Re-normalise the hit into a sub-roll choosing *which* ore.
    let r = (roll01 / spawn).clamp(0.0, 0.9999);
    Some(if depth < 16 {
        if r < 0.60 {
            BlockId::CoalOre
        } else if r < 0.85 {
            BlockId::CopperOre
        } else {
            BlockId::IronOre
        }
    } else if depth < 40 {
        if r < 0.35 {
            BlockId::IronOre
        } else if r < 0.60 {
            BlockId::CopperOre
        } else if r < 0.80 {
            BlockId::CoalOre
        } else if r < 0.93 {
            BlockId::GoldOre
        } else {
            BlockId::Quartz
        }
    } else if depth < 72 {
        if r < 0.30 {
            BlockId::GoldOre
        } else if r < 0.50 {
            BlockId::IronOre
        } else if r < 0.68 {
            BlockId::Quartz
        } else if r < 0.82 {
            BlockId::Amber
        } else if r < 0.94 {
            BlockId::Crystal
        } else {
            BlockId::DiamondOre
        }
    } else {
        if r < 0.40 {
            BlockId::DiamondOre
        } else if r < 0.65 {
            BlockId::Crystal
        } else if r < 0.82 {
            BlockId::GoldOre
        } else {
            BlockId::Quartz
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_rock_is_plain() {
        for r in [0.0, 0.01, 0.5, 0.99] {
            assert_eq!(ore_for(2, r), None);
        }
    }

    #[test]
    fn high_roll_never_spawns() {
        // Above the spawn ceiling (0.12), nothing appears at any depth.
        for d in [8, 30, 60, 100] {
            assert_eq!(ore_for(d, 0.2), None);
            assert_eq!(ore_for(d, 0.5), None);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(ore_for(50, 0.01), ore_for(50, 0.01));
    }

    #[test]
    fn deep_can_yield_diamond() {
        // Smallest rolls at great depth hit the richest tier.
        assert_eq!(ore_for(90, 0.0), Some(BlockId::DiamondOre));
    }

    #[test]
    fn shallow_low_roll_is_coal() {
        assert_eq!(ore_for(10, 0.0), Some(BlockId::CoalOre));
    }
}
