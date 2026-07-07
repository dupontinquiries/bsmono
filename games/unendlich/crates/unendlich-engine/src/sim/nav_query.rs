//! nav_query — the brain's cheap pathing read. A shared flow-field (computed once
//! per goal by the world tier — one field, many readers) gives a desired
//! direction at the mob's cell; this blends that with the mob's own learned
//! `path_memory` bin scores and its live `touch_rays` contacts to pick an actual
//! heading. No search per mob — just a sample and a blend, so it runs every tick.
//! Pure: the field's local direction and the bin data are passed in (the
//! coordinator borrows the real grid). Deterministic.

use std::f32::consts::{PI, TAU};

/// Heading bins, matching `path_memory::BINS`.
pub const BINS: usize = 16;

fn wrap_tau(a: f32) -> f32 {
    let mut x = a % TAU;
    if x < 0.0 {
        x += TAU;
    }
    x
}
fn ang_diff(a: f32, b: f32) -> f32 {
    let d = wrap_tau(a - b);
    if d > PI {
        TAU - d
    } else {
        d
    }
}
fn bin_heading(bin: usize) -> f32 {
    (bin as f32 + 0.5) / BINS as f32 * TAU
}

/// Heading (radians) the flow-field vector points along, on the X/Z plane. Returns
/// `None` for a near-zero vector (no gradient here).
pub fn flow_heading(flow_xz: [f32; 2]) -> Option<f32> {
    if flow_xz[0].abs() < 1e-6 && flow_xz[1].abs() < 1e-6 {
        None
    } else {
        Some(flow_xz[1].atan2(flow_xz[0]))
    }
}

/// Pick a heading. `flow_xz` is the field's local direction (toward the goal,
/// down-gradient); `bin_scores` (len ≤ `BINS`) are the mob's learned per-bin
/// confidences (higher = clearer, missing/short ⇒ treated as neutral 1.0);
/// `blocked` (len ≤ `BINS`) marks bins a `touch_rays` contact rules out this tick.
/// With a clear field and no blocks the result points down the flow toward the
/// goal; where the goal bin is blocked or low-scoring it veers to the best open
/// neighbour. If the field is flat (no gradient), falls back to `fallback`.
pub fn query(flow_xz: [f32; 2], bin_scores: &[f32], blocked: &[bool], fallback: f32) -> f32 {
    let desired = match flow_heading(flow_xz) {
        Some(h) => h,
        None => fallback,
    };
    let mut best_bin = 0usize;
    let mut best_w = f32::MIN;
    for b in 0..BINS {
        if blocked.get(b).copied().unwrap_or(false) {
            continue; // a live contact rules this bin out
        }
        let score = bin_scores.get(b).copied().unwrap_or(1.0);
        let aff = 0.5 * (1.0 + ang_diff(bin_heading(b), desired).cos()); // 1 toward goal, 0 away
        let w = aff * score;
        if w > best_w {
            best_w = w;
            best_bin = b;
        }
    }
    if best_w == f32::MIN {
        // every bin blocked — keep the desired heading rather than stall
        desired
    } else {
        bin_heading(best_bin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }
    fn bin_of(h: f32) -> usize {
        ((wrap_tau(h) / TAU * BINS as f32) as usize) % BINS
    }

    #[test]
    fn clear_field_points_down_gradient_toward_goal() {
        // flow toward +X (heading 0); all clear
        let scores = [1.0; BINS];
        let blocked = [false; BINS];
        let h = query([1.0, 0.0], &scores, &blocked, 0.0);
        assert!(ang_diff(h, 0.0) <= TAU / BINS as f32, "heads along the flow (~0): {h}");
    }

    #[test]
    fn a_blocked_goal_bin_biases_away() {
        let scores = [1.0; BINS];
        let mut blocked = [false; BINS];
        let goal_bin = bin_of(0.0); // flow toward +X
        blocked[goal_bin] = true;
        let h = query([1.0, 0.0], &scores, &blocked, 0.0);
        assert_ne!(bin_of(h), goal_bin, "avoids the blocked goal bin");
        assert!(ang_diff(h, 0.0) < PI * 0.6, "veers to a near neighbour, not backward");
    }

    #[test]
    fn low_score_bins_are_avoided() {
        let mut scores = [1.0; BINS];
        let goal_bin = bin_of(0.0);
        scores[goal_bin] = 0.01; // learned-bad
        let h = query([1.0, 0.0], &scores, &[false; BINS], 0.0);
        assert_ne!(bin_of(h), goal_bin, "shuns the low-scoring goal bin");
    }

    #[test]
    fn flat_field_uses_fallback() {
        let h = query([0.0, 0.0], &[1.0; BINS], &[false; BINS], 1.234);
        // desired == fallback; best bin is the one nearest fallback
        assert!(ang_diff(h, 1.234) <= TAU / BINS as f32, "falls back when no gradient");
    }

    #[test]
    fn prop_heading_finite_and_deterministic() {
        let mut seed = 0x4A12_0099u32;
        for _ in 0..4000 {
            let flow = [lcg(&mut seed) * 2.0 - 1.0, lcg(&mut seed) * 2.0 - 1.0];
            let mut scores = [0.0; BINS];
            let mut blocked = [false; BINS];
            for b in 0..BINS {
                scores[b] = lcg(&mut seed);
                blocked[b] = lcg(&mut seed) < 0.2;
            }
            let fb = lcg(&mut seed) * TAU;
            let a = query(flow, &scores, &blocked, fb);
            let b = query(flow, &scores, &blocked, fb);
            assert_eq!(a, b, "deterministic");
            assert!(a.is_finite(), "finite heading");
        }
    }
}
