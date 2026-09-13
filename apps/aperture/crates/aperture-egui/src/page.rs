//! page — the ONE place a content-space y becomes a screen-space y.
//!
//! ## The bug this exists to kill
//!
//! `view.rs` had eight independent copies of `rect.top() - scroll + line as
//! f32 * m.line_h`. That looks like one formula written eight times, and the
//! duplication was not the problem: the problem is what epaint does to each
//! result *afterwards*.
//!
//! epaint's tessellator rounds to the physical pixel grid, per shape, by
//! default — `round_text_to_pixels` for every galley origin and
//! `round_rects_to_pixels` for every rect (`TessellationOptions`, both `true`).
//! So each row's y is rounded on its own. With a fractional `line_h` (it is
//! `f.row_height(font) + 2.0`, typically something like 16.8), consecutive
//! rows sit at different fractional offsets, so as `scroll` sweeps
//! continuously past a boundary **row N crosses it on a different frame than
//! row N+1**. The page stops translating as a sheet and starts shimmering
//! row-against-row. That is the reported "disjointed pixel snapping", and no
//! amount of care at any one of the eight call sites can see it, because the
//! defect is in the *relationship* between them.
//!
//! ## The fix: quantize the camera, not the elements
//!
//! Round the page origin ONCE, to the physical pixel grid, and derive every
//! row from it:
//!
//! ```text
//! origin_y = round_to_pixels(view_top - scroll)
//! row_y(n) = round_to_pixels(origin_y + n * line_h)
//! ```
//!
//! Rows are still rounded individually — they have to be, or the text is
//! blurry — but now `origin_y` is *already on the grid*, so when it steps by
//! one physical pixel every `row_y(n)` steps by exactly one physical pixel
//! too. The page moves rigidly. The fractional part of `line_h` is still
//! there; it is spread across the rows as a FIXED pattern (a pitch of, say,
//! 17/17/16/17 px rather than a uniform 16.8), and that pattern does not
//! change as you scroll. That is the "dither": the residue is distributed
//! once and held, instead of being re-decided every frame by every element
//! separately.
//!
//! Note what is deliberately NOT done: `line_h` is not itself rounded. A
//! rounded pitch would make the page perfectly rigid and then drift from the
//! layout — 0.2 px per row is 20 px by row 100 — so hit-testing, `max_scroll`
//! and the caret would all disagree with what is on screen, further down the
//! file the worse it gets. The exact `line_h` stays the authority; only the
//! *paint* is quantized.
//!
//! ## Crisp vs smooth
//!
//! Two rules, and the difference is the point:
//!
//! * [`PageGrid::row_y`] / [`PageGrid::y`] — **crisp**. For anything at rest
//!   on the page: text rows, gutter marks, selection slabs, find slabs. These
//!   want to sit exactly on pixel boundaries.
//! * [`PageGrid::y_smooth`] — **not quantized**. For something genuinely
//!   mid-animation, where a 1-px quantum is larger than a frame's motion and
//!   snapping reads as stutter. The caret bar while it glides, and rows while
//!   the insert/delete line-slide is running. The caller must ALSO pass
//!   `with_round_to_pixels(false)` on those shapes, or epaint re-rounds them
//!   and the smooth position is thrown away — that is the whole reason this
//!   is a separate method rather than a flag.
//!
//! A thing drawn with `y_smooth` while moving and `y` once settled changes
//! position by under half a pixel at the handover, and gains crispness. That
//! asymmetry is intentional and is the answer to "not snapped quite the same
//! way at the end of the animation": it should not be, and now it is not by
//! decision rather than by accident.
//!
//! ## Hit-testing
//!
//! [`PageGrid::line_at`] is the exact inverse of [`PageGrid::row_y`] — the
//! `n` whose painted band contains a screen y. Without it the click path
//! keeps its own un-quantized formula and disagrees with the paint on the
//! boundary pixel of every row: a click in that band lands on the row above
//! the one it visually hit. Sub-pixel, but it is precisely the kind of
//! ninth call site that makes "I unified the other eight" untrue.
//!
//! Pure f32 — no egui types — so all of this is pinned by unit tests rather
//! than by looking at a build.

/// The editor page's coordinate map for one frame.
///
/// Construct once per frame, immediately after the camera has stepped, and
/// pass it to everything that paints. Cheap (four floats, `Copy`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGrid {
    /// Screen y of content y = 0, already on the physical pixel grid.
    origin_y: f32,
    /// Exact (unrounded) row pitch — the layout authority.
    line_h: f32,
    /// Physical pixels per logical point.
    ppp: f32,
}

/// `(v * ppp).round() / ppp` — emath's `GuiRounding::round_to_pixels`,
/// restated here so this module stays egui-free and testable in isolation.
/// A non-finite or non-positive `ppp` means "no rounding" rather than a NaN
/// screen position; a zero would divide by zero and paint the whole page at
/// infinity, which is a worse failure than a slightly blurry row.
#[inline]
fn quantize(v: f32, ppp: f32) -> f32 {
    if ppp.is_finite() && ppp > 0.0 {
        (v * ppp).round() / ppp
    } else {
        v
    }
}

impl PageGrid {
    /// `view_top` is the viewport's screen-space top edge, `scroll` the
    /// camera's current (animated, unrounded) scroll position in content px.
    pub fn new(view_top: f32, scroll: f32, line_h: f32, ppp: f32) -> Self {
        PageGrid { origin_y: quantize(view_top - scroll, ppp), line_h, ppp }
    }

    /// Screen y for a content-space y, on the pixel grid. Crisp.
    #[inline]
    pub fn y(&self, content_y: f32) -> f32 {
        quantize(self.origin_y + content_y, self.ppp)
    }

    /// Screen y for a content-space y, NOT quantized. For shapes that are
    /// mid-animation; pair with `with_round_to_pixels(false)`.
    #[inline]
    pub fn y_smooth(&self, content_y: f32) -> f32 {
        self.origin_y + content_y
    }

    /// Screen y of row `line`'s top edge. Crisp.
    #[inline]
    pub fn row_y(&self, line: usize) -> f32 {
        self.y(line as f32 * self.line_h)
    }

    /// Row `line`'s painted height — the distance to the next row's top.
    /// Not constant: with a fractional `line_h` it alternates by a pixel,
    /// which is exactly the dither described above. Use this rather than
    /// `line_h` for any slab that must tile with its neighbours without a
    /// seam or an overlap.
    #[inline]
    pub fn row_h(&self, line: usize) -> f32 {
        self.row_y(line + 1) - self.row_y(line)
    }

    /// The row whose painted band contains `screen_y`. Exact inverse of
    /// [`Self::row_y`]: `line_at(y)` is the `n` with
    /// `row_y(n) <= y < row_y(n+1)`. Clamps at 0; unbounded above (the
    /// caller clamps to the buffer, as it always did).
    pub fn line_at(&self, screen_y: f32) -> usize {
        // Degenerate pitch: no meaningful band structure, and the correction
        // loop below would not terminate in a useful number of steps.
        if !(self.line_h > 0.0) || !self.line_h.is_finite() {
            return 0;
        }
        let approx = ((screen_y - self.origin_y) / self.line_h).floor();
        if !approx.is_finite() {
            return 0;
        }
        let mut n = approx.max(0.0) as i64;
        // Quantization moves a row edge by at most half a physical pixel, so
        // the approximation is off by at most one row. The bound is a belt
        // for a brace: it costs nothing and turns any future violation of
        // that reasoning into a wrong pixel instead of a hang.
        for _ in 0..2 {
            if n > 0 && screen_y < self.row_y_i(n) {
                n -= 1;
            } else if screen_y >= self.row_y_i(n + 1) {
                n += 1;
            } else {
                break;
            }
        }
        n.max(0) as usize
    }

    #[inline]
    fn row_y_i(&self, line: i64) -> f32 {
        self.y(line as f32 * self.line_h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point. Sweep the scroll continuously and assert that every
    /// row's painted offset from the FIRST row is unchanged — i.e. the page
    /// translates rigidly instead of shimmering row-against-row.
    ///
    /// This is the test that fails on the old code: with an unquantized
    /// origin, `row_y(n) - row_y(0)` changes by a pixel for some `n` at some
    /// point in the sweep, on a different frame for each `n`.
    #[test]
    fn the_page_translates_rigidly_as_it_scrolls() {
        let line_h = 16.8; // deliberately fractional — this is the real shape
        let ppp = 1.0;
        let reference: Vec<f32> = {
            let g = PageGrid::new(0.0, 0.0, line_h, ppp);
            (0..40).map(|n| g.row_y(n) - g.row_y(0)).collect()
        };
        // 400 sub-pixel steps across 25 px of scroll.
        let mut checked = 0;
        for i in 0..400 {
            let scroll = i as f32 * 0.0625;
            let g = PageGrid::new(0.0, scroll, line_h, ppp);
            for n in 0..40 {
                let rel = g.row_y(n) - g.row_y(0);
                assert!(
                    (rel - reference[n]).abs() < 1e-3,
                    "row {n} drifted at scroll {scroll}: {rel} vs {}",
                    reference[n]
                );
                checked += 1;
            }
        }
        // §5b: assert the scan found something. A loop that ran zero times
        // passes every assertion inside it.
        assert_eq!(checked, 400 * 40);
    }

    /// And the page really does MOVE — a rigidity test is trivially satisfied
    /// by a grid that ignores scroll entirely, so pin that it doesn't.
    #[test]
    fn scrolling_actually_moves_the_page() {
        let g0 = PageGrid::new(0.0, 0.0, 16.8, 1.0);
        let g1 = PageGrid::new(0.0, 40.0, 16.8, 1.0);
        assert_eq!(g1.row_y(0) - g0.row_y(0), -40.0);
        // …and by a whole number of physical pixels, always.
        for i in 0..97 {
            let s = i as f32 * 0.37;
            let g = PageGrid::new(0.0, s, 16.8, 2.0);
            let px = (g.row_y(0) - g0.row_y(0)) * 2.0;
            assert!((px - px.round()).abs() < 1e-3, "origin off-grid at scroll {s}: {px}");
        }
    }

    /// Rows land on the physical pixel grid — the reason for quantizing at
    /// all. Checked at 2x too, where the grid is half a logical point.
    #[test]
    fn rows_sit_on_the_pixel_grid() {
        for &ppp in &[1.0_f32, 1.5, 2.0, 3.0] {
            let g = PageGrid::new(7.3, 13.7, 16.8, ppp);
            for n in 0..30 {
                let px = g.row_y(n) * ppp;
                assert!((px - px.round()).abs() < 1e-3, "ppp {ppp} row {n} off-grid: {px}");
            }
        }
    }

    /// The pitch dithers around `line_h` and never drifts from it. Both
    /// directions are failures, so this asserts a RANGE (CLAUDE.md §5b):
    /// a floor alone would pass for a pitch that had stopped tracking
    /// `line_h` at all.
    #[test]
    fn row_pitch_dithers_but_does_not_accumulate_error() {
        let line_h = 16.8;
        let g = PageGrid::new(0.0, 0.0, line_h, 1.0);
        let mut saw_17 = 0;
        let mut saw_16 = 0;
        for n in 0..200 {
            let h = g.row_h(n);
            assert!((16.0..=17.0).contains(&h), "row {n} pitch {h} outside the dither band");
            if h > 16.5 { saw_17 += 1 } else { saw_16 += 1 }
        }
        // Both values must actually occur, or it is not dithering — it is
        // rounding the pitch, which is the thing this module refuses to do.
        assert!(saw_17 > 0 && saw_16 > 0, "pitch never dithered: {saw_17} long, {saw_16} short");
        // No accumulated drift: row 200's top is within one pixel of exact.
        let exact = 200.0 * line_h;
        assert!((g.row_y(200) - exact).abs() <= 1.0, "drifted: {} vs {exact}", g.row_y(200));
    }

    /// `line_at` is the exact inverse of `row_y`, including on the boundary
    /// pixel where the old un-quantized click formula disagreed.
    #[test]
    fn line_at_inverts_row_y_exactly() {
        let g = PageGrid::new(11.0, 42.3, 16.8, 2.0);
        for n in 0..60 {
            let top = g.row_y(n);
            let next = g.row_y(n + 1);
            assert_eq!(g.line_at(top), n, "top edge of row {n}");
            assert_eq!(g.line_at((top + next) * 0.5), n, "middle of row {n}");
            // One physical pixel below the next row's top is still row n.
            assert_eq!(g.line_at(next - 0.5), n, "last pixel of row {n}");
        }
    }

    #[test]
    fn line_at_clamps_above_the_page_and_survives_degenerate_input() {
        let g = PageGrid::new(0.0, 0.0, 16.8, 1.0);
        assert_eq!(g.line_at(-1000.0), 0);
        assert_eq!(g.line_at(f32::NEG_INFINITY), 0);
        // A zero pitch must not hang or divide its way to NaN.
        let bad = PageGrid::new(0.0, 0.0, 0.0, 1.0);
        assert_eq!(bad.line_at(50.0), 0);
    }

    /// `y_smooth` must NOT be quantized — it is the escape hatch, and a
    /// version of it that quietly rounded would leave the caret stuttering
    /// with no visible sign that anything was wrong.
    #[test]
    fn y_smooth_keeps_the_subpixel_residue() {
        let g = PageGrid::new(0.0, 0.0, 16.8, 1.0);
        let a = g.y_smooth(10.25);
        let b = g.y_smooth(10.75);
        assert_ne!(a, b);
        assert!((b - a - 0.5).abs() < 1e-4);
        // …and the crisp one does round, so the two are genuinely different.
        assert_eq!(g.y(10.25), g.y(10.4));
    }

    #[test]
    fn a_nonsense_pixel_ratio_degrades_to_no_rounding_rather_than_nan() {
        for &ppp in &[0.0_f32, -2.0, f32::NAN, f32::INFINITY] {
            let g = PageGrid::new(0.0, 3.7, 16.8, ppp);
            assert!(g.row_y(3).is_finite(), "ppp {ppp} produced a non-finite row y");
        }
    }
}
