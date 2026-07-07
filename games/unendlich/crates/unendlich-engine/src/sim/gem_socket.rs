//! `sim::gem_socket` — apply socketed gems to an item's stat block.
//!
//! [`apply_gems`] takes a `base` stat vector and a list of `(stat_index, amount)`
//! gems, and returns a new stat vector with each gem's amount added to the stat
//! it targets. Gems stack additively, multiple gems can hit the same stat, and a
//! gem pointing at a stat index the item doesn't have is simply ignored (no
//! panic, no resize). With no gems the base is returned unchanged.
//!
//! Pure and deterministic: a copy and indexed adds — no clock, no randomness, no
//! cross-imports. The output always has the same length as `base`.

/// Apply `gems` to `base`, returning the modified stat vector.
///
/// Each gem `(idx, amount)` adds `amount` to `base[idx]`; gems with
/// `idx >= base.len()` are ignored. Output length equals `base.len()`.
pub fn apply_gems(base: &[f32], gems: &[(u8, f32)]) -> Vec<f32> {
    let mut out = base.to_vec();
    let mut i = 0;
    while i < gems.len() {
        let (idx, amount) = gems[i];
        let idx = idx as usize;
        if idx < out.len() {
            out[idx] += amount;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn empty_gems_returns_base() {
        let base = [1.0, 2.0, 3.0];
        assert_eq!(apply_gems(&base, &[]), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn gem_adds_to_its_stat() {
        let base = [1.0, 2.0, 3.0];
        let out = apply_gems(&base, &[(1, 5.0)]);
        assert_eq!(out, vec![1.0, 7.0, 3.0]);
    }

    #[test]
    fn multiple_gems_stack() {
        let base = [0.0, 0.0];
        let out = apply_gems(&base, &[(0, 1.0), (0, 2.0), (1, 4.0), (0, 0.5)]);
        assert_eq!(out, vec![3.5, 4.0]);
    }

    #[test]
    fn out_of_range_stat_ignored() {
        let base = [1.0, 2.0];
        // index 5 doesn't exist → ignored, length unchanged
        let out = apply_gems(&base, &[(5, 9.0), (0, 1.0)]);
        assert_eq!(out, vec![2.0, 2.0]);
        assert_eq!(out.len(), base.len());
    }

    #[test]
    fn length_preserved_and_finite() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 8) as usize;
            let mut base = Vec::new();
            for _ in 0..n {
                base.push((lcg(&mut seed) % 1000) as f32 / 10.0);
            }
            let g = (lcg(&mut seed) % 8) as usize;
            let mut gems = Vec::new();
            for _ in 0..g {
                gems.push(((lcg(&mut seed) & 0xFF) as u8, (lcg(&mut seed) % 1000) as f32 / 10.0));
            }
            let out = apply_gems(&base, &gems);
            assert_eq!(out.len(), base.len());
            for v in out {
                assert!(v.is_finite());
            }
        }
    }

    #[test]
    fn deterministic() {
        let base = [1.0, 2.0, 3.0, 4.0];
        let gems = [(0u8, 1.0f32), (2, 2.0), (9, 5.0), (3, 0.5)];
        assert_eq!(apply_gems(&base, &gems), apply_gems(&base, &gems));
    }
}
