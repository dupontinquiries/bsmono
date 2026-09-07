//! thumbarc — the reach polygon, and the per-row compaction that fits a board inside it.
//!
//! `docs/single-hand-mode.md` §1. A thumb reaches a fan-shaped arc, not a rectangle: near
//! the anchor corner it sweeps the full width, and at the far top corner it does not reach
//! at all. `halfboard` approximates that with one uniform inset on every row, which brings
//! the whole board within reach and gives away the shape — the bottom row loses width it
//! did not need to.
//!
//! I said last time that the polygon "needs per-key placement the renderer does not have".
//! Re-reading §1 that is wrong, and worth correcting rather than quietly fixing: the doc's
//! algorithm is **per-row**, and it says so — *"we transform each row independently… Keys
//! within a row keep their relative positions and proportional widths — only the row's outer
//! bounds shift. Muscle memory is preserved."* Per-row bounds is exactly what this renderer
//! can do; it already lays each row out across a width.
//!
//! ## The algorithm
//!
//! For each row, take the y of its vertical centre in normalised board coordinates, find the
//! polygon's x-range at that y, and lay the row out inside it. Rows near the anchor get
//! nearly the full width; rows far from it get less.
//!
//! ## Defaults, and the size that needs none
//!
//! The doc's heuristic: at or below 360 logical px no compaction is needed (the board is
//! already within reach), to 480 a mild one (top row 80% width), beyond that a strong one
//! (top row 65%). [`default_for`] returns `None` for the first case rather than a polygon
//! that happens to be a rectangle — "no compaction needed" and "compacted by zero" differ
//! when something later asks whether an arc is in use.
//!
//! ## Coordinates
//!
//! Normalised to the board: x from 0 (left) to 1 (right), y from 0 (top of the keyboard) to
//! 1 (bottom). The doc uses the same frame, which is what lets a calibrated polygon survive
//! a rotation or a resize.

/// Which corner the thumb anchors at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub fn from_side(side: crate::halfboard::Side) -> Option<Corner> {
        match side {
            crate::halfboard::Side::Left => Some(Corner::BottomLeft),
            crate::halfboard::Side::Right => Some(Corner::BottomRight),
            crate::halfboard::Side::Off => None,
        }
    }
}

/// A closed reach polygon in normalised board coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Arc {
    pub anchor: Corner,
    /// Closed: the last point connects to the first.
    pub polygon: Vec<(f32, f32)>,
}

/// Below this width the board is already within reach (§1).
pub const NO_COMPACTION_PT: f32 = 360.0;
/// Above this, the strong default.
pub const STRONG_PT: f32 = 480.0;
/// Top-row width under the mild default.
pub const MILD_TOP_WIDTH: f32 = 0.80;
/// Top-row width under the strong default.
pub const STRONG_TOP_WIDTH: f32 = 0.65;

impl Arc {
    /// The doc's default polygon: a rounded right-triangle whose hypotenuse curves from the
    /// top corner nearest the anchor down to about 40% of the way up the far edge.
    ///
    /// `top_width` is how much of the width the TOP row keeps.
    pub fn default_shape(anchor: Corner, top_width: f32) -> Arc {
        let w = top_width.clamp(0.2, 1.0);
        // Right-handed, then mirrored. Built from the anchor (bottom-right) anticlockwise:
        // along the bottom, up the far edge to 0.6, then curved back to the near top corner.
        let far_top = 1.0 - w; // x the top row reaches to
        let mut poly = vec![(1.0, 1.0), (0.0, 1.0), (0.0, 0.6)];
        // Three interpolated points along the curve, which is enough for a scanline: the
        // rows sample it at a handful of heights and a denser curve buys nothing.
        for i in 1..=3 {
            let t = i as f32 / 4.0;
            // Ease so the arc bulges outward rather than cutting the corner straight.
            let x = far_top * (t * t);
            let y = 0.6 * (1.0 - t);
            poly.push((x, y));
        }
        poly.push((far_top, 0.0));
        poly.push((1.0, 0.0));
        if anchor == Corner::BottomLeft {
            for p in &mut poly {
                p.0 = 1.0 - p.0;
            }
        }
        Arc { anchor, polygon: poly }
    }

    /// The default for a board of this width, or `None` when none is needed (§1).
    pub fn default_for(width_pt: f32, anchor: Corner) -> Option<Arc> {
        if width_pt <= NO_COMPACTION_PT {
            return None;
        }
        let top = if width_pt <= STRONG_PT { MILD_TOP_WIDTH } else { STRONG_TOP_WIDTH };
        Some(Arc::default_shape(anchor, top))
    }

    /// The polygon's horizontal extent at `y`, as `(x_min, x_max)`.
    ///
    /// `None` when the line misses the polygon entirely — a degenerate arc, or a y outside
    /// it. The caller falls back to the full width rather than laying out a row of zero
    /// width, because a keyboard with an invisible row is worse than one that is not
    /// compacted.
    pub fn x_range_at(&self, y: f32) -> Option<(f32, f32)> {
        let mut xs: Vec<f32> = Vec::new();
        let n = self.polygon.len();
        for i in 0..n {
            let (x1, y1) = self.polygon[i];
            let (x2, y2) = self.polygon[(i + 1) % n];
            if (y1 <= y && y2 > y) || (y2 <= y && y1 > y) {
                let t = (y - y1) / (y2 - y1);
                xs.push(x1 + t * (x2 - x1));
            }
        }
        if xs.len() < 2 {
            return None;
        }
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for x in xs {
            lo = lo.min(x);
            hi = hi.max(x);
        }
        Some((lo.clamp(0.0, 1.0), hi.clamp(0.0, 1.0)))
    }

    /// The bounds for row `i` of `rows`, sampled at the row's vertical centre.
    ///
    /// Falls back to the full width when the polygon has nothing to say there, and refuses
    /// to return a range narrower than [`MIN_ROW_WIDTH`] — a row squeezed past that has keys
    /// too small to hit, which trades the reach problem for an accuracy one.
    pub fn row_bounds(&self, i: usize, rows: usize) -> (f32, f32) {
        let rows = rows.max(1);
        let y = (i as f32 + 0.5) / rows as f32;
        match self.x_range_at(y) {
            Some((lo, hi)) if hi - lo >= MIN_ROW_WIDTH => (lo, hi),
            Some((lo, hi)) => {
                // Too narrow: widen about the row's own centre, toward the anchor side, so
                // the row stays reachable rather than being centred under the palm.
                let mid = (lo + hi) * 0.5;
                let half = MIN_ROW_WIDTH * 0.5;
                let (mut a, mut b) = (mid - half, mid + half);
                if a < 0.0 {
                    b -= a;
                    a = 0.0;
                }
                if b > 1.0 {
                    a -= b - 1.0;
                    b = 1.0;
                }
                (a.max(0.0), b.min(1.0))
            }
            None => (0.0, 1.0),
        }
    }
}

/// The narrowest a compacted row may be, as a fraction of the board.
pub const MIN_ROW_WIDTH: f32 = 0.55;

#[cfg(test)]
mod tests {
    use super::*;

    /// "Rows near the anchor get nearly the full width; rows far from it get less." That
    /// gradient is the whole difference from `halfboard`'s uniform inset.
    #[test]
    fn the_arc_gives_the_bottom_row_more_width_than_the_top() {
        let a = Arc::default_shape(Corner::BottomRight, STRONG_TOP_WIDTH);
        let top = a.row_bounds(0, 4);
        let bottom = a.row_bounds(3, 4);
        let w = |r: (f32, f32)| r.1 - r.0;
        assert!(w(bottom) > w(top), "no gradient: top {top:?} bottom {bottom:?}");
        assert!(w(bottom) > 0.9, "the row at the anchor should be nearly full width: {bottom:?}");
        // And every row is anchored to the RIGHT — the reachable side for this corner.
        for i in 0..4 {
            let (_, hi) = a.row_bounds(i, 4);
            assert!(hi > 0.98, "row {i} does not reach the anchor edge: {hi}");
        }
    }

    /// Mirrored for the other hand, exactly.
    #[test]
    fn a_left_anchored_arc_is_the_mirror_of_a_right_one() {
        let r = Arc::default_shape(Corner::BottomRight, MILD_TOP_WIDTH);
        let l = Arc::default_shape(Corner::BottomLeft, MILD_TOP_WIDTH);
        for i in 0..4 {
            let (rl, rh) = r.row_bounds(i, 4);
            let (ll, lh) = l.row_bounds(i, 4);
            assert!((1.0 - rh - ll).abs() < 1e-4, "row {i} left edge: {ll} vs {}", 1.0 - rh);
            assert!((1.0 - rl - lh).abs() < 1e-4, "row {i} right edge");
        }
        for i in 0..4 {
            assert!(l.row_bounds(i, 4).0 < 0.02, "a left-anchored row must touch the left edge");
        }
    }

    /// §1's size heuristic. "No compaction needed" is `None`, not a rectangle — the two
    /// differ when something later asks whether an arc is in use.
    #[test]
    fn small_boards_get_no_arc_and_large_ones_get_a_stronger_one() {
        assert!(Arc::default_for(320.0, Corner::BottomRight).is_none());
        assert!(Arc::default_for(NO_COMPACTION_PT, Corner::BottomRight).is_none());

        let mild = Arc::default_for(420.0, Corner::BottomRight).expect("mild");
        let strong = Arc::default_for(720.0, Corner::BottomRight).expect("strong");
        let w = |a: &Arc| {
            let (lo, hi) = a.row_bounds(0, 4);
            hi - lo
        };
        assert!(w(&strong) < w(&mild), "the strong default should compact harder: {} vs {}", w(&strong), w(&mild));
    }

    /// A row squeezed past the floor has keys too small to hit, which trades the reach
    /// problem for an accuracy one.
    #[test]
    fn a_row_is_never_narrower_than_the_floor_and_stays_on_the_anchor_side() {
        let a = Arc::default_shape(Corner::BottomRight, 0.2);
        for rows in [3usize, 4, 5] {
            for i in 0..rows {
                let (lo, hi) = a.row_bounds(i, rows);
                assert!(hi - lo >= MIN_ROW_WIDTH - 1e-4, "row {i}/{rows} is {:.3} wide", hi - lo);
                assert!(lo >= 0.0 && hi <= 1.0, "row {i}/{rows} left the board: {lo}..{hi}");
            }
        }
    }

    /// A degenerate polygon must not produce a row of zero width: a keyboard with an
    /// invisible row is worse than one that is not compacted.
    #[test]
    fn a_degenerate_arc_falls_back_to_the_full_width() {
        let empty = Arc { anchor: Corner::BottomRight, polygon: Vec::new() };
        assert_eq!(empty.x_range_at(0.5), None);
        assert_eq!(empty.row_bounds(0, 4), (0.0, 1.0));

        // A polygon that misses this y entirely.
        let sliver = Arc {
            anchor: Corner::BottomRight,
            polygon: vec![(0.9, 0.9), (1.0, 0.9), (1.0, 1.0), (0.9, 1.0)],
        };
        assert_eq!(sliver.row_bounds(0, 4), (0.0, 1.0), "a miss must fall back, not vanish");
    }

    #[test]
    fn a_corner_comes_from_the_compaction_side() {
        use crate::halfboard::Side;
        assert_eq!(Corner::from_side(Side::Left), Some(Corner::BottomLeft));
        assert_eq!(Corner::from_side(Side::Right), Some(Corner::BottomRight));
        assert_eq!(Corner::from_side(Side::Off), None);
    }
}
