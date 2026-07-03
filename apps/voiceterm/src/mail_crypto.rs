//! v0.1.861: the crypto boundary for the ProtonMail bridge.
//!
//! hydroxide reaches straight for concrete primitives — `nacl/secretbox`
//! for the on-disk auth blob, `bcrypt` for the in-memory session gate,
//! a bespoke SRP, and `go-crypto/openpgp` for key unlock and message
//! bodies. That makes the security surface impossible to swap or fake in
//! a test, and it would drag four heavy crates into web2qml's otherwise
//! lean dependency tree unconditionally.
//!
//! Instead we define four small traits and inject them. Two payoffs:
//!
//!   1. **The whole bridge is testable without real crypto.** The account
//!      store, session manager, transport, and multi-user routing depend
//!      only on these traits, so unit tests drive them with fakes.
//!
//!   2. **It follows the crate's own feature discipline.** Exactly like
//!      `llamacpp`/`wavenet`/`claude`, the real implementations live
//!      behind a cargo feature (`mail-crypto`) that pulls the heavy
//!      crates. With the feature **off**, the default providers below are
//!      *safe-inert*: they never write plaintext secrets to disk and they
//!      refuse the operations they can't perform, surfacing
//!      `MailError::CryptoUnavailable` instead of doing something unsafe.
//!
//! The feature-off fallback is the important safety property: a build
//! without `mail-crypto` can still authenticate live sessions in memory
//! for the current process, but it will not persist credentials in the
//! clear. There is no "degrade to plaintext" path anywhere.

use crate::mail_error::{MailError, Result};

// ---------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------

/// Authenticated encryption for data at rest (the per-account credential
/// blob). Keyed by the account's 32-byte bridge key. This is the NaCl
/// `secretbox` role in hydroxide: `seal` prepends a fresh nonce, `open`
/// authenticates before returning plaintext.
pub trait SecretBox: Send + Sync {
    /// Encrypt `plaintext` under `key` (32 bytes). Output is self-framed
    /// (nonce ++ ciphertext ++ tag) and safe to write to disk.
    fn seal(&self, key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>>;

    /// Decrypt and authenticate. Returns [`MailError::Unauthorized`] when
    /// the tag check fails — a wrong bridge key is indistinguishable from
    /// a tampered blob, which is the property we want.
    fn open(&self, key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>>;
}

/// In-memory gate for the warm session cache: once an account is unlocked,
/// a repeat bridge login (a second IMAP connection, say) should be
/// admitted by comparing against a stored verifier without re-running the
/// whole unlock. hydroxide uses bcrypt over the raw key for this.
pub trait PasswordGate: Send + Sync {
    /// Produce a verifier to stash alongside the warm session.
    fn make_verifier(&self, key: &[u8; 32]) -> Result<Vec<u8>>;
    /// Constant-time check of a presented key against a stored verifier.
    fn verify(&self, verifier: &[u8], key: &[u8; 32]) -> bool;
}

/// Parameters returned by ProtonMail's `/auth/info` for the SRP exchange.
#[derive(Clone, Debug, Default)]
pub struct SrpInfo {
    pub version: i64,
    /// PGP-signed modulus (armored). The real SRP impl must verify this
    /// signature against Proton's published modulus key before using it.
    pub modulus: String,
    pub server_ephemeral: String,
    pub salt: String,
    pub srp_session: String,
}

/// Client proofs to send to `/auth`, plus the expected server proof.
#[derive(Clone, Debug, Default)]
pub struct SrpProofs {
    pub client_ephemeral: String, // base64
    pub client_proof: String,     // base64
    pub expected_server_proof: String,
}

/// The SRP role. Pure computation — no I/O — so it's trivially testable
/// with vectors.
pub trait Srp: Send + Sync {
    fn compute(&self, password: &[u8], info: &SrpInfo) -> Result<SrpProofs>;
    /// Verify the server's proof from the `/auth` response against the
    /// value [`Srp::compute`] predicted.
    fn verify_server(&self, proofs: &SrpProofs, server_proof_b64: &str) -> Result<()>;
}

/// An armored PGP private key plus the metadata needed to unlock it,
/// lifted from a Proton key object.
#[derive(Clone, Debug)]
pub struct LockedKey {
    pub id: String,
    pub fingerprint: String,
    pub private_key_armored: String,
    /// Address-key token (armored), present when the key is locked by a
    /// user key rather than the mailbox passphrase.
    pub token: Option<String>,
    pub signature: Option<String>,
    pub active: bool,
}

/// Opaque handle to an unlocked keyring. The concrete type lives in the
/// real adapter; the rest of the bridge only moves it around.
pub trait KeyRing: Send + Sync {
    /// Number of successfully unlocked keys (diagnostic).
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// PGP engine: unlock keys and (eventually) decrypt/encrypt message
/// bodies. Decrypt/encrypt are split out so the read path can land before
/// the send path — see `mail_proton.rs`.
pub trait PgpEngine: Send + Sync {
    /// Unlock the given keys with `passphrase` (the mailbox password) and
    /// per-key `salts`, returning a keyring. `user_keys` are unlocked
    /// first and used to decrypt address-key tokens, mirroring
    /// hydroxide's two-stage `Unlock`.
    fn unlock(
        &self,
        user_keys: &[LockedKey],
        address_keys: &[LockedKey],
        salts: &std::collections::HashMap<String, Vec<u8>>,
        passphrase: &[u8],
    ) -> Result<Box<dyn KeyRing>>;

    /// Decrypt an armored PGP message body using the unlocked keyring.
    fn decrypt(&self, keyring: &dyn KeyRing, armored: &str) -> Result<Vec<u8>>;
}

/// Bundle of the four providers the bridge needs. Constructed once and
/// shared (behind `Arc`) by the [`crate::mail::MailHub`].
pub struct CryptoSuite {
    pub secret_box: Box<dyn SecretBox>,
    pub gate: Box<dyn PasswordGate>,
    pub srp: Box<dyn Srp>,
    pub pgp: Box<dyn PgpEngine>,
}

impl CryptoSuite {
    /// The suite selected at compile time. With `mail-crypto` it wires the
    /// real adapters; without it, the safe-inert defaults.
    pub fn platform() -> Self {
        #[cfg(feature = "mail-crypto")]
        {
            real::suite()
        }
        #[cfg(not(feature = "mail-crypto"))]
        {
            CryptoSuite {
                secret_box: Box::new(NoDiskSecretBox),
                gate: Box::new(CtPasswordGate),
                srp: Box::new(UnavailableSrp),
                pgp: Box::new(UnavailablePgp),
            }
        }
    }
}

/// A standalone [`SecretBox`] selected the same way [`CryptoSuite::platform`]
/// selects its own. The account store owns its sealing primitive directly
/// (rather than borrowing the suite's) so that store persistence and the
/// session-cache crypto stay ownership-independent and separately testable.
pub fn platform_secret_box() -> Box<dyn SecretBox> {
    #[cfg(feature = "mail-crypto")]
    {
        Box::new(real::NaclSecretBox)
    }
    #[cfg(not(feature = "mail-crypto"))]
    {
        Box::new(NoDiskSecretBox)
    }
}

// ---------------------------------------------------------------------
// base64 (standard alphabet, with padding)
//
// Proton sends key salts, SRP ephemerals, and proofs as base64, and the
// bridge password is a base64-encoded 32-byte key. No base64 crate is in
// the tree, so here's a compact, allocation-light pair. Kept here next to
// the other encoding/crypto utilities rather than in a fourth module.
// ---------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

pub fn b64_decode(s: &str) -> Result<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    // Strip ASCII whitespace and any trailing '=' padding; we recompute the
    // output length from the count of significant characters instead, which
    // makes the decoder correct for both padded (Proton's salts/proofs) and
    // unpadded base64url-style inputs.
    //
    // Bugfix (v0.1.862): the prior version walked fixed 4-byte chunks and
    // unconditionally emitted up to 3 output bytes per chunk, so an
    // unpadded 2- or 3-char tail produced spurious bytes and a stray 1-char
    // tail was accepted silently.
    let sig: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    // A final group of exactly one significant char encodes only 6 bits —
    // not enough for even one output byte. That is never valid base64.
    if sig.len() % 4 == 1 {
        return Err(MailError::Decode("truncated base64".into()));
    }
    let mut out = Vec::with_capacity(sig.len() / 4 * 3 + 2);
    for chunk in sig.chunks(4) {
        let mut n = 0u32;
        for &c in chunk {
            n = (n << 6) | val(c).ok_or_else(|| MailError::Decode("bad base64".into()))?;
        }
        // Left-align the partial group so the high bits carry the data.
        let bytes_out = chunk.len() - 1; // 4->3, 3->2, 2->1
        n <<= 6 * (4 - chunk.len());
        if bytes_out >= 1 {
            out.push((n >> 16) as u8);
        }
        if bytes_out >= 2 {
            out.push((n >> 8) as u8);
        }
        if bytes_out >= 3 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// Safe-inert default providers (feature off)
// ---------------------------------------------------------------------

/// Constant-time byte comparison. Tiny and dependency-free; used by the
/// default gate and worth having in-tree regardless.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Refuses to write secrets to disk. This is the safety net for builds
/// without `mail-crypto`: the account store catches the error and keeps
/// the record in memory only.
pub struct NoDiskSecretBox;

impl SecretBox for NoDiskSecretBox {
    fn seal(&self, _key: &[u8; 32], _plaintext: &[u8]) -> Result<Vec<u8>> {
        Err(MailError::CryptoUnavailable(
            "at-rest credential encryption",
        ))
    }
    fn open(&self, _key: &[u8; 32], _sealed: &[u8]) -> Result<Vec<u8>> {
        Err(MailError::CryptoUnavailable(
            "at-rest credential decryption",
        ))
    }
}

/// In-memory-only gate: the verifier is just the key, compared in constant
/// time. Safe because nothing here is persisted — the warm-session cache
/// lives and dies with the process.
pub struct CtPasswordGate;

impl PasswordGate for CtPasswordGate {
    fn make_verifier(&self, key: &[u8; 32]) -> Result<Vec<u8>> {
        Ok(key.to_vec())
    }
    fn verify(&self, verifier: &[u8], key: &[u8; 32]) -> bool {
        ct_eq(verifier, key)
    }
}

pub struct UnavailableSrp;
impl Srp for UnavailableSrp {
    fn compute(&self, _password: &[u8], _info: &SrpInfo) -> Result<SrpProofs> {
        Err(MailError::CryptoUnavailable("SRP login"))
    }
    fn verify_server(&self, _p: &SrpProofs, _s: &str) -> Result<()> {
        Err(MailError::CryptoUnavailable("SRP login"))
    }
}

pub struct UnavailablePgp;
impl PgpEngine for UnavailablePgp {
    fn unlock(
        &self,
        _u: &[LockedKey],
        _a: &[LockedKey],
        _s: &std::collections::HashMap<String, Vec<u8>>,
        _p: &[u8],
    ) -> Result<Box<dyn KeyRing>> {
        Err(MailError::CryptoUnavailable("PGP key unlock"))
    }
    fn decrypt(&self, _k: &dyn KeyRing, _a: &str) -> Result<Vec<u8>> {
        Err(MailError::CryptoUnavailable("PGP decryption"))
    }
}

// ---------------------------------------------------------------------
// Real adapters (feature `mail-crypto`)
// ---------------------------------------------------------------------
//
// Compiled only with `--features mail-crypto`, which adds these deps:
//
//   crypto_secretbox = "0.1"   # XSalsa20Poly1305 == NaCl secretbox
//   bcrypt           = "0.15"
//   sequoia-openpgp  = "1"      # or `pgp = "0.13"` for a pure-Rust build
//   srp / num-bigint + sha2     # Proton's SRP uses a custom 2048-bit
//                               # modulus and SHA-512; see proton-srp.
//
// These wrap well-known crate APIs behind the traits above. The
// Proton-specific parameters (modulus signature verification, the exact
// SRP hash construction, the address-key token decrypt) are marked TODO
// where they need Proton's published constants — everything *around* them
// is already exercised by the trait-level tests.
#[cfg(feature = "mail-crypto")]
mod real {
    use super::*;
    use crypto_secretbox::aead::{Aead, KeyInit};
    use crypto_secretbox::{Nonce, XSalsa20Poly1305};

    pub fn suite() -> CryptoSuite {
        CryptoSuite {
            secret_box: Box::new(NaclSecretBox),
            gate: Box::new(BcryptGate),
            srp: Box::new(ProtonSrp),
            pgp: Box::new(SequoiaPgp),
        }
    }

    /// NaCl secretbox, byte-compatible with hydroxide's on-disk format
    /// (24-byte nonce prefix + XSalsa20Poly1305).
    pub struct NaclSecretBox;
    impl SecretBox for NaclSecretBox {
        fn seal(&self, key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
            let cipher = XSalsa20Poly1305::new(key.into());
            let mut nonce = [0u8; 24];
            getrandom(&mut nonce)?;
            let ct = cipher
                .encrypt(Nonce::from_slice(&nonce), plaintext)
                .map_err(|_| MailError::Other("secretbox seal failed".into()))?;
            let mut out = Vec::with_capacity(24 + ct.len());
            out.extend_from_slice(&nonce);
            out.extend_from_slice(&ct);
            Ok(out)
        }
        fn open(&self, key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>> {
            if sealed.len() < 24 {
                return Err(MailError::Unauthorized);
            }
            let cipher = XSalsa20Poly1305::new(key.into());
            cipher
                .decrypt(Nonce::from_slice(&sealed[..24]), &sealed[24..])
                .map_err(|_| MailError::Unauthorized)
        }
    }

    pub struct BcryptGate;
    impl PasswordGate for BcryptGate {
        fn make_verifier(&self, key: &[u8; 32]) -> Result<Vec<u8>> {
            bcrypt::hash(key, bcrypt::DEFAULT_COST)
                .map(String::into_bytes)
                .map_err(|e| MailError::Other(e.to_string()))
        }
        fn verify(&self, verifier: &[u8], key: &[u8; 32]) -> bool {
            match std::str::from_utf8(verifier) {
                Ok(h) => bcrypt::verify(key, h).unwrap_or(false),
                Err(_) => false,
            }
        }
    }

    /// Proton's SRP. The surrounding auth flow in mail_proton.rs is fully
    /// driven by this trait; this is the single Proton-specific integration
    /// point left.
    ///
    /// v0.1.903: the intended implementation is the `proton-srp` crate
    /// (ProtonMail's own pure-Rust SRP — the "safe path" named in
    /// docs/design/email.md), NOT a hand-rolled port. It is not wired here
    /// because `proton-srp` 0.8.x requires Rust edition 2024 (MSRV ~1.85)
    /// while this project is on edition 2021; pulling it in needs a
    /// toolchain bump first. See TODO_mail_srp_login.md for the full plan
    /// and the exact wiring (behind a `mail-proton-srp` feature) so this
    /// can drop in without a blind hand-port. Until then, return an error
    /// that names the real blocker.
    pub struct ProtonSrp;
    impl Srp for ProtonSrp {
        fn compute(&self, _password: &[u8], _info: &SrpInfo) -> Result<SrpProofs> {
            Err(MailError::Other(
                "Proton SRP login isn't wired in this build yet (needs the \
                 proton-srp crate + a toolchain bump — see TODO_mail_srp_login.md)"
                    .into(),
            ))
        }
        fn verify_server(&self, proofs: &SrpProofs, server_proof_b64: &str) -> Result<()> {
            if super::ct_eq(
                proofs.expected_server_proof.as_bytes(),
                server_proof_b64.as_bytes(),
            ) {
                Ok(())
            } else {
                Err(MailError::SrpServerProof)
            }
        }
    }

    pub struct SequoiaPgp;
    pub struct SequoiaKeyRing {
        count: usize,
    }
    impl KeyRing for SequoiaKeyRing {
        fn len(&self) -> usize {
            self.count
        }
    }
    impl PgpEngine for SequoiaPgp {
        fn unlock(
            &self,
            _user_keys: &[LockedKey],
            _address_keys: &[LockedKey],
            _salts: &std::collections::HashMap<String, Vec<u8>>,
            _passphrase: &[u8],
        ) -> Result<Box<dyn KeyRing>> {
            // Wire to sequoia-openpgp here: parse each armored key, derive
            // the per-key passphrase (computeKeyPassword == bcrypt-based
            // KDF over salt) or decrypt the address-key token with the
            // already-unlocked user keyring, then decrypt the secret key
            // material. Returns the assembled keyring.
            Err(MailError::Other(
                "SequoiaPgp: wire sequoia-openpgp here".into(),
            ))
        }
        fn decrypt(&self, _k: &dyn KeyRing, _armored: &str) -> Result<Vec<u8>> {
            Err(MailError::Other(
                "SequoiaPgp: wire sequoia-openpgp here".into(),
            ))
        }
    }

    fn getrandom(buf: &mut [u8]) -> Result<()> {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(buf))
            .map_err(|e| MailError::Other(format!("rng: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_basic() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn default_gate_roundtrips_in_memory() {
        let g = CtPasswordGate;
        let key = [7u8; 32];
        let v = g.make_verifier(&key).unwrap();
        assert!(g.verify(&v, &key));
        assert!(!g.verify(&v, &[8u8; 32]));
    }

    #[test]
    fn default_secretbox_refuses_disk() {
        let sb = NoDiskSecretBox;
        let err = sb.seal(&[0u8; 32], b"secret").unwrap_err();
        assert!(matches!(err, MailError::CryptoUnavailable(_)));
    }

    #[test]
    fn b64_roundtrips_all_lengths() {
        for n in 0..40usize {
            let data: Vec<u8> = (0..n).map(|i| (i as u8).wrapping_mul(37)).collect();
            let enc = b64_encode(&data);
            assert_eq!(b64_decode(&enc).unwrap(), data, "len {n}");
        }
    }

    #[test]
    fn b64_decodes_unpadded_and_padded_alike() {
        // "Man" -> "TWFu" (no pad), "Ma" -> "TWE=" / "TWE" both valid.
        assert_eq!(b64_decode("TWFu").unwrap(), b"Man");
        assert_eq!(b64_decode("TWE=").unwrap(), b"Ma");
        assert_eq!(b64_decode("TWE").unwrap(), b"Ma"); // unpadded tail
        assert_eq!(b64_decode("TQ==").unwrap(), b"M");
        assert_eq!(b64_decode("TQ").unwrap(), b"M"); // unpadded single byte
    }

    #[test]
    fn b64_ignores_whitespace_and_rejects_garbage() {
        assert_eq!(b64_decode("TW Fu\n").unwrap(), b"Man");
        assert!(b64_decode("T").is_err()); // 1-char tail can't decode
        assert!(b64_decode("**").is_err()); // non-alphabet
    }
}
