//! Multitask switcher.
//!
//! "Multitask focused" means three things here, each a deliberate departure
//! from a one-card-at-a-time phone switcher:
//!
//! 1. **MRU order, always.** The window you were just in is first; the one
//!    before it second. Recency comes from watching `activated` flip in the
//!    toplevel list, so it also tracks switches made by other means (the bar,
//!    keyboard shortcuts, a notification).
//! 2. **Grid by default.** Two columns show four to six windows at once on a
//!    phone, so picking among several open apps is one look and one tap.
//!    `Deck` (horizontal, one large card) is the alternative style.
//! 3. **Quick switch without opening anything.** A horizontal swipe along the
//!    bottom edge steps through recent windows. Consecutive steps within
//!    [`STREAK_MS`] walk a frozen snapshot of the MRU list, so three swipes
//!    reach the third-most-recent app instead of bouncing between two (the
//!    iOS behaviour). The streak settles after a pause, and the order is
//!    rebuilt from the window you landed on.

use crate::edge::Dir;
use crate::{Action, Millis, WindowCard};

/// Pause after which a quick-switch streak settles.
pub const STREAK_MS: Millis = 1500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Grid,
    Deck,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Streak {
    snapshot: Vec<u64>,
    cursor: usize,
    last: Millis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Switcher {
    pub open: bool,
    pub style: Style,
    windows: Vec<WindowCard>,
    /// Window ids, most recent first.
    mru: Vec<u64>,
    streak: Option<Streak>,
    /// Card being flicked: (id, vertical offset px).
    pub dragging: Option<(u64, f32)>,
    /// Upward offset past which a released card closes its window.
    pub close_px: f32,
}

impl Default for Switcher {
    fn default() -> Self {
        Switcher { open: false, style: Style::Grid, windows: Vec::new(), mru: Vec::new(), streak: None, dragging: None, close_px: 120.0 }
    }
}

impl Switcher {
    /// Replace the window list. Maintains MRU from `activated` and returns
    /// nothing: the switcher never acts on its own when windows change.
    pub fn sync(&mut self, windows: &[WindowCard]) {
        self.mru.retain(|id| windows.iter().any(|w| w.id == *id));
        // New windows go to the back unless activated (then front, below).
        for w in windows {
            if !self.mru.contains(&w.id) {
                self.mru.push(w.id);
            }
        }
        // During a streak, the compositor reporting our own activations must
        // not reorder the list the user is walking.
        if self.streak.is_none() {
            if let Some(active) = windows.iter().find(|w| w.activated) {
                self.promote(active.id);
            }
        }
        if let Some(s) = &mut self.streak {
            s.snapshot.retain(|id| windows.iter().any(|w| w.id == *id));
            s.cursor = s.cursor.min(s.snapshot.len().saturating_sub(1));
        }
        if let Some((id, _)) = self.dragging {
            if !windows.iter().any(|w| w.id == id) {
                self.dragging = None;
            }
        }
        self.windows = windows.to_vec();
    }

    fn promote(&mut self, id: u64) {
        if let Some(pos) = self.mru.iter().position(|x| *x == id) {
            let v = self.mru.remove(pos);
            self.mru.insert(0, v);
        }
    }

    pub fn mru_ids(&self) -> &[u64] {
        &self.mru
    }

    /// Cards in display order.
    pub fn cards(&self) -> Vec<&WindowCard> {
        self.mru.iter().filter_map(|id| self.windows.iter().find(|w| w.id == *id)).collect()
    }

    pub fn is_streaking(&self) -> bool {
        self.streak.is_some()
    }

    /// One quick-switch step. `Back` goes to older windows.
    pub fn quick_switch(&mut self, dir: Dir, now: Millis) -> Option<Action> {
        let snapshot_fresh = |s: &Streak| now.saturating_sub(s.last) < STREAK_MS;
        if !self.streak.as_ref().is_some_and(snapshot_fresh) {
            self.settle();
            if self.mru.len() < 2 {
                return None;
            }
            self.streak = Some(Streak { snapshot: self.mru.clone(), cursor: 0, last: now });
        }
        let s = self.streak.as_mut()?;
        let n = s.snapshot.len();
        let next = match dir {
            Dir::Back if s.cursor + 1 < n => s.cursor + 1,
            Dir::Forward if s.cursor > 0 => s.cursor - 1,
            // At either end: stay (a wrap would surprise more than a stop).
            _ => {
                s.last = now;
                return None;
            }
        };
        s.cursor = next;
        s.last = now;
        Some(Action::Activate(s.snapshot[next]))
    }

    /// Called from the shell's tick: settle a streak after the pause.
    pub fn tick(&mut self, now: Millis) {
        if self.streak.as_ref().is_some_and(|s| now.saturating_sub(s.last) >= STREAK_MS) {
            self.settle();
        }
    }

    /// End a streak: the landed-on window becomes most recent.
    pub fn settle(&mut self) {
        if let Some(s) = self.streak.take() {
            if let Some(&id) = s.snapshot.get(s.cursor) {
                self.promote(id);
            }
        }
    }

    pub fn show(&mut self) {
        self.settle();
        self.open = true;
        self.dragging = None;
    }

    pub fn hide(&mut self) {
        self.open = false;
        self.dragging = None;
    }

    pub fn tap(&mut self, id: u64) -> Vec<Action> {
        self.hide();
        self.promote(id);
        vec![Action::Activate(id)]
    }

    pub fn drag_card(&mut self, id: u64, dy: f32) {
        // Only upward travel means anything; clamp so a downward tug does not
        // push the card off its slot.
        self.dragging = Some((id, dy.min(0.0)));
    }

    pub fn release_card(&mut self) -> Vec<Action> {
        match self.dragging.take() {
            Some((id, dy)) if -dy >= self.close_px => vec![Action::Close(id)],
            _ => vec![],
        }
    }

    pub fn close_all(&mut self) -> Vec<Action> {
        self.hide();
        self.mru.iter().map(|id| Action::Close(*id)).collect()
    }

    /// Card rectangles for the current style, in display order, inside a
    /// `w`×`h` area. Cards keep the screen's aspect ratio.
    pub fn layout(&self, w: f32, h: f32) -> Vec<Rect> {
        let n = self.cards().len();
        let aspect = if w > 0.0 { h / w } else { 2.0 };
        let gap = 14.0;
        match self.style {
            Style::Grid => {
                let cw = (w - gap * 3.0) / 2.0;
                let ch = cw * aspect * 0.78; // a little squatter than the screen so titles fit
                (0..n)
                    .map(|i| {
                        let (col, row) = ((i % 2) as f32, (i / 2) as f32);
                        Rect { x: gap + col * (cw + gap), y: gap + row * (ch + gap + 22.0), w: cw, h: ch }
                    })
                    .collect()
            }
            Style::Deck => {
                let cw = w * 0.72;
                let ch = cw * aspect;
                let step = cw * 0.55;
                (0..n)
                    .map(|i| Rect { x: (w - cw) / 2.0 + i as f32 * step, y: (h - ch) / 2.0, w: cw, h: ch })
                    .collect()
            }
        }
    }

    pub fn card_at(&self, w: f32, h: f32, x: f32, y: f32) -> Option<u64> {
        let cards = self.cards();
        // Reverse: later cards draw on top in Deck, so they win the hit test.
        self.layout(w, h).iter().zip(cards.iter()).rev().find(|(r, _)| r.contains(x, y)).map(|(_, c)| c.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: u64, activated: bool) -> WindowCard {
        WindowCard { id, title: format!("w{id}"), app_id: format!("app{id}"), activated, maximized: false }
    }

    /// Three windows, activated in order 1, 2, 3 → MRU [3, 2, 1].
    fn three() -> Switcher {
        let mut s = Switcher::default();
        s.sync(&[win(1, true)]);
        s.sync(&[win(1, false), win(2, true)]);
        s.sync(&[win(1, false), win(2, false), win(3, true)]);
        s
    }

    #[test]
    fn mru_follows_activation() {
        let mut s = three();
        assert_eq!(s.mru_ids(), &[3, 2, 1]);
        s.sync(&[win(1, true), win(2, false), win(3, false)]);
        assert_eq!(s.mru_ids(), &[1, 3, 2]);
        s.sync(&[win(1, true), win(3, false)]);
        assert_eq!(s.mru_ids(), &[1, 3], "closed window dropped");
    }

    #[test]
    fn streak_walks_snapshot_instead_of_bouncing() {
        let mut s = three();
        assert_eq!(s.quick_switch(Dir::Back, 0), Some(Action::Activate(2)));
        // The compositor reports 2 activated mid-streak; order must not change.
        s.sync(&[win(1, false), win(2, true), win(3, false)]);
        assert_eq!(s.quick_switch(Dir::Back, 300), Some(Action::Activate(1)));
        assert_eq!(s.quick_switch(Dir::Back, 600), None, "stops at the oldest");
        assert_eq!(s.quick_switch(Dir::Forward, 900), Some(Action::Activate(2)));
        s.tick(900 + STREAK_MS);
        assert!(!s.is_streaking());
        assert_eq!(s.mru_ids(), &[2, 3, 1], "landed window promoted on settle");
    }

    #[test]
    fn after_a_streak_the_origin_window_is_one_step_back() {
        // Walk 3 -> 2 -> 1 with the compositor reporting each activation, as
        // it does on a real phone. Those reports must not reorder the list
        // mid-streak: after settling, Back from 1 returns to 3 (where the
        // walk started), not to 2 (merely the last window passed through).
        let mut s = three();
        s.quick_switch(Dir::Back, 0);
        s.sync(&[win(1, false), win(2, true), win(3, false)]);
        s.quick_switch(Dir::Back, 200);
        s.sync(&[win(1, true), win(2, false), win(3, false)]);
        s.tick(200 + STREAK_MS);
        assert_eq!(s.mru_ids(), &[1, 3, 2]);
        assert_eq!(s.quick_switch(Dir::Back, 10_000), Some(Action::Activate(3)));
    }

    #[test]
    fn a_pause_starts_a_fresh_streak_from_new_order() {
        let mut s = three();
        assert_eq!(s.quick_switch(Dir::Back, 0), Some(Action::Activate(2)));
        // No tick in between: quick_switch itself must notice the pause.
        assert_eq!(s.quick_switch(Dir::Back, STREAK_MS + 10), Some(Action::Activate(3)), "2 is now first, so Back goes to 3");
    }

    #[test]
    fn quick_switch_needs_two_windows_and_forward_at_start_is_noop() {
        let mut s = Switcher::default();
        s.sync(&[win(1, true)]);
        assert_eq!(s.quick_switch(Dir::Back, 0), None);
        let mut s = three();
        assert_eq!(s.quick_switch(Dir::Forward, 0), None);
    }

    #[test]
    fn card_flick_closes_only_past_threshold() {
        let mut s = three();
        s.drag_card(2, -119.0);
        assert!(s.release_card().is_empty());
        s.drag_card(2, -121.0);
        assert_eq!(s.release_card(), vec![Action::Close(2)]);
        s.drag_card(2, 300.0);
        assert!(s.release_card().is_empty(), "downward drag never closes");
    }

    #[test]
    fn tap_activates_hides_and_promotes() {
        let mut s = three();
        s.show();
        assert_eq!(s.tap(1), vec![Action::Activate(1)]);
        assert!(!s.open);
        assert_eq!(s.mru_ids()[0], 1);
    }

    #[test]
    fn grid_layout_is_two_columns_and_hit_tests() {
        let s = three();
        let r = s.layout(400.0, 800.0);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].y, r[1].y);
        assert!(r[2].y > r[0].y && r[2].x == r[0].x);
        let c = r[1];
        assert_eq!(s.card_at(400.0, 800.0, c.x + 5.0, c.y + 5.0), Some(s.cards()[1].id));
        assert_eq!(s.card_at(400.0, 800.0, 1.0, 1.0), None);
    }

    #[test]
    fn deck_overlap_hit_goes_to_topmost() {
        let mut s = three();
        s.style = Style::Deck;
        let r = s.layout(400.0, 800.0);
        // A point inside both card 0 and card 1: card 1 draws later, so it wins.
        let x = r[1].x + 2.0;
        assert!(r[0].contains(x, r[0].y + 10.0));
        assert_eq!(s.card_at(400.0, 800.0, x, r[0].y + 10.0), Some(s.cards()[1].id));
    }
}
