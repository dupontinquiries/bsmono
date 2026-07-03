//! capture.rs — fixed-size frame production for the voice send path.
//!
//! The OS/Qt audio backend delivers captured PCM in whatever buffer sizes it
//! likes, but every downstream stage wants exact, uniform frames: the VAD
//! (`sc_audio_vad_process`) and the Opus encoder both operate on a fixed frame
//! length (e.g. 20 ms = 320 samples at 16 kHz mono). `CaptureFramer` is the
//! re-chunker that sits right after the mic: push it arbitrary-length mono i16
//! PCM, pop exact frames. It is pure buffering — no audio I/O, no codec — so it
//! is fully unit-testable, and it is what produces the frames the already-built
//! transmit gate consumes.
//!
//! Mono only (group voice is mono to keep the bitrate ~4–6 kbps).

use std::collections::VecDeque;
use std::os::raw::c_int;

/// Capture re-framing parameters.
#[derive(Debug, Clone, Copy)]
pub struct CaptureConfig {
    /// Sample rate in Hz (e.g. 16000 or 12000 for Opus wideband/mediumband).
    pub sample_rate: u32,
    /// Frame length in milliseconds (Opus allows 2.5/5/10/20/40/60; 20 is the
    /// VOIP sweet spot).
    pub frame_ms: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { sample_rate: 16_000, frame_ms: 20 }
    }
}

impl CaptureConfig {
    /// Samples in one frame: sample_rate * frame_ms / 1000. Mono, so this is the
    /// sample count and the value count both. Minimum 1.
    pub fn frame_samples(&self) -> usize {
        let n = (self.sample_rate as usize).saturating_mul(self.frame_ms as usize) / 1000;
        n.max(1)
    }
}

/// Accumulates arbitrary-length captured PCM and emits exact-size frames FIFO.
#[derive(Debug)]
pub struct CaptureFramer {
    frame_samples: usize,
    buf: VecDeque<i16>,
    frames_emitted: u64,
}

impl CaptureFramer {
    pub fn new(cfg: &CaptureConfig) -> Self {
        Self {
            frame_samples: cfg.frame_samples(),
            buf: VecDeque::new(),
            frames_emitted: 0,
        }
    }

    /// Samples per emitted frame.
    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    /// Feed newly captured samples (any length, including 0). Cheap — buffers
    /// only; call `pop`/`available` to drain into frames.
    pub fn push(&mut self, samples: &[i16]) {
        self.buf.extend(samples.iter().copied());
    }

    /// How many complete frames can be popped right now.
    pub fn available(&self) -> usize {
        self.buf.len() / self.frame_samples
    }

    /// Pop one frame into `out` (which must hold at least `frame_samples`).
    /// Returns true and writes a full frame, or false if not enough is buffered
    /// (or `out` is too small) — in which case `out` is left untouched.
    pub fn pop(&mut self, out: &mut [i16]) -> bool {
        if self.buf.len() < self.frame_samples || out.len() < self.frame_samples {
            return false;
        }
        for slot in out.iter_mut().take(self.frame_samples) {
            *slot = self.buf.pop_front().expect("checked len above");
        }
        self.frames_emitted += 1;
        true
    }

    /// Total frames emitted over the framer's life (diagnostics / seq seeding).
    pub fn frames_emitted(&self) -> u64 {
        self.frames_emitted
    }

    /// Drop all buffered samples — call when transmit stops so the next talk
    /// burst doesn't start with stale tail audio. Keeps the frame size and the
    /// emitted-count.
    pub fn reset(&mut self) {
        self.buf.clear();
    }

    /// Bound the backlog: if more than `max_frames` worth of samples are queued
    /// (a stalled encoder, say), drop the OLDEST excess so memory and latency
    /// can't grow without limit. Returns the number of samples dropped.
    pub fn cap_backlog(&mut self, max_frames: usize) -> usize {
        let cap = max_frames.saturating_mul(self.frame_samples);
        if self.buf.len() <= cap {
            return 0;
        }
        let drop = self.buf.len() - cap;
        self.buf.drain(0..drop);
        drop
    }
}

/// RMS level of a frame, normalised to [0,1] (for a mic-level meter). Empty → 0.
/// A full-scale square wave reads ~1.0; a full-scale sine reads ~0.707.
pub fn frame_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples
        .iter()
        .map(|&s| {
            let v = s as f64;
            v * v
        })
        .sum();
    let mean = sum_sq / samples.len() as f64;
    ((mean.sqrt() / i16::MAX as f64) as f32).min(1.0)
}

/// Peak absolute level of a frame, normalised to [0,1] (for clip indication).
pub fn frame_peak(samples: &[i16]) -> f32 {
    let mut peak: i32 = 0;
    for &s in samples {
        let a = (s as i32).abs();
        if a > peak {
            peak = a;
        }
    }
    (peak as f32 / i16::MAX as f32).min(1.0)
}

// ─── Mic conditioner: boost + gentle compression ─────────────────────────────
//
// The Fairphone 5's built-in mics capture at a very low level, so raw frames are
// far too quiet for a call. This stage sits right after the framer (before VAD /
// gate / Opus) and does two things, in order:
//   1. a fixed makeup boost (default +9 dB) to lift the quiet signal, then
//   2. a gentle feed-forward compressor (default 1.5:1, 0 ms attack, 100 ms
//      release, soft threshold) so the boost doesn't push transients into
//      clipping and the overall level stays even.
// A final hard clamp to [-1, 1] is a safety brickwall so the boost can never
// wrap the i16 output. All knobs are configurable per device.

#[derive(Clone, Copy, Debug)]
pub struct CompressorConfig {
    pub pre_gain_db:  f32, // makeup boost applied BEFORE compression
    pub threshold_db: f32, // dBFS above which compression engages (post-boost)
    pub ratio:        f32, // compression ratio, e.g. 1.5 → 1.5:1
    pub attack_ms:    f32, // envelope rise time (0 = instant)
    pub release_ms:   f32, // envelope fall time
}

impl Default for CompressorConfig {
    fn default() -> Self {
        // Fairphone 5 defaults from on-device testing.
        Self { pre_gain_db: 9.0, threshold_db: -12.0, ratio: 1.5,
               attack_ms: 0.0, release_ms: 100.0 }
    }
}

#[inline]
fn db_to_lin(db: f32) -> f32 { 10f32.powf(db / 20.0) }

#[inline]
fn time_coeff(ms: f32, sample_rate: f32) -> f32 {
    // One-pole smoothing coefficient for a time constant. 0 ms → 0.0 (instant),
    // larger ms → closer to 1.0 (slower).
    if ms <= 0.0 || sample_rate <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * sample_rate)).exp()
    }
}

/// Stateful mic boost + compressor. One instance per capture stream; `process`
/// runs in place on each popped frame.
pub struct MicConditioner {
    pre_gain:      f32, // linear
    threshold:     f32, // linear amplitude
    ratio:         f32,
    attack_coeff:  f32,
    release_coeff: f32,
    env:           f32, // envelope follower state
}

impl MicConditioner {
    pub fn new(sample_rate: u32, cfg: &CompressorConfig) -> Self {
        let sr = sample_rate.max(1) as f32;
        Self {
            pre_gain:      db_to_lin(cfg.pre_gain_db),
            threshold:     db_to_lin(cfg.threshold_db),
            ratio:         cfg.ratio.max(1.0), // <1 would be an expander; clamp
            attack_coeff:  time_coeff(cfg.attack_ms,  sr),
            release_coeff: time_coeff(cfg.release_ms, sr),
            env:           0.0,
        }
    }

    /// Process one frame of mono i16 PCM in place: boost, compress, clamp.
    pub fn process(&mut self, samples: &mut [i16]) {
        for s in samples.iter_mut() {
            // Normalize to [-1, 1] and apply the makeup boost first.
            let mut x = (*s as f32 / 32_768.0) * self.pre_gain;

            // Envelope follower on the boosted magnitude. 0 ms attack → env
            // jumps instantly on a rise (coeff 0); release smooths the fall.
            let level = x.abs();
            let coeff = if level > self.env { self.attack_coeff } else { self.release_coeff };
            self.env = level + (self.env - level) * coeff;

            // Gain reduction above threshold for a `ratio:1` compressor.
            if self.env > self.threshold && self.env > 0.0 {
                let over_db    = 20.0 * (self.env / self.threshold).log10();
                let reduced_db = over_db * (1.0 - 1.0 / self.ratio);
                x *= db_to_lin(-reduced_db);
            }

            // Safety brickwall so the boost can never wrap the i16 output.
            let y = (x.clamp(-1.0, 1.0) * 32_767.0).round();
            *s = y as i16;
        }
    }

    /// Reset envelope state — call when transmit stops so the next talk burst
    /// doesn't inherit a stale envelope.
    pub fn reset(&mut self) {
        self.env = 0.0;
    }
}

// ─── HarshGuard: perceptual painful-sound limiter ────────────────────────────
//
// Ducks the output when the signal turns "painful" — too loud, too sharp, or
// too harsh — to protect the listener from sudden bangs, feedback screech, and
// sibilant spikes. Fast-acting gain reduction keyed on time-domain soundwave
// characteristics (no FFT), not a fixed limiter:
//   • loudness  — peak level over a threshold (shouting / feedback)
//   • sharpness — crest factor (peak/RMS): clicks, claps, bangs
//   • harshness — high-frequency energy ratio (first difference): sibilance,
//                 screech, whistle/feedback tones
// These combine into a "pain" estimate in [0,1] that drives up to
// max_attenuation_db of ducking, fast attack (catch the onset) / slow release
// (recover smoothly). Apply per frame on the final play-out PCM.
//
// Limitation: no look-ahead (to avoid adding latency), so the very first sample
// of a single-sample click slips through before the gain reacts; it tames
// SUSTAINED painful sounds and the body/tail of transients.

#[derive(Clone, Copy, Debug)]
pub struct HarshGuardConfig {
    pub peak_threshold_db:  f32, // peaks above this contribute to "loud"
    pub crest_threshold:    f32, // peak/RMS above this contributes to "sharp"
    pub hf_ratio_threshold: f32, // HF/total energy above this → "harsh"
    pub max_attenuation_db: f32, // most we'll duck at pain = 1.0 (negative dB)
    pub attack_ms:          f32, // gain-reduction onset (fast)
    pub release_ms:         f32, // recovery (slow)
}

impl Default for HarshGuardConfig {
    fn default() -> Self {
        Self {
            peak_threshold_db:  -3.0,
            crest_threshold:    6.0,
            hf_ratio_threshold: 0.55,
            max_attenuation_db: -12.0,
            attack_ms:          1.0,
            release_ms:         220.0,
        }
    }
}

pub struct HarshGuard {
    peak_thresh:   f32,   // linear
    crest_thresh:  f32,
    hf_thresh:     f32,
    floor_gain:    f32,   // linear gain at pain = 1.0 (e.g. 0.25 for -12 dB)
    attack_coeff:  f32,
    release_coeff: f32,
    peak_env:  f32,       // fast peak follower
    rms_env:   f32,       // slower mean-square follower
    hf_env:    f32,       // high-frequency energy follower
    prev_x:    f32,       // for the first-difference HF measure
    gain:      f32,       // smoothed applied gain
    primed:    bool,      // rms_env seeded from the first energetic frame
}

impl HarshGuard {
    pub fn new(sample_rate: u32, cfg: &HarshGuardConfig) -> Self {
        let sr = sample_rate.max(1) as f32;
        Self {
            peak_thresh:   db_to_lin(cfg.peak_threshold_db),
            crest_thresh:  cfg.crest_threshold.max(1.0),
            hf_thresh:     cfg.hf_ratio_threshold.max(1.0e-3),
            floor_gain:    db_to_lin(cfg.max_attenuation_db.min(0.0)),
            attack_coeff:  time_coeff(cfg.attack_ms,  sr),
            release_coeff: time_coeff(cfg.release_ms, sr),
            peak_env: 0.0, rms_env: 0.0, hf_env: 0.0, prev_x: 0.0,
            gain: 1.0, primed: false,
        }
    }

    pub fn process(&mut self, samples: &mut [i16]) {
        const ENV_FAST: f32 = 0.99;  // peak/HF follower retention (~6 ms hold)
        const ENV_SLOW: f32 = 0.999; // rms follower retention (~60 ms average)
        const QUIET_FLOOR: f32 = 0.01; // ~-40 dBFS: below this nothing is "painful"

        // Seed the RMS follower from the first frame that has real energy, so the
        // crest / HF ratios aren't divided by a near-zero warming-up RMS (which
        // would falsely duck the onset of otherwise-clean audio).
        if !self.primed && !samples.is_empty() {
            let ms: f32 = samples.iter()
                .map(|&s| { let v = s as f32 / 32_768.0; v * v })
                .sum::<f32>() / samples.len() as f32;
            if ms > 1.0e-6 { self.rms_env = ms; self.primed = true; }
        }

        for s in samples.iter_mut() {
            let x = *s as f32 / 32_768.0;
            let ax = x.abs();
            let hf = (x - self.prev_x).abs(); // first-difference = HF content
            self.prev_x = x;

            // detectors: peak/HF jump instantly, decay fast; rms averages slowly
            self.peak_env = ax.max(self.peak_env * ENV_FAST);
            self.hf_env   = hf.max(self.hf_env * ENV_FAST);
            self.rms_env  = (x * x) + (self.rms_env - x * x) * ENV_SLOW;
            let rms = self.rms_env.max(1.0e-9).sqrt();

            // pain components, each 0 until its threshold is crossed; gated off
            // entirely while the signal is quiet (quiet audio is never painful).
            let pain = if self.peak_env < QUIET_FLOOR {
                0.0
            } else {
                let loud  = (self.peak_env / self.peak_thresh - 1.0).max(0.0);
                let sharp = ((self.peak_env / rms) / self.crest_thresh - 1.0).max(0.0);
                let harsh = ((self.hf_env / (rms + 1.0e-6)) / self.hf_thresh - 1.0).max(0.0);
                (1.0 * loud + 0.25 * sharp + 0.4 * harsh).clamp(0.0, 1.0)
            };

            // target gain interpolates 1.0 (no pain) → floor_gain (max pain)
            let target = 1.0 + pain * (self.floor_gain - 1.0);
            let coeff  = if target < self.gain { self.attack_coeff } else { self.release_coeff };
            self.gain  = target + (self.gain - target) * coeff;

            let y = (x * self.gain).clamp(-1.0, 1.0) * 32_767.0;
            *s = y.round() as i16;
        }
    }

    pub fn reset(&mut self) {
        self.peak_env = 0.0; self.rms_env = 0.0; self.hf_env = 0.0;
        self.prev_x = 0.0; self.gain = 1.0; self.primed = false;
    }
}

// ─── Output AGC (play-out loudness normalizer) ───────────────────────────────
//
// The far end's level varies a lot (quiet mics, conservative encoders), and a
// fixed make-up gain can't rescue genuinely quiet speech without clipping the
// loud parts. This adaptive AGC measures the running level and drives it toward
// a target loudness — boosting quiet speech by up to a ceiling (far more than a
// fixed gain) and easing loud speech down to the same target, so every call
// sits at a consistent, comfortable volume. A tanh soft-limiter on the output
// means the boost can never hard-clip. Gain falls fast (attack) when the signal
// gets loud and rises slowly (release) when it's quiet, so it doesn't pump; a
// noise-floor gate keeps it from amplifying hiss or silence up to target.

pub struct OutputAgcConfig {
    pub target_rms:     f32, // desired output RMS, linear (0.25 ≈ -12 dBFS)
    pub max_gain_db:    f32, // ceiling on boost (quiet speech)
    pub min_gain_db:    f32, // floor on gain (loud speech is eased toward target, not crushed)
    pub noise_floor_db: f32, // below this input RMS, hold ~unity (don't amplify silence/hiss)
    pub attack_ms:      f32, // gain-decrease speed when signal exceeds target (fast)
    pub release_ms:     f32, // gain-increase speed when signal below target (slow)
    pub limit_ceiling:  f32, // tanh soft-limit ceiling, linear (< 1.0)
}

impl Default for OutputAgcConfig {
    fn default() -> Self {
        Self {
            target_rms:     0.25,
            max_gain_db:    27.0,
            min_gain_db:    -6.0,
            noise_floor_db: -55.0,
            attack_ms:      10.0,
            release_ms:     300.0,
            limit_ceiling:  0.97,
        }
    }
}

pub struct OutputAgc {
    target_rms:    f32,
    max_gain:      f32, // linear
    min_gain:      f32, // linear
    noise_floor:   f32, // linear RMS
    attack_coeff:  f32,
    release_coeff: f32,
    ceiling:       f32,
    rms_env:       f32, // smoothed mean-square
    gain:          f32, // current smoothed gain (linear)
    primed:        bool,
}

impl OutputAgc {
    pub fn new(sample_rate: u32, cfg: &OutputAgcConfig) -> Self {
        let sr = sample_rate.max(1) as f32;
        Self {
            target_rms:    cfg.target_rms.max(1.0e-4),
            max_gain:      db_to_lin(cfg.max_gain_db.max(0.0)),
            min_gain:      db_to_lin(cfg.min_gain_db.min(0.0)),
            noise_floor:   db_to_lin(cfg.noise_floor_db),
            attack_coeff:  time_coeff(cfg.attack_ms,  sr),
            release_coeff: time_coeff(cfg.release_ms, sr),
            ceiling:       cfg.limit_ceiling.clamp(0.1, 0.999),
            rms_env: 0.0, gain: 1.0, primed: false,
        }
    }

    pub fn process(&mut self, samples: &mut [i16]) {
        const ENV_SLOW: f32 = 0.999; // mean-square follower (~60 ms)

        // Seed the RMS follower from the first energetic frame so the gain
        // doesn't lurch to max while the follower warms up from zero.
        if !self.primed && !samples.is_empty() {
            let ms: f32 = samples.iter()
                .map(|&s| { let v = s as f32 / 32_768.0; v * v })
                .sum::<f32>() / samples.len() as f32;
            if ms > 1.0e-7 { self.rms_env = ms; self.primed = true; }
        }

        for s in samples.iter_mut() {
            let x = *s as f32 / 32_768.0;
            self.rms_env = (x * x) + (self.rms_env - x * x) * ENV_SLOW;
            let rms = self.rms_env.max(1.0e-9).sqrt();

            // Target gain brings rms to target, clamped. Below the noise floor we
            // hold at unity so hiss/silence isn't pulled up to target.
            let target_gain = if rms < self.noise_floor {
                1.0
            } else {
                (self.target_rms / rms).clamp(self.min_gain, self.max_gain)
            };
            let coeff = if target_gain < self.gain { self.attack_coeff } else { self.release_coeff };
            self.gain = target_gain + (self.gain - target_gain) * coeff;

            // Apply, then tanh soft-limit to the ceiling — output ∈ (-ceiling,
            // ceiling), so the boost can never hard-clip.
            let g = x * self.gain;
            let y = self.ceiling * (g / self.ceiling).tanh();
            *s = (y * 32_767.0).round() as i16;
        }
    }

    pub fn reset(&mut self) {
        self.rms_env = 0.0;
        self.gain = 1.0;
        self.primed = false;
    }
}

// ─── FFI ─────────────────────────────────────────────────────────────────────
//
// Opaque-handle, mirroring the VAD / radio-gate FFI in this crate. The C++ mic
// callback pushes whatever buffer Qt hands it, then drains frames in a loop —
// each popped frame goes to the VAD + gate and, when transmitting, to the Opus
// encoder + group seal. `frame_rms` / `frame_peak` are stateless level helpers
// for the UI meter.

/// Create a capture framer. `sample_rate` Hz, `frame_ms` frame length. Never
/// null. Free with `sc_capture_framer_free`.
#[no_mangle]
pub extern "C" fn sc_capture_framer_new(sample_rate: u32, frame_ms: u32) -> *mut CaptureFramer {
    let cfg = CaptureConfig { sample_rate, frame_ms };
    Box::into_raw(Box::new(CaptureFramer::new(&cfg)))
}

/// Free a framer from `sc_capture_framer_new`.
///
/// # Safety
/// `handle` must be from `sc_capture_framer_new` and not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_capture_framer_free(handle: *mut CaptureFramer) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Samples per frame for this framer (so the caller can size its pop buffer).
/// Returns 0 on a null handle.
///
/// # Safety
/// `handle` from `sc_capture_framer_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_capture_framer_frame_samples(handle: *const CaptureFramer) -> c_int {
    if handle.is_null() {
        return 0;
    }
    (*handle).frame_samples() as c_int
}

/// Push `n` captured samples. Null-safe no-op if `handle`/`samples` is null.
///
/// # Safety
/// `samples` must be valid for `n` i16 values; `handle` from `sc_capture_framer_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_capture_framer_push(
    handle: *mut CaptureFramer,
    samples: *const i16,
    n: usize,
) {
    if handle.is_null() || samples.is_null() {
        return;
    }
    let slice = std::slice::from_raw_parts(samples, n);
    (*handle).push(slice);
}

/// Complete frames available to pop right now. 0 on a null handle.
///
/// # Safety
/// `handle` from `sc_capture_framer_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_capture_framer_available(handle: *const CaptureFramer) -> c_int {
    if handle.is_null() {
        return 0;
    }
    (*handle).available() as c_int
}

/// Pop one frame into `out` (capacity `out_cap` samples). Returns 1 if a full
/// frame was written, 0 otherwise (not enough buffered, too-small buffer, or
/// null).
///
/// # Safety
/// `out` must be writable for `out_cap` i16 values; `handle` from
/// `sc_capture_framer_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_capture_framer_pop(
    handle: *mut CaptureFramer,
    out: *mut i16,
    out_cap: usize,
) -> c_int {
    if handle.is_null() || out.is_null() {
        return 0;
    }
    let slice = std::slice::from_raw_parts_mut(out, out_cap);
    if (*handle).pop(slice) {
        1
    } else {
        0
    }
}

/// Drop buffered samples (transmit stopped). Null-safe.
///
/// # Safety
/// `handle` from `sc_capture_framer_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_capture_framer_reset(handle: *mut CaptureFramer) {
    if !handle.is_null() {
        (*handle).reset();
    }
}

/// Bound the backlog to `max_frames`; returns samples dropped. Null-safe (0).
///
/// # Safety
/// `handle` from `sc_capture_framer_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_capture_framer_cap_backlog(
    handle: *mut CaptureFramer,
    max_frames: usize,
) -> c_int {
    if handle.is_null() {
        return 0;
    }
    (*handle).cap_backlog(max_frames) as c_int
}

/// RMS level of `n` samples in [0,1] (mic meter). Stateless. 0 on null/empty.
///
/// # Safety
/// `samples` valid for `n` i16 values, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_audio_frame_rms(samples: *const i16, n: usize) -> f32 {
    if samples.is_null() {
        return 0.0;
    }
    frame_rms(std::slice::from_raw_parts(samples, n))
}

/// Peak absolute level of `n` samples in [0,1] (clip indicator). Stateless.
///
/// # Safety
/// `samples` valid for `n` i16 values, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_audio_frame_peak(samples: *const i16, n: usize) -> f32 {
    if samples.is_null() {
        return 0.0;
    }
    frame_peak(std::slice::from_raw_parts(samples, n))
}

// ── Mic conditioner FFI ──────────────────────────────────────────────────────

/// Create a mic conditioner with explicit parameters. Never null; free with
/// `sc_mic_conditioner_free`.
#[no_mangle]
pub extern "C" fn sc_mic_conditioner_new(
    sample_rate: u32, pre_gain_db: f32, threshold_db: f32,
    ratio: f32, attack_ms: f32, release_ms: f32,
) -> *mut MicConditioner {
    let cfg = CompressorConfig { pre_gain_db, threshold_db, ratio, attack_ms, release_ms };
    Box::into_raw(Box::new(MicConditioner::new(sample_rate, &cfg)))
}

/// Create a mic conditioner with the Fairphone-5 defaults (+9 dB, -12 dBFS,
/// 1.5:1, 0 ms attack, 100 ms release).
#[no_mangle]
pub extern "C" fn sc_mic_conditioner_new_default(sample_rate: u32) -> *mut MicConditioner {
    Box::into_raw(Box::new(MicConditioner::new(sample_rate, &CompressorConfig::default())))
}

/// Process `n` i16 samples in place. Null-safe no-op.
///
/// # Safety
/// `samples` valid for `n` i16 values; `handle` from `sc_mic_conditioner_new*`.
#[no_mangle]
pub unsafe extern "C" fn sc_mic_conditioner_process(
    handle: *mut MicConditioner, samples: *mut i16, n: usize,
) {
    if handle.is_null() || samples.is_null() {
        return;
    }
    let slice = std::slice::from_raw_parts_mut(samples, n);
    (*handle).process(slice);
}

/// Reset the conditioner envelope.
///
/// # Safety
/// `handle` from `sc_mic_conditioner_new*`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_mic_conditioner_reset(handle: *mut MicConditioner) {
    if !handle.is_null() {
        (*handle).reset();
    }
}

/// Free a conditioner from `sc_mic_conditioner_new*`.
///
/// # Safety
/// `handle` must be from `sc_mic_conditioner_new*` and not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_mic_conditioner_free(handle: *mut MicConditioner) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

// ── HarshGuard FFI ───────────────────────────────────────────────────────────

/// Create a HarshGuard with explicit parameters. Never null.
#[no_mangle]
pub extern "C" fn sc_harsh_guard_new(
    sample_rate: u32, peak_threshold_db: f32, crest_threshold: f32,
    hf_ratio_threshold: f32, max_attenuation_db: f32, attack_ms: f32, release_ms: f32,
) -> *mut HarshGuard {
    let cfg = HarshGuardConfig {
        peak_threshold_db, crest_threshold, hf_ratio_threshold,
        max_attenuation_db, attack_ms, release_ms,
    };
    Box::into_raw(Box::new(HarshGuard::new(sample_rate, &cfg)))
}

/// Create a HarshGuard with defaults (-3 dBFS, crest 6, HF 0.55, -12 dB, 1/220 ms).
#[no_mangle]
pub extern "C" fn sc_harsh_guard_new_default(sample_rate: u32) -> *mut HarshGuard {
    Box::into_raw(Box::new(HarshGuard::new(sample_rate, &HarshGuardConfig::default())))
}

/// Process n i16 samples in place (duck painful sounds). Null-safe no-op.
///
/// # Safety
/// `samples` valid for `n` i16 values; `handle` from `sc_harsh_guard_new*`.
#[no_mangle]
pub unsafe extern "C" fn sc_harsh_guard_process(
    handle: *mut HarshGuard, samples: *mut i16, n: usize,
) {
    if handle.is_null() || samples.is_null() {
        return;
    }
    (*handle).process(std::slice::from_raw_parts_mut(samples, n));
}

/// Reset the HarshGuard envelopes/gain.
///
/// # Safety
/// `handle` from `sc_harsh_guard_new*`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_harsh_guard_reset(handle: *mut HarshGuard) {
    if !handle.is_null() {
        (*handle).reset();
    }
}

/// Free a HarshGuard from `sc_harsh_guard_new*`.
///
/// # Safety
/// `handle` must be from `sc_harsh_guard_new*` and not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_harsh_guard_free(handle: *mut HarshGuard) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

// ── Output AGC FFI ───────────────────────────────────────────────────────────

/// Create an output AGC with explicit params. Never null. Free with
/// `sc_output_agc_free`.
#[no_mangle]
pub extern "C" fn sc_output_agc_new(
    sample_rate: u32, target_rms: f32, max_gain_db: f32, min_gain_db: f32,
    noise_floor_db: f32, attack_ms: f32, release_ms: f32, limit_ceiling: f32,
) -> *mut OutputAgc {
    let cfg = OutputAgcConfig {
        target_rms, max_gain_db, min_gain_db, noise_floor_db,
        attack_ms, release_ms, limit_ceiling,
    };
    Box::into_raw(Box::new(OutputAgc::new(sample_rate, &cfg)))
}

/// Create an output AGC with defaults (target -12 dBFS, +27/-6 dB, gate -55 dB,
/// 10/300 ms, ceiling 0.97).
#[no_mangle]
pub extern "C" fn sc_output_agc_new_default(sample_rate: u32) -> *mut OutputAgc {
    Box::into_raw(Box::new(OutputAgc::new(sample_rate, &OutputAgcConfig::default())))
}

/// Normalize n i16 samples in place toward the target loudness. Null-safe no-op.
///
/// # Safety
/// `samples` valid for `n` i16 values; `handle` from `sc_output_agc_new*`.
#[no_mangle]
pub unsafe extern "C" fn sc_output_agc_process(
    handle: *mut OutputAgc, samples: *mut i16, n: usize,
) {
    if handle.is_null() || samples.is_null() {
        return;
    }
    (*handle).process(std::slice::from_raw_parts_mut(samples, n));
}

/// Reset the AGC envelope/gain.
///
/// # Safety
/// `handle` from `sc_output_agc_new*`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_output_agc_reset(handle: *mut OutputAgc) {
    if !handle.is_null() {
        (*handle).reset();
    }
}

/// Free an OutputAgc from `sc_output_agc_new*`.
///
/// # Safety
/// `handle` must be from `sc_output_agc_new*` and not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_output_agc_free(handle: *mut OutputAgc) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_samples_math() {
        assert_eq!(CaptureConfig { sample_rate: 16_000, frame_ms: 20 }.frame_samples(), 320);
        assert_eq!(CaptureConfig { sample_rate: 48_000, frame_ms: 20 }.frame_samples(), 960);
        assert_eq!(CaptureConfig { sample_rate: 12_000, frame_ms: 10 }.frame_samples(), 120);
        // degenerate config never yields a zero frame size
        assert_eq!(CaptureConfig { sample_rate: 8, frame_ms: 1 }.frame_samples(), 1);
    }

    #[test]
    fn accumulates_partial_then_emits() {
        let mut f = CaptureFramer::new(&CaptureConfig { sample_rate: 16_000, frame_ms: 20 });
        assert_eq!(f.frame_samples(), 320);
        f.push(&vec![1i16; 200]);
        assert_eq!(f.available(), 0); // 200 < 320
        let mut out = [0i16; 320];
        assert!(!f.pop(&mut out)); // not enough yet
        f.push(&vec![2i16; 200]); // now 400 buffered
        assert_eq!(f.available(), 1); // one full frame, 80 remainder
        assert!(f.pop(&mut out));
        assert_eq!(out[0], 1);
        assert_eq!(out[319], 2); // frame spans both pushes
        assert_eq!(f.available(), 0);
        assert_eq!(f.frames_emitted(), 1);
    }

    #[test]
    fn drains_multiple_frames_keeps_remainder() {
        let mut f = CaptureFramer::new(&CaptureConfig { sample_rate: 16_000, frame_ms: 20 });
        f.push(&vec![7i16; 320 * 3 + 50]); // 3 frames + 50 leftover
        assert_eq!(f.available(), 3);
        let mut out = [0i16; 320];
        let mut got = 0;
        while f.pop(&mut out) {
            got += 1;
        }
        assert_eq!(got, 3);
        assert_eq!(f.available(), 0);
        assert_eq!(f.frames_emitted(), 3);
    }

    #[test]
    fn pop_rejects_small_buffer() {
        let mut f = CaptureFramer::new(&CaptureConfig::default());
        f.push(&vec![1i16; 320]);
        let mut tiny = [0i16; 100];
        assert!(!f.pop(&mut tiny)); // out too small → no write, frame stays
        assert_eq!(f.available(), 1);
    }

    #[test]
    fn reset_clears_backlog() {
        let mut f = CaptureFramer::new(&CaptureConfig::default());
        f.push(&vec![1i16; 1000]);
        assert!(f.available() > 0);
        f.reset();
        assert_eq!(f.available(), 0);
        // frame size preserved
        assert_eq!(f.frame_samples(), 320);
    }

    #[test]
    fn cap_backlog_drops_oldest() {
        let mut f = CaptureFramer::new(&CaptureConfig::default());
        // mark frame boundaries: 5 frames, value = frame index
        for v in 0..5i16 {
            f.push(&vec![v; 320]);
        }
        assert_eq!(f.available(), 5);
        let dropped = f.cap_backlog(2); // keep newest 2 frames
        assert_eq!(dropped, 320 * 3);
        assert_eq!(f.available(), 2);
        // the surviving frames should be the NEWEST (values 3 then 4)
        let mut out = [0i16; 320];
        assert!(f.pop(&mut out));
        assert_eq!(out[0], 3);
        assert!(f.pop(&mut out));
        assert_eq!(out[0], 4);
    }

    #[test]
    fn rms_and_peak() {
        assert_eq!(frame_rms(&[]), 0.0);
        assert_eq!(frame_rms(&[0i16; 320]), 0.0);
        // full-scale square wave ~ 1.0
        let full = vec![i16::MAX; 320];
        assert!((frame_rms(&full) - 1.0).abs() < 0.01);
        assert!((frame_peak(&full) - 1.0).abs() < 0.01);
        // i16::MIN peak clamps to 1.0 (abs would be 32768 > 32767)
        assert!((frame_peak(&[i16::MIN]) - 1.0).abs() < 0.001);
        // a mid-level constant
        let half = vec![i16::MAX / 2; 320];
        assert!((frame_rms(&half) - 0.5).abs() < 0.01);
    }

    #[test]
    fn ffi_roundtrip() {
        unsafe {
            let h = sc_capture_framer_new(16_000, 20);
            assert!(!h.is_null());
            assert_eq!(sc_capture_framer_frame_samples(h), 320);

            let input = vec![5i16; 700]; // 2 frames + 60
            sc_capture_framer_push(h, input.as_ptr(), input.len());
            assert_eq!(sc_capture_framer_available(h), 2);

            let mut out = [0i16; 320];
            assert_eq!(sc_capture_framer_pop(h, out.as_mut_ptr(), out.len()), 1);
            assert_eq!(out[0], 5);
            assert_eq!(sc_capture_framer_available(h), 1);

            // stateless level helpers
            assert!(sc_audio_frame_rms(out.as_ptr(), out.len()) > 0.0);
            assert!((sc_audio_frame_peak(out.as_ptr(), out.len())
                - (5.0 / i16::MAX as f32)).abs() < 1e-4);

            sc_capture_framer_reset(h);
            assert_eq!(sc_capture_framer_available(h), 0);

            // null-safety
            assert_eq!(sc_capture_framer_pop(std::ptr::null_mut(), out.as_mut_ptr(), out.len()), 0);
            assert_eq!(sc_capture_framer_available(std::ptr::null()), 0);
            assert_eq!(sc_audio_frame_rms(std::ptr::null(), 0), 0.0);

            sc_capture_framer_free(h);
        }
    }

    // ── Mic conditioner ──────────────────────────────────────────────────────

    fn tone(amp_fs: f32, freq: f32, n: usize) -> Vec<i16> {
        (0..n).map(|i| {
            let a = amp_fs * 32767.0;
            (a * (2.0 * std::f32::consts::PI * freq * i as f32 / 16000.0).sin()) as i16
        }).collect()
    }

    #[test]
    fn conditioner_boosts_quiet_signal() {
        // A quiet (~-24 dBFS) tone stays below threshold, so it gets the full
        // +9 dB boost (~2.82×) with no compression.
        let mut c = MicConditioner::new(16_000, &CompressorConfig::default());
        let mut buf = tone(0.063, 200.0, 320);
        let rms_in = frame_rms(&buf);
        c.process(&mut buf);
        let rms_out = frame_rms(&buf);
        assert!(rms_out > rms_in * 2.0,
                "quiet signal should be boosted ~2.8x: {rms_in} -> {rms_out}");
    }

    #[test]
    fn conditioner_clamps_loud_input_no_wrap() {
        // Near-full-scale + 9 dB would overflow i16; the safety clamp must hold
        // (peak <= full-scale, and no sign-wrap to the opposite rail).
        let mut c = MicConditioner::new(16_000, &CompressorConfig::default());
        let mut buf = tone(0.9, 300.0, 320);
        c.process(&mut buf);
        let peak = frame_peak(&buf);
        assert!(peak <= 1.0, "peak must be clamped to full-scale: {peak}");
        assert!(peak > 0.5, "loud signal should remain substantial: {peak}");
    }

    #[test]
    fn conditioner_silence_stays_silent() {
        let mut c = MicConditioner::new(16_000, &CompressorConfig::default());
        let mut buf = vec![0i16; 320];
        c.process(&mut buf);
        assert!(buf.iter().all(|&s| s == 0));
    }

    #[test]
    fn conditioner_zero_attack_is_instant() {
        // 0 ms attack → release/attack coeff for the rise is 0, so the envelope
        // equals the instantaneous level on a rising edge.
        let c = MicConditioner::new(16_000, &CompressorConfig::default());
        assert_eq!(c.attack_coeff, 0.0);
        assert!(c.release_coeff > 0.99 && c.release_coeff < 1.0,
                "100 ms release coeff ~0.999: {}", c.release_coeff);
    }

    #[test]
    fn conditioner_ffi_roundtrip_and_null_safety() {
        unsafe {
            let h = sc_mic_conditioner_new_default(16_000);
            assert!(!h.is_null());
            let mut buf = vec![1000i16; 320];
            sc_mic_conditioner_process(h, buf.as_mut_ptr(), buf.len());
            // +9 dB ≈ 2.82×; 1000 stays below threshold so ~2818.
            assert!(buf[100] > 1800, "boost should raise level: {}", buf[100]);
            sc_mic_conditioner_reset(h);
            // null-safe no-ops
            sc_mic_conditioner_process(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len());
            sc_mic_conditioner_free(std::ptr::null_mut());
            sc_mic_conditioner_free(h);
        }
    }

    // ── HarshGuard ───────────────────────────────────────────────────────────

    #[test]
    fn harshguard_ducks_loud_sustained_tone() {
        // A sustained near-full-scale tone is "painful loud" → ducked.
        let mut g = HarshGuard::new(16_000, &HarshGuardConfig::default());
        let mut buf = tone(0.95, 400.0, 1600); // 100 ms, time to engage
        let rms_in = frame_rms(&buf);
        g.process(&mut buf);
        // measure the tail (after the gain has ramped down)
        let tail = &buf[1200..];
        let rms_tail = frame_rms(tail);
        assert!(rms_tail < rms_in * 0.85,
                "loud tone should be ducked: in {rms_in}, tail {rms_tail}");
    }

    #[test]
    fn harshguard_ducks_harsh_high_frequency() {
        // Nyquist-ish alternating signal = maximum HF content (screech/buzz).
        let mut g = HarshGuard::new(16_000, &HarshGuardConfig::default());
        let mut buf: Vec<i16> = (0..1600)
            .map(|i| if i % 2 == 0 { 12000 } else { -12000 })
            .collect();
        let rms_in = frame_rms(&buf);
        g.process(&mut buf);
        let rms_tail = frame_rms(&buf[1200..]);
        assert!(rms_tail < rms_in * 0.9,
                "harsh HF buzz should be ducked: in {rms_in}, tail {rms_tail}");
    }

    #[test]
    fn harshguard_passes_normal_speech_level() {
        // A moderate tone (~-12 dBFS, below the loudness threshold and low HF)
        // should pass essentially untouched.
        let mut g = HarshGuard::new(16_000, &HarshGuardConfig::default());
        let mut buf = tone(0.25, 220.0, 1600);
        let rms_in = frame_rms(&buf);
        g.process(&mut buf);
        let rms_out = frame_rms(&buf);
        assert!(rms_out > rms_in * 0.95,
                "normal level should pass ~unchanged: in {rms_in}, out {rms_out}");
    }

    #[test]
    fn harshguard_silence_and_reset() {
        let mut g = HarshGuard::new(16_000, &HarshGuardConfig::default());
        let mut buf = vec![0i16; 320];
        g.process(&mut buf);
        assert!(buf.iter().all(|&s| s == 0));
        g.reset();
    }

    #[test]
    fn harshguard_ffi_null_safety() {
        unsafe {
            let h = sc_harsh_guard_new_default(16_000);
            assert!(!h.is_null());
            let mut buf = tone(0.9, 300.0, 320);
            sc_harsh_guard_process(h, buf.as_mut_ptr(), buf.len());
            sc_harsh_guard_reset(h);
            sc_harsh_guard_process(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len());
            sc_harsh_guard_free(std::ptr::null_mut());
            sc_harsh_guard_free(h);
        }
    }

    #[test]
    fn agc_boosts_quiet() {
        let mut a = OutputAgc::new(16_000, &OutputAgcConfig::default());
        let mut buf = tone(0.03, 200.0, 16_000); // ~-33 dBFS, far too quiet
        let rms_in = frame_rms(&buf);
        a.process(&mut buf);
        let rms_out_tail = frame_rms(&buf[12_000..]); // after gain has ramped up
        assert!(rms_out_tail > rms_in * 4.0,
                "quiet speech should be boosted a lot: in {rms_in}, tail {rms_out_tail}");
    }

    #[test]
    fn agc_tames_loud() {
        let mut a = OutputAgc::new(16_000, &OutputAgcConfig::default());
        let mut buf = tone(0.7, 200.0, 16_000); // hot input
        let rms_in = frame_rms(&buf);
        a.process(&mut buf);
        let rms_out_tail = frame_rms(&buf[12_000..]);
        assert!(rms_out_tail < rms_in,
                "loud input should be normalized down: in {rms_in}, tail {rms_out_tail}");
    }

    #[test]
    fn agc_never_clips() {
        let mut a = OutputAgc::new(16_000, &OutputAgcConfig::default());
        let mut buf = tone(0.6, 200.0, 16_000);
        a.process(&mut buf);
        assert!(buf.iter().all(|&s| s.unsigned_abs() < 32_767),
                "tanh limiter must prevent hard clipping");
    }

    #[test]
    fn agc_holds_on_silence() {
        let mut a = OutputAgc::new(16_000, &OutputAgcConfig::default());
        let mut buf = tone(0.0008, 200.0, 16_000); // below the noise-floor gate
        a.process(&mut buf);
        let rms_out = frame_rms(&buf[12_000..]);
        assert!(rms_out < 0.01,
                "near-silence must not be amplified to target: tail {rms_out}");
    }

    #[test]
    fn agc_ffi_null_safety() {
        unsafe {
            let a = sc_output_agc_new_default(16_000);
            assert!(!a.is_null());
            let mut buf = tone(0.05, 200.0, 320);
            sc_output_agc_process(a, buf.as_mut_ptr(), buf.len());
            sc_output_agc_reset(a);
            sc_output_agc_process(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len());
            sc_output_agc_free(std::ptr::null_mut());
            sc_output_agc_free(a);
        }
    }
}
