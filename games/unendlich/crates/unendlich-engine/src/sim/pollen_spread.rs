//! `sim::pollen_spread` — one step of pollen drifting and diffusing across a
//! grid, the CPU reference a GPU field-kernel will mirror (see
//! `GPU_COMPUTE_PLAN`).
//!
//! [`spread_step`] updates a `w × h` scalar field in place: each cell blends a
//! little toward its neighbours (diffusion), with the blend biased toward the
//! upwind side so the whole pattern drifts downwind (advection). A uniform field
//! stays uniform, a point of pollen spreads out and, under wind, its centre of
//! mass shifts downwind. The update is a convex blend, so values stay within the
//! field's existing range (no negatives appear) and total mass is conserved
//! exactly in the still-air interior.
//!
//! Pure and deterministic: a double-buffered gather over 4-neighbours — no clock,
//! no randomness, no cross-imports.

fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    let x = if x > lo { x } else { lo };
    if x < hi {
        x
    } else {
        hi
    }
}
fn pos(x: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

/// Advect + diffuse the field one step.
///
/// `rate` (clamped `0..1`) is how far each cell blends toward its neighbours;
/// `wind` biases the gather toward the upwind side so mass drifts downwind. A
/// malformed call (`w*h != field.len()`, or a zero dimension) is a no-op.
pub fn spread_step(field: &mut [f32], w: usize, h: usize, wind: [f32; 2], rate: f32) {
    if w == 0 || h == 0 || field.len() != w * h {
        return;
    }
    let rate = clampf(rate, 0.0, 1.0);
    // upwind-heavy neighbour weights (wind +x ⇒ pull harder from the west)
    let w_west = 1.0 + pos(wind[0]);
    let w_east = 1.0 + pos(-wind[0]);
    let w_south = 1.0 + pos(wind[1]);
    let w_north = 1.0 + pos(-wind[1]);

    let mut next = vec![0.0f32; field.len()];
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let c = y * w + x;
            let mut acc = 0.0f32;
            let mut wsum = 0.0f32;
            if x > 0 {
                acc += w_west * field[c - 1];
                wsum += w_west;
            }
            if x + 1 < w {
                acc += w_east * field[c + 1];
                wsum += w_east;
            }
            if y > 0 {
                acc += w_south * field[c - w];
                wsum += w_south;
            }
            if y + 1 < h {
                acc += w_north * field[c + w];
                wsum += w_north;
            }
            let blend = if wsum > 0.0 { acc / wsum } else { field[c] };
            next[c] = (1.0 - rate) * field[c] + rate * blend;
            x += 1;
        }
        y += 1;
    }
    field.copy_from_slice(&next);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mass(f: &[f32]) -> f32 {
        let mut s = 0.0;
        let mut i = 0;
        while i < f.len() {
            s += f[i];
            i += 1;
        }
        s
    }

    #[test]
    fn uniform_stays_uniform() {
        let mut f = vec![3.0f32; 25];
        spread_step(&mut f, 5, 5, [0.7, -0.2], 0.5);
        for v in &f {
            assert!((v - 3.0).abs() < 1e-5, "drifted from uniform: {}", v);
        }
    }

    #[test]
    fn point_spreads() {
        let mut f = vec![0.0f32; 49]; // 7x7
        f[3 * 7 + 3] = 1.0; // center
        spread_step(&mut f, 7, 7, [0.0, 0.0], 0.4);
        // center dropped, orthogonal neighbours gained
        assert!(f[3 * 7 + 3] < 1.0);
        assert!(f[3 * 7 + 2] > 0.0 && f[3 * 7 + 4] > 0.0);
        assert!(f[2 * 7 + 3] > 0.0 && f[4 * 7 + 3] > 0.0);
    }

    #[test]
    fn wind_shifts_mass_downwind() {
        let mut f = vec![0.0f32; 49];
        f[3 * 7 + 3] = 1.0;
        spread_step(&mut f, 7, 7, [1.0, 0.0], 0.5); // wind toward +x (east)
        let east = f[3 * 7 + 4];
        let west = f[3 * 7 + 2];
        assert!(east > west, "east {} should exceed west {}", east, west);
    }

    #[test]
    fn mass_conserved_still_air_interior() {
        // point well inside a large grid, no wind, a few steps → mass preserved
        let (w, h) = (15usize, 15usize);
        let mut f = vec![0.0f32; w * h];
        f[7 * w + 7] = 10.0;
        let m0 = mass(&f);
        for _ in 0..3 {
            spread_step(&mut f, w, h, [0.0, 0.0], 0.2);
        }
        assert!((mass(&f) - m0).abs() < 1e-3, "mass drifted: {} -> {}", m0, mass(&f));
    }

    #[test]
    fn rate_scales_spreading() {
        let center = 3 * 7 + 3;
        let mut slow = vec![0.0f32; 49];
        slow[center] = 1.0;
        let mut fast = slow.clone();
        spread_step(&mut slow, 7, 7, [0.0, 0.0], 0.1);
        spread_step(&mut fast, 7, 7, [0.0, 0.0], 0.5);
        // a higher rate drains the center cell more
        assert!(fast[center] < slow[center]);
    }

    #[test]
    fn deterministic() {
        let mut a = vec![0.0f32; 49];
        a[10] = 1.0;
        a[20] = 2.0;
        let mut b = a.clone();
        spread_step(&mut a, 7, 7, [0.3, 0.4], 0.35);
        spread_step(&mut b, 7, 7, [0.3, 0.4], 0.35);
        assert_eq!(a, b);
    }

    #[test]
    fn malformed_is_noop() {
        let mut f = vec![1.0f32, 2.0, 3.0];
        spread_step(&mut f, 2, 2, [0.0, 0.0], 0.5); // len 3 != 2*2
        assert_eq!(f, vec![1.0, 2.0, 3.0]);
    }
}
