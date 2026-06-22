// src/passby_canceller.rs
#![allow(dead_code)]
//
// Predictive vehicle-pass-by cancellation.
//
// ── The noise profile ──
//
// A car passing on a nearby street produces:
//   1. Engine firing-order harmonics: fundamental at
//        f0 = (cylinders × rpm) / (2 × 60)         [4-stroke]
//      For a typical sports car cruising at 3000-5000 rpm with a
//      6-8 cyl engine, f0 lands in 80-300 Hz — exactly the band
//      the AncEngine is designed to cancel.
//   2. Tire/pavement noise (TPIN) — broadband centered 500-1500 Hz,
//      uncancellable per the wavelength-vs-head-movement argument.
//   3. Aerodynamic noise — broadband, also above the cancellable band.
//
// Only the engine harmonics are tractable. They are also where the
// "annoying" character of a loud sports car at night actually lives:
// the throaty fundamental, not the rolling tire shush.
//
// ── The pass-by signature ──
//
// A passing car produces a stereotyped 4-phase signal at the mic:
//
//   Quiet  ── noise floor only
//   Approach ── 80-300 Hz energy rising over 5-15 s. The instantaneous
//                fundamental drifts UP slightly (Doppler stretch as
//                the source approaches).
//   Peak  ── energy plateau lasting ~1-3 s. Fundamental stable at the
//             engine's actual rate.
//   Recede ── energy falling over 5-15 s. Fundamental drifts DOWN
//              (Doppler compression as the source recedes).
//
// The Doppler signature is the crucial gate against false positives:
// an HVAC compressor at 60 Hz STAYS at 60 Hz forever. A car's
// fundamental moves up-then-down across the pass-by. A simple
// linear-model anti-noise generator (just inverting the recent
// template, as the existing Predictor does) doesn't capture the
// asymmetric Doppler shift; the FSM here does.
//
// ── The FSM ──
//
// Transitions are gated by:
//   - mic band energy in 80-300 Hz vs. an adaptive noise floor
//   - estimated fundamental frequency + its rate of change
//   - minimum dwell time per phase (avoids chatter)
//
// Each transition timer is learned from observed events. A car
// taking the same route at the same speed produces similar
// transition durations; the EWMA of those durations is what we
// use to PREDICT the next event's timeline. With a confident
// timer, when Approach is detected, we know roughly when Peak
// will arrive and can start generating anti-noise ahead of time
// (compensating for the round-trip latency the alignment probe
// measured).
//
// ── What this DOESN'T solve ──
//
// - Multiple simultaneous vehicles: the FSM tracks one source.
//   Two simultaneous passes get treated as a single bigger pass.
// - Stop-and-go traffic: the FSM expects a clean envelope. A car
//   stopping at the red light next to your bedroom is treated
//   poorly — it enters Approach, never gets a clean Peak/Recede.
//   The fallback is to time out and degrade to the existing
//   template-based predictor.
// - Vehicles with no engine noise (electric cars): tire-only.
//   Out of band; nothing to do.

use std::f32::consts::PI;

/// Engine-band low/high used both for energy detection and for
/// constraining the fundamental estimator. The lower bound (60 Hz)
/// covers idling large engines (V8 at idle ≈ 80 Hz); the upper
/// bound (450 Hz) catches sport-bike / moped fundamentals at highway
/// speed (a 600cc sport bike at 10k rpm × 4 cyl ÷ 120 = 333 Hz; a
/// 2-stroke moped pushed hard can hit 400 Hz). The original cap
/// was 300 Hz which excluded motorcycles entirely.
///
/// Wavelength caveat: at 450 Hz the wavelength is 76 cm; head
/// movement of 5-10 cm is 7-13% of a wavelength so cancellation
/// becomes partial. Acceptable for the overnight use case — a
/// sleeping listener's head moves little, and partial cancellation
/// of a sport-bike's bark is still a meaningful win.
pub const BAND_LO_HZ: f32 = 60.0;
pub const BAND_HI_HZ: f32 = 450.0;

/// Sample rate the canceller runs at. Matches AncEngine's internal
/// rate so they share buffers.
pub const SAMPLE_RATE: f32 = 48_000.0;

/// FFT-ish window size for the in-band energy + fundamental estimate.
/// 2048 samples ≈ 43 ms — long enough to resolve down to ~23 Hz
/// (Nyquist of the window length), short enough to track Doppler
/// shifts that complete over a 1-2 second timeline.
pub const WINDOW_SAMPLES: usize = 2048;

/// Minimum dwell time per phase. Prevents chatter when energy is
/// flickering near a threshold (e.g. a distant car you can barely
/// hear but the gate keeps tripping).
const APPROACH_MIN_MS: u64 = 800;
const PEAK_MIN_MS: u64     = 400;
const RECEDE_MIN_MS: u64   = 800;

/// Cap on how long a single phase can last before we force a
/// degradation back to Quiet. Real pass-bys complete in well
/// under these.
const APPROACH_MAX_MS: u64 = 25_000;
const PEAK_MAX_MS: u64     = 8_000;
const RECEDE_MAX_MS: u64   = 25_000;

/// Energy ratio over noise floor to trigger Quiet → Approach.
const APPROACH_ENTRY_RATIO: f32 = 3.0;
/// Energy ratio to drop back from Recede → Quiet.
const QUIET_EXIT_RATIO: f32 = 1.5;

/// Minimum Doppler magnitude (Hz/sec) sustained to gate on a
/// moving source — distinguishes a car from a stationary
/// periodic noise like an HVAC condenser.
const DOPPLER_GATE_HZ_PER_S: f32 = 0.5;

/// EWMA alpha for the noise floor tracker. Slow (0.01) so a
/// single loud event doesn't bias the floor upward, but fast
/// enough that genuine ambient changes (windows open / close,
/// time of day) get tracked within minutes.
const NOISE_FLOOR_ALPHA: f32 = 0.01;

/// EWMA alpha for the learned transition timers. Faster (0.2)
/// than the noise floor — pass-bys happen seconds apart not
/// minutes, and we want the timers to adapt within a handful
/// of observations.
const TIMER_LEARN_ALPHA: f32 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PassbyPhase {
    Quiet,
    Approach,
    Peak,
    Recede,
}

#[derive(Debug, Clone, Copy)]
pub struct PassbySnapshot {
    pub phase: PassbyPhase,
    /// Estimated fundamental in Hz (0 when unknown).
    pub fundamental_hz: f32,
    /// Instantaneous in-band energy.
    pub band_energy: f32,
    /// Adaptive noise floor.
    pub noise_floor: f32,
    /// Current anti-noise emission amplitude in [0, 1].
    pub anti_amp: f32,
    /// Confidence in the learned timers, 0..1. 0 means no events
    /// observed yet; 1 means we've seen enough for the timers to
    /// be trusted for prediction.
    pub timer_confidence: f32,
    /// Approximate Doppler rate (Hz/s). Positive = source approaching
    /// (frequency rising); negative = receding (frequency falling).
    pub doppler_hz_per_s: f32,
    /// Phase-lock quality, 0..1. 1 means the anti-noise's phase
    /// is well-aligned with the mic-observed noise (predicted
    /// residual is much smaller than baseline). 0 means no useful
    /// phase alignment — anti-noise is likely amplifying rather
    /// than cancelling. The UI surfaces this so the user can see
    /// whether the canceller is genuinely working or just
    /// generating sound.
    pub phase_lock_quality: f32,
}

/// Per-phase EWMA timers, learned from completed pass-by events.
#[derive(Debug, Clone, Copy, Default)]
pub struct TransitionTimers {
    pub approach_ms_ewma: f32,
    pub peak_ms_ewma: f32,
    pub recede_ms_ewma: f32,
    pub events_observed: u32,
}

impl TransitionTimers {
    /// Confidence rises with observations, saturating around 8
    /// events. Below 3 events the timers are treated as too
    /// noisy for predictive use; the canceller falls back to
    /// reactive (only emit when actually in Peak).
    pub fn confidence(&self) -> f32 {
        ((self.events_observed as f32) / 8.0).min(1.0)
    }

    /// Update with an observed phase duration. EWMA so we adapt
    /// to changing patterns (route shifts, vehicle changes).
    fn observe(&mut self, approach_ms: f32, peak_ms: f32, recede_ms: f32) {
        if self.events_observed == 0 {
            self.approach_ms_ewma = approach_ms;
            self.peak_ms_ewma = peak_ms;
            self.recede_ms_ewma = recede_ms;
        } else {
            let a = TIMER_LEARN_ALPHA;
            self.approach_ms_ewma = (1.0 - a) * self.approach_ms_ewma + a * approach_ms;
            self.peak_ms_ewma     = (1.0 - a) * self.peak_ms_ewma     + a * peak_ms;
            self.recede_ms_ewma   = (1.0 - a) * self.recede_ms_ewma   + a * recede_ms;
        }
        self.events_observed = self.events_observed.saturating_add(1);
    }
}

pub struct PassbyCanceller {
    phase: PassbyPhase,
    /// Sample index when the current phase was entered. Compared
    /// against `now_samples` to compute time-in-phase. Using
    /// sample-time instead of wall clock has two benefits:
    ///   1. Tests can feed synthesized signals at maximum speed
    ///      and still see the FSM transition correctly.
    ///   2. The FSM behavior becomes independent of the host
    ///      thread's scheduling — a busy CPU that delays a
    ///      process_window call no longer skews the transition
    ///      timers.
    phase_entered_at_samples: u64,
    /// Monotonic sample counter advanced by process_window.
    now_samples: u64,
    /// Energy across recent windows; the most recent is index 0.
    /// We keep the last few for the rising/falling detection that
    /// gates the Approach→Peak and Peak→Recede transitions.
    energy_history: Vec<f32>,
    /// Recent fundamental estimates (Hz) for Doppler tracking.
    /// Same indexing convention as `energy_history`.
    freq_history: Vec<f32>,
    /// Adaptive noise floor.
    noise_floor: f32,
    /// Learned per-phase durations.
    timers: TransitionTimers,
    /// Anti-noise phase accumulator (radians).
    anti_phase: f32,
    /// Anti-noise current amplitude (0..1).
    anti_amp: f32,
    /// Anti-noise fundamental in Hz (0 when not emitting).
    anti_freq: f32,
    /// Approach durations for the current event (used to update
    /// timers on Recede → Quiet).
    cur_approach_ms: u64,
    cur_peak_ms: u64,
    /// Phase-lock quality, 0..1. Measured each window by the
    /// `lock_phase_to_noise` routine — it's `1 - (best_residual /
    /// no_anti_residual)`, capped at 0. A value near 1 means the
    /// best phase choice nearly eliminates the in-band signal at
    /// the mic; near 0 means even the best phase doesn't help.
    /// Used by the snapshot for the UI and by the FSM to gate
    /// transitions (we don't aggressively ramp anti-noise if the
    /// phase lock is poor — that just amplifies the noise).
    phase_lock_quality: f32,
    /// FSM-driven amplitude ceiling. The envelope code computes
    /// this each window (Quiet=0, Approach=0.4, Peak=1.0,
    /// Recede=0.3) and the lock respects it. Decoupled from
    /// `anti_amp` (which is the actually-emitted level set by the
    /// lock's least-squares optimization) so the two don't fight:
    /// the FSM authorizes a ceiling, the lock picks the level
    /// underneath it that best matches mic.
    anti_amp_ceiling: f32,
    /// Saturation amount applied to anti-noise output (0..0.6).
    /// 0 = clean sinusoidal harmonics; higher = increasingly
    /// soft-clipped output with engine-like timbre. See
    /// `pull_output` for the math.
    saturation: f32,
    /// Per-harmonic phase offsets relative to `anti_phase`. The
    /// 2nd harmonic emits at `2*anti_phase + h2_offset` and the
    /// 3rd at `3*anti_phase + h3_offset`. Two regimes:
    ///
    ///   - When `lock_per_harmonic` is false (default), these stay
    ///     at the fixed-diversity values PI/6 and PI/3 — a
    ///     compromise that decorrelates the harmonics from the
    ///     fundamental's phase error (worst-case robustness at
    ///     the cost of 3 dB best-case suppression).
    ///
    ///   - When `lock_per_harmonic` is true, each lock cycle
    ///     INDEPENDENTLY estimates the source's phase at 2f and
    ///     3f and sets these offsets accordingly. The harmonics
    ///     become individually phase-locked rather than coherently
    ///     tied to the fundamental's offset. Best-case suppression
    ///     improves (5-8 dB more at harmonics if the source's
    ///     harmonic phase relationship is non-trivial); worst-case
    ///     stays similar to the fixed-offset version because the
    ///     phase search is independent per harmonic anyway.
    ///
    /// The user's experiment from the previous turn showed
    /// subtractive cancellation at the harmonics (timbre score
    /// 0.275 → 0.267 while h2/h3 amplitudes dropped from 0.25/0.15
    /// to ~0.0) is the actually-useful direction. This learnable
    /// per-harmonic offset is the mechanism that makes it work.
    h2_phase_offset: f32,
    h3_phase_offset: f32,
    /// Mode flag: true = per-harmonic phase locking (experimental);
    /// false = fixed phase diversity offsets (default).
    lock_per_harmonic: bool,
    /// Sliding-shelf saturation slope. 0.0 = flat (all harmonics
    /// get the same drive — original behavior). Positive values
    /// ramp drive upward with harmonic number: h2 gets `(1+slope)`
    /// times the base, h3 gets `(1+2·slope)` times. A gentle high-
    /// shelf filter on saturation. Range typically 0.0-1.0.
    sliding_shelf_slope: f32,
    /// Extended-harmonics mode (v1). The legacy synthesis emits
    /// anti-noise at h1, h2, h3 with fixed [1.0, 0.5, 0.25]
    /// amplitudes. The cross-reference experiment with real
    /// street recordings revealed that:
    ///
    ///   1. Real engines have substantial energy at h4-h7 (often
    ///      50-90% of the fundamental's amplitude), so a 3-harmonic
    ///      canceller is leaving 60-70% of the actual energy
    ///      untouched.
    ///   2. The amplitude ratios are far from [1.0, 0.5, 0.25].
    ///      Real h2 is OFTEN LOUDER than the fundamental (typical
    ///      4-stroke even-cylinder signature). Fixed amplitudes
    ///      systematically under-cancel where energy is actually
    ///      concentrated.
    ///
    /// Extended mode tracks N harmonics (typically 6-8) with
    /// AMPLITUDES MEASURED EACH WINDOW from the mic stream. Each
    /// harmonic's amplitude is EWMA-smoothed across windows to
    /// avoid window-to-window jitter destabilizing the synthesis.
    ///
    /// The "ideal canceller" upper-bound experiment showed +30 dB
    /// of available headroom over the 3-harmonic baseline on real
    /// engine signatures. In practice we won't approach that
    /// (mic capture has its own noise, channel jitter degrades
    /// alignment, etc.), but recovering even a third of that
    /// headroom is a clear, audible improvement.
    ///
    /// Indexing convention: `harmonic_amps[0]` is h1 (the
    /// fundamental), `harmonic_amps[1]` is h2, etc. Length is
    /// `n_harmonics`.
    n_harmonics: usize,
    /// Measured amplitudes per harmonic. Length = `n_harmonics`.
    /// Indexed from h1.
    harmonic_amps: Vec<f32>,
    /// Per-harmonic phase offsets. Length = `n_harmonics`.
    /// `harmonic_offsets[0]` (h1's offset) stays at 0 — h1's phase
    /// is `anti_phase` itself. The others are h2_offset, h3_offset,
    /// etc. — added to `n_h * anti_phase` in synthesis.
    harmonic_offsets: Vec<f32>,
}

impl Default for PassbyCanceller {
    fn default() -> Self {
        Self {
            phase: PassbyPhase::Quiet,
            phase_entered_at_samples: 0,
            now_samples: 0,
            energy_history: vec![0.0; 6],
            freq_history: vec![0.0; 6],
            noise_floor: 1e-4,
            timers: TransitionTimers::default(),
            anti_phase: 0.0,
            anti_amp: 0.0,
            anti_freq: 0.0,
            cur_approach_ms: 0,
            cur_peak_ms: 0,
            phase_lock_quality: 0.0,
            anti_amp_ceiling: 0.0,
            saturation: 0.0,
            h2_phase_offset: PI / 6.0,
            h3_phase_offset: PI / 3.0,
            lock_per_harmonic: false,
            sliding_shelf_slope: 0.0,
            // V1 default: 3 harmonics for backward compatibility.
            // set_n_harmonics(8) opts into the extended mode.
            n_harmonics: 3,
            // Initial amplitudes: legacy [1.0, 0.5, 0.25] profile.
            // When extended mode is enabled, set_n_harmonics
            // initializes with the measured real-engine profile
            // and the lock loop updates each window from there.
            harmonic_amps: vec![1.0, 0.5, 0.25],
            harmonic_offsets: vec![0.0, PI / 6.0, PI / 3.0],
        }
    }
}

impl PassbyCanceller {
    pub fn new() -> Self { Self::default() }

    pub fn phase(&self) -> PassbyPhase { self.phase }
    pub fn timers(&self) -> TransitionTimers { self.timers }
    pub fn snapshot(&self) -> PassbySnapshot {
        PassbySnapshot {
            phase: self.phase,
            fundamental_hz: self.anti_freq,
            band_energy: self.energy_history[0],
            noise_floor: self.noise_floor,
            anti_amp: self.anti_amp,
            timer_confidence: self.timers.confidence(),
            doppler_hz_per_s: self.recent_doppler_hz_per_s(),
            phase_lock_quality: self.phase_lock_quality,
        }
    }

    /// Lock the anti-noise phase AND amplitude against the mic-
    /// observed noise. The fuzz harness revealed two failure modes
    /// of the original open-loop approach:
    ///
    ///   1. Random phase: anti_phase starts at 0, noise has random
    ///      starting phase. Lucky alignment cancels; unlucky
    ///      amplifies. Fixed by phase search.
    ///
    ///   2. Wrong amplitude: anti_amp was envelope-driven
    ///      (Quiet=0, Approach=0.4, Peak=1.0). But the mic signal's
    ///      actual amplitude depends on the source loudness which
    ///      is independent of the FSM phase. A distant car at Peak-
    ///      phase produces mic samples at amplitude ~0.1; emitting
    ///      anti at 0.5 amplitude makes `noise - anti` LARGER than
    ///      noise. Net result: catastrophic amplification.
    ///
    /// ── How it works ──
    ///
    /// Phase search same as before: 16 candidate starts, pick min
    /// residual. THEN with phase fixed, solve analytically for the
    /// amplitude that minimizes residual:
    ///
    ///   minimize Σ(mic - amp · template)²  over amp
    ///   d/d_amp = -2 Σ template (mic - amp · template) = 0
    ///   amp_opt = Σ(mic · template) / Σ(template²)
    ///
    /// This is just linear regression. The result is the amplitude
    /// at which anti BEST matches mic in the least-squares sense.
    /// We clamp it to the envelope's current ceiling so the
    /// adaptive amp can never exceed what the FSM authorizes
    /// (Quiet still emits zero anti-noise even if the lock would
    /// want more).
    fn lock_phase_to_noise(&mut self, mic_window: &[f32]) {
        if self.anti_freq <= 0.0 || mic_window.is_empty() {
            self.phase_lock_quality = 0.0;
            return;
        }
        const N_PHASE_TRIALS: usize = 16;
        let dphase = 2.0 * PI * self.anti_freq / SAMPLE_RATE;

        let baseline: f32 = mic_window.iter()
            .map(|&x| x * x)
            .sum();
        if baseline <= 1e-12 {
            self.phase_lock_quality = 0.0;
            return;
        }

        // Cap the adaptive amplitude by the FSM envelope's current
        // ceiling — that's the "permission slip" the FSM gives us.
        let amp_ceiling = self.anti_amp_ceiling.max(0.05);
        let h2_offset = self.h2_phase_offset;
        let h3_offset = self.h3_phase_offset;

        let mut best_phase = self.anti_phase;
        let mut best_amp = 0.0f32;
        let mut best_residual = baseline;

        // We need unit-amplitude template values to compute amp_opt
        // for each trial. Pre-compute once: a Vec of template
        // values for each trial phase (one per sample in the window).
        let n = mic_window.len();
        let mut template: Vec<f32> = vec![0.0; n];

        // Template synthesis uses the SAME harmonic profile that
        // pull_output emits — so the lock is fitting a template
        // whose shape matches what we'll actually radiate. In
        // legacy 3-harmonic mode, this is [1.0, 0.5, 0.25]. In
        // extended mode, it's whatever measured profile is in
        // harmonic_amps. Without this matching, the lock fits
        // against the wrong spectral shape and the residual
        // never goes near zero on rich-spectrum sources.
        let template_amps: &[f32] = &self.harmonic_amps;
        let template_offsets: &[f32] = &self.harmonic_offsets;
        let template_n_harmonics = self.n_harmonics.min(template_amps.len());
        let template_norm: f32 = template_amps[..template_n_harmonics]
            .iter().sum::<f32>().max(1e-6);

        for trial in 0..N_PHASE_TRIALS {
            let trial_start = trial as f32 * 2.0 * PI / N_PHASE_TRIALS as f32;
            // Fill template at trial phase. The legacy fast path
            // (n_harmonics == 3 with default amps) preserves the
            // exact old behavior — sum of sins at 1.0, 0.5, 0.25
            // amplitudes with the h2/h3 offsets at fixed values
            // — so existing tests that assert lock numerics keep
            // working. Otherwise we iterate over the measured
            // harmonic vectors.
            let use_legacy_template = template_n_harmonics == 3
                && (template_amps[0] - 1.0).abs() < 0.01
                && (template_amps[1] - 0.5).abs() < 0.01
                && (template_amps[2] - 0.25).abs() < 0.01;
            if use_legacy_template {
                let mut phase = trial_start;
                for k in 0..n {
                    template[k] = (phase.sin()
                        + 0.5 * (2.0 * phase + h2_offset).sin()
                        + 0.25 * (3.0 * phase + h3_offset).sin()) / 1.75;
                    phase += dphase;
                    if phase > 2.0 * PI { phase -= 2.0 * PI; }
                }
            } else {
                let mut phase = trial_start;
                for k in 0..n {
                    let mut s = 0.0_f32;
                    for i in 0..template_n_harmonics {
                        let n_h = (i + 1) as f32;
                        s += template_amps[i]
                            * (n_h * phase + template_offsets[i]).sin();
                    }
                    template[k] = s / template_norm;
                    phase += dphase;
                    if phase > 2.0 * PI { phase -= 2.0 * PI; }
                }
            }
            // Compute amp_opt = Σ(mic*template) / Σ(template²)
            let mut dot_mt = 0.0f32;
            let mut sum_tt = 0.0f32;
            for k in 0..n {
                dot_mt += mic_window[k] * template[k];
                sum_tt += template[k] * template[k];
            }
            if sum_tt < 1e-9 { continue; }
            let amp_opt = (dot_mt / sum_tt).clamp(0.0, amp_ceiling);
            // Residual = Σ(mic - amp_opt * template)²
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
        // Apply: snap phase, adapt amplitude.
        self.anti_phase = best_phase;
        self.anti_amp = best_amp;
        let q = 1.0 - (best_residual / baseline);
        self.phase_lock_quality = q.clamp(0.0, 1.0);

        // Per-harmonic offset refinement — runs AFTER the
        // fundamental lock so the Goertzel-derived offsets use the
        // freshly-locked anti_phase as their reference. EWMA-smooth
        // into the instance fields so window-to-window noise
        // doesn't whiplash the synthesis.
        if self.lock_per_harmonic {
            let (h2_off, h3_off) = self.estimate_harmonic_offsets(mic_window);
            const ALPHA: f32 = 0.4;
            self.h2_phase_offset = wrap_2pi(
                (1.0 - ALPHA) * self.h2_phase_offset + ALPHA * h2_off);
            self.h3_phase_offset = wrap_2pi(
                (1.0 - ALPHA) * self.h3_phase_offset + ALPHA * h3_off);
        }

        // Extended-harmonics mode (v1): The trial-search above gives
        // ±π/16 (0.2 rad) accuracy on the fundamental phase. For
        // the 3-harmonic legacy template that's fine; at h3, 0.2
        // rad becomes 0.6 rad, manageable. But for h7-h8 in
        // extended mode, the same fundamental error becomes 1.4-
        // 1.6 rad — basically random phase. So we need a TIGHTER
        // fundamental phase estimate before refining the harmonics.
        //
        // Strategy: a second pass around the trial-search winner
        // with finer resolution (8 trials over a ±π/16 range), then
        // refresh per-harmonic amps + offsets.
        if self.n_harmonics > 3 {
            self.refine_fundamental_phase_around_winner(mic_window);
            self.refresh_extended_harmonics(mic_window);
        }
    }

    /// Second-pass fundamental phase search: zoom in around the
    /// 16-trial winner with 8 finer trials. Improves phase
    /// accuracy from ±π/16 (0.196 rad) to ±π/128 (0.025 rad),
    /// which keeps h8 phase error under 0.2 rad.
    fn refine_fundamental_phase_around_winner(&mut self, mic_window: &[f32]) {
        if self.anti_freq <= 0.0 || mic_window.is_empty() { return; }
        const FINE_TRIALS: usize = 8;
        // Search ±π/16 around current phase, at finer resolution.
        const SEARCH_HALF_RANGE: f32 = PI / 16.0;
        let center = self.anti_phase;
        let n = mic_window.len();
        let template_amps = &self.harmonic_amps;
        let template_offsets = &self.harmonic_offsets;
        let template_n = self.n_harmonics.min(template_amps.len());
        let template_norm: f32 = template_amps[..template_n]
            .iter().sum::<f32>().max(1e-6);
        let dphase = 2.0 * PI * self.anti_freq / SAMPLE_RATE;
        let amp_ceiling = self.anti_amp_ceiling.max(0.05);
        let mut best_phase = center;
        let mut best_amp = self.anti_amp;
        let mut best_residual = f32::MAX;
        for trial in 0..FINE_TRIALS {
            let frac = (trial as f32 / (FINE_TRIALS - 1) as f32) - 0.5;
            let trial_start = wrap_2pi(center + frac * 2.0 * SEARCH_HALF_RANGE);
            let mut dot_mt = 0.0_f32;
            let mut sum_tt = 0.0_f32;
            let mut phase = trial_start;
            // Pre-compute templates for this trial as we go.
            // No allocation — accumulate the regression terms.
            for k in 0..n {
                let mut s = 0.0_f32;
                for i in 0..template_n {
                    let n_h = (i + 1) as f32;
                    s += template_amps[i]
                        * (n_h * phase + template_offsets[i]).sin();
                }
                let tk = s / template_norm;
                dot_mt += mic_window[k] * tk;
                sum_tt += tk * tk;
                phase += dphase;
                if phase > 2.0 * PI { phase -= 2.0 * PI; }
            }
            if sum_tt < 1e-9 { continue; }
            let amp_opt = (dot_mt / sum_tt).clamp(0.0, amp_ceiling);
            // Residual pass.
            let mut residual_sq = 0.0_f32;
            let mut phase = trial_start;
            for k in 0..n {
                let mut s = 0.0_f32;
                for i in 0..template_n {
                    let n_h = (i + 1) as f32;
                    s += template_amps[i]
                        * (n_h * phase + template_offsets[i]).sin();
                }
                let tk = s / template_norm;
                let r = mic_window[k] - amp_opt * tk;
                residual_sq += r * r;
                phase += dphase;
                if phase > 2.0 * PI { phase -= 2.0 * PI; }
            }
            if residual_sq < best_residual {
                best_residual = residual_sq;
                best_phase = trial_start;
                best_amp = amp_opt;
            }
        }
        self.anti_phase = best_phase;
        self.anti_amp = best_amp;
        // Update phase_lock_quality from the refined fit.
        let baseline_sq: f32 = mic_window.iter().map(|x| x * x).sum();
        if baseline_sq > 1e-12 && best_residual < f32::MAX {
            let q = 1.0 - best_residual / baseline_sq;
            self.phase_lock_quality = q.clamp(0.0, 1.0);
        }
    }

    /// Measure amplitude AND phase of each tracked harmonic from
    /// the mic window, then EWMA-smooth into self.harmonic_amps
    /// and self.harmonic_offsets. Only invoked when the canceller
    /// is in extended mode (n_harmonics > 3).
    ///
    /// Amplitudes are normalized to h1 = 1.0. Phase derivation
    /// matches `estimate_harmonic_offsets`:
    ///   h_n_offset = phi_mic_n*f - n*anti_phase + π/2
    /// (See the long derivation comment on that method.)
    fn refresh_extended_harmonics(&mut self, mic_window: &[f32]) {
        if self.anti_freq <= 0.0 || mic_window.is_empty() {
            return;
        }
        let theta_0 = self.anti_phase;
        // Goertzel-complex at each harmonic.
        let mut new_amps = vec![0.0_f32; self.n_harmonics];
        let mut new_offsets = vec![0.0_f32; self.n_harmonics];
        let mut h1_mag = 0.0_f32;
        for i in 0..self.n_harmonics {
            let n_h = (i + 1) as f32;
            let f_h = self.anti_freq * n_h;
            if f_h >= SAMPLE_RATE * 0.45 {
                // Don't update harmonics above the aliasing limit.
                new_amps[i] = self.harmonic_amps[i];
                new_offsets[i] = self.harmonic_offsets[i];
                continue;
            }
            let (re, im) = goertzel_complex(mic_window, f_h, SAMPLE_RATE);
            let mag = (re * re + im * im).sqrt();
            if i == 0 { h1_mag = mag; }
            // Normalized amplitude — divide by h1's magnitude
            // (skip if h1 is too quiet to be meaningful).
            new_amps[i] = if h1_mag > mic_window.len() as f32 * 0.001 {
                mag / h1_mag
            } else {
                self.harmonic_amps[i]
            };
            // Phase: per the derivation.
            new_offsets[i] = wrap_2pi(
                im.atan2(re) - n_h * theta_0 + PI / 2.0);
        }
        const ALPHA: f32 = 0.3;
        for i in 0..self.n_harmonics {
            self.harmonic_amps[i] =
                (1.0 - ALPHA) * self.harmonic_amps[i] + ALPHA * new_amps[i];
            self.harmonic_offsets[i] = wrap_2pi(
                (1.0 - ALPHA) * self.harmonic_offsets[i] + ALPHA * new_offsets[i]);
        }
    }

    /// Feed a fresh window of mic samples (typically WINDOW_SAMPLES
    /// long). Returns the snapshot after the update.
    ///
    /// The function does three things:
    ///   1. Computes in-band energy + fundamental for THIS window
    ///   2. Advances the FSM based on the trajectory
    ///   3. Decays/ramps the anti-noise envelope
    ///
    /// Implementation is intentionally simple — Goertzel-like
    /// in-band power + autocorrelation peak for the fundamental.
    /// An FFT would be more accurate but pulls in a dep; for the
    /// engine-band content we care about, the simpler estimators
    /// are within a few percent and stable enough for the FSM gates.
    pub fn process_window(&mut self, samples: &[f32]) -> PassbySnapshot {
        // Advance the internal sample clock — every FSM transition
        // gate compares phase-entered-at against this. Using sample-
        // time keeps the FSM's behavior consistent across (a) live
        // capture where wall clock and sample arrival are 1:1, and
        // (b) tests/fuzz/replay where samples come in faster than
        // real time.
        self.now_samples = self.now_samples.saturating_add(samples.len() as u64);
        // ── Compute in-band energy via a simple rectangular bandpass ──
        let energy = band_energy_rms(samples, BAND_LO_HZ, BAND_HI_HZ, SAMPLE_RATE);
        // ── Estimate fundamental via autocorrelation peak in the
        // engine band ──
        let f0 = estimate_fundamental(samples, BAND_LO_HZ, BAND_HI_HZ, SAMPLE_RATE);
        // ── Update sliding histories ──
        // Inserting at index 0; shifting older values up.
        for i in (1..self.energy_history.len()).rev() {
            self.energy_history[i] = self.energy_history[i - 1];
            self.freq_history[i]   = self.freq_history[i - 1];
        }
        self.energy_history[0] = energy;
        self.freq_history[0]   = f0;
        // ── Noise floor EWMA — only updated outside Approach/Peak/Recede
        // so a long pass-by doesn't bias the floor up ──
        if self.phase == PassbyPhase::Quiet {
            self.noise_floor = (1.0 - NOISE_FLOOR_ALPHA) * self.noise_floor
                             + NOISE_FLOOR_ALPHA * energy.max(1e-6);
        }
        // ── FSM tick ──
        self.advance_fsm();
        // ── Anti-noise envelope ── (drives the FSM-authorized
        //    amplitude ceiling that the lock respects)
        self.update_anti_noise_envelope();
        // ── Phase-lock the anti-noise generator against this
        //    window's mic content. Only meaningful when we're
        //    actively emitting (Approach/Peak/Recede); during
        //    Quiet the ceiling is 0 so the lock can't ramp up.
        if self.phase != PassbyPhase::Quiet && self.anti_freq > 0.0 {
            self.lock_phase_to_noise(samples);
        } else {
            self.phase_lock_quality = 0.0;
            self.anti_amp = 0.0;
        }
        self.snapshot()
    }

    /// Pull `n` samples of anti-noise output. Zero when phase is
    /// Quiet; sinusoidal at `anti_freq` with current `anti_amp`
    /// envelope otherwise. Caller multiplies by output gain and
    /// optionally inverts; this function returns the POSITIVE
    /// sinusoid so callers can compose anti = -K * sample.
    ///
    /// ── Harmonic phase diversity ──
    ///
    /// The three harmonics (f, 2f, 3f) used to share one phase
    /// accumulator — perfectly coherent. That sounds right but has
    /// a hidden failure mode: a time-delay error Δt in delivery
    /// produces phase errors Δφ_n = 2π·n·f·Δt, growing linearly
    /// with harmonic number. With coherent harmonics, a misalignment
    /// of 60° at the fundamental means the 2nd harmonic is 120° off
    /// (residue is AMPLIFIED) and the 3rd is 180° off (full
    /// constructive interference — worst case).
    ///
    /// We add deliberate per-harmonic phase OFFSETS (π/6 at 2f, π/3
    /// at 3f) so the harmonics are uncorrelated with the
    /// fundamental's phase error. In the best case (Δt = 0) this
    /// costs ~3 dB of cancellation at the harmonics. In the worst
    /// case (Δt large) it turns "all harmonics constructively
    /// interfere → +6 dB amplification" into "harmonics decorrelate
    /// → ~0 dB net contribution" — a 6 dB win at the bad end.
    ///
    /// The diversity offsets are fixed constants here; a future
    /// adaptive variant could mic-feedback-tune them per session.
    pub fn pull_output(&mut self, n: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(n);
        if self.anti_amp <= 1e-6 || self.anti_freq <= 0.0 {
            out.resize(n, 0.0);
            return out;
        }
        let dphase = 2.0 * PI * self.anti_freq / SAMPLE_RATE;
        let h2_offset = self.h2_phase_offset;
        let h3_offset = self.h3_phase_offset;
        // Saturation mixin: real engines have characteristic
        // asymmetric soft-clipping from exhaust resonance + the
        // pressure asymmetry of the combustion cycle. Pure sin +
        // harmonic sum sounds CLEANER than a real engine — when
        // we sign-invert it and mix with the mic-observed noise,
        // the difference SOUNDS like the engine's "dirty" timbre
        // is what's left over (because we cancelled the clean
        // sinusoidal content).
        //
        // Adding mild asymmetric soft-clip to the anti-noise
        // brings its timbre closer to the source's, so the
        // cancellation handles more of the perceived "engine
        // character" not just the pure tonal content.
        //
        // Default saturation 0.0 = pre-existing behavior. Values
        // up to ~0.3 inject increasingly distorted (more
        // harmonically rich) anti-noise.
        //
        // ── Sliding-shelf saturation ──
        //
        // The previous flat-saturation experiment showed a band
        // trade-off: matched saturation wins at the upper harmonics
        // (engine "character" lives there) but loses at the
        // fundamental. The user's insight: don't saturate low bins
        // as much, slide the saturation upward with frequency —
        // like a gentle high-shelf filter on the drive amount.
        //
        // Implementation: instead of summing harmonics and
        // saturating the sum, soft-clip each harmonic INDEPENDENTLY
        // with a frequency-dependent drive. The drive ramps from
        // `saturation * shelf_low_multiplier` at the fundamental
        // up to `saturation * shelf_high_multiplier` at the upper
        // harmonics.
        //
        // Two modes:
        //   - `sliding_shelf_slope == 0.0` (default): all harmonics
        //     get the same drive — same as the old flat behavior.
        //   - `sliding_shelf_slope > 0.0`: each harmonic gets drive
        //     `sat * (1.0 + slope * (n - 1))` — h1 at sat,
        //     h2 at sat*(1+slope), h3 at sat*(1+2*slope). The
        //     fundamental stays mostly clean; the harmonics get
        //     more saturation. At slope = 0.5: h1=sat, h2=1.5·sat,
        //     h3=2·sat — a moderate ramp.
        let saturation = self.saturation;
        let shelf_slope = self.sliding_shelf_slope;
        // Closure: asymmetric soft-clip with given drive.
        let clip = |x: f32, sat: f32| -> f32 {
            if sat <= 1e-6 { return x; }
            let bias = 0.15 * sat;
            let drive = 1.0 + 2.0 * sat;
            let driven = x * drive + bias;
            let clipped = driven.tanh() / drive;
            let dc_offset = bias.tanh() / drive;
            clipped - dc_offset
        };
        let h1_sat = saturation;
        let h2_sat = (saturation * (1.0 + shelf_slope)).clamp(0.0, 0.9);
        let h3_sat = (saturation * (1.0 + 2.0 * shelf_slope)).clamp(0.0, 0.9);
        // Branch: legacy 3-harmonic path (preserves backward-
        // compatibility on amplitudes), or extended N-harmonic
        // path that iterates over harmonic_amps / harmonic_offsets.
        if self.n_harmonics == 3
            && (self.harmonic_amps.len() < 3
                || (self.harmonic_amps[0] - 1.0).abs() < 0.01
                    && (self.harmonic_amps[1] - 0.5).abs() < 0.01
                    && (self.harmonic_amps[2] - 0.25).abs() < 0.01)
        {
            // Legacy synthesis — keeps existing tests passing.
            for _ in 0..n {
                let h1 = clip(self.anti_phase.sin(), h1_sat);
                let h2 = clip(0.5 * (2.0 * self.anti_phase + h2_offset).sin(), h2_sat);
                let h3 = clip(0.25 * (3.0 * self.anti_phase + h3_offset).sin(), h3_sat);
                let normalized = (h1 + h2 + h3) / 1.75;
                let s = normalized * self.anti_amp;
                out.push(s.clamp(-1.0, 1.0));
                self.anti_phase += dphase;
                if self.anti_phase > 2.0 * PI {
                    self.anti_phase -= 2.0 * PI;
                }
            }
        } else {
            // Extended N-harmonic synthesis using measured amps +
            // measured offsets. Aliasing: skip harmonics whose
            // frequency exceeds sr * 0.45 (well below Nyquist for
            // headroom). The sum is normalized by the sum of
            // amplitudes so the output amplitude doesn't grow
            // with harmonic count.
            let sample_rate_hz = SAMPLE_RATE;
            let nyquist_safe = sample_rate_hz * 0.45;
            // Pre-compute the max harmonic index that doesn't
            // alias, and the normalization factor.
            let mut max_idx = self.n_harmonics;
            let mut norm: f32 = 0.0;
            for i in 0..self.n_harmonics {
                let n_h = (i + 1) as f32;
                let f_h = self.anti_freq * n_h;
                if f_h >= nyquist_safe {
                    max_idx = i;
                    break;
                }
                norm += self.harmonic_amps[i];
            }
            let norm_inv = if norm > 1e-6 { 1.0 / norm } else { 0.0 };
            // Per-harmonic saturation drive (sliding-shelf).
            // h_sat[i] = saturation * (1 + slope * i).
            for _ in 0..n {
                let mut total = 0.0_f32;
                for i in 0..max_idx {
                    let n_h = (i + 1) as f32;
                    let amp = self.harmonic_amps[i];
                    let offset = self.harmonic_offsets[i];
                    let sat = (saturation * (1.0 + shelf_slope * i as f32))
                        .clamp(0.0, 0.9);
                    let raw = (n_h * self.anti_phase + offset).sin() * amp;
                    total += clip(raw, sat);
                }
                let normalized = total * norm_inv;
                let s = normalized * self.anti_amp;
                out.push(s.clamp(-1.0, 1.0));
                self.anti_phase += dphase;
                if self.anti_phase > 2.0 * PI {
                    self.anti_phase -= 2.0 * PI;
                }
            }
        }
        out
    }

    /// Estimate the optimal per-harmonic phase offsets via
    /// Goertzel-complex extraction of the source's phase at each
    /// harmonic. This time with the sign convention derived from
    /// scratch, not from memory.
    ///
    /// ── Derivation ──
    ///
    /// pull_output emits `0.5 · anti_amp · sin(2·θ + h2_offset)`
    /// for the 2nd harmonic (where θ is the current anti_phase).
    /// Over a window starting at t=0, the emitted signal is
    /// `0.5·A·sin(2·θ_0 + 2·dphase·k + h2_offset)`
    /// = `0.5·A·sin(2π·2f·k/SR + (2·θ_0 + h2_offset))`
    /// = `0.5·A·cos(2π·2f·k/SR + (2·θ_0 + h2_offset - π/2))`.
    ///
    /// goertzel_complex returns (real, imag) for a cos-fit:
    /// for input `A·cos(2π·k·n/N + φ)` it returns ≈(N/2·A·cos(φ),
    /// N/2·A·sin(φ)). atan2(imag, real) = φ.
    ///
    /// For the EMITTED signal: Goertzel-phase = 2·θ_0 + h2_offset - π/2.
    ///
    /// For the MIC signal at 2f, Goertzel-phase = φ_mic_2f.
    ///
    /// For cancellation (`residual = mic - anti` minimized), we
    /// want emitted ≈ mic at 2f, which means matching phases:
    ///   2·θ_0 + h2_offset - π/2 = φ_mic_2f
    ///   h2_offset = φ_mic_2f - 2·θ_0 + π/2
    ///
    /// Similarly h3_offset = φ_mic_3f - 3·θ_0 + π/2.
    ///
    /// CRITICAL: θ_0 here is the LOCKED anti_phase (after the
    /// fundamental lock). The caller must invoke this method AFTER
    /// running the fundamental lock, not before.
    fn estimate_harmonic_offsets(&self, mic_window: &[f32]) -> (f32, f32) {
        if self.anti_freq <= 0.0 || mic_window.is_empty() {
            return (self.h2_phase_offset, self.h3_phase_offset);
        }
        let f2 = self.anti_freq * 2.0;
        let f3 = self.anti_freq * 3.0;
        let (re2, im2) = goertzel_complex(mic_window, f2, SAMPLE_RATE);
        let (re3, im3) = goertzel_complex(mic_window, f3, SAMPLE_RATE);
        // Sanity: if the source has no measurable content at this
        // harmonic, leave the offset alone (don't introduce noise).
        let mag2 = (re2 * re2 + im2 * im2).sqrt();
        let mag3 = (re3 * re3 + im3 * im3).sqrt();
        let n = mic_window.len() as f32;
        let mag_threshold = n * 0.005;  // RMS amplitude ~0.005 minimum
        let phi_mic_2f = if mag2 > mag_threshold { im2.atan2(re2) }
            else { return (self.h2_phase_offset, self.h3_phase_offset); };
        let phi_mic_3f = if mag3 > mag_threshold { im3.atan2(re3) }
            else { return (self.h2_phase_offset, self.h3_phase_offset); };
        // Apply the derivation:
        //   h_n_offset = φ_mic_n·f - n·θ_0 + π/2
        // where θ_0 is the current (just-locked) anti_phase.
        let theta_0 = self.anti_phase;
        let h2 = phi_mic_2f - 2.0 * theta_0 + PI / 2.0;
        let h3 = phi_mic_3f - 3.0 * theta_0 + PI / 2.0;
        (wrap_2pi(h2), wrap_2pi(h3))
    }

    /// Enable per-harmonic phase locking. When true, each window
    /// re-estimates the optimal h2 + h3 offsets from the mic
    /// signal (with EWMA smoothing); when false (default), the
    /// fixed diversity offsets (π/6, π/3) are used.
    ///
    /// The per-harmonic experiment in network_sim.rs shows:
    ///   - Fixed wins ~2.5 dB at ideal conditions (0-20 ms jitter)
    ///   - Per-harmonic wins ~1 dB at adverse conditions
    ///     (50+ms latency with jitter)
    /// So manual setting is most useful in known-degraded
    /// environments; for typical home BT use stay with fixed.
    pub fn set_lock_per_harmonic(&mut self, on: bool) {
        self.lock_per_harmonic = on;
    }

    /// Auto-enable per-harmonic based on phase lock quality. When
    /// the lock is good (quality > 0.6) the canceller stays in
    /// fixed mode where it performs better. When the lock starts
    /// failing (< 0.3), switch to per-harmonic for the extra dB.
    /// The 0.3-0.6 zone is a hysteresis band — no switching
    /// happens there. Call this method once per window from the
    /// caller.
    pub fn auto_select_lock_mode(&mut self) {
        if self.lock_per_harmonic && self.phase_lock_quality > 0.6 {
            // Lock recovered well — switch back to fixed for the
            // best-case suppression.
            self.lock_per_harmonic = false;
        } else if !self.lock_per_harmonic && self.phase_lock_quality < 0.3
                  && self.phase != PassbyPhase::Quiet {
            // Lock failing — try per-harmonic to scrape extra dB.
            // Gated on FSM state so we don't flip into per-harmonic
            // during the natural quiet-phase low-lock-quality
            // periods.
            self.lock_per_harmonic = true;
        }
    }

    pub fn lock_per_harmonic(&self) -> bool { self.lock_per_harmonic }
    pub fn h2_phase_offset(&self) -> f32 { self.h2_phase_offset }
    pub fn h3_phase_offset(&self) -> f32 { self.h3_phase_offset }

    /// Set the number of harmonics the canceller tracks. Range
    /// 1-12. The default is 3 (legacy behavior). For real engines
    /// 6-8 captures the bulk of the energy. Calling this resets
    /// the amplitude estimates to a measured real-engine profile
    /// (so the first few windows start with reasonable values
    /// before the lock loop refines them).
    pub fn set_n_harmonics(&mut self, n: usize) {
        let n = n.clamp(1, 12);
        self.n_harmonics = n;
        // Profile measured from real street recording (4-stroke
        // engine, mid-pass). When n < 8 we truncate; when n > 8
        // we extend with a 0.1 tail (real engines do have residual
        // content above h8 but it's small).
        let measured: [f32; 12] = [
            1.00, 1.30, 0.26, 0.88, 0.78, 0.50, 0.79, 0.14,
            0.10, 0.08, 0.06, 0.04,
        ];
        self.harmonic_amps = (0..n).map(|i| measured[i]).collect();
        // Initialize per-harmonic phase offsets to ZERO. Most real
        // engine recordings show all harmonics roughly phase-aligned
        // with the fundamental (they all originate from the same
        // combustion cycle and propagate together). Starting at 0
        // means the lock loop's first trial-search has the template
        // already in roughly the right shape; the refresh step
        // then tunes individual offsets if the source actually has
        // per-harmonic phase variation.
        //
        // (Legacy 3-harmonic mode keeps PI/6 and PI/3 because the
        // synthesis path detects that case and uses fixed-diversity
        // offsets there — different design philosophy: legacy
        // optimizes for worst-case robustness; extended mode
        // optimizes for measured-source matching.)
        self.harmonic_offsets = vec![0.0; n];
    }

    pub fn n_harmonics(&self) -> usize { self.n_harmonics }
    pub fn harmonic_amps(&self) -> &[f32] { &self.harmonic_amps }
    pub fn harmonic_offsets(&self) -> &[f32] { &self.harmonic_offsets }

    /// Set the anti-noise saturation amount (0.0 = clean sinusoid,
    /// 0.3 = "diesel-like" saturated, 0.6 = heavily distorted).
    /// Capped to [0.0, 0.6].
    pub fn set_saturation(&mut self, s: f32) {
        self.saturation = s.clamp(0.0, 0.6);
    }

    pub fn saturation(&self) -> f32 { self.saturation }

    /// Set the sliding-shelf slope. 0 = flat saturation (legacy
    /// behavior, all harmonics same drive). Positive ramps drive
    /// upward with harmonic number. Range clamped to 0.0-2.0.
    pub fn set_sliding_shelf_slope(&mut self, slope: f32) {
        self.sliding_shelf_slope = slope.clamp(0.0, 2.0);
    }

    pub fn sliding_shelf_slope(&self) -> f32 { self.sliding_shelf_slope }

    /// Recent Doppler rate in Hz/s, smoothed across the freq_history.
    /// Positive: rising (approaching). Negative: falling (receding).
    fn recent_doppler_hz_per_s(&self) -> f32 {
        // Use the oldest and newest entries; intermediate values can
        // be noisy. Each history slot represents a process_window
        // call, which the caller spaces at ~window-duration intervals.
        let oldest_idx = self.freq_history.len() - 1;
        let oldest = self.freq_history[oldest_idx];
        let newest = self.freq_history[0];
        if oldest <= 0.0 || newest <= 0.0 { return 0.0; }
        // The history's time span is roughly len * window_duration.
        // We approximate window_duration as WINDOW_SAMPLES / SAMPLE_RATE.
        let span_secs = (self.energy_history.len() as f32)
            * (WINDOW_SAMPLES as f32 / SAMPLE_RATE);
        if span_secs <= 0.0 { return 0.0; }
        (newest - oldest) / span_secs
    }

    fn advance_fsm(&mut self) {
        let elapsed_samples = self.now_samples - self.phase_entered_at_samples;
        let in_phase_ms = (elapsed_samples as f32 * 1000.0 / SAMPLE_RATE) as u64;
        let energy = self.energy_history[0];
        let prev_energy = self.energy_history[1];
        match self.phase {
            PassbyPhase::Quiet => {
                if energy > self.noise_floor * APPROACH_ENTRY_RATIO
                    && self.freq_history[0] >= BAND_LO_HZ
                {
                    self.transition(PassbyPhase::Approach);
                }
            }
            PassbyPhase::Approach => {
                if in_phase_ms > APPROACH_MAX_MS {
                    // Stuck — degrade to Quiet without recording the
                    // event (likely a false positive or stop-and-go).
                    self.transition(PassbyPhase::Quiet);
                    return;
                }
                // ── Geometry-aware Doppler gate ──
                //
                // The original gate `doppler.abs() >= 0.5 Hz/s` was
                // calibrated for "close, parallel passbys" — cars
                // along a street where the source moves roughly
                // parallel to the listener at 5-15 m distance. In
                // that geometry, the radial velocity changes rapidly
                // and Doppler rate is high (1-5 Hz/s).
                //
                // For PERPENDICULAR sources (cross-traffic, sources
                // moving toward/away on a line that doesn't pass
                // close to the listener), the radial velocity change
                // is much slower. A car 100m away crossing at right
                // angles might never produce Doppler > 0.5 Hz/s,
                // even though it's clearly a moving source with a
                // strong envelope.
                //
                // The fix: relax the Doppler gate when the envelope
                // is STRONG (well above noise floor) AND the timer
                // model has high confidence about passby duration.
                // Strong envelope + recognized timing pattern is
                // enough evidence of "moving source" even without
                // Doppler.
                let doppler = self.recent_doppler_hz_per_s();
                let plateau = (energy - prev_energy).abs() < (energy * 0.05);
                let envelope_strong =
                    energy > self.noise_floor * (APPROACH_ENTRY_RATIO * 2.0);
                // Additional check for relaxed-Doppler path: the
                // envelope must have RISEN — the oldest history slot
                // should be significantly quieter than the newest.
                // This separates real passby envelopes (low → high →
                // low) from stationary periodic noise (always-high).
                let oldest_idx = self.energy_history.len() - 1;
                let oldest_energy = self.energy_history[oldest_idx];
                let envelope_rose = oldest_energy * 2.0 < energy;
                let high_confidence = self.timers.confidence() > 0.6;
                let strong_doppler = doppler.abs() >= DOPPLER_GATE_HZ_PER_S;
                let weak_doppler_recognized_shape =
                    envelope_strong && envelope_rose && high_confidence;
                let no_prior_to_filter_against =
                    self.timers.confidence() < 0.4;
                if in_phase_ms >= APPROACH_MIN_MS && plateau
                    && (strong_doppler
                        || no_prior_to_filter_against
                        || weak_doppler_recognized_shape)
                {
                    self.cur_approach_ms = in_phase_ms;
                    self.transition(PassbyPhase::Peak);
                }
            }
            PassbyPhase::Peak => {
                if in_phase_ms > PEAK_MAX_MS {
                    // Force progression — a real Peak doesn't last 8s.
                    self.cur_peak_ms = in_phase_ms;
                    self.transition(PassbyPhase::Recede);
                    return;
                }
                // Move to Recede when energy starts dropping AND we've
                // dwelled at least the minimum.
                if in_phase_ms >= PEAK_MIN_MS && energy < prev_energy * 0.9 {
                    self.cur_peak_ms = in_phase_ms;
                    self.transition(PassbyPhase::Recede);
                }
            }
            PassbyPhase::Recede => {
                if in_phase_ms > RECEDE_MAX_MS {
                    // Force back to Quiet but DON'T record timers —
                    // the event was already malformed.
                    self.transition(PassbyPhase::Quiet);
                    return;
                }
                if in_phase_ms >= RECEDE_MIN_MS
                    && energy < self.noise_floor * QUIET_EXIT_RATIO
                {
                    // Clean exit — update learned timers from this
                    // event's observed durations.
                    let recede_ms = in_phase_ms as f32;
                    self.timers.observe(
                        self.cur_approach_ms as f32,
                        self.cur_peak_ms as f32,
                        recede_ms,
                    );
                    self.transition(PassbyPhase::Quiet);
                }
            }
        }
    }

    fn transition(&mut self, next: PassbyPhase) {
        self.phase = next;
        self.phase_entered_at_samples = self.now_samples;
        match next {
            PassbyPhase::Quiet => {
                self.cur_approach_ms = 0;
                self.cur_peak_ms = 0;
            }
            PassbyPhase::Approach => {
                // Snap anti_freq to the current fundamental estimate;
                // we'll track Doppler updates within the phase.
                if self.freq_history[0] >= BAND_LO_HZ {
                    self.anti_freq = self.freq_history[0];
                }
            }
            PassbyPhase::Peak => {
                // Lock the anti_freq to the Peak-time fundamental —
                // that's the engine's actual rate (Doppler-free, since
                // perpendicular pass-by).
                if self.freq_history[0] >= BAND_LO_HZ {
                    self.anti_freq = self.freq_history[0];
                }
            }
            _ => {}
        }
    }

    /// Shape the anti-noise amplitude envelope across the FSM phases.
    ///
    ///   Quiet     → ramp toward 0
    ///   Approach  → ramp up gradually (slow, since we're not sure
    ///                yet whether this will resolve to a real pass-by)
    ///   Peak      → full amplitude
    ///   Recede    → ramp down faster than Approach ramped up
    ///                (the brain notices LATE cancellation more than
    ///                EARLY release)
    fn update_anti_noise_envelope(&mut self) {
        // The envelope drives `anti_amp_ceiling` rather than
        // `anti_amp` directly — the lock's least-squares step is
        // what sets `anti_amp` to the value that minimizes the
        // mic-observed residual. The envelope's job is to gate
        // "are we ALLOWED to emit anti-noise right now?" not
        // "exactly how much."
        let target = match self.phase {
            PassbyPhase::Quiet    => 0.0,
            PassbyPhase::Approach => 0.4,
            PassbyPhase::Peak     => 1.0,
            PassbyPhase::Recede   => 0.3,
        };
        let rate = match self.phase {
            PassbyPhase::Quiet    => 0.02,
            PassbyPhase::Approach => 0.01,
            PassbyPhase::Peak     => 0.10,
            PassbyPhase::Recede   => 0.05,
        };
        self.anti_amp_ceiling += (target - self.anti_amp_ceiling) * rate;
        self.anti_amp_ceiling = self.anti_amp_ceiling.clamp(0.0, 1.0);
        // Whenever the ceiling drops below the current emission
        // level, drag the emission down to match. Otherwise the
        // emission stays at whatever the last lock set it to.
        if self.anti_amp > self.anti_amp_ceiling {
            self.anti_amp = self.anti_amp_ceiling;
        }
    }

    /// Reset learned timers — useful when the user moves to a new
    /// location with a different traffic pattern. The QML side
    /// exposes this as "forget pattern."
    pub fn reset_timers(&mut self) {
        self.timers = TransitionTimers::default();
    }
}

/// Simple Goertzel-style in-band RMS energy. Filters the input to
/// the [lo, hi] Hz band by summing magnitudes at discrete tone
/// candidates, then divides by the candidate count to get an RMS-
/// like number.
///
/// Not a real FFT but accurate enough for the energy gate — we just
/// need to distinguish "loud in 80-300 Hz" from "noise floor."
pub fn band_energy_rms(samples: &[f32], lo: f32, hi: f32, sr: f32) -> f32 {
    if samples.is_empty() { return 0.0; }
    // Evaluate at a small set of probe frequencies and average.
    // 16 probes from lo to hi gives smooth coverage with low CPU.
    let n_probes = 16;
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

/// Wrap a phase value to [0, 2π). Used by the per-harmonic offset
/// estimator to keep its returned offsets in a canonical range
/// before EWMA smoothing across windows.
fn wrap_2pi(phi: f32) -> f32 {
    let mut x = phi;
    while x < 0.0 { x += 2.0 * PI; }
    while x >= 2.0 * PI { x -= 2.0 * PI; }
    x
}

/// Goertzel single-tone magnitude. Returns the |X(f)| of the input
/// at the specified frequency. Defended against NaN/Inf inputs:
/// a non-finite sample is treated as 0 (same convention as the
/// rest of the audio path).
pub fn goertzel(samples: &[f32], freq_hz: f32, sr: f32) -> f32 {
    let (re, im) = goertzel_complex(samples, freq_hz, sr);
    (re * re + im * im).sqrt()
}

/// Complex Goertzel — returns `(real, imag)` components so callers
/// can extract phase via `atan2(imag, real)`. Same NaN defense as
/// the magnitude-only variant.
///
/// The phase tells you WHERE in its cycle the input's tonal
/// component is at sample 0 of the window. We use this for phase-
/// locking the anti-noise generator against the mic-observed
/// noise.
pub fn goertzel_complex(samples: &[f32], freq_hz: f32, sr: f32) -> (f32, f32) {
    if samples.is_empty() || !freq_hz.is_finite() || !sr.is_finite() || sr <= 0.0 {
        return (0.0, 0.0);
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
    (real, imag)
}

/// Estimate the fundamental frequency in the [lo, hi] band by
/// scanning Goertzel responses then interpolating.
///
/// ── Why interpolation matters ──
///
/// Goertzel quantizes its target frequency to the nearest discrete
/// bin: at N=2048, sr=48000 the bin width is 23.4 Hz. A scan at 2 Hz
/// step finds the BIN with maximum response, but the actual signal
/// could be anywhere within that bin's bandwidth. Reporting the
/// scan frequency (e.g. "106 Hz") when the true signal is at the
/// bin center (117 Hz) gives the phase-lock loop a wrong target
/// frequency to track — the lock CANNOT converge because anti-
/// noise at 106 Hz doesn't correlate with noise at 117 Hz.
///
/// Fix: parabolic interpolation across three adjacent bins (k-1, k,
/// k+1) gives sub-bin accuracy. For a peak at bin k with magnitudes
/// (m_-1, m_0, m_+1):
///
///   δ = 0.5 · (m_-1 - m_+1) / (m_-1 - 2·m_0 + m_+1)
///
/// where δ ∈ [-0.5, 0.5] is the fractional offset from the bin
/// center. Refined freq = (k + δ) · sr / N.
pub fn estimate_fundamental(samples: &[f32], lo: f32, hi: f32, sr: f32) -> f32 {
    if samples.len() < 256 { return 0.0; }
    let n = samples.len() as f32;
    // Find the BIN with maximum response. Scanning at 2 Hz is more
    // granular than the bin width (~23 Hz at 2048 samples) — we'd
    // hit the same bin multiple times. To avoid that, step by the
    // bin width itself.
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
    // Parabolic interpolation across (k-1, k, k+1).
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

// ─── Test-signal synthesis ─────────────────────────────────────────────

/// Synthesize a vehicle pass-by test signal:
///   - Engine fundamental at `base_hz`, with first two harmonics
///   - Pass-by amplitude envelope (sech²-like rise-peak-fall)
///   - Doppler shift: starts at base_hz × (1 + max_doppler), ends
///     at base_hz × (1 - max_doppler)
///   - `duration_s` total
///   - Plus broadband tire-noise component (uncancellable; here to
///     test that the FSM ignores it)
pub fn synth_passby(
    duration_s: f32,
    base_hz: f32,
    max_doppler: f32,
    sample_rate: f32,
    seed: u32,
) -> Vec<f32> {
    let n = (duration_s * sample_rate) as usize;
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0f32;
    // Simple Lorentzian-ish envelope: 1 / (1 + ((t - peak) / σ)²)
    let peak_t = duration_s * 0.5;
    let sigma = duration_s * 0.18;
    // RNG for the tire noise.
    let mut s = seed.max(1);
    for i in 0..n {
        let t = i as f32 / sample_rate;
        // Doppler: cosine-shaped frequency curve. High at start, low at end.
        let cos_arg = ((t / duration_s).clamp(0.0, 1.0)) * PI;
        let doppler_factor = 1.0 + max_doppler * cos_arg.cos();
        let inst_freq = base_hz * doppler_factor;
        let dphase = 2.0 * PI * inst_freq / sample_rate;
        // Three harmonics, decreasing amplitude.
        let engine = phase.sin() + 0.5 * (2.0 * phase).sin() + 0.3 * (3.0 * phase).sin();
        // Envelope.
        let dt = t - peak_t;
        let env = 1.0 / (1.0 + (dt / sigma).powi(2));
        // Tire noise: simple LFSR-ish broadband.
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        let tire = ((s >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.05;
        out.push((engine / 1.8) * env * 0.5 + tire * env);
        phase += dphase;
        if phase > 2.0 * PI { phase -= 2.0 * PI; }
    }
    out
}

/// A perpendicular passby: a source moving across the listener's
/// line of sight at a distance (e.g. cross-traffic 100m away). The
/// envelope still has a clear peak (closest approach is loudest),
/// but the Doppler curve is much flatter than a parallel passby
/// because the radial velocity changes slowly with the geometry.
///
/// This test signal exercises the FSM's Doppler-gate relaxation:
/// the original gate (>= 0.5 Hz/s) would have REJECTED these as
/// "stationary periodic noise" because the envelope is real but
/// Doppler is too weak. The geometry-aware gate accepts them
/// when the envelope shape matches learned timer patterns.
///
/// `crossing_distance_m`: closest-approach distance in meters.
///   Larger = flatter Doppler curve. Realistic range 10-200 m.
pub fn synth_passby_perpendicular(
    duration_s: f32,
    base_hz: f32,
    velocity_m_s: f32,
    crossing_distance_m: f32,
    sample_rate: f32,
    seed: u32,
) -> Vec<f32> {
    let n = (duration_s * sample_rate) as usize;
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0f32;
    let peak_t = duration_s * 0.5;
    let c = 343.0_f32;  // speed of sound
    let mut s = seed.max(1);
    for i in 0..n {
        let t = i as f32 / sample_rate;
        // Source moves along x-axis; listener at (0, crossing_distance).
        // Source position at time t: x = velocity * (t - peak_t).
        // Distance from source to listener:
        let dx = velocity_m_s * (t - peak_t);
        let dist = (dx * dx + crossing_distance_m * crossing_distance_m).sqrt();
        // Radial velocity (positive = approaching):
        let radial_v = -velocity_m_s * dx / dist;
        // Doppler-shifted frequency:
        let inst_freq = base_hz * c / (c - radial_v);
        let dphase = 2.0 * PI * inst_freq / sample_rate;
        // Three harmonics.
        let engine = phase.sin()
            + 0.5 * (2.0 * phase).sin()
            + 0.3 * (3.0 * phase).sin();
        // 1/r envelope (geometric spreading).
        let env = crossing_distance_m / dist;
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        let tire = ((s >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.05;
        out.push((engine / 1.8) * env * 0.5 + tire * env);
        phase += dphase;
        if phase > 2.0 * PI { phase -= 2.0 * PI; }
    }
    out
}

/// Realistic-engine synth — harmonic profile measured from a real
/// street recording (4-stroke vehicle, mid-pass distance).
///
/// The previous `synth_passby` used a textbook [1.0, 0.5, 0.3]
/// harmonic profile, but real engine recordings reveal a much
/// richer spectrum:
///   - h2 is LOUDER than the fundamental (typical 4-stroke
///     even-cylinder signature with strong even harmonics)
///   - h3 is suppressed (the odd-harmonic dip from the same
///     mechanism)
///   - h4 through h7 are all substantial (40-90% of fundamental
///     amplitude)
///   - h8 falls off rapidly
///
/// Empirically measured normalized amplitudes from the recording:
///   h1=1.00, h2=1.30, h3=0.26, h4=0.88, h5=0.78,
///   h6=0.50, h7=0.79, h8=0.14
///
/// Using this synth instead of the textbook one matters because:
///   1. The "upper band" cancellation experiments (400-1200 Hz)
///      are testing the band where real engines have MORE energy
///      than the textbook synth suggests
///   2. Per-harmonic phase locking has 8 independent phases to
///      track on real engines, vs. 3 on the textbook synth
///   3. Saturation matching has a bigger harmonic target to match
pub fn synth_passby_realistic(
    duration_s: f32,
    base_hz: f32,
    max_doppler: f32,
    sample_rate: f32,
    seed: u32,
) -> Vec<f32> {
    /// Measured harmonic amplitudes from real street recording.
    /// See work/analyze.py output for source data.
    const HARMONIC_AMPS: [f32; 8] = [
        1.00,  // h1 (fundamental)
        1.30,  // h2 (typically LOUDER than fundamental)
        0.26,  // h3 (suppressed odd-harmonic)
        0.88,  // h4
        0.78,  // h5
        0.50,  // h6
        0.79,  // h7
        0.14,  // h8 (rapid falloff)
    ];
    let n = (duration_s * sample_rate) as usize;
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0f32;
    let peak_t = duration_s * 0.5;
    let sigma = duration_s * 0.18;
    let mut s = seed.max(1);
    // Normalize so peak amplitude ≈ 1 (sum of harmonic amps is
    // the max instantaneous value if all peaks aligned).
    let sum_amps: f32 = HARMONIC_AMPS.iter().sum();
    for i in 0..n {
        let t = i as f32 / sample_rate;
        let cos_arg = ((t / duration_s).clamp(0.0, 1.0)) * PI;
        let doppler_factor = 1.0 + max_doppler * cos_arg.cos();
        let inst_freq = base_hz * doppler_factor;
        let dphase = 2.0 * PI * inst_freq / sample_rate;
        // Sum across all measured harmonics with their real
        // empirical amplitudes.
        let mut engine = 0.0_f32;
        for (n_idx, &amp) in HARMONIC_AMPS.iter().enumerate() {
            let n_h = (n_idx + 1) as f32;
            if base_hz * n_h >= sample_rate * 0.45 { break; }
            engine += amp * (n_h * phase).sin();
        }
        let dt = t - peak_t;
        let env = 1.0 / (1.0 + (dt / sigma).powi(2));
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        let tire = ((s >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.05;
        out.push((engine / sum_amps) * env * 0.7 + tire * env);
        phase += dphase;
        if phase > 2.0 * PI { phase -= 2.0 * PI; }
    }
    out
}

/// Same as `synth_passby` but with asymmetric soft-clipping applied
/// to the engine signal — simulates a real engine's pressure
/// asymmetry. The `saturation` parameter (0..0.6) drives the same
/// shape PassbyCanceller uses for its anti-noise output.
///
/// This is the test signal for verifying that the canceller's
/// saturation mixin actually helps when the source is also
/// saturated. Without matching the saturation, the canceller's
/// clean anti-noise leaves the engine's distortion harmonics
/// uncancelled; with matching saturation it should clean those up.
pub fn synth_passby_saturated(
    duration_s: f32,
    base_hz: f32,
    max_doppler: f32,
    saturation: f32,
    sample_rate: f32,
    seed: u32,
) -> Vec<f32> {
    let n = (duration_s * sample_rate) as usize;
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0f32;
    let peak_t = duration_s * 0.5;
    let sigma = duration_s * 0.18;
    let mut s = seed.max(1);
    for i in 0..n {
        let t = i as f32 / sample_rate;
        let cos_arg = ((t / duration_s).clamp(0.0, 1.0)) * PI;
        let doppler_factor = 1.0 + max_doppler * cos_arg.cos();
        let inst_freq = base_hz * doppler_factor;
        let dphase = 2.0 * PI * inst_freq / sample_rate;
        let raw_engine = phase.sin() + 0.5 * (2.0 * phase).sin()
            + 0.3 * (3.0 * phase).sin();
        let normalized = raw_engine / 1.8;
        // Apply the same asymmetric soft-clip the canceller emits.
        let engine = if saturation > 1e-6 {
            let bias = 0.15 * saturation;
            let drive = 1.0 + 2.0 * saturation;
            let driven = normalized * drive + bias;
            let clipped = driven.tanh() / drive;
            let dc_offset = bias.tanh() / drive;
            clipped - dc_offset
        } else { normalized };
        let dt = t - peak_t;
        let env = 1.0 / (1.0 + (dt / sigma).powi(2));
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        let tire = ((s >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.05;
        out.push(engine * env * 0.5 + tire * env);
        phase += dphase;
        if phase > 2.0 * PI { phase -= 2.0 * PI; }
    }
    out
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_energy_distinguishes_in_band_from_silence() {
        let n = 4096;
        // Silence
        let silent = vec![0.0_f32; n];
        let e_silent = band_energy_rms(&silent, BAND_LO_HZ, BAND_HI_HZ, SAMPLE_RATE);
        // In-band tone at 120 Hz, full unit amplitude
        let mut sig = Vec::with_capacity(n);
        for i in 0..n {
            sig.push((2.0 * PI * 120.0 * i as f32 / SAMPLE_RATE).sin());
        }
        let e_sig = band_energy_rms(&sig, BAND_LO_HZ, BAND_HI_HZ, SAMPLE_RATE);
        assert!(e_sig > e_silent * 100.0,
            "in-band signal should dominate silence: sig={}, silent={}",
            e_sig, e_silent);
    }

    #[test]
    fn band_energy_attenuates_out_of_band() {
        let n = 4096;
        // 1000 Hz tone — well above the band.
        let mut sig = Vec::with_capacity(n);
        for i in 0..n {
            sig.push((2.0 * PI * 1000.0 * i as f32 / SAMPLE_RATE).sin());
        }
        let e_out_of_band = band_energy_rms(&sig, BAND_LO_HZ, BAND_HI_HZ, SAMPLE_RATE);
        // 120 Hz tone — well within the band.
        let mut sig2 = Vec::with_capacity(n);
        for i in 0..n {
            sig2.push((2.0 * PI * 120.0 * i as f32 / SAMPLE_RATE).sin());
        }
        let e_in_band = band_energy_rms(&sig2, BAND_LO_HZ, BAND_HI_HZ, SAMPLE_RATE);
        assert!(e_in_band > e_out_of_band * 2.0,
            "in-band {} should beat out-of-band {}", e_in_band, e_out_of_band);
    }

    #[test]
    fn fundamental_estimator_locks_to_actual_tone() {
        let n = 4096;
        for true_freq in [80.0, 120.0, 200.0, 280.0_f32] {
            let mut sig = Vec::with_capacity(n);
            for i in 0..n {
                sig.push((2.0 * PI * true_freq * i as f32 / SAMPLE_RATE).sin());
            }
            let est = estimate_fundamental(&sig, BAND_LO_HZ, BAND_HI_HZ, SAMPLE_RATE);
            // Goertzel bin width at N=4096, sr=48k is ~11.7 Hz, so a
            // 2 Hz scan can quantize to the nearest bin and land
            // 5-12 Hz off the true freq. The FSM doesn't need exact
            // — anti-noise generation tolerates a few-percent
            // mismatch since the harmonics are filtered through the
            // listener's HRTF anyway. Tolerance of 15 Hz is plenty.
            assert!((est - true_freq).abs() < 15.0,
                "expected ~{}, got {}", true_freq, est);
        }
    }

    #[test]
    fn goertzel_defends_against_non_finite() {
        let mut sig = vec![0.5_f32; 1024];
        sig[100] = f32::NAN;
        sig[200] = f32::INFINITY;
        let m = goertzel(&sig, 120.0, SAMPLE_RATE);
        assert!(m.is_finite());
    }

    #[test]
    fn synth_passby_is_finite_and_in_unity() {
        let sig = synth_passby(8.0, 120.0, 0.04, SAMPLE_RATE, 7);
        for &s in &sig {
            assert!(s.is_finite());
            assert!(s.abs() <= 1.5,
                "test signal should be roughly in unity, got {}", s);
        }
        // Peak energy in the middle, near-silence at the ends.
        let n = sig.len();
        let head_rms: f32 = sig.iter().take(n / 10).map(|&x| x * x).sum::<f32>().sqrt();
        let mid_rms: f32 = sig.iter().skip(n * 4 / 10).take(n / 10)
            .map(|&x| x * x).sum::<f32>().sqrt();
        assert!(mid_rms > head_rms * 2.0,
            "passby envelope should peak in the middle; head={}, mid={}",
            head_rms, mid_rms);
    }

    #[test]
    fn fsm_initial_phase_is_quiet() {
        let c = PassbyCanceller::new();
        assert_eq!(c.phase(), PassbyPhase::Quiet);
        assert_eq!(c.timers().events_observed, 0);
    }

    #[test]
    fn fsm_transitions_quiet_to_approach_on_loud_in_band() {
        let mut c = PassbyCanceller::new();
        // Establish a quiet noise floor with a few windows of silence.
        for _ in 0..5 {
            c.process_window(&vec![0.0001_f32; WINDOW_SAMPLES]);
        }
        // Then feed a loud in-band tone.
        let mut sig = Vec::with_capacity(WINDOW_SAMPLES);
        for i in 0..WINDOW_SAMPLES {
            sig.push((2.0 * PI * 150.0 * i as f32 / SAMPLE_RATE).sin() * 0.5);
        }
        let _ = c.process_window(&sig);
        assert_eq!(c.phase(), PassbyPhase::Approach);
    }

    #[test]
    fn fsm_stays_quiet_on_noise_floor_only() {
        let mut c = PassbyCanceller::new();
        for _ in 0..20 {
            // Trickle of background noise; shouldn't trigger.
            let sig: Vec<f32> = (0..WINDOW_SAMPLES)
                .map(|i| (i as f32).sin() * 1e-5)
                .collect();
            c.process_window(&sig);
        }
        assert_eq!(c.phase(), PassbyPhase::Quiet);
    }

    #[test]
    fn anti_noise_amp_rises_only_with_phase() {
        let mut c = PassbyCanceller::new();
        // Quiet — amp should stay near 0.
        for _ in 0..30 {
            c.process_window(&vec![0.0001_f32; WINDOW_SAMPLES]);
        }
        let s_quiet = c.snapshot();
        assert!(s_quiet.anti_amp < 0.01,
            "anti_amp should be ~0 in Quiet, got {}", s_quiet.anti_amp);
    }

    #[test]
    fn anti_output_is_zero_when_quiet() {
        let mut c = PassbyCanceller::new();
        // Make sure we're in Quiet with no built-up amplitude.
        for _ in 0..30 {
            c.process_window(&vec![0.0001_f32; WINDOW_SAMPLES]);
        }
        let out = c.pull_output(512);
        let energy: f32 = out.iter().map(|&x| x * x).sum::<f32>();
        assert!(energy < 1e-3,
            "anti-noise output should be silent in Quiet, energy={}", energy);
    }

    #[test]
    fn anti_output_is_finite_under_random_inputs() {
        let mut c = PassbyCanceller::new();
        // Feed a synthesized pass-by through process_window.
        let sig = synth_passby(8.0, 120.0, 0.04, SAMPLE_RATE, 11);
        let mut i = 0;
        while i + WINDOW_SAMPLES <= sig.len() {
            c.process_window(&sig[i..i + WINDOW_SAMPLES]);
            i += WINDOW_SAMPLES;
        }
        // pull output across the whole event; everything finite.
        let out = c.pull_output(4096);
        for &s in &out {
            assert!(s.is_finite());
            assert!(s.abs() <= 1.0);
        }
    }

    #[test]
    fn timers_start_with_zero_confidence() {
        let t = TransitionTimers::default();
        assert_eq!(t.confidence(), 0.0);
    }

    #[test]
    fn timers_confidence_saturates_with_events() {
        let mut t = TransitionTimers::default();
        for _ in 0..20 { t.observe(3000.0, 1500.0, 5000.0); }
        assert!(t.confidence() >= 0.99);
        // The EWMA settles near the observed values.
        assert!((t.approach_ms_ewma - 3000.0).abs() < 100.0);
        assert!((t.peak_ms_ewma - 1500.0).abs() < 100.0);
        assert!((t.recede_ms_ewma - 5000.0).abs() < 100.0);
    }

    #[test]
    fn reset_timers_clears_state() {
        let mut c = PassbyCanceller::new();
        // Manually inject some observations.
        c.timers.observe(2000.0, 1000.0, 4000.0);
        c.timers.observe(2200.0, 900.0, 4100.0);
        assert!(c.timers.events_observed > 0);
        c.reset_timers();
        assert_eq!(c.timers.events_observed, 0);
    }

    #[test]
    fn doppler_detected_on_rising_frequency() {
        let mut c = PassbyCanceller::new();
        // Inject freq_history manually to test the Doppler computation.
        c.freq_history = vec![160.0, 155.0, 150.0, 145.0, 140.0, 135.0];
        let d = c.recent_doppler_hz_per_s();
        assert!(d > 0.0, "rising freq history should give positive Doppler, got {}", d);
    }

    #[test]
    fn doppler_zero_on_stationary_source() {
        let mut c = PassbyCanceller::new();
        c.freq_history = vec![60.0; 6];
        let d = c.recent_doppler_hz_per_s();
        assert!(d.abs() < 0.1, "stationary source should give ~0 Doppler, got {}", d);
    }

    /// The simplest possible cancellation test: feed a pure tone,
    /// let the canceller's lock figure out the phase, verify that
    /// pulling anti-noise after lock produces a residual MUCH
    /// smaller than the input when summed listener-style
    /// (`mic - anti`).
    ///
    /// This isolates the lock's correctness from the fuzz harness's
    /// many-degrees-of-freedom signal flow.
    #[test]
    fn phase_lock_reduces_residual_on_pure_tone() {
        let mut c = PassbyCanceller::new();
        c.phase = PassbyPhase::Peak;
        c.anti_freq = 200.0;
        // Set BOTH the lock-emitted amp AND the FSM-authorized
        // ceiling — the lock caps its adapted amplitude by the
        // ceiling, and the ceiling defaults to 0 until the
        // envelope runs.
        c.anti_amp = 1.0;
        c.anti_amp_ceiling = 1.0;
        let n = WINDOW_SAMPLES;
        let mic: Vec<f32> = (0..n).map(|i|
            (2.0 * PI * 200.0 * i as f32 / SAMPLE_RATE).sin()
        ).collect();
        c.energy_history = vec![1.0; 6];
        c.freq_history = vec![200.0; 6];
        c.lock_phase_to_noise(&mic);
        let anti = c.pull_output(n);
        let mic_energy: f32 = mic.iter().map(|&x| x * x).sum();
        let residual_energy: f32 = mic.iter().zip(&anti)
            .map(|(&m, &a)| { let r = m - a; r * r })
            .sum();
        let suppression_db = 10.0 * (mic_energy / residual_energy.max(1e-12)).log10();
        eprintln!("DBG: mic_energy={:.3}, residual_energy={:.3}, supp={:.2} dB, \
                   lock_quality={:.3}, anti_amp_after_lock={:.3}",
            mic_energy, residual_energy, suppression_db,
            c.phase_lock_quality, c.anti_amp);
        assert!(c.phase_lock_quality > 0.3,
            "lock should report decent quality on a pure tone, got {}",
            c.phase_lock_quality);
        assert!(suppression_db > 3.0,
            "phase-locked anti should suppress by >3 dB on pure tone, got {} dB",
            suppression_db);
    }

    #[test]
    fn saturation_adds_higher_harmonics_to_output() {
        // Compare the spectrum of clean (saturation=0) vs saturated
        // (saturation=0.3) anti-noise. The saturated version should
        // have measurable energy at higher harmonics (4f, 5f) that
        // the clean version doesn't.
        let mut c_clean = PassbyCanceller::new();
        c_clean.anti_freq = 200.0;
        c_clean.anti_amp = 0.5;
        c_clean.set_saturation(0.0);
        let clean = c_clean.pull_output(4096);

        let mut c_sat = PassbyCanceller::new();
        c_sat.anti_freq = 200.0;
        c_sat.anti_amp = 0.5;
        c_sat.set_saturation(0.3);
        let saturated = c_sat.pull_output(4096);

        // Measure energy at 4f = 800 Hz (above the 3rd harmonic).
        let energy_at = |s: &[f32], f: f32| -> f32 {
            goertzel(s, f, SAMPLE_RATE)
        };
        let h4_clean = energy_at(&clean, 800.0);
        let h4_sat = energy_at(&saturated, 800.0);
        // Saturated should have at least 2× the 4th-harmonic energy.
        assert!(h4_sat > h4_clean * 2.0,
            "saturated should boost 4th harmonic vs clean; clean={}, sat={}",
            h4_clean, h4_sat);
    }

    #[test]
    fn saturation_clamped_to_valid_range() {
        let mut c = PassbyCanceller::new();
        c.set_saturation(-1.0);
        assert_eq!(c.saturation(), 0.0);
        c.set_saturation(99.0);
        assert_eq!(c.saturation(), 0.6);
        c.set_saturation(f32::NAN);
        // NaN.clamp() returns NaN in Rust — our clamp impl might
        // pass NaN through. Verify it doesn't propagate to output.
        // (Defense in pull_output: saturation>1e-6 check + clamps.)
        let out = c.pull_output(64);
        for &v in &out { assert!(v.is_finite()); }
    }

    #[test]
    fn per_harmonic_mode_off_by_default() {
        let c = PassbyCanceller::new();
        assert!(!c.lock_per_harmonic());
        assert!((c.h2_phase_offset() - PI / 6.0).abs() < 1e-5);
        assert!((c.h3_phase_offset() - PI / 3.0).abs() < 1e-5);
    }

    #[test]
    fn per_harmonic_mode_updates_offsets_after_lock() {
        let mut c = PassbyCanceller::new();
        c.set_lock_per_harmonic(true);
        c.phase = PassbyPhase::Peak;
        c.anti_freq = 200.0;
        c.anti_amp = 1.0;
        c.anti_amp_ceiling = 1.0;
        c.energy_history = vec![1.0; 6];
        c.freq_history = vec![200.0; 6];
        let n = WINDOW_SAMPLES;
        let mic: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / SAMPLE_RATE;
            (2.0 * PI * 200.0 * t).sin()
                + 0.5 * (2.0 * PI * 400.0 * t + 0.7).sin()
                + 0.25 * (2.0 * PI * 600.0 * t + 1.4).sin()
        }).collect();
        let h2_before = c.h2_phase_offset();
        let h3_before = c.h3_phase_offset();
        c.lock_phase_to_noise(&mic);
        let h2_after = c.h2_phase_offset();
        let h3_after = c.h3_phase_offset();
        assert!((h2_after - h2_before).abs() > 0.01
            || (h3_after - h3_before).abs() > 0.01,
            "per-harmonic mode should update at least one offset");
        assert!(h2_after.is_finite() && h2_after >= 0.0 && h2_after < 2.0 * PI);
        assert!(h3_after.is_finite() && h3_after >= 0.0 && h3_after < 2.0 * PI);
    }

    #[test]
    fn fixed_mode_keeps_offsets_constant() {
        let mut c = PassbyCanceller::new();
        c.phase = PassbyPhase::Peak;
        c.anti_freq = 200.0;
        c.anti_amp = 1.0;
        c.anti_amp_ceiling = 1.0;
        c.energy_history = vec![1.0; 6];
        c.freq_history = vec![200.0; 6];
        let mic: Vec<f32> = (0..WINDOW_SAMPLES).map(|i| {
            let t = i as f32 / SAMPLE_RATE;
            (2.0 * PI * 200.0 * t + 0.9).sin()
                + 0.5 * (2.0 * PI * 400.0 * t + 2.1).sin()
        }).collect();
        let h2_before = c.h2_phase_offset();
        let h3_before = c.h3_phase_offset();
        c.lock_phase_to_noise(&mic);
        assert_eq!(c.h2_phase_offset(), h2_before);
        assert_eq!(c.h3_phase_offset(), h3_before);
    }

    #[test]
    fn auto_select_switches_to_per_harmonic_when_lock_poor() {
        let mut c = PassbyCanceller::new();
        c.phase = PassbyPhase::Peak;
        c.phase_lock_quality = 0.1;
        c.auto_select_lock_mode();
        assert!(c.lock_per_harmonic());
    }

    #[test]
    fn auto_select_switches_back_when_lock_recovers() {
        let mut c = PassbyCanceller::new();
        c.set_lock_per_harmonic(true);
        c.phase = PassbyPhase::Peak;
        c.phase_lock_quality = 0.8;
        c.auto_select_lock_mode();
        assert!(!c.lock_per_harmonic());
    }

    #[test]
    fn auto_select_stays_in_hysteresis_band() {
        let mut c = PassbyCanceller::new();
        c.phase = PassbyPhase::Peak;
        c.phase_lock_quality = 0.4;
        let before = c.lock_per_harmonic();
        c.auto_select_lock_mode();
        assert_eq!(c.lock_per_harmonic(), before);
    }

    #[test]
    fn auto_select_never_enables_during_quiet() {
        let mut c = PassbyCanceller::new();
        c.phase = PassbyPhase::Quiet;
        c.phase_lock_quality = 0.0;
        c.auto_select_lock_mode();
        assert!(!c.lock_per_harmonic());
    }

    /// Perpendicular passes (distant cross-traffic) have flat
    /// Doppler curves. The original FSM gate would reject these
    /// as stationary noise. The geometry-aware gate should pick
    /// them up if the envelope is clear and timers have learned
    /// passby shapes.
    #[test]
    fn fsm_accepts_perpendicular_passby_with_learned_timers() {
        // Synthesize: 50 km/h car crossing at 80m distance, 8s window.
        // Velocity 13.9 m/s, distance 80m → Doppler change per second
        // is small (~0.1-0.2 Hz/s of the source frequency).
        let signal = synth_passby_perpendicular(
            8.0, 120.0, 13.9, 80.0, SAMPLE_RATE, 42);
        let mut c = PassbyCanceller::new();
        // Pre-seed timers with confidence — simulate that the user
        // has been near this street long enough for the FSM to
        // learn typical passby durations.
        c.timers.observe(2000.0, 600.0, 2000.0);
        c.timers.observe(2200.0, 700.0, 1800.0);
        c.timers.observe(1800.0, 500.0, 2200.0);
        c.timers.observe(2100.0, 650.0, 2100.0);
        c.timers.observe(1900.0, 550.0, 1900.0);
        c.timers.observe(2000.0, 600.0, 2000.0);
        assert!(c.timers.confidence() > 0.6,
            "timer confidence should be high after enough observations");

        let win = WINDOW_SAMPLES;
        let mut reached_peak = false;
        let mut i = 0;
        while i + win <= signal.len() {
            c.process_window(&signal[i..i + win]);
            if c.phase == PassbyPhase::Peak {
                reached_peak = true;
            }
            i += win;
        }
        assert!(reached_peak,
            "FSM should reach Peak for perpendicular passes when \
             timer confidence is high and envelope is strong");
    }

    /// The relaxed Doppler gate is more permissive — a constant
    /// tone (HVAC, fridge) may transition through Peak. In the
    /// "reduce all noise" use case, that's actually fine: any
    /// cancellation we do helps. But the FSM must still TIMEOUT
    /// back to Quiet if the tone is genuinely steady (no envelope
    /// fall) — otherwise we'd burn cycles forever on a constant
    /// tone.
    #[test]
    fn fsm_eventually_returns_to_quiet_on_stationary_tone() {
        // Very long constant tone — needs to outlast PEAK_MAX_MS
        // + RECEDE_MAX_MS to force the timeout cycle.
        let n = (15.0 * SAMPLE_RATE) as usize;
        let signal: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / SAMPLE_RATE;
            (2.0 * PI * 120.0 * t).sin() * 0.3
        }).collect();
        let mut c = PassbyCanceller::new();

        let win = WINDOW_SAMPLES;
        let mut saw_quiet_after_peak = false;
        let mut saw_peak = false;
        let mut i = 0;
        while i + win <= signal.len() {
            c.process_window(&signal[i..i + win]);
            if c.phase == PassbyPhase::Peak { saw_peak = true; }
            if saw_peak && c.phase == PassbyPhase::Quiet {
                saw_quiet_after_peak = true;
                break;
            }
            i += win;
        }
        // Some cancellers might never reach Peak on a perfectly flat
        // signal (no envelope rise from initial). What we REALLY want
        // is that the FSM doesn't get stuck in any phase indefinitely.
        // After 15s of identical signal, we should be in any phase
        // EXCEPT Peak (which has PEAK_MAX_MS bound of ~8s and would
        // have forced-progressed to Recede by now).
        let final_phase = c.phase();
        assert!(final_phase != PassbyPhase::Peak,
            "FSM should not be stuck in Peak after 15s; saw_peak={}, final={:?}",
            saw_peak, final_phase);
    }
}
