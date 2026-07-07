//! Lightning-arc jitter offset for a shock VFX: deterministic pseudo-random
//! displacement in [−amplitude, amplitude] from (t, seed). Pure.

pub fn jitter(t: f32, seed: u32, amplitude: f32) -> f32 {
    let amp = amplitude.max(0.0);
    let mut x = t.to_bits()
        .wrapping_mul(2654435761)
        .wrapping_add(seed.wrapping_mul(2246822519));
    x ^= x >> 15;
    x = x.wrapping_mul(2246822519);
    x ^= x >> 13;
    let n = (x as f32 / u32::MAX as f32) * 2.0 - 1.0;
    amp * n
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_amp_zero() { assert_eq!(jitter(1.0, 7, 0.0), 0.0); }
    #[test] fn bounded() { assert!(jitter(2.3, 9, 5.0).abs() <= 5.0 + 1e-5); }
    #[test] fn deterministic() { assert_eq!(jitter(1.0, 7, 5.0).to_bits(), jitter(1.0, 7, 5.0).to_bits()); }
    #[test] fn t_differs() { assert_ne!(jitter(1.0, 7, 5.0), jitter(2.0, 7, 5.0)); }
    #[test] fn seed_differs() { assert_ne!(jitter(1.0, 7, 5.0), jitter(1.0, 8, 5.0)); }
    #[test] fn amplitude_scales() { assert!((jitter(1.0, 7, 10.0).abs() - 2.0 * jitter(1.0, 7, 5.0).abs()).abs() < 1e-4); }
}
