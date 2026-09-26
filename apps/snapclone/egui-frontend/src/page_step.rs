//! One gesture, one step — and a step visits a page's sections before it
//! leaves the page.
//!
//! Two rules, kept apart from egui so both can be tested without a frame:
//!
//! * [`step`] says where a step lands. A page with sections (Inbox's
//!   All/Unread/Sent, Settings' five tabs) is walked section by section; only
//!   past its last section does the step reach the next page, and it arrives
//!   at that page's FIRST section. Stepping back arrives at the previous
//!   page's LAST section, so going forward then back returns to where you
//!   were rather than jumping to a section you never saw.
//! * [`Gate`] says whether a scroll-driven step is allowed at all. A trackpad
//!   swipe is not one event: it is a burst of deltas, and with kinetic
//!   scrolling it keeps coming after the fingers lift. Every detent inside
//!   that burst used to turn a page, so one swipe travelled three. The gate
//!   allows one step per burst, and a burst ends only after [`IDLE_GAP_MS`]
//!   with no scroll at all.
//!
//! Touch drags do not go through the gate: the live swipe decides once, at
//! release, and that is already one step per gesture.

/// Where the view is: a page, and a section within it (0 on a page that has
/// none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub page: usize,
    pub section: usize,
}

/// The position one step from `pos` in the direction of `sign` (only its
/// sign matters). `sections(page)` is how many sections a page shows; 0 and
/// 1 both mean "no sections". At either end of the whole sequence the
/// position is returned unchanged.
pub fn step(pos: Pos, sign: i32, pages: usize, sections: impl Fn(usize) -> usize) -> Pos {
    if pages == 0 || sign == 0 {
        return pos;
    }
    let count = |p: usize| sections(p).max(1);
    let here = count(pos.page);
    let section = pos.section.min(here - 1);
    if sign > 0 {
        if section + 1 < here {
            Pos { page: pos.page, section: section + 1 }
        } else if pos.page + 1 < pages {
            Pos { page: pos.page + 1, section: 0 }
        } else {
            Pos { page: pos.page, section }
        }
    } else if section > 0 {
        Pos { page: pos.page, section: section - 1 }
    } else if pos.page > 0 {
        let prev = pos.page - 1;
        Pos { page: prev, section: count(prev) - 1 }
    } else {
        Pos { page: pos.page, section }
    }
}

/// How long the scroll must be quiet before another swipe counts. Longer
/// than a kinetic tail's gaps between samples (a frame or two), shorter
/// than a deliberate second swipe — two quick swipes still travel two.
pub const IDLE_GAP_MS: f64 = 280.0;

/// One step per scroll burst.
#[derive(Clone, Copy, Debug, Default)]
pub struct Gate {
    /// When scroll along the page axis was last seen, if a burst is open.
    last_ms: Option<f64>,
    /// Whether this burst has already stepped.
    spent: bool,
}

impl Gate {
    /// Feed once per frame: `active` is whether this frame carried any
    /// scroll along the page axis. A quiet frame past the gap closes the
    /// burst; an active frame after the gap opens a new one.
    pub fn observe(&mut self, now_ms: f64, active: bool) {
        let expired = self.last_ms.is_some_and(|t| now_ms - t > IDLE_GAP_MS);
        if expired {
            self.last_ms = None;
            self.spent = false;
        }
        if active {
            self.last_ms = Some(now_ms);
        }
    }

    /// Ask to step. True at most once per burst; a request outside any
    /// burst (a caller that did not observe first) opens one.
    pub fn try_step(&mut self, now_ms: f64) -> bool {
        self.observe(now_ms, true);
        if self.spent {
            return false;
        }
        self.spent = true;
        true
    }

    /// Whether a burst is open and has already stepped — for callers that
    /// want to swallow the rest of the burst rather than hand it on.
    pub fn spent(&self) -> bool {
        self.spent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Camera, Inbox(3), Friends, Wayfinder, Settings(5) — the app's shape.
    fn shape(p: usize) -> usize {
        match p { 1 => 3, 4 => 5, _ => 0 }
    }

    fn walk(mut pos: Pos, sign: i32, n: usize) -> Vec<Pos> {
        (0..n).map(|_| { pos = step(pos, sign, 5, shape); pos }).collect()
    }

    #[test]
    fn sections_come_before_the_next_page() {
        let got = walk(Pos { page: 0, section: 0 }, 1, 10);
        let want = [(1, 0), (1, 1), (1, 2), (2, 0), (3, 0), (4, 0), (4, 1), (4, 2), (4, 3), (4, 4)];
        assert_eq!(got.iter().map(|p| (p.page, p.section)).collect::<Vec<_>>(), want);
    }

    #[test]
    fn backwards_arrives_at_the_last_section() {
        let got = walk(Pos { page: 2, section: 0 }, -1, 4);
        let want = [(1, 2), (1, 1), (1, 0), (0, 0)];
        assert_eq!(got.iter().map(|p| (p.page, p.section)).collect::<Vec<_>>(), want);
    }

    #[test]
    fn both_ends_stop_rather_than_wrap() {
        assert_eq!(step(Pos { page: 0, section: 0 }, -1, 5, shape), Pos { page: 0, section: 0 });
        assert_eq!(step(Pos { page: 4, section: 4 }, 1, 5, shape), Pos { page: 4, section: 4 });
    }

    #[test]
    fn an_out_of_range_section_is_clamped_first() {
        // A section index left over from a page with more sections must not
        // make the step skip a page or panic.
        assert_eq!(step(Pos { page: 2, section: 7 }, 1, 5, shape), Pos { page: 3, section: 0 });
        assert_eq!(step(Pos { page: 1, section: 9 }, -1, 5, shape), Pos { page: 1, section: 1 });
    }

    #[test]
    fn a_burst_steps_once_however_many_detents_it_fires() {
        let mut g = Gate::default();
        let mut steps = 0;
        // 600 ms of continuous scroll at 60 fps, a detent every 5th frame —
        // what a long trackpad swipe with a kinetic tail produces.
        for f in 0..36 {
            let t = f as f64 * 16.7;
            g.observe(t, true);
            if f % 5 == 0 && g.try_step(t) { steps += 1; }
        }
        assert_eq!(steps, 1);
    }

    #[test]
    fn a_second_swipe_after_a_pause_steps_again() {
        let mut g = Gate::default();
        g.observe(0.0, true);
        assert!(g.try_step(0.0));
        // Quiet frames: the burst closes once the gap passes...
        for f in 1..=20 { g.observe(f as f64 * 16.7, false); }
        // ...and the next swipe is its own gesture.
        let t = 21.0 * 16.7;
        g.observe(t, true);
        assert!(g.try_step(t), "a deliberate second swipe was swallowed");
    }

    #[test]
    fn a_short_lull_inside_a_burst_does_not_reopen_it() {
        let mut g = Gate::default();
        g.observe(0.0, true);
        assert!(g.try_step(0.0));
        // Two quiet frames — a kinetic tail's sampling gap, not a new swipe.
        g.observe(16.7, false);
        g.observe(33.4, false);
        g.observe(50.1, true);
        assert!(!g.try_step(50.1), "a gap shorter than IDLE_GAP_MS split one swipe in two");
    }
}
