//! `sim::field_diffuse_ref` — pure CPU reference for **one explicit 2D field
//! diffusion step**, the exact math a GPU compute kernel mirrors (R22: every GPU
//! kernel ships a CPU reference that defines its math; the `.comp` is proven
//! bit-parity against this on lavapipe by the coordinator).
//!
//! This is the water/weather field update the GPU-offload plan moves off
//! `tick_sim` (`handoff/GPU_COMPUTE_PLAN.md` §2): a scalar field on a `w × h`
//! row-major grid (moisture, temperature anomaly, etc.) relaxed one explicit
//! Euler step of the heat equation
//!
//! ```text
//!   u'(c) = u(c) + coeff·dt · ∇²u(c)
//!   ∇²u(c) = (u_left − u_c) + (u_right − u_c) + (u_up − u_c) + (u_down − u_c)
//! ```
//!
//! ## Why this shape (vs the `world::field_step` scatter form)
//! A GPU kernel runs one thread per cell that **gathers** its four neighbours —
//! no cross-thread writes, no atomics. So this reference is a *gather* stencil:
//! every output cell is a pure function of the **input** grid only. Two
//! consequences the tests pin down: the result is independent of the order cells
//! are visited (it is a Jacobi update, not Gauss–Seidel), and the kernel maps
//! 1:1 onto a compute dispatch.
//!
//! ## Boundary handling — no-flux (Neumann)
//! Off-grid neighbours are taken equal to the centre cell (a "ghost = centre"
//! cell), implemented by **clamping the neighbour index** to the grid edge. The
//! corresponding difference `(centre − centre) = 0`, so no field crosses the
//! border. Under this boundary the discrete Laplacian sums to zero over the
//! whole grid, so **total field (mass) is conserved** — exactly in real
//! arithmetic, and to f32 rounding in practice.
//!
//! ## Stability
//! Explicit diffusion is stable for `coeff·dt ≤ 0.25` (2D, unit grid spacing).
//! Outside that the step still produces finite output for finite input (it is a
//! finite sum/product, no division), it just rings — callers pick a stable
//! `coeff·dt`.
//!
//! Pure: data in, data out. Allocation-light — the caller owns both buffers
//! (`src` read, `dst` written), so a real tick ping-pongs two grids with zero
//! per-step allocation. No render/Vulkan, no clock, no RNG, no `.unwrap()`.

#[inline]
fn idx(x: usize, y: usize, w: usize) -> usize {
    y * w + x
}

/// Next value of one cell `(x, y)` — a pure function of the **input** grid
/// `src`, hence independent of any traversal order. Neighbour indices are
/// clamped to the grid (no-flux boundary).
#[inline]
fn cell_next(src: &[f32], w: usize, h: usize, x: usize, y: usize, coeff: f32, dt: f32) -> f32 {
    let c = src[idx(x, y, w)];
    let xm = if x > 0 { x - 1 } else { 0 };
    let xp = if x + 1 < w { x + 1 } else { w - 1 };
    let ym = if y > 0 { y - 1 } else { 0 };
    let yp = if y + 1 < h { y + 1 } else { h - 1 };

    let left = src[idx(xm, y, w)];
    let right = src[idx(xp, y, w)];
    let down = src[idx(x, ym, w)];
    let up = src[idx(x, yp, w)];

    // Laplacian as a sum of edge differences. For a flat field every term is
    // exactly 0, so a flat field is unchanged bit-for-bit; at a clamped border
    // the off-grid term is (c − c) = 0 (no flux).
    let laplacian = (left - c) + (right - c) + (up - c) + (down - c);
    c + coeff * dt * laplacian
}

/// One explicit diffusion step over a `width × height` row-major field.
///
/// Reads `src`, writes the next grid into `dst` (same length, caller-owned
/// scratch — no allocation). `coeff` is the diffusion coefficient, `dt` the
/// timestep; their product scales how much crosses each edge (keep
/// `coeff·dt ≤ 0.25` for stability). `coeff` or `dt` of 0 leaves the field
/// unchanged.
///
/// No-op (leaves `dst` untouched) if the grid is empty or `src`/`dst` are not
/// both exactly `width·height` long — never panics.
pub fn diffuse_step(
    src: &[f32],
    dst: &mut [f32],
    width: usize,
    height: usize,
    coeff: f32,
    dt: f32,
) {
    let n = width * height;
    if n == 0 || src.len() != n || dst.len() != n {
        return;
    }
    for y in 0..height {
        for x in 0..width {
            dst[idx(x, y, width)] = cell_next(src, width, height, x, y, coeff, dt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Seeded deterministic noise (R20). Seeds documented inline.
    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn randf(seed: &mut u32, lo: f32, hi: f32) -> f32 {
        lo + (lcg(seed) % 1_000_001) as f32 / 1_000_000.0 * (hi - lo)
    }
    fn random_grid(seed: &mut u32, n: usize, lo: f32, hi: f32) -> Vec<f32> {
        (0..n).map(|_| randf(seed, lo, hi)).collect()
    }
    fn sum(v: &[f32]) -> f64 {
        // f64 accumulation so the *test's* reduction error doesn't mask the
        // property under test.
        v.iter().map(|&x| x as f64).sum()
    }

    #[test]
    fn flat_field_is_steady_state() {
        // Every cell equal ⇒ all edge differences are exactly 0 ⇒ unchanged.
        let (w, h) = (7usize, 5usize);
        let src = vec![3.5f32; w * h];
        let mut dst = vec![0.0f32; w * h];
        diffuse_step(&src, &mut dst, w, h, 0.2, 1.0);
        assert_eq!(dst, src, "a flat field must be a steady state (bit-exact)");
    }

    #[test]
    fn coefficient_zero_is_identity() {
        let mut s = 0xF1E1_0000u32;
        let (w, h) = (9usize, 6usize);
        let src = random_grid(&mut s, w * h, -10.0, 10.0);
        let mut dst = vec![0.0f32; w * h];
        diffuse_step(&src, &mut dst, w, h, 0.0, 5.0);
        assert_eq!(dst, src, "coeff=0 must reproduce the field exactly");
    }

    #[test]
    fn dt_zero_is_identity() {
        let mut s = 0xF1E1_0001u32;
        let (w, h) = (6usize, 6usize);
        let src = random_grid(&mut s, w * h, -10.0, 10.0);
        let mut dst = vec![0.0f32; w * h];
        diffuse_step(&src, &mut dst, w, h, 0.25, 0.0);
        assert_eq!(dst, src, "dt=0 must reproduce the field exactly");
    }

    #[test]
    fn mass_conserved_under_no_flux() {
        // No-flux boundary ⇒ total field unchanged (to f32 rounding).
        let mut s = 0xF1E1_0002u32;
        for _ in 0..200 {
            let w = 2 + (lcg(&mut s) % 12) as usize;
            let h = 2 + (lcg(&mut s) % 12) as usize;
            let src = random_grid(&mut s, w * h, -50.0, 50.0);
            let mut dst = vec![0.0f32; w * h];
            // stable coefficient so values stay well-scaled
            let coeff = randf(&mut s, 0.0, 0.2);
            let dt = randf(&mut s, 0.0, 1.0);
            diffuse_step(&src, &mut dst, w, h, coeff, dt);
            let before = sum(&src);
            let after = sum(&dst);
            let tol = 1e-3 * (1.0 + before.abs()) + 1e-3 * (w * h) as f64;
            assert!(
                (after - before).abs() <= tol,
                "mass drift {} (before {}, after {}, {}x{})",
                after - before, before, after, w, h
            );
        }
    }

    #[test]
    fn outputs_finite_for_bounded_inputs() {
        let mut s = 0xF1E1_0003u32;
        for _ in 0..300 {
            let w = 1 + (lcg(&mut s) % 16) as usize;
            let h = 1 + (lcg(&mut s) % 16) as usize;
            let src = random_grid(&mut s, w * h, -1e3, 1e3);
            let mut dst = vec![0.0f32; w * h];
            let coeff = randf(&mut s, 0.0, 2.0); // incl. unstable range
            let dt = randf(&mut s, 0.0, 2.0);
            diffuse_step(&src, &mut dst, w, h, coeff, dt);
            for (i, &v) in dst.iter().enumerate() {
                assert!(v.is_finite(), "cell {} not finite: {}", i, v);
            }
        }
    }

    #[test]
    fn independent_of_traversal_order() {
        // Recompute every cell in REVERSE index order; gather-from-input means
        // the result must be byte-identical regardless of visit order.
        let mut s = 0xF1E1_0004u32;
        for _ in 0..100 {
            let w = 2 + (lcg(&mut s) % 10) as usize;
            let h = 2 + (lcg(&mut s) % 10) as usize;
            let src = random_grid(&mut s, w * h, -20.0, 20.0);
            let coeff = randf(&mut s, 0.0, 0.25);
            let dt = randf(&mut s, 0.0, 1.0);

            let mut fwd = vec![0.0f32; w * h];
            diffuse_step(&src, &mut fwd, w, h, coeff, dt);

            let mut rev = vec![0.0f32; w * h];
            for y in (0..h).rev() {
                for x in (0..w).rev() {
                    rev[idx(x, y, w)] = cell_next(&src, w, h, x, y, coeff, dt);
                }
            }
            assert_eq!(fwd, rev, "result must not depend on traversal order");
        }
    }

    #[test]
    fn spike_spreads_to_neighbours_and_conserves() {
        // A single hot cell in a zero field: centre drops, 4 neighbours rise,
        // total unchanged.
        let (w, h) = (5usize, 5usize);
        let mut src = vec![0.0f32; w * h];
        let cx = 2;
        let cy = 2;
        src[idx(cx, cy, w)] = 100.0;
        let mut dst = vec![0.0f32; w * h];
        diffuse_step(&src, &mut dst, w, h, 0.2, 1.0);

        assert!(dst[idx(cx, cy, w)] < 100.0, "centre should lose heat");
        assert!(dst[idx(cx - 1, cy, w)] > 0.0, "left neighbour should warm");
        assert!(dst[idx(cx + 1, cy, w)] > 0.0, "right neighbour should warm");
        assert!(dst[idx(cx, cy - 1, w)] > 0.0, "down neighbour should warm");
        assert!(dst[idx(cx, cy + 1, w)] > 0.0, "up neighbour should warm");
        // diagonals untouched in a single 4-neighbour step
        assert_eq!(dst[idx(cx - 1, cy - 1, w)], 0.0);
        let drift = (sum(&dst) - sum(&src)).abs();
        assert!(drift < 1e-3, "mass drift {}", drift);
    }

    #[test]
    fn dimension_mismatch_is_noop() {
        let src = vec![1.0f32; 6];
        let mut dst = vec![9.0f32; 6];
        // width*height (6) but wrong-length buffers, and a zero-area grid
        diffuse_step(&src, &mut dst, 3, 2, 0.2, 1.0); // ok dims → writes
        assert_ne!(dst, vec![9.0f32; 6]);
        let mut dst2 = vec![9.0f32; 5]; // wrong len for 3x2
        diffuse_step(&src, &mut dst2, 3, 2, 0.2, 1.0);
        assert_eq!(dst2, vec![9.0f32; 5], "mismatched dst must be left untouched");
        let mut dst3 = vec![9.0f32; 6];
        diffuse_step(&src, &mut dst3, 0, 0, 0.2, 1.0); // empty grid
        assert_eq!(dst3, vec![9.0f32; 6], "empty grid must be a no-op");
    }
}
