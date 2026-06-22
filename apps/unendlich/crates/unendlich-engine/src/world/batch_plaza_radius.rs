//! Perf: batched plaza radii over many settlements, bit-identical to ref.

pub fn radius_one(population: f32, per_capita_area: f32, max_radius: f32) -> f32 {
    let pop = population.max(0.0);
    let pca = per_capita_area.max(0.0);
    let mr = max_radius.max(0.0);
    (pop * pca / std::f32::consts::PI).sqrt().min(mr)
}

pub fn radius_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(p, a, m)| radius_one(p, a, m)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 2.0f32, 1.0e6f32), (1000.0, 2.0, 1.0e6), (100.0, 2.0, 1.0e6), (1.0e9, 2.0, 50.0)];
        let b = radius_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), radius_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(radius_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(radius_batch(&[(500.0, 2.0, 1.0e6)])[0].to_bits(), radius_one(500.0, 2.0, 1.0e6).to_bits()); }
    #[test] fn empty_zero() { assert_eq!(radius_batch(&[(0.0, 2.0, 1.0e6)])[0], 0.0); }
    #[test] fn capped() { assert_eq!(radius_batch(&[(1.0e9, 2.0, 50.0)])[0], 50.0); }
    #[test] fn deterministic() { assert_eq!(radius_batch(&[(500.0, 2.0, 1.0e6)]), radius_batch(&[(500.0, 2.0, 1.0e6)])); }
}
