//! Perf: batched ore grade, bit-identical to the scalar reference.
//! Mirrors ore_grade: (1−exp(−depth/200))·noise·richness, clamped.

pub fn grade_one(depth: f32, noise: f32, richness: f32) -> f32 {
    let d = depth.max(0.0);
    let n = noise.clamp(0.0, 1.0);
    let r = richness.max(0.0);
    let depth_factor = 1.0 - (-(d / 200.0)).exp();
    (depth_factor * n * r).clamp(0.0, 1.0)
}

pub fn grade_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, n, r)| grade_one(d, n, r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 1.0f32, 1.0f32), (500.0, 0.8, 1.0), (300.0, 0.5, 0.7), (1000.0, 1.0, 0.5)];
        let b = grade_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), grade_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(grade_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(grade_batch(&[(300.0, 0.8, 1.0)])[0].to_bits(), grade_one(300.0, 0.8, 1.0).to_bits()); }
    #[test] fn surface_zero() { assert_eq!(grade_one(0.0, 1.0, 1.0), 0.0); }
    #[test] fn deeper_more() { assert!(grade_one(500.0, 1.0, 1.0) > grade_one(50.0, 1.0, 1.0)); }
    #[test] fn deterministic() { assert_eq!(grade_batch(&[(300.0, 0.8, 1.0)]), grade_batch(&[(300.0, 0.8, 1.0)])); }
}
