//! pattern — the pure half of haptic feedback: given a named vibration
//! pattern and how much time has elapsed since it started, is the motor
//! currently on, and is the pattern finished? No sysfs, no device, no
//! `std::thread::sleep` — a caller drives this from whatever clock it
//! already has (a redraw loop's own delta time, same idea
//! `light_kit_bridge::next_fraction` uses: fed a reading, not reading a
//! clock itself).
//!
//! ## Why this exists at all, not just "write brightness, sleep, write 0"
//!
//! The naive version blocks the calling thread for the pattern's whole
//! duration — fine for a one-shot fire-and-forget call, wrong for anything
//! that needs to keep rendering (an egui redraw loop) or cancel a pattern
//! partway through (a notification that gets dismissed mid-buzz shouldn't
//! keep vibrating). `PatternPlayer::state_at(elapsed_ms)` is a pure
//! function of elapsed time — poll it every frame, same as
//! `IdleSession::poll`/`LayerSurface::poll` elsewhere in this fleet, and
//! stopping early is just "stop polling and turn the motor off", not a
//! separate cancellation mechanism to build.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pulse {
    pub on_ms: u32,
    pub off_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub pulses: Vec<Pulse>,
}

impl Pattern {
    /// A single brief tap — the default "something happened" acknowledgment.
    pub fn tap() -> Pattern {
        Pattern { pulses: vec![Pulse { on_ms: 20, off_ms: 0 }] }
    }

    /// Two short pulses — a deliberate confirmation, distinct from a single
    /// tap so the two are never confused by feel alone (the same reasoning
    /// a physical button click vs. double-click needs a real gap between
    /// pulses to read as two events rather than one longer one).
    pub fn confirm() -> Pattern {
        Pattern { pulses: vec![Pulse { on_ms: 25, off_ms: 80 }, Pulse { on_ms: 25, off_ms: 0 }] }
    }

    /// One long, unmistakable buzz — an error or a failed action, distinct
    /// in feel from either success pattern above without needing to be
    /// louder, just longer.
    pub fn error() -> Pattern {
        Pattern { pulses: vec![Pulse { on_ms: 200, off_ms: 0 }] }
    }

    /// Three quick pulses — a landing notification, or a `Scope::Always`
    /// voice intent firing while the app that owns it isn't focused (the
    /// exact case this whole module exists for: confirming an action
    /// happened without requiring the screen to be on to see it).
    pub fn notification() -> Pattern {
        Pattern {
            pulses: vec![
                Pulse { on_ms: 15, off_ms: 60 },
                Pulse { on_ms: 15, off_ms: 60 },
                Pulse { on_ms: 15, off_ms: 0 },
            ],
        }
    }

    pub fn total_duration_ms(&self) -> u32 {
        self.pulses.iter().map(|p| p.on_ms + p.off_ms).sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotorState {
    On,
    Off,
    Finished,
}

/// Drives a `Pattern` from elapsed time. Stateless by design — `state_at`
/// takes elapsed time as a parameter rather than tracking a start time
/// itself, so a caller can use whatever clock it already has (matching
/// `light_kit_bridge`'s own reasoning for staying stateless where possible)
/// and so this is trivially testable with plain numbers.
pub struct PatternPlayer;

impl PatternPlayer {
    /// Which state the motor should be in at `elapsed_ms` since the
    /// pattern started. `Finished` once elapsed time passes the pattern's
    /// total duration — a caller polling past that point keeps getting
    /// `Finished`, not an error or a wrap-around repeat; looping a pattern
    /// is a caller decision (call this again with `elapsed_ms` reset to 0),
    /// not something this function assumes.
    pub fn state_at(pattern: &Pattern, elapsed_ms: u32) -> MotorState {
        let mut cursor = 0u32;
        for pulse in &pattern.pulses {
            if elapsed_ms < cursor + pulse.on_ms {
                return MotorState::On;
            }
            cursor += pulse.on_ms;
            if elapsed_ms < cursor + pulse.off_ms {
                return MotorState::Off;
            }
            cursor += pulse.off_ms;
        }
        MotorState::Finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_is_on_for_its_whole_short_duration_then_finished() {
        let p = Pattern::tap();
        assert_eq!(PatternPlayer::state_at(&p, 0), MotorState::On);
        assert_eq!(PatternPlayer::state_at(&p, 19), MotorState::On);
        assert_eq!(PatternPlayer::state_at(&p, 20), MotorState::Finished);
        assert_eq!(PatternPlayer::state_at(&p, 1000), MotorState::Finished, "stays Finished, doesn't wrap or error");
    }

    #[test]
    fn confirm_has_a_real_gap_between_its_two_pulses() {
        let p = Pattern::confirm();
        assert_eq!(PatternPlayer::state_at(&p, 10), MotorState::On, "first pulse");
        assert_eq!(PatternPlayer::state_at(&p, 25), MotorState::Off, "the gap");
        assert_eq!(PatternPlayer::state_at(&p, 60), MotorState::Off, "still the gap");
        assert_eq!(PatternPlayer::state_at(&p, 105), MotorState::On, "second pulse");
        assert_eq!(PatternPlayer::state_at(&p, 130), MotorState::Finished);
    }

    #[test]
    fn notification_is_three_distinct_pulses_not_one_long_buzz() {
        let p = Pattern::notification();
        let on_windows =
            (0..=200).filter(|&t| PatternPlayer::state_at(&p, t) == MotorState::On).count();
        let off_windows =
            (0..=200).filter(|&t| PatternPlayer::state_at(&p, t) == MotorState::Off).count();
        assert!(on_windows > 0 && off_windows > 0, "must actually alternate, not be on or off the whole time");
    }

    #[test]
    fn error_is_a_single_uninterrupted_buzz() {
        let p = Pattern::error();
        for t in 0..200 {
            assert_eq!(PatternPlayer::state_at(&p, t), MotorState::On, "no gap partway through at t={t}");
        }
        assert_eq!(PatternPlayer::state_at(&p, 200), MotorState::Finished);
    }

    #[test]
    fn total_duration_matches_the_sum_of_every_pulse_and_gap() {
        let p = Pattern { pulses: vec![Pulse { on_ms: 10, off_ms: 5 }, Pulse { on_ms: 20, off_ms: 0 }] };
        assert_eq!(p.total_duration_ms(), 35);
        assert_eq!(PatternPlayer::state_at(&p, 34), MotorState::On, "the last millisecond of the last pulse");
        assert_eq!(PatternPlayer::state_at(&p, 35), MotorState::Finished);
    }

    #[test]
    fn an_empty_pattern_is_immediately_finished() {
        let p = Pattern { pulses: vec![] };
        assert_eq!(PatternPlayer::state_at(&p, 0), MotorState::Finished);
    }
}
