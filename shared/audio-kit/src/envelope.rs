//! envelope — signal energy at one value per millisecond.
//!
//! The domain that level-based decisions belong in, as opposed to raw samples.
//! Three properties, and all three are the reason rather than a bonus:
//!
//! * **Rate-agnostic to within the hop rounding.** A 44.1 kHz take and a 48 kHz
//!   reference land on the same time axis, so a threshold in milliseconds means
//!   the same thing against both — whereas a threshold in SAMPLES has to be
//!   converted at every call site, and one of them eventually is not.
//!
//!   The rounding is worth stating because it is not zero: the hop is
//!   `sr / 1000` in INTEGER samples, so 44100 gives 44 rather than 44.1 and
//!   each step is 0.998 ms. That is 0.23% fast — about 2 ms over half a second,
//!   1.4 s over a ten-minute session. Fine for segmentation and for
//!   correlation over a short window; NOT a clock, and not something to
//!   accumulate across a long file. `studio-audio::envelope_ms` has the same
//!   behaviour, which is deliberate (see below) rather than a bug inherited.
//!
//!   Conversion back through [`hop_samples`] uses the same integer hop, so the
//!   ms → samples round trip is exact even though the ms axis is not.
//! * **Phase-blind.** Two microphones on one performance correlate here even
//!   when their waveforms do not.
//! * **A thousand times cheaper** than working at sample rate, which matters
//!   when the operation is a correlation sweep rather than a single pass.
//!
//! `apps/studio/crates/studio-audio` arrived at the same function
//! independently (`envelope_ms`, with `align_offset_ms` correlating over it),
//! and `games/shared/linebank` needed it for silence segmentation. That is the
//! third occurrence, which is what this crate exists to absorb — the header
//! notes the structural-clone scan that put the largest duplication in the repo
//! right here.
//!
//! The semantics match `studio-audio::envelope_ms` EXACTLY — mean rectified
//! amplitude over a `sr / 1000` hop — so studio can adopt this without any
//! behaviour change. That was a constraint on the design, not an accident: an
//! extraction that shifts numbers is not an extraction, it is a rewrite that
//! silently invalidates the caller's tuning.

/// Energy envelope at one value per millisecond: mean rectified amplitude over
/// each `sr / 1000` sample hop.
///
/// A zero sample rate yields an empty envelope rather than dividing by zero;
/// durations are meaningless without a rate and a nonsense answer is worse
/// than none.
pub fn envelope_ms(samples: &[f32], sr: u32) -> Vec<f32> {
    if sr == 0 || samples.is_empty() {
        return Vec::new();
    }
    let hop = (sr / 1000).max(1) as usize;
    samples
        .chunks(hop)
        .map(|c| c.iter().map(|s| s.abs()).sum::<f32>() / c.len() as f32)
        .collect()
}

/// Envelope with an explicit hop, for callers that want a coarser or finer
/// grid than one millisecond.
pub fn envelope_hop(samples: &[f32], hop: usize) -> Vec<f32> {
    let hop = hop.max(1);
    samples
        .chunks(hop)
        .map(|c| c.iter().map(|s| s.abs()).sum::<f32>() / c.len() as f32)
        .collect()
}

/// Samples per envelope step for a rate — the conversion back to a
/// sample-accurate cut point.
///
/// Detection wants milliseconds; a CUT wants samples, because an encoder is
/// handed a sample range and a millisecond boundary would be a rounding error
/// baked into an asset.
pub fn hop_samples(sr: u32) -> usize {
    (sr / 1000).max(1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(secs: f32, amp: f32, sr: u32) -> Vec<f32> {
        let n = (secs * sr as f32) as usize;
        (0..n).map(|i| amp * (i as f32 * 0.05).sin()).collect()
    }

    #[test]
    fn one_value_per_millisecond() {
        let s = sine(0.25, 0.5, 48_000);
        assert_eq!(envelope_ms(&s, 48_000).len(), 250);
        let s = sine(0.25, 0.5, 44_100);
        // 44100/1000 = 44 samples per hop, so 11025 samples -> 251 chunks
        assert!((envelope_ms(&s, 44_100).len() as i32 - 250).abs() <= 1);
    }

    #[test]
    fn the_same_signal_at_two_rates_lands_on_the_same_axis_within_the_hop_rounding() {
        // THE REASON THIS IS THE RIGHT DOMAIN, and the size of its one caveat.
        // A first version of this test asserted the two lengths were within 1
        // and failed at 500 vs 502: the integer hop makes 44.1k run 0.23%
        // fast. Loosening the tolerance would have hidden that; asserting the
        // drift is BOUNDED AND KNOWN keeps it visible, and fails if the hop
        // arithmetic ever changes shape.
        let a = envelope_ms(&sine(0.5, 0.4, 48_000), 48_000);
        let b = envelope_ms(&sine(0.5, 0.4, 44_100), 44_100);
        let drift = (b.len() as f32 - a.len() as f32).abs() / a.len() as f32;
        assert!(drift < 0.005, "axes differ by {:.3}% ({} vs {})", drift * 100.0, a.len(), b.len());
        // 48k divides evenly, so it is the exact one
        assert_eq!(a.len(), 500, "the 48 kHz axis should be exactly one per ms");
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!((mean(&a) - mean(&b)).abs() < 0.02, "{} vs {}", mean(&a), mean(&b));
    }

    #[test]
    fn the_ms_to_samples_round_trip_is_exact_even_though_the_axis_is_not() {
        // What segmentation actually needs: detect on the ms grid, cut on a
        // sample. Both directions use the same integer hop, so a boundary
        // found at step N is exactly N * hop samples with no accumulated error.
        for sr in [8_000u32, 44_100, 48_000, 96_000] {
            let hop = hop_samples(sr);
            let s = sine(0.2, 0.5, sr);
            let env = envelope_ms(&s, sr);
            for step in [0usize, 1, 17, env.len().saturating_sub(1)] {
                assert!(step * hop <= s.len(), "step {step} at {sr} Hz ran past the buffer");
            }
        }
    }

    #[test]
    fn it_matches_studio_audios_semantics_exactly() {
        // The extraction constraint: studio-audio::envelope_ms must be able to
        // call this and see no change. Reproduced here rather than imported,
        // because audio-kit does not depend on an app.
        let studio = |samples: &[f32], rate: u32| -> Vec<f32> {
            let hop = (rate / 1000).max(1) as usize;
            samples
                .chunks(hop)
                .map(|c| c.iter().map(|s| s.abs()).sum::<f32>() / c.len() as f32)
                .collect()
        };
        let s = sine(0.3, 0.6, 48_000);
        assert_eq!(envelope_ms(&s, 48_000), studio(&s, 48_000));
    }

    #[test]
    fn silence_is_low_and_signal_is_high_by_a_wide_margin() {
        let quiet = envelope_ms(&sine(0.2, 0.0008, 48_000), 48_000);
        let loud = envelope_ms(&sine(0.2, 0.6, 48_000), 48_000);
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!(mean(&loud) / mean(&quiet) > 100.0, "not separable");
    }

    #[test]
    fn a_zero_rate_or_empty_input_yields_nothing_rather_than_panicking() {
        assert!(envelope_ms(&sine(0.1, 0.5, 48_000), 0).is_empty());
        assert!(envelope_ms(&[], 48_000).is_empty());
        assert!(envelope_hop(&[], 0).is_empty());
    }

    #[test]
    fn hop_samples_is_the_way_back_to_a_sample_accurate_cut() {
        assert_eq!(hop_samples(48_000), 48);
        assert_eq!(hop_samples(44_100), 44);
        // never zero, however small the rate
        assert_eq!(hop_samples(100), 1);
        assert_eq!(hop_samples(0), 1);
    }
}
