//! `sim::flee_decision` — fight or flee.
//!
//! [`should_flee`] decides whether a creature breaks off given its own health,
//! the threat's power, and its courage. It stands its ground while its resolve —
//! health plus courage — covers the threat's power, and flees once the threat
//! outweighs that resolve. So a hurt creature flees sooner, a brave one holds
//! longer, and an overwhelming threat routs anyone; the threshold is a strict
//! comparison so the fight/flee boundary is stable and deterministic.
//!
//! Pure: clamps and one comparison, no clock, no randomness, no cross-imports.

fn nonneg(x: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

/// Whether the creature should flee.
///
/// Flees when `threat_power > own_hp + courage` (all floored at `0`). Equal
/// resolve and power means hold (the strict `>` keeps the boundary decisive).
pub fn should_flee(own_hp: f32, threat_power: f32, courage: f32) -> bool {
    let resolve = nonneg(own_hp) + nonneg(courage);
    nonneg(threat_power) > resolve
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn low_hp_flees() {
        // almost no health, a real threat, ordinary courage → flee
        assert!(should_flee(0.0, 1.0, 0.3));
    }

    #[test]
    fn high_courage_holds() {
        // same threat, but plenty of courage covers it → hold
        assert!(!should_flee(0.5, 1.0, 1.0));
    }

    #[test]
    fn overwhelming_power_routs_anyone() {
        assert!(should_flee(1.0, 1000.0, 1.0));
    }

    #[test]
    fn boundary_is_stable_and_holds_on_equal() {
        // power exactly equals resolve → hold (strict >)
        assert!(!should_flee(0.6, 1.0, 0.4)); // 0.6+0.4 == 1.0
        // a hair more power → flee
        assert!(should_flee(0.6, 1.0001, 0.4));
    }

    #[test]
    fn monotone_in_hp() {
        // sweeping hp upward, once it stops fleeing it never resumes
        let power = 1.5;
        let courage = 0.2;
        let mut prev_flee = true;
        let mut i = 0;
        while i <= 300 {
            let hp = i as f32 / 100.0; // 0 .. 3
            let f = should_flee(hp, power, courage);
            // must not flip false → true as hp increases
            assert!(!(f && !prev_flee), "flee resumed at hp {}", hp);
            prev_flee = f;
            i += 1;
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let hp = (lcg(&mut seed) % 100_001) as f32 / 10_000.0;
            let power = (lcg(&mut seed) % 100_001) as f32 / 10_000.0;
            let courage = (lcg(&mut seed) % 100_001) as f32 / 10_000.0;
            assert_eq!(should_flee(hp, power, courage), should_flee(hp, power, courage));
        }
    }
}
