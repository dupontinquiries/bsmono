//! `river_carve` — turn flow accumulation into a carved channel profile.
//!
//! Worker-03 / WORLD-PERF pure core, round 9. `flow_accumulation` says how much
//! water funnels through a cell; this decides whether that's enough to be a
//! river and, if so, how wide/deep to carve. Width and depth grow *sub-linearly*
//! with accumulation (a river with 100× the catchment isn't 100× deeper) and are
//! capped, so big basins get broad valleys rather than bottomless trenches.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const WIDTH_BASE: f32 = 1.0;
const WIDTH_SCALE: f32 = 0.6;
const WIDTH_MAX: f32 = 32.0;

const DEPTH_BASE: f32 = 0.5;
const DEPTH_SCALE: f32 = 0.30;
const DEPTH_MAX: f32 = 12.0;

/// A carved channel cross-section at a cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Channel {
    pub width: f32,
    pub depth: f32,
    /// Carved bed height = `base_y - depth`.
    pub bed_y: f32,
}

/// Channel profile for a cell with upstream `accum`. Below `threshold` there's
/// no river (`None`); at or above it, width/depth grow with `sqrt(accum)`
/// (sub-linear) up to a cap, and the bed is carved below `base_y`.
pub fn channel(accum: u32, base_y: f32, threshold: u32) -> Option<Channel> {
    if accum < threshold {
        return None;
    }
    let over = (accum - threshold) as f32;
    let g = (over + 1.0).sqrt(); // sub-linear (concave) growth in accum
    let width = (WIDTH_BASE + WIDTH_SCALE * g).min(WIDTH_MAX);
    let depth = (DEPTH_BASE + DEPTH_SCALE * g).min(DEPTH_MAX);
    Some(Channel { width, depth, bed_y: base_y - depth })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_threshold_is_none() {
        assert!(channel(0, 64.0, 10).is_none());
        assert!(channel(9, 64.0, 10).is_none());
        assert!(channel(10, 64.0, 10).is_some()); // at threshold → carves
    }

    #[test]
    fn monotonic_in_accum() {
        let t = 5;
        let mut prev = channel(t, 64.0, t).unwrap();
        for a in (t + 1)..2000 {
            let c = channel(a, 64.0, t).unwrap();
            assert!(c.width >= prev.width - 1e-5);
            assert!(c.depth >= prev.depth - 1e-5);
            prev = c;
        }
    }

    #[test]
    fn depth_and_width_bounded() {
        let c = channel(u32::MAX, 64.0, 0).unwrap();
        assert!(c.depth <= DEPTH_MAX + 1e-4);
        assert!(c.width <= WIDTH_MAX + 1e-4);
        assert_eq!(c.depth, DEPTH_MAX);
        assert_eq!(c.width, WIDTH_MAX);
    }

    #[test]
    fn bed_is_below_base_when_carved() {
        let base = 70.0;
        let c = channel(100, base, 1).unwrap();
        assert!(c.depth > 0.0);
        assert!(c.bed_y < base);
        assert!((c.bed_y - (base - c.depth)).abs() < 1e-5);
    }

    #[test]
    fn growth_is_sublinear() {
        // Concave: equal accum steps give diminishing depth gains (before the cap).
        let t = 0;
        let d = |a: u32| channel(a, 0.0, t).unwrap().depth;
        let g1 = d(100) - d(0);
        let g2 = d(200) - d(100);
        let g3 = d(300) - d(200);
        assert!(g1 > g2 && g2 > g3, "depth gains should diminish (sub-linear)");
        // And clearly less than linear: depth at 400 ≪ 4× depth at 100.
        assert!(d(400) < 4.0 * d(100));
    }

    #[test]
    fn deterministic() {
        assert_eq!(channel(250, 64.0, 8), channel(250, 64.0, 8));
    }
}
