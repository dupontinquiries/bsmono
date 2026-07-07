//! `sim::spawn_budget` — how many entities may actually spawn this tick.
//!
//! [`allowed`] takes the number of spawns waiting (`pending`), the per-tick
//! `budget`, the current `active` count, and the global `cap`, and returns how
//! many to spawn now: the smallest of what's pending, what the budget permits,
//! and what room is left under the cap. When the world is already at (or over)
//! capacity, nothing spawns.
//!
//! Pure and deterministic: two saturating mins — no clock, no randomness, no
//! cross-imports.

/// Number of spawns to allow this tick: `min(pending, budget, cap - active)`,
/// with the room under the cap saturating at 0.
pub fn allowed(pending: u32, budget: u32, active: u32, cap: u32) -> u32 {
    let room = cap.saturating_sub(active);
    pending.min(budget).min(room)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn respects_budget() {
        // plenty pending and room, but the budget limits it
        assert_eq!(allowed(100, 5, 0, 1000), 5);
    }

    #[test]
    fn respects_cap() {
        // budget and pending high, but only 3 slots under the cap
        assert_eq!(allowed(100, 100, 7, 10), 3);
    }

    #[test]
    fn zero_when_full() {
        assert_eq!(allowed(50, 50, 10, 10), 0); // at cap
        assert_eq!(allowed(50, 50, 15, 10), 0); // over cap (saturates)
    }

    #[test]
    fn pending_below_budget() {
        // fewer pending than budget and room → spawn all pending
        assert_eq!(allowed(3, 10, 0, 100), 3);
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let pending = lcg(&mut seed) % 10_000;
            let budget = lcg(&mut seed) % 10_000;
            let active = lcg(&mut seed) % 10_000;
            let cap = lcg(&mut seed) % 10_000;
            let a = allowed(pending, budget, active, cap);
            assert!(a <= pending && a <= budget);
            assert!(active + a <= cap || active >= cap, "would exceed cap");
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let p = lcg(&mut seed) % 10_000;
            let b = lcg(&mut seed) % 10_000;
            let ac = lcg(&mut seed) % 10_000;
            let c = lcg(&mut seed) % 10_000;
            assert_eq!(allowed(p, b, ac, c), allowed(p, b, ac, c));
        }
    }
}
