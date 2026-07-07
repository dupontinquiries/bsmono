//! `rebuild_coalescer` — one octree rebuild per chunk per cadence.
//!
//! Worker-03 / WORLD pure core, round 7. Plant growth dirties a chunk many
//! times within a short window; rebuilding the octree on every edit is wasted
//! work. This accumulator collapses a burst of edits into a single rebuild once
//! a cadence has elapsed — cutting rebuild *frequency*, complementing
//! `recompaction` (which cut per-rebuild *cost*) and `octree_remerge`.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Per-chunk rebuild accumulator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coalescer {
    /// Tick of the last rebuild (cadence is measured from here).
    pub last_rebuild_tick: u64,
    /// Whether any edit is pending since the last rebuild.
    pub dirty: bool,
    /// Number of edits coalesced since the last rebuild (diagnostic).
    pub pending: u32,
}

impl Default for Coalescer {
    fn default() -> Self {
        Coalescer {
            last_rebuild_tick: 0,
            dirty: false,
            pending: 0,
        }
    }
}

impl Coalescer {
    /// A coalescer whose cadence clock starts at `tick` (so a fresh chunk
    /// doesn't immediately look "overdue").
    pub fn started_at(tick: u64) -> Self {
        Coalescer {
            last_rebuild_tick: tick,
            dirty: false,
            pending: 0,
        }
    }
}

/// Record an edit at `tick`: mark dirty and count it. The actual rebuild is
/// deferred to `should_rebuild`.
pub fn note_edit(c: &mut Coalescer, tick: u64) {
    let _ = tick; // edits coalesce by cadence, not by individual tick
    c.dirty = true;
    c.pending = c.pending.saturating_add(1);
}

/// Whether a rebuild should fire now: there is pending work AND at least
/// `cadence` ticks have elapsed since the last rebuild.
pub fn should_rebuild(c: &Coalescer, tick: u64, cadence: u64) -> bool {
    c.dirty && tick.saturating_sub(c.last_rebuild_tick) >= cadence
}

/// Mark a rebuild done at `tick`: clears the pending edits and resets the
/// cadence clock.
pub fn after_rebuild(c: &mut Coalescer, tick: u64) {
    c.dirty = false;
    c.pending = 0;
    c.last_rebuild_tick = tick;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_coalescer_never_rebuilds() {
        let c = Coalescer::started_at(0);
        for tick in [0u64, 5, 100, 10_000] {
            assert!(!should_rebuild(&c, tick, 10));
        }
    }

    #[test]
    fn rebuild_fires_only_after_cadence_elapses() {
        let mut c = Coalescer::started_at(0);
        note_edit(&mut c, 0);
        assert!(!should_rebuild(&c, 5, 10)); // too soon
        assert!(should_rebuild(&c, 10, 10)); // cadence reached
        assert!(should_rebuild(&c, 50, 10)); // and after
    }

    #[test]
    fn n_edits_in_a_window_coalesce_to_one_rebuild() {
        let mut c = Coalescer::started_at(0);
        for t in 0..9 {
            note_edit(&mut c, t);
        }
        assert_eq!(c.pending, 9);
        // Within the window (cadence 10) it's still just "pending", one rebuild.
        assert!(!should_rebuild(&c, 9, 10));
        assert!(should_rebuild(&c, 10, 10));
        after_rebuild(&mut c, 10);
        // One rebuild consumed all 9 edits.
        assert_eq!(c.pending, 0);
        assert!(!should_rebuild(&c, 11, 10));
    }

    #[test]
    fn after_rebuild_clears_dirty_and_pending() {
        let mut c = Coalescer::started_at(0);
        note_edit(&mut c, 1);
        note_edit(&mut c, 2);
        after_rebuild(&mut c, 12);
        assert!(!c.dirty);
        assert_eq!(c.pending, 0);
        assert_eq!(c.last_rebuild_tick, 12);
    }

    #[test]
    fn at_most_one_rebuild_per_cadence_window() {
        // Simulate a stream of edits + rebuild checks; count rebuilds.
        let mut c = Coalescer::started_at(0);
        let cadence = 10u64;
        let mut rebuilds = 0u32;
        for tick in 1..=100u64 {
            note_edit(&mut c, tick); // an edit every tick
            if should_rebuild(&c, tick, cadence) {
                after_rebuild(&mut c, tick);
                rebuilds += 1;
            }
        }
        // 100 ticks, cadence 10 ⇒ ~10 rebuilds, never one per edit.
        assert!(rebuilds <= 11, "too many rebuilds: {rebuilds}");
        assert!(rebuilds >= 9);
    }

    #[test]
    fn deterministic() {
        let mut a = Coalescer::started_at(3);
        let mut b = Coalescer::started_at(3);
        for t in 0..20 {
            note_edit(&mut a, t);
            note_edit(&mut b, t);
        }
        assert_eq!(a, b);
        assert_eq!(should_rebuild(&a, 30, 10), should_rebuild(&b, 30, 10));
    }
}
