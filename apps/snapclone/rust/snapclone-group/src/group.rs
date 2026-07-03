//! group.rs — group identity, membership epochs, and the re-key flow.
//!
//! A `GroupId` is a random 256-bit value, NEVER derived from membership, so it
//! survives member changes (§1.1). Membership is tracked by an `epoch` counter
//! that bumps on every add/remove; each bump forces every (remaining) member to
//! generate a FRESH sender key for the new epoch (§1.4):
//!
//!   * ADD  → newcomer gets epoch e+1 keys but NOT epoch ≤e (no back-secrecy
//!            leak / post-compromise security).
//!   * REMOVE → removed member never receives epoch e+1 keys, so it cannot read
//!            e+1+ content (forward secrecy on removal) — proven in tests.
//!
//! This module owns the *local* per-member view: which epoch we're in, our own
//! sender chain for that epoch, and the sender chains we've received from other
//! members. Distribution of keys (sealing/opening) is `seal.rs`; wire framing
//! of entries is `entry.rs`. Keeping policy here and crypto there makes the
//! safety properties easy to audit.

use crate::ratchet::SenderChain;
use rand_core::{CryptoRng, RngCore};
use std::collections::HashMap;

/// 256-bit group identifier, hex-formatted `grp-<64hex>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupId(String);

impl GroupId {
    pub fn generate<R: RngCore + CryptoRng>(rng: &mut R) -> Self {
        let mut raw = [0u8; 32];
        rng.fill_bytes(&mut raw);
        Self(format!("grp-{}", hex::encode(raw)))
    }

    pub fn parse(s: &str) -> Option<Self> {
        if s.len() == 68 && s.starts_with("grp-") && s[4..].bytes().all(|b| b.is_ascii_hexdigit()) {
            Some(Self(s.to_owned()))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The DHT feed bucket for a group at a given epoch (§2.1). Members poll this
/// one key; the epoch in the key means a removed member who can compute the
/// next epoch's key string still can't read it (no sender keys), and old epochs
/// age out on their own.
pub fn feed_bucket(group: &GroupId, epoch: u64) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(format!("group-feed:{}:{}", group.as_str(), epoch).as_bytes());
    hex::encode(h)
}

/// A member reference: their sc-ID plus the X25519 public key we seal sender
/// keys to. (Ed25519 identity for signing lives at the entry layer.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub sc_id: String,
    pub x25519_pub: [u8; 32],
}

/// Local per-member group state.
pub struct GroupState {
    pub group: GroupId,
    pub epoch: u64,
    /// Roster for the CURRENT epoch (sc_id -> member).
    members: HashMap<String, Member>,
    /// Our own sender chain for the current epoch.
    my_chain: SenderChain,
    /// Other members' sender chains we've received (sc_id -> chain), for the
    /// current epoch. A receiver ratchets these forward to decrypt.
    peer_chains: HashMap<String, SenderChain>,
}

impl GroupState {
    /// Create a brand-new group at epoch 0 with us as the only member-with-keys.
    /// Other members get added via `add_member`, which bumps the epoch.
    pub fn create<R: RngCore + CryptoRng>(self_id: &str, rng: &mut R) -> Self {
        Self {
            group: GroupId::generate(rng),
            epoch: 0,
            members: HashMap::from([(
                self_id.to_owned(),
                Member { sc_id: self_id.to_owned(), x25519_pub: [0u8; 32] },
            )]),
            my_chain: SenderChain::generate(rng),
            peer_chains: HashMap::new(),
        }
    }

    /// Join an EXISTING group: we know its id + epoch + roster, and we mint our
    /// own fresh sender chain for this epoch. (We still need to RECEIVE other
    /// members' chains via `accept_peer_chain` to read their messages.)
    pub fn join<R: RngCore + CryptoRng>(
        group: GroupId,
        epoch: u64,
        roster: Vec<Member>,
        rng: &mut R,
    ) -> Self {
        let members = roster.into_iter().map(|m| (m.sc_id.clone(), m)).collect();
        Self {
            group,
            epoch,
            members,
            my_chain: SenderChain::generate(rng),
            peer_chains: HashMap::new(),
        }
    }

    pub fn member_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.members.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn is_member(&self, sc_id: &str) -> bool {
        self.members.contains_key(sc_id)
    }

    /// Our current sender chain (for sending).
    pub fn my_chain_mut(&mut self) -> &mut SenderChain {
        &mut self.my_chain
    }

    /// Export our current sender-key ROOT for sealing to a recipient. Caller
    /// zeroizes after sealing.
    pub fn my_sender_root(&self) -> [u8; 32] {
        self.my_chain.export_root()
    }

    /// Record a peer's sender-key root we received (sealed) for this epoch.
    pub fn accept_peer_chain(&mut self, sc_id: &str, root: [u8; 32]) {
        self.peer_chains
            .insert(sc_id.to_owned(), SenderChain::from_root(root));
    }

    /// Get a peer's chain to derive a message key at a given index.
    pub fn peer_chain_mut(&mut self, sc_id: &str) -> Option<&mut SenderChain> {
        self.peer_chains.get_mut(sc_id)
    }

    /// ADD a member: bump epoch, refresh OUR sender key, drop stale peer chains
    /// (they'll re-distribute for the new epoch). Returns the new epoch. The
    /// caller is responsible for sealing the new `my_sender_root()` to every
    /// member of the new roster (including the newcomer).
    pub fn add_member<R: RngCore + CryptoRng>(&mut self, m: Member, rng: &mut R) -> u64 {
        self.members.insert(m.sc_id.clone(), m);
        self.rekey_for_new_epoch(rng);
        self.epoch
    }

    /// REMOVE a member: drop them from the roster, bump epoch, refresh OUR
    /// sender key, drop ALL peer chains. The removed member is NOT in the new
    /// roster, so the caller seals the new sender key only to remaining members
    /// → the removed member can't read epoch e+1+ (forward secrecy on removal).
    pub fn remove_member<R: RngCore + CryptoRng>(&mut self, sc_id: &str, rng: &mut R) -> u64 {
        self.members.remove(sc_id);
        self.peer_chains.remove(sc_id);
        self.rekey_for_new_epoch(rng);
        self.epoch
    }

    /// The shared epoch transition: bump, fresh own chain, forget peer chains
    /// (each peer re-distributes its new-epoch chain). The OLD chains are
    /// dropped here, which is what enforces that a member removed at this
    /// transition cannot ride an old chain into the new epoch.
    fn rekey_for_new_epoch<R: RngCore + CryptoRng>(&mut self, rng: &mut R) {
        self.epoch += 1;
        self.my_chain = SenderChain::generate(rng);
        self.peer_chains.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seal;
    use x25519_dalek::{PublicKey, StaticSecret};

    struct DummyRng(u64);
    impl RngCore for DummyRng {
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
    impl CryptoRng for DummyRng {}

    #[test]
    fn group_id_format() {
        let mut rng = DummyRng(1);
        let g = GroupId::generate(&mut rng);
        assert!(g.as_str().starts_with("grp-"));
        assert_eq!(g.as_str().len(), 68);
        assert!(GroupId::parse(g.as_str()).is_some());
        assert!(GroupId::parse("grp-xyz").is_none());
        assert!(GroupId::parse("notgrp").is_none());
    }

    #[test]
    fn feed_bucket_depends_on_epoch() {
        let mut rng = DummyRng(2);
        let g = GroupId::generate(&mut rng);
        assert_ne!(feed_bucket(&g, 0), feed_bucket(&g, 1));
        assert_eq!(feed_bucket(&g, 5), feed_bucket(&g, 5));
    }

    #[test]
    fn add_and_remove_bump_epoch() {
        let mut rng = DummyRng(3);
        let mut g = GroupState::create("sc-aaaaaaaa", &mut rng);
        assert_eq!(g.epoch, 0);
        let e1 = g.add_member(
            Member { sc_id: "sc-bbbbbbbb".into(), x25519_pub: [2u8; 32] },
            &mut rng,
        );
        assert_eq!(e1, 1);
        assert!(g.is_member("sc-bbbbbbbb"));
        let e2 = g.remove_member("sc-bbbbbbbb", &mut rng);
        assert_eq!(e2, 2);
        assert!(!g.is_member("sc-bbbbbbbb"));
    }

    // The headline safety property (§3(6)): a member removed at an epoch
    // transition cannot read content sent in the NEW epoch, because it never
    // receives the new sender key and its old chain is useless for the new one.
    #[test]
    fn removed_member_cannot_read_new_epoch() {
        let mut rng = DummyRng(42);

        // Alice's X25519 identity (sender of content in the new epoch).
        let alice_x = StaticSecret::from([11u8; 32]);
        let alice_xpub = PublicKey::from(&alice_x);

        // Bob (to be removed) and Carol (stays) X25519 identities.
        let bob_x = StaticSecret::from([22u8; 32]);
        let bob_xpub = PublicKey::from(&bob_x);
        let carol_x = StaticSecret::from([33u8; 32]);
        let carol_xpub = PublicKey::from(&carol_x);

        // Group: Alice creates, adds Bob and Carol (epoch climbs).
        let mut alice = GroupState::create("sc-alice000", &mut rng);
        alice.add_member(
            Member { sc_id: "sc-bob00000".into(), x25519_pub: *bob_xpub.as_bytes() },
            &mut rng,
        );
        alice.add_member(
            Member { sc_id: "sc-carol000".into(), x25519_pub: *carol_xpub.as_bytes() },
            &mut rng,
        );

        // --- Epoch E: Alice distributes her sender key to Bob and Carol, then
        //     sends a message both can read. ---
        let epoch_e = alice.epoch;
        let root_e = alice.my_sender_root();
        let sealed_to_bob_e =
            seal::seal_sender_key(&alice_x, &bob_xpub, &root_e, &mut rng).unwrap();
        let sealed_to_carol_e =
            seal::seal_sender_key(&alice_x, &carol_xpub, &root_e, &mut rng).unwrap();

        let (idx_e, mk_e) = alice.my_chain_mut().next_message_key();
        let ct_e = seal::encrypt_content(&mk_e, b"epoch E secret", &mut rng).unwrap();

        // Bob (still a member at E) can read it.
        let bob_root_e = seal::open_sender_key(&bob_x, &alice_xpub, &sealed_to_bob_e).unwrap();
        let mut bob_chain_e = crate::ratchet::SenderChain::from_root(bob_root_e);
        let bob_mk_e = bob_chain_e.message_key_for(idx_e).unwrap();
        assert_eq!(seal::decrypt_content(&bob_mk_e, &ct_e).unwrap(), b"epoch E secret");

        // Carol too (sanity).
        let carol_root_e =
            seal::open_sender_key(&carol_x, &alice_xpub, &sealed_to_carol_e).unwrap();
        let mut carol_chain_e = crate::ratchet::SenderChain::from_root(carol_root_e);
        let carol_mk_e = carol_chain_e.message_key_for(idx_e).unwrap();
        assert_eq!(seal::decrypt_content(&carol_mk_e, &ct_e).unwrap(), b"epoch E secret");

        // --- REMOVE Bob: epoch bumps, Alice re-keys. ---
        let epoch_f = alice.remove_member("sc-bob00000", &mut rng);
        assert_eq!(epoch_f, epoch_e + 1);

        // Alice distributes her NEW sender key to remaining members only
        // (Carol). Bob gets NOTHING.
        let root_f = alice.my_sender_root();
        assert_ne!(root_e, root_f, "epoch re-key must produce a fresh sender key");
        let sealed_to_carol_f =
            seal::seal_sender_key(&alice_x, &carol_xpub, &root_f, &mut rng).unwrap();

        let (idx_f, mk_f) = alice.my_chain_mut().next_message_key();
        let ct_f = seal::encrypt_content(&mk_f, b"epoch F secret", &mut rng).unwrap();

        // Carol (still a member) reads epoch F fine.
        let carol_root_f =
            seal::open_sender_key(&carol_x, &alice_xpub, &sealed_to_carol_f).unwrap();
        let mut carol_chain_f = crate::ratchet::SenderChain::from_root(carol_root_f);
        let carol_mk_f = carol_chain_f.message_key_for(idx_f).unwrap();
        assert_eq!(seal::decrypt_content(&carol_mk_f, &ct_f).unwrap(), b"epoch F secret");

        // BOB (removed) tries to read epoch F using everything he legitimately
        // had: his OLD epoch-E sender-key root. It must NOT work — it's a
        // different key entirely, and he never received root_f.
        let mut bob_old_chain = crate::ratchet::SenderChain::from_root(bob_root_e);
        if let Some(bob_attempt_mk) = bob_old_chain.message_key_for(idx_f) {
            assert!(
                seal::decrypt_content(&bob_attempt_mk, &ct_f).is_err(),
                "removed member must NOT be able to decrypt new-epoch content"
            );
        }
        // And he cannot open Carol's sealed key (not addressed to him).
        assert!(
            seal::open_sender_key(&bob_x, &alice_xpub, &sealed_to_carol_f).is_err(),
            "removed member must not open a key sealed to someone else"
        );
    }

    #[test]
    fn added_member_cannot_read_prior_epoch() {
        // Post-compromise security on ADD: a newcomer gets epoch e+1 keys but
        // can't read epoch e content (it never had those sender keys).
        let mut rng = DummyRng(7);
        let alice_x = StaticSecret::from([11u8; 32]);
        let alice_xpub = PublicKey::from(&alice_x);

        let mut alice = GroupState::create("sc-alice000", &mut rng);

        // Epoch 0 message (only Alice).
        let (idx0, mk0) = alice.my_chain_mut().next_message_key();
        let ct0 = seal::encrypt_content(&mk0, b"pre-join", &mut rng).unwrap();
        let _ = (idx0, &ct0);

        // Newcomer Dave joins → epoch 1, Alice re-keys.
        let dave_x = StaticSecret::from([44u8; 32]);
        let dave_xpub = PublicKey::from(&dave_x);
        alice.add_member(
            Member { sc_id: "sc-dave0000".into(), x25519_pub: *dave_xpub.as_bytes() },
            &mut rng,
        );
        // Alice gives Dave only the NEW (epoch-1) sender key.
        let root1 = alice.my_sender_root();
        let sealed_dave = seal::seal_sender_key(&alice_x, &dave_xpub, &root1, &mut rng).unwrap();
        let dave_root1 = seal::open_sender_key(&dave_x, &alice_xpub, &sealed_dave).unwrap();

        // Dave's epoch-1 chain cannot derive the epoch-0 message key (different
        // root); confirm he can't read ct0 with anything he has.
        let mut dave_chain = crate::ratchet::SenderChain::from_root(dave_root1);
        if let Some(mk_try) = dave_chain.message_key_for(idx0) {
            assert!(seal::decrypt_content(&mk_try, &ct0).is_err(),
                    "newcomer must not read pre-join content");
        }
    }
}
