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

/// The per-pane byte buffer bound for that pane's shell.
///
/// It used to carry a grid, a VT parser and two render caches as well.
/// None of that was ever DRAWN: `app.render_at` paints
/// `app.terminal.grid`, and a pane's terminal lives in its `App` —
/// focused in `self.app`, parked in `pane_apps`. The render half here
/// was a second copy that was fed, laid out and then overpainted.
///
/// What is left is the part that was always real: bytes going to the
/// shell, and how much of them has been handled.
pub struct PaneSession {
    /// Bytes going TO the shell; a host drains this to a real PTY.
    pub pty_out: Vec<u8>,
    /// High-water mark of `pty_out` already handled.
    pub pty_echoed: usize,
}

impl PaneSession {
    pub fn new() -> PaneSession {
        PaneSession {
            pty_out: Vec::new(),
            pty_echoed: 0,
        }
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
        assert_eq!(s.pty_echoed, 0);
    }
}
