//! `thermal_erosion` — one thermal-erosion pass on a heightfield, in place.
//!
//! Worker-03 / WORLD pure core, round 16. Wherever a cell's drop to an
//! orthogonal neighbor exceeds the talus angle, a `rate`-scaled fraction of the
//! excess slides downhill. Mass-conserving and double-buffered (all moves from
//! one snapshot, applied together), so it's order-independent. A single step so
//! the caller controls how many passes to run. Complements `hydraulic_erosion`
//! (R10) with the brief's exact `erode_step` signature.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// One thermal-erosion step over a `w`-wide, row-major heightfield `h`
/// (rows inferred as `h.len()/w`). Slope drops above `talus` move `rate·0.5` of
/// the excess to lower neighbors. No-op on ragged dimensions.
pub fn erode_step(h: &mut [f32], w: usize, talus: f32, rate: f32) {
    let n = h.len();
    if w == 0 || n == 0 || n % w != 0 {
        return;
    }
    let rows = n / w;
    let t = if talus.is_finite() && talus > 0.0 { talus } else { 0.0 };
    let r = clamp01(rate);

    let mut delta = vec![0.0f32; n];
    for y in 0..rows {
        for x in 0..w {
            let c = y * w + x;
            let hc = h[c];
            let mut d_total = 0.0f32;
            let mut d_max = 0.0f32;
            for nb in neigh4(x, y, w, rows) {
                let diff = hc - h[nb];
                if diff > t {
                    d_total += diff;
                    if diff > d_max {
                        d_max = diff;
                    }
                }
            }
            if d_max <= t || d_total <= 0.0 {
                continue;
            }
            let amount = r * (d_max - t) * 0.5;
            for nb in neigh4(x, y, w, rows) {
                let diff = hc - h[nb];
                if diff > t {
                    let share = amount * (diff / d_total);
                    delta[c] -= share;
                    delta[nb] += share;
                }
            }
        }
    }
    for i in 0..n {
        h[i] += delta[i];
    }
}

fn neigh4(x: usize, y: usize, w: usize, rows: usize) -> impl Iterator<Item = usize> {
    let c = y * w + x;
    [
        if x > 0 { Some(c - 1) } else { None },
        if x + 1 < w { Some(c + 1) } else { None },
        if y > 0 { Some(c - w) } else { None },
        if y + 1 < rows { Some(c + w) } else { None },
    ]
    .into_iter()
    .flatten()
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total(h: &[f32]) -> f32 {
        h.iter().sum()
    }

    #[test]
    fn flat_unchanged() {
        let mut h = vec![3.0f32; 16];
        erode_step(&mut h, 4, 0.5, 1.0);
        assert_eq!(h, vec![3.0f32; 16]);
    }

    #[test]
    fn spike_spreads() {
        let mut h = vec![0.0f32; 25];
        h[12] = 10.0;
        erode_step(&mut h, 5, 0.5, 1.0);
        assert!(h[12] < 10.0, "spike did not erode");
        assert!(h[11] > 0.0 || h[13] > 0.0 || h[7] > 0.0 || h[17] > 0.0);
    }

    #[test]
    fn mass_conserved() {
        let mut h: Vec<f32> = (0..36).map(|i| ((i * 7) % 13) as f32).collect();
        let before = total(&h);
        for _ in 0..10 {
            erode_step(&mut h, 6, 0.7, 0.6);
        }
        assert!((total(&h) - before).abs() < 1e-2);
    }

    #[test]
    fn talus_threshold_respected() {
        // Gentle slope below the talus angle → nothing moves.
        let mut h = vec![0.0, 0.3, 0.6, 0.9]; // step 0.3 < talus 0.5
        let before = h.clone();
        erode_step(&mut h, 4, 0.5, 1.0);
        assert_eq!(h, before);
    }

    #[test]
    fn rate_scales_movement() {
        let mut slow = vec![0.0f32; 9];
        slow[4] = 6.0;
        let mut fast = slow.clone();
        erode_step(&mut slow, 3, 0.5, 0.2);
        erode_step(&mut fast, 3, 0.5, 0.8);
        // Higher rate erodes the spike more in one step.
        assert!(fast[4] < slow[4], "higher rate should move more");
    }

    #[test]
    fn deterministic() {
        let mut a: Vec<f32> = (0..64).map(|i| ((i * 11) % 17) as f32).collect();
        let mut b = a.clone();
        erode_step(&mut a, 8, 0.6, 0.5);
        erode_step(&mut b, 8, 0.6, 0.5);
        assert_eq!(a, b);
    }
}
