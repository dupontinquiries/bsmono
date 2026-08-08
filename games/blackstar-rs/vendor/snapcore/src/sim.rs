//! sim.rs — an in-memory network of nodes for deterministic, offline,
//! multi-node testing (and the seed of the net-sim CI harness). A message from
//! one node to another is a direct call into the target's handler, so iterative
//! lookups traverse real routing tables across many nodes with no sockets and no
//! clock — exactly the determinism `microdht` was built to give us.

use crate::node::{FindValue, Node, Transport};
use crate::routing::{Contact, NodeId};
use crate::store::Key;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A registry of nodes addressable by id.
#[derive(Default)]
pub struct Network {
    pub nodes: HashMap<NodeId, Rc<RefCell<Node>>>,
}

impl Network {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, node: Node) -> NodeId {
        let id = node.id();
        self.nodes.insert(id, Rc::new(RefCell::new(node)));
        id
    }

    pub fn get(&self, id: &NodeId) -> Rc<RefCell<Node>> {
        self.nodes.get(id).expect("unknown node id").clone()
    }

    /// One-shot bidirectional acquaintance, as if the two had exchanged a ping.
    pub fn introduce(&self, a: &NodeId, b: &NodeId, now: u64) {
        let (na, nb) = (self.get(a), self.get(b));
        let (ca, cb) = (na.borrow().contact(now), nb.borrow().contact(now));
        na.borrow_mut().rt.saw(cb.id, cb.addr, now);
        nb.borrow_mut().rt.saw(ca.id, ca.addr, now);
    }
}

/// A [`Transport`] backed by a [`Network`], delivering RPCs in-process at logical
/// time `now`. Never targets the caller, so no node is borrowed twice.
pub struct SimTransport<'a> {
    pub net: &'a Network,
    pub now: u64,
}

impl Transport for SimTransport<'_> {
    fn find_node(&self, from: &Contact, to: &Contact, target: &NodeId) -> Vec<Contact> {
        self.net
            .nodes
            .get(&to.id)
            .map(|n| n.borrow_mut().handle_find_node(from, target, self.now))
            .unwrap_or_default()
    }

    fn find_value(&self, from: &Contact, to: &Contact, key: &Key) -> FindValue {
        self.net
            .nodes
            .get(&to.id)
            .map(|n| n.borrow_mut().handle_find_value(from, key, self.now))
            .unwrap_or(FindValue::Closest(Vec::new()))
    }

    fn store(&self, from: &Contact, to: &Contact, key: Key, value: Vec<u8>, ttl: u64) {
        if let Some(n) = self.net.nodes.get(&to.id) {
            n.borrow_mut().handle_store(from, key, value, ttl, self.now);
        }
    }

    fn ping(&self, from: &Contact, to: &Contact) -> bool {
        self.net
            .nodes
            .get(&to.id)
            .map(|n| n.borrow_mut().handle_ping(from, self.now))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lookup::{lookup_nodes, lookup_value};
    use crate::routing::ID_LEN;

    fn node_id(i: u8) -> NodeId {
        // Spread ids across the keyspace so buckets actually differ.
        let mut b = [0u8; ID_LEN];
        b[0] = i.wrapping_mul(37).wrapping_add(11);
        b[1] = i.wrapping_mul(101);
        b[2] = i;
        NodeId(b)
    }

    fn key_id(seed: u8) -> Key {
        let mut b = [0u8; ID_LEN];
        b[0] = seed;
        b[1] = seed.wrapping_mul(3).wrapping_add(7);
        b[2] = 0xAA;
        NodeId(b)
    }

    /// Build N nodes that all know one bootstrap node (and the bootstrap knows
    /// all) — the realistic cold-join shape: everyone has one entry point.
    fn bootstrapped_net(n: u8, now: u64) -> (Network, Vec<NodeId>) {
        let mut net = Network::new();
        let mut ids = Vec::new();
        for i in 0..n {
            let id = net.add(Node::new(node_id(i), format!("127.0.0.1:{}", 1000 + i as u16).parse().unwrap()));
            ids.push(id);
        }
        let boot = ids[0];
        for &id in &ids[1..] {
            net.introduce(&boot, &id, now);
        }
        (net, ids)
    }

    /// The real Kademlia join step: every node performs a self-lookup, which both
    /// fills its own routing table with id-space neighbours and (because FIND_NODE
    /// carries the caller) teaches those neighbours about it. After this the star
    /// has become a properly interconnected network.
    fn warm_up(net: &Network, ids: &[NodeId], now: u64) {
        for &id in ids {
            let rc = net.get(&id);
            let me = rc.borrow().contact(now);
            let tx = SimTransport { net, now };
            let mut node = rc.borrow_mut();
            lookup_nodes(&me, &mut node.rt, &id, &tx, 3, now);
        }
    }

    #[test]
    fn lookup_nodes_converges_to_true_closest() {
        let now = 1;
        let (net, ids) = bootstrapped_net(60, now);
        warm_up(&net, &ids, now); // real Kademlia join makes the network connected
        let target = key_id(200);

        // A node looks up the target after the network has interconnected.
        let seeker = ids[20];
        let seeker_rc = net.get(&seeker);
        let me = seeker_rc.borrow().contact(now);
        let found = {
            let tx = SimTransport { net: &net, now };
            let mut s = seeker_rc.borrow_mut();
            lookup_nodes(&me, &mut s.rt, &target, &tx, 3, now)
        };

        // Ground truth: the k closest ids to target among all nodes EXCEPT the
        // seeker itself (a node's lookup returns other nodes, never itself).
        let k = seeker_rc.borrow().rt.params().k;
        let mut others: Vec<NodeId> = ids.iter().cloned().filter(|id| *id != seeker).collect();
        others.sort_by(|a, b| a.distance(&target).cmp(&b.distance(&target)));
        let truth: Vec<NodeId> = others.into_iter().take(k).collect();
        let got: Vec<NodeId> = found.iter().map(|c| c.id).collect();

        assert_eq!(got.first(), truth.first(), "closest node differs from ground truth");
        for t in &truth {
            assert!(got.contains(t), "lookup missed a true-closest node {t:?}");
        }
    }

    #[test]
    fn store_on_one_node_is_findable_from_another() {
        let now = 1;
        let (net, ids) = bootstrapped_net(60, now);
        let key = key_id(200);
        let payload = b"the-shared-blob".to_vec();

        // Storer finds the k closest to the key and STOREs on them.
        let storer_rc = net.get(&ids[5]);
        let me_store = storer_rc.borrow().contact(now);
        {
            let tx = SimTransport { net: &net, now };
            let closest = {
                let mut sn = storer_rc.borrow_mut();
                lookup_nodes(&me_store, &mut sn.rt, &key, &tx, 3, now)
            };
            assert!(!closest.is_empty());
            for c in &closest {
                tx.store(&me_store, c, key, payload.clone(), 10_000);
            }
        }

        // A different node, knowing only the bootstrap, retrieves it by routing.
        let seeker_rc = net.get(&ids[42]);
        let me_seek = seeker_rc.borrow().contact(now);
        let found = {
            let tx = SimTransport { net: &net, now };
            let mut sk = seeker_rc.borrow_mut();
            lookup_value(&me_seek, &mut sk.rt, &key, &tx, 3, now)
        };
        assert_eq!(found, Some(vec![payload]), "value not found across nodes");
    }

    #[test]
    fn lookup_value_returns_none_when_absent() {
        let now = 1;
        let (net, ids) = bootstrapped_net(40, now);
        let seeker_rc = net.get(&ids[10]);
        let me = seeker_rc.borrow().contact(now);
        let found = {
            let tx = SimTransport { net: &net, now };
            let mut sk = seeker_rc.borrow_mut();
            lookup_value(&me, &mut sk.rt, &key_id(123), &tx, 3, now)
        };
        assert_eq!(found, None);
    }

    #[test]
    fn lookup_grows_the_callers_routing_table() {
        let now = 1;
        let (net, ids) = bootstrapped_net(50, now);
        let seeker_rc = net.get(&ids[33]);
        let before = seeker_rc.borrow().rt.len();
        let me = seeker_rc.borrow().contact(now);
        {
            let tx = SimTransport { net: &net, now };
            let mut sk = seeker_rc.borrow_mut();
            lookup_nodes(&me, &mut sk.rt, &key_id(77), &tx, 3, now);
        }
        let after = seeker_rc.borrow().rt.len();
        assert!(after > before, "routing table did not grow from {before} via lookup");
    }
}
