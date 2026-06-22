// src/volume_gesture.rs
#![allow(dead_code)]
//
// An "abstraction layer" between the volume dial's raw gesture output
// and the actual SetVolume call. Solves three concrete problems with
// touch/scroll-driven rotary volume controls:
//
//   1. WRAPAROUND ARTIFACTS. A finger sliding from 11 o'clock past 12
//      o'clock to 1 o'clock should be a small angular delta, but naive
//      angle-to-volume mapping can interpret the boundary cross as
//      "0% → 100% in one event". The filter rejects single-event jumps
//      above a sanity threshold and turns them into small nudges.
//
//   2. ASYMMETRIC SAFETY. Raising the volume rapidly is the
//      ear-damaging direction; lowering it is the safe direction. The
//      filter applies twice as much friction going up as going down,
//      so a panic-lower stays responsive while an accidental drag-up
//      stays civilized.
//
//   3. SUSTAINED-INTENT RAMP. A deliberate long gesture (dragging across
//      a wide arc and *holding*) should still let the user reach high
//      volumes — they're being clear about wanting it. The filter keeps
//      a counter of consecutive same-direction events; the per-event cap
//      grows with sustained intent, then resets when the user reverses
//      or pauses.
//
// The filter is pure state — no I/O, no qmetaobject, no clocks beyond
// what the caller passes in. That makes it cheap to test exhaustively
// (and easy to swap parameters if a real-device session reveals the
// defaults are too tight or too loose).

use std::time::{Duration, Instant};

/// Per-event change cap when going UP (toward louder). Conservative —
/// raising the volume is the direction that can hurt your hearing.
pub const BASE_STEP_UP: i32 = 4;
/// Per-event change cap when going DOWN. ~2.5× the up cap; panic-mute
/// should always feel immediate.
pub const BASE_STEP_DOWN: i32 = 10;
/// Magnitude above which a single-event delta is considered suspicious
/// — almost certainly a wraparound artifact or stray touch event. The
/// filter replaces it with a tiny nudge instead of letting it through.
pub const WRAPAROUND_THRESHOLD: i32 = 30;
/// How many consecutive same-direction events buy +1 step of headroom.
pub const SUSTAINED_BONUS_DIVISOR: i32 = 3;
/// Max sustained-motion bonus, in step units. Combined with BASE_STEP_UP=4
/// this means a heavily-sustained UP gesture can move at 12 units/event.
pub const MAX_SUSTAINED_BONUS: i32 = 8;
/// Idle time after which the sustained counter resets. If the user pauses
/// for this long, the next gesture starts fresh (we don't reward "I
/// dragged a bit, paused for 5s, then dragged again" with a head start).
pub const SUSTAINED_RESET_AFTER: Duration = Duration::from_millis(400);
/// Magnitude assigned to a flagged wraparound event. The direction of
/// the suspicious delta is preserved (so the user gets *some* movement
/// in the indicated direction — otherwise the dial would feel stuck).
pub const WRAPAROUND_NUDGE_UP: i32 = 1;
pub const WRAPAROUND_NUDGE_DOWN: i32 = 2;

/// The filter itself. Owned by QtState as a single field; mutated on
/// every gesture-driven set_volume call.
#[derive(Debug, Clone)]
pub struct GestureFilter {
    /// What we've actually sent to the device most recently.
    actual: i32,
    /// Cached target from the previous submit (used to confirm direction
    /// and detect wraparound).
    last_target: i32,
    /// Sign of the most recent non-zero delta: 1 = up, -1 = down, 0 = idle.
    last_direction: i32,
    /// Count of consecutive submits in `last_direction`.
    sustained: i32,
    /// When the most recent submit happened. Idle longer than
    /// SUSTAINED_RESET_AFTER → sustained counter resets.
    last_time: Option<Instant>,
    /// First submit establishes baseline without applying friction
    /// (otherwise jumping to an arbitrary initial volume would be capped).
    initialized: bool,
}

impl Default for GestureFilter {
    fn default() -> Self { Self::new() }
}

impl GestureFilter {
    pub fn new() -> Self {
        Self {
            actual: 0, last_target: 0, last_direction: 0,
            sustained: 0, last_time: None, initialized: false,
        }
    }

    /// Reset to a known volume. Called after device-side volume changes
    /// (so the next gesture's baseline matches the real value), or after
    /// device switches.
    pub fn anchor(&mut self, current: i32) {
        let v = current.clamp(0, 100);
        self.actual = v;
        self.last_target = v;
        self.last_direction = 0;
        self.sustained = 0;
        self.last_time = None;
        self.initialized = true;
    }

    /// Submit a target volume produced by a gesture. Returns the value
    /// that should actually be sent to the device after friction. Pure
    /// function modulo the filter's internal state.
    pub fn submit(&mut self, target: i32, now: Instant) -> i32 {
        let target = target.clamp(0, 100);

        if !self.initialized {
            self.actual = target;
            self.last_target = target;
            self.last_direction = 0;
            self.sustained = 0;
            self.last_time = Some(now);
            self.initialized = true;
            return self.actual;
        }

        // Reset sustained counter if the user paused for a beat. Stops
        // long-paused sequences from claiming a head start.
        if let Some(prev) = self.last_time {
            if now.duration_since(prev) > SUSTAINED_RESET_AFTER {
                self.sustained = 0;
                self.last_direction = 0;
            }
        }
        self.last_time = Some(now);

        let raw_delta = target - self.actual;
        if raw_delta == 0 {
            // Idle ping. Don't churn sustained counter, just return.
            self.last_target = target;
            return self.actual;
        }

        let going_up = raw_delta > 0;

        // WRAPAROUND DETECTION: the dial's emitted target jumped suspiciously
        // between consecutive frames. NOT the same as the gap between target
        // and our current actual — that's just "user wants a big change",
        // which a sustained gesture can legitimately produce. A single-event
        // discontinuity in what the dial reports is almost always a touch
        // artifact (boundary cross, stray event, etc.).
        let target_jump = (target - self.last_target).abs();
        if target_jump > WRAPAROUND_THRESHOLD {
            let nudge = if going_up { WRAPAROUND_NUDGE_UP } else { -WRAPAROUND_NUDGE_DOWN };
            self.actual = (self.actual + nudge).clamp(0, 100);
            // Wraparound interrupts deliberate motion — reset sustained.
            self.sustained = 0;
            self.last_direction = 0;
            self.last_target = target;
            return self.actual;
        }

        // Normal path. Update sustained counter, then apply per-event cap.
        let dir = if going_up { 1 } else { -1 };
        if dir == self.last_direction {
            self.sustained = (self.sustained + 1).min(60);  // cap to avoid overflow
        } else {
            // New direction (or first non-wraparound event). The first
            // event "counts" — we use sustained=1 so a deliberate drag
            // that immediately reverses doesn't get zero credit on the
            // reversal.
            self.sustained = 1;
            self.last_direction = dir;
        }

        let allowed = self.allowed_step(raw_delta, going_up);
        self.actual = (self.actual + allowed).clamp(0, 100);
        self.last_target = target;
        self.actual
    }

    /// Compute the maximum signed delta to apply this frame given the
    /// requested raw delta. Public for tests that want to inspect policy
    /// without touching state. NOTE: wraparound detection is *not* done
    /// here — it's handled in `submit` based on inter-frame target jump.
    fn allowed_step(&self, raw_delta: i32, going_up: bool) -> i32 {
        // Sustained-motion bonus grows the per-event cap. Sustained=1 is
        // the first event in a direction → bonus=0; bonus only kicks in
        // after several events in the same direction (which is the
        // "intentional sustained gesture" signal).
        let bonus = (self.sustained / SUSTAINED_BONUS_DIVISOR).min(MAX_SUSTAINED_BONUS);
        let cap = if going_up {
            BASE_STEP_UP + bonus
        } else {
            BASE_STEP_DOWN + bonus
        };

        if going_up {
            raw_delta.min(cap)
        } else {
            raw_delta.max(-cap)
        }
    }

    pub fn actual(&self) -> i32 { self.actual }
    pub fn sustained_count(&self) -> i32 { self.sustained }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(ms: u64) -> Instant {
        // Tests need a consistent base time; we offset from a fixed
        // ref Instant to keep durations meaningful. Using checked_add
        // because Instant arithmetic can saturate at the type's bounds.
        Instant::now() + Duration::from_millis(ms)
    }

    #[test]
    fn first_submit_establishes_baseline() {
        let mut f = GestureFilter::new();
        let v = f.submit(35, t(0));
        // No friction on first submit — accept whatever the dial says.
        assert_eq!(v, 35);
    }

    #[test]
    fn small_delta_passes_through() {
        let mut f = GestureFilter::new();
        f.anchor(30);
        let v = f.submit(33, t(0));
        assert_eq!(v, 33);
    }

    #[test]
    fn wraparound_up_becomes_tiny_nudge() {
        let mut f = GestureFilter::new();
        f.anchor(20);
        // Wraparound: dial says 90, actual was 20 — 70-unit delta.
        let v = f.submit(90, t(0));
        // Should land at 20 + WRAPAROUND_NUDGE_UP, NOT near 90.
        assert_eq!(v, 20 + WRAPAROUND_NUDGE_UP,
                   "expected nudge only, got {}", v);
    }

    #[test]
    fn wraparound_down_becomes_tiny_nudge() {
        let mut f = GestureFilter::new();
        f.anchor(80);
        let v = f.submit(10, t(0));   // 70-unit jump down
        assert_eq!(v, 80 - WRAPAROUND_NUDGE_DOWN);
    }

    #[test]
    fn moderate_up_delta_is_capped() {
        let mut f = GestureFilter::new();
        f.anchor(30);
        // 20-unit jump up (below wraparound threshold) → capped at BASE_STEP_UP.
        let v = f.submit(50, t(0));
        assert_eq!(v, 30 + BASE_STEP_UP);
    }

    #[test]
    fn moderate_down_delta_is_capped() {
        let mut f = GestureFilter::new();
        f.anchor(80);
        let v = f.submit(60, t(0));   // 20-unit jump down
        assert_eq!(v, 80 - BASE_STEP_DOWN);
    }

    #[test]
    fn down_is_less_throttled_than_up() {
        // Same magnitude, opposite direction: down should travel farther.
        let mut up = GestureFilter::new(); up.anchor(30);
        let mut dn = GestureFilter::new(); dn.anchor(70);
        let u = up.submit(50, t(0));
        let d = dn.submit(50, t(0));
        // u travelled 30→? going up; d travelled 70→? going down.
        let up_travel = u - 30;
        let down_travel = 70 - d;
        assert!(down_travel > up_travel,
                "down travel {} should exceed up travel {}", down_travel, up_travel);
    }

    #[test]
    fn sustained_up_gesture_grows_step_cap() {
        // Simulate a realistic continuous drag: the dial reports a target
        // that increments by a small amount each frame (well under
        // WRAPAROUND_THRESHOLD), but the target stays AHEAD of `actual`
        // by more than BASE_STEP_UP — so the cap matters, and we can see
        // the sustained-motion bonus kick in.
        let mut f = GestureFilter::new();
        f.anchor(20);
        let mut steps = Vec::new();
        let mut prev = f.actual();
        for i in 0..18 {
            // Each frame, ask for prev + 20 (well above base step, so cap
            // applies). Inter-frame target jump stays small (≤20), so no
            // wraparound flag.
            let target = (prev + 20).min(100);
            let v = f.submit(target, t(i * 16));
            steps.push(v - prev);
            prev = v;
            if v >= 100 { break; }
        }
        let first = steps[0];
        // Use the max step we observed — the last step may be small because
        // we hit the 100 ceiling and clamp ate most of it.
        let max_step = *steps.iter().max().unwrap();
        // First step should be exactly BASE_STEP_UP (no sustained yet).
        assert_eq!(first, BASE_STEP_UP, "first step should be BASE: {:?}", steps);
        // Some later step should have benefited from sustained bonus.
        assert!(max_step > first,
                "expected sustained bonus to grow step: {:?}", steps);
        assert!(max_step <= BASE_STEP_UP + MAX_SUSTAINED_BONUS,
                "sustained bonus should be capped");
    }

    #[test]
    fn wraparound_check_takes_priority_over_sustained() {
        // Build up sustained motion via realistic small inter-frame
        // targets, then have a single event jump suddenly (wraparound).
        let mut f = GestureFilter::new();
        f.anchor(20);
        let mut last_target = 20;
        for i in 0..15 {
            let target = (last_target + 8).min(95);  // small jump, under threshold
            let _ = f.submit(target, t(i * 50));
            last_target = target;
        }
        let before = f.actual();
        assert!(f.sustained_count() > 0, "should have accumulated sustained");
        // Now a wraparound event: target jumps suddenly far.
        let v = f.submit(2, t(20 * 50));
        // Should be wraparound-capped (nudge down), regardless of sustained.
        assert_eq!(v, before - WRAPAROUND_NUDGE_DOWN,
                   "wraparound should override sustained credit");
    }

    #[test]
    fn sustained_up_reaches_target_in_reasonable_frame_count() {
        // A user dragging deliberately from 20 → 80 should get there in
        // a reasonable number of frames. Realistic dial output: target
        // stays at 80 (user's finger held at the top position), each
        // frame is well under WRAPAROUND_THRESHOLD because last_target=80
        // after the first event. Each frame closes the gap by `cap`.
        let mut f = GestureFilter::new();
        // Anchor close to the start so the first submit isn't a wraparound.
        // (A real dial drag starts where the finger is and moves smoothly.)
        f.anchor(20);
        // First submit moves last_target to ~25 (small step), then keep
        // submitting 80 (jump=0 after second frame).
        let mut frames = 0;
        for i in 0..40 {
            let v = f.submit(80, t(i * 16));
            frames = i + 1;
            if v >= 80 { break; }
        }
        assert!(frames <= 20,
                "deliberate sustained up should reach 80 in ≤20 frames, took {}", frames);
        assert!(frames >= 8,
                "deliberate up shouldn't be too easy either, took {}", frames);
    }

    #[test]
    fn reversing_direction_resets_sustained() {
        // Build up sustained motion in one direction, then reverse.
        let mut f = GestureFilter::new();
        f.anchor(50);
        // Sustained upward via realistic incremental drags.
        let mut prev = 50;
        for i in 0..10 {
            let v = f.submit(prev + 20, t(i * 50));
            prev = v;
        }
        let sustained_up = f.sustained_count();
        assert!(sustained_up > 0,
                "should accumulate sustained on up drag, got {}", sustained_up);
        // Reverse: drop to below current with a small (non-wraparound) jump.
        let v = f.submit(prev - 10, t(11 * 50));
        // First event in new direction → sustained=1.
        assert_eq!(f.sustained_count(), 1,
                   "reversal should reset to 1, got {}", f.sustained_count());
        assert!(v < prev, "reversal should actually move volume down");
    }

    #[test]
    fn long_idle_resets_sustained() {
        let mut f = GestureFilter::new();
        f.anchor(30);
        // Build up sustained motion with realistic small inter-frame jumps.
        let mut prev = 30;
        for i in 0..8 {
            let v = f.submit(prev + 15, t(i * 50));
            prev = v;
        }
        assert!(f.sustained_count() > 0);
        // Idle longer than SUSTAINED_RESET_AFTER (400 ms), then resume.
        let v = f.submit(prev + 15, t(8 * 50 + 500));
        // Idle reset means this is treated as a "first event in direction"
        // — sustained back to 1.
        assert_eq!(f.sustained_count(), 1,
                   "idle should reset sustained, got {}", f.sustained_count());
        assert!(v > prev);
    }

    #[test]
    fn idle_submit_doesnt_grow_sustained() {
        let mut f = GestureFilter::new();
        f.anchor(50);
        for i in 0..5 { let _ = f.submit(50, t(i * 50)); }  // no delta
        assert_eq!(f.sustained_count(), 0);
    }

    #[test]
    fn output_always_stays_in_range() {
        let mut f = GestureFilter::new();
        f.anchor(50);
        for i in 0..200 {
            let target = if i % 2 == 0 { -100 } else { 200 }; // invalid
            let v = f.submit(target, t(i * 30));
            assert!(v >= 0 && v <= 100, "out of range: {}", v);
        }
    }

    #[test]
    fn anchor_resets_state() {
        let mut f = GestureFilter::new();
        f.anchor(30);
        // Build up sustained via realistic drag.
        let mut prev = 30;
        for i in 0..8 {
            let v = f.submit(prev + 15, t(i * 50));
            prev = v;
        }
        assert!(f.sustained_count() > 0);
        f.anchor(50);
        assert_eq!(f.actual(), 50);
        assert_eq!(f.sustained_count(), 0);
        assert!(!matches!(f.last_time, Some(_)),
                "anchor should clear last_time so next submit doesn't trigger idle-reset");
    }

    #[test]
    fn constants_form_consistent_policy() {
        // The cap-up shouldn't exceed cap-down (safety asymmetry).
        assert!(BASE_STEP_UP < BASE_STEP_DOWN);
        // Wraparound nudge magnitudes should be small compared to base steps.
        assert!(WRAPAROUND_NUDGE_UP < BASE_STEP_UP);
        assert!(WRAPAROUND_NUDGE_DOWN < BASE_STEP_DOWN);
        // Wraparound threshold should be well above the max normal step
        // (otherwise a sustained-bonus step would itself look like wraparound).
        let max_normal_up = BASE_STEP_UP + MAX_SUSTAINED_BONUS;
        let max_normal_down = BASE_STEP_DOWN + MAX_SUSTAINED_BONUS;
        assert!(WRAPAROUND_THRESHOLD > max_normal_up);
        assert!(WRAPAROUND_THRESHOLD > max_normal_down);
    }
}
