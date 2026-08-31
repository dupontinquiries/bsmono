//! send_state — what happens to a snap between "Send" and "Delivered".
//!
//! [`crate::composer`] builds one and [`crate::outbox`] decides its lifetime in
//! storage. This is the part between: the row the sender watches, and the rules
//! that move it.
//!
//! ## Delivery cancels expiry, it does not race it
//!
//! The Qt original: *"Cancel 60s expiry — we'll clear after kDeliveredClearMs
//! instead."* A pending row gives up after a minute; a delivered one shows its
//! tick briefly and goes. Those are two different timers and the delivery must
//! **cancel** the first rather than run alongside it — otherwise a snap
//! delivered at 59 seconds shows "✓ Delivered" and then "Failed" a moment
//! later, which is the worst possible pair of messages to show in that order.
//!
//! ## Two prunes at two horizons, and both are deliberate
//!
//! The inbox clears a delivered row after **4 seconds** — it is a notification.
//! The outbox keeps it for **24 hours** — it is a record. Same event, different
//! surfaces, different jobs; collapsing them to one number makes the outbox
//! useless as a record or the inbox a list of ticks.
//!
//! ## Failure is a state, not the absence of success
//!
//! A snap that never arrives has to *say so*. The Qt code notes that *"a true
//! 'retry failed sends' path isn't wired through"* — so this models the state
//! honestly: `Failed` is terminal and retryable **by the user**, and nothing
//! here silently retries. A message the sender believes was delivered and was
//! not is the failure this whole surface exists to prevent.

/// How long a pending send waits before it is called failed.
pub const PENDING_TIMEOUT_MS: u64 = 60_000;
/// How long a delivered tick stays in the inbox notification.
pub const DELIVERED_CLEAR_MS: u64 = 4_000;
/// How long a delivered row stays in the outbox record.
pub const OUTBOX_KEEP_MS: u64 = 24 * 3600 * 1000;

/// Where a send has got to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SendState {
    /// Handed to the transport, no acknowledgement yet.
    Pending,
    /// The peer acknowledged.
    Delivered,
    /// Gave up. Terminal until the user asks again.
    Failed,
}

impl SendState {
    pub fn is_terminal(self) -> bool { self != SendState::Pending }

    /// The label the sender reads.
    pub fn label(self) -> &'static str {
        match self {
            SendState::Pending => "Sending…",
            SendState::Delivered => "✓ Delivered",
            SendState::Failed => "Not delivered",
        }
    }

    /// May the user retry this?
    ///
    /// Only a failure. Offering retry on a pending send invites a double
    /// delivery, and on a delivered one it is a button that lies.
    pub fn can_retry(self) -> bool { self == SendState::Failed }
}

/// One row in the outbox.
#[derive(Clone, Debug, PartialEq)]
pub struct SendRow {
    pub id: String,
    pub state: SendState,
    /// When it was handed to the transport.
    pub sent_at_ms: u64,
    /// When it reached a terminal state, if it has.
    pub settled_at_ms: Option<u64>,
    /// How many times the user has asked again.
    pub attempts: u32,
}

impl SendRow {
    pub fn new(id: &str, now_ms: u64) -> SendRow {
        SendRow { id: id.into(), state: SendState::Pending, sent_at_ms: now_ms,
                  settled_at_ms: None, attempts: 1 }
    }

    /// The peer acknowledged.
    ///
    /// Cancels the pending timeout by settling the row — a later `tick` sees a
    /// terminal state and leaves it alone, so a delivery at 59 seconds cannot
    /// be overwritten by a timeout at 60.
    pub fn delivered(&mut self, now_ms: u64) -> bool {
        if self.state.is_terminal() { return false; }
        self.state = SendState::Delivered;
        self.settled_at_ms = Some(now_ms);
        true
    }

    /// The transport reported a hard failure.
    pub fn failed(&mut self, now_ms: u64) -> bool {
        if self.state.is_terminal() { return false; }
        self.state = SendState::Failed;
        self.settled_at_ms = Some(now_ms);
        true
    }

    /// Time passing. Returns true if this call settled the row.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        if self.state.is_terminal() { return false; }
        if now_ms.saturating_sub(self.sent_at_ms) >= PENDING_TIMEOUT_MS {
            return self.failed(now_ms);
        }
        false
    }

    /// The user asked again. Returns false if there was nothing to retry.
    ///
    /// A retry is a NEW attempt with a fresh clock, not a resumption: the
    /// original send is gone and the timeout must start over, or a retry
    /// inherits an already-expired deadline and fails instantly.
    pub fn retry(&mut self, now_ms: u64) -> bool {
        if !self.state.can_retry() { return false; }
        self.state = SendState::Pending;
        self.sent_at_ms = now_ms;
        self.settled_at_ms = None;
        self.attempts += 1;
        true
    }

    /// Should the inbox notification still show this?
    pub fn in_notification(&self, now_ms: u64) -> bool {
        match (self.state, self.settled_at_ms) {
            (SendState::Pending, _) => true,
            // A failure stays until acknowledged: it is the one the user has to
            // do something about, and clearing it on a timer hides the problem.
            (SendState::Failed, _) => true,
            (SendState::Delivered, Some(t)) =>
                now_ms.saturating_sub(t) < DELIVERED_CLEAR_MS,
            (SendState::Delivered, None) => true,
        }
    }

    /// Should the outbox record still hold this?
    pub fn in_outbox(&self, now_ms: u64) -> bool {
        match (self.state, self.settled_at_ms) {
            (SendState::Delivered, Some(t)) => now_ms.saturating_sub(t) < OUTBOX_KEEP_MS,
            // Pending and failed rows are never pruned by age — a failure that
            // vanished overnight is a message the sender never learns did not
            // arrive.
            _ => true,
        }
    }
}

/// The whole outbox.
#[derive(Clone, Debug, Default)]
pub struct Outbox {
    rows: Vec<SendRow>,
}

impl Outbox {
    pub fn rows(&self) -> &[SendRow] { &self.rows }
    /// Mutable, so a host can mark one Failed. Kept beside `rows` rather
    /// than exposing a `fail(id)` helper because `SendRow` already owns the
    /// transition and its rules (a terminal row does not go back to
    /// Pending); a second path to the same state is a second place for
    /// those rules to be forgotten.
    pub fn rows_mut(&mut self) -> &mut [SendRow] { &mut self.rows }
    pub fn len(&self) -> usize { self.rows.len() }
    pub fn is_empty(&self) -> bool { self.rows.is_empty() }

    pub fn push(&mut self, id: &str, now_ms: u64) {
        self.rows.push(SendRow::new(id, now_ms));
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut SendRow> {
        self.rows.iter_mut().find(|r| r.id == id)
    }

    /// An acknowledgement arrived for `id`.
    ///
    /// An ack for an unknown id is ignored rather than creating a row: it is a
    /// duplicate ack for something already pruned, and inventing a row would
    /// resurrect a delivered snap in the record.
    pub fn ack(&mut self, id: &str, now_ms: u64) -> bool {
        self.get_mut(id).map(|r| r.delivered(now_ms)).unwrap_or(false)
    }

    /// Advance every row and drop what has aged out of the record.
    pub fn tick(&mut self, now_ms: u64) {
        for r in self.rows.iter_mut() { r.tick(now_ms); }
        self.rows.retain(|r| r.in_outbox(now_ms));
    }

    /// Rows the sender needs to act on.
    pub fn needs_attention(&self) -> Vec<&SendRow> {
        self.rows.iter().filter(|r| r.state == SendState::Failed).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delivery_at_the_last_second_cancels_the_timeout() {
        // "Cancel 60s expiry — we'll clear after kDeliveredClearMs instead."
        // Without the cancel, a snap delivered at 59 s shows "✓ Delivered" and
        // then "Not delivered" a moment later — the worst possible pair of
        // messages to show in that order.
        let mut r = SendRow::new("s1", 0);
        assert!(r.delivered(PENDING_TIMEOUT_MS - 1_000));
        r.tick(PENDING_TIMEOUT_MS + 10_000);
        assert_eq!(r.state, SendState::Delivered, "a timeout overwrote a delivery");
    }

    #[test]
    fn a_pending_send_gives_up_after_the_timeout() {
        let mut r = SendRow::new("s1", 0);
        assert!(!r.tick(PENDING_TIMEOUT_MS - 1));
        assert_eq!(r.state, SendState::Pending);
        assert!(r.tick(PENDING_TIMEOUT_MS));
        assert_eq!(r.state, SendState::Failed);
    }

    #[test]
    fn a_late_ack_does_not_revive_a_failed_row() {
        // The sender was already told it did not arrive. Flipping to delivered
        // afterwards means the two things they saw contradict each other, and
        // they cannot tell which was true.
        let mut r = SendRow::new("s1", 0);
        r.tick(PENDING_TIMEOUT_MS);
        assert_eq!(r.state, SendState::Failed);
        assert!(!r.delivered(PENDING_TIMEOUT_MS + 500));
        assert_eq!(r.state, SendState::Failed);
    }

    #[test]
    fn the_two_prunes_are_at_different_horizons_on_purpose() {
        // The inbox clears a tick after 4 s — it is a notification. The outbox
        // keeps it 24 h — it is a record. One number makes the outbox useless
        // as a record or the inbox a list of ticks.
        let mut r = SendRow::new("s1", 0);
        r.delivered(1_000);
        assert!(r.in_notification(1_000 + DELIVERED_CLEAR_MS - 1));
        assert!(!r.in_notification(1_000 + DELIVERED_CLEAR_MS));
        assert!(r.in_outbox(1_000 + DELIVERED_CLEAR_MS), "the record dropped it with the tick");
        assert!(!r.in_outbox(1_000 + OUTBOX_KEEP_MS));
    }

    #[test]
    fn a_failure_is_never_pruned_by_age() {
        // A failure that vanished overnight is a message the sender never
        // learns did not arrive.
        let mut r = SendRow::new("s1", 0);
        r.failed(1_000);
        assert!(r.in_outbox(OUTBOX_KEEP_MS * 10));
        assert!(r.in_notification(OUTBOX_KEEP_MS * 10), "the alert cleared itself");
    }

    #[test]
    fn a_pending_row_survives_a_prune_too() {
        // Slow transports exist; dropping a row still in flight loses the only
        // record that a send was attempted.
        let mut o = Outbox::default();
        o.push("s1", 0);
        o.tick(10);
        assert_eq!(o.len(), 1);
    }

    #[test]
    fn only_a_failure_offers_retry() {
        // Retry on a pending send invites a double delivery; on a delivered one
        // it is a button that lies.
        assert!(!SendState::Pending.can_retry());
        assert!(!SendState::Delivered.can_retry());
        assert!(SendState::Failed.can_retry());
    }

    #[test]
    fn a_retry_starts_a_fresh_clock_rather_than_resuming() {
        // Inheriting the old deadline means the retry fails instantly, which
        // looks like the network is down when it is the timer that is stale.
        let mut r = SendRow::new("s1", 0);
        r.tick(PENDING_TIMEOUT_MS);
        let at = PENDING_TIMEOUT_MS + 5_000;
        assert!(r.retry(at));
        assert_eq!(r.state, SendState::Pending);
        assert_eq!(r.attempts, 2);
        assert!(!r.tick(at + 1), "retried into an already-expired deadline");
        assert!(r.tick(at + PENDING_TIMEOUT_MS));
    }

    #[test]
    fn retrying_something_that_did_not_fail_does_nothing() {
        let mut r = SendRow::new("s1", 0);
        assert!(!r.retry(100));
        assert_eq!(r.attempts, 1);
    }

    #[test]
    fn an_ack_for_an_unknown_id_is_ignored() {
        // A duplicate ack for something already pruned. Creating a row would
        // resurrect a delivered snap in the record.
        let mut o = Outbox::default();
        assert!(!o.ack("gone", 100));
        assert!(o.is_empty());
    }

    #[test]
    fn a_duplicate_ack_settles_once() {
        // Transports retransmit; the second ack must not move the clear timer
        // and keep the tick on screen indefinitely.
        let mut o = Outbox::default();
        o.push("s1", 0);
        assert!(o.ack("s1", 1_000));
        assert!(!o.ack("s1", 5_000), "settled twice");
        let r = &o.rows()[0];
        assert_eq!(r.settled_at_ms, Some(1_000));
    }

    #[test]
    fn failures_are_what_the_sender_is_asked_to_act_on() {
        let mut o = Outbox::default();
        o.push("ok", 0);
        o.push("bad", 0);
        o.ack("ok", 100);
        o.tick(PENDING_TIMEOUT_MS);
        let need = o.needs_attention();
        assert_eq!(need.len(), 1);
        assert_eq!(need[0].id, "bad");
    }

    #[test]
    fn a_backwards_clock_does_not_fail_everything_in_flight() {
        // Saturating: an NTP correction mid-send would otherwise make every
        // pending row look a minute old at once.
        let mut r = SendRow::new("s1", 100_000);
        assert!(!r.tick(50_000));
        assert_eq!(r.state, SendState::Pending);
    }

    #[test]
    fn every_state_has_a_label_the_sender_can_read() {
        // "Failed" alone reads as a bug in the app; "Not delivered" is about
        // the message, which is what the sender actually needs to know.
        for s in [SendState::Pending, SendState::Delivered, SendState::Failed] {
            assert!(!s.label().is_empty());
        }
        assert!(SendState::Failed.label().contains("Not delivered"));
    }
}
