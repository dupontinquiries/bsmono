//! Integration test: a full group message round-trip through the micro-DHT.
//!
//! This is the "next verifiable step" from docs/GROUP_AND_SERVER_DESIGN.md §5:
//! prove the group crypto + the sealed/signed group entry + the DHT value layer
//! all compose. Flow exercised:
//!
//!   1. Alice creates a group, adds Bob (epoch bumps), distributes her sender
//!      key to Bob via the per-recipient SEAL (the only per-recipient step).
//!   2. Alice composes a message: ratchet → message key → seal+sign a
//!      GroupEntryBody into wire bytes.
//!   3. Alice PUTs the wire bytes into the DHT under the group FEED BUCKET for
//!      the current epoch (the micro-DHT stands in for OpenDHT's value layer).
//!   4. Bob GETs the feed bucket, sees the opaque bytes, peeks the sender id,
//!      ratchets Alice's chain to the message index, and verify+opens.
//!   5. Bob reads the plaintext. The DHT only ever saw the bucket key + opaque
//!      signed/sealed bytes — never the content, sender↔recipient mapping, or
//!      group size.
//!
//! Also asserts the security property end-to-end: an eavesdropper who scrapes
//! the same feed bucket from the DHT but lacks Alice's sender key gets only
//! ciphertext it cannot open.

use std::sync::atomic::Ordering;
use std::time::Duration;

use snapclone_group::{
    entry::{seal_and_sign, verify_and_open, GroupEntryBody, MessageKind},
    group::{feed_bucket, GroupState, Member},
    ratchet::SenderChain,
    seal,
};
use snapclone_microdht::{Client, Node};

use ed25519_dalek::SigningKey;
use x25519_dalek::{PublicKey, StaticSecret};

// Deterministic test RNG (NOT for production).
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

#[test]
fn group_message_round_trip_through_microdht() {
    let mut rng = DummyRng(0xC0FFEE);

    // ── Identities ──
    let alice_x = StaticSecret::from([11u8; 32]); // Alice X25519 (for sealing)
    let alice_xpub = PublicKey::from(&alice_x);
    let alice_sign = SigningKey::from_bytes(&[12u8; 32]); // Alice Ed25519 (signing)

    let bob_x = StaticSecret::from([22u8; 32]);
    let bob_xpub = PublicKey::from(&bob_x);

    // ── 1. Alice creates the group, adds Bob, distributes her sender key ──
    let mut alice = GroupState::create("sc-alice000", &mut rng);
    alice.add_member(
        Member { sc_id: "sc-bob00000".into(), x25519_pub: *bob_xpub.as_bytes() },
        &mut rng,
    );
    let epoch = alice.epoch;
    let alice_root = alice.my_sender_root();
    let sealed_key = seal::seal_sender_key(&alice_x, &bob_xpub, &alice_root, &mut rng).unwrap();
    // Bob opens the sealed sender key (would arrive via his 1:1 mailbox).
    let bob_alice_root = seal::open_sender_key(&bob_x, &alice_xpub, &sealed_key).unwrap();

    // ── 2. Alice composes + seals + signs a message ──
    let (idx, mk) = alice.my_chain_mut().next_message_key();
    let body = GroupEntryBody {
        epoch,
        index: idx,
        kind: MessageKind::Text,
        payload: b"first group message".to_vec(),
    };
    let wire = seal_and_sign(&body, &mk, "sc-alice000", &alice_sign, &mut rng).unwrap();

    // ── Spin up the micro-DHT node (stands in for OpenDHT's value layer) ──
    let node = Node::bind("127.0.0.1:0", false).unwrap();
    let addr = node.local_addr().unwrap();
    let shutdown = node.shutdown_handle();
    let server = std::thread::spawn(move || node.run().unwrap());
    let dht = Client::connect(addr, Duration::from_secs(2)).unwrap();

    // ── 3. Alice PUTs the entry under the group FEED BUCKET ──
    let bucket = feed_bucket(&alice.group, epoch);
    dht.put(&bucket, &wire, Duration::from_secs(300)).unwrap();

    // ── 4. Bob GETs the bucket and reads the opaque values ──
    let values = dht.get(&bucket).unwrap();
    assert_eq!(values.len(), 1, "exactly one group entry in the feed bucket");
    let got = &values[0];

    // Bob peeks the (cleartext, untrusted) sender id to choose the right chain.
    let sender = snapclone_group::entry::peek_sender_id(got).unwrap();
    assert_eq!(sender, "sc-alice000");

    // Bob ratchets Alice's chain to the message index and derives the same key.
    let mut bob_chain = SenderChain::from_root(bob_alice_root);
    let bob_mk = bob_chain.message_key_for(idx).unwrap();

    // Bob verifies + opens.
    let opened = verify_and_open(got, &bob_mk).unwrap();
    assert_eq!(opened.sender_id, "sc-alice000");
    assert_eq!(opened.body.payload, b"first group message");
    assert_eq!(opened.sender_pubkey, alice_sign.verifying_key().to_bytes());

    // ── 5. An eavesdropper scraping the SAME bucket gets only ciphertext ──
    // (No sender key → cannot derive mk → verify_and_open fails at decrypt even
    // though the signature is valid.)
    let eve_values = dht.get(&bucket).unwrap();
    let eve_attempt = verify_and_open(&eve_values[0], &[0xEEu8; 32]);
    assert!(
        eve_attempt.is_err(),
        "eavesdropper without the sender key must not open the entry"
    );

    shutdown.store(true, Ordering::Relaxed);
    server.join().unwrap();
}
