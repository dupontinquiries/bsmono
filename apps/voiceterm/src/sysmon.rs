//! v0.1.602: pure core of the resource overlay — the /proc parsing and
//! the event-driven "is this notable enough to flash the overlay?"
//! decision logic, extracted out of system_info.rs so they can be unit-
//! tested. system_info.rs does the IO (read /proc/meminfo, /proc/loadavg)
//! and holds the cross-call state (last sample); this module is the pure
//! transform: bytes → numbers, and (prev, cur) → optional summary.
//!
//! Why extract: the threshold/crossing logic (does load crossing 1.0
//! fire once vs every poll? does the 80% pressure zone trigger on the
//! right edge?) is exactly where bugs hide, and it was previously
//! untestable — tangled with a Mutex + file IO inside resource_overlay.
//! And the meminfo percentage math was DUPLICATED in two IO functions.

/// Parse the used-memory percentage (0..100) from /proc/meminfo
/// contents. Returns None if MemTotal is missing/zero. Saturating so a
/// kernel quirk where MemAvailable > MemTotal can't underflow.
///
/// Assumes MemAvailable is present (every Linux >= 3.14, so always true
/// on the Asahi target). On a hypothetical pre-3.14 kernel lacking it,
/// avail_kb stays 0 and this reports 100% — acceptable since that
/// kernel can't run on the target hardware. Not adding a MemFree
/// fallback would only change behavior for a case that can't occur.
pub fn parse_meminfo_pct(content: &str) -> Option<u32> {
    let mut total_kb = 0u64;
    let mut avail_kb = 0u64;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = first_u64(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail_kb = first_u64(rest);
        }
    }
    if total_kb == 0 {
        return None;
    }
    let used = total_kb.saturating_sub(avail_kb.min(total_kb));
    Some((used * 100 / total_kb) as u32)
}

/// Parse (used_kb, total_kb) from /proc/meminfo for a human-readable
/// "X.YGB / Z.WGB used (P%)" string. Returns None if total is zero.
pub fn parse_meminfo_used_total(content: &str) -> Option<(u64, u64)> {
    let mut total_kb = 0u64;
    let mut avail_kb = 0u64;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = first_u64(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail_kb = first_u64(rest);
        }
    }
    if total_kb == 0 {
        return None;
    }
    let used = total_kb.saturating_sub(avail_kb.min(total_kb));
    Some((used, total_kb))
}

/// Format the memory usage line from (used_kb, total_kb).
pub fn format_mem_usage(used_kb: u64, total_kb: u64) -> String {
    let pct = if total_kb > 0 { used_kb * 100 / total_kb } else { 0 };
    format!(
        "{:.1}GB / {:.1}GB used ({}%)",
        used_kb as f64 / 1_048_576.0,
        total_kb as f64 / 1_048_576.0,
        pct
    )
}

/// Parse the 1-minute load average from /proc/loadavg contents.
pub fn parse_loadavg_1min(content: &str) -> f32 {
    content
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

/// The event-driven overlay decision, as a pure function of the previous
/// and current samples. Returns Some(summary) only when something
/// notable just happened, else None (the common case — overlay stays
/// quiet). `had_prev` is false on the very first sample (nothing to
/// compare against → quiet).
///
/// Priority order (first match wins), mirroring the original:
///   1. high-alert sustained: mem > 90% ("! ..."), load > 4.0
///   2. mem 80% pressure-zone crossing (entering ↑ / leaving ↓)
///   3. mem jump ≥ 5 percentage points (↑/↓)
///   4. load crossing 1.0 upward (↑) / falling below 0.5 from >1.0 (↓)
///   5. nothing → None
pub fn overlay_decision(
    had_prev: bool,
    prev_load: f32,
    prev_mem: u32,
    cur_load: f32,
    cur_mem: u32,
) -> Option<String> {
    // First sample ever — nothing to compare against, stay quiet. This
    // preserves the original resource_overlay behavior exactly: even a
    // high-alert reading waits for the second poll (~2s), which avoids
    // a startup flash before the overlay has a baseline.
    if !had_prev {
        return None;
    }
    // High-alert sustained conditions take priority.
    if cur_mem > 90 {
        return Some(format!("! mem {}% (low free)", cur_mem));
    }
    if cur_load > 4.0 {
        return Some(format!("! cpu load {:.1}", cur_load));
    }
    // Memory 80% pressure-zone crossing.
    if prev_mem < 80 && cur_mem >= 80 {
        return Some(format!("\u{2191} mem {}% \u{2192} {}%", prev_mem, cur_mem));
    }
    if prev_mem >= 80 && cur_mem < 80 {
        return Some(format!("\u{2193} mem {}% (eased)", cur_mem));
    }
    // Big memory jump either direction.
    let mem_delta = (cur_mem as i32 - prev_mem as i32).abs();
    if mem_delta >= 5 {
        let arrow = if cur_mem > prev_mem { "\u{2191}" } else { "\u{2193}" };
        return Some(format!("{} mem {}% \u{2192} {}%", arrow, prev_mem, cur_mem));
    }
    // CPU load threshold crossing.
    if prev_load < 1.0 && cur_load >= 1.0 {
        return Some(format!("\u{2191} cpu load {:.2}", cur_load));
    }
    if prev_load > 1.0 && cur_load < 0.5 {
        return Some(format!("\u{2193} cpu load {:.2}", cur_load));
    }
    None
}

fn first_u64(s: &str) -> u64 {
    s.split_whitespace()
        .next()
        .and_then(|t| t.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
MemTotal:        8000000 kB
MemFree:         1000000 kB
MemAvailable:    2000000 kB
Buffers:          100000 kB
";

    #[test]
    fn meminfo_pct_basic() {
        // used = 8000000 - 2000000 = 6000000 → 75%
        assert_eq!(parse_meminfo_pct(SAMPLE), Some(75));
    }

    #[test]
    fn meminfo_missing_total_is_none() {
        assert_eq!(parse_meminfo_pct("MemFree: 100 kB\n"), None);
    }

    #[test]
    fn meminfo_avail_exceeds_total_saturates() {
        // kernel quirk: avail > total must not underflow → used 0 → 0%
        let s = "MemTotal: 1000 kB\nMemAvailable: 5000 kB\n";
        assert_eq!(parse_meminfo_pct(s), Some(0));
    }

    #[test]
    fn loadavg_parse() {
        assert_eq!(parse_loadavg_1min("0.42 0.30 0.15 1/234 5678"), 0.42);
        assert_eq!(parse_loadavg_1min(""), 0.0);
        assert_eq!(parse_loadavg_1min("garbage"), 0.0);
    }

    #[test]
    fn format_mem_line() {
        let s = format_mem_usage(6_000_000, 8_000_000);
        assert!(s.contains("75%"), "got {}", s);
        assert!(s.contains("GB"));
    }

    // ── overlay_decision: the bug-prone threshold logic ──

    #[test]
    fn first_sample_is_always_quiet() {
        // Preserves original behavior: the first poll has no baseline, so
        // it stays quiet even for a high-alert reading (it'll fire on the
        // next poll ~2s later). Avoids a startup flash.
        assert_eq!(overlay_decision(false, 0.0, 50, 0.3, 52), None);
        assert_eq!(overlay_decision(false, 0.0, 0, 0.0, 95), None);
        // on the SECOND sample, the alert fires.
        assert_eq!(overlay_decision(true, 0.0, 0, 0.0, 95),
            Some("! mem 95% (low free)".to_string()));
    }

    #[test]
    fn steady_state_is_quiet() {
        // small mem change, load unchanged, no crossing → None
        assert_eq!(overlay_decision(true, 0.3, 50, 0.3, 52), None);
    }

    #[test]
    fn mem_pressure_zone_crossing_fires_once() {
        // crossing INTO 80% fires
        assert!(overlay_decision(true, 0.3, 78, 0.3, 82).is_some());
        // but once already in the zone, a small further change is quiet
        // (this is the "fires once, not every poll" property)
        assert_eq!(overlay_decision(true, 0.3, 82, 0.3, 83), None);
        // leaving the zone fires (eased)
        let eased = overlay_decision(true, 0.3, 82, 0.3, 78).unwrap();
        assert!(eased.contains("eased"));
    }

    #[test]
    fn load_crossing_one_fires_then_quiets() {
        // crossing 1.0 upward fires
        assert!(overlay_decision(true, 0.8, 50, 1.2, 50).is_some());
        // staying above 1.0 (no re-cross) is quiet — the key anti-spam
        // property: it must NOT fire every poll while load stays high
        assert_eq!(overlay_decision(true, 1.2, 50, 1.5, 50), None);
        // falling below 0.5 from above 1.0 fires (calmed)
        assert!(overlay_decision(true, 1.5, 50, 0.4, 50).is_some());
    }

    #[test]
    fn alert_takes_priority_over_crossing() {
        // mem 95% is high-alert; even though it also "crossed" 80,
        // the alert form wins.
        let s = overlay_decision(true, 0.3, 50, 0.3, 95).unwrap();
        assert!(s.starts_with("! mem"), "alert should win, got {}", s);
    }

    #[test]
    fn mem_jump_threshold_is_5_points() {
        // exactly 5 fires
        assert!(overlay_decision(true, 0.3, 50, 0.3, 55).is_some());
        // 4 stays quiet
        assert_eq!(overlay_decision(true, 0.3, 50, 0.3, 54), None);
    }
}
