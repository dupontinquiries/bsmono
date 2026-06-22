//! v0.1.632: PURE wake-gate decision — should we spend a full (~1s)
//! whisper transcription on this audio burst, or skip it as noise?
//!
//! This is the highest-leverage live-detection efficiency point: whisper
//! is the expensive step, and the wake-listen loop was deciding inline
//! (untestable, tangled in the capture loop) whether to run it. The
//! thresholds here are historically hard-won (see the journal references
//! in voice.rs): a burst that's acoustically distant from the registered
//! wake word might still be an "ask ai <question>" phrase, so we can't
//! skip purely on a low wake-match score — but we CAN skip the obvious
//! noise cases cheaply.
//!
//! Extracting it makes the thresholds testable + tunable without touching
//! the audio capture loop, and gives a single place to refine the
//! middle-band policy (the "low acoustic but long burst → run whisper
//! anyway" case that the profile showed firing on ambient chatter).

/// Outcome of the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeGate {
    /// Skip whisper — not worth the ~1s on this burst.
    Skip(SkipReason),
    /// Run whisper (either a likely wake match, or a long burst that may
    /// be an "ask ai" phrase).
    RunWhisper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Short burst + acoustically distant from wake → probably ambient.
    ShortLowAcoustic,
    /// Long burst but acoustic ~0 → broadband noise / music / distant
    /// conversation (the phonetic engine didn't even consider it a
    /// candidate). The hallucination-source case.
    NoiseFloor,
}

/// v0.1.676: SNR-informed gate. Fixes the LONGSTANDING custom-wake bug:
/// the original wake_gate vetoes a LONG burst whenever the acoustic
/// fingerprint score is ~0 (`NoiseFloor`), so Whisper never runs and the
/// user's enrolled TEXT phrase ("hey apple") never gets checked. But a
/// near-zero acoustic on a real utterance is common when the enrolled
/// fingerprint doesn't capture how the user actually says the word in the
/// current environment (different room/mic/enrollment quality) — the acoustic
/// score is a HINT, not ground truth. The text classifier downstream IS the
/// ground truth for a custom phrase, so a long burst must reach Whisper.
///
/// Rule: a long burst runs Whisper UNLESS it's confirmed ambient — i.e. its
/// RMS does NOT rise an SNR margin above the rolling noise floor (broadband
/// noise/music/distant babble that the phonetic engine also didn't flag).
/// That preserves noise rejection (the café case) while no longer vetoing a
/// genuine custom wake phrase purely on a low acoustic fingerprint score.
///
/// `burst_rms` / `noise_floor`: pass 0.0 for noise_floor when no estimate is
/// available → then a long burst always runs Whisper (old "always transcribe
/// long bursts" behavior, which is the SAFE side for not missing wakes).
pub fn wake_gate_v2(
    registered: bool,
    acoustic: f32,
    burst_samples: usize,
    sample_rate: u32,
    burst_rms: f32,
    noise_floor: f32,
) -> WakeGate {
    if !registered {
        return WakeGate::RunWhisper;
    }
    const MATCH_THRESH: f32 = 0.40;
    const SNR_MARGIN: f32 = 1.8; // RMS must rise this × over the floor
    if acoustic >= MATCH_THRESH {
        return WakeGate::RunWhisper;
    }
    let one_second = sample_rate as usize;
    if burst_samples < one_second {
        // short + acoustically distant → still ambient blip
        return WakeGate::Skip(SkipReason::ShortLowAcoustic);
    }
    // LONG burst, acoustically distant. The KEY fix: do NOT veto on acoustic
    // alone. Only skip if it's confirmed ambient by SNR (no rise over floor).
    if noisy_env_skip(burst_rms, noise_floor, SNR_MARGIN) {
        return WakeGate::Skip(SkipReason::NoiseFloor);
    }
    // A real-sounding long utterance — run Whisper so the enrolled TEXT
    // phrase (and the "ask ai" path) get their chance. This is what makes
    // custom wake words fire.
    WakeGate::RunWhisper
}

/// Decide whether to run whisper on a captured burst.
///
/// - `registered`: is a standard wake word registered? (if not, the
///   acoustic score is meaningless and we always transcribe — the only
///   wake path is the "ask ai" phrase).
/// - `acoustic`: wake-word acoustic match score (0..~1+). 0.0 means the
///   phonetic engine found no wake candidate at all (noise).
/// - `burst_samples` / `sample_rate`: burst length, to tell short
///   (likely ambient blips) from long (likely a spoken phrase).
///
/// Thresholds mirror the live logic they replace:
///   - acoustic >= MATCH_THRESH → clearly a wake match → run.
///   - distant (acoustic < MATCH_THRESH):
///       - short burst (< 1s) → Skip(ShortLowAcoustic)
///       - long burst, acoustic < NOISE_EPS → Skip(NoiseFloor)
///       - long burst, NOISE_EPS <= acoustic < MATCH_THRESH → run
///         (possible "ask ai <question>").
pub fn wake_gate(
    registered: bool,
    acoustic: f32,
    burst_samples: usize,
    sample_rate: u32,
) -> WakeGate {
    // No registered wake word → acoustic is 0.0/meaningless; the only way
    // in is an "ask ai" phrase, which we detect via whisper. Always run.
    if !registered {
        return WakeGate::RunWhisper;
    }
    const MATCH_THRESH: f32 = 0.40; // >= this is a confident wake match
    const NOISE_EPS: f32 = 0.005; // < this == phonetic engine found nothing
    if acoustic >= MATCH_THRESH {
        return WakeGate::RunWhisper;
    }
    // acoustically distant from the registered wake.
    let one_second = sample_rate as usize; // samples in 1s of mono audio
    if burst_samples < one_second {
        return WakeGate::Skip(SkipReason::ShortLowAcoustic);
    }
    if acoustic < NOISE_EPS {
        return WakeGate::Skip(SkipReason::NoiseFloor);
    }
    // long burst, low-but-nonzero acoustic: could be "ask ai <question>".
    WakeGate::RunWhisper
}

/// v0.1.675: noisy-environment refinement (café problem). In sustained
/// loud ambient noise, long bursts repeatedly clear the basic gate and waste
/// Whisper on non-speech (the logged `rms=0.0455 thr=0.0053` → `whisper said
/// ""` pattern). This adds an SNR-margin test: a burst is plausibly speech
/// only if its RMS rises a MARGIN above the rolling noise floor. When the
/// floor itself is high (loud room) the absolute RMS isn't enough — we need
/// a RELATIVE rise. Pure + testable; the caller (live wake path) can apply it
/// as an extra skip for long low-acoustic bursts before spending Whisper.
///
/// Returns true if the burst should be SKIPPED as ambient (no clear rise over
/// the floor). `snr_margin` ~1.6–2.0 works in practice (mirrors the
/// audio_profile speech_threshold = noise_floor * 1.8 idiom).
pub fn noisy_env_skip(burst_rms: f32, noise_floor: f32, snr_margin: f32) -> bool {
    if noise_floor <= 0.0 { return false; } // no floor estimate → don't skip
    burst_rms < noise_floor * snr_margin
}

#[cfg(test)]
mod tests {
    use super::*;
    const SR: u32 = 16000;

    #[test]
    fn v2_long_burst_with_low_acoustic_still_runs_whisper() {
        // THE BUG FIX: registered custom wake, long burst, acoustic ~0
        // (fingerprint doesn't match how the user says it), but the RMS
        // rises clearly over the floor (a real utterance) → must RUN whisper
        // so the enrolled TEXT phrase gets checked. Old wake_gate would Skip.
        let long = 2 * SR as usize; // 2s
        assert_eq!(
            wake_gate_v2(true, 0.0, long, SR, /*rms*/0.09, /*floor*/0.03),
            WakeGate::RunWhisper,
            "a real long utterance must reach the text classifier even at acoustic 0"
        );
        // contrast: the OLD gate vetoes the same burst (documents the bug)
        assert_eq!(wake_gate(true, 0.0, long, SR),
                   WakeGate::Skip(SkipReason::NoiseFloor));
    }

    #[test]
    fn v2_still_rejects_confirmed_ambient_noise() {
        // café babble: long, acoustic ~0, RMS does NOT rise over the floor
        let long = 2 * SR as usize;
        assert_eq!(
            wake_gate_v2(true, 0.0, long, SR, /*rms*/0.04, /*floor*/0.03),
            WakeGate::Skip(SkipReason::NoiseFloor),
            "ambient with no SNR rise is still skipped"
        );
    }

    #[test]
    fn v2_no_floor_estimate_runs_long_bursts() {
        // no noise-floor estimate → don't veto; safe side is to transcribe
        let long = 2 * SR as usize;
        assert_eq!(wake_gate_v2(true, 0.0, long, SR, 0.05, 0.0),
                   WakeGate::RunWhisper);
    }

    #[test]
    fn v2_short_low_acoustic_still_skipped() {
        assert_eq!(wake_gate_v2(true, 0.0, 100, SR, 0.2, 0.03),
                   WakeGate::Skip(SkipReason::ShortLowAcoustic));
    }

    #[test]
    fn v2_high_acoustic_always_runs() {
        assert_eq!(wake_gate_v2(true, 0.9, 100, SR, 0.01, 0.03),
                   WakeGate::RunWhisper);
    }

    #[test]
    fn noisy_env_skips_ambient_but_passes_real_rise() {
        // café: floor 0.030, ambient burst 0.045 → only 1.5× → below 1.8 margin → skip
        assert!(noisy_env_skip(0.045, 0.030, 1.8));
        // a real spoken phrase rises well above the floor → not skipped
        assert!(!noisy_env_skip(0.090, 0.030, 1.8));
        // quiet room, no floor estimate → never skip on this basis
        assert!(!noisy_env_skip(0.045, 0.0, 1.8));
        // exactly at margin → not skipped (>= passes)
        assert!(!noisy_env_skip(0.054, 0.030, 1.8));
    }

    #[test]
    fn unregistered_always_runs() {
        // no wake word → only the ask-ai path exists → always transcribe
        assert_eq!(wake_gate(false, 0.0, 100, SR), WakeGate::RunWhisper);
        assert_eq!(wake_gate(false, 0.0, 99_999, SR), WakeGate::RunWhisper);
    }

    #[test]
    fn confident_match_runs() {
        assert_eq!(wake_gate(true, 0.55, 5000, SR), WakeGate::RunWhisper);
        assert_eq!(wake_gate(true, 0.40, 100, SR), WakeGate::RunWhisper); // exactly threshold
    }

    #[test]
    fn short_low_acoustic_skips() {
        // < 1s + distant → ambient blip
        assert_eq!(
            wake_gate(true, 0.20, SR as usize / 2, SR),
            WakeGate::Skip(SkipReason::ShortLowAcoustic)
        );
    }

    #[test]
    fn long_noise_floor_skips() {
        // long burst, acoustic ~0 → noise (the hallucination source)
        assert_eq!(
            wake_gate(true, 0.0, SR as usize * 3, SR),
            WakeGate::Skip(SkipReason::NoiseFloor)
        );
        assert_eq!(
            wake_gate(true, 0.004, SR as usize * 3, SR),
            WakeGate::Skip(SkipReason::NoiseFloor)
        );
    }

    #[test]
    fn long_low_but_nonzero_runs_for_ask_ai() {
        // the "running whisper anyway" case from the logs (acoustic 0.227)
        assert_eq!(
            wake_gate(true, 0.227, SR as usize * 3, SR),
            WakeGate::RunWhisper
        );
    }

    #[test]
    fn boundary_exactly_one_second_is_long() {
        // exactly 1s is NOT "< 1s" → treated as long; with noise-floor
        // acoustic it should skip as NoiseFloor, not ShortLowAcoustic.
        assert_eq!(
            wake_gate(true, 0.0, SR as usize, SR),
            WakeGate::Skip(SkipReason::NoiseFloor)
        );
    }

    #[test]
    fn just_under_one_second_low_acoustic_is_short() {
        assert_eq!(
            wake_gate(true, 0.10, SR as usize - 1, SR),
            WakeGate::Skip(SkipReason::ShortLowAcoustic)
        );
    }
}
