//! Chain-of-hops type and verification logic.
//!
//! A `RelayChain` is an ordered sequence of `ChainEntry` records.
//! Each entry attests "I, node `my_id`, received this blob from
//! `prev_hop_id` at `signed_at_ms` and forwarded it." Each entry is
//! ed25519-signed by `my_id`'s key over the deterministic encoding of
//! the entry's fields.
//!
//! Verification at the recipient:
//!
//!   1. Chain length ≤ MAX_HOP_DEPTH
//!   2. Every entry's signature validates against its claimed `my_id`
//!      public key (pulled from the recipient's contact graph)
//!   3. Adjacent entries chain correctly:
//!      - entry[i+1].prev_hop_id == entry[i].my_id
//!      - entry[i+1].signed_at_ms >= entry[i].signed_at_ms
//!   4. No node appears twice (loop prevention)
//!   5. All hops are within the contact-of-contact reachable set
//!      (enforced by recipient before accepting; the chain proves
//!       cryptographic identity, the contact graph proves trust)
//!
//! Verification is offline once the chain is in hand — no need to ask
//! anyone in the chain about anything.

use crate::limits::MAX_HOP_DEPTH;
use serde::{Deserialize, Serialize};

/// One hop's signed declaration. Append-only — once a hop signs its
/// entry, no later hop can modify it without breaking the signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainEntry {
    /// Public key of the previous hop (or the original sender for the
    /// first entry). 32 zero bytes when this entry is the FIRST hop,
    /// meaning the originating sender handed the blob directly to us.
    pub prev_hop_id: [u8; 32],

    /// Public key of this hop — the node that signed this entry.
    pub my_id: [u8; 32],

    /// Unix millis when this hop accepted the blob for forwarding.
    pub signed_at_ms: i64,

    /// 32-byte SHA-256 of the ENCRYPTED blob. Used so each hop can
    /// confirm they forwarded the same payload that earlier hops
    /// signed — a hop can't substitute different ciphertext mid-chain
    /// without breaking the verifiability for later checks.
    pub blob_hash: [u8; 32],

    /// 64-byte ed25519 signature over the deterministic byte encoding
    /// of (prev_hop_id || my_id || signed_at_ms || blob_hash).
    pub signature: [u8; 64],
}

impl ChainEntry {
    /// Byte size of the SIGNED PORTION (everything ed25519 covers).
    /// 32 + 32 + 8 + 32 = 104 bytes. Useful for buffer sizing during
    /// verification.
    pub const SIGNED_BYTES: usize = 32 + 32 + 8 + 32;

    /// Total byte size including the 64-byte signature.
    pub const TOTAL_BYTES: usize = Self::SIGNED_BYTES + 64;

    /// Serialize the signed-portion of this entry into the canonical
    /// byte form. The output is what an ed25519 signer would sign or
    /// a verifier would verify against. Network order; no length
    /// prefixes since all fields are fixed size.
    pub fn signed_bytes(&self) -> [u8; Self::SIGNED_BYTES] {
        let mut out = [0u8; Self::SIGNED_BYTES];
        out[0..32].copy_from_slice(&self.prev_hop_id);
        out[32..64].copy_from_slice(&self.my_id);
        out[64..72].copy_from_slice(&self.signed_at_ms.to_be_bytes());
        out[72..104].copy_from_slice(&self.blob_hash);
        out
    }
}

/// Ordered sequence of hops, oldest-first.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelayChain {
    /// Entries in order: entries[0] = first hop after the sender,
    /// entries.last() = the hop currently holding the blob.
    pub entries: Vec<ChainEntry>,
}

impl RelayChain {
    /// Empty chain — meaning the blob is still with the original sender.
    pub fn new() -> Self { Self { entries: Vec::new() } }

    /// Length of the chain. 0 means "not yet relayed once."
    pub fn len(&self) -> usize { self.entries.len() }

    /// Same idiom as `Vec::is_empty` so clippy doesn't grumble at us.
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// Quick policy check: is appending one more hop allowed?
    /// Returns false once the chain reaches MAX_HOP_DEPTH.
    pub fn can_extend(&self) -> bool { self.entries.len() < MAX_HOP_DEPTH }

    /// Append an entry to the chain. Performs the cheap structural
    /// checks (hop limit, prev_hop_id matches the current tail's
    /// my_id, no immediate loop). Does NOT verify the signature —
    /// signature checks are done by `verify` over the whole chain.
    pub fn append(&mut self, entry: ChainEntry) -> Result<(), ChainError> {
        if !self.can_extend() {
            return Err(ChainError::HopLimitExceeded { len: self.entries.len() });
        }
        if let Some(tail) = self.entries.last() {
            if entry.prev_hop_id != tail.my_id {
                return Err(ChainError::PrevHopMismatch);
            }
            if entry.signed_at_ms < tail.signed_at_ms {
                return Err(ChainError::NonMonotonicTimestamp);
            }
        }
        // Loop prevention: refuse if my_id already appears.
        if self.entries.iter().any(|e| e.my_id == entry.my_id) {
            return Err(ChainError::Loop);
        }
        self.entries.push(entry);
        Ok(())
    }

    /// Returns the id of the hop currently holding the blob, or
    /// None if the chain is empty.
    pub fn current_holder(&self) -> Option<&[u8; 32]> {
        self.entries.last().map(|e| &e.my_id)
    }

    /// Full chain verification. V32.32: structural checks only —
    /// ed25519 signature checks are stubbed until the identity
    /// integration lands.
    ///
    /// `blob_hash` should be the SHA-256 of the encrypted blob the
    /// recipient just received. The chain is rejected if any entry's
    /// declared blob_hash differs (ciphertext substitution attempt).
    pub fn verify(&self, blob_hash: &[u8; 32]) -> Result<(), ChainError> {
        if self.entries.is_empty() {
            // Empty chain means "delivered directly" — caller's responsibility
            // to decide if that's expected. Verification is trivially Ok.
            return Ok(());
        }
        if self.entries.len() > MAX_HOP_DEPTH {
            return Err(ChainError::HopLimitExceeded { len: self.entries.len() });
        }
        // Adjacent-entry chain checks.
        for window in self.entries.windows(2) {
            let prev = &window[0];
            let next = &window[1];
            if next.prev_hop_id != prev.my_id {
                return Err(ChainError::PrevHopMismatch);
            }
            if next.signed_at_ms < prev.signed_at_ms {
                return Err(ChainError::NonMonotonicTimestamp);
            }
        }
        // Loop prevention: no id appears twice.
        let mut seen: Vec<&[u8; 32]> = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            if seen.iter().any(|s| **s == entry.my_id) {
                return Err(ChainError::Loop);
            }
            seen.push(&entry.my_id);
        }
        // Blob-hash consistency: every entry must agree on the hash
        // of what they forwarded. A mid-chain substitution attempt
        // would break this.
        for entry in &self.entries {
            if entry.blob_hash != *blob_hash {
                return Err(ChainError::BlobHashMismatch);
            }
        }
        // V32.32: skip signature verification — pending integration
        // with snapclone-identity. Adding ad-hoc ed25519 here would
        // duplicate key-handling and risk inconsistency.
        // TODO: pk = identity::lookup(&entry.my_id)
        //       pk.verify(&entry.signed_bytes(), &entry.signature)
        Ok(())
    }
}

/// Chain-specific errors. Distinct from PiggybackError so callers
/// can match on chain failure modes specifically.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ChainError {
    /// Chain has more hops than the configured maximum.
    #[error("hop limit exceeded: chain has {len} hops, max is {}", MAX_HOP_DEPTH)]
    HopLimitExceeded { len: usize },

    /// An entry's prev_hop_id doesn't match the previous entry's my_id.
    /// Indicates the chain has been spliced or reordered.
    #[error("prev_hop_id doesn't match previous entry's my_id")]
    PrevHopMismatch,

    /// An entry's timestamp is earlier than the previous entry's.
    /// Either the chain is reordered or a hop's clock was rolled back.
    #[error("non-monotonic timestamp in chain")]
    NonMonotonicTimestamp,

    /// Same node appears twice — a loop attempt.
    #[error("chain contains a loop")]
    Loop,

    /// An entry's blob_hash doesn't match the actual blob the recipient
    /// received. Someone mid-chain substituted ciphertext.
    #[error("blob_hash mismatch within chain")]
    BlobHashMismatch,

    /// Signature failed cryptographic verification.
    /// (Reserved — not produced by stub verification yet.)
    #[error("entry signature invalid")]
    BadSignature,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(prev: u8, me: u8, ts: i64, blob: u8) -> ChainEntry {
        ChainEntry {
            prev_hop_id: [prev; 32],
            my_id: [me; 32],
            signed_at_ms: ts,
            blob_hash: [blob; 32],
            signature: [0u8; 64],
        }
    }

    #[test]
    fn append_ok_when_chain_consistent() {
        let mut chain = RelayChain::new();
        chain.append(entry(0, 1, 100, 42)).unwrap();
        chain.append(entry(1, 2, 200, 42)).unwrap();
        chain.append(entry(2, 3, 300, 42)).unwrap();
        assert_eq!(chain.len(), 3);
    }

    #[test]
    fn append_rejects_loop() {
        let mut chain = RelayChain::new();
        chain.append(entry(0, 1, 100, 42)).unwrap();
        chain.append(entry(1, 2, 200, 42)).unwrap();
        let err = chain.append(entry(2, 1, 300, 42)).unwrap_err();
        assert!(matches!(err, ChainError::Loop));
    }

    #[test]
    fn append_rejects_prev_mismatch() {
        let mut chain = RelayChain::new();
        chain.append(entry(0, 1, 100, 42)).unwrap();
        // prev_hop=9 doesn't match tail.my_id=1
        let err = chain.append(entry(9, 2, 200, 42)).unwrap_err();
        assert!(matches!(err, ChainError::PrevHopMismatch));
    }

    #[test]
    fn append_rejects_at_limit() {
        let mut chain = RelayChain::new();
        // Fill to MAX_HOP_DEPTH.
        let mut prev = 0;
        for i in 1..=MAX_HOP_DEPTH as u8 {
            chain.append(entry(prev, i, 100 + i as i64, 42)).unwrap();
            prev = i;
        }
        // Next append should fail.
        let err = chain
            .append(entry(prev, prev + 1, 1000, 42))
            .unwrap_err();
        assert!(matches!(err, ChainError::HopLimitExceeded { .. }));
    }

    #[test]
    fn verify_passes_consistent_chain() {
        let mut chain = RelayChain::new();
        chain.append(entry(0, 1, 100, 42)).unwrap();
        chain.append(entry(1, 2, 200, 42)).unwrap();
        chain.verify(&[42; 32]).unwrap();
    }

    #[test]
    fn verify_catches_blob_hash_mismatch() {
        let mut chain = RelayChain::new();
        chain.append(entry(0, 1, 100, 42)).unwrap();
        // Recipient received a blob with hash 99, not 42.
        let err = chain.verify(&[99; 32]).unwrap_err();
        assert!(matches!(err, ChainError::BlobHashMismatch));
    }
}
