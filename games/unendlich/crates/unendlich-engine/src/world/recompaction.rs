//! `recompaction` — when to trigger a cold-path octree recompaction.
//!
//! Worker-03 / WORLD-PERF pure core. Split-only `set_block` edits let an
//! edited chunk's octree grow past its post-load baseline (see
//! `octree_compact` for the reclaim). This module is the *trigger policy*: a
//! pure decision function that, given the chunk's current node count, its
//! post-load baseline, and how many ticks since its last rebuild, decides
//! whether to run a recompaction now — with hysteresis so it doesn't thrash
//! around the threshold, and a cooldown so it never rebuilds too often.
//!
//! Pure & deterministic: no wall-clock, no rng (the caller supplies the tick
//! delta). Self-contained for standalone `rustc --test`.
#![allow(dead_code)]

/// Tunable thresholds for the recompaction trigger.
#[derive(Clone, Copy, Debug)]
pub struct RecompactionPolicy {
    /// Fire when `node_count` rises above `trigger_ratio × baseline`.
    pub trigger_ratio: f32,
    /// Re-arm only after `node_count` falls back below `clear_ratio × baseline`
    /// (the hysteresis band; must be `< trigger_ratio`).
    pub clear_ratio: f32,
    /// Cooldown: never fire within this many ticks of the last rebuild.
    pub min_ticks_between: u32,
    /// Chunks with a baseline below this never recompact (tiny chunks can't
    /// drift enough to matter, and we don't want them thrashing).
    pub min_baseline: u32,
}

impl Default for RecompactionPolicy {
    fn default() -> Self {
        RecompactionPolicy {
            trigger_ratio: 1.5,
            clear_ratio: 1.2,
            min_ticks_between: 120,
            min_baseline: 64,
        }
    }
}

/// Per-chunk hysteresis latch, carried across decisions. `armed` means the
/// policy is ready to fire; it disarms after firing and re-arms only once the
/// ratio has fallen back under the clear band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriggerState {
    pub armed: bool,
}

impl Default for TriggerState {
    fn default() -> Self {
        TriggerState { armed: true }
    }
}

/// Decide whether to recompact now, updating the hysteresis latch in `state`.
///
/// Returns `true` exactly when: the chunk is big enough to bother with
/// (`baseline >= min_baseline`), the latch is armed, the node count is at or
/// above the trigger band, and the cooldown has elapsed. Firing disarms the
/// latch until the ratio falls back under the clear band — so oscillation
/// just above the trigger produces at most one fire, not a flip-flop.
///
/// Pure & deterministic: output depends only on `(policy, state, inputs)`.
pub fn decide(
    policy: &RecompactionPolicy,
    state: &mut TriggerState,
    node_count: u32,
    baseline: u32,
    ticks_since_last: u32,
) -> bool {
    // Tiny chunks: never recompact; keep armed so we don't accumulate state.
    if baseline < policy.min_baseline {
        state.armed = true;
        return false;
    }

    let ratio = node_count as f32 / baseline.max(1) as f32;

    // Re-arm once we've fallen back under the clear band.
    if !state.armed && ratio < policy.clear_ratio {
        state.armed = true;
    }

    if state.armed && ratio >= policy.trigger_ratio && ticks_since_last >= policy.min_ticks_between {
        state.armed = false;
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pol() -> RecompactionPolicy {
        RecompactionPolicy::default()
    }

    #[test]
    fn below_threshold_does_not_trigger() {
        let p = pol();
        let mut s = TriggerState::default();
        // node_count == baseline → ratio 1.0 < 1.5
        assert!(!decide(&p, &mut s, 100, 100, 10_000));
        // just under the trigger band
        assert!(!decide(&p, &mut s, 149, 100, 10_000));
    }

    #[test]
    fn well_above_triggers_when_armed_and_cooled_down() {
        let p = pol();
        let mut s = TriggerState::default();
        assert!(decide(&p, &mut s, 300, 100, 10_000)); // 3× baseline, past cooldown
        assert!(!s.armed); // disarmed after firing
    }

    #[test]
    fn cooldown_blocks_even_when_high() {
        let p = pol();
        let mut s = TriggerState::default();
        // ratio high but only 10 ticks since last rebuild (< 120)
        assert!(!decide(&p, &mut s, 300, 100, 10));
        assert!(s.armed); // still armed — it never fired
    }

    #[test]
    fn tiny_baseline_never_triggers() {
        let p = pol();
        let mut s = TriggerState::default();
        // baseline below min_baseline (64), even at huge ratio
        assert!(!decide(&p, &mut s, 10_000, 32, 10_000));
    }

    #[test]
    fn hysteresis_prevents_flip_flop() {
        let p = pol();
        let mut s = TriggerState::default();
        // Oscillate node_count just around the trigger band (×1.49 / ×1.51 of
        // baseline 100), always past the cooldown. Without hysteresis this
        // would fire on every ×1.51 sample.
        let mut fires = 0;
        for step in 0..20 {
            let nc = if step % 2 == 0 { 151 } else { 149 };
            if decide(&p, &mut s, nc, 100, 10_000) {
                fires += 1;
            }
        }
        // Disarmed after the first fire; ratio never drops under clear_ratio
        // (1.2 → 120 nodes) during the oscillation, so it never re-arms.
        assert_eq!(fires, 1);

        // Drop well under the clear band → re-arms → can fire again.
        assert!(!decide(&p, &mut s, 100, 100, 10_000)); // ratio 1.0 re-arms, but < trigger so no fire
        assert!(s.armed);
        assert!(decide(&p, &mut s, 200, 100, 10_000)); // armed + above trigger → fires
    }

    #[test]
    fn prop_deterministic() {
        let p = pol();
        for nc in (0..600u32).step_by(7) {
            for base in [1u32, 50, 64, 100, 256] {
                for ticks in [0u32, 60, 120, 5000] {
                    let mut s1 = TriggerState::default();
                    let mut s2 = TriggerState::default();
                    let a = decide(&p, &mut s1, nc, base, ticks);
                    let b = decide(&p, &mut s2, nc, base, ticks);
                    assert_eq!(a, b);
                    assert_eq!(s1, s2);
                }
            }
        }
    }

    #[test]
    fn prop_no_fire_below_trigger_ratio() {
        let p = pol();
        // For any baseline >= min_baseline and ratio strictly below trigger,
        // a fresh armed latch never fires (regardless of cooldown).
        for base in [64u32, 100, 256, 1000] {
            let limit = (p.trigger_ratio * base as f32) as u32; // floor(1.5*base)
            for nc in (base..limit).step_by(((limit - base).max(1) / 8).max(1) as usize) {
                let mut s = TriggerState::default();
                assert!(!decide(&p, &mut s, nc, base, 1_000_000));
            }
        }
    }
}
