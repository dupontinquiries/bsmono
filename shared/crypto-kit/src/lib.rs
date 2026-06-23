//! crypto-kit — shared cryptographic primitives for the monorepo.
//!
//! Lifted from `snapclone-crypto`, presented with two faces:
//!   * [`ffi`] — the original C-ABI surface (`sc_*`), unchanged, so the C++/Qt
//!     consumers (snapclone) stay ABI- and wire-compatible;
//!   * the crate root — a safe, idiomatic **Rust** API over the *same* scheme,
//!     for Rust consumers (share-kit, unendlich, …).
//!
//! Scheme (best-of-breed, post-quantum hybrid):
//!   * Identity / auth : Ed25519
//!   * Key agreement   : X25519 ECDH **hybridised** with ML-KEM-768 (FIPS 203) —
//!     both shared secrets are concatenated and HKDF-SHA256-expanded, so an
//!     attacker must break *both* primitives to recover the session key;
//!   * AEAD            : AES-256-GCM, output `[12-byte nonce][ciphertext+16-byte tag]`.
//!
//! All secret material is wrapped in `Zeroizing` so it is scrubbed on drop. The
//! HKDF domain-separation label is kept identical to snapclone's wire format.

pub mod ffi;

pub use ffi::{
    CryptoError, KEY_LEN, MLKEM_CT_LEN, MLKEM_DK_LEN, MLKEM_EK_LEN, MLKEM_SS_LEN, NONCE_LEN,
    PRIV_KEY_LEN, PUB_KEY_LEN, SIG_LEN,
};

use aes_gcm::aead::Aead;
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use fips203::ml_kem_768;
use fips203::traits::{Decaps as _, Encaps as _, KeyGen as _, SerDes as _};
use hkdf::Hkdf;
use rand_core::OsRng;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

/// HKDF domain-separation label for the hybrid session key. Kept byte-identical
/// to snapclone's wire format so the two interoperate.
pub const HYBRID_SESSION_INFO: &[u8] = b"snapclone-v1-pq-session";

// ── X25519 ───────────────────────────────────────────────────────────────────

/// Generate an X25519 static keypair → (secret, public).
pub fn x25519_keypair() -> (Zeroizing<[u8; PRIV_KEY_LEN]>, [u8; PUB_KEY_LEN]) {
    let secret = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret);
    (Zeroizing::new(secret.to_bytes()), public.to_bytes())
}

/// X25519 Diffie–Hellman shared secret.
pub fn x25519_shared(
    my_secret: &[u8; PRIV_KEY_LEN],
    their_public: &[u8; PUB_KEY_LEN],
) -> Zeroizing<[u8; KEY_LEN]> {
    let secret = StaticSecret::from(*my_secret);
    let public = PublicKey::from(*their_public);
    Zeroizing::new(secret.diffie_hellman(&public).to_bytes())
}

// ── HKDF / hybrid session key ────────────────────────────────────────────────

/// HKDF-SHA256-expand a shared secret into a 32-byte session key under `info`.
pub fn derive_session_key(shared: &[u8], info: &[u8]) -> Zeroizing<[u8; KEY_LEN]> {
    let hk = Hkdf::<Sha256>::new(None, shared);
    let mut okm = Zeroizing::new([0u8; KEY_LEN]);
    hk.expand(info, okm.as_mut()).expect("HKDF expand of 32 bytes never fails");
    okm
}

/// Derive the hybrid post-quantum session key from the X25519 and ML-KEM shared
/// secrets: `HKDF-SHA256(x25519_ss ‖ mlkem_ss, HYBRID_SESSION_INFO)`.
pub fn derive_hybrid_session_key(
    x25519_ss: &[u8; KEY_LEN],
    mlkem_ss: &[u8; MLKEM_SS_LEN],
) -> Zeroizing<[u8; KEY_LEN]> {
    let mut combined = Zeroizing::new([0u8; KEY_LEN + MLKEM_SS_LEN]);
    combined[..KEY_LEN].copy_from_slice(x25519_ss);
    combined[KEY_LEN..].copy_from_slice(mlkem_ss);
    derive_session_key(combined.as_ref(), HYBRID_SESSION_INFO)
}

// ── ML-KEM-768 (FIPS 203) ────────────────────────────────────────────────────

/// Generate an ML-KEM-768 keypair → (encapsulation key, decapsulation key).
pub fn mlkem_keypair() -> ([u8; MLKEM_EK_LEN], Zeroizing<[u8; MLKEM_DK_LEN]>) {
    let (ek, dk) = ml_kem_768::KG::try_keygen().expect("ml-kem-768 keygen");
    (ek.into_bytes(), Zeroizing::new(dk.into_bytes()))
}

/// Encapsulate against a peer's encapsulation key → (shared secret, ciphertext).
pub fn mlkem_encapsulate(
    peer_ek: &[u8; MLKEM_EK_LEN],
) -> Result<(Zeroizing<[u8; MLKEM_SS_LEN]>, [u8; MLKEM_CT_LEN]), CryptoError> {
    let ek = ml_kem_768::EncapsKey::try_from_bytes(*peer_ek).map_err(|_| CryptoError::InvalidInput)?;
    let (ssk, ct) = ek.try_encaps().map_err(|_| CryptoError::EncryptFailed)?;
    Ok((Zeroizing::new(ssk.into_bytes()), ct.into_bytes()))
}

/// Decapsulate a ciphertext with our decapsulation key → shared secret.
pub fn mlkem_decapsulate(
    our_dk: &[u8; MLKEM_DK_LEN],
    ct: &[u8; MLKEM_CT_LEN],
) -> Result<Zeroizing<[u8; MLKEM_SS_LEN]>, CryptoError> {
    let dk = ml_kem_768::DecapsKey::try_from_bytes(*our_dk).map_err(|_| CryptoError::InvalidInput)?;
    let ct = ml_kem_768::CipherText::try_from_bytes(*ct).map_err(|_| CryptoError::InvalidInput)?;
    let ss = dk.try_decaps(&ct).map_err(|_| CryptoError::DecryptFailed)?;
    Ok(Zeroizing::new(ss.into_bytes()))
}

// ── Ed25519 ──────────────────────────────────────────────────────────────────

/// Generate an Ed25519 signing keypair → (signing key, verifying key).
pub fn ed25519_keypair() -> (Zeroizing<[u8; PRIV_KEY_LEN]>, [u8; PUB_KEY_LEN]) {
    let sk = SigningKey::generate(&mut OsRng);
    let vk = sk.verifying_key();
    (Zeroizing::new(sk.to_bytes()), vk.to_bytes())
}

/// Sign a message with an Ed25519 signing key.
pub fn sign(signing_key: &[u8; PRIV_KEY_LEN], msg: &[u8]) -> [u8; SIG_LEN] {
    let sk = SigningKey::from_bytes(signing_key);
    sk.sign(msg).to_bytes()
}

/// Verify an Ed25519 signature. Returns `false` on any error (bad key or sig).
pub fn verify(verifying_key: &[u8; PUB_KEY_LEN], msg: &[u8], sig: &[u8; SIG_LEN]) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(verifying_key) else {
        return false;
    };
    vk.verify(msg, &Signature::from_bytes(sig)).is_ok()
}

// ── AEAD (AES-256-GCM) ───────────────────────────────────────────────────────

/// Seal `plaintext` under `key`. Output layout: `[12-byte nonce][ciphertext+16-byte tag]`.
pub fn seal(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(*key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ct = cipher.encrypt(&nonce, plaintext).expect("AES-256-GCM encrypt");
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

/// Open a blob produced by [`seal`]. Returns `None` if it is too short or if
/// authentication fails.
pub fn open(key: &[u8; KEY_LEN], sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < NONCE_LEN + 16 {
        return None;
    }
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(*key));
    let nonce = Nonce::from_slice(&sealed[..NONCE_LEN]);
    cipher.decrypt(nonce, &sealed[NONCE_LEN..]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aead_seal_open_roundtrip_and_tamper() {
        let key = [7u8; KEY_LEN];
        let pt = b"hello world";
        let sealed = seal(&key, pt);
        assert_ne!(&sealed[NONCE_LEN..], &pt[..], "payload must be encrypted");
        assert_eq!(open(&key, &sealed).as_deref(), Some(&pt[..]));
        assert!(open(&[8u8; KEY_LEN], &sealed).is_none(), "wrong key must fail");
        let mut tampered = sealed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(open(&key, &tampered).is_none(), "tamper must fail the tag");
    }

    #[test]
    fn ed25519_sign_verify_and_reject() {
        let (sk, vk) = ed25519_keypair();
        let msg = b"sign me";
        let sig = sign(&sk, msg);
        assert!(verify(&vk, msg, &sig));
        assert!(!verify(&vk, b"other message", &sig), "wrong message must fail");
        let (_, vk2) = ed25519_keypair();
        assert!(!verify(&vk2, msg, &sig), "wrong key must fail");
    }

    #[test]
    fn x25519_agreement_matches_both_sides() {
        let (a_sk, a_pk) = x25519_keypair();
        let (b_sk, b_pk) = x25519_keypair();
        let a_shared = x25519_shared(&a_sk, &b_pk);
        let b_shared = x25519_shared(&b_sk, &a_pk);
        assert_eq!(a_shared.as_ref(), b_shared.as_ref());
    }

    #[test]
    fn mlkem_encaps_decaps_roundtrip() {
        let (ek, dk) = mlkem_keypair();
        let (ss_enc, ct) = mlkem_encapsulate(&ek).unwrap();
        let ss_dec = mlkem_decapsulate(&dk, &ct).unwrap();
        assert_eq!(ss_enc.as_ref(), ss_dec.as_ref());
    }

    #[test]
    fn hybrid_session_key_needs_both_halves() {
        let (a_xsk, a_xpk) = x25519_keypair();
        let (b_xsk, b_xpk) = x25519_keypair();
        let (b_ek, b_dk) = mlkem_keypair();
        // A side
        let a_x = x25519_shared(&a_xsk, &b_xpk);
        let (a_pq, ct) = mlkem_encapsulate(&b_ek).unwrap();
        let a_key = derive_hybrid_session_key(&a_x, &a_pq);
        // B side
        let b_x = x25519_shared(&b_xsk, &a_xpk);
        let b_pq = mlkem_decapsulate(&b_dk, &ct).unwrap();
        let b_key = derive_hybrid_session_key(&b_x, &b_pq);
        assert_eq!(a_key.as_ref(), b_key.as_ref(), "both sides derive the same key");
        // breaking either half changes the key
        let wrong_pq = derive_hybrid_session_key(&a_x, &[0u8; MLKEM_SS_LEN]);
        assert_ne!(a_key.as_ref(), wrong_pq.as_ref());
        let wrong_x = derive_hybrid_session_key(&[0u8; KEY_LEN], &a_pq);
        assert_ne!(a_key.as_ref(), wrong_x.as_ref());
    }

    #[test]
    fn end_to_end_hybrid_session_seals_and_opens() {
        // Agree a hybrid PQ key, seal under it, open on the other side.
        let (a_xsk, a_xpk) = x25519_keypair();
        let (b_xsk, b_xpk) = x25519_keypair();
        let (b_ek, b_dk) = mlkem_keypair();
        let a_x = x25519_shared(&a_xsk, &b_xpk);
        let (a_pq, ct) = mlkem_encapsulate(&b_ek).unwrap();
        let a_key = derive_hybrid_session_key(&a_x, &a_pq);
        let sealed = seal(&a_key, b"secret payload");

        let b_x = x25519_shared(&b_xsk, &a_xpk);
        let b_pq = mlkem_decapsulate(&b_dk, &ct).unwrap();
        let b_key = derive_hybrid_session_key(&b_x, &b_pq);
        assert_eq!(open(&b_key, &sealed).as_deref(), Some(&b"secret payload"[..]));
    }
}
