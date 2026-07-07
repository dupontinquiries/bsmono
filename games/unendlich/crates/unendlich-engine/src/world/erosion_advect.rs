//! `erosion_advect` — advect suspended sediment one step downhill.
//!
//! Worker-03 / WORLD pure core, round 23. Moves a `rate` fraction of each cell's
//! suspended sediment to the cell its flow direction points at (from
//! `flow_route`, R19), so eroded material travels with the water before
//! `sediment_deposit` (R19) drops it. Sinks and edges retain their load, so the
//! total is conserved. The transport companion to `field_step` (R22)'s
//! diffusion.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Advect `sed` one step over a `w × h` grid following per-cell `flow_dir`
/// (`0..=3` = N,E,S,W; `4` = sink). `rate` (clamped [0,1]) is the fraction moved
/// downstream. Sinks and out-of-bounds targets retain their sediment.
pub fn advect_step(sed: &mut [f32], flow_dir: &[u8], w: usize, h: usize, rate: f32) {
    let n = w * h;
    if n == 0 || sed.len() < n || flow_dir.len() < n {
        return;
    }
    let r = clamp01(rate);
    if r == 0.0 {
        return;
    }

    let mut delta = vec![0.0f32; n];
    for y in 0..h {
        for x in 0..w {
            let c = y * w + x;
            let amount = sed[c] * r;
            if amount <= 0.0 {
                continue;
            }
            let target = match flow_dir[c] {
                0 if y > 0 => Some(c - w),       // N
                1 if x + 1 < w => Some(c + 1),   // E
                2 if y + 1 < h => Some(c + w),   // S
                3 if x > 0 => Some(c - 1),       // W
                _ => None,                       // sink or off-grid
            };
            if let Some(t) = target {
                delta[c] -= amount;
                delta[t] += amount;
            }
        }
    }
    for i in 0..n {
        sed[i] += delta[i];
    }
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total(s: &[f32]) -> f32 {
        s.iter().sum()
    }

    #[test]
    fn no_flow_unchanged() {
        let mut sed = vec![1.0f32; 9];
        let dir = vec![4u8; 9]; // all sinks
        advect_step(&mut sed, &dir, 3, 3, 0.5);
        assert_eq!(sed, vec![1.0f32; 9]);
    }

    #[test]
    fn moves_along_flow() {
        // 3-wide row; cell 0 flows East into cell 1.
        let mut sed = vec![10.0, 0.0, 0.0];
        let dir = vec![1u8, 4, 4]; // E, sink, sink
        advect_step(&mut sed, &dir, 3, 1, 0.5);
        assert!(sed[0] < 10.0 && sed[1] > 0.0);
        assert!((sed[0] - 5.0).abs() < 1e-5 && (sed[1] - 5.0).abs() < 1e-5);
    }

    #[test]
    fn mass_conserved() {
        let mut sed: Vec<f32> = (0..16).map(|i| (i % 5) as f32).collect();
        // a deterministic mix of directions
        let dir: Vec<u8> = (0..16).map(|i| (i % 5) as u8).collect();
        let before = total(&sed);
        for _ in 0..20 {
            advect_step(&mut sed, &dir, 4, 4, 0.4);
        }
        assert!((total(&sed) - before).abs() < 1e-3);
    }

    #[test]
    fn rate_scales_transport() {
        let dir = vec![1u8, 4, 4];
        let mut slow = vec![10.0, 0.0, 0.0];
        let mut fast = vec![10.0, 0.0, 0.0];
        advect_step(&mut slow, &dir, 3, 1, 0.2);
        advect_step(&mut fast, &dir, 3, 1, 0.8);
        assert!(fast[1] > slow[1], "higher rate should move more");
    }

    #[test]
    fn edges_retain_sediment() {
        // Cell 0 flows West (off-grid) → sediment stays, mass conserved.
        let mut sed = vec![7.0, 0.0, 0.0];
        let dir = vec![3u8, 4, 4]; // W at the left edge
        let before = total(&sed);
        advect_step(&mut sed, &dir, 3, 1, 0.5);
        assert_eq!(sed[0], 7.0);
        assert!((total(&sed) - before).abs() < 1e-6);
    }

    #[test]
    fn deterministic() {
        let dir: Vec<u8> = (0..16).map(|i| (i % 5) as u8).collect();
        let mut a: Vec<f32> = (0..16).map(|i| (i % 3) as f32).collect();
        let mut b = a.clone();
        advect_step(&mut a, &dir, 4, 4, 0.5);
        advect_step(&mut b, &dir, 4, 4, 0.5);
        assert_eq!(a, b);
    }
}
