//! keyshare.rs — multiway key distribution for group media.
//!
//! A media blob is encrypted once with a random 32-byte content key (the chunks
//! carry that ciphertext). The content key is then WRAPPED for every group
//! member: one fresh ephemeral X25519 key, a per-recipient shared secret, and a
//! ChaCha20-Poly1305 wrap. Each member recovers the content key with their own
//! X25519 secret. Adding a recipient costs one 56-byte wrap; the bulk ciphertext
//! is shared by all (and deduped by the chunk layer).

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

/// Content-key length.
pub const CONTENT_KEY_LEN: usize = 32;
/// Each wrapped key is the content key (32) + the AEAD tag (16).
pub const WRAP_LEN: usize = CONTENT_KEY_LEN + 16;

// A fixed nonce is safe here: every wrap uses a DISTINCT key (a fresh ephemeral
// per envelope, and a different X25519 shared secret per recipient), so the
// (key, nonce) pair never repeats.
const WRAP_NONCE: [u8; 12] = [0u8; 12];
const HKDF_SALT: &[u8] = b"snapclone-media-keyshare-v1";
const HKDF_INFO: &[u8] = b"wrap-key";

/// One recipient's wrapped copy of the content key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedKey {
    /// First 8 bytes of the recipient's X25519 public key — a non-secret hint so
    /// a member can find their own wrap quickly.
    pub recipient_hint: [u8; 8],
    /// ChaCha20-Poly1305(content_key) under HKDF(X25519(eph, recipient_pub)).
    pub ciphertext: Vec<u8>,
}

/// A content key wrapped for every group member; travels with the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyShareEnvelope {
    pub ephemeral_pub: [u8; 32],
    pub wraps: Vec<WrappedKey>,
}

impl KeyShareEnvelope {
    /// [ephemeral_pub 32][count u32 BE][ (hint 8)(ct_len u32 BE)(ct) * count ]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(36 + self.wraps.len() * (8 + 4 + WRAP_LEN));
        out.extend_from_slice(&self.ephemeral_pub);
        out.extend_from_slice(&(self.wraps.len() as u32).to_be_bytes());
        for w in &self.wraps {
            out.extend_from_slice(&w.recipient_hint);
            out.extend_from_slice(&(w.ciphertext.len() as u32).to_be_bytes());
            out.extend_from_slice(&w.ciphertext);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 36 {
            return None;
        }
        let mut ephemeral_pub = [0u8; 32];
        ephemeral_pub.copy_from_slice(&bytes[0..32]);
        let count = u32::from_be_bytes(bytes[32..36].try_into().ok()?) as usize;
        let mut off = 36;
        let mut wraps = Vec::with_capacity(count);
        for _ in 0..count {
            if off + 12 > bytes.len() {
                return None;
            }
            let mut hint = [0u8; 8];
            hint.copy_from_slice(&bytes[off..off + 8]);
            let ct_len = u32::from_be_bytes(bytes[off + 8..off + 12].try_into().ok()?) as usize;
            off += 12;
            if off + ct_len > bytes.len() {
                return None;
            }
            wraps.push(WrappedKey {
                recipient_hint: hint,
                ciphertext: bytes[off..off + ct_len].to_vec(),
            });
            off += ct_len;
        }
        Some(Self { ephemeral_pub, wraps })
    }
}

fn wrap_key_for(shared: &[u8; 32]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT), shared);
    let mut k = [0u8; 32];
    hk.expand(HKDF_INFO, &mut k).expect("hkdf expand of 32 bytes never fails");
    k
}

/// A fresh random content key.
pub fn new_content_key() -> [u8; CONTENT_KEY_LEN] {
    let mut k = [0u8; CONTENT_KEY_LEN];
    OsRng.fill_bytes(&mut k);
    k
}

/// Wrap `content_key` for each recipient public key, using one fresh ephemeral.
pub fn seal_for_group(
    content_key: &[u8; CONTENT_KEY_LEN],
    recipients: &[[u8; 32]],
) -> KeyShareEnvelope {
    let eph_secret = StaticSecret::random_from_rng(&mut OsRng);
    let eph_pub = PublicKey::from(&eph_secret);
    let mut wraps = Vec::with_capacity(recipients.len());
    for r in recipients {
        let shared = eph_secret.diffie_hellman(&PublicKey::from(*r));
        let wk = wrap_key_for(shared.as_bytes());
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&wk));
        let ct = cipher
            .encrypt(Nonce::from_slice(&WRAP_NONCE), Payload { msg: content_key, aad: &[] })
            .expect("wrap encrypt never fails");
        let mut hint = [0u8; 8];
        hint.copy_from_slice(&r[..8]);
        wraps.push(WrappedKey { recipient_hint: hint, ciphertext: ct });
    }
    KeyShareEnvelope { ephemeral_pub: *eph_pub.as_bytes(), wraps }
}

/// Recover the content key with your X25519 secret + public key. None if no wrap
/// is for you, or decryption fails.
pub fn open(
    envelope: &KeyShareEnvelope,
    my_secret: &[u8; 32],
    my_pub: &[u8; 32],
) -> Option<[u8; CONTENT_KEY_LEN]> {
    let secret = StaticSecret::from(*my_secret);
    let shared = secret.diffie_hellman(&PublicKey::from(envelope.ephemeral_pub));
    let wk = wrap_key_for(shared.as_bytes());
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&wk));
    let mut hint = [0u8; 8];
    hint.copy_from_slice(&my_pub[..8]);
    // try our hinted wrap(s) first, then fall back to any (hint collisions exist)
    let hinted = envelope.wraps.iter().filter(|w| w.recipient_hint == hint);
    let rest = envelope.wraps.iter().filter(|w| w.recipient_hint != hint);
    for w in hinted.chain(rest) {
        if let Ok(pt) =
            cipher.decrypt(Nonce::from_slice(&WRAP_NONCE), Payload { msg: &w.ciphertext, aad: &[] })
        {
            if pt.len() == CONTENT_KEY_LEN {
                let mut k = [0u8; CONTENT_KEY_LEN];
                k.copy_from_slice(&pt);
                return Some(k);
            }
        }
    }
    None
}

/// Media-seal nonce length (ChaCha20-Poly1305).
pub const MEDIA_NONCE_LEN: usize = 12;
/// AEAD tag length appended by ChaCha20-Poly1305.
pub const MEDIA_TAG_LEN: usize = 16;

/// Encrypt a media blob with the content key. Output = a fresh random 12-byte
/// nonce || ChaCha20-Poly1305 ciphertext (ciphertext includes the 16-byte tag).
/// A new nonce is drawn per call, so the same content key may safely seal more
/// than once. This is the "encrypt the media once with a content key" half of
/// the keyshare design — the resulting ciphertext is what the chunk layer splits
/// + dedups.
pub fn seal_media(content_key: &[u8; CONTENT_KEY_LEN], plaintext: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; MEDIA_NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(content_key));
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: &[] })
        .expect("media seal never fails");
    let mut out = Vec::with_capacity(MEDIA_NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

/// Decrypt a blob produced by [`seal_media`]. `None` on a short input, a wrong
/// content key, or a tampered nonce/ciphertext/tag.
pub fn open_media(content_key: &[u8; CONTENT_KEY_LEN], sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < MEDIA_NONCE_LEN + MEDIA_TAG_LEN {
        return None;
    }
    let (nonce, ct) = sealed.split_at(MEDIA_NONCE_LEN);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(content_key));
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad: &[] })
        .ok()
}

// ─── FFI ─────────────────────────────────────────────────────────────────────

use std::os::raw::c_int;

/// Write a fresh random 32-byte content key.
///
/// # Safety
/// `out_key32` writable for 32 bytes.
#[no_mangle]
pub unsafe extern "C" fn sc_media_content_key_new(out_key32: *mut u8) {
    if out_key32.is_null() {
        return;
    }
    let k = new_content_key();
    std::ptr::copy_nonoverlapping(k.as_ptr(), out_key32, CONTENT_KEY_LEN);
}

/// Wrap `content_key32` for each recipient. `recipients` is `recipient_count`
/// X25519 public keys concatenated (32 bytes each). Writes the encoded envelope
/// to `out`. Returns 0; -1 (null), -2 (out too small — `out_len` = required).
///
/// # Safety
/// `content_key32` valid for 32; `recipients` valid for `recipient_count*32`;
/// `out` writable for `out_cap`; pointers valid.
#[no_mangle]
pub unsafe extern "C" fn sc_media_keyshare_seal(
    content_key32: *const u8,
    recipients: *const u8,
    recipient_count: usize,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> c_int {
    if content_key32.is_null() || out_len.is_null() {
        return -1;
    }
    if recipient_count > 0 && recipients.is_null() {
        return -1;
    }
    let mut ck = [0u8; CONTENT_KEY_LEN];
    std::ptr::copy_nonoverlapping(content_key32, ck.as_mut_ptr(), CONTENT_KEY_LEN);

    let mut recips = Vec::with_capacity(recipient_count);
    for i in 0..recipient_count {
        let mut p = [0u8; 32];
        std::ptr::copy_nonoverlapping(recipients.add(i * 32), p.as_mut_ptr(), 32);
        recips.push(p);
    }
    let enc = seal_for_group(&ck, &recips).encode();
    *out_len = enc.len();
    if out.is_null() || enc.len() > out_cap {
        return -2;
    }
    std::ptr::copy_nonoverlapping(enc.as_ptr(), out, enc.len());
    0
}

/// Recover the content key from an encoded envelope. Writes 32 bytes to
/// `out_key32`. Returns 0; -1 (null/bad envelope), -3 (no wrap for you / decrypt
/// failed).
///
/// # Safety
/// `envelope` valid for `env_len`; `my_secret32`/`my_pub32` valid for 32;
/// `out_key32` writable for 32; pointers valid.
#[no_mangle]
pub unsafe extern "C" fn sc_media_keyshare_open(
    envelope: *const u8,
    env_len: usize,
    my_secret32: *const u8,
    my_pub32: *const u8,
    out_key32: *mut u8,
) -> c_int {
    if envelope.is_null() || my_secret32.is_null() || my_pub32.is_null() || out_key32.is_null() {
        return -1;
    }
    let env = match KeyShareEnvelope::decode(std::slice::from_raw_parts(envelope, env_len)) {
        Some(e) => e,
        None => return -1,
    };
    let mut sec = [0u8; 32];
    let mut pubk = [0u8; 32];
    std::ptr::copy_nonoverlapping(my_secret32, sec.as_mut_ptr(), 32);
    std::ptr::copy_nonoverlapping(my_pub32, pubk.as_mut_ptr(), 32);
    match open(&env, &sec, &pubk) {
        Some(k) => {
            std::ptr::copy_nonoverlapping(k.as_ptr(), out_key32, CONTENT_KEY_LEN);
            0
        }
        None => -3,
    }
}

/// Seal a media blob with the content key. Writes `nonce || ciphertext` to `out`.
/// Returns 0; -1 (null), -2 (out too small — `out_len` = required size).
/// Output size is always `len + 28` (12 nonce + 16 tag).
///
/// # Safety
/// `content_key32` valid for 32; `data` valid for `len`; `out` writable for
/// `out_cap`; pointers valid.
#[no_mangle]
pub unsafe extern "C" fn sc_media_seal(
    content_key32: *const u8,
    data: *const u8,
    len: usize,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> c_int {
    if content_key32.is_null() || out_len.is_null() {
        return -1;
    }
    if len > 0 && data.is_null() {
        return -1;
    }
    let mut ck = [0u8; CONTENT_KEY_LEN];
    std::ptr::copy_nonoverlapping(content_key32, ck.as_mut_ptr(), CONTENT_KEY_LEN);
    let pt: &[u8] = if len == 0 { &[] } else { std::slice::from_raw_parts(data, len) };
    let sealed = seal_media(&ck, pt);
    *out_len = sealed.len();
    if out.is_null() || sealed.len() > out_cap {
        return -2;
    }
    std::ptr::copy_nonoverlapping(sealed.as_ptr(), out, sealed.len());
    0
}

/// Open a media blob sealed by [`sc_media_seal`]. Writes plaintext to `out`.
/// Returns 0; -1 (null), -2 (out too small — `out_len` = required), -3 (decrypt
/// failed / wrong key / tampered). Plaintext is `len - 28` bytes, so sizing
/// `out_cap = len` always suffices.
///
/// # Safety
/// `content_key32` valid for 32; `sealed` valid for `len`; `out` writable for
/// `out_cap`; pointers valid.
#[no_mangle]
pub unsafe extern "C" fn sc_media_open(
    content_key32: *const u8,
    sealed: *const u8,
    len: usize,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> c_int {
    if content_key32.is_null() || sealed.is_null() || out_len.is_null() {
        return -1;
    }
    let mut ck = [0u8; CONTENT_KEY_LEN];
    std::ptr::copy_nonoverlapping(content_key32, ck.as_mut_ptr(), CONTENT_KEY_LEN);
    let s = std::slice::from_raw_parts(sealed, len);
    match open_media(&ck, s) {
        Some(pt) => {
            *out_len = pt.len();
            if out.is_null() || pt.len() > out_cap {
                return -2;
            }
            std::ptr::copy_nonoverlapping(pt.as_ptr(), out, pt.len());
            0
        }
        None => -3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_seal_round_trip() {
        let ck = new_content_key();
        let pt = b"the quick brown fox jumps over the lazy dog, more than once here";
        let sealed = seal_media(&ck, pt);
        assert_eq!(sealed.len(), pt.len() + MEDIA_NONCE_LEN + MEDIA_TAG_LEN);
        assert_eq!(open_media(&ck, &sealed).as_deref(), Some(&pt[..]));
    }

    #[test]
    fn media_seal_empty_is_valid() {
        let ck = new_content_key();
        let sealed = seal_media(&ck, &[]);
        assert_eq!(open_media(&ck, &sealed).as_deref(), Some(&[][..]));
    }

    #[test]
    fn media_open_wrong_key_fails() {
        let ck = new_content_key();
        let other = new_content_key();
        let sealed = seal_media(&ck, b"secret payload bytes");
        assert!(open_media(&other, &sealed).is_none());
    }

    #[test]
    fn media_open_tamper_fails() {
        let ck = new_content_key();
        let mut sealed = seal_media(&ck, b"secret payload bytes");
        let n = sealed.len();
        sealed[n - 1] ^= 0xFF; // flip a tag/ciphertext byte
        assert!(open_media(&ck, &sealed).is_none());
        // also tamper the nonce region
        let mut s2 = seal_media(&ck, b"secret payload bytes");
        s2[0] ^= 0xFF;
        assert!(open_media(&ck, &s2).is_none());
    }

    #[test]
    fn media_seal_uses_fresh_nonce() {
        let ck = new_content_key();
        let a = seal_media(&ck, b"same plaintext");
        let b = seal_media(&ck, b"same plaintext");
        assert_ne!(a, b, "a fresh nonce per seal must yield different ciphertexts");
        assert_eq!(open_media(&ck, &a), open_media(&ck, &b));
    }

    #[test]
    fn ffi_media_seal_open_round_trip() {
        unsafe {
            let mut ck = [0u8; CONTENT_KEY_LEN];
            sc_media_content_key_new(ck.as_mut_ptr());
            let pt = b"ffi media bytes traverse the boundary";

            // size query (out null) → -2 with required length
            let mut need = 0usize;
            let rc =
                sc_media_seal(ck.as_ptr(), pt.as_ptr(), pt.len(), std::ptr::null_mut(), 0, &mut need);
            assert_eq!(rc, -2);
            assert_eq!(need, pt.len() + MEDIA_NONCE_LEN + MEDIA_TAG_LEN);

            let mut sealed = vec![0u8; need];
            let mut slen = 0usize;
            assert_eq!(
                sc_media_seal(ck.as_ptr(), pt.as_ptr(), pt.len(),
                              sealed.as_mut_ptr(), sealed.len(), &mut slen),
                0
            );
            sealed.truncate(slen);

            let mut out = vec![0u8; sealed.len()];
            let mut olen = 0usize;
            assert_eq!(
                sc_media_open(ck.as_ptr(), sealed.as_ptr(), sealed.len(),
                              out.as_mut_ptr(), out.len(), &mut olen),
                0
            );
            out.truncate(olen);
            assert_eq!(&out[..], &pt[..]);

            // wrong key → -3
            let mut bad = [0u8; CONTENT_KEY_LEN];
            sc_media_content_key_new(bad.as_mut_ptr());
            let mut o2 = vec![0u8; sealed.len()];
            let mut ol2 = 0usize;
            assert_eq!(
                sc_media_open(bad.as_ptr(), sealed.as_ptr(), sealed.len(),
                              o2.as_mut_ptr(), o2.len(), &mut ol2),
                -3
            );
        }
    }

    fn keypair(seed: u8) -> ([u8; 32], [u8; 32]) {
        let mut s = [seed; 32];
        s[0] ^= 1; // avoid all-equal trivial keys
        let secret = StaticSecret::from(s);
        let public = PublicKey::from(&secret);
        (s, *public.as_bytes())
    }

    #[test]
    fn every_member_recovers_the_key() {
        let (s1, p1) = keypair(1);
        let (s2, p2) = keypair(2);
        let (s3, p3) = keypair(3);
        let ck = new_content_key();
        let env = seal_for_group(&ck, &[p1, p2, p3]);
        assert_eq!(env.wraps.len(), 3);
        assert_eq!(open(&env, &s1, &p1), Some(ck));
        assert_eq!(open(&env, &s2, &p2), Some(ck));
        assert_eq!(open(&env, &s3, &p3), Some(ck));
    }

    #[test]
    fn non_member_cannot_open() {
        let (_, p1) = keypair(1);
        let (s_out, p_out) = keypair(9); // not in the recipient set
        let ck = new_content_key();
        let env = seal_for_group(&ck, &[p1]);
        assert_eq!(open(&env, &s_out, &p_out), None);
    }

    #[test]
    fn envelope_encode_decode_roundtrips() {
        let (_, p1) = keypair(1);
        let (_, p2) = keypair(2);
        let env = seal_for_group(&new_content_key(), &[p1, p2]);
        let enc = env.encode();
        let dec = KeyShareEnvelope::decode(&enc).expect("decode");
        assert_eq!(dec, env);
        assert!(KeyShareEnvelope::decode(&enc[..20]).is_none());
    }

    #[test]
    fn tampered_wrap_rejected() {
        let (s1, p1) = keypair(1);
        let ck = new_content_key();
        let mut env = seal_for_group(&ck, &[p1]);
        env.wraps[0].ciphertext[0] ^= 0xff;
        assert_eq!(open(&env, &s1, &p1), None);
    }

    #[test]
    fn decode_then_open_works_end_to_end() {
        let (s1, p1) = keypair(7);
        let (_, p2) = keypair(8);
        let ck = new_content_key();
        let wire = seal_for_group(&ck, &[p1, p2]).encode();
        let env = KeyShareEnvelope::decode(&wire).unwrap();
        assert_eq!(open(&env, &s1, &p1), Some(ck));
    }

    #[test]
    fn ffi_seal_open_roundtrip() {
        unsafe {
            let (s1, p1) = keypair(4);
            let (_, p2) = keypair(5);
            let mut ck = [0u8; 32];
            sc_media_content_key_new(ck.as_mut_ptr());

            let mut recips = Vec::new();
            recips.extend_from_slice(&p1);
            recips.extend_from_slice(&p2);
            let mut env = vec![0u8; 512];
            let mut elen = 0usize;
            assert_eq!(
                sc_media_keyshare_seal(ck.as_ptr(), recips.as_ptr(), 2,
                    env.as_mut_ptr(), env.len(), &mut elen),
                0
            );

            let mut got = [0u8; 32];
            assert_eq!(
                sc_media_keyshare_open(env.as_ptr(), elen, s1.as_ptr(), p1.as_ptr(), got.as_mut_ptr()),
                0
            );
            assert_eq!(got, ck);

            let (s_out, p_out) = keypair(99);
            assert_eq!(
                sc_media_keyshare_open(env.as_ptr(), elen, s_out.as_ptr(), p_out.as_ptr(), got.as_mut_ptr()),
                -3
            );
        }
    }
}
