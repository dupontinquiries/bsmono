//! Perf: batched ambient intensity over sampled sun elevations, bit-identical to ref.

use std::f32::consts::FRAC_PI_2;

pub fn intensity_one(elevation: f32) -> f32 {
    let night_floor = 0.05;
    if elevation <= 0.0 {
        night_floor
    } else {
        let day = (elevation / FRAC_PI_2).clamp(0.0, 1.0);
        (night_floor + (1.0 - night_floor) * day).clamp(0.0, 1.0)
    }
}

pub fn intensity_batch(elevations: &[f32]) -> Vec<f32> {
    elevations.iter().map(|&e| intensity_one(e)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let es = [-0.5f32, 0.0, 0.3, 1.0, FRAC_PI_2];
        let b = intensity_batch(&es);
        for (i, &e) in es.iter().enumerate() { assert_eq!(b[i].to_bits(), intensity_one(e).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(intensity_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(intensity_batch(&[0.5])[0].to_bits(), intensity_one(0.5).to_bits()); }
    #[test] fn night_floor() { assert!((intensity_batch(&[-0.5])[0] - 0.05).abs() < 1e-6); }
    #[test] fn zenith_full() { assert!((intensity_batch(&[FRAC_PI_2])[0] - 1.0).abs() < 1e-5); }
    #[test] fn deterministic() { assert_eq!(intensity_batch(&[0.5]), intensity_batch(&[0.5])); }
}
