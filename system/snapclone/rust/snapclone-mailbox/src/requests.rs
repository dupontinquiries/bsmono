//! Connection-request approval lifecycle — the UI-facing state the unified
//! mailbox displays and the snapclone-menu `MailboxEntry` actions act on.
//!
//! This is DISTINCT from the rest of this crate, which models the DHT *transfer*
//! rendezvous (intent → deferred data transfer). This module models the human
//! decision layer on top: when a stranger's connection request arrives, it sits
//! as `Pending` in the mailbox until the user Accepts (→ becomes a contact),
//! Rejects (→ dismissed, but they can ask again), or Blocks (→ dismissed AND
//! future requests from them are auto-dropped).
//!
//! Why a tested model and not just Qt state: the legal transitions and their
//! invariants (you can't accept a blocked sender; re-requesting after a reject
//! is allowed but after a block is silently dropped; an already-accepted request
//! is idempotent) are exactly the kind of rules that rot into subtle bugs when
//! scattered across UI slots. Here they're one state machine with tests, and the
//! Qt side just renders `entries()` and calls `apply(id, action)`.
//!
//! The menu model (snapclone-menu) reads an entry's [`RequestState`] to choose
//! which actions to show; this module is the authority for what that state IS
//! and how it changes.

use std::collections::BTreeMap;

/// Where an inbound connection request sits in the approval lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestState {
    /// Awaiting the user's decision — shows Accept / Reject / Block.
    Pending,
    /// Accepted — the sender is now a contact. Terminal for this request
    /// (the conversation takes over from here).
    Accepted,
    /// Rejected — dismissed from the mailbox, but the sender MAY send a fresh
    /// request later (which re-enters as Pending).
    Rejected,
    /// Blocked — dismissed, and future requests from this sender are dropped
    /// before they ever reach Pending.
    Blocked,
}

/// The user action applied to a pending request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestAction {
    Accept,
    Reject,
    Block,
}

/// One inbound request in the mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundRequest {
    /// Stable sender id (e.g. "sc-c1dd601b").
    pub sender_id: String,
    /// Display label at request time (may be just the id for a stranger).
    pub display_name: String,
    /// Monotonic arrival timestamp (ms). Used for ordering; the model doesn't
    /// interpret it beyond sorting newest-first.
    pub received_ms: i64,
    pub state: RequestState,
}

/// The outcome of applying an action — lets the caller react (e.g. create a
/// contact on Accept) without re-deriving what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// State changed to the given new state; caller should act on it
    /// (Accept → add contact; Block → also add to blocklist).
    Changed(RequestState),
    /// No-op: the action didn't apply (e.g. accepting an already-accepted
    /// request, or any action on a request id we don't have).
    NoChange,
    /// The action was refused because it's illegal for the current state
    /// (e.g. Accept on a Blocked sender). Caller may surface a message.
    Refused,
}

/// The mailbox's request-approval state. Keyed by sender id so a re-request
/// from the same sender updates in place rather than stacking duplicates.
#[derive(Debug, Default, Clone)]
pub struct RequestMailbox {
    entries: BTreeMap<String, InboundRequest>,
    /// Senders the user has blocked. A request from a blocked sender is dropped
    /// on arrival and never becomes Pending.
    blocked: BTreeMap<String, ()>,
}

impl RequestMailbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an inbound request. Returns true if it was admitted as Pending,
    /// false if it was dropped (blocked sender) or merged into an existing
    /// non-terminal entry.
    ///
    /// Rules:
    ///   * Blocked sender → dropped, returns false (never shown).
    ///   * New sender → admitted Pending, returns true.
    ///   * Sender with a prior Rejected/Accepted entry → a fresh request
    ///     re-opens it as Pending (people can re-ask, or re-connect after a
    ///     conversation lapsed), returns true.
    ///   * Sender already Pending → updates the timestamp/name in place,
    ///     returns false (not a NEW pending; avoids duplicate rows).
    pub fn receive(&mut self, sender_id: &str, display_name: &str, received_ms: i64) -> bool {
        if self.blocked.contains_key(sender_id) {
            return false;
        }
        if let Some(existing) = self.entries.get_mut(sender_id) {
            if existing.state == RequestState::Pending {
                // Already pending — refresh metadata, don't duplicate.
                existing.received_ms = received_ms;
                existing.display_name = display_name.to_string();
                return false;
            }
            // Rejected/Accepted → re-open as a fresh pending request.
            existing.state = RequestState::Pending;
            existing.received_ms = received_ms;
            existing.display_name = display_name.to_string();
            return true;
        }
        self.entries.insert(
            sender_id.to_string(),
            InboundRequest {
                sender_id: sender_id.to_string(),
                display_name: display_name.to_string(),
                received_ms,
                state: RequestState::Pending,
            },
        );
        true
    }

    /// Apply a user action to a request by sender id.
    pub fn apply(&mut self, sender_id: &str, action: RequestAction) -> ApplyOutcome {
        let Some(entry) = self.entries.get_mut(sender_id) else {
            return ApplyOutcome::NoChange;
        };
        match (entry.state, action) {
            // Accept/Reject/Block only act on a Pending request.
            (RequestState::Pending, RequestAction::Accept) => {
                entry.state = RequestState::Accepted;
                ApplyOutcome::Changed(RequestState::Accepted)
            }
            (RequestState::Pending, RequestAction::Reject) => {
                entry.state = RequestState::Rejected;
                ApplyOutcome::Changed(RequestState::Rejected)
            }
            (RequestState::Pending, RequestAction::Block) => {
                entry.state = RequestState::Blocked;
                self.blocked.insert(sender_id.to_string(), ());
                ApplyOutcome::Changed(RequestState::Blocked)
            }
            // Acting on an already-resolved request: accepting/rejecting an
            // already-accepted one is a harmless no-op; blocking a sender who
            // was accepted/rejected is a legitimate later decision.
            (RequestState::Accepted, RequestAction::Block)
            | (RequestState::Rejected, RequestAction::Block) => {
                entry.state = RequestState::Blocked;
                self.blocked.insert(sender_id.to_string(), ());
                ApplyOutcome::Changed(RequestState::Blocked)
            }
            // Accepting a blocked sender is refused — they must be unblocked
            // first (an explicit, separate user action).
            (RequestState::Blocked, RequestAction::Accept) => ApplyOutcome::Refused,
            // Everything else (re-rejecting a rejected, re-accepting an
            // accepted, rejecting a blocked) is a no-op.
            _ => ApplyOutcome::NoChange,
        }
    }

    /// Unblock a sender — removes them from the blocklist so future requests
    /// can arrive again. Does NOT resurrect the old request (it stays Blocked
    /// in history until a fresh request re-opens it). Returns true if the
    /// sender was actually blocked.
    pub fn unblock(&mut self, sender_id: &str) -> bool {
        self.blocked.remove(sender_id).is_some()
    }

    pub fn is_blocked(&self, sender_id: &str) -> bool {
        self.blocked.contains_key(sender_id)
    }

    /// All requests currently awaiting a decision, newest-first — exactly the
    /// rows the unified mailbox shows with Accept/Reject/Block.
    pub fn pending(&self) -> Vec<&InboundRequest> {
        let mut v: Vec<&InboundRequest> = self
            .entries
            .values()
            .filter(|e| e.state == RequestState::Pending)
            .collect();
        v.sort_by(|a, b| b.received_ms.cmp(&a.received_ms));
        v
    }

    /// Count of pending requests — for the mailbox badge.
    pub fn pending_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.state == RequestState::Pending)
            .count()
    }

    /// Look up a single request's current state (the menu model reads this to
    /// choose actions).
    pub fn state_of(&self, sender_id: &str) -> Option<RequestState> {
        self.entries.get(sender_id).map(|e| e.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mb() -> RequestMailbox {
        RequestMailbox::new()
    }

    #[test]
    fn new_request_is_pending_and_shown() {
        let mut m = mb();
        assert!(m.receive("sc-aaa", "Alice", 100));
        assert_eq!(m.state_of("sc-aaa"), Some(RequestState::Pending));
        assert_eq!(m.pending_count(), 1);
        assert_eq!(m.pending()[0].sender_id, "sc-aaa");
    }

    #[test]
    fn accept_makes_accepted_and_drops_from_pending() {
        let mut m = mb();
        m.receive("sc-aaa", "Alice", 100);
        assert_eq!(m.apply("sc-aaa", RequestAction::Accept),
                   ApplyOutcome::Changed(RequestState::Accepted));
        assert_eq!(m.pending_count(), 0);
        assert_eq!(m.state_of("sc-aaa"), Some(RequestState::Accepted));
    }

    #[test]
    fn reject_allows_re_request() {
        let mut m = mb();
        m.receive("sc-aaa", "Alice", 100);
        m.apply("sc-aaa", RequestAction::Reject);
        assert_eq!(m.state_of("sc-aaa"), Some(RequestState::Rejected));
        assert_eq!(m.pending_count(), 0);
        // A fresh request re-opens as pending.
        assert!(m.receive("sc-aaa", "Alice", 200));
        assert_eq!(m.state_of("sc-aaa"), Some(RequestState::Pending));
        assert_eq!(m.pending_count(), 1);
    }

    #[test]
    fn block_drops_future_requests() {
        let mut m = mb();
        m.receive("sc-bad", "Spammer", 100);
        assert_eq!(m.apply("sc-bad", RequestAction::Block),
                   ApplyOutcome::Changed(RequestState::Blocked));
        assert!(m.is_blocked("sc-bad"));
        assert_eq!(m.pending_count(), 0);
        // A new request from the blocked sender is dropped — never pending.
        assert!(!m.receive("sc-bad", "Spammer", 200));
        assert_eq!(m.pending_count(), 0);
        assert_eq!(m.state_of("sc-bad"), Some(RequestState::Blocked));
    }

    #[test]
    fn accept_on_blocked_is_refused() {
        let mut m = mb();
        m.receive("sc-bad", "Spammer", 100);
        m.apply("sc-bad", RequestAction::Block);
        assert_eq!(m.apply("sc-bad", RequestAction::Accept), ApplyOutcome::Refused);
        assert_eq!(m.state_of("sc-bad"), Some(RequestState::Blocked));
    }

    #[test]
    fn unblock_lets_requests_through_again() {
        let mut m = mb();
        m.receive("sc-x", "X", 100);
        m.apply("sc-x", RequestAction::Block);
        assert!(m.unblock("sc-x"));
        assert!(!m.is_blocked("sc-x"));
        // Now a fresh request is admitted again.
        assert!(m.receive("sc-x", "X", 300));
        assert_eq!(m.state_of("sc-x"), Some(RequestState::Pending));
    }

    #[test]
    fn duplicate_pending_does_not_stack() {
        let mut m = mb();
        assert!(m.receive("sc-aaa", "Alice", 100));
        // Same sender re-sends while still pending — refreshes, doesn't stack.
        assert!(!m.receive("sc-aaa", "Alice (new name)", 150));
        assert_eq!(m.pending_count(), 1);
        assert_eq!(m.pending()[0].display_name, "Alice (new name)");
        assert_eq!(m.pending()[0].received_ms, 150);
    }

    #[test]
    fn pending_sorted_newest_first() {
        let mut m = mb();
        m.receive("sc-old", "Old", 100);
        m.receive("sc-new", "New", 300);
        m.receive("sc-mid", "Mid", 200);
        let p = m.pending();
        assert_eq!(p[0].sender_id, "sc-new");
        assert_eq!(p[1].sender_id, "sc-mid");
        assert_eq!(p[2].sender_id, "sc-old");
    }

    #[test]
    fn block_after_accept_is_allowed() {
        // You accepted someone, later decide to block them.
        let mut m = mb();
        m.receive("sc-aaa", "Alice", 100);
        m.apply("sc-aaa", RequestAction::Accept);
        assert_eq!(m.apply("sc-aaa", RequestAction::Block),
                   ApplyOutcome::Changed(RequestState::Blocked));
        assert!(m.is_blocked("sc-aaa"));
    }

    #[test]
    fn action_on_unknown_sender_is_noop() {
        let mut m = mb();
        assert_eq!(m.apply("sc-ghost", RequestAction::Accept), ApplyOutcome::NoChange);
    }

    #[test]
    fn re_accepting_is_idempotent_noop() {
        let mut m = mb();
        m.receive("sc-aaa", "Alice", 100);
        m.apply("sc-aaa", RequestAction::Accept);
        assert_eq!(m.apply("sc-aaa", RequestAction::Accept), ApplyOutcome::NoChange);
        assert_eq!(m.state_of("sc-aaa"), Some(RequestState::Accepted));
    }
}
