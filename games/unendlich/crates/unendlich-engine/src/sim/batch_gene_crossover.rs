//! Perf: batched gene crossover over a whole genome vector, bit-identical to ref.
//! Mirrors gene_crossover: lerp(a,b,mix), clamped.

pub fn cross_one(parent_a: f32, parent_b: f32, mix: f32) -> f32 {
    let a = parent_a.clamp(0.0, 1.0);
    let b = parent_b.clamp(0.0, 1.0);
    let m = mix.clamp(0.0, 1.0);
    (a * (1.0 - m) + b * m).clamp(0.0, 1.0)
}

pub fn cross_batch(pairs: &[(f32, f32)], mix: f32) -> Vec<f32> {
    pairs.iter().map(|&(a, b)| cross_one(a, b, mix)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let pairs = [(0.2f32, 0.9f32), (0.5, 0.5), (0.1, 0.8), (0.7, 0.3)];
        let b = cross_batch(&pairs, 0.4);
        for (i, &p) in pairs.iter().enumerate() { assert_eq!(b[i].to_bits(), cross_one(p.0, p.1, 0.4).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(cross_batch(&[], 0.4).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(cross_batch(&[(0.2, 0.9)], 0.4)[0].to_bits(), cross_one(0.2, 0.9, 0.4).to_bits()); }
    #[test] fn mix0_takes_a() { assert_eq!(cross_batch(&[(0.2, 0.9), (0.3, 0.7)], 0.0), vec![0.2, 0.3]); }
    #[test] fn bounded() { for v in cross_batch(&[(0.3, 0.7), (0.1, 0.9)], 0.5) { assert!((0.0..=1.0).contains(&v)); } }
    #[test] fn deterministic() { assert_eq!(cross_batch(&[(0.2, 0.9)], 0.4), cross_batch(&[(0.2, 0.9)], 0.4)); }
}
