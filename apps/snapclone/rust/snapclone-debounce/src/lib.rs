//! snapclone-debounce — debounce/coalesce accumulators for UI transitions.
//!
//! # The problem
//!
//! A swipe-driven page transition currently fans out into three kinds of work
//! on every `pageChanged`:
//!
//!   1. **Visual / animation** — the page slide itself. Must be INSTANT, every
//!      transition. Not debounced. (Driven by the animation system; not this
//!      crate's concern.)
//!   2. **Lightweight UI sync** — e.g. the tab-bar highlight following the
//!      page. Should also feel instant → fires on the LEADING edge.
//!   3. **Consequential / expensive work** — network-priority changes, usage
//!      stats, milestone celebrations, ambient-layer toggling. Firing these for
//!      every page a user blows past during a fast swipe spams the network
//!      layer and burns battery for pages nobody settled on. These should fire
//!      ONCE, on the page the user actually SETTLES on → TRAILING edge,
//!      coalescing the intermediates.
//!
//! The accumulator below is the separation-of-layers that makes this robust: it
//! takes the raw stream of "value changed to X" events and turns it into two
//! clean edges — a leading edge (fire now, for responsiveness) and a trailing
//! "settled" edge (fire once when motion quiets, coalesced to the LATEST value).
//!
//! # Why a separate conceptual layer
//!
//! Each call site previously reinvented debouncing with an ad-hoc restart-the-
//! QTimer trick (see UxGuide). That couples timing policy to each consumer and
//! gets subtly different every time (does it fire leading? does it coalesce to
//! latest or replay? what resets the window?). Centralizing it here makes the
//! "in-flight vs settled" distinction a first-class, tested concept the whole UI
//! stack shares. The C++ side owns NO timing logic beyond "call `tick(now)` from
//! one repeating timer and `push(value, now)` on each transition"; every policy
//! decision lives here under test.
//!
//! # Driving model (no async, no threads)
//!
//! Pure state machine driven by two calls:
//!   * `push(value, now_ms)` — a new transition to `value` happened at `now_ms`.
//!   * `tick(now_ms) -> Vec<Edge<T>>` — called periodically (e.g. a 16–50ms
//!     QTimer); returns any edges that became due. `push` can ALSO return a
//!     leading edge synchronously so the responsive work happens without waiting
//!     for the next tick.
//!
//! All times are caller-supplied monotonic milliseconds (the C++ side passes
//! QElapsedTimer / steady-clock ms), so this crate needs no clock and is fully
//! deterministic under test.

use std::fmt::Debug;

/// An edge the accumulator emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edge<T> {
    /// The first change after a quiet period. Fire responsive/lightweight work
    /// now (tab highlight, etc.). Carries the value that triggered it.
    Leading(T),
    /// Motion has quieted for `quiet_ms`. Fire the consequential/expensive work
    /// ONCE for this — the LATEST — value. Intermediate values the user swept
    /// through are intentionally never reported here.
    Settled(T),
}

/// Configuration for the accumulator.
#[derive(Debug, Clone, Copy)]
pub struct DebounceConfig {
    /// Quiet period (ms) with no new `push` before a `Settled` edge fires.
    pub quiet_ms: u64,
    /// Optional hard cap (ms): if the user keeps swiping without ever pausing
    /// for `quiet_ms`, force a `Settled` edge after this long so the network
    /// layer isn't starved indefinitely during a long continuous swipe. 0 =
    /// no cap (settle strictly on quiet).
    pub max_wait_ms: u64,
    /// Whether to emit a `Leading` edge on the first change after quiet. When
    /// false, only `Settled` edges fire (pure trailing debounce — use this for
    /// consumers with no responsive component, like a pure disk save).
    pub leading: bool,
}

impl DebounceConfig {
    /// Sensible default for page transitions: instant leading edge for the tab
    /// highlight, settle 180ms after the last swipe, and never wait more than
    /// 600ms of continuous swiping before committing the consequential work.
    pub fn page_transition() -> Self {
        Self { quiet_ms: 180, max_wait_ms: 600, leading: true }
    }

    /// Pure trailing debounce (no leading edge) for save-style consumers.
    pub fn trailing_only(quiet_ms: u64) -> Self {
        Self { quiet_ms, max_wait_ms: 0, leading: false }
    }
}

/// Leading+trailing debounce that coalesces to the latest value.
///
/// `T` is the transition payload (e.g. a page index). Cheap to clone.
#[derive(Debug, Clone)]
pub struct CoalescingDebouncer<T: Clone + PartialEq + Debug> {
    cfg: DebounceConfig,
    /// The latest value pushed (the one a `Settled` edge will report).
    pending: Option<T>,
    /// The value most recently reported via a `Settled` edge — used to suppress
    /// a redundant settle when the user swiped away and back to the same page.
    last_settled: Option<T>,
    /// Time of the most recent push (the quiet window counts from here).
    last_push_ms: u64,
    /// Time the current active burst began (for `max_wait_ms`). None when idle.
    burst_start_ms: Option<u64>,
    /// True while we're in an active (un-settled) window — used to know whether
    /// the next push is a leading edge.
    active: bool,
}

impl<T: Clone + PartialEq + Debug> CoalescingDebouncer<T> {
    pub fn new(cfg: DebounceConfig) -> Self {
        Self {
            cfg,
            pending: None,
            last_settled: None,
            last_push_ms: 0,
            burst_start_ms: None,
            active: false,
        }
    }

    /// True if a settle is currently pending (motion in flight).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The latest pending value, if any (what a settle would report).
    pub fn pending(&self) -> Option<&T> {
        self.pending.as_ref()
    }

    /// Record a transition to `value` at `now_ms`. Returns a `Leading` edge if
    /// this push opens a new burst and leading edges are enabled; otherwise
    /// `None` (the value is coalesced into the pending settle).
    pub fn push(&mut self, value: T, now_ms: u64) -> Option<Edge<T>> {
        let opening = !self.active;
        self.pending = Some(value.clone());
        self.last_push_ms = now_ms;
        if opening {
            self.burst_start_ms = Some(now_ms);
            self.active = true;
            if self.cfg.leading {
                return Some(Edge::Leading(value));
            }
        }
        None
    }

    /// Advance time. Returns the edges that became due (at most one `Settled`).
    /// Call from a single repeating timer.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Edge<T>> {
        let mut edges = Vec::new();
        if !self.active {
            return edges;
        }
        let quiet_elapsed = now_ms.saturating_sub(self.last_push_ms) >= self.cfg.quiet_ms;
        let capped = self.cfg.max_wait_ms > 0
            && self
                .burst_start_ms
                .map(|s| now_ms.saturating_sub(s) >= self.cfg.max_wait_ms)
                .unwrap_or(false);

        if quiet_elapsed || capped {
            if let Some(v) = self.pending.take() {
                // Suppress a redundant settle if we'd report the same value we
                // last settled on (user swiped away and back). The leading edge
                // already kept the UI in sync, so there's nothing consequential
                // to redo.
                let redundant = self.last_settled.as_ref() == Some(&v);
                self.last_settled = Some(v.clone());
                if !redundant {
                    edges.push(Edge::Settled(v));
                }
            }
            self.active = false;
            self.burst_start_ms = None;
        }
        edges
    }

    /// Force an immediate settle (e.g. app backgrounding / shutdown): if a value
    /// is pending, return its `Settled` edge now and clear state. Mirrors the
    /// `flush()` on the ad-hoc save debouncers.
    pub fn flush(&mut self) -> Option<Edge<T>> {
        if !self.active {
            return None;
        }
        self.active = false;
        self.burst_start_ms = None;
        let v = self.pending.take()?;
        let redundant = self.last_settled.as_ref() == Some(&v);
        self.last_settled = Some(v.clone());
        if redundant {
            None
        } else {
            Some(Edge::Settled(v))
        }
    }

    /// Reset all state without emitting (e.g. the consumer was torn down).
    pub fn reset(&mut self) {
        self.pending = None;
        self.burst_start_ms = None;
        self.active = false;
        // Keep last_settled so a re-show that lands on the same page doesn't
        // re-fire consequential work. Callers wanting a full reset can drop the
        // whole struct.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_cfg() -> DebounceConfig {
        DebounceConfig { quiet_ms: 100, max_wait_ms: 500, leading: true }
    }

    #[test]
    fn leading_fires_immediately_then_settles_after_quiet() {
        let mut d = CoalescingDebouncer::new(page_cfg());
        // First push → leading edge now.
        assert_eq!(d.push(1, 0), Some(Edge::Leading(1)));
        // Before quiet elapses, no settle.
        assert!(d.tick(50).is_empty());
        // After quiet elapses, settle on 1.
        assert_eq!(d.tick(100), vec![Edge::Settled(1)]);
        // No more edges once settled.
        assert!(d.tick(200).is_empty());
    }

    #[test]
    fn rapid_swipes_coalesce_to_latest() {
        // The headline behavior: blow through pages 1→2→3→4→5 fast; only ONE
        // leading edge (for 1, responsiveness) and ONE settled edge (for 5, the
        // page actually landed on).
        let mut d = CoalescingDebouncer::new(page_cfg());
        assert_eq!(d.push(1, 0), Some(Edge::Leading(1)));
        assert_eq!(d.push(2, 20), None); // coalesced
        assert_eq!(d.push(3, 40), None);
        assert_eq!(d.push(4, 60), None);
        assert_eq!(d.push(5, 80), None);
        // Quiet window counts from the LAST push (80), so nothing at 120.
        assert!(d.tick(120).is_empty());
        // 100ms after the last push → settle on 5, never 2/3/4.
        assert_eq!(d.tick(180), vec![Edge::Settled(5)]);
    }

    #[test]
    fn quiet_window_resets_on_each_push() {
        let mut d = CoalescingDebouncer::new(page_cfg());
        d.push(1, 0);
        d.tick(90); // not yet quiet
        d.push(2, 95); // resets the window
        // 100ms after FIRST push but only 5ms after second → not settled.
        assert!(d.tick(100).is_empty());
        // 100ms after the second push → settle on 2.
        assert_eq!(d.tick(195), vec![Edge::Settled(2)]);
    }

    #[test]
    fn max_wait_forces_settle_during_continuous_swiping() {
        // A user who keeps swiping without ever pausing for quiet_ms must still
        // get a settle by max_wait_ms, so the network layer isn't starved.
        let mut d = CoalescingDebouncer::new(page_cfg());
        d.push(1, 0);
        // Push every 50ms (never quiet for 100ms) up to the 500ms cap.
        let mut t = 50;
        while t < 500 {
            assert!(d.push(t as i32, t).is_none());
            // ticks in between never settle (always within quiet window)
            assert!(d.tick(t + 10).is_empty());
            t += 50;
        }
        // At/after max_wait from burst start (0) → forced settle on the latest.
        let edges = d.tick(500);
        assert_eq!(edges.len(), 1);
        match &edges[0] {
            Edge::Settled(v) => assert_eq!(*v, 450),
            other => panic!("expected settled, got {other:?}"),
        }
    }

    #[test]
    fn second_burst_fires_a_new_leading() {
        let mut d = CoalescingDebouncer::new(page_cfg());
        d.push(1, 0);
        d.tick(100); // settle 1
        // New burst later → fresh leading edge.
        assert_eq!(d.push(2, 1000), Some(Edge::Leading(2)));
        assert_eq!(d.tick(1100), vec![Edge::Settled(2)]);
    }

    #[test]
    fn swipe_away_and_back_suppresses_redundant_settle() {
        // Settle on page 2, then later swipe 2→3→2: the leading edge keeps the
        // UI synced, but the consequential settle for 2 is suppressed because we
        // already did that work — no pointless network re-prioritize.
        let mut d = CoalescingDebouncer::new(page_cfg());
        d.push(2, 0);
        assert_eq!(d.tick(100), vec![Edge::Settled(2)]);
        // Burst back to 2 via 3.
        assert_eq!(d.push(3, 1000), Some(Edge::Leading(3)));
        assert_eq!(d.push(2, 1020), None);
        // Settles to 2 again, but that's where we already are → suppressed.
        assert!(d.tick(1120).is_empty());
    }

    #[test]
    fn trailing_only_config_has_no_leading() {
        let mut d = CoalescingDebouncer::new(DebounceConfig::trailing_only(100));
        assert_eq!(d.push(1, 0), None); // no leading edge
        assert_eq!(d.tick(100), vec![Edge::Settled(1)]);
    }

    #[test]
    fn flush_commits_pending_immediately() {
        let mut d = CoalescingDebouncer::new(page_cfg());
        d.push(7, 0);
        // Shutdown path: flush before quiet elapses.
        assert_eq!(d.flush(), Some(Edge::Settled(7)));
        // Nothing left pending.
        assert!(d.tick(1000).is_empty());
        assert_eq!(d.flush(), None);
    }

    #[test]
    fn flush_suppresses_redundant() {
        let mut d = CoalescingDebouncer::new(page_cfg());
        d.push(1, 0);
        d.tick(100); // settle 1
        d.push(1, 200); // back to same (within a new burst)
        // flush would settle 1 again → suppressed.
        assert_eq!(d.flush(), None);
    }

    #[test]
    fn is_active_and_pending_reflect_state() {
        let mut d = CoalescingDebouncer::new(page_cfg());
        assert!(!d.is_active());
        d.push(3, 0);
        assert!(d.is_active());
        assert_eq!(d.pending(), Some(&3));
        d.tick(100);
        assert!(!d.is_active());
        assert_eq!(d.pending(), None);
    }
}
