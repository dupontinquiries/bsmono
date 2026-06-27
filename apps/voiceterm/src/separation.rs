// src/separation.rs
//
// Basic speaker separation library. v0.1.116 foundations.
//
// Owns the logic that takes incoming audio chunks and answers:
// "who is this, if anyone we know?". Used in two modes:
//
//   1. Multi-user mode (registered speakers > 1): route each chunk
//      to the matching speaker's accumulator. Whisper sees per-speaker
//      audio streams.
//
//   2. Single-user noise rejection: with one registered speaker,
//      chunks that DON'T match the registered profile are tagged as
//      noise/other and rejected before whisper. This is useful for
//      filtering out partner side-channel chatter when the user is
//      the primary speaker.
//
// Design principles:
//
// - **Library, not framework.** Caller owns the audio source and the
//   sink. This module is stateful (it has a separator instance with
//   hysteresis state) but doesn't manage threads or buffers.
//
// - **Hysteresis is mandatory.** TonalProfile per ~500ms chunk is
//   noisy enough that without hysteresis we'd flip speakers on every
//   chunk. The separator's `Decision` enum carries confidence, and
//   we require N consecutive matches above threshold before switching.
//
// - **Incremental feature additions.** Today: TonalProfile match.
//   Future: MFCC-based embeddings, voice-activity-only frames (skip
//   pure silence), explicit noise classifier, learned per-speaker
//   prior weights. Each feature plugs into `classify_chunk` as an
//   additional signal.
//
// - **No allocation in the hot path.** The separator's working
//   buffers are pre-allocated. `classify_chunk` takes `&[f32]` and
//   returns a small enum.
//
// What's NOT here:
//
// - Actual stream splitting (caller's job — separator says "this
//   chunk is speaker X", caller appends to that speaker's buffer)
// - Whisper invocation (whisper_pool handles that)
// - Overlap detection (chunk attributed to single best match;
//   simultaneous speech misroutes — future research)

use crate::phonetic::TonalProfile;

/// Result of classifying a single audio chunk.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Confident match to a registered speaker.
    Speaker { name: String, confidence: f32 },
    /// Audio looks like speech but doesn't match anyone known.
    /// In single-speaker mode this is "rejected as noise"; in
    /// multi-mode it's "unknown speaker, capture anyway for later
    /// registration prompting".
    Unknown { confidence: f32 },
    /// Chunk is below voice-activity threshold (just background).
    Silence,
}

/// Per-session separator state. One instance per active recording.
///
/// Threading: NOT Sync. Caller owns it on a single thread (typically
/// the streaming preview thread, which is the only consumer that
/// needs chunk-level routing right now).
pub struct Separator {
    /// Recent decisions for hysteresis. We require HYSTERESIS_DEPTH
    /// matching decisions in a row before switching active speaker.
    recent: std::collections::VecDeque<Decision>,
    /// Last "confirmed" speaker label after hysteresis filtering.
    /// `None` until enough consistent decisions arrive.
    active: Option<String>,
    /// RMS threshold below which a chunk is treated as silence.
    /// Auto-calibrated from running average; updated on every chunk.
    silence_rms: f32,
    /// Sample count seen by this instance (informational/debug).
    chunks_seen: u64,
}

/// How many consecutive matching decisions are required before the
/// active speaker switches. Higher = more stable but slower to react.
/// 3 chunks at 500ms each = 1.5s of consistent signal needed for
/// a switch.
const HYSTERESIS_DEPTH: usize = 3;

/// Minimum similarity score to count as "this is them". Per-chunk
/// threshold is LOWER than the whole-utterance threshold (0.65)
/// because chunk profiles are noisier. We trade per-chunk recall
/// for less aggressive switching via hysteresis.
const PER_CHUNK_THRESHOLD: f32 = 0.55;

/// Initial silence RMS. Updated adaptively as chunks come in.
const INITIAL_SILENCE_RMS: f32 = 0.005;

impl Separator {
    pub fn new() -> Self {
        Self {
            recent: std::collections::VecDeque::with_capacity(HYSTERESIS_DEPTH + 1),
            active: None,
            silence_rms: INITIAL_SILENCE_RMS,
            chunks_seen: 0,
        }
    }

    /// Classify a 16kHz mono audio chunk. Returns a Decision.
    ///
    /// Side effect: updates internal hysteresis state. Two calls
    /// with identical audio are NOT idempotent — the second call
    /// counts as more evidence for the same speaker.
    ///
    /// Recommended chunk size: 8000 samples (500ms at 16kHz). Smaller
    /// chunks produce unreliable pitch estimates; larger chunks delay
    /// speaker switching.
    pub fn classify_chunk(&mut self, audio_16k: &[f32]) -> Decision {
        self.chunks_seen += 1;

        // Silence check first — RMS too low → not speech.
        let rms = compute_rms(audio_16k);
        if rms < self.silence_rms {
            self.push_decision(Decision::Silence);
            return Decision::Silence;
        }

        // Adapt silence floor: slow EMA of low-rms chunks.
        // Only update when we're confident the chunk IS silence-like.
        if rms < self.silence_rms * 2.0 {
            self.silence_rms = self.silence_rms * 0.9 + rms * 0.1;
        }

        // Build TonalProfile and match against registry.
        let profile = TonalProfile::from_audio(audio_16k);
        let decision = match best_match_with_threshold(&profile, PER_CHUNK_THRESHOLD) {
            Some((name, sim)) => Decision::Speaker { name, confidence: sim },
            None => Decision::Unknown { confidence: 0.0 },
        };

        self.push_decision(decision.clone());
        self.update_active();
        decision
    }

    /// The current "active speaker" after hysteresis. None until
    /// enough consistent chunks arrive. Used by callers to decide
    /// which audio buffer to append a chunk to.
    pub fn active_speaker(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// True if the most recent N decisions are all UNKNOWN (i.e. no
    /// registered speaker matches). Useful for single-speaker noise
    /// rejection: if user is the only registered speaker and recent
    /// chunks don't match, treat as background noise.
    pub fn last_n_unknown(&self, n: usize) -> bool {
        if self.recent.len() < n { return false; }
        self.recent.iter().rev().take(n).all(|d| matches!(d, Decision::Unknown { .. }))
    }

    fn push_decision(&mut self, d: Decision) {
        if self.recent.len() >= HYSTERESIS_DEPTH {
            self.recent.pop_front();
        }
        self.recent.push_back(d);
    }

    fn update_active(&mut self) {
        if self.recent.len() < HYSTERESIS_DEPTH { return; }
        let recent: Vec<Decision> = self.recent.iter().cloned().collect();
        let new = confirm_active(&recent, self.active.as_deref());
        if new.as_deref() != self.active.as_deref() {
            if let Some(ref c) = new {
                crate::clog!("separation", "active speaker → {:?} (after {} chunks)",
                    c, self.chunks_seen);
            }
            self.active = new;
        }
    }
}

/// v0.1.855: decide the confirmed active speaker from the recent decision window.
///
/// Previous rule required ALL `HYSTERESIS_DEPTH` decisions to be the *same*
/// speaker with no Unknown/Silence — so a single noisy chunk (e.g. [Alice,
/// Unknown, Alice]) blocked confirmation even though Alice clearly held the
/// floor. New rule: a speaker is confirmed when they are the ONLY registered
/// speaker seen in the window AND hold a majority of slots. This tolerates the
/// odd noisy/Unknown/Silence chunk while still refusing to switch when two
/// different speakers appear (ambiguous → keep whoever currently has the floor).
/// Once set, the active speaker is never cleared to None — they keep the floor
/// until a different speaker is confirmed (matches the prior "keeps the floor"
/// behavior). Pure (no audio/registry) → directly unit-tested.
fn confirm_active(recent: &[Decision], current: Option<&str>) -> Option<String> {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for d in recent {
        if let Decision::Speaker { name, .. } = d {
            *counts.entry(name.as_str()).or_insert(0) += 1;
        }
    }
    // Exactly one registered speaker present AND holding a majority → confirm.
    if counts.len() == 1 {
        let (&name, &n) = counts.iter().next().unwrap();
        let majority = recent.len().div_ceil(2);
        if n >= majority {
            return Some(name.to_string());
        }
    }
    // Ambiguous (two speakers) or insufficient evidence → keep current floor.
    current.map(|s| s.to_string())
}

impl Default for Separator {
    fn default() -> Self { Self::new() }
}

fn compute_rms(audio: &[f32]) -> f32 { audio_kit::level::rms(audio) }

/// Match profile against registered speakers at the per-chunk threshold.
///
/// v0.1.855: now uses `speakers::identify_at`, which applies OUR threshold
/// directly (instead of identify()'s fixed 0.65 gating first and making our
/// lower 0.55 a no-op). Short chunk profiles are noisier, so the lower threshold
/// recovers a speaker that a strict 0.65 would drop — and identify_at keeps the
/// ambiguity guard, so the lower threshold still never merges two close speakers.
/// Hysteresis (HYSTERESIS_DEPTH) absorbs the extra per-chunk false positives.
fn best_match_with_threshold(profile: &TonalProfile, threshold: f32) -> Option<(String, f32)> {
    crate::speakers::identify_at(profile, threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── compute_rms tests ───

    #[test]
    fn rms_empty_returns_zero() {
        assert_eq!(compute_rms(&[]), 0.0);
    }

    #[test]
    fn rms_constant_signal() {
        // RMS of [0.5, 0.5, 0.5, 0.5] = 0.5
        assert!((compute_rms(&[0.5, 0.5, 0.5, 0.5]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn rms_alternating_signal() {
        // RMS of [-1, 1, -1, 1] = 1.0
        assert!((compute_rms(&[-1.0, 1.0, -1.0, 1.0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rms_silence() {
        assert_eq!(compute_rms(&[0.0; 100]), 0.0);
    }

    #[test]
    fn rms_single_sample() {
        // RMS of single sample = abs(sample).
        assert!((compute_rms(&[0.3]) - 0.3).abs() < 1e-6);
        assert!((compute_rms(&[-0.7]) - 0.7).abs() < 1e-6);
    }

    // ─── Separator state machine tests ───

    #[test]
    fn separator_starts_empty() {
        let s = Separator::new();
        assert_eq!(s.active_speaker(), None);
        assert!(!s.last_n_unknown(1));
        assert!(!s.last_n_unknown(3));
    }

    #[test]
    fn separator_default_matches_new() {
        let a = Separator::new();
        let b = Separator::default();
        // Both start with no active speaker.
        assert_eq!(a.active_speaker(), b.active_speaker());
    }

    #[test]
    fn last_n_unknown_requires_enough_history() {
        let mut s = Separator::new();
        // Manually push decisions through the private API. We push
        // Unknown but not enough.
        s.push_decision(Decision::Unknown { confidence: 0.3 });
        assert!(s.last_n_unknown(1), "1 Unknown should satisfy n=1");
        // n=2 with only 1 decision: not enough history.
        assert!(!s.last_n_unknown(2), "n=2 with 1 decision should fail");
    }

    #[test]
    fn last_n_unknown_mixed_decisions_fails() {
        let mut s = Separator::new();
        s.push_decision(Decision::Speaker { name: "alice".into(), confidence: 0.7 });
        s.push_decision(Decision::Unknown { confidence: 0.3 });
        s.push_decision(Decision::Unknown { confidence: 0.3 });
        // Last 2 are Unknown — OK.
        assert!(s.last_n_unknown(2));
        // Last 3 include Speaker — NOT all Unknown.
        assert!(!s.last_n_unknown(3));
    }

    #[test]
    fn last_n_unknown_silence_breaks_the_streak() {
        let mut s = Separator::new();
        s.push_decision(Decision::Unknown { confidence: 0.3 });
        s.push_decision(Decision::Silence);
        s.push_decision(Decision::Unknown { confidence: 0.3 });
        // Last 3 include Silence — not all Unknown.
        assert!(!s.last_n_unknown(3));
    }

    #[test]
    fn decision_enum_equality() {
        // Sanity: PartialEq derived works for our Decision variants.
        let a = Decision::Silence;
        let b = Decision::Silence;
        assert_eq!(a, b);

        let c = Decision::Unknown { confidence: 0.5 };
        let d = Decision::Unknown { confidence: 0.5 };
        assert_eq!(c, d);

        let e = Decision::Speaker { name: "alice".into(), confidence: 0.8 };
        let f = Decision::Speaker { name: "alice".into(), confidence: 0.8 };
        assert_eq!(e, f);

        // Different names → unequal.
        let g = Decision::Speaker { name: "bob".into(), confidence: 0.8 };
        assert_ne!(e, g);
    }

    // ─── confirm_active (majority floor) tests ───
    fn spk(n: &str) -> Decision { Decision::Speaker { name: n.into(), confidence: 0.7 } }
    fn unk() -> Decision { Decision::Unknown { confidence: 0.0 } }

    #[test]
    fn confirm_clean_run() {
        assert_eq!(confirm_active(&[spk("alice"), spk("alice"), spk("alice")], None),
                   Some("alice".to_string()));
    }

    #[test]
    fn confirm_tolerates_one_noisy_chunk() {
        // [Alice, Unknown, Alice] — the strict old rule blocked this; majority confirms.
        assert_eq!(confirm_active(&[spk("alice"), unk(), spk("alice")], None),
                   Some("alice".to_string()));
        // a single silence in the middle is also fine
        assert_eq!(confirm_active(&[spk("alice"), Decision::Silence, spk("alice")], None),
                   Some("alice".to_string()));
    }

    #[test]
    fn conflicting_speakers_keep_current_floor() {
        // two different speakers in the window → ambiguous → don't switch
        assert_eq!(confirm_active(&[spk("alice"), spk("bob"), spk("alice")], Some("carol")),
                   Some("carol".to_string()));
        // and with no current floor, stays unset
        assert_eq!(confirm_active(&[spk("alice"), spk("bob"), spk("alice")], None), None);
    }

    #[test]
    fn no_majority_keeps_floor() {
        // only one Alice among three → not a majority → keep current
        assert_eq!(confirm_active(&[spk("alice"), unk(), unk()], Some("bob")),
                   Some("bob".to_string()));
        assert_eq!(confirm_active(&[unk(), unk(), unk()], Some("alice")),
                   Some("alice".to_string()));
        // never clears an established floor to None
        assert_eq!(confirm_active(&[Decision::Silence, Decision::Silence, Decision::Silence], Some("alice")),
                   Some("alice".to_string()));
    }

    #[test]
    fn switches_floor_to_new_majority_speaker() {
        assert_eq!(confirm_active(&[spk("bob"), spk("bob"), spk("bob")], Some("alice")),
                   Some("bob".to_string()));
    }
}
