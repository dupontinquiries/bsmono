//! level.rs — signal level primitives: RMS, peak, and linear↔dB conversion.
//!
//! `rms` was byte-identical in bo-suite (`podcast_diar`), voiceterm
//! (`separation`), and snapclone-audio — the universal "how loud is this frame"
//! leaf. The dB conversion is the same `20·log10` the spectrogram uses, factored
//! out so level metering, gating, and normalization share one implementation.

/// Root-mean-square level of a frame. `0.0` for an empty frame.
pub fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = frame.iter().map(|&s| s * s).sum();
    (sum_sq / frame.len() as f32).sqrt()
}

/// Peak absolute amplitude of a frame. `0.0` for an empty frame.
pub fn peak(frame: &[f32]) -> f32 {
    frame.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
}

/// Linear amplitude ratio → decibels (`20·log10`), floored at `-120 dB` so
/// silence returns a finite value rather than `-inf` (a `1e-9` epsilon guards
/// `log10(0)`).
pub fn lin_to_db(amp: f32) -> f32 {
    let v = 20.0 * (amp.abs() + 1e-9).log10();
    v.max(-120.0)
}

/// Decibels → linear amplitude ratio.
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Scale `buf` in place so its peak amplitude becomes `target`. No-op on a silent
/// buffer (avoids dividing by zero).
pub fn normalize_peak(buf: &mut [f32], target: f32) {
    let p = peak(buf);
    if p > 1e-12 {
        let g = target / p;
        for s in buf.iter_mut() {
            *s *= g;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_zero_for_silence_and_empty() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.0; 100]), 0.0);
    }

    #[test]
    fn rms_of_full_scale_dc_is_one() {
        assert!((rms(&[1.0; 64]) - 1.0).abs() < 1e-6);
        // ±1 square wave also has RMS 1
        let sq: Vec<f32> = (0..64).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect();
        assert!((rms(&sq) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn peak_finds_largest_magnitude() {
        assert_eq!(peak(&[0.2, -0.9, 0.5]), 0.9);
        assert_eq!(peak(&[]), 0.0);
    }

    #[test]
    fn db_roundtrips_and_floors_silence() {
        assert!((db_to_lin(lin_to_db(0.5)) - 0.5).abs() < 1e-4);
        assert!((lin_to_db(1.0)).abs() < 1e-3, "unity = 0 dB");
        assert_eq!(lin_to_db(0.0), -120.0, "silence floored, not -inf");
    }

    #[test]
    fn normalize_brings_peak_to_target() {
        let mut buf = [0.1, -0.25, 0.05];
        normalize_peak(&mut buf, 1.0);
        assert!((peak(&buf) - 1.0).abs() < 1e-6);
        let mut silent = [0.0; 8];
        normalize_peak(&mut silent, 1.0); // no panic, no change
        assert_eq!(peak(&silent), 0.0);
    }
}
