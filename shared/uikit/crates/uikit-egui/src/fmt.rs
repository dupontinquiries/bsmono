//! fmt — the small text helpers every app was writing for itself.
//!
//! Promoted for the usual reason: written four times, and the fourth copy
//! **missed a rule the other three had**.
//!
//! `relative_time` existed in bo-suite's library and sources pages and in
//! snapclone's outbox and snap feed. Three of the four guarded against a
//! negative interval — a peer or device whose clock runs fast produces a
//! future timestamp, and "-4m ago" is the visible result. snapclone's outbox
//! did not have the guard, so it would have rendered exactly that. Nobody had
//! noticed, because you need a skewed clock to see it.
//!
//! That is the whole argument for promotion, and it is not "less code": it is
//! that a rule discovered once should not have to be discovered again by
//! everyone who needs it.
//!
//! ## The styles stay, because they are not the same request
//!
//! A snap tile has room for `3h` and a sent-items list wants `3h ago`; a
//! library row, which is read rather than glanced at, says `3 hours ago`. Those
//! are three genuinely different asks, so `Style` is a parameter rather than a
//! thing to unify. Only the arithmetic and the guard are shared.

/// How much room the caller has, and how the text will be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// `3h` — a badge or a tile caption, glanced at.
    Bare,
    /// `3h ago` — a list column.
    Ago,
    /// `3 hours ago` — prose, read rather than scanned.
    Verbose,
}

/// Seconds under a minute are "now"/"just now" rather than a count.
const MINUTE: i64 = 60;
const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;

/// Format an elapsed interval.
///
/// A NEGATIVE interval reads as "now", never as a negative count. Clocks skew:
/// a peer's timestamp can land in the future, a device can report one, and the
/// user should see a harmless "now" rather than "-4m ago", which looks like the
/// app is broken rather than the clock.
pub fn relative_time(secs: i64, style: Style) -> String {
    if secs < MINUTE {
        // Covers the negative case too, deliberately: it is the same answer.
        return match style {
            Style::Bare => if secs < 0 { "now".into() } else { format!("{secs}s") },
            _ => "just now".into(),
        };
    }
    let (n, unit) = if secs < HOUR { (secs / MINUTE, "minute") }
                    else if secs < DAY { (secs / HOUR, "hour") }
                    else { (secs / DAY, "day") };
    // The short forms take the unit's first letter, so the two stay in step if
    // a unit is ever added.
    let short = &unit[..1];
    match style {
        Style::Bare => format!("{n}{short}"),
        Style::Ago => format!("{n}{short} ago"),
        Style::Verbose => format!("{} ago", plural(n, unit)),
    }
}

/// `1 track` / `2 tracks`.
///
/// Written inline in eight modules across three apps before this existed. The
/// rule it carries is small and universally got right on the second try:
/// **zero is plural** — "0 tracks", not "0 track" — which reads wrong to
/// everyone the one time an app gets it backwards.
pub fn plural(n: i64, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

/// A count for a badge, capped so it cannot outgrow the thing it sits on.
///
/// Shared because two bars in one app disagreeing about what a large number
/// looks like is visible on a single screen. Zero returns `None`: a badge
/// saying "0" is telling you about nothing.
pub fn badge_count(n: u32, cap: u32) -> Option<String> {
    if n == 0 { return None; }
    Some(if n > cap { format!("{cap}+") } else { n.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_future_timestamp_never_reads_as_a_negative_count() {
        // THE rule three of the four copies had and the fourth did not.
        // snapclone's outbox would have rendered "-4m ago" on a skewed clock.
        for style in [Style::Bare, Style::Ago, Style::Verbose] {
            let out = relative_time(-240, style);
            assert!(!out.contains('-'), "{style:?} produced {out:?}");
        }
        assert_eq!(relative_time(-240, Style::Bare), "now");
        assert_eq!(relative_time(-240, Style::Ago), "just now");
    }

    #[test]
    fn the_three_styles_answer_three_different_questions() {
        // A tile has room for "3h"; a list column wants "3h ago"; prose reads
        // "3 hours ago". Not a thing to unify.
        assert_eq!(relative_time(3 * HOUR, Style::Bare), "3h");
        assert_eq!(relative_time(3 * HOUR, Style::Ago), "3h ago");
        assert_eq!(relative_time(3 * HOUR, Style::Verbose), "3 hours ago");
    }

    #[test]
    fn the_short_form_is_derived_from_the_unit_so_they_cannot_drift() {
        for (secs, bare, verbose) in [
            (90i64, "1m", "1 minute ago"),
            (2 * HOUR, "2h", "2 hours ago"),
            (5 * DAY, "5d", "5 days ago"),
        ] {
            assert_eq!(relative_time(secs, Style::Bare), bare);
            assert_eq!(relative_time(secs, Style::Verbose), verbose);
        }
    }

    #[test]
    fn under_a_minute_is_now_rather_than_a_count_of_minutes() {
        assert_eq!(relative_time(0, Style::Ago), "just now");
        assert_eq!(relative_time(59, Style::Ago), "just now");
        assert_eq!(relative_time(60, Style::Ago), "1m ago");
        // The bare style keeps seconds, because a tile has room and "5s" is
        // more useful than "just now" when watching something arrive.
        assert_eq!(relative_time(5, Style::Bare), "5s");
    }

    #[test]
    fn the_boundaries_land_where_the_units_change() {
        assert_eq!(relative_time(HOUR - 1, Style::Ago), "59m ago");
        assert_eq!(relative_time(HOUR, Style::Ago), "1h ago");
        assert_eq!(relative_time(DAY - 1, Style::Ago), "23h ago");
        assert_eq!(relative_time(DAY, Style::Ago), "1d ago");
    }

    #[test]
    fn zero_is_plural() {
        // Reads wrong to everyone the one time an app gets it backwards.
        assert_eq!(plural(0, "track"), "0 tracks");
        assert_eq!(plural(1, "track"), "1 track");
        assert_eq!(plural(2, "track"), "2 tracks");
    }

    #[test]
    fn a_zero_badge_is_absent_rather_than_a_zero() {
        // A badge saying "0" is telling you about nothing.
        assert_eq!(badge_count(0, 99), None);
        assert_eq!(badge_count(7, 99).as_deref(), Some("7"));
    }

    #[test]
    fn a_badge_caps_so_it_cannot_outgrow_what_it_sits_on() {
        assert_eq!(badge_count(99, 99).as_deref(), Some("99"));
        assert_eq!(badge_count(100, 99).as_deref(), Some("99+"));
        assert_eq!(badge_count(5000, 9).as_deref(), Some("9+"));
    }
}
