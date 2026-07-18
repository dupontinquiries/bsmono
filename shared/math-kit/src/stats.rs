//! Descriptive statistics over `f32` slices — the mean/variance/median/
//! percentile family re-written in voiceterm, bo-suite (speaker_calibration,
//! rtt_tracker, stream_separator, stem_split), ambient-learner, and
//! snapclone-audio. Empty-slice behavior is explicit: `0.0` for the moments,
//! `None` for the order statistics.

/// Arithmetic mean; `0.0` for an empty slice.
pub fn mean(xs: &[f32]) -> f32 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f32>() / xs.len() as f32
}

/// Population variance (÷ N); `0.0` for an empty slice.
pub fn variance_pop(xs: &[f32]) -> f32 {
    if xs.is_empty() {
        return 0.0;
    }
    let m = mean(xs);
    xs.iter().map(|&x| (x - m) * (x - m)).sum::<f32>() / xs.len() as f32
}

/// Sample variance (÷ N−1, Bessel); `0.0` for fewer than 2 samples.
pub fn variance_sample(xs: &[f32]) -> f32 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    xs.iter().map(|&x| (x - m) * (x - m)).sum::<f32>() / (xs.len() - 1) as f32
}

/// Population standard deviation.
pub fn stddev_pop(xs: &[f32]) -> f32 {
    variance_pop(xs).sqrt()
}

/// Sample standard deviation.
pub fn stddev_sample(xs: &[f32]) -> f32 {
    variance_sample(xs).sqrt()
}

/// `(min, max)`; `None` for an empty slice. NaN-poisoned inputs are the
/// caller's problem (this is `f32::min`/`max` folding).
pub fn min_max(xs: &[f32]) -> Option<(f32, f32)> {
    if xs.is_empty() {
        return None;
    }
    Some(
        xs.iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &x| (lo.min(x), hi.max(x))),
    )
}

/// Percentile by linear interpolation between closest ranks (the numpy
/// `linear` method): `p` in [0, 100]. Sorts a copy; `None` when empty.
pub fn percentile(xs: &[f32], p: f32) -> Option<f32> {
    if xs.is_empty() {
        return None;
    }
    let mut v: Vec<f32> = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p = p.clamp(0.0, 100.0);
    let rank = p / 100.0 * (v.len() - 1) as f32;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let frac = rank - lo as f32;
    Some(v[lo] + (v[hi] - v[lo]) * frac)
}

/// Median = 50th percentile (interpolated for even counts).
pub fn median(xs: &[f32]) -> Option<f32> {
    percentile(xs, 50.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments() {
        let xs = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]; // classic example: μ=5, σ=2
        assert_eq!(mean(&xs), 5.0);
        assert_eq!(variance_pop(&xs), 4.0);
        assert_eq!(stddev_pop(&xs), 2.0);
        assert!((variance_sample(&xs) - 32.0 / 7.0).abs() < 1e-5);
        assert_eq!(mean(&[]), 0.0);
        assert_eq!(variance_sample(&[3.0]), 0.0);
    }

    #[test]
    fn order_statistics() {
        let xs = [15.0, 20.0, 35.0, 40.0, 50.0];
        assert_eq!(percentile(&xs, 0.0), Some(15.0));
        assert_eq!(percentile(&xs, 100.0), Some(50.0));
        assert_eq!(percentile(&xs, 50.0), Some(35.0));
        assert_eq!(percentile(&xs, 25.0), Some(20.0)); // exact rank
        assert_eq!(percentile(&xs, 37.5), Some(27.5)); // interpolated
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
        assert_eq!(median(&[]), None);
        // order-independent
        assert_eq!(percentile(&[50.0, 15.0, 40.0, 20.0, 35.0], 50.0), Some(35.0));
        assert_eq!(min_max(&xs), Some((15.0, 50.0)));
        assert_eq!(min_max(&[]), None);
    }
}
