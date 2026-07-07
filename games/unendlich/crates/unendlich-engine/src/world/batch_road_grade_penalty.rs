//! Perf: batched road grade penalties over a route's segments, bit-identical to ref.

pub fn penalty_one(grade: f32, steepness_cost: f32, max_penalty: f32) -> f32 {
    let g = grade.abs();
    let sc = steepness_cost.max(0.0);
    let mp = max_penalty.max(1.0);
    (1.0 + g * sc).min(mp)
}

pub fn penalty_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(g, s, m)| penalty_one(g, s, m)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 5.0f32, 10.0f32), (0.5, 5.0, 10.0), (-0.3, 5.0, 10.0), (100.0, 5.0, 10.0)];
        let b = penalty_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), penalty_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(penalty_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(penalty_batch(&[(0.3, 5.0, 10.0)])[0].to_bits(), penalty_one(0.3, 5.0, 10.0).to_bits()); }
    #[test] fn flat_one() { assert_eq!(penalty_batch(&[(0.0, 5.0, 10.0)])[0], 1.0); }
    #[test] fn capped() { assert_eq!(penalty_batch(&[(100.0, 5.0, 10.0)])[0], 10.0); }
    #[test] fn deterministic() { assert_eq!(penalty_batch(&[(0.3, 5.0, 10.0)]), penalty_batch(&[(0.3, 5.0, 10.0)])); }
}
