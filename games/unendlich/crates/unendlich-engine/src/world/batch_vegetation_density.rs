//! Perf: batched vegetation density over a terrain grid, bit-identical to ref.

pub fn dens_one(moisture: f32, temp_norm: f32, sunlight: f32) -> f32 {
    let m = moisture.clamp(0.0, 1.0);
    let t = temp_norm.clamp(0.0, 1.0);
    let sun = sunlight.clamp(0.0, 1.0);
    let temp_fit = (-((t - 0.65) * (t - 0.65)) / 0.12).exp();
    (m * temp_fit * sun).clamp(0.0, 1.0)
}

pub fn dens_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(m, t, s)| dens_one(m, t, s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 0.65f32, 1.0f32), (0.9, 0.65, 1.0), (0.8, 0.65, 0.0), (0.7, 0.3, 0.6)];
        let b = dens_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), dens_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(dens_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(dens_batch(&[(0.9, 0.65, 1.0)])[0].to_bits(), dens_one(0.9, 0.65, 1.0).to_bits()); }
    #[test] fn dry_barren() { assert_eq!(dens_batch(&[(0.0, 0.65, 1.0)])[0], 0.0); }
    #[test] fn lush_high() { assert!(dens_batch(&[(0.9, 0.65, 1.0)])[0] > 0.8); }
    #[test] fn deterministic() { assert_eq!(dens_batch(&[(0.9, 0.65, 1.0)]), dens_batch(&[(0.9, 0.65, 1.0)])); }
}
