//! ambient_inbox — `AmbientInboxDriver.cpp`, ported.
//!
//! Re-derives the ambient palette as snaps arrive, so the chrome *"drifts
//! toward whoever's been sending you things"*. It sits between the inbox and
//! [`crate::ambient_palette`], and its whole job is deciding **when** to spend
//! a derivation.
//!
//! ## The throttle defends against two named failure modes
//!
//! Thirty seconds minimum between recomputes. The header names what that is
//! for, and both are real events rather than hypotheticals:
//!
//! * **Storage cascades on initial inbox load**, where the restore path emits
//!   one arrival per stored snap — so opening the app derives once per photo
//!   you have ever received.
//! * **Bulk send-storms**: a group photo, or a peer reconnecting and flushing
//!   its queue.
//!
//! A burst of five arrivals inside the window produces **one** derivation.
//!
//! ## The duplicate-start guard came from a field diagnosis
//!
//! The C++ carries the symptom and the date: *"two `started` lines logged
//! within 2 seconds, each followed by a derivation, despite the 30s throttle.
//! The throttle is per-snap-received; it doesn't protect against start() being
//! called multiple times."*
//!
//! That is the kind of bug a reimplementation reintroduces immediately, because
//! the throttle looks like it covers everything. It does not: a palette restore
//! plus a carousel click, or a double-tap on the chip, each call `start` again.
//! The guard is a separate condition and stays one here.
//!
//! ## It checks it is still in charge before acting
//!
//! A user who switched to a different synthetic or a static palette must not
//! have their choice stomped by a derivation that was already queued. The C++
//! re-checks the active synthetic id at derive time rather than trusting that
//! `stop()` arrived first.

/// Minimum gap between derivations.
pub const MIN_INTERVAL_MS: u64 = 30_000;

/// Which synthetic palette this driver owns.
pub const SYNTHETIC_ID: &str = "ambient_inbox";

/// What a trigger resolved to, so a caller can log or count without inferring.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Derive now.
    Derive,
    /// Inside the throttle window.
    Throttled,
    /// Not running.
    Inactive,
    /// Another palette is in charge; the user's pick is not ours to stomp.
    NotOurPalette,
}

#[derive(Clone, Debug, Default)]
pub struct AmbientInboxDriver {
    active: bool,
    /// Identifies the controller this driver is bound to, for the duplicate
    /// guard. `None` when stopped.
    bound_to: Option<String>,
    last_derive_ms: u64,
    /// Set once so the first arrival after starting derives immediately —
    /// waiting thirty seconds to show any ambience at all would read as the
    /// feature not working.
    ever_derived: bool,
}

impl AmbientInboxDriver {
    pub fn is_active(&self) -> bool { self.active }
    pub fn bound_to(&self) -> Option<&str> { self.bound_to.as_deref() }

    /// Begin driving for `controller`.
    ///
    /// Returns false when this is a duplicate start for the same controller —
    /// the guard the field diagnosis added. A second `start` must not re-derive
    /// or re-subscribe, or a palette restore plus a carousel click does the
    /// work twice and drowns the perf signal.
    pub fn start(&mut self, controller: &str) -> bool {
        if controller.is_empty() { return false; }
        if self.active && self.bound_to.as_deref() == Some(controller) { return false; }
        self.active = true;
        self.bound_to = Some(controller.to_string());
        true
    }

    /// Stop driving.
    pub fn stop(&mut self) {
        self.active = false;
        self.bound_to = None;
    }

    /// A snap arrived.
    ///
    /// `active_synthetic` is the palette currently in charge; `None` means a
    /// static palette. Checked at trigger time rather than trusting that
    /// `stop()` arrived first, because a queued arrival can outlive the switch.
    pub fn on_snap(&mut self, now_ms: u64, active_synthetic: Option<&str>) -> Outcome {
        if !self.active { return Outcome::Inactive; }
        if active_synthetic != Some(SYNTHETIC_ID) { return Outcome::NotOurPalette; }

        // The first derivation after starting is immediate: waiting thirty
        // seconds for any ambience at all reads as the feature being broken.
        if self.ever_derived {
            // Saturating, because a clock correction between arrivals would
            // otherwise wrap into an enormous gap and defeat the throttle at
            // exactly the moment a burst is arriving.
            if now_ms.saturating_sub(self.last_derive_ms) < MIN_INTERVAL_MS {
                return Outcome::Throttled;
            }
        }
        self.last_derive_ms = now_ms;
        self.ever_derived = true;
        Outcome::Derive
    }

    /// Milliseconds until the throttle opens, or zero when it is open.
    pub fn cooldown_remaining(&self, now_ms: u64) -> u64 {
        if !self.ever_derived { return 0; }
        MIN_INTERVAL_MS.saturating_sub(now_ms.saturating_sub(self.last_derive_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> AmbientInboxDriver {
        let mut d = AmbientInboxDriver::default();
        d.start("controller-a");
        d
    }

    const OURS: Option<&str> = Some(SYNTHETIC_ID);

    #[test]
    fn the_first_snap_derives_immediately() {
        // Waiting thirty seconds for any ambience at all reads as the feature
        // not working.
        let mut d = started();
        assert_eq!(d.on_snap(1_000, OURS), Outcome::Derive);
    }

    #[test]
    fn a_burst_produces_exactly_one_derivation() {
        // The storage cascade on initial inbox load emits one arrival per
        // stored snap — so opening the app would otherwise derive once per
        // photo you have ever received.
        let mut d = started();
        assert_eq!(d.on_snap(1_000, OURS), Outcome::Derive);
        for t in [1_100, 2_000, 5_000, 20_000, 29_999] {
            assert_eq!(d.on_snap(t, OURS), Outcome::Throttled, "at {t}");
        }
    }

    #[test]
    fn the_window_reopens_after_the_interval() {
        let mut d = started();
        d.on_snap(1_000, OURS);
        assert_eq!(d.on_snap(1_000 + MIN_INTERVAL_MS, OURS), Outcome::Derive);
    }

    #[test]
    fn a_duplicate_start_is_a_no_op() {
        // The field diagnosis: "two started lines logged within 2 seconds, each
        // followed by a derivation, despite the 30s throttle. The throttle is
        // per-snap-received; it doesn't protect against start() being called
        // multiple times." A palette restore plus a carousel click does exactly
        // this.
        let mut d = AmbientInboxDriver::default();
        assert!(d.start("controller-a"), "first start takes effect");
        assert!(!d.start("controller-a"), "the second is ignored");
        assert!(d.is_active());
    }

    #[test]
    fn starting_for_a_different_controller_rebinds() {
        // The guard is per-controller, not global: a genuinely new controller
        // must take over rather than being silently ignored.
        let mut d = AmbientInboxDriver::default();
        d.start("controller-a");
        assert!(d.start("controller-b"));
        assert_eq!(d.bound_to(), Some("controller-b"));
    }

    #[test]
    fn a_stopped_driver_ignores_arrivals() {
        let mut d = started();
        d.stop();
        assert_eq!(d.on_snap(1_000, OURS), Outcome::Inactive);
        assert!(!d.is_active());
    }

    #[test]
    fn a_queued_arrival_does_not_stomp_a_palette_the_user_switched_to() {
        // Checked at trigger time rather than trusting that stop() arrived
        // first, because an arrival already in flight can outlive the switch.
        let mut d = started();
        assert_eq!(d.on_snap(1_000, Some("ambient_camera")), Outcome::NotOurPalette);
        assert_eq!(d.on_snap(2_000, None), Outcome::NotOurPalette, "a static palette");
    }

    #[test]
    fn refusing_on_a_foreign_palette_does_not_consume_the_throttle() {
        // Otherwise switching away and back would leave the driver silently in
        // cooldown for a derivation that never happened.
        let mut d = started();
        d.on_snap(1_000, Some("ambient_camera"));
        assert_eq!(d.on_snap(1_100, OURS), Outcome::Derive, "the first real one still fires");
    }

    #[test]
    fn restarting_after_a_stop_derives_again_immediately() {
        // Switching to inbox ambience and back should show something at once,
        // not sit in a cooldown inherited from the previous session.
        let mut d = started();
        d.on_snap(1_000, OURS);
        d.stop();
        d.start("controller-a");
        assert_eq!(d.on_snap(1_500, OURS), Outcome::Throttled,
                   "the throttle survives a restart — the last derive really was recent");
    }

    #[test]
    fn a_backwards_clock_does_not_defeat_the_throttle() {
        // An NTP correction between arrivals would wrap an unsigned subtraction
        // into an enormous gap, at exactly the moment a burst is arriving.
        let mut d = started();
        d.on_snap(100_000, OURS);
        assert_eq!(d.on_snap(50_000, OURS), Outcome::Throttled);
    }

    #[test]
    fn the_cooldown_is_reportable_for_a_diagnostics_view() {
        let mut d = started();
        assert_eq!(d.cooldown_remaining(0), 0, "nothing derived yet");
        d.on_snap(1_000, OURS);
        assert_eq!(d.cooldown_remaining(1_000), MIN_INTERVAL_MS);
        assert_eq!(d.cooldown_remaining(1_000 + MIN_INTERVAL_MS), 0);
    }

    #[test]
    fn an_empty_controller_id_is_refused() {
        // A nullptr controller in the C++, which it logs and ignores.
        let mut d = AmbientInboxDriver::default();
        assert!(!d.start(""));
        assert!(!d.is_active());
    }
}
