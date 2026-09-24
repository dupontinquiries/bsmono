//! wear — NVMe health over time, not just now. `smart::probe` says what a drive reports today; this
//! keeps a ledger of those reports per drive (by serial) and turns the deltas into the numbers that
//! actually decide anything: terabytes written per day, percentage-used per day, projected life
//! against a rated endurance, unsafe shutdowns since last time (an external enclosure losing power),
//! media errors since last time (the only number that means "copy it off now"). Internal Apple NVMe
//! and external NVMe behind Thunderbolt or USB bridges look the same here; the probe ladder in
//! `smart` is what differs.
//!
//! Storage is JSONL under the XDG data dir; nothing here touches a device. The swap check is in
//! here too because on a Mac swap is the one thing that writes to the internal module without you
//! asking, and the rule for a machine you mean to keep ten years is "swap at zero".
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::smart::{Finding, Severity, SmartReport};
use crate::SysRunner;

/// NVMe "data units" are 512,000 bytes (1000 × 512-byte blocks), per the spec.
pub const DATA_UNIT_BYTES: f64 = 512_000.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub at_unix: u64,
    pub device: String,
    pub serial: Option<String>,
    pub model: Option<String>,
    pub protocol: Option<String>,
    pub temperature_c: Option<i64>,
    pub power_on_hours: Option<i64>,
    pub percentage_used: Option<i64>,
    pub available_spare: Option<i64>,
    pub available_spare_threshold: Option<i64>,
    pub media_errors: Option<i64>,
    pub unsafe_shutdowns: Option<i64>,
    pub critical_warning: Option<i64>,
    pub data_units_written: Option<i64>,
    pub data_units_read: Option<i64>,
}

impl Snapshot {
    pub fn from_report(r: &SmartReport, at_unix: u64) -> Snapshot {
        let log = r.raw.get("nvme_smart_health_information_log");
        let g = |k: &str| log.and_then(|l| l.get(k)).and_then(|v| v.as_i64());
        Snapshot {
            at_unix, device: r.device.clone(), serial: r.serial.clone(), model: r.model.clone(), protocol: r.protocol.clone(),
            temperature_c: r.temperature_c, power_on_hours: r.power_on_hours,
            percentage_used: r.nvme.as_ref().and_then(|n| n.percentage_used).or_else(|| g("percentage_used")),
            available_spare: r.nvme.as_ref().and_then(|n| n.available_spare).or_else(|| g("available_spare")),
            available_spare_threshold: r.nvme.as_ref().and_then(|n| n.available_spare_threshold).or_else(|| g("available_spare_threshold")),
            media_errors: r.nvme.as_ref().and_then(|n| n.media_errors).or_else(|| g("media_errors")),
            unsafe_shutdowns: r.nvme.as_ref().and_then(|n| n.unsafe_shutdowns).or_else(|| g("unsafe_shutdowns")),
            critical_warning: r.nvme.as_ref().map(|n| n.critical_warning as i64).or_else(|| g("critical_warning")),
            data_units_written: r.nvme.as_ref().and_then(|n| n.data_units_written).or_else(|| g("data_units_written")),
            data_units_read: g("data_units_read"),
        }
    }
    /// The ledger key: serial when the drive gives one, else the device path (bridges sometimes hide it).
    pub fn key(&self) -> String { self.serial.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| self.device.clone()) }
    pub fn tb_written(&self) -> Option<f64> { self.data_units_written.map(|u| u as f64 * DATA_UNIT_BYTES / 1e12) }
}

pub fn ledger_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share"));
    std::env::var("DRIVEMIND_WEAR_LEDGER").map(PathBuf::from).unwrap_or_else(|_| base.join("drivemind").join("wear.jsonl"))
}

pub fn record(s: &Snapshot) -> Result<(), String> {
    let p = ledger_path();
    if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&p).map_err(|e| e.to_string())?;
    writeln!(f, "{}", serde_json::to_string(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// All snapshots, grouped by key, oldest first.
pub fn history() -> BTreeMap<String, Vec<Snapshot>> {
    let mut m: BTreeMap<String, Vec<Snapshot>> = BTreeMap::new();
    for line in std::fs::read_to_string(ledger_path()).unwrap_or_default().lines() {
        if let Ok(s) = serde_json::from_str::<Snapshot>(line) { m.entry(s.key()).or_default().push(s); }
    }
    for v in m.values_mut() { v.sort_by_key(|s| s.at_unix); }
    m
}

/// What the deltas say. Computed between the oldest and newest snapshot for rates, and between the
/// two newest for "since last time" counters, because a counter that moved once matters even if the
/// long-run rate is tiny.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Trend {
    pub key: String,
    pub snapshots: usize,
    pub span_days: f64,
    pub tb_written_total: Option<f64>,
    pub tb_written_per_day: Option<f64>,
    pub percent_used_per_day: Option<f64>,
    /// Days until percentage_used reaches 100 at the observed rate.
    pub days_to_100_percent: Option<f64>,
    /// Days until the rated endurance is reached at the observed write rate, when a rating is known.
    pub days_to_rated_tbw: Option<f64>,
    pub rated_tbw: Option<f64>,
    pub unsafe_shutdowns_since_last: Option<i64>,
    pub media_errors_since_last: Option<i64>,
    pub temp_max_c: Option<i64>,
    pub latest: Snapshot,
}

pub fn trend(key: &str, snaps: &[Snapshot], rated_tbw: Option<f64>) -> Option<Trend> {
    let (first, last) = (snaps.first()?, snaps.last()?);
    let span_days = (last.at_unix.saturating_sub(first.at_unix)) as f64 / 86_400.0;
    let prev = if snaps.len() >= 2 { Some(&snaps[snaps.len() - 2]) } else { None };
    let tb_total = last.tb_written();
    let per_day = |a: Option<i64>, b: Option<i64>, scale: f64| -> Option<f64> {
        if span_days < 1.0 / 24.0 { return None; }   // under an hour of history says nothing about rates
        Some((b? - a?) as f64 * scale / span_days)
    };
    let tb_per_day = per_day(first.data_units_written, last.data_units_written, DATA_UNIT_BYTES / 1e12);
    let pct_per_day = per_day(first.percentage_used, last.percentage_used, 1.0);
    let days_to_100 = match (pct_per_day, last.percentage_used) { (Some(r), Some(p)) if r > 0.0 => Some((100 - p).max(0) as f64 / r), _ => None };
    let days_to_rated = match (tb_per_day, tb_total, rated_tbw) { (Some(r), Some(t), Some(rated)) if r > 0.0 => Some((rated - t).max(0.0) / r), _ => None };
    let delta = |f: fn(&Snapshot) -> Option<i64>| prev.and_then(|p| Some(f(last)? - f(p)?));
    Some(Trend {
        key: key.to_string(), snapshots: snaps.len(), span_days, tb_written_total: tb_total, tb_written_per_day: tb_per_day,
        percent_used_per_day: pct_per_day, days_to_100_percent: days_to_100, days_to_rated_tbw: days_to_rated, rated_tbw,
        unsafe_shutdowns_since_last: delta(|s| s.unsafe_shutdowns), media_errors_since_last: delta(|s| s.media_errors),
        temp_max_c: snaps.iter().filter_map(|s| s.temperature_c).max(), latest: last.clone(),
    })
}

/// Rated endurance for drives whose model string we recognise, in TB written. A config file
/// (`DRIVEMIND_ENDURANCE`, `model substring=TBW` lines) overrides; unknown drives get None and the
/// projection falls back to percentage_used, which the controller computes from its own rating anyway.
pub fn rated_tbw_for(model: &str) -> Option<f64> {
    let m = model.to_ascii_lowercase();
    if let Ok(p) = std::env::var("DRIVEMIND_ENDURANCE") {
        for line in std::fs::read_to_string(p).unwrap_or_default().lines() {
            if let Some((needle, tbw)) = line.split_once('=') { if !needle.trim().is_empty() && m.contains(&needle.trim().to_ascii_lowercase()) { return tbw.trim().parse().ok(); } }
        }
    }
    // capacity from the model string: "2TB", "2 TB", "2000GB", "2000 GB", "8TB"
    let tb = capacity_tb(&m);
    let table: &[(&str, f64, f64)] = &[
        ("990 pro", 1.0, 600.0), ("990 pro", 2.0, 1200.0), ("990 pro", 4.0, 2400.0),
        ("sn850x", 1.0, 600.0), ("sn850x", 2.0, 1200.0), ("sn850x", 4.0, 2400.0), ("sn850x", 8.0, 4800.0),
        ("t705", 1.0, 600.0), ("t705", 2.0, 1200.0), ("t705", 4.0, 2400.0),
        ("studio drive", 2.0, 4000.0), ("studio drive", 4.0, 8000.0), ("studio drive", 8.0, 16000.0),
    ];
    table.iter().find(|(a, cap, _)| m.contains(a) && tb.map(|t| (t - cap).abs() < 0.3).unwrap_or(false)).map(|(_, _, t)| *t)
}

/// The capacity a model string names, in TB (2000GB → 2.0, 1TB → 1.0). None when it names none.
pub fn capacity_tb(model_lower: &str) -> Option<f64> {
    let b = model_lower.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i; while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') { i += 1; }
            let n: f64 = model_lower[start..i].parse().ok()?;
            let rest = model_lower[i..].trim_start();
            if rest.starts_with("tb") { return Some(n); }
            if rest.starts_with("gb") { return Some(n / 1000.0); }
        } else { i += 1; }
    }
    None
}

/// Findings from the trend: the counters that mean "act", with the evidence in the sentence.
pub fn findings(t: &Trend) -> Vec<Finding> {
    let mut v = Vec::new();
    let f = |severity: Severity, key: &str, message: String| Finding { severity, key: key.to_string(), message };
    if let Some(n) = t.media_errors_since_last { if n > 0 { v.push(f(Severity::Critical, "wear.media_errors_delta", format!("{n} new media/data integrity errors since last snapshot: copy the data off and stop trusting this drive"))); } }
    if let Some(c) = t.latest.critical_warning { if c != 0 { v.push(f(Severity::Critical, "wear.critical_warning", format!("controller critical warning 0x{c:02x}"))); } }
    if let (Some(s), Some(th)) = (t.latest.available_spare, t.latest.available_spare_threshold) { if s <= th { v.push(f(Severity::Critical, "wear.spare", format!("available spare {s}% at or under threshold {th}%"))); } }
    if let Some(p) = t.latest.percentage_used {
        if p >= 90 { v.push(f(Severity::Warning, "wear.percentage_used", format!("{p}% of rated endurance used"))); }
        else if p >= 70 { v.push(f(Severity::Advisory, "wear.percentage_used", format!("{p}% of rated endurance used"))); }
    }
    if let Some(n) = t.unsafe_shutdowns_since_last { if n > 0 { v.push(f(Severity::Warning, "wear.unsafe_shutdowns_delta", format!("{n} unsafe shutdowns since last snapshot: power loss or unplugged without eject (external enclosure power is the usual cause)"))); } }
    if let Some(c) = t.temp_max_c { if c >= 70 { v.push(f(Severity::Warning, "wear.temperature", format!("peak {c} °C in history; NVMe throttles around 70–80 and the enclosure is the fix"))); } }
    if let (Some(rate), Some(days)) = (t.tb_written_per_day, t.days_to_rated_tbw.or(t.days_to_100_percent)) {
        let sev = if days < 365.0 { Severity::Warning } else { Severity::Ok };
        v.push(f(sev, "wear.projection", format!("{rate:.2} TB/day written; ~{:.0} days ({:.1} years) of endurance left at this rate", days, days / 365.0)));
    }
    v
}

/// Swap in use, from the OS. macOS: `sysctl vm.swapusage`; Linux: /proc/swaps. Any non-zero used
/// swap on a machine holding models resident is a Warning: it is the one write load the internal
/// module takes that nobody asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwapStatus { pub total_mb: f64, pub used_mb: f64, pub source: String }

pub fn swap_status(sys: &dyn SysRunner) -> Result<SwapStatus, String> {
    if cfg!(target_os = "macos") {
        let out = sys.run("sysctl", &["vm.swapusage"])?;
        return parse_swapusage(&out.stdout).ok_or_else(|| format!("unparsed: {}", out.stdout.trim()));
    }
    let text = std::fs::read_to_string("/proc/swaps").map_err(|e| format!("/proc/swaps: {e}"))?;
    Ok(parse_proc_swaps(&text))
}

/// `vm.swapusage: total = 2048.00M  used = 1027.25M  free = 1020.75M  (encrypted)`
pub fn parse_swapusage(s: &str) -> Option<SwapStatus> {
    let grab = |k: &str| -> Option<f64> {
        let i = s.find(&format!("{k} = "))? + k.len() + 3;
        let rest = &s[i..]; let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        let n: f64 = rest[..end].parse().ok()?;
        Some(match rest[end..].chars().next() { Some('G') => n * 1024.0, Some('K') => n / 1024.0, _ => n })
    };
    Some(SwapStatus { total_mb: grab("total")?, used_mb: grab("used")?, source: "sysctl vm.swapusage".into() })
}

pub fn parse_proc_swaps(text: &str) -> SwapStatus {
    let (mut total, mut used) = (0.0, 0.0);
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 4 { total += cols[2].parse::<f64>().unwrap_or(0.0) / 1024.0; used += cols[3].parse::<f64>().unwrap_or(0.0) / 1024.0; }
    }
    SwapStatus { total_mb: total, used_mb: used, source: "/proc/swaps".into() }
}

pub fn swap_finding(s: &SwapStatus) -> Finding {
    if s.used_mb > 0.0 {
        Finding { severity: Severity::Warning, key: "swap.used".into(), message: format!("{:.0} MB of swap in use ({:.0} MB configured): the internal SSD is taking writes nobody asked for; free memory or stop overcommitting before long runs", s.used_mb, s.total_mb) }
    } else {
        Finding { severity: Severity::Ok, key: "swap.used".into(), message: format!("swap at zero ({:.0} MB configured)", s.total_mb) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(at_days: f64, written_units: i64, pct: i64, unsafe_sd: i64, media: i64, temp: i64) -> Snapshot {
        Snapshot { at_unix: (at_days * 86_400.0) as u64, device: "/dev/nvme1n1".into(), serial: Some("S1".into()), model: Some("Samsung SSD 990 PRO 2TB".into()), protocol: Some("NVMe".into()),
            temperature_c: Some(temp), power_on_hours: Some(100), percentage_used: Some(pct), available_spare: Some(100), available_spare_threshold: Some(10),
            media_errors: Some(media), unsafe_shutdowns: Some(unsafe_sd), critical_warning: Some(0), data_units_written: Some(written_units), data_units_read: Some(0) }
    }

    #[test]
    fn rates_projection_and_deltas() {
        // 10 days, 1 TB written (1 TB = 1e12 / 512_000 = 1_953_125 units), 1% used → 100 days per %, 99% left → 9,900 days
        let s = vec![snap(0.0, 0, 1, 3, 0, 40), snap(10.0, 1_953_125, 2, 5, 0, 66)];
        let t = trend("S1", &s, rated_tbw_for("Samsung SSD 990 PRO 2TB")).unwrap();
        assert!((t.tb_written_per_day.unwrap() - 0.1).abs() < 1e-6, "{t:?}");
        assert!((t.percent_used_per_day.unwrap() - 0.1).abs() < 1e-9);
        assert!((t.days_to_100_percent.unwrap() - 980.0).abs() < 1e-6);
        assert_eq!(t.rated_tbw, Some(1200.0));
        assert!((t.days_to_rated_tbw.unwrap() - 11_990.0).abs() < 1e-6);
        assert_eq!(t.unsafe_shutdowns_since_last, Some(2));
        assert_eq!(t.temp_max_c, Some(66));
        let f = findings(&t);
        assert!(f.iter().any(|x| x.key == "wear.unsafe_shutdowns_delta" && x.severity == Severity::Warning), "{f:?}");
        assert!(!f.iter().any(|x| x.severity == Severity::Critical));
    }

    #[test]
    fn media_errors_are_critical_and_short_spans_give_no_rate() {
        let s = vec![snap(0.0, 0, 5, 0, 0, 40), snap(0.01, 10, 5, 0, 1, 41)];
        let t = trend("S1", &s, None).unwrap();
        assert_eq!(t.tb_written_per_day, None);
        assert!(findings(&t).iter().any(|x| x.key == "wear.media_errors_delta" && x.severity == Severity::Critical));
    }

    #[test]
    fn endurance_lookup_reads_capacity_in_gb_or_tb() {
        assert_eq!(rated_tbw_for("WD_BLACK SN850X 2000GB"), Some(1200.0));
        assert_eq!(rated_tbw_for("Samsung SSD 990 PRO 4TB"), Some(2400.0));
        assert_eq!(rated_tbw_for("Polysoft Studio Drive M2 8TB"), Some(16000.0));
        assert_eq!(rated_tbw_for("APPLE SSD AP2048Z"), None);
    }

    #[test]
    fn swap_parsers() {
        let s = parse_swapusage("vm.swapusage: total = 2048.00M  used = 1027.25M  free = 1020.75M  (encrypted)").unwrap();
        assert!((s.used_mb - 1027.25).abs() < 1e-6 && (s.total_mb - 2048.0).abs() < 1e-6);
        assert_eq!(swap_finding(&s).severity, Severity::Warning);
        let z = parse_proc_swaps("Filename Type Size Used Priority\n/dev/zram0 partition 8388604 0 100\n");
        assert!((z.total_mb - 8191.99).abs() < 0.1 && z.used_mb == 0.0);
        assert_eq!(swap_finding(&z).severity, Severity::Ok);
    }
}
