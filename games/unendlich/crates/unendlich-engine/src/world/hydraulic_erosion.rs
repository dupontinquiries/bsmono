//! `hydraulic_erosion` — thermal/talus smoothing over a heightmap.
//!
//! Worker-03 / WORLD-PERF pure core, round 10. Classic thermal (talus) erosion:
//! wherever a cell's slope to a lower neighbor exceeds the talus angle, some
//! material slides downhill. It's mass-conserving (what leaves a cell is added
//! to its neighbors) and double-buffered (all moves computed from one snapshot,
//! then applied), so it's order-independent and deterministic. Softens the
//! sharp ridges `orogeny` leaves before meshing.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Fraction of the steepest excess slope moved per iteration (stability < 1).
const FLOW_FACTOR: f32 = 0.5;

/// Run `iterations` of talus erosion over a `w × h` row-major heightmap.
/// Material slides to orthogonal lower neighbors whose drop exceeds `talus`.
pub fn erode(height: &[f32], w: usize, h: usize, iterations: u32, talus: f32) -> Vec<f32> {
    let n = w * h;
    if n == 0 || height.len() < n {
        return height.to_vec();
    }
    let t = if talus.is_finite() && talus > 0.0 { talus } else { 0.0 };
    let mut cur: Vec<f32> = height[..n].to_vec();

    for _ in 0..iterations {
        let mut delta = vec![0.0f32; n];
        for y in 0..h {
            for x in 0..w {
                let c = y * w + x;
                let hc = cur[c];
                // Gather orthogonal neighbors whose drop exceeds talus.
                let mut d_total = 0.0f32;
                let mut d_max = 0.0f32;
                for (nb, _) in neigh4(x, y, w, h) {
                    let diff = hc - cur[nb];
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
                let amount = FLOW_FACTOR * (d_max - t);
                for (nb, _) in neigh4(x, y, w, h) {
                    let diff = hc - cur[nb];
                    if diff > t {
                        let share = amount * (diff / d_total);
                        delta[c] -= share;
                        delta[nb] += share;
                    }
                }
            }
        }
        for i in 0..n {
            cur[i] += delta[i];
        }
    }
    cur
}

fn neigh4(x: usize, y: usize, w: usize, h: usize) -> impl Iterator<Item = (usize, ())> {
    let c = y * w + x;
    [
        if x > 0 { Some(c - 1) } else { None },
        if x + 1 < w { Some(c + 1) } else { None },
        if y > 0 { Some(c - w) } else { None },
        if y + 1 < h { Some(c + w) } else { None },
    ]
    .into_iter()
    .flatten()
    .map(|i| (i, ()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total(v: &[f32]) -> f32 {
        v.iter().sum()
    }
    fn maxv(v: &[f32]) -> f32 {
        v.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
    }

    #[test]
    fn flat_is_unchanged() {
        let flat = vec![5.0f32; 16];
        let out = erode(&flat, 4, 4, 10, 1.0);
        assert_eq!(out, flat);
    }

    #[test]
    fn sharp_peak_is_smoothed() {
        let mut g = vec![0.0f32; 25]; // 5×5
        g[12] = 10.0; // central spike
        let before = maxv(&g);
        let out = erode(&g, 5, 5, 20, 0.5);
        assert!(maxv(&out) < before, "peak not lowered");
        // some material reached neighbors
        assert!(out[11] > 0.0 || out[13] > 0.0 || out[7] > 0.0 || out[17] > 0.0);
    }

    #[test]
    fn mass_conserved() {
        let mut g = vec![0.0f32; 36];
        for i in 0..36 {
            g[i] = ((i * 7) % 13) as f32;
        }
        let before = total(&g);
        let out = erode(&g, 6, 6, 30, 0.7);
        assert!((total(&out) - before).abs() < 1e-2, "mass drifted");
    }

    #[test]
    fn more_iterations_lower_the_peak_monotonically() {
        let mut g = vec![0.0f32; 49];
        g[24] = 20.0; // 7×7 spike
        let a = maxv(&erode(&g, 7, 7, 5, 0.5));
        let b = maxv(&erode(&g, 7, 7, 20, 0.5));
        let c = maxv(&erode(&g, 7, 7, 80, 0.5));
        assert!(a >= b && b >= c, "peak should not rise with more iterations");
        assert!(c < a, "more iterations should make progress");
    }

    #[test]
    fn borders_handled() {
        // Corner spike — fewer neighbors, must not panic and must conserve.
        let mut g = vec![0.0f32; 9];
        g[0] = 9.0;
        let before = total(&g);
        let out = erode(&g, 3, 3, 15, 0.5);
        assert!((total(&out) - before).abs() < 1e-3);
        assert!(out[0] < 9.0);
    }

    #[test]
    fn deterministic() {
        let g: Vec<f32> = (0..64).map(|i| ((i * 11) % 17) as f32).collect();
        assert_eq!(erode(&g, 8, 8, 12, 0.6), erode(&g, 8, 8, 12, 0.6));
    }
}
