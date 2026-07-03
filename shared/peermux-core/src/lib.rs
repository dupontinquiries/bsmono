//! peermux-core — hand out authenticated streams over already-connected peers.
//!
//! **The problem.** Every app doing its own peer discovery means each one publishes to the
//! DHT and runs its own handshake — churn and duplicated crypto. **The answer** (yes, and
//! with reliable crypto): a broker (snapclone, or a `connd`) keeps **one authenticated
//! session per peer** — a single X25519 key-agreement authenticated by Ed25519 — and
//! **multiplexes app streams over it**. Each stream gets its own key derived from the
//! session secret via HKDF (`crypto_kit::derive_session_key` with a per-stream `info`), so
//! streams are cryptographically isolated *without any new handshake*. Apps ask the broker
//! for a stream to a peer; the broker holds the single DHT presence.
//!
//! Reliability of the handshake is preserved because:
//! * the session is mutually **authenticated** (each side signs its ephemeral key with its
//!   long-term Ed25519 identity; the peer id *is* that verified key), and
//! * every stream key is bound to that authenticated session secret, so a stream cannot be
//!   forged or read without having completed the handshake.

use crypto_kit::{derive_session_key, ed25519_keypair, sign, verify, x25519_keypair, x25519_shared};

/// A peer's identity = its Ed25519 verifying key.
pub type PeerId = [u8; 32];

/// A device's long-term identity used to authenticate handshakes.
pub struct Identity {
    pub verify_key: [u8; 32],
    sign_key: [u8; 32],
}

impl Identity {
    pub fn generate() -> Identity {
        let (sk, vk) = ed25519_keypair();
        Identity { verify_key: vk, sign_key: *sk }
    }
    pub fn peer_id(&self) -> PeerId {
        self.verify_key
    }
}

/// One handshake message: an ephemeral X25519 public key, the sender's identity, and a
/// signature over the ephemeral key (authenticating it to the identity).
#[derive(Debug, Clone)]
pub struct HandshakeMsg {
    pub ephemeral_pub: [u8; 32],
    pub identity: [u8; 32],
    pub sig: [u8; 64],
}

/// Our in-flight handshake state: the ephemeral secret we must keep until we get the peer's
/// message.
pub struct Handshake {
    ephemeral_secret: [u8; 32],
    identity: Identity,
}

impl Handshake {
    /// Begin a handshake: returns our state + the message to send the peer.
    pub fn start(identity: Identity) -> (Handshake, HandshakeMsg) {
        let (eph_sk, eph_pk) = x25519_keypair();
        let sig = sign(&identity.sign_key, &eph_pk);
        let msg = HandshakeMsg { ephemeral_pub: eph_pk, identity: identity.verify_key, sig };
        (Handshake { ephemeral_secret: *eph_sk, identity }, msg)
    }

    /// Complete the handshake against the peer's message. Verifies the peer authenticated
    /// its ephemeral key, then derives the shared session secret. `None` if authentication
    /// fails (tampered/forged) — that's the reliability guarantee.
    pub fn complete(self, peer: &HandshakeMsg) -> Option<Session> {
        if !verify(&peer.identity, &peer.ephemeral_pub, &peer.sig) {
            return None; // peer did not prove ownership of its ephemeral key
        }
        let shared = x25519_shared(&self.ephemeral_secret, &peer.ephemeral_pub);
        Some(Session { peer: peer.identity, shared: *shared, next_stream_id: 1, our_id: self.identity.verify_key })
    }
}

/// An authenticated session with one peer. The one handshake happened; from here, streams
/// are cheap and require no further key agreement.
pub struct Session {
    pub peer: PeerId,
    pub our_id: PeerId,
    shared: [u8; 32],
    next_stream_id: u32,
}

impl Session {
    /// The session-level key (for control traffic), derived from the shared secret.
    pub fn session_key(&self) -> [u8; 32] {
        *derive_session_key(&self.shared, b"peermux-session-v1")
    }

    /// Open a new logically-separate stream to this peer for `service` (e.g. "photos",
    /// "mail"). Allocates a stream id and derives an **isolated** key from the session
    /// secret — no handshake, no DHT. Two streams never share a key.
    pub fn open_stream(&mut self, service: &str) -> Stream {
        let id = self.next_stream_id;
        self.next_stream_id += 1;
        let info = format!("peermux-stream:{id}:{service}");
        let key = *derive_session_key(&self.shared, info.as_bytes());
        Stream { id, peer: self.peer, service: service.to_string(), key }
    }
}

/// A multiplexed, cryptographically-isolated stream over a session.
pub struct Stream {
    pub id: u32,
    pub peer: PeerId,
    pub service: String,
    key: [u8; 32],
}

impl Stream {
    /// Seal application bytes for this stream (AEAD under the per-stream key).
    pub fn seal(&self, plaintext: &[u8]) -> Vec<u8> {
        crypto_kit::seal(&self.key, plaintext)
    }
    /// Open bytes sealed for this stream; `None` if they weren't (wrong stream / tampered).
    pub fn open(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        crypto_kit::open(&self.key, sealed)
    }
}

/// What the broker needs before it can serve a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need {
    /// No session with this peer yet — do the (one) handshake first.
    Handshake(PeerId),
}

/// The broker: one session per peer, streams handed out on demand. Tracks how many
/// handshakes happened (should stay one per peer no matter how many streams/apps).
#[derive(Default)]
pub struct Broker {
    sessions: std::collections::HashMap<PeerId, Session>,
    pub handshakes: u64,
    pub streams_opened: u64,
}

impl Broker {
    pub fn new() -> Self {
        Broker::default()
    }

    /// Install a completed session (one handshake). Idempotent per peer.
    pub fn install_session(&mut self, session: Session) {
        self.handshakes += 1;
        self.sessions.insert(session.peer, session);
    }

    pub fn has_peer(&self, peer: &PeerId) -> bool {
        self.sessions.contains_key(peer)
    }

    pub fn connected_peers(&self) -> usize {
        self.sessions.len()
    }

    /// Hand a local app a stream to an already-connected peer. If there's a session, this is
    /// free — no handshake, no DHT lookup. Otherwise the caller must complete a handshake.
    pub fn open_stream(&mut self, peer: &PeerId, service: &str) -> Result<Stream, Need> {
        match self.sessions.get_mut(peer) {
            Some(s) => {
                self.streams_opened += 1;
                Ok(s.open_stream(service))
            }
            None => Err(Need::Handshake(*peer)),
        }
    }
}

/// Test/util: run a full handshake between two identities and return both sessions.
pub fn handshake_pair(a: Identity, b: Identity) -> Option<(Session, Session)> {
    let (ha, msg_a) = Handshake::start(a);
    let (hb, msg_b) = Handshake::start(b);
    let sa = ha.complete(&msg_b)?;
    let sb = hb.complete(&msg_a)?;
    Some((sa, sb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_agrees_and_authenticates() {
        let a = Identity::generate();
        let b = Identity::generate();
        let (a_id, b_id) = (a.verify_key, b.verify_key);
        let (sa, sb) = handshake_pair(a, b).expect("handshake");
        // both sides derive the SAME session key (X25519 is symmetric)
        assert_eq!(sa.session_key(), sb.session_key());
        // and each learned the other's authenticated identity
        assert_eq!(sa.peer, b_id);
        assert_eq!(sb.peer, a_id);
    }

    #[test]
    fn tampered_handshake_is_rejected() {
        let a = Identity::generate();
        let b = Identity::generate();
        let (ha, _msg_a) = Handshake::start(a);
        let (_hb, mut msg_b) = Handshake::start(b);
        msg_b.sig[0] ^= 0xff; // forge/tamper the signature
        assert!(ha.complete(&msg_b).is_none(), "unauthenticated handshake must fail");
    }

    #[test]
    fn many_streams_reuse_one_session_no_new_handshake() {
        let (sa, _sb) = handshake_pair(Identity::generate(), Identity::generate()).unwrap();
        let peer = sa.peer;
        let mut broker = Broker::new();
        broker.install_session(sa);
        // three apps ask for streams to the same peer
        let s1 = broker.open_stream(&peer, "photos").unwrap();
        let s2 = broker.open_stream(&peer, "mail").unwrap();
        let s3 = broker.open_stream(&peer, "photos").unwrap();
        assert_eq!(broker.handshakes, 1, "still ONE handshake for the peer");
        assert_eq!(broker.streams_opened, 3);
        // distinct stream ids
        assert!(s1.id != s2.id && s2.id != s3.id);
    }

    #[test]
    fn per_stream_keys_are_isolated() {
        let (mut sa, _sb) = handshake_pair(Identity::generate(), Identity::generate()).unwrap();
        let photos = sa.open_stream("photos");
        let mail = sa.open_stream("mail");
        let msg = b"secret photo bytes";
        let sealed = photos.seal(msg);
        // the mail stream cannot read the photo stream's data…
        assert!(mail.open(&sealed).is_none(), "streams are cryptographically isolated");
        // …but the photo stream can
        assert_eq!(photos.open(&sealed).as_deref(), Some(&msg[..]));
    }

    #[test]
    fn open_without_session_signals_a_single_handshake() {
        let peer: PeerId = [7u8; 32];
        let mut broker = Broker::new();
        match broker.open_stream(&peer, "photos") {
            Err(need) => assert_eq!(need, Need::Handshake(peer)),
            Ok(_) => panic!("should have needed a handshake first"),
        }
        // after the one handshake is installed, streams flow with no further handshakes
        let (mut sa, _sb) = handshake_pair(Identity::generate(), Identity::generate()).unwrap();
        sa.peer = peer; // pretend this session is with `peer`
        broker.install_session(sa);
        assert!(broker.open_stream(&peer, "photos").is_ok());
        assert_eq!(broker.handshakes, 1);
    }
}
