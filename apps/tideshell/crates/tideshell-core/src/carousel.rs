//! Carousel focus mode: one window at a time, maximized into the centre
//! CENTER_FRAC of the screen, with its ring neighbours peeking in the strips
//! either side. A TAP on a peek cycles to that neighbour: no swipe, no
//! dragging, one tap. The bottom-edge horizontal swipe still steps too.
//!
//! The peeks are the runtime's business (layer surfaces with exclusive
//! zones, so maximized windows land in the centre); this model says which
//! window each peek shows and what a tap on it does. The ring is frozen when the mode starts (MRU order at that
//! moment), so stepping is spatial and predictable: left is always the same
//! window until you leave the mode.
//!
//! The mode maximizes rather than fullscreens so the right-hand bar (an
//! exclusive-zone layer surface) stays visible when that layout is active.
//! Windows that were already maximized when the mode started are left
//! maximized on exit; only windows the carousel maximized are restored.

use crate::edge::Dir;
use crate::{Action, WindowCard};

/// Width fraction the active window gets; the rest is split into two peeks.
pub const CENTER_FRAC: f32 = 0.85;

/// Width fraction of one peek strip.
pub fn peek_frac() -> f32 {
    (1.0 - CENTER_FRAC) / 2.0
}

/// The peek strip's width fraction under a layout: the thick bar's carousel is 5% narrower, the thin bar's half
/// as wide (reduced carousel), and with no bar it is what it always was.
pub fn peek_frac_for(layout: crate::prefs::Layout) -> f32 {
    use crate::prefs::Layout;
    match layout {
        Layout::PullDown => peek_frac(),
        Layout::Thin => peek_frac() * 0.5,
        Layout::RightBar => peek_frac() * crate::prefs::THICK_SCALE,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Carousel {
    active: bool,
    ring: Vec<u64>,
    idx: usize,
    /// Windows this mode maximized (and so must un-maximize on exit).
    touched: Vec<u64>,
}

impl Carousel {
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn ring(&self) -> &[u64] {
        &self.ring
    }

    pub fn current(&self) -> Option<u64> {
        self.active.then(|| self.ring.get(self.idx).copied()).flatten()
    }

    /// Position as (index, count) for a page indicator.
    pub fn position(&self) -> (usize, usize) {
        (self.idx, self.ring.len())
    }

    /// The window a peek shows: the ring neighbour on that side (wrapping).
    /// None with fewer than two windows (nothing to cycle to).
    pub fn peek(&self, side: Side) -> Option<u64> {
        let n = self.ring.len();
        if !self.active || n < 2 {
            return None;
        }
        let i = match side {
            Side::Left => (self.idx + n - 1) % n,
            Side::Right => (self.idx + 1) % n,
        };
        self.ring.get(i).copied()
    }

    /// A tap on a peek: cycle to the window it shows.
    pub fn tap_peek(&mut self, side: Side, windows: &[WindowCard]) -> Vec<Action> {
        match side {
            Side::Left => self.step(Dir::Back, windows),
            Side::Right => self.step(Dir::Forward, windows),
        }
    }

    fn focus(&mut self, id: u64, windows: &[WindowCard]) -> Vec<Action> {
        let already_max = windows.iter().any(|w| w.id == id && w.maximized);
        let mut out = vec![Action::Activate(id)];
        if !already_max {
            out.push(Action::SetMaximized(id, true));
            if !self.touched.contains(&id) {
                self.touched.push(id);
            }
        }
        out
    }

    /// Enter with `mru` (most recent first). Starts on the most recent window.
    pub fn enter(&mut self, mru: &[u64], windows: &[WindowCard]) -> Vec<Action> {
        if mru.is_empty() {
            return vec![];
        }
        self.active = true;
        self.ring = mru.to_vec();
        self.idx = 0;
        self.touched.clear();
        self.focus(self.ring[0], windows)
    }

    pub fn exit(&mut self) -> Vec<Action> {
        if !self.active {
            return vec![];
        }
        self.active = false;
        let out = self.touched.drain(..).map(|id| Action::SetMaximized(id, false)).collect();
        self.ring.clear();
        self.idx = 0;
        out
    }

    /// Step to a neighbour. Wraps: in a spatial ring, wrapping is the
    /// expected shape (unlike MRU quick-switch, where it would be surprising).
    pub fn step(&mut self, dir: Dir, windows: &[WindowCard]) -> Vec<Action> {
        if !self.active || self.ring.len() < 2 {
            return vec![];
        }
        let n = self.ring.len();
        self.idx = match dir {
            Dir::Back => (self.idx + n - 1) % n,
            Dir::Forward => (self.idx + 1) % n,
        };
        self.focus(self.ring[self.idx], windows)
    }

    /// Window list changed. Closed windows leave the ring; new ones join
    /// right after the current window (where the user will look for them).
    /// If the current window closed, focus moves to what took its place.
    pub fn sync(&mut self, windows: &[WindowCard]) -> Vec<Action> {
        if !self.active {
            return vec![];
        }
        let cur = self.ring.get(self.idx).copied();
        self.ring.retain(|id| windows.iter().any(|w| w.id == *id));
        self.touched.retain(|id| windows.iter().any(|w| w.id == *id));
        let at = cur.and_then(|c| self.ring.iter().position(|x| *x == c));
        // A window that opens while the mode is on is what the user just launched: it joins right after
        // the current one AND becomes current, maximized, instead of appearing tiled beside the carousel.
        let mut opened: Option<u64> = None;
        let mut pos = at.map(|a| a + 1).unwrap_or(self.ring.len()).min(self.ring.len());
        for w in windows {
            if !self.ring.contains(&w.id) {
                self.ring.insert(pos, w.id);
                pos += 1;
                opened = Some(w.id);
            }
        }
        if let Some(id) = opened {
            if self.ring.len() > 0 {
                self.idx = self.ring.iter().position(|x| *x == id).unwrap_or(self.idx);
                return self.focus(id, windows);
            }
        }
        if self.ring.is_empty() {
            self.active = false;
            self.idx = 0;
            return vec![];
        }
        match at {
            Some(a) => {
                self.idx = a;
                vec![]
            }
            None => {
                // Current closed: stay at the same index (the next window slid
                // into it), clamped at the end.
                self.idx = self.idx.min(self.ring.len() - 1);
                self.focus(self.ring[self.idx], windows)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_carousel_peeks_narrow_with_the_bar() {
        use crate::prefs::Layout;
        let (top, thick, thin) = (peek_frac_for(Layout::PullDown), peek_frac_for(Layout::RightBar), peek_frac_for(Layout::Thin));
        assert!((top - 0.075).abs() < 1e-6, "no bar: unchanged");
        assert!((thick / top - 0.95).abs() < 1e-5, "thick: 5% narrower");
        assert!((thin / top - 0.5).abs() < 1e-5, "thin: half");
    }

    #[test]
    fn a_window_that_opens_in_the_mode_becomes_current_and_maximized() {
        let mut c = Carousel::default();
        let ws = [w(1, false), w(2, false)];
        c.enter(&[1, 2], &ws);
        let ws3 = [w(1, true), w(2, false), w(3, false)];
        let out = c.sync(&ws3);
        assert_eq!(out, vec![Action::Activate(3), Action::SetMaximized(3, true)]);
        assert_eq!(c.current(), Some(3));
        assert_eq!(c.ring(), &[1, 3, 2], "it sits right after the window it was launched from");
        // and is restored on exit with the others the mode maximized
        assert!(c.exit().contains(&Action::SetMaximized(3, false)));
    }

    #[test]
    fn a_sync_with_no_new_window_changes_nothing() {
        let mut c = Carousel::default();
        let ws = [w(1, false), w(2, false)];
        c.enter(&[1, 2], &ws);
        assert!(c.sync(&ws).is_empty());
        assert_eq!(c.current(), Some(1));
    }

    fn w(id: u64, max: bool) -> WindowCard {
        WindowCard { id, title: String::new(), app_id: String::new(), activated: false, maximized: max, width: None }
    }

    #[test]
    fn enter_maximizes_current_and_exit_restores_only_what_it_touched() {
        let ws = [w(1, false), w(2, true), w(3, false)];
        let mut c = Carousel::default();
        assert_eq!(c.enter(&[1, 2, 3], &ws), vec![Action::Activate(1), Action::SetMaximized(1, true)]);
        assert_eq!(c.step(Dir::Forward, &ws), vec![Action::Activate(2)], "2 was already maximized");
        assert_eq!(c.step(Dir::Forward, &ws), vec![Action::Activate(3), Action::SetMaximized(3, true)]);
        let mut out = c.exit();
        out.sort_by_key(|a| format!("{a:?}"));
        assert_eq!(out, vec![Action::SetMaximized(1, false), Action::SetMaximized(3, false)]);
        assert!(!c.is_active());
        assert!(c.exit().is_empty());
    }

    #[test]
    fn step_wraps_both_ways() {
        let ws = [w(1, true), w(2, true), w(3, true)];
        let mut c = Carousel::default();
        c.enter(&[1, 2, 3], &ws);
        c.step(Dir::Back, &ws);
        assert_eq!(c.current(), Some(3));
        c.step(Dir::Forward, &ws);
        assert_eq!(c.current(), Some(1));
    }

    #[test]
    fn closing_current_focuses_the_one_that_slid_in() {
        let mut c = Carousel::default();
        c.enter(&[1, 2, 3], &[w(1, true), w(2, true), w(3, true)]);
        c.step(Dir::Forward, &[w(1, true), w(2, true), w(3, true)]);
        assert_eq!(c.current(), Some(2));
        let out = c.sync(&[w(1, true), w(3, false)]);
        assert_eq!(c.current(), Some(3));
        assert_eq!(out, vec![Action::Activate(3), Action::SetMaximized(3, true)]);
    }

    #[test]
    fn closing_last_in_ring_clamps_and_empty_ring_exits() {
        let mut c = Carousel::default();
        c.enter(&[1, 2], &[w(1, true), w(2, true)]);
        c.step(Dir::Forward, &[w(1, true), w(2, true)]);
        c.sync(&[w(1, true)]);
        assert_eq!(c.current(), Some(1));
        assert!(c.sync(&[]).is_empty());
        assert!(!c.is_active());
    }

    #[test]
    fn new_window_joins_after_current_and_takes_focus() {
        // (It used to stay put, tiled, until stepped to; a launch should land full size.)
        let mut c = Carousel::default();
        c.enter(&[1, 2, 3], &[w(1, true), w(2, true), w(3, true)]);
        let out = c.sync(&[w(1, true), w(2, true), w(3, true), w(9, false)]);
        assert_eq!(out, vec![Action::Activate(9), Action::SetMaximized(9, true)]);
        assert_eq!(c.ring(), &[1, 9, 2, 3]);
        assert_eq!(c.current(), Some(9));
    }

    #[test]
    fn peeks_show_the_ring_neighbours_and_a_tap_cycles() {
        let ws = [w(1, true), w(2, true), w(3, true)];
        let mut c = Carousel::default();
        assert_eq!(c.peek(Side::Left), None, "inactive: no peeks");
        c.enter(&[1, 2, 3], &ws);
        assert_eq!((c.peek(Side::Left), c.peek(Side::Right)), (Some(3), Some(2)), "wraps at the start");
        assert_eq!(c.tap_peek(Side::Right, &ws), vec![Action::Activate(2)]);
        assert_eq!((c.current(), c.peek(Side::Left), c.peek(Side::Right)), (Some(2), Some(1), Some(3)));
        assert_eq!(c.tap_peek(Side::Left, &ws), vec![Action::Activate(1)]);
        assert_eq!(c.current(), Some(1));
        // Two windows: both peeks show the same other window.
        let mut c = Carousel::default();
        c.enter(&[1, 2], &ws[..2]);
        assert_eq!((c.peek(Side::Left), c.peek(Side::Right)), (Some(2), Some(2)));
        // One window: nothing to cycle to.
        let mut c = Carousel::default();
        c.enter(&[1], &ws[..1]);
        assert_eq!((c.peek(Side::Left), c.peek(Side::Right)), (None, None));
        assert!(c.tap_peek(Side::Right, &ws[..1]).is_empty());
        assert!((peek_frac() - 0.075).abs() < 1e-6);
    }

    #[test]
    fn entering_with_nothing_open_stays_inactive() {
        let mut c = Carousel::default();
        assert!(c.enter(&[], &[]).is_empty());
        assert!(!c.is_active());
    }
}

// ── the bumper in sidebar mode ──────────────────────────────────────────────
//
// With the pull-down layout, stepping the carousel is a full-height peek
// strip down the screen's edge: a wide target that also shows the
// neighbouring window. On the right-hand bar that strip cannot exist — the
// bar is already there and already narrow — so the step control becomes a
// BUTTON inside the bar instead.
//
// It is vertically centred because that is the one place a thumb reaches
// without regripping, whatever the size of the hand or the phone. And it
// reserves its own space rather than floating over the bar's other widgets:
// a control that overlaps what it sits on is a control people press by
// accident.

/// How tall the bumper button is, as a fraction of the bar. Enough to be a
/// comfortable target, small enough to leave the bar useful.
pub const BUMPER_FRAC: f32 = 0.18;
/// It never shrinks below this many logical pixels, however short the bar.
pub const BUMPER_MIN_PX: f32 = 56.0;

/// Where the bumper sits in a bar of `bar_h` logical pixels starting at
/// `bar_top`, and what is left for everything else.
///
/// Returns `(bumper_top, bumper_h, above_h, below_h)`: the widgets above and
/// below get the remaining halves, so the bar's own content is laid out
/// around the button rather than under it.
pub fn bumper_slot(bar_top: f32, bar_h: f32) -> (f32, f32, f32, f32) {
    let h = (bar_h * BUMPER_FRAC).max(BUMPER_MIN_PX).min(bar_h);
    let top = bar_top + (bar_h - h) / 2.0;
    let above = (top - bar_top).max(0.0);
    let below = (bar_top + bar_h - (top + h)).max(0.0);
    (top, h, above, below)
}

/// Is `y` inside the bumper, for hit testing?
pub fn bumper_hit(y: f32, bar_top: f32, bar_h: f32) -> bool {
    let (top, h, _, _) = bumper_slot(bar_top, bar_h);
    y >= top && y < top + h
}

#[cfg(test)]
mod bumper_tests {
    use super::*;

    #[test]
    fn the_bumper_is_centred_whatever_the_bar() {
        for bar_h in [200.0f32, 640.0, 1200.0, 2400.0] {
            let (top, h, above, below) = bumper_slot(0.0, bar_h);
            assert!((above - below).abs() < 1e-3, "off centre at {bar_h}: {above} vs {below}");
            assert!((top + h / 2.0 - bar_h / 2.0).abs() < 1e-3, "the middle is where a thumb reaches");
        }
        // An offset bar is centred on itself, not on the screen.
        let (top, h, _, _) = bumper_slot(100.0, 400.0);
        assert!((top + h / 2.0 - 300.0).abs() < 1e-3);
    }

    #[test]
    fn it_makes_space_rather_than_floating_over_the_bar() {
        let (top, h, above, below) = bumper_slot(0.0, 1000.0);
        assert!((above + h + below - 1000.0).abs() < 1e-3, "the three regions tile the bar exactly");
        assert!(above > 0.0 && below > 0.0, "and the bar keeps room on both sides");
        assert_eq!(top, above);
    }

    #[test]
    fn a_short_bar_keeps_a_pressable_target() {
        // 18% of a short bar would be a sliver; the floor wins, and the
        // button still fits.
        let (_, h, above, below) = bumper_slot(0.0, 200.0);
        assert_eq!(h, BUMPER_MIN_PX);
        assert!(above >= 0.0 && below >= 0.0);
        // Absurdly short: the bumper takes what there is rather than
        // overflowing the bar.
        let (top, h, above, below) = bumper_slot(0.0, 40.0);
        assert_eq!(h, 40.0);
        assert_eq!((top, above, below), (0.0, 0.0, 0.0));
    }

    #[test]
    fn hit_testing_agrees_with_the_layout() {
        let (top, h, _, _) = bumper_slot(0.0, 1000.0);
        assert!(bumper_hit(top + h / 2.0, 0.0, 1000.0));
        assert!(bumper_hit(top, 0.0, 1000.0), "the top edge belongs to the button");
        assert!(!bumper_hit(top + h, 0.0, 1000.0), "and the bottom edge to what follows");
        assert!(!bumper_hit(top - 1.0, 0.0, 1000.0));
        assert!(!bumper_hit(0.0, 0.0, 1000.0));
    }
}
