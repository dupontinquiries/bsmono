//! dwell.rs — stopping an autotuner from oscillating between decisions that are nearly tied.
//!
//! The engine has several measure-then-choose loops: which kernel per shape, whether the fast-decode
//! levers pay, which experts to pin, and — if hybrid placement lands — whether a model runs on the
//! accelerator or the host. All of them re-decide from live measurements, and live measurements move.
//!
//! ## A grace period alone does not fix oscillation
//!
//! The obvious guard is "do not switch again for five minutes". It bounds the *rate* of flapping and
//! not the flapping itself: two options within noise of each other will still alternate, just slowly,
//! and each switch throws away a warm cache to land somewhere no better.
//!
//! **The margin is what actually stops it.** A challenger must beat the incumbent by enough to pay for
//! the switch, and "enough" is a property of the switch cost rather than of the measurement. Both are
//! needed: the margin stops pointless switches, the dwell stops rapid ones when the margin is
//! genuinely crossed by a moving workload.
//!
//! ## Why an override exists
//!
//! A dwell that cannot be broken is a dwell that keeps a model resident after the device has told you
//! it cannot hold it. Pressure — an OOM, a device error, a watchdog kill — is not a preference to be
//! weighed against a timer, so it bypasses both guards.

use std::time::{Duration, Instant};

/// How much better a challenger must be, as a fraction, before switching is worth it.
///
/// Not a tuning knob so much as a statement about the switch: at 10% a challenger that is 5% faster
/// is declined, because the reload costs more than 5% of a session.
pub const DEFAULT_MARGIN: f32 = 0.10;

/// How long a choice holds before it may be revisited.
pub const DEFAULT_DWELL: Duration = Duration::from_secs(300);

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// Keep the incumbent; the dwell has not elapsed.
    HeldByDwell { remaining: Duration },
    /// Keep the incumbent; the challenger is not enough better.
    HeldByMargin { challenger_gain: i32 },
    /// Switch.
    Switch,
    /// Switch regardless of both guards.
    Forced,
}

impl Verdict {
    pub fn switches(&self) -> bool { matches!(self, Verdict::Switch | Verdict::Forced) }

    pub fn explain(&self) -> String {
        match self {
            Verdict::HeldByDwell { remaining } => format!(
                "holding for another {}s — a choice made this recently has not had time to prove \
itself, and re-deciding on a partial sample is how a tuner ends up alternating", remaining.as_secs()),
            Verdict::HeldByMargin { challenger_gain } => format!(
                "the alternative is only {challenger_gain}% better, which does not pay for the \
switch — a reload costs more than that, so taking it would be a net loss dressed as an improvement"),
            Verdict::Switch => "the alternative is enough better to pay for the switch".to_string(),
            Verdict::Forced =>
                "switching under pressure — an OOM or device error is not a preference to be weighed \
against a timer".to_string(),
        }
    }
}

/// One autotuned choice with hysteresis.
#[derive(Clone, Debug)]
pub struct Dwelling {
    since: Instant,
    dwell: Duration,
    margin: f32,
    switches: u32,
}

impl Dwelling {
    pub fn new(dwell: Duration, margin: f32) -> Self {
        Dwelling { since: Instant::now(), dwell, margin, switches: 0 }
    }

    pub fn switches_taken(&self) -> u32 { self.switches }

    /// Should the incumbent be replaced?
    ///
    /// Scores are "higher is better" — tokens/sec, hit rate, whatever the loop measures. `pressure`
    /// bypasses both guards.
    pub fn consider(&mut self, incumbent: f32, challenger: f32, pressure: bool) -> Verdict {
        if pressure {
            self.since = Instant::now();
            self.switches += 1;
            return Verdict::Forced;
        }
        let elapsed = self.since.elapsed();
        if elapsed < self.dwell {
            return Verdict::HeldByDwell { remaining: self.dwell - elapsed };
        }
        // A zero or negative incumbent has no meaningful ratio, so any positive challenger wins —
        // otherwise a loop that has never produced a score can never start.
        let gain = if incumbent > 0.0 { (challenger - incumbent) / incumbent } else if challenger > 0.0 { 1.0 } else { 0.0 };
        if gain <= self.margin {
            return Verdict::HeldByMargin { challenger_gain: (gain * 100.0).round() as i32 };
        }
        self.since = Instant::now();
        self.switches += 1;
        Verdict::Switch
    }

    /// Reset the clock without switching — for when the workload changes enough that the current
    /// measurement is about a different thing.
    pub fn restart_dwell(&mut self) { self.since = Instant::now(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(margin: f32) -> Dwelling {
        let mut d = Dwelling::new(Duration::from_secs(300), margin);
        d.since = Instant::now() - Duration::from_secs(400);   // dwell already elapsed
        d
    }

    #[test]
    fn a_fresh_choice_is_held_by_the_DWELL_however_good_the_alternative() {
        // Re-deciding on a partial sample is how a tuner ends up alternating: the first seconds after
        // a switch are the least representative, because caches are cold.
        let mut d = Dwelling::new(Duration::from_secs(300), 0.10);
        let v = d.consider(1.0, 100.0, false);
        assert!(matches!(v, Verdict::HeldByDwell { .. }), "{v:?}");
        assert!(!v.switches());
    }

    #[test]
    fn a_NEARLY_TIED_alternative_is_declined_even_after_the_dwell() {
        // The case a grace period alone does not fix. Two options within noise still alternate, just
        // slowly, and each switch throws away a warm cache to land somewhere no better.
        let v = ready(0.10).consider(10.0, 10.5, false);
        assert!(matches!(v, Verdict::HeldByMargin { .. }), "{v:?}");
        assert!(v.explain().contains("net loss dressed as an improvement"));
    }

    #[test]
    fn a_clearly_better_alternative_is_taken() {
        let mut d = ready(0.10);
        assert_eq!(d.consider(10.0, 14.0, false), Verdict::Switch);
        assert_eq!(d.switches_taken(), 1);
    }

    #[test]
    fn PRESSURE_bypasses_both_guards() {
        // A dwell that cannot be broken keeps a model resident after the device has said it cannot
        // hold it. An OOM is not a preference to be weighed against a timer.
        let mut fresh = Dwelling::new(Duration::from_secs(300), 0.10);
        assert_eq!(fresh.consider(100.0, 1.0, true), Verdict::Forced);
    }

    #[test]
    fn taking_a_switch_RESTARTS_the_dwell() {
        // Without this the second switch is free and the guard only ever stops the first.
        let mut d = ready(0.10);
        assert_eq!(d.consider(10.0, 14.0, false), Verdict::Switch);
        assert!(matches!(d.consider(14.0, 40.0, false), Verdict::HeldByDwell { .. }));
    }

    #[test]
    fn a_WORSE_alternative_is_declined_with_a_negative_gain() {
        // Reported rather than silently rejected: a loop repeatedly proposing something worse is
        // itself a finding, and a bare "held" hides it.
        match ready(0.10).consider(10.0, 5.0, false) {
            Verdict::HeldByMargin { challenger_gain } => assert_eq!(challenger_gain, -50),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn a_loop_with_NO_incumbent_score_can_still_start() {
        // Otherwise a tuner that has never measured anything can never make its first choice, and the
        // margin locks in whatever the default was.
        assert_eq!(ready(0.10).consider(0.0, 5.0, false), Verdict::Switch);
        assert!(matches!(ready(0.10).consider(0.0, 0.0, false), Verdict::HeldByMargin { .. }));
    }

    #[test]
    fn the_defaults_are_five_minutes_and_ten_percent() {
        assert_eq!(DEFAULT_DWELL.as_secs(), 300);
        assert!((DEFAULT_MARGIN - 0.10).abs() < 1e-6);
    }
}
