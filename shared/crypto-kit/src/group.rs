//! group.rs — the group-messaging crypto layer: a one-way sender-key ratchet
//! plus per-recipient sender-key sealing. Lifted from snapclone-group and built
//! entirely on crypto-kit's existing primitives (no fresh AEAD or KDF).
//!
//! Scheme (sender-key, from snapclone's GROUP_AND_SERVER_DESIGN.md):
//!   * each (member, epoch) owns a symmetric chain key `CK`; message `i` uses
//!     `MK_i = HKDF(CK_i, MSG_KEY)` and the chain advances
//!     `CK_{i+1} = HKDF(CK_i, RATCHET)`, discarding `CK_i` — forward secrecy
//!     within an epoch (the ratchet is one-way);
//!   * a member hands its epoch sender-key root to each other member by
//!     [`seal_sender_key`]-ing it over the 1:1 X25519 shared secret (small, so
//!     N-way fan-out is cheap);
//!   * content is encrypted ONCE per message under `MK_i` with crypto-kit's
//!     AES-256-GCM [`crate::seal`] / [`crate::open`], regardless of group size.
//!
//! Membership epochs (add/remove re-key) are a usage pattern on top: on a
//! membership change every member starts a fresh [`SenderChain`] and seals the
//! new root only to the new member set — so a removed member cannot read the
//! new epoch and a newly-added member cannot read the prior one. The
//! `epoch_rekey_*` test proves both. The epoch/membership *bookkeeping* itself
//! (rosters, group ids, feed buckets) belongs in share-kit, not here.

use crate::{derive_session_key, open, seal, x25519_shared, KEY_LEN};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use zeroize::Zeroize;

// ── versioned HKDF domain-separation labels (byte-identical to snapclone) ──────

/// Chain ratchet step: `CK_{i+1} = HKDF(CK_i, RATCHET)`.
pub const RATCHET: &[u8] = b"snapclone-v1-group-ratchet";
/// Per-message key: `MK_i = HKDF(CK_i, MSG_KEY)`.
pub const MSG_KEY: &[u8] = b"snapclone-v1-group-msg";
/// Sealing a sender key to one recipient: `seal_key = HKDF(x25519_shared, SENDERKEY_SEAL)`.
pub const SENDERKEY_SEAL: &[u8] = b"snapclone-v1-group-senderkey-seal";

/// A sender's symmetric chain for one epoch. `index` counts messages already
/// emitted (the NEXT message uses the key at `index`). The chain key is
/// zeroized on drop and on each advance.
pub struct SenderChain {
    ck: [u8; KEY_LEN],
    index: u32,
}

impl Drop for SenderChain {
    fn drop(&mut self) {
        self.ck.zeroize();
    }
}

impl SenderChain {
    /// Start a chain from a 32-byte root sender key.
    pub fn from_root(root: [u8; KEY_LEN]) -> Self {
        Self { ck: root, index: 0 }
    }

    /// Generate a fresh random root and start a chain (a member (re)keying for a
    /// new epoch).
    pub fn generate() -> Self {
        let mut root = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut root);
        let c = Self::from_root(root);
        root.zeroize();
        c
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    /// HKDF helper: expand the chain key under `info` into 32 bytes.
    fn expand(ck: &[u8; KEY_LEN], info: &[u8]) -> [u8; KEY_LEN] {
        let hk = Hkdf::<Sha256>::new(None, ck);
        let mut out = [0u8; KEY_LEN];
        hk.expand(info, &mut out).expect("HKDF expand of 32 bytes never fails");
        out
    }

    /// Derive the message key for the CURRENT index WITHOUT advancing. Used by a
    /// receiver that has ratcheted its chain copy exactly to the sender's index.
    pub fn message_key_at_current(&self) -> [u8; KEY_LEN] {
        Self::expand(&self.ck, MSG_KEY)
    }

    /// Derive the next message key and advance the chain (discarding the old
    /// chain key — forward secrecy). Returns `(index_used, MK)`.
    pub fn next_message_key(&mut self) -> (u32, [u8; KEY_LEN]) {
        let mk = Self::expand(&self.ck, MSG_KEY);
        let used = self.index;
        let next = Self::expand(&self.ck, RATCHET);
        self.ck.zeroize();
        self.ck = next;
        self.index += 1;
        (used, mk)
    }

    /// Ratchet forward to `target` (a receiver catching up to a sender's index),
    /// returning the message key AT `target`. `None` if `target` is behind the
    /// current index — the chain cannot go backward (the forward-secrecy
    /// guarantee).
    pub fn message_key_for(&mut self, target: u32) -> Option<[u8; KEY_LEN]> {
        if target < self.index {
            return None;
        }
        while self.index < target {
            let next = Self::expand(&self.ck, RATCHET);
            self.ck.zeroize();
            self.ck = next;
            self.index += 1;
        }
        Some(self.message_key_at_current())
    }

    /// Snapshot the current root chain key for sealing/distribution. Callers
    /// MUST treat the result as secret and zeroize it after use.
    pub fn export_root(&self) -> [u8; KEY_LEN] {
        self.ck
    }
}

// ── per-recipient sender-key sealing (over the 1:1 X25519 secret) ──────────────

/// Seal a 32-byte sender-key root to one recipient. `my_secret` is OUR X25519
/// static secret, `their_public` the recipient's X25519 public key. Output is
/// crypto-kit's sealed layout `[12-byte nonce][ciphertext+16-byte tag]`.
pub fn seal_sender_key(
    my_secret: &[u8; KEY_LEN],
    their_public: &[u8; KEY_LEN],
    sender_key_root: &[u8; KEY_LEN],
) -> Vec<u8> {
    let shared = x25519_shared(my_secret, their_public);
    let seal_key = derive_session_key(shared.as_ref(), SENDERKEY_SEAL);
    seal(&seal_key, sender_key_root)
}

/// Open a sealed sender-key root. `my_secret` is OUR X25519 static secret,
/// `their_public` the SENDER's X25519 public key. `None` if authentication
/// fails or the plaintext isn't exactly 32 bytes.
pub fn open_sender_key(
    my_secret: &[u8; KEY_LEN],
    their_public: &[u8; KEY_LEN],
    sealed: &[u8],
) -> Option<[u8; KEY_LEN]> {
    let shared = x25519_shared(my_secret, their_public);
    let seal_key = derive_session_key(shared.as_ref(), SENDERKEY_SEAL);
    let pt = open(&seal_key, sealed)?;
    if pt.len() != KEY_LEN {
        return None;
    }
    let mut root = [0u8; KEY_LEN];
    root.copy_from_slice(&pt);
    Some(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::x25519_keypair;

    #[test]
    fn sender_and_receiver_agree() {
        let root = [7u8; KEY_LEN];
        let mut sender = SenderChain::from_root(root);
        let mut receiver = SenderChain::from_root(root);
        let (i0, k0) = sender.next_message_key();
        let (i1, k1) = sender.next_message_key();
        let (i2, k2) = sender.next_message_key();
        assert_eq!((i0, i1, i2), (0, 1, 2));
        assert_eq!(receiver.message_key_for(0).unwrap(), k0);
        assert_eq!(receiver.message_key_for(1).unwrap(), k1);
        assert_eq!(receiver.message_key_for(2).unwrap(), k2);
    }

    #[test]
    fn forward_secrecy_cannot_go_backward() {
        let mut receiver = SenderChain::from_root([3u8; KEY_LEN]);
        let _ = receiver.message_key_for(5).unwrap();
        assert_eq!(receiver.index(), 5);
        assert!(receiver.message_key_for(2).is_none(), "chain must not rewind");
    }

    #[test]
    fn keys_are_distinct_per_index() {
        let mut s = SenderChain::from_root([1u8; KEY_LEN]);
        let (_, a) = s.next_message_key();
        let (_, b) = s.next_message_key();
        let (_, c) = s.next_message_key();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn ratchet_and_message_labels_differ() {
        // MK and the next CK come from the same CK under DIFFERENT labels, so
        // they must differ — else the message key would leak the next chain key.
        let s = SenderChain::from_root([9u8; KEY_LEN]);
        assert_ne!(SenderChain::expand(&s.ck, MSG_KEY), SenderChain::expand(&s.ck, RATCHET));
    }

    #[test]
    fn generate_is_random() {
        assert_ne!(SenderChain::generate().export_root(), SenderChain::generate().export_root());
    }

    #[test]
    fn sender_key_seal_roundtrip() {
        let (alice_sk, alice_pk) = x25519_keypair();
        let (bob_sk, bob_pk) = x25519_keypair();
        let root = [42u8; KEY_LEN];
        let sealed = seal_sender_key(&alice_sk, &bob_pk, &root);
        assert_eq!(open_sender_key(&bob_sk, &alice_pk, &sealed), Some(root));
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let (alice_sk, alice_pk) = x25519_keypair();
        let (_bob_sk, bob_pk) = x25519_keypair();
        let (eve_sk, _eve_pk) = x25519_keypair();
        let root = [42u8; KEY_LEN];
        let sealed = seal_sender_key(&alice_sk, &bob_pk, &root);
        // Eve's shared secret with Alice differs from Bob's → GCM auth fails.
        assert!(open_sender_key(&eve_sk, &alice_pk, &sealed).is_none());
    }

    /// The headline group property: on an epoch re-key, a removed member cannot
    /// read the new epoch, and a newly-added member cannot read the prior one.
    #[test]
    fn epoch_rekey_excludes_removed_and_newly_added_members() {
        let (alice_sk, alice_pk) = x25519_keypair();
        let (bob_sk, bob_pk) = x25519_keypair();
        let (carol_sk, carol_pk) = x25519_keypair();

        // ── Epoch 1: members {Alice, Bob}. Alice seals her root to Bob only. ──
        let mut s1 = SenderChain::generate();
        let sealed_bob = seal_sender_key(&alice_sk, &bob_pk, &s1.export_root());
        let bob_root1 = open_sender_key(&bob_sk, &alice_pk, &sealed_bob).unwrap();
        let (i0, mk_e1) = s1.next_message_key();
        let ct_e1 = seal(&mk_e1, b"epoch1 secret");
        // Bob reads epoch-1 content by ratcheting his copy to the same index.
        let bob_mk = SenderChain::from_root(bob_root1).message_key_for(i0).unwrap();
        assert_eq!(open(&bob_mk, &ct_e1).as_deref(), Some(&b"epoch1 secret"[..]));

        // ── Membership change → Epoch 2: remove Bob, add Carol. Alice re-keys
        //    and seals the NEW root only to Carol. ──
        let mut s2 = SenderChain::generate();
        let sealed_carol = seal_sender_key(&alice_sk, &carol_pk, &s2.export_root());
        let carol_root2 = open_sender_key(&carol_sk, &alice_pk, &sealed_carol).unwrap();
        let (j0, mk_e2) = s2.next_message_key();
        let ct_e2 = seal(&mk_e2, b"epoch2 secret");
        // Carol reads epoch-2 content.
        let carol_mk = SenderChain::from_root(carol_root2).message_key_for(j0).unwrap();
        assert_eq!(open(&carol_mk, &ct_e2).as_deref(), Some(&b"epoch2 secret"[..]));

        // PROPERTY 1: removed Bob holds only root1 (root2 was never sealed to
        // him), so his best attempt yields the wrong key for epoch 2.
        let bob_wrong = SenderChain::from_root(bob_root1).message_key_for(j0).unwrap();
        assert!(open(&bob_wrong, &ct_e2).is_none(), "removed member must not read the new epoch");

        // PROPERTY 2: newly-added Carol holds only root2, so she cannot read the
        // prior epoch.
        let carol_wrong = SenderChain::from_root(carol_root2).message_key_for(i0).unwrap();
        assert!(open(&carol_wrong, &ct_e1).is_none(), "newly-added member must not read the prior epoch");
    }
}
