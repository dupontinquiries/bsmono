//! `sim::net::mp_dht_mailbox` — run the join mailbox on the real DHT model.
//!
//! The join handshake in [`dht_mailbox`](crate::sim::net::dht_mailbox) is written
//! against the [`MailboxDht`] trait; the only implementation so far is `LocalDht`,
//! an unbounded in-test `HashMap`. This backs the same trait with
//! [`mp_dht::Mailboxes`](crate::sim::mp_dht::Mailboxes) — the project's Kademlia
//! model with per-recipient **bounded** FIFO boxes (a full box applies
//! backpressure) — so the handshake runs on the routing-aware DHT instead of the
//! test stand-in. A `MailboxKey` is projected to a `NodeId` (its first FNV lane),
//! the key space `mp_dht` actually routes on; the bin maps `NodeId → socket`.
//!
//! Behaviourally a drop-in for `LocalDht` on the non-full path: `put` stores,
//! `get` drains in arrival order, distinct keys don't cross-talk. The added
//! property is the bound — a flooded mailbox rejects rather than growing without
//! limit.

use crate::sim::mp_dht::{Mail, Mailboxes, NodeId};
use crate::sim::net::dht_mailbox::{MailboxDht, MailboxKey};
use std::sync::Mutex;

/// `topic` stamped on join-handshake mail (mailbox blobs are opaque to mp_dht;
/// the tag just lets a real node demux mailbox traffic from other DHT topics).
pub const TOPIC_JOIN: u16 = 0x4A4E; // "JN"

/// Project a 32-byte mailbox key onto mp_dht's 64-bit node-id space. The first
/// lane of `mailbox_key`'s FNV expansion is already a full hash of the id, so the
/// low 8 bytes are a sound, deterministic key.
pub fn key_to_node(key: MailboxKey) -> NodeId {
    let mut b = [0u8; 8];
    b.copy_from_slice(&key.0[0..8]);
    u64::from_le_bytes(b)
}

/// `MailboxDht` over a bounded `mp_dht::Mailboxes`. Interior-mutable (the trait is
/// `&self`) via a `Mutex`, matching `LocalDht`.
pub struct MpDhtMailbox {
    boxes: Mutex<Mailboxes>,
    local: NodeId,
}

impl MpDhtMailbox {
    /// `local` is this node's id (used as `Mail.from`); `max_per_box` bounds each
    /// recipient's queue.
    pub fn new(local: NodeId, max_per_box: usize) -> MpDhtMailbox {
        MpDhtMailbox { boxes: Mutex::new(Mailboxes::new(max_per_box)), local }
    }

    /// Pending count for a key (for tests / backpressure introspection).
    pub fn pending(&self, key: MailboxKey) -> usize {
        self.boxes.lock().map(|b| b.pending(key_to_node(key))).unwrap_or(0)
    }
}

impl MailboxDht for MpDhtMailbox {
    fn put(&self, key: MailboxKey, msg: Vec<u8>) {
        if let Ok(mut b) = self.boxes.lock() {
            // A full box returns false → the message is dropped (backpressure);
            // the requester retries on its next poll, as with a real DHT node.
            let _ = b.put(Mail {
                to: key_to_node(key),
                from: self.local,
                topic: TOPIC_JOIN,
                body: msg,
                t: 0,
            });
        }
    }

    fn get(&self, key: MailboxKey) -> Vec<Vec<u8>> {
        match self.boxes.lock() {
            Ok(mut b) => b.fetch(key_to_node(key)).into_iter().map(|m| m.body).collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte0: u8) -> MailboxKey {
        let mut k = [0u8; 32];
        k[0] = byte0;
        k[7] = 0xAB; // ensure the low-8 projection varies with byte0
        MailboxKey(k)
    }

    #[test]
    fn put_then_get_drains_in_order() {
        let dht = MpDhtMailbox::new(1, 16);
        let k = key(1);
        dht.put(k, b"first".to_vec());
        dht.put(k, b"second".to_vec());
        let got = dht.get(k);
        assert_eq!(got, vec![b"first".to_vec(), b"second".to_vec()]);
        // draining empties the box
        assert!(dht.get(k).is_empty());
    }

    #[test]
    fn distinct_keys_do_not_cross_talk() {
        let dht = MpDhtMailbox::new(1, 16);
        dht.put(key(1), b"for-1".to_vec());
        dht.put(key(2), b"for-2".to_vec());
        assert_eq!(dht.get(key(1)), vec![b"for-1".to_vec()]);
        assert_eq!(dht.get(key(2)), vec![b"for-2".to_vec()]);
    }

    #[test]
    fn key_projection_is_deterministic_and_distinguishing() {
        assert_eq!(key_to_node(key(5)), key_to_node(key(5)));
        assert_ne!(key_to_node(key(5)), key_to_node(key(6)));
    }

    #[test]
    fn box_is_bounded_full_rejects() {
        let dht = MpDhtMailbox::new(1, 2); // cap 2
        let k = key(9);
        dht.put(k, b"a".to_vec());
        dht.put(k, b"b".to_vec());
        dht.put(k, b"c".to_vec()); // dropped — box full
        assert_eq!(dht.pending(k), 2);
        assert_eq!(dht.get(k), vec![b"a".to_vec(), b"b".to_vec()]);
    }

    #[test]
    fn matches_localdht_contract_on_a_sequence() {
        // The handshake is tested against LocalDht; assert this backend produces
        // the same observable put/get behaviour on the non-full path.
        use std::collections::HashMap;
        let mp = MpDhtMailbox::new(0, 64);
        let mut local: HashMap<NodeId, Vec<Vec<u8>>> = HashMap::new();

        let ops: &[(u8, &[u8])] = &[(1, b"x"), (2, b"y"), (1, b"z")];
        for &(kb, payload) in ops {
            let k = key(kb);
            mp.put(k, payload.to_vec());
            local.entry(key_to_node(k)).or_default().push(payload.to_vec());
        }
        for kb in [1u8, 2, 3] {
            let k = key(kb);
            let mp_got = mp.get(k);
            let local_got = local.remove(&key_to_node(k)).unwrap_or_default();
            assert_eq!(mp_got, local_got, "key {kb} drains identically to LocalDht");
        }
    }
}
