//! `batch_frost_spread` — batched frost coverage (perf bonus).
//!
//! Worker-03 / WORLD pure core, round 25. Evaluates frost coverage for many
//! `(temp, humidity)` samples in one call, producing **bit-identical** values to
//! the scalar reference (a verbatim mirror of R21 `frost_spread`:
//! `clamp01((1 − clamp01(temp)) · clamp01(humidity))`).
//!
//! PERF: frost coverage is sampled per surface cell when updating winter
//! material/albedo (CHUNK_SIZE² per chunk). Batching walks a contiguous sample
//! slice; because each sample runs the exact scalar `frost_one`, coverage is
//! bit-for-bit equal to per-cell calls — a batched pass renders the same frost
//! as a scalar fallback.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Scalar reference — identical to `frost_spread::frost` (R21).
pub fn frost_one(temp: f32, humidity: f32) -> f32 {
    let t = clamp01(temp);
    let h = clamp01(humidity);
    let cold = 1.0 - t;
    clamp01(cold * h)
}

/// Batched frost coverage over `(temp, humidity)` samples.
/// Bit-identical to calling `frost_one` per sample.
pub fn frost_batch(samples: &[(f32, f32)]) -> Vec<f32> {
    samples.iter().map(|&(t, h)| frost_one(t, h)).collect()
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let s = [
            (0.0f32, 1.0),
            (1.0, 1.0),
            (0.3, 0.6),
            (-1.0, 2.0), // clamps
            (0.5, 0.5),
        ];
        let batch = frost_batch(&s);
        for (i, &(t, h)) in s.iter().enumerate() {
            assert_eq!(batch[i].to_bits(), frost_one(t, h).to_bits(), "bit drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(frost_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = frost_one(0.2, 0.7);
        let batch = frost_batch(&[(0.2, 0.7)]);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].to_bits(), one.to_bits());
    }

    #[test]
    fn bounded_unit_interval() {
        for v in frost_batch(&[(0.0, 1.0), (1.0, 1.0), (0.5, 0.5), (-2.0, 3.0)]) {
            assert!(v >= 0.0 && v <= 1.0);
        }
    }

    #[test]
    fn deterministic() {
        let s = [(0.3f32, 0.7), (0.3, 0.7)];
        assert_eq!(frost_batch(&s), frost_batch(&s));
    }

    #[test]
    fn cold_humid_max_warm_zero_parity() {
        let s = [(0.0f32, 1.0), (1.0, 1.0)];
        let b = frost_batch(&s);
        assert!((b[0] - 1.0).abs() < 1e-6); // cold + humid
        assert_eq!(b[1], 0.0); // warm
        assert_eq!(b[0].to_bits(), frost_one(0.0, 1.0).to_bits());
    }
}
