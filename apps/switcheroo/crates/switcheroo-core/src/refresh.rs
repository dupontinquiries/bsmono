//! refresh — deciding when a stale row is worth a connection.
//!
//! An advert tells switcheroo that a plug changed. Acting on that is not free: reading a
//! HomeKit accessory means taking the radio away from the scanner, a Curve25519 pair-verify,
//! and a characteristic read — call it a second of exclusive radio per accessory. Doing it
//! the moment an advert arrives means a house with eight plugs reconnects to all eight
//! whenever any one of them moves, and the scanner never runs. That is worse than the
//! polling it replaced, which is why `AccessoryChanged` currently only marks a row stale and
//! waits for the operator.
//!
//! This is the part that decides for them. It is a scheduler, not a policy argument: given
//! which rows are stale, which one the operator is looking at, and what the radio is doing,
//! it answers "read this one now" or "not yet, and here is why".
//!
//! ## The rules, and why each exists
//!
//! * **One at a time.** The radio is exclusive (see [`crate::radio`]). Two refreshes cannot
//!   overlap, so the scheduler picks ONE and the rest wait.
//! * **Never over a session.** A pairing or a user-initiated command outranks a background
//!   read by a lot: a dropped scan costs a stale list, a dropped pairing usually costs the
//!   attempt and a trip to the plug to put it back in pairing mode.
//! * **The visible row goes first.** If the operator is looking at a plug, that is the one
//!   whose staleness they can see. Refreshing the other seven first is work they cannot
//!   observe finishing.
//! * **Settle before reading.** A plug being switched at the wall emits several adverts in
//!   a second or two — someone flicking a light, a dimmer ramping. Reading on the first one
//!   reads a value that is about to change again. [`SETTLE`] is the quiet period a row must
//!   have before it is worth the connection.
//! * **Back off on failure.** An accessory that is out of range, asleep or refusing does
//!   not become reachable by being asked more often, and a scheduler with no backoff turns
//!   one unreachable plug into a permanent radio hog. Doubling, capped.
//! * **A budget.** Even with everything above, a house that is genuinely busy could keep the
//!   radio saturated. [`MAX_PER_MINUTE`] is the ceiling; past it, rows stay stale and the
//!   operator refreshes by hand, which is exactly the behaviour before this existed.
//!
//! Pure: every decision takes the clock as an argument. No timers, no radio, no I/O.

use crate::DeviceId;
use std::time::{Duration, Instant};

/// Quiet period a stale row needs before it is read. A wall switch or a dimmer emits
/// several adverts in a couple of seconds and reading the first one reads a value about to
/// change again.
pub const SETTLE: Duration = Duration::from_millis(1500);

/// First backoff after a failed read, doubled per consecutive failure.
pub const BACKOFF_BASE: Duration = Duration::from_secs(4);

/// Ceiling on the backoff. Past this an accessory is checked occasionally rather than
/// never — a plug that was out of range for an hour should still come back on its own.
pub const BACKOFF_MAX: Duration = Duration::from_secs(300);

/// Most background reads per rolling minute, across all accessories.
pub const MAX_PER_MINUTE: usize = 12;

/// Why nothing is being refreshed right now. Worth naming rather than returning a bare
/// `None`: "the radio is busy" and "everything is up to date" look identical from outside
/// and mean opposite things when something seems stuck.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idle {
    /// Nothing is stale.
    NothingStale,
    /// A pairing or a command holds the radio.
    RadioBusy,
    /// Stale rows exist but none has settled yet.
    Settling,
    /// Stale rows exist but all are in backoff after failures.
    BackingOff,
    /// The per-minute budget is spent.
    BudgetSpent,
    /// A refresh is already running.
    InFlight,
}

/// What the scheduler wants done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Refresh(DeviceId),
    Wait(Idle),
}

/// One stale accessory, as the scheduler sees it.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: DeviceId,
    /// When its most recent advertised change arrived.
    pub changed_at: Instant,
    /// Is the operator looking at this row?
    pub visible: bool,
}

#[derive(Debug, Clone, Copy)]
struct Failures {
    count: u32,
    last: Instant,
}

/// The scheduler's memory: failures, the budget window, and what is in flight.
#[derive(Debug, Default)]
pub struct Refresher {
    failures: std::collections::HashMap<DeviceId, Failures>,
    /// Completion times of recent background reads, for the rolling budget.
    recent: std::collections::VecDeque<Instant>,
    in_flight: Option<DeviceId>,
}

impl Refresher {
    pub fn new() -> Refresher {
        Refresher::default()
    }

    /// Choose what to do now.
    ///
    /// `radio_busy` is true when a session or pairing holds the adapter — the scheduler
    /// never preempts one, so it does not consult [`crate::radio::Radio`] itself; the
    /// caller, which owns it, passes the answer in.
    pub fn decide(&mut self, stale: &[Candidate], radio_busy: bool, now: Instant) -> Decision {
        if self.in_flight.is_some() {
            return Decision::Wait(Idle::InFlight);
        }
        if stale.is_empty() {
            return Decision::Wait(Idle::NothingStale);
        }
        if radio_busy {
            return Decision::Wait(Idle::RadioBusy);
        }
        self.expire_budget(now);
        if self.recent.len() >= MAX_PER_MINUTE {
            return Decision::Wait(Idle::BudgetSpent);
        }

        let mut settling = false;
        let mut backing_off = false;
        let mut best: Option<&Candidate> = None;
        for c in stale {
            if now.duration_since(c.changed_at) < SETTLE {
                settling = true;
                continue;
            }
            if let Some(f) = self.failures.get(&c.id) {
                if now.duration_since(f.last) < backoff_for(f.count) {
                    backing_off = true;
                    continue;
                }
            }
            // The visible row wins; among equals, the one stale longest, so a row cannot be
            // starved by newer changes elsewhere.
            best = match best {
                None => Some(c),
                Some(b) => {
                    let better = (c.visible, b.changed_at) > (b.visible, c.changed_at);
                    if better { Some(c) } else { Some(b) }
                }
            };
        }
        match best {
            Some(c) => {
                self.in_flight = Some(c.id.clone());
                Decision::Refresh(c.id.clone())
            }
            None if settling => Decision::Wait(Idle::Settling),
            None if backing_off => Decision::Wait(Idle::BackingOff),
            None => Decision::Wait(Idle::NothingStale),
        }
    }

    /// The read finished and worked.
    pub fn succeeded(&mut self, id: &DeviceId, now: Instant) {
        self.finish(id, now);
        self.failures.remove(id);
    }

    /// The read failed. The next attempt on this accessory waits longer.
    pub fn failed(&mut self, id: &DeviceId, now: Instant) {
        self.finish(id, now);
        let e = self.failures.entry(id.clone()).or_insert(Failures { count: 0, last: now });
        e.count = e.count.saturating_add(1);
        e.last = now;
    }

    fn finish(&mut self, id: &DeviceId, now: Instant) {
        if self.in_flight.as_ref() == Some(id) {
            self.in_flight = None;
        }
        // A failed attempt still spent the radio, so it counts against the budget. Not
        // counting it would let an unreachable accessory retry without limit — the exact
        // case the budget is for.
        self.recent.push_back(now);
        self.expire_budget(now);
    }

    fn expire_budget(&mut self, now: Instant) {
        while let Some(front) = self.recent.front() {
            if now.duration_since(*front) >= Duration::from_secs(60) {
                self.recent.pop_front();
            } else {
                break;
            }
        }
    }

    /// Reads counted against the current rolling minute.
    pub fn spent(&self) -> usize {
        self.recent.len()
    }

    /// Consecutive failures recorded for an accessory.
    pub fn failure_count(&self, id: &DeviceId) -> u32 {
        self.failures.get(id).map(|f| f.count).unwrap_or(0)
    }

    /// Forget an accessory — unpaired, or gone from the roster.
    pub fn forget(&mut self, id: &DeviceId) {
        self.failures.remove(id);
        if self.in_flight.as_ref() == Some(id) {
            self.in_flight = None;
        }
    }
}

/// Backoff after `n` consecutive failures: doubling from [`BACKOFF_BASE`], capped.
pub fn backoff_for(n: u32) -> Duration {
    if n == 0 {
        return Duration::ZERO;
    }
    let shift = (n - 1).min(16);
    let d = BACKOFF_BASE.saturating_mul(1u32 << shift);
    d.min(BACKOFF_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> DeviceId {
        DeviceId::new(s)
    }

    fn cand(s: &str, changed_at: Instant, visible: bool) -> Candidate {
        Candidate { id: id(s), changed_at, visible }
    }

    /// A plug switched at the wall emits several adverts in a couple of seconds. Reading the
    /// first one reads a value that is about to change again.
    #[test]
    fn a_row_must_settle_before_it_is_worth_a_connection() {
        let mut r = Refresher::new();
        let t0 = Instant::now();
        let c = vec![cand("a", t0, false)];
        assert_eq!(r.decide(&c, false, t0), Decision::Wait(Idle::Settling));
        assert_eq!(r.decide(&c, false, t0 + SETTLE / 2), Decision::Wait(Idle::Settling));
        assert_eq!(r.decide(&c, false, t0 + SETTLE), Decision::Refresh(id("a")));
    }

    /// A pairing outranks a background read by a lot: a dropped scan costs a stale list, a
    /// dropped pairing usually costs the attempt and a trip to the plug.
    #[test]
    fn a_session_is_never_preempted_for_a_background_read() {
        let mut r = Refresher::new();
        let t = Instant::now() + SETTLE * 2;
        let c = vec![cand("a", Instant::now(), true)];
        assert_eq!(r.decide(&c, true, t), Decision::Wait(Idle::RadioBusy));
        // and the moment it is free, the read happens — the wait is not a refusal
        assert_eq!(r.decide(&c, false, t), Decision::Refresh(id("a")));
    }

    /// The row the operator is looking at is the one whose staleness they can see.
    #[test]
    fn the_visible_row_is_read_first_and_nothing_is_starved() {
        let mut r = Refresher::new();
        let t0 = Instant::now();
        let now = t0 + SETTLE * 2;
        let c = vec![
            cand("old", t0, false),
            cand("visible", t0 + Duration::from_millis(100), true),
            cand("newer", t0 + Duration::from_millis(200), false),
        ];
        assert_eq!(r.decide(&c, false, now), Decision::Refresh(id("visible")));
        r.succeeded(&id("visible"), now);
        // Among the rest, the one stale LONGEST — otherwise a row can be starved by newer
        // changes elsewhere.
        let c: Vec<Candidate> = c.into_iter().filter(|x| x.id != id("visible")).collect();
        assert_eq!(r.decide(&c, false, now), Decision::Refresh(id("old")));
    }

    /// One at a time: the radio is exclusive, so a second decision while one is running
    /// must not start another.
    #[test]
    fn only_one_refresh_runs_at_a_time() {
        let mut r = Refresher::new();
        let t0 = Instant::now();
        let now = t0 + SETTLE * 2;
        let c = vec![cand("a", t0, false), cand("b", t0, false)];
        assert!(matches!(r.decide(&c, false, now), Decision::Refresh(_)));
        assert_eq!(r.decide(&c, false, now), Decision::Wait(Idle::InFlight));
        r.succeeded(&id("a"), now);
        assert!(matches!(r.decide(&c, false, now), Decision::Refresh(_)));
    }

    /// An accessory out of range does not become reachable by being asked more often, and a
    /// scheduler with no backoff turns one unreachable plug into a permanent radio hog.
    #[test]
    fn a_failing_accessory_backs_off_and_recovers() {
        let mut r = Refresher::new();
        let t0 = Instant::now();
        let mut now = t0 + SETTLE * 2;
        let c = vec![cand("a", t0, false)];

        assert_eq!(r.decide(&c, false, now), Decision::Refresh(id("a")));
        r.failed(&id("a"), now);
        assert_eq!(r.failure_count(&id("a")), 1);
        // Immediately after: backing off, not retrying.
        assert_eq!(r.decide(&c, false, now), Decision::Wait(Idle::BackingOff));
        now += BACKOFF_BASE;
        assert_eq!(r.decide(&c, false, now), Decision::Refresh(id("a")));
        r.failed(&id("a"), now);
        // Second failure waits twice as long.
        assert_eq!(r.decide(&c, false, now + BACKOFF_BASE), Decision::Wait(Idle::BackingOff));
        assert_eq!(r.decide(&c, false, now + BACKOFF_BASE * 2), Decision::Refresh(id("a")));

        // A success clears it: a plug that came back is not punished for having been away.
        r.succeeded(&id("a"), now);
        assert_eq!(r.failure_count(&id("a")), 0);

        // Doubling, capped — an hour-long absence still gets checked occasionally.
        assert_eq!(backoff_for(0), Duration::ZERO);
        assert_eq!(backoff_for(1), BACKOFF_BASE);
        assert_eq!(backoff_for(2), BACKOFF_BASE * 2);
        assert_eq!(backoff_for(99), BACKOFF_MAX);
    }

    /// Even with everything else, a genuinely busy house could saturate the radio. Past the
    /// budget rows stay stale and the operator refreshes by hand — the behaviour before any
    /// of this existed.
    #[test]
    fn the_budget_caps_a_busy_house_and_refills_over_a_minute() {
        let mut r = Refresher::new();
        let t0 = Instant::now();
        let mut now = t0 + SETTLE * 2;
        let c = vec![cand("a", t0, false)];
        for _ in 0..MAX_PER_MINUTE {
            assert!(matches!(r.decide(&c, false, now), Decision::Refresh(_)));
            r.succeeded(&id("a"), now);
            now += Duration::from_millis(10);
        }
        assert_eq!(r.spent(), MAX_PER_MINUTE);
        assert_eq!(r.decide(&c, false, now), Decision::Wait(Idle::BudgetSpent));
        // The window rolls.
        assert!(matches!(r.decide(&c, false, now + Duration::from_secs(61)), Decision::Refresh(_)));

        // A FAILED read spends budget too — not counting it would let an unreachable
        // accessory retry without limit, which is the case the budget is for.
        let mut r = Refresher::new();
        let now = t0 + SETTLE * 2;
        let _ = r.decide(&c, false, now);
        r.failed(&id("a"), now);
        assert_eq!(r.spent(), 1);
    }

    /// "Nothing is stale" and "the radio is busy" look identical from outside and mean
    /// opposite things when something seems stuck.
    #[test]
    fn every_reason_for_doing_nothing_is_named() {
        let mut r = Refresher::new();
        let now = Instant::now();
        assert_eq!(r.decide(&[], false, now), Decision::Wait(Idle::NothingStale));
        assert_eq!(r.decide(&[], true, now), Decision::Wait(Idle::NothingStale),
                   "an empty list is answered before the radio is consulted");
        // forgetting an in-flight accessory releases the slot, so unpairing mid-read does
        // not wedge the scheduler
        let c = vec![cand("a", now, false)];
        let later = now + SETTLE * 2;
        assert!(matches!(r.decide(&c, false, later), Decision::Refresh(_)));
        r.forget(&id("a"));
        assert_eq!(r.decide(&[], false, later), Decision::Wait(Idle::NothingStale));
    }
}
