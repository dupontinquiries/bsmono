//! psola — TD-PSOLA pitch-shift resynthesis.
//!
//! `retune` decides *what pitch a frame should become*; this module is what
//! actually moves the audio there. Together they're autotune's brain and
//! hands — this file is the hands.
//!
//! ## Scope, stated plainly
//!
//! What's here operates on a whole buffer at once and preserves its
//! duration (a pure pitch shift, not a time-stretch). It has NOT been
//! adapted to a fixed-size realtime callback: a streaming version needs
//! epochs to carry over correctly across hop boundaries (an epoch found near
//! the end of one hop is also needed to place the first grain of the next),
//! and that continuity is real additional work this pass does not include.
//! What ships here is the harder DSP core — epoch marking and windowed
//! overlap-add grain placement — which a streaming wrapper can be built on
//! top of without redesigning either.
//!
//! ## Why epoch-synchronous grains, not a phase vocoder
//!
//! A phase vocoder pitch-shifts by manipulating the phase spectrum, which is
//! robust but tends to smear transients and add a characteristic "phasiness"
//! on sustained tones — exactly the wrong trade for a vocal, where the whole
//! point is that it should still sound like a voice. TD-PSOLA instead marks
//! the actual pitch periods in the time domain (epochs) and resynthesizes by
//! placing windowed copies of those periods closer together (raising pitch)
//! or farther apart (lowering it). It's cheaper, and because each grain is a
//! real chunk of the original waveform, it keeps the formants — the vocal
//! character — largely intact at moderate shifts, which is the reason this
//! family of algorithms is what vocal-specific pitch correction actually uses.

use std::f32::consts::PI;

/// Locate epoch (pitch-mark) positions across `samples`, given an initial
/// period estimate in samples (from [`crate::retune::yin_pitch`]'s detected
/// Hz) and a search radius for peak correction.
///
/// Starting from `start`, each subsequent epoch is expected near
/// `previous + period`; rather than trusting that blindly (periods drift
/// slightly even within one detected pitch, and drifting the search window
/// by the same fixed amount every time would accumulate the error), each
/// epoch is placed at the local absolute-maximum sample within
/// `search_radius` of that expectation — which is also what re-centers the
/// search for the next epoch, so small period drift is self-correcting
/// rather than compounding.
pub fn mark_epochs(samples: &[f32], start: usize, period: f32, search_radius: usize) -> Vec<usize> {
    let mut epochs = Vec::new();
    if samples.is_empty() || period <= 0.0 {
        return epochs;
    }
    let mut expected = start as f32;
    while (expected.round() as usize) < samples.len() {
        let center = expected.round() as usize;
        let lo = center.saturating_sub(search_radius);
        let hi = (center + search_radius).min(samples.len() - 1);
        let mut best = center.min(samples.len() - 1);
        let mut best_val = f32::MIN;
        for i in lo..=hi {
            let v = samples[i].abs();
            if v > best_val {
                best_val = v;
                best = i;
            }
        }
        epochs.push(best);
        expected += period;
    }
    epochs
}

/// Pitch-shift `samples` by a linear `ratio` (`> 1.0` raises pitch, `< 1.0`
/// lowers it), preserving the buffer's duration. A thin, constant-ratio
/// wrapper over [`psola_shift_varying`] — see that function for how the
/// shift itself works.
pub fn psola_shift(samples: &[f32], analysis_epochs: &[usize], ratio: f32) -> Vec<f32> {
    if ratio <= 0.0 {
        return samples.to_vec();
    }
    psola_shift_varying(samples, analysis_epochs, |_| ratio)
}

/// The general form of [`psola_shift`]: the ratio may vary by output sample
/// position, via `ratio_at(sample_index) -> f32`. This is what
/// [`crate::autotune::AutotuneChannel`] needs — a correction that changes
/// hop to hop as the detected pitch moves — without it having to duplicate
/// the grain-placement loop itself.
///
/// For each output time position, the analysis epoch nearest that SAME time
/// (not a scaled time — duration is preserved, only pitch changes) supplies
/// a Hann-windowed grain roughly two local periods wide, overlap-added into
/// the output at that position; successive grains are placed
/// `local_period / ratio_at(position)` apart, which is what changes the
/// perceived pitch while grains still come from the correct point in time.
/// Output samples are then divided by the accumulated window weight wherever
/// grains overlapped, which is what keeps amplitude roughly constant despite
/// overlap density changing with the ratio — without it, raising pitch
/// (denser grains) would also make the result louder, and lowering it
/// quieter, for no reason connected to the actual signal.
///
/// Returns a buffer the same length as `samples`. `analysis_epochs` with
/// fewer than 2 entries is treated as "nothing to shift": the input is
/// returned unchanged.
pub fn psola_shift_varying(samples: &[f32], analysis_epochs: &[usize], ratio_at: impl Fn(usize) -> f32) -> Vec<f32> {
    let n = samples.len();
    if analysis_epochs.len() < 2 || n == 0 {
        return samples.to_vec();
    }

    let local_period = |idx: usize| -> f32 {
        if idx + 1 < analysis_epochs.len() {
            (analysis_epochs[idx + 1] - analysis_epochs[idx]) as f32
        } else if idx > 0 {
            (analysis_epochs[idx] - analysis_epochs[idx - 1]) as f32
        } else {
            100.0
        }
    };

    let mut out = vec![0.0f32; n];
    let mut weight = vec![0.0f32; n];

    let mut t_out = 0.0f32;
    while (t_out.round() as usize) < n {
        let idx = analysis_epochs.partition_point(|&e| (e as f32) < t_out);
        let idx = idx.min(analysis_epochs.len() - 1);
        let a_epoch = analysis_epochs[idx];
        let period = local_period(idx).max(2.0);
        let half = period.round().max(1.0) as usize;

        let lo = a_epoch.saturating_sub(half);
        let hi = (a_epoch + half).min(n - 1);
        let glen = hi - lo + 1;
        let out_center = t_out.round() as isize;

        if glen >= 4 {
            for (gi, si) in (lo..=hi).enumerate() {
                let w = 0.5 - 0.5 * (2.0 * PI * gi as f32 / (glen as f32 - 1.0)).cos();
                let oi = out_center - half as isize + gi as isize;
                if oi < 0 || oi as usize >= n {
                    continue;
                }
                out[oi as usize] += samples[si] * w;
                weight[oi as usize] += w;
            }
        }

        let ratio = ratio_at(out_center.max(0) as usize).max(0.01);
        let step = (period / ratio).max(1.0);
        t_out += step;
    }

    for i in 0..n {
        if weight[i] > 1e-6 {
            out[i] /= weight[i];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retune::{yin_pitch, DEFAULT_MAX_HZ, DEFAULT_MIN_HZ};

    fn sine(hz: f32, sr: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * PI * hz * i as f32 / sr).sin()).collect()
    }

    #[test]
    fn epochs_land_near_true_peaks_at_roughly_the_right_spacing() {
        let sr = 48_000.0;
        let hz = 200.0;
        let period = sr / hz;
        let samples = sine(hz, sr, 8192);
        // Start the search near the very first peak (a sine's first peak
        // after t=0 is at a quarter period).
        let epochs = mark_epochs(&samples, (period / 4.0).round() as usize, period, (period / 4.0) as usize);
        assert!(epochs.len() > 10, "should find many periods in this buffer");

        for &e in &epochs {
            assert!(samples[e].abs() > 0.9, "epoch at {e} should sit near a peak, got amplitude {}", samples[e]);
        }
        let spacings: Vec<f32> = epochs.windows(2).map(|w| (w[1] - w[0]) as f32).collect();
        let mean_spacing = spacings.iter().sum::<f32>() / spacings.len() as f32;
        assert!((mean_spacing - period).abs() < 1.0, "mean epoch spacing {mean_spacing} should track the true period {period}");
    }

    #[test]
    fn mark_epochs_handles_degenerate_inputs_without_panicking() {
        assert_eq!(mark_epochs(&[], 0, 100.0, 10), Vec::<usize>::new());
        assert_eq!(mark_epochs(&[0.1, 0.2], 0, 0.0, 10), Vec::<usize>::new(), "zero period must not infinite-loop or panic");
        assert_eq!(mark_epochs(&[0.1, 0.2], 0, -5.0, 10), Vec::<usize>::new(), "negative period is rejected the same way");
    }

    #[test]
    fn psola_shift_at_unity_ratio_stays_close_to_the_original() {
        let sr = 48_000.0;
        let hz = 220.0;
        let period = sr / hz;
        let samples = sine(hz, sr, 16_384);
        let epochs = mark_epochs(&samples, (period / 4.0).round() as usize, period, (period / 4.0) as usize);
        let out = psola_shift(&samples, &epochs, 1.0);

        assert_eq!(out.len(), samples.len(), "duration must be preserved");
        assert!(out.iter().all(|s| s.is_finite()));

        let p = yin_pitch(&out[2000..14_000], sr, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15)
            .expect("a ratio-1.0 shift of a clean tone should still read as pitched");
        assert!((p.hz - hz).abs() < 3.0, "unity ratio should reproduce the original pitch closely, got {}", p.hz);

        let peak = |s: &[f32]| s.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let ratio_peak = peak(&out[2000..14_000]) / peak(&samples[2000..14_000]);
        assert!((0.5..2.0).contains(&ratio_peak), "unity shift should not wildly change amplitude, got {ratio_peak}x");
    }

    #[test]
    fn psola_shift_raises_pitch_by_the_requested_ratio() {
        let sr = 48_000.0;
        let hz = 200.0;
        let period = sr / hz;
        let samples = sine(hz, sr, 16_384);
        let epochs = mark_epochs(&samples, (period / 4.0).round() as usize, period, (period / 4.0) as usize);

        let ratio = 1.5;
        let out = psola_shift(&samples, &epochs, ratio);
        assert_eq!(out.len(), samples.len());

        let p = yin_pitch(&out[2000..14_000], sr, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15).expect("shifted tone should still be pitched");
        let expected = hz * ratio;
        assert!((p.hz - expected).abs() < expected * 0.05, "expected ~{expected}Hz (±5%), got {}", p.hz);
    }

    #[test]
    fn psola_shift_lowers_pitch_by_the_requested_ratio() {
        let sr = 48_000.0;
        let hz = 300.0;
        let period = sr / hz;
        let samples = sine(hz, sr, 16_384);
        let epochs = mark_epochs(&samples, (period / 4.0).round() as usize, period, (period / 4.0) as usize);

        let ratio = 0.75;
        let out = psola_shift(&samples, &epochs, ratio);
        let p = yin_pitch(&out[2000..14_000], sr, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15).expect("shifted tone should still be pitched");
        let expected = hz * ratio;
        assert!((p.hz - expected).abs() < expected * 0.05, "expected ~{expected}Hz (±5%), got {}", p.hz);
    }

    #[test]
    fn fewer_than_two_epochs_or_a_non_positive_ratio_returns_the_input_unchanged() {
        let samples = vec![0.1, 0.2, -0.3, 0.4];
        assert_eq!(psola_shift(&samples, &[], 1.5), samples);
        assert_eq!(psola_shift(&samples, &[5], 1.5), samples, "one epoch alone has no period to shift by");
        assert_eq!(psola_shift(&samples, &[2, 5, 8], 0.0), samples);
        assert_eq!(psola_shift(&samples, &[2, 5, 8], -1.0), samples);
    }

    #[test]
    fn psola_shift_never_produces_nan_or_a_length_mismatch_even_on_short_buffers() {
        let sr = 48_000.0;
        let samples = sine(150.0, sr, 200); // shorter than even one full period search
        let epochs = mark_epochs(&samples, 0, sr / 150.0, 20);
        for ratio in [0.5, 1.0, 2.0, 3.0] {
            let out = psola_shift(&samples, &epochs, ratio);
            assert_eq!(out.len(), samples.len());
            assert!(out.iter().all(|s| s.is_finite()), "ratio {ratio} produced non-finite output on a short buffer");
        }
    }
}
