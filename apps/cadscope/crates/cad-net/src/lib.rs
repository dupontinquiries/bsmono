//! cad-net — collaborative editing (plan §11).
//!
//! The design rule is one sentence: **replicate forge ops, not geometry.** A `.cadf` document is
//! already a sequence of named, tolerant, byte-stable operations, so the thing to send over a wire
//! is the operation somebody performed — tens of bytes — rather than the mesh it produced, which
//! may be megabytes and which every peer can rebuild identically anyway. That is only sound
//! because `Doc::apply` is deterministic, which the kernel's tests already pin.
//!
//! Sans-io, in the style of `shared/snapcore`: this crate owns the *rules* of a session — ordering,
//! conflict, locks, convergence — and never touches a socket. Time is a logical `u64`. That is what
//! makes a hundred-peer session testable in a unit test, and it is the same reason snapcore's own
//! Kademlia engine is structured that way.
//!
//! A session maps onto snapcore's cluster abstraction, whose own documentation puts it well: a
//! cluster is something you JOIN — a shared mailbox. A CADscope session is exactly that, with the
//! document as its subject.

use std::collections::{HashMap, HashSet};

use cad_forge::{Doc, Op};
use serde::{Deserialize, Serialize};

pub mod lock;
pub use lock::{LockError, Locks};

/// Who made a change. Opaque here; the transport decides what it means.
pub type PeerId = u64;

/// A replicated edit. Ops carry their author and a Lamport stamp, because two peers editing at once
/// must produce the same document on both machines, and wall-clock time cannot be trusted to
/// decide that.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    pub peer: PeerId,
    /// Lamport clock: max(seen) + 1 at the moment of authoring.
    pub stamp: u64,
    pub op: Op,
}

impl Edit {
    /// Total order: stamp first, then peer id as the tiebreak. Deterministic on every machine,
    /// which is the whole requirement — it does not matter *which* order is chosen, only that
    /// everyone chooses the same one.
    fn key(&self) -> (u64, PeerId) {
        (self.stamp, self.peer)
    }
}

/// What changed as a result of applying remote edits, so a UI can react without diffing documents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Delta {
    pub applied: usize,
    /// Edits that arrived out of order and were inserted before existing ones, forcing a rebuild.
    pub reordered: usize,
    pub duplicates: usize,
    /// Warnings from replaying the document, which the forge reports per op rather than fatally.
    pub warnings: Vec<String>,
}

/// One peer's view of a shared document.
///
/// The log is the truth and the `Doc` is derived from it. That is the opposite of the obvious
/// design — mutate the doc, send a diff — and it is what makes convergence provable: two peers
/// holding the same set of edits sort them the same way and replay them into the same document,
/// no matter what order they arrived in.
#[derive(Clone, Debug)]
pub struct Session {
    pub me: PeerId,
    pub name: String,
    log: Vec<Edit>,
    seen: HashSet<(u64, PeerId)>,
    clock: u64,
    pub locks: Locks,
    peers: HashMap<PeerId, u64>,
}

impl Session {
    pub fn new(me: PeerId, name: &str) -> Session {
        Session {
            me,
            name: name.into(),
            log: Vec::new(),
            seen: HashSet::new(),
            clock: 0,
            locks: Locks::default(),
            peers: HashMap::new(),
        }
    }

    /// Author an edit locally. Returns it for broadcast; the caller sends it however it likes.
    pub fn author(&mut self, op: Op) -> Edit {
        self.clock += 1;
        let e = Edit { peer: self.me, stamp: self.clock, op };
        self.insert(e.clone());
        e
    }

    /// Receive an edit from a peer. Idempotent: the same edit arriving twice is counted and
    /// ignored, because a gossip transport will deliver duplicates and a session that double-applies
    /// them would diverge.
    pub fn receive(&mut self, e: Edit) -> Delta {
        let mut d = Delta::default();
        if self.seen.contains(&e.key()) {
            d.duplicates = 1;
            return d;
        }
        // Lamport: a received stamp advances our own clock past it.
        self.clock = self.clock.max(e.stamp);
        self.peers.insert(e.peer, e.stamp);
        let tail = self.log.last().map(|l| l.key());
        if tail.is_some_and(|t| e.key() < t) {
            d.reordered = 1;
        }
        self.insert(e);
        d.applied = 1;
        d
    }

    /// Receive a batch, reporting one delta.
    pub fn receive_all(&mut self, edits: impl IntoIterator<Item = Edit>) -> Delta {
        let mut total = Delta::default();
        for e in edits {
            let d = self.receive(e);
            total.applied += d.applied;
            total.reordered += d.reordered;
            total.duplicates += d.duplicates;
        }
        let (_, warns) = self.document().apply();
        total.warnings = warns.into_iter().map(|w| format!("op #{}: {}", w.index, w.reason)).collect();
        total
    }

    fn insert(&mut self, e: Edit) {
        self.seen.insert(e.key());
        // Insert in sorted position rather than push-and-sort: an in-order arrival, which is the
        // common case, costs a comparison.
        let at = self.log.partition_point(|x| x.key() < e.key());
        self.log.insert(at, e);
    }

    /// The document this session's log describes. Rebuilt from the log, always.
    pub fn document(&self) -> Doc {
        Doc { name: self.name.clone(), ops: self.log.iter().map(|e| e.op.clone()).collect() }
    }

    /// Evaluate to a model, with the forge's per-op warnings.
    pub fn model(&self) -> (cad_kernel::Model, Vec<cad_forge::OpWarning>) {
        self.document().apply()
    }

    pub fn log(&self) -> &[Edit] {
        &self.log
    }

    /// Edits this session holds that the given peer has not acknowledged — what to send on
    /// reconnect. Cheap and correct rather than clever: a real transport would carry a version
    /// vector, and PARITY says so.
    pub fn since(&self, stamp: u64) -> Vec<Edit> {
        self.log.iter().filter(|e| e.stamp > stamp).cloned().collect()
    }

    /// A fingerprint of the whole log, for cheaply checking that two peers agree. Two sessions
    /// that converged have equal digests; unequal digests mean a real divergence to investigate.
    pub fn digest(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for e in &self.log {
            for b in serde_json::to_string(e).unwrap_or_default().bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100_0000_01b3);
            }
        }
        h
    }

    pub fn peers(&self) -> Vec<PeerId> {
        let mut p: Vec<PeerId> = self.peers.keys().copied().collect();
        p.sort_unstable();
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_forge::ShapeSpec;

    fn add(name: &str, r: f32) -> Op {
        Op::Add {
            name: name.into(),
            shape: ShapeSpec::Sphere { r },
            at: [0.0; 3],
            rot_axis_deg: None,
            color: [1.0; 4],
        }
    }

    #[test]
    fn two_peers_editing_at_once_converge() {
        // The property the whole crate exists for. Alice and Bob each author locally, exchange,
        // and must end up with identical documents — regardless of who sent first.
        let mut alice = Session::new(1, "shared");
        let mut bob = Session::new(2, "shared");
        let a1 = alice.author(add("bracket", 2.0));
        let b1 = bob.author(add("boss", 1.0));
        bob.receive(a1);
        alice.receive(b1);
        assert_eq!(alice.digest(), bob.digest());
        assert_eq!(alice.document(), bob.document());
        assert_eq!(alice.model().0.parts.len(), 2);
    }

    #[test]
    fn arrival_order_does_not_matter() {
        let mut source = Session::new(1, "d");
        let edits: Vec<Edit> = (0..6).map(|i| source.author(add(&format!("p{i}"), 1.0))).collect();

        let mut forward = Session::new(9, "d");
        forward.receive_all(edits.clone());
        let mut backward = Session::new(9, "d");
        backward.receive_all(edits.iter().rev().cloned());
        let mut shuffled = Session::new(9, "d");
        // A fixed non-trivial permutation; no rand dependency in a kernel-adjacent crate.
        for i in [3usize, 0, 5, 1, 4, 2] {
            shuffled.receive(edits[i].clone());
        }

        assert_eq!(forward.digest(), backward.digest());
        assert_eq!(forward.digest(), shuffled.digest());
        assert_eq!(forward.document(), shuffled.document());
    }

    #[test]
    fn duplicates_are_ignored_not_reapplied() {
        let mut alice = Session::new(1, "d");
        let e = alice.author(add("part", 1.0));
        let mut bob = Session::new(2, "d");
        assert_eq!(bob.receive(e.clone()).applied, 1);
        let again = bob.receive(e.clone());
        assert_eq!(again.duplicates, 1);
        assert_eq!(again.applied, 0);
        assert_eq!(bob.log().len(), 1, "a gossip transport will deliver this twice");
        assert_eq!(bob.model().0.parts.len(), 1);
    }

    #[test]
    fn a_late_edit_reorders_rather_than_appending() {
        let mut alice = Session::new(1, "d");
        let mut bob = Session::new(2, "d");
        let early = alice.author(add("first", 1.0));
        // Bob authors twice before hearing from Alice, so his clock runs ahead.
        bob.author(add("bob_a", 1.0));
        bob.author(add("bob_b", 1.0));
        let d = bob.receive(early);
        assert_eq!(d.reordered, 1, "an edit older than the tail must be inserted, not appended");
        // And it lands first in the log, so the document reads in causal order.
        assert_eq!(bob.log()[0].peer, 1);
    }

    #[test]
    fn a_conflicting_name_is_a_warning_not_a_crash() {
        // Both peers add a part with the same name. The forge tolerates it per-op, so the session
        // converges with a warning rather than one peer's work vanishing silently.
        let mut alice = Session::new(1, "d");
        let mut bob = Session::new(2, "d");
        let a = alice.author(add("same", 2.0));
        let b = bob.author(add("same", 3.0));
        let da = alice.receive_all([b]);
        let db = bob.receive_all([a]);
        assert_eq!(alice.digest(), bob.digest(), "they must still converge");
        assert!(!da.warnings.is_empty(), "and the collision must be reported");
        assert_eq!(da.warnings, db.warnings, "identically on both sides");
        assert_eq!(alice.model().0.parts.len(), 1);
    }

    #[test]
    fn ten_peers_converge_on_one_document() {
        let mut peers: Vec<Session> = (1..=10).map(|i| Session::new(i, "big")).collect();
        let mut wire: Vec<Edit> = Vec::new();
        for (i, p) in peers.iter_mut().enumerate() {
            wire.push(p.author(add(&format!("part{i}"), 1.0 + i as f32)));
        }
        // Every peer hears every edit, each in a different rotation.
        for (i, p) in peers.iter_mut().enumerate() {
            let mut order = wire.clone();
            order.rotate_left(i);
            p.receive_all(order);
        }
        let first = peers[0].digest();
        for (i, p) in peers.iter().enumerate() {
            assert_eq!(p.digest(), first, "peer {i} diverged");
            assert_eq!(p.model().0.parts.len(), 10);
        }
    }

    #[test]
    fn since_returns_what_a_reconnecting_peer_missed() {
        let mut alice = Session::new(1, "d");
        alice.author(add("a", 1.0));
        let mark = alice.log().last().unwrap().stamp;
        alice.author(add("b", 1.0));
        alice.author(add("c", 1.0));
        let missed = alice.since(mark);
        assert_eq!(missed.len(), 2);
        let mut rejoin = Session::new(2, "d");
        rejoin.receive_all(missed);
        assert_eq!(rejoin.log().len(), 2);
    }

    #[test]
    fn geometry_never_travels() {
        // The design rule, asserted: an edit serialises to an op, and an op names shapes and
        // parameters. A mesh would be orders of magnitude larger and is not representable here.
        let mut alice = Session::new(1, "d");
        let e = alice.author(add("bracket", 2.0));
        let wire = serde_json::to_string(&e).unwrap();
        assert!(wire.len() < 200, "an edit is tens of bytes, not a mesh: {} bytes", wire.len());
        assert!(wire.contains("sphere"));
        let back: Edit = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, e);
    }
}
