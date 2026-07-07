//! `sim::dive` — a dragon's power dive.
//!
//! Folding the wings sheds drag area, so the dragon accelerates toward a much
//! higher terminal speed than in level flight. [`terminal_velocity`](crate::sim::terminal_velocity)
//! gives that cap (heavier ⇒ faster, more drag area ⇒ slower); this adds the two
//! missing pieces — how wing-tuck changes the drag area, and how speed
//! approaches the cap under drag — to turn the orphaned core into a usable dive.
//! It composes that core; it doesn't reimplement the physics. Pure and
//! deterministic.

use crate::sim::terminal_velocity;

const GRAVITY: f32 = 9.81;

/// Terminal dive speed for a body of `mass` whose spread-wing drag area is
/// `wing_drag_area`, tucked by `tuck01` in `[0,1]`. Full tuck keeps only 10 % of
/// the area (so terminal speed roughly triples), gliding keeps it all. Uses
/// `terminal_velocity::terminal`.
pub fn dive_terminal(mass: f32, wing_drag_area: f32, tuck01: f32) -> f32 {
    let tuck = tuck01.clamp(0.0, 1.0);
    let area = (wing_drag_area.max(0.0) * (1.0 - 0.9 * tuck)).max(1e-3);
    terminal_velocity::terminal(mass, area, GRAVITY)
}

/// One dive step: airspeed approaches `terminal` with a drag-limited exponential
/// (rate `k` per second), from either side — it accelerates toward the cap from
/// below and decays toward it from above (e.g. when the wings spread to pull
/// out). Never overshoots; finite and non-negative.
pub fn dive_step(speed: f32, terminal: f32, dt: f32, k: f32) -> f32 {
    let s = speed.max(0.0);
    let t = terminal.max(0.0);
    let a = (1.0 - (-(k.max(0.0) * dt.max(0.0))).exp()).clamp(0.0, 1.0);
    (s + (t - s) * a).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tucking_wings_raises_terminal_speed() {
        let glide = dive_terminal(60.0, 4.0, 0.0);
        let tuck = dive_terminal(60.0, 4.0, 1.0);
        assert!(tuck > glide, "a tucked dive reaches a higher terminal speed");
        // ~triple (area → 10%), within tolerance
        assert!(tuck > glide * 2.5 && tuck < glide * 3.6);
    }

    #[test]
    fn heavier_dragon_dives_faster() {
        assert!(dive_terminal(90.0, 4.0, 0.5) > dive_terminal(45.0, 4.0, 0.5));
    }

    #[test]
    fn speed_accelerates_toward_terminal_from_below() {
        let term = dive_terminal(60.0, 4.0, 1.0);
        let mut v = 0.0;
        let mut last = -1.0;
        for _ in 0..40 {
            v = dive_step(v, term, 0.1, 1.5);
            assert!(v >= last - 1e-4, "monotonically speeding up");
            assert!(v <= term + 1e-3, "never overshoots terminal");
            last = v;
        }
        assert!(v > 0.9 * term, "converges close to terminal after enough time");
    }

    #[test]
    fn pulling_out_decays_toward_lower_terminal() {
        // diving fast, then spread wings (low terminal) → speed bleeds down.
        let fast = dive_terminal(60.0, 4.0, 1.0);
        let slow = dive_terminal(60.0, 4.0, 0.0);
        let after = dive_step(fast, slow, 0.5, 1.5);
        assert!(after < fast && after > slow, "speed decays toward the spread-wing terminal");
    }

    #[test]
    fn faster_rate_approaches_quicker() {
        let term = 80.0;
        let slow = dive_step(0.0, term, 0.2, 0.5);
        let fast = dive_step(0.0, term, 0.2, 4.0);
        assert!(fast > slow, "a larger rate constant accelerates faster");
    }

    #[test]
    fn deterministic_and_finite() {
        assert_eq!(dive_step(10.0, 80.0, 0.1, 1.5), dive_step(10.0, 80.0, 0.1, 1.5));
        assert!(dive_terminal(60.0, 4.0, 0.3).is_finite());
    }
}
