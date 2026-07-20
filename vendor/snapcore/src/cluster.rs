//! cluster.rs — the explicit-cluster overlay: smaller connected graphs with
//! id-mapping, for resilience.
//!
//! A cluster is something you JOIN (a shared mailbox, a group, a LAN-affinity
//! set), named by a [`ClusterId`] — a point in the keyspace, so the cluster's
//! member roster can live in the global DHT under that key. Within a cluster you
//! track co-members **densely**, regardless of XOR distance, so a node that's
//! sparse and flaky in the global Kademlia table is still a well-connected,
//! reliable member of its small graphs. A node that belongs to two clusters
//! bridges them — that's the id-mapping across graphs.
//!
//! Two resilience primitives sit on top of the DHT:
//!   * [`lookup_value_resilient`] — ask densely-known co-members directly first
//!     (the small connected graph, likely reachable even when the global path is
//!     degraded), falling back to the global Kademlia walk only if none have it;
//!   * [`replicate_to_cluster`] — push a value to co-members so the cluster
//!     collectively holds it (durability-through-population).
//!
//! The roster itself is bootstrapped through the DHT: [`announce`] stores your
//! contact under the cluster key; [`discover`] reads it back. Members coexist
//! under one key via the store's multi-value-per-key.

use crate::lookup::{lookup_nodes, lookup_value};
use crate::node::{FindValue, Transport};
use crate::routing::{Contact, Liveness, NodeId, Params, RoutingTable, ID_LEN};
use crate::store::Key;
use std::collections::HashMap;

/// A cluster identifier — a keyspace point, so its roster lives in the DHT.
pub type ClusterId = NodeId;

#[derive(Clone, Copy, Debug)]
pub struct ClusterParams {
    pub max_members: usize,
    pub questionable_after: u64,
    pub bad_after_failures: u32,
}

impl Default for ClusterParams {
    fn default() -> Self {
        Self { max_members: 256, questionable_after: 15 * 60_000, bad_after_failures: 3 }
    }
}

impl ClusterParams {
    fn liveness_params(&self) -> Params {
        Params {
            k: self.max_members,
            questionable_after: self.questionable_after,
            bad_after_failures: self.bad_after_failures,
        }
    }
}

/// The densely-tracked members of one cluster — a flat set, no XOR filtering.
#[derive(Clone, Debug, Default)]
struct MemberSet {
    members: Vec<Contact>,
}

impl MemberSet {
    fn record(&mut self, c: Contact, now: u64, p: &ClusterParams) {
        if let Some(i) = self.members.iter().position(|m| m.id == c.id) {
            let mut m = self.members.remove(i);
            m.last_seen = now;
            m.failed = 0;
            m.addr = c.addr;
            self.members.push(m);
            return;
        }
        if self.members.len() >= p.max_members {
            let lp = p.liveness_params();
            let evict = self
                .members
                .iter()
                .position(|m| m.liveness(now, &lp) == Liveness::Bad)
                .unwrap_or(0);
            self.members.remove(evict);
        }
        self.members.push(c);
    }

    fn failed(&mut self, id: &NodeId) {
        if let Some(m) = self.members.iter_mut().find(|m| &m.id == id) {
            m.failed += 1;
        }
    }

    fn live(&self, now: u64, p: &ClusterParams) -> Vec<Contact> {
        let lp = p.liveness_params();
        self.members
            .iter()
            .filter(|m| m.liveness(now, &lp) != Liveness::Bad)
            .cloned()
            .collect()
    }
}

/// Every cluster this node belongs to, with its densely-tracked roster.
#[derive(Clone, Debug)]
pub struct Clusters {
    local: NodeId,
    sets: HashMap<ClusterId, MemberSet>,
    params: ClusterParams,
}

impl Clusters {
    pub fn new(local: NodeId) -> Self {
        Self::with_params(local, ClusterParams::default())
    }

    pub fn with_params(local: NodeId, params: ClusterParams) -> Self {
        Self { local, sets: HashMap::new(), params }
    }

    pub fn join(&mut self, cluster: ClusterId) {
        self.sets.entry(cluster).or_default();
    }

    pub fn leave(&mut self, cluster: &ClusterId) {
        self.sets.remove(cluster);
    }

    pub fn is_member(&self, cluster: &ClusterId) -> bool {
        self.sets.contains_key(cluster)
    }

    /// The clusters this node belongs to — i.e. the clusters it bridges.
    pub fn clusters(&self) -> Vec<ClusterId> {
        self.sets.keys().copied().collect()
    }

    /// Learn a co-member of a cluster (auto-joins the cluster if new). Ignores
    /// our own id.
    pub fn record_member(&mut self, cluster: ClusterId, contact: Contact, now: u64) {
        if contact.id == self.local {
            return;
        }
        let p = self.params;
        self.sets.entry(cluster).or_default().record(contact, now, &p);
    }

    pub fn member_failed(&mut self, cluster: &ClusterId, id: &NodeId) {
        if let Some(set) = self.sets.get_mut(cluster) {
            set.failed(id);
        }
    }

    /// Live co-members of a cluster.
    pub fn members(&self, cluster: &ClusterId, now: u64) -> Vec<Contact> {
        self.sets.get(cluster).map(|s| s.live(now, &self.params)).unwrap_or_default()
    }

    /// Clusters `node` shares with us — the bridge relation that lets a
    /// multi-cluster node route between small graphs.
    pub fn shared_with(&self, node: &NodeId, now: u64) -> Vec<ClusterId> {
        self.sets
            .iter()
            .filter(|(_, set)| set.live(now, &self.params).iter().any(|c| &c.id == node))
            .map(|(id, _)| *id)
            .collect()
    }
}

// ── wire: a contact as a DHT value (id ++ addr-string), no external deps ──────

fn encode_contact(c: &Contact) -> Vec<u8> {
    let mut v = Vec::with_capacity(ID_LEN + 24);
    v.extend_from_slice(&c.id.0);
    v.extend_from_slice(c.addr.to_string().as_bytes());
    v
}

fn decode_contact(bytes: &[u8], now: u64) -> Option<Contact> {
    if bytes.len() <= ID_LEN {
        return None;
    }
    let mut id = [0u8; ID_LEN];
    id.copy_from_slice(&bytes[..ID_LEN]);
    let addr = std::str::from_utf8(&bytes[ID_LEN..]).ok()?.parse().ok()?;
    Some(Contact::new(NodeId(id), addr, now))
}

// ── DHT-backed roster (the id-mapping) ───────────────────────────────────────

/// Announce our presence in a cluster: store our contact under the cluster key on
/// the DHT nodes closest to it, so other members can discover us. Members coexist
/// under the one key via multi-value-per-key.
pub fn announce<T: Transport>(
    me: &Contact,
    rt: &mut RoutingTable,
    cluster: &ClusterId,
    tx: &T,
    ttl: u64,
    alpha: usize,
    now: u64,
) {
    let targets = lookup_nodes(me, rt, cluster, tx, alpha, now);
    let payload = encode_contact(me);
    for t in &targets {
        tx.store(me, t, *cluster, payload.clone(), ttl);
    }
}

/// Discover a cluster's roster from the DHT and fold it into local membership.
/// Returns how many distinct co-members were learned.
pub fn discover<T: Transport>(
    me: &Contact,
    rt: &mut RoutingTable,
    clusters: &mut Clusters,
    cluster: &ClusterId,
    tx: &T,
    alpha: usize,
    now: u64,
) -> usize {
    clusters.join(*cluster);
    let mut learned = 0;
    if let Some(values) = lookup_value(me, rt, cluster, tx, alpha, now) {
        for v in values {
            if let Some(c) = decode_contact(&v, now) {
                if c.id != me.id {
                    clusters.record_member(*cluster, c, now);
                    learned += 1;
                }
            }
        }
    }
    learned
}

// ── resilience primitives ────────────────────────────────────────────────────

/// Resilient value lookup: ask densely-known co-members of `cluster` directly
/// first — the small connected graph stays reachable even when the global DHT
/// path is degraded — and only fall back to the global Kademlia walk if none of
/// them have it.
pub fn lookup_value_resilient<T: Transport>(
    me: &Contact,
    rt: &mut RoutingTable,
    clusters: &Clusters,
    cluster: &ClusterId,
    key: &Key,
    tx: &T,
    alpha: usize,
    now: u64,
) -> Option<Vec<Vec<u8>>> {
    for c in clusters.members(cluster, now) {
        if c.id == me.id {
            continue;
        }
        if let FindValue::Values(v) = tx.find_value(me, &c, key) {
            return Some(v);
        }
    }
    lookup_value(me, rt, key, tx, alpha, now)
}

/// Replicate a value to densely-known co-members so the cluster collectively
/// holds it — durability-through-population, resilient to the global DHT thinning.
pub fn replicate_to_cluster<T: Transport>(
    me: &Contact,
    clusters: &Clusters,
    cluster: &ClusterId,
    key: Key,
    value: Vec<u8>,
    ttl: u64,
    now: u64,
    tx: &T,
) {
    for c in clusters.members(cluster, now) {
        if c.id == me.id {
            continue;
        }
        tx.store(me, &c, key, value.clone(), ttl);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Node;
    use crate::sim::{Network, SimTransport};

    fn nid(b: u8) -> NodeId {
        NodeId([b; ID_LEN])
    }

    fn member_node(net: &mut Network, b: u8, port: u16) -> NodeId {
        net.add(Node::new(nid(b), format!("127.0.0.1:{port}").parse().unwrap()))
    }

    #[test]
    fn join_leave_and_membership() {
        let mut c = Clusters::new(nid(1));
        let cl = nid(0xCC);
        assert!(!c.is_member(&cl));
        c.join(cl);
        assert!(c.is_member(&cl));
        assert_eq!(c.clusters(), vec![cl]);
        c.leave(&cl);
        assert!(!c.is_member(&cl));
    }

    #[test]
    fn record_member_dedups_and_ignores_self() {
        let me = nid(1);
        let mut c = Clusters::new(me);
        let cl = nid(0xCC);
        let addr = "127.0.0.1:9".parse().unwrap();
        c.record_member(cl, Contact::new(nid(2), addr, 0), 0);
        c.record_member(cl, Contact::new(nid(2), addr, 5), 5); // same id -> refresh
        c.record_member(cl, Contact::new(me, addr, 0), 0); // self -> ignored
        assert_eq!(c.members(&cl, 10).len(), 1);
    }

    #[test]
    fn shared_with_reports_bridged_clusters() {
        let mut c = Clusters::new(nid(1));
        let (a, b) = (nid(0xA0), nid(0xB0));
        let peer = nid(7);
        let addr = "127.0.0.1:9".parse().unwrap();
        c.record_member(a, Contact::new(peer, addr, 0), 0);
        c.record_member(b, Contact::new(peer, addr, 0), 0);
        c.record_member(a, Contact::new(nid(8), addr, 0), 0); // only in a
        let mut shared = c.shared_with(&peer, 10);
        shared.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(shared, vec![a, b]); // peer bridges both
        assert_eq!(c.shared_with(&nid(8), 10), vec![a]); // only a
    }

    /// THE headline resilience property: a node whose global routing table is
    /// empty (its global path is dead) still reaches a value through its densely
    /// tracked cluster co-members.
    #[test]
    fn cluster_path_finds_value_when_global_path_is_dead() {
        let now = 1;
        let mut net = Network::new();
        let members: Vec<NodeId> = (0..5u8).map(|i| member_node(&mut net, i + 1, 2000 + i as u16)).collect();
        let cluster = nid(0xCC);
        let key = nid(0xAB);

        // A holder co-member has the value.
        net.get(&members[0]).borrow_mut().store.put(key, b"resilient".to_vec(), 10_000, now);

        // Seeker has an EMPTY global routing table but knows its co-members.
        let seeker = members[4];
        let seeker_rc = net.get(&seeker);
        let mut clusters = Clusters::new(seeker);
        clusters.join(cluster);
        for &m in &members[..4] {
            let mc = net.get(&m).borrow().contact(now);
            clusters.record_member(cluster, mc, now);
        }
        assert!(seeker_rc.borrow().rt.is_empty(), "global table must be empty for this test");

        let me = seeker_rc.borrow().contact(now);
        let tx = SimTransport { net: &net, now };

        // Global-only lookup fails — there is no routing knowledge.
        {
            let mut s = seeker_rc.borrow_mut();
            assert_eq!(lookup_value(&me, &mut s.rt, &key, &tx, 3, now), None, "global path should be dead");
        }
        // Cluster-first lookup succeeds via co-members.
        {
            let mut s = seeker_rc.borrow_mut();
            let got = lookup_value_resilient(&me, &mut s.rt, &clusters, &cluster, &key, &tx, 3, now);
            assert_eq!(got, Some(vec![b"resilient".to_vec()]), "cluster path should find the value");
        }
    }

    #[test]
    fn replicate_to_cluster_distributes_to_co_members() {
        let now = 1;
        let mut net = Network::new();
        let members: Vec<NodeId> = (0..4u8).map(|i| member_node(&mut net, i + 1, 3000 + i as u16)).collect();
        let cluster = nid(0xCC);
        let key = nid(0xAB);

        let replicator = members[0];
        let mut clusters = Clusters::new(replicator);
        clusters.join(cluster);
        for &m in &members[1..] {
            let mc = net.get(&m).borrow().contact(now);
            clusters.record_member(cluster, mc, now);
        }
        let me = net.get(&replicator).borrow().contact(now);
        let tx = SimTransport { net: &net, now };
        replicate_to_cluster(&me, &clusters, &cluster, key, b"dur".to_vec(), 10_000, now, &tx);

        for &m in &members[1..] {
            assert_eq!(
                net.get(&m).borrow().store.get(&key, now),
                vec![b"dur".to_vec()],
                "co-member did not receive the replicated value"
            );
        }
    }

    /// The id-mapping loop through the DHT: members announce under the cluster
    /// key, another node discovers the roster from the DHT.
    #[test]
    fn announce_then_discover_via_dht() {
        let now = 1;
        let mut net = Network::new();
        // Small full mesh so the DHT routes deterministically.
        let ids: Vec<NodeId> = (0..8u8).map(|i| member_node(&mut net, i * 17 + 3, 4000 + i as u16)).collect();
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                net.introduce(&ids[i], &ids[j], now);
            }
        }
        let cluster = nid(0x55);

        // Members 0,1,2 announce themselves to the cluster.
        for &m in &ids[0..3] {
            let rc = net.get(&m);
            let me = rc.borrow().contact(now);
            let tx = SimTransport { net: &net, now };
            let mut node = rc.borrow_mut();
            announce(&me, &mut node.rt, &cluster, &tx, 10_000, 3, now);
        }

        // Member 5 discovers the roster from the DHT.
        let seeker = ids[5];
        let seeker_rc = net.get(&seeker);
        let me = seeker_rc.borrow().contact(now);
        let mut clusters = Clusters::new(seeker);
        let learned = {
            let tx = SimTransport { net: &net, now };
            let mut node = seeker_rc.borrow_mut();
            discover(&me, &mut node.rt, &mut clusters, &cluster, &tx, 3, now)
        };
        assert!(learned >= 1, "discovered no members");
        let known: Vec<NodeId> = clusters.members(&cluster, now).iter().map(|c| c.id).collect();
        for &m in &ids[0..3] {
            assert!(known.contains(&m), "discovery missed announced member {m:?}");
        }
    }
}
