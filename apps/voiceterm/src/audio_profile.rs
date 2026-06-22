// src/audio_profile.rs
//
// Rolling speaker + noise profiles built from background audio captures.
// Free byproduct of the bg-listen loop: every sample we already process
// gets statistical features extracted, and we maintain decaying averages
// over recent N samples for both:
//
//   - SPEAKER profile: derived from samples that whisper transcribed
//     successfully and matched intent/history. These are confirmed-
//     speech-from-the-primary-user moments. Average TonalProfile over
//     last 30 such samples gives us a reasonable model of the user's
//     pitch range, typical speaking energy, and rhythm.
//
//   - NOISE profile: derived from samples where bg-listen rejected the
//     audio (low RMS, or transcription failed sanity checks). Average
//     RMS over these gives us an actual noise floor instead of the
//     hardcoded 0.008 threshold currently used by VAD.
//
// Use cases for the profiles:
//
//   1. Adaptive VAD threshold: speech-RMS = max(noise_floor * 1.8,
//      hardcoded_minimum). Quiet office gets sensitive VAD, noisy cafe
//      gets a higher floor. Replaces the currently hand-tuned 0.008.
//
//   2. Speaker-match scoring: when a fingerprint matches a stored DB
//      entry, weight the score by how well the current audio matches
//      the speaker profile. Same-speaker = boost; very-different-pitch
//      = demote (probably someone else in the room).
//
//   3. Noise-presence detection: if current frame's RMS minus rolling
//      noise floor is small, treat as silence even if absolute RMS
//      is high (reverberant room with constant HVAC). Reduces false
//      VAD triggers.
//
// Persistence: profiles are saved to disk (~/.config/web2qml/audio_profile.bin)
// so they survive across sessions. Loaded at startup; the rolling averages
// pre-populate from the saved values rather than starting cold.
//
// Privacy: the profiles are derived statistical features, NOT raw audio.
// You can't reconstruct what the user said from a TonalProfile (it's
// pitch + rms + activity, three floats). Same for noise — it's just a
// running RMS average. Safe to persist, safe to log.

use std::sync::{Mutex, OnceLock};
use std::path::PathBuf;
use crate::phonetic::TonalProfile;

/// Maximum samples to retain in rolling averages. Older entries get
/// dropped when the buffer fills. 30 samples × ~3min between captures
/// = ~90min of usage history, which is enough to characterize a session
/// without letting old long-ago profiles bias current decisions if the
/// user moves to a new environment.
const MAX_SAMPLES: usize = 30;

/// State of the rolling profile system. Held in a OnceLock<Mutex<>>
/// because the bg-listen worker thread updates it from off the QML
/// thread, while QML/main can read it when scoring.
struct ProfileState {
    /// Recent confirmed-speech profiles. Newest at the back.
    speaker_samples: Vec<TonalProfile>,
    /// Recent noise-floor RMS values from rejected-audio captures.
    /// We don't keep full TonalProfiles for noise — RMS is the only
    /// feature that's reliable on non-speech (zero-crossing on noise
    /// produces meaningless pitch estimates).
    noise_rms: Vec<f32>,
    /// Cached aggregate speaker profile, recomputed when speaker_samples
    /// changes. None means "not enough samples yet".
    cached_speaker: Option<TonalProfile>,
    /// Cached noise floor (mean of recent noise_rms). 0 = no samples.
    cached_noise_floor: f32,
    /// Wake-word reference profile. Captured at enrollment time and
    /// updated each time a wake word fires successfully (rolling
    /// average over the most-recent N firings). Used as the pitch
    /// hint for the COMMAND that follows the wake word — gives us
    /// a confident speaker pitch even if the command itself is short
    /// and ZCR-derived pitch is unreliable.
    ///
    /// This is OUR speaker's profile by definition: if you said the
    /// wake word, you're the registered user. Multi-user support
    /// would extend this to a Vec<(name, TonalProfile)>.
    wake_word_profile: Option<TonalProfile>,
    /// Recent wake-word firings — used to update wake_word_profile
    /// as a rolling average. We don't keep all of history; just enough
    /// samples for a stable estimate.
    wake_word_samples: Vec<TonalProfile>,
}

impl ProfileState {
    fn new() -> Self {
        Self {
            speaker_samples: Vec::new(),
            noise_rms: Vec::new(),
            cached_speaker: None,
            cached_noise_floor: 0.0,
            wake_word_profile: None,
            wake_word_samples: Vec::new(),
        }
    }
}

static STATE: OnceLock<Mutex<ProfileState>> = OnceLock::new();

fn state() -> &'static Mutex<ProfileState> {
    STATE.get_or_init(|| {
        let mut s = ProfileState::new();
        // Try to load any persisted profile from disk so first-session
        // measurements aren't cold.
        if let Some((spk, noise, wakes)) = load_from_disk() {
            for p in spk { s.speaker_samples.push(p); }
            for r in noise { s.noise_rms.push(r); }
            for w in wakes { s.wake_word_samples.push(w); }
            // Initialize wake_word_profile from loaded wake_word_samples
            // so cold-start sessions can use the saved profile right away.
            let n = s.wake_word_samples.len() as f32;
            if n > 0.0 {
                let pitch = s.wake_word_samples.iter().map(|p| p.pitch_hz).sum::<f32>() / n;
                let rms = s.wake_word_samples.iter().map(|p| p.rms).sum::<f32>() / n;
                let act = s.wake_word_samples.iter().map(|p| p.activity).sum::<f32>() / n;
                s.wake_word_profile = Some(TonalProfile {
                    pitch_hz: pitch, rms, activity: act,
                    ..Default::default()
                });
            }
            recompute_caches(&mut s);
        }
        Mutex::new(s)
    })
}

/// Add a confirmed-speech sample. Called by the bg-listen drain path
/// when an audio capture matched a known intent or history command.
/// The audio is the 16kHz mono buffer (same format as fingerprint).
pub fn record_speech_sample(audio_16k_mono: &[f32]) {
    let profile = TonalProfile::from_audio(audio_16k_mono);
    // Skip if pitch wasn't determinable — usually means the sample
    // was too short or all-silent. Don't pollute the speaker profile
    // with garbage.
    if profile.pitch_hz <= 0.0 || profile.rms <= 0.0 { return; }
    if let Ok(mut s) = state().lock() {
        s.speaker_samples.push(profile);
        while s.speaker_samples.len() > MAX_SAMPLES {
            s.speaker_samples.remove(0);
        }
        recompute_caches(&mut s);
        // Persist after every meaningful update — small cost, big
        // robustness benefit (if app crashes we don't lose the day's
        // learning).
        let _ = save_to_disk_locked(&s);
    }
}

/// Add a noise floor sample. Called by bg-listen when audio was
/// captured but determined to be silence/noise (low RMS, or
/// transcription failed sanity). The audio is the same 16kHz buffer.
/// We compute RMS only, since pitch on noise is meaningless.
pub fn record_noise_sample(audio_16k_mono: &[f32]) {
    if audio_16k_mono.is_empty() { return; }
    let mut sum_sq = 0.0f32;
    for &s in audio_16k_mono { sum_sq += s * s; }
    let rms = (sum_sq / audio_16k_mono.len() as f32).sqrt();
    if rms <= 0.0 { return; }
    if let Ok(mut s) = state().lock() {
        s.noise_rms.push(rms);
        while s.noise_rms.len() > MAX_SAMPLES {
            s.noise_rms.remove(0);
        }
        recompute_caches(&mut s);
        let _ = save_to_disk_locked(&s);
    }
}

/// The current best-effort speaker profile. None until we have at least
/// 3 samples — below that the average is too noisy to trust. Use this
/// to score whether an utterance matches the primary user.
pub fn speaker_profile() -> Option<TonalProfile> {
    state().lock().ok().and_then(|s| s.cached_speaker.clone())
}

/// Wake-word reference profile. Returns the rolling-average TonalProfile
/// from recent successful wake-word firings, or None if no wake word
/// has been enrolled / detected yet.
///
/// Use case: when the user has just said "Yo Big K", capturing the
/// wake-word audio gives us a SPEAKER PROFILE with a known label
/// (the wake word). The command that follows likely comes from the
/// same speaker. Pass this profile as a pitch hint to phonetic
/// matching to pre-filter the fingerprint DB.
///
/// Why this is better than speaker_profile() for the immediate-
/// next-command case: speaker_profile() averages over many samples
/// across time and contexts. wake_word_profile() is grounded in the
/// most recent confirmed-self-utterance, so it captures the user's
/// CURRENT pitch (which can drift across the day with mood/fatigue).
pub fn wake_word_profile() -> Option<TonalProfile> {
    state().lock().ok().and_then(|s| s.wake_word_profile.clone())
}

/// Best available pitch hint for the next utterance. Order of
/// preference:
///   1. wake_word_profile (most recent + most context-relevant)
///   2. cached_speaker (long-running average)
///   3. None (no profile data yet — phonetic falls back to scanning all)
///
/// Used by phonetic::top_matches_with_hint to seed the pitch-bin
/// prefilter when the audio's own pitch is hard to determine
/// (e.g. very short utterances, or commands like "yes"/"no" where
/// ZCR doesn't have enough cycles to estimate).
pub fn pitch_hint() -> Option<TonalProfile> {
    if let Ok(s) = state().lock() {
        s.wake_word_profile.clone().or_else(|| s.cached_speaker.clone())
    } else { None }
}

/// Record a wake-word firing. The audio is the captured "Yo Big K"
/// or whatever phrase triggered. We extract its TonalProfile and
/// fold it into the rolling wake_word_profile via averaging.
///
/// If this is the very first wake-word capture (cold start), the
/// profile becomes that single sample's profile. After 3+ samples
/// it stabilizes via averaging.
///
/// Capacity: the wake_word_samples buffer keeps at most 10 entries
/// — enough for a stable average without slow drift away from any
/// recent change in speaking style.
pub fn record_wake_word(audio_16k_mono: &[f32]) {
    let profile = TonalProfile::from_audio(audio_16k_mono);
    if profile.pitch_hz <= 0.0 {
        // Pitch couldn't be determined — wake-word audio was too
        // short, too quiet, or too noisy. Don't pollute the profile.
        // We still successfully detected the wake word (caller
        // already decided that), we just can't extract a useful
        // pitch from this particular instance.
        crate::clog!("audio_profile", "wake-word fired but pitch indeterminate, not updating profile");
        return;
    }
    if let Ok(mut s) = state().lock() {
        s.wake_word_samples.push(profile);
        const MAX_WAKE_SAMPLES: usize = 10;
        while s.wake_word_samples.len() > MAX_WAKE_SAMPLES {
            s.wake_word_samples.remove(0);
        }
        // Recompute wake_word_profile as the average of recent samples.
        let n = s.wake_word_samples.len() as f32;
        if n > 0.0 {
            let pitch = s.wake_word_samples.iter().map(|p| p.pitch_hz).sum::<f32>() / n;
            let rms = s.wake_word_samples.iter().map(|p| p.rms).sum::<f32>() / n;
            let act = s.wake_word_samples.iter().map(|p| p.activity).sum::<f32>() / n;
            s.wake_word_profile = Some(TonalProfile {
                pitch_hz: pitch, rms, activity: act,
                ..Default::default()
            });
            crate::clog!("audio_profile", "wake-word profile updated: pitch~{:.0}Hz from {} samples",
                pitch, s.wake_word_samples.len());
        }
        // Wake-word firings are also speech samples — feed the main
        // speaker profile too. Highest-quality signal: confirmed
        // wake by the registered user.
        s.speaker_samples.push(profile);
        while s.speaker_samples.len() > MAX_SAMPLES {
            s.speaker_samples.remove(0);
        }
        recompute_caches(&mut s);
        let _ = save_to_disk_locked(&s);
    }
}

/// Current noise floor RMS. 0 if we don't have enough samples to
/// estimate. Use as the basis for adaptive VAD threshold:
///   speech_threshold = max(noise_floor() * 1.8, hardcoded_min)
pub fn noise_floor() -> f32 {
    state().lock().ok().map(|s| s.cached_noise_floor).unwrap_or(0.0)
}

/// Score 0..1 of how likely this audio is from the primary speaker.
/// 0.5 means "no opinion" (no profile yet, or no pitch detected).
/// > 0.7 = likely same speaker. < 0.3 = likely different speaker.
///
/// Use case: foreground voice flow can call this on the captured
/// audio. If significantly < 0.5, surface a "wasn't sure that was
/// you" affordance OR demote to lower-confidence candidate
/// presentation. Don't reject outright — false negatives on yourself
/// would feel terrible.
pub fn speaker_match_score(audio_16k_mono: &[f32]) -> f32 {
    let speaker = match speaker_profile() {
        Some(p) => p,
        None => return 0.5, // no opinion
    };
    let cur = TonalProfile::from_audio(audio_16k_mono);
    if cur.pitch_hz <= 0.0 { return 0.5; }
    speaker.similarity(&cur)
}

/// Stats string for debug HUD / logging.
/// Format: "spk_n=NN spk_pitch=NNN noise=N.NNNN noise_n=NN
///          wake_n=NN wake_pitch=NNN"
pub fn stats_string() -> String {
    if let Ok(s) = state().lock() {
        let spk_n = s.speaker_samples.len();
        let pitch = s.cached_speaker.as_ref().map(|p| p.pitch_hz).unwrap_or(0.0);
        let nf = s.cached_noise_floor;
        let wake_n = s.wake_word_samples.len();
        let wake_pitch = s.wake_word_profile.as_ref().map(|p| p.pitch_hz).unwrap_or(0.0);
        format!("spk_n={} spk_pitch={:.0} noise={:.4} noise_n={} wake_n={} wake_pitch={:.0}",
            spk_n, pitch, nf, s.noise_rms.len(), wake_n, wake_pitch)
    } else {
        String::from("(profile state locked)")
    }
}

fn recompute_caches(s: &mut ProfileState) {
    s.cached_speaker = if s.speaker_samples.len() >= 3 {
        let n = s.speaker_samples.len() as f32;
        let pitch = s.speaker_samples.iter().map(|p| p.pitch_hz).sum::<f32>() / n;
        let rms = s.speaker_samples.iter().map(|p| p.rms).sum::<f32>() / n;
        let act = s.speaker_samples.iter().map(|p| p.activity).sum::<f32>() / n;
        Some(TonalProfile { pitch_hz: pitch, rms, activity: act, ..Default::default() })
    } else { None };

    s.cached_noise_floor = if !s.noise_rms.is_empty() {
        let sum: f32 = s.noise_rms.iter().copied().sum();
        sum / s.noise_rms.len() as f32
    } else { 0.0 };
}

fn profile_path() -> PathBuf {
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into()))
        .join(".config/web2qml");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("audio_profile.bin")
}

/// Disk format (binary):
///   [u32 magic = 0x41504346 'APCF']  (Audio Profile Cache File)
///   [u16 version]
///   [u16 speaker_count]
///   [speaker_count * 12 bytes: 3 * f32 (pitch_hz, rms, activity)]
///   [u16 noise_count]
///   [noise_count * 4 bytes: f32 (rms)]
///
/// Version 2 adds (after noise block):
///   [u16 wake_word_count]
///   [wake_word_count * 12 bytes: 3 * f32 (pitch_hz, rms, activity)]
///
/// Loaded files at version 1 have no wake_word section — load_from_disk
/// detects EOF after the noise block and treats wake_word_samples as
/// empty, no failure. Forward-compat by design.
///
/// All little-endian. No compression — file is tiny (~520 bytes max
/// at 30 + 30 + 10 samples).
fn save_to_disk_locked(s: &ProfileState) -> std::io::Result<()> {
    if !crate::security::persistence_enabled() {
        return Ok(());  // silently skip — caller doesn't need to know
    }
    use std::io::Write;
    // v0.1.335: route through the pure encoder so the format
    // can be tested without touching disk.
    let buf = encode_profile_bytes(
        &s.speaker_samples,
        &s.noise_rms,
        &s.wake_word_samples,
    );
    let path = profile_path();
    let tmp = path.with_extension("bin.tmp");
    std::fs::File::create(&tmp)?.write_all(&buf)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn load_from_disk() -> Option<(Vec<TonalProfile>, Vec<f32>, Vec<TonalProfile>)> {
    let bytes = std::fs::read(profile_path()).ok()?;
    let (speakers, noises, wake_words) = decode_profile_bytes(&bytes)?;
    // Determine version for the log message — re-read from bytes
    // since decode_profile_bytes doesn't return it.
    let version = if bytes.len() >= 6 {
        u16::from_le_bytes([bytes[4], bytes[5]])
    } else { 0 };
    crate::clog!("audio_profile", "loaded {} speaker + {} noise + {} wake-word samples (v{})",
        speakers.len(), noises.len(), wake_words.len(), version);
    Some((speakers, noises, wake_words))
}

/// Pure bytes-level encode of a profile state. Used by
/// `save_to_disk_locked` (with file I/O wrapped around it) and by
/// the test suite (no I/O at all). Mirrors the v2 disk format
/// documented at `save_to_disk_locked`.
///
/// v0.1.335: extracted from the inline encoder so we can test the
/// format round-trip without a temp file dance.
fn encode_profile_bytes(speakers: &[TonalProfile],
                        noise_rms: &[f32],
                        wake_words: &[TonalProfile]) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::with_capacity(8 + speakers.len() * 12
        + 2 + noise_rms.len() * 4
        + 2 + wake_words.len() * 12);
    buf.extend_from_slice(&0x41504346u32.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());  // v2
    buf.extend_from_slice(&(speakers.len() as u16).to_le_bytes());
    for p in speakers {
        buf.extend_from_slice(&p.pitch_hz.to_le_bytes());
        buf.extend_from_slice(&p.rms.to_le_bytes());
        buf.extend_from_slice(&p.activity.to_le_bytes());
    }
    buf.extend_from_slice(&(noise_rms.len() as u16).to_le_bytes());
    for r in noise_rms {
        buf.extend_from_slice(&r.to_le_bytes());
    }
    buf.extend_from_slice(&(wake_words.len() as u16).to_le_bytes());
    for p in wake_words {
        buf.extend_from_slice(&p.pitch_hz.to_le_bytes());
        buf.extend_from_slice(&p.rms.to_le_bytes());
        buf.extend_from_slice(&p.activity.to_le_bytes());
    }
    buf
}

/// Pure bytes-level decode. Returns None on magic mismatch,
/// version mismatch, or truncation mid-record. v1 files
/// (with no wake_word section) decode to an empty wake_words Vec.
///
/// v0.1.335: extracted from the inline `load_from_disk` decoder
/// to allow unit tests + cleaner separation between I/O and parsing.
fn decode_profile_bytes(bytes: &[u8])
    -> Option<(Vec<TonalProfile>, Vec<f32>, Vec<TonalProfile>)>
{
    if bytes.len() < 8 { return None; }
    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if magic != 0x41504346 { return None; }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != 1 && version != 2 { return None; }
    let mut pos = 6usize;
    let read_u16 = |b: &[u8], p: usize| -> Option<u16> {
        if p + 2 > b.len() { return None; }
        Some(u16::from_le_bytes([b[p], b[p+1]]))
    };
    let read_f32 = |b: &[u8], p: usize| -> Option<f32> {
        if p + 4 > b.len() { return None; }
        Some(f32::from_le_bytes([b[p], b[p+1], b[p+2], b[p+3]]))
    };
    let spk_n = read_u16(bytes, pos)? as usize;
    pos += 2;
    let mut speakers = Vec::with_capacity(spk_n);
    for _ in 0..spk_n {
        let pitch = read_f32(bytes, pos)?; pos += 4;
        let rms = read_f32(bytes, pos)?; pos += 4;
        let act = read_f32(bytes, pos)?; pos += 4;
        speakers.push(TonalProfile { pitch_hz: pitch, rms, activity: act, ..Default::default() });
    }
    let noise_n = read_u16(bytes, pos)? as usize;
    pos += 2;
    let mut noises = Vec::with_capacity(noise_n);
    for _ in 0..noise_n {
        let r = read_f32(bytes, pos)?; pos += 4;
        noises.push(r);
    }
    let mut wake_words: Vec<TonalProfile> = Vec::new();
    if version >= 2 && pos + 2 <= bytes.len() {
        let ww_n = read_u16(bytes, pos)? as usize;
        pos += 2;
        for _ in 0..ww_n {
            let pitch = match read_f32(bytes, pos) { Some(v) => v, None => break };
            pos += 4;
            let rms = match read_f32(bytes, pos) { Some(v) => v, None => break };
            pos += 4;
            let act = match read_f32(bytes, pos) { Some(v) => v, None => break };
            pos += 4;
            wake_words.push(TonalProfile { pitch_hz: pitch, rms, activity: act, ..Default::default() });
        }
    }
    Some((speakers, noises, wake_words))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tp(pitch: f32, rms: f32, activity: f32) -> TonalProfile {
        TonalProfile { pitch_hz: pitch, rms, activity, ..Default::default() }
    }

    // ── recompute_caches: cached_speaker ──

    #[test]
    fn cached_speaker_none_below_three_samples() {
        let mut s = ProfileState::new();
        s.speaker_samples.push(tp(120.0, 0.05, 0.5));
        s.speaker_samples.push(tp(130.0, 0.06, 0.6));
        recompute_caches(&mut s);
        // Two samples is below the 3-sample threshold.
        assert!(s.cached_speaker.is_none(),
            "should be None with 2 samples, got {:?}", s.cached_speaker);
    }

    #[test]
    fn cached_speaker_averages_three_or_more() {
        let mut s = ProfileState::new();
        s.speaker_samples.push(tp(100.0, 0.04, 0.4));
        s.speaker_samples.push(tp(200.0, 0.08, 0.8));
        s.speaker_samples.push(tp(300.0, 0.12, 1.2));
        recompute_caches(&mut s);
        let avg = s.cached_speaker.as_ref().expect("Some with 3 samples");
        // Mean pitch = 200, mean rms = 0.08, mean activity = 0.8.
        assert!((avg.pitch_hz - 200.0).abs() < 0.01,
            "pitch avg: {}", avg.pitch_hz);
        assert!((avg.rms - 0.08).abs() < 0.001, "rms avg: {}", avg.rms);
        assert!((avg.activity - 0.8).abs() < 0.01, "activity avg: {}", avg.activity);
    }

    // ── recompute_caches: cached_noise_floor ──

    #[test]
    fn cached_noise_floor_zero_when_empty() {
        let mut s = ProfileState::new();
        recompute_caches(&mut s);
        assert_eq!(s.cached_noise_floor, 0.0);
    }

    #[test]
    fn cached_noise_floor_is_mean() {
        let mut s = ProfileState::new();
        s.noise_rms.push(0.01);
        s.noise_rms.push(0.02);
        s.noise_rms.push(0.03);
        recompute_caches(&mut s);
        assert!((s.cached_noise_floor - 0.02).abs() < 1e-6,
            "noise floor: {}", s.cached_noise_floor);
    }

    #[test]
    fn cached_noise_floor_single_sample() {
        let mut s = ProfileState::new();
        s.noise_rms.push(0.007);
        recompute_caches(&mut s);
        assert!((s.cached_noise_floor - 0.007).abs() < 1e-7);
    }

    // ── disk format: encode + decode round-trip ──

    #[test]
    fn encode_decode_empty_state_roundtrips() {
        let bytes = encode_profile_bytes(&[], &[], &[]);
        let (sp, ns, ww) = decode_profile_bytes(&bytes)
            .expect("empty state should decode");
        assert_eq!(sp.len(), 0);
        assert_eq!(ns.len(), 0);
        assert_eq!(ww.len(), 0);
    }

    #[test]
    fn encode_decode_populated_state_roundtrips() {
        let speakers = vec![
            tp(120.0, 0.05, 0.5),
            tp(130.0, 0.06, 0.6),
            tp(140.0, 0.07, 0.7),
        ];
        let noise = vec![0.008, 0.010, 0.012];
        let wakes = vec![tp(135.0, 0.06, 0.65), tp(138.0, 0.065, 0.7)];

        let bytes = encode_profile_bytes(&speakers, &noise, &wakes);
        let (sp_back, ns_back, ww_back) = decode_profile_bytes(&bytes)
            .expect("populated state should decode");

        assert_eq!(sp_back.len(), speakers.len());
        for (orig, back) in speakers.iter().zip(sp_back.iter()) {
            assert!((orig.pitch_hz - back.pitch_hz).abs() < 1e-6);
            assert!((orig.rms - back.rms).abs() < 1e-7);
            assert!((orig.activity - back.activity).abs() < 1e-6);
        }
        assert_eq!(ns_back, noise);
        assert_eq!(ww_back.len(), wakes.len());
        for (orig, back) in wakes.iter().zip(ww_back.iter()) {
            assert!((orig.pitch_hz - back.pitch_hz).abs() < 1e-6);
            assert!((orig.rms - back.rms).abs() < 1e-7);
        }
    }

    // ── decode failure modes ──

    #[test]
    fn decode_rejects_empty_bytes() {
        assert!(decode_profile_bytes(&[]).is_none());
    }

    #[test]
    fn decode_rejects_too_short() {
        // <8 bytes can't even hold the header.
        assert!(decode_profile_bytes(&[0; 4]).is_none());
        assert!(decode_profile_bytes(&[0; 7]).is_none());
    }

    #[test]
    fn decode_rejects_wrong_magic() {
        let mut bytes = encode_profile_bytes(&[], &[], &[]);
        // Corrupt the magic.
        bytes[0] = 0xFF;
        assert!(decode_profile_bytes(&bytes).is_none(),
            "should reject bad magic");
    }

    #[test]
    fn decode_rejects_unknown_version() {
        let mut bytes = encode_profile_bytes(&[], &[], &[]);
        // Bump version to something unknown.
        bytes[4] = 99;
        bytes[5] = 0;
        assert!(decode_profile_bytes(&bytes).is_none(),
            "should reject unknown version");
    }

    /// v1 files (no wake_word section) must decode cleanly to
    /// an empty wake_words Vec — forward-compat on the disk
    /// format. Construct a v1 file by hand: magic + version=1 +
    /// 0 speakers + 0 noise.
    #[test]
    fn decode_handles_v1_files_without_wake_section() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x41504346u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());  // v1
        bytes.extend_from_slice(&0u16.to_le_bytes());  // 0 speakers
        bytes.extend_from_slice(&0u16.to_le_bytes());  // 0 noise
        // No wake-word section.
        let (sp, ns, ww) = decode_profile_bytes(&bytes)
            .expect("v1 file should decode");
        assert_eq!(sp.len(), 0);
        assert_eq!(ns.len(), 0);
        assert_eq!(ww.len(), 0, "v1 file should have empty wake_words");
    }

    /// Truncated payload (header says N samples but bytes don't
    /// contain enough) returns None.
    #[test]
    fn decode_rejects_truncation_in_speaker_section() {
        // Claim 3 speakers but only encode 1.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x41504346u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes());  // 3 speakers claimed
        // Only one speaker's worth of f32s (12 bytes).
        bytes.extend_from_slice(&120.0f32.to_le_bytes());
        bytes.extend_from_slice(&0.05f32.to_le_bytes());
        bytes.extend_from_slice(&0.5f32.to_le_bytes());
        assert!(decode_profile_bytes(&bytes).is_none(),
            "truncated speaker section should reject");
    }

    /// Truncation in the wake-word section is more forgiving:
    /// the decoder breaks out of the inner loop. So a file that
    /// claims 5 wakes but only contains 2 still decodes — with
    /// 2 entries returned. This is intentional (forward-compat
    /// behavior). Pin it so future refactors don't tighten it.
    #[test]
    fn decode_partial_wake_section_returns_complete_entries() {
        let mut bytes = encode_profile_bytes(&[], &[],
            &[tp(120.0, 0.05, 0.5), tp(130.0, 0.06, 0.6)]);
        // Truncate the last byte so the second wake's `activity`
        // field is incomplete.
        bytes.pop();
        let (_, _, ww) = decode_profile_bytes(&bytes)
            .expect("partial wake section should still decode");
        assert_eq!(ww.len(), 1,
            "should return only the complete entries: got {} (bytes: {})",
            ww.len(), bytes.len());
    }

    // ── encoding format byte-level checks ──

    #[test]
    fn encoded_magic_is_apcf() {
        let bytes = encode_profile_bytes(&[], &[], &[]);
        // Magic 0x41504346 little-endian = "FCPA" reversed = "APCF".
        // Read it back via u32::from_le_bytes to verify match.
        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(magic, 0x41504346);
        // Spot-check the ASCII bytes too — easier to debug
        // a corrupt file from a hex dump.
        assert_eq!(&bytes[0..4], b"FCPA",
            "little-endian bytes of 0x41504346 are 'F' 'C' 'P' 'A'");
    }

    #[test]
    fn encoded_version_field_is_two() {
        // The encoder always writes v2. Pin this so anyone
        // bumping the version remembers to bump the load_from_disk
        // accepted-versions set too.
        let bytes = encode_profile_bytes(&[], &[], &[]);
        let v = u16::from_le_bytes([bytes[4], bytes[5]]);
        assert_eq!(v, 2, "encoder must write v2 files");
    }

    // ── speaker_match_score smoke check (no I/O) ──

    /// speaker_match_score should return 0.5 ("no opinion") in
    /// at least one of two cases:
    ///   - There's no profile yet → first early return
    ///   - The audio has no detectable pitch → second early return
    ///
    /// Both paths produce 0.5. We pass empty audio so pitch=0 and
    /// the second path triggers. This holds regardless of whether
    /// the module's OnceLock state was populated from disk at
    /// `state()` init (which would happen if the user already had
    /// a `~/.config/web2qml/audio_profile.bin`).
    #[test]
    fn speaker_match_score_returns_half_with_no_pitch() {
        let score = speaker_match_score(&[]);
        assert!((score - 0.5).abs() < 1e-6,
            "empty audio should score 0.5, got {}", score);
    }
}
