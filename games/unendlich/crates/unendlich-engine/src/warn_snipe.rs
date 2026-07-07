//! Warning sniper — opt-in tally of WARN/ERROR log records by call site.
//!
//! Motivation: warnings scroll past during a session and are easy to miss, but
//! each one is usually a little bug trying to get noticed (a CPU-fallback
//! mesh, an upload drop, a parity divergence). With `UNENDLICH_WARN_SNIPE=1`,
//! every WARN+ record is bucketed by `(target, normalized-message)` and a
//! compact summary is emitted in the shutdown run block, so each test run ends
//! with a list of exactly which warning sites fired and how often.
//!
//! Normalization collapses variable runs (coordinates, counts, hex) so that
//! e.g. "chunk (-4,2,-7) overflowed v=42781" and "chunk (1,0,3) overflowed
//! v=39912" land in the same bucket — you want the SITE, not every instance.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static ENABLED: OnceLock<bool> = OnceLock::new();
static TALLY: OnceLock<Mutex<HashMap<String, (u64, String)>>> = OnceLock::new();

/// True if `UNENDLICH_WARN_SNIPE` is set to a truthy value. Cached on first
/// call so the format closure stays cheap.
pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var("UNENDLICH_WARN_SNIPE")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
    })
}

/// Collapse variable substrings so instances of the same warning site bucket
/// together: digit runs (incl. signs/decimals) → `#`, hex → `0x#`.
fn normalize(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let bytes = msg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        // hex literal 0x....
        if c == '0' && i + 1 < bytes.len() && (bytes[i + 1] | 0x20) == b'x' {
            out.push_str("0x#");
            i += 2;
            while i < bytes.len() && (bytes[i] as char).is_ascii_hexdigit() { i += 1; }
            continue;
        }
        if c.is_ascii_digit() || ((c == '-' || c == '+' || c == '.')
            && i + 1 < bytes.len() && (bytes[i + 1] as char).is_ascii_digit())
        {
            out.push('#');
            i += 1;
            while i < bytes.len()
                && ((bytes[i] as char).is_ascii_digit() || bytes[i] == b'.' || bytes[i] == b'-')
            { i += 1; }
            // collapse a run into a single '#'
            while out.ends_with("##") { out.pop(); }
            continue;
        }
        out.push(c);
        i += 1;
    }
    // Cap the bucket key length so a giant message doesn't bloat the map.
    if out.len() > 120 { out.truncate(120); }
    out
}

/// Record one WARN/ERROR. No-op unless `enabled()`. `level` is the textual
/// level ("WARN"/"ERROR"); `target` is the module path; `msg` the rendered
/// message. Cheap: one lock + hash on the normalized key.
pub fn record(level: &str, target: &str, msg: &str) {
    if !enabled() { return; }
    let key = format!("{level} {target} :: {}", normalize(msg));
    let map = TALLY.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = map.lock().expect("warn_snipe tally poisoned");
    let e = g.entry(key).or_insert((0, String::new()));
    e.0 += 1;
    // Keep the FIRST raw instance as an example (cheap; only on first hit).
    if e.0 == 1 {
        e.1 = if msg.len() > 160 { msg[..160].to_string() } else { msg.to_string() };
    }
}

/// Render the multi-line shutdown summary. Empty string when disabled or no
/// warnings fired. Sorted by descending count so the noisiest site is first.
pub fn summary() -> String {
    if !enabled() { return String::new(); }
    let Some(map) = TALLY.get() else {
        return "warn_snipe: 0 warning sites (clean run)".to_string();
    };
    let g = map.lock().expect("warn_snipe tally poisoned");
    if g.is_empty() {
        return "warn_snipe: 0 warning sites (clean run)".to_string();
    }
    let mut rows: Vec<(&String, &(u64, String))> = g.iter().collect();
    rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then_with(|| a.0.cmp(b.0)));
    let total: u64 = rows.iter().map(|(_, v)| v.0).sum();
    let mut out = format!(
        "warn_snipe: {} site(s), {} total warning(s) [UNENDLICH_WARN_SNIPE]",
        rows.len(), total
    );
    for (key, (count, example)) in rows.iter().take(20) {
        out.push_str(&format!("\n  x{:<6} {}", count, key));
        // Show the first raw instance indented, for the actual values.
        if !example.is_empty() {
            out.push_str(&format!("\n           e.g. {}", example));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_coords_and_counts() {
        assert_eq!(
            normalize("chunk (-4,2,-7) overflowed v=42781, i=131856"),
            normalize("chunk (1,0,3) overflowed v=39912, i=120004"),
            "same warning site with different coords/counts must bucket together"
        );
    }

    #[test]
    fn normalize_collapses_hex() {
        assert_eq!(normalize("seed 0xB10E0001 drift"), normalize("seed 0xdeadbeef drift"));
    }

    #[test]
    fn normalize_keeps_distinct_messages_distinct() {
        assert_ne!(
            normalize("octree pool exhausted"),
            normalize("mesh upload failed"),
        );
    }
}
