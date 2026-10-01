//! Morse unlock: a pattern of short and long power-key presses that dismisses the cover.
//!
//! Why the power key: the volume keys stay free for volume (and, in cold mode, steering), so the
//! phone stays pocketable; the power key already wakes the screen, and a pattern tapped on it is
//! something a gloved thumb can do without looking. A press shorter than [`DASH_MS`] is a dot,
//! from there up to [`SLEEP_MS`] a dash, and longer than that is not part of the code at all: it
//! puts the screen back to sleep (see the shell), so a pocket cannot hold the phone "in" the code.
//!
//! This is a cover, not security: the lock screen's cover mode has no PIN either. The pattern is
//! brute-force limited anyway: [`MAX_WRONG`] wrong sequences in a row lock the entry out for
//! [`LOCKOUT_MS`], and a sequence abandoned for [`GAP_MS`] starts over.
//!
//! Pure: durations in, an outcome out, `now` supplied.

use crate::Millis;

pub const DASH_MS: Millis = 350;
pub const SLEEP_MS: Millis = 1500;
pub const GAP_MS: Millis = 3000;
pub const MAX_WRONG: u32 = 5;
pub const LOCKOUT_MS: Millis = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sym {
    Dot,
    Dash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Not configured, locked out, or the press is too long to be a symbol.
    Ignored,
    /// A correct symbol; this many entered so far.
    Progress(usize),
    /// The whole pattern matched.
    Unlocked,
    /// A wrong symbol: the sequence starts over.
    Wrong,
}

/// `.` and `-` only (anything else, or fewer than 3 symbols, is "not configured": a one- or
/// two-press code is guessed by a pocket).
pub fn parse(pattern: &str) -> Option<Vec<Sym>> {
    let v: Option<Vec<Sym>> = pattern
        .trim()
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '.' => Some(Sym::Dot),
            '-' => Some(Sym::Dash),
            _ => None,
        })
        .collect();
    v.filter(|s| s.len() >= 3)
}

pub fn symbol_of(held_ms: Millis) -> Option<Sym> {
    if held_ms >= SLEEP_MS {
        None
    } else if held_ms >= DASH_MS {
        Some(Sym::Dash)
    } else {
        Some(Sym::Dot)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Morse {
    pattern: Vec<Sym>,
    entered: usize,
    last_at: Option<Millis>,
    wrong: u32,
    locked_until: Option<Millis>,
}

impl Morse {
    /// Replace the pattern (an empty or invalid one disables the unlock).
    pub fn set_pattern(&mut self, pattern: &str) {
        self.pattern = parse(pattern).unwrap_or_default();
        self.reset();
        self.wrong = 0;
        self.locked_until = None;
    }

    pub fn enabled(&self) -> bool {
        !self.pattern.is_empty()
    }

    pub fn entered(&self) -> usize {
        self.entered
    }

    pub fn len(&self) -> usize {
        self.pattern.len()
    }

    fn reset(&mut self) {
        self.entered = 0;
        self.last_at = None;
    }

    pub fn locked_out(&self, now: Millis) -> bool {
        self.locked_until.is_some_and(|t| now < t)
    }

    /// A completed press that lasted `held_ms`, released at `now`.
    pub fn press(&mut self, held_ms: Millis, now: Millis) -> Outcome {
        if !self.enabled() || self.locked_out(now) {
            return Outcome::Ignored;
        }
        let Some(sym) = symbol_of(held_ms) else { return Outcome::Ignored };
        if self.last_at.is_some_and(|t| now.saturating_sub(t) > GAP_MS) {
            self.entered = 0; // abandoned: start over, and this press is the first symbol
        }
        self.last_at = Some(now);
        if self.pattern[self.entered] != sym {
            self.reset();
            self.wrong += 1;
            if self.wrong >= MAX_WRONG {
                self.wrong = 0;
                self.locked_until = Some(now + LOCKOUT_MS);
            }
            return Outcome::Wrong;
        }
        self.entered += 1;
        if self.entered == self.pattern.len() {
            self.reset();
            self.wrong = 0;
            return Outcome::Unlocked;
        }
        Outcome::Progress(self.entered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str) -> Morse {
        let mut m = Morse::default();
        m.set_pattern(p);
        m
    }

    #[test]
    fn a_pattern_needs_at_least_three_symbols_of_dots_and_dashes() {
        assert!(parse("..-").is_some() && parse(" . - . - ").is_some());
        for bad in ["", ".", "..", "..x", "123"] {
            assert!(parse(bad).is_none(), "{bad:?} must not enable the unlock");
        }
        assert!(!m("..").enabled());
    }

    #[test]
    fn short_is_a_dot_long_a_dash_and_very_long_is_not_a_symbol() {
        assert_eq!(symbol_of(DASH_MS - 1), Some(Sym::Dot));
        assert_eq!(symbol_of(DASH_MS), Some(Sym::Dash));
        assert_eq!(symbol_of(SLEEP_MS - 1), Some(Sym::Dash));
        assert_eq!(symbol_of(SLEEP_MS), None);
    }

    #[test]
    fn the_whole_pattern_unlocks_and_restarts() {
        let mut k = m(".-.");
        assert_eq!(k.press(100, 0), Outcome::Progress(1));
        assert_eq!(k.press(600, 500), Outcome::Progress(2));
        assert_eq!(k.press(100, 1000), Outcome::Unlocked);
        assert_eq!(k.entered(), 0, "ready for the next lock");
    }

    #[test]
    fn a_wrong_symbol_starts_over_even_if_it_would_have_been_the_next_one_of_a_longer_try() {
        let mut k = m(".-.");
        k.press(100, 0);
        assert_eq!(k.press(100, 400), Outcome::Wrong, "dot where a dash was due");
        assert_eq!(k.entered(), 0);
        // and the right sequence works from scratch afterwards
        assert_eq!(k.press(100, 900), Outcome::Progress(1));
    }

    #[test]
    fn an_abandoned_sequence_times_out_and_the_late_press_counts_as_the_first_symbol() {
        let mut k = m(".-.");
        k.press(100, 0);
        assert_eq!(k.press(600, GAP_MS + 1), Outcome::Wrong, "a dash is not the FIRST symbol of .-.");
        let mut k = m(".-.");
        k.press(100, 0);
        assert_eq!(k.press(100, GAP_MS + 1), Outcome::Progress(1), "restarted: a dot is");
    }

    #[test]
    fn five_wrong_in_a_row_lock_it_out_and_the_right_code_is_refused_meanwhile() {
        let mut k = m("..-");
        for i in 0..MAX_WRONG as u64 {
            assert_eq!(k.press(600, i * 100), Outcome::Wrong);
        }
        let t = MAX_WRONG as u64 * 100;
        assert!(k.locked_out(t));
        for (i, held) in [100u64, 100, 600].into_iter().enumerate() {
            assert_eq!(k.press(held, t + i as u64 * 100), Outcome::Ignored);
        }
        let later = t + LOCKOUT_MS + 1;
        assert!(!k.locked_out(later));
        assert_eq!(k.press(100, later), Outcome::Progress(1));
    }

    #[test]
    fn a_success_clears_the_wrong_count() {
        let mut k = m("..-");
        for i in 0..(MAX_WRONG - 1) as u64 {
            k.press(600, i * 100);
        }
        k.press(100, 1000);
        k.press(100, 1100);
        assert_eq!(k.press(600, 1200), Outcome::Unlocked);
        for i in 0..(MAX_WRONG - 1) as u64 {
            assert_eq!(k.press(600, 2000 + i * 100), Outcome::Wrong);
        }
        assert!(!k.locked_out(3000), "four more wrongs after a success are not a lockout");
    }

    #[test]
    fn a_press_that_is_too_long_is_ignored_and_does_not_break_the_sequence() {
        let mut k = m(".-.");
        k.press(100, 0);
        assert_eq!(k.press(SLEEP_MS + 1, 500), Outcome::Ignored);
        assert_eq!(k.entered(), 1);
    }

    #[test]
    fn unconfigured_ignores_everything() {
        let mut k = Morse::default();
        assert_eq!(k.press(100, 0), Outcome::Ignored);
    }
}
