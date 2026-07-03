//! Fixed-size encrypted envelope — prevents size-based traffic analysis.
//!
//! NETWORK_ARCHITECTURE.md §5: "normalize network messages, fixed-size
//! encrypted envelopes."
//!
//! All snap payloads are padded to a multiple of ENVELOPE_BLOCK (4KB)
//! before encryption. This means an observer can't distinguish a text
//! snap from a photo snap by observing packet sizes on the wire.

use anyhow::Result;

/// Block size for envelope padding (4 KB).
pub const ENVELOPE_BLOCK: usize = 4096;

/// Maximum snap payload in bytes (40 MB).
pub const MAX_PAYLOAD: usize = 40 * 1024 * 1024;

/// A fixed-size padded envelope around an encrypted payload.
/// Layout: [4 bytes real_len (BE)][payload][random padding to next 4KB boundary]
#[derive(Debug, Clone)]
pub struct EncryptedEnvelope(pub Vec<u8>);

impl EncryptedEnvelope {
    /// Pad `payload` to the next ENVELOPE_BLOCK boundary.
    pub fn pack(payload: &[u8]) -> Result<Self> {
        if payload.len() > MAX_PAYLOAD {
            anyhow::bail!("Payload too large: {} bytes", payload.len());
        }
        let real_len = payload.len() as u32;
        let padded_len = ((payload.len() + 4 + ENVELOPE_BLOCK - 1) / ENVELOPE_BLOCK) * ENVELOPE_BLOCK;
        let mut buf = vec![0u8; padded_len];
        // Write real length
        buf[..4].copy_from_slice(&real_len.to_be_bytes());
        buf[4..4 + payload.len()].copy_from_slice(payload);
        // V32.34+ (S-6): fill padding with CRYPTOGRAPHICALLY RANDOM bytes.
        //
        // The previous implementation seeded a non-crypto DefaultHasher with
        // SystemTime and wrote a structured ramp (`(seed >> i%8) + i`). That
        // is fully predictable given an approximate timestamp, so the padding
        // provided NO defense against a compression/known-plaintext attack on
        // the (separately-encrypted) envelope — despite the comment claiming
        // it did. OsRng draws from the OS CSPRNG, giving padding an attacker
        // can't predict or distinguish from ciphertext. We reach OsRng via
        // ed25519-dalek's re-exported rand_core to avoid a direct getrandom
        // dependency edge (toolchain pin reasons — see Cargo.toml).
        let pad = &mut buf[4 + payload.len()..];
        if !pad.is_empty() {
            use rand_core::{OsRng, RngCore};
            OsRng.fill_bytes(pad);
        }
        Ok(Self(buf))
    }

    /// Unpack and return the real payload.
    pub fn unpack(&self) -> Result<Vec<u8>> {
        if self.0.len() < 4 { anyhow::bail!("Envelope too short"); }
        let real_len = u32::from_be_bytes(self.0[..4].try_into()?) as usize;
        if real_len > MAX_PAYLOAD { anyhow::bail!("Real length too large"); }
        if self.0.len() < 4 + real_len { anyhow::bail!("Envelope truncated"); }
        Ok(self.0[4..4 + real_len].to_vec())
    }

    pub fn padded_len(&self) -> usize { self.0.len() }
}

/// Convenience: wrap plaintext into a fixed-size envelope.
/// Caller encrypts the returned bytes; envelope padding happens first.
pub fn fixed_size_envelope(plaintext: &[u8]) -> Result<Vec<u8>> {
    Ok(EncryptedEnvelope::pack(plaintext)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packs_to_block_boundary() {
        let data = b"hello"; // 5 bytes
        let env = EncryptedEnvelope::pack(data).unwrap();
        assert_eq!(env.padded_len() % ENVELOPE_BLOCK, 0);
        assert_eq!(env.padded_len(), ENVELOPE_BLOCK); // one block
    }
    #[test]
    fn roundtrip() {
        let data: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
        let env = EncryptedEnvelope::pack(&data).unwrap();
        assert_eq!(env.padded_len() % ENVELOPE_BLOCK, 0);
        assert_eq!(env.unpack().unwrap(), data);
    }
    #[test]
    fn large_payload_blocked() {
        let data = vec![0u8; MAX_PAYLOAD + 1];
        assert!(EncryptedEnvelope::pack(&data).is_err());
    }

    #[test]
    fn padding_is_nondeterministic() {
        // S-6: two packs of the SAME payload must differ in their padding
        // region (CSPRNG), proving we're not writing a predictable ramp.
        let data = b"same payload";
        let a = EncryptedEnvelope::pack(data).unwrap();
        let b = EncryptedEnvelope::pack(data).unwrap();
        // The header (len) + payload bytes are identical; the padding after
        // them must differ with overwhelming probability.
        let pad_start = 4 + data.len();
        assert_eq!(a.0[..pad_start], b.0[..pad_start], "header+payload identical");
        assert_ne!(a.0[pad_start..], b.0[pad_start..],
                   "padding should be random, not deterministic");
        // And both still unpack correctly.
        assert_eq!(a.unpack().unwrap(), data);
        assert_eq!(b.unpack().unwrap(), data);
    }
}
