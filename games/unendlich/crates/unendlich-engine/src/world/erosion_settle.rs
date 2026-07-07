//! `erosion_settle` — settle loose material toward the angle of repose.
//!
//! Worker-03 / WORLD pure core, round 20. One relaxation step: wherever a cell
//! stands more than `repose` above an orthogonal neighbor, half the excess
//! slides down, split among the steep neighbors. Mass-conserving and
//! double-buffered, so a sand/gravel pile slumps toward a stable cone over
//! repeated steps. Distinct from `thermal_erosion` (R16) only in intent (loose
//! material vs bedrock talus) and the `repose` naming.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// One settle step over a `w`-wide, row-major heightfield `h`. Drops above the
/// `repose` slope move downhill; mass is conserved. No-op on ragged dims.
pub fn settle(h: &mut [f32], w: usize, repose: f32) {
    let n = h.len();
    if w == 0 || n == 0 || n % w != 0 {
        return;
    }
    let rows = n / w;
    let r = if repose.is_finite() && repose > 0.0 { repose } else { 0.0 };

    let mut delta = vec![0.0f32; n];
    for y in 0..rows {
        for x in 0..w {
            let c = y * w + x;
            let hc = h[c];
            let mut d_total = 0.0f32;
            let mut d_max = 0.0f32;
            for nb in neigh4(x, y, w, rows) {
                let diff = hc - h[nb];
                if diff > r {
                    d_total += diff;
                    if diff > d_max {
                        d_max = diff;
                    }
                }
            }
            if d_max <= r || d_total <= 0.0 {
                continue;
            }
            let amount = 0.5 * (d_max - r);
            for nb in neigh4(x, y, w, rows) {
                let diff = hc - h[nb];
                if diff > r {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn total(h: &[f32]) -> f32 {
        h.iter().sum()
    }

    #[test]
    fn flat_unchanged() {
        let mut h = vec![4.0f32; 16];
        settle(&mut h, 4, 0.5);
        assert_eq!(h, vec![4.0f32; 16]);
    }

    #[test]
    fn steep_pile_slumps() {
        let mut h = vec![0.0f32; 25];
        h[12] = 12.0; // tall column
        settle(&mut h, 5, 0.5);
        assert!(h[12] < 12.0, "pile did not slump");
    }

    #[test]
    fn mass_conserved() {
        let mut h: Vec<f32> = (0..36).map(|i| ((i * 5) % 11) as f32).collect();
        let before = total(&h);
        for _ in 0..10 {
            settle(&mut h, 6, 0.6);
        }
        assert!((total(&h) - before).abs() < 1e-2);
    }

    #[test]
    fn repose_respected() {
        // A slope at or below the repose angle is already stable.
        let mut h = vec![0.0, 0.4, 0.8, 1.2]; // step 0.4 ≤ repose 0.5
        let before = h.clone();
        settle(&mut h, 4, 0.5);
        assert_eq!(h, before);
    }

    #[test]
    fn single_cell_safe() {
        let mut h = vec![5.0f32];
        settle(&mut h, 1, 0.5); // no neighbors, no panic
        assert_eq!(h, vec![5.0]);
        // ragged dims are a no-op
        let mut r = vec![1.0, 2.0, 3.0];
        settle(&mut r, 2, 0.5); // 3 % 2 != 0
        assert_eq!(r, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn deterministic() {
        let mut a: Vec<f32> = (0..64).map(|i| ((i * 7) % 13) as f32).collect();
        let mut b = a.clone();
        settle(&mut a, 8, 0.6);
        settle(&mut b, 8, 0.6);
        assert_eq!(a, b);
    }
}
