//! snapclone-audio — classical voice-activity detection (VAD) and
//! speech-likelihood scoring for the accessibility "speech boost" feature.
//!
//! No ML, no model files, no FFT library. The whole crate is f32 arithmetic
//! over short PCM frames, designed to run cheaply on the Fairphone 5's A55
//! cores during real-time playback. It mirrors the `snapclone-saliency`
//! crate's design: pure stdlib, a couple of `#[no_mangle]` C FFI entry
//! points, opaque-by-value result structs, and a calibration test suite.
//!
//! ## What it computes
//!
//! Given a frame of mono 16-bit PCM samples (as i16), `speech_likelihood`
//! returns a score in `[0, 1]` blending three classical VAD cues:
//!
//!   1. **Short-time energy** — speech has substantially more energy than
//!      silence. Normalized against a noise-floor estimate the caller
//!      maintains (or a fixed floor for the stateless path).
//!
//!   2. **Zero-crossing rate (ZCR)** — voiced speech (vowels) has LOW ZCR
//!      (dominant low-frequency periodicity), while unvoiced speech
//!      (fricatives /s/ /f/) and broadband noise have HIGH ZCR. Speech as
//!      a whole sits in a characteristic mid band; pure tones and hiss sit
//!      at the extremes. We score the ZCR by how close it is to the
//!      speech-typical range rather than monotonically.
//!
//!   3. **Speech-band energy ratio** — the fraction of total energy that
//!      falls in the speech-intelligibility band (≈300 Hz – 3.4 kHz, the
//!      classic telephone band). Estimated with a cheap second-order
//!      bandpass biquad rather than an FFT. High ratio = energy is
//!      concentrated where speech lives, not in rumble (<300 Hz handling
//!      noise) or hiss (>3.4 kHz).
//!
//! The three cues are combined into a single likelihood. The intended
//! consumer is the playback-side speech boost: when likelihood is high,
//! apply (or ramp up) the mid-band EQ lift; when low, leave audio flat so
//! music / ambience isn't coloured. A v1 caller can also just threshold
//! the likelihood for a binary "speech present" gate.
//!
//! ## Statefulness
//!
//! Two API styles:
//!   - **Stateless** `sc_audio_speech_likelihood(samples, n, sample_rate)`
//!     — scores one frame against fixed internal assumptions. Simplest to
//!     call; good enough for "is this clip mostly speech" batch decisions.
//!   - **Stateful** detector handle (`sc_audio_vad_new` / `_process` /
//!     `_free`) — carries a noise-floor estimate and the biquad filter
//!     state across frames, which makes the energy cue adaptive and the
//!     band filter continuous (no per-frame transient). Preferred for
//!     real-time streaming. Opaque handle, exactly like the house FFI
//!     pattern for stateful Rust objects.

use std::os::raw::c_int;

/// V32.39+: Buzz radio / push-to-talk talk-mode gate (PTT / VOX / continuous).
/// Pure decision layer; consumes the VAD likelihood this crate already computes.
pub mod radio;
pub mod capture;
pub mod wire;
pub mod jitter;
pub mod voicecrypt;
pub mod security;
pub mod obfuscate;
pub mod pqhandshake;

/// Opus encode/decode FFI. Compiled only with `--features opus` (needs libopus).
#[cfg(feature = "opus")]
pub mod codec;

/// Research scaffold: synchronized shared codebook for an index-based voice
/// codec. The transport/sync core (NOT a working low-bitrate codec — needs a
/// synthesis feature extractor + vocoder, the pluggable model hooks).
pub mod codebook;

/// Research scaffold: the prosody / "base tones" channel of a parametric voice
/// codec — pitch (F0) + energy + voicing extraction, plus a bounded-memory
/// predictor so most frames cost only a small residual. Deterministic, no model.
pub mod prosody;

/// Research scaffold: deterministic adaptive model over the codebook index
/// stream — the lossless "prediction" layer. Likely indices / transitions cost
/// fewer bits; both endpoints hold the identical model, so decode is exact.
pub mod indexmodel;

/// LPC content channel + source-filter synthesis — the per-frame spectral
/// envelope (the "what's being said" payload) and a robotic vocoder that makes
/// the whole parametric stack audible. Analysis → coeffs + gain; synthesis →
/// excite an all-pole filter with a pitch impulse train (voiced) or noise.
pub mod lpc;

// ─── Tunable constants ───────────────────────────────────────────────────────

/// Speech intelligibility band edges in Hz (classic telephone band).
const SPEECH_LO_HZ: f32 = 300.0;
const SPEECH_HI_HZ: f32 = 3400.0;

/// ZCR (per-sample crossing fraction) range typical of speech. Below this
/// is dominated by low rumble / hum; above is dominated by hiss / noise.
/// These are crossings-per-sample, i.e. (#sign-changes / #samples).
const ZCR_SPEECH_LO: f32 = 0.02;
const ZCR_SPEECH_HI: f32 = 0.20;

/// Energy floor for the stateless path: RMS below this (on a [-1,1] scale)
/// is treated as silence. -45 dBFS ≈ 0.0056 linear.
const STATELESS_NOISE_FLOOR: f32 = 0.0056;

/// Blend weights (sum to 1.0).
const W_ENERGY: f32 = 0.45;
const W_ZCR: f32 = 0.20;
const W_BAND: f32 = 0.35;

// ─── Core scoring ─────────────────────────────────────────────────────────────

/// Convert i16 PCM to normalized f32 in [-1, 1].
#[inline]
fn norm(s: i16) -> f32 {
    s as f32 / 32768.0
}

/// Root-mean-square of a normalized frame.
fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = frame.iter().map(|x| x * x).sum();
    (sum_sq / frame.len() as f32).sqrt()
}

/// Zero-crossing rate as a fraction of sample count (0..~1).
fn zero_crossing_rate(frame: &[f32]) -> f32 {
    if frame.len() < 2 {
        return 0.0;
    }
    let mut crossings = 0usize;
    for w in frame.windows(2) {
        // Sign change (treat exact-zero as no crossing to avoid
        // double-counting a zero-valued sample between two same-sign ones).
        if (w[0] >= 0.0) != (w[1] >= 0.0) {
            crossings += 1;
        }
    }
    crossings as f32 / frame.len() as f32
}

/// A second-order bandpass biquad (Direct Form I), used to estimate the
/// energy inside the speech band without a full FFT. Coefficients are
/// derived once for the given sample rate using the standard RBJ cookbook
/// bandpass (constant skirt gain) formulas centred on the geometric mean
/// of the band edges with a Q chosen to span 300–3400 Hz.
#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    // state
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn bandpass(sample_rate: f32) -> Self {
        // Centre frequency = geometric mean of the band edges; this places
        // the peak in the middle of the speech band on a log axis.
        let f0 = (SPEECH_LO_HZ * SPEECH_HI_HZ).sqrt();
        // Bandwidth in octaves spanning the edges → Q. BW = log2(hi/lo).
        let bw_oct = (SPEECH_HI_HZ / SPEECH_LO_HZ).log2();
        let w0 = 2.0 * std::f32::consts::PI * f0 / sample_rate;
        let (sin_w0, cos_w0) = (w0.sin(), w0.cos());
        // alpha for a given bandwidth in octaves (RBJ cookbook).
        let alpha = sin_w0
            * (std::f32::consts::LN_2 / 2.0 * bw_oct * w0 / sin_w0).sinh();
        // Constant skirt-gain bandpass (peak gain = Q).
        let b0 = sin_w0 / 2.0;
        let b1 = 0.0;
        let b2 = -sin_w0 / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Map ZCR to a 0..1 "speech-like" score: 1.0 when inside the speech band,
/// falling off linearly outside it. A flat-top trapezoid keeps the middle
/// of the range fully scored rather than peaking at a single point.
fn zcr_score(zcr: f32) -> f32 {
    if zcr >= ZCR_SPEECH_LO && zcr <= ZCR_SPEECH_HI {
        1.0
    } else if zcr < ZCR_SPEECH_LO {
        // Below the band — ramp from 0 at zero to 1 at the low edge.
        (zcr / ZCR_SPEECH_LO).clamp(0.0, 1.0)
    } else {
        // Above the band — ramp down from 1 at the high edge to 0 at 2×.
        (1.0 - (zcr - ZCR_SPEECH_HI) / ZCR_SPEECH_HI).clamp(0.0, 1.0)
    }
}

/// Map an energy ratio (current RMS / noise floor) to 0..1. 1× floor = 0,
/// ~8× floor and above = 1, log-ish in between so a moderate lift over the
/// floor already scores meaningfully (speech is typically 10–30 dB over a
/// quiet floor).
fn energy_score(rms_val: f32, floor: f32) -> f32 {
    if floor <= 0.0 {
        return if rms_val > STATELESS_NOISE_FLOOR { 1.0 } else { 0.0 };
    }
    let ratio = rms_val / floor;
    if ratio <= 1.0 {
        0.0
    } else {
        // log2(ratio)/3 saturates at ratio=8 (2^3). 8× ≈ +18 dB.
        (ratio.log2() / 3.0).clamp(0.0, 1.0)
    }
}

/// Score one frame given a noise floor and a (fresh or carried) biquad.
/// Returns (likelihood, frame_rms) so a stateful caller can update its
/// noise floor with the returned RMS.
fn score_frame(frame: &[f32], floor: f32, bp: &mut Biquad) -> (f32, f32) {
    if frame.is_empty() {
        return (0.0, 0.0);
    }
    let total_rms = rms(frame);
    // Band energy: run the frame through the bandpass and take its RMS.
    let mut band_sum_sq = 0.0f32;
    for &x in frame {
        let y = bp.process(x);
        band_sum_sq += y * y;
    }
    let band_rms = (band_sum_sq / frame.len() as f32).sqrt();
    let band_ratio = if total_rms > 1e-9 {
        (band_rms / total_rms).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let e = energy_score(total_rms, floor);
    let z = zcr_score(zero_crossing_rate(frame));
    let b = band_ratio; // already 0..1

    // If the frame is essentially silent, force likelihood to ~0 regardless
    // of ZCR/band cues (which get noisy at very low energy and would
    // otherwise produce false positives on dead air).
    let likelihood = if e <= 0.0 {
        0.0
    } else {
        (W_ENERGY * e + W_ZCR * z + W_BAND * b).clamp(0.0, 1.0)
    };
    (likelihood, total_rms)
}

// ─── Stateless FFI ────────────────────────────────────────────────────────────

/// Score one frame of mono i16 PCM for speech likelihood, stateless.
///
/// `samples` points to `n` i16 values. `sample_rate` is the PCM rate in Hz
/// (e.g. 48000). Returns a likelihood in [0, 1]; 0.0 on invalid input.
///
/// Stateless means: fixed noise floor, fresh biquad per call (a one-frame
/// filter transient is negligible for frames of ≥10 ms). Good for batch
/// "is this clip speech-heavy" decisions. For real-time streaming prefer
/// the stateful detector so the noise floor adapts and the filter is
/// continuous.
///
/// # Safety
/// `samples` must be valid for reads of `n` i16 values, or null (→ 0.0).
#[no_mangle]
pub unsafe extern "C" fn sc_audio_speech_likelihood(
    samples: *const i16,
    n: usize,
    sample_rate: u32,
) -> f32 {
    if samples.is_null() || n == 0 || sample_rate == 0 {
        return 0.0;
    }
    let slice = std::slice::from_raw_parts(samples, n);
    let frame: Vec<f32> = slice.iter().map(|&s| norm(s)).collect();
    let mut bp = Biquad::bandpass(sample_rate as f32);
    let (likelihood, _) = score_frame(&frame, STATELESS_NOISE_FLOOR, &mut bp);
    likelihood
}

// ─── Stateful detector ──────────────────────────────────────────────────────

/// Streaming VAD with an adaptive noise floor and continuous bandpass state.
pub struct VadDetector {
    sample_rate: f32,
    noise_floor: f32,
    bp: Biquad,
    /// Smoothed likelihood for hysteresis-free ramping by the caller.
    smoothed: f32,
}

impl VadDetector {
    fn new(sample_rate: f32) -> Self {
        VadDetector {
            sample_rate,
            // Start the floor pessimistically low so the first speech frame
            // isn't suppressed; it adapts upward in silence.
            noise_floor: STATELESS_NOISE_FLOOR,
            bp: Biquad::bandpass(sample_rate),
            smoothed: 0.0,
        }
    }

    fn process(&mut self, frame: &[f32]) -> f32 {
        let (likelihood, frame_rms) =
            score_frame(frame, self.noise_floor, &mut self.bp);

        // Adapt the noise floor: when the frame looks like NON-speech
        // (low likelihood), let the floor track its RMS slowly upward/
        // downward. When it looks like speech, hold the floor (don't let
        // loud speech inflate the floor and suppress subsequent speech).
        if likelihood < 0.3 {
            // Asymmetric: rise slowly (so a brief loud noise doesn't spike
            // the floor) and fall a bit faster (so the floor recovers when
            // a noisy passage ends).
            let target = frame_rms.max(1e-6);
            let a = if target > self.noise_floor { 0.05 } else { 0.10 };
            self.noise_floor += a * (target - self.noise_floor);
            // Clamp the floor to a sane minimum so it can't collapse to 0.
            if self.noise_floor < STATELESS_NOISE_FLOOR * 0.25 {
                self.noise_floor = STATELESS_NOISE_FLOOR * 0.25;
            }
        }

        // Smooth the likelihood for a click-free boost ramp on the caller
        // side. One-pole; attack faster than release so speech onsets feel
        // responsive while the boost lingers slightly past the last word.
        let a = if likelihood > self.smoothed { 0.5 } else { 0.15 };
        self.smoothed += a * (likelihood - self.smoothed);
        self.smoothed.clamp(0.0, 1.0)
    }
}

/// Allocate a streaming VAD detector for the given sample rate. Returns an
/// opaque handle; free with `sc_audio_vad_free`. Returns null on invalid
/// sample rate.
///
/// # Safety
/// The returned pointer must be freed exactly once via `sc_audio_vad_free`
/// and must not be used after freeing.
#[no_mangle]
pub extern "C" fn sc_audio_vad_new(sample_rate: u32) -> *mut VadDetector {
    if sample_rate == 0 {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(VadDetector::new(sample_rate as f32)))
}

/// Process one frame of mono i16 PCM through a streaming detector. Returns
/// the smoothed speech likelihood in [0, 1]; 0.0 on invalid input.
///
/// # Safety
/// `handle` must be a live pointer from `sc_audio_vad_new`. `samples` must
/// be valid for reads of `n` i16 values, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_audio_vad_process(
    handle: *mut VadDetector,
    samples: *const i16,
    n: usize,
) -> f32 {
    if handle.is_null() || samples.is_null() || n == 0 {
        return 0.0;
    }
    let det = &mut *handle;
    let slice = std::slice::from_raw_parts(samples, n);
    let frame: Vec<f32> = slice.iter().map(|&s| norm(s)).collect();
    det.process(&frame)
}

/// Free a streaming VAD detector.
///
/// # Safety
/// `handle` must be a pointer from `sc_audio_vad_new` that has not already
/// been freed. Passing null is a safe no-op.
#[no_mangle]
pub unsafe extern "C" fn sc_audio_vad_free(handle: *mut VadDetector) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Return the sample rate a detector was created with (Hz). 0 if the
/// handle is null. Mainly a debugging / sanity aid for the C side to
/// confirm it created the detector at the rate it thinks it did.
///
/// # Safety
/// `handle` must be a live pointer from `sc_audio_vad_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_audio_vad_sample_rate(handle: *const VadDetector) -> u32 {
    if handle.is_null() {
        return 0;
    }
    (*handle).sample_rate as u32
}

/// Version probe so the C side can confirm the crate linked. Returns 1.
#[no_mangle]
pub extern "C" fn sc_audio_abi_version() -> c_int {
    1
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: u32 = 48_000;

    /// Generate a sine tone at `freq` Hz, `n` samples, amplitude `amp`
    /// (0..1), as i16 PCM.
    fn tone(freq: f32, n: usize, amp: f32) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f32 / SR as f32;
                let v = (2.0 * PI * freq * t).sin() * amp;
                (v * 32767.0) as i16
            })
            .collect()
    }

    /// Deterministic white-ish noise via xorshift, amplitude `amp`.
    fn noise(n: usize, amp: f32) -> Vec<i16> {
        let mut state: u32 = 0x1234_5678;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                // map to [-1,1]
                let f = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
                (f * amp * 32767.0) as i16
            })
            .collect()
    }

    #[test]
    fn silence_scores_zero() {
        let frame = vec![0i16; 960]; // 20ms @ 48k
        let s = unsafe { sc_audio_speech_likelihood(frame.as_ptr(), frame.len(), SR) };
        assert!(s < 0.05, "silence scored {s}");
    }

    #[test]
    fn null_and_empty_safe() {
        let s = unsafe { sc_audio_speech_likelihood(std::ptr::null(), 0, SR) };
        assert_eq!(s, 0.0);
        let frame = vec![1i16; 10];
        let s2 = unsafe { sc_audio_speech_likelihood(frame.as_ptr(), frame.len(), 0) };
        assert_eq!(s2, 0.0);
    }

    #[test]
    fn speech_band_tone_scores_higher_than_rumble() {
        // A 1 kHz tone sits squarely in the speech band; a 60 Hz tone is
        // sub-band rumble. Same amplitude — the band-ratio cue should make
        // the in-band tone score higher.
        let speech_band = tone(1000.0, 4800, 0.5); // 100ms
        let rumble = tone(60.0, 4800, 0.5);
        let s_speech =
            unsafe { sc_audio_speech_likelihood(speech_band.as_ptr(), speech_band.len(), SR) };
        let s_rumble =
            unsafe { sc_audio_speech_likelihood(rumble.as_ptr(), rumble.len(), SR) };
        assert!(
            s_speech > s_rumble,
            "in-band {s_speech} should beat rumble {s_rumble}"
        );
    }

    #[test]
    fn speech_band_tone_scores_higher_than_hiss() {
        // High-frequency hiss (broadband noise) has its energy spread out
        // and a very high ZCR; an in-band tone should beat it.
        let speech_band = tone(1500.0, 4800, 0.5);
        let hiss = noise(4800, 0.5);
        let s_speech =
            unsafe { sc_audio_speech_likelihood(speech_band.as_ptr(), speech_band.len(), SR) };
        let s_hiss = unsafe { sc_audio_speech_likelihood(hiss.as_ptr(), hiss.len(), SR) };
        assert!(
            s_speech > s_hiss,
            "in-band {s_speech} should beat hiss {s_hiss}"
        );
    }

    #[test]
    fn loud_in_band_scores_meaningfully() {
        // A loud 800 Hz tone (well above the noise floor, in the speech
        // band) should produce a clearly-positive likelihood.
        let frame = tone(800.0, 4800, 0.6);
        let s = unsafe { sc_audio_speech_likelihood(frame.as_ptr(), frame.len(), SR) };
        assert!(s > 0.4, "loud in-band tone scored {s}, expected > 0.4");
    }

    #[test]
    fn stateful_detector_roundtrip() {
        let h = sc_audio_vad_new(SR);
        assert!(!h.is_null());
        // Feed several speech-band frames; smoothed likelihood should climb.
        let frame = tone(1000.0, 960, 0.5);
        let mut last = 0.0f32;
        for _ in 0..10 {
            last = unsafe { sc_audio_vad_process(h, frame.as_ptr(), frame.len()) };
        }
        assert!(last > 0.3, "sustained speech-band smoothed to {last}");
        // Then feed silence; it should fall.
        let sil = vec![0i16; 960];
        let mut quiet = last;
        for _ in 0..20 {
            quiet = unsafe { sc_audio_vad_process(h, sil.as_ptr(), sil.len()) };
        }
        assert!(quiet < last, "silence {quiet} should fall below speech {last}");
        unsafe { sc_audio_vad_free(h) };
    }

    #[test]
    fn vad_new_rejects_zero_rate() {
        let h = sc_audio_vad_new(0);
        assert!(h.is_null());
    }

    #[test]
    fn abi_version_is_one() {
        assert_eq!(sc_audio_abi_version(), 1);
    }

    // ── V32.37+ characterization tests for the adaptive noise floor ──────────
    // These pin the CURRENT tuned behavior so a future DSP tweak can't silently
    // regress it. They document the design, not just assert it works.

    #[test]
    fn floor_rises_in_sustained_quiet_noise() {
        // Floor adaptation is GATED on likelihood < 0.3 (frames that look like
        // non-speech). A quiet, low-frequency frame scores low likelihood (low
        // energy over the floor + out of the speech band), so the floor adapts
        // UPWARD toward its RMS over time. This documents that the adaptation is
        // intentionally low-likelihood-gated — loud activity (even out-of-band)
        // is treated as signal and HELD, see the companion characterization note
        // below.
        let mut det = VadDetector::new(SR as f32);
        let start_floor = det.noise_floor;
        // quiet 80Hz rumble: low amp so energy_score stays modest, out-of-band
        let rumble = norm_frame(&tone(80.0, 960, 0.02));
        for _ in 0..80 {
            det.process(&rumble);
        }
        // The floor should have MOVED from its initial value (adapted toward the
        // observed RMS), not stayed pinned at the start.
        assert!(det.noise_floor != start_floor,
                "floor should adapt to sustained quiet noise: stayed at {}",
                start_floor);
    }

    #[test]
    fn loud_out_of_band_noise_is_treated_as_activity_not_adapted_away() {
        // CHARACTERIZATION of a known tradeoff: a LOUD out-of-band frame scores
        // high enough (energy dominates) that likelihood stays >= 0.3, so the
        // floor is HELD rather than adapted. This is the documented downside of
        // the energy-weighted score — pinned here so a future tweak that changes
        // it is a conscious decision, not an accident.
        let mut det = VadDetector::new(SR as f32);
        let start_floor = det.noise_floor;
        let loud_rumble = norm_frame(&tone(80.0, 960, 0.5));
        for _ in 0..50 {
            det.process(&loud_rumble);
        }
        // floor held near its start (not driven up by the loud frame's RMS)
        assert!(det.noise_floor < start_floor * 3.0,
                "loud out-of-band frame unexpectedly drove the floor way up: {} -> {}",
                start_floor, det.noise_floor);
    }

    #[test]
    fn floor_held_during_speech_does_not_ratchet_away_speech() {
        // The anti-ratchet guarantee: sustained in-band speech must NOT inflate
        // the floor so much that speech stops registering. After a long speech
        // passage the smoothed likelihood must still be high.
        let mut det = VadDetector::new(SR as f32);
        let speech = norm_frame(&tone(900.0, 960, 0.5)); // in speech band
        let mut last = 0.0;
        for _ in 0..100 {
            last = det.process(&speech);
        }
        assert!(last > 0.3,
                "sustained speech must keep registering (anti-ratchet); got {}", last);
    }

    #[test]
    fn floor_never_collapses_below_clamp() {
        // Feeding pure silence must not drive the floor to 0 (which would make
        // any tiny signal read as huge SNR). It's clamped at 0.25× the stateless
        // floor.
        let mut det = VadDetector::new(SR as f32);
        let sil = vec![0.0f32; 960];
        for _ in 0..200 {
            det.process(&sil);
        }
        assert!(det.noise_floor >= STATELESS_NOISE_FLOOR * 0.25,
                "floor collapsed to {}", det.noise_floor);
    }

    #[test]
    fn smoothed_attack_faster_than_release() {
        // Onset should ramp up faster than offset rings down — the attack/release
        // asymmetry. Measure frames-to-cross-0.3 up vs frames-to-fall-under it.
        let mut det = VadDetector::new(SR as f32);
        let speech = norm_frame(&tone(900.0, 960, 0.6));
        let mut up = 0;
        loop {
            let v = det.process(&speech);
            up += 1;
            if v > 0.3 || up > 200 { break; }
        }
        let mut down = 0;
        let sil = vec![0.0f32; 960];
        loop {
            let v = det.process(&sil);
            down += 1;
            if v < 0.3 || down > 200 { break; }
        }
        // attack should reach 0.3 in fewer frames than release falls below it
        assert!(up <= down,
                "attack ({} frames) should be <= release ({} frames)", up, down);
    }

    // helper: convert an i16 frame to the normalized f32 the detector consumes
    fn norm_frame(s16: &[i16]) -> Vec<f32> {
        s16.iter().map(|&s| norm(s)).collect()
    }
}
