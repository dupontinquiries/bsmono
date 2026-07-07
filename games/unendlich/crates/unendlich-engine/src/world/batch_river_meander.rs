//! Perf: batched river-meander offset, bit-identical to the scalar reference.
//! Mirrors river_meander: amplitude · sat(flow) · sat(|curvature|), signed by curvature.

fn sat(x: f32) -> f32 { (x.abs() / (1.0 + x.abs())).clamp(0.0, 1.0) }

pub fn meander_one(amplitude: f32, flow: f32, curvature: f32) -> f32 {
    let amp = amplitude.max(0.0);
    let m = amp * sat(flow) * sat(curvature);
    if curvature < 0.0 { -m } else { m }
}

pub fn meander_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(a, f, c)| meander_one(a, f, c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn bit_parity() {
        let items = [(5.0f32, 10.0f32, 2.0f32), (3.0, 1.0, -4.0), (8.0, 0.0, 1.0), (2.0, 5.0, 0.0)];
        let b = meander_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), meander_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(meander_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(meander_batch(&[(5.0, 2.0, 1.0)])[0].to_bits(), meander_one(5.0, 2.0, 1.0).to_bits()); }
    #[test] fn straight_is_zero() { assert_eq!(meander_one(5.0, 10.0, 0.0), 0.0); }
    #[test] fn sign_follows_curvature() { assert!(meander_one(5.0, 10.0, -2.0) < 0.0 && meander_one(5.0, 10.0, 2.0) > 0.0); }
    #[test] fn bounded_by_amplitude() {
        for &(f, c) in &[(10.0f32, 5.0f32), (100.0, 100.0), (1.0, 0.5)] { assert!(meander_one(5.0, f, c).abs() <= 5.0); }
    }
    #[test] fn deterministic() { assert_eq!(meander_batch(&[(5.0, 2.0, 1.0)]), meander_batch(&[(5.0, 2.0, 1.0)])); }
}
