//! Small shared utilities: hashing, time, and human-readable formatting.
//! Kept dependency-free (no chrono) — everything is done with plain
//! SystemTime/UNIX arithmetic, which is all this app needs.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch, saturating to 0 on clock weirdness.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Whole days since the Unix epoch (UTC), used for streaks/heatmaps.
pub fn days_since_epoch(unix: i64) -> i64 {
    unix.div_euclid(86_400)
}

pub fn today_epoch_day() -> i64 {
    days_since_epoch(now_unix())
}

/// FNV-1a 64-bit hash, used for stable ids from bytes.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn fnv1a64_str(s: &str) -> u64 {
    fnv1a64(s.as_bytes())
}

/// Format a byte count as a short human string, e.g. "3.4 MB".
pub fn humanize_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n == 0 {
        return "0 B".to_string();
    }
    let mut val = n as f64;
    let mut idx = 0usize;
    while val >= 1024.0 && idx < UNITS.len() - 1 {
        val /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{n} B")
    } else {
        format!("{val:.1} {}", UNITS[idx])
    }
}

/// Format a duration in seconds as "Hh Mm", "Mm Ss", or "Ss".
pub fn humanize_duration_secs(secs: i64) -> String {
    let secs = secs.max(0);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Format a unix timestamp difference relative to now as "3d ago", "just now", etc.
pub fn relative_time(unix: i64) -> String {
    let now = now_unix();
    let delta = (now - unix).max(0);
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else if delta < 86_400 * 30 {
        format!("{}d ago", delta / 86_400)
    } else if delta < 86_400 * 365 {
        format!("{}mo ago", delta / (86_400 * 30))
    } else {
        format!("{}y ago", delta / (86_400 * 365))
    }
}

/// Clamp + format a fraction (0.0..=1.0) as a whole-number percent string.
pub fn percent_string(frac: f32) -> String {
    format!("{}%", (frac.clamp(0.0, 1.0) * 100.0).round() as i32)
}

/// Turn an arbitrary string into a filesystem/URL-safe slug (lowercase,
/// ascii-alnum runs joined by single hyphens). Used for anchors and export
/// filenames.
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = true; // avoid leading dash
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "item".to_string()
    } else {
        out
    }
}

/// Truncate a string to at most `max_chars` characters, appending an
/// ellipsis if truncated. Operates on char boundaries, not bytes.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// Simple ordinal suffix helper (1st, 2nd, 3rd, 4th...), used in stats UI.
pub fn ordinal(n: i64) -> String {
    let rem100 = n.abs() % 100;
    let rem10 = n.abs() % 10;
    let suffix = if (11..=13).contains(&rem100) {
        "th"
    } else {
        match rem10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_stable() {
        assert_eq!(fnv1a64_str("hello"), fnv1a64_str("hello"));
        assert_ne!(fnv1a64_str("hello"), fnv1a64_str("world"));
    }

    #[test]
    fn bytes_human() {
        assert_eq!(humanize_bytes(0), "0 B");
        assert_eq!(humanize_bytes(512), "512 B");
        assert_eq!(humanize_bytes(2048), "2.0 KB");
    }

    #[test]
    fn slug_basic() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("  ---  "), "item");
        assert_eq!(slugify("Chapter 3.2: The Rise"), "chapter-3-2-the-rise");
    }

    #[test]
    fn ordinal_suffixes() {
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(21), "21st");
    }

    #[test]
    fn truncate_respects_chars() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        let t = truncate_chars("hello world", 5);
        assert_eq!(t.chars().count(), 5);
    }
}
