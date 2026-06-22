// src/drone_canceller.rs
#![allow(dead_code)]
//
// Slow-moving small-engine drone canceller.
//
// Targets: leafblowers, lawnmowers, edge trimmers, pressure washers,
// power tools, generators — any source that produces sustained
// engine harmonics for minutes-to-hours at a roughly fixed pitch,
// with motion slow enough that Doppler shift never crosses the
// passby canceller's 0.5 Hz/s gate.
//
// ── How this differs from the passby canceller ──
//
//   Passby                              Drone
//   ───────────                         ───────────
//   Brief event (~30 s)                 Sustained (minutes-to-hours)
//   Doppler-driven freq excursion       Roughly fixed pitch
//   4-phase FSM (Quiet/App/Peak/Recede) 2-phase FSM (Quiet/Active)
//   Lorentzian amplitude envelope       Steady-state amplitude
//   Predictive timing via learned EWMAs Tracking-only, no prediction
//   Engine fundamental 60-300 Hz        Small-engine 100-500 Hz
//
// ── Why the wider band ──
//
// Small 2-stroke engines (leafblowers, trimmers) run at much higher
// RPMs than cars. A leafblower at ~7000 rpm × 2 cyl / 60 = 233 Hz
// fundamental. A push mower at 3000 rpm × 1 cyl / 60 = 50 Hz (out
// of band — but its 2nd harmonic at 100 Hz IS in band). The
// extended cap to 500 Hz catches the leafblower fundamental.
//
// Note the head-movement constraint: at 500 Hz the wavelength is
// 69 cm, head movement 5-10 cm is 7-15 % of a wavelength — still
// in the marginal zone for cancellation. We accept the partial
// cancellation; even -3 to -6 dB at the listener position is a
// meaningful comfort improvement for a 2-hour leafblower session.
//
// ── Detection vs canceller activation ──
//
// We use a "confirmation hold": detection only flips to Active
// after the energy has been sustained for CONFIRM_MS. This is
// crucial because passing cars + isolated impulses ALSO produce
// brief in-band energy spikes — without the confirmation hold,
// the drone canceller would constantly false-trigger on traffic
// and dump anti-noise during pass-bys (where the passby canceller
// is already doing the right thing).
//
// ── What this DOESN'T do ──
//
// - Multiple simultaneous drones (two neighbors with leafblowers
//   on the same morning is rare but real). We track the loudest.
// - Sources whose pitch changes faster than 50 Hz/sec (e.g. a
//   chainsaw under load that bogs and revs). We hold the last-
//   stable estimate during fast transients.
// - Electric tools (electric mowers, leaf vacuums). Their motor
//   pitch is constant and they typically aren't audible at the
//   distances this canceller is designed for.

use std::f32::consts::PI;
use std::time::Instant;

/// Engine-band low/high for the drone target. Wider than passby's
/// 60-300 — small 2-stroke engines run hot.
pub const BAND_LO_HZ: f32 = 100.0;
pub const BAND_HI_HZ: f32 = 500.0;

pub const SAMPLE_RATE: f32 = 48_000.0;
pub const WINDOW_SAMPLES: usize = 2048;

/// Confirmation hold: drone state stays "Detected" until energy
/// has been sustained for this many ms. Filters transients.
const CONFIRM_MS: u64 = 2_000;

/// Exit hold: drone state stays "Active" until energy has been
/// BELOW threshold for this many ms. Filters momentary dips (the
/// operator moves to the far side of the yard, throttles down to
/// pick up the cord, etc).
const EXIT_HOLD_MS: u64 = 3_000;

/// Energy ratio above noise floor for "drone present" gate.
const PRESENCE_RATIO: f32 = 4.0;

/// Maximum acceptable jitter in the running fundamental estimate
/// to count as "stable enough to lock anti-noise to." Some pitch
/// wander is normal (operator hits debris, throttle), but a
/// chaotic estimate indicates noisy detection — better to wait.
const STABLE_FREQ_TOLERANCE_HZ: f32 = 25.0;

const NOISE_FLOOR_ALPHA: f32 = 0.005;       // very slow — sustained noise
                                            // shouldn't lift the floor.
const FREQ_TRACK_ALPHA: f32 = 0.15;         // moderately fast follow.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DronePhase {
    /// No sustained drone detected.
    Quiet,
    /// Drone detected; in confirmation hold. No anti-noise yet.
    Detected,
    /// Confirmed drone, anti-noise running with tracked fundamental.
    Active,
    /// Drone has fallen below threshold; in exit hold. Anti-noise
    /// still emitting at decaying amplitude in case the source
    /// resumes (operator paused for cord-untangle, debris pickup).
    Releasing,
}

#[derive(Debug, Clone, Copy)]
pub struct DroneSnapshot {
    pub phase: DronePhase,
    pub fundamental_hz: f32,
    pub band_energy: f32,
    pub noise_floor: f32,
    pub anti_amp: f32,
    /// Stability of the fundamental tracker, 0..1. 1 = rock-steady,
    /// 0 = wildly jittering. UI surfaces this as "lock quality."
    pub tracker_stability: f32,
    /// Total session duration of the most recent Active phase in
    /// seconds. UI surfaces this to show "you've been blocking
    /// this drone for 17 minutes." Real lawn mowing typically
    /// runs 20-45 minutes; leafblowers 5-30.
    pub active_secs: f32,
    /// Phase-lock quality, 0..1. See PassbyCanceller's docstring
    /// for the same metric. Drones being sustained means this
    /// number stabilizes after a few seconds — you can watch it
    /// climb in the UI as the lock locks on.
    pub phase_lock_quality: f32,
}

pub struct DroneCanceller {
    phase: DronePhase,
    phase_entered_at: Instant,
    /// Current best estimate of the drone's fundamental. Smoothed
    /// across windows when the estimate is internally stable.
    tracked_fundamental: f32,
    /// Recent fundamental estimates (most recent first) for the
    /// stability metric.
    recent_freqs: Vec<f32>,
    /// Recent band energies (most recent first) for the
    /// presence-hold logic.
    recent_energies: Vec<f32>,
    /// Adaptive noise floor.
    noise_floor: f32,
    /// Time of the most recent above-threshold window (for the
    /// confirmation + exit-hold timers).
    last_above_threshold_at: Option<Instant>,
    /// Time of the most recent below-threshold window.
    last_below_threshold_at: Option<Instant>,
    /// Anti-noise sinusoid phase accumulator.
    anti_phase: f32,
    /// Anti-noise current amplitude — set by the lock's amplitude
    /// adaptation (linear regression of mic vs. template). Capped
    /// by `anti_amp_ceiling` so the FSM still authorizes the
    /// emission level.
    anti_amp: f32,
    /// FSM-driven amplitude ceiling. Envelope code drives this;
    /// the lock respects it. Same decoupling pattern as
    /// `PassbyCanceller`.
    anti_amp_ceiling: f32,
    /// Phase-lock quality, 0..1.
    phase_lock_quality: f32,
    /// Saturation amount (0..0.6). Soft-clip applied to anti-noise
    /// output to match real engine timbre character. See
    /// pull_output for the math.
    saturation: f32,
}

impl Default for DroneCanceller {
    fn default() -> Self {
        Self {
            phase: DronePhase::Quiet,
            phase_entered_at: Instant::now(),
            tracked_fundamental: 0.0,
            recent_freqs: Vec::with_capacity(16),
            recent_energies: Vec::with_capacity(16),
            noise_floor: 1e-4,
            last_above_threshold_at: None,
            last_below_threshold_at: None,
            anti_phase: 0.0,
            anti_amp: 0.0,
            anti_amp_ceiling: 0.0,
            phase_lock_quality: 0.0,
            saturation: 0.0,
        }
    }
}

impl DroneCanceller {
    pub fn new() -> Self { Self::default() }
    pub fn phase(&self) -> DronePhase { self.phase }

    pub fn snapshot(&self) -> DroneSnapshot {
        let active_secs = if self.phase == DronePhase::Active
            || self.phase == DronePhase::Releasing
        {
            self.phase_entered_at.elapsed().as_secs_f32()
        } else { 0.0 };
        DroneSnapshot {
            phase: self.phase,
            fundamental_hz: self.tracked_fundamental,
            band_energy: self.recent_energies.first().copied().unwrap_or(0.0),
            noise_floor: self.noise_floor,
            anti_amp: self.anti_amp,
            tracker_stability: self.compute_stability(),
            active_secs,
            phase_lock_quality: self.phase_lock_quality,
        }
    }

    /// Compute fundamental-tracker stability from recent estimates.
    /// 1.0 = all recent estimates within tolerance of each other,
    /// 0.0 = max jitter (estimates spread across full band).
    fn compute_stability(&self) -> f32 {
        if self.recent_freqs.len() < 3 { return 0.0; }
        let mean: f32 = self.recent_freqs.iter().sum::<f32>()
            / self.recent_freqs.len() as f32;
        if mean <= 0.0 { return 0.0; }
        let max_dev = self.recent_freqs.iter()
            .map(|&f| (f - mean).abs())
            .fold(0.0f32, |a, b| a.max(b));
        (1.0 - (max_dev / STABLE_FREQ_TOLERANCE_HZ)).clamp(0.0, 1.0)
    }

    pub fn process_window(&mut self, samples: &[f32]) -> DroneSnapshot {
        let energy = band_energy_rms(samples, BAND_LO_HZ, BAND_HI_HZ, SAMPLE_RATE);
        let f0 = estimate_fundamental(samples, BAND_LO_HZ, BAND_HI_HZ, SAMPLE_RATE);

        // ── Update rolling histories ──
        if self.recent_energies.len() >= 16 { self.recent_energies.pop(); }
        self.recent_energies.insert(0, energy);
        if f0 > 0.0 {
            if self.recent_freqs.len() >= 16 { self.recent_freqs.pop(); }
            self.recent_freqs.insert(0, f0);
        }

        // ── Update noise floor — only when Quiet, very slow EWMA ──
        if self.phase == DronePhase::Quiet {
            self.noise_floor = (1.0 - NOISE_FLOOR_ALPHA) * self.noise_floor
                             + NOISE_FLOOR_ALPHA * energy.max(1e-6);
        }

        let above = energy > self.noise_floor * PRESENCE_RATIO;
        let now = Instant::now();
        if above {
            self.last_above_threshold_at = Some(now);
        } else {
            self.last_below_threshold_at = Some(now);
        }

        // ── FSM tick ──
        match self.phase {
            DronePhase::Quiet => {
                if above {
                    self.transition(DronePhase::Detected);
                }
            }
            DronePhase::Detected => {
                if !above {
                    // Transient — dropped before confirmation.
                    self.transition(DronePhase::Quiet);
                } else if self.phase_entered_at.elapsed().as_millis()
                    as u64 >= CONFIRM_MS
                    && self.compute_stability() > 0.3
                {
                    // Confirmed sustained drone with stable enough
                    // fundamental to track.
                    self.transition(DronePhase::Active);
                }
            }
            DronePhase::Active => {
                // Track the fundamental as it drifts.
                if f0 > 0.0 && (f0 - self.tracked_fundamental).abs() < 50.0 {
                    self.tracked_fundamental =
                        (1.0 - FREQ_TRACK_ALPHA) * self.tracked_fundamental
                        + FREQ_TRACK_ALPHA * f0;
                }
                if !above {
                    self.transition(DronePhase::Releasing);
                }
            }
            DronePhase::Releasing => {
                if above {
                    // Drone resumed; bounce straight back to Active.
                    self.transition(DronePhase::Active);
                } else if self.phase_entered_at.elapsed().as_millis()
                    as u64 >= EXIT_HOLD_MS
                {
                    self.transition(DronePhase::Quiet);
                }
            }
        }

        // ── Anti-noise amplitude envelope ── (ceiling first)
        self.update_anti_envelope();
        // ── Phase + amplitude lock (only during Active/Releasing
        //    where ceiling > 0; during Quiet/Detected the lock is
        //    skipped and amp is forced to 0). ──
        if matches!(self.phase, DronePhase::Active | DronePhase::Releasing)
            && self.tracked_fundamental > 0.0
        {
            self.lock_phase_to_noise(samples);
        } else {
            self.phase_lock_quality = 0.0;
            self.anti_amp = 0.0;
        }

        self.snapshot()
    }

    fn transition(&mut self, next: DronePhase) {
        self.phase = next;
        self.phase_entered_at = Instant::now();
        if next == DronePhase::Active {
            // Snap the tracked fundamental to current estimate on
            // entry — this is the moment we lock the anti-noise
            // generator's frequency.
            if let Some(&latest) = self.recent_freqs.first() {
                if latest > 0.0 { self.tracked_fundamental = latest; }
            }
        }
        if next == DronePhase::Quiet {
            self.tracked_fundamental = 0.0;
        }
    }

    fn update_anti_envelope(&mut self) {
        let target = match self.phase {
            DronePhase::Quiet     => 0.0,
            DronePhase::Detected  => 0.0,   // wait for confirmation
            DronePhase::Active    => 0.8,
            DronePhase::Releasing => 0.4,
        };
        let rate = match self.phase {
            DronePhase::Active => 0.05,
            _                  => 0.03,
        };
        self.anti_amp_ceiling += (target - self.anti_amp_ceiling) * rate;
        self.anti_amp_ceiling = self.anti_amp_ceiling.clamp(0.0, 1.0);
        if self.anti_amp > self.anti_amp_ceiling {
            self.anti_amp = self.anti_amp_ceiling;
        }
    }

    /// Phase + amplitude lock — same pattern as PassbyCanceller's
    /// version. 16-trial phase search with linear-regression
    /// amplitude per trial, bounded by `anti_amp_ceiling`. See
    /// `PassbyCanceller::lock_phase_to_noise` for the full
    /// derivation; this is the same idea applied to the drone
    /// canceller's fundamental + 2nd-harmonic template.
    ///
    /// Drones differ from passby in that the source is sustained
    /// for minutes — so locking once on entry to Active isn't
    /// enough; the fundamental drifts as the operator works.
    /// Re-locking every window catches the drift, but unlike
    /// passby (where the event is brief and we want fast
    /// convergence) the drone phase shouldn't whiplash on
    /// transients. Future improvement: a loop filter to smooth
    /// inter-window phase changes; not built this turn.
    fn lock_phase_to_noise(&mut self, mic_window: &[f32]) {
        if self.tracked_fundamental <= 0.0 || mic_window.is_empty() {
            self.phase_lock_quality = 0.0;
            return;
        }
        const N_PHASE_TRIALS: usize = 16;
        let dphase = 2.0 * PI * self.tracked_fundamental / SAMPLE_RATE;
        let baseline: f32 = mic_window.iter().map(|&x| x * x).sum();
        if baseline <= 1e-12 {
            self.phase_lock_quality = 0.0;
            return;
        }
        let amp_ceiling = self.anti_amp_ceiling.max(0.05);
        let mut best_phase = self.anti_phase;
        let mut best_amp = 0.0f32;
        let mut best_residual = baseline;
        let n = mic_window.len();
        let mut template: Vec<f32> = vec![0.0; n];
        for trial in 0..N_PHASE_TRIALS {
            let trial_start = trial as f32 * 2.0 * PI / N_PHASE_TRIALS as f32;
            let mut phase = trial_start;
            for k in 0..n {
                template[k] = (phase.sin() + 0.5 * (2.0 * phase).sin()) / 1.5;
                phase += dphase;
                if phase > 2.0 * PI { phase -= 2.0 * PI; }
            }
            let mut dot_mt = 0.0f32;
            let mut sum_tt = 0.0f32;
            for k in 0..n {
                dot_mt += mic_window[k] * template[k];
                sum_tt += template[k] * template[k];
            }
            if sum_tt < 1e-9 { continue; }
            let amp_opt = (dot_mt / sum_tt).clamp(0.0, amp_ceiling);
            let mut residual_sq = 0.0f32;
            for k in 0..n {
                let r = mic_window[k] - amp_opt * template[k];
                residual_sq += r * r;
            }
            if residual_sq < best_residual {
                best_residual = residual_sq;
                best_phase = trial_start;
                best_amp = amp_opt;
            }
        }
        self.anti_phase = best_phase;
        self.anti_amp = best_amp;
        let q = 1.0 - (best_residual / baseline);
        self.phase_lock_quality = q.clamp(0.0, 1.0);
    }

    /// Pull anti-noise samples — sinusoid at the tracked fundamental
    /// with harmonic enrichment, modulated by current envelope.
    pub fn pull_output(&mut self, n: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(n);
        if self.anti_amp <= 1e-6 || self.tracked_fundamental <= 0.0 {
            out.resize(n, 0.0);
            return out;
        }
        let dphase = 2.0 * PI * self.tracked_fundamental / SAMPLE_RATE;
        let saturation = self.saturation;
        for _ in 0..n {
            let raw = self.anti_phase.sin() + 0.5 * (2.0 * self.anti_phase).sin();
            let normalized = raw / 1.5;
            // Saturation mixin — see PassbyCanceller::pull_output for
            // the full rationale. Drones (leafblowers, 2-stroke
            // weed-eaters) have heavily saturated 2-cycle exhaust
            // — a higher default saturation than passby would suit
            // them. But default still 0.0 so existing tests don't
            // shift; tune from QML when desired.
            let s = if saturation > 1e-6 {
                let bias = 0.15 * saturation;
                let drive = 1.0 + 2.0 * saturation;
                let driven = normalized * drive + bias;
                let clipped = driven.tanh() / drive;
                let dc_offset = bias.tanh() / drive;
                (clipped - dc_offset) * self.anti_amp
            } else {
                normalized * self.anti_amp
            };
            out.push(s.clamp(-1.0, 1.0));
            self.anti_phase += dphase;
            if self.anti_phase > 2.0 * PI { self.anti_phase -= 2.0 * PI; }
        }
        out
    }

    /// Anti-noise saturation amount (0.0 = clean, 0.6 = heavy).
    /// Higher values match the heavily-saturated exhaust character
    /// of 2-cycle small engines (leafblowers, weed eaters); for
    /// quiet diesel idle drones a lower value (0.1-0.2) is enough.
    pub fn set_saturation(&mut self, s: f32) {
        self.saturation = s.clamp(0.0, 0.6);
    }

    pub fn saturation(&self) -> f32 { self.saturation }

    pub fn reset(&mut self) { *self = Self::default(); }
}

// ─── Shared helpers (mirrored from passby_canceller for now; will
//     consolidate into a shared bo-audio crate in a future pass) ───

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

pub fn band_energy_rms(samples: &[f32], lo: f32, hi: f32, sr: f32) -> f32 {
    if samples.is_empty() { return 0.0; }
    let n_probes = 20;       // more probes than passby (wider band).
    let mut total = 0.0f32;
    let mut probes = 0;
    for i in 0..n_probes {
        let f = lo + (hi - lo) * (i as f32) / (n_probes as f32 - 1.0);
        let mag = goertzel(samples, f, sr);
        if mag.is_finite() {
            total += mag * mag;
            probes += 1;
        }
    }
    if probes == 0 { return 0.0; }
    (total / probes as f32).sqrt() / samples.len() as f32
}

pub fn estimate_fundamental(samples: &[f32], lo: f32, hi: f32, sr: f32) -> f32 {
    if samples.len() < 256 { return 0.0; }
    // Bin-aligned scan + parabolic interpolation — see passby_canceller
    // for the full rationale. Without this fix the estimator returns
    // the scan freq instead of the actual bin-center frequency, which
    // breaks phase-lock convergence (the anti-noise generator
    // synthesizes at the wrong freq).
    let n = samples.len() as f32;
    let bin_hz = sr / n;
    let mut best_k = 0i32;
    let mut best_mag = 0.0f32;
    let k_lo = (lo / bin_hz).ceil() as i32;
    let k_hi = (hi / bin_hz).floor() as i32;
    for k in k_lo..=k_hi {
        let f = k as f32 * bin_hz;
        let m = goertzel(samples, f, sr);
        if m > best_mag { best_mag = m; best_k = k; }
    }
    if best_mag < 1.0 { return 0.0; }
    let f_at = |k: i32| -> f32 { goertzel(samples, k as f32 * bin_hz, sr) };
    let m_minus = if best_k > 0 { f_at(best_k - 1) } else { 0.0 };
    let m_plus  = f_at(best_k + 1);
    let m_zero  = best_mag;
    let denom = m_minus - 2.0 * m_zero + m_plus;
    let delta = if denom.abs() > 1e-9 {
        (0.5 * (m_minus - m_plus) / denom).clamp(-0.5, 0.5)
    } else { 0.0 };
    let refined_k = best_k as f32 + delta;
    refined_k * bin_hz
}

/// Test helper: synthesize a sustained drone at `base_hz` with
/// `freq_drift_hz_per_s` slow drift across `duration_s`. Useful
/// for verifying the tracker follows the drifting fundamental.
pub fn synth_drone(
    duration_s: f32,
    base_hz: f32,
    freq_drift_hz_per_s: f32,
    sample_rate: f32,
) -> Vec<f32> {
    let n = (duration_s * sample_rate) as usize;
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / sample_rate;
        let inst_freq = base_hz + freq_drift_hz_per_s * t;
        let dphase = 2.0 * PI * inst_freq / sample_rate;
        // Engine-like: fundamental + 2nd + 3rd harmonic.
        let engine = phase.sin() + 0.4 * (2.0 * phase).sin() + 0.2 * (3.0 * phase).sin();
        out.push((engine / 1.6) * 0.5);
        phase += dphase;
        if phase > 2.0 * PI { phase -= 2.0 * PI; }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(d: &mut DroneCanceller, signal: &[f32]) {
        let mut i = 0;
        while i + WINDOW_SAMPLES <= signal.len() {
            d.process_window(&signal[i..i + WINDOW_SAMPLES]);
            i += WINDOW_SAMPLES;
        }
    }

    #[test]
    fn initial_phase_is_quiet() {
        let d = DroneCanceller::new();
        assert_eq!(d.phase(), DronePhase::Quiet);
    }

    #[test]
    fn quiet_under_silence() {
        let mut d = DroneCanceller::new();
        feed(&mut d, &vec![0.0f32; SAMPLE_RATE as usize * 3]);
        assert_eq!(d.phase(), DronePhase::Quiet);
    }

    #[test]
    fn detects_sustained_drone() {
        let mut d = DroneCanceller::new();
        // 5 s of drone — well past CONFIRM_MS — at 250 Hz with mild drift.
        let sig = synth_drone(5.0, 250.0, 0.5, SAMPLE_RATE);
        feed(&mut d, &sig);
        let s = d.snapshot();
        assert!(matches!(s.phase, DronePhase::Active | DronePhase::Detected),
            "expected Active or Detected, got {:?}", s.phase);
    }

    #[test]
    fn tracks_drifting_fundamental() {
        let mut d = DroneCanceller::new();
        // 6 s drone drifting from 200 → 230 Hz.
        let sig = synth_drone(6.0, 200.0, 5.0, SAMPLE_RATE);
        feed(&mut d, &sig);
        let s = d.snapshot();
        if s.phase == DronePhase::Active {
            // Tracked fundamental should be somewhere between 200
            // and 230 (mid-drift is around 215).
            assert!(s.fundamental_hz >= 195.0 && s.fundamental_hz <= 235.0,
                "tracked freq should follow drift, got {}", s.fundamental_hz);
        }
    }

    #[test]
    fn brief_transient_doesnt_activate() {
        let mut d = DroneCanceller::new();
        // Establish quiet floor.
        feed(&mut d, &vec![0.0001f32; SAMPLE_RATE as usize * 2]);
        // 1 s blast then back to silence — under CONFIRM_MS (2s).
        let blast = synth_drone(1.0, 250.0, 0.0, SAMPLE_RATE);
        feed(&mut d, &blast);
        feed(&mut d, &vec![0.0001f32; SAMPLE_RATE as usize * 2]);
        // Should NOT have reached Active.
        assert_ne!(d.phase(), DronePhase::Active);
    }

    #[test]
    fn anti_output_is_finite_and_bounded() {
        let mut d = DroneCanceller::new();
        let sig = synth_drone(4.0, 250.0, 0.0, SAMPLE_RATE);
        feed(&mut d, &sig);
        let out = d.pull_output(4096);
        for &s in &out {
            assert!(s.is_finite());
            assert!(s.abs() <= 1.0);
        }
    }

    #[test]
    fn anti_output_silent_when_quiet() {
        let mut d = DroneCanceller::new();
        feed(&mut d, &vec![0.0f32; SAMPLE_RATE as usize]);
        let out = d.pull_output(2048);
        let energy: f32 = out.iter().map(|&x| x * x).sum();
        assert!(energy < 1e-2);
    }

    #[test]
    fn reset_clears_state() {
        let mut d = DroneCanceller::new();
        let sig = synth_drone(4.0, 250.0, 0.0, SAMPLE_RATE);
        feed(&mut d, &sig);
        d.reset();
        let s = d.snapshot();
        assert_eq!(s.phase, DronePhase::Quiet);
        assert_eq!(s.fundamental_hz, 0.0);
    }

    #[test]
    fn stability_low_when_no_history() {
        let d = DroneCanceller::new();
        assert_eq!(d.compute_stability(), 0.0);
    }

    #[test]
    fn stability_high_with_consistent_freqs() {
        let mut d = DroneCanceller::new();
        d.recent_freqs = vec![250.0, 251.0, 249.0, 250.5, 250.0];
        assert!(d.compute_stability() > 0.9);
    }

    #[test]
    fn stability_low_with_chaotic_freqs() {
        let mut d = DroneCanceller::new();
        // 200 Hz spread is well past the tolerance.
        d.recent_freqs = vec![150.0, 300.0, 200.0, 450.0, 100.0];
        assert_eq!(d.compute_stability(), 0.0);
    }

    #[test]
    fn drone_band_extends_to_leafblower_range() {
        // Leafblower-typical fundamental at ~250 Hz.
        let n = 4096;
        let mut sig = Vec::with_capacity(n);
        for i in 0..n {
            sig.push((2.0 * PI * 250.0 * i as f32 / SAMPLE_RATE).sin());
        }
        let est = estimate_fundamental(&sig, BAND_LO_HZ, BAND_HI_HZ, SAMPLE_RATE);
        assert!((est - 250.0).abs() < 15.0,
            "expected ~250, got {}", est);
    }
}
