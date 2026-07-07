//! viewport_layout.rs — pure splitscreen layout solver.
//!
//! Given a player count and a framebuffer size, produce per-player viewport
//! rects (integer pixels) that **exactly tile** the framebuffer — no overlaps,
//! no gaps — with reasonably balanced areas and a tiling chosen to keep each
//! sub-viewport's aspect close to the framebuffer's. Pure integer/float math:
//! no clocks, no RNG, no rendering types (R1/R11/R21), fully deterministic.
//!
//! The coordinator consumes these rects when setting per-player scissor/viewport
//! in the world pass; this module just decides the geometry.

/// An integer pixel rectangle within the framebuffer. `x + w <= fb_w` and
/// `y + h <= fb_h` always hold for solver output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    #[inline]
    pub fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }
}

/// Split `len` into `parts` contiguous integer spans `(start, size)` that sum to
/// exactly `len` (remainder pixels go to the earliest spans). `parts` is assumed
/// >= 1 by callers; a 0 is treated as 1 to stay panic-free (R21).
fn split(len: u32, parts: u32) -> Vec<(u32, u32)> {
    let parts = parts.max(1);
    let base = len / parts;
    let rem = len % parts;
    let mut out = Vec::with_capacity(parts as usize);
    let mut pos = 0u32;
    for i in 0..parts {
        let sz = base + if i < rem { 1 } else { 0 };
        out.push((pos, sz));
        pos += sz;
    }
    out
}

/// Choose the row count whose resulting cells best match the framebuffer aspect.
/// Cells per row are distributed as evenly as possible, so the widest row has
/// `ceil(n/rows)` cells. Ties resolve to fewer rows (deterministic).
fn choose_rows(n: u32, fb_w: u32, fb_h: u32) -> u32 {
    let fb_aspect_ln = (fb_w as f32 / fb_h as f32).ln();
    let mut best_rows = 1u32;
    let mut best_dev = f32::INFINITY;
    for rows in 1..=n {
        let max_cols = (n + rows - 1) / rows; // widest row
        let cell_w = fb_w as f32 / max_cols as f32;
        let cell_h = fb_h as f32 / rows as f32;
        let dev = ((cell_w / cell_h).ln() - fb_aspect_ln).abs();
        if dev < best_dev - 1e-6 {
            best_dev = dev;
            best_rows = rows;
        }
    }
    best_rows
}

/// Solve viewport rects for `players` over a `fb_w` x `fb_h` framebuffer.
///
/// Guarantees: exactly `max(players,1)` rects; they tile the framebuffer with no
/// overlaps and no gaps (their areas sum to `fb_w*fb_h`); each rect lies fully
/// within the framebuffer with positive size; cell areas differ by at most one
/// "cells-per-row" step (balanced within ~2x). 1 player = full screen.
pub fn layout(players: u32, fb_w: u32, fb_h: u32) -> Vec<Rect> {
    let n = players.max(1);
    let (w, h) = (fb_w.max(1), fb_h.max(1));

    let rows = choose_rows(n, w, h);
    let base = n / rows; // cells in a "short" row (>= 1 since rows <= n)
    let extra = n % rows; // first `extra` rows get one more cell

    let row_spans = split(h, rows);
    let mut rects = Vec::with_capacity(n as usize);
    for r in 0..rows {
        let ncells = base + if r < extra { 1 } else { 0 };
        let (ry, rh) = row_spans[r as usize];
        for (cx, cw) in split(w, ncells) {
            rects.push(Rect { x: cx, y: ry, w: cw, h: rh });
        }
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: &[(u32, u32)] = &[
        (1920, 1080),
        (1280, 800),
        (800, 1280),
        (2560, 1080),
        (1000, 1000),
        (1366, 768),
    ];

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        let ax2 = a.x + a.w;
        let ay2 = a.y + a.h;
        let bx2 = b.x + b.w;
        let by2 = b.y + b.h;
        a.x < bx2 && b.x < ax2 && a.y < by2 && b.y < ay2
    }

    #[test]
    fn prop_exact_tiling_no_overlap_no_gap() {
        for &(w, h) in SIZES {
            for n in 1..=16u32 {
                let rects = layout(n, w, h);
                assert_eq!(rects.len(), n as usize, "count n={} {}x{}", n, w, h);
                // in bounds + positive
                let mut sum = 0u64;
                for r in &rects {
                    assert!(r.w > 0 && r.h > 0, "degenerate rect n={}", n);
                    assert!(r.x + r.w <= w && r.y + r.h <= h, "out of bounds n={}", n);
                    sum += r.area();
                }
                // pairwise non-overlapping
                for i in 0..rects.len() {
                    for j in (i + 1)..rects.len() {
                        assert!(!overlaps(&rects[i], &rects[j]), "overlap n={} {} {}", n, i, j);
                    }
                }
                // no gaps: in-bounds + non-overlapping + areas summing to the
                // whole framebuffer => exact cover.
                assert_eq!(sum, w as u64 * h as u64, "area mismatch n={} {}x{}", n, w, h);
            }
        }
    }

    #[test]
    fn single_player_is_fullscreen() {
        for &(w, h) in SIZES {
            let r = layout(1, w, h);
            assert_eq!(r.len(), 1);
            assert_eq!(r[0], Rect { x: 0, y: 0, w, h });
        }
    }

    #[test]
    fn zero_players_clamps_to_one() {
        let r = layout(0, 1920, 1080);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], Rect { x: 0, y: 0, w: 1920, h: 1080 });
    }

    #[test]
    fn prop_areas_balanced() {
        // On framebuffers large enough that integer rounding is negligible, the
        // largest viewport is at most ~2x the smallest (even cell distribution).
        for &(w, h) in SIZES {
            for n in 1..=12u32 {
                let rects = layout(n, w, h);
                let mut lo = u64::MAX;
                let mut hi = 0u64;
                for r in &rects {
                    lo = lo.min(r.area());
                    hi = hi.max(r.area());
                }
                assert!(
                    hi as f64 <= 2.0 * lo as f64 + (w + h) as f64,
                    "unbalanced n={} {}x{}: hi={} lo={}",
                    n, w, h, hi, lo
                );
            }
        }
    }

    #[test]
    fn prop_deterministic() {
        for &(w, h) in SIZES {
            for n in 1..=16u32 {
                assert_eq!(layout(n, w, h), layout(n, w, h));
            }
        }
    }
}
