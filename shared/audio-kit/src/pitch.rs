//! pitch.rs — fundamental-frequency (pitch) estimation by a Goertzel bin-scan
//! with parabolic interpolation. Lifted from bo-suite's pass-by canceller, where
//! it tracks an engine's fundamental; reusable for voice pitch (voiceterm speaker
//! features, logscope voicing checks) just as well.
//!
//! One change from the lifted original: the canceller used an absolute magnitude
//! floor (tuned for its un-normalized signal levels) to decide "is there a tone
//! here." That floor is wrong for normalized `[-1, 1]` audio, so this version
//! reports the interpolated peak whenever any energy is found and leaves the
//! "is this actually voiced/loud enough" decision to the caller (gate on
//! [`crate::level::rms`] or a voicing test) — the right split for a primitive.

use crate::goertzel::goertzel;

/// Estimate the dominant frequency in `[lo, hi]` Hz. Scans Goertzel at the DFT
/// bin spacing (`sr / n`), picks the strongest bin, then parabolically
/// interpolates across its neighbours for sub-bin precision. Returns `0.0` if the
/// buffer is too short (< 256 samples) or no energy is present in the band.
pub fn estimate_fundamental(samples: &[f32], lo: f32, hi: f32, sr: f32) -> f32 {
    if samples.len() < 256 || sr <= 0.0 {
        return 0.0;
    }
    let n = samples.len() as f32;
    // Scan at the bin width (~23 Hz at 2048 samples) — a finer step would hit the
    // same bin repeatedly.
    let bin_hz = sr / n;
    let mut best_k = 0i32;
    let mut best_mag = 0.0f32;
    let k_lo = (lo / bin_hz).ceil() as i32;
    let k_hi = (hi / bin_hz).floor() as i32;
    for k in k_lo..=k_hi {
        let f = k as f32 * bin_hz;
        let m = goertzel(samples, f, sr);
        if m > best_mag {
            best_mag = m;
            best_k = k;
        }
    }
    if best_mag <= 0.0 {
        return 0.0;
    }
    // Parabolic interpolation across (k-1, k, k+1).
    let f_at = |k: i32| -> f32 { goertzel(samples, k as f32 * bin_hz, sr) };
    let m_minus = if best_k > 0 { f_at(best_k - 1) } else { 0.0 };
    let m_plus = f_at(best_k + 1);
    let m_zero = best_mag;
    let denom = m_minus - 2.0 * m_zero + m_plus;
    let delta = if denom.abs() > 1e-9 {
        (0.5 * (m_minus - m_plus) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    (best_k as f32 + delta) * bin_hz
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: f32 = 48_000.0;

    fn synth_tone(freq: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * PI * freq * i as f32 / SR).sin()).collect()
    }

    #[test]
    fn finds_a_pure_tone_within_a_few_hz() {
        let tone = synth_tone(220.0, 4096);
        let f = estimate_fundamental(&tone, 50.0, 1000.0, SR);
        assert!((f - 220.0).abs() < 5.0, "estimated {f}, expected ~220");
    }

    #[test]
    fn too_short_or_silent_returns_zero() {
        assert_eq!(estimate_fundamental(&[0.0; 64], 50.0, 1000.0, SR), 0.0);
        assert_eq!(estimate_fundamental(&[0.0; 4096], 50.0, 1000.0, SR), 0.0);
        assert_eq!(estimate_fundamental(&synth_tone(220.0, 512), 50.0, 1000.0, 0.0), 0.0);
    }

    #[test]
    fn tracks_a_different_tone() {
        let tone = synth_tone(440.0, 8192);
        let f = estimate_fundamental(&tone, 100.0, 2000.0, SR);
        assert!((f - 440.0).abs() < 5.0, "estimated {f}, expected ~440");
    }
}
