//! `sim::grove_decay` — the vitality of a keystone grove: it thrives while its
//! grower (the bonded creature that tends it) is near, and wilts when left
//! alone — dying off if neglected long enough.
//!
//! [`Grove::vitality`] is a single `0..1` bar. [`tick`] raises it while the
//! grower is present and lowers it otherwise, by a rate proportional to elapsed
//! time, clamped to the bar. [`alive`] reports whether the grove is still above
//! the death floor. Pure and deterministic — no clock, no randomness; the same
//! `(grove, presence, dt)` always yields the same next grove.

/// Vitality gained per second while the grower is present.
pub const GROW_PER_SEC: f32 = 0.10;
/// Vitality lost per second while the grower is absent.
pub const DECAY_PER_SEC: f32 = 0.05;
/// At or below this vitality the grove is dead.
pub const DEATH_FLOOR: f32 = 0.05;

/// A keystone grove.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grove {
    /// Vitality, `0..1`.
    pub vitality: f32,
}

impl Grove {
    /// A grove at a given vitality, clamped into range.
    pub fn new(vitality: f32) -> Self {
        Grove {
            vitality: clamp01(vitality),
        }
    }
}

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Advance a grove one tick.
///
/// Vitality rises by `GROW_PER_SEC * dt` when `grower_present`, otherwise falls
/// by `DECAY_PER_SEC * dt`, and is clamped to `[0, 1]`. Negative `dt` is treated
/// as zero, so a tick never runs the grove backwards.
pub fn tick(g: Grove, grower_present: bool, dt: f32) -> Grove {
    let dt = if dt > 0.0 { dt } else { 0.0 };
    let delta = if grower_present {
        GROW_PER_SEC * dt
    } else {
        -DECAY_PER_SEC * dt
    };
    Grove {
        vitality: clamp01(g.vitality + delta),
    }
}

/// Whether the grove is still alive (vitality above [`DEATH_FLOOR`]).
pub fn alive(g: &Grove) -> bool {
    g.vitality > DEATH_FLOOR
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn grows_with_grower() {
        let g = Grove::new(0.5);
        let after = tick(g, true, 1.0);
        assert!(after.vitality > g.vitality);
    }

    #[test]
    fn decays_without_grower() {
        let g = Grove::new(0.5);
        let after = tick(g, false, 1.0);
        assert!(after.vitality < g.vitality);
    }

    #[test]
    fn bounded_in_unit_range() {
        let mut seed = 0x6701_u32;
        for _ in 0..4000 {
            let v = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let present = lcg(&mut seed) & 1 == 0;
            let dt = (lcg(&mut seed) % 100_001) as f32 / 1000.0 - 50.0; // wild, incl. negative
            let after = tick(Grove::new(v), present, dt);
            assert!(after.vitality >= 0.0 && after.vitality <= 1.0);
        }
    }

    #[test]
    fn dies_below_floor() {
        // neglect a low-vitality grove until it dies
        let mut g = Grove::new(0.2);
        let mut steps = 0;
        while alive(&g) && steps < 1000 {
            g = tick(g, false, 1.0);
            steps += 1;
        }
        assert!(!alive(&g));
        assert!(g.vitality <= DEATH_FLOOR);
    }

    #[test]
    fn revives_and_a_thriving_grove_is_alive() {
        let g = tick(Grove::new(0.5), true, 3.0);
        assert!(alive(&g));
    }

    #[test]
    fn monotone_in_dt_each_direction() {
        let mut seed = 0xA0A0_u32;
        for _ in 0..3000 {
            let v = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let d1 = (lcg(&mut seed) % 10_001) as f32 / 1000.0;
            let d2 = (lcg(&mut seed) % 10_001) as f32 / 1000.0;
            let lo = d1.min(d2);
            let hi = d1.max(d2);
            // with grower: more time → at least as much vitality
            let grow_lo = tick(Grove::new(v), true, lo).vitality;
            let grow_hi = tick(Grove::new(v), true, hi).vitality;
            assert!(grow_hi >= grow_lo - 1e-6);
            // without grower: more time → at most as much vitality
            let dec_lo = tick(Grove::new(v), false, lo).vitality;
            let dec_hi = tick(Grove::new(v), false, hi).vitality;
            assert!(dec_hi <= dec_lo + 1e-6);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xF1F1_u32;
        for _ in 0..3000 {
            let v = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let present = lcg(&mut seed) & 1 == 0;
            let dt = (lcg(&mut seed) % 10_001) as f32 / 1000.0;
            let g = Grove::new(v);
            assert_eq!(tick(g, present, dt), tick(g, present, dt));
        }
    }
}

// ── service registration (S3 world/items/misc) ──
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

struct GroveService(Grove);
impl Service for GroveService {
    fn name(&self) -> &'static str {
        "grove_decay"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        // neutral: no grower present until wired to a world owner
        self.0 = crate::sim::grove_decay::tick(self.0, false, ctx.dt);
    }
}
#[distributed_slice(SERVICES)]
static GROVE_DECAY_SERVICE: ServiceEntry =
    ServiceEntry { name: "grove_decay", make: || Box::new(GroveService(Grove::new(1.0))) };
