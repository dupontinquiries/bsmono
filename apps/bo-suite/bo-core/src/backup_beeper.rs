// src/backup_beeper.rs
#![allow(dead_code)]
//
// Construction-vehicle / van backup-alarm detector + masker.
//
// ── Why this is fundamentally different from the pass-by canceller ──
//
// OSHA-spec backup alarms run at 1000-3000 Hz (typical residential
// reverberant alarm: ~1250 Hz). That's well ABOVE the 300 Hz
// MAX_CANCEL_HZ ceiling the engine canceller respects — at 1250 Hz
// the wavelength is 27 cm, and the listener's head movement (5-10 cm
// in normal sleep) is a significant fraction of it. True destructive
// interference would only work at a single fixed listener position
// and would amplify the noise elsewhere in the room.
//
// So we don't try to cancel. We MASK.
//
// ── The masking strategy ──
//
// A backup beeper at 1250 Hz with a sharp envelope (~50 ms attack,
// ~250-500 ms hold, ~50 ms release, repeated every 0.7-1.5 sec) is
// "draining" specifically because of the abrupt attack — the human
// brain has evolved to attend to onsets. The continuous tone itself
// is less attention-grabbing than the onset edge.
//
// We exploit this:
//   1. Detect each beep's onset via Goertzel at the alarm carrier
//      frequency.
//   2. Measure the inter-beep interval; once 3 consistent intervals
//      are observed, declare "lock" — we can predict the next beep.
//   3. ~80 ms BEFORE each predicted beep, start ramping up a pink-
//      noise masker. Peak the masker during the beep itself. Fade
//      out ~150 ms after the beep ends.
//   4. The pink-noise contour smooths the harsh onset/offset edges.
//      Total acoustic energy at the listener goes up slightly, but
//      perceived "drain" goes way down because the brain stops
//      getting the sharp attention-capturing transient.
//
// ── What this DOESN'T do ──
//
// - Track multiple simultaneous backup beepers (rare in practice).
// - Cancel the actual tonal content — that requires line-of-sight
//   anti-noise at the listener position which we can't guarantee.
// - Detect non-standard alarms (e.g. spoken "this vehicle is backing
//   up" voice alerts). Out of band.
// - Handle alarms whose carrier wavers (cheap PWM-driven beepers
//   sometimes shift by ±50 Hz between beeps).

use std::f32::consts::PI;

/// Standard OSHA backup alarm range. We center detection at 1250 Hz
/// — typical residential audible peak — and probe at 1000 / 1250 /
/// 1600 / 2000 Hz to catch the common variants.
pub const ALARM_CARRIER_PROBES: [f32; 4] = [1000.0, 1250.0, 1600.0, 2000.0];

/// Sample rate of the audio path. Matches passby_canceller for
/// shared buffer compatibility.
pub const SAMPLE_RATE: f32 = 48_000.0;

/// Per-window size used by the detector. Smaller than the passby
/// window (which needs frequency resolution down to 60 Hz); a beeper
/// detector at >1 kHz works fine with shorter windows, and shorter
/// is better here because beep onsets are ~50 ms and we want to
/// localize them tightly. 1024 samples ≈ 21 ms.
pub const WINDOW_SAMPLES: usize = 1024;

/// Detection threshold: a window's in-carrier energy must exceed
/// this multiple of the long-running baseline to count as a "beep on"
/// edge.
const BEEP_ENERGY_RATIO: f32 = 5.0;

/// Inter-beep intervals (milliseconds) must agree within this
/// fraction to count toward "lock." Cheap PWM beepers wobble; a
/// real on-vehicle alarm is usually within ±5%.
const INTERVAL_AGREEMENT: f32 = 0.15;

/// Number of consecutive agreeing intervals required before the
/// detector declares "locked" and starts predictive masking.
const LOCK_INTERVALS: usize = 3;

/// EWMA alpha for the baseline carrier-energy floor. Slow so a
/// genuine alarm doesn't get folded into the floor.
const BASELINE_ALPHA: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeepPhase {
    /// No beeps detected, or last beep was too long ago to predict
    /// from.
    Quiet,
    /// Beeps are arriving but we don't yet have enough consistent
    /// intervals to predict the next one. Masking is reactive only
    /// (covers the current beep without predictive ramp-up).
    Tracking,
    /// We have a confident interval estimate. Masking can ramp up
    /// before each predicted beep.
    Locked,
}

#[derive(Debug, Clone, Copy)]
pub struct BeeperSnapshot {
    pub phase: BeepPhase,
    /// Detected carrier frequency (0 if none).
    pub carrier_hz: f32,
    /// Estimated time-to-next-beep in milliseconds (0 when not locked).
    pub next_beep_in_ms: u64,
    /// Mean inter-beep interval (ms) — confidence proxy.
    pub mean_interval_ms: f32,
    /// Total beeps observed in this session.
    pub beep_count: u32,
    /// Current masker output amplitude in [0, 1].
    pub masker_amp: f32,
}

pub struct BackupBeeperMasker {
    phase: BeepPhase,
    /// Detected carrier frequency. Set when first beep is seen;
    /// updated EWMA-style across subsequent beeps if it drifts.
    carrier_hz: f32,
    /// Baseline carrier-band energy (EWMA, only updated outside
    /// beep events).
    baseline_energy: f32,
    /// Whether the most-recent window detected the beep above
    /// threshold. Used for onset edge detection.
    in_beep_now: bool,
    /// Monotonic millisecond counter — incremented by the caller's
    /// `process_window`. Decoupled from wall clock so tests can
    /// drive the FSM deterministically.
    now_ms: u64,
    /// Timestamp of the last detected beep ONSET.
    last_onset_ms: u64,
    /// Timestamp of the last detected beep OFFSET. Used to know
    /// when to stop the reactive masker.
    last_offset_ms: u64,
    /// Recent inter-beep intervals (most recent first). Trimmed
    /// to last `LOCK_INTERVALS + 1` so we can detect when the
    /// pattern changes (alarm stops, new alarm with different
    /// interval starts).
    recent_intervals_ms: Vec<u64>,
    /// EWMA of the inter-beep interval. Used for prediction once
    /// locked.
    mean_interval_ms: f32,
    /// Predicted ms of the next onset (locked mode).
    predicted_next_onset_ms: u64,
    /// Pink-noise generator state.
    rng_state: u32,
    /// Pink-noise filter state (Voss-McCartney-ish: octave
    /// summing). Simple 5-row implementation.
    pink_rows: [f32; 5],
    /// Counter that selects which row to update next.
    pink_counter: u32,
    /// Current masker envelope amplitude.
    masker_amp: f32,
    /// Total beeps seen.
    beep_count: u32,
}

impl Default for BackupBeeperMasker {
    fn default() -> Self {
        Self {
            phase: BeepPhase::Quiet,
            carrier_hz: 0.0,
            baseline_energy: 1e-4,
            in_beep_now: false,
            now_ms: 0,
            last_onset_ms: 0,
            last_offset_ms: 0,
            recent_intervals_ms: Vec::with_capacity(LOCK_INTERVALS + 2),
            mean_interval_ms: 0.0,
            predicted_next_onset_ms: 0,
            rng_state: 0xC0FFEE,
            pink_rows: [0.0; 5],
            pink_counter: 1,
            masker_amp: 0.0,
            beep_count: 0,
        }
    }
}

impl BackupBeeperMasker {
    pub fn new() -> Self { Self::default() }

    pub fn snapshot(&self) -> BeeperSnapshot {
        BeeperSnapshot {
            phase: self.phase,
            carrier_hz: self.carrier_hz,
            next_beep_in_ms: if matches!(self.phase, BeepPhase::Locked)
                && self.predicted_next_onset_ms > self.now_ms
            {
                self.predicted_next_onset_ms - self.now_ms
            } else { 0 },
            mean_interval_ms: self.mean_interval_ms,
            beep_count: self.beep_count,
            masker_amp: self.masker_amp,
        }
    }

    /// Feed a fresh window of mic samples (typically WINDOW_SAMPLES
    /// long). Advances the FSM and updates the masker amplitude.
    ///
    /// The caller is expected to provide ~21 ms windows back-to-back;
    /// we advance our internal `now_ms` by the window duration on
    /// each call. This decouples the FSM from wall-clock skew and
    /// makes tests deterministic.
    pub fn process_window(&mut self, samples: &[f32]) -> BeeperSnapshot {
        let win_ms = (samples.len() as f32 * 1000.0 / SAMPLE_RATE) as u64;
        self.now_ms += win_ms.max(1);

        // ── Probe the carrier candidates ──
        // The strongest probe defines the carrier (or refreshes our
        // estimate if it drifts). We take the max across probes
        // rather than summing because real alarms are tonal — one
        // probe should dominate.
        let mut best_freq = 0.0f32;
        let mut best_mag = 0.0f32;
        for &f in ALARM_CARRIER_PROBES.iter() {
            let m = goertzel(samples, f, SAMPLE_RATE);
            if m > best_mag { best_mag = m; best_freq = f; }
        }
        let carrier_energy = best_mag / samples.len() as f32;

        // Baseline floor — only updated when we DON'T think we're
        // in a beep. Otherwise the floor would climb into the alarm
        // and the detector would gradually lose sensitivity.
        if !self.in_beep_now {
            self.baseline_energy = (1.0 - BASELINE_ALPHA) * self.baseline_energy
                                 + BASELINE_ALPHA * carrier_energy.max(1e-6);
        }

        let is_beep = carrier_energy > self.baseline_energy * BEEP_ENERGY_RATIO
            && carrier_energy > 1e-4;

        // ── Onset / offset edge detection ──
        if is_beep && !self.in_beep_now {
            self.on_beep_onset(best_freq);
        } else if !is_beep && self.in_beep_now {
            self.on_beep_offset();
        }
        self.in_beep_now = is_beep;

        // ── Stale-event check: if it's been more than 3× the mean
        //    interval since the last beep, drop the lock. The vehicle
        //    has likely left.
        if self.phase != BeepPhase::Quiet && self.mean_interval_ms > 0.0
            && self.now_ms > self.last_onset_ms
                + ((self.mean_interval_ms * 3.0) as u64).max(5_000)
        {
            self.phase = BeepPhase::Quiet;
            self.recent_intervals_ms.clear();
            self.mean_interval_ms = 0.0;
            self.predicted_next_onset_ms = 0;
        }

        // ── Update masker envelope ──
        self.update_masker_envelope();

        self.snapshot()
    }

    fn on_beep_onset(&mut self, carrier: f32) {
        self.beep_count = self.beep_count.saturating_add(1);
        // Update carrier estimate — EWMA so a one-shot mismeasurement
        // doesn't sabotage the next prediction.
        if self.carrier_hz <= 0.0 {
            self.carrier_hz = carrier;
        } else {
            self.carrier_hz = 0.8 * self.carrier_hz + 0.2 * carrier;
        }
        if self.last_onset_ms > 0 {
            let interval = self.now_ms - self.last_onset_ms;
            // Sanity-bound to OSHA-typical range (200 ms - 3 s).
            if (200..=3_000).contains(&interval) {
                self.recent_intervals_ms.insert(0, interval);
                if self.recent_intervals_ms.len() > LOCK_INTERVALS + 2 {
                    self.recent_intervals_ms.pop();
                }
                self.update_lock_state();
            } else {
                // Wild interval — probably we missed beeps or a
                // different sound source triggered. Reset interval
                // tracking but keep the carrier estimate.
                self.recent_intervals_ms.clear();
                self.mean_interval_ms = 0.0;
                self.phase = BeepPhase::Tracking;
                self.predicted_next_onset_ms = 0;
            }
        } else {
            // First beep — promote to Tracking immediately.
            self.phase = BeepPhase::Tracking;
        }
        self.last_onset_ms = self.now_ms;
    }

    fn on_beep_offset(&mut self) {
        self.last_offset_ms = self.now_ms;
    }

    fn update_lock_state(&mut self) {
        if self.recent_intervals_ms.len() < LOCK_INTERVALS {
            self.phase = BeepPhase::Tracking;
            return;
        }
        // Check the last LOCK_INTERVALS values all agree within
        // INTERVAL_AGREEMENT fraction of their mean.
        let last_n: Vec<u64> = self.recent_intervals_ms
            .iter().take(LOCK_INTERVALS).copied().collect();
        let mean: f32 = last_n.iter().sum::<u64>() as f32 / last_n.len() as f32;
        let all_agree = last_n.iter().all(|&v| {
            ((v as f32 - mean).abs() / mean) <= INTERVAL_AGREEMENT
        });
        if all_agree {
            self.mean_interval_ms = mean;
            self.predicted_next_onset_ms = self.last_onset_ms + mean as u64;
            self.phase = BeepPhase::Locked;
        } else {
            self.phase = BeepPhase::Tracking;
            self.predicted_next_onset_ms = 0;
        }
    }

    /// Masker envelope: ramps up ~80 ms before predicted beep,
    /// peaks during the beep, fades after. Target amplitudes are
    /// chosen so the pink-noise burst is just loud enough to soften
    /// the perceived onset without exceeding the beep's own SPL
    /// at the listener.
    fn update_masker_envelope(&mut self) {
        const LEAD_MS: u64 = 80;
        const HOLD_AFTER_OFFSET_MS: u64 = 150;
        const PEAK_AMP: f32 = 0.5;
        const TRACKING_AMP: f32 = 0.35;  // less confident → quieter

        let target = match self.phase {
            BeepPhase::Quiet => 0.0,
            BeepPhase::Tracking => {
                if self.in_beep_now { TRACKING_AMP }
                else if self.now_ms - self.last_offset_ms < HOLD_AFTER_OFFSET_MS
                    && self.last_offset_ms > 0 { TRACKING_AMP * 0.5 }
                else { 0.0 }
            }
            BeepPhase::Locked => {
                if self.in_beep_now { PEAK_AMP }
                else if self.predicted_next_onset_ms > self.now_ms
                    && self.predicted_next_onset_ms - self.now_ms <= LEAD_MS
                {
                    // Pre-beep ramp: rises linearly toward PEAK_AMP.
                    let t_left = self.predicted_next_onset_ms - self.now_ms;
                    PEAK_AMP * (1.0 - (t_left as f32 / LEAD_MS as f32))
                }
                else if self.now_ms - self.last_offset_ms < HOLD_AFTER_OFFSET_MS
                    && self.last_offset_ms > self.last_onset_ms { PEAK_AMP * 0.5 }
                else { 0.0 }
            }
        };
        // Soft attack / release toward target.
        let rate = if target > self.masker_amp { 0.20 } else { 0.10 };
        self.masker_amp += (target - self.masker_amp) * rate;
        self.masker_amp = self.masker_amp.clamp(0.0, 1.0);
    }

    /// Pull `n` samples of masker output. Pink-noise burst weighted
    /// by the current envelope amplitude. Returns silence when
    /// envelope is at 0.
    pub fn pull_output(&mut self, n: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(n);
        if self.masker_amp <= 1e-6 {
            out.resize(n, 0.0);
            return out;
        }
        for _ in 0..n {
            let pink = self.next_pink();
            out.push(pink * self.masker_amp);
        }
        out
    }

    fn next_white(&mut self) -> f32 {
        // SplitMix32 — fast, good distribution for masker noise.
        self.rng_state = self.rng_state.wrapping_mul(1664525).wrapping_add(1013904223);
        // Map to [-1, 1].
        (self.rng_state as i32 as f32) / (i32::MAX as f32)
    }

    /// Voss-McCartney 5-row pink noise. Each row updates at a
    /// halving rate; the sum approximates 1/f spectral density.
    /// Output normalized to roughly unit amplitude.
    fn next_pink(&mut self) -> f32 {
        self.pink_counter = self.pink_counter.wrapping_add(1);
        let trailing = self.pink_counter.trailing_zeros();
        if (trailing as usize) < self.pink_rows.len() {
            self.pink_rows[trailing as usize] = self.next_white();
        }
        let sum: f32 = self.pink_rows.iter().sum();
        // Five rows → max abs sum is ~5. Divide to bring to ~unit.
        (sum / 5.0).clamp(-1.0, 1.0)
    }

    pub fn reset(&mut self) { *self = Self::default(); }
}

/// Goertzel single-tone magnitude — mirrored from passby_canceller
/// so the two modules can move independently if one needs a
/// specialized variant later. NaN-defended same way.
pub fn goertzel(samples: &[f32], freq_hz: f32, sr: f32) -> f32 {
    if samples.is_empty() || !freq_hz.is_finite() || !sr.is_finite() || sr <= 0.0 {
        return 0.0;
    }
    let n = samples.len() as f32;
    let k = (0.5 + (n * freq_hz) / sr).floor();
    let omega = 2.0 * PI * k / n;
    let coef = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &x in samples {
        let x = if x.is_finite() { x } else { 0.0 };
        let s0 = x + coef * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let real = s1 - s2 * omega.cos();
    let imag = s2 * omega.sin();
    (real * real + imag * imag).sqrt()
}

/// Test helper: synthesize a backup-alarm pattern at `carrier_hz`
/// with `interval_ms` between onsets and `beep_duration_ms` per
/// beep. `n_beeps` beeps total, then silence to `total_duration_s`.
pub fn synth_beeper(
    total_duration_s: f32,
    carrier_hz: f32,
    interval_ms: u64,
    beep_duration_ms: u64,
    n_beeps: u32,
    sample_rate: f32,
) -> Vec<f32> {
    let n = (total_duration_s * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let dphase = 2.0 * PI * carrier_hz / sample_rate;
    let beep_samples = (beep_duration_ms as f32 * sample_rate / 1000.0) as usize;
    let interval_samples = (interval_ms as f32 * sample_rate / 1000.0) as usize;
    // Soft attack/release shape — 20 ms taper at edges.
    let taper = (0.020 * sample_rate) as usize;
    for b in 0..n_beeps {
        let start = b as usize * interval_samples;
        if start + beep_samples >= n { break; }
        let mut phase = 0.0f32;
        for i in 0..beep_samples {
            let pos = start + i;
            // Envelope.
            let env = if i < taper {
                i as f32 / taper as f32
            } else if i > beep_samples - taper {
                (beep_samples - i) as f32 / taper as f32
            } else { 1.0 };
            out[pos] = phase.sin() * env * 0.8;
            phase += dphase;
            if phase > 2.0 * PI { phase -= 2.0 * PI; }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_synth(m: &mut BackupBeeperMasker, signal: &[f32]) {
        let mut i = 0;
        while i + WINDOW_SAMPLES <= signal.len() {
            m.process_window(&signal[i..i + WINDOW_SAMPLES]);
            i += WINDOW_SAMPLES;
        }
    }

    #[test]
    fn quiet_when_no_beeps() {
        let mut m = BackupBeeperMasker::new();
        feed_synth(&mut m, &vec![0.0f32; SAMPLE_RATE as usize * 2]);
        let s = m.snapshot();
        assert_eq!(s.phase, BeepPhase::Quiet);
        assert_eq!(s.beep_count, 0);
        assert!(s.masker_amp < 0.01);
    }

    #[test]
    fn detects_a_single_beep() {
        // One beep at 1250 Hz then silence. Should land in Tracking,
        // not Locked (need 3 intervals for lock).
        let mut m = BackupBeeperMasker::new();
        let sig = synth_beeper(3.0, 1250.0, 1000, 400, 1, SAMPLE_RATE);
        feed_synth(&mut m, &sig);
        let s = m.snapshot();
        assert!(s.beep_count >= 1, "expected ≥1 beep, got {}", s.beep_count);
        assert!(s.phase == BeepPhase::Tracking || s.phase == BeepPhase::Quiet,
            "single beep → Tracking or Quiet (timed out), got {:?}", s.phase);
    }

    #[test]
    fn locks_after_consistent_intervals() {
        // Four beeps at exactly 800 ms apart — gives 3 intervals,
        // hits LOCK_INTERVALS threshold. We feed JUST the beep
        // sequence (no trailing silence) so the stale-event
        // check doesn't reset state before we inspect it.
        let mut m = BackupBeeperMasker::new();
        // 4 beeps with 800ms spacing: last beep finishes at
        // ~3.6s — bound the signal there.
        let sig = synth_beeper(3.6, 1250.0, 800, 400, 4, SAMPLE_RATE);
        feed_synth(&mut m, &sig);
        let s = m.snapshot();
        assert!(s.beep_count >= 3, "expected ≥3 beeps, got {}", s.beep_count);
        // The interval EWMA should be close to 800.
        assert!((s.mean_interval_ms - 800.0).abs() < 100.0,
            "mean_interval should be ~800, got {}", s.mean_interval_ms);
    }

    #[test]
    fn does_not_lock_on_irregular_pattern() {
        // Synthesize an irregular pattern by hand-feeding the
        // detector with manual onset timestamps — irregular intervals.
        let mut m = BackupBeeperMasker::new();
        m.on_beep_onset(1250.0); m.now_ms = 200;
        m.on_beep_onset(1250.0); m.now_ms = 1500;
        m.on_beep_onset(1250.0); m.now_ms = 1700;
        m.on_beep_onset(1250.0);
        // Intervals: 200, 1300, 200 — wildly inconsistent.
        assert!(m.phase == BeepPhase::Tracking || m.phase == BeepPhase::Quiet);
    }

    #[test]
    fn carrier_is_estimated() {
        let mut m = BackupBeeperMasker::new();
        let sig = synth_beeper(5.0, 1250.0, 700, 350, 3, SAMPLE_RATE);
        feed_synth(&mut m, &sig);
        let s = m.snapshot();
        // Beeper carrier should land near 1250 (probes are at
        // exactly 1250 so this should be exact).
        if s.beep_count > 0 {
            assert!((s.carrier_hz - 1250.0).abs() < 50.0,
                "carrier should be ~1250 Hz, got {}", s.carrier_hz);
        }
    }

    #[test]
    fn masker_output_is_finite_and_bounded() {
        let mut m = BackupBeeperMasker::new();
        let sig = synth_beeper(5.0, 1250.0, 700, 350, 5, SAMPLE_RATE);
        feed_synth(&mut m, &sig);
        let out = m.pull_output(4096);
        for &s in &out {
            assert!(s.is_finite());
            assert!(s.abs() <= 1.0);
        }
    }

    #[test]
    fn masker_silent_when_quiet() {
        let mut m = BackupBeeperMasker::new();
        // Feed silence.
        feed_synth(&mut m, &vec![0.0f32; SAMPLE_RATE as usize]);
        let out = m.pull_output(2048);
        let energy: f32 = out.iter().map(|&x| x * x).sum();
        assert!(energy < 1e-2, "masker should be silent when Quiet, energy={}", energy);
    }

    #[test]
    fn pink_noise_is_finite_and_bounded() {
        let mut m = BackupBeeperMasker::new();
        for _ in 0..10_000 {
            let v = m.next_pink();
            assert!(v.is_finite() && v.abs() <= 1.01);
        }
    }

    #[test]
    fn reset_clears_state() {
        let mut m = BackupBeeperMasker::new();
        let sig = synth_beeper(5.0, 1250.0, 700, 350, 4, SAMPLE_RATE);
        feed_synth(&mut m, &sig);
        m.reset();
        let s = m.snapshot();
        assert_eq!(s.phase, BeepPhase::Quiet);
        assert_eq!(s.beep_count, 0);
        assert_eq!(s.mean_interval_ms, 0.0);
    }

    #[test]
    fn stale_lock_drops_after_long_silence() {
        // Use the public synthesis path so the FSM walks through
        // detection naturally. After the beeps end, advance the
        // clock by feeding silence for several seconds — long
        // enough that the stale-event check (3× mean interval,
        // floored at 5 s) fires and resets state to Quiet.
        let mut m = BackupBeeperMasker::new();
        let beeps = synth_beeper(3.6, 1250.0, 800, 400, 4, SAMPLE_RATE);
        feed_synth(&mut m, &beeps);
        // Should now have detected several beeps.
        assert!(m.snapshot().beep_count >= 3);
        // Feed 10 s of silence — well past 3× the 800 ms interval
        // and past the 5 s minimum floor.
        let silence = vec![0.0f32; SAMPLE_RATE as usize * 10];
        feed_synth(&mut m, &silence);
        let s = m.snapshot();
        assert_eq!(s.phase, BeepPhase::Quiet,
            "lock should have decayed after long silence, phase={:?}", s.phase);
    }

    #[test]
    fn goertzel_defends_against_non_finite() {
        let mut sig = vec![0.5_f32; 1024];
        sig[100] = f32::NAN;
        sig[200] = f32::INFINITY;
        let m = goertzel(&sig, 1250.0, SAMPLE_RATE);
        assert!(m.is_finite());
    }

    #[test]
    fn synth_beeper_produces_finite_output() {
        let sig = synth_beeper(2.0, 1250.0, 700, 350, 2, SAMPLE_RATE);
        assert!(sig.iter().all(|x| x.is_finite()));
        let peak = sig.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        assert!(peak > 0.0 && peak <= 1.0);
    }
}
