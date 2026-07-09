//! fingerprint — a fast, compact acoustic fingerprint of an utterance, for skipping whisper on
//! repeated speech. Built on [`crate::goertzel::spectrogram`] (no new DSP): a band-energy
//! spectrogram is time-normalized to a fixed grid and reduced to a 64-bit signature. Cheap to
//! compute and to compare (XOR + popcount), and stable enough that the SAME word spoken twice lands
//! within a small Hamming distance — so a duplicate can reuse a prior transcription instead of
//! paying whisper's cost again. Used by rusty-board (keyboard) and any voice surface via voice-kit's
//! cache; the fingerprint stays here where the signal math lives.
use crate::goertzel::{spectrogram, SpectrogramConfig};

/// Frequency bands (speech-band) — one bit column each.
const BANDS: usize = 8;
/// Time buckets the utterance is normalized to — one bit row each. BANDS × BUCKETS = 64 bits.
const BUCKETS: usize = 8;

/// A 64-bit acoustic fingerprint. `0` means "too short / silent to fingerprint" — callers should not
/// cache or match on it.
pub fn fingerprint(samples: &[f32], sr: f32) -> u64 {
    let cfg = SpectrogramConfig {
        window_samples: 512,
        hop_samples: 256,
        freq_lo_hz: 300.0,
        freq_hi_hz: 3400.0, // telephone speech band
        n_bins: BANDS,
    };
    let spec = spectrogram(samples, sr, cfg); // [n_frames][BANDS] log-magnitude
    if spec.is_empty() {
        return 0;
    }
    // Time-normalize: average-pool the frames into BUCKETS, so utterances of different length map to
    // the same 8-row grid (short repeats still align).
    let n = spec.len();
    let mut buckets = [[0f32; BANDS]; BUCKETS];
    let mut counts = [0usize; BUCKETS];
    for (i, row) in spec.iter().enumerate() {
        let b = ((i * BUCKETS) / n).min(BUCKETS - 1);
        for k in 0..BANDS {
            buckets[b][k] += row.get(k).copied().unwrap_or(0.0);
        }
        counts[b] += 1;
    }
    for b in 0..BUCKETS {
        if counts[b] > 0 {
            for k in 0..BANDS {
                buckets[b][k] /= counts[b] as f32;
            }
        }
    }
    // Bit per (bucket, band): set when that band is above the bucket's mean energy — a spectral
    // shape descriptor that's robust to overall loudness.
    let mut fp = 0u64;
    let mut bit = 0u32;
    for b in 0..BUCKETS {
        let mean: f32 = buckets[b].iter().sum::<f32>() / BANDS as f32;
        for k in 0..BANDS {
            if buckets[b][k] > mean {
                fp |= 1u64 << bit;
            }
            bit += 1;
        }
    }
    fp
}

/// Hamming distance between two fingerprints (differing bits).
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Do two fingerprints likely come from the same utterance? (Within `tol` bits, and neither is the
/// degenerate `0`.)
pub fn similar(a: u64, b: u64, tol: u32) -> bool {
    a != 0 && b != 0 && hamming(a, b) <= tol
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, secs: f32, sr: f32) -> Vec<f32> {
        let n = (secs * sr) as usize;
        (0..n).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin() * 0.5).collect()
    }

    #[test]
    fn same_utterance_fingerprints_identically() {
        let sr = 16_000.0;
        let a = sine(440.0, 0.4, sr);
        let b = sine(440.0, 0.4, sr);
        let (fa, fb) = (fingerprint(&a, sr), fingerprint(&b, sr));
        assert_ne!(fa, 0);
        assert_eq!(fa, fb, "identical audio → identical fingerprint");
        assert!(similar(fa, fb, 0));
    }

    #[test]
    fn a_light_perturbation_stays_within_tolerance() {
        let sr = 16_000.0;
        let clean = sine(600.0, 0.4, sr);
        // same word, slightly noised + amplitude-shifted (a realistic repeat)
        let noisy: Vec<f32> = clean.iter().enumerate()
            .map(|(i, &s)| s * 0.9 + ((i as f32 * 12.9898).sin() * 43758.547).fract() * 0.02 - 0.01)
            .collect();
        let (fc, fn_) = (fingerprint(&clean, sr), fingerprint(&noisy, sr));
        assert!(similar(fc, fn_, 8), "a light perturbation stays a match: hamming={}", hamming(fc, fn_));
    }

    #[test]
    fn different_utterances_do_not_match() {
        let sr = 16_000.0;
        // a rising sweep vs a falling sweep — clearly different spectro-temporal shapes
        let n = (0.4 * sr) as usize;
        let up: Vec<f32> = (0..n).map(|i| {
            let f = 300.0 + 3000.0 * i as f32 / n as f32;
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin() * 0.5
        }).collect();
        let down: Vec<f32> = (0..n).map(|i| {
            let f = 3300.0 - 3000.0 * i as f32 / n as f32;
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin() * 0.5
        }).collect();
        let (fu, fd) = (fingerprint(&up, sr), fingerprint(&down, sr));
        assert!(!similar(fu, fd, 8), "different shapes must not match: hamming={}", hamming(fu, fd));
    }

    #[test]
    fn too_short_is_zero_and_never_matches() {
        assert_eq!(fingerprint(&[0.0; 16], 16_000.0), 0);
        assert!(!similar(0, 0, 64), "degenerate fingerprints never match");
    }
}
