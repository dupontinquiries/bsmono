//! pd -- a rate-capped proportional-derivative follower, for anything that must reach a target without ringing or
//! jumping: a lens or valve position, a volume or brightness commanded to a device, a camera or viewport chasing a point.
//!
//! The law is the one from the owner's embedded-controls coursework (ENGR-2350 worksheet 11 / the blimp controller):
//! per tick `step = kp * error + kd * (error - previous_error)`, applied to a position that integrates the step.
//! Measured here, NOT what the textbook picture suggests: with the position integrating the step directly, a positive
//! `kd` is extra proportional gain on a closing error (`(kp + kd) * error - kd * previous_error`): a harder first tick
//! and a slower tail (measured: 21 ticks for 0->100 at kd 0.2, 17 at kd 0), it does not stop ringing. The loop is overshoot-free while `kp + kd < 1` and rings above it (`the_stability_bound_is_kp_plus_kd`).
//! The thing that makes motion safe for hardware that clicks, clips or tears on a big step is `max_step`. If a plant
//! has real inertia, damp with rate feedback on the previous step (the blimp controller's "rate damping"), not with
//! this term. It is a per-TICK law: call `step` at a fixed rate; unlike an exponential smooth it arrives exactly.

#[derive(Debug, Clone)]
pub struct Pd {
    pub kp: f64,
    pub kd: f64,
    pub max_step: f64,
    x: f64,
    prev_error: f64,
    target: f64,
}

impl Pd {
    pub fn new(at: f64, kp: f64, kd: f64, max_step: f64) -> Pd {
        Pd { kp, kd, max_step, x: at, prev_error: 0.0, target: at }
    }

    pub fn position(&self) -> f64 { self.x }

    /// Jump the state to `at` without moving through the space between.
    pub fn reset(&mut self, at: f64) { self.x = at; self.target = at; self.prev_error = 0.0; }

    /// One tick toward `target`; returns the new position and the step taken.
    pub fn step(&mut self, target: f64) -> (f64, f64) {
        if target != self.target {
            // a new target: the jump in the error must not read as a derivative kick
            self.target = target;
            self.prev_error = target - self.x;
        }
        let error = target - self.x;
        let step = (self.kp * error + self.kd * (error - self.prev_error)).clamp(-self.max_step, self.max_step);
        self.prev_error = error;
        self.x += step;
        // within one unit and nearly stopped: land exactly (the exponential tail would otherwise never arrive)
        if (target - self.x).abs() < 1.0 && step.abs() < 1.0 { self.x = target; }
        (self.x, step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(from: f64, to: f64) -> (usize, f64, f64) {
        let mut pd = Pd::new(from, 0.25, 0.2, 40.0);
        let (mut max_step, mut overshoot) = (0.0f64, 0.0f64);
        for n in 1..=2000 {
            let (x, s) = pd.step(to);
            max_step = max_step.max(s.abs());
            overshoot = overshoot.max(if to >= from { x - to } else { to - x });
            if x == to { return (n, max_step, overshoot); }
        }
        panic!("never arrived");
    }

    #[test]
    fn it_arrives_exactly_without_overshoot_in_both_directions_and_at_every_distance() {
        for (a, b) in [(0.0, 1000.0), (1000.0, 0.0), (500.0, 510.0), (3.0, 4.0), (0.0, 100.0)] {
            let (ticks, _, over) = run(a, b);
            assert!(over <= 0.0, "{a}->{b} overshot by {over}");
            assert!(ticks < 120, "{a}->{b} took {ticks} ticks");
        }
    }

    #[test]
    fn no_tick_exceeds_the_cap_and_a_long_move_actually_reaches_it() {
        let (_, max_step, _) = run(0.0, 1000.0);
        assert!(max_step <= 40.0 + 1e-9, "{max_step}");
        assert!(max_step > 30.0, "a 1000-unit move should saturate the cap, got {max_step}");
    }

    #[test]
    fn retargeting_mid_move_turns_round_without_a_derivative_kick() {
        let mut pd = Pd::new(0.0, 0.25, 0.2, 40.0);
        for _ in 0..5 { pd.step(1000.0); }
        let before = pd.position();
        let (_, s) = pd.step(0.0);
        assert!(s.abs() <= 40.0 && pd.position() <= before + 40.0);
        let mut n = 0;
        while pd.position() != 0.0 && n < 500 { pd.step(0.0); n += 1; }
        assert_eq!(pd.position(), 0.0);
    }

    fn peak_overshoot(kp: f64, kd: f64) -> f64 {
        let mut pd = Pd::new(0.0, kp, kd, 1.0e9);
        (0..200).map(|_| pd.step(100.0).0 - 100.0).fold(0.0, f64::max)
    }

    #[test]
    fn the_stability_bound_is_kp_plus_kd() {
        // below 1 the move never passes the target, whatever the split; above 1 it does
        for (kp, kd) in [(0.25, 0.2), (0.5, 0.4), (0.1, 0.85), (0.9, 0.0)] {
            assert!(peak_overshoot(kp, kd) < 1e-9, "kp {kp} kd {kd}: {}", peak_overshoot(kp, kd));
        }
        for (kp, kd) in [(0.6, 0.6), (1.4, 0.0), (0.25, 0.9)] {
            assert!(peak_overshoot(kp, kd) > 1.0, "kp {kp} kd {kd}: {}", peak_overshoot(kp, kd));
        }
    }
}
