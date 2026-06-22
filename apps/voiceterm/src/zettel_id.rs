//! v0.1.710: Zettelkasten note IDs. Two schemes the setup wizard offers:
//!
//! - **Timestamp** (`202606071430`) — Luhmann's digital descendants (Obsidian,
//!   The Archive). Always unique, sortable, no bookkeeping. Default.
//! - **Folgezettel** (`1`, `1a`, `1a1`, `2`) — Luhmann's analog scheme: a new
//!   note either continues a thread (append: `1a` → `1a1`) or branches beside
//!   it (`1a` → `1b`). Encodes structure IN the id. Power-user option.
//!
//! PURE + testable. Filename = `<id>` + optional ` <title-slug>`; this module
//! owns id generation + the "next id" logic, slug lives in workspace_wizard.

/// A timestamp id from broken-down local time parts (so the caller supplies
/// the clock — keeps this pure/testable). Format: YYYYMMDDHHMM.
pub fn timestamp_id(year: u32, month: u32, day: u32, hour: u32, min: u32) -> String {
    format!("{:04}{:02}{:02}{:02}{:02}", year, month, day, hour, min)
}

/// Broken-down UTC parts (year, month, day, hour, min, sec) from a unix
/// timestamp. Pure civil-date conversion (Howard Hinnant's algorithm) so the
/// timestamp id needs no chrono dependency. Caller adds any tz offset in
/// seconds before calling if local time is wanted.
pub fn civil_from_unix(unix_secs: i64) -> (u32, u32, u32, u32, u32, u32) {
    let days = unix_secs.div_euclid(86_400);
    let secs = unix_secs.rem_euclid(86_400);
    let (hour, min, sec) = ((secs / 3600) as u32, ((secs % 3600) / 60) as u32, (secs % 60) as u32);
    // days since 1970-01-01 → civil date
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = (y + if m <= 2 { 1 } else { 0 }) as u32;
    (year, m, d, hour, min, sec)
}

/// Convenience: a timestamp id directly from a unix timestamp (UTC).
pub fn timestamp_id_from_unix(unix_secs: i64) -> String {
    let (y, mo, d, h, mi, _s) = civil_from_unix(unix_secs);
    timestamp_id(y, mo, d, h, mi)
}

/// Folgezettel: the id that CONTINUES `id` (a child thread).
/// Alternates number↔letter by depth: `1` → `1a` → `1a1` → `1a1a`.
pub fn folge_child(id: &str) -> String {
    // last segment numeric? next child is a letter 'a'. else a digit '1'.
    if ends_with_digit(id) {
        format!("{}a", id)
    } else {
        format!("{}1", id)
    }
}

/// Folgezettel: the id that BRANCHES beside `id` (next sibling).
/// `1` → `2`; `1a` → `1b`; `1z` → `1aa` (carry); `1a1` → `1a2`.
pub fn folge_sibling(id: &str) -> String {
    let (head, tail) = split_last_segment(id);
    if tail.chars().all(|c| c.is_ascii_digit()) {
        let n: u64 = tail.parse().unwrap_or(0);
        format!("{}{}", head, n + 1)
    } else {
        format!("{}{}", head, incr_letters(&tail))
    }
}

/// The next top-level id given the existing top-level numeric ids (e.g. all
/// notes whose id is a bare number). Returns "1" for an empty kasten.
pub fn next_toplevel(existing_ids: &[String]) -> String {
    let max = existing_ids.iter()
        .filter(|s| s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty())
        .filter_map(|s| s.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    (max + 1).to_string()
}

/// Validate a folgezettel id: alternating runs of digits and lowercase letters,
/// starting with a digit, non-empty.
pub fn is_valid_folge(id: &str) -> bool {
    if id.is_empty() || !id.starts_with(|c: char| c.is_ascii_digit()) {
        return false;
    }
    let mut prev_digit: Option<bool> = None;
    let mut run_changes = 0;
    for c in id.chars() {
        let is_d = c.is_ascii_digit();
        let is_l = c.is_ascii_lowercase();
        if !is_d && !is_l { return false; }
        if prev_digit != Some(is_d) {
            if prev_digit.is_some() { run_changes += 1; }
            prev_digit = Some(is_d);
        }
    }
    let _ = run_changes;
    true
}

// ── helpers ──

fn ends_with_digit(s: &str) -> bool {
    s.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false)
}

/// Split off the trailing run of same-class chars (the last segment).
/// "1a1" → ("1a", "1"); "1a" → ("1", "a"); "12" → ("", "12").
fn split_last_segment(s: &str) -> (String, String) {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() { return (String::new(), String::new()); }
    let last_is_digit = chars[chars.len() - 1].is_ascii_digit();
    let mut i = chars.len();
    while i > 0 && chars[i - 1].is_ascii_digit() == last_is_digit {
        i -= 1;
    }
    (chars[..i].iter().collect(), chars[i..].iter().collect())
}

/// Increment a lowercase-letter run like a base-26 odometer: "a"→"b",
/// "z"→"aa", "az"→"ba".
fn incr_letters(s: &str) -> String {
    let mut chars: Vec<u8> = s.bytes().collect();
    let mut i = chars.len();
    loop {
        if i == 0 { chars.insert(0, b'a'); break; }
        i -= 1;
        if chars[i] == b'z' { chars[i] = b'a'; } // carry
        else { chars[i] += 1; break; }
    }
    String::from_utf8(chars).unwrap_or_else(|_| "a".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_format() {
        assert_eq!(timestamp_id(2026, 6, 7, 14, 30), "202606071430");
        assert_eq!(timestamp_id(2026, 12, 1, 9, 5), "202612010905");
    }

    #[test]
    fn civil_date_conversion() {
        // 2021-01-01 00:00:00 UTC = 1609459200
        assert_eq!(civil_from_unix(1609459200), (2021, 1, 1, 0, 0, 0));
        // 1970-01-01 00:00:00 = epoch
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        // 2024-02-29 (leap day) 12:34:56 UTC = 1709210096
        assert_eq!(civil_from_unix(1709210096), (2024, 2, 29, 12, 34, 56));
        // id from unix
        assert_eq!(timestamp_id_from_unix(1609459200), "202101010000");
    }

    #[test]
    fn folge_children_alternate() {
        assert_eq!(folge_child("1"), "1a");
        assert_eq!(folge_child("1a"), "1a1");
        assert_eq!(folge_child("1a1"), "1a1a");
        assert_eq!(folge_child("12"), "12a");
    }

    #[test]
    fn folge_siblings_increment() {
        assert_eq!(folge_sibling("1"), "2");
        assert_eq!(folge_sibling("1a"), "1b");
        assert_eq!(folge_sibling("1a1"), "1a2");
        assert_eq!(folge_sibling("9"), "10");
    }

    #[test]
    fn folge_sibling_letter_carry() {
        assert_eq!(folge_sibling("1z"), "1aa");
        assert_eq!(folge_sibling("1az"), "1ba");
    }

    #[test]
    fn next_toplevel_picks_max_plus_one() {
        assert_eq!(next_toplevel(&[]), "1");
        assert_eq!(next_toplevel(&["1".into(), "2".into(), "1a".into()]), "3");
        // ignores non-toplevel ids
        assert_eq!(next_toplevel(&["1a".into(), "1a1".into()]), "1");
        assert_eq!(next_toplevel(&["1".into(), "5".into(), "3".into()]), "6");
    }

    #[test]
    fn validates_folge() {
        assert!(is_valid_folge("1"));
        assert!(is_valid_folge("1a"));
        assert!(is_valid_folge("1a1b"));
        assert!(!is_valid_folge(""));
        assert!(!is_valid_folge("a1"));   // must start with digit
        assert!(!is_valid_folge("1-a"));  // invalid char
    }
}
