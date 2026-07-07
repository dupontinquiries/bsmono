//! Perf: batched wing-flap thrust, bit-identical to the scalar reference.
//! thrust ≈ flap_rate² · span · effort.

pub fn thrust_one(flap_rate: f32, span: f32, effort: f32) -> f32 {
    let r = flap_rate.max(0.0);
    let s = span.max(0.0);
    let e = effort.clamp(0.0, 1.0);
    r * r * s * e
}

pub fn thrust_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(r, s, e)| thrust_one(r, s, e)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(2.0f32, 8.0f32, 0.8f32), (0.0, 5.0, 1.0), (3.0, 10.0, 0.5), (1.5, 6.0, 0.3)];
        let b = thrust_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), thrust_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(thrust_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(thrust_batch(&[(2.0, 8.0, 0.8)])[0].to_bits(), thrust_one(2.0, 8.0, 0.8).to_bits()); }
    #[test] fn zero_rate_no_thrust() { assert_eq!(thrust_one(0.0, 8.0, 1.0), 0.0); }
    #[test] fn faster_flap_more() { assert!(thrust_one(3.0, 8.0, 0.8) > thrust_one(1.0, 8.0, 0.8)); }
    #[test] fn deterministic() { assert_eq!(thrust_batch(&[(2.0, 8.0, 0.8)]), thrust_batch(&[(2.0, 8.0, 0.8)])); }
}
