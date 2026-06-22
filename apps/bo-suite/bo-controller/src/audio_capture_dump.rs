// src/audio_capture_dump.rs
#![allow(dead_code)]
//
// Triggered audio capture for offline analysis + CNN training data.
//
// ── Why this exists ──
//
// The fuzz harness tests the canceller against SYNTHESIZED engine
// sounds. Real engines are richer — combustion timbre, exhaust
// resonance, tire/road noise, wind — and a CNN trained to recognize
// + clean up residual engine character would need REAL recordings,
// not synthesizer output, to learn from. This module is the data
// collection layer for that future training pipeline.
//
// ── Design ──
//
// A continuous ring buffer holds the last RING_SECS of mic input.
// When the passby (or drone) FSM enters Active/Peak, the ring is
// promoted to a permanent WAV file along with the next CAPTURE_SECS
// of audio. Result: each capture is a 40-second window centered on
// the event, giving the CNN both "what does an engine look like
// approaching" and "what does the residual sound like during the
// event."
//
// A JSON sidecar records per-window FSM snapshots (phase, freq,
// amp, lock quality) so the training pipeline can label which
// portions of audio correspond to which canceller state.
//
// ── Disk discipline ──
//
// Each capture is ~3.8 MB (40 s × 48 kHz × 2 bytes/sample). With
// MAX_CAPTURES=50 the worst case is ~190 MB — fits comfortably on
// the FP5's storage. Oldest captures evict when the budget fills.
//
// The ring buffer itself is in-memory only; it lives as long as the
// dump is enabled and gets dropped on disable.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SAMPLE_RATE: u32 = 48_000;
pub const RING_SECS: usize = 30;
pub const CAPTURE_SECS: usize = 10;
pub const MAX_CAPTURES: usize = 50;

/// FSM snapshot at a moment in time. Mirrors what the canceller's
/// own snapshot returns but stays serializable + decoupled from
/// any specific canceller's struct (so the same sidecar format
/// works for passby, drone, beeper).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureSnapshot {
    /// Sample offset from the start of the WAV file.
    pub sample_offset: u64,
    pub event_type: String,
    pub fsm_phase: String,
    pub fundamental_hz: f32,
    pub band_energy: f32,
    pub anti_amp: f32,
    pub phase_lock_quality: f32,
}

/// Sidecar metadata stored next to each capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureMetadata {
    /// Unix timestamp when capture started (ring-buffer start).
    pub unix_ms: u64,
    /// What triggered the capture (FSM event, manual trigger).
    pub trigger: String,
    /// Optional context tag set via the QML "tag this capture"
    /// field — "downtown bedroom window", "kitchen", etc. The
    /// CNN training pipeline can use this to split by acoustic
    /// environment.
    pub location_tag: String,
    /// Sample rate of the WAV.
    pub sample_rate: u32,
    /// Total samples in the WAV.
    pub total_samples: u64,
    /// FSM snapshots indexed by sample offset.
    pub snapshots: Vec<CaptureSnapshot>,
}

struct ActiveCapture {
    wav_path: PathBuf,
    sidecar_path: PathBuf,
    writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    samples_remaining: usize,
    metadata: CaptureMetadata,
    samples_written: u64,
}

struct DumpState {
    enabled: bool,
    ring: VecDeque<f32>,
    ring_capacity: usize,
    active: Option<ActiveCapture>,
    capture_dir: PathBuf,
    location_tag: String,
}

impl Default for DumpState {
    fn default() -> Self {
        Self {
            enabled: false,
            ring: VecDeque::with_capacity(RING_SECS * SAMPLE_RATE as usize),
            ring_capacity: RING_SECS * SAMPLE_RATE as usize,
            active: None,
            capture_dir: capture_dir(),
            location_tag: String::new(),
        }
    }
}

fn capture_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("bo-controller").join("captures");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn state() -> &'static Mutex<DumpState> {
    static S: OnceLock<Mutex<DumpState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(DumpState::default()))
}

/// Enable or disable the dump. Disabling drops the ring buffer
/// (frees memory) and finalizes any in-progress capture.
pub fn set_enabled(on: bool) {
    let Ok(mut s) = state().lock() else { return; };
    if on == s.enabled { return; }
    s.enabled = on;
    if !on {
        s.ring.clear();
        finalize_active_locked(&mut s);
    }
}

pub fn is_enabled() -> bool {
    state().lock().map(|s| s.enabled).unwrap_or(false)
}

/// Set the location tag attached to subsequent captures. Free-form
/// string; UI surfaces this as "tag for tonight's recording."
pub fn set_location_tag(tag: &str) {
    let Ok(mut s) = state().lock() else { return; };
    s.location_tag = tag.to_string();
}

/// Push a mic window into the ring buffer + active capture.
/// Called every mic-window cycle by the canceller's runner.
pub fn ingest_mic_window(samples: &[f32]) {
    let Ok(mut s) = state().lock() else { return; };
    if !s.enabled { return; }
    // Ring buffer maintenance.
    for &v in samples {
        if s.ring.len() >= s.ring_capacity {
            s.ring.pop_front();
        }
        s.ring.push_back(v.clamp(-1.0, 1.0));
    }
    // Active capture writing.
    let mut finalize_now = false;
    if let Some(cap) = s.active.as_mut() {
        let to_write = samples.len().min(cap.samples_remaining);
        for &v in &samples[..to_write] {
            let i16_val = (v.clamp(-1.0, 1.0) * 32_767.0) as i16;
            let _ = cap.writer.write_sample(i16_val);
            cap.samples_written += 1;
        }
        cap.samples_remaining -= to_write;
        if cap.samples_remaining == 0 {
            finalize_now = true;
        }
    }
    if finalize_now {
        finalize_active_locked(&mut s);
    }
}

/// Record an FSM snapshot into the active capture's sidecar. No-op
/// when no capture is active. Should be called once per mic
/// window with the canceller's current state.
pub fn record_snapshot(
    event_type: &str,
    fsm_phase: &str,
    fundamental_hz: f32,
    band_energy: f32,
    anti_amp: f32,
    phase_lock_quality: f32,
) {
    let Ok(mut s) = state().lock() else { return; };
    if !s.enabled { return; }
    let Some(cap) = s.active.as_mut() else { return; };
    cap.metadata.snapshots.push(CaptureSnapshot {
        sample_offset: cap.samples_written,
        event_type: event_type.to_string(),
        fsm_phase: fsm_phase.to_string(),
        fundamental_hz,
        band_energy,
        anti_amp,
        phase_lock_quality,
    });
}

/// Trigger a new capture. Promotes the ring buffer's current
/// contents to a permanent file and starts recording the next
/// CAPTURE_SECS of audio. Idempotent if a capture is already
/// in progress (no-op).
pub fn trigger_capture(trigger_reason: &str) {
    let Ok(mut s) = state().lock() else { return; };
    if !s.enabled || s.active.is_some() { return; }
    enforce_capture_budget(&s.capture_dir);
    let now = SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0);
    let fname_stem = format!("{}_{}", now, sanitize(trigger_reason));
    let wav_path = s.capture_dir.join(format!("{}.wav", fname_stem));
    let sidecar_path = s.capture_dir.join(format!("{}.json", fname_stem));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let writer = match hound::WavWriter::create(&wav_path, spec) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("capture: cannot create {}: {}", wav_path.display(), e);
            return;
        }
    };
    let mut cap = ActiveCapture {
        wav_path,
        sidecar_path,
        writer,
        samples_remaining: CAPTURE_SECS * SAMPLE_RATE as usize,
        samples_written: 0,
        metadata: CaptureMetadata {
            unix_ms: now,
            trigger: trigger_reason.to_string(),
            location_tag: s.location_tag.clone(),
            sample_rate: SAMPLE_RATE,
            total_samples: 0,
            snapshots: Vec::new(),
        },
    };
    // Dump the ring buffer's contents first — that's the pre-event
    // audio. After this, the active-capture path appends the next
    // CAPTURE_SECS of mic windows.
    for &v in s.ring.iter() {
        let i16_val = (v * 32_767.0) as i16;
        let _ = cap.writer.write_sample(i16_val);
        cap.samples_written += 1;
    }
    s.active = Some(cap);
}

fn finalize_active_locked(s: &mut DumpState) {
    let Some(mut cap) = s.active.take() else { return; };
    cap.metadata.total_samples = cap.samples_written;
    // Flush the WAV writer. hound's finalize is consuming; we use
    // its return for error reporting only.
    if let Err(e) = cap.writer.finalize() {
        eprintln!("capture: WAV finalize failed: {}", e);
    }
    if let Ok(json) = serde_json::to_string_pretty(&cap.metadata) {
        let _ = std::fs::write(&cap.sidecar_path, json);
    }
}

fn enforce_capture_budget(dir: &PathBuf) {
    // List .wav files by modified-time ascending; if more than
    // MAX_CAPTURES, delete the oldest. Cheap: directory expected
    // to hold dozens of files, not thousands.
    let Ok(entries) = std::fs::read_dir(dir) else { return; };
    let mut wavs: Vec<(PathBuf, SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension().map(|x| x == "wav").unwrap_or(false) {
                e.metadata().ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| (p, t))
            } else { None }
        })
        .collect();
    if wavs.len() < MAX_CAPTURES { return; }
    wavs.sort_by_key(|(_, t)| *t);
    let to_drop = wavs.len() - MAX_CAPTURES + 1;  // make room for the new one
    for (p, _) in wavs.iter().take(to_drop) {
        let _ = std::fs::remove_file(p);
        // Also remove sidecar if present.
        let json = p.with_extension("json");
        let _ = std::fs::remove_file(json);
    }
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| match c {
        'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
        _ => '_',
    }).collect()
}

/// Convenience: list capture files in date order (newest first).
/// UI uses this to show the user what's been captured.
pub fn list_captures() -> Vec<(PathBuf, u64)> {
    let dir = capture_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new(); };
    let mut list: Vec<(PathBuf, u64)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension().map(|x| x == "wav").unwrap_or(false) {
                e.metadata().ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| (p, d.as_secs()))
            } else { None }
        })
        .collect();
    list.sort_by(|a, b| b.1.cmp(&a.1));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_xdg<F: FnOnce()>(label: &str, f: F) {
        let tmp = std::env::temp_dir().join(format!(
            "bo-cap-{}-{}", label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("XDG_DATA_HOME");
        unsafe { std::env::set_var("XDG_DATA_HOME", &tmp); }
        // Reset state so capture_dir() re-resolves.
        if let Ok(mut s) = state().lock() {
            *s = DumpState {
                enabled: false,
                ring: VecDeque::with_capacity(s.ring_capacity),
                ring_capacity: s.ring_capacity,
                active: None,
                capture_dir: capture_dir(),
                location_tag: String::new(),
            };
        }
        f();
        if let Some(p) = prev {
            unsafe { std::env::set_var("XDG_DATA_HOME", p); }
        } else {
            unsafe { std::env::remove_var("XDG_DATA_HOME"); }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn disabled_ingestion_does_not_record() {
        isolated_xdg("disabled", || {
            set_enabled(false);
            ingest_mic_window(&vec![0.5; 1000]);
            trigger_capture("test");
            // No file should have been created.
            let dir = capture_dir();
            let count = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
            assert_eq!(count, 0, "disabled dump should produce no files");
        });
    }

    #[test]
    fn enable_starts_capturing() {
        isolated_xdg("enable", || {
            set_enabled(true);
            // Pre-populate ring with 5 sec.
            ingest_mic_window(&vec![0.1; 5 * SAMPLE_RATE as usize]);
            trigger_capture("test_trigger");
            // Add another 10 sec to fill the capture.
            ingest_mic_window(&vec![0.2; CAPTURE_SECS * SAMPLE_RATE as usize]);
            let captures = list_captures();
            assert_eq!(captures.len(), 1, "should have one capture");
            let (path, _) = &captures[0];
            // File should exist with reasonable size.
            let meta = std::fs::metadata(path).unwrap();
            // 15 sec * 48k * 2 bytes = 1.44 MB minimum.
            assert!(meta.len() > 1_400_000,
                "capture file should be ~1.4MB+, got {}", meta.len());
            set_enabled(false);
        });
    }

    #[test]
    fn cannot_trigger_two_captures_simultaneously() {
        isolated_xdg("conc", || {
            set_enabled(true);
            ingest_mic_window(&vec![0.1; SAMPLE_RATE as usize]);
            trigger_capture("first");
            trigger_capture("second"); // should be no-op
            ingest_mic_window(&vec![0.2; CAPTURE_SECS * SAMPLE_RATE as usize]);
            let captures = list_captures();
            assert_eq!(captures.len(), 1, "second trigger should not start a new file");
            set_enabled(false);
        });
    }

    #[test]
    fn snapshots_recorded_during_capture() {
        isolated_xdg("snap", || {
            set_enabled(true);
            ingest_mic_window(&vec![0.1; SAMPLE_RATE as usize]);
            trigger_capture("snap_test");
            // Record a few snapshots while writing samples.
            for i in 0..5 {
                ingest_mic_window(&vec![0.0; 2048]);
                record_snapshot("passby", "Peak", 200.0, 0.5, 0.6, 0.85);
                let _ = i;
            }
            // Finalize.
            ingest_mic_window(&vec![0.0; CAPTURE_SECS * SAMPLE_RATE as usize]);
            let captures = list_captures();
            assert_eq!(captures.len(), 1);
            let (wav, _) = &captures[0];
            let sidecar = wav.with_extension("json");
            assert!(sidecar.exists(), "sidecar JSON should exist");
            let meta_str = std::fs::read_to_string(&sidecar).unwrap();
            let meta: CaptureMetadata = serde_json::from_str(&meta_str).unwrap();
            assert!(!meta.snapshots.is_empty(), "snapshots should be recorded");
            assert_eq!(meta.snapshots[0].event_type, "passby");
            assert_eq!(meta.snapshots[0].fsm_phase, "Peak");
            set_enabled(false);
        });
    }

    #[test]
    fn budget_enforced_evicts_oldest() {
        isolated_xdg("budget", || {
            // Create MAX_CAPTURES+3 capture files manually with
            // staggered modified-times to test eviction.
            let dir = capture_dir();
            for i in 0..MAX_CAPTURES + 3 {
                let spec = hound::WavSpec {
                    channels: 1, sample_rate: 48000,
                    bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
                };
                let path = dir.join(format!("{:05}_test.wav", i));
                let writer = hound::WavWriter::create(&path, spec).unwrap();
                writer.finalize().unwrap();
                // Stagger mtime so eviction order is predictable.
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), MAX_CAPTURES + 3);
            enforce_capture_budget(&dir);
            // Should have evicted enough that we're at MAX_CAPTURES - 1
            // (making room for one more).
            let remaining = std::fs::read_dir(&dir).unwrap().count();
            assert!(remaining < MAX_CAPTURES,
                "budget should evict files, got {} of {} max", remaining, MAX_CAPTURES);
        });
    }

    #[test]
    fn location_tag_propagates_to_sidecar() {
        isolated_xdg("tag", || {
            set_enabled(true);
            set_location_tag("bedroom_window_north");
            ingest_mic_window(&vec![0.1; SAMPLE_RATE as usize]);
            trigger_capture("loc_test");
            ingest_mic_window(&vec![0.0; CAPTURE_SECS * SAMPLE_RATE as usize]);
            let captures = list_captures();
            let (wav, _) = &captures[0];
            let sidecar = wav.with_extension("json");
            let meta_str = std::fs::read_to_string(&sidecar).unwrap();
            let meta: CaptureMetadata = serde_json::from_str(&meta_str).unwrap();
            assert_eq!(meta.location_tag, "bedroom_window_north");
            set_enabled(false);
        });
    }
}
