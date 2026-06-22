// src/pre_buffer.rs
#![allow(dead_code)]
//
// "Predictive stream packets in the low frequencies" — short, low-fidelity,
// low-frequency audio that bridges the perceptual gap between a user's
// tap and the real high-quality stream arriving. The technique exploits
// two well-studied properties of human hearing:
//
//   1. Low frequencies are SLOW. Below ~200 Hz, a single cycle takes 5+
//      ms, so the ear can't pin down arrival time with sub-frame precision
//      (above ~1 kHz, a few ms shift is noticeable). We can begin a
//      low-frequency precursor up to ~30 ms before the "real" audio and
//      the listener perceives them as a single event.
//
//   2. Low frequencies are SPECTRALLY MASKING. A bass pulse playing
//      under a starting track's transient hides clicks/seams that would
//      otherwise be audible. This is how DJs do beat-matched transitions
//      — the kick drum masks the splice.
//
// What this module does:
//
//   * `synthesize_cue(bpm, key_hz)` generates a 500 ms low-pass cue at
//     the requested tempo. One soft "thump" at the BPM pulse, anchored
//     at the track's tonic frequency (if known, else 60 Hz default).
//   * Output is mono 16-bit PCM @ 48 kHz — same format as the rest of
//     the audio pipeline so it can be served via track_server or piped
//     into paplay.
//   * Falls back to a brown-noise burst when neither tempo nor key is
//     known. Lower information content but still perceptually masks the
//     latency gap.
//
// HONEST SCOPE — what this is NOT:
//
//   * Not a decoded preview of the actual track. Real audio decoding (to
//     filter the first ~100 ms of an MP3/FLAC/M4A down to its bass band)
//     requires a decoder dependency we haven't added. The synthesized
//     cue is a *placeholder* whose tempo+pitch match the predicted track
//     so the perceptual handoff to the real audio is smooth.
//   * Not active noise masking. Different purpose from sanitizer.rs;
//     this is intended as a brief earcon, not continuous background.
//   * Not zero-latency by itself. The cue still has to travel to the
//     device. But because it's small (~50 KB for 500 ms at 48 kHz mono
//     16-bit) and we serve it locally, the round-trip is dominated by
//     network latency to the speaker — same as everything else, but the
//     content is much smaller than a full track.

use std::f32::consts::PI;

/// Reasonable defaults when we don't know the predicted track's traits.
pub const DEFAULT_BPM: f32 = 100.0;
pub const DEFAULT_TONIC_HZ: f32 = 60.0;  // C2-ish, deep but audible
pub const SAMPLE_RATE: u32 = 48_000;

/// One soft "thump" envelope at the given duration. Smooth attack/decay
/// so the cue doesn't itself click. Returns a multiplier in [0, 1].
fn envelope_at(t_normalized: f32) -> f32 {
    // Asymmetric envelope: short attack (10%), longer release (90%).
    // Peak at 10% of the cue length feels like a percussive pulse.
    let t = t_normalized.clamp(0.0, 1.0);
    if t < 0.1 {
        // Smooth attack
        let x = t / 0.1;
        x * x * (3.0 - 2.0 * x)  // smoothstep
    } else {
        // Exponential release
        let x = (t - 0.1) / 0.9;
        (-x * 5.0).exp()
    }
}

/// Synthesize a single low-frequency cue. Returns mono 16-bit PCM ready
/// to wrap in a WAV header. `bpm` is for tempo-locked pulse spacing if
/// the cue duration spans multiple beats; for typical 500 ms cues at
/// BPM ≥ 90 we get a single thump.
pub fn synthesize_cue_pcm(
    bpm: f32,
    tonic_hz: f32,
    duration_ms: u32,
) -> Vec<u8> {
    // f32::clamp(NaN, lo, hi) returns NaN — propagates through the
    // entire trig path and turns every sample into NaN. The PCM cast
    // saturates NaN → 0 so it'd come out as silence, but that's
    // user-visible as "cue went missing"; cheaper to coerce here.
    let bpm = if bpm.is_finite() { bpm.clamp(40.0, 220.0) } else { DEFAULT_BPM };
    let tonic = if tonic_hz.is_finite() { tonic_hz.clamp(30.0, 250.0) } else { DEFAULT_TONIC_HZ };
    let n_samples = (SAMPLE_RATE as u64 * duration_ms as u64 / 1000) as usize;
    let mut out = Vec::with_capacity(n_samples * 2);

    // Period between beats (samples).
    let beat_period_s = 60.0 / bpm;
    let beat_period_samples = (beat_period_s * SAMPLE_RATE as f32) as usize;
    // Within each beat, the cue is the envelope shape modulating the
    // tonic frequency (with octave harmonic for body).
    let cue_len_samples = (SAMPLE_RATE as f32 * 0.18) as usize;  // ~180 ms thump

    for i in 0..n_samples {
        let beat_pos = if beat_period_samples > 0 { i % beat_period_samples } else { i };
        let in_cue = beat_pos < cue_len_samples;
        let sample = if in_cue {
            let t_norm = (beat_pos as f32) / (cue_len_samples as f32);
            let env = envelope_at(t_norm);
            let t = (i as f32) / (SAMPLE_RATE as f32);
            let fundamental = (2.0 * PI * tonic * t).sin();
            // Octave harmonic at half amplitude for body without taking
            // attention from the fundamental.
            let harmonic   = 0.5 * (2.0 * PI * (tonic * 2.0) * t).sin();
            env * (fundamental + harmonic) * 0.4
        } else {
            0.0
        };
        let i16_val = (sample.clamp(-1.0, 1.0) * 32_767.0) as i16;
        out.extend_from_slice(&i16_val.to_le_bytes());
    }
    out
}

/// Convenience: synthesize + wrap as a mono WAV.
pub fn synthesize_cue_wav(bpm: f32, tonic_hz: f32, duration_ms: u32) -> Vec<u8> {
    let pcm = synthesize_cue_pcm(bpm, tonic_hz, duration_ms);
    wrap_mono_wav(&pcm, SAMPLE_RATE, 16)
}

// ── ESTIMATE CUE ─────────────────────────────────────────────────────
//
// A richer "estimate" cue designed to be a gapless fade INTO the real
// audio when it arrives. Unlike `synthesize_cue_pcm` (a quick percussive
// thump), this one builds up and then exponentially decays — so when the
// device's real audio kicks in, the local cue is already trailing off,
// producing perceptual crossfade rather than a hard cut.
//
// Envelope (three phases, summing to `duration_ms`):
//
//                 ┌──── peak ────┐
//                /│              │\__
//      build-up / │   sustain    │   \____ decay
//   ────────── /  │  (modulated  │        \__________
//             /   │   on BPM)    │
//             0%  30%            80%       100%
//
// At higher aggression levels we add more harmonics (richer timbre that
// blends better with arbitrary incoming music) and a longer overall
// duration. The decay tail is exponential so the user perceives the
// real-audio arrival as a continuation, not a transition.

/// Synthesize a build-up cue at the predicted tempo and tonic. Returns
/// raw 16-bit PCM ready to pipe into paplay; empty Vec when harmonics=0
/// (the OFF aggression level).
pub fn synthesize_estimate_cue(
    bpm: f32,
    tonic_hz: f32,
    duration_ms: u32,
    harmonics: u32,
) -> Vec<u8> {
    if harmonics == 0 || duration_ms == 0 { return Vec::new(); }
    let bpm = if bpm.is_finite() { bpm.clamp(40.0, 220.0) } else { DEFAULT_BPM };
    let tonic = if tonic_hz.is_finite() { tonic_hz.clamp(30.0, 250.0) } else { DEFAULT_TONIC_HZ };
    let harmonics = harmonics.min(8);  // ceiling — beyond 8 we just add noise
    let n_samples = (SAMPLE_RATE as u64 * duration_ms as u64 / 1000) as usize;
    let mut out = Vec::with_capacity(n_samples * 2);

    // Precompute harmonic amplitudes — 1/h^0.7 rolloff is gentle but
    // keeps the tonic dominant.
    let amps: Vec<f32> = (1..=harmonics)
        .map(|h| 1.0 / (h as f32).powf(0.7))
        .collect();
    let norm: f32 = amps.iter().map(|a| a * a).sum::<f32>().sqrt().max(1e-6);

    for i in 0..n_samples {
        let t_norm = i as f32 / n_samples as f32;
        let t = i as f32 / SAMPLE_RATE as f32;

        // Three-phase envelope: smoothstep build-up, sustained with
        // BPM-locked tremolo, exponential decay.
        let env = if t_norm < 0.30 {
            // Smoothstep 0→1 over 0..0.30
            let x = t_norm / 0.30;
            x * x * (3.0 - 2.0 * x)
        } else if t_norm < 0.80 {
            // Sustain plateau with ±20% modulation locked to BPM. The
            // modulation gives the cue a sense of motion that matches
            // the predicted track's pulse.
            let beat_period = 60.0 / bpm;
            let beat_phase = (t / beat_period) % 1.0;
            0.85 + 0.15 * (2.0 * PI * beat_phase).cos()
        } else {
            // Exponential decay over last 20%. exp(-5x) ≈ 0.007 at x=1,
            // so by the time `duration_ms` elapses the cue is silent.
            let x = (t_norm - 0.80) / 0.20;
            (-x * 5.0).exp()
        };

        // Stack harmonics
        let mut signal = 0.0_f32;
        for (idx, &amp) in amps.iter().enumerate() {
            let h = (idx + 1) as f32;
            let freq = tonic * h;
            // Skip super-aliasing or sub-audible harmonics. The lower
            // bound stays in for sub-bass content.
            if freq > 12_000.0 { continue; }
            signal += amp * (2.0 * PI * freq * t).sin();
        }
        let normalized = signal / norm;

        // Final scale: 0.4 keeps the cue audible but well below the
        // ~0.8 headroom of typical playback. Avoids any clipping when
        // the harmonics align constructively.
        let s = (env * normalized * 0.4).clamp(-1.0, 1.0);
        let i16_val = (s * 32_767.0) as i16;
        out.extend_from_slice(&i16_val.to_le_bytes());
    }
    out
}

/// Cue scheduling: returns the duration that best fits the predicted
/// device latency, aligned to the track's BPM so the cue ends on a
/// musical beat. The cue is fired immediately on tap; the device's real
/// audio arrives some time later (depending on its buffering) and
/// overlaps with the cue's decay tail.
#[derive(Debug, Clone, Copy)]
pub struct CueSchedule {
    /// How long the cue should play in total.
    pub duration_ms: u32,
    /// Estimated time before device real-audio arrives. Used purely for
    /// callers who want to anticipate the crossfade boundary.
    pub estimated_device_arrival_ms: u32,
}

/// Compute a duration that fits the predicted device latency, aligned to
/// the BPM downbeat so the cue's tail meets the real audio at a musical
/// boundary. `duration_request_ms` is the policy's preferred duration;
/// we round to the nearest quarter-beat without going below 150ms or
/// above 800ms.
pub fn schedule_for_track(
    bpm: f32,
    duration_request_ms: u32,
    estimated_device_latency_ms: u32,
) -> CueSchedule {
    if duration_request_ms == 0 {
        return CueSchedule {
            duration_ms: 0,
            estimated_device_arrival_ms: estimated_device_latency_ms,
        };
    }
    let bpm = bpm.clamp(40.0, 220.0);
    let beat_period_ms = (60_000.0 / bpm) as u32;
    let quarter_beat_ms = (beat_period_ms / 4).max(1);
    // Round duration to the nearest quarter beat
    let aligned = if duration_request_ms < quarter_beat_ms {
        duration_request_ms
    } else {
        let q = (duration_request_ms + quarter_beat_ms / 2) / quarter_beat_ms;
        q * quarter_beat_ms
    };
    let bounded = aligned.clamp(150, 800);
    CueSchedule {
        duration_ms: bounded,
        estimated_device_arrival_ms: estimated_device_latency_ms,
    }
}

fn wrap_mono_wav(samples: &[u8], sample_rate: u32, bits: u16) -> Vec<u8> {
    let byte_rate = sample_rate * (bits as u32 / 8);
    let block_align = bits / 8;
    let data_len = samples.len() as u32;
    let mut w = Vec::with_capacity(44 + samples.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());    // PCM
    w.extend_from_slice(&1u16.to_le_bytes());    // mono
    w.extend_from_slice(&sample_rate.to_le_bytes());
    w.extend_from_slice(&byte_rate.to_le_bytes());
    w.extend_from_slice(&block_align.to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    w.extend_from_slice(samples);
    w
}

/// Map a Camelot/key string to its approximate tonic frequency. The key
/// detector elsewhere in the catalog gives us camelot like "5A" / "8B"
/// which corresponds to specific keys. We just need the tonic note's
/// pitch in Hz at a comfortable bass octave (~2nd octave, C2 = 65.4).
pub fn tonic_hz_for_camelot(camelot: &str) -> f32 {
    // Camelot Wheel → tonic note. A = minor, B = major.
    // C/Am=8B/8A, G/Em=9B/9A, D/Bm=10B/10A, A/F#m=11B/11A, E/C#m=12B/12A,
    // B/G#m=1B/1A, F#/D#m=2B/2A, Db/Bbm=3B/3A, Ab/Fm=4B/4A, Eb/Cm=5B/5A,
    // Bb/Gm=6B/6A, F/Dm=7B/7A
    // Frequencies are at octave 2 (C2 = 65.41 Hz).
    let tonic = match camelot.to_ascii_uppercase().trim_end_matches(|c: char| c == 'A' || c == 'B') {
        "8"  => 65.41,   // C2
        "9"  => 98.00,   // G2 (still in our 30-250 band)
        "10" => 73.42,   // D2
        "11" => 55.00,   // A1 — drop an octave to stay <250
        "12" => 82.41,   // E2
        "1"  => 61.74,   // B1
        "2"  => 92.50,   // F#2
        "3"  => 69.30,   // C#2 / Db2
        "4"  => 51.91,   // G#1 / Ab1
        "5"  => 77.78,   // D#2 / Eb2
        "6"  => 58.27,   // A#1 / Bb1
        "7"  => 87.31,   // F2
        _    => DEFAULT_TONIC_HZ,
    };
    tonic
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_starts_and_ends_quiet() {
        assert!(envelope_at(0.0) < 0.01);
        assert!(envelope_at(1.0) < 0.05);
    }

    #[test]
    fn envelope_peaks_near_ten_percent() {
        let peak = envelope_at(0.1);
        // Should be near 1.0 — that's the boundary of attack→release.
        assert!(peak > 0.9, "peak should be near 1.0, got {}", peak);
    }

    #[test]
    fn synthesize_produces_correct_pcm_length() {
        let pcm = synthesize_cue_pcm(120.0, 60.0, 500);
        // 500 ms at 48 kHz mono 16-bit = 48_000 * 0.5 * 2 = 48_000 bytes
        assert_eq!(pcm.len(), 48_000);
    }

    #[test]
    fn synthesize_pcm_stays_in_int16_range() {
        let pcm = synthesize_cue_pcm(120.0, 80.0, 200);
        for chunk in pcm.chunks_exact(2) {
            let s = i16::from_le_bytes([chunk[0], chunk[1]]);
            // The waveform is normalized; should never clip.
            assert!(s.abs() < 32_000, "sample {} too hot", s);
        }
    }

    #[test]
    fn wav_starts_with_riff() {
        let wav = synthesize_cue_wav(100.0, 60.0, 100);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
    }

    #[test]
    fn pre_buffer__cue_nan_inputs_produce_valid_audio() {
        // f32::clamp returns NaN for NaN input — would zero the cue
        // (PCM cast saturates NaN to 0) and lose perceptual masking.
        // Verify the cue falls back to defaults and emits real samples.
        for (bpm, tonic) in [
            (f32::NAN, 60.0_f32), (120.0_f32, f32::NAN),
            (f32::INFINITY, 60.0_f32), (120.0_f32, f32::INFINITY),
            (f32::NEG_INFINITY, f32::NAN),
        ] {
            let pcm = synthesize_cue_pcm(bpm, tonic, 200);
            assert!(!pcm.is_empty());
            // Must contain at least some non-zero samples.
            let nonzero = pcm.chunks_exact(2)
                .filter(|c| i16::from_le_bytes([c[0], c[1]]) != 0)
                .count();
            assert!(nonzero > 100,
                    "cue went silent on bpm={} tonic={}: {} nonzero samples",
                    bpm, tonic, nonzero);
        }
    }

    #[test]
    fn pre_buffer__estimate_cue_nan_inputs_produce_valid_audio() {
        for (bpm, tonic) in [
            (f32::NAN, 60.0_f32), (120.0_f32, f32::NAN),
            (f32::INFINITY, f32::INFINITY),
        ] {
            let pcm = synthesize_estimate_cue(bpm, tonic, 200, 4);
            assert!(!pcm.is_empty());
            let nonzero = pcm.chunks_exact(2)
                .filter(|c| i16::from_le_bytes([c[0], c[1]]) != 0)
                .count();
            assert!(nonzero > 100,
                    "estimate cue went silent on bpm={} tonic={}",
                    bpm, tonic);
        }
    }

    #[test]
    fn cue_has_actual_energy_at_tonic() {
        // Quick correlation against the tonic frequency.
        let bpm = 90.0_f32;
        let tonic = 80.0_f32;
        let pcm = synthesize_cue_pcm(bpm, tonic, 500);
        // Read back as i16 samples.
        let samples: Vec<f32> = pcm.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_767.0)
            .collect();
        let sr = SAMPLE_RATE as f32;
        // Correlate against a same-frequency reference sine.
        let mut corr = 0.0_f32;
        for (i, s) in samples.iter().enumerate() {
            let ref_sin = (2.0 * PI * tonic * (i as f32) / sr).sin();
            corr += s * ref_sin;
        }
        // Should be significantly positive (cue contains energy at tonic).
        assert!(corr > 50.0,
                "expected significant tonic correlation, got {}", corr);
    }

    #[test]
    fn higher_bpm_gives_more_pulses_in_same_window() {
        // At 60 BPM, beat period = 48000 samples; in 1000 ms (48000 samples)
        // we get 1 thump. At 240 BPM, beat period = 12000; we get 4 thumps.
        let slow = synthesize_cue_pcm(60.0, 60.0, 1000);
        let fast = synthesize_cue_pcm(240.0, 60.0, 1000);
        // Count significant peaks (local maxima above some threshold).
        let count_peaks = |pcm: &[u8]| -> usize {
            let samples: Vec<f32> = pcm.chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]).abs() as f32 / 32_767.0)
                .collect();
            // RMS over 1000-sample windows; peaks where RMS exceeds threshold.
            let mut peaks = 0;
            let mut in_peak = false;
            for w in samples.chunks(1000) {
                let rms = (w.iter().map(|x| x*x).sum::<f32>() / w.len() as f32).sqrt();
                if rms > 0.1 && !in_peak { peaks += 1; in_peak = true; }
                else if rms <= 0.05 { in_peak = false; }
            }
            peaks
        };
        let slow_peaks = count_peaks(&slow);
        let fast_peaks = count_peaks(&fast);
        assert!(fast_peaks > slow_peaks,
                "240 BPM should have more peaks ({} vs {})", fast_peaks, slow_peaks);
    }

    #[test]
    fn camelot_5b_maps_to_eflat_freq() {
        // 5B is Eb major; tonic Eb2 ≈ 77.78 Hz.
        assert!((tonic_hz_for_camelot("5B") - 77.78).abs() < 1.0);
        assert!((tonic_hz_for_camelot("5A") - 77.78).abs() < 1.0);
    }

    #[test]
    fn unknown_camelot_returns_default() {
        assert_eq!(tonic_hz_for_camelot("garbage"), DEFAULT_TONIC_HZ);
        assert_eq!(tonic_hz_for_camelot(""), DEFAULT_TONIC_HZ);
    }

    #[test]
    fn tonic_freqs_stay_in_low_band() {
        for camelot in ["1A","1B","2A","2B","3A","3B","4A","4B",
                        "5A","5B","6A","6B","7A","7B","8A","8B",
                        "9A","9B","10A","10B","11A","11B","12A","12B"] {
            let hz = tonic_hz_for_camelot(camelot);
            assert!(hz >= 30.0 && hz <= 250.0,
                    "tonic for {} = {} out of low-band", camelot, hz);
        }
    }

    // ── Estimate cue tests ───────────────────────────────────────────

    #[test]
    fn estimate_cue_zero_harmonics_returns_empty() {
        let v = synthesize_estimate_cue(120.0, 60.0, 400, 0);
        assert!(v.is_empty());
    }

    #[test]
    fn estimate_cue_zero_duration_returns_empty() {
        let v = synthesize_estimate_cue(120.0, 60.0, 0, 4);
        assert!(v.is_empty());
    }

    #[test]
    fn estimate_cue_length_matches_request() {
        let pcm = synthesize_estimate_cue(120.0, 80.0, 400, 4);
        // 400 ms * 48000 samples/s * 2 bytes = 38400
        assert_eq!(pcm.len(), 38_400);
    }

    #[test]
    fn estimate_cue_envelope_starts_quiet() {
        // First ~5% of samples should be very low amplitude (build-up
        // is just starting). Compare RMS of first 5% to peak.
        let pcm = synthesize_estimate_cue(120.0, 80.0, 500, 4);
        let samples: Vec<f32> = pcm.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_767.0)
            .collect();
        let first_5pct = &samples[..samples.len() / 20];
        let middle = &samples[samples.len() / 3..samples.len() / 2];
        let rms = |s: &[f32]| (s.iter().map(|x| x*x).sum::<f32>() / s.len() as f32).sqrt();
        assert!(rms(first_5pct) < rms(middle) * 0.5,
                "build-up should ramp up: start_rms={} mid_rms={}",
                rms(first_5pct), rms(middle));
    }

    #[test]
    fn estimate_cue_decays_to_silence() {
        // Last ~5% of samples should be very low amplitude.
        let pcm = synthesize_estimate_cue(120.0, 80.0, 500, 4);
        let samples: Vec<f32> = pcm.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_767.0)
            .collect();
        let last_5pct = &samples[samples.len() * 19 / 20..];
        let middle = &samples[samples.len() / 3..samples.len() / 2];
        let rms = |s: &[f32]| (s.iter().map(|x| x*x).sum::<f32>() / s.len() as f32).sqrt();
        assert!(rms(last_5pct) < rms(middle) * 0.3,
                "decay should fade to near silence: end_rms={} mid_rms={}",
                rms(last_5pct), rms(middle));
    }

    #[test]
    fn estimate_cue_concentrates_energy_at_tonic() {
        let bpm = 100.0;
        let tonic = 65.0;
        let pcm = synthesize_estimate_cue(bpm, tonic, 400, 4);
        let samples: Vec<f32> = pcm.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_767.0)
            .collect();
        // Sustain region only — outside the build-up & decay where
        // the signal exists predominantly at the tonic
        let start = samples.len() * 35 / 100;
        let end   = samples.len() * 75 / 100;
        let sustain = &samples[start..end];
        let sr = SAMPLE_RATE as f32;
        // Correlate against reference tonic sine
        let mut corr = 0.0_f32;
        for (i, s) in sustain.iter().enumerate() {
            let t = (start + i) as f32 / sr;
            let ref_sin = (2.0 * PI * tonic * t).sin();
            corr += s * ref_sin;
        }
        assert!(corr > 30.0,
                "expected tonic correlation, got {}", corr);
    }

    #[test]
    fn estimate_cue_stays_in_int16_range() {
        for harmonics in [2, 4, 6, 8] {
            let pcm = synthesize_estimate_cue(120.0, 60.0, 300, harmonics);
            for chunk in pcm.chunks_exact(2) {
                let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                assert!(s.abs() < 32_000,
                        "sample {} too hot at harmonics={}", s, harmonics);
            }
        }
    }

    // ── Schedule tests ───────────────────────────────────────────────

    #[test]
    fn schedule_zero_request_gives_zero() {
        let s = schedule_for_track(120.0, 0, 300);
        assert_eq!(s.duration_ms, 0);
    }

    #[test]
    fn schedule_respects_min_bound() {
        let s = schedule_for_track(120.0, 50, 300);
        assert_eq!(s.duration_ms, 150);
    }

    #[test]
    fn schedule_respects_max_bound() {
        let s = schedule_for_track(120.0, 2000, 300);
        assert_eq!(s.duration_ms, 800);
    }

    #[test]
    fn schedule_aligns_to_quarter_beat() {
        // 120 BPM → beat = 500ms, quarter = 125ms.
        // Request 400ms → should round to nearest multiple of 125.
        let s = schedule_for_track(120.0, 400, 300);
        assert_eq!(s.duration_ms % 125, 0,
                   "duration {} should align to quarter-beat 125", s.duration_ms);
    }

    #[test]
    fn schedule_preserves_arrival_estimate() {
        let s = schedule_for_track(120.0, 400, 350);
        assert_eq!(s.estimated_device_arrival_ms, 350);
    }
}
