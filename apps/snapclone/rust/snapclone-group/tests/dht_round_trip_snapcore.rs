//! The same group round-trip, carried by the FLEET's DHT instead of snapclone's own.
//!
//! `dht_round_trip.rs` proves the group crypto composes with a value layer, using
//! `snapclone-microdht` — a crate only this test still depends on. Meanwhile
//! `shared/snapcore` carries the value layer for weather-share, share-kit,
//! net-kit, games/netcode and bsgames, and its own header says the pieces "were
//! proven in snapclone-microdht and are now all lifted into `store`".
//!
//! So the fleet has two DHTs and snapclone is the only holdout. This test asks
//! whether the shared one can carry the same traffic. It is written ALONGSIDE
//! rather than replacing, because "snapcore can do this" is only credible if
//! both run and agree.
//!
//! ## Two differences from the socket version, and both are improvements
//!
//! * **No socket, no port, no thread.** `snapcore::sim` calls straight into the
//!   target node's handler, so an iterative lookup traverses real routing tables
//!   with no clock and no binding. The socket version spawns a server thread and
//!   binds 127.0.0.1:0, which is a source of CI flakiness that buys nothing the
//!   crypto cares about.
//! * **More than one node.** The socket test PUTs and GETs against a single
//!   node, so it never exercises a lookup at all. Here the value is stored on
//!   the nodes closest to the key and retrieved by a peer that does not hold it,
//!   which is what "the DHT carried it" actually means.

use snapclone_group::{
    entry::{seal_and_sign, verify_and_open, GroupEntryBody, MessageKind},
    group::{feed_bucket, GroupState, Member},
    ratchet::SenderChain,
    seal,
};
use snapcore::lookup::{lookup_nodes, lookup_value};
use snapcore::node::Node as ScNode;
use snapcore::routing::{NodeId, RoutingTable};
use snapcore::sim::{Network, SimTransport};

use ed25519_dalek::SigningKey;
use x25519_dalek::{PublicKey, StaticSecret};

struct DummyRng(u64);
impl rand_core::RngCore for DummyRng {
    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        (self.0 >> 33) as u32
    }
    fn next_u64(&mut self) -> u64 {
        ((self.next_u32() as u64) << 32) | self.next_u32() as u64
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(4) {
            let v = self.next_u32().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}
impl rand_core::CryptoRng for DummyRng {}

/// The group's feed bucket is a hex SHA-256; snapcore's Key is a fixed-width id.
/// Taking the leading bytes is the right mapping and not a shortcut: the bucket
/// is already a uniform hash, so its prefix is uniform too, and truncating a
/// hash is how every Kademlia maps an application key onto its id space.
fn bucket_key(bucket: &str) -> NodeId {
    let raw = hex::decode(bucket).expect("feed_bucket returns hex");
    let mut id = [0u8; 20];
    let n = id.len().min(raw.len());
    id[..n].copy_from_slice(&raw[..n]);
    NodeId(id)
}

fn addr(i: u16) -> std::net::SocketAddr {
    format!("127.0.0.1:{}", 9000 + i).parse().unwrap()
}

#[test]
fn group_message_round_trip_through_snapcore() {
    let mut rng = DummyRng(0xC0FFEE);

    let alice_x = StaticSecret::from([11u8; 32]);
    let alice_xpub = PublicKey::from(&alice_x);
    let alice_sign = SigningKey::from_bytes(&[12u8; 32]);
    let bob_x = StaticSecret::from([22u8; 32]);
    let bob_xpub = PublicKey::from(&bob_x);

    // Setup copied verbatim from dht_round_trip.rs rather than re-derived: the
    // point of this test is the DHT underneath, so any difference in the crypto
    // half would make the comparison meaningless.
    let mut alice = GroupState::create("sc-alice000", &mut rng);
    alice.add_member(
        Member { sc_id: "sc-bob00000".into(), x25519_pub: *bob_xpub.as_bytes() },
        &mut rng,
    );
    let epoch = alice.epoch;
    let alice_root = alice.my_sender_root();
    let sealed_key = seal::seal_sender_key(&alice_x, &bob_xpub, &alice_root, &mut rng).unwrap();
    let bob_alice_root = seal::open_sender_key(&bob_x, &alice_xpub, &sealed_key).unwrap();

    let (idx, mk) = alice.my_chain_mut().next_message_key();
    let body = GroupEntryBody {
        epoch,
        index: idx,
        kind: MessageKind::Text,
        payload: b"first group message".to_vec(),
    };
    let wire = seal_and_sign(&body, &mk, "sc-alice000", &alice_sign, &mut rng).unwrap();

    // ── A small snapcore network, all mutually introduced ──
    let mut net = Network::new();
    let ids: Vec<NodeId> = (0..6u8)
        .map(|i| {
            let mut b = [0u8; 20];
            b[0] = i * 40;
            b[19] = i;
            NodeId(b)
        })
        .collect();
    for (i, id) in ids.iter().enumerate() {
        net.add(ScNode::new(*id, addr(i as u16)));
    }
    let now = 1_000u64;
    for a in &ids {
        for b in &ids {
            if a != b {
                net.introduce(a, b, now);
            }
        }
    }

    let bucket = feed_bucket(&alice.group, epoch);
    let key = bucket_key(&bucket);

    // ── Alice PUTs: find the closest nodes, then store on them ──
    let tx = SimTransport { net: &net, now };
    let alice_node = net.get(&ids[0]);
    let (me, mut rt) = {
        let n = alice_node.borrow();
        (n.contact(now), RoutingTable::new(n.id()))
    };
    for id in &ids {
        if *id != ids[0] {
            let c = net.get(id).borrow().contact(now);
            rt.saw(c.id, c.addr, now);
        }
    }
    let closest = lookup_nodes(&me, &mut rt, &key, &tx, 3, now);
    assert!(!closest.is_empty(), "the network resolved somebody to store on");
    for c in closest.iter().take(3) {
        tx_store(&net, &me, c, key, wire.clone(), 300, now);
    }

    // ── Bob GETs from a node that does not hold the value ──
    let bob_node = net.get(ids.last().unwrap());
    let (bob_me, mut bob_rt) = {
        let n = bob_node.borrow();
        (n.contact(now), RoutingTable::new(n.id()))
    };
    for id in &ids {
        if id != ids.last().unwrap() {
            let c = net.get(id).borrow().contact(now);
            bob_rt.saw(c.id, c.addr, now);
        }
    }
    let values = lookup_value(&bob_me, &mut bob_rt, &key, &tx, 3, now)
        .expect("the DHT found the group entry");
    assert_eq!(values.len(), 1, "exactly one group entry in the feed bucket");
    let got = &values[0];

    // ── The crypto half is unchanged: this is the point of the test ──
    let sender = snapclone_group::entry::peek_sender_id(got).unwrap();
    assert_eq!(sender, "sc-alice000");

    let mut bob_chain = SenderChain::from_root(bob_alice_root);
    let bob_mk = bob_chain.message_key_for(idx).unwrap();
    let opened = verify_and_open(got, &bob_mk).unwrap();
    assert_eq!(opened.sender_id, "sc-alice000");
    assert_eq!(opened.body.payload, b"first group message");
    assert_eq!(opened.sender_pubkey, alice_sign.verifying_key().to_bytes());

    // An eavesdropper scraping the same bucket gets ciphertext only: the DHT
    // carries opaque bytes and changing the DHT must not change that.
    let eve = lookup_value(&bob_me, &mut bob_rt, &key, &tx, 3, now).unwrap();
    assert!(verify_and_open(&eve[0], &[0xEEu8; 32]).is_err(),
            "no sender key means no message, whichever DHT carried it");
}

fn tx_store(net: &Network, from: &snapcore::routing::Contact,
            to: &snapcore::routing::Contact, key: NodeId, value: Vec<u8>,
            ttl: u64, now: u64) {
    if let Some(n) = net.nodes.get(&to.id) {
        n.borrow_mut().handle_store(from, key, value, ttl, now);
    }
}
