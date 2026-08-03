//! row_render — using `RowCache` in an immediate-mode UI.
//!
//! `RowCache` is included, not rewritten (`#[path]`, as with the table engine):
//! it is pure and dependency-free, the grid feeds it dirty row indices, and it
//! produces the minimal repaint set. Its measured win is large — 50x200 grid
//! with one row changing per frame went from 18.0 to 0.34 us/frame, ~53x.
//!
//! ## But what it SAVES is different here, and getting that wrong would waste it
//!
//! In the Qt path the expensive thing was serialising rows to QML text for the
//! view to re-parse. There is no serialisation in egui — the renderer paints
//! from the grid directly every frame, because that is what immediate mode
//! means. Skipping the paint is not available.
//!
//! What IS expensive, and what this cache therefore protects, is TEXT LAYOUT:
//! shaping a row into a galley — measuring glyphs, resolving fonts, wrapping.
//! egui will do that per row per frame unless the result is kept, and it costs
//! far more than the painting does. So `RowCache`'s dirty set keys a per-row
//! layout cache: an unchanged row reuses its galley, a dirty row re-shapes.
//!
//! Same lever, different fulcrum. Wiring it to skip painting instead would save
//! nothing and break the display.
//!
//! ## Full means drop everything
//!
//! A resize changes every row's wrap width, so a cached galley from before it is
//! wrong in a way that is invisible until you notice text overflowing its
//! column. `Repaint::Full` clears the cache rather than marking rows dirty —
//! there is no such thing as a galley that survives a width change.

use crate::row_cache::{RowCache, Repaint};

/// Above this fraction of rows changing, patching row-by-row costs more than
/// redoing the lot. Passed to `flush`.
pub const CHURN_FULL_THRESHOLD: f32 = 0.6;

/// A per-row cache of whatever laying out a row produces.
///
/// Generic over the laid-out type so the policy can be tested without an egui
/// context — the rules here are about WHEN to rebuild, and none of them need to
/// know what a galley is.
#[derive(Debug, Clone, Default)]
pub struct RowLayoutCache<T> {
    rows: Vec<Option<T>>,
    /// Wrap width the cache was built at. A change invalidates everything.
    width: f32,
    /// Counts, for the perf line in the diagnostics report.
    pub built: u64,
    pub reused: u64,
}

impl<T: Clone> RowLayoutCache<T> {
    pub fn new() -> Self {
        RowLayoutCache { rows: Vec::new(), width: 0.0, built: 0, reused: 0 }
    }

    pub fn len(&self) -> usize { self.rows.len() }
    pub fn is_empty(&self) -> bool { self.rows.is_empty() }
    /// How many rows currently hold a usable layout.
    pub fn cached(&self) -> usize { self.rows.iter().filter(|r| r.is_some()).count() }

    /// Drop everything. Used on a full repaint and on a width change.
    pub fn clear(&mut self) {
        for r in self.rows.iter_mut() { *r = None; }
    }

    fn resize(&mut self, n: usize) {
        if self.rows.len() != n { self.rows.resize(n, None); }
    }

    /// Apply a repaint decision, returning the rows that must be re-laid-out.
    ///
    /// A width change forces Full regardless of what the cache said: every
    /// cached galley was shaped to the old width, and one that survives a
    /// resize is wrong in a way that only shows up as text overflowing.
    pub fn plan(&mut self, repaint: Repaint, rows: usize, width: f32) -> Vec<usize> {
        let resized = (width - self.width).abs() > f32::EPSILON;
        self.resize(rows);
        if resized {
            self.width = width;
            self.clear();
            return (0..rows).collect();
        }
        match repaint {
            Repaint::None => Vec::new(),
            Repaint::Full => {
                self.clear();
                (0..rows).collect()
            }
            Repaint::Rows(idx) => {
                for i in &idx {
                    // A dirty row's cached layout is stale; drop it so a caller
                    // that forgets to rebuild shows nothing rather than showing
                    // the previous frame's text.
                    if let Some(slot) = self.rows.get_mut(*i) { *slot = None; }
                }
                idx.into_iter().filter(|i| *i < rows).collect()
            }
        }
    }

    /// Fetch a row's layout, building it if the cache does not have one.
    pub fn get_or_build(&mut self, i: usize, build: impl FnOnce() -> T) -> Option<T> {
        let slot = self.rows.get_mut(i)?;
        if let Some(v) = slot {
            self.reused += 1;
            return Some(v.clone());
        }
        let v = build();
        *slot = Some(v.clone());
        self.built += 1;
        Some(v)
    }

    /// Fraction of lookups served from the cache — the number that says whether
    /// any of this is working.
    pub fn hit_rate(&self) -> f32 {
        let total = self.built + self.reused;
        if total == 0 { return 0.0; }
        self.reused as f32 / total as f32
    }
}

/// Drive a cache from a grid's dirty tracking for one frame.
///
/// One call so the ORDER cannot be got wrong elsewhere: flush the row cache
/// first (which clears its dirty bits), then plan against the layout cache.
/// Planning before flushing would use the previous frame's dirty set and lag a
/// frame behind, which shows up as a character appearing one keystroke late.
pub fn frame_plan<T: Clone>(rc: &mut RowCache, cache: &mut RowLayoutCache<T>,
                            width: f32) -> Vec<usize> {
    let rows = rc.rows();
    let repaint = rc.flush(CHURN_FULL_THRESHOLD);
    cache.plan(repaint, rows, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> RowLayoutCache<String> { RowLayoutCache::new() }

    #[test]
    fn an_idle_frame_lays_out_nothing() {
        // The whole point: typing in one row must not re-shape the other 49.
        let mut rc = RowCache::new(50);
        let mut c = cache();
        frame_plan(&mut rc, &mut c, 800.0);      // first frame is Full
        let plan = frame_plan(&mut rc, &mut c, 800.0);
        assert!(plan.is_empty());
    }

    #[test]
    fn one_dirty_row_lays_out_one_row() {
        let mut rc = RowCache::new(50);
        let mut c = cache();
        frame_plan(&mut rc, &mut c, 800.0);
        rc.mark_row(7);
        assert_eq!(frame_plan(&mut rc, &mut c, 800.0), vec![7]);
    }

    #[test]
    fn the_first_frame_lays_out_everything() {
        let mut rc = RowCache::new(4);
        let mut c = cache();
        assert_eq!(frame_plan(&mut rc, &mut c, 800.0), vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_dirty_rows_stale_layout_is_dropped_not_left_behind() {
        // A caller that forgets to rebuild should show nothing rather than the
        // previous frame's text, which would be a silently wrong display.
        let mut c = cache();
        c.plan(Repaint::Full, 3, 800.0);
        for i in 0..3 { c.get_or_build(i, || format!("row {i}")); }
        assert_eq!(c.cached(), 3);
        c.plan(Repaint::Rows(vec![1]), 3, 800.0);
        assert_eq!(c.cached(), 2, "row 1's layout was dropped");
    }

    #[test]
    fn an_unchanged_row_reuses_its_layout() {
        let mut c = cache();
        c.plan(Repaint::Full, 2, 800.0);
        c.get_or_build(0, || "built".to_string());
        let again = c.get_or_build(0, || panic!("should not rebuild"));
        assert_eq!(again.as_deref(), Some("built"));
        assert_eq!(c.built, 1);
        assert_eq!(c.reused, 1);
    }

    #[test]
    fn a_width_change_invalidates_everything_even_on_an_idle_frame() {
        // Every cached galley was shaped to the old width. One that survives a
        // resize is wrong in a way that only shows as text overflowing its
        // column — invisible until someone notices.
        let mut c = cache();
        c.plan(Repaint::Full, 3, 800.0);
        for i in 0..3 { c.get_or_build(i, || format!("row {i}")); }
        let plan = c.plan(Repaint::None, 3, 640.0);
        assert_eq!(plan, vec![0, 1, 2], "an idle frame still re-lays out after a resize");
        assert_eq!(c.cached(), 0);
    }

    #[test]
    fn the_same_width_does_not_invalidate() {
        let mut c = cache();
        c.plan(Repaint::Full, 2, 800.0);
        c.get_or_build(0, || "x".to_string());
        assert!(c.plan(Repaint::None, 2, 800.0).is_empty());
        assert_eq!(c.cached(), 1);
    }

    #[test]
    fn a_full_repaint_drops_every_cached_layout() {
        // Alt-screen apps that genuinely repaint everything fall back to this,
        // and a stale galley surviving it would show the old screen's text.
        let mut c = cache();
        c.plan(Repaint::Full, 3, 800.0);
        for i in 0..3 { c.get_or_build(i, || "x".to_string()); }
        let plan = c.plan(Repaint::Full, 3, 800.0);
        assert_eq!(plan.len(), 3);
        assert_eq!(c.cached(), 0);
    }

    #[test]
    fn heavy_churn_falls_back_to_a_full_pass() {
        // Above the threshold, patching row by row costs more than redoing the
        // lot — which is exactly the alt-screen scroll case.
        let mut rc = RowCache::new(10);
        let mut c = cache();
        frame_plan(&mut rc, &mut c, 800.0);
        for i in 0..9 { rc.mark_row(i); }
        let plan = frame_plan(&mut rc, &mut c, 800.0);
        assert_eq!(plan.len(), 10, "everything, rather than nine patches");
    }

    #[test]
    fn a_row_index_past_the_end_is_dropped_from_the_plan() {
        // The grid can shrink between the mark and the flush; laying out a row
        // that no longer exists would panic in the renderer.
        let mut c = cache();
        c.plan(Repaint::Full, 5, 800.0);
        let plan = c.plan(Repaint::Rows(vec![1, 99]), 3, 800.0);
        assert_eq!(plan, vec![1]);
    }

    #[test]
    fn the_cache_follows_the_grid_when_it_grows_and_shrinks() {
        let mut c = cache();
        c.plan(Repaint::Full, 3, 800.0);
        assert_eq!(c.len(), 3);
        c.plan(Repaint::Full, 8, 800.0);
        assert_eq!(c.len(), 8);
        c.plan(Repaint::Full, 2, 800.0);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn fetching_a_row_that_does_not_exist_reports_nothing() {
        let mut c = cache();
        c.plan(Repaint::Full, 2, 800.0);
        assert_eq!(c.get_or_build(9, || "x".to_string()), None);
    }

    #[test]
    fn the_hit_rate_shows_whether_any_of_this_is_working() {
        let mut c = cache();
        c.plan(Repaint::Full, 50, 800.0);
        // First frame: everything built.
        for i in 0..50 { c.get_or_build(i, || "x".to_string()); }
        assert_eq!(c.hit_rate(), 0.0);
        // Then a hundred frames with one row changing each time.
        for f in 0..100 {
            c.plan(Repaint::Rows(vec![f % 50]), 50, 800.0);
            for i in 0..50 { c.get_or_build(i, || "x".to_string()); }
        }
        assert!(c.hit_rate() > 0.95, "got {}", c.hit_rate());
    }

    #[test]
    fn an_empty_cache_reports_a_zero_hit_rate_rather_than_dividing_by_zero() {
        assert_eq!(cache().hit_rate(), 0.0);
    }

    #[test]
    fn flushing_happens_before_planning_or_the_display_lags_a_keystroke() {
        // frame_plan exists so this order cannot be got wrong elsewhere.
        // Planning against the PREVIOUS frame's dirty set shows a character one
        // keystroke late, which is the most maddening possible terminal bug.
        let mut rc = RowCache::new(10);
        let mut c = cache();
        frame_plan(&mut rc, &mut c, 800.0);
        rc.mark_row(3);
        assert_eq!(frame_plan(&mut rc, &mut c, 800.0), vec![3], "this frame, not next");
        assert!(frame_plan(&mut rc, &mut c, 800.0).is_empty(), "and not again after");
    }
}
