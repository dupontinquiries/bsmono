//! Touch rejection, in front of every consumer (strips, overlay, pad):
//! what water, a palm or a pocket produce, dropped before it means anything.
//!
//! * **Rain / wet screen**: water makes bursts of brief, still touches at
//!   DIFFERENT places. Four such touches (each under RAIN_TAP_MS, moving
//!   less than RAIN_SLOP), spread over the screen (each at least RAIN_SPREAD
//!   from the previous one), within RAIN_WINDOW_MS put the filter in a wet
//!   state for WET_MS, during which every new contact is dropped; more
//!   phantoms extend it. A person hammering one button lands on the same
//!   spot every time and is never rain (the FP5 log showed eight taps on a
//!   dead launcher button trip the first version, and 13 touches dropped).
//! * **Jumps**: a contact that moves more than JUMP_PX in one short sample is
//!   not a finger (two-point-to-one merges, water bridging): that contact is
//!   cancelled and its remaining events dropped.
//! * **Too many contacts**: beyond MAX_CONTACTS the newest are dropped
//!   (a palm, a pocket, a hand holding the phone across the screen).
//!
//! Glove sensitivity is the digitizer's business (firmware "glove mode"),
//! not something software can add; this filter only removes false input.

use crate::{Millis, TouchEvent};
use std::collections::HashMap;

pub const RAIN_TAP_MS: Millis = 120;
pub const RAIN_SLOP: f32 = 8.0;
pub const RAIN_N: usize = 4;
/// Brief touches closer than this to the previous one are the same finger.
pub const RAIN_SPREAD: f32 = 60.0;
pub const RAIN_WINDOW_MS: Millis = 600;
pub const WET_MS: Millis = 2000;
pub const JUMP_PX: f32 = 250.0;
pub const JUMP_MAX_DT: Millis = 30;
pub const MAX_CONTACTS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Contact {
    down: Millis,
    start: (f32, f32),
    last: (f32, f32),
    last_t: Millis,
    rejected: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub dropped_wet: u32,
    pub cancelled_jump: u32,
    pub dropped_excess: u32,
    pub wet_episodes: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TouchFilter {
    /// Cold mode multiplies the slops (a gloved finger wobbles).
    pub slop_scale: f32,
    contacts: HashMap<i32, Contact>,
    /// Recent brief, still touches: (lift time, x, y).
    brief: Vec<(Millis, f32, f32)>,
    wet_until: Option<Millis>,
    pub stats: Stats,
}

impl Default for TouchFilter {
    fn default() -> Self {
        TouchFilter { slop_scale: 1.0, contacts: HashMap::new(), brief: Vec::new(), wet_until: None, stats: Stats::default() }
    }
}

impl TouchFilter {
    pub fn is_wet(&self, now: Millis) -> bool {
        self.wet_until.is_some_and(|t| now < t)
    }

    /// Pass an event through. `Some(ev)` = deliver (possibly turned into a
    /// Cancel for a rejected contact); `None` = drop silently.
    pub fn filter(&mut self, ev: TouchEvent) -> Option<TouchEvent> {
        match ev {
            TouchEvent::Down { slot, x, y, t } => {
                if self.is_wet(t) {
                    self.stats.dropped_wet += 1;
                    return None;
                }
                let live = self.contacts.values().filter(|c| !c.rejected).count();
                let rejected = live >= MAX_CONTACTS;
                if rejected {
                    self.stats.dropped_excess += 1;
                }
                self.contacts.insert(slot, Contact { down: t, start: (x, y), last: (x, y), last_t: t, rejected });
                (!rejected).then_some(ev)
            }
            TouchEvent::Move { slot, x, y, t } => {
                let Some(c) = self.contacts.get_mut(&slot) else { return None };
                if c.rejected {
                    return None;
                }
                let d = (x - c.last.0).hypot(y - c.last.1);
                if d > JUMP_PX && t.saturating_sub(c.last_t) <= JUMP_MAX_DT {
                    c.rejected = true;
                    self.stats.cancelled_jump += 1;
                    return Some(TouchEvent::Cancel);
                }
                c.last = (x, y);
                c.last_t = t;
                Some(ev)
            }
            TouchEvent::Up { slot, t } => {
                let Some(c) = self.contacts.remove(&slot) else { return None };
                if c.rejected {
                    return None;
                }
                let brief = t.saturating_sub(c.down) <= RAIN_TAP_MS && (c.last.0 - c.start.0).hypot(c.last.1 - c.start.1) <= RAIN_SLOP * self.slop_scale;
                if brief {
                    self.brief.retain(|b| t.saturating_sub(b.0) <= RAIN_WINDOW_MS);
                    let same_spot = self.brief.last().is_some_and(|b| (c.start.0 - b.1).hypot(c.start.1 - b.2) < RAIN_SPREAD);
                    if !same_spot {
                        self.brief.push((t, c.start.0, c.start.1));
                    }
                    if self.brief.len() >= RAIN_N {
                        if !self.is_wet(t) {
                            self.stats.wet_episodes += 1;
                        }
                        self.wet_until = Some(t + WET_MS);
                        self.brief.clear();
                        // The touch that tipped it over is itself a phantom:
                        // cancel rather than deliver a click.
                        self.stats.dropped_wet += 1;
                        return Some(TouchEvent::Cancel);
                    }
                }
                Some(ev)
            }
            TouchEvent::Cancel => {
                self.contacts.clear();
                Some(ev)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(f: &mut TouchFilter, slot: i32, x: f32, y: f32, t: Millis, dur: Millis) -> (Option<TouchEvent>, Option<TouchEvent>) {
        (f.filter(TouchEvent::Down { slot, x, y, t }), f.filter(TouchEvent::Up { slot, t: t + dur }))
    }

    #[test]
    fn ordinary_taps_and_drags_pass_untouched() {
        let mut f = TouchFilter::default();
        let (d, u) = tap(&mut f, 0, 100.0, 100.0, 0, 80);
        assert!(d.is_some() && u == Some(TouchEvent::Up { slot: 0, t: 80 }));
        assert!(f.filter(TouchEvent::Down { slot: 0, x: 100.0, y: 100.0, t: 500 }).is_some());
        for i in 1..=20 {
            assert!(f.filter(TouchEvent::Move { slot: 0, x: 100.0 + 15.0 * i as f32, y: 100.0, t: 500 + 16 * i }).is_some());
        }
        assert!(f.filter(TouchEvent::Up { slot: 0, t: 900 }).is_some());
        assert_eq!(f.stats, Stats::default());
        // A real triple tap is three touches: still delivered.
        for i in 0..3 {
            let (d, u) = tap(&mut f, 0, 200.0, 500.0, 2000 + i * 200, 80);
            assert!(d.is_some() && matches!(u, Some(TouchEvent::Up { .. })));
        }
    }

    #[test]
    fn a_burst_of_brief_touches_makes_it_wet_and_new_touches_are_dropped() {
        let mut f = TouchFilter::default();
        let mut last = None;
        for i in 0..4 {
            let (_, u) = tap(&mut f, i, 50.0 + 60.0 * i as f32, 300.0, 100 * i as u64, 40);
            last = u;
        }
        assert_eq!(last, Some(TouchEvent::Cancel), "the fourth phantom is cancelled, not clicked");
        assert!(f.is_wet(500));
        assert_eq!(f.filter(TouchEvent::Down { slot: 9, x: 200.0, y: 200.0, t: 800 }), None, "dropped while wet");
        assert_eq!(f.stats.wet_episodes, 1);
        assert!(f.stats.dropped_wet >= 2);
        assert!(!f.is_wet(500 + WET_MS + 1));
        assert!(f.filter(TouchEvent::Down { slot: 9, x: 200.0, y: 200.0, t: 500 + WET_MS + 1 }).is_some(), "dry again");
    }

    #[test]
    fn hammering_one_button_is_never_rain() {
        let mut f = TouchFilter::default();
        for i in 0..12 {
            let (d, u) = tap(&mut f, 0, 200.0, 700.0, 120 * i as u64, 60);
            assert!(d.is_some() && matches!(u, Some(TouchEvent::Up { .. })), "tap {i} delivered");
        }
        assert!(!f.is_wet(2000) && f.stats.wet_episodes == 0);
    }

    #[test]
    fn slow_taps_do_not_count_as_rain() {
        let mut f = TouchFilter::default();
        for i in 0..6 {
            tap(&mut f, 0, 200.0, 300.0, 300 * i as u64, 80); // spread over 1.8 s
        }
        assert!(!f.is_wet(2000));
        let mut f = TouchFilter::default();
        for i in 0..6 {
            tap(&mut f, 0, 200.0, 300.0, 100 * i as u64, 200); // held too long to be a drop
        }
        assert!(!f.is_wet(700));
    }

    #[test]
    fn a_contact_that_jumps_is_cancelled_and_then_ignored() {
        let mut f = TouchFilter::default();
        f.filter(TouchEvent::Down { slot: 0, x: 100.0, y: 100.0, t: 0 });
        assert!(f.filter(TouchEvent::Move { slot: 0, x: 120.0, y: 100.0, t: 16 }).is_some());
        assert_eq!(f.filter(TouchEvent::Move { slot: 0, x: 400.0, y: 800.0, t: 32 }), Some(TouchEvent::Cancel));
        assert_eq!(f.filter(TouchEvent::Move { slot: 0, x: 405.0, y: 805.0, t: 48 }), None);
        assert_eq!(f.filter(TouchEvent::Up { slot: 0, t: 60 }), None);
        assert_eq!(f.stats.cancelled_jump, 1);
        // A large move over a LONG interval is a fast swipe, not a jump.
        f.filter(TouchEvent::Down { slot: 1, x: 100.0, y: 100.0, t: 1000 });
        // Must come back as the Move itself, not a Cancel (the first version
        // of this assertion accepted either and missed a missing time bound).
        assert_eq!(f.filter(TouchEvent::Move { slot: 1, x: 400.0, y: 100.0, t: 1200 }), Some(TouchEvent::Move { slot: 1, x: 400.0, y: 100.0, t: 1200 }));
    }

    #[test]
    fn too_many_contacts_are_dropped_from_the_newest() {
        let mut f = TouchFilter::default();
        for i in 0..MAX_CONTACTS as i32 {
            assert!(f.filter(TouchEvent::Down { slot: i, x: 50.0 * i as f32, y: 400.0, t: 0 }).is_some());
        }
        assert_eq!(f.filter(TouchEvent::Down { slot: 9, x: 300.0, y: 400.0, t: 5 }), None);
        assert_eq!(f.filter(TouchEvent::Move { slot: 9, x: 310.0, y: 400.0, t: 20 }), None);
        assert_eq!(f.filter(TouchEvent::Up { slot: 9, t: 40 }), None);
        assert_eq!(f.stats.dropped_excess, 1);
        assert!(f.filter(TouchEvent::Up { slot: 0, t: 50 }).is_some(), "the real ones still lift");
    }
}
