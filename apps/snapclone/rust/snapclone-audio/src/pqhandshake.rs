//! pqhandshake.rs — post-quantum hybrid key exchange for the private voice tier.
//!
//! When both peers opt into Private mode (see security.rs), they run this before
//! voice flows, to agree a 32-byte session key that feeds the per-frame AEAD and
//! the obfuscation seed. It is HYBRID: a classical X25519 ECDH plus a post-quantum
//! ML-KEM-768 KEM, with both shared secrets fed into one HKDF. The session key is
//! safe as long as EITHER primitive holds — resisting both today's attackers and
//! a future quantum one ("harvest now, decrypt later"). Same belt-and-braces
//! shape TLS uses for its PQ rollout.
//!
//! Two messages:
//!   offer  (initiator -> responder): x25519_pub_i || mlkem_encapsulation_key
//!   answer (responder -> initiator): x25519_pub_r || mlkem_ciphertext
//! The initiator is stateful (new -> offer -> finish); the responder is one-shot
//! (answer produces the reply AND the key in one call). Key material is per-
//! handshake and dropped after, for forward secrecy.

use kem::{Decapsulate, Encapsulate};
use ml_kem::{Ciphertext, Encoded, EncodedSizeUser, KemCore, MlKem768};
use rand_core::OsRng;
use x25519_dalek::{PublicKey, StaticSecret};

use hkdf::Hkdf;
use sha2::Sha256;

/// Length of the derived voice session key.
pub const PQ_SESSION_KEY_LEN: usize = 32;
const X25519_PUB_LEN: usize = 32;
const HKDF_SALT: &[u8] = b"snapclone-voice-pq-v1";
const HKDF_INFO: &[u8] = b"voice-session-key";

type Ek = <MlKem768 as KemCore>::EncapsulationKey;
type Dk = <MlKem768 as KemCore>::DecapsulationKey;

/// Combine the two shared secrets into the session key via HKDF-SHA256.
fn derive_session_key(x25519_ss: &[u8], mlkem_ss: &[u8]) -> [u8; PQ_SESSION_KEY_LEN] {
    let mut ikm = Vec::with_capacity(x25519_ss.len() + mlkem_ss.len());
    ikm.extend_from_slice(x25519_ss);
    ikm.extend_from_slice(mlkem_ss);
    let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT), &ikm);
    let mut okm = [0u8; PQ_SESSION_KEY_LEN];
    hk.expand(HKDF_INFO, &mut okm)
        .expect("HKDF expand of 32 bytes never fails");
    okm
}

/// Initiator state, held between `offer` and `finish`.
pub struct PqInitiator {
    x_secret: StaticSecret, // per-handshake ephemeral (dropped with the struct)
    mlkem_dk: Dk,
    ek_bytes: Vec<u8>, // our ML-KEM encapsulation key, emitted in the offer
}

impl PqInitiator {
    /// Generate fresh X25519 + ML-KEM key material.
    pub fn new() -> Self {
        let mut rng = OsRng;
        let x_secret = StaticSecret::random_from_rng(&mut rng);
        let (mlkem_dk, ek) = MlKem768::generate(&mut rng);
        Self {
            x_secret,
            mlkem_dk,
            ek_bytes: ek.as_bytes().to_vec(),
        }
    }

    /// The offer bytes to send to the responder.
    pub fn offer(&self) -> Vec<u8> {
        let x_pub = PublicKey::from(&self.x_secret);
        let mut out = Vec::with_capacity(X25519_PUB_LEN + self.ek_bytes.len());
        out.extend_from_slice(x_pub.as_bytes());
        out.extend_from_slice(&self.ek_bytes);
        out
    }

    /// Process the responder's answer and derive the session key.
    pub fn finish(&self, answer: &[u8]) -> Option<[u8; PQ_SESSION_KEY_LEN]> {
        if answer.len() <= X25519_PUB_LEN {
            return None;
        }
        let (x_pub_r, mlkem_ct_bytes) = answer.split_at(X25519_PUB_LEN);

        let their_pub: [u8; 32] = x_pub_r.try_into().ok()?;
        let x_ss = self.x_secret.diffie_hellman(&PublicKey::from(their_pub));

        let ct = Ciphertext::<MlKem768>::try_from(mlkem_ct_bytes).ok()?;
        let mlkem_ss = self.mlkem_dk.decapsulate(&ct).ok()?;

        Some(derive_session_key(x_ss.as_bytes(), mlkem_ss.as_slice()))
    }
}

impl Default for PqInitiator {
    fn default() -> Self {
        Self::new()
    }
}

/// Responder one-shot: consume an offer, produce the answer bytes and the shared
/// session key. Returns None if the offer is malformed.
pub fn responder_answer(offer: &[u8]) -> Option<(Vec<u8>, [u8; PQ_SESSION_KEY_LEN])> {
    if offer.len() <= X25519_PUB_LEN {
        return None;
    }
    let (x_pub_i, mlkem_ek_bytes) = offer.split_at(X25519_PUB_LEN);

    let their_pub: [u8; 32] = x_pub_i.try_into().ok()?;

    let mut rng = OsRng;
    let x_secret = StaticSecret::random_from_rng(&mut rng);
    let x_pub_r = PublicKey::from(&x_secret);
    let x_ss = x_secret.diffie_hellman(&PublicKey::from(their_pub));

    let ek_encoded = Encoded::<Ek>::try_from(mlkem_ek_bytes).ok()?;
    let ek = Ek::from_bytes(&ek_encoded);
    let (ct, mlkem_ss) = ek.encapsulate(&mut rng).ok()?;

    let key = derive_session_key(x_ss.as_bytes(), mlkem_ss.as_slice());

    let mut answer = Vec::with_capacity(X25519_PUB_LEN + ct.len());
    answer.extend_from_slice(x_pub_r.as_bytes());
    answer.extend_from_slice(ct.as_slice());
    Some((answer, key))
}

// ─── FFI ─────────────────────────────────────────────────────────────────────
use std::os::raw::c_int;

/// Create an initiator (generates fresh key material). Free with
/// `sc_pq_initiator_free`.
#[no_mangle]
pub extern "C" fn sc_pq_initiator_new() -> *mut PqInitiator {
    Box::into_raw(Box::new(PqInitiator::new()))
}

/// # Safety
/// `handle` from `sc_pq_initiator_new`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_pq_initiator_free(handle: *mut PqInitiator) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Write the offer bytes (send to the responder). Returns 0; -1 (null), -2 (out
/// too small — `out_len` = required).
///
/// # Safety
/// `out` writable for `out_cap`; `out_len` a valid `*mut usize`; `handle` valid.
#[no_mangle]
pub unsafe extern "C" fn sc_pq_initiator_offer(
    handle: *const PqInitiator,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> c_int {
    if handle.is_null() || out_len.is_null() {
        return -1;
    }
    let offer = (*handle).offer();
    *out_len = offer.len();
    if out.is_null() || offer.len() > out_cap {
        return -2;
    }
    std::ptr::copy_nonoverlapping(offer.as_ptr(), out, offer.len());
    0
}

/// Finish from the responder's answer; writes the 32-byte session key. Returns
/// 0; -1 (null), -3 (malformed answer / decapsulation failed).
///
/// # Safety
/// `answer` valid for `answer_len`; `out_key` writable for 32 bytes; `handle` valid.
#[no_mangle]
pub unsafe extern "C" fn sc_pq_initiator_finish(
    handle: *const PqInitiator,
    answer: *const u8,
    answer_len: usize,
    out_key: *mut u8,
) -> c_int {
    if handle.is_null() || answer.is_null() || out_key.is_null() {
        return -1;
    }
    let answer_slice = std::slice::from_raw_parts(answer, answer_len);
    match (*handle).finish(answer_slice) {
        Some(key) => {
            std::ptr::copy_nonoverlapping(key.as_ptr(), out_key, PQ_SESSION_KEY_LEN);
            0
        }
        None => -3,
    }
}

/// Responder one-shot: consume `offer`, write the answer bytes and the 32-byte
/// session key. Returns 0; -1 (null), -2 (answer buffer too small — `out_answer_len`
/// = required), -3 (malformed offer).
///
/// # Safety
/// `offer` valid for `offer_len`; `out_answer` writable for `out_answer_cap`;
/// `out_answer_len` valid; `out_key` writable for 32 bytes.
#[no_mangle]
pub unsafe extern "C" fn sc_pq_responder_answer(
    offer: *const u8,
    offer_len: usize,
    out_answer: *mut u8,
    out_answer_cap: usize,
    out_answer_len: *mut usize,
    out_key: *mut u8,
) -> c_int {
    if offer.is_null() || out_answer_len.is_null() || out_key.is_null() {
        return -1;
    }
    let offer_slice = std::slice::from_raw_parts(offer, offer_len);
    let (answer, key) = match responder_answer(offer_slice) {
        Some(v) => v,
        None => return -3,
    };
    *out_answer_len = answer.len();
    if out_answer.is_null() || answer.len() > out_answer_cap {
        return -2;
    }
    std::ptr::copy_nonoverlapping(answer.as_ptr(), out_answer, answer.len());
    std::ptr::copy_nonoverlapping(key.as_ptr(), out_key, PQ_SESSION_KEY_LEN);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_handshake_agrees_on_key() {
        let initiator = PqInitiator::new();
        let offer = initiator.offer();
        let (answer, responder_key) = responder_answer(&offer).expect("responder");
        let initiator_key = initiator.finish(&answer).expect("initiator finish");
        assert_eq!(initiator_key, responder_key, "both sides derive the same key");
        assert_eq!(initiator_key.len(), 32);
        // sanity: key isn't all zeros
        assert_ne!(initiator_key, [0u8; 32]);
    }

    #[test]
    fn independent_handshakes_differ() {
        let a = PqInitiator::new();
        let (_, ka) = responder_answer(&a.offer()).unwrap();
        let b = PqInitiator::new();
        let (_, kb) = responder_answer(&b.offer()).unwrap();
        assert_ne!(ka, kb, "fresh handshakes yield distinct keys");
    }

    #[test]
    fn tampered_answer_breaks_agreement() {
        let initiator = PqInitiator::new();
        let offer = initiator.offer();
        let (mut answer, responder_key) = responder_answer(&offer).unwrap();
        // flip a byte in the X25519 portion → different x25519 shared secret
        answer[0] ^= 0xff;
        match initiator.finish(&answer) {
            // decapsulation may still succeed but the derived key must not match
            Some(k) => assert_ne!(k, responder_key),
            None => {} // or it fails outright — also acceptable
        }
    }

    #[test]
    fn malformed_offer_rejected() {
        assert!(responder_answer(&[]).is_none());
        assert!(responder_answer(&[0u8; 10]).is_none()); // too short
        // right x25519 length but junk ML-KEM key → rejected at decode
        assert!(responder_answer(&[0u8; 32 + 5]).is_none());
    }

    #[test]
    fn ffi_full_handshake() {
        unsafe {
            let init = sc_pq_initiator_new();
            assert!(!init.is_null());

            // offer (size query then fill)
            let mut need = 0usize;
            assert_eq!(sc_pq_initiator_offer(init, std::ptr::null_mut(), 0, &mut need), -2);
            assert!(need > 32);
            let mut offer = vec![0u8; need];
            let mut olen = 0usize;
            assert_eq!(sc_pq_initiator_offer(init, offer.as_mut_ptr(), offer.len(), &mut olen), 0);
            assert_eq!(olen, need);

            // responder answers
            let mut answer = vec![0u8; 2048];
            let mut alen = 0usize;
            let mut rkey = [0u8; 32];
            assert_eq!(
                sc_pq_responder_answer(offer.as_ptr(), olen, answer.as_mut_ptr(),
                    answer.len(), &mut alen, rkey.as_mut_ptr()),
                0
            );

            // initiator finishes → same key
            let mut ikey = [0u8; 32];
            assert_eq!(
                sc_pq_initiator_finish(init, answer.as_ptr(), alen, ikey.as_mut_ptr()),
                0
            );
            assert_eq!(ikey, rkey);

            sc_pq_initiator_free(init);
        }
    }
}
