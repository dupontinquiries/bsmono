//! Perf: batched nest suitability over candidate sites, bit-identical to ref.

pub fn suit_one(elevation_norm: f32, shelter: f32, prey_density: f32) -> f32 {
    let e = elevation_norm.clamp(0.0, 1.0);
    let sh = shelter.clamp(0.0, 1.0);
    let prey = prey_density.clamp(0.0, 1.0);
    let elev_fit = (-((e - 0.7) * (e - 0.7)) / 0.1).exp();
    (elev_fit * sh * prey).clamp(0.0, 1.0)
}

pub fn suit_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(e, s, p)| suit_one(e, s, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.7f32, 1.0f32, 1.0f32), (0.0, 1.0, 1.0), (0.7, 0.0, 1.0), (0.5, 0.8, 0.6)];
        let b = suit_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), suit_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(suit_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(suit_batch(&[(0.7, 1.0, 1.0)])[0].to_bits(), suit_one(0.7, 1.0, 1.0).to_bits()); }
    #[test] fn no_shelter_zero() { assert_eq!(suit_batch(&[(0.7, 0.0, 1.0)])[0], 0.0); }
    #[test] fn ideal_high() { assert!(suit_batch(&[(0.7, 1.0, 1.0)])[0] > 0.9); }
    #[test] fn deterministic() { assert_eq!(suit_batch(&[(0.7, 1.0, 1.0)]), suit_batch(&[(0.7, 1.0, 1.0)])); }
}
