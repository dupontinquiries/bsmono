//! lock.rs — soft locks on named parts.
//!
//! The plan calls for "optimistic editing with feature soft-locks", and *soft* is the operative
//! word. A lock here is a courtesy, not a mutex: it tells you somebody else is working on the
//! bracket so you do not both reshape it, and it expires on its own so a peer that crashes or
//! walks away does not leave the document permanently unpickable.
//!
//! Nothing enforces a lock against a determined editor, and that is deliberate. A hard lock in a
//! distributed session needs consensus to be correct, and consensus needs a quorum, and a quorum
//! makes a two-person session on a flaky link stop working entirely. The document tolerates
//! conflicting ops by design — `cad-forge` reports rather than fails — so the worst outcome of
//! ignoring a lock is a warning, which is a much better failure than a stall.

use std::collections::HashMap;

use crate::PeerId;

/// How long a lock survives without renewal, in whatever logical unit the caller ticks in.
pub const LEASE: u64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockError {
    /// Somebody else holds it, and the lease has not run out.
    HeldBy(PeerId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Lease {
    peer: PeerId,
    until: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Locks {
    held: HashMap<String, Lease>,
}

impl Locks {
    /// Claim a part, or renew a claim you already hold. Expired leases are taken over silently,
    /// which is the point of a lease.
    pub fn claim(&mut self, part: &str, peer: PeerId, now: u64) -> Result<(), LockError> {
        match self.held.get(part) {
            Some(l) if l.until > now && l.peer != peer => Err(LockError::HeldBy(l.peer)),
            _ => {
                self.held.insert(part.to_string(), Lease { peer, until: now + LEASE });
                Ok(())
            }
        }
    }

    /// Release a part you hold. Releasing someone else's lock does nothing — this is the one place
    /// a soft lock is not soft, because a peer that could drop another's lock could do it by
    /// accident on every reconnect.
    pub fn release(&mut self, part: &str, peer: PeerId) {
        if self.held.get(part).is_some_and(|l| l.peer == peer) {
            self.held.remove(part);
        }
    }

    /// Who holds this part right now, if anyone.
    pub fn holder(&self, part: &str, now: u64) -> Option<PeerId> {
        self.held.get(part).filter(|l| l.until > now).map(|l| l.peer)
    }

    /// Advisory: may this peer edit this part without stepping on someone? A `false` here should
    /// produce a warning in the UI, never a refusal to act.
    pub fn advisable(&self, part: &str, peer: PeerId, now: u64) -> bool {
        self.holder(part, now).map(|h| h == peer).unwrap_or(true)
    }

    /// Drop expired leases. Not required for correctness — `holder` already ignores them — but it
    /// keeps the map from growing across a long session.
    pub fn expire(&mut self, now: u64) -> usize {
        let before = self.held.len();
        self.held.retain(|_, l| l.until > now);
        before - self.held.len()
    }

    /// Everything currently held, for a presence display. Sorted, so a UI does not flicker.
    pub fn all(&self, now: u64) -> Vec<(String, PeerId)> {
        let mut v: Vec<(String, PeerId)> = self
            .held
            .iter()
            .filter(|(_, l)| l.until > now)
            .map(|(k, l)| (k.clone(), l.peer))
            .collect();
        v.sort();
        v
    }

    /// Release everything a departing peer held, so leaving a session does not require waiting out
    /// every lease.
    pub fn release_all(&mut self, peer: PeerId) -> usize {
        let before = self.held.len();
        self.held.retain(|_, l| l.peer != peer);
        before - self.held.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_excludes_others_but_not_yourself() {
        let mut l = Locks::default();
        assert!(l.claim("bracket", 1, 0).is_ok());
        assert_eq!(l.claim("bracket", 2, 0), Err(LockError::HeldBy(1)));
        assert!(l.claim("bracket", 1, 5).is_ok(), "renewal by the holder must succeed");
        assert!(l.claim("other", 2, 0).is_ok(), "a different part is unaffected");
    }

    #[test]
    fn a_lease_expires_so_a_crashed_peer_does_not_freeze_the_document() {
        let mut l = Locks::default();
        l.claim("bracket", 1, 0).unwrap();
        assert_eq!(l.holder("bracket", LEASE - 1), Some(1));
        assert_eq!(l.holder("bracket", LEASE + 1), None, "the lease must lapse on its own");
        assert!(l.claim("bracket", 2, LEASE + 1).is_ok(), "and be claimable afterwards");
    }

    #[test]
    fn you_cannot_release_someone_elses_lock() {
        let mut l = Locks::default();
        l.claim("bracket", 1, 0).unwrap();
        l.release("bracket", 2);
        assert_eq!(l.holder("bracket", 0), Some(1), "peer 2 must not be able to drop peer 1's lock");
        l.release("bracket", 1);
        assert_eq!(l.holder("bracket", 0), None);
    }

    #[test]
    fn locks_are_advisory() {
        let mut l = Locks::default();
        l.claim("bracket", 1, 0).unwrap();
        // The API reports that editing is inadvisable; it does not and cannot prevent it, because
        // nothing here sits between a peer and its own document.
        assert!(!l.advisable("bracket", 2, 0));
        assert!(l.advisable("bracket", 1, 0));
        assert!(l.advisable("unlocked", 2, 0));
    }

    #[test]
    fn departure_and_expiry_clean_up() {
        let mut l = Locks::default();
        l.claim("a", 1, 0).unwrap();
        l.claim("b", 1, 0).unwrap();
        l.claim("c", 2, 0).unwrap();
        assert_eq!(l.all(0).len(), 3);
        assert_eq!(l.release_all(1), 2);
        assert_eq!(l.all(0), vec![("c".to_string(), 2)]);
        assert_eq!(l.expire(LEASE + 1), 1);
        assert!(l.all(LEASE + 1).is_empty());
    }
}
