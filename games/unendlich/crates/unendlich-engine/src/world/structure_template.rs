//! `structure_template` — deterministic structure templates + footprint
//! placement scoring. **OPTIONAL.**
//!
//! Worker-03 / WORLD-PERF pure core, round 4. A structure (e.g. an underwater
//! forge or ruin) has a fixed footprint of cells; placement must fit the
//! footprint in bounds and prefer ground that is flat *under the whole
//! footprint*. This extends round 2/3's single-cell placement scoring with a
//! multi-cell footprint. Templates are generated deterministically from a seed.
//!
//! Pure, deterministic; no clock/rng/unwrap. Property-testable, not visual.
#![allow(dead_code)]

/// A structure footprint: `w × h` cells of block-id codes (0 = empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub w: u32,
    pub h: u32,
    pub cells: Vec<u8>,
}

/// Build a deterministic ruin-like template: solid wall ring, floor inside.
/// `wall` / `floor` are block-id codes. Deterministic in `(w, h, seed)`.
pub fn make_ruin(w: u32, h: u32, wall: u8, floor: u8, seed: u64) -> Template {
    let mut cells = vec![0u8; (w as usize) * (h as usize)];
    for y in 0..h {
        for x in 0..w {
            let edge = x == 0 || y == 0 || x + 1 == w || y + 1 == h;
            let idx = (y * w + x) as usize;
            if edge {
                // Deterministic "rubble": some wall cells have crumbled (empty).
                let h6 = hash(seed ^ ((x as u64) << 20) ^ (y as u64));
                cells[idx] = if h6 % 5 == 0 { 0 } else { wall };
            } else {
                cells[idx] = floor;
            }
        }
    }
    Template { w, h, cells }
}

/// A candidate placement (top-left origin of the footprint) and its score.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: u32,
    pub y: u32,
    pub score: f32,
}

/// Mean flatness under a `tw × th` footprint placed with top-left at
/// `(ox, oy)` on a `gw × gh` flatness grid (row-major, values in [0,1]).
/// Returns 0 if the footprint doesn't fit or the grid is too small.
pub fn footprint_score(
    flatness: &[f32],
    gw: u32,
    gh: u32,
    ox: u32,
    oy: u32,
    tw: u32,
    th: u32,
) -> f32 {
    if tw == 0 || th == 0 || ox + tw > gw || oy + th > gh {
        return 0.0;
    }
    let mut sum = 0.0f32;
    let mut n = 0u32;
    for y in oy..oy + th {
        for x in ox..ox + tw {
            let idx = (y * gw + x) as usize;
            if idx < flatness.len() {
                sum += clamp01(flatness[idx]);
                n += 1;
            }
        }
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}

/// Rank every in-bounds placement of a `tw × th` footprint on the grid by mean
/// flatness, returning the top `top_k`. All returned origins fit in bounds.
/// Deterministic: ties break by (row, then column).
pub fn rank_placements(
    flatness: &[f32],
    gw: u32,
    gh: u32,
    tw: u32,
    th: u32,
    top_k: usize,
) -> Vec<Placement> {
    let mut out: Vec<Placement> = Vec::new();
    if tw == 0 || th == 0 || tw > gw || th > gh {
        return out;
    }
    for oy in 0..=(gh - th) {
        for ox in 0..=(gw - tw) {
            out.push(Placement {
                x: ox,
                y: oy,
                score: footprint_score(flatness, gw, gh, ox, oy, tw, th),
            });
        }
    }
    out.sort_by(|a, b| {
        if a.score > b.score {
            std::cmp::Ordering::Less
        } else if a.score < b.score {
            std::cmp::Ordering::Greater
        } else {
            a.y.cmp(&b.y).then(a.x.cmp(&b.x))
        }
    });
    out.truncate(top_k);
    out
}

fn hash(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen_flat(gw: u32, gh: u32, seed: u64) -> Vec<f32> {
        let mut v = Vec::with_capacity((gw * gh) as usize);
        for i in 0..(gw * gh) as u64 {
            let h = hash(i ^ seed.wrapping_mul(40503));
            v.push(((h >> 8) % 1000) as f32 / 1000.0);
        }
        v
    }

    #[test]
    fn template_is_deterministic_and_sized() {
        let a = make_ruin(6, 4, 1, 2, 99);
        let b = make_ruin(6, 4, 1, 2, 99);
        assert_eq!(a, b);
        assert_eq!(a.cells.len(), 24);
        assert_eq!((a.w, a.h), (6, 4));
    }

    #[test]
    fn prop_sites_within_bounds() {
        let (gw, gh, tw, th) = (12u32, 9u32, 3u32, 2u32);
        let flat = gen_flat(gw, gh, 5);
        let sites = rank_placements(&flat, gw, gh, tw, th, 20);
        for p in &sites {
            assert!(p.x + tw <= gw && p.y + th <= gh, "footprint out of bounds");
        }
    }

    #[test]
    fn prop_score_monotone_in_suitability() {
        // Raising flatness everywhere under a footprint never lowers its score.
        let (gw, gh, tw, th) = (8u32, 8u32, 3u32, 3u32);
        let base = gen_flat(gw, gh, 7);
        let raised: Vec<f32> = base.iter().map(|&v| (v + 0.2).min(1.0)).collect();
        for oy in 0..=(gh - th) {
            for ox in 0..=(gw - tw) {
                let s0 = footprint_score(&base, gw, gh, ox, oy, tw, th);
                let s1 = footprint_score(&raised, gw, gh, ox, oy, tw, th);
                assert!(s1 >= s0 - 1e-6, "score not monotone in flatness");
            }
        }
    }

    #[test]
    fn prop_deterministic_and_sorted() {
        let (gw, gh) = (10u32, 10u32);
        let flat = gen_flat(gw, gh, 11);
        let a = rank_placements(&flat, gw, gh, 2, 2, 15);
        let b = rank_placements(&flat, gw, gh, 2, 2, 15);
        assert_eq!(a, b);
        for win in a.windows(2) {
            assert!(win[0].score >= win[1].score);
        }
        assert!(a.len() <= 15);
    }

    #[test]
    fn footprint_too_big_yields_no_sites() {
        let flat = gen_flat(4, 4, 1);
        assert!(rank_placements(&flat, 4, 4, 5, 5, 8).is_empty());
        assert_eq!(footprint_score(&flat, 4, 4, 0, 0, 5, 1), 0.0);
    }
}
