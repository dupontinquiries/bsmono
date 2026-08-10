//! export — dithered WAV output.
//!
//! `mixdown` produces `f32` samples; a deliverable file needs a fixed bit
//! depth. Rounding straight to 16-bit integers is the wrong way to get
//! there: on a quiet passage or a sustained tone whose true value sits
//! between two quantization levels, plain rounding produces the SAME
//! rounding decision every sample, which is a periodic artifact — audible as
//! a tone or "birdie" riding on the quantization noise floor, not random
//! noise. Adding **TPDF dither** (the sum of two independent uniform random
//! values, triangular probability density, ±1 LSB peak-to-peak) before
//! rounding breaks that periodicity: the rounding decision becomes
//! effectively random rather than a fixed function of the sample value,
//! which trades a fixed, audible artifact for an unstructured, much less
//! audible noise floor at the same level. This is standard practice, not
//! novel — the point of doing it here is that skipping it is a real,
//! audible defect a "just write the samples" export would have.

use std::path::Path;

/// Dither and quantize `samples` (already at unity full-scale = ±1.0) to
/// 16-bit PCM. Deterministic given `seed` — the same input and seed always
/// produce the same output, which matters for reproducible bounces and for
/// testing this function at all.
///
/// TPDF dither is the sum of two independent uniform `[-0.5, 0.5)` LSB
/// values (triangular density, ±1 LSB peak-to-peak) added before rounding —
/// two independent draws rather than one uniform draw is what makes it
/// *triangular* rather than rectangular, which is the shape that avoids
/// adding its own audible modulation on top of the signal.
pub fn tpdf_dither_and_quantize_i16(samples: &[f32], seed: u64) -> Vec<i16> {
    let mut rng = core_kit::rng::SplitMix64::new(seed);
    samples
        .iter()
        .map(|&x| {
            let d1 = rng.next_f64() as f32 - 0.5;
            let d2 = rng.next_f64() as f32 - 0.5;
            let dither_lsb = d1 + d2; // triangular, range [-1, 1) LSB
            let scaled = (x.clamp(-1.0, 1.0) * i16::MAX as f32) + dither_lsb;
            scaled.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
        })
        .collect()
}

/// Write `samples` (interleaved if `channels > 1`) to a 16-bit PCM WAV file
/// at `path`, with TPDF dither applied. `seed` is exposed (rather than
/// picked internally) so a caller that needs a reproducible bounce — a test,
/// or an A/B comparison export — can ask for the exact same dither pattern
/// twice.
pub fn export_wav_dithered(
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
    seed: u64,
    path: &Path,
) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for s in tpdf_dither_and_quantize_i16(samples, seed) {
        writer.write_sample(s)?;
    }
    writer.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peak_dbfs;

    #[test]
    fn dither_output_stays_within_its_own_specified_bound() {
        // TPDF dither's own amplitude is at most ±1 LSB (two ±0.5 LSB draws
        // summed) — quantized output must never deviate from the nearest
        // representable value by more than that (plus a hair of rounding
        // slack), or the dither generator itself is broken.
        let samples: Vec<f32> = (0..10_000).map(|i| ((i as f32 * 0.01).sin()) * 0.7).collect();
        let quantized = tpdf_dither_and_quantize_i16(&samples, 1);
        for (i, (&x, &q)) in samples.iter().zip(quantized.iter()).enumerate() {
            let ideal = (x * i16::MAX as f32).round();
            let deviation = (q as f32 - ideal).abs();
            assert!(deviation <= 1.5, "sample {i}: dither deviated {deviation} LSB from the plain-rounded value, exceeding TPDF's ±1 LSB bound");
        }
    }

    #[test]
    fn dither_is_deterministic_given_the_same_seed() {
        let samples: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.02).sin() * 0.3).collect();
        let a = tpdf_dither_and_quantize_i16(&samples, 42);
        let b = tpdf_dither_and_quantize_i16(&samples, 42);
        assert_eq!(a, b, "the same seed must reproduce identical dithered output for a reproducible bounce");
        let c = tpdf_dither_and_quantize_i16(&samples, 43);
        assert_ne!(a, c, "a different seed should (with overwhelming probability) produce a different dither pattern");
    }

    #[test]
    fn dither_breaks_up_the_periodic_pattern_plain_rounding_produces_on_a_sub_lsb_signal() {
        // A DC value that sits exactly at a half-LSB boundary rounds the
        // SAME way every single time under plain rounding — the classic
        // "birdie" artifact. Dithered, consecutive quantized values should
        // vary rather than all being identical.
        let half_lsb = 0.5 / i16::MAX as f32;
        let samples = vec![half_lsb; 5000];
        let quantized = tpdf_dither_and_quantize_i16(&samples, 7);
        let distinct: std::collections::BTreeSet<i16> = quantized.iter().copied().collect();
        assert!(distinct.len() > 1, "a constant sub-LSB signal should NOT quantize to the same value every sample once dithered");
    }

    #[test]
    fn a_signal_already_at_the_export_ceiling_does_not_clip_after_dither() {
        // Simulate a mastering chain that has already limited to a ceiling
        // (e.g. -0.3dBTP) — dither must not be enough to push samples over
        // 0dBFS, which would be an audible click on export the mastering
        // stage never intended.
        let ceiling_linear = 10f32.powf(-0.3 / 20.0); // -0.3dBFS peak
        let samples: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.05).sin() * ceiling_linear).collect();
        let quantized = tpdf_dither_and_quantize_i16(&samples, 3);
        let peak = quantized.iter().map(|&s| s.unsigned_abs()).max().unwrap();
        assert!(peak <= i16::MAX as u16, "quantized peak must never exceed full scale, got {peak}");
        let peak_dbfs_out = 20.0 * (peak as f32 / i16::MAX as f32).log10();
        assert!(peak_dbfs_out < 0.0, "a signal already at -0.3dBTP must not read as clipping (>=0dBFS) after dither, got {peak_dbfs_out}dBFS");
        assert!((peak_dbfs_out - (-0.3)).abs() < 0.1, "should still read close to the original -0.3dBFS ceiling, got {peak_dbfs_out}dBFS");
    }

    #[test]
    fn export_and_read_back_round_trips_close_to_the_original_peak() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("studio_export_test_{}.wav", std::process::id()));
        let samples: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.03).sin() * 0.6).collect();
        export_wav_dithered(&samples, 1, 48_000, 5, &path).expect("export should succeed");

        let mut reader = hound::WavReader::open(&path).expect("should read back the file we just wrote");
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().bits_per_sample, 16);

        let read_back: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / i16::MAX as f32)
            .collect();
        assert_eq!(read_back.len(), samples.len());

        let original_peak = peak_dbfs(&samples);
        let readback_peak = peak_dbfs(&read_back);
        assert!((original_peak - readback_peak).abs() < 0.1,
            "round-tripping through 16-bit export should preserve peak level closely: {original_peak}dBFS vs {readback_peak}dBFS");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn silence_exports_and_reads_back_as_silence() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("studio_export_silence_test_{}.wav", std::process::id()));
        let samples = vec![0.0f32; 4800];
        export_wav_dithered(&samples, 1, 48_000, 1, &path).expect("export should succeed");

        let mut reader = hound::WavReader::open(&path).unwrap();
        let read_back: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        // Dither still applies to silence — it's not exactly zero, but it
        // must stay within the dither's own tiny bound, not audible hiss.
        assert!(read_back.iter().all(|&s| s.abs() <= 1), "dithered silence should stay within ±1 LSB, not introduce audible noise");

        let _ = std::fs::remove_file(&path);
    }
}
