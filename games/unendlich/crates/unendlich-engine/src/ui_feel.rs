//! The motion and interaction logic behind a deliberate, weighty skill screen —
//! the deterministic state a renderer reads to produce the "feel" (subtle camera
//! drift, parallax layers, frosted-background focus ramp, slot shimmer, and a
//! tap-and-hold commit). The actual blur / bokeh / shader work lives in the
//! render layer; this is purely the timing and math that drives it, so the look
//! is a thin renderer over tested state.
//!
//! Presentation-side, so it uses `f32` (no rollback/determinism constraint like
//! the sim) — but it's still deterministic given the same inputs.

/// Tap-and-hold to commit an action (e.g. spend a skill point). Holding fills a
/// progress 0..1; reaching the threshold commits once; releasing early cancels.
/// The deliberate hold is what makes investing feel weighty rather than a flick.
#[derive(Debug, Clone)]
pub struct HoldToCommit {
    hold_secs: f32,
    elapsed: f32,
    committed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HoldEvent {
    /// Not being held.
    Idle,
    /// Held, filling — carries progress 0..1 for the radial fill.
    Charging(f32),
    /// Crossed the threshold this update (fires exactly once).
    Committed,
    /// Released before the threshold — nothing happens.
    Cancelled,
}

impl HoldToCommit {
    pub fn new(hold_secs: f32) -> Self {
        Self { hold_secs: hold_secs.max(0.0001), elapsed: 0.0, committed: false }
    }

    pub fn progress(&self) -> f32 {
        (self.elapsed / self.hold_secs).clamp(0.0, 1.0)
    }

    pub fn reset(&mut self) {
        self.elapsed = 0.0;
        self.committed = false;
    }

    /// Advance with the current button state. Returns what happened this update.
    pub fn update(&mut self, held: bool, dt: f32) -> HoldEvent {
        if held {
            if self.committed {
                return HoldEvent::Idle; // already fired; wait for release
            }
            self.elapsed += dt.max(0.0);
            if self.elapsed >= self.hold_secs {
                self.committed = true;
                return HoldEvent::Committed;
            }
            HoldEvent::Charging(self.progress())
        } else {
            let was_charging = self.elapsed > 0.0 && !self.committed;
            self.reset();
            if was_charging {
                HoldEvent::Cancelled
            } else {
                HoldEvent::Idle
            }
        }
    }
}

/// Slot shimmer: a sweep position 0..1 that the renderer maps to a highlight
/// travelling across the slot. A plain looping sawtooth of wall-clock time.
pub fn shimmer_phase(time_secs: f32, period_secs: f32) -> f32 {
    if period_secs <= 0.0 {
        return 0.0;
    }
    let t = time_secs.rem_euclid(period_secs);
    t / period_secs
}

/// Parallax offset for a layer at `depth` (0 = pinned foreground, larger = deeper
/// background) as the pointer moves off `center`. Background slides *opposite* the
/// pointer, so deeper layers lag more — the look-around depth cue. `strength`
/// scales the whole effect.
pub fn parallax_offset(
    pointer: (f32, f32),
    center: (f32, f32),
    depth: f32,
    strength: f32,
) -> (f32, f32) {
    let dx = (center.0 - pointer.0) * depth * strength;
    let dy = (center.1 - pointer.1) * depth * strength;
    (dx, dy)
}

/// A value that eases smoothly toward a target — drives the frosted-background
/// focus ramp and subtle camera drift. Frame-rate-aware lerp; never overshoots.
#[derive(Debug, Clone, Copy)]
pub struct Eased {
    pub current: f32,
    pub target: f32,
    /// Approach rate (per second). Higher snaps faster.
    pub rate: f32,
}

impl Eased {
    pub fn new(value: f32, rate: f32) -> Self {
        Self { current: value, target: value, rate: rate.max(0.0) }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Step toward the target. Returns the new current value.
    pub fn approach(&mut self, dt: f32) -> f32 {
        let k = (self.rate * dt.max(0.0)).clamp(0.0, 1.0);
        self.current += (self.target - self.current) * k;
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_short_tap_never_commits() {
        let mut h = HoldToCommit::new(0.5);
        // Held for 0.3s total in small steps — under the 0.5s threshold.
        for _ in 0..3 {
            let e = h.update(true, 0.1);
            assert!(matches!(e, HoldEvent::Charging(_)));
        }
        assert!(h.progress() < 1.0);
    }

    #[test]
    fn holding_past_the_threshold_commits_once() {
        let mut h = HoldToCommit::new(0.3);
        match h.update(true, 0.2) {
            HoldEvent::Charging(p) => assert!((p - 0.6667).abs() < 1e-3, "progress {p}"),
            other => panic!("expected Charging, got {other:?}"),
        }
        assert_eq!(h.update(true, 0.2), HoldEvent::Committed, "crossed 0.3s");
        assert!(approx(h.progress(), 1.0));
        // Continuing to hold does not re-fire.
        assert_eq!(h.update(true, 0.2), HoldEvent::Idle);
    }

    #[test]
    fn releasing_early_cancels_and_resets() {
        let mut h = HoldToCommit::new(0.5);
        h.update(true, 0.2);
        assert_eq!(h.update(false, 0.0), HoldEvent::Cancelled);
        assert_eq!(h.progress(), 0.0, "progress reset after cancel");
    }

    #[test]
    fn idle_when_never_held() {
        let mut h = HoldToCommit::new(0.5);
        assert_eq!(h.update(false, 0.1), HoldEvent::Idle);
    }

    #[test]
    fn shimmer_sweeps_and_wraps() {
        assert!(approx(shimmer_phase(0.0, 2.0), 0.0));
        assert!(approx(shimmer_phase(1.0, 2.0), 0.5));
        assert!(approx(shimmer_phase(2.0, 2.0), 0.0), "wraps at the period");
        assert!(approx(shimmer_phase(2.5, 2.0), 0.25));
    }

    #[test]
    fn parallax_is_zero_at_center_and_opposes_the_pointer() {
        assert_eq!(parallax_offset((50.0, 50.0), (50.0, 50.0), 1.0, 1.0), (0.0, 0.0));
        // Pointer right of center → background slides left.
        let (dx, _) = parallax_offset((80.0, 50.0), (50.0, 50.0), 1.0, 1.0);
        assert!(dx < 0.0, "background moves opposite the pointer");
    }

    #[test]
    fn deeper_layers_parallax_more() {
        let near = parallax_offset((80.0, 50.0), (50.0, 50.0), 0.5, 1.0).0.abs();
        let far = parallax_offset((80.0, 50.0), (50.0, 50.0), 2.0, 1.0).0.abs();
        assert!(far > near, "deeper background shifts more: {far} > {near}");
    }

    #[test]
    fn easing_converges_without_overshoot() {
        let mut e = Eased::new(0.0, 4.0);
        e.set_target(1.0);
        let mut last = 0.0;
        for _ in 0..200 {
            let v = e.approach(0.016);
            assert!(v <= 1.0 + 1e-5, "never overshoots the target");
            assert!(v >= last - 1e-6, "monotonic toward target");
            last = v;
        }
        assert!(approx(e.current, 1.0), "reaches the target: {}", e.current);
    }
}
