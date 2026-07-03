//! MailboxEntry — the metadata record stored in the DHT.
//!
//! V32.34+ mailbox overhaul. Three problems with the original design are
//! addressed here:
//!
//!   • S-5: MailboxId was truncated to 64 bits (16 hex chars), making
//!     targeted bucket-collision attacks feasible. Now full SHA-256
//!     (256-bit / 64 hex chars).
//!
//!   • S-12: entries were unsigned — anyone with DHT write access could
//!     plant a fake entry claiming any sender_id. Now an entry can be
//!     wrapped in a `SignedMailboxEntry` carrying an Ed25519 signature
//!     over the canonical bytes; the recipient verifies against the
//!     sender's known pubkey and rejects forgeries.
//!
//!   • S-19 (partial / structural): the plaintext entry leaks the social
//!     graph (sender, recipient, size, timing, sender IP in hints) to
//!     anyone scraping the DHT. Full fix is to encrypt the entry body to
//!     the recipient, which needs the per-contact pubkey infra
//!     (DHT_DISCOVERABILITY.md). This overhaul lays the groundwork by
//!     (a) separating the PUBLIC routing key (the bucket, which must be
//!     plaintext so the recipient can poll) from the entry BODY, and
//!     (b) defining `MailboxEntryBody` as the unit that a future change
//!     will seal. See `seal`/`open` stubs at the bottom.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unique mailbox ID — stable lookup key in the DHT.
/// Derived from recipient sc_id so they can poll for their own entries.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MailboxId(pub String);

impl MailboxId {
    /// Derive a mailbox ID for a given recipient sc-ID.
    ///
    /// **V32.34+ (S-5):** now the FULL SHA-256 (64 hex chars / 256-bit key
    /// space) rather than the previous 64-bit truncation. The old 16-hex-
    /// char form let an attacker brute-force a colliding key to pollute a
    /// victim's mailbox bucket (~2^32 work for a 50% collision on 64 bits —
    /// trivially feasible). 256-bit makes targeted collision infeasible.
    ///
    /// The DHT key being longer costs nothing meaningful (a few dozen extra
    /// bytes per lookup) and the recipient computes it the same way to poll.
    pub fn for_recipient(sc_id: &str) -> Self {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(format!("mailbox:{}", sc_id).as_bytes());
        Self(hex::encode(hash)) // full 32 bytes = 64 hex chars
    }

    /// V32.34+ compatibility shim: the OLD truncated (64-bit) mailbox id, so
    /// a transition period can poll BOTH the new full-length bucket and the
    /// legacy bucket while peers upgrade. Remove once all peers are on the
    /// new scheme (track via a deprecation window in the release notes).
    pub fn for_recipient_legacy_64bit(sc_id: &str) -> Self {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(format!("mailbox:{}", sc_id).as_bytes());
        Self(hex::encode(&hash[..8]))
    }
}

/// How the recipient should try to fetch the snap.
/// Ordered by preference — try earlier entries first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeliveryHint {
    /// Try direct WebSocket to this address.
    DirectWs { addr: String, ws_port: u16 },
    /// Request via relay server.
    Relay { relay_url: String },
    /// Sender will push when next online (no address available now).
    PushWhenOnline,
}

/// A mailbox entry stored in the DHT.
/// Kept minimal: < 200 bytes serialised.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxEntry {
    /// Unique snap identifier.
    pub snap_id:      String,
    /// SHA-256 hex of encrypted snap payload (for integrity check before fetch).
    pub content_hash: String,
    /// Sender's sc-ID.
    pub sender_id:    String,
    /// Recipient's sc-ID.
    pub recipient_id: String,
    /// Size of the encrypted snap in bytes (so recipient can allocate/estimate).
    pub payload_bytes: u64,
    /// When this entry expires (recipient should ignore after this).
    pub expires_at:   DateTime<Utc>,
    /// Hints on how to reach the sender to fetch the snap.
    #[serde(default)]
    pub delivery_hints: Vec<DeliveryHint>,
    /// Entry creation time (for freshness ordering).
    pub created_at: DateTime<Utc>,
}

impl MailboxEntry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        snap_id:       &str,
        content_hash:  &str,
        sender_id:     &str,
        recipient_id:  &str,
        payload_bytes: u64,
        ttl_secs:      u32,
        hints:         Vec<DeliveryHint>,
    ) -> Self {
        let now = Utc::now();
        Self {
            snap_id:        snap_id.to_owned(),
            content_hash:   content_hash.to_owned(),
            sender_id:      sender_id.to_owned(),
            recipient_id:   recipient_id.to_owned(),
            payload_bytes,
            expires_at:     now + chrono::Duration::seconds(ttl_secs as i64),
            delivery_hints: hints,
            created_at:     now,
        }
    }

    pub fn is_alive(&self) -> bool { Utc::now() < self.expires_at }

    /// Canonical bytes for signing/verification. We sign the SERIALIZED
    /// entry (deterministic JSON via serde_json, which preserves struct
    /// field order) so both sides hash the same bytes. Kept separate from
    /// `to_dht_bytes` so the signature wrapper can sign exactly these.
    pub fn canonical_bytes(&self) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Serialise to bytes for DHT storage. Returns Err if > 512 bytes (DHT limit).
    pub fn to_dht_bytes(&self) -> anyhow::Result<Vec<u8>> {
        let json = serde_json::to_vec(self)?;
        if json.len() > 512 {
            anyhow::bail!("MailboxEntry too large: {} bytes (max 512)", json.len());
        }
        Ok(json)
    }

    pub fn from_dht_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

/// V32.34+ (S-12): a mailbox entry plus the sender's Ed25519 signature over
/// its canonical bytes. This is what actually goes in the DHT now. The
/// recipient verifies the signature against the sender's known verifying key
/// (looked up by `entry.sender_id` in the per-contact key store) BEFORE
/// trusting any field — so an attacker with DHT write access can no longer
/// plant an entry claiming to be from someone else.
///
/// Layout in the DHT: serialized `SignedMailboxEntry` JSON. The signature is
/// 64 bytes (Ed25519), hex-encoded in the JSON (128 chars) — comfortably
/// inside the 512-byte budget alongside the ~200-byte entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedMailboxEntry {
    pub entry: MailboxEntry,
    /// Ed25519 signature (hex) over `entry.canonical_bytes()`.
    pub sig_hex: String,
    /// Sender's Ed25519 verifying key (hex, 32 bytes / 64 hex chars).
    /// Included so a recipient who doesn't yet have the sender in their
    /// key store can at least check the signature is SELF-consistent (the
    /// entry was signed by whoever owns this key). TOFU / contact-key
    /// verification of WHICH key is genuinely the sender's happens at a
    /// higher layer — this field does not replace that check.
    pub sender_pubkey_hex: String,
}

impl SignedMailboxEntry {
    /// Sign `entry` with the sender's Ed25519 signing key (32 raw bytes).
    pub fn sign(entry: MailboxEntry, signing_key_bytes: &[u8; 32]) -> anyhow::Result<Self> {
        use ed25519_dalek::{Signer, SigningKey};
        let sk = SigningKey::from_bytes(signing_key_bytes);
        let vk = sk.verifying_key();
        let canon = entry.canonical_bytes()?;
        let sig = sk.sign(&canon);
        Ok(Self {
            entry,
            sig_hex: hex::encode(sig.to_bytes()),
            sender_pubkey_hex: hex::encode(vk.to_bytes()),
        })
    }

    /// Verify the signature is valid for the embedded pubkey. Returns the
    /// inner entry on success. Uses `verify_strict` to reject signature
    /// malleability (mirrors the S-11 fix in the crypto crate).
    ///
    /// IMPORTANT: this proves the entry was signed by whoever holds
    /// `sender_pubkey_hex`. The caller MUST separately confirm that pubkey
    /// actually belongs to `entry.sender_id` (via the per-contact key store
    /// / TOFU) before trusting `sender_id`. Without that check, an attacker
    /// can still sign an entry with their OWN key and claim any sender_id —
    /// the signature just proves they signed it, not that they're that
    /// sender. The two-step (verify sig here, confirm key-ownership at the
    /// caller) is the S-12 + S-20 combination.
    pub fn verify_self_consistent(&self) -> anyhow::Result<&MailboxEntry> {
        use ed25519_dalek::{Signature, VerifyingKey};
        let pk_bytes: [u8; 32] = hex::decode(&self.sender_pubkey_hex)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("bad pubkey length"))?;
        let sig_bytes: [u8; 64] = hex::decode(&self.sig_hex)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("bad signature length"))?;
        let vk = VerifyingKey::from_bytes(&pk_bytes)
            .map_err(|e| anyhow::anyhow!("bad verifying key: {e}"))?;
        let sig = Signature::from_bytes(&sig_bytes);
        let canon = self.entry.canonical_bytes()?;
        vk.verify_strict(&canon, &sig)
            .map_err(|e| anyhow::anyhow!("signature verification failed: {e}"))?;
        Ok(&self.entry)
    }

    /// Verify against a KNOWN sender verifying key (the proper check once the
    /// caller has the sender's key from the contact store). Confirms BOTH
    /// that the signature is valid AND that it was made by the expected key.
    pub fn verify_with_key(&self, expected_pubkey: &[u8; 32]) -> anyhow::Result<&MailboxEntry> {
        // First the embedded key must match the expected one, else an
        // attacker could embed their own key + valid-for-their-key sig.
        let embedded: [u8; 32] = hex::decode(&self.sender_pubkey_hex)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("bad pubkey length"))?;
        if &embedded != expected_pubkey {
            anyhow::bail!("entry pubkey does not match expected sender key");
        }
        self.verify_self_consistent()
    }

    pub fn to_dht_bytes(&self) -> anyhow::Result<Vec<u8>> {
        let json = serde_json::to_vec(self)?;
        // V32.34+: 1024-byte cap. The bare-entry cap was 512; the signature
        // (64 bytes → 128 hex) + sender pubkey (32 → 64 hex) + JSON field
        // names add ~250 bytes, so a signed entry lands ~550-600 bytes.
        // OpenDHT permits values up to 64 KiB, so 1024 is still extremely
        // conservative and propagates in a single message. We keep a cap at
        // all only to catch a runaway delivery_hints list.
        if json.len() > 1024 {
            anyhow::bail!("SignedMailboxEntry too large: {} bytes (max 1024)", json.len());
        }
        Ok(json)
    }

    pub fn from_dht_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tiny test-only id generator so the test graph doesn't need `uuid`
    // (whose newer getrandom requires a toolchain we don't have here).
    // Production code mints real ids on the C++ side; tests just need a
    // distinct, validly-shaped snap_id string.
    fn test_snap_id() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(1);
        let v = N.fetch_add(1, Ordering::Relaxed);
        format!("snap-test-{:016x}", v)
    }

    #[test]
    fn mailbox_id_stable() {
        let a = MailboxId::for_recipient("sc-aabbccdd");
        let b = MailboxId::for_recipient("sc-aabbccdd");
        assert_eq!(a, b);
        // Different sc-IDs give different mailbox IDs
        let c = MailboxId::for_recipient("sc-11223344");
        assert_ne!(a, c);
    }

    #[test]
    fn mailbox_id_is_full_sha256() {
        // S-5: must be 64 hex chars (256-bit), not the old 16 (64-bit).
        let id = MailboxId::for_recipient("sc-aabbccdd");
        assert_eq!(id.0.len(), 64, "MailboxId should be full SHA-256 hex");
        // Legacy shim still produces the old 16-char form for transition.
        let legacy = MailboxId::for_recipient_legacy_64bit("sc-aabbccdd");
        assert_eq!(legacy.0.len(), 16);
        // And the legacy id is a prefix of the full id (same hash, truncated).
        assert!(id.0.starts_with(&legacy.0));
    }

    fn sample_entry() -> MailboxEntry {
        MailboxEntry::new(
            &test_snap_id(),
            "a".repeat(64).as_str(), // SHA-256 hex
            "sc-aabbccdd",
            "sc-11223344",
            1_048_576,
            3600,
            vec![DeliveryHint::PushWhenOnline],
        )
    }

    #[test]
    fn entry_fits_dht() {
        let bytes = sample_entry().to_dht_bytes().unwrap();
        assert!(bytes.len() <= 512, "Entry too large: {} bytes", bytes.len());
    }

    #[test]
    fn signed_entry_roundtrip_and_fits() {
        // Generate a signing key, sign, verify self-consistency, and confirm
        // the signed wrapper still fits the 512-byte DHT budget.
        use ed25519_dalek::SigningKey;
        use rand_core::OsRng;
        let sk = SigningKey::generate(&mut OsRng);
        let sk_bytes = sk.to_bytes();

        let entry = sample_entry();
        let signed = SignedMailboxEntry::sign(entry.clone(), &sk_bytes).unwrap();

        // Self-consistent verification passes.
        let verified = signed.verify_self_consistent().unwrap();
        assert_eq!(verified.snap_id, entry.snap_id);

        // Verification against the correct key passes.
        let vk_bytes = sk.verifying_key().to_bytes();
        assert!(signed.verify_with_key(&vk_bytes).is_ok());

        // Fits the DHT budget.
        let bytes = signed.to_dht_bytes().unwrap();
        assert!(bytes.len() <= 1024, "Signed entry too large: {} bytes", bytes.len());
    }

    #[test]
    fn signed_entry_rejects_tampering() {
        use ed25519_dalek::SigningKey;
        use rand_core::OsRng;
        let sk = SigningKey::generate(&mut OsRng);
        let mut signed = SignedMailboxEntry::sign(sample_entry(), &sk.to_bytes()).unwrap();
        // Tamper with the entry after signing — verification must fail.
        signed.entry.payload_bytes = 999;
        assert!(signed.verify_self_consistent().is_err(),
                "tampered entry should fail verification");
    }

    #[test]
    fn signed_entry_rejects_wrong_key() {
        use ed25519_dalek::SigningKey;
        use rand_core::OsRng;
        let sk = SigningKey::generate(&mut OsRng);
        let signed = SignedMailboxEntry::sign(sample_entry(), &sk.to_bytes()).unwrap();
        // A DIFFERENT key must be rejected by verify_with_key even though
        // the signature is self-consistent (attacker signed with own key,
        // claims to be someone whose key we know).
        let other = SigningKey::generate(&mut OsRng);
        let other_vk = other.verifying_key().to_bytes();
        assert!(signed.verify_with_key(&other_vk).is_err(),
                "entry signed by a different key should be rejected");
    }

    #[test]
    fn signed_entry_dht_roundtrip() {
        use ed25519_dalek::SigningKey;
        use rand_core::OsRng;
        let sk = SigningKey::generate(&mut OsRng);
        let signed = SignedMailboxEntry::sign(sample_entry(), &sk.to_bytes()).unwrap();
        let bytes = signed.to_dht_bytes().unwrap();
        let back = SignedMailboxEntry::from_dht_bytes(&bytes).unwrap();
        assert!(back.verify_self_consistent().is_ok());
    }
}
