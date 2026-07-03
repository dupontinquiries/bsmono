/* snapclone_crypto_ffi.h — C FFI header for the snapclone-crypto Rust library.
 *
 * This file is committed to the repo so the project compiles without running
 * cargo first. Run `cargo build --release` in rust/snapclone-crypto to rebuild
 * the static library; cbindgen regenerates this header as part of that build.
 *
 * Usage: #include "crypto/snapclone_crypto_ffi.h" inside extern "C" {} in C++.
 */

#ifndef SNAPCLONE_CRYPTO_FFI_H
#define SNAPCLONE_CRYPTO_FFI_H

#include <stdint.h>
#include <stddef.h>

/* ─── Error codes ─────────────────────────────────────────────────────────── */

typedef enum CryptoError {
    CryptoError_Ok               = 0,
    CryptoError_InvalidInput     = 1,
    CryptoError_EncryptFailed    = 2,
    CryptoError_DecryptFailed    = 3,
    CryptoError_InvalidSignature = 4,
    CryptoError_BufferTooSmall   = 5,
} CryptoError;

/* ─── Runtime size queries ───────────────────────────────────────────────── */

uintptr_t sc_nonce_len(void);
uintptr_t sc_key_len(void);
uintptr_t sc_sig_len(void);
uintptr_t sc_pub_key_len(void);
uintptr_t sc_priv_key_len(void);

/* ─── Key generation ─────────────────────────────────────────────────────── */

/* Generate X25519 keypair: priv_out/pub_out each 32 bytes */
CryptoError sc_generate_x25519_keypair(uint8_t *priv_out, uint8_t *pub_out);

/* Generate Ed25519 keypair: priv_out/pub_out each 32 bytes */
CryptoError sc_generate_ed25519_keypair(uint8_t *priv_out, uint8_t *pub_out);

/* ─── ECDH ───────────────────────────────────────────────────────────────── */

/* X25519(our_priv, peer_pub) -> HKDF-SHA256 -> key_out (32 bytes) */
CryptoError sc_derive_session_key(
    const uint8_t *our_priv,
    const uint8_t *peer_pub,
    uint8_t       *key_out
);

/* X25519(our_priv, peer_pub) -> raw 32-byte shared secret (NO HKDF).
 * For use in hybrid constructions where the raw shared secret is
 * combined with another (e.g. ML-KEM-768 output) before HKDF.
 * V32.33+ */
CryptoError sc_x25519_raw_shared(
    const uint8_t *our_priv,
    const uint8_t *peer_pub,
    uint8_t       *shared_out
);

/* ─── AES-256-GCM ────────────────────────────────────────────────────────── */

/* Encrypt: output = [12-byte nonce][ciphertext][16-byte GCM tag]
 * cipher_cap must be >= plaintext_len + 12 + 16 */
CryptoError sc_encrypt_aes256gcm(
    const uint8_t *key,
    const uint8_t *plaintext,
    uintptr_t      plaintext_len,
    uint8_t       *cipher_out,
    uintptr_t      cipher_cap,
    uintptr_t     *cipher_len
);

/* Decrypt: input = [12-byte nonce][ciphertext][16-byte GCM tag] */
CryptoError sc_decrypt_aes256gcm(
    const uint8_t *key,
    const uint8_t *ciphertext,
    uintptr_t      cipher_len,
    uint8_t       *plain_out,
    uintptr_t      plain_cap,
    uintptr_t     *plain_len
);

/* ─── Ed25519 ─────────────────────────────────────────────────────────────── */

CryptoError sc_sign_ed25519(
    const uint8_t *priv_key,
    const uint8_t *data,
    uintptr_t      data_len,
    uint8_t       *sig_out    /* 64 bytes */
);

CryptoError sc_verify_ed25519(
    const uint8_t *pub_key,
    const uint8_t *data,
    uintptr_t      data_len,
    const uint8_t *sig        /* 64 bytes */
);

/* ─── ML-KEM-768 post-quantum KEM (FIPS 203) ───────────────────────────────
 *
 * Hybrid construction: combine output of sc_derive_session_key (X25519)
 * with sc_mlkem_* (ML-KEM-768) via sc_derive_hybrid_session_key. Both
 * primitives must be broken for the session to fall.
 *
 * Sizes are runtime-queryable via sc_mlkem_*_len() to defend against
 * future crate version drift, but they match FIPS 203 exactly:
 *   encapsulation key (public)  : 1184 bytes
 *   decapsulation key (private) : 2400 bytes
 *   ciphertext                  : 1088 bytes
 *   shared secret               :   32 bytes
 *
 * Added: V32.33+ (cbindgen-regenerable, but committed by hand here to
 *        avoid round-tripping through a Rust toolchain to update the
 *        committed header on every API change).
 */

uintptr_t sc_mlkem_ek_len(void);   /* 1184 */
uintptr_t sc_mlkem_dk_len(void);   /* 2400 */
uintptr_t sc_mlkem_ct_len(void);   /* 1088 */
uintptr_t sc_mlkem_ss_len(void);   /*   32 */

/* Generate an ML-KEM-768 keypair.
 *   dk_out: 2400 bytes — keep private; expand in memory only when decapsulating.
 *   ek_out: 1184 bytes — ship to peer in handshake. */
CryptoError sc_mlkem_generate_keypair(uint8_t *dk_out, uint8_t *ek_out);

/* Encapsulate to a peer's encapsulation key. Called by the RESPONDER.
 *   peer_ek: 1184 bytes (peer's public encapsulation key).
 *   ct_out : 1088 bytes (ciphertext to send back to peer).
 *   ss_out :   32 bytes (shared secret derived locally). */
CryptoError sc_mlkem_encapsulate(
    const uint8_t *peer_ek,
    uint8_t       *ct_out,
    uint8_t       *ss_out
);

/* Decapsulate a peer's ciphertext. Called by the INITIATOR.
 *   our_dk: 2400 bytes (our decapsulation key, kept since keygen).
 *   ct    : 1088 bytes (ciphertext received from peer).
 *   ss_out:   32 bytes (shared secret — matches what peer derived).
 *
 * Note: FIPS 203 §6.3 specifies "implicit rejection". On tampered ct
 * the function may return Ok but ss_out will be a deterministic
 * pseudo-random value derived from the dk's implicit-reject component,
 * NOT the original shared secret. Detection must happen at the AEAD
 * layer above (the bogus key will fail GCM tag verification). */
CryptoError sc_mlkem_decapsulate(
    const uint8_t *our_dk,
    const uint8_t *ct,
    uint8_t       *ss_out
);

/* Hybrid session key derivation.
 *   x25519_shared: 32 bytes (output of X25519 ECDH, same as sc_derive_session_key
 *                  input, but here we want the raw shared secret BEFORE HKDF —
 *                  in this call we do the HKDF ourselves over concat).
 *   mlkem_shared : 32 bytes (output of sc_mlkem_encapsulate / sc_mlkem_decapsulate).
 *   key_out      : 32 bytes (AES-256 session key for sealing payloads).
 *
 * Concatenation order is FIXED: x25519 first, then mlkem. Both peers MUST
 * use the same ordering or they'll arrive at different session keys. */
CryptoError sc_derive_hybrid_session_key(
    const uint8_t *x25519_shared,
    const uint8_t *mlkem_shared,
    uint8_t       *key_out
);

/* ─── Canonical constant aliases ─────────────────────────────────────────── */
/* cbindgen variant naming differs across versions and configurations.
 * Define canonical CryptoError_X macros as integer casts — guaranteed
 * to match the #[repr(C)] values regardless of how cbindgen named the variants.
 * In C++ enum members are NOT preprocessor macros, so these defines never
 * conflict even if cbindgen also emits CryptoError_Ok as an enum member. */
#ifndef SNAPCLONE_CRYPTO_ALIASES_DEFINED
#define SNAPCLONE_CRYPTO_ALIASES_DEFINED
#  define CryptoError_Ok               ((CryptoError)0)
#  define CryptoError_InvalidInput     ((CryptoError)1)
#  define CryptoError_EncryptFailed    ((CryptoError)2)
#  define CryptoError_DecryptFailed    ((CryptoError)3)
#  define CryptoError_InvalidSignature ((CryptoError)4)
#  define CryptoError_BufferTooSmall   ((CryptoError)5)
#endif

#endif /* SNAPCLONE_CRYPTO_FFI_H */
