//! chunk_sync.rs — a durable, per-peer photo sync channel, built on
//! `snapclone-group`'s ephemeral-snap machinery aimed at something it
//! wasn't written for.
//!
//! # What's borrowed, and why it fits anyway
//!
//! `snapclone-group::dag::SnapDag` is a content-addressed Merkle DAG built to
//! order ephemeral group snaps and diff two replicas' histories cheaply
//! (`missing_for`/the tips exchange). `sync::SyncEngine` is the transport-free
//! anti-entropy state machine on top of it: `AnnounceTips` / `WantNodes` /
//! `HaveNode`, with no I/O and no assumption about what a "snap" actually is
//! — `DagNode::content_hash` is a bare string. photosync needs exactly this
//! shape for a different reason: not "which ephemeral snaps has this group
//! seen" but "which chunks of my photo library does this peer have", and a
//! chunk hash fits in that same bare string with room to spare.
//!
//! # What's deliberately NOT borrowed
//!
//! snapclone's DAG nodes are references; the media itself is "content-
//! addressed, fetched P2P, ephemeral by TTL" (see `dag.rs`). That TTL is
//! correct for snaps that are supposed to disappear and wrong for photos
//! that are not. So here the DAG carries `ChunkRef`s — pointers — and the
//! actual bytes stay exactly where they already durably live:
//! `PhotoSyncDB`'s block store, keyed by the same chunk hash
//! `chunk_and_store` already computed. Nothing about a photo's only copy
//! ever depends on a DHT feed bucket's retention window.
//!
//! # Granularity: chunk, not file
//!
//! One `DagNode` per chunk, not per file. A dropped connection partway
//! through a large file loses only the chunks not yet exchanged — the ones
//! already admitted into the peer's DAG stay there, so resuming is "run
//! another round," not "start the file over." This is the same reasoning
//! that made `PhotoSyncDB` chunk files in the first place; it just extends
//! to the wire, not only to local dedup.
//!
//! # Durable channel, ephemeral connection
//!
//! A `ChunkSync` is meant to be created once per (library, peer) pair and
//! kept — persisted, eventually; in-memory only today — across many short
//! sync rounds, not rebuilt from scratch each time a connection opens. A
//! fresh `SnapDag` every round would throw away convergence already reached
//! and force a full tips renegotiation each time, which is exactly the cost
//! anti-entropy is supposed to amortize away.
//!
//! # Diarization
//!
//! `snapclone_group::delivery::DeliveryTracker` is reused unchanged: keyed
//! by `NodeId` (here, a chunk), `Pending → Delivered → Seen`, idempotent,
//! monotonic. Its own doc is explicit that it does no I/O and expects the
//! transport to call `record_delivered`/`record_seen` on real acks — this
//! module doesn't fabricate those from the sync protocol either. The one
//! thing added here is `record_sent` at the point a chunk's node id is
//! actually handed to a peer, which is `announce_file`'s job.

use snapclone_group::dag::{DagNode, NodeId, SnapDag};
use snapclone_group::delivery::DeliveryTracker;
use snapclone_group::sync::{SyncEngine, SyncMessage};

/// One chunk of one file, as it travels through the DAG. `DagNode` is
/// `snapclone-group`'s shared type — used by its real ephemeral snaps too —
/// so this packs into its single opaque `content_hash` string rather than
/// widening that struct to fit photosync's shape onto it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRef {
    pub path: String,
    pub chunk_index: u32,
    pub chunk_hash: String,
}

impl ChunkRef {
    fn encode(&self) -> String {
        // NUL-separated: a path can hold almost anything else printable.
        format!("{}\0{}\0{}", self.path, self.chunk_index, self.chunk_hash)
    }

    fn decode(s: &str) -> Option<ChunkRef> {
        let mut parts = s.splitn(3, '\0');
        let path = parts.next()?.to_string();
        let chunk_index: u32 = parts.next()?.parse().ok()?;
        let chunk_hash = parts.next()?.to_string();
        Some(ChunkRef { path, chunk_index, chunk_hash })
    }
}

/// A durable sync channel to one peer, for one library. See the module doc
/// for why this outlives any single connection.
pub struct ChunkSync {
    engine: SyncEngine,
    delivery: DeliveryTracker,
    device_id: String,
}

impl ChunkSync {
    pub fn new(device_id: &str) -> ChunkSync {
        ChunkSync {
            engine: SyncEngine::with_default_config(SnapDag::new()),
            delivery: DeliveryTracker::new(),
            device_id: device_id.to_string(),
        }
    }

    /// Start a new connection to the peer. Keeps everything admitted so far
    /// (the whole point of a durable channel) but discards in-flight
    /// request bookkeeping.
    ///
    /// This matters because `SyncEngine`'s `in_flight` set has no timeout:
    /// once a `WantNodes` is sent, that id is never re-requested until a
    /// `HaveNode` (or a bad/duplicate reply) for it arrives — there's no
    /// other way to clear it. A connection that dies with a request
    /// outstanding leaves that id permanently stuck as "already asked for"
    /// even though the answer is never coming, which silently stalls the
    /// whole chain behind it (discovered by testing an interrupted-then-
    /// resumed exchange: without this, resuming after a drop converged on
    /// nothing, forever, from that point on). Cloning the DAG into a fresh
    /// `SyncEngine` is the fix: a new connection is a legitimate reason to
    /// say "whatever I asked for on the old one, ask again" — nothing here
    /// discards admitted history, only what's mid-flight.
    pub fn begin_connection(&mut self) {
        let dag = self.engine.dag().clone();
        self.engine = SyncEngine::with_default_config(dag);
    }

    /// Register a file's chunks as available to sync — the chunk hashes
    /// `PhotoSyncDB::chunk_and_store` already computed, nothing recomputed
    /// here. One node per chunk, authored in order, each parenting the
    /// previous one's tip.
    ///
    /// Returns the new node ids in chunk order. `SyncEngine::author` only
    /// hands back an `AnnounceTips`, not the id of the node it just made —
    /// but this device is the DAG's sole writer until a peer's `HaveNode`s
    /// start arriving, and a solo author's tip set is always exactly the
    /// one node it just authored (everything before it is that node's
    /// parent, not a sibling). That invariant is what `expect` below is
    /// trusting; it would only fail if this method were called between
    /// unmerged inbound messages, which nothing in this module does.
    pub fn announce_file(&mut self, path: &str, chunk_hashes: &[String], epoch: u64) -> Vec<NodeId> {
        chunk_hashes
            .iter()
            .enumerate()
            .map(|(i, hash)| {
                let payload = ChunkRef { path: path.to_string(), chunk_index: i as u32, chunk_hash: hash.clone() }.encode();
                match self.engine.author(&self.device_id, epoch, &payload) {
                    SyncMessage::AnnounceTips { tips } => {
                        tips.into_iter().next().expect("a solo author always leaves exactly one tip")
                    }
                    _ => unreachable!("SyncEngine::author always returns AnnounceTips"),
                }
            })
            .collect()
    }

    /// Record that `node_ids` (typically `announce_file`'s return value)
    /// were sent to `peer_device_id` — the diarization entry point. Actual
    /// delivered/seen upgrades come later, from the transport, via
    /// `delivery_mut()`.
    pub fn record_sent(&mut self, node_ids: &[NodeId], peer_device_id: &str) {
        for id in node_ids {
            self.delivery.record_sent(id, [peer_device_id], &self.device_id);
        }
    }

    /// The message to broadcast on a sync round: this device's current
    /// frontier of known chunks.
    pub fn announce(&self) -> SyncMessage {
        self.engine.announce()
    }

    /// Handle one inbound protocol message; returns the messages to send
    /// back (possibly empty). The whole anti-entropy protocol, unchanged
    /// from `snapclone_group::sync` — see that module's doc for the
    /// convergence argument.
    pub fn handle(&mut self, msg: SyncMessage) -> Vec<SyncMessage> {
        self.engine.handle(msg)
    }

    /// What chunk a given (now-admitted) node actually refers to — the step
    /// between "the DAG converged" and "go fetch these bytes from
    /// `PhotoSyncDB`'s block store by hash." `None` for an id this DAG
    /// hasn't admitted, or (defensively) for a node whose payload isn't a
    /// `ChunkRef` at all — a `ChunkSync` should never author anything else,
    /// but a corrupt/foreign payload should fail closed, not panic.
    pub fn chunk_ref(&self, node_id: &NodeId) -> Option<ChunkRef> {
        self.engine.dag().get(node_id).and_then(|n: &DagNode| ChunkRef::decode(&n.content_hash))
    }

    /// Every chunk this channel currently knows about — this device's own
    /// plus anything received from the peer — in the DAG's deterministic
    /// causal order. What a caller re-scans after a sync round to find
    /// newly-arrived chunks worth fetching.
    pub fn known_chunks(&self) -> Vec<ChunkRef> {
        self.engine
            .dag()
            .total_order()
            .into_iter()
            .filter_map(|id| self.chunk_ref(&id))
            .collect()
    }

    pub fn delivery(&self) -> &DeliveryTracker {
        &self.delivery
    }

    pub fn delivery_mut(&mut self) -> &mut DeliveryTracker {
        &mut self.delivery
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hashes(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("chunkhash-{i:04}")).collect()
    }

    /// Runs both engines against each other until neither produces any more
    /// messages — the anti-entropy loop `sync.rs`'s own doc describes,
    /// bounded generously so a protocol bug shows up as a test failure
    /// rather than a hang.
    fn converge(a: &mut ChunkSync, b: &mut ChunkSync) {
        let mut to_b = vec![a.announce()];
        let mut to_a: Vec<SyncMessage> = Vec::new();
        for _ in 0..50 {
            if to_b.is_empty() && to_a.is_empty() {
                return;
            }
            let mut next_to_a = Vec::new();
            for m in to_b.drain(..) {
                next_to_a.extend(b.handle(m));
            }
            let mut next_to_b = Vec::new();
            for m in to_a.drain(..) {
                next_to_b.extend(a.handle(m));
            }
            to_a = next_to_a;
            to_b = next_to_b;
        }
        panic!("did not converge within 50 rounds");
    }

    #[test]
    fn a_whole_file_reaches_a_peer_that_had_none_of_it() {
        let mut alice = ChunkSync::new("alice-phone");
        let mut bob = ChunkSync::new("bob-laptop");

        let chunk_hashes = hashes(5);
        let node_ids = alice.announce_file("vacation/img_001.raw", &chunk_hashes, 1);
        alice.record_sent(&node_ids, "bob-laptop");

        assert!(bob.known_chunks().is_empty(), "bob starts with nothing");
        converge(&mut alice, &mut bob);

        let got = bob.known_chunks();
        assert_eq!(got.len(), 5);
        for (i, c) in got.iter().enumerate() {
            assert_eq!(c.path, "vacation/img_001.raw");
            assert_eq!(c.chunk_index, i as u32);
            assert_eq!(c.chunk_hash, chunk_hashes[i]);
        }
    }

    /// Like `converge`, but stops after `rounds` message-passing rounds
    /// even if messages remain in flight — for simulating a connection that
    /// dies partway through a multi-round exchange.
    fn exchange_rounds(a: &mut ChunkSync, b: &mut ChunkSync, rounds: usize) {
        let mut to_b = vec![a.announce()];
        let mut to_a: Vec<SyncMessage> = Vec::new();
        for _ in 0..rounds {
            if to_b.is_empty() && to_a.is_empty() {
                return;
            }
            let mut next_to_a = Vec::new();
            for m in to_b.drain(..) {
                next_to_a.extend(b.handle(m));
            }
            let mut next_to_b = Vec::new();
            for m in to_a.drain(..) {
                next_to_b.extend(a.handle(m));
            }
            to_a = next_to_a;
            to_b = next_to_b;
        }
    }

    #[test]
    fn a_dropped_connection_mid_file_resumes_instead_of_restarting() {
        let mut alice = ChunkSync::new("alice-phone");
        let mut bob = ChunkSync::new("bob-laptop");
        let chunk_hashes = hashes(6);
        alice.announce_file("vacation/img_002.raw", &chunk_hashes, 1);

        // A linear chain resolves one ancestor per round-trip (each node's
        // only missing parent is requested, arrives, reveals the next missing
        // parent, and so on) until the chain bottoms out and the whole run
        // cascade-admits. Two rounds is deliberately short of that — standing
        // in for a connection that died partway through resolving history.
        exchange_rounds(&mut alice, &mut bob, 2);
        let partial = bob.known_chunks().len();
        assert!(partial < 6, "the connection died before the full chain arrived (got {partial})");

        // A fresh connection: both sides drop in-flight bookkeeping for the
        // dead one (see `begin_connection`'s doc — without this the request
        // stuck mid-flight when round 1 died never gets re-asked) but keep
        // every chunk already admitted.
        alice.begin_connection();
        bob.begin_connection();
        converge(&mut alice, &mut bob);
        let got = bob.known_chunks();
        assert_eq!(got.len(), 6, "the rest arrived without redoing round 1's progress");
        assert_eq!(got.iter().map(|c| c.chunk_hash.clone()).collect::<Vec<_>>(), chunk_hashes);
    }

    #[test]
    fn diarization_tracks_the_actual_transport_acks_not_the_dag_state() {
        let mut alice = ChunkSync::new("alice-phone");
        let node_ids = alice.announce_file("vacation/img_003.raw", &hashes(2), 1);
        alice.record_sent(&node_ids, "bob-laptop");

        // Sending is not delivering: DagSync convergence alone shouldn't be
        // read as a receipt (see the module doc — the transport drives this).
        let before = alice.delivery().summary(&node_ids[0]).expect("record_sent registered this entry");
        assert_eq!(before.delivered, 0);

        alice.delivery_mut().record_delivered(&node_ids[0], "bob-laptop");
        let after = alice.delivery().summary(&node_ids[0]).unwrap();
        assert_eq!(after.delivered, 1);
        assert_eq!(after.seen, 0);
    }
}
