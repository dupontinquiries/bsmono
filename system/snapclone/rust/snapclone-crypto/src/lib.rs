//! snapclone-crypto — Rust crypto backend for SnapClone
//!
//! Exposes a C-compatible FFI surface so the Qt/C++ layer can call real
//! cryptographic primitives without linking OpenSSL.
//!
//! Scheme:
//!   • Identity  : Ed25519 keypair (signing / gossip authentication)
//!   • Key exchange: X25519 ECDH → HKDF-SHA256 → 32-byte session key
//!   • Encryption : AES-256-GCM (nonce prepended, tag appended)
//!
//! All key material is wrapped in Zeroize so it is scrubbed on drop.
//!
//! # Safety
//! All `*mut u8` / `*const u8` pointers in the FFI functions are caller-
//! owned and must remain valid for the duration of the call.  No Rust heap
//! memory is returned to C++ — outputs are written into caller-provided
//! buffers whose lengths are validated before writing.

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey, Signature, Verifier};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;
use std::slice;

// V32.33+: post-quantum hybrid KEM. ML-KEM-768 (FIPS 203) provides
// lattice-based key encapsulation. Used IN HYBRID with X25519 — the
// session key derives from BOTH shared secrets concatenated. Breaks
// only if BOTH primitives are broken.
//
// fips203 crate API:
//   • ml_kem_768::KG::try_keygen() → Result<(EncapsKey, DecapsKey)>
//   • encaps_key.try_encaps() → Result<(SharedSecretKey, CipherText)>
//   • decaps_key.try_decaps(&ct) → Result<SharedSecretKey>
//
// All serializations are plain byte arrays via into_bytes() (consuming
// to_bytes) and try_from_bytes (returning Result):
//   • EncapsKey   ↔  [u8; 1184]
//   • DecapsKey   ↔  [u8; 2400]
//   • CipherText  ↔  [u8; 1088]
//   • SharedSecretKey ↔  [u8; 32]
//
// The traits `KeyGen`, `Encaps`, `Decaps`, `SerDes` provide the methods.
use fips203::ml_kem_768;
use fips203::traits::{KeyGen as _, Encaps as _, Decaps as _, SerDes as _};

// ─── DHT peer discovery module ────────────────────────────────────────────────
// Only compiled when SNAPCLONE_OPENDHT_BACKEND is set (via build.rs env check).
// The module is always present in source but its extern "C" functions are only

// ─── Constants ────────────────────────────────────────────────────────────────

pub const NONCE_LEN:   usize = 12; // AES-GCM 96-bit nonce
pub const KEY_LEN:     usize = 32; // AES-256 / X25519 key size
pub const SIG_LEN:     usize = 64; // Ed25519 signature
pub const PUB_KEY_LEN: usize = 32; // X25519 / Ed25519 public key
pub const PRIV_KEY_LEN:usize = 32; // X25519 / Ed25519 private key

// ─── ML-KEM-768 size constants (FIPS 203) ────────────────────────────────────
// Encapsulation key (public)  : 1184 bytes
// Decapsulation key (private) : 2400 bytes
// Ciphertext (encapsulated)   : 1088 bytes
// Shared secret               : 32 bytes
pub const MLKEM_EK_LEN: usize = 1184;
pub const MLKEM_DK_LEN: usize = 2400;
pub const MLKEM_CT_LEN: usize = 1088;
pub const MLKEM_SS_LEN: usize = 32;

// ─── Error codes returned to C++ ─────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    Ok               = 0,
    InvalidInput     = 1,
    EncryptFailed    = 2,
    DecryptFailed    = 3,  // includes MAC failure
    InvalidSignature = 4,
    BufferTooSmall   = 5,
}

// ─── Session key derivation ───────────────────────────────────────────────────

/// Derive a 32-byte AES-256 session key from an X25519 shared secret
/// via HKDF-SHA256.  `info` is the domain-separation label.
fn derive_session_key(shared: &[u8], info: &[u8]) -> Zeroizing<[u8; KEY_LEN]> {
    let hk = Hkdf::<Sha256>::new(None, shared);
    let mut okm = Zeroizing::new([0u8; KEY_LEN]);
    hk.expand(info, okm.as_mut()).expect("HKDF expand never fails for 32 bytes");
    okm
}

// ─── FFI: Key generation ──────────────────────────────────────────────────────

/// Generate a new X25519 static keypair.
///
/// # Safety
/// `priv_out` must point to at least `PRIV_KEY_LEN` bytes.
/// `pub_out`  must point to at least `PUB_KEY_LEN` bytes.
#[no_mangle]
pub unsafe extern "C" fn sc_generate_x25519_keypair(
    priv_out: *mut u8,
    pub_out:  *mut u8,
) -> CryptoError {
    if priv_out.is_null() || pub_out.is_null() {
        return CryptoError::InvalidInput;
    }
    let secret = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret);

    let priv_slice = slice::from_raw_parts_mut(priv_out, PRIV_KEY_LEN);
    let pub_slice  = slice::from_raw_parts_mut(pub_out,  PUB_KEY_LEN);
    priv_slice.copy_from_slice(secret.as_bytes());
    pub_slice.copy_from_slice(public.as_bytes());
    CryptoError::Ok
}

/// Generate a new Ed25519 signing keypair.
///
/// # Safety
/// `priv_out` must point to at least `PRIV_KEY_LEN` bytes.
/// `pub_out`  must point to at least `PUB_KEY_LEN` bytes.
#[no_mangle]
pub unsafe extern "C" fn sc_generate_ed25519_keypair(
    priv_out: *mut u8,
    pub_out:  *mut u8,
) -> CryptoError {
    if priv_out.is_null() || pub_out.is_null() {
        return CryptoError::InvalidInput;
    }
    let signing = SigningKey::generate(&mut OsRng);
    let verifying = signing.verifying_key();

    let priv_slice = slice::from_raw_parts_mut(priv_out, PRIV_KEY_LEN);
    let pub_slice  = slice::from_raw_parts_mut(pub_out,  PUB_KEY_LEN);
    priv_slice.copy_from_slice(signing.as_bytes());
    pub_slice.copy_from_slice(verifying.as_bytes());
    CryptoError::Ok
}

// ─── FFI: ECDH session key derivation ────────────────────────────────────────

/// Derive a 32-byte session key from our X25519 private key and the peer's
/// X25519 public key.
///
/// # Safety
/// All pointers must be valid and non-null.
/// `our_priv`  : `PRIV_KEY_LEN` bytes (X25519 private key).
/// `peer_pub`  : `PUB_KEY_LEN`  bytes (X25519 public key).
/// `key_out`   : `KEY_LEN`      bytes (output AES-256 key).
#[no_mangle]
pub unsafe extern "C" fn sc_derive_session_key(
    our_priv:  *const u8,
    peer_pub:  *const u8,
    key_out:   *mut u8,
) -> CryptoError {
    if our_priv.is_null() || peer_pub.is_null() || key_out.is_null() {
        return CryptoError::InvalidInput;
    }

    let priv_bytes: [u8; PRIV_KEY_LEN] = slice::from_raw_parts(our_priv, PRIV_KEY_LEN)
        .try_into().unwrap();
    let pub_bytes: [u8; PUB_KEY_LEN] = slice::from_raw_parts(peer_pub, PUB_KEY_LEN)
        .try_into().unwrap();

    let our_secret   = StaticSecret::from(priv_bytes);
    let their_public = PublicKey::from(pub_bytes);
    let shared       = Zeroizing::new(our_secret.diffie_hellman(&their_public));

    let session_key = derive_session_key(shared.as_bytes(), b"snapclone-v1-session");

    let out_slice = slice::from_raw_parts_mut(key_out, KEY_LEN);
    out_slice.copy_from_slice(session_key.as_ref());
    CryptoError::Ok
}

/// Compute the RAW X25519 shared secret — no HKDF applied.
///
/// Use when the caller wants to combine this with another shared secret
/// (e.g. ML-KEM-768 output for the hybrid construction) and then run
/// HKDF over the concatenation via sc_derive_hybrid_session_key.
///
/// For standalone session-key derivation, use sc_derive_session_key
/// instead — it bundles HKDF in one call.
///
/// # Safety
/// `our_priv`     : PRIV_KEY_LEN bytes (X25519 private key).
/// `peer_pub`     : PUB_KEY_LEN  bytes (X25519 public key).
/// `shared_out`   : KEY_LEN      bytes (raw 32-byte X25519 DH output).
#[no_mangle]
pub unsafe extern "C" fn sc_x25519_raw_shared(
    our_priv:    *const u8,
    peer_pub:    *const u8,
    shared_out:  *mut u8,
) -> CryptoError {
    if our_priv.is_null() || peer_pub.is_null() || shared_out.is_null() {
        return CryptoError::InvalidInput;
    }
    let priv_slice = slice::from_raw_parts(our_priv, PRIV_KEY_LEN);
    let pub_slice  = slice::from_raw_parts(peer_pub, PUB_KEY_LEN);
    let priv_bytes: [u8; PRIV_KEY_LEN] = match priv_slice.try_into() {
        Ok(a)  => a,
        Err(_) => return CryptoError::InvalidInput,
    };
    let pub_bytes: [u8; PUB_KEY_LEN] = match pub_slice.try_into() {
        Ok(a)  => a,
        Err(_) => return CryptoError::InvalidInput,
    };
    let our_secret   = StaticSecret::from(priv_bytes);
    let their_public = PublicKey::from(pub_bytes);
    let shared       = Zeroizing::new(our_secret.diffie_hellman(&their_public));

    slice::from_raw_parts_mut(shared_out, KEY_LEN).copy_from_slice(shared.as_bytes());
    CryptoError::Ok
}

// ─── FFI: Encryption ──────────────────────────────────────────────────────────

/// Encrypt `plaintext_len` bytes with AES-256-GCM.
///
/// Output layout: [12-byte nonce][ciphertext][16-byte GCM tag]
/// So `cipher_out` must hold at least `plaintext_len + NONCE_LEN + 16` bytes.
///
/// # Safety
/// All pointers must be valid.
#[no_mangle]
pub unsafe extern "C" fn sc_encrypt_aes256gcm(
    key:           *const u8,   // KEY_LEN bytes
    plaintext:     *const u8,
    plaintext_len: usize,
    cipher_out:    *mut u8,
    cipher_cap:    usize,       // capacity of cipher_out
    cipher_len:    *mut usize,  // written: actual output length
) -> CryptoError {
    if key.is_null() || plaintext.is_null() || cipher_out.is_null() || cipher_len.is_null() {
        return CryptoError::InvalidInput;
    }

    let required = NONCE_LEN + plaintext_len + 16; // 16 = GCM tag
    if cipher_cap < required {
        return CryptoError::BufferTooSmall;
    }

    let key_bytes: [u8; KEY_LEN] = slice::from_raw_parts(key, KEY_LEN)
        .try_into().unwrap();
    let pt_slice = slice::from_raw_parts(plaintext, plaintext_len);

    let aes_key  = Key::<Aes256Gcm>::from(key_bytes);
    let cipher   = Aes256Gcm::new(&aes_key);
    let nonce    = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = match cipher.encrypt(&nonce, pt_slice) {
        Ok(ct) => ct,
        Err(_) => return CryptoError::EncryptFailed,
    };

    let out = slice::from_raw_parts_mut(cipher_out, cipher_cap);
    out[..NONCE_LEN].copy_from_slice(&nonce);
    out[NONCE_LEN..NONCE_LEN + ciphertext.len()].copy_from_slice(&ciphertext);
    *cipher_len = NONCE_LEN + ciphertext.len();

    CryptoError::Ok
}

// ─── FFI: Decryption ──────────────────────────────────────────────────────────

/// Decrypt AES-256-GCM ciphertext.
///
/// Input layout: [12-byte nonce][ciphertext][16-byte GCM tag]
/// Output: plaintext written to `plain_out`.
///
/// # Safety
/// All pointers must be valid.
#[no_mangle]
pub unsafe extern "C" fn sc_decrypt_aes256gcm(
    key:        *const u8,  // KEY_LEN bytes
    ciphertext: *const u8,  // nonce || ct || tag
    cipher_len: usize,
    plain_out:  *mut u8,
    plain_cap:  usize,
    plain_len:  *mut usize,
) -> CryptoError {
    if key.is_null() || ciphertext.is_null() || plain_out.is_null() || plain_len.is_null() {
        return CryptoError::InvalidInput;
    }
    if cipher_len <= NONCE_LEN + 16 {
        return CryptoError::InvalidInput;
    }

    let key_bytes: [u8; KEY_LEN] = slice::from_raw_parts(key, KEY_LEN)
        .try_into().unwrap();
    let ct_slice = slice::from_raw_parts(ciphertext, cipher_len);

    let nonce_bytes: [u8; NONCE_LEN] = ct_slice[..NONCE_LEN].try_into().unwrap();
    let nonce       = Nonce::from(nonce_bytes);
    let ct_body     = &ct_slice[NONCE_LEN..]; // includes tag

    let aes_key  = Key::<Aes256Gcm>::from(key_bytes);
    let cipher   = Aes256Gcm::new(&aes_key);

    let plaintext = match cipher.decrypt(&nonce, ct_body) {
        Ok(pt) => pt,
        Err(_) => return CryptoError::DecryptFailed, // MAC failure
    };

    if plain_cap < plaintext.len() {
        return CryptoError::BufferTooSmall;
    }

    let out = slice::from_raw_parts_mut(plain_out, plain_cap);
    out[..plaintext.len()].copy_from_slice(&plaintext);
    *plain_len = plaintext.len();

    CryptoError::Ok
}

// ─── FFI: Signatures ──────────────────────────────────────────────────────────

/// Sign `data_len` bytes with an Ed25519 private key.
/// `sig_out` must hold at least `SIG_LEN` (64) bytes.
///
/// # Safety
/// All pointers must be valid.
#[no_mangle]
pub unsafe extern "C" fn sc_sign_ed25519(
    priv_key: *const u8, // PRIV_KEY_LEN bytes
    data:     *const u8,
    data_len: usize,
    sig_out:  *mut u8,   // SIG_LEN bytes output
) -> CryptoError {
    if priv_key.is_null() || data.is_null() || sig_out.is_null() {
        return CryptoError::InvalidInput;
    }

    let priv_bytes: [u8; PRIV_KEY_LEN] = slice::from_raw_parts(priv_key, PRIV_KEY_LEN)
        .try_into().unwrap();
    let data_slice = slice::from_raw_parts(data, data_len);

    let signing = SigningKey::from_bytes(&priv_bytes);
    let sig: Signature = signing.sign(data_slice);

    let out = slice::from_raw_parts_mut(sig_out, SIG_LEN);
    out.copy_from_slice(&sig.to_bytes());
    CryptoError::Ok
}

/// Verify an Ed25519 signature.  Returns `CryptoError::Ok` on success.
///
/// # Safety
/// All pointers must be valid.
#[no_mangle]
pub unsafe extern "C" fn sc_verify_ed25519(
    pub_key:  *const u8, // PUB_KEY_LEN bytes
    data:     *const u8,
    data_len: usize,
    sig:      *const u8, // SIG_LEN bytes
) -> CryptoError {
    if pub_key.is_null() || data.is_null() || sig.is_null() {
        return CryptoError::InvalidInput;
    }

    let pub_bytes: [u8; PUB_KEY_LEN] = slice::from_raw_parts(pub_key, PUB_KEY_LEN)
        .try_into().unwrap();
    let data_slice = slice::from_raw_parts(data, data_len);
    let sig_bytes: [u8; SIG_LEN] = slice::from_raw_parts(sig, SIG_LEN)
        .try_into().unwrap();

    let verifying = match VerifyingKey::from_bytes(&pub_bytes) {
        Ok(vk) => vk,
        Err(_) => return CryptoError::InvalidInput,
    };
    let signature = Signature::from_bytes(&sig_bytes);

    match verifying.verify(data_slice, &signature) {
        Ok(_)  => CryptoError::Ok,
        Err(_) => CryptoError::InvalidSignature,
    }
}

// ─── Constant queries ─────────────────────────────────────────────────────────

#[no_mangle] pub extern "C" fn sc_nonce_len()    -> usize { NONCE_LEN    }
#[no_mangle] pub extern "C" fn sc_key_len()      -> usize { KEY_LEN      }
#[no_mangle] pub extern "C" fn sc_sig_len()      -> usize { SIG_LEN      }
#[no_mangle] pub extern "C" fn sc_pub_key_len()  -> usize { PUB_KEY_LEN  }
#[no_mangle] pub extern "C" fn sc_priv_key_len() -> usize { PRIV_KEY_LEN }
#[no_mangle] pub extern "C" fn sc_mlkem_ek_len() -> usize { MLKEM_EK_LEN }
#[no_mangle] pub extern "C" fn sc_mlkem_dk_len() -> usize { MLKEM_DK_LEN }
#[no_mangle] pub extern "C" fn sc_mlkem_ct_len() -> usize { MLKEM_CT_LEN }
#[no_mangle] pub extern "C" fn sc_mlkem_ss_len() -> usize { MLKEM_SS_LEN }

// ─── ML-KEM-768 (post-quantum hybrid KEM, FIPS 203) ──────────────────────────
//
// The protocol:
//
//   Initiator (Alice):
//     1. Generate ML-KEM-768 keypair (dk_A, ek_A).
//     2. Send ek_A (1184 bytes) to responder in the SHAK (alongside her
//        X25519 public key).
//     3. Receive ct_B (1088 bytes ML-KEM ciphertext) from responder.
//     4. Decapsulate: ss = decap(dk_A, ct_B). 32 bytes.
//
//   Responder (Bob):
//     1. Receive ek_A.
//     2. Encapsulate: (ct_B, ss) = encap(ek_A). 1088 + 32 bytes.
//     3. Send ct_B back.
//
// Both sides now have the same 32-byte `ss` (ML-KEM shared secret).
// In the hybrid construction, this is concatenated with the X25519
// shared secret and HKDF-expanded:
//
//   session_key = HKDF(x25519_shared || mlkem_shared, "snapclone-v1-pq-session")
//
// If a quantum computer breaks X25519 → mlkem_shared still secret →
// session_key still secret. If future cryptanalysis breaks ML-KEM →
// x25519_shared still secret → session_key still secret. Both must
// fall for the session to be compromised.
//
// IMPORTANT: ML-KEM is an EPHEMERAL KEM in the way we use it here.
// Each session generates a fresh keypair on the initiator side; the
// responder's encapsulation key is short-lived too. This means PQ
// forward secrecy: even if the long-term keys are compromised later,
// past sessions can't be decrypted from harvested ciphertext —
// because the ML-KEM secret isn't recoverable from anything that
// went on the wire.
//
// (The X25519 side uses a STATIC long-term key on each peer, which
// means no classical forward secrecy. Fixing classical forward
// secrecy is a separate concern — needs ephemeral X25519 keys per
// session. Filed as future work; the PQ side gives us PQ-FS for
// free even without the classical side being ephemeral.)

/// Generate a new ML-KEM-768 keypair.
///
/// # Safety
/// `dk_out` must point to at least MLKEM_DK_LEN (2400) bytes.
/// `ek_out` must point to at least MLKEM_EK_LEN (1184) bytes.
#[no_mangle]
pub unsafe extern "C" fn sc_mlkem_generate_keypair(
    dk_out: *mut u8,
    ek_out: *mut u8,
) -> CryptoError {
    if dk_out.is_null() || ek_out.is_null() {
        return CryptoError::InvalidInput;
    }
    // fips203: try_keygen() uses default-rng feature to draw from getrandom.
    let (ek, dk) = match ml_kem_768::KG::try_keygen() {
        Ok(pair) => pair,
        Err(_)   => return CryptoError::EncryptFailed,
    };
    // into_bytes() consumes and returns [u8; N] arrays of the FIPS 203
    // declared sizes. No allocation, no length-mismatch surprise.
    let ek_bytes = ek.into_bytes();
    let dk_bytes = dk.into_bytes();
    // Compile-time sanity check via const assertions: if fips203 ever
    // changes byte sizes, the array-length mismatch will fail to build.
    let _: &[u8; MLKEM_DK_LEN] = &dk_bytes;
    let _: &[u8; MLKEM_EK_LEN] = &ek_bytes;

    slice::from_raw_parts_mut(dk_out, MLKEM_DK_LEN).copy_from_slice(&dk_bytes);
    slice::from_raw_parts_mut(ek_out, MLKEM_EK_LEN).copy_from_slice(&ek_bytes);
    CryptoError::Ok
}

/// Encapsulate a shared secret to a peer's ML-KEM-768 encapsulation key.
/// Called by the RESPONDER side of the handshake (Bob).
///
/// # Safety
/// `peer_ek`  : MLKEM_EK_LEN (1184) bytes — peer's encapsulation key.
/// `ct_out`   : MLKEM_CT_LEN (1088) bytes — ciphertext we send back.
/// `ss_out`   : MLKEM_SS_LEN (32)   bytes — shared secret we derive locally.
#[no_mangle]
pub unsafe extern "C" fn sc_mlkem_encapsulate(
    peer_ek: *const u8,
    ct_out:  *mut u8,
    ss_out:  *mut u8,
) -> CryptoError {
    if peer_ek.is_null() || ct_out.is_null() || ss_out.is_null() {
        return CryptoError::InvalidInput;
    }
    // Copy into a fixed-size array so we can hand it to try_from_bytes.
    let ek_slice = slice::from_raw_parts(peer_ek, MLKEM_EK_LEN);
    let ek_array: [u8; MLKEM_EK_LEN] = match ek_slice.try_into() {
        Ok(a)  => a,
        Err(_) => return CryptoError::InvalidInput,
    };

    // Deserialize. FIPS 203 §7.2 requires public key validation prior
    // to encapsulation (modulus check on the polynomial coefficients).
    // The fips203 crate's try_encaps performs this — try_from_bytes
    // succeeds for any well-formed byte sequence, with full validation
    // happening at encapsulation time. Either path produces an error
    // for malformed input, which we surface as InvalidInput.
    let ek = match ml_kem_768::EncapsKey::try_from_bytes(ek_array) {
        Ok(k)  => k,
        Err(_) => return CryptoError::InvalidInput,
    };

    let (ssk, ct) = match ek.try_encaps() {
        Ok(pair) => pair,
        Err(_)   => return CryptoError::EncryptFailed,
    };

    let ct_bytes = ct.into_bytes();
    let ss_bytes = ssk.into_bytes();
    let _: &[u8; MLKEM_CT_LEN] = &ct_bytes;
    let _: &[u8; MLKEM_SS_LEN] = &ss_bytes;

    slice::from_raw_parts_mut(ct_out, MLKEM_CT_LEN).copy_from_slice(&ct_bytes);
    slice::from_raw_parts_mut(ss_out, MLKEM_SS_LEN).copy_from_slice(&ss_bytes);
    CryptoError::Ok
}

/// Decapsulate an ML-KEM-768 ciphertext to recover the shared secret.
/// Called by the INITIATOR side (Alice) after receiving ct from Bob.
///
/// # Safety
/// `our_dk` : MLKEM_DK_LEN (2400) bytes — our decapsulation key.
/// `ct`     : MLKEM_CT_LEN (1088) bytes — ciphertext from peer.
/// `ss_out` : MLKEM_SS_LEN (32)   bytes — shared secret.
#[no_mangle]
pub unsafe extern "C" fn sc_mlkem_decapsulate(
    our_dk: *const u8,
    ct:     *const u8,
    ss_out: *mut u8,
) -> CryptoError {
    if our_dk.is_null() || ct.is_null() || ss_out.is_null() {
        return CryptoError::InvalidInput;
    }
    let dk_slice = slice::from_raw_parts(our_dk, MLKEM_DK_LEN);
    let ct_slice = slice::from_raw_parts(ct, MLKEM_CT_LEN);

    let dk_array: [u8; MLKEM_DK_LEN] = match dk_slice.try_into() {
        Ok(a)  => a,
        Err(_) => return CryptoError::InvalidInput,
    };
    let ct_array: [u8; MLKEM_CT_LEN] = match ct_slice.try_into() {
        Ok(a)  => a,
        Err(_) => return CryptoError::InvalidInput,
    };

    let dk = match ml_kem_768::DecapsKey::try_from_bytes(dk_array) {
        Ok(k)  => k,
        Err(_) => return CryptoError::InvalidInput,
    };
    let ct_obj = match ml_kem_768::CipherText::try_from_bytes(ct_array) {
        Ok(c)  => c,
        Err(_) => return CryptoError::InvalidInput,
    };

    let ssk = match dk.try_decaps(&ct_obj) {
        Ok(s)  => s,
        Err(_) => return CryptoError::DecryptFailed,
    };

    let ss_bytes = ssk.into_bytes();
    let _: &[u8; MLKEM_SS_LEN] = &ss_bytes;
    slice::from_raw_parts_mut(ss_out, MLKEM_SS_LEN).copy_from_slice(&ss_bytes);
    CryptoError::Ok
}

/// Hybrid session key derivation: HKDF over the concatenation of
/// X25519 shared secret and ML-KEM shared secret. Returns a 32-byte
/// AES-256 session key.
///
/// The concatenation order matters for interoperability: x25519_shared
/// first, then mlkem_shared. Both peers MUST use the same order.
///
/// # Safety
/// `x25519_shared` : KEY_LEN (32) bytes
/// `mlkem_shared`  : MLKEM_SS_LEN (32) bytes
/// `key_out`       : KEY_LEN (32) bytes
#[no_mangle]
pub unsafe extern "C" fn sc_derive_hybrid_session_key(
    x25519_shared: *const u8,
    mlkem_shared:  *const u8,
    key_out:       *mut u8,
) -> CryptoError {
    if x25519_shared.is_null() || mlkem_shared.is_null() || key_out.is_null() {
        return CryptoError::InvalidInput;
    }
    let x_slice = slice::from_raw_parts(x25519_shared, KEY_LEN);
    let m_slice = slice::from_raw_parts(mlkem_shared, MLKEM_SS_LEN);

    let mut combined = Zeroizing::new([0u8; KEY_LEN + MLKEM_SS_LEN]);
    combined[..KEY_LEN].copy_from_slice(x_slice);
    combined[KEY_LEN..].copy_from_slice(m_slice);

    let session_key = derive_session_key(combined.as_ref(), b"snapclone-v1-pq-session");
    slice::from_raw_parts_mut(key_out, KEY_LEN).copy_from_slice(session_key.as_ref());
    CryptoError::Ok
}

// ─── Tests ────────────────────────────────────────────────────────────────────
//
// `cargo test --features test-only` runs these. The FFI surface is exercised
// exactly as the C++ side will use it (raw pointers, fixed-size buffers,
// error codes — no Rust idioms leaking through). If a test passes here, the
// C++ side calling the same function with valid inputs will see the same
// behavior.

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip: Alice generates an ML-KEM-768 keypair. Bob encapsulates
    /// against her ek. Alice decapsulates Bob's ct. Both must arrive at the
    /// same 32-byte shared secret.
    #[test]
    fn mlkem_round_trip() {
        let mut dk = vec![0u8; MLKEM_DK_LEN];
        let mut ek = vec![0u8; MLKEM_EK_LEN];
        let mut ct = vec![0u8; MLKEM_CT_LEN];
        let mut ss_bob   = vec![0u8; MLKEM_SS_LEN];
        let mut ss_alice = vec![0u8; MLKEM_SS_LEN];

        unsafe {
            let err = sc_mlkem_generate_keypair(dk.as_mut_ptr(), ek.as_mut_ptr());
            assert!(matches!(err, CryptoError::Ok), "keygen failed");

            let err = sc_mlkem_encapsulate(
                ek.as_ptr(), ct.as_mut_ptr(), ss_bob.as_mut_ptr());
            assert!(matches!(err, CryptoError::Ok), "encapsulate failed");

            let err = sc_mlkem_decapsulate(
                dk.as_ptr(), ct.as_ptr(), ss_alice.as_mut_ptr());
            assert!(matches!(err, CryptoError::Ok), "decapsulate failed");
        }

        // The FIPS 203 contract: encap and decap produce the SAME 32-byte
        // shared secret for matched (ek, dk) and a faithfully-transmitted ct.
        assert_eq!(ss_bob, ss_alice, "shared secrets diverge");

        // Sanity: the SS should not be all zeros (would indicate uninitialized).
        assert!(ss_bob.iter().any(|&b| b != 0), "shared secret is all zeros");
    }

    /// Hybrid: feed a known X25519 shared and a known ML-KEM shared through
    /// sc_derive_hybrid_session_key on "both sides" and verify they agree.
    /// In a real run both sides will derive THE SAME inputs via the X25519
    /// DH + ML-KEM round-trip; here we just verify the HKDF is deterministic
    /// over its inputs.
    #[test]
    fn hybrid_session_key_determinism() {
        let x_shared = [0x42u8; KEY_LEN];
        let m_shared = [0x88u8; MLKEM_SS_LEN];

        let mut k1 = vec![0u8; KEY_LEN];
        let mut k2 = vec![0u8; KEY_LEN];

        unsafe {
            assert!(matches!(sc_derive_hybrid_session_key(
                x_shared.as_ptr(), m_shared.as_ptr(), k1.as_mut_ptr()),
                CryptoError::Ok));
            assert!(matches!(sc_derive_hybrid_session_key(
                x_shared.as_ptr(), m_shared.as_ptr(), k2.as_mut_ptr()),
                CryptoError::Ok));
        }
        assert_eq!(k1, k2, "HKDF is non-deterministic — bug");
        assert!(k1.iter().any(|&b| b != 0));
    }

    /// Different inputs must produce different outputs (HKDF correctness).
    #[test]
    fn hybrid_session_key_input_separation() {
        let x_shared = [0x42u8; KEY_LEN];
        let m_shared_a = [0x88u8; MLKEM_SS_LEN];
        let m_shared_b = [0x99u8; MLKEM_SS_LEN];

        let mut ka = vec![0u8; KEY_LEN];
        let mut kb = vec![0u8; KEY_LEN];

        unsafe {
            sc_derive_hybrid_session_key(x_shared.as_ptr(), m_shared_a.as_ptr(), ka.as_mut_ptr());
            sc_derive_hybrid_session_key(x_shared.as_ptr(), m_shared_b.as_ptr(), kb.as_mut_ptr());
        }
        assert_ne!(ka, kb, "different ML-KEM secrets must yield different session keys");
    }

    /// Concatenation order matters. Confirm that (x_a, m_b) ≠ (x_b, m_a)
    /// even if the underlying byte values are otherwise swapped. This
    /// guards against a regression where someone "simplifies" the
    /// derivation by sorting the inputs or hashing them together —
    /// both sides MUST use the same order or they'll arrive at
    /// different keys without an obvious failure mode.
    #[test]
    fn hybrid_session_key_order_matters() {
        let a = [0x11u8; KEY_LEN];
        let b = [0x22u8; MLKEM_SS_LEN];   // same length but different bytes

        let mut k_ab = vec![0u8; KEY_LEN];
        let mut k_ba = vec![0u8; KEY_LEN];
        unsafe {
            sc_derive_hybrid_session_key(a.as_ptr(), b.as_ptr(), k_ab.as_mut_ptr());
            // Swap: feed `b` as "x25519" position and `a` as "mlkem" position.
            // (We can do this because KEY_LEN == MLKEM_SS_LEN == 32 happens
            // to hold. The test fails-by-design if those lengths ever diverge.)
            sc_derive_hybrid_session_key(b.as_ptr(), a.as_ptr(), k_ba.as_mut_ptr());
        }
        assert_ne!(k_ab, k_ba, "order MUST matter — both sides need same convention");
    }

    /// Verify FFI null-pointer safety: passing null returns InvalidInput,
    /// not a segfault.
    #[test]
    fn mlkem_null_pointer_safety() {
        let mut buf = vec![0u8; MLKEM_EK_LEN];
        unsafe {
            assert!(matches!(
                sc_mlkem_generate_keypair(std::ptr::null_mut(), buf.as_mut_ptr()),
                CryptoError::InvalidInput));
            assert!(matches!(
                sc_mlkem_generate_keypair(buf.as_mut_ptr(), std::ptr::null_mut()),
                CryptoError::InvalidInput));
            assert!(matches!(
                sc_mlkem_encapsulate(std::ptr::null(), buf.as_mut_ptr(), buf.as_mut_ptr()),
                CryptoError::InvalidInput));
            assert!(matches!(
                sc_mlkem_decapsulate(std::ptr::null(), std::ptr::null(), buf.as_mut_ptr()),
                CryptoError::InvalidInput));
        }
    }

    /// Tampered ciphertext: per FIPS 203 §6.3 "implicit rejection",
    /// decapsulating a malformed/tampered ct returns a deterministic
    /// pseudo-random key (not the real shared secret) WITHOUT signaling
    /// failure. The implicit-reject value is baked into the dk.
    ///
    /// What we can verify: Alice's decap on a tampered ct yields a
    /// DIFFERENT secret from Bob's. The session key derivation will
    /// then diverge, which the AEAD tag check at the application layer
    /// will catch — but at the KEM level we don't detect tampering.
    #[test]
    fn mlkem_tampered_ct_yields_different_secret() {
        let mut dk = vec![0u8; MLKEM_DK_LEN];
        let mut ek = vec![0u8; MLKEM_EK_LEN];
        let mut ct = vec![0u8; MLKEM_CT_LEN];
        let mut ss_bob   = vec![0u8; MLKEM_SS_LEN];
        let mut ss_alice = vec![0u8; MLKEM_SS_LEN];

        unsafe {
            sc_mlkem_generate_keypair(dk.as_mut_ptr(), ek.as_mut_ptr());
            sc_mlkem_encapsulate(ek.as_ptr(), ct.as_mut_ptr(), ss_bob.as_mut_ptr());
            ct[0] ^= 0xFF;  // flip a byte
            // Decap of a tampered ct may return Ok (implicit reject) or
            // an error (depending on where the tamper lands). Either way
            // the resulting secret must NOT match Bob's.
            let _ = sc_mlkem_decapsulate(dk.as_ptr(), ct.as_ptr(), ss_alice.as_mut_ptr());
        }
        assert_ne!(ss_bob, ss_alice,
                   "tampered ct decap yielded the original SS — implicit reject failed");
    }

    /// FULL HYBRID HANDSHAKE round-trip. This is the integration test that
    /// matters most — it simulates exactly what SHAK_v2 will do:
    ///
    ///   Alice (initiator):
    ///     - has long-term X25519 keypair
    ///     - generates ephemeral ML-KEM keypair
    ///     - sends (x25519_pub_A, mlkem_ek_A) to Bob
    ///
    ///   Bob (responder):
    ///     - has long-term X25519 keypair
    ///     - receives (x25519_pub_A, mlkem_ek_A)
    ///     - computes ss_dh = X25519(x25519_priv_B, x25519_pub_A)
    ///     - computes (mlkem_ct, ss_pq) = mlkem_encap(mlkem_ek_A)
    ///     - session_key = HKDF(ss_dh || ss_pq, "snapclone-v1-pq-session")
    ///     - sends (x25519_pub_B, mlkem_ct) to Alice
    ///
    ///   Alice (finalize):
    ///     - receives (x25519_pub_B, mlkem_ct)
    ///     - computes ss_dh = X25519(x25519_priv_A, x25519_pub_B)
    ///     - computes ss_pq = mlkem_decap(mlkem_dk_A, mlkem_ct)
    ///     - session_key = HKDF(ss_dh || ss_pq, "snapclone-v1-pq-session")
    ///
    /// Both sides MUST arrive at the same 32-byte session_key.
    #[test]
    fn full_hybrid_handshake() {
        // Long-term X25519 keypairs.
        let mut alice_x_priv = vec![0u8; PRIV_KEY_LEN];
        let mut alice_x_pub  = vec![0u8; PUB_KEY_LEN];
        let mut bob_x_priv   = vec![0u8; PRIV_KEY_LEN];
        let mut bob_x_pub    = vec![0u8; PUB_KEY_LEN];

        // Alice's ephemeral ML-KEM keypair.
        let mut alice_mlkem_dk = vec![0u8; MLKEM_DK_LEN];
        let mut alice_mlkem_ek = vec![0u8; MLKEM_EK_LEN];

        // Wire material Bob sends back.
        let mut mlkem_ct = vec![0u8; MLKEM_CT_LEN];

        // Bob-side computed.
        let mut bob_ss_dh = vec![0u8; KEY_LEN];
        let mut bob_ss_pq = vec![0u8; MLKEM_SS_LEN];
        let mut bob_session = vec![0u8; KEY_LEN];

        // Alice-side computed.
        let mut alice_ss_dh = vec![0u8; KEY_LEN];
        let mut alice_ss_pq = vec![0u8; MLKEM_SS_LEN];
        let mut alice_session = vec![0u8; KEY_LEN];

        unsafe {
            // Setup
            assert!(matches!(
                sc_generate_x25519_keypair(alice_x_priv.as_mut_ptr(), alice_x_pub.as_mut_ptr()),
                CryptoError::Ok));
            assert!(matches!(
                sc_generate_x25519_keypair(bob_x_priv.as_mut_ptr(), bob_x_pub.as_mut_ptr()),
                CryptoError::Ok));
            assert!(matches!(
                sc_mlkem_generate_keypair(alice_mlkem_dk.as_mut_ptr(), alice_mlkem_ek.as_mut_ptr()),
                CryptoError::Ok));

            // Bob receives Alice's x25519_pub + mlkem_ek.
            assert!(matches!(
                sc_x25519_raw_shared(bob_x_priv.as_ptr(), alice_x_pub.as_ptr(), bob_ss_dh.as_mut_ptr()),
                CryptoError::Ok));
            assert!(matches!(
                sc_mlkem_encapsulate(alice_mlkem_ek.as_ptr(), mlkem_ct.as_mut_ptr(), bob_ss_pq.as_mut_ptr()),
                CryptoError::Ok));
            assert!(matches!(
                sc_derive_hybrid_session_key(bob_ss_dh.as_ptr(), bob_ss_pq.as_ptr(), bob_session.as_mut_ptr()),
                CryptoError::Ok));

            // Alice receives Bob's x25519_pub + mlkem_ct.
            assert!(matches!(
                sc_x25519_raw_shared(alice_x_priv.as_ptr(), bob_x_pub.as_ptr(), alice_ss_dh.as_mut_ptr()),
                CryptoError::Ok));
            assert!(matches!(
                sc_mlkem_decapsulate(alice_mlkem_dk.as_ptr(), mlkem_ct.as_ptr(), alice_ss_pq.as_mut_ptr()),
                CryptoError::Ok));
            assert!(matches!(
                sc_derive_hybrid_session_key(alice_ss_dh.as_ptr(), alice_ss_pq.as_ptr(), alice_session.as_mut_ptr()),
                CryptoError::Ok));
        }

        // The crown jewel: both sides must derive the same hybrid session key.
        assert_eq!(bob_session, alice_session,
                   "hybrid handshake failed — bob and alice diverged");

        // Sanity: it should not be all zeros.
        assert!(bob_session.iter().any(|&b| b != 0));

        // Also verify the components match individually (helps debug if
        // the final session key differs — points at which half is broken).
        assert_eq!(bob_ss_dh, alice_ss_dh, "X25519 ECDH diverged");
        assert_eq!(bob_ss_pq, alice_ss_pq, "ML-KEM encap/decap diverged");
    }

    /// What happens if classical and PQ shares disagree? Either side
    /// alone diverging must break the final session key. This guards
    /// against an implementation bug where someone accidentally uses
    /// the X25519-only path on one side.
    #[test]
    fn hybrid_breaks_if_either_half_breaks() {
        let mut ss_dh    = [0x11u8; KEY_LEN];
        let ss_pq        = [0x22u8; MLKEM_SS_LEN];
        let mut key_good = vec![0u8; KEY_LEN];
        let mut key_bad  = vec![0u8; KEY_LEN];

        unsafe {
            sc_derive_hybrid_session_key(ss_dh.as_ptr(), ss_pq.as_ptr(), key_good.as_mut_ptr());
            ss_dh[0] ^= 0xFF;  // flip one byte of the classical share
            sc_derive_hybrid_session_key(ss_dh.as_ptr(), ss_pq.as_ptr(), key_bad.as_mut_ptr());
        }
        assert_ne!(key_good, key_bad,
                   "single-byte change in DH share should propagate to session key");
    }

    /// Rough timing benchmark for ML-KEM-768 ops. NOT a proper criterion
    /// bench — just an order-of-magnitude check that operations complete
    /// within reasonable bounds on the testing hardware.
    ///
    /// Output is "ignored" by default; run with `cargo test --release --
    /// --ignored --nocapture` to see timings. On ARM/phone hardware,
    /// expect ~3-10x slowdown vs x86 desktop.
    #[test]
    #[ignore]
    fn mlkem_timing_bench() {
        use std::time::Instant;
        const ITERATIONS: u32 = 100;

        let mut dk = vec![0u8; MLKEM_DK_LEN];
        let mut ek = vec![0u8; MLKEM_EK_LEN];
        let mut ct = vec![0u8; MLKEM_CT_LEN];
        let mut ss = vec![0u8; MLKEM_SS_LEN];

        let t0 = Instant::now();
        for _ in 0..ITERATIONS {
            unsafe {
                sc_mlkem_generate_keypair(dk.as_mut_ptr(), ek.as_mut_ptr());
            }
        }
        let keygen_ms = t0.elapsed().as_secs_f64() * 1000.0 / ITERATIONS as f64;

        let t0 = Instant::now();
        for _ in 0..ITERATIONS {
            unsafe {
                sc_mlkem_encapsulate(ek.as_ptr(), ct.as_mut_ptr(), ss.as_mut_ptr());
            }
        }
        let encap_ms = t0.elapsed().as_secs_f64() * 1000.0 / ITERATIONS as f64;

        let t0 = Instant::now();
        for _ in 0..ITERATIONS {
            unsafe {
                sc_mlkem_decapsulate(dk.as_ptr(), ct.as_ptr(), ss.as_mut_ptr());
            }
        }
        let decap_ms = t0.elapsed().as_secs_f64() * 1000.0 / ITERATIONS as f64;

        println!("ML-KEM-768 (avg over {} iters):", ITERATIONS);
        println!("  keygen:       {:.3} ms", keygen_ms);
        println!("  encapsulate:  {:.3} ms", encap_ms);
        println!("  decapsulate:  {:.3} ms", decap_ms);
        println!("  full handshake (1 keygen + 1 encap + 1 decap): {:.3} ms",
                 keygen_ms + encap_ms + decap_ms);
    }

    // ── Classical core (the path used when SNAPCLONE_RUST_CRYPTO=ON) ──────────

    #[test]
    fn aes_gcm_roundtrip() {
        let key = [7u8; KEY_LEN];
        let pt = b"the quick brown fox snaps over the lazy dog";
        let mut ct = vec![0u8; NONCE_LEN + pt.len() + 16];
        let mut ct_len = 0usize;
        let e = unsafe {
            sc_encrypt_aes256gcm(key.as_ptr(), pt.as_ptr(), pt.len(),
                                 ct.as_mut_ptr(), ct.len(), &mut ct_len)
        };
        assert_eq!(e, CryptoError::Ok);
        assert_eq!(ct_len, NONCE_LEN + pt.len() + 16);

        let mut out = vec![0u8; pt.len()];
        let mut out_len = 0usize;
        let d = unsafe {
            sc_decrypt_aes256gcm(key.as_ptr(), ct.as_ptr(), ct_len,
                                 out.as_mut_ptr(), out.len(), &mut out_len)
        };
        assert_eq!(d, CryptoError::Ok);
        assert_eq!(&out[..out_len], pt);
    }

    #[test]
    fn aes_gcm_tamper_is_detected() {
        // GCM is authenticated — flipping a ciphertext byte must fail decryption,
        // not silently return garbage. This is the property that makes it safe
        // to flip RUST_CRYPTO on as the default.
        let key = [3u8; KEY_LEN];
        let pt = b"sensitive snap payload";
        let mut ct = vec![0u8; NONCE_LEN + pt.len() + 16];
        let mut ct_len = 0usize;
        unsafe {
            sc_encrypt_aes256gcm(key.as_ptr(), pt.as_ptr(), pt.len(),
                                 ct.as_mut_ptr(), ct.len(), &mut ct_len);
        }
        // flip a byte inside the ciphertext body (after the nonce)
        ct[NONCE_LEN + 1] ^= 0xFF;
        let mut out = vec![0u8; pt.len()];
        let mut out_len = 0usize;
        let d = unsafe {
            sc_decrypt_aes256gcm(key.as_ptr(), ct.as_ptr(), ct_len,
                                 out.as_mut_ptr(), out.len(), &mut out_len)
        };
        assert_ne!(d, CryptoError::Ok, "tampered ciphertext must NOT decrypt OK");
    }

    #[test]
    fn aes_gcm_wrong_key_fails() {
        let key = [1u8; KEY_LEN];
        let wrong = [2u8; KEY_LEN];
        let pt = b"hello";
        let mut ct = vec![0u8; NONCE_LEN + pt.len() + 16];
        let mut ct_len = 0usize;
        unsafe {
            sc_encrypt_aes256gcm(key.as_ptr(), pt.as_ptr(), pt.len(),
                                 ct.as_mut_ptr(), ct.len(), &mut ct_len);
        }
        let mut out = vec![0u8; pt.len()];
        let mut out_len = 0usize;
        let d = unsafe {
            sc_decrypt_aes256gcm(wrong.as_ptr(), ct.as_ptr(), ct_len,
                                 out.as_mut_ptr(), out.len(), &mut out_len)
        };
        assert_ne!(d, CryptoError::Ok, "wrong key must not decrypt");
    }

    #[test]
    fn aes_gcm_buffer_too_small() {
        let key = [0u8; KEY_LEN];
        let pt = b"data";
        let mut tiny = vec![0u8; 4]; // way too small
        let mut ct_len = 0usize;
        let e = unsafe {
            sc_encrypt_aes256gcm(key.as_ptr(), pt.as_ptr(), pt.len(),
                                 tiny.as_mut_ptr(), tiny.len(), &mut ct_len)
        };
        assert_eq!(e, CryptoError::BufferTooSmall);
    }

    #[test]
    fn x25519_both_sides_agree() {
        // The core of the session-key handshake: A(priv_a, pub_b) and
        // B(priv_b, pub_a) must derive the SAME shared secret.
        let mut priv_a = [0u8; PRIV_KEY_LEN];
        let mut pub_a  = [0u8; PUB_KEY_LEN];
        let mut priv_b = [0u8; PRIV_KEY_LEN];
        let mut pub_b  = [0u8; PUB_KEY_LEN];
        unsafe {
            sc_generate_x25519_keypair(priv_a.as_mut_ptr(), pub_a.as_mut_ptr());
            sc_generate_x25519_keypair(priv_b.as_mut_ptr(), pub_b.as_mut_ptr());
        }
        let mut shared_a = [0u8; KEY_LEN];
        let mut shared_b = [0u8; KEY_LEN];
        let ea = unsafe { sc_x25519_raw_shared(priv_a.as_ptr(), pub_b.as_ptr(), shared_a.as_mut_ptr()) };
        let eb = unsafe { sc_x25519_raw_shared(priv_b.as_ptr(), pub_a.as_ptr(), shared_b.as_mut_ptr()) };
        assert_eq!(ea, CryptoError::Ok);
        assert_eq!(eb, CryptoError::Ok);
        assert_eq!(shared_a, shared_b, "ECDH must produce a matching shared secret");
        assert_ne!(shared_a, [0u8; KEY_LEN], "shared secret must not be all-zero");
    }

    #[test]
    fn ed25519_sign_verify_roundtrip_and_tamper() {
        let mut priv_k = [0u8; PRIV_KEY_LEN];
        let mut pub_k  = [0u8; PUB_KEY_LEN];
        unsafe { sc_generate_ed25519_keypair(priv_k.as_mut_ptr(), pub_k.as_mut_ptr()); }
        let msg = b"landmark wire-canonical bytes";
        let mut sig = [0u8; SIG_LEN];
        let es = unsafe { sc_sign_ed25519(priv_k.as_ptr(), msg.as_ptr(), msg.len(), sig.as_mut_ptr()) };
        assert_eq!(es, CryptoError::Ok);
        let ev = unsafe { sc_verify_ed25519(pub_k.as_ptr(), msg.as_ptr(), msg.len(), sig.as_ptr()) };
        assert_eq!(ev, CryptoError::Ok, "valid signature must verify");
        // tamper the message → verification must fail
        let bad = b"landmark wire-canonical bytez";
        let ev2 = unsafe { sc_verify_ed25519(pub_k.as_ptr(), bad.as_ptr(), bad.len(), sig.as_ptr()) };
        assert_ne!(ev2, CryptoError::Ok, "signature must not verify a tampered message");
    }
}
