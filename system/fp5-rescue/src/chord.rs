//! The rescue chord: hold POWER, tap VOLUME-DOWN three times.
//!
//! Pure: key events in, "open the console" out. Deliberately hard to hit by accident (a pocket
//! presses one key at a time and rarely three taps of one while another is held) and easy to hit on
//! purpose with gloves on. It has to work when the shell is dead, so it lives in its own process
//! and does not share a line of code with tideshell's key handling.
//!
//! Rules, each pinned by a test:
//! * The taps only count while power is HELD (down and not yet released).
//! * Three taps, each within [`TAP_GAP_MS`] of the one before.
//! * The whole chord must finish within [`WINDOW_MS`] of power going down, so holding power for a
//!   minute in a pocket cannot slowly collect stray taps.
//! * Releasing power, or a pause, starts it over. After it fires it is quiet for [`COOLDOWN_MS`].

pub const TAPS: usize = 3;
pub const TAP_GAP_MS: u64 = 1200;
pub const WINDOW_MS: u64 = 5000;
pub const COOLDOWN_MS: u64 = 3000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Power,
    VolumeDown,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub key: Key,
    pub down: bool,
    pub t_ms: u64,
}

#[derive(Debug, Default, Clone)]
pub struct Chord {
    power_down_at: Option<u64>,
    taps: usize,
    last_tap: u64,
    quiet_until: u64,
}

impl Chord {
    /// Feed one event; true exactly when the chord completes.
    pub fn feed(&mut self, e: Event) -> bool {
        match (e.key, e.down) {
            (Key::Power, true) => {
                self.power_down_at = Some(e.t_ms);
                self.taps = 0;
            }
            (Key::Power, false) => {
                self.power_down_at = None;
                self.taps = 0;
            }
            (Key::VolumeDown, true) => {
                let Some(since) = self.power_down_at else { return false };
                if e.t_ms < self.quiet_until {
                    return false;
                }
                if e.t_ms.saturating_sub(since) > WINDOW_MS {
                    self.power_down_at = None; // too late: this hold is spent
                    self.taps = 0;
                    return false;
                }
                if self.taps > 0 && e.t_ms.saturating_sub(self.last_tap) > TAP_GAP_MS {
                    self.taps = 0; // a pause: this tap is the first again
                }
                self.taps += 1;
                self.last_tap = e.t_ms;
                if self.taps == TAPS {
                    self.taps = 0;
                    self.quiet_until = e.t_ms + COOLDOWN_MS;
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(key: Key, down: bool, t_ms: u64) -> Event {
        Event { key, down, t_ms }
    }

    fn run(c: &mut Chord, evs: &[Event]) -> usize {
        evs.iter().filter(|e| c.feed(**e)).count()
    }

    fn taps(c: &mut Chord, at: &[u64]) -> usize {
        at.iter().map(|&t| c.feed(ev(Key::VolumeDown, true, t)) as usize).sum()
    }

    #[test]
    fn hold_power_and_three_taps_fires_once() {
        let mut c = Chord::default();
        c.feed(ev(Key::Power, true, 0));
        assert_eq!(taps(&mut c, &[400, 800, 1200]), 1);
    }

    #[test]
    fn taps_without_power_held_do_nothing() {
        let mut c = Chord::default();
        assert_eq!(taps(&mut c, &[100, 200, 300, 400, 500]), 0);
    }

    #[test]
    fn releasing_power_starts_it_over() {
        let mut c = Chord::default();
        c.feed(ev(Key::Power, true, 0));
        taps(&mut c, &[300, 600]);
        c.feed(ev(Key::Power, false, 700));
        c.feed(ev(Key::Power, true, 800));
        assert_eq!(taps(&mut c, &[900]), 0, "one tap after the restart, not the third");
        assert_eq!(taps(&mut c, &[1000, 1100]), 1);
    }

    #[test]
    fn a_pause_between_taps_restarts_the_count() {
        let mut c = Chord::default();
        c.feed(ev(Key::Power, true, 0));
        assert_eq!(taps(&mut c, &[200, 400, 400 + TAP_GAP_MS + 1]), 0, "the late tap is a first tap");
        assert_eq!(taps(&mut c, &[400 + TAP_GAP_MS + 300, 400 + TAP_GAP_MS + 600]), 1);
    }

    #[test]
    fn a_long_hold_cannot_collect_taps_over_time() {
        let mut c = Chord::default();
        c.feed(ev(Key::Power, true, 0));
        assert_eq!(taps(&mut c, &[WINDOW_MS + 100, WINDOW_MS + 300, WINDOW_MS + 500]), 0);
        // and the spent hold stays spent until power is released and pressed again
        assert_eq!(taps(&mut c, &[WINDOW_MS + 700, WINDOW_MS + 900, WINDOW_MS + 1100]), 0);
    }

    #[test]
    fn it_is_quiet_after_firing_then_works_again() {
        let mut c = Chord::default();
        c.feed(ev(Key::Power, true, 0));
        assert_eq!(taps(&mut c, &[100, 200, 300]), 1);
        assert_eq!(taps(&mut c, &[400, 500, 600]), 0, "inside the cooldown");
        c.feed(ev(Key::Power, false, 650));
        c.feed(ev(Key::Power, true, 100_000));
        assert_eq!(taps(&mut c, &[100_100, 100_200, 100_300]), 1);
    }

    #[test]
    fn other_keys_and_releases_of_volume_are_ignored() {
        let mut c = Chord::default();
        let evs = [
            ev(Key::Power, true, 0),
            ev(Key::VolumeDown, true, 100),
            ev(Key::VolumeDown, false, 150),
            ev(Key::Other, true, 200),
            ev(Key::VolumeDown, true, 300),
            ev(Key::VolumeDown, false, 350),
            ev(Key::VolumeDown, true, 500),
        ];
        assert_eq!(run(&mut c, &evs), 1);
    }
}
