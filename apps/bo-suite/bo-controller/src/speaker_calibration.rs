// src/speaker_calibration.rs
#![allow(dead_code)]
//
// Per-speaker latency + room-response calibration store.
//
// ── Why per-speaker ──
//
// The ANC engine's existing AlignmentProbe measures the round-trip
// from "anti-noise emitted" to "anti-noise observed at mic" for the
// CURRENTLY-ACTIVE sink. Without persistence, every fresh
// connection re-probes from scratch — and the probe takes ~1.5 s
// during which the speaker emits an audible chirp.
//
// Different speakers have very different round-trip profiles:
//
//   - Local PA sink:      ~5 ms,  stddev ~1 ms
//   - BT A2DP speaker:    ~150 ms, stddev ~30 ms (depends on stack)
//   - Mozart REST device: 300+ ms, stddev ~80 ms
//
// Carrying these across sessions saves an audible probe per
// reconnection AND lets the canceller use a starting estimate that's
// likely close to right, refining via the periodic re-probe.
//
// ── Why "the phone moves" matters ──
//
// The phone holding the mic is part of the acoustic path. When the
// user puts the phone on the nightstand vs. the desk, the geometry
// changes:
//
//   - Different listener-to-mic distance → the canceller's
//     correlation peak shifts by a few ms (a meter = ~3 ms)
//   - Different room-mode coupling → which frequencies the mic
//     hears strongly varies
//   - Different background noise floor
//
// So even for the same speaker, "where is the phone" affects the
// useful calibration. We don't (yet) model position explicitly —
// instead we treat each calibration as a sample, keep a small
// rolling history, and the canceller uses the MEDIAN over recent
// calibrations. Outliers (one bad probe due to a slamming door)
// don't permanently bias the estimate.
//
// ── What this module is and isn't ──
//
// IS:    storage + lookup of {(speaker_id, calibration_history)},
//        serialized to JSON in $XDG_DATA_HOME/bo-controller/.
// ISN'T: the actual probe runner (that lives in anc_runner). This
//        module exposes `record_calibration` for the runner to call
//        when it completes a probe.
//
// ── Future: smart-follow / staging ──
//
// The user's mention of "smart follow model for the eventual
// speaker instrument separation that way we can always know front
// stage vs back stage" is the long-game use case. Once we have
// per-speaker calibration + per-stem separation, we can route
// (e.g.) brass to the front speaker and strings to the back,
// adjusting per-stem latency to keep them temporally aligned at
// the listener. That's a much bigger system; this module is the
// data-layer prerequisite.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// How many recent calibrations to retain per speaker. The median
/// of N samples is robust against the occasional bad probe (door
/// slam, brief BT stack stall).
pub const HISTORY_DEPTH: usize = 8;

/// A calibration sample is stale if older than this. Beyond stale,
/// the canceller should probe fresh on next engagement.
pub const STALE_AGE_HOURS: u64 = 24;

/// Reasonable correlation floor for a probe to be CONSIDERED for
/// inclusion. Probes below this are tossed (the underlying engine
/// already does this, but we double-check at the persistence
/// boundary to avoid poisoning the history with junk).
pub const MIN_PROBE_CORRELATION: f32 = 0.15;

/// One observed calibration sample.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationSample {
    /// Unix-millis when the probe completed.
    pub timestamp_ms: u64,
    /// Measured round-trip in samples (the probe's native unit).
    pub round_trip_samples: i64,
    /// Correlation strength of the probe — proxy for confidence.
    pub correlation: f32,
    /// Optional context tag: where was the phone? Mostly free-form
    /// — UI offers "nightstand", "desk", "kitchen" as quick picks
    /// but any string is accepted. Empty when the user didn't
    /// label the location.
    #[serde(default)]
    pub location_tag: String,
}

/// Per-speaker history.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpeakerHistory {
    /// Stable speaker identifier — BT MAC (with colons), Mozart
    /// product_id, or "local" for the system default sink.
    pub speaker_id: String,
    /// Human-readable label for the UI. Falls back to speaker_id
    /// when empty.
    #[serde(default)]
    pub label: String,
    /// Recent calibration samples, newest first.
    pub samples: Vec<CalibrationSample>,
}

impl SpeakerHistory {
    /// Median round-trip across the history. Returns None when
    /// there are no samples.
    pub fn median_round_trip_samples(&self) -> Option<i64> {
        if self.samples.is_empty() { return None; }
        let mut vals: Vec<i64> = self.samples.iter()
            .map(|s| s.round_trip_samples).collect();
        vals.sort_unstable();
        Some(vals[vals.len() / 2])
    }

    /// Mean correlation across the history. UI surfaces this as a
    /// "calibration confidence" indicator. Returns 0 when empty.
    pub fn mean_correlation(&self) -> f32 {
        if self.samples.is_empty() { return 0.0; }
        let n = self.samples.len() as f32;
        self.samples.iter().map(|s| s.correlation).sum::<f32>() / n
    }

    /// Most recent calibration timestamp (ms), or 0 if none.
    pub fn latest_timestamp_ms(&self) -> u64 {
        self.samples.first().map(|s| s.timestamp_ms).unwrap_or(0)
    }

    pub fn is_stale(&self, now_ms: u64) -> bool {
        let latest = self.latest_timestamp_ms();
        if latest == 0 { return true; }
        let age_ms = now_ms.saturating_sub(latest);
        age_ms > STALE_AGE_HOURS * 3_600 * 1_000
    }

    fn push(&mut self, sample: CalibrationSample) {
        self.samples.insert(0, sample);
        if self.samples.len() > HISTORY_DEPTH {
            self.samples.truncate(HISTORY_DEPTH);
        }
    }
}

/// The full calibration registry — speaker_id → SpeakerHistory.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CalibrationRegistry {
    pub speakers: HashMap<String, SpeakerHistory>,
}

fn registry_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("bo-controller");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("speaker_calibration.json")
}

fn registry() -> &'static Mutex<Option<CalibrationRegistry>> {
    static R: OnceLock<Mutex<Option<CalibrationRegistry>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(None))
}

fn load(reg: &mut Option<CalibrationRegistry>) {
    if reg.is_some() { return; }
    let path = registry_path();
    let loaded = std::fs::read_to_string(&path).ok()
        .and_then(|s| serde_json::from_str::<CalibrationRegistry>(&s).ok())
        .unwrap_or_default();
    *reg = Some(loaded);
}

fn save(reg: &CalibrationRegistry) {
    let path = registry_path();
    if let Ok(json) = serde_json::to_string_pretty(reg) {
        let _ = std::fs::write(path, json);
    }
}

/// Record a new calibration sample. Filters out samples below the
/// MIN_PROBE_CORRELATION threshold so we don't poison the history
/// with junk probes.
pub fn record_calibration(
    speaker_id: &str,
    label: &str,
    round_trip_samples: i64,
    correlation: f32,
    location_tag: &str,
) {
    if speaker_id.is_empty() { return; }
    if !correlation.is_finite() || correlation < MIN_PROBE_CORRELATION {
        return;
    }
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0);
    let Ok(mut guard) = registry().lock() else { return; };
    load(&mut guard);
    let r = guard.as_mut().unwrap();
    let entry = r.speakers.entry(speaker_id.to_string())
        .or_insert_with(|| SpeakerHistory {
            speaker_id: speaker_id.to_string(),
            label: label.to_string(),
            samples: Vec::new(),
        });
    if entry.label.is_empty() && !label.is_empty() {
        entry.label = label.to_string();
    }
    entry.push(CalibrationSample {
        timestamp_ms: now_ms,
        round_trip_samples,
        correlation,
        location_tag: location_tag.to_string(),
    });
    save(r);
}

/// Look up a speaker's history. Returns a clone (so callers don't
/// hold the global lock); the data is small.
pub fn lookup(speaker_id: &str) -> Option<SpeakerHistory> {
    if speaker_id.is_empty() { return None; }
    let Ok(mut guard) = registry().lock() else { return None; };
    load(&mut guard);
    guard.as_ref().and_then(|r| r.speakers.get(speaker_id).cloned())
}

/// Return all known speakers (cloned). Used by the UI's calibration
/// management page.
pub fn list_all() -> Vec<SpeakerHistory> {
    let Ok(mut guard) = registry().lock() else { return Vec::new(); };
    load(&mut guard);
    guard.as_ref().map(|r| r.speakers.values().cloned().collect())
        .unwrap_or_default()
}

/// Clear a single speaker's history. Used when the user retires a
/// device or moves it to a fundamentally different acoustic context.
pub fn clear(speaker_id: &str) {
    if speaker_id.is_empty() { return; }
    let Ok(mut guard) = registry().lock() else { return; };
    load(&mut guard);
    if let Some(r) = guard.as_mut() {
        if r.speakers.remove(speaker_id).is_some() {
            save(r);
        }
    }
}

/// Whether this speaker's calibration is stale (older than
/// STALE_AGE_HOURS). Callers use this to decide whether to re-probe
/// before engaging ANC.
pub fn needs_recalibration(speaker_id: &str) -> bool {
    let Some(h) = lookup(speaker_id) else { return true; };
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0);
    h.is_stale(now_ms)
}

/// Best-estimate round-trip samples for a speaker (median of recent
/// calibrations). None if no calibration on file.
pub fn best_estimate_round_trip(speaker_id: &str) -> Option<i64> {
    lookup(speaker_id).and_then(|h| h.median_round_trip_samples())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_isolated_xdg<F: FnOnce()>(label: &str, f: F) {
        let tmp = std::env::temp_dir().join(format!(
            "bo-spk-cal-test-{}-{}", label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("XDG_DATA_HOME");
        unsafe { std::env::set_var("XDG_DATA_HOME", &tmp); }
        // Reset the global so the next access reloads.
        if let Ok(mut g) = registry().lock() { *g = None; }
        f();
        if let Some(p) = prev {
            unsafe { std::env::set_var("XDG_DATA_HOME", p); }
        } else {
            unsafe { std::env::remove_var("XDG_DATA_HOME"); }
        }
        if let Ok(mut g) = registry().lock() { *g = None; }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn record_persists_and_lookup_returns() {
        with_isolated_xdg("persist", || {
            assert!(lookup("AA:BB:CC:DD:EE:FF").is_none());
            record_calibration("AA:BB:CC:DD:EE:FF", "Smokey", 7200, 0.83, "nightstand");
            let h = lookup("AA:BB:CC:DD:EE:FF").unwrap();
            assert_eq!(h.samples.len(), 1);
            assert_eq!(h.samples[0].round_trip_samples, 7200);
            assert_eq!(h.samples[0].location_tag, "nightstand");
            assert_eq!(h.label, "Smokey");
        });
    }

    #[test]
    fn low_correlation_samples_are_rejected() {
        with_isolated_xdg("low-corr", || {
            record_calibration("X", "X", 1000, 0.10, "");  // below floor
            assert!(lookup("X").is_none());
            record_calibration("X", "X", 1000, 0.50, "");  // above floor
            assert!(lookup("X").is_some());
        });
    }

    #[test]
    fn non_finite_correlation_rejected() {
        with_isolated_xdg("nonfinite", || {
            record_calibration("X", "X", 1000, f32::NAN, "");
            record_calibration("X", "X", 1000, f32::INFINITY, "");
            assert!(lookup("X").is_none());
        });
    }

    #[test]
    fn history_caps_at_depth() {
        with_isolated_xdg("cap", || {
            for i in 0..HISTORY_DEPTH + 5 {
                record_calibration("X", "X", i as i64 * 100, 0.5, "");
            }
            let h = lookup("X").unwrap();
            assert_eq!(h.samples.len(), HISTORY_DEPTH);
            // Newest first — should have the most recent value.
            assert_eq!(h.samples[0].round_trip_samples,
                (HISTORY_DEPTH + 4) as i64 * 100);
        });
    }

    #[test]
    fn median_is_robust_to_outlier() {
        with_isolated_xdg("median", || {
            for v in [5000, 5100, 5050, 5080, 50_000_i64, 5070, 5090] {
                // Note: 50_000 is the outlier
                record_calibration("X", "X", v, 0.5, "");
            }
            let h = lookup("X").unwrap();
            let med = h.median_round_trip_samples().unwrap();
            // Median should be around 5070-5090, not pulled toward 50000.
            assert!(med >= 5050 && med <= 5100,
                "median should ignore outlier, got {}", med);
        });
    }

    #[test]
    fn stale_check_works_correctly() {
        with_isolated_xdg("stale", || {
            record_calibration("X", "X", 1000, 0.5, "");
            // Fresh — not stale.
            assert!(!needs_recalibration("X"));
            // Unknown speaker — stale.
            assert!(needs_recalibration("UNKNOWN"));
        });
    }

    #[test]
    fn clear_removes_speaker() {
        with_isolated_xdg("clear", || {
            record_calibration("X", "X", 1000, 0.5, "");
            assert!(lookup("X").is_some());
            clear("X");
            assert!(lookup("X").is_none());
        });
    }

    #[test]
    fn list_all_returns_every_speaker() {
        with_isolated_xdg("list", || {
            record_calibration("A", "A", 100, 0.5, "");
            record_calibration("B", "B", 200, 0.5, "");
            record_calibration("C", "C", 300, 0.5, "");
            let all = list_all();
            assert_eq!(all.len(), 3);
        });
    }

    #[test]
    fn label_updates_on_subsequent_calibration_if_empty() {
        with_isolated_xdg("label", || {
            record_calibration("X", "", 100, 0.5, "");
            assert_eq!(lookup("X").unwrap().label, "");
            record_calibration("X", "Smokey", 110, 0.5, "");
            assert_eq!(lookup("X").unwrap().label, "Smokey");
            // Once labeled, don't overwrite on later calibration
            // (the user's preferred name wins).
            record_calibration("X", "", 120, 0.5, "");
            assert_eq!(lookup("X").unwrap().label, "Smokey");
        });
    }

    #[test]
    fn location_tag_stored() {
        with_isolated_xdg("loc", || {
            record_calibration("X", "X", 100, 0.5, "nightstand");
            record_calibration("X", "X", 110, 0.5, "desk");
            let h = lookup("X").unwrap();
            assert_eq!(h.samples[0].location_tag, "desk");
            assert_eq!(h.samples[1].location_tag, "nightstand");
        });
    }
}
