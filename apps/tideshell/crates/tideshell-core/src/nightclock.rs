//! Bedside mode: during sleeping hours (bsettings: `sleep.hours`), the screen
//! stays OFF and a dim clock pulses once a minute, then the panel goes dark
//! again. OLED care is the point: lit pixels age, a static bright clock
//! burns in, and a dark room does not need 400 nits.
//!
//! * The pulse lights the panel for [`PULSE_MS`] at each new minute (the
//!   time only changes then), drawing a dim clock on black: on an OLED,
//!   black pixels are off, so the pulse lights a few hundred pixels.
//! * Every pulse moves the clock ([`shift`]), so no pixel carries the same
//!   digit edge night after night.
//! * The triple tap and the power key still wake the phone fully, from a
//!   pulse or from dark; the pulse never unlocks anything.
//! * The cover shown on a lit screen inside the window is dimmed too
//!   ([`night_alpha`]).
//!
//! The window arrives as minutes after midnight (settings-kit's SleepPrefs);
//! this module only needs "inside or not", so it takes that as a bool.

use crate::Millis;

/// How long each pulse lights the panel.
pub const PULSE_MS: Millis = 2_500;
/// The clock's opacity at night, pulsing or on the cover.
pub const NIGHT_ALPHA: f32 = 0.38;
/// Largest pixel-shift offset, in points.
pub const SHIFT_PT: f32 = 18.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    On,
    Off,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NightClock {
    pulse_until: Option<Millis>,
    /// Minute of day of the last pulse, so each minute pulses once.
    last_minute: Option<u16>,
    /// Pulses this session (for the trace / doctor).
    pub pulses: u32,
}

impl NightClock {
    pub fn is_pulsing(&self) -> bool {
        self.pulse_until.is_some()
    }

    /// One step. `asleep`: the Sleep model is Off (dark, touches captured).
    /// `window`: inside sleeping hours with pulsing on. Returns a panel
    /// request when one is needed.
    pub fn tick(&mut self, now: Millis, minute_of_day: u16, window: bool, asleep: bool) -> Option<Panel> {
        if let Some(until) = self.pulse_until {
            if !asleep {
                // Woken (triple tap / power key) mid-pulse: the wake owns the panel now.
                self.pulse_until = None;
                return None;
            }
            if now >= until || !window {
                self.pulse_until = None;
                return Some(Panel::Off);
            }
            return None;
        }
        if asleep && window && self.last_minute != Some(minute_of_day) {
            // The first minute seen asleep only records itself: going to sleep
            // should not flash the clock straight back on.
            let first = self.last_minute.is_none();
            self.last_minute = Some(minute_of_day);
            if !first {
                self.pulse_until = Some(now + PULSE_MS);
                self.pulses += 1;
                return Some(Panel::On);
            }
        }
        if !asleep {
            self.last_minute = None;
        }
        None
    }

    /// How long the loop may sleep before this needs a tick: to the end of a
    /// pulse, or to the next minute boundary (`ms_into_minute` of wall time).
    pub fn next_wake_ms(&self, now: Millis, ms_into_minute: u64, window: bool) -> Option<u64> {
        if let Some(until) = self.pulse_until {
            return Some(until.saturating_sub(now).max(1));
        }
        // A little past the boundary, so the minute has changed when we look.
        window.then(|| 60_000u64.saturating_sub(ms_into_minute.min(59_999)) + 50)
    }
}

/// Pixel shift for the clock at `minute_of_day`: a nine-position walk over a
/// 3x3 grid, visiting every cell before repeating, so consecutive minutes
/// land on different pixels.
pub fn shift(minute_of_day: u16) -> (f32, f32) {
    const ORDER: [(i8, i8); 9] = [(0, 0), (1, 1), (-1, 0), (0, -1), (1, -1), (-1, 1), (1, 0), (0, 1), (-1, -1)];
    let (x, y) = ORDER[(minute_of_day % 9) as usize];
    (x as f32 * SHIFT_PT, y as f32 * SHIFT_PT)
}

/// Opacity multiplier for the cover clock: dim inside sleeping hours.
pub fn night_alpha(in_window: bool) -> f32 {
    if in_window {
        NIGHT_ALPHA
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pulse_per_minute_then_dark_again_and_none_on_falling_asleep() {
        let mut n = NightClock::default();
        assert_eq!(n.tick(0, 100, true, true), None, "the minute we fell asleep: no flash");
        assert_eq!(n.tick(30_000, 100, true, true), None);
        assert_eq!(n.tick(60_010, 101, true, true), Some(Panel::On));
        assert!(n.is_pulsing());
        assert_eq!(n.tick(61_000, 101, true, true), None);
        assert_eq!(n.tick(60_010 + PULSE_MS, 101, true, true), Some(Panel::Off));
        assert_eq!(n.tick(90_000, 101, true, true), None, "same minute: once");
        assert_eq!(n.tick(120_010, 102, true, true), Some(Panel::On));
        assert_eq!(n.pulses, 2);
    }

    #[test]
    fn outside_the_window_or_awake_nothing_pulses_and_a_wake_takes_over() {
        let mut n = NightClock::default();
        n.tick(0, 100, false, true);
        assert_eq!(n.tick(60_000, 101, false, true), None, "daytime: dark stays dark");
        let mut n = NightClock::default();
        n.tick(0, 100, true, false);
        assert_eq!(n.tick(60_000, 101, true, false), None, "awake: the screen is on anyway");
        let mut n = NightClock::default();
        n.tick(0, 100, true, true);
        n.tick(60_000, 101, true, true);
        assert_eq!(n.tick(60_500, 101, true, false), None, "triple tap mid-pulse: no Off fights the wake");
        assert!(!n.is_pulsing());
        // The window closing mid-pulse turns it off.
        let mut n = NightClock::default();
        n.tick(0, 419, true, true);
        n.tick(60_000, 420, true, true);
        assert_eq!(n.tick(60_100, 420, false, true), Some(Panel::Off));
    }

    #[test]
    fn the_loop_wakes_at_the_minute_or_the_pulse_end() {
        let mut n = NightClock::default();
        assert_eq!(n.next_wake_ms(0, 45_000, true), Some(15_050));
        assert_eq!(n.next_wake_ms(0, 45_000, false), None);
        n.tick(0, 1, true, true);
        n.tick(60_000, 2, true, true);
        assert_eq!(n.next_wake_ms(61_000, 1_000, true), Some(PULSE_MS - 1_000));
    }

    #[test]
    fn the_shift_visits_all_nine_cells_and_consecutive_minutes_differ() {
        let cells: std::collections::HashSet<(i32, i32)> = (0..9u16).map(|m| {
            let (x, y) = shift(m);
            (x as i32, y as i32)
        }).collect();
        assert_eq!(cells.len(), 9);
        assert!((0..2000u16).all(|m| shift(m) != shift(m + 1)));
        assert!(night_alpha(true) < 0.5 && night_alpha(false) == 1.0);
    }
}
