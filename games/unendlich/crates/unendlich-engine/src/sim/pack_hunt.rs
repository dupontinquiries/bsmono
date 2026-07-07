//! `sim::pack_hunt` — where each pack member should stand to surround prey.
//!
//! [`flank_pos`] places pack member `member_idx` (of `pack_n`) on a ring of the
//! given `radius` around the prey, evenly spaced so the pack encircles its
//! target. Member 0 takes the anchor slot (`+x` from the prey), and the rest
//! fan out around the circle; a lone hunter (`pack_n == 1`) just takes that one
//! slot. Every member sits exactly `radius` from the prey.
//!
//! Pure and deterministic: one angle and a cos/sin — no clock, no randomness, no
//! cross-imports.

use std::f32::consts::PI;

/// Position for a pack member flanking `prey`.
///
/// Member `i` of `pack_n` sits at angle `2π·i/pack_n` on a circle of `radius`
/// around the prey (`pack_n == 0` is treated as 1). The result is always
/// `radius` from the prey.
pub fn flank_pos(prey: [f32; 2], member_idx: usize, pack_n: usize, radius: f32) -> [f32; 2] {
    let n = if pack_n == 0 { 1 } else { pack_n };
    let angle = 2.0 * PI * (member_idx as f32) / (n as f32);
    [prey[0] + radius * angle.cos(), prey[1] + radius * angle.sin()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
        let dx = a[0] - b[0];
        let dy = a[1] - b[1];
        (dx * dx + dy * dy).sqrt()
    }

    #[test]
    fn members_spread_around_prey() {
        let prey = [3.0, 4.0];
        let n = 4;
        let mut positions = Vec::new();
        let mut i = 0;
        while i < n {
            positions.push(flank_pos(prey, i, n, 2.0));
            i += 1;
        }
        // all on the ring, all distinct
        for p in &positions {
            assert!((dist(*p, prey) - 2.0).abs() < 1e-4);
        }
        for a in 0..n {
            for b in (a + 1)..n {
                assert!(dist(positions[a], positions[b]) > 1e-3, "{} {} coincide", a, b);
            }
        }
    }

    #[test]
    fn ring_radius_is_honored() {
        let prey = [0.0, 0.0];
        let r = 5.0;
        let mut i = 0;
        while i < 8 {
            assert!((dist(flank_pos(prey, i, 8, r), prey) - r).abs() < 1e-4);
            i += 1;
        }
    }

    #[test]
    fn idx_lt_n_distinct() {
        let prey = [1.0, 1.0];
        let n = 6;
        let mut i = 0;
        while i < n {
            let mut j = i + 1;
            while j < n {
                assert!(dist(flank_pos(prey, i, n, 3.0), flank_pos(prey, j, n, 3.0)) > 1e-3);
                j += 1;
            }
            i += 1;
        }
    }

    #[test]
    fn n_one_takes_anchor_slot() {
        let prey = [2.0, 7.0];
        let p = flank_pos(prey, 0, 1, 4.0);
        assert!((p[0] - (prey[0] + 4.0)).abs() < 1e-4);
        assert!((p[1] - prey[1]).abs() < 1e-4);
        assert!((dist(p, prey) - 4.0).abs() < 1e-4);
    }

    #[test]
    fn bounded_to_radius() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let prey = [
                (lcg(&mut seed) % 2000) as f32 - 1000.0,
                (lcg(&mut seed) % 2000) as f32 - 1000.0,
            ];
            let n = 1 + (lcg(&mut seed) % 12) as usize;
            let idx = (lcg(&mut seed) as usize) % n;
            let r = (lcg(&mut seed) % 1000) as f32 / 10.0;
            assert!((dist(flank_pos(prey, idx, n, r), prey) - r).abs() < 1e-2);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..2000 {
            let prey = [(lcg(&mut seed) % 100) as f32, (lcg(&mut seed) % 100) as f32];
            let n = 1 + (lcg(&mut seed) % 10) as usize;
            let idx = (lcg(&mut seed) as usize) % n;
            let r = (lcg(&mut seed) % 100) as f32;
            assert_eq!(flank_pos(prey, idx, n, r), flank_pos(prey, idx, n, r));
        }
    }
}
