//! `locate` — nearest matching cell in a sampled region grid (for `/find`).
//!
//! Worker-03 / WORLD-WORLDGEN pure core, round 6. Scans a row-major grid of
//! tags (structure / biome / mob codes) for the cell nearest an origin that
//! matches a target tag — the query behind a `/find <thing>` command. Manhattan
//! distance; ties broken deterministically by (row, then column).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// A located cell and its Manhattan distance from the query origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub x: u32,
    pub y: u32,
    pub dist: u32,
}

/// Find the cell matching `target` nearest to `(ox, oy)` in a `w × h` tag grid
/// (row-major). Manhattan distance; ties → lowest `y`, then lowest `x`. Returns
/// `None` if no cell matches.
pub fn find_nearest(tags: &[u32], w: u32, h: u32, ox: i32, oy: i32, target: u32) -> Option<Found> {
    let wu = w as usize;
    let hu = h as usize;
    if tags.len() < wu * hu {
        return None;
    }
    let mut best: Option<Found> = None;
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            if tags[idx] != target {
                continue;
            }
            let dist = manhattan(ox, oy, x as i32, y as i32);
            let cand = Found { x, y, dist };
            best = Some(match best {
                None => cand,
                Some(b) => better(b, cand),
            });
        }
    }
    best
}

/// All cells matching `target`, in scan order (row-major). Useful for `/find`
/// variants that list every match.
pub fn find_all(tags: &[u32], w: u32, h: u32, target: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let wu = w as usize;
    let hu = h as usize;
    if tags.len() < wu * hu {
        return out;
    }
    for y in 0..h {
        for x in 0..w {
            if tags[(y * w + x) as usize] == target {
                out.push((x, y));
            }
        }
    }
    out
}

fn manhattan(ax: i32, ay: i32, bx: i32, by: i32) -> u32 {
    ((ax - bx).unsigned_abs()) + ((ay - by).unsigned_abs())
}

// Keep the nearer; on a tie keep the lower (y, then x).
fn better(a: Found, b: Found) -> Found {
    if b.dist < a.dist {
        b
    } else if b.dist > a.dist {
        a
    } else if (b.y, b.x) < (a.y, a.x) {
        b
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a w×h grid with `target` placed at the given coords.
    fn grid_with(w: u32, h: u32, target: u32, at: &[(u32, u32)]) -> Vec<u32> {
        let mut g = vec![0u32; (w * h) as usize];
        for &(x, y) in at {
            g[(y * w + x) as usize] = target;
        }
        g
    }

    #[test]
    fn finds_the_nearest_match() {
        let w = 10;
        let h = 10;
        let g = grid_with(w, h, 5, &[(1, 1), (8, 8), (3, 0)]);
        let f = find_nearest(&g, w, h, 0, 0, 5).unwrap();
        assert_eq!((f.x, f.y), (1, 1)); // dist 2, nearest to origin
        assert_eq!(f.dist, 2);
    }

    #[test]
    fn none_when_absent() {
        let g = vec![0u32; 100];
        assert!(find_nearest(&g, 10, 10, 5, 5, 9).is_none());
    }

    #[test]
    fn ties_broken_deterministically() {
        let w = 5;
        let h = 5;
        // Two matches equidistant (Manhattan 2) from origin (2,2): (0,2) and (2,0).
        let g = grid_with(w, h, 7, &[(0, 2), (2, 0)]);
        let f = find_nearest(&g, w, h, 2, 2, 7).unwrap();
        // Tie → lower y first: (2,0) has y=0 < (0,2) y=2.
        assert_eq!((f.x, f.y), (2, 0));
        // Deterministic.
        let f2 = find_nearest(&g, w, h, 2, 2, 7).unwrap();
        assert_eq!(f, f2);
    }

    #[test]
    fn find_all_lists_in_scan_order() {
        let w = 4;
        let h = 4;
        let g = grid_with(w, h, 3, &[(3, 0), (0, 0), (1, 2)]);
        let all = find_all(&g, w, h, 3);
        assert_eq!(all, vec![(0, 0), (3, 0), (1, 2)]); // row-major order
    }

    #[test]
    fn prop_returns_a_true_minimum() {
        // Brute check: the returned distance equals the min over all matches.
        let w = 12;
        let h = 9;
        let g = grid_with(w, h, 2, &[(11, 8), (5, 5), (0, 0), (6, 4)]);
        let (ox, oy) = (6, 6);
        let f = find_nearest(&g, w, h, ox, oy, 2).unwrap();
        let mut brute = u32::MAX;
        for &(x, y) in &[(11u32, 8u32), (5, 5), (0, 0), (6, 4)] {
            let d = ((ox - x as i32).unsigned_abs()) + ((oy - y as i32).unsigned_abs());
            if d < brute {
                brute = d;
            }
        }
        assert_eq!(f.dist, brute);
    }
}
