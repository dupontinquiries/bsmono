//! grid — pure geometry: cell layout, and the plasmoid's slide-in patterns.
//!
//! Everything here is a function of numbers; nothing knows about egui or time.
//! The QML delegate computed each tile's arrival vector and stagger from its
//! (col, row) in the grid — those formulas live here so both the tests and the
//! [`crate::anim::Board`] use the same ones the plasmoid documented
//! (`plasmoid/docs/docs_tile_geometry.md`).

/// The grid's static layout for a given viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSpec {
    pub cols: usize,
    /// Rows that fit the visible height (content may scroll past them).
    pub rows_visible: usize,
    /// Square cell edge, in points.
    pub cell: f32,
}

impl GridSpec {
    /// Fit a grid to `w`×`h` with cells at least `cell_min` wide, after
    /// reserving `reserved_bottom` (search field + keyboard inset + system
    /// row). Cells grow to consume the leftover width so the grid always
    /// spans it — the plasmoid's 0.7-width centering translated to
    /// full-bleed-with-gutter, which reads better on a phone.
    pub fn fit(w: f32, h: f32, cell_min: f32, reserved_bottom: f32) -> GridSpec {
        let cols = ((w / cell_min).floor() as usize).max(1);
        let cell = w / cols as f32;
        let usable_h = (h - reserved_bottom).max(cell);
        let rows_visible = ((usable_h / cell).floor() as usize).max(1);
        GridSpec { cols, rows_visible, cell }
    }

    pub fn col_row(&self, index: usize) -> (usize, usize) {
        (index % self.cols, index / self.cols)
    }

    /// A tile's home position (top-left) in grid-content coordinates.
    pub fn cell_pos(&self, index: usize) -> (f32, f32) {
        let (c, r) = self.col_row(index);
        (c as f32 * self.cell, r as f32 * self.cell)
    }

    pub fn rows_for(&self, count: usize) -> usize {
        count.div_ceil(self.cols).max(1)
    }

    /// Total content height for `count` tiles (for the scroll range).
    pub fn content_height(&self, count: usize) -> f32 {
        self.rows_for(count) as f32 * self.cell
    }
}

/// The slide-in patterns, numbered as in the plasmoid's config schema
/// (`tileSlideStyle`) so one preference value drives both surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlidePattern {
    None = 0,
    FromBelow = 1,
    FromEdges = 2,
    Cascade = 3,
    Ripple = 4,
    Scatter = 5,
    Wave = 6,
}

impl SlidePattern {
    pub fn from_index(i: usize) -> SlidePattern {
        use SlidePattern::*;
        [None, FromBelow, FromEdges, Cascade, Ripple, Scatter, Wave]
            .get(i)
            .copied()
            .unwrap_or(FromBelow)
    }

    pub fn label(self) -> &'static str {
        match self {
            SlidePattern::None => "None",
            SlidePattern::FromBelow => "From below",
            SlidePattern::FromEdges => "From edges",
            SlidePattern::Cascade => "Cascade",
            SlidePattern::Ripple => "Ripple",
            SlidePattern::Scatter => "Scatter",
            SlidePattern::Wave => "Wave",
        }
    }

    pub fn all() -> [SlidePattern; 7] {
        use SlidePattern::*;
        [None, FromBelow, FromEdges, Cascade, Ripple, Scatter, Wave]
    }
}

/// Where a tile starts relative to its home, and how, for one pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlideStart {
    pub dx: f32,
    pub dy: f32,
    /// Initial scale (1.0 = full size).
    pub scale: f32,
    /// Initial rotation, radians.
    pub rot: f32,
    /// Delay before this tile starts moving, ms.
    pub delay_ms: f32,
}

const IDENTITY: SlideStart = SlideStart { dx: 0.0, dy: 0.0, scale: 1.0, rot: 0.0, delay_ms: 0.0 };

/// Deterministic per-index jitter in [-1, 1] — Scatter's "pseudo-random",
/// reproducible across frames and across the two front-ends.
fn jitter(index: usize, salt: u32) -> f32 {
    let mut x = (index as u32).wrapping_mul(2654435761).wrapping_add(salt.wrapping_mul(40503));
    x ^= x >> 15;
    x = x.wrapping_mul(2246822519);
    x ^= x >> 13;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// Compute a tile's slide-in start for `pattern`, given its grid position.
/// `cell` scales the travel distances; `stagger_ms` is the per-step delay unit.
pub fn slide_start(
    pattern: SlidePattern,
    index: usize,
    cols: usize,
    rows: usize,
    cell: f32,
    stagger_ms: f32,
) -> SlideStart {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let col = index % cols;
    let row = index / cols;
    match pattern {
        SlidePattern::None => IDENTITY,
        SlidePattern::FromBelow => SlideStart {
            dy: cell * 0.8,
            delay_ms: index as f32 * stagger_ms,
            ..IDENTITY
        },
        SlidePattern::FromEdges => {
            // Left half arrives from the left, right half from the right;
            // outer columns first (outside-in stagger).
            let mid = (cols as f32 - 1.0) / 2.0;
            let from_left = (col as f32) < mid || ((col as f32) == mid && cols % 2 == 1);
            let dist_from_edge = if from_left { col } else { cols - 1 - col };
            SlideStart {
                dx: if from_left { -cell * (mid + 1.0 - col as f32) } else { cell * (col as f32 - mid + 1.0) },
                delay_ms: dist_from_edge as f32 * stagger_ms,
                ..IDENTITY
            }
        }
        SlidePattern::Cascade => SlideStart {
            dx: -cell * 0.4,
            dy: cell * 0.4,
            // Diagonal wavefront: delay grows along col+row.
            delay_ms: (col + row) as f32 * stagger_ms,
            ..IDENTITY
        },
        SlidePattern::Ripple => {
            let cx = (cols as f32 - 1.0) / 2.0;
            let cy = (rows as f32 - 1.0) / 2.0;
            let d = ((col as f32 - cx).powi(2) + (row as f32 - cy).powi(2)).sqrt();
            SlideStart {
                scale: 0.3,
                delay_ms: d * stagger_ms,
                ..IDENTITY
            }
        }
        SlidePattern::Scatter => SlideStart {
            dx: jitter(index, 1) * cell * 1.5,
            dy: jitter(index, 2) * cell * 1.5,
            scale: 0.5,
            rot: jitter(index, 3) * 0.5,
            delay_ms: (jitter(index, 4) * 0.5 + 0.5) * stagger_ms * cols as f32,
            ..IDENTITY
        },
        SlidePattern::Wave => SlideStart {
            dy: cell * 0.8,
            // Per-column sinusoidal offset over a linear row ramp.
            delay_ms: row as f32 * stagger_ms * 2.0
                + ((col as f32 / cols as f32) * std::f32::consts::PI).sin() * stagger_ms * 3.0,
            ..IDENTITY
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_covers_width_and_respects_reserve() {
        let g = GridSpec::fit(1080.0, 2340.0, 200.0, 400.0);
        assert_eq!(g.cols, 5);
        assert!((g.cell - 216.0).abs() < 0.001); // cells grew to span the width
        assert_eq!(g.rows_visible, ((2340.0 - 400.0) / 216.0) as usize);
    }

    #[test]
    fn fit_never_returns_zero() {
        let g = GridSpec::fit(50.0, 10.0, 200.0, 500.0);
        assert_eq!(g.cols, 1);
        assert_eq!(g.rows_visible, 1);
    }

    #[test]
    fn cell_positions_walk_rows() {
        let g = GridSpec { cols: 4, rows_visible: 3, cell: 100.0 };
        assert_eq!(g.cell_pos(0), (0.0, 0.0));
        assert_eq!(g.cell_pos(3), (300.0, 0.0));
        assert_eq!(g.cell_pos(4), (0.0, 100.0));
        assert_eq!(g.content_height(9), 300.0);
    }

    #[test]
    fn none_pattern_is_identity() {
        let s = slide_start(SlidePattern::None, 7, 4, 3, 100.0, 20.0);
        assert_eq!(s, IDENTITY);
    }

    #[test]
    fn from_below_comes_up_with_linear_stagger() {
        let a = slide_start(SlidePattern::FromBelow, 0, 4, 3, 100.0, 20.0);
        let b = slide_start(SlidePattern::FromBelow, 5, 4, 3, 100.0, 20.0);
        assert!(a.dy > 0.0 && a.dx == 0.0);
        assert_eq!(a.delay_ms, 0.0);
        assert_eq!(b.delay_ms, 100.0);
    }

    #[test]
    fn edges_split_left_right_and_stagger_outside_in() {
        let left = slide_start(SlidePattern::FromEdges, 0, 4, 1, 100.0, 20.0); // col 0
        let right = slide_start(SlidePattern::FromEdges, 3, 4, 1, 100.0, 20.0); // col 3
        let inner = slide_start(SlidePattern::FromEdges, 1, 4, 1, 100.0, 20.0); // col 1
        assert!(left.dx < 0.0 && right.dx > 0.0);
        assert_eq!(left.delay_ms, 0.0);
        assert_eq!(right.delay_ms, 0.0);
        assert!(inner.delay_ms > 0.0); // inner columns arrive later
    }

    #[test]
    fn ripple_center_starts_first_and_scaled_down() {
        // 5x5, center at (2,2) = index 12.
        let center = slide_start(SlidePattern::Ripple, 12, 5, 5, 100.0, 20.0);
        let corner = slide_start(SlidePattern::Ripple, 0, 5, 5, 100.0, 20.0);
        assert_eq!(center.delay_ms, 0.0);
        assert!(corner.delay_ms > center.delay_ms);
        assert!((center.scale - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn scatter_is_deterministic() {
        let a = slide_start(SlidePattern::Scatter, 9, 4, 4, 100.0, 20.0);
        let b = slide_start(SlidePattern::Scatter, 9, 4, 4, 100.0, 20.0);
        assert_eq!(a, b);
        assert!(a.delay_ms >= 0.0);
    }
}
