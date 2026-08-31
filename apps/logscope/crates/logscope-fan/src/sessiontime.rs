//! sessiontime — how long this has been going, and how long was asked for.
//!
//! ## The gap
//!
//! An operator typed "keep working for the next hour" and the model had no way
//! to read it. Nothing in the conversation carries a clock:
//!
//! * `linearize::Turn` is `{ id, role, text }` — no timestamp;
//! * the markdown export writes `## You` / `## Assistant` with no time, so a
//!   session reopened tomorrow looks identical to one from a minute ago;
//! * the only `elapsed()` calls in the engine are performance diagnostics that
//!   never reach a prompt.
//!
//! So "for the next hour" is a phrase the model can only pattern-match against,
//! and "keep working" three turns later carries no signal that fifty minutes
//! have passed. A long-horizon mode that cannot tell whether it has been
//! running for four minutes or four hours is guessing about the one thing the
//! operator just told it.
//!
//! ## What this is
//!
//! Pure and CLOCK-INJECTED: every function takes `now` rather than reading one.
//! That is what makes a four-hour session a unit test, and it is the same shape
//! [`crate::capability`] and beatspace's `StopPolicy` use.
//!
//! Two jobs, deliberately separate:
//!
//! 1. [`parse_work_limit`] reads a duration out of what the operator said.
//! 2. [`TimeContext::render`] turns elapsed time and that limit into a line
//!    for the prompt.
//!
//! It does NOT enforce anything. A model told it has nine minutes left may still
//! choose to start something large, and a module that silently truncated the
//! work would be a worse failure than one that reports honestly.

use std::time::Duration;

/// Seconds since the unix epoch. A plain number rather than `SystemTime` so
/// tests can name a moment, and so a transcript can store one.
pub type Stamp = u64;

/// Read a work limit out of an instruction.
///
/// Only what an operator actually types. "For the next hour" and "for 30
/// minutes" are the common forms; open-ended phrases like "keep working" return
/// `None`, which means no limit rather than zero — the distinction matters,
/// since a zero limit would read as "stop now".
///
/// Deliberately NOT a general date parser. "Until Tuesday" is not something a
/// chat turn can honour, and half-parsing it would produce a confident wrong
/// deadline where returning `None` produces an honest absence.
pub fn parse_work_limit(text: &str) -> Option<Duration> {
    let t = text.to_ascii_lowercase();
    // Word forms first: "an hour" has no digits to find.
    // LONGEST PHRASE FIRST. "half an hour" CONTAINS "an hour", so checking the
    // shorter one earlier returns 3600 for a request for 1800 — caught by the
    // test, and the kind of ordering bug that reads as correct in isolation.
    for (phrase, secs) in [
        ("half an hour", 1800u64), ("half hour", 1800),
        ("next few hours", 3 * 3600),
        ("next hour", 3600), ("an hour", 3600), ("one hour", 3600),
    ] {
        if t.contains(phrase) {
            return Some(Duration::from_secs(secs));
        }
    }
    // Numeric forms: "for 30 minutes", "2 hours", "90 min".
    let b: Vec<char> = t.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() { i += 1; continue }
        let s = i;
        while i < b.len() && b[i].is_ascii_digit() { i += 1 }
        let n: u64 = b[s..i].iter().collect::<String>().parse().ok()?;
        let rest: String = b[i..].iter().take(12).collect();
        let rest = rest.trim_start();
        let unit = if rest.starts_with("hour") || rest.starts_with("hr") || rest.starts_with('h') {
            Some(3600)
        } else if rest.starts_with("min") || rest.starts_with('m') {
            Some(60)
        } else {
            None
        };
        if let Some(u) = unit {
            // Guard the absurd: a "for 900 hours" is a typo or a joke, and a
            // deadline five weeks out is indistinguishable from none.
            let secs = n.saturating_mul(u);
            if secs > 0 && secs <= 24 * 3600 {
                return Some(Duration::from_secs(secs));
            }
            return None;
        }
    }
    None
}

/// What the model should be told about time.
#[derive(Debug, Clone)]
pub struct TimeContext {
    /// When the session's first turn happened.
    pub started: Stamp,
    /// Now.
    pub now: Stamp,
    /// Turns so far, user and assistant together.
    pub turns: usize,
    /// A limit the operator asked for, and when they asked.
    pub limit: Option<(Stamp, Duration)>,
}

impl TimeContext {
    pub fn elapsed(&self) -> Duration {
        Duration::from_secs(self.now.saturating_sub(self.started))
    }

    /// Time left against the operator's limit. `None` when none was asked for;
    /// `Some(ZERO)` when it has run out, which is a different thing.
    pub fn remaining(&self) -> Option<Duration> {
        let (asked, dur) = self.limit?;
        let deadline = asked.saturating_add(dur.as_secs());
        Some(Duration::from_secs(deadline.saturating_sub(self.now)))
    }

    /// One line for the prompt.
    ///
    /// Short on purpose. It is prepended to every turn, so a paragraph here
    /// costs context on every exchange for information that is usually just
    /// "still early".
    pub fn render(&self) -> String {
        let mut s = format!("Time: {} into this session, {} turn{}.",
            human(self.elapsed()), self.turns, if self.turns == 1 { "" } else { "s" });
        match self.remaining() {
            None => {}
            Some(d) if d.is_zero() => {
                // Said plainly rather than omitted. The operator asked for a
                // duration and it is gone; continuing is now a choice rather
                // than the instruction, and the model should know which.
                s.push_str(" The time you were asked to work for has RUN OUT — \
report where you are rather than starting something new.");
            }
            Some(d) => {
                s.push_str(&format!(" You were asked to keep working for {} more; \
about {} remain.", human(self.limit.unwrap().1), human(d)));
            }
        }
        s
    }
}

/// "47 minutes", "2 hours 5 minutes", "40 seconds".
///
/// No decimals: a model reading "1.7 hours" has to do arithmetic to act on it,
/// and the arithmetic is the part it should not have to do.
pub fn human(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 90 { return format!("{secs} seconds") }
    let mins = secs / 60;
    if mins < 60 { return format!("{mins} minutes") }
    let (h, m) = (mins / 60, mins % 60);
    if m == 0 { format!("{h} hour{}", if h == 1 { "" } else { "s" }) }
    else { format!("{h} hour{} {m} minutes", if h == 1 { "" } else { "s" }) }
}

/// The same duration for a DENSE surface: `3s`, `12m`, `2h`, `4d`.
///
/// Beside `human` rather than in the UI crate, because one module should decide how a duration
/// reads. They are different renderings of one idea: `human` is for something spoken or budgeted
/// ("you have 2 hours 5 minutes"), this is for a timestamp in a list where the row is forty pixels
/// tall and the words would not fit.
///
/// Rounds DOWN, always. A message written 119 seconds ago is "1m", not "2m" — a relative time that
/// rounds up claims something happened longer ago than it did, and in a log that is the direction
/// that misleads.
pub fn compact(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 { return format!("{secs}s"); }
    let mins = secs / 60;
    if mins < 60 { return format!("{mins}m"); }
    let hours = mins / 60;
    if hours < 24 { return format!("{hours}h"); }
    format!("{}d", hours / 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phrase_from_the_transcript_parses() {
        // The literal thing the operator typed.
        assert_eq!(parse_work_limit("keep working for the next hour"),
                   Some(Duration::from_secs(3600)));
    }

    #[test]
    fn common_forms_parse() {
        for (text, secs) in [
            ("for 30 minutes", 1800u64),
            ("work for 2 hours", 7200),
            ("90 min please", 5400),
            ("half an hour", 1800),
            ("give it an hour", 3600),
        ] {
            assert_eq!(parse_work_limit(text), Some(Duration::from_secs(secs)), "{text}");
        }
    }

    /// No limit and a zero limit are different, and conflating them is how a
    /// model decides it has already run out of time it was never given.
    #[test]
    fn open_ended_instructions_yield_no_limit_rather_than_zero() {
        for text in ["keep working", "carry on", "continue", "doesn't compile keep working"] {
            assert_eq!(parse_work_limit(text), None, "{text}");
        }
    }

    #[test]
    fn absurd_and_unhonourable_durations_are_refused() {
        assert_eq!(parse_work_limit("for 900 hours"), None, "beyond a day is not a deadline");
        assert_eq!(parse_work_limit("until tuesday"), None, "not a duration a turn can honour");
        assert_eq!(parse_work_limit("for 0 minutes"), None);
    }

    #[test]
    fn a_number_with_no_unit_is_not_a_duration() {
        // "write 3 functions" must not become a three-something deadline.
        assert_eq!(parse_work_limit("write 3 functions"), None);
        assert_eq!(parse_work_limit("crossterm 0.24"), None);
    }

    fn ctx(elapsed: u64, turns: usize, limit: Option<(u64, u64)>) -> TimeContext {
        TimeContext { started: 1_000_000, now: 1_000_000 + elapsed, turns,
                      limit: limit.map(|(at, d)| (1_000_000 + at, Duration::from_secs(d))) }
    }

    #[test]
    fn early_in_a_session_the_line_is_short() {
        let s = ctx(120, 3, None).render();
        assert!(s.contains("2 minutes") && s.contains("3 turns"), "{s}");
        assert!(!s.contains("asked"), "no limit was given, so none should be mentioned: {s}");
    }

    #[test]
    fn a_live_limit_reports_what_remains() {
        // Asked for an hour at t=0; forty minutes gone.
        let s = ctx(2400, 8, Some((0, 3600))).render();
        assert!(s.contains("20 minutes"), "should report the remainder: {s}");
    }

    /// The case the transcript would have hit.
    #[test]
    fn an_expired_limit_says_so_instead_of_going_quiet() {
        let c = ctx(4200, 20, Some((0, 3600)));
        assert_eq!(c.remaining(), Some(Duration::ZERO));
        let s = c.render();
        assert!(s.contains("RUN OUT"), "an expired limit must be stated: {s}");
        assert!(s.contains("report where you are"),
                "and it must say what to do instead of starting new work: {s}");
    }

    #[test]
    fn the_deadline_runs_from_when_it_was_asked_not_from_session_start() {
        // Session an hour old; the operator asks for 30 more minutes NOW.
        let c = ctx(3600, 12, Some((3600, 1800)));
        assert_eq!(c.remaining(), Some(Duration::from_secs(1800)),
                   "asking for '30 more minutes' an hour in must mean 30 from now");
    }

    #[test]
    fn the_compact_form_fits_a_list_row() {
        assert_eq!(compact(Duration::from_secs(0)), "0s");
        assert_eq!(compact(Duration::from_secs(3)), "3s");
        assert_eq!(compact(Duration::from_secs(59)), "59s");
        assert_eq!(compact(Duration::from_secs(60)), "1m");
        assert_eq!(compact(Duration::from_secs(12 * 60)), "12m");
        assert_eq!(compact(Duration::from_secs(3600)), "1h");
        assert_eq!(compact(Duration::from_secs(2 * 3600)), "2h");
        assert_eq!(compact(Duration::from_secs(47 * 3600)), "1d");
        assert_eq!(compact(Duration::from_secs(30 * 24 * 3600)), "30d");
    }

    /// A relative time that rounds UP says something happened longer ago than it did, which in a
    /// log is the direction that misleads.
    #[test]
    fn the_compact_form_rounds_down() {
        assert_eq!(compact(Duration::from_secs(119)), "1m", "not 2m");
        assert_eq!(compact(Duration::from_secs(59 * 60 + 59)), "59m", "not 1h");
        assert_eq!(compact(Duration::from_secs(23 * 3600 + 3599)), "23h", "not 1d");
    }

    /// Both renderings answer the same question and must not disagree about which unit applies.
    #[test]
    fn compact_and_human_agree_about_the_unit() {
        for secs in [0u64, 30, 89, 120, 600, 3600, 7500] {
            let d = Duration::from_secs(secs);
            let (c, h) = (compact(d), human(d));
            let unit_c = c.chars().last().unwrap();
            let word = h.split_whitespace().nth(1).unwrap_or("");
            // `human` switches to minutes at 90s where `compact` switches at 60s — a deliberate
            // difference (a spoken "90 seconds" is fine, a column reading "90s" is not), so agree
            // only where both are past their own boundary.
            if secs >= 90 || secs < 60 {
                let expect = if word.starts_with("second") { 's' }
                    else if word.starts_with("minute") { 'm' } else { 'h' };
                assert_eq!(unit_c, expect, "{secs}s: compact {c:?} vs human {h:?}");
            }
        }
    }

    #[test]
    fn durations_read_without_arithmetic() {
        assert_eq!(human(Duration::from_secs(40)), "40 seconds");
        assert_eq!(human(Duration::from_secs(600)), "10 minutes");
        assert_eq!(human(Duration::from_secs(3600)), "1 hour");
        assert_eq!(human(Duration::from_secs(7500)), "2 hours 5 minutes");
    }
}
