//! Snow accumulation factor on terrain from temperature, precipitation, and slope:
//! cold + precip accumulates; steep slopes shed snow. In [0,1]. Pure, deterministic.

pub fn cover(temp_norm: f32, precip: f32, slope: f32) -> f32 {
    let t = temp_norm.clamp(0.0, 1.0);
    let p = precip.clamp(0.0, 1.0);
    let s = slope.clamp(0.0, 1.0);
    ((1.0 - t) * p * (1.0 - s)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn warm_no_snow() { assert_eq!(cover(1.0, 1.0, 0.0), 0.0); }
    #[test] fn no_precip_zero() { assert_eq!(cover(0.0, 0.0, 0.0), 0.0); }
    #[test] fn steep_sheds() { assert!(cover(0.0, 1.0, 0.9) < cover(0.0, 1.0, 0.1)); }
    #[test] fn cold_flat_precip_high() { assert!((cover(0.0, 1.0, 0.0) - 1.0).abs() < 1e-6); }
    #[test] fn bounded() {
        for t in [0.0f32, 0.5, 1.0] { for s in [0.0f32, 0.5, 1.0] { let v = cover(t, 0.8, s); assert!((0.0..=1.0).contains(&v)); } }
    }
    #[test] fn deterministic() { assert_eq!(cover(0.2, 0.8, 0.3).to_bits(), cover(0.2, 0.8, 0.3).to_bits()); }
}
