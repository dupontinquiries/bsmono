//! idle — tracks `ext-idle-notify-v1` state for however many independent
//! thresholds a shell wants (dim at T1, lock at T2, suspend at T3), the
//! same layered shape `cosmic-idle` uses: one `ext_idle_notification_v1`
//! object per threshold, each firing its own `idled`/`resumed` pair
//! independent of the others.
//!
//! `ext-idle-notify-v1` was confirmed (via `cosmic-idle`'s own source)
//! paired with `wlr-output-power-management-unstable-v1` for the actual
//! screen-off step — that pairing is a `wayland_io` concern, not this
//! module's; this only tracks which named thresholds currently consider
//! the user idle, from whatever named thresholds a caller registers.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleEvent {
    Idled,
    Resumed,
}

/// Named so a caller doesn't have to remember "index 0 is dim, index 1 is
/// lock" — `BTreeMap` for deterministic iteration order in tests and
/// anywhere a caller lists all thresholds (a debug view, say), not for any
/// performance reason with the handful of thresholds a shell actually has.
#[derive(Debug, Default)]
pub struct IdleTracker {
    thresholds: BTreeMap<String, bool>, // name -> is_idle
}

impl IdleTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a threshold as known and not-idle. Idempotent — calling
    /// this again for a name that already exists does not reset it to
    /// not-idle, since `wayland_io` re-registering on reconnect shouldn't
    /// silently un-idle something that was genuinely idle when the
    /// connection dropped.
    pub fn register(&mut self, name: &str) {
        self.thresholds.entry(name.to_string()).or_insert(false);
    }

    /// Applying an event for a name never `register`ed is a silent no-op —
    /// mirrors `ToplevelTracker::apply`'s reasoning for an id with no
    /// prior `New`: a stream from a source `wayland_io` doesn't fully
    /// trust yet shouldn't be able to materialize state this tracker never
    /// agreed to track.
    pub fn apply(&mut self, name: &str, event: IdleEvent) {
        if let Some(is_idle) = self.thresholds.get_mut(name) {
            *is_idle = matches!(event, IdleEvent::Idled);
        }
    }

    pub fn is_idle(&self, name: &str) -> bool {
        self.thresholds.get(name).copied().unwrap_or(false)
    }

    /// Every threshold currently idle, in registration-name order — the
    /// thing a caller deciding "how far along the idle sequence are we"
    /// actually wants, rather than checking each named threshold one at a
    /// time.
    pub fn idle_thresholds(&self) -> Vec<&str> {
        self.thresholds.iter().filter(|(_, &idle)| idle).map(|(name, _)| name.as_str()).collect()
    }

    pub fn any_idle(&self) -> bool {
        self.thresholds.values().any(|&idle| idle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_freshly_registered_threshold_starts_not_idle() {
        let mut t = IdleTracker::new();
        t.register("dim");
        assert!(!t.is_idle("dim"));
    }

    #[test]
    fn idled_then_resumed_round_trips() {
        let mut t = IdleTracker::new();
        t.register("lock");
        t.apply("lock", IdleEvent::Idled);
        assert!(t.is_idle("lock"));
        t.apply("lock", IdleEvent::Resumed);
        assert!(!t.is_idle("lock"));
    }

    #[test]
    fn thresholds_are_independent_of_each_other() {
        let mut t = IdleTracker::new();
        t.register("dim");
        t.register("lock");
        t.apply("dim", IdleEvent::Idled);
        assert!(t.is_idle("dim"));
        assert!(!t.is_idle("lock"), "dim going idle must not affect lock");
    }

    #[test]
    fn an_event_for_an_unregistered_threshold_is_a_silent_no_op() {
        let mut t = IdleTracker::new();
        t.apply("suspend", IdleEvent::Idled); // never registered
        assert!(!t.is_idle("suspend"));
        assert!(t.idle_thresholds().is_empty());
    }

    #[test]
    fn re_registering_does_not_reset_an_already_idle_threshold() {
        let mut t = IdleTracker::new();
        t.register("lock");
        t.apply("lock", IdleEvent::Idled);
        t.register("lock"); // e.g. wayland_io re-binding after a reconnect
        assert!(t.is_idle("lock"), "reconnect bookkeeping must not silently un-idle real state");
    }

    #[test]
    fn idle_thresholds_lists_only_the_idle_ones_in_name_order() {
        let mut t = IdleTracker::new();
        for name in ["suspend", "dim", "lock"] {
            t.register(name);
        }
        t.apply("dim", IdleEvent::Idled);
        t.apply("suspend", IdleEvent::Idled);
        assert_eq!(t.idle_thresholds(), vec!["dim", "suspend"], "alphabetical BTreeMap order, and lock excluded");
    }

    #[test]
    fn any_idle_reflects_the_union_across_thresholds() {
        let mut t = IdleTracker::new();
        t.register("dim");
        t.register("lock");
        assert!(!t.any_idle());
        t.apply("dim", IdleEvent::Idled);
        assert!(t.any_idle());
    }
}
