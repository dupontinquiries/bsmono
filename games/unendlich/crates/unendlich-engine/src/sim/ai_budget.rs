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


// ------------------------------------------------------------------------
// FRAME BUDGET — a time box on the whole roamer pass (perfbisect, desktop
// report of 2026-09-01).
//
// The distance bands above bound how OFTEN a brain runs. They do not bound
// how long the pass takes: with ~20 creatures clustered near the player,
// `roamer_tick` measured 12–16 ms p50 in four of eighteen runs and 2–4 ms
// in the rest — the same code, a different herd. Whatever the per-creature
// cost is (and it is the next thing to cut), a pass that can take the whole
// frame budget whenever the herd is unlucky is not scalable, and on a phone
// it is worse.
//
// So: a wall-clock budget for the pass. Once it is spent, the rest of the
// creatures that frame are skipped — EXCEPT the ones within `near_blocks`
// of the viewer, which always tick, because those are the ones on screen
// and Minecraft's simulation distance is the same principle. A skipped
// creature does not record `last_tick_frame`, so the r265 catch-up path
// integrates the missed ticks the next time it runs; nothing freezes for
// good. Persistent creatures (dragons) are exempt at the call site.
//
// Fairness across frames comes from the catch-up, not from rotating the
// loop start: a creature skipped this frame has a larger `since_last` next
// frame and fires with a larger `n`, so its share of simulation is
// preserved even when its share of frames is not.
//
// `UNENDLICH_ROAMER_BUDGET_MS` sets the box (default 4.0; 0 disables).

use std::time::{Duration, Instant};

/// Default budget, ms. GUESS: a quarter of a 60 Hz frame, a sixth of the
/// 24 ms the herd cost when it was heavy.
pub const DEFAULT_ROAMER_BUDGET_MS: f32 = 4.0;
/// Creatures inside this many blocks always tick, budget or not.
pub const NEAR_ALWAYS_BLOCKS: i32 = 24;

#[derive(Debug)]
pub struct FrameBudget {
    budget: Duration,
    near_blocks: i32,
    start: Instant,
    /// how many this pass skipped, for the log
    pub skipped: u32,
    /// how many this pass ran
    pub ran: u32,
    /// the elapsed time when the budget was first found spent, ms
    pub spent_at_ms: Option<f32>,
}

impl FrameBudget {
    /// A budget of `ms` milliseconds; `0.0` (or negative) disables it.
    pub fn new(ms: f32) -> FrameBudget {
        FrameBudget {
            // milliseconds in, and `from_secs_f32` takes SECONDS: the first
            // version made a 4 ms budget a 4 s one, and the test caught it
            budget: Duration::from_secs_f32(ms.max(0.0) / 1000.0),
            near_blocks: NEAR_ALWAYS_BLOCKS,
            start: Instant::now(),
            skipped: 0,
            ran: 0,
            spent_at_ms: None,
        }
    }

    /// From the environment: `UNENDLICH_ROAMER_BUDGET_MS`, default
    /// `DEFAULT_ROAMER_BUDGET_MS`. Lenient: an unparsable value is the
    /// default, "0" is off.
    pub fn from_env() -> FrameBudget {
        let ms = std::env::var("UNENDLICH_ROAMER_BUDGET_MS")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(DEFAULT_ROAMER_BUDGET_MS);
        FrameBudget::new(ms)
    }

    pub fn enabled(&self) -> bool {
        !self.budget.is_zero()
    }

    /// Start a pass. Call once per frame before the creature loop.
    pub fn begin(&mut self) {
        self.start = Instant::now();
        self.skipped = 0;
        self.ran = 0;
        self.spent_at_ms = None;
    }

    /// May the creature at `dist_blocks` from the viewer tick this frame?
    /// Always when the budget is off or unspent, always when near; else
    /// no. Counts either way.
    pub fn allow(&mut self, dist_blocks: i32) -> bool {
        self.allow_at(dist_blocks, self.start.elapsed())
    }

    /// The decision with the clock passed in, so it can be tested without
    /// sleeping.
    pub fn allow_at(&mut self, dist_blocks: i32, elapsed: Duration) -> bool {
        let spent = self.enabled() && elapsed >= self.budget;
        if spent && self.spent_at_ms.is_none() {
            self.spent_at_ms = Some(elapsed.as_secs_f32() * 1000.0);
        }
        if !spent || dist_blocks < self.near_blocks {
            self.ran += 1;
            true
        } else {
            self.skipped += 1;
            false
        }
    }

    /// One line for the window log, or nothing when nothing was skipped.
    pub fn report(&self) -> Option<String> {
        if self.skipped == 0 {
            return None;
        }
        Some(format!(
            "[roamer_budget] ran {} skipped {} (budget {:.1} ms spent at {:.1} ms; near {} blocks always)",
            self.ran,
            self.skipped,
            self.budget.as_secs_f32() * 1000.0,
            self.spent_at_ms.unwrap_or(0.0),
            self.near_blocks
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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

    // ---- FrameBudget ----

    #[test]
    fn under_budget_everyone_ticks() {
        let mut b = FrameBudget::new(4.0);
        b.begin();
        let t = Duration::from_millis(1);
        for d in [0, 30, 100, 500] {
            assert!(b.allow_at(d, t));
        }
        assert_eq!(b.ran, 4);
        assert_eq!(b.skipped, 0);
        assert!(b.report().is_none());
    }

    #[test]
    fn over_budget_only_the_near_tick_and_the_rest_are_counted_as_skipped() {
        let mut b = FrameBudget::new(4.0);
        b.begin();
        let t = Duration::from_millis(5);
        assert!(b.allow_at(0, t));
        assert!(b.allow_at(NEAR_ALWAYS_BLOCKS - 1, t));
        assert!(!b.allow_at(NEAR_ALWAYS_BLOCKS, t));
        assert!(!b.allow_at(400, t));
        assert_eq!(b.ran, 2);
        assert_eq!(b.skipped, 2);
        assert!(b.spent_at_ms.unwrap() >= 5.0);
        assert!(b.report().unwrap().contains("skipped 2"));
    }

    #[test]
    fn a_zero_budget_is_off_and_never_skips() {
        let mut b = FrameBudget::new(0.0);
        b.begin();
        assert!(!b.enabled());
        assert!(b.allow_at(1000, Duration::from_secs(1)));
        assert_eq!(b.skipped, 0);
    }

    #[test]
    fn begin_resets_the_counters_each_frame() {
        let mut b = FrameBudget::new(1.0);
        b.begin();
        assert!(!b.allow_at(500, Duration::from_millis(2)));
        assert_eq!(b.skipped, 1);
        b.begin();
        assert_eq!(b.skipped, 0);
        assert!(b.spent_at_ms.is_none());
    }
}
