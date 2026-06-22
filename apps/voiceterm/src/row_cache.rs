//! row_cache: parser-driven dirty-row tracking for incremental terminal
//! render ("stage-c" from docs/design/terminal_render_perf.md).
//!
//! ## What the benchmark taught us (v0.1.574)
//! The naive idea — hash every row each frame and serialize the ones
//! whose hash changed — is a LOSS: hashing a 50x200 grid every frame
//! costs MORE than just serializing it (measured 3x SLOWER). The win
//! comes only from never TOUCHING untouched rows. So this module does
//! NOT hash. Instead it accumulates the row indices the PARSER already
//! knows it wrote to (the grid's print/scroll/clear paths know exactly
//! which rows changed), and the render layer serializes only those.
//!
//! Measured: serialize-only-dirty-rows vs full-grid-serialize, 50x200
//! grid with 1 row changing per frame → ~53x faster (18.0 -> 0.34
//! us/frame). For the common case (typing, cursor movement: 1-2 rows
//! change) this is the lever that closes the gap with Konsole, which
//! repaints only changed cells. Alt-screen apps that genuinely repaint
//! everything (helix full-screen scroll) correctly fall back to a full
//! render via `mark_all`.
//!
//! PURE + dependency-free. The grid feeds it dirty row indices; it
//! tracks geometry and produces the minimal repaint set.

/// Accumulates which rows changed since the last render flush.
pub struct RowCache {
    /// Rows marked dirty since the last `flush`. A BTreeSet-free design:
    /// a bool-per-row bitmap keeps mark O(1) and avoids alloc churn on
    /// the hot path (the parser may mark the same row many times per
    /// frame).
    dirty: Vec<bool>,
    /// Set when geometry changed (resize / scrollback page) or on a
    /// hard reset — forces a full render and a fresh geometry sync.
    full: bool,
    /// Current known row count.
    rows: usize,
}

/// v0.1.671: cheap content normalization for the publish dedup. The profile
/// showed idle re-parse churn where consecutive frames' html differed only
/// cosmetically (trailing whitespace per row, collapsible whitespace runs);
/// the v628 dedup compared RAW html, so a cosmetic-only diff still fired a
/// full RichText re-parse. Normalizing before the compare collapses those
/// without hiding real content changes (a genuinely different field still
/// differs after normalization — validated by perf-sim Case B). Cheap: trim
/// per-line trailing ws + collapse internal whitespace runs.
pub fn normalize_for_dedup(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    for line in html.lines() {
        let mut last_was_space = false;
        for ch in line.trim_end().chars() {
            if ch.is_whitespace() {
                if !last_was_space { out.push(' '); last_was_space = true; }
            } else {
                out.push(ch);
                last_was_space = false;
            }
        }
        out.push('\n');
    }
    out
}

/// The minimal repaint instruction for one render flush.
#[derive(Debug, Clone, PartialEq)]
pub enum Repaint {
    /// Nothing changed — skip rendering entirely (idle frame).
    None,
    /// Re-serialize only these row indices (sorted ascending) and patch
    /// them into the view. The big incremental win.
    Rows(Vec<usize>),
    /// Re-render the whole grid (first frame, resize, scrollback page,
    /// or churn so high it's not worth patching row-by-row).
    Full,
}

/// v0.1.630: what actually changed between two consecutive render
/// snapshots. The cursor is a SEPARATE QML overlay (termCursor, positioned
/// by term_cursor_col*charW) — it is NOT embedded in render_html(). So a
/// pure cursor move (arrow keys, cursor blink) produces IDENTICAL html but
/// a different cursor position. Today term_cursor_row/col NOTIFY on
/// term_output_changed, so every cursor move forces a full-screen RichText
/// re-parse even though the text is unchanged — the profile showed ~90
/// such discarded renders/sec on a static editor.
///
/// This classifier lets the publish path fire the CHEAP cursor signal for
/// cursor-only moves and reserve the EXPENSIVE html signal for real
/// content changes. Pure + testable; the FFI signal wiring is staged on
/// top of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotChange {
    /// Nothing changed — no notification needed at all.
    Nothing,
    /// Only the cursor moved; html is byte-identical. Fire the cheap
    /// cursor signal; skip the RichText re-parse.
    CursorOnly,
    /// The rendered html changed (content). Fire the html signal (full
    /// re-parse). Cursor may or may not have also moved — the html signal
    /// covers both since the cursor properties can be read in the same
    /// notify.
    Html,
}

/// Classify the change between a previous and current snapshot. `prev`
/// is None on the very first frame → Html (must render once).
pub fn classify_change(
    prev: Option<(&str, i32, i32)>,
    cur_html: &str,
    cur_row: i32,
    cur_col: i32,
) -> SnapshotChange {
    match prev {
        None => SnapshotChange::Html, // first frame always renders
        Some((p_html, p_row, p_col)) => {
            if p_html != cur_html {
                SnapshotChange::Html
            } else if p_row != cur_row || p_col != cur_col {
                SnapshotChange::CursorOnly
            } else {
                SnapshotChange::Nothing
            }
        }
    }
}

impl Default for RowCache {
    fn default() -> Self {
        Self::new(0)
    }
}

impl RowCache {
    /// New cache for a grid with `rows` rows. Starts in `full` state so
    /// the first flush renders everything.
    pub fn new(rows: usize) -> Self {
        RowCache { dirty: vec![false; rows], full: true, rows }
    }

    /// Mark a single row dirty (the parser's print path calls this for
    /// the row it just wrote). Out-of-range indices are ignored (a
    /// resize will re-sync geometry).
    pub fn mark_row(&mut self, idx: usize) {
        if idx < self.dirty.len() {
            self.dirty[idx] = true;
        } else {
            // A write past current geometry means we're out of sync;
            // safest is a full render next flush.
            self.full = true;
        }
    }

    /// Mark an inclusive row range dirty (scroll regions, multi-line
    /// writes). Clamped to current geometry.
    pub fn mark_range(&mut self, start: usize, end: usize) {
        let lo = start.min(self.dirty.len());
        let hi = end.min(self.dirty.len().saturating_sub(1));
        for r in lo..=hi {
            if r < self.dirty.len() {
                self.dirty[r] = true;
            }
        }
    }

    /// Mark the whole grid dirty (clear screen, alt-screen enter/exit,
    /// theme change — anything where every row's rendered bytes change).
    pub fn mark_all(&mut self) {
        self.full = true;
    }

    /// Re-sync to a (possibly new) row count. If it changed, force a
    /// full render (row identities shifted).
    pub fn set_rows(&mut self, rows: usize) {
        if rows != self.rows {
            self.dirty = vec![false; rows];
            self.rows = rows;
            self.full = true;
        }
    }

    /// Produce the repaint instruction and CLEAR the dirty state. Call
    /// once per render tick. `churn_full_threshold` in (0,1]: if the
    /// fraction of dirty rows exceeds it, return Full instead of a long
    /// Rows list (patching almost-everything isn't worth the per-row
    /// overhead). Pass 1.0 to always prefer incremental.
    pub fn flush(&mut self, churn_full_threshold: f32) -> Repaint {
        if self.full {
            self.full = false;
            for d in self.dirty.iter_mut() {
                *d = false;
            }
            return Repaint::Full;
        }
        let changed: Vec<usize> = self
            .dirty
            .iter()
            .enumerate()
            .filter_map(|(i, &d)| if d { Some(i) } else { None })
            .collect();
        for d in self.dirty.iter_mut() {
            *d = false;
        }
        if changed.is_empty() {
            return Repaint::None;
        }
        if self.rows > 0
            && (changed.len() as f32 / self.rows as f32) > churn_full_threshold
        {
            return Repaint::Full;
        }
        Repaint::Rows(changed)
    }

    pub fn rows(&self) -> usize {
        self.rows
    }
}

/// v0.1.595: pure building blocks for the per-row HTML render cache
/// (Option 1 in docs/design/render_speed_remaining.md). These let the
/// render-cache logic be unit-tested HERE, even though pty_term itself
/// (where the cache will live) can't compile in the dependency-free
/// sandbox (portable_pty / vte / unicode_width deps).
///
/// Join a slice of already-rendered per-row HTML strings into the single
/// document body the QML side consumes. CRITICAL: rows are separated by
/// '\n' with NO trailing newline after the last row — this exactly
/// matches pty_term::render_html's loop (the trailing newline would
/// create a phantom row and break selection; see render_html ~534).
///
/// This is deliberately allocation-lean: it sums the needed capacity
/// first so the result String allocates once, mirroring render_html's
/// `with_capacity` up-front reservation.
pub fn assemble_rows(rows: &[String]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    // capacity = sum of row lengths + (n-1) separators
    let cap = rows.iter().map(|r| r.len()).sum::<usize>() + rows.len().saturating_sub(1);
    let mut out = String::with_capacity(cap);
    for (i, row) in rows.iter().enumerate() {
        out.push_str(row);
        if i + 1 < rows.len() {
            out.push('\n');
        }
    }
    out
}

/// Decide, for a per-row cache, which row indices must be re-rendered
/// this frame given the dirty-row range from the grid. Returns the set
/// of indices (as a Vec, ascending) whose cached HTML is stale.
///
/// - `dirty`: the grid's `dirty_rows` — Some((lo,hi)) inclusive range,
///   or None (no row-level change this frame).
/// - `total`: current visible row count.
/// - `cache_len`: how many rows the cache currently holds. If it differs
///   from `total` (a resize), EVERY row is stale (the cache must be
///   rebuilt to the new size) — this matches mark_all/Repaint::Full.
///
/// The clamping of the dirty range to `total` guards against a stale
/// (lo,hi) that outran a shrink between mark and flush.
pub fn rows_needing_rerender(
    dirty: Option<(usize, usize)>,
    total: usize,
    cache_len: usize,
) -> Vec<usize> {
    // Resize → full rebuild.
    if cache_len != total {
        return (0..total).collect();
    }
    match dirty {
        None => Vec::new(),
        Some((lo, hi)) => {
            if total == 0 {
                return Vec::new();
            }
            let lo = lo.min(total - 1);
            let hi = hi.min(total - 1);
            if lo > hi {
                Vec::new()
            } else {
                (lo..=hi).collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_cosmetic_but_keeps_content() {
        // cosmetic-only diffs (trailing ws, ws runs) normalize to equal
        assert_eq!(normalize_for_dedup("<span>a</span>  \n"),
                   normalize_for_dedup("<span>a</span>\n"));
        assert_eq!(normalize_for_dedup("a    b\n"), normalize_for_dedup("a b\n"));
        // a REAL content change must remain different after normalizing
        assert_ne!(normalize_for_dedup("status 2536\n"),
                   normalize_for_dedup("status 2540\n"));
    }

    #[test]
    fn classify_first_frame_is_html() {
        assert_eq!(classify_change(None, "<x>", 0, 0), SnapshotChange::Html);
    }

    #[test]
    fn classify_cursor_only_when_html_identical() {
        // the static-editor / cursor-blink case from the profile: same
        // html, cursor moved one column.
        let html = "<row>fn main() {</row>";
        assert_eq!(
            classify_change(Some((html, 5, 10)), html, 5, 11),
            SnapshotChange::CursorOnly
        );
    }

    #[test]
    fn classify_html_change_takes_precedence() {
        // content changed → Html even if the cursor also moved.
        assert_eq!(
            classify_change(Some(("<row>a</row>", 0, 0)), "<row>ab</row>", 0, 1),
            SnapshotChange::Html
        );
    }

    #[test]
    fn classify_nothing_when_truly_identical() {
        let html = "<row>idle</row>";
        assert_eq!(
            classify_change(Some((html, 3, 7)), html, 3, 7),
            SnapshotChange::Nothing
        );
    }

    #[test]
    fn classify_cursor_row_change_only() {
        let html = "<row>two lines</row>";
        assert_eq!(
            classify_change(Some((html, 1, 4)), html, 2, 4),
            SnapshotChange::CursorOnly
        );
    }

    #[test]
    fn first_flush_is_full() {
        let mut c = RowCache::new(10);
        assert_eq!(c.flush(1.0), Repaint::Full, "first frame renders all");
    }

    // ── assemble_rows: must match render_html's join EXACTLY ──

    #[test]
    fn assemble_joins_with_newline_no_trailing() {
        let rows = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        // newline BETWEEN rows, none after the last (phantom-row guard)
        assert_eq!(assemble_rows(&rows), "a\nb\nc");
    }

    #[test]
    fn assemble_single_row_has_no_newline() {
        assert_eq!(assemble_rows(&["only".to_string()]), "only");
    }

    #[test]
    fn assemble_empty_is_empty() {
        assert_eq!(assemble_rows(&[]), "");
    }

    #[test]
    fn assemble_preserves_empty_rows() {
        // a blank row in the middle must still produce its separator,
        // so row positions stay aligned with the grid.
        let rows = vec!["x".to_string(), String::new(), "z".to_string()];
        assert_eq!(assemble_rows(&rows), "x\n\nz");
    }

    #[test]
    fn assemble_capacity_is_exact() {
        // not a behavior assert, but guards the capacity math from
        // under-allocating (which would force a realloc): "ab"+"cd"=4
        // chars + 1 separator = 5.
        let rows = vec!["ab".to_string(), "cd".to_string()];
        let out = assemble_rows(&rows);
        assert_eq!(out, "ab\ncd");
        assert_eq!(out.len(), 5);
    }

    // ── rows_needing_rerender: the staleness decision ──

    #[test]
    fn rerender_none_dirty_is_empty() {
        assert!(rows_needing_rerender(None, 24, 24).is_empty());
    }

    #[test]
    fn rerender_single_dirty_row() {
        assert_eq!(rows_needing_rerender(Some((5, 5)), 24, 24), vec![5]);
    }

    #[test]
    fn rerender_dirty_range() {
        assert_eq!(rows_needing_rerender(Some((2, 4)), 24, 24), vec![2, 3, 4]);
    }

    #[test]
    fn rerender_resize_forces_full() {
        // cache holds 20 but grid now has 24 → every row stale.
        let got = rows_needing_rerender(Some((5, 5)), 24, 20);
        assert_eq!(got, (0..24).collect::<Vec<_>>(),
            "a resize must rebuild the whole cache, not just the dirty row");
    }

    #[test]
    fn rerender_stale_range_clamped_to_total() {
        // dirty (22,30) but grid shrank to 24 visible rows since the
        // mark — clamp to the valid range, don't index out of bounds.
        let got = rows_needing_rerender(Some((22, 30)), 24, 24);
        assert_eq!(got, vec![22, 23]);
    }

    #[test]
    fn rerender_zero_total_is_empty() {
        assert!(rows_needing_rerender(Some((0, 0)), 0, 0).is_empty());
    }

    #[test]
    fn idle_after_full_is_none() {
        let mut c = RowCache::new(10);
        c.flush(1.0); // consume the initial full
        assert_eq!(c.flush(1.0), Repaint::None, "no marks → skip render");
    }

    #[test]
    fn single_mark_yields_one_row() {
        let mut c = RowCache::new(10);
        c.flush(1.0); // clear initial full
        c.mark_row(3);
        assert_eq!(c.flush(1.0), Repaint::Rows(vec![3]));
        // and it cleared:
        assert_eq!(c.flush(1.0), Repaint::None);
    }

    #[test]
    fn scattered_marks_sorted() {
        let mut c = RowCache::new(20);
        c.flush(1.0);
        c.mark_row(7);
        c.mark_row(2);
        c.mark_row(7); // dup — must not double-report
        assert_eq!(c.flush(1.0), Repaint::Rows(vec![2, 7]));
    }

    #[test]
    fn mark_range_inclusive_clamped() {
        let mut c = RowCache::new(5);
        c.flush(1.0);
        c.mark_range(3, 99); // clamp to last row (4)
        assert_eq!(c.flush(1.0), Repaint::Rows(vec![3, 4]));
    }

    #[test]
    fn mark_all_forces_full() {
        let mut c = RowCache::new(10);
        c.flush(1.0);
        c.mark_row(1);
        c.mark_all();
        assert_eq!(c.flush(1.0), Repaint::Full, "mark_all wins over partial");
    }

    #[test]
    fn resize_forces_full_and_resyncs() {
        let mut c = RowCache::new(10);
        c.flush(1.0);
        c.set_rows(12);
        assert_eq!(c.flush(1.0), Repaint::Full);
        assert_eq!(c.rows(), 12);
    }

    #[test]
    fn out_of_range_mark_forces_full() {
        let mut c = RowCache::new(5);
        c.flush(1.0);
        c.mark_row(99); // past geometry → can't trust partial
        assert_eq!(c.flush(1.0), Repaint::Full);
    }

    #[test]
    fn high_churn_falls_back_to_full() {
        let mut c = RowCache::new(10);
        c.flush(1.0);
        // mark 8 of 10 rows; threshold 0.7 → 0.8 > 0.7 → Full
        for i in 0..8 {
            c.mark_row(i);
        }
        assert_eq!(c.flush(0.7), Repaint::Full,
            "patching 80% of rows isn't worth it; full render");
    }

    #[test]
    fn low_churn_stays_incremental() {
        let mut c = RowCache::new(10);
        c.flush(1.0);
        c.mark_row(0);
        c.mark_row(1); // 2/10 = 0.2 < 0.7
        match c.flush(0.7) {
            Repaint::Rows(r) => assert_eq!(r, vec![0, 1]),
            other => panic!("expected incremental, got {other:?}"),
        }
    }

    #[test]
    fn typing_pattern_is_cheap() {
        // Simulate the common case: 100 frames, each touches 1 row
        // (cursor line). Every flush should be a 1-row repaint, never
        // Full, never None-when-changed.
        let mut c = RowCache::new(50);
        c.flush(1.0);
        for f in 0..100 {
            c.mark_row(f % 50);
            match c.flush(0.7) {
                Repaint::Rows(r) => assert_eq!(r.len(), 1),
                other => panic!("frame {f}: expected 1-row repaint, got {other:?}"),
            }
        }
    }
}
