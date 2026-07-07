//! Fear intensity from a roar on a nearby mob: proximity-attenuated power reduced
//! by the target's resolve. In [0,1]. Pure, deterministic.

pub fn fear(distance: f32, max_range: f32, power: f32, resolve: f32) -> f32 {
    let prox = (1.0 - distance.max(0.0) / max_range.max(1e-4)).clamp(0.0, 1.0);
    let p = power.clamp(0.0, 1.0);
    let res = resolve.clamp(0.0, 1.0);
    (prox * p * (1.0 - res)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn out_of_range_zero() { assert_eq!(fear(20.0, 10.0, 1.0, 0.0), 0.0); }
    #[test] fn close_low_resolve_high() { assert!(fear(0.0, 10.0, 1.0, 0.0) > 0.8); }
    #[test] fn full_resolve_zero() { assert_eq!(fear(0.0, 10.0, 1.0, 1.0), 0.0); }
    #[test] fn farther_less() { assert!(fear(8.0, 10.0, 1.0, 0.0) < fear(2.0, 10.0, 1.0, 0.0)); }
    #[test] fn stronger_more() { assert!(fear(2.0, 10.0, 1.0, 0.0) > fear(2.0, 10.0, 0.3, 0.0)); }
    #[test] fn deterministic() { assert_eq!(fear(2.0, 10.0, 1.0, 0.2).to_bits(), fear(2.0, 10.0, 1.0, 0.2).to_bits()); }
}
