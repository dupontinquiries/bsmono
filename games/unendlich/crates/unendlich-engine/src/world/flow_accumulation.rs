//! `flow_accumulation` — D8 drainage accumulation (where rivers form).
//!
//! Worker-03 / WORLD-PERF pure core, round 8. Over a height grid, each cell
//! drains to its lowest 8-neighbor (D8); accumulation is the number of cells
//! that drain *through* each cell (itself + everything upstream). High
//! accumulation marks valleys/river channels — feeds river placement, extends
//! `flow_field` / `river_source`.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const NEIGH8: [(isize, isize); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// D8 flow accumulation. `height` is row-major `w × h`. Returns a per-cell
/// upstream count (≥ 1, including the cell itself). Cells with no strictly
/// lower neighbor are pits (basin outlets) and collect their whole basin.
pub fn flow_accum(height: &[f32], w: usize, h: usize) -> Vec<u32> {
    let n = w * h;
    if n == 0 || height.len() < n {
        return Vec::new();
    }

    // Downstream target per cell (None = pit).
    let mut down: Vec<Option<usize>> = vec![None; n];
    for y in 0..h {
        for x in 0..w {
            let c = y * w + x;
            let hc = height[c];
            let mut best: Option<(f32, usize)> = None;
            for &(dx, dy) in &NEIGH8 {
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let nc = (ny as usize) * w + nx as usize;
                let hn = height[nc];
                match best {
                    None => best = Some((hn, nc)),
                    Some((bh, _)) => {
                        if hn < bh {
                            best = Some((hn, nc));
                        }
                    }
                }
            }
            if let Some((bh, nc)) = best {
                if bh < hc {
                    down[c] = Some(nc);
                }
            }
        }
    }

    // Process cells high → low so a cell's upstream total is complete before it
    // pushes downstream. Tie-break by index for determinism.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let (ha, hb) = (height[a], height[b]);
        if ha > hb {
            std::cmp::Ordering::Less
        } else if ha < hb {
            std::cmp::Ordering::Greater
        } else {
            a.cmp(&b)
        }
    });

    let mut accum = vec![1u32; n];
    for &c in &order {
        if let Some(d) = down[c] {
            accum[d] = accum[d].saturating_add(accum[c]);
        }
    }
    accum
}

/// Indices of pit cells (no downstream) — the basin outlets.
pub fn pits(height: &[f32], w: usize, h: usize) -> Vec<usize> {
    let n = w * h;
    let mut out = Vec::new();
    if n == 0 || height.len() < n {
        return out;
    }
    for y in 0..h {
        for x in 0..w {
            let c = y * w + x;
            let hc = height[c];
            let mut has_lower = false;
            for &(dx, dy) in &NEIGH8 {
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                if height[(ny as usize) * w + nx as usize] < hc {
                    has_lower = true;
                    break;
                }
            }
            if !has_lower {
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_is_all_ones() {
        let a = flow_accum(&vec![5.0f32; 16], 4, 4);
        assert!(a.iter().all(|&v| v == 1));
    }

    #[test]
    fn linear_slope_accumulates_downhill() {
        // 1×5 row descending left→right: [4,3,2,1,0].
        let a = flow_accum(&[4.0, 3.0, 2.0, 1.0, 0.0], 5, 1);
        assert_eq!(a, vec![1, 2, 3, 4, 5]);
        // outlet (lowest cell) has the max == w*h.
        assert_eq!(a[4], 5);
    }

    #[test]
    fn single_outlet_collects_everything() {
        // height = x + y → unique global min at (0,0); everything drains there.
        let (w, h) = (4usize, 4usize);
        let mut height = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                height[y * w + x] = (x + y) as f32;
            }
        }
        let a = flow_accum(&height, w, h);
        assert_eq!(a[0], (w * h) as u32); // (0,0) collects all 16
        assert_eq!(*a.iter().max().unwrap(), (w * h) as u32);
    }

    #[test]
    fn pit_accums_partition_the_grid() {
        // Every cell drains to exactly one pit, so the pit accumulations sum to
        // w*h regardless of the terrain.
        let (w, h) = (6usize, 5usize);
        let mut height = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                // a couple of basins via a bumpy field (deterministic)
                let v = ((x * 7 + y * 13) % 11) as f32 + (x as f32) * 0.01;
                height[y * w + x] = v;
            }
        }
        let a = flow_accum(&height, w, h);
        let pit_sum: u32 = pits(&height, w, h).iter().map(|&p| a[p]).sum();
        assert_eq!(pit_sum, (w * h) as u32);
    }

    #[test]
    fn border_cells_handled() {
        // 3×3 bowl: center lowest. No panic; center collects, all ≥ 1.
        let height = [9.0, 8.0, 9.0, 8.0, 0.0, 8.0, 9.0, 8.0, 9.0];
        let a = flow_accum(&height, 3, 3);
        assert!(a.iter().all(|&v| v >= 1));
        assert_eq!(a[4], 9); // center pit collects all
    }

    #[test]
    fn deterministic() {
        let height = [2.0, 1.0, 3.0, 0.0, 5.0, 4.0, 1.0, 2.0, 0.5];
        assert_eq!(flow_accum(&height, 3, 3), flow_accum(&height, 3, 3));
    }
}
