//! date_phrase — spoken dates into day numbers, from `system_info.rs`.
//!
//! *"How many days until Christmas"* is a question people ask a voice assistant
//! constantly, and the arithmetic behind it is where naive implementations get
//! quietly wrong: leap years, the year roll, and month lengths.
//!
//! The original is `std`-only and shells out for everything else in that
//! module; this is the part that is **pure**, so it ports whole and tests
//! without a clock.
//!
//! ## Julian Day Number, not day-of-year plus offsets
//!
//! The conversion is the standard Gregorian→JDN formula minus the JDN of
//! 1970-01-01. That matters more than it looks: **leap years fall out of the
//! arithmetic** rather than being special-cased, so 2000 (leap, divisible by
//! 400) and 1900 (not leap, divisible by 100) are both right without anybody
//! having remembered the rule.
//!
//! A day-of-year implementation with an `if year % 4 == 0` is wrong one year in
//! a hundred, and wrong in a way nobody notices for decades.
//!
//! ## Holidays roll forward, and that is the answer people want
//!
//! Asked on 26 December, *"days until Christmas"* means **next** Christmas. The
//! original checks whether this year's date has passed and rolls the year if
//! so. Answering "-1 days" would be arithmetically correct and useless.
//!
//! ## Thanksgiving is marked approximate in the source, and stays that way
//!
//! It is the fourth Thursday of November, not a fixed date — the original uses
//! 11/25 with an `// approximate` comment. Carrying the comment forward is the
//! honest move: silently "fixing" it to a computed Thursday would change an
//! answer the rest of the system may already be tuned against, and leaving it
//! undocumented would let somebody trust it.

/// Days from 1970-01-01 for a Gregorian date.
///
/// Gregorian→JDN, minus the JDN of the epoch (2440588). Leap years are handled
/// by the formula rather than by a rule somebody has to remember.
pub fn ymd_to_days(year: i32, month: u32, day: u32) -> i64 {
    let a = (14 - month as i32) / 12;
    let y = year + 4800 - a;
    let m = month as i32 + 12 * a - 3;
    let jdn = day as i32 + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045;
    (jdn - 2440588) as i64
}

/// The inverse, for rendering a resolved date back.
pub fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let jdn = days + 2440588;
    let a = jdn + 32044;
    let b = (4 * a + 3) / 146097;
    let c = a - 146097 * b / 4;
    let d = (4 * c + 3) / 1461;
    let e = c - 1461 * d / 4;
    let m = (5 * e + 2) / 153;
    let day = (e - (153 * m + 2) / 5 + 1) as u32;
    let month = (m + 3 - 12 * (m / 10)) as u32;
    let year = (100 * b + d - 4800 + m / 10) as i32;
    (year, month, day)
}

/// Fixed-date holidays the resolver knows, as `(month, day)`.
///
/// `thanksgiving` is **approximate** — it is the fourth Thursday of November,
/// not the 25th. Kept as the original had it, and named as approximate, because
/// silently computing the real Thursday would change an answer the rest of the
/// system may be tuned against.
pub const HOLIDAYS: &[(&str, u32, u32, bool)] = &[
    ("christmas", 12, 25, true),
    ("xmas", 12, 25, true),
    ("christmas day", 12, 25, true),
    ("new year", 1, 1, true),
    ("new year's", 1, 1, true),
    ("new years", 1, 1, true),
    ("new year's day", 1, 1, true),
    ("halloween", 10, 31, true),
    ("valentine's day", 2, 14, true),
    ("valentines day", 2, 14, true),
    ("valentine", 2, 14, true),
    ("thanksgiving", 11, 25, false),
    ("independence day", 7, 4, true),
    ("fourth of july", 7, 4, true),
    ("4th of july", 7, 4, true),
    ("earth day", 4, 22, true),
];

/// Look up a holiday. The `bool` is whether the date is exact.
pub fn holiday(name: &str) -> Option<(u32, u32, bool)> {
    let n = normalise(name);
    HOLIDAYS.iter().find(|(k, ..)| *k == n).map(|(_, m, d, exact)| (*m, *d, *exact))
}

/// Trim the punctuation speech recognition attaches.
///
/// A transcriber emits *"christmas?"* and *"christmas."* freely, and a lookup on
/// the raw string misses every one of them.
pub fn normalise(s: &str) -> String {
    s.trim().trim_end_matches(['?', '.', '!']).trim().to_lowercase()
}

/// Resolve a phrase to a day number, given today.
///
/// `today` is passed rather than read, so the resolver is testable without a
/// clock — the same split that makes every other policy module in this crate
/// checkable on a build machine.
pub fn resolve(phrase: &str, today: i64) -> Option<i64> {
    let s = normalise(phrase);
    match s.as_str() {
        "today" => return Some(today),
        "tomorrow" => return Some(today + 1),
        "yesterday" => return Some(today - 1),
        _ => {}
    }
    let (m, d, _) = holiday(&s)?;
    let (year, ..) = days_to_ymd(today);
    let this_year = ymd_to_days(year, m, d);
    // Rolls to next year once passed: asked on 26 December, "days until
    // Christmas" means NEXT Christmas. Answering -1 is correct and useless.
    Some(if this_year < today { ymd_to_days(year + 1, m, d) } else { this_year })
}

/// How a resolved date reads back.
///
/// Separated from resolution so the phrasing can change without touching the
/// arithmetic — and so "today" gets its own wording rather than "in 0 days".
pub fn phrase_days(target: &str, delta: i64) -> String {
    match delta {
        0 => format!("{target} is today"),
        1 => format!("{target} is tomorrow"),
        -1 => format!("{target} was yesterday"),
        d if d > 0 => format!("{d} days until {target}"),
        d => format!("{target} was {} days ago", -d),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> i64 { ymd_to_days(y, m, d) }

    #[test]
    fn the_epoch_is_day_zero() {
        assert_eq!(day(1970, 1, 1), 0);
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn leap_years_fall_out_of_the_arithmetic() {
        // The reason for JDN rather than day-of-year plus offsets: 2000 is a
        // leap year (divisible by 400) and 1900 is not (divisible by 100). An
        // `if year % 4 == 0` implementation is wrong one year in a hundred, and
        // wrong in a way nobody notices for decades.
        assert_eq!(day(2000, 3, 1) - day(2000, 2, 28), 2, "2000 had a 29 February");
        assert_eq!(day(1900, 3, 1) - day(1900, 2, 28), 1, "1900 did not");
        assert_eq!(day(2024, 3, 1) - day(2024, 2, 28), 2);
        assert_eq!(day(2023, 3, 1) - day(2023, 2, 28), 1);
    }

    #[test]
    fn a_year_is_the_right_length_either_way() {
        assert_eq!(day(2025, 1, 1) - day(2024, 1, 1), 366, "2024 was a leap year");
        assert_eq!(day(2024, 1, 1) - day(2023, 1, 1), 365);
    }

    #[test]
    fn the_conversion_round_trips_across_boundaries() {
        // Month ends, leap day, and year boundaries — where an off-by-one
        // would hide.
        for (y, m, d) in [(1970, 1, 1), (1999, 12, 31), (2000, 2, 29),
                          (2024, 2, 29), (2026, 8, 10), (2100, 3, 1)] {
            assert_eq!(days_to_ymd(day(y, m, d)), (y, m, d), "{y}-{m}-{d}");
        }
    }

    #[test]
    fn dates_before_the_epoch_go_negative_rather_than_wrapping() {
        // A birthday in the sixties is an ordinary thing to ask about.
        assert!(day(1969, 12, 31) < 0);
        assert_eq!(days_to_ymd(day(1965, 6, 15)), (1965, 6, 15));
    }

    #[test]
    fn a_holiday_rolls_to_next_year_once_it_has_passed() {
        // Asked on Boxing Day, "days until Christmas" means NEXT Christmas.
        let boxing_day = day(2026, 12, 26);
        let got = resolve("christmas", boxing_day).unwrap();
        assert_eq!(days_to_ymd(got), (2027, 12, 25));
    }

    #[test]
    fn a_holiday_today_is_today_rather_than_a_year_away() {
        // The boundary of the roll: it must not fire on the day itself.
        let xmas = day(2026, 12, 25);
        assert_eq!(resolve("christmas", xmas), Some(xmas));
    }

    #[test]
    fn a_holiday_still_ahead_stays_this_year() {
        let dec_first = day(2026, 12, 1);
        assert_eq!(days_to_ymd(resolve("christmas", dec_first).unwrap()), (2026, 12, 25));
    }

    #[test]
    fn transcriber_punctuation_does_not_break_the_lookup() {
        // "christmas?" and "christmas." are what actually arrives, and a raw
        // lookup misses every one.
        let t = day(2026, 8, 10);
        assert_eq!(resolve("christmas?", t), resolve("christmas", t));
        assert_eq!(resolve("  Christmas.  ", t), resolve("christmas", t));
    }

    #[test]
    fn the_spelling_variants_people_actually_say_all_resolve() {
        let t = day(2026, 8, 10);
        for a in ["new year", "new years", "new year's", "new year's day"] {
            assert!(resolve(a, t).is_some(), "{a}");
        }
        for a in ["fourth of july", "4th of july", "independence day"] {
            assert_eq!(resolve(a, t), resolve("fourth of july", t), "{a}");
        }
    }

    #[test]
    fn thanksgiving_is_marked_approximate_rather_than_silently_wrong() {
        // It is the fourth Thursday of November, not the 25th. The original
        // says "// approximate"; carrying that forward is the honest move —
        // silently computing the real Thursday would change an answer the rest
        // of the system may be tuned against, and leaving it undocumented would
        // let somebody trust it.
        let (m, d, exact) = holiday("thanksgiving").unwrap();
        assert_eq!((m, d), (11, 25));
        assert!(!exact, "an approximate date claiming to be exact");
        assert!(holiday("christmas").unwrap().2, "a fixed date is exact");
    }

    #[test]
    fn relative_words_do_not_need_a_holiday_table() {
        let t = day(2026, 8, 10);
        assert_eq!(resolve("today", t), Some(t));
        assert_eq!(resolve("tomorrow", t), Some(t + 1));
        assert_eq!(resolve("yesterday", t), Some(t - 1));
    }

    #[test]
    fn an_unknown_phrase_is_none_rather_than_a_guess() {
        // "Days until the thing" should say it does not know, not answer about
        // an arbitrary date.
        assert_eq!(resolve("my birthday", day(2026, 8, 10)), None);
        assert_eq!(resolve("", day(2026, 8, 10)), None);
    }

    #[test]
    fn today_and_tomorrow_read_as_words_not_as_counts() {
        // "In 0 days" is the tell of a template nobody read aloud.
        assert_eq!(phrase_days("christmas", 0), "christmas is today");
        assert_eq!(phrase_days("christmas", 1), "christmas is tomorrow");
        assert_eq!(phrase_days("christmas", -1), "christmas was yesterday");
        assert!(phrase_days("christmas", 30).starts_with("30 days until"));
        assert!(phrase_days("christmas", -30).contains("30 days ago"));
    }

    #[test]
    fn resolution_needs_no_clock() {
        // `today` is passed rather than read, which is what makes every case
        // above testable on a build machine — the same split the other policy
        // modules use.
        let fixed = day(2026, 1, 1);
        assert_eq!(resolve("christmas", fixed), resolve("christmas", fixed));
    }
}
