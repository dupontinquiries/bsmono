//! Express a phenotype from two alleles with a dominance weight. `dominance` in
//! [0,1]: 0 = codominant (average), 1 = the larger allele fully dominates. Pure.

pub fn express(allele_a: f32, allele_b: f32, dominance: f32) -> f32 {
    let a = allele_a.clamp(0.0, 1.0);
    let b = allele_b.clamp(0.0, 1.0);
    let d = dominance.clamp(0.0, 1.0);
    let avg = (a + b) * 0.5;
    let hi = a.max(b);
    (avg * (1.0 - d) + hi * d).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn codominant_is_average() { assert!((express(0.2, 0.8, 0.0) - 0.5).abs() < 1e-6); }
    #[test] fn full_dominance_is_max() { assert!((express(0.2, 0.8, 1.0) - 0.8).abs() < 1e-6); }
    #[test] fn equal_alleles() { assert!((express(0.4, 0.4, 0.7) - 0.4).abs() < 1e-6); }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d <= 1.0 { let v = express(0.3, 0.9, d); assert!((0.0..=1.0).contains(&v)); d += 0.1; }
    }
    #[test] fn monotone_toward_hi() {
        let (mut prev, mut d) = (f32::MIN, 0.0f32);
        while d <= 1.0 { let v = express(0.2, 0.8, d); assert!(v >= prev - 1e-6); prev = v; d += 0.1; }
    }
    #[test] fn deterministic() { assert_eq!(express(0.2, 0.8, 0.5).to_bits(), express(0.2, 0.8, 0.5).to_bits()); }
}
