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
pub const CENTER_FRAC: f32 = 0.8;

/// Width fraction of one peek strip.
pub fn peek_frac() -> f32 {
    (1.0 - CENTER_FRAC) / 2.0
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
        for w in windows {
            if !self.ring.contains(&w.id) {
                let pos = at.map(|a| a + 1).unwrap_or(self.ring.len()).min(self.ring.len());
                self.ring.insert(pos, w.id);
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
    fn new_window_joins_after_current() {
        let mut c = Carousel::default();
        c.enter(&[1, 2, 3], &[w(1, true), w(2, true), w(3, true)]);
        assert!(c.sync(&[w(1, true), w(2, true), w(3, true), w(9, false)]).is_empty());
        assert_eq!(c.ring(), &[1, 9, 2, 3]);
        assert_eq!(c.current(), Some(1));
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
        assert!((peek_frac() - 0.1).abs() < 1e-6);
    }

    #[test]
    fn entering_with_nothing_open_stays_inactive() {
        let mut c = Carousel::default();
        assert!(c.enter(&[], &[]).is_empty());
        assert!(!c.is_active());
    }
}
