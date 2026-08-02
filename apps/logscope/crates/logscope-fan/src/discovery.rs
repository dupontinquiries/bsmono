//! discovery.rs — the loop that drives announce/drain and keeps the peer table.
//!
//! Every piece of LAN discovery existed and nothing called them: `lanwire` sends and receives,
//! `announce` decides what a datagram means, `trustkit` decides whose datagrams matter, `rendezvous`
//! decides which address to prefer. This is the driver, and it is the shape a UI calls once per frame.
//!
//! ## Why it holds a table rather than returning events
//!
//! A caller that must remember what it heard will forget on the frame it matters. The table is the
//! answer to "where is peer X right now", which is the only question anything downstream asks.
//!
//! ## Ageing is the point, not bookkeeping
//!
//! An entry that is never removed turns a machine that left the network into one that appears present
//! and unreachable — the failure `rendezvous::MAX_AGE_S` exists to prevent, and it only works if
//! something advances the clock. That something is here.

/// One known peer and where it was last heard.
#[derive(Clone, PartialEq, Debug)]
pub struct Peer {
    pub id: String,
    pub candidate: crate::rendezvous::Candidate,
}

/// The discovery state a caller keeps between frames.
#[derive(Default)]
pub struct Discovery {
    peers: Vec<Peer>,
    /// Seconds since the last announcement was sent.
    since_announce: f32,
    /// Ids seen that nobody registered, for the UI to offer. Never acted on.
    pub unregistered: Vec<String>,
}

impl Discovery {
    pub fn new() -> Discovery { Discovery::default() }

    /// Should an announcement go out this frame?
    ///
    /// Advanced by the caller's own delta rather than a wall-clock read, so a suspended app does not
    /// wake up and emit a burst — and so this is testable without sleeping.
    pub fn tick(&mut self, dt_s: f32) -> bool {
        self.since_announce += dt_s.max(0.0);
        if self.since_announce >= crate::lanwire::INTERVAL_S as f32 {
            self.since_announce = 0.0;
            return true;
        }
        false
    }

    /// Age every entry and drop the ones past `rendezvous::MAX_AGE_S`.
    ///
    /// Dropped rather than marked stale: a stale entry still renders, and a peer that left the network
    /// showing as present-but-unreachable is worse than one that is simply gone.
    pub fn age(&mut self, dt_s: f32) {
        let d = dt_s.max(0.0) as u64;
        for p in self.peers.iter_mut() { p.candidate.age_s = p.candidate.age_s.saturating_add(d); }
        self.peers.retain(|p| p.candidate.age_s <= crate::rendezvous::MAX_AGE_S);
    }

    /// Fold one accepted announcement in, replacing any earlier entry for that id.
    ///
    /// Replaces rather than appends: a machine that changed address must not appear twice, with the
    /// stale one winning a `find` by being first.
    pub fn observed(&mut self, id: &str, candidate: crate::rendezvous::Candidate) {
        match self.peers.iter_mut().find(|p| p.id == id) {
            Some(p) => p.candidate = candidate,
            None => self.peers.push(Peer { id: id.to_string(), candidate }),
        }
    }

    /// Record an id nobody registered, once.
    ///
    /// Shown so an operator can notice a machine they meant to add. Deduplicated because a peer
    /// announcing every five seconds would otherwise fill the list with itself.
    pub fn saw_unregistered(&mut self, id: &str) {
        if !self.unregistered.iter().any(|x| x == id) {
            self.unregistered.push(id.to_string());
        }
    }

    pub fn peers(&self) -> &[Peer] { &self.peers }

    /// Where is this peer now?
    pub fn find(&self, id: &str) -> Option<&crate::rendezvous::Candidate> {
        self.peers.iter().find(|p| p.id == id).map(|p| &p.candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rendezvous::{Candidate, Route};

    fn cand(addr: &str) -> Candidate {
        Candidate { addr: addr.into(), route: Route::Lan, age_s: 0 }
    }

    #[test]
    fn announcements_go_out_on_the_INTERVAL_not_every_frame() {
        let mut d = Discovery::new();
        assert!(!d.tick(1.0), "one second in: too early");
        assert!(!d.tick(1.0));
        assert!(d.tick(crate::lanwire::INTERVAL_S as f32), "past the interval: send");
        assert!(!d.tick(0.1), "and the clock resets, so the next is a full interval away");
    }

    #[test]
    fn a_suspended_app_does_not_emit_a_BURST() {
        // The clock advances by the caller's delta rather than a wall-clock read. A phone that was
        // backgrounded for an hour should send ONE announcement on resume, not seven hundred.
        let mut d = Discovery::new();
        assert!(d.tick(3600.0), "one send");
        assert!(!d.tick(0.016), "and then back to the normal cadence");
    }

    #[test]
    fn a_peer_that_stops_announcing_is_DROPPED_not_marked_stale() {
        // A stale entry still renders, and a machine that left the network showing as
        // present-but-unreachable is worse than one that is simply gone.
        let mut d = Discovery::new();
        d.observed("m2", cand("10.0.0.5:7000"));
        d.age(crate::rendezvous::MAX_AGE_S as f32);
        assert!(d.find("m2").is_some(), "at the bound it is still known");
        d.age(1.0);
        assert!(d.find("m2").is_none(), "past it, gone");
    }

    #[test]
    fn a_peer_that_MOVED_replaces_its_old_address() {
        // Appending would leave two entries for one machine, with the stale one winning `find` by
        // being first — a peer that is listed at an address it no longer holds.
        let mut d = Discovery::new();
        d.observed("m2", cand("10.0.0.5:7000"));
        d.observed("m2", cand("10.0.0.9:7000"));
        assert_eq!(d.peers().len(), 1);
        assert_eq!(d.find("m2").unwrap().addr, "10.0.0.9:7000");
    }

    #[test]
    fn hearing_a_peer_again_RESETS_its_age() {
        // Otherwise a machine that is present and announcing would age out anyway, and the drop above
        // would fire on a peer that never left.
        let mut d = Discovery::new();
        d.observed("m2", cand("10.0.0.5:7000"));
        d.age(crate::rendezvous::MAX_AGE_S as f32);
        d.observed("m2", cand("10.0.0.5:7000"));
        d.age(1.0);
        assert!(d.find("m2").is_some(), "still here: it announced again");
    }

    #[test]
    fn an_unregistered_id_is_listed_ONCE() {
        // A peer announcing every five seconds would otherwise fill the list with itself, and a list
        // that is mostly one repeated name is one nobody reads.
        let mut d = Discovery::new();
        for _ in 0..10 { d.saw_unregistered("stranger"); }
        assert_eq!(d.unregistered, vec!["stranger"]);
    }
}
