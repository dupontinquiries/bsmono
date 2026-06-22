//! Perf: batched sound attenuation over many emitters, bit-identical to ref.

pub fn atten_one(distance: f32, reference: f32, rolloff: f32) -> f32 {
    let d = distance.max(0.0);
    let r = reference.max(1e-6);
    let k = rolloff.max(0.0);
    if d <= r { 1.0 } else { (r / (r + k * (d - r))).clamp(0.0, 1.0) }
}

pub fn atten_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, r, k)| atten_one(d, r, k)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(5.0f32, 10.0f32, 1.0f32), (100.0, 10.0, 1.0), (50.0, 10.0, 2.0), (20.0, 10.0, 0.5)];
        let b = atten_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), atten_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(atten_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(atten_batch(&[(50.0, 10.0, 1.0)])[0].to_bits(), atten_one(50.0, 10.0, 1.0).to_bits()); }
    #[test] fn within_reference_full() { assert_eq!(atten_batch(&[(5.0, 10.0, 1.0)])[0], 1.0); }
    #[test] fn far_quieter() { assert!(atten_batch(&[(100.0, 10.0, 1.0)])[0] < 1.0); }
    #[test] fn deterministic() { assert_eq!(atten_batch(&[(50.0, 10.0, 1.0)]), atten_batch(&[(50.0, 10.0, 1.0)])); }
}
