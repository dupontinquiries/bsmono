//! debounce — leading+trailing debounce that coalesces to the latest value.
//!
//! Ported from `CoalescingDebouncer`, whose header states the shape: a Leading
//! edge on the first change after quiet, a Settled edge once `quietMs` has
//! passed with no further change, and a `maxWaitMs` cap so continuous motion
//! still produces a Settled edge rather than starving forever.
//!
//! ## Why leading AND trailing
//!
//! A page transition should feel instant, so the *first* swipe fires
//! immediately. But the expensive work — loading the page's content — should
//! only happen once you have stopped, so it waits for quiet. A pure trailing
//! debounce makes the UI feel laggy; a pure leading one does the expensive work
//! on every swipe of a fast scroll.
//!
//! ## Why the max-wait cap exists
//!
//! Trailing debounce alone **starves under continuous input**: keep swiping and
//! the Settled edge never fires, so the content never loads and the user is
//! looking at a stale page while actively using it. The cap converts "when you
//! stop" into "when you stop, or 600 ms, whichever comes first".
//!
//! ## Coalescing is the third property
//!
//! Intermediate values are discarded, not queued. A burst of ten swipes should
//! load the tenth page once — not ten pages in sequence, which is both slow and
//! wrong, since nine of them were never looked at.

/// What kind of edge fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// First change after a quiet period. Fires immediately.
    Leading,
    /// The value has settled, or the max wait elapsed.
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Quiet period before a Settled edge fires.
    pub quiet_ms: u64,
    /// Hard cap during continuous motion. 0 disables it.
    pub max_wait_ms: u64,
    /// Emit a Leading edge on the first change after quiet.
    pub leading: bool,
}

impl Config {
    /// The QML's page-transition default: instant leading edge, settle 180 ms
    /// after the last swipe, never wait more than 600 ms of continuous swiping.
    pub fn page_transition() -> Config {
        Config { quiet_ms: 180, max_wait_ms: 600, leading: true }
    }
    /// Pure trailing, for save-style consumers where an early write is wasted.
    pub fn save() -> Config {
        Config { quiet_ms: 180, max_wait_ms: 600, leading: false }
    }
}

/// A debouncer over `T`.
#[derive(Debug, Clone)]
pub struct Debouncer<T> {
    cfg: Config,
    /// The latest value seen. Intermediates are overwritten, not queued.
    pending: Option<T>,
    last_change_ms: u64,
    /// When the current burst began, for the max-wait cap.
    burst_started_ms: Option<u64>,
}

impl<T: Clone + PartialEq> Debouncer<T> {
    pub fn new(cfg: Config) -> Debouncer<T> {
        Debouncer { cfg, pending: None, last_change_ms: 0, burst_started_ms: None }
    }

    /// Record a transition. Returns a Leading edge if this opens a new burst.
    pub fn change(&mut self, value: T, now_ms: u64) -> Option<(Edge, T)> {
        let new_burst = self.burst_started_ms.is_none();
        self.last_change_ms = now_ms;
        // Coalesce: the latest value replaces whatever was waiting.
        self.pending = Some(value.clone());
        if new_burst {
            self.burst_started_ms = Some(now_ms);
            if self.cfg.leading { return Some((Edge::Leading, value)); }
        }
        None
    }

    /// Advance time. Returns a Settled edge if one became due.
    ///
    /// At most one Settled edge per burst: firing repeatedly while idle would
    /// make every consumer re-do work for a value that has not changed.
    pub fn tick(&mut self, now_ms: u64) -> Option<(Edge, T)> {
        let started = self.burst_started_ms?;
        let value = self.pending.clone()?;
        let quiet = now_ms.saturating_sub(self.last_change_ms) >= self.cfg.quiet_ms;
        let capped = self.cfg.max_wait_ms > 0
            && now_ms.saturating_sub(started) >= self.cfg.max_wait_ms;
        if quiet || capped {
            self.pending = None;
            self.burst_started_ms = None;
            return Some((Edge::Settled, value));
        }
        None
    }

    /// Is a burst in progress?
    pub fn pending(&self) -> Option<&T> { self.pending.as_ref() }

    /// Drop everything without emitting — for a view being torn down mid-burst.
    pub fn cancel(&mut self) {
        self.pending = None;
        self.burst_started_ms = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> Debouncer<u32> { Debouncer::new(Config::page_transition()) }

    #[test]
    fn the_first_change_fires_immediately() {
        // A page transition should feel instant; a pure trailing debounce makes
        // the UI feel laggy.
        let mut d = page();
        assert_eq!(d.change(1, 0), Some((Edge::Leading, 1)));
    }

    #[test]
    fn a_burst_coalesces_to_the_latest_value() {
        // Ten swipes should load the TENTH page once, not ten pages in
        // sequence — nine of which were never looked at.
        let mut d = page();
        d.change(1, 0);
        for (i, t) in (2u32..=10).zip((20..).step_by(20)) {
            assert_eq!(d.change(i, t), None, "intermediates are coalesced");
        }
        let settled = d.tick(1000);
        assert_eq!(settled, Some((Edge::Settled, 10)), "only the last value");
    }

    #[test]
    fn continuous_motion_still_settles_at_the_cap() {
        // Trailing debounce alone STARVES: keep swiping and the content never
        // loads, so the user works against a stale page.
        let mut d = page();
        d.change(1, 0);
        let mut now = 0u64;
        let mut settled = None;
        // change every 100ms — never quiet, so only the 600ms cap can fire
        for i in 2u32..=12 {
            now += 100;
            d.change(i, now);
            if let Some(e) = d.tick(now) { settled = Some(e); break; }
        }
        let (edge, _) = settled.expect("the cap must fire under continuous input");
        assert_eq!(edge, Edge::Settled);
        assert!(now >= 600, "fired at {now}ms, not before the cap");
    }

    #[test]
    fn quiet_settles_before_the_cap() {
        let mut d = page();
        d.change(1, 0);
        assert_eq!(d.tick(100), None, "not quiet yet");
        assert_eq!(d.tick(180), Some((Edge::Settled, 1)));
    }

    #[test]
    fn only_one_settled_edge_per_burst() {
        // Firing repeatedly while idle makes every consumer re-do work for a
        // value that has not changed.
        let mut d = page();
        d.change(1, 0);
        assert!(d.tick(500).is_some());
        assert_eq!(d.tick(1000), None);
        assert_eq!(d.tick(5000), None);
    }

    #[test]
    fn a_new_burst_after_quiet_leads_again() {
        // The leading edge is per burst, not per lifetime.
        let mut d = page();
        d.change(1, 0);
        d.tick(500);
        assert_eq!(d.change(2, 600), Some((Edge::Leading, 2)));
    }

    #[test]
    fn a_save_style_consumer_gets_no_leading_edge() {
        // An early write is wasted work and can persist a value the user was
        // still in the middle of choosing.
        let mut d: Debouncer<u32> = Debouncer::new(Config::save());
        assert_eq!(d.change(1, 0), None);
        assert_eq!(d.tick(200), Some((Edge::Settled, 1)));
    }

    #[test]
    fn a_zero_cap_means_no_cap() {
        // Documented as "0 = none". A consumer that genuinely wants pure quiet
        // must not get a surprise edge at some default.
        let mut d: Debouncer<u32> =
            Debouncer::new(Config { quiet_ms: 100, max_wait_ms: 0, leading: true });
        d.change(1, 0);
        for t in (50..5000).step_by(50) {
            d.change(2, t);
            assert_eq!(d.tick(t), None, "never settles under continuous change");
        }
    }

    #[test]
    fn cancelling_mid_burst_emits_nothing() {
        // A view torn down while swiping must not load a page into a dead
        // widget.
        let mut d = page();
        d.change(1, 0);
        d.cancel();
        assert_eq!(d.tick(5000), None);
        assert!(d.pending().is_none());
    }

    #[test]
    fn ticking_with_nothing_pending_is_harmless() {
        let mut d: Debouncer<u32> = page();
        assert_eq!(d.tick(1000), None);
    }
}
