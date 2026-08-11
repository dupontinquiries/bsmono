//! peer_session — the per-peer state `SnapRouter` keeps, and the rule for
//! forgetting it.
//!
//! [`crate::wire_v3`] defines the frame; this holds what a sender must remember
//! *per peer* to build one: sequence, acknowledgement, negotiated version, and
//! what that peer already has.
//!
//! ## The predictive rule, with its trade stated
//!
//! *"If we have no record of what this peer knows, send data immediately.
//! Better to spend one extra transmission than to wait for a round trip."*
//!
//! That is the right way round on a phone. An extra transmission costs bytes on
//! a radio that is already awake; a round trip costs a **second wakeup**, and
//! the sleeping radio has to come back up for it. The bytes are cheaper than
//! the wakeup.
//!
//! ## Forgetting a peer must forget all of it
//!
//! `clearPeerState` clears **seven** maps, one of them marked "Bug 5 fix" and
//! another added later with its own note — *"otherwise it lingers for every
//! ever-connected peer"*.
//!
//! That shape is the warning. Per-peer state spread across parallel maps means
//! forgetting is a checklist, and a checklist grows a hole every time somebody
//! adds a map. Here it is **one struct in one map**, so removing the peer
//! removes everything by construction and no future field can be missed.

use std::collections::{HashMap, HashSet};

/// Everything remembered about one peer.
#[derive(Clone, Debug, Default)]
pub struct PeerSession {
    /// Next outbound sequence number.
    seq: u64,
    /// Highest sequence received from them, echoed as our ack.
    ack_recv: u64,
    /// Wire version they negotiated.
    version: u8,
    /// Content hashes they have acknowledged holding.
    known: HashSet<String>,
    /// An ack we owe but have not yet piggybacked.
    pending_ack: bool,
    /// When we last answered a SYNC, to dedup storms.
    last_sync_reply_ms: Option<u64>,
}

impl PeerSession {
    pub fn version(&self) -> u8 { self.version }
    pub fn is_v3(&self) -> bool { self.version >= 3 }
    pub fn pending_ack(&self) -> bool { self.pending_ack }
    pub fn known_count(&self) -> usize { self.known.len() }

    /// Record a negotiated version.
    ///
    /// Never downgrades: a peer that spoke v3 once does not become a v2 peer
    /// because a later frame omitted the field, and treating it as one would
    /// send it a full payload where a delta was expected.
    pub fn set_version(&mut self, v: u8) {
        if v > self.version { self.version = v; }
    }

    /// Claim the next sequence number.
    pub fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// A frame arrived carrying this sequence.
    ///
    /// Only advances: reordered frames are normal on UDP, and letting a late
    /// low sequence move the ack backwards would make the peer re-send
    /// everything after it.
    pub fn observe_seq(&mut self, their_seq: u64) {
        if their_seq > self.ack_recv {
            self.ack_recv = their_seq;
            self.pending_ack = true;
        }
    }

    /// The ack to put in the next outbound frame, clearing the debt.
    pub fn take_ack(&mut self) -> u64 {
        self.pending_ack = false;
        self.ack_recv
    }

    /// The peer told us they hold this content.
    pub fn learn(&mut self, hash: &str) {
        if self.known.len() >= crate::snap_expiry::MAX_KNOWN_HASHES {
            self.known.clear();
        }
        self.known.insert(hash.to_string());
    }

    /// Should the payload go out with the announcement?
    ///
    /// True when we have no record at all — the predictive case — and true when
    /// we have a record that does not include this hash. Only a positive
    /// "they already have it" suppresses the payload.
    pub fn should_send_data(&self, hash: &str) -> bool {
        if self.known.is_empty() { return true; }
        !self.known.contains(hash)
    }

    /// May we answer a SYNC, or is this a storm?
    pub fn may_reply_sync(&self, now_ms: u64, min_gap_ms: u64) -> bool {
        match self.last_sync_reply_ms {
            None => true,
            Some(t) => now_ms.saturating_sub(t) >= min_gap_ms,
        }
    }

    pub fn note_sync_reply(&mut self, now_ms: u64) {
        self.last_sync_reply_ms = Some(now_ms);
    }
}

/// Minimum gap between SYNC replies to one peer.
pub const SYNC_REPLY_GAP_MS: u64 = 2_000;

/// All peer sessions.
///
/// One map, so `forget` cannot develop a hole.
#[derive(Clone, Debug, Default)]
pub struct Sessions {
    peers: HashMap<String, PeerSession>,
}

impl Sessions {
    pub fn len(&self) -> usize { self.peers.len() }
    pub fn is_empty(&self) -> bool { self.peers.is_empty() }

    /// Get or create a peer's session.
    pub fn entry(&mut self, peer: &str) -> &mut PeerSession {
        self.peers.entry(peer.to_string()).or_default()
    }

    pub fn get(&self, peer: &str) -> Option<&PeerSession> { self.peers.get(peer) }

    /// Forget a peer entirely.
    ///
    /// One removal, not seven. The C++ version clears a list of parallel maps
    /// and has already grown two late additions — a "Bug 5 fix" and a leak note
    /// about state that "lingers for every ever-connected peer". A checklist
    /// grows a hole every time somebody adds a map; a struct cannot.
    pub fn forget(&mut self, peer: &str) -> bool {
        self.peers.remove(peer).is_some()
    }

    /// Peers that have negotiated v3.
    pub fn v3_peers(&self) -> Vec<&String> {
        self.peers.iter().filter(|(_, s)| s.is_v3()).map(|(k, _)| k).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_peer_gets_the_payload_immediately() {
        // "Better to spend one extra transmission than to wait for a round
        // trip." On a phone the bytes go out on a radio already awake; a round
        // trip costs a SECOND WAKEUP.
        let s = PeerSession::default();
        assert!(s.should_send_data("abc"));
    }

    #[test]
    fn a_peer_known_to_hold_the_content_does_not_get_it_again() {
        let mut s = PeerSession::default();
        s.learn("abc");
        assert!(!s.should_send_data("abc"));
        assert!(s.should_send_data("def"), "a different hash still sends");
    }

    #[test]
    fn sequences_start_at_one_and_never_repeat() {
        // A repeated sequence is a frame the peer treats as a duplicate and
        // silently drops.
        let mut s = PeerSession::default();
        assert_eq!(s.next_seq(), 1);
        assert_eq!(s.next_seq(), 2);
        assert_eq!(s.next_seq(), 3);
    }

    #[test]
    fn a_reordered_frame_does_not_move_the_ack_backwards() {
        // Reordering is normal on UDP, and a late low sequence moving the ack
        // back would make the peer re-send everything after it.
        let mut s = PeerSession::default();
        s.observe_seq(10);
        s.observe_seq(4);
        assert_eq!(s.take_ack(), 10);
    }

    #[test]
    fn an_ack_debt_is_raised_and_cleared() {
        // The piggyback rule from wire_v3: no standalone acks, so the debt has
        // to be carried until a frame goes out.
        let mut s = PeerSession::default();
        assert!(!s.pending_ack());
        s.observe_seq(1);
        assert!(s.pending_ack());
        s.take_ack();
        assert!(!s.pending_ack());
    }

    #[test]
    fn a_duplicate_frame_raises_no_new_debt() {
        // Otherwise a retransmitting peer keeps us sending acks forever.
        let mut s = PeerSession::default();
        s.observe_seq(5);
        s.take_ack();
        s.observe_seq(5);
        assert!(!s.pending_ack());
    }

    #[test]
    fn a_version_never_downgrades() {
        // A peer that spoke v3 once does not become a v2 peer because a later
        // frame omitted the field, and treating it as one would send a full
        // payload where a delta was expected.
        let mut s = PeerSession::default();
        s.set_version(3);
        s.set_version(2);
        assert_eq!(s.version(), 3);
        assert!(s.is_v3());
    }

    #[test]
    fn forgetting_a_peer_forgets_everything_at_once() {
        // The C++ clearPeerState clears SEVEN parallel maps and has already
        // grown two late additions — a "Bug 5 fix" and a leak note about state
        // that "lingers for every ever-connected peer". One struct in one map
        // cannot develop that hole.
        let mut ss = Sessions::default();
        {
            let s = ss.entry("peer-a");
            s.set_version(3);
            s.next_seq();
            s.observe_seq(9);
            s.learn("abc");
            s.note_sync_reply(1_000);
        }
        assert!(ss.forget("peer-a"));
        assert!(ss.get("peer-a").is_none());
        // and a reconnect starts genuinely clean
        let s = ss.entry("peer-a");
        assert_eq!(s.version(), 0);
        assert_eq!(s.known_count(), 0);
        assert!(!s.pending_ack());
        assert!(s.should_send_data("abc"), "stale dedup survived a disconnect");
        assert!(s.may_reply_sync(1_001, SYNC_REPLY_GAP_MS), "a stale sync gate survived");
    }

    #[test]
    fn forgetting_an_unknown_peer_is_not_an_error() {
        // Disconnect can fire twice; the second must be a no-op rather than a
        // surprise.
        let mut ss = Sessions::default();
        assert!(!ss.forget("never-seen"));
    }

    #[test]
    fn sync_replies_are_rate_limited_per_peer() {
        // A peer resending SYNC in a loop would otherwise have us answer every
        // one, which on a phone is the storm the gap exists to stop.
        let mut s = PeerSession::default();
        assert!(s.may_reply_sync(0, SYNC_REPLY_GAP_MS));
        s.note_sync_reply(0);
        assert!(!s.may_reply_sync(SYNC_REPLY_GAP_MS - 1, SYNC_REPLY_GAP_MS));
        assert!(s.may_reply_sync(SYNC_REPLY_GAP_MS, SYNC_REPLY_GAP_MS));
    }

    #[test]
    fn the_known_set_is_bounded_by_the_shared_cap() {
        // Same constant as snap_expiry, so two places cannot disagree about how
        // much dedup history a session keeps.
        let mut s = PeerSession::default();
        for i in 0..crate::snap_expiry::MAX_KNOWN_HASHES * 2 {
            s.learn(&format!("h{i}"));
        }
        assert!(s.known_count() <= crate::snap_expiry::MAX_KNOWN_HASHES);
    }

    #[test]
    fn sessions_are_independent() {
        // One peer's dedup history must never suppress a send to another.
        let mut ss = Sessions::default();
        ss.entry("a").learn("shared-hash");
        assert!(ss.entry("b").should_send_data("shared-hash"));
    }

    #[test]
    fn v3_peers_are_listed_for_the_broadcast_path() {
        let mut ss = Sessions::default();
        ss.entry("old").set_version(2);
        ss.entry("new").set_version(3);
        let v3 = ss.v3_peers();
        assert_eq!(v3.len(), 1);
        assert_eq!(v3[0], "new");
    }
}
