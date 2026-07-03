//! entry.rs — the sealed, signed group message entry that lands in the DHT
//! group-feed bucket.
//!
//! See `docs/GROUP_AND_SERVER_DESIGN.md` §2.2. This is the unit a group member
//! PUTs into the feed bucket (`group::feed_bucket(group_id, epoch)`) and every
//! other member GETs and decrypts. It closes the S-19 social-graph leak for
//! groups from day one: the DHT (and the thin LAN server) sees only
//!
//!   * the public routing bucket (required so members can poll),
//!   * an opaque SEALED body (AES-256-GCM under the per-message sender key),
//!   * an Ed25519 signature + sender pubkey (so forgeries are rejected WITHOUT
//!     decrypting — verify-before-trust).
//!
//! It never sees sender↔recipient mapping, group size, or message content.
//!
//! # Framing (deterministic, hand-rolled — no serde)
//!
//! Signing requires CANONICAL bytes both sides reproduce identically. A
//! hand-rolled length-prefixed binary layout gives full control over that and
//! avoids pulling serde_json (and its dependency drift) into a crypto crate.
//!
//! Plaintext body (encrypted, never on the wire in clear):
//! ```text
//!   epoch:    u64  (8 bytes, big-endian)
//!   index:    u32  (4 bytes, BE)     — sender ratchet index
//!   kind:     u8                     — 0=Text, 1=MediaRef, 2=Control
//!   payload:  len-prefixed bytes (u32 BE length + bytes)
//! ```
//!
//! Signed wire entry (what goes in the DHT, all big-endian length prefixes):
//! ```text
//!   magic:        4 bytes  "SGE1"   (SnapClone Group Entry v1)
//!   sender_id:    len-prefixed UTF-8 (sc-XXXXXXXX)
//!   sealed_body:  len-prefixed bytes (nonce||ciphertext||tag from seal)
//!   sender_pk:    32 bytes (Ed25519 verifying key)
//!   sig:          64 bytes (Ed25519 over magic..sender_pk, i.e. everything
//!                 above the signature)
//! ```
//!
//! The signature covers the sender id + sealed body + sender pubkey, so an
//! attacker can neither alter the ciphertext nor re-attribute the entry without
//! invalidating it. As with `SignedMailboxEntry`, the signature proves WHO
//! signed; the caller still confirms that pubkey belongs to `sender_id` via the
//! contact-key store (TOFU) before trusting the id.

use crate::seal::{self, SealError};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

const MAGIC: &[u8; 4] = b"SGE1";

/// What a group message carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// Inline UTF-8 text.
    Text = 0,
    /// A reference to content-addressed media (the payload is the content hash
    /// + fetch hints, encoded by the caller); the blob itself is fetched P2P.
    MediaRef = 1,
    /// A group control message (membership change, admin rotation, etc.).
    Control = 2,
}

impl MessageKind {
    fn from_u8(b: u8) -> Option<Self> {
        match b {
            0 => Some(MessageKind::Text),
            1 => Some(MessageKind::MediaRef),
            2 => Some(MessageKind::Control),
            _ => None,
        }
    }
}

/// The decrypted inner body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupEntryBody {
    pub epoch: u64,
    pub index: u32,
    pub kind: MessageKind,
    pub payload: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EntryError {
    Seal(SealError),
    Truncated,
    BadMagic,
    BadKind,
    BadSignature,
    BadKey,
}

impl From<SealError> for EntryError {
    fn from(e: SealError) -> Self {
        EntryError::Seal(e)
    }
}

// ── little length-prefixed helpers ──────────────────────────────────────────

fn put_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// Read a u32-length-prefixed slice at `pos`, advancing `pos`.
fn take_lp<'a>(buf: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    if *pos + 4 > buf.len() {
        return None;
    }
    let len = u32::from_be_bytes(buf[*pos..*pos + 4].try_into().ok()?) as usize;
    *pos += 4;
    if *pos + len > buf.len() {
        return None;
    }
    let s = &buf[*pos..*pos + len];
    *pos += len;
    Some(s)
}

// ── body framing ────────────────────────────────────────────────────────────

impl GroupEntryBody {
    /// Serialize the plaintext body (pre-encryption).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 4 + 1 + 4 + self.payload.len());
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&self.index.to_be_bytes());
        out.push(self.kind as u8);
        put_lp(&mut out, &self.payload);
        out
    }

    /// Parse a plaintext body (post-decryption).
    pub fn from_bytes(buf: &[u8]) -> Result<Self, EntryError> {
        if buf.len() < 8 + 4 + 1 {
            return Err(EntryError::Truncated);
        }
        let epoch = u64::from_be_bytes(buf[0..8].try_into().unwrap());
        let index = u32::from_be_bytes(buf[8..12].try_into().unwrap());
        let kind = MessageKind::from_u8(buf[12]).ok_or(EntryError::BadKind)?;
        let mut pos = 13;
        let payload = take_lp(buf, &mut pos).ok_or(EntryError::Truncated)?.to_vec();
        Ok(GroupEntryBody { epoch, index, kind, payload })
    }

    /// Bridge: wrap a DAG node as a group entry body. The node's ordering
    /// metadata (parents, Lamport clock, content hash) becomes the body payload
    /// under `kind = MediaRef`, so once this body is sealed+signed the ordering
    /// info is itself encrypted + authenticated on the wire — the DHT and any
    /// relay see only opaque bytes. `epoch` is taken from the node; `index` is
    /// the node's Lamport clock truncated to u32 (a cheap monotonic hint for
    /// consumers that don't reconstruct the full DAG; the authoritative order
    /// still comes from the parsed node).
    pub fn from_dag_node(node: &crate::dag::DagNode) -> Self {
        GroupEntryBody {
            epoch: node.epoch,
            index: node.lamport as u32,
            kind: MessageKind::MediaRef,
            payload: node.to_payload(),
        }
    }

    /// Bridge: recover the DAG node from a group entry body. Returns None if the
    /// body isn't a MediaRef carrying a well-formed node payload.
    pub fn to_dag_node(&self) -> Option<crate::dag::DagNode> {
        if self.kind != MessageKind::MediaRef {
            return None;
        }
        crate::dag::DagNode::from_payload(&self.payload)
    }
}

/// Seal + sign a group entry into the bytes that go in the DHT feed bucket.
///
/// `message_key` is the per-message key from the sender's ratchet
/// (`SenderChain::next_message_key`). `signing_key` is the sender's Ed25519
/// identity. `sender_id` is the sc-ID string.
pub fn seal_and_sign<R: rand_core::RngCore + rand_core::CryptoRng>(
    body: &GroupEntryBody,
    message_key: &[u8; 32],
    sender_id: &str,
    signing_key: &SigningKey,
    rng: &mut R,
) -> Result<Vec<u8>, EntryError> {
    let sealed = seal::encrypt_content(message_key, &body.to_bytes(), rng)?;
    let vk = signing_key.verifying_key();

    // Build the signed region: magic || sender_id || sealed || sender_pk.
    let mut signed_region = Vec::new();
    signed_region.extend_from_slice(MAGIC);
    put_lp(&mut signed_region, sender_id.as_bytes());
    put_lp(&mut signed_region, &sealed);
    signed_region.extend_from_slice(vk.as_bytes());

    let sig: Signature = signing_key.sign(&signed_region);

    // Wire = signed_region || signature.
    let mut wire = signed_region;
    wire.extend_from_slice(&sig.to_bytes());
    Ok(wire)
}

/// A verified, opened group entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedEntry {
    pub sender_id: String,
    /// The sender's Ed25519 verifying key (caller confirms it matches sender_id
    /// via the contact-key store / TOFU).
    pub sender_pubkey: [u8; 32],
    pub body: GroupEntryBody,
}

/// Verify the signature, then decrypt. ORDER MATTERS (§3.5): we authenticate
/// the wire bytes BEFORE decrypting attacker-controlled ciphertext.
///
/// `message_key` is the per-message key the recipient derives by ratcheting the
/// sender's chain to `body.index` (the caller reads the index from a cheap
/// pre-parse, or simply tries the expected next index). Returns the opened
/// entry on success.
pub fn verify_and_open(
    wire: &[u8],
    message_key: &[u8; 32],
) -> Result<OpenedEntry, EntryError> {
    // Minimum: magic(4) + lp sender(4) + lp sealed(4) + pk(32) + sig(64).
    if wire.len() < 4 + 4 + 4 + 32 + 64 {
        return Err(EntryError::Truncated);
    }
    if &wire[0..4] != MAGIC {
        return Err(EntryError::BadMagic);
    }
    // Split off the trailing 64-byte signature; everything before is signed.
    let sig_start = wire.len() - 64;
    let signed_region = &wire[..sig_start];
    let sig_bytes: [u8; 64] = wire[sig_start..].try_into().map_err(|_| EntryError::Truncated)?;

    // Parse the signed region: magic, sender_id, sealed, sender_pk(32).
    let mut pos = 4;
    let sender_id = take_lp(signed_region, &mut pos).ok_or(EntryError::Truncated)?;
    let sealed = take_lp(signed_region, &mut pos).ok_or(EntryError::Truncated)?;
    if pos + 32 != signed_region.len() {
        return Err(EntryError::Truncated);
    }
    let pk_bytes: [u8; 32] = signed_region[pos..pos + 32]
        .try_into()
        .map_err(|_| EntryError::BadKey)?;

    // VERIFY signature first.
    let vk = VerifyingKey::from_bytes(&pk_bytes).map_err(|_| EntryError::BadKey)?;
    let sig = Signature::from_bytes(&sig_bytes);
    vk.verify(signed_region, &sig).map_err(|_| EntryError::BadSignature)?;

    // THEN decrypt.
    let plain = seal::decrypt_content(message_key, sealed)?;
    let body = GroupEntryBody::from_bytes(&plain)?;

    Ok(OpenedEntry {
        sender_id: String::from_utf8_lossy(sender_id).into_owned(),
        sender_pubkey: pk_bytes,
        body,
    })
}

/// Cheap pre-parse: read just the sealed body's required ratchet INDEX without
/// verifying or decrypting — wait, the index lives INSIDE the sealed body, so
/// it can't be read without the key. Instead this returns the sender_id (which
/// IS in cleartext for routing/attribution display) so a recipient can pick the
/// right sender chain before attempting decryption. Does NOT verify the
/// signature; treat the result as untrusted until verify_and_open succeeds.
pub fn peek_sender_id(wire: &[u8]) -> Option<String> {
    if wire.len() < 8 || &wire[0..4] != MAGIC {
        return None;
    }
    let mut pos = 4;
    take_lp(wire, &mut pos).map(|s| String::from_utf8_lossy(s).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ratchet::SenderChain;

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

    fn body() -> GroupEntryBody {
        GroupEntryBody {
            epoch: 3,
            index: 7,
            kind: MessageKind::Text,
            payload: b"hello group".to_vec(),
        }
    }

    #[test]
    fn dag_node_rides_through_sealed_entry() {
        // The full layering: author a DAG node, wrap it in a body, seal+sign,
        // verify+open, extract the node, and insert it into a fresh DAG — it
        // must admit with the SAME content-addressed id. This proves the
        // ordering metadata travels encrypted+authenticated and survives intact.
        use crate::dag::{DagNode, SnapDag};

        // Build a node by authoring on a source replica (so it has a real id).
        let mut src = SnapDag::new();
        let (root, root_id) = src.author_node("sc-abcdef01", 3, "snap-hash-1");

        // Wrap → seal → ship.
        let mut rng = DummyRng(11);
        let signing = SigningKey::from_bytes(&[5u8; 32]);
        let mut sender_chain = SenderChain::from_root([9u8; 32]);
        let (_idx, mk) = sender_chain.next_message_key();

        let entry_body = GroupEntryBody::from_dag_node(&root);
        assert_eq!(entry_body.kind, MessageKind::MediaRef);
        assert_eq!(entry_body.epoch, 3);
        let wire = seal_and_sign(&entry_body, &mk, "sc-abcdef01", &signing, &mut rng).unwrap();

        // Receiver opens and recovers the node.
        let mut recv_chain = SenderChain::from_root([9u8; 32]);
        let recv_mk = recv_chain.message_key_for(0).unwrap();
        let opened = verify_and_open(&wire, &recv_mk).unwrap();
        let recovered = opened.body.to_dag_node().expect("recovers a DAG node");
        assert_eq!(recovered.compute_id(), root_id, "id survived the sealed trip");

        // Insert into a fresh replica's DAG — admits cleanly with the same id.
        let mut dst = SnapDag::new();
        match dst.insert(recovered) {
            crate::dag::InsertOutcome::Admitted { admitted_ids } => {
                assert_eq!(admitted_ids, vec![root_id]);
            }
            other => panic!("expected admit, got {other:?}"),
        }
        assert_eq!(dst.tips(), src.tips(), "replicas agree on the tip");
    }

    #[test]
    fn non_mediaref_body_is_not_a_dag_node() {
        // A plain Text body must not be misread as a DAG node.
        assert!(body().to_dag_node().is_none());
    }

    #[test]
    fn body_framing_roundtrip() {
        let b = body();
        let back = GroupEntryBody::from_bytes(&b.to_bytes()).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn seal_sign_open_roundtrip() {
        let mut rng = DummyRng(1);
        let signing = SigningKey::from_bytes(&[5u8; 32]);

        // Sender derives a message key from its chain.
        let mut sender_chain = SenderChain::from_root([9u8; 32]);
        let (_idx, mk) = sender_chain.next_message_key();

        let wire = seal_and_sign(&body(), &mk, "sc-abcdef01", &signing, &mut rng).unwrap();

        // Receiver ratchets a copy of the chain to the same index, gets the
        // same mk, verifies + opens.
        let mut recv_chain = SenderChain::from_root([9u8; 32]);
        let recv_mk = recv_chain.message_key_for(0).unwrap();
        let opened = verify_and_open(&wire, &recv_mk).unwrap();
        assert_eq!(opened.sender_id, "sc-abcdef01");
        assert_eq!(opened.body, body());
        assert_eq!(opened.sender_pubkey, signing.verifying_key().to_bytes());
    }

    #[test]
    fn tampered_ciphertext_fails_signature() {
        let mut rng = DummyRng(2);
        let signing = SigningKey::from_bytes(&[5u8; 32]);
        let mk = [3u8; 32];
        let mut wire = seal_and_sign(&body(), &mk, "sc-1", &signing, &mut rng).unwrap();
        // Flip a byte inside the sealed region (before the signature) — sig must
        // fail since it covers the sealed body.
        wire[20] ^= 0x01;
        assert_eq!(verify_and_open(&wire, &mk).unwrap_err(), EntryError::BadSignature);
    }

    #[test]
    fn forged_signature_fails() {
        let mut rng = DummyRng(3);
        let signing = SigningKey::from_bytes(&[5u8; 32]);
        let mk = [3u8; 32];
        let mut wire = seal_and_sign(&body(), &mk, "sc-1", &signing, &mut rng).unwrap();
        // Corrupt the trailing signature.
        let n = wire.len();
        wire[n - 1] ^= 0xff;
        assert_eq!(verify_and_open(&wire, &mk).unwrap_err(), EntryError::BadSignature);
    }

    #[test]
    fn wrong_message_key_fails_decrypt_after_valid_sig() {
        // A correctly-SIGNED entry whose content we can't decrypt (we lack the
        // right sender key, e.g. wrong epoch) must fail at the decrypt step, not
        // the signature step — proving verify-before-decrypt ordering and that
        // a valid sig doesn't leak plaintext.
        let mut rng = DummyRng(4);
        let signing = SigningKey::from_bytes(&[5u8; 32]);
        let wire = seal_and_sign(&body(), &[1u8; 32], "sc-1", &signing, &mut rng).unwrap();
        match verify_and_open(&wire, &[2u8; 32]) {
            Err(EntryError::Seal(_)) => {} // decrypt failed, as expected
            other => panic!("expected Seal error, got {other:?}"),
        }
    }

    #[test]
    fn peek_sender_id_without_key() {
        let mut rng = DummyRng(5);
        let signing = SigningKey::from_bytes(&[5u8; 32]);
        let wire = seal_and_sign(&body(), &[3u8; 32], "sc-peekme0", &signing, &mut rng).unwrap();
        // The recipient can read the sender id for routing BEFORE having a key,
        // but it's untrusted until verify_and_open.
        assert_eq!(peek_sender_id(&wire).as_deref(), Some("sc-peekme0"));
        assert!(peek_sender_id(b"garbage").is_none());
    }

    #[test]
    fn truncated_and_bad_magic_rejected() {
        assert_eq!(verify_and_open(b"short", &[0u8; 32]).unwrap_err(), EntryError::Truncated);
        let mut rng = DummyRng(6);
        let signing = SigningKey::from_bytes(&[5u8; 32]);
        let mut wire = seal_and_sign(&body(), &[3u8; 32], "sc-1", &signing, &mut rng).unwrap();
        wire[0] = b'X'; // break magic
        assert_eq!(verify_and_open(&wire, &[3u8; 32]).unwrap_err(), EntryError::BadMagic);
    }

    #[test]
    fn media_ref_and_control_kinds() {
        for kind in [MessageKind::MediaRef, MessageKind::Control] {
            let b = GroupEntryBody { epoch: 1, index: 0, kind, payload: vec![1, 2, 3] };
            let back = GroupEntryBody::from_bytes(&b.to_bytes()).unwrap();
            assert_eq!(back.kind, kind);
        }
    }
}
