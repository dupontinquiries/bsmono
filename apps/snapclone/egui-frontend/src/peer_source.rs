//! peer_source — a roster backed by real sessions, not a list of names.
//!
//! snapclone's `Peer` was `{ name: String, online: bool }` — a label and a
//! guess. `shared/peermux-core` is the connection broker: **one** authenticated
//! X25519+Ed25519 handshake per peer, with streams handed out per service, and
//! it has no stubs.
//!
//! ## What "online" actually means, and why one bool cannot say it
//!
//! There are three states a roster has to distinguish and a bool collapses to
//! two:
//!
//! * **Known** — we have a peer id from a previous session, but no live session
//!   now. Messages queue; they are not lost.
//! * **Connected** — a session exists. Anything sent goes now.
//! * **Reachable but unauthenticated** — seen on the network, no handshake yet.
//!   This is the one a bool hides, and it is the one where the UI must not offer
//!   to send: the message would sit in a queue against a peer whose identity has
//!   never been verified.
//!
//! ## A note on what is NOT used here
//!
//! `shared/snapcore`'s proximity layer (Bluetooth, AdHocWifi, Acoustic) looks
//! like the obvious peer-discovery source and every `discover()` is
//! `unimplemented!()`. Wiring a roster to it would compile and then panic on
//! first use, which is worse than the hardcoded list it replaced. Recorded here
//! so the next person checking that crate does not repeat the walk.

use peermux_core::{Broker, PeerId};

/// How reachable a peer is right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Known from a previous session; nothing live.
    Known,
    /// Seen on the network, never handshaken. NOT sendable.
    Unverified,
    /// A session exists.
    Connected,
}

impl Reach {
    /// May we send to this peer now?
    ///
    /// `Unverified` is false: queueing a message against an identity that has
    /// never been verified is how a message ends up delivered to whoever claims
    /// that name next.
    pub fn sendable(self) -> bool { matches!(self, Reach::Connected | Reach::Known) }

    /// Does it go out immediately, or queue? A roster that shows one dot for
    /// both leaves the user unable to tell a delivered message from a pending
    /// one.
    pub fn immediate(self) -> bool { matches!(self, Reach::Connected) }

    pub fn label(self) -> &'static str {
        match self {
            Reach::Known => "offline — will queue",
            Reach::Unverified => "not yet verified",
            Reach::Connected => "connected",
        }
    }
}

/// A roster entry.
#[derive(Debug, Clone, PartialEq)]
pub struct RosterPeer {
    pub id: PeerId,
    pub name: String,
    pub reach: Reach,
}

impl RosterPeer {
    /// What to show. An unnamed peer shows a short form of its id rather than
    /// blank — a peer id is unreadable in full and absent is worse.
    pub fn label(&self) -> String {
        if !self.name.trim().is_empty() { return self.name.clone(); }
        let hex: String = self.id.iter().take(4).map(|b| format!("{b:02x}")).collect();
        format!("peer {hex}")
    }
}

/// Build the roster from the broker plus whatever was merely seen.
///
/// `seen` are peer ids observed on the network; `known` are ids from previous
/// sessions with their names. A peer in the broker outranks both, because a
/// live session is the strongest evidence available.
pub fn roster(broker: &Broker, known: &[(PeerId, String)], seen: &[PeerId]) -> Vec<RosterPeer> {
    let mut out: Vec<RosterPeer> = Vec::new();
    for (id, name) in known {
        let reach = if broker.has_peer(id) { Reach::Connected } else { Reach::Known };
        out.push(RosterPeer { id: *id, name: name.clone(), reach });
    }
    for id in seen {
        if out.iter().any(|p| &p.id == id) { continue; }
        let reach = if broker.has_peer(id) { Reach::Connected } else { Reach::Unverified };
        out.push(RosterPeer { id: *id, name: String::new(), reach });
    }
    // Connected first, then known, then unverified; ties on label so the list
    // cannot reshuffle between frames.
    out.sort_by(|a, b| {
        let rank = |r: Reach| match r { Reach::Connected => 0, Reach::Known => 1, Reach::Unverified => 2 };
        rank(a.reach).cmp(&rank(b.reach)).then(a.label().cmp(&b.label()))
    });
    out
}

/// How many peers can be sent to right now, for the compose screen's "to N
/// people" line.
pub fn sendable_count(roster: &[RosterPeer]) -> usize {
    roster.iter().filter(|p| p.reach.sendable()).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use peermux_core::{handshake_pair, Identity};

    fn ids() -> (PeerId, PeerId) {
        let (a, b) = (Identity::generate(), Identity::generate());
        (a.peer_id(), b.peer_id())
    }

    #[test]
    fn an_unverified_peer_is_not_sendable() {
        // Queueing against an identity that has never been verified is how a
        // message ends up delivered to whoever claims that name next.
        assert!(!Reach::Unverified.sendable());
        assert!(Reach::Known.sendable(), "known peers queue, which is fine");
        assert!(Reach::Connected.sendable());
    }

    #[test]
    fn queued_and_immediate_are_distinguishable() {
        // One dot for both leaves the user unable to tell a delivered message
        // from a pending one.
        assert!(Reach::Connected.immediate());
        assert!(!Reach::Known.immediate());
        assert!(Reach::Known.label().contains("queue"));
    }

    #[test]
    fn a_live_session_outranks_a_remembered_name() {
        let a = Identity::generate();
        let b = Identity::generate();
        let (pa, pb) = (a.peer_id(), b.peer_id());
        let (sa, _sb) = handshake_pair(a, b).expect("handshake");
        let mut broker = Broker::new();
        broker.install_session(sa);
        let r = roster(&broker, &[(pb, "Ada".into())], &[]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].reach, Reach::Connected, "the broker knows this peer");
        let _ = pa;
    }

    #[test]
    fn a_known_peer_with_no_session_queues_rather_than_disappearing() {
        // The hardcoded roster's `online: false` lost the peer entirely; a
        // remembered peer is still someone you can write to.
        let (_, pb) = ids();
        let r = roster(&Broker::new(), &[(pb, "Bo".into())], &[]);
        assert_eq!(r[0].reach, Reach::Known);
        assert!(r[0].reach.sendable());
        assert_eq!(sendable_count(&r), 1);
    }

    #[test]
    fn a_seen_but_unhandshaken_peer_is_listed_and_not_sendable() {
        let (pa, _) = ids();
        let r = roster(&Broker::new(), &[], &[pa]);
        assert_eq!(r.len(), 1, "listed, so the user knows someone is nearby");
        assert_eq!(r[0].reach, Reach::Unverified);
        assert_eq!(sendable_count(&r), 0, "but not sendable");
    }

    #[test]
    fn a_peer_that_is_both_known_and_seen_appears_once() {
        // Otherwise the same person is in the list twice with different states,
        // and the user cannot tell which one to tap.
        let (_, pb) = ids();
        let r = roster(&Broker::new(), &[(pb, "Bo".into())], &[pb]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "Bo", "the known name wins over the bare id");
    }

    #[test]
    fn the_list_is_ordered_and_does_not_reshuffle() {
        let (pa, pb) = ids();
        let a = roster(&Broker::new(), &[(pb, "Bo".into())], &[pa]);
        let b = roster(&Broker::new(), &[(pb, "Bo".into())], &[pa]);
        assert_eq!(a, b);
        assert_eq!(a[0].reach, Reach::Known, "known before unverified");
    }

    #[test]
    fn an_unnamed_peer_shows_a_short_id_rather_than_blank() {
        let (pa, _) = ids();
        let r = roster(&Broker::new(), &[], &[pa]);
        assert!(r[0].label().starts_with("peer "), "{}", r[0].label());
        assert!(r[0].label().len() < 20, "a full peer id is unreadable");
    }
}
