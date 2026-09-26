//! node.rs — a DHT node (routing table + value store) and the RPC surface it
//! exposes. The in-memory `sim` harness implements [`Transport`] by calling
//! handlers directly; the real backend will implement it over UDP. Every RPC
//! carries the caller's [`Contact`] so the recipient learns it — in Kademlia,
//! inbound requests populate routing tables.

use crate::routing::{Contact, NodeId, RoutingTable};
use crate::store::{Key, Store};
use std::net::SocketAddr;

/// Result of a FIND_VALUE: either the values (the key lives here) or the closest
/// known nodes (it doesn't — ask these next).
#[derive(Clone, Debug)]
pub enum FindValue {
    Values(Vec<Vec<u8>>),
    Closest(Vec<Contact>),
}

/// The RPC surface. A lookup driver is generic over this, so the same iterative
/// logic runs over the in-memory harness and over real UDP.
pub trait Transport {
    fn find_node(&self, from: &Contact, to: &Contact, target: &NodeId) -> Vec<Contact>;
    fn find_value(&self, from: &Contact, to: &Contact, key: &Key) -> FindValue;
    fn store(&self, from: &Contact, to: &Contact, key: Key, value: Vec<u8>, ttl: u64);
    fn ping(&self, from: &Contact, to: &Contact) -> bool;

    // ── The checked surface. Defaults wrap the four above, so a transport that
    //    cannot tell "no answer" from "an empty answer" (the in-memory sims)
    //    keeps working unchanged; a real network transport overrides them. ──

    /// `find_node`, with `None` when the peer did not answer at all. The lookup
    /// driver needs the difference: an empty answer is a live node that knows
    /// nobody closer, and no answer is a strike against that node. The plain
    /// method returns an empty list for both, and the driver used to count a
    /// timeout as the node having responded -- dead nodes never aged out.
    fn query_nodes(&self, from: &Contact, to: &Contact, target: &NodeId) -> Option<Vec<Contact>> {
        Some(self.find_node(from, to, target))
    }

    /// `find_value`, with `None` for no answer. See [`Transport::query_nodes`].
    fn query_value(&self, from: &Contact, to: &Contact, key: &Key) -> Option<FindValue> {
        Some(self.find_value(from, to, key))
    }

    /// `store`, reporting whether the peer ACCEPTED it. `store` is fire and
    /// forget, so a publish could only count the peers it asked, and a publish
    /// every peer refused looked like a success.
    fn store_acked(&self, from: &Contact, to: &Contact, key: Key, value: Vec<u8>, ttl: u64) -> bool {
        self.store(from, to, key, value, ttl);
        true
    }

    /// One round of a lookup: ask each of `to`. Sequential by default; a
    /// transport whose round trips can overlap (the served UDP socket) asks them
    /// concurrently, as OpenDHT does (`MAX_REQUESTED_SEARCH_NODES`), so a round
    /// costs one timeout rather than one per dead node.
    fn query_nodes_many(&self, from: &Contact, to: &[Contact], target: &NodeId) -> Vec<Option<Vec<Contact>>> {
        to.iter().map(|c| self.query_nodes(from, c, target)).collect()
    }

    /// [`Transport::query_nodes_many`] for `find_value`.
    fn query_value_many(&self, from: &Contact, to: &[Contact], key: &Key) -> Vec<Option<FindValue>> {
        to.iter().map(|c| self.query_value(from, c, key)).collect()
    }
}

/// A DHT node.
#[derive(Clone, Debug)]
pub struct Node {
    pub rt: RoutingTable,
    pub store: Store,
    pub addr: SocketAddr,
}

impl Node {
    pub fn new(id: NodeId, addr: SocketAddr) -> Self {
        Self { rt: RoutingTable::new(id), store: Store::new(), addr }
    }

    pub fn id(&self) -> NodeId {
        *self.rt.local()
    }

    pub fn contact(&self, now: u64) -> Contact {
        Contact::new(self.id(), self.addr, now)
    }

    fn learn(&mut self, from: &Contact, now: u64) {
        self.rt.saw(from.id, from.addr, now);
    }

    // ── handlers (each learns the caller, then serves) ──────────────────────

    pub fn handle_find_node(&mut self, from: &Contact, target: &NodeId, now: u64) -> Vec<Contact> {
        self.learn(from, now);
        // A lookup landing in a bucket is that bucket being exercised, which is
        // exactly what "stale" means the absence of. Recording it here rather
        // than in a refresh job means ordinary traffic keeps buckets fresh and
        // maintenance only covers what traffic missed — the whole point of
        // measuring staleness instead of refreshing on a timer.
        self.rt.note_query(target, now);
        self.rt.closest(target, self.rt.params().k)
    }

    pub fn handle_find_value(&mut self, from: &Contact, key: &Key, now: u64) -> FindValue {
        self.learn(from, now);
        self.rt.note_query(key, now);
        let values = self.store.get(key, now);
        if values.is_empty() {
            FindValue::Closest(self.rt.closest(key, self.rt.params().k))
        } else {
            FindValue::Values(values)
        }
    }

    pub fn handle_store(&mut self, from: &Contact, key: Key, value: Vec<u8>, ttl: u64, now: u64) {
        self.learn(from, now);
        self.store.put(key, value, ttl, now);
    }

    pub fn handle_ping(&mut self, from: &Contact, now: u64) -> bool {
        self.learn(from, now);
        true
    }
}

/// Maintenance: what this node should do next, and when.
///
/// `schedule::plan` has existed with eleven tests and **no caller** — a policy
/// nothing consulted. Meanwhile the wake-kit gating went into
/// `snapclone-dht`'s engine, which is the weaker of the two for this job:
/// wake-kit knows Interactive/Background/Opportunistic across arbitrary app
/// work, while this scheduler knows *which bucket* went stale, *which values*
/// are durable, and whether the device is on battery. Wiring the better one is
/// the correction.
impl Node {
    /// Per-bucket last-query times, for the scheduler.
    ///
    /// Zero means "never queried", which the scheduler treats as *not stale* —
    /// a fresh node has 160 empty buckets and refreshing all of them on first
    /// boot is the worst possible opening move for a battery.
    pub fn bucket_activity(&self) -> [u64; crate::routing::N_BUCKETS] {
        let mut out = [0u64; crate::routing::N_BUCKETS];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.rt.bucket_last_query(i);
        }
        out
    }

    /// Plan the next maintenance window, or `None` when there is nothing to do.
    ///
    /// Returning `None` rather than an empty window is the load-bearing part:
    /// the caller must not wake for an empty window, and a `Window` with no
    /// jobs is exactly the shape that gets scheduled anyway.
    pub fn next_maintenance(
        &self,
        now: u64,
        regime: crate::schedule::Regime,
        values: &[(u64, bool)],
    ) -> Option<crate::schedule::Window> {
        crate::schedule::plan(&self.rt.local(), now, regime, &self.bucket_activity(), values)
    }
}

#[cfg(test)]
mod maintenance_tests {
    use super::*;
    use crate::schedule::{Job, Regime};

    fn node(seed: u8) -> Node {
        let mut b = [0u8; crate::routing::ID_LEN];
        b[0] = seed;
        Node::new(NodeId(b), "127.0.0.1:4222".parse().unwrap())
    }

    fn peer(seed: u8) -> Contact {
        let mut b = [0u8; crate::routing::ID_LEN];
        b[0] = seed;
        b[19] = seed ^ 0xFF;
        Contact { id: NodeId(b), addr: "10.0.0.9:4222".parse().unwrap(),
                  last_seen: 0, failed: 0 }
    }

    #[test]
    fn a_fresh_node_has_nothing_to_do() {
        // 160 empty buckets, none ever queried. Refreshing them all on first
        // boot is the worst possible opening move for a battery, so the plan
        // must be None rather than an enormous window.
        let n = node(1);
        assert!(n.next_maintenance(10_000, Regime::Constrained, &[]).is_none());
    }

    #[test]
    fn ordinary_traffic_keeps_a_bucket_fresh() {
        // Staleness is measured, not assumed. A bucket exercised by real
        // lookups should never need a refresh job, which is the saving that
        // measuring buys over refreshing on a timer.
        let mut n = node(2);
        let target = peer(9).id;
        n.handle_find_node(&peer(3), &target, 1_000);
        let plan = n.next_maintenance(1_000 + 10, Regime::Constrained, &[]);
        assert!(plan.is_none(), "a just-queried bucket is not stale");
    }

    #[test]
    fn a_bucket_that_goes_quiet_earns_a_refresh() {
        let mut n = node(3);
        let target = peer(9).id;
        n.handle_find_node(&peer(4), &target, 1_000);
        let plan = n.next_maintenance(1_000 + crate::schedule::BUCKET_STALE * 3,
                                      Regime::Constrained, &[])
            .expect("a quiet bucket should be planned");
        assert!(plan.jobs.iter().any(|j| matches!(j, Job::BucketRefresh(_))));
    }

    #[test]
    fn an_ephemeral_value_is_not_kept_alive() {
        // Re-announcing something meant to disappear spends battery to defeat
        // the user's own choice.
        let n = node(4);
        let soon = 100u64;
        assert!(n.next_maintenance(50, Regime::Constrained, &[(soon, false)]).is_none());
    }

    #[test]
    fn a_durable_value_near_its_ttl_is_re_announced() {
        let n = node(5);
        let expiry = 10_000u64;
        let plan = n.next_maintenance(expiry - 10, Regime::Constrained, &[(expiry, true)])
            .expect("a durable value about to lapse must be planned");
        assert!(plan.jobs.iter().any(|j| matches!(j, Job::ReAnnounce)));
    }

    #[test]
    fn a_relaxed_device_may_run_the_window_immediately() {
        // The id-derived slot exists to spread RADIO wake-ups. On wifi or
        // plugged in there is no wake-up to spread, so waiting for the slot
        // buys nothing and costs latency.
        let n = node(6);
        let expiry = 10_000u64;
        let now = expiry - 10;
        let relaxed = n.next_maintenance(now, Regime::Relaxed, &[(expiry, true)]).unwrap();
        let constrained = n.next_maintenance(now, Regime::Constrained, &[(expiry, true)]).unwrap();
        assert_eq!(relaxed.at, now);
        assert!(constrained.at >= now, "constrained waits for its slot");
    }

    #[test]
    fn an_out_of_range_bucket_index_reads_as_never_rather_than_panicking() {
        // The activity array is N_BUCKETS long and the table may be shorter.
        let n = node(7);
        assert_eq!(n.rt.bucket_last_query(crate::routing::N_BUCKETS + 50), 0);
        assert_eq!(n.bucket_activity().len(), crate::routing::N_BUCKETS);
    }
}
