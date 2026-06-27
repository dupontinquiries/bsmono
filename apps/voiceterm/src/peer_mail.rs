// src/peer_mail.rs
//
// v0.1.879: in-house, device-as-provider messaging groundwork.
//
// The idea: every web2qml device is its own mail "provider." A user's
// ADDRESS is their public key. To message another w2q user you encrypt
// to THEIR public key; they decrypt with their secret key. No central
// server, no account — the device holds the identity and does the
// crypto. Transport (how a sealed envelope actually reaches the peer)
// is deliberately left open here: an envelope is a self-describing
// blob that can travel over any channel (a future IP/rendezvous layer,
// a file, a QR code, a relay). This module is the crypto + format +
// keyring groundwork that layer will sit on.
//
// Crypto: NaCl `crypto_box` (X25519 ECDH → XSalsa20-Poly1305 AEAD).
// `seal` encrypts to the recipient's public key and AUTHENTICATES the
// sender: the envelope carries the sender's public key, and `open`
// only succeeds when decrypting against that exact sender key. So the
// recipient learns both "this is for me" and "this came from that
// peer" — the property we want for w2q-to-w2q messages.
//
// Like the Proton at-rest crypto, the real primitive is gated behind
// the `mail-crypto` feature; without it the seal/open paths return
// `Unavailable` (the format, keyring, and identity plumbing still
// compile and are testable). Enable with `--features mail-crypto`.

use std::path::PathBuf;

use crate::mail_crypto::{b64_decode, b64_encode};
use crate::mail_json::{Json, Obj};

/// Length of an X25519 key in bytes.
pub const KEY_LEN: usize = 32;
/// Length of the XSalsa20 nonce in bytes.
pub const NONCE_LEN: usize = 24;

#[derive(Debug)]
pub enum PeerError {
    /// The crypto feature is compiled out (`mail-crypto` off).
    Unavailable,
    /// A crypto operation failed (bad key, auth/tag failure, tamper).
    Crypto(String),
    /// Malformed envelope / key / message text.
    Format(String),
    /// Filesystem error around identity/keyring storage.
    Io(String),
}

impl std::fmt::Display for PeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerError::Unavailable => write!(
                f,
                "peer messaging crypto is unavailable (build with --features mail-crypto)"
            ),
            PeerError::Crypto(m) => write!(f, "crypto error: {}", m),
            PeerError::Format(m) => write!(f, "format error: {}", m),
            PeerError::Io(m) => write!(f, "io error: {}", m),
        }
    }
}

pub type Result<T> = std::result::Result<T, PeerError>;

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// An X25519 public key — a w2q user's address.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PublicKey(pub [u8; KEY_LEN]);

/// An X25519 secret key. Held in memory; persisted via the identity
/// store (see the SECURITY note on `Identity`).
#[derive(Clone)]
pub struct SecretKey(pub [u8; KEY_LEN]);

impl PublicKey {
    pub fn to_b64(&self) -> String {
        b64_encode(&self.0)
    }
    pub fn from_b64(s: &str) -> Result<PublicKey> {
        let bytes = b64_decode(s.trim()).map_err(|e| PeerError::Format(e.to_string()))?;
        let arr = into_key_array(&bytes).ok_or_else(|| {
            PeerError::Format(format!("public key must be {} bytes", KEY_LEN))
        })?;
        Ok(PublicKey(arr))
    }
    /// Short fingerprint for display (first 8 b64 chars).
    pub fn fingerprint(&self) -> String {
        self.to_b64().chars().take(8).collect()
    }
}

impl SecretKey {
    pub fn to_b64(&self) -> String {
        b64_encode(&self.0)
    }
    pub fn from_b64(s: &str) -> Result<SecretKey> {
        let bytes = b64_decode(s.trim()).map_err(|e| PeerError::Format(e.to_string()))?;
        let arr = into_key_array(&bytes).ok_or_else(|| {
            PeerError::Format(format!("secret key must be {} bytes", KEY_LEN))
        })?;
        Ok(SecretKey(arr))
    }
}

fn into_key_array(bytes: &[u8]) -> Option<[u8; KEY_LEN]> {
    if bytes.len() != KEY_LEN {
        return None;
    }
    let mut arr = [0u8; KEY_LEN];
    arr.copy_from_slice(bytes);
    Some(arr)
}

// ---------------------------------------------------------------------------
// Message payload (plaintext, before sealing)
// ---------------------------------------------------------------------------

/// The plaintext a peer message carries. Mirrors a normal mail compose
/// so the existing compose pane can produce one.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PeerMessage {
    pub to: String,
    pub subject: String,
    pub body: String,
}

impl PeerMessage {
    fn to_json(&self) -> String {
        Obj::new()
            .str("to", &self.to)
            .str("subject", &self.subject)
            .str("body", &self.body)
            .build()
    }
    fn from_json(s: &str) -> Result<PeerMessage> {
        let j = Json::parse(s).map_err(PeerError::Format)?;
        Ok(PeerMessage {
            to: j.str_field("to").unwrap_or_default(),
            subject: j.str_field("subject").unwrap_or_default(),
            body: j.str_field("body").unwrap_or_default(),
        })
    }
}

// ---------------------------------------------------------------------------
// Sealed envelope (the transport-agnostic wire blob)
// ---------------------------------------------------------------------------

const ENVELOPE_HEADER: &str = "w2qmsg/1";

/// A sealed, transport-agnostic message. Carries the sender's public
/// key (for authentication + reply), the nonce, and the ciphertext.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub sender: PublicKey,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

impl Envelope {
    /// Serialize to the wire form: a 4-line, base64 text blob that can
    /// ride over any channel.
    pub fn to_wire(&self) -> String {
        format!(
            "{}\n{}\n{}\n{}",
            ENVELOPE_HEADER,
            self.sender.to_b64(),
            b64_encode(&self.nonce),
            b64_encode(&self.ciphertext),
        )
    }

    /// Parse a wire blob back into an envelope.
    pub fn from_wire(s: &str) -> Result<Envelope> {
        let mut lines = s.trim().lines();
        let header = lines
            .next()
            .ok_or_else(|| PeerError::Format("empty envelope".into()))?;
        if header.trim() != ENVELOPE_HEADER {
            return Err(PeerError::Format(format!(
                "unexpected header {:?}",
                header.trim()
            )));
        }
        let sender_b64 = lines
            .next()
            .ok_or_else(|| PeerError::Format("missing sender".into()))?;
        let nonce_b64 = lines
            .next()
            .ok_or_else(|| PeerError::Format("missing nonce".into()))?;
        let ct_b64 = lines
            .next()
            .ok_or_else(|| PeerError::Format("missing ciphertext".into()))?;
        let sender = PublicKey::from_b64(sender_b64)?;
        let nonce = b64_decode(nonce_b64.trim()).map_err(|e| PeerError::Format(e.to_string()))?;
        if nonce.len() != NONCE_LEN {
            return Err(PeerError::Format(format!(
                "nonce must be {} bytes",
                NONCE_LEN
            )));
        }
        let ciphertext =
            b64_decode(ct_b64.trim()).map_err(|e| PeerError::Format(e.to_string()))?;
        Ok(Envelope {
            sender,
            nonce,
            ciphertext,
        })
    }
}

// ---------------------------------------------------------------------------
// Keyring (contacts: handle -> public key)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    pub handle: String,
    pub key: PublicKey,
}

/// A simple list of known peers. Persisted as JSON at
/// `~/.config/web2qml/mail/peers/keyring.json`.
#[derive(Default)]
pub struct Keyring {
    pub contacts: Vec<Contact>,
}

impl Keyring {
    fn dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join(".config/web2qml").join("mail").join("peers")
    }
    fn path() -> PathBuf {
        Self::dir().join("keyring.json")
    }

    pub fn to_json(&self) -> String {
        let items: Vec<String> = self
            .contacts
            .iter()
            .map(|c| Obj::new().str("handle", &c.handle).str("key", &c.key.to_b64()).build())
            .collect();
        format!("{{\"contacts\":[{}]}}", items.join(","))
    }

    pub fn from_json(s: &str) -> Result<Keyring> {
        let j = Json::parse(s).map_err(PeerError::Format)?;
        let mut contacts = Vec::new();
        if let Some(arr) = j.get("contacts").and_then(|c| c.as_array()) {
            for item in arr {
                let handle = item.str_field("handle").unwrap_or_default();
                let key_b64 = item.str_field("key").unwrap_or_default();
                if handle.is_empty() || key_b64.is_empty() {
                    continue;
                }
                if let Ok(key) = PublicKey::from_b64(&key_b64) {
                    contacts.push(Contact { handle, key });
                }
            }
        }
        Ok(Keyring { contacts })
    }

    /// Add or update a contact by handle.
    pub fn upsert(&mut self, handle: &str, key: PublicKey) {
        if let Some(c) = self.contacts.iter_mut().find(|c| c.handle == handle) {
            c.key = key;
        } else {
            self.contacts.push(Contact {
                handle: handle.to_string(),
                key,
            });
        }
    }

    pub fn lookup(&self, handle: &str) -> Option<PublicKey> {
        self.contacts.iter().find(|c| c.handle == handle).map(|c| c.key)
    }

    pub fn load() -> Keyring {
        match std::fs::read_to_string(Self::path()) {
            Ok(s) => Keyring::from_json(&s).unwrap_or_default(),
            Err(_) => Keyring::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::dir();
        std::fs::create_dir_all(&dir).map_err(|e| PeerError::Io(e.to_string()))?;
        std::fs::write(Self::path(), self.to_json()).map_err(|e| PeerError::Io(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Identity (this device's keypair)
// ---------------------------------------------------------------------------

/// This device's messaging identity.
///
/// SECURITY (groundwork TODO): the secret key is currently stored
/// base64 in `identity.json`. Before this leaves the prototype stage it
/// must be sealed at rest with the existing `PolicySecretBox`
/// (machine-bound, policy-gated) the account store uses — the seam is
/// already there. Tracked in docs/design/email.md.
pub struct Identity {
    pub secret: SecretKey,
    pub public: PublicKey,
}

impl Identity {
    fn path() -> PathBuf {
        Keyring::dir().join("identity.json")
    }

    fn to_json(&self) -> String {
        Obj::new()
            .str("secret", &self.secret.to_b64())
            .str("public", &self.public.to_b64())
            .build()
    }

    fn from_json(s: &str) -> Result<Identity> {
        let j = Json::parse(s).map_err(PeerError::Format)?;
        let secret = SecretKey::from_b64(
            &j.str_field("secret")
                .ok_or_else(|| PeerError::Format("identity missing secret".into()))?,
        )?;
        let public = PublicKey::from_b64(
            &j.str_field("public")
                .ok_or_else(|| PeerError::Format("identity missing public".into()))?,
        )?;
        Ok(Identity { secret, public })
    }

    /// Load the device identity, generating + persisting one on first
    /// run. Requires the crypto feature to generate.
    pub fn load_or_create() -> Result<Identity> {
        if let Ok(s) = std::fs::read_to_string(Self::path()) {
            if let Ok(id) = Identity::from_json(&s) {
                return Ok(id);
            }
        }
        let (secret, public) = generate_keypair()?;
        let id = Identity { secret, public };
        let dir = Keyring::dir();
        std::fs::create_dir_all(&dir).map_err(|e| PeerError::Io(e.to_string()))?;
        std::fs::write(Self::path(), id.to_json()).map_err(|e| PeerError::Io(e.to_string()))?;
        Ok(id)
    }
}

// ---------------------------------------------------------------------------
// Crypto: real (feature) + stub (no feature)
// ---------------------------------------------------------------------------

/// Generate a fresh X25519 keypair.
pub fn generate_keypair() -> Result<(SecretKey, PublicKey)> {
    imp::generate_keypair()
}

/// Seal `msg` for `recipient`, authenticated by the sender keypair.
pub fn seal(
    recipient: &PublicKey,
    sender_secret: &SecretKey,
    sender_public: &PublicKey,
    msg: &PeerMessage,
) -> Result<Envelope> {
    imp::seal(recipient, sender_secret, sender_public, msg)
}

/// Open an envelope sealed for us, authenticating it against the
/// sender public key carried in the envelope.
pub fn open(env: &Envelope, my_secret: &SecretKey) -> Result<PeerMessage> {
    imp::open(env, my_secret)
}

#[cfg(feature = "mail-crypto")]
mod imp {
    use super::*;
    use crypto_box::aead::{Aead, AeadCore, OsRng};
    use crypto_box::{PublicKey as CbPublic, SalsaBox, SecretKey as CbSecret};

    pub fn generate_keypair() -> Result<(SecretKey, PublicKey)> {
        let sk = CbSecret::generate(&mut OsRng);
        let pk = sk.public_key();
        Ok((SecretKey(sk.to_bytes()), PublicKey(*pk.as_bytes())))
    }

    pub fn seal(
        recipient: &PublicKey,
        sender_secret: &SecretKey,
        sender_public: &PublicKey,
        msg: &PeerMessage,
    ) -> Result<Envelope> {
        let their_pub = CbPublic::from(recipient.0);
        let our_sec = CbSecret::from(sender_secret.0);
        let salsa = SalsaBox::new(&their_pub, &our_sec);
        let nonce = SalsaBox::generate_nonce(&mut OsRng);
        let plaintext = msg.to_json();
        let ciphertext = salsa
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| PeerError::Crypto(e.to_string()))?;
        Ok(Envelope {
            sender: *sender_public,
            nonce: nonce.to_vec(),
            ciphertext,
        })
    }

    pub fn open(env: &Envelope, my_secret: &SecretKey) -> Result<PeerMessage> {
        let their_pub = CbPublic::from(env.sender.0);
        let our_sec = CbSecret::from(my_secret.0);
        let salsa = SalsaBox::new(&their_pub, &our_sec);
        let nonce = crypto_box::Nonce::from_slice(&env.nonce);
        let plaintext = salsa
            .decrypt(nonce, env.ciphertext.as_ref())
            .map_err(|e| PeerError::Crypto(e.to_string()))?;
        let s = String::from_utf8(plaintext)
            .map_err(|e| PeerError::Format(e.to_string()))?;
        PeerMessage::from_json(&s)
    }
}

#[cfg(not(feature = "mail-crypto"))]
mod imp {
    use super::*;

    pub fn generate_keypair() -> Result<(SecretKey, PublicKey)> {
        Err(PeerError::Unavailable)
    }
    pub fn seal(
        _recipient: &PublicKey,
        _sender_secret: &SecretKey,
        _sender_public: &PublicKey,
        _msg: &PeerMessage,
    ) -> Result<Envelope> {
        Err(PeerError::Unavailable)
    }
    pub fn open(_env: &Envelope, _my_secret: &SecretKey) -> Result<PeerMessage> {
        Err(PeerError::Unavailable)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn key_a() -> PublicKey {
        PublicKey([7u8; KEY_LEN])
    }

    #[test]
    fn public_key_b64_roundtrip() {
        let k = key_a();
        let back = PublicKey::from_b64(&k.to_b64()).unwrap();
        assert_eq!(k, back);
        assert_eq!(k.fingerprint().len(), 8);
    }

    #[test]
    fn wrong_length_key_rejected() {
        assert!(PublicKey::from_b64(&b64_encode(&[1u8; 8])).is_err());
        assert!(PublicKey::from_b64("not base64 @@@").is_err());
    }

    #[test]
    fn peer_message_json_roundtrip() {
        let m = PeerMessage {
            to: "alice".into(),
            subject: "hi \"there\"".into(),
            body: "line1\nline2".into(),
        };
        let back = PeerMessage::from_json(&m.to_json()).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn envelope_wire_roundtrip() {
        let env = Envelope {
            sender: key_a(),
            nonce: vec![3u8; NONCE_LEN],
            ciphertext: vec![9u8; 40],
        };
        let wire = env.to_wire();
        assert!(wire.starts_with("w2qmsg/1\n"));
        let back = Envelope::from_wire(&wire).unwrap();
        assert_eq!(env, back);
    }

    #[test]
    fn envelope_rejects_bad_header_and_nonce() {
        assert!(Envelope::from_wire("nope/9\na\nb\nc").is_err());
        // valid header but truncated
        assert!(Envelope::from_wire("w2qmsg/1\nonly-sender").is_err());
        // wrong nonce length
        let bad = format!(
            "w2qmsg/1\n{}\n{}\n{}",
            key_a().to_b64(),
            b64_encode(&[1u8; 4]),
            b64_encode(&[2u8; 8])
        );
        assert!(Envelope::from_wire(&bad).is_err());
    }

    #[test]
    fn keyring_json_roundtrip_and_upsert() {
        let mut kr = Keyring::default();
        kr.upsert("alice", PublicKey([1u8; KEY_LEN]));
        kr.upsert("bob", PublicKey([2u8; KEY_LEN]));
        // upsert same handle replaces
        kr.upsert("alice", PublicKey([9u8; KEY_LEN]));
        assert_eq!(kr.contacts.len(), 2);
        assert_eq!(kr.lookup("alice"), Some(PublicKey([9u8; KEY_LEN])));
        assert_eq!(kr.lookup("nobody"), None);

        let back = Keyring::from_json(&kr.to_json()).unwrap();
        assert_eq!(back.contacts.len(), 2);
        assert_eq!(back.lookup("bob"), Some(PublicKey([2u8; KEY_LEN])));
    }

    #[cfg(not(feature = "mail-crypto"))]
    #[test]
    fn crypto_unavailable_without_feature() {
        assert!(matches!(generate_keypair(), Err(PeerError::Unavailable)));
    }

    // Real crypto roundtrip — only when the feature is on.
    // Run: cargo test --features mail-crypto peer_mail
    #[cfg(feature = "mail-crypto")]
    mod crypto {
        use super::*;

        #[test]
        fn seal_open_roundtrip_authenticated() {
            let (a_sec, a_pub) = generate_keypair().unwrap();
            let (b_sec, b_pub) = generate_keypair().unwrap();
            let msg = PeerMessage {
                to: b_pub.to_b64(),
                subject: "hello".into(),
                body: "from alice to bob".into(),
            };
            // Alice seals for Bob.
            let env = seal(&b_pub, &a_sec, &a_pub, &msg).unwrap();
            // The envelope is addressed-from Alice.
            assert_eq!(env.sender, a_pub);
            // Bob opens it.
            let got = open(&env, &b_sec).unwrap();
            assert_eq!(got, msg);
        }

        #[test]
        fn wrong_recipient_cannot_open() {
            let (a_sec, a_pub) = generate_keypair().unwrap();
            let (_b_sec, b_pub) = generate_keypair().unwrap();
            let (c_sec, _c_pub) = generate_keypair().unwrap();
            let msg = PeerMessage { to: "bob".into(), subject: "s".into(), body: "secret".into() };
            let env = seal(&b_pub, &a_sec, &a_pub, &msg).unwrap();
            // Carol (not the recipient) cannot decrypt.
            assert!(open(&env, &c_sec).is_err());
        }

        #[test]
        fn tampered_ciphertext_fails_auth() {
            let (a_sec, a_pub) = generate_keypair().unwrap();
            let (b_sec, b_pub) = generate_keypair().unwrap();
            let msg = PeerMessage { to: "bob".into(), subject: "s".into(), body: "data".into() };
            let mut env = seal(&b_pub, &a_sec, &a_pub, &msg).unwrap();
            // Flip a ciphertext byte → AEAD tag check must fail.
            env.ciphertext[0] ^= 0x01;
            assert!(open(&env, &b_sec).is_err());
        }

        #[test]
        fn forged_sender_key_breaks_open() {
            // If an attacker rewrites the envelope's sender key, the box
            // no longer matches and open fails — sender authenticity.
            let (a_sec, a_pub) = generate_keypair().unwrap();
            let (b_sec, b_pub) = generate_keypair().unwrap();
            let (_x_sec, x_pub) = generate_keypair().unwrap();
            let msg = PeerMessage { to: "bob".into(), subject: "s".into(), body: "auth".into() };
            let mut env = seal(&b_pub, &a_sec, &a_pub, &msg).unwrap();
            env.sender = x_pub; // forge sender
            assert!(open(&env, &b_sec).is_err());
        }
    }
}
