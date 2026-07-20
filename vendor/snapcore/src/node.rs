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
        self.rt.closest(target, self.rt.params().k)
    }

    pub fn handle_find_value(&mut self, from: &Contact, key: &Key, now: u64) -> FindValue {
        self.learn(from, now);
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
