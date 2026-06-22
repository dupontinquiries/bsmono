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

/// Tonality-gate thresholds. Asymmetric hysteresis: enter (open
/// the gate, let canceller emit) at 0.30; exit (close the gate,
/// silence the canceller) at 0.15. Wider gap than passby's
/// 0.25/0.10 because drones are SUSTAINED — false-positive
/// emission on broadband fan noise compounds for minutes, not
/// seconds, so we want a more conservative open threshold.
pub const TONALITY_GATE_ENTER: f32 = 0.30;
pub const TONALITY_GATE_EXIT:  f32 = 0.15;

/// EWMA smoothing factor for the per-window tonality score.
/// At a 2048-sample window @ 48 kHz that's ~43 ms / window;
/// α=0.2 gives a time constant of ~200 ms. Long enough that
/// a single noisy window can't slam the gate; short enough
/// that the gate responds in well under a second when the
/// noise character changes.
pub const TONALITY_SMOOTHING_ALPHA: f32 = 0.2;
                                            // shouldn't lift the floor.
const FREQ_TRACK_ALPHA: f32 = 0.15;         // moderately fast follow.

/// Frequency-confidence taper (sim-grounded, see anc_sim/FINDINGS.txt).
///
/// A 2-speaker room null is only ~λ/4 wide, so the spatial quiet zone
/// shrinks as frequency rises. The acoustic sim measured the zone radius
/// dropping below head size (~9 cm) around 500–800 Hz: at 300 Hz the zone
/// is ~29 cm (head-sized — robust to small head movement), but by 500 Hz
/// it's ~17 cm (already smaller than the gap between the ears), so
/// full-amplitude anti-noise there risks nulling one ear while BOOSTING
/// the other. Rather than a hard cap, we taper the anti-noise ceiling:
/// full confidence at/below TAPER_LO_HZ, rolling linearly to a small floor
/// by BAND_HI_HZ. This keeps the leafblower-fundamental coverage the wide
/// band was added for, while backing off the output where the physics says
/// a stable head-sized null no longer exists.
pub const TAPER_LO_HZ: f32 = 300.0;   // full-confidence ceiling (zone ≈ head)
pub const TAPER_HI_FLOOR: f32 = 0.35; // residual confidence at BAND_HI_HZ

/// Confidence multiplier in [TAPER_HI_FLOOR, 1.0] for a tracked
/// fundamental. 1.0 at/below 300 Hz, linearly down to TAPER_HI_FLOOR at
/// BAND_HI_HZ (500 Hz). Frequencies above the band shouldn't reach here
/// (they're out of the detector's range), but we clamp for safety.
pub fn freq_confidence(f0_hz: f32) -> f32 {
    if f0_hz <= TAPER_LO_HZ {
        1.0
    } else if f0_hz >= BAND_HI_HZ {
        TAPER_HI_FLOOR
    } else {
        let frac = (f0_hz - TAPER_LO_HZ) / (BAND_HI_HZ - TAPER_LO_HZ);
        1.0 - frac * (1.0 - TAPER_HI_FLOOR)
    }
}

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
    /// Smoothed tonality score (0..1). Fraction of in-band energy
    /// concentrated at the tracked harmonic frequencies. High =
    /// clearly harmonic (engine, motor); low = broadband (rumble,
    /// wind buffet, distant traffic).
    pub tonality: f32,
    /// True when tonality gate is OPEN — canceller is allowed to
    /// emit anti-noise. False = gate closed, anti-noise silenced
    /// because the source isn't tonal enough to safely cancel.
    pub gate_open: bool,
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
    /// EWMA-smoothed tonality score in [0, 1]. Updated each
    /// process_window from the per-window measurement; the gate
    /// reads this (not the raw per-window value) to avoid
    /// pumping on a single noisy frame. Initialized to 0.5 so a
    /// cold-start canceller doesn't prematurely flip its gate
    /// based on a half-decay of nothing.
    smoothed_tonality: f32,
    /// Tonality gate state. When false, pull_output returns
    /// silence and the FSM continues advancing internally — same
    /// behavior as the (future) PassbyCanceller. Initialized
    /// true so a freshly-constructed canceller will give the
    /// next process_window a chance to measure; the first window
    /// of broadband content closes it within a few frames.
    gate_open: bool,
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
            smoothed_tonality: 0.5,
            gate_open: true,
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
            tonality: self.smoothed_tonality,
            gate_open: self.gate_open,
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

        // ── Tonality gate ──
        //
        // After the FSM and lock have processed this window, look at
        // the spectral character of the input: is it harmonic
        // (engine, motor — what the canceller is for) or broadband
        // (rumble, wind, fan turbulence — what the canceller cannot
        // fix and shouldn't try to)?
        //
        // tonality = fraction of in-band energy concentrated at the
        // first 4 harmonics of the tracked fundamental. High = clean
        // engine tone; low = the energy is spread across the band
        // and the canceller would just be adding correlated noise to
        // the listener's ear without subtracting any.
        //
        // The gate has asymmetric hysteresis (open 0.30, close 0.15)
        // so a marginal source doesn't oscillate. When closed, the
        // FSM still advances (so we don't forget where we were) but
        // pull_output suppresses the anti-noise emission. This is
        // the same defense-in-depth pattern as the passby canceller.
        let f0 = self.tracked_fundamental;
        let raw_tonality = if f0 > 0.0 {
            measure_tonality(samples, f0, 4, SAMPLE_RATE)
        } else { 0.0 };
        self.smoothed_tonality = TONALITY_SMOOTHING_ALPHA * raw_tonality
            + (1.0 - TONALITY_SMOOTHING_ALPHA) * self.smoothed_tonality;
        // Apply hysteresis: open above ENTER, close below EXIT,
        // stay where we were in the middle band.
        if self.gate_open && self.smoothed_tonality < TONALITY_GATE_EXIT {
            self.gate_open = false;
        } else if !self.gate_open && self.smoothed_tonality > TONALITY_GATE_ENTER {
            self.gate_open = true;
        }
        // When gate is closed, force anti_amp to 0 so pull_output
        // returns silence regardless of what the lock thinks.
        if !self.gate_open {
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
        // Taper the ceiling by how cancellable the tracked frequency is in
        // space. Above ~300 Hz the quiet zone is smaller than a head, so we
        // back off rather than risk boosting the off-null ear (sim-grounded;
        // see freq_confidence). Below 300 Hz this is a no-op (factor 1.0).
        let target = if self.tracked_fundamental > 0.0 {
            target * freq_confidence(self.tracked_fundamental)
        } else {
            target
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

/// Measure how "tonal" a window is, given a candidate fundamental
/// frequency. Returns the fraction of in-band energy concentrated
/// at f0 and its first `n_harmonics-1` harmonics, in [0, 1].
///
/// 1.0 = all in-band energy is exactly at the harmonic frequencies
/// (a perfect sine + harmonics — engine, motor).
/// 0.0 = none of the energy is at the harmonics (pure broadband
/// noise like wind, fan turbulence, rumble).
///
/// In practice typical results:
///   • Sustained motor / engine drone:      0.5 — 0.8
///   • Borderline traffic with engine note: 0.2 — 0.4
///   • Wind buffet, HVAC fan, rumble:       0.0 — 0.1
///
/// The drone canceller's gate uses an EWMA-smoothed version of
/// this with hysteresis ENTER=0.30 / EXIT=0.15.
pub fn measure_tonality(samples: &[f32], f0: f32, n_harmonics: usize, sr: f32) -> f32 {
    if samples.is_empty() || f0 <= 0.0 || !f0.is_finite() || n_harmonics == 0 {
        return 0.0;
    }
    // Sum squared Goertzel magnitudes at each harmonic. For a pure
    // cos(2πkn/N) at amplitude A, goertzel returns ~ N*A/2, so
    // mag² ~ N²A²/4 at that bin.
    let mut harmonic_energy = 0.0f32;
    for i in 0..n_harmonics {
        let freq = f0 * (i + 1) as f32;
        if freq > BAND_HI_HZ * 1.5 { break; }
        let mag = goertzel(samples, freq, sr);
        if mag.is_finite() { harmonic_energy += mag * mag; }
    }
    // Total signal energy from the time domain (Parseval). For a
    // pure cos at amplitude A, sum x² = N*A²/2.
    //
    // Combining: for a pure tone with N samples,
    //   harmonic_energy / (time_total * N / 2)
    //   = (N²A²/4) / (N*A²/2 * N/2)
    //   = (N²A²/4) / (N²A²/4)
    //   = 1.0
    //
    // For broadband white noise of unit variance, harmonic_energy
    // captures ~ n_harmonics bins out of ~ N/2 total bins (one-sided),
    // and the ratio comes out near n_harmonics / (N/2) — tiny for
    // reasonable N (e.g. 2048).
    let time_total: f32 = samples.iter()
        .map(|&x| if x.is_finite() { x * x } else { 0.0 })
        .sum();
    let n = samples.len() as f32;
    let denom = time_total * n / 2.0;
    if denom < 1e-12 { return 0.0; }
    (harmonic_energy / denom).clamp(0.0, 1.0)
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

    // ── Tonality gate tests ──────────────────────────────────────

    /// Pure-tone input should produce high tonality (most energy at f0).
    #[test]
    fn tonality_high_for_pure_tone() {
        let n = 2048;
        let f = 150.0;
        let sig: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * f * i as f32 / SAMPLE_RATE).sin())
            .collect();
        let t = measure_tonality(&sig, f, 4, SAMPLE_RATE);
        assert!(t > 0.5,
            "pure tone should score > 0.5, got {}", t);
    }

    /// Broadband noise should produce low tonality (no concentration).
    #[test]
    fn tonality_low_for_broadband_noise() {
        let n = 2048;
        // Deterministic pseudo-random in band — LCG so the test is
        // hermetic. Mean zero, unit-ish variance.
        let mut s = 12345u64;
        let sig: Vec<f32> = (0..n).map(|_| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 33) as f32 / u32::MAX as f32) - 0.5
        }).collect();
        let t = measure_tonality(&sig, 200.0, 4, SAMPLE_RATE);
        assert!(t < 0.2,
            "broadband should score < 0.2, got {}", t);
    }

    /// Zero f0 returns 0.
    #[test]
    fn tonality_zero_for_no_fundamental() {
        let n = 1024;
        let sig: Vec<f32> = (0..n).map(|_| 0.0).collect();
        assert_eq!(measure_tonality(&sig, 0.0, 4, SAMPLE_RATE), 0.0);
        assert_eq!(measure_tonality(&sig, -10.0, 4, SAMPLE_RATE), 0.0);
    }

    /// Gate starts open (initialized true) so a fresh canceller
    /// gives the next window a chance to measure.
    #[test]
    fn gate_starts_open() {
        let d = DroneCanceller::new();
        assert!(d.gate_open);
        assert_eq!(d.smoothed_tonality, 0.5);
    }

    /// Process repeated broadband windows → gate should close.
    #[test]
    fn gate_closes_on_sustained_broadband() {
        let mut d = DroneCanceller::new();
        d.tracked_fundamental = 150.0;  // pretend we locked
        let n = 2048;
        let mut s = 99u64;
        let noise: Vec<f32> = (0..n).map(|_| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 33) as f32 / u32::MAX as f32) - 0.5
        }).collect();
        // 30 windows is well past the smoothing time constant
        // (alpha=0.2 → ~5 windows to reach steady state, plus
        // hysteresis margin to actually flip the gate).
        for _ in 0..30 { d.process_window(&noise); }
        assert!(!d.gate_open,
            "gate should close after sustained broadband. \
             smoothed_tonality={}", d.smoothed_tonality);
    }

    /// Process repeated harmonic windows → gate should open.
    #[test]
    fn gate_opens_on_sustained_tone() {
        let mut d = DroneCanceller::new();
        d.tracked_fundamental = 200.0;
        // Start with gate closed (simulating recovery from broadband).
        d.gate_open = false;
        d.smoothed_tonality = 0.0;
        let n = 2048;
        let f = 200.0;
        let sig: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * f * i as f32 / SAMPLE_RATE).sin())
            .collect();
        for _ in 0..30 { d.process_window(&sig); }
        assert!(d.gate_open,
            "gate should open on sustained tone. \
             smoothed_tonality={}", d.smoothed_tonality);
    }

    /// When the gate is closed, anti_amp must be forced to 0
    /// regardless of FSM state or lock estimates.
    #[test]
    fn closed_gate_forces_anti_amp_zero() {
        let mut d = DroneCanceller::new();
        d.tracked_fundamental = 150.0;
        d.phase = DronePhase::Active;
        d.anti_amp_ceiling = 0.5;
        // Force gate closed by simulating broadband input.
        let n = 2048;
        let mut s = 7u64;
        let noise: Vec<f32> = (0..n).map(|_| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 33) as f32 / u32::MAX as f32) - 0.5
        }).collect();
        for _ in 0..30 { d.process_window(&noise); }
        assert!(!d.gate_open, "precondition: gate closed");
        assert_eq!(d.anti_amp, 0.0,
            "anti_amp must be 0 when gate closed");
    }

    /// Hysteresis: marginal tonality between EXIT (0.15) and
    /// ENTER (0.30) should leave the gate in its prior state.
    #[test]
    fn gate_has_hysteresis() {
        let mut d = DroneCanceller::new();
        d.gate_open = false;
        d.smoothed_tonality = 0.20;
        // Manually run the gate-update branch (no process_window
        // since we want to control smoothed_tonality exactly).
        // Replicate the body: open above 0.30, close below 0.15.
        if d.gate_open && d.smoothed_tonality < TONALITY_GATE_EXIT {
            d.gate_open = false;
        } else if !d.gate_open && d.smoothed_tonality > TONALITY_GATE_ENTER {
            d.gate_open = true;
        }
        assert!(!d.gate_open,
            "0.20 with prior=closed should stay closed");
        // Now from the other side.
        d.gate_open = true;
        if d.gate_open && d.smoothed_tonality < TONALITY_GATE_EXIT {
            d.gate_open = false;
        } else if !d.gate_open && d.smoothed_tonality > TONALITY_GATE_ENTER {
            d.gate_open = true;
        }
        assert!(d.gate_open,
            "0.20 with prior=open should stay open");
    }

    /// Snapshot exposes the new tonality + gate fields.
    #[test]
    fn snapshot_includes_tonality_and_gate() {
        let mut d = DroneCanceller::new();
        d.smoothed_tonality = 0.42;
        d.gate_open = false;
        let s = d.snapshot();
        assert!((s.tonality - 0.42).abs() < 1e-5);
        assert!(!s.gate_open);
    }

    /// Frequency confidence taper: full below 300 Hz, tapered above,
    /// floored at BAND_HI_HZ. Grounded in the acoustic sim's finding that
    /// the spatial quiet zone shrinks below head size above ~300-500 Hz.
    #[test]
    fn freq_confidence_tapers_above_300hz() {
        // Full confidence in the robust low band.
        assert_eq!(freq_confidence(60.0), 1.0);
        assert_eq!(freq_confidence(150.0), 1.0);
        assert_eq!(freq_confidence(300.0), 1.0);
        // Monotonic roll-off across the marginal band.
        let c350 = freq_confidence(350.0);
        let c450 = freq_confidence(450.0);
        assert!(c350 < 1.0 && c350 > c450,
            "confidence should decrease with frequency: 350={} 450={}", c350, c450);
        // Floored at the top of the band, and clamped above it.
        assert!((freq_confidence(500.0) - TAPER_HI_FLOOR).abs() < 1e-6);
        assert!((freq_confidence(900.0) - TAPER_HI_FLOOR).abs() < 1e-6);
        // Midpoint (400 Hz) is halfway between 1.0 and the floor.
        let expect_mid = 1.0 - 0.5 * (1.0 - TAPER_HI_FLOOR);
        assert!((freq_confidence(400.0) - expect_mid).abs() < 1e-5);
    }

    /// A high-frequency drone (e.g. 450 Hz leafblower harmonic) must get a
    /// LOWER anti-noise ceiling than a low-frequency one (e.g. 120 Hz car),
    /// because the high-freq null is sub-head-sized and risks boosting the
    /// off-null ear. Both fully Active.
    #[test]
    fn high_freq_drone_gets_lower_ceiling() {
        let settle = |f0: f32| {
            let mut d = DroneCanceller::new();
            d.phase = DronePhase::Active;
            d.tracked_fundamental = f0;
            // Run the envelope to steady state.
            for _ in 0..400 { d.update_anti_envelope(); }
            d.anti_amp_ceiling
        };
        let low = settle(120.0);
        let high = settle(450.0);
        assert!(low > high,
            "low-freq ceiling {} should exceed high-freq ceiling {}", low, high);
        // Low-freq should reach near the full 0.8 Active target.
        assert!(low > 0.7, "low-freq ceiling should approach 0.8, got {}", low);
    }
}
