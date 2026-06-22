//! voicecrypt.rs — lightweight per-frame AEAD for the public (fast) voice tier.
//!
//! The default voice tier optimises for low latency and small packets: voice on
//! a public network is assumed eavesdroppable, so we want confidentiality +
//! integrity with minimal overhead, not the full signed/ratcheted message seal.
//! ChaCha20-Poly1305 fits — fast in software (no AES hardware needed on the FP5),
//! 16-byte tag, no per-frame signature. The private tier layers a post-quantum
//! key exchange + playback obfuscation ON TOP of this; this module is the floor.
//!
//! NONCE DISCIPLINE IS THE CALLER'S JOB: a 96-bit nonce must never repeat under
//! one key. The voice sequence number restarts each talk-spurt, so DO NOT use it
//! directly as the nonce — the channel maintains a monotonic per-session counter
//! (or a random per-talkspurt prefix + counter) and passes it in. This module
//! just seals/opens with the key + nonce it is given.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use std::os::raw::c_int;

/// Bytes the tag adds to a sealed frame (ciphertext = plaintext + this).
pub const VOICE_AEAD_TAG_LEN: usize = 16;
/// Required key length.
pub const VOICE_AEAD_KEY_LEN: usize = 32;
/// Required nonce length.
pub const VOICE_AEAD_NONCE_LEN: usize = 12;

/// Seal `plaintext` with `key` (32 B) + `nonce` (12 B), binding `aad`. Returns
/// the ciphertext+tag bytes, or None on an internal error.
pub fn seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Option<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: plaintext, aad })
        .ok()
}

/// Open `ciphertext` (ct+tag) with `key` + `nonce`, checking `aad`. Returns the
/// plaintext, or None if authentication fails (tampered / wrong key / wrong aad).
pub fn open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> Option<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad })
        .ok()
}

// ─── FFI ─────────────────────────────────────────────────────────────────────

unsafe fn arr32(p: *const u8) -> Option<[u8; 32]> {
    if p.is_null() {
        return None;
    }
    let mut a = [0u8; 32];
    std::ptr::copy_nonoverlapping(p, a.as_mut_ptr(), 32);
    Some(a)
}
unsafe fn arr12(p: *const u8) -> Option<[u8; 12]> {
    if p.is_null() {
        return None;
    }
    let mut a = [0u8; 12];
    std::ptr::copy_nonoverlapping(p, a.as_mut_ptr(), 12);
    Some(a)
}

/// Seal a voice frame. `key32`/`nonce12` point to 32/12 bytes. `out` receives
/// ciphertext+tag (plaintext_len + 16). Returns 0; or -1 (null), -2 (out too
/// small — `out_len` = required), -3 (internal).
///
/// # Safety
/// Pointers valid for their documented lengths; `out_len` a valid `*mut usize`.
#[no_mangle]
pub unsafe extern "C" fn sc_voice_seal(
    key32: *const u8,
    nonce12: *const u8,
    aad: *const u8,
    aad_len: usize,
    plaintext: *const u8,
    pt_len: usize,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> c_int {
    if out_len.is_null() {
        return -1;
    }
    let (key, nonce) = match (arr32(key32), arr12(nonce12)) {
        (Some(k), Some(n)) => (k, n),
        _ => return -1,
    };
    let aad_slice = if aad.is_null() || aad_len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(aad, aad_len)
    };
    let pt = if plaintext.is_null() || pt_len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(plaintext, pt_len)
    };
    let sealed = match seal(&key, &nonce, aad_slice, pt) {
        Some(v) => v,
        None => return -3,
    };
    *out_len = sealed.len();
    if out.is_null() || sealed.len() > out_cap {
        return -2;
    }
    std::ptr::copy_nonoverlapping(sealed.as_ptr(), out, sealed.len());
    0
}

/// Open a voice frame. Returns 0; or -1 (null), -2 (out too small — `out_len` =
/// required), -4 (authentication failed). Size `out` to `ct_len` and -2 cannot
/// happen.
///
/// # Safety
/// Pointers valid for their documented lengths; `out_len` a valid `*mut usize`.
#[no_mangle]
pub unsafe extern "C" fn sc_voice_open(
    key32: *const u8,
    nonce12: *const u8,
    aad: *const u8,
    aad_len: usize,
    ciphertext: *const u8,
    ct_len: usize,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> c_int {
    if out_len.is_null() || ciphertext.is_null() {
        return -1;
    }
    let (key, nonce) = match (arr32(key32), arr12(nonce12)) {
        (Some(k), Some(n)) => (k, n),
        _ => return -1,
    };
    let aad_slice = if aad.is_null() || aad_len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(aad, aad_len)
    };
    let ct = std::slice::from_raw_parts(ciphertext, ct_len);
    let plain = match open(&key, &nonce, aad_slice, ct) {
        Some(v) => v,
        None => return -4, // auth failure
    };
    *out_len = plain.len();
    if out.is_null() || plain.len() > out_cap {
        return -2;
    }
    std::ptr::copy_nonoverlapping(plain.as_ptr(), out, plain.len());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip() {
        let key = [7u8; 32];
        let nonce = [3u8; 12];
        let aad = b"channel-42";
        let msg = b"opus frame bytes here";
        let sealed = seal(&key, &nonce, aad, msg).expect("seal");
        assert_eq!(sealed.len(), msg.len() + VOICE_AEAD_TAG_LEN);
        let opened = open(&key, &nonce, aad, &sealed).expect("open");
        assert_eq!(opened, msg);
    }

    #[test]
    fn tamper_is_rejected() {
        let key = [1u8; 32];
        let nonce = [2u8; 12];
        let mut sealed = seal(&key, &nonce, b"", b"hello voice").expect("seal");
        sealed[0] ^= 0xff; // flip a byte
        assert!(open(&key, &nonce, b"", &sealed).is_none());
    }

    #[test]
    fn wrong_key_or_aad_rejected() {
        let nonce = [9u8; 12];
        let sealed = seal(&[1u8; 32], &nonce, b"aad-A", b"secret").expect("seal");
        assert!(open(&[2u8; 32], &nonce, b"aad-A", &sealed).is_none()); // wrong key
        assert!(open(&[1u8; 32], &nonce, b"aad-B", &sealed).is_none()); // wrong aad
    }

    #[test]
    fn ffi_roundtrip() {
        unsafe {
            let key = [5u8; 32];
            let nonce = [6u8; 12];
            let aad = b"ch";
            let msg = [11u8, 22, 33, 44, 55];
            let mut sealed = [0u8; 64];
            let mut slen = 0usize;
            assert_eq!(
                sc_voice_seal(key.as_ptr(), nonce.as_ptr(), aad.as_ptr(), aad.len(),
                    msg.as_ptr(), msg.len(), sealed.as_mut_ptr(), sealed.len(), &mut slen),
                0
            );
            assert_eq!(slen, msg.len() + 16);

            let mut plain = [0u8; 64];
            let mut plen = 0usize;
            assert_eq!(
                sc_voice_open(key.as_ptr(), nonce.as_ptr(), aad.as_ptr(), aad.len(),
                    sealed.as_ptr(), slen, plain.as_mut_ptr(), plain.len(), &mut plen),
                0
            );
            assert_eq!(&plain[..plen], &msg);

            // auth failure → -4
            sealed[2] ^= 1;
            assert_eq!(
                sc_voice_open(key.as_ptr(), nonce.as_ptr(), aad.as_ptr(), aad.len(),
                    sealed.as_ptr(), slen, plain.as_mut_ptr(), plain.len(), &mut plen),
                -4
            );
            // null key → -1
            assert_eq!(
                sc_voice_seal(std::ptr::null(), nonce.as_ptr(), std::ptr::null(), 0,
                    msg.as_ptr(), msg.len(), sealed.as_mut_ptr(), sealed.len(), &mut slen),
                -1
            );
        }
    }
}
