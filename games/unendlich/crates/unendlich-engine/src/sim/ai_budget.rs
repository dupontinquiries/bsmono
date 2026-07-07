//! `sim::ai_budget` — deterministic brain-tick cadence ("curfew") for fauna.
//!
//! Every roamer running a full brain every frame is the headline fauna cost: the
//! VISION doc calls out per-roamer surface probing, biome lookups, and the
//! O(n²) mating scan, and recommends capping/curfewing *distant* roamer ticks.
//! This is that policy, as a pure function.
//!
//! A creature far from the player does not need to think every frame. We bin by
//! distance and tick distant creatures on a longer **period**, and we **stagger**
//! by the creature's stable id so the cost spreads evenly across frames instead
//! of every far creature thinking on the same frame (which would just move the
//! spike, not remove it).
//!
//! Deterministic and rollback-safe: [`should_full_tick`] is a pure function of
//! the global tick counter, the entity id, and the band — no clock, no RNG, no
//! float. When a creature *does* tick after skipping frames, advance its
//! integration by [`TickBand::dt_scale`] ticks' worth so movement stays correct.

/// How often a creature thinks, by distance from the viewer. Thresholds are
/// placeholders (tune against the mob cap and render distance).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TickBand {
    /// Close: full simulation every frame.
    Near,
    /// Mid: every 2nd frame.
    Mid,
    /// Far: every 4th frame.
    Far,
    /// Very far: every 8th frame (still alive, just coarse).
    VFar,
}

/// Band cutoffs in blocks (squared comparisons would be used at the call site if
/// distance is kept squared; here we take a plain block distance for clarity).
const NEAR_BLOCKS: i32 = 32;
const MID_BLOCKS: i32 = 96;
const FAR_BLOCKS: i32 = 256;

impl TickBand {
    /// Classify by distance-to-viewer in blocks.
    pub fn from_distance(blocks: i32) -> TickBand {
        if blocks < NEAR_BLOCKS {
            TickBand::Near
        } else if blocks < MID_BLOCKS {
            TickBand::Mid
        } else if blocks < FAR_BLOCKS {
            TickBand::Far
        } else {
            TickBand::VFar
        }
    }

    /// Frames between full ticks for this band (always ≥ 1, always a power of two
    /// so the stagger distributes evenly).
    pub fn period(self) -> u64 {
        match self {
            TickBand::Near => 1,
            TickBand::Mid => 2,
            TickBand::Far => 4,
            TickBand::VFar => 8,
        }
    }

    /// How much to scale a skipped creature's integration timestep on the frame it
    /// finally ticks, so a curfewed brain still moves the right distance.
    pub fn dt_scale(self) -> u64 {
        self.period()
    }
}

/// Should this creature run a full brain tick on global frame `tick`?
///
/// Pure and deterministic: distant creatures tick once per `period`, offset by a
/// hash of their id so the population's full-tick frames are spread out rather
/// than synchronized. `Near` always ticks.
pub fn should_full_tick(tick: u64, entity_id: u64, band: TickBand) -> bool {
    let p = band.period();
    if p <= 1 {
        return true;
    }
    // Stagger the phase by the id so two same-band creatures rarely tick together.
    (tick.wrapping_add(stagger(entity_id))) % p == 0
}

/// A cheap, deterministic per-id phase offset. (Mix the id so sequential ids
/// don't all share a phase.)
#[inline]
fn stagger(id: u64) -> u64 {
    // splitmix64 finalizer — well-distributed, deterministic, no allocations.
    let mut z = id.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_are_monotonic_in_distance() {
        assert_eq!(TickBand::from_distance(0), TickBand::Near);
        assert_eq!(TickBand::from_distance(31), TickBand::Near);
        assert_eq!(TickBand::from_distance(32), TickBand::Mid);
        assert_eq!(TickBand::from_distance(95), TickBand::Mid);
        assert_eq!(TickBand::from_distance(96), TickBand::Far);
        assert_eq!(TickBand::from_distance(255), TickBand::Far);
        assert_eq!(TickBand::from_distance(256), TickBand::VFar);
        assert_eq!(TickBand::from_distance(10_000), TickBand::VFar);
    }

    #[test]
    fn near_always_ticks() {
        for t in 0..50 {
            assert!(should_full_tick(t, 12345, TickBand::Near));
        }
    }

    #[test]
    fn far_ticks_once_per_period() {
        // For any fixed creature, exactly one full tick per `period` frames.
        for &band in &[TickBand::Mid, TickBand::Far, TickBand::VFar] {
            let p = band.period();
            for id in [0u64, 1, 7, 99, 1_000_000] {
                let count = (0..p).filter(|&t| should_full_tick(t, id, band)).count() as u64;
                assert_eq!(count, 1, "id {id} band {band:?} ticks exactly once per period");
            }
        }
    }

    #[test]
    fn every_creature_eventually_ticks() {
        // Over a full period window each id ticks at least once (no starvation).
        let band = TickBand::VFar;
        let p = band.period();
        for id in 0..200u64 {
            let any = (0..p).any(|t| should_full_tick(t, id, band));
            assert!(any, "id {id} must tick within its period");
        }
    }

    #[test]
    fn stagger_spreads_load_across_frames() {
        // Many same-band creatures should NOT all tick on the same frame.
        let band = TickBand::Far; // period 4
        let n = 400u64;
        let on_frame0 = (0..n).filter(|&id| should_full_tick(0, id, band)).count();
        // If unstaggered, all n would tick on frame 0. With staggering, ~1/4 do.
        assert!(on_frame0 < (n as usize * 2 / 5), "load is spread, not synchronized: {on_frame0}/{n}");
        assert!(on_frame0 > 0, "but some do tick on frame 0");
    }

    #[test]
    fn dt_scale_matches_period() {
        assert_eq!(TickBand::Near.dt_scale(), 1);
        assert_eq!(TickBand::Far.dt_scale(), 4);
        assert_eq!(TickBand::VFar.dt_scale(), 8);
    }

    #[test]
    fn is_deterministic() {
        for t in 0..32u64 {
            for id in 0..32u64 {
                let band = TickBand::Far;
                assert_eq!(should_full_tick(t, id, band), should_full_tick(t, id, band));
            }
        }
    }
}
