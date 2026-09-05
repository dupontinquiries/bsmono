//! radio — who owns the Bluetooth adapter right now.
//!
//! switcheroo has two BLE stacks in one process. The device scanner uses
//! `btleplug`; `hap-controller` (the published HomeKit stack this app now
//! pairs through) uses `bluest`. On Linux both are BlueZ over D-Bus, and
//! BlueZ permits several clients — but a discovery session held by one
//! client while another tries to connect and run a multi-round pairing is
//! how you get a pairing that times out on a plug sitting a metre away.
//!
//! On a phone this is not a theoretical worry. A Fairphone 5 (Bluetooth
//! 5.2 LE, QCM6490) or Fairphone 6 (5.4 LE) has ONE radio, and the scanner
//! is a continuous duty cycle by design — it is how the device list stays
//! live. Pairing has to be able to take the radio and give it back.
//!
//! This is the pure part: a lease. It does not touch a radio; it decides
//! who may. The transport asks before it scans and asks before it pairs,
//! so the rule lives in one testable place rather than in two callbacks
//! that each assume they are alone.
//!
//! # Why a lease and not a mutex
//!
//! A mutex would block the UI thread on a pairing that can legitimately
//! run for thirty seconds. The lease is advisory and non-blocking: asking
//! for the radio while pairing holds it returns [`Grant::Denied`] with the
//! reason, and the caller shows that reason instead of freezing.

/// What holds the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    /// Nobody. Scanning may start.
    Free,
    /// The device scanner's discovery session.
    Scan,
    /// A pairing or control exchange — a HAP session over the plug's GATT.
    /// Outranks the scanner: a dropped scan costs a stale list for a few
    /// seconds, a dropped pairing costs the whole attempt and the plug
    /// usually has to be put back into pairing mode by hand.
    Session,
}

/// The answer to "may I use the radio".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    /// Take it. Carries what was interrupted, if anything, so the caller
    /// can restore it afterwards rather than guessing.
    Granted { preempted: Option<Holder> },
    /// Someone with a stronger claim has it.
    Denied { held_by: Holder, reason: &'static str },
}

impl Grant {
    pub fn is_granted(&self) -> bool { matches!(self, Grant::Granted { .. }) }
}

/// The adapter's current owner. One per adapter; switcheroo has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Radio {
    holder: Holder,
}

impl Default for Holder {
    fn default() -> Self { Holder::Free }
}

impl Radio {
    pub fn new() -> Radio { Radio::default() }
    pub fn holder(&self) -> Holder { self.holder }
    pub fn is_free(&self) -> bool { self.holder == Holder::Free }

    /// Ask for the radio as `who`.
    ///
    /// A session takes it from a scan — that is the whole point, and the
    /// preempted holder comes back in the grant so the caller knows to
    /// restart the scan when it releases. A scan never takes it from a
    /// session; it is told to wait.
    pub fn acquire(&mut self, who: Holder) -> Grant {
        match (self.holder, who) {
            (_, Holder::Free) => Grant::Denied {
                held_by: self.holder,
                reason: "Free is not something to acquire",
            },
            (Holder::Free, w) => {
                self.holder = w;
                Grant::Granted { preempted: None }
            }
            // Re-entrant: asking for what you already hold is not a failure,
            // and treating it as one makes every caller track its own state.
            (h, w) if h == w => Grant::Granted { preempted: None },
            (Holder::Scan, Holder::Session) => {
                self.holder = Holder::Session;
                Grant::Granted { preempted: Some(Holder::Scan) }
            }
            (Holder::Session, Holder::Scan) => Grant::Denied {
                held_by: Holder::Session,
                reason: "pairing is using the radio — the device list will resume when it finishes",
            },
            (h, _) => Grant::Denied { held_by: h, reason: "the radio is busy" },
        }
    }

    /// Give it back. Releasing something you do not hold is ignored rather
    /// than panicking: a task that lost a race should not take the process
    /// with it, and the holder is the authority either way.
    pub fn release(&mut self, who: Holder) {
        if self.holder == who {
            self.holder = Holder::Free;
        }
    }

    /// What the UI should say about the scanner right now.
    pub fn scan_note(&self) -> Option<&'static str> {
        match self.holder {
            Holder::Session => Some("paused while pairing uses the radio"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_takes_the_radio_from_the_scanner_and_says_what_it_took() {
        let mut r = Radio::new();
        assert!(r.acquire(Holder::Scan).is_granted());
        // The grant names what it interrupted, so the caller knows to put the
        // scan back rather than having to remember it did one.
        assert_eq!(r.acquire(Holder::Session),
                   Grant::Granted { preempted: Some(Holder::Scan) });
        assert_eq!(r.holder(), Holder::Session);
    }

    #[test]
    fn the_scanner_never_takes_it_back_mid_pairing() {
        let mut r = Radio::new();
        r.acquire(Holder::Session);
        match r.acquire(Holder::Scan) {
            Grant::Denied { held_by, reason } => {
                assert_eq!(held_by, Holder::Session);
                assert!(reason.contains("pairing"), "{reason}");
            }
            other => panic!("a scan must not preempt a pairing: {other:?}"),
        }
        assert_eq!(r.holder(), Holder::Session, "the denial changed nothing");
        assert_eq!(r.scan_note(), Some("paused while pairing uses the radio"));
    }

    #[test]
    fn asking_for_what_you_already_hold_is_not_a_failure() {
        // Otherwise every caller has to track whether it already asked, and
        // one of them will get it wrong.
        let mut r = Radio::new();
        r.acquire(Holder::Scan);
        assert!(r.acquire(Holder::Scan).is_granted());
        assert_eq!(r.holder(), Holder::Scan);
    }

    #[test]
    fn releasing_frees_it_and_releasing_what_you_do_not_hold_is_ignored() {
        let mut r = Radio::new();
        r.acquire(Holder::Session);
        // A scanner task that lost the race releasing on its way out must not
        // hand the radio away from the session that preempted it.
        r.release(Holder::Scan);
        assert_eq!(r.holder(), Holder::Session, "a stale release cannot free another holder");
        r.release(Holder::Session);
        assert!(r.is_free());
        r.release(Holder::Session); // twice is fine
        assert!(r.is_free());
    }

    #[test]
    fn a_full_pair_then_resume_cycle_ends_where_it_started() {
        let mut r = Radio::new();
        r.acquire(Holder::Scan);
        let Grant::Granted { preempted } = r.acquire(Holder::Session) else { panic!() };
        r.release(Holder::Session);
        assert!(r.is_free());
        // The caller restores what it interrupted.
        if let Some(p) = preempted {
            assert!(r.acquire(p).is_granted());
        }
        assert_eq!(r.holder(), Holder::Scan);
        assert_eq!(r.scan_note(), None);
    }

    #[test]
    fn free_is_a_state_not_a_request() {
        let mut r = Radio::new();
        assert!(!r.acquire(Holder::Free).is_granted());
        assert!(r.is_free(), "a nonsense request changes nothing");
    }
}
