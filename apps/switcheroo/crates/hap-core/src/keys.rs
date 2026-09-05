//! The primitives pair-setup, pair-verify and the session compose, and the
//! long-term pairing record that survives between them.
//!
//! Nothing here decides anything about the protocol; each function is one
//! spec sentence made callable, so the state machines read like the spec.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha512;

/// `HKDF-SHA-512(salt, ikm, info)` → 32 bytes. Every key HAP derives is
/// exactly this shape (spec §5.6.6.1 and friends).
pub fn hkdf_32(salt: &[u8], info: &[u8], ikm: &[u8]) -> [u8; 32] {
    let hk = Hkdf::<Sha512>::new(Some(salt), ikm);
    let mut out = [0u8; 32];
    hk.expand(info, &mut out).expect("32 bytes is within HKDF-SHA512's limit");
    out
}

/// A 96-bit ChaCha20-Poly1305 nonce from HAP's 8-byte labels ("PS-Msg05",
/// "PV-Msg02"): four zero bytes then the label. Spec §5.6.6.2.
pub fn label_nonce(label: &[u8; 8]) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(label);
    n
}

/// A 96-bit nonce from a 64-bit counter: four zero bytes then the counter
/// little-endian. What the encrypted session uses (spec §6.5.2, §7.4.6).
pub fn counter_nonce(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_le_bytes());
    n
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AeadError;

impl std::fmt::Display for AeadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ChaCha20-Poly1305 authentication failed")
    }
}

/// Encrypt; the 16-byte Poly1305 tag is appended to the ciphertext.
pub fn seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let c = ChaCha20Poly1305::new(Key::from_slice(key));
    c.encrypt(Nonce::from_slice(nonce), Payload { msg: plaintext, aad }).expect("encryption is infallible for in-memory buffers")
}

/// Decrypt and verify the appended tag.
pub fn open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, AeadError> {
    let c = ChaCha20Poly1305::new(Key::from_slice(key));
    c.decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad }).map_err(|_| AeadError)
}

/// The controller's long-term Ed25519 identity plus what it learned about
/// one accessory in pair-setup. This is the record whose loss means
/// factory-resetting the plug (HOMEKIT.md), so it round-trips through a
/// fixed byte layout with a version byte rather than through a format that
/// might drift.
#[derive(Clone, PartialEq, Eq)]
pub struct Pairing {
    /// Our pairing identifier — HAP wants a UUID-shaped string, e.g.
    /// "2C3E5F1A-0B7D-4A19-9E11-3D2F0A6B8C41". 36 ASCII bytes.
    pub controller_id: String,
    /// Our long-term Ed25519 secret; the public key is derived from it.
    pub controller_ltsk: [u8; 32],
    /// The accessory's pairing identifier as it reported it in M6.
    pub accessory_id: String,
    /// The accessory's long-term Ed25519 public key from M6.
    pub accessory_ltpk: [u8; 32],
}

impl std::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The secret key never goes to a log.
        f.debug_struct("Pairing")
            .field("controller_id", &self.controller_id)
            .field("accessory_id", &self.accessory_id)
            .field("accessory_ltpk", &hex(&self.accessory_ltpk))
            .finish()
    }
}

const PAIRING_MAGIC: &[u8; 4] = b"HAP1";

impl Pairing {
    pub fn controller_ltpk(&self) -> [u8; 32] {
        ed25519_dalek::SigningKey::from_bytes(&self.controller_ltsk).verifying_key().to_bytes()
    }

    /// `HAP1` | u8 len | controller_id | 32 ltsk | u8 len | accessory_id | 32 ltpk.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 1 + 36 + 32 + 1 + 36 + 32);
        out.extend_from_slice(PAIRING_MAGIC);
        out.push(self.controller_id.len() as u8);
        out.extend_from_slice(self.controller_id.as_bytes());
        out.extend_from_slice(&self.controller_ltsk);
        out.push(self.accessory_id.len() as u8);
        out.extend_from_slice(self.accessory_id.as_bytes());
        out.extend_from_slice(&self.accessory_ltpk);
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Pairing> {
        let mut i = 0;
        let take = |i: &mut usize, n: usize| -> Option<&[u8]> {
            let s = b.get(*i..*i + n)?;
            *i += n;
            Some(s)
        };
        if take(&mut i, 4)? != PAIRING_MAGIC { return None; }
        let n = *take(&mut i, 1)?.first()? as usize;
        let controller_id = String::from_utf8(take(&mut i, n)?.to_vec()).ok()?;
        let controller_ltsk: [u8; 32] = take(&mut i, 32)?.try_into().ok()?;
        let n = *take(&mut i, 1)?.first()? as usize;
        let accessory_id = String::from_utf8(take(&mut i, n)?.to_vec()).ok()?;
        let accessory_ltpk: [u8; 32] = take(&mut i, 32)?.try_into().ok()?;
        if i != b.len() { return None; }
        Some(Pairing { controller_id, controller_ltsk, accessory_id, accessory_ltpk })
    }
}

/// A fresh controller identity: random UUID-shaped id, random Ed25519 key.
/// Made once per install and then kept; every accessory paired afterwards
/// stores this identity.
pub fn new_controller_identity() -> (String, [u8; 32]) {
    let sk = ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng);
    let mut r = [0u8; 16];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut r);
    // RFC 4122 v4 layout, upper-case hex, dashed — the shape iOS uses.
    r[6] = (r[6] & 0x0F) | 0x40;
    r[8] = (r[8] & 0x3F) | 0x80;
    let h = hex(&r).to_ascii_uppercase();
    let id = format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]);
    (id, sk.to_bytes())
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonces_are_four_zeros_then_the_payload() {
        assert_eq!(label_nonce(b"PS-Msg05"), [0, 0, 0, 0, b'P', b'S', b'-', b'M', b's', b'g', b'0', b'5']);
        assert_eq!(counter_nonce(1), [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(counter_nonce(0x0102), [0, 0, 0, 0, 2, 1, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn seal_then_open_round_trips_and_tampering_is_caught() {
        let key = hkdf_32(b"salt", b"info", b"ikm");
        let n = label_nonce(b"PV-Msg02");
        let ct = seal(&key, &n, b"", b"hello");
        assert_eq!(ct.len(), 5 + 16);
        assert_eq!(open(&key, &n, b"", &ct).unwrap(), b"hello");
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert_eq!(open(&key, &n, b"", &bad), Err(AeadError));
        assert_eq!(open(&key, &counter_nonce(0), b"", &ct), Err(AeadError), "wrong nonce");
    }

    #[test]
    fn hkdf_is_deterministic_and_salt_sensitive() {
        assert_eq!(hkdf_32(b"a", b"i", b"k"), hkdf_32(b"a", b"i", b"k"));
        assert_ne!(hkdf_32(b"a", b"i", b"k"), hkdf_32(b"b", b"i", b"k"));
    }

    #[test]
    fn pairing_record_round_trips_and_rejects_garbage() {
        let (id, sk) = new_controller_identity();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        let p = Pairing { controller_id: id, controller_ltsk: sk, accessory_id: "AA:BB:CC:DD:EE:FF".into(), accessory_ltpk: [9; 32] };
        let b = p.to_bytes();
        assert_eq!(Pairing::from_bytes(&b).unwrap(), p);
        assert!(Pairing::from_bytes(&b[..b.len() - 1]).is_none());
        assert!(Pairing::from_bytes(b"HAP2....").is_none());
        assert!(!format!("{p:?}").contains(&hex(&sk)), "the secret never prints");
    }
}
