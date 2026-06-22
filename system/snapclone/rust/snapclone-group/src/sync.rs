//! sync.rs — the group anti-entropy sync engine over the DAG.
//!
//! This is the loop that makes an ephemeral group actually converge: each member
//! periodically announces its DAG tips; a peer that hears tips it can't explain
//! computes the delta (`missing_for`) and sends back the nodes the announcer
//! lacks; a member that receives a node whose parents haven't arrived buffers it
//! and requests the missing parents. Run it on every member and the DAGs
//! converge to the same total order — no central server, no full-history
//! transfer.
//!
//! # Why a transport-free state machine
//!
//! The protocol logic — what to send when, what to do with each inbound message,
//! how to avoid re-requesting the same node forever — is pure logic and belongs
//! in tested Rust. The actual moving of bytes (DHT feed bucket, LAN socket,
//! relay) is the app's job. So this engine:
//!   * takes ticks and inbound [`SyncMessage`]s,
//!   * mutates the local [`SnapDag`],
//!   * and RETURNS the [`SyncMessage`]s the caller should transmit.
//! It performs no I/O. The caller seals/signs each outgoing message (a node is a
//! sealed `GroupEntryBody` — see `entry::from_dag_node`) and ships it; inbound,
//! the caller verifies+opens before handing the node here.
//!
//! # The three message types
//!
//!   * `AnnounceTips { tips }` — "here's my frontier." Cheap, sent on a timer.
//!   * `WantNodes { ids }` — "send me these node ids" (parents I'm missing, or
//!     the delta you implied I lack).
//!   * `HaveNode { node }` — "here's a node." The unit of actual data transfer.
//!
//! # Convergence + termination
//!
//! Two members exchanging tips will, in a bounded number of rounds, each end up
//! with every node in the union of their histories (anti-entropy). Re-request
//! storms are avoided with a small in-flight set + a cap on outstanding wants;
//! a node already present or already requested is never re-requested.

use crate::dag::{DagNode, InsertOutcome, NodeId, SnapDag};
use std::collections::HashSet;

/// A protocol message. The caller serializes/seals these for transport; here
/// they're just data. Kept small + cloneable.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncMessage {
    /// "My current frontier is these tips."
    AnnounceTips { tips: Vec<NodeId> },
    /// "Please send me these nodes."
    WantNodes { ids: Vec<NodeId> },
    /// "Here is a node."
    HaveNode { node: DagNode },
}

/// Tuning for the sync engine.
#[derive(Debug, Clone, Copy)]
pub struct SyncConfig {
    /// Max node ids to request in a single WantNodes (keeps messages bounded;
    /// the rest are requested on the next round).
    pub max_wants_per_round: usize,
    /// Max HaveNode messages to emit in response to one WantNodes (bounds a
    /// single peer's burst; remaining wanted nodes come on a re-request).
    pub max_haves_per_round: usize,
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            max_wants_per_round: 64,
            max_haves_per_round: 32,
        }
    }
}

/// The per-peer (or per-group) sync engine. Wraps the local DAG and tracks which
/// node ids are already requested so it never re-requests in a tight loop.
#[derive(Debug)]
pub struct SyncEngine {
    dag: SnapDag,
    cfg: SyncConfig,
    /// Node ids we've asked for and are still waiting on (avoid duplicate WANTs).
    in_flight: HashSet<NodeId>,
}

impl SyncEngine {
    pub fn new(dag: SnapDag, cfg: SyncConfig) -> Self {
        SyncEngine {
            dag,
            cfg,
            in_flight: HashSet::new(),
        }
    }

    pub fn with_default_config(dag: SnapDag) -> Self {
        Self::new(dag, SyncConfig::default())
    }

    /// Borrow the underlying DAG (read-only) — for the app to render the ordered
    /// feed via `dag.total_order()`.
    pub fn dag(&self) -> &SnapDag {
        &self.dag
    }

    /// Author a local snap reference: extends the DAG and returns the
    /// AnnounceTips the caller should broadcast so peers learn the new tip.
    pub fn author(&mut self, author: &str, epoch: u64, content_hash: &str) -> SyncMessage {
        self.dag.author_node(author, epoch, content_hash);
        self.announce()
    }

    /// The message to broadcast on a timer: our current tips.
    pub fn announce(&self) -> SyncMessage {
        SyncMessage::AnnounceTips {
            tips: self.dag.tips(),
        }
    }

    /// Handle one inbound message; mutate the DAG; return the messages to send
    /// back (possibly empty). This is the whole protocol.
    pub fn handle(&mut self, msg: SyncMessage) -> Vec<SyncMessage> {
        match msg {
            SyncMessage::AnnounceTips { tips } => self.on_announce(tips),
            SyncMessage::WantNodes { ids } => self.on_want(ids),
            SyncMessage::HaveNode { node } => self.on_have(node),
        }
    }

    /// A peer told us their tips. Two things to do:
    ///   1. If any of their tips are nodes WE don't have, we want them.
    ///   2. Compute what THEY are missing (relative to our DAG) and offer it —
    ///      but offering is "here are nodes"; to keep messages bounded we send
    ///      the delta as HaveNode messages directly (the common case: a peer a
    ///      few nodes behind catches up immediately).
    fn on_announce(&mut self, peer_tips: Vec<NodeId>) -> Vec<SyncMessage> {
        let mut out = Vec::new();

        // 1. Their tips we lack → want them.
        let want: Vec<NodeId> = peer_tips
            .iter()
            .filter(|id| !self.dag.contains(id) && !self.in_flight.contains(*id))
            .cloned()
            .collect();
        if !want.is_empty() {
            out.extend(self.emit_wants(want));
        }

        // 2. What they're missing relative to us → push as HaveNode (bounded).
        let missing = self.dag.missing_for(&peer_tips);
        for id in missing.into_iter().take(self.cfg.max_haves_per_round) {
            if let Some(node) = self.dag.get(&id) {
                out.push(SyncMessage::HaveNode { node: node.clone() });
            }
        }
        out
    }

    /// A peer asked for nodes. Send the ones we have (bounded per round).
    fn on_want(&mut self, ids: Vec<NodeId>) -> Vec<SyncMessage> {
        let mut out = Vec::new();
        for id in ids.into_iter().take(self.cfg.max_haves_per_round) {
            if let Some(node) = self.dag.get(&id) {
                out.push(SyncMessage::HaveNode { node: node.clone() });
            }
        }
        out
    }

    /// A peer sent us a node. Insert it; if it's an orphan, request its missing
    /// parents; once admitted, clear it from in-flight. A successful admit can
    /// cascade (orphans waiting on it), and may reveal further missing parents
    /// to request.
    fn on_have(&mut self, node: DagNode) -> Vec<SyncMessage> {
        let id = node.compute_id();
        let outcome = self.dag.insert(node);
        let mut out = Vec::new();
        match outcome {
            InsertOutcome::Admitted { admitted_ids } => {
                for aid in &admitted_ids {
                    self.in_flight.remove(aid);
                }
                // Admitting may have left other orphans still waiting on parents
                // we don't have — request the current outstanding fetch set.
                let still_missing: Vec<NodeId> = self
                    .dag
                    .pending_fetches()
                    .into_iter()
                    .filter(|p| !self.in_flight.contains(p))
                    .collect();
                if !still_missing.is_empty() {
                    out.extend(self.emit_wants(still_missing));
                }
            }
            InsertOutcome::Orphaned { missing_parents } => {
                self.in_flight.remove(&id); // this node arrived; now we want its parents
                let want: Vec<NodeId> = missing_parents
                    .into_iter()
                    .filter(|p| !self.in_flight.contains(p))
                    .collect();
                if !want.is_empty() {
                    out.extend(self.emit_wants(want));
                }
            }
            // Duplicate / BadId / BadClock: nothing to send. A bad node is
            // dropped; the peer will keep announcing tips so we can re-request a
            // good copy if one exists.
            _ => {
                self.in_flight.remove(&id);
            }
        }
        out
    }

    /// Build WantNodes message(s) for `ids`, marking them in-flight and honoring
    /// the per-round cap. Returns at most one message (the cap bounds it); the
    /// remainder will be re-derived on the next inbound round.
    fn emit_wants(&mut self, ids: Vec<NodeId>) -> Vec<SyncMessage> {
        let mut batch = Vec::new();
        for id in ids {
            if batch.len() >= self.cfg.max_wants_per_round {
                break;
            }
            if self.in_flight.insert(id.clone()) {
                batch.push(id);
            }
        }
        if batch.is_empty() {
            Vec::new()
        } else {
            vec![SyncMessage::WantNodes { ids: batch }]
        }
    }

    /// Outstanding wants (for diagnostics / back-pressure).
    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Drive two engines against each other until no more messages flow or a
    // round cap is hit. Returns the number of rounds. Models a reliable
    // transport (every emitted message is delivered to the other side).
    fn run_to_quiescence(a: &mut SyncEngine, b: &mut SyncEngine, max_rounds: usize) -> usize {
        // Seed: each announces its tips to the other.
        let mut a_inbox = vec![b.announce()];
        let mut b_inbox = vec![a.announce()];
        for round in 1..=max_rounds {
            let mut a_out = Vec::new();
            for m in a_inbox.drain(..) {
                a_out.extend(a.handle(m));
            }
            let mut b_out = Vec::new();
            for m in b_inbox.drain(..) {
                b_out.extend(b.handle(m));
            }
            if a_out.is_empty() && b_out.is_empty() {
                return round;
            }
            // Deliver each side's output to the other.
            b_inbox = a_out;
            a_inbox = b_out;
        }
        max_rounds
    }

    #[test]
    fn two_peers_converge_to_same_total_order() {
        // A has a chain of 3 nodes; B is empty. After sync, B has all of A's.
        let mut da = SnapDag::new();
        da.author_node("sc-a", 1, "h0");
        da.author_node("sc-a", 1, "h1");
        da.author_node("sc-a", 1, "h2");
        let mut a = SyncEngine::with_default_config(da);
        let mut b = SyncEngine::with_default_config(SnapDag::new());

        run_to_quiescence(&mut a, &mut b, 50);

        assert_eq!(a.dag().len(), 3);
        assert_eq!(b.dag().len(), 3, "B caught up to all of A's nodes");
        assert_eq!(
            a.dag().total_order(),
            b.dag().total_order(),
            "both converge to the identical order"
        );
    }

    #[test]
    fn divergent_histories_merge_both_ways() {
        // A and B each authored their own nodes independently; after sync both
        // should hold the UNION and agree on the order.
        let mut da = SnapDag::new();
        da.author_node("sc-a", 1, "a0");
        da.author_node("sc-a", 1, "a1");
        let mut db = SnapDag::new();
        db.author_node("sc-b", 1, "b0");
        db.author_node("sc-b", 1, "b1");
        db.author_node("sc-b", 1, "b2");

        let mut a = SyncEngine::with_default_config(da);
        let mut b = SyncEngine::with_default_config(db);
        run_to_quiescence(&mut a, &mut b, 50);

        assert_eq!(a.dag().len(), 5, "A has the union");
        assert_eq!(b.dag().len(), 5, "B has the union");
        assert_eq!(a.dag().total_order(), b.dag().total_order());
    }

    #[test]
    fn out_of_order_delivery_still_converges() {
        // A has a 4-node chain. We hand B the HaveNode messages in REVERSE so it
        // must buffer orphans and request parents. The engine should still end
        // up complete.
        let mut da = SnapDag::new();
        let (_n0, _) = da.author_node("sc-a", 1, "h0");
        let (_n1, _) = da.author_node("sc-a", 1, "h1");
        let (_n2, _) = da.author_node("sc-a", 1, "h2");
        let (_n3, _) = da.author_node("sc-a", 1, "h3");
        let mut a = SyncEngine::with_default_config(da);
        let mut b = SyncEngine::with_default_config(SnapDag::new());

        // Collect A's nodes newest-first and feed them to B out of order; B will
        // emit WantNodes for missing parents, which A answers.
        let mut b_inbox: Vec<SyncMessage> = {
            // Ask A for everything by announcing empty tips.
            a.handle(SyncMessage::AnnounceTips { tips: vec![] })
        };
        // Reverse the HaveNode order to stress orphan buffering.
        b_inbox.reverse();

        for _ in 0..50 {
            let mut b_out = Vec::new();
            for m in b_inbox.drain(..) {
                b_out.extend(b.handle(m));
            }
            if b_out.is_empty() {
                break;
            }
            // A answers B's wants.
            let mut a_out = Vec::new();
            for m in b_out {
                a_out.extend(a.handle(m));
            }
            b_inbox = a_out;
        }
        assert_eq!(b.dag().len(), 4, "B reassembled the full chain out of order");
        assert_eq!(a.dag().total_order(), b.dag().total_order());
    }

    #[test]
    fn no_duplicate_wants_for_in_flight_nodes() {
        // Announcing the same unknown tip twice must not double-request it.
        let mut da = SnapDag::new();
        let (_n, id) = da.author_node("sc-a", 1, "h0");
        let mut b = SyncEngine::with_default_config(SnapDag::new());

        let out1 = b.handle(SyncMessage::AnnounceTips { tips: vec![id.clone()] });
        let out2 = b.handle(SyncMessage::AnnounceTips { tips: vec![id.clone()] });
        // First produces a WantNodes; second produces nothing (already in-flight).
        assert!(matches!(out1.as_slice(), [SyncMessage::WantNodes { .. }]));
        assert!(out2.is_empty(), "no re-request while in-flight");
        assert_eq!(b.in_flight_count(), 1);
        let _ = da; // (da only used to mint a real id)
    }

    #[test]
    fn author_returns_announce_with_new_tip() {
        let mut a = SyncEngine::with_default_config(SnapDag::new());
        let msg = a.author("sc-a", 1, "h0");
        match msg {
            SyncMessage::AnnounceTips { tips } => assert_eq!(tips.len(), 1),
            other => panic!("expected AnnounceTips, got {other:?}"),
        }
        assert_eq!(a.dag().len(), 1);
    }

    #[test]
    fn already_synced_peers_are_quiet() {
        // Two identical DAGs should exchange tips and then fall silent fast.
        let mut da = SnapDag::new();
        da.author_node("sc-a", 1, "h0");
        da.author_node("sc-a", 1, "h1");
        let db = da.clone(); // identical
        let mut a = SyncEngine::with_default_config(da);
        let mut b = SyncEngine::with_default_config(db);
        let rounds = run_to_quiescence(&mut a, &mut b, 50);
        assert!(rounds <= 2, "identical peers quiesce almost immediately, took {rounds}");
        assert_eq!(a.dag().len(), 2);
        assert_eq!(b.dag().len(), 2);
    }

    #[test]
    fn want_for_unknown_node_yields_nothing() {
        // Being asked for a node we don't have produces no HaveNode (we can't
        // fabricate it) — and doesn't panic.
        let mut a = SyncEngine::with_default_config(SnapDag::new());
        let out = a.handle(SyncMessage::WantNodes {
            ids: vec!["deadbeef".to_owned()],
        });
        assert!(out.is_empty());
    }
}
