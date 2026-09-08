//! chat_fmt — turning numbers and raw text into the strings the transcript shows.
//!
//! Fourth cut of `chat.rs`. Five pure functions with one thing in common: each takes data and
//! returns something for a human to read, and none of them touches `Chat`.
//!
//! Their tests were already grouped — `time_stamp_tests` and `time_line_tests` — which is a
//! good sign that the grouping is real rather than one I invented to make the file shorter.
//!
//! `pub(super)`: `chat`'s own formatting, not a public API. A child module, so `use super::*`
//! reaches the types these borrow without re-importing each.

use super::*;

/// How long ago a turn happened, for the list — `3s ago`, `12m ago`, `2h ago`.
///
/// WHICH CLOCK, AND WHY IT MATTERS. `Msg` carries two: `born`, a monotonic `Instant`, and `at`,
/// unix seconds. `born` is the obvious choice and is wrong for the case that matters — it cannot
/// survive a save, so on a reopened conversation every turn is stamped at load time and a
/// three-day-old exchange reads as seconds old. A relative time that confidently lies about an
/// archived session is worse than no relative time.
///
/// So the wall clock wins when it exists, `born` covers the live session, and a turn from before
/// `at` was recorded gets NOTHING rather than a guess. That last case is real: the field was added
/// later and older saves carry `0`.
///
/// `now` is passed in rather than read here so the boundaries are testable.
pub(super) fn turn_age(at: u64, born: std::time::Instant, now_unix: u64) -> Option<String> {
    let d = if at > 0 {
        // A clock that has moved backwards (NTP step, a save from a machine set ahead) would
        // underflow into a nonsense duration; saturating_sub reports "0s" instead.
        std::time::Duration::from_secs(now_unix.saturating_sub(at))
    } else if born.elapsed() >= std::time::Duration::from_secs(1) {
        // No wall clock recorded. Only trust `born` once it has measured something: a freshly
        // loaded old message has an elapsed of ~0, which is exactly the lie above.
        born.elapsed()
    } else {
        return None;
    };
    Some(format!("{} ago", logscope_fan::sessiontime::compact(d)))
}

/// `YYYY-MM-DD HH:MM UTC` from unix seconds, without pulling a date crate into
/// a frontend that has none.
///
/// UTC rather than local: a transcript is a file that gets read on another
/// machine, and a bare local time with no offset is the one format that cannot
/// be corrected later.
pub(super) fn stamp_utc(secs: u64) -> String {
    if secs == 0 { return "unknown".into() }
    let days = secs / 86_400;
    let tod = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm), integer-only.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", tod / 3600, (tod % 3600) / 60)
}

/// Wall-clock seconds. Falls back to 0 rather than panicking: a clock before
/// the epoch is a broken machine, not a reason to lose the message.
pub(super) fn wall_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0)
}

pub(super) fn wrap_label(text: &str, cols: usize, max_lines: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut truncated = false;
    'outer: for word in text.split_whitespace() {
        let mut word = word;
        // hard-split words longer than a line
        while word.chars().count() > cols {
            if !cur.is_empty() { lines.push(std::mem::take(&mut cur)); if lines.len() >= max_lines { truncated = true; break 'outer; } }
            let head: String = word.chars().take(cols).collect();
            lines.push(head);
            if lines.len() >= max_lines { truncated = true; break 'outer; }
            let skip = cols; word = &word[word.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(word.len())..];
        }
        if cur.is_empty() { cur = word.to_string(); }
        else if cur.chars().count() + 1 + word.chars().count() <= cols { cur.push(' '); cur.push_str(word); }
        else { lines.push(std::mem::take(&mut cur)); if lines.len() >= max_lines { truncated = true; break; } cur = word.to_string(); }
    }
    if !truncated && !cur.is_empty() && lines.len() < max_lines { lines.push(cur); }
    else if !cur.is_empty() { truncated = true; }
    if truncated {
        if let Some(last) = lines.last_mut() {
            while last.chars().count() >= cols { last.pop(); }
            last.push('…');
        }
    }
    lines
}

/// Summarize the LATEST activity from an in-flight assistant reply for a temporary progress line —
/// so a long or quiet turn (a coordinator "add more blocks" run that thinks before emitting visible
/// prose) still shows what it's doing. Pure + heuristic (no second model): prefers the most recent
/// structural signal in the tail — a ⏸ STEP marker, a markdown heading, a `// file:` path, a fenced
/// block's language, or a `- `/`* ` bullet — else the last non-trivial sentence, trimmed to ~64 chars.
/// Returns None if there's nothing meaningful yet (pure whitespace / still inside an empty think block).
pub(super) fn summarize_progress(text: &str) -> Option<String> {
    let clip = |s: &str| -> String {
        let s = s.trim();
        let mut out: String = s.chars().take(64).collect();
        if s.chars().count() > 64 { out.push('…'); }
        out
    };
    for line in text.lines().rev() {
        let l = line.trim();
        if l.is_empty() { continue; }
        if (l.contains('⏸') || l.starts_with("STEP")) && l.contains("STEP") {
            if let Some((_, rest)) = l.split_once("STEP") { return Some(clip(&format!("step{rest}"))); }
        }
        if let Some(h) = l.strip_prefix('#') {
            return Some(clip(h.trim_start_matches('#').trim()));
        }
        if let Some(idx) = l.find("// file:").or_else(|| l.find("file:")) {
            return Some(clip(&format!("writing {}", l[idx..].trim_start_matches("// ").trim_start_matches("file:").trim())));
        }
        if let Some(lang) = l.strip_prefix("```") {
            let lang = lang.trim();
            if !lang.is_empty() { return Some(clip(&format!("writing {lang}"))); }
        }
        if let Some(item) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")) {
            return Some(clip(item));
        }
        if l.len() > 3 && l != "```" {
            return Some(clip(l));
        }
    }
    None
}
