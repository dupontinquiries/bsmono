//! delivery.rs — per-member delivery / seen state for group entries we sent.
//!
//! The sender-key crypto and the DAG say *what* was sent and in what causal
//! order; they say nothing about *who has actually received or opened it*. This
//! module is the local bookkeeping that turns inbound delivery / seen receipts
//! into the summary the UI shows beside an outbound group snap — "delivered to
//! 2, seen by 1 of 3" — closing the "no per-member delivery indicator in
//! groups" gap from the three-party-meetup scenario analysis (gap D).
//!
//! It is deliberately pure state-tracking — no crypto, no I/O — so it is
//! exhaustively unit-tested here. The transport feeds it (`record_delivered`
//! on an ACK, `record_seen` on an open/view receipt); a thin FFI to expose
//! `summary()` to the C++ inbox UI is a later step, matching this crate's
//! "prove the logic first, wire it after" convention.
//!
//! Invariants:
//!   * Keyed by the DAG [`NodeId`] (content-addressed entry id) — stable/unique.
//!   * Recipients are SNAPSHOTTED at send time, so "k of N" reflects the roster
//!     the entry was actually sent to. A later add/remove never rewrites the
//!     delivery history of an already-sent entry (history is immutable).
//!   * Per-member state is MONOTONIC: `Pending → Delivered → Seen`. A receipt
//!     never moves a member backward, and `Seen` implies `Delivered` (an open
//!     receipt can arrive when the delivery receipt was lost or raced).
//!   * IDEMPOTENT: duplicate receipts are no-ops; a receipt from someone who
//!     was not a recipient of that entry is ignored (you can't be "seen by" a
//!     non-recipient). Self is never tracked as a recipient.

use crate::dag::NodeId;
use std::collections::BTreeMap;

/// How far a single member has progressed for one entry. Ordered so a simple
/// `>` comparison enforces the monotonic upgrade rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemberDelivery {
    Pending,
    Delivered,
    Seen,
}

/// Aggregate counts for one entry, for the "k of N" badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliverySummary {
    /// Members the entry was sent to (excludes self).
    pub recipients: usize,
    /// Members who have received it (Delivered or Seen).
    pub delivered: usize,
    /// Members who have opened it (Seen).
    pub seen: usize,
}

impl DeliverySummary {
    /// True once every recipient has opened it (and there was at least one).
    pub fn all_seen(&self) -> bool {
        self.recipients > 0 && self.seen == self.recipients
    }
    /// True once every recipient has received it.
    pub fn all_delivered(&self) -> bool {
        self.recipients > 0 && self.delivered == self.recipients
    }
}

/// Tracks delivery/seen state for the entries this device sent to a group.
#[derive(Debug, Default)]
pub struct DeliveryTracker {
    /// entry id → (member sc_id → state). BTreeMap for deterministic iteration.
    entries: BTreeMap<NodeId, BTreeMap<String, MemberDelivery>>,
}

impl DeliveryTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an entry we sent to `recipients` (member sc_ids). `self_id` is
    /// excluded defensively (you never receive a receipt from yourself).
    ///
    /// Idempotent on the entry id: calling again merges any *new* recipients
    /// (e.g. a re-send to a larger roster) but never downgrades a member who
    /// has already progressed.
    pub fn record_sent<I, S>(&mut self, entry: &NodeId, recipients: I, self_id: &str)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let map = self.entries.entry(entry.clone()).or_default();
        for r in recipients {
            let id = r.as_ref();
            if id == self_id {
                continue;
            }
            map.entry(id.to_owned()).or_insert(MemberDelivery::Pending);
        }
    }

    /// A delivery receipt arrived from `member` for `entry`.
    pub fn record_delivered(&mut self, entry: &NodeId, member: &str) {
        self.bump(entry, member, MemberDelivery::Delivered);
    }

    /// A seen/opened receipt arrived from `member` for `entry` (implies delivered).
    pub fn record_seen(&mut self, entry: &NodeId, member: &str) {
        self.bump(entry, member, MemberDelivery::Seen);
    }

    fn bump(&mut self, entry: &NodeId, member: &str, to: MemberDelivery) {
        if let Some(map) = self.entries.get_mut(entry) {
            if let Some(state) = map.get_mut(member) {
                if to > *state {
                    *state = to; // monotonic upgrade only
                }
            }
            // member not in the recipient set → ignore (can't be seen by a non-recipient)
        }
        // unknown entry → ignore (receipt for something we didn't track)
    }

    /// Aggregate counts for the UI badge, or `None` if the entry isn't tracked.
    pub fn summary(&self, entry: &NodeId) -> Option<DeliverySummary> {
        let map = self.entries.get(entry)?;
        let recipients = map.len();
        let mut delivered = 0;
        let mut seen = 0;
        for st in map.values() {
            match st {
                MemberDelivery::Pending => {}
                MemberDelivery::Delivered => delivered += 1,
                MemberDelivery::Seen => {
                    delivered += 1;
                    seen += 1;
                }
            }
        }
        Some(DeliverySummary {
            recipients,
            delivered,
            seen,
        })
    }

    /// Per-member state for an entry (for a detailed receipt list), sorted by
    /// sc_id. `None` if the entry isn't tracked.
    pub fn members(&self, entry: &NodeId) -> Option<Vec<(String, MemberDelivery)>> {
        self.entries
            .get(entry)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect())
    }

    /// Members who have not yet reached `at_least` — e.g. pass `Delivered` to
    /// find who to nudge with a resend, or `Seen` for "still hasn't opened it".
    pub fn pending_members(&self, entry: &NodeId, at_least: MemberDelivery) -> Vec<String> {
        self.entries
            .get(entry)
            .map(|m| {
                m.iter()
                    .filter(|(_, v)| **v < at_least)
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stop tracking an entry (it aged out / was deleted).
    pub fn forget(&mut self, entry: &NodeId) {
        self.entries.remove(entry);
    }

    /// Optional: scrub a departed member from EVERY tracked entry, so a person
    /// who left stops counting against historical "k of N" badges (turning
    /// "2 of 3" into "2 of 2"). Off by default — history is immutable unless a
    /// caller explicitly opts into this when handling a remove.
    pub fn forget_member_everywhere(&mut self, member: &str) {
        for map in self.entries.values_mut() {
            map.remove(member);
        }
    }

    /// Number of entries currently tracked (for bounding / diagnostics).
    pub fn tracked_len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> NodeId {
        s.to_owned()
    }

    #[test]
    fn sent_snapshots_recipients_excluding_self() {
        let mut t = DeliveryTracker::new();
        t.record_sent(&id("e1"), ["me", "alice", "bob"], "me");
        let s = t.summary(&id("e1")).unwrap();
        assert_eq!(s.recipients, 2); // alice, bob — not me
        assert_eq!(s.delivered, 0);
        assert_eq!(s.seen, 0);
    }

    #[test]
    fn delivered_then_seen_counts_correctly() {
        let mut t = DeliveryTracker::new();
        t.record_sent(&id("e1"), ["alice", "bob", "carol"], "me");
        t.record_delivered(&id("e1"), "alice");
        t.record_delivered(&id("e1"), "bob");
        t.record_seen(&id("e1"), "alice");
        let s = t.summary(&id("e1")).unwrap();
        assert_eq!(s.recipients, 3);
        assert_eq!(s.delivered, 2); // alice (seen⇒delivered) + bob
        assert_eq!(s.seen, 1); // alice
        assert!(!s.all_delivered());
        assert!(!s.all_seen());
    }

    #[test]
    fn seen_implies_delivered_even_without_delivery_receipt() {
        let mut t = DeliveryTracker::new();
        t.record_sent(&id("e1"), ["alice"], "me");
        // open receipt arrives but the delivery receipt was lost/raced
        t.record_seen(&id("e1"), "alice");
        let s = t.summary(&id("e1")).unwrap();
        assert_eq!(s.delivered, 1);
        assert_eq!(s.seen, 1);
        assert!(s.all_seen());
        assert!(s.all_delivered());
    }

    #[test]
    fn state_is_monotonic_never_downgrades() {
        let mut t = DeliveryTracker::new();
        t.record_sent(&id("e1"), ["alice"], "me");
        t.record_seen(&id("e1"), "alice"); // Seen
        t.record_delivered(&id("e1"), "alice"); // must NOT downgrade to Delivered
        assert_eq!(t.members(&id("e1")).unwrap(), vec![("alice".to_owned(), MemberDelivery::Seen)]);
    }

    #[test]
    fn receipts_are_idempotent() {
        let mut t = DeliveryTracker::new();
        t.record_sent(&id("e1"), ["alice"], "me");
        t.record_delivered(&id("e1"), "alice");
        t.record_delivered(&id("e1"), "alice");
        t.record_delivered(&id("e1"), "alice");
        assert_eq!(t.summary(&id("e1")).unwrap().delivered, 1);
    }

    #[test]
    fn non_recipient_receipt_is_ignored() {
        let mut t = DeliveryTracker::new();
        t.record_sent(&id("e1"), ["alice", "bob"], "me");
        t.record_seen(&id("e1"), "mallory"); // never a recipient
        let s = t.summary(&id("e1")).unwrap();
        assert_eq!(s.recipients, 2);
        assert_eq!(s.seen, 0);
    }

    #[test]
    fn unknown_entry_yields_none_and_receipts_noop() {
        let mut t = DeliveryTracker::new();
        assert!(t.summary(&id("ghost")).is_none());
        t.record_delivered(&id("ghost"), "alice"); // must not panic or create state
        assert!(t.summary(&id("ghost")).is_none());
        assert_eq!(t.tracked_len(), 0);
    }

    #[test]
    fn pending_members_for_nudge() {
        let mut t = DeliveryTracker::new();
        t.record_sent(&id("e1"), ["alice", "bob", "carol"], "me");
        t.record_delivered(&id("e1"), "alice");
        t.record_seen(&id("e1"), "bob");
        // who hasn't received yet:
        let mut not_delivered = t.pending_members(&id("e1"), MemberDelivery::Delivered);
        not_delivered.sort();
        assert_eq!(not_delivered, vec!["carol".to_owned()]);
        // who hasn't opened yet (delivered-but-not-seen + never-received):
        let mut not_seen = t.pending_members(&id("e1"), MemberDelivery::Seen);
        not_seen.sort();
        assert_eq!(not_seen, vec!["alice".to_owned(), "carol".to_owned()]);
    }

    #[test]
    fn resend_to_larger_roster_merges_without_downgrade() {
        let mut t = DeliveryTracker::new();
        t.record_sent(&id("e1"), ["alice"], "me");
        t.record_seen(&id("e1"), "alice");
        // later the same entry is (re)sent including a new member
        t.record_sent(&id("e1"), ["alice", "dave"], "me");
        let s = t.summary(&id("e1")).unwrap();
        assert_eq!(s.recipients, 2); // alice + dave
        assert_eq!(s.seen, 1); // alice still seen, not reset
        assert_eq!(
            t.pending_members(&id("e1"), MemberDelivery::Delivered),
            vec!["dave".to_owned()]
        );
    }

    #[test]
    fn forget_member_everywhere_is_opt_in() {
        let mut t = DeliveryTracker::new();
        t.record_sent(&id("e1"), ["alice", "bob", "carol"], "me");
        t.record_seen(&id("e1"), "alice");
        t.record_seen(&id("e1"), "bob");
        // carol leaves; caller opts into scrubbing her so the badge reads "2 of 2"
        t.forget_member_everywhere("carol");
        let s = t.summary(&id("e1")).unwrap();
        assert_eq!(s.recipients, 2);
        assert!(s.all_seen());
    }

    #[test]
    fn forget_drops_entry() {
        let mut t = DeliveryTracker::new();
        t.record_sent(&id("e1"), ["alice"], "me");
        t.forget(&id("e1"));
        assert!(t.summary(&id("e1")).is_none());
        assert_eq!(t.tracked_len(), 0);
    }
}
