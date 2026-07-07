//! `field_step` — one diffusion stencil step (the GPU-perfect field reference).
//!
//! Worker-03 / WORLD pure core, round 22 (GPU_COMPUTE_PLAN #2). A single explicit
//! diffusion step over a 2D grid (weather / water field). Written as symmetric
//! per-edge flux — each interior edge moves the same amount out of one cell and
//! into the other — so total field is conserved exactly (the property a GPU
//! kernel must reproduce bit-for-bit when this moves to compute). Frees the CPU
//! tick_sim once ported.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// One diffusion step over a `w × h` row-major `field`. `rate` (clamped [0,1])
/// scales how much crosses each edge. Conserves total field; `rate 0` is a
/// no-op.
pub fn diffuse_step(field: &mut [f32], w: usize, h: usize, rate: f32) {
    let n = w * h;
    if n == 0 || field.len() < n {
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
            // Only +x and +y edges, so each edge is handled once (symmetric flux).
            if x + 1 < w {
                let nb = c + 1;
                let f = r * 0.25 * (field[c] - field[nb]);
                delta[c] -= f;
                delta[nb] += f;
            }
            if y + 1 < h {
                let nb = c + w;
                let f = r * 0.25 * (field[c] - field[nb]);
                delta[c] -= f;
                delta[nb] += f;
            }
        }
    }
    for i in 0..n {
        field[i] += delta[i];
    }
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total(f: &[f32]) -> f32 {
        f.iter().sum()
    }

    #[test]
    fn flat_unchanged() {
        let mut f = vec![3.0f32; 16];
        diffuse_step(&mut f, 4, 4, 0.5);
        assert_eq!(f, vec![3.0f32; 16]);
    }

    #[test]
    fn spike_spreads() {
        let mut f = vec![0.0f32; 25];
        f[12] = 10.0;
        diffuse_step(&mut f, 5, 5, 0.8);
        assert!(f[12] < 10.0, "spike did not spread");
        assert!(f[11] > 0.0 && f[13] > 0.0 && f[7] > 0.0 && f[17] > 0.0);
    }

    #[test]
    fn mass_conserved() {
        let mut f: Vec<f32> = (0..36).map(|i| ((i * 7) % 13) as f32).collect();
        let before = total(&f);
        for _ in 0..30 {
            diffuse_step(&mut f, 6, 6, 0.7);
        }
        assert!((total(&f) - before).abs() < 1e-2);
    }

    #[test]
    fn rate_zero_is_noop() {
        let mut f = vec![1.0, 5.0, 2.0, 9.0];
        let before = f.clone();
        diffuse_step(&mut f, 2, 2, 0.0);
        assert_eq!(f, before);
    }

    #[test]
    fn edges_handled() {
        // Corner spike — fewer edges, no panic, conserves.
        let mut f = vec![0.0f32; 9];
        f[0] = 9.0;
        let before = total(&f);
        diffuse_step(&mut f, 3, 3, 0.5);
        assert!((total(&f) - before).abs() < 1e-4);
        assert!(f[0] < 9.0);
    }

    #[test]
    fn deterministic() {
        let mut a: Vec<f32> = (0..64).map(|i| ((i * 11) % 17) as f32).collect();
        let mut b = a.clone();
        diffuse_step(&mut a, 8, 8, 0.6);
        diffuse_step(&mut b, 8, 8, 0.6);
        assert_eq!(a, b);
    }
}
