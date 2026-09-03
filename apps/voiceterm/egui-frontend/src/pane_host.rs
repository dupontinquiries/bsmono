//! pane_host — what follows a pane, and how it follows.
//!
//! C11-6a: the live VT session used to be five singleton fields on the app
//! (grid, pty_out, echo mark, row caches), so switching panes showed the SAME
//! session in whichever pane held focus — typed history did not follow its
//! pane. [`PaneSession`] bundles exactly what must travel, and [`swap_focus`]
//! is the focused/parked exchange, extracted from the binary's inline
//! `pane_apps` swap so the property that matters — content survives a
//! focus round-trip — is pinned by a test instead of by reading.
//!
//! The extraction also FIXES the inline version's silent hole: it swapped
//! only `if let Some(next) = parked.remove(&now)`, so focusing a pane with
//! no parked state (a fresh split, before the reconcile ran) did nothing —
//! the new pane inherited the old pane's state and the old pane lost it on
//! return. Here the old state is ALWAYS parked under its own id, and a pane
//! with nothing parked gets a fresh default. The test for that case is the
//! one the old code could not have passed.

use std::collections::HashMap;

/// The per-pane terminal session: everything the typed history IS.
pub struct PaneSession {
    /// The live VT screen — where every typed byte echoes.
    pub grid: crate::term_grid::TermGrid,
    /// Bytes going TO the shell; a host drains this to a real PTY.
    pub pty_out: Vec<u8>,
    /// High-water mark of `pty_out` already echoed to the grid.
    pub pty_echoed: usize,
    /// Which grid rows changed since last shape.
    pub row_dirty: crate::row_cache::RowCache,
    /// Shaped galleys per row, reused while a row is unchanged.
    pub row_galleys: crate::row_render::RowLayoutCache<String>,
    /// This pane's VT parser state.
    ///
    /// Per-pane because an escape sequence can be split across reads: a
    /// single app-level parser fed from two panes resumes one pane's
    /// half-finished CSI with another pane's bytes. It also has to
    /// travel with focus, which it does for free by living here.
    pub vt: crate::term_grid::Vt,
}

impl PaneSession {
    pub fn new() -> PaneSession {
        PaneSession {
            grid: crate::term_grid::TermGrid::new(80, 24),
            pty_out: Vec::new(),
            pty_echoed: 0,
            row_dirty: crate::row_cache::RowCache::new(24), // matches the 80x24 grid above
            row_galleys: crate::row_render::RowLayoutCache::new(),
            vt: crate::term_grid::Vt::default(),
        }
    }
}

impl PaneSession {
    /// Feed bytes from this pane's shell into ITS OWN grid, through its
    /// own parser.
    ///
    /// The background drain used to write into `pane_apps[id].terminal`
    /// — the uikit-term widget's grid — while the view renders
    /// `session.grid`. So an unfocused pane's output went somewhere
    /// nothing draws, and was still missing when the pane regained
    /// focus. The drain was added to fix "an unfocused pane looks
    /// frozen" and moved the output to a second invisible place instead.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut vt = std::mem::take(&mut self.vt);
        vt.feed(&mut self.grid, bytes);
        self.vt = vt;
    }
}

impl Default for PaneSession {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether the focused pane needs a shell spawned for it right now.
///
/// ## The hole this closes
///
/// The reconcile loop builds `hosted` as the parked-pane ids PLUS the
/// focused id, then spawns for everything in the workspace that is not
/// hosted. That is correct for background panes and wrong for the
/// focused one: `swap_focus` hands a newly focused pane `fresh()`, which
/// for `Option<PtyHost>` is `None`, and then `hosted` claims the pane is
/// already served so the spawn loop skips it.
///
/// So splitting a pane produced a focused terminal with NO shell —
/// permanently, because the next frame has `prev == now` and
/// `swap_focus` is a no-op. Keys went to `pty_out`, fell through to
/// local echo, and nothing ever ran. Every piece was individually
/// correct; the focused pane simply fell between two rules.
///
/// `tried` is what stops a failing spawn being retried every frame
/// forever — the same reason the background loop remembers a `None`.
pub fn focused_needs_shell(has_shell: bool, tried: bool) -> bool {
    !has_shell && !tried
}

/// Exchange the focused state with the parked map when focus moves from
/// `prev` to `now`. The old focused state is parked under `prev`
/// unconditionally; the new focused state is whatever was parked under
/// `now`, or `fresh()` when nothing was — a fresh pane starts empty rather
/// than inheriting its neighbour's history.
///
/// No-op when `prev == now`, so callers may invoke it unconditionally.
pub fn swap_focus<T>(
    focused: &mut T,
    parked: &mut HashMap<u64, T>,
    prev: u64,
    now: u64,
    fresh: impl FnOnce() -> T,
) {
    if prev == now {
        return;
    }
    let next = parked.remove(&now).unwrap_or_else(fresh);
    let old = std::mem::replace(focused, next);
    parked.insert(prev, old);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_parses_into_its_own_grid() {
        // the background drain's destination: what gets DRAWN
        let mut s = PaneSession::new();
        s.feed(b"hello");
        assert_eq!(s.grid.row_text(0).trim_end(), "hello");
    }

    #[test]
    fn an_escape_sequence_split_across_reads_survives() {
        // why the parser is per-pane: a shared Vt resumes one pane's
        // half-finished CSI with another pane's bytes
        let mut s = PaneSession::new();
        s.feed(b"a\x1b[");
        s.feed(b"1;3Hb");
        // `1;3H` moved the cursor to column 3 (1-based), so 'b' lands in
        // the third cell: "a b". The point is that the CSI was OBEYED
        // rather than printed — a parser that dropped its state between
        // feeds would render the literal text `1;3H`.
        assert_eq!(s.grid.row_text(0).trim_end(), "a b");
        assert!(!s.grid.row_text(0).contains("1;3H"), "the sequence was not printed as text");
    }

    #[test]
    fn two_sessions_do_not_share_parser_state() {
        let mut a = PaneSession::new();
        let mut b = PaneSession::new();
        a.feed(b"\x1b["); // a leaves a sequence open
        b.feed(b"1;1Hxy"); // b must not complete it
        assert_eq!(b.grid.row_text(0).trim_end(), "1;1Hxy");
    }


    #[test]
    fn a_freshly_focused_pane_with_no_shell_asks_for_one() {
        // the split case: swap_focus handed it None and the reconcile
        // loop skipped it because `hosted` already claimed the id
        assert!(focused_needs_shell(false, false));
    }

    #[test]
    fn a_pane_that_already_has_a_shell_is_left_alone() {
        assert!(!focused_needs_shell(true, false));
        assert!(!focused_needs_shell(true, true));
    }

    #[test]
    fn a_failed_spawn_is_not_retried_every_frame() {
        // sixty spawn attempts a second against a machine with no shell
        // is worse than a pane that falls back to local echo
        assert!(!focused_needs_shell(false, true));
    }


    /// The property the whole module exists for: a pane's content survives
    /// losing and regaining focus, and each pane sees only its own.
    #[test]
    fn content_follows_its_pane_across_a_focus_round_trip() {
        let mut focused = String::from("pane-1 history");
        let mut parked: HashMap<u64, String> = HashMap::new();
        parked.insert(2, "pane-2 history".into());

        swap_focus(&mut focused, &mut parked, 1, 2, || "FRESH".into());
        assert_eq!(focused, "pane-2 history");
        assert_eq!(parked.get(&1).map(String::as_str), Some("pane-1 history"));

        swap_focus(&mut focused, &mut parked, 2, 1, || "FRESH".into());
        assert_eq!(focused, "pane-1 history", "pane 1 got its own history back");
        assert_eq!(parked.get(&2).map(String::as_str), Some("pane-2 history"));
    }

    /// The case the old inline swap silently mishandled: focusing a pane
    /// with nothing parked. The new pane must start fresh, and — the half
    /// the old code lost — the previous pane's state must still be parked.
    #[test]
    fn a_fresh_pane_starts_fresh_and_the_old_pane_keeps_its_state() {
        let mut focused = String::from("pane-1 history");
        let mut parked: HashMap<u64, String> = HashMap::new();

        swap_focus(&mut focused, &mut parked, 1, 7, || "FRESH".into());
        assert_eq!(focused, "FRESH", "a new split does not inherit its neighbour");
        assert_eq!(
            parked.get(&1).map(String::as_str),
            Some("pane-1 history"),
            "the old pane's history is parked, not dropped"
        );
    }

    #[test]
    fn same_pane_is_a_no_op() {
        let mut focused = String::from("stay");
        let mut parked: HashMap<u64, String> = HashMap::new();
        swap_focus(&mut focused, &mut parked, 3, 3, || "FRESH".into());
        assert_eq!(focused, "stay");
        assert!(parked.is_empty());
    }

    /// The session bundle is constructible and starts empty — the smoke the
    /// binary's reconcile loop relies on when it spawns one per new pane.
    #[test]
    fn a_new_session_is_empty() {
        let s = PaneSession::new();
        assert!(s.pty_out.is_empty());
        assert_eq!(s.pty_echoed, 0);
        assert_eq!(s.grid.cursor(), (0, 0));
    }
}
