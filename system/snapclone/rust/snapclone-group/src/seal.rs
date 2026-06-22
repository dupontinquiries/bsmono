//! seal.rs — the two encryption operations group messaging needs, both built
//! ONLY from vetted primitives (§3(1): reuse, don't reinvent).
//!
//!   1. `seal_sender_key` / `open_sender_key`: encrypt a 32-byte sender-key
//!      root to ONE recipient, over the 1:1 X25519 shared secret. This is how
//!      a member hands its epoch sender key to each other member (§1.3). Small,
//!      so N-way fan-out is cheap.
//!
//!   2. `encrypt_content` / `decrypt_content`: AES-256-GCM under a per-message
//!      key `MK_i` from the ratchet. This encrypts the actual text or media
//!      blob ONCE per message regardless of group size (§1.2).
//!
//! Both use AES-256-GCM with a random 12-byte nonce prepended to the
//! ciphertext (§3(3): random-per-message nonce; sender keys never reused across
//! epochs so (key,nonce) can't collide).

use crate::labels;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hkdf::Hkdf;
use rand_core::{CryptoRng, RngCore};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

#[derive(Debug, PartialEq, Eq)]
pub enum SealError {
    Encrypt,
    Decrypt,
    BadLength,
}

const NONCE_LEN: usize = 12;

/// AES-256-GCM seal: returns nonce(12) || ciphertext(+16 tag).
fn aead_seal<R: RngCore + CryptoRng>(
    key: &[u8; 32],
    plaintext: &[u8],
    rng: &mut R,
) -> Result<Vec<u8>, SealError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher.encrypt(nonce, plaintext).map_err(|_| SealError::Encrypt)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// AES-256-GCM open of nonce(12) || ciphertext.
fn aead_open(key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>, SealError> {
    if sealed.len() < NONCE_LEN + 16 {
        return Err(SealError::BadLength);
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(&sealed[..NONCE_LEN]);
    cipher
        .decrypt(nonce, &sealed[NONCE_LEN..])
        .map_err(|_| SealError::Decrypt)
}

/// Derive the per-recipient seal key from the X25519 shared secret + a
/// dedicated label, so it can't collide with the session key derived for the
/// live transport (§3(4): no key reuse across roles).
fn seal_key_from_shared(shared: &[u8; 32]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, shared);
    let mut out = [0u8; 32];
    hk.expand(labels::SENDERKEY_SEAL, &mut out)
        .expect("HKDF expand never fails for 32 bytes");
    out
}

/// Seal a 32-byte sender-key root to a recipient. `my_secret` is OUR X25519
/// static secret; `their_public` is the recipient's X25519 public key.
pub fn seal_sender_key<R: RngCore + CryptoRng>(
    my_secret: &StaticSecret,
    their_public: &PublicKey,
    sender_key_root: &[u8; 32],
    rng: &mut R,
) -> Result<Vec<u8>, SealError> {
    let shared = my_secret.diffie_hellman(their_public);
    let mut sk = seal_key_from_shared(shared.as_bytes());
    let res = aead_seal(&sk, sender_key_root, rng);
    sk.zeroize();
    res
}

/// Open a sealed sender-key root. `my_secret` is OUR X25519 static secret;
/// `their_public` is the SENDER's X25519 public key. Returns the 32-byte root.
pub fn open_sender_key(
    my_secret: &StaticSecret,
    their_public: &PublicKey,
    sealed: &[u8],
) -> Result<[u8; 32], SealError> {
    let shared = my_secret.diffie_hellman(their_public);
    let mut sk = seal_key_from_shared(shared.as_bytes());
    let pt = aead_open(&sk, sealed);
    sk.zeroize();
    let pt = pt?;
    if pt.len() != 32 {
        return Err(SealError::BadLength);
    }
    let mut root = [0u8; 32];
    root.copy_from_slice(&pt);
    Ok(root)
}

/// Encrypt group content (text bytes or a media blob) under a per-message key.
pub fn encrypt_content<R: RngCore + CryptoRng>(
    message_key: &[u8; 32],
    plaintext: &[u8],
    rng: &mut R,
) -> Result<Vec<u8>, SealError> {
    aead_seal(message_key, plaintext, rng)
}

/// Decrypt group content under a per-message key.
pub fn decrypt_content(message_key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>, SealError> {
    aead_open(message_key, sealed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Deterministic test RNG (NOT for production).
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

    fn keypair(seed: u8) -> (StaticSecret, PublicKey) {
        let sk = StaticSecret::from([seed; 32]);
        let pk = PublicKey::from(&sk);
        (sk, pk)
    }

    #[test]
    fn sender_key_seal_roundtrip() {
        let mut rng = DummyRng(1);
        let (alice_sk, alice_pk) = keypair(1);
        let (bob_sk, bob_pk) = keypair(2);
        let root = [42u8; 32];

        // Alice seals to Bob.
        let sealed = seal_sender_key(&alice_sk, &bob_pk, &root, &mut rng).unwrap();
        // Bob opens with his secret + Alice's public.
        let opened = open_sender_key(&bob_sk, &alice_pk, &sealed).unwrap();
        assert_eq!(opened, root);
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let mut rng = DummyRng(2);
        let (alice_sk, _alice_pk) = keypair(1);
        let (_bob_sk, bob_pk) = keypair(2);
        let (eve_sk, _eve_pk) = keypair(9);
        let root = [42u8; 32];

        let sealed = seal_sender_key(&alice_sk, &bob_pk, &root, &mut rng).unwrap();
        // Eve tries to open with HER secret — the shared secret differs, GCM
        // auth fails.
        let (alice_sk2, alice_pk2) = keypair(1);
        let _ = alice_sk2;
        assert!(open_sender_key(&eve_sk, &alice_pk2, &sealed).is_err());
    }

    #[test]
    fn content_roundtrip() {
        let mut rng = DummyRng(3);
        let mk = [5u8; 32];
        let msg = b"hello group, this is a photo caption";
        let ct = encrypt_content(&mk, msg, &mut rng).unwrap();
        assert_ne!(&ct[12..], &msg[..]); // actually encrypted
        let pt = decrypt_content(&mk, &ct).unwrap();
        assert_eq!(pt, msg);
    }

    #[test]
    fn content_wrong_key_fails() {
        let mut rng = DummyRng(4);
        let ct = encrypt_content(&[1u8; 32], b"secret", &mut rng).unwrap();
        assert!(decrypt_content(&[2u8; 32], &ct).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let mut rng = DummyRng(5);
        let mut ct = encrypt_content(&[7u8; 32], b"integrity", &mut rng).unwrap();
        let last = ct.len() - 1;
        ct[last] ^= 0x01; // flip a tag bit
        assert!(decrypt_content(&[7u8; 32], &ct).is_err());
    }

    #[test]
    fn nonce_makes_ciphertext_unique() {
        let mut rng = DummyRng(6);
        let mk = [8u8; 32];
        let a = encrypt_content(&mk, b"same", &mut rng).unwrap();
        let b = encrypt_content(&mk, b"same", &mut rng).unwrap();
        assert_ne!(a, b); // different random nonce → different bytes
    }
}
