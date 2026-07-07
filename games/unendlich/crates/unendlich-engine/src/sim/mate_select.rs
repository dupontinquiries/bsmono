//! `sim::mate_select` — how compatible two creatures' genomes are as mates
//! (pairs with `egg_genome`, round 8).
//!
//! [`compat`] scores two trait vectors in `0..1`: identical genomes score high,
//! wildly different ones score low. It compares the shared prefix of the two
//! slices (so mismatched lengths are safe), measuring the mean absolute
//! per-trait difference and mapping it to `1 - mean_diff`, clamped. The score is
//! symmetric in its arguments.
//!
//! Pure and deterministic: a mean and a clamp — no clock, no randomness, no
//! cross-imports. Trait values are assumed roughly `0..1`; out-of-range inputs
//! still produce a bounded score.

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Mate compatibility of two genomes, `0..1`.
///
/// Compares `min(a.len(), b.len())` traits; `1 - mean|a_i - b_i|`, clamped.
/// Returns `0` when there are no shared traits (nothing to compare).
pub fn compat(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let mut sum = 0.0f32;
    let mut i = 0;
    while i < n {
        sum += (a[i] - b[i]).abs();
        i += 1;
    }
    let mean = sum / n as f32;
    clamp01(1.0 - mean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_genome(seed: &mut u32, len: usize) -> Vec<f32> {
        let mut g = Vec::with_capacity(len);
        let mut i = 0;
        while i < len {
            g.push((lcg(seed) % 100_001) as f32 / 100_000.0);
            i += 1;
        }
        g
    }

    #[test]
    fn identical_is_high() {
        let g = [0.2, 0.7, 0.5, 0.9];
        assert_eq!(compat(&g, &g), 1.0);
    }

    #[test]
    fn opposite_is_low() {
        let a = [0.0, 0.0, 0.0];
        let b = [1.0, 1.0, 1.0];
        assert_eq!(compat(&a, &b), 0.0);
    }

    #[test]
    fn symmetric() {
        let mut seed = 0x5111_u32;
        for _ in 0..3000 {
            let n = (lcg(&mut seed) % 8) as usize;
            let a = rand_genome(&mut seed, n);
            let b = rand_genome(&mut seed, n);
            assert_eq!(compat(&a, &b), compat(&b, &a));
        }
    }

    #[test]
    fn length_guarded() {
        // mismatched lengths use the shared prefix, no panic
        let a = [0.5, 0.5, 0.5, 0.5, 0.5];
        let b = [0.5, 0.5];
        let c = compat(&a, &b);
        assert!(c >= 0.0 && c <= 1.0);
        assert_eq!(c, 1.0); // shared prefix identical
        // empty overlap → 0
        assert_eq!(compat(&a, &[]), 0.0);
        assert_eq!(compat(&[], &[]), 0.0);
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let n = (lcg(&mut seed) % 8) as usize;
            // include out-of-[0,1] trait values
            let mut a = Vec::new();
            let mut b = Vec::new();
            let mut i = 0;
            while i < n {
                a.push((lcg(&mut seed) % 300_001) as f32 / 100_000.0 - 1.0);
                b.push((lcg(&mut seed) % 300_001) as f32 / 100_000.0 - 1.0);
                i += 1;
            }
            let c = compat(&a, &b);
            assert!(c >= 0.0 && c <= 1.0, "compat {}", c);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let n = (lcg(&mut seed) % 8) as usize;
            let a = rand_genome(&mut seed, n);
            let b = rand_genome(&mut seed, n);
            assert_eq!(compat(&a, &b), compat(&a, &b));
        }
    }
}
