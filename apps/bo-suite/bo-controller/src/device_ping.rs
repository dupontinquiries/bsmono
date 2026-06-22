// src/device_ping.rs
#![allow(dead_code)]
//
// Device identification "ping": a short, distinctive sound that plays
// ON the target device (not the controller) so the user can hear which
// physical speaker they're tapping. Standard pattern in multi-speaker
// systems — AirPlay does it, Sonos does it. B&O didn't ship this in
// the stock app; we add it.
//
// Design choices:
//
//   * Two-tone "perfect fifth" (A4 + E5, 440 Hz + ~659 Hz). Consonant,
//     pleasant, recognizable, doesn't sound alarming or system-y.
//   * Mid-range so any speaker reproduces it well. The pre_buffer cue
//     deliberately stays in the bass to crossfade with music; this
//     ping does the opposite — clear and prominent, the listener
//     SHOULD locate it spatially.
//   * Same WAV used on every device in the cluster — variance is the
//     room the sound comes from, not the sound itself.
//   * ~600 ms total: long enough to register in a noisy room, short
//     enough that pinging multiple devices in sequence (the "identify
//     all" pattern) doesn't drag.
//
// Output format matches the rest of the pipeline: 48 kHz mono 16-bit
// PCM, wrapped as RIFF/WAV. The device's network-audio path accepts
// WAV directly, so we expose the bytes via track_server and POST a
// PlayResolvedUrl just like any track.

use std::f32::consts::PI;

pub const SAMPLE_RATE: u32 = 48_000;

/// Mid-octave tones — A4 (440 Hz) and E5 (~659 Hz). A perfect fifth
/// apart, so the interval reads as "consonant" rather than "alarm".
pub const TONE_A_HZ: f32 = 440.0;
pub const TONE_B_HZ: f32 = 659.25;

/// Durations summing to ~600 ms. The smoothstep attack avoids any click
/// at the start of each tone; the exponential decay lets the tail blend
/// with room reflections rather than cutting hard.
pub const TONE_DURATION_MS: u32 = 220;
pub const GAP_MS: u32 = 60;
pub const TAIL_MS: u32 = 120;

/// Synthesize the identification ping as raw 16-bit mono PCM.
/// Total length: 2 × TONE_DURATION_MS + GAP_MS + TAIL_MS ≈ 620 ms.
pub fn synthesize_ping_pcm() -> Vec<u8> {
    let total_ms = 2 * TONE_DURATION_MS + GAP_MS + TAIL_MS;
    let n_total = (SAMPLE_RATE as u64 * total_ms as u64 / 1000) as usize;
    let n_tone = (SAMPLE_RATE as u64 * TONE_DURATION_MS as u64 / 1000) as usize;
    let n_gap = (SAMPLE_RATE as u64 * GAP_MS as u64 / 1000) as usize;
    let n_tail = (SAMPLE_RATE as u64 * TAIL_MS as u64 / 1000) as usize;

    let mut out = Vec::with_capacity(n_total * 2);

    // Phase 1: first tone (A4)
    for i in 0..n_tone {
        let t = i as f32 / SAMPLE_RATE as f32;
        let env = tone_envelope(i as f32 / n_tone as f32);
        let s = env * (2.0 * PI * TONE_A_HZ * t).sin() * 0.45;
        let v = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    // Phase 2: silent gap
    for _ in 0..n_gap {
        out.extend_from_slice(&[0u8, 0u8]);
    }
    // Phase 3: second tone (E5) — same envelope, different freq. We
    // continue the t-clock so the phase relationship across tones is
    // smooth rather than restarting at 0 (which would click).
    let phase_offset = (n_tone + n_gap) as f32 / SAMPLE_RATE as f32;
    for i in 0..n_tone {
        let t = phase_offset + (i as f32 / SAMPLE_RATE as f32);
        let env = tone_envelope(i as f32 / n_tone as f32);
        let s = env * (2.0 * PI * TONE_B_HZ * t).sin() * 0.45;
        let v = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    // Phase 4: exponential decay tail. Carries the second tone's
    // frequency content into silence so the ping doesn't cut abruptly.
    let tail_phase_offset = (2 * n_tone + n_gap) as f32 / SAMPLE_RATE as f32;
    for i in 0..n_tail {
        let t = tail_phase_offset + (i as f32 / SAMPLE_RATE as f32);
        let x = i as f32 / n_tail as f32;
        let env = (-x * 6.0).exp() * 0.3;
        let s = env * (2.0 * PI * TONE_B_HZ * t).sin();
        let v = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Synthesize and wrap as a mono WAV ready to serve via track_server.
pub fn synthesize_ping_wav() -> Vec<u8> {
    let pcm = synthesize_ping_pcm();
    wrap_mono_wav(&pcm, SAMPLE_RATE, 16)
}

/// Per-tone envelope: smoothstep attack (first 15%), level body (middle
/// 70%), smoothstep release (last 15%). Pure function so it's easy to
/// reason about and test.
fn tone_envelope(t_norm: f32) -> f32 {
    let t = t_norm.clamp(0.0, 1.0);
    if t < 0.15 {
        let x = t / 0.15;
        x * x * (3.0 - 2.0 * x)   // smoothstep up
    } else if t < 0.85 {
        1.0   // body
    } else {
        let x = (1.0 - t) / 0.15;
        x * x * (3.0 - 2.0 * x)   // smoothstep down
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
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&sample_rate.to_le_bytes());
    w.extend_from_slice(&byte_rate.to_le_bytes());
    w.extend_from_slice(&block_align.to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    w.extend_from_slice(samples);
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_starts_and_ends_at_zero() {
        assert_eq!(tone_envelope(0.0), 0.0);
        assert_eq!(tone_envelope(1.0), 0.0);
    }

    #[test]
    fn envelope_peaks_in_body() {
        assert!((tone_envelope(0.5) - 1.0).abs() < 0.01);
        assert!((tone_envelope(0.30) - 1.0).abs() < 0.01);
        assert!((tone_envelope(0.70) - 1.0).abs() < 0.01);
    }

    #[test]
    fn envelope_attack_is_monotonic_increasing() {
        let mut prev = 0.0;
        for i in 0..150 {
            let t = i as f32 / 1000.0;
            let v = tone_envelope(t);
            assert!(v >= prev - 0.001, "decrease at t={}: {}→{}", t, prev, v);
            prev = v;
        }
    }

    #[test]
    fn envelope_release_is_monotonic_decreasing() {
        let mut prev = 1.0;
        for i in 850..=1000 {
            let t = i as f32 / 1000.0;
            let v = tone_envelope(t);
            assert!(v <= prev + 0.001, "increase at t={}: {}→{}", t, prev, v);
            prev = v;
        }
    }

    #[test]
    fn ping_pcm_has_expected_length() {
        let pcm = synthesize_ping_pcm();
        let total_ms = 2 * TONE_DURATION_MS + GAP_MS + TAIL_MS;
        let expected_bytes = (SAMPLE_RATE as u64 * total_ms as u64 / 1000) as usize * 2;
        assert_eq!(pcm.len(), expected_bytes);
    }

    #[test]
    fn ping_pcm_stays_in_int16_range() {
        let pcm = synthesize_ping_pcm();
        for chunk in pcm.chunks_exact(2) {
            let s = i16::from_le_bytes([chunk[0], chunk[1]]);
            assert!(s.abs() < 32_000, "sample {} too hot", s);
        }
    }

    #[test]
    fn ping_has_silent_gap() {
        let pcm = synthesize_ping_pcm();
        let samples: Vec<i16> = pcm.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let n_tone = (SAMPLE_RATE as u64 * TONE_DURATION_MS as u64 / 1000) as usize;
        let n_gap = (SAMPLE_RATE as u64 * GAP_MS as u64 / 1000) as usize;
        // The middle of the gap region should be silent.
        let mid_gap = n_tone + n_gap / 2;
        assert_eq!(samples[mid_gap], 0,
                   "expected silence at gap midpoint, got {}", samples[mid_gap]);
    }

    #[test]
    fn ping_concentrates_energy_at_two_tones() {
        let pcm = synthesize_ping_pcm();
        let samples: Vec<f32> = pcm.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_767.0)
            .collect();
        let n_tone = (SAMPLE_RATE as u64 * TONE_DURATION_MS as u64 / 1000) as usize;
        let n_gap = (SAMPLE_RATE as u64 * GAP_MS as u64 / 1000) as usize;
        let sr = SAMPLE_RATE as f32;

        // First tone region: correlate with TONE_A_HZ
        let first = &samples[..n_tone];
        let mut corr_a = 0.0_f32;
        for (i, s) in first.iter().enumerate() {
            let r = (2.0 * PI * TONE_A_HZ * (i as f32) / sr).sin();
            corr_a += s * r;
        }

        // Second tone region: correlate with TONE_B_HZ
        let second = &samples[n_tone + n_gap .. 2 * n_tone + n_gap];
        let mut corr_b = 0.0_f32;
        for (i, s) in second.iter().enumerate() {
            let i_global = n_tone + n_gap + i;
            let r = (2.0 * PI * TONE_B_HZ * (i_global as f32) / sr).sin();
            corr_b += s * r;
        }

        assert!(corr_a > 100.0,
                "first tone should correlate with TONE_A, got {}", corr_a);
        assert!(corr_b > 100.0,
                "second tone should correlate with TONE_B, got {}", corr_b);
    }

    #[test]
    fn ping_wav_has_correct_riff_header() {
        let wav = synthesize_ping_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        // Sample rate at byte offset 24
        let sr = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]);
        assert_eq!(sr, SAMPLE_RATE);
    }

    #[test]
    fn ping_tail_decays_to_near_silence() {
        let pcm = synthesize_ping_pcm();
        let samples: Vec<i16> = pcm.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        // Last 50 samples should be very quiet (exp decay).
        let tail: i32 = samples[samples.len() - 50..].iter()
            .map(|s| (*s as i32).abs())
            .sum();
        let avg = tail / 50;
        assert!(avg < 200, "tail should decay near silence, avg amplitude {}", avg);
    }
}
