//! Laying out the camera roll — the arithmetic, with no renderer.
//!
//! The last pure piece before a shell can be written. `album` decides what to
//! show and in what order; this decides where it goes, and both are testable
//! without a window.
//!
//! # Why a target size rather than a column count
//!
//! A gallery that fixes the number of columns looks right on exactly one
//! screen. Three columns is comfortable on a phone and absurd on a 27-inch
//! monitor, where each thumbnail becomes 600 pixels of a picture you were
//! trying to scan past.
//!
//! So the caller says how big a tile should be — roughly a thumb's width on a
//! phone, roughly the same physical size on a desktop — and the column count
//! falls out of the available width. That is how every gallery that survives
//! a resize does it, and it means portrait and landscape need no special case.
//!
//! # Square cells, on purpose
//!
//! Photos are 4:3, 3:2, 16:9, and vertical video is 9:16. A grid that honoured
//! each aspect would be a masonry layout, where a row's height depends on its
//! contents and scrolling backwards reflows everything above. Square cells with
//! centre-cropped thumbnails keep row heights constant, which is what makes a
//! ten-thousand-photo scroll possible at all: the position of any row is
//! arithmetic rather than a measurement.

/// How the grid is arranged for one width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub columns: usize,
    /// Edge length of one square cell.
    pub cell: f32,
    /// Gap between cells, and between the grid and the edges.
    pub gap: f32,
}

/// Smallest a cell may be before the grid stops adding columns.
///
/// Below about this a thumbnail stops being recognisable as a particular
/// photo, which is the only job it has — a wall of 40-pixel squares is a
/// texture, not a gallery.
pub const MIN_CELL: f32 = 72.0;

/// Compute the grid for `width`, aiming for cells near `target`.
///
/// The count is chosen so cells land as close to `target` as possible without
/// dropping below [`MIN_CELL`], and the remaining width is divided evenly —
/// so cells are never smaller than asked for by rounding, and the grid always
/// fills the row rather than leaving a ragged margin on the right.
pub fn grid_for(width: f32, target: f32, gap: f32) -> Grid {
    let target = target.max(MIN_CELL);
    let usable = (width - gap).max(0.0);
    // Each column costs `cell + gap`; solve for how many fit at the target.
    let per = (target + gap).max(1.0);
    let mut columns = (usable / per).floor() as usize;
    // At least one, always. A window narrower than a single cell still shows
    // one photo, cropped by the viewport — better than a division by zero or
    // an empty screen.
    if columns == 0 {
        columns = 1;
    }
    let cell = ((usable / columns as f32) - gap).max(1.0);
    Grid { columns, cell, gap }
}

/// Where cell `index` sits, as `(x, y)` from the grid's top-left.
pub fn cell_pos(g: &Grid, index: usize) -> (f32, f32) {
    let col = index % g.columns;
    let row = index / g.columns;
    (
        g.gap + col as f32 * (g.cell + g.gap),
        g.gap + row as f32 * (g.cell + g.gap),
    )
}

/// Total height for `count` cells.
pub fn grid_height(g: &Grid, count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let rows = count.div_ceil(g.columns);
    g.gap + rows as f32 * (g.cell + g.gap)
}

/// The half-open range of cells that intersect a viewport.
///
/// The reason the grid is uniform: with constant row heights, what is on
/// screen is arithmetic. A ten-thousand-photo library draws the forty tiles
/// that are visible, and scrolling does not get slower as you go down.
///
/// `scroll_y` is how far down the grid the viewport's top edge is.
pub fn visible_range(g: &Grid, count: usize, scroll_y: f32, viewport_h: f32) -> (usize, usize) {
    if count == 0 || g.columns == 0 {
        return (0, 0);
    }
    let stride = g.cell + g.gap;
    let first_row = ((scroll_y - g.gap) / stride).floor().max(0.0) as usize;
    // One row of overscan each way, so a tile is decoded before it scrolls
    // into view rather than popping in blank. Cheap: one row is `columns`
    // thumbnails, and the alternative is visible.
    let last_row = ((scroll_y + viewport_h - g.gap) / stride).ceil() as usize + 1;
    let start = (first_row.saturating_sub(1)) * g.columns;
    let end = (last_row * g.columns).min(count);
    (start.min(count), end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_gets_a_few_columns_and_a_desktop_gets_many() {
        // The whole reason the caller passes a target size instead of a column
        // count: three columns is right on a phone and absurd at 2560px, where
        // each tile becomes 600 pixels of a photo you were scrolling past.
        let phone = grid_for(400.0, 120.0, 4.0);
        let desktop = grid_for(2560.0, 120.0, 4.0);
        assert!(phone.columns >= 2 && phone.columns <= 4, "{phone:?}");
        assert!(desktop.columns > 15, "{desktop:?}");
        // And the cells stay near the target on both, which is the point.
        for g in [phone, desktop] {
            assert!(g.cell >= MIN_CELL, "{g:?}");
            assert!(g.cell < 200.0, "cells drifted far above the target: {g:?}");
        }
    }

    #[test]
    fn the_row_is_filled_exactly_with_no_ragged_margin() {
        // Rounding the cell down and leaving the remainder on the right is the
        // usual bug, and it looks like a misaligned grid rather than a rounding
        // choice.
        for width in [320.0, 400.0, 733.0, 1080.0, 2560.0] {
            let g = grid_for(width, 120.0, 4.0);
            let used = g.gap + g.columns as f32 * (g.cell + g.gap);
            assert!((used - width).abs() < 1.0,
                    "width {width} left {} unused: {g:?}", width - used);
        }
    }

    #[test]
    fn a_window_narrower_than_one_cell_still_shows_one() {
        // Zero columns is a division by zero in `cell_pos` and an empty screen
        // for the user. One cropped photo beats both.
        let g = grid_for(40.0, 120.0, 4.0);
        assert_eq!(g.columns, 1);
        assert!(g.cell > 0.0);
        assert_eq!(cell_pos(&g, 0), (4.0, 4.0));
        // And a zero-width window does not panic.
        let z = grid_for(0.0, 120.0, 4.0);
        assert_eq!(z.columns, 1);
        assert!(z.cell > 0.0);
    }

    #[test]
    fn cells_are_placed_in_reading_order() {
        let g = Grid { columns: 3, cell: 100.0, gap: 10.0 };
        assert_eq!(cell_pos(&g, 0), (10.0, 10.0));
        assert_eq!(cell_pos(&g, 2), (230.0, 10.0));
        // Index 3 wraps to the next row, not further right.
        assert_eq!(cell_pos(&g, 3), (10.0, 120.0));
    }

    #[test]
    fn the_height_counts_a_partial_last_row() {
        // Ten photos in a three-column grid is four rows. Integer division
        // gives three and cuts the last row off the bottom of the scroll.
        let g = Grid { columns: 3, cell: 100.0, gap: 10.0 };
        assert_eq!(grid_height(&g, 9), 10.0 + 3.0 * 110.0);
        assert_eq!(grid_height(&g, 10), 10.0 + 4.0 * 110.0);
        assert_eq!(grid_height(&g, 0), 0.0);
        assert_eq!(grid_height(&g, 1), 10.0 + 110.0);
    }

    #[test]
    fn only_the_visible_tiles_are_asked_for() {
        // The reason the grid is uniform. Ten thousand photos, a viewport
        // showing about four rows: the range must be dozens, not thousands, or
        // the gallery decodes the whole library to draw one screen.
        let g = Grid { columns: 4, cell: 100.0, gap: 10.0 };
        let (s, e) = visible_range(&g, 10_000, 0.0, 400.0);
        assert_eq!(s, 0);
        assert!(e < 60, "asked for {e} tiles to fill four rows");

        // Scrolled a long way down, the window moves with it.
        let (s2, e2) = visible_range(&g, 10_000, 5_000.0, 400.0);
        assert!(s2 > 150, "the range did not follow the scroll: {s2}");
        assert!(e2 - s2 < 60, "the window grew: {s2}..{e2}");
    }

    #[test]
    fn the_visible_range_overscans_so_tiles_do_not_pop_in_blank() {
        // One row each way. Without it a thumbnail starts decoding at the
        // moment it becomes visible, which on a phone is exactly when it is
        // most obvious.
        let g = Grid { columns: 4, cell: 100.0, gap: 10.0 };
        // Scrolled well down, so the row ABOVE exists and the overscan is
        // visible in the number. At y=550 the first fully-visible row is 4;
        // with one row of overscan the range starts at row 3, i.e. index 12.
        // My first version tested from the top, where `saturating_sub` clamps
        // to 0 either way — it passed with the overscan removed.
        let (s, _) = visible_range(&g, 1000, 550.0, 400.0);
        assert_eq!(s, 12, "no overscan above: a row scrolling in would be blank");
        // At the very top there is nothing above to fetch, and it must not
        // underflow.
        assert_eq!(visible_range(&g, 1000, 0.0, 400.0).0, 0);
    }

    #[test]
    fn an_empty_library_produces_an_empty_range() {
        let g = Grid { columns: 3, cell: 100.0, gap: 10.0 };
        assert_eq!(visible_range(&g, 0, 0.0, 400.0), (0, 0));
        // And a range never runs past the end of the library.
        let (s, e) = visible_range(&g, 5, 0.0, 4000.0);
        assert!(e <= 5, "range {s}..{e} exceeds 5 photos");
    }
}
