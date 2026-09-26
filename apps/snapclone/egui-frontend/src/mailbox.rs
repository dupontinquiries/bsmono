//! mailbox — connection requests left in the DHT for a contact who is offline.
//!
//! ## What the Qt build does, and why this matches it byte for byte
//!
//! Without a mailbox two people can only connect while both are online at the
//! same moment: a lookup that finds nobody ends there. The Qt build
//! (`JamiAdapter_dht_impl.cpp`, "Two-step DHT connection requests") leaves a
//! request under the TARGET's mailbox key, and the target picks it up on its next
//! mailbox poll and connects back:
//!
//! ```text
//! key        sha1(target_sc_id + "/connreq")
//! value      sender_sc_id "\n" nonce(12) ciphertext+tag      (≤ 2048 bytes)
//! plaintext  "CONNREQ\n" from "\n" host:port "\n" punch_host:punch_port "\n" ts "\n" [lan_host:port "\n"]
//! cipher     AES-256-GCM under HKDF-SHA256(X25519(ours, theirs), "snapclone-v1-session")
//! lifetime   non-permanent (10 min); requester republishes every 8 min
//! poll       every 90 s (Timings::kMailboxCheckMs)
//! ```
//!
//! The key derivation and AEAD are `crypto_kit`'s, which was lifted from
//! `snapclone-crypto` -- the same `derive_session_key` label and the same
//! `seal` layout (nonce ‖ ciphertext ‖ tag) the Qt build's FFI calls. So a
//! request sealed here opens there and the reverse, given each side has the
//! other's X25519 key.
//!
//! ## Where the peer's X25519 key comes from
//!
//! The Qt build publishes its X25519 key, base64, as a fourth field on the first
//! address line of its presence record, and trusts it on first use. This build
//! does the same (so the two interoperate) and adds a binding the Qt build does
//! not have: a `#xsig:` line carrying an Ed25519 signature over the X25519 key,
//! made with the identity key whose fingerprint IS the sc-ID. A peer whose
//! Ed25519 key is pinned (from signed LAN gossip) therefore has its mailbox key
//! AUTHENTICATED rather than trusted: a record planted under their key by
//! someone else cannot carry a valid signature. See [`accept_peer_key`].
//!
//! This module is pure -- no socket, no clock, no file. `Snapclone::tick_mailbox`
//! performs what it decides.

/// The mailbox key's string, before hashing (the DHT layer SHA-1s it exactly as
/// it does an sc-ID: `presence::key_for_scid`).
pub fn mailbox_key(sc_id: &str) -> String {
    format!("{sc_id}/connreq")
}

/// Is this DHT id a mailbox rather than a contact? The node reports every
/// lookup under the string it was asked, so the mailbox's answers arrive on the
/// same event as presence lookups.
pub fn is_mailbox_key(id: &str) -> bool {
    id.ends_with("/connreq")
}

/// The Qt build's receive cap (V32.34+). A larger value is dropped, not parsed.
pub const MAX_BLOB: usize = 2048;

/// A request older than this is ignored: it matches the value lifetime, and a
/// request that old describes an address the sender may no longer have.
pub const MAX_AGE_SECS: i64 = 600;

/// How often to look in our own mailbox (`Timings::kMailboxCheckMs`).
pub const CHECK_EVERY_MS: u64 = 90_000;

/// How often a pending request is republished while the target is unreachable
/// (the Qt build's 8 minutes, under the 10-minute value lifetime).
pub const REPUBLISH_MS: u64 = 8 * 60 * 1000;

/// How long we keep leaving requests for someone who never answers. The Qt
/// build retries "while peer is unreachable"; an unbounded retry is a beacon
/// that outlives the user's interest, so this stops after a day.
pub const GIVE_UP_MS: u64 = 24 * 60 * 60 * 1000;

/// The session label both builds use.
pub const SESSION_INFO: &[u8] = b"snapclone-v1-session";

/// One decoded request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnReq {
    pub from: String,
    /// Where to open a WebSocket/TCP connection.
    pub tcp: (String, u16),
    /// Where to aim a UDP hole punch (the sender's STUN-reflexive address).
    pub punch: (String, u16),
    /// Epoch seconds when it was made.
    pub ts: i64,
    /// The sender's LAN address, so a same-Wi-Fi peer does not hairpin.
    pub lan: Option<(String, u16)>,
}

/// The Qt plaintext, including its trailing newline.
pub fn render(r: &ConnReq) -> String {
    let mut s = format!("CONNREQ\n{}\n{}:{}\n{}:{}\n{}\n",
                        r.from, r.tcp.0, r.tcp.1, r.punch.0, r.punch.1, r.ts);
    if let Some((h, p)) = &r.lan {
        s.push_str(&format!("{h}:{p}\n"));
    }
    s
}

/// `host:port`, splitting at the LAST colon as the Qt parser does.
fn host_port(s: &str) -> Option<(String, u16)> {
    let (h, p) = s.trim().rsplit_once(':')?;
    let port: u16 = p.parse().ok()?;
    if h.is_empty() || port == 0 {
        return None;
    }
    Some((h.to_string(), port))
}

/// Parse a CONNREQ plaintext. `None` for anything else -- the mailbox also
/// carries other kinds (`PEERREFL` reports), which are not requests.
pub fn parse(text: &str) -> Option<ConnReq> {
    let f: Vec<&str> = text.split('\n').filter(|l| !l.trim().is_empty()).collect();
    if f.len() < 5 || f[0].trim() != "CONNREQ" {
        return None;
    }
    let from = f[1].trim().to_string();
    if from.is_empty() {
        return None;
    }
    Some(ConnReq {
        from,
        tcp: host_port(f[2])?,
        punch: host_port(f[3])?,
        ts: f[4].trim().parse().ok()?,
        lan: f.get(5).and_then(|s| host_port(s)),
    })
}

/// The shared session key with a peer: X25519, then HKDF under [`SESSION_INFO`].
pub fn session_key(our_x_secret: &[u8; 32], their_x_public: &[u8; 32]) -> [u8; 32] {
    let shared = crypto_kit::x25519_shared(our_x_secret, their_x_public);
    *crypto_kit::derive_session_key(shared.as_ref(), SESSION_INFO)
}

/// `sender "\n" seal(key, plaintext)` -- the Qt blob layout.
pub fn seal_blob(sender: &str, key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(sender.len() + 1 + 12 + plaintext.len() + 16);
    out.extend_from_slice(sender.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(&crypto_kit::seal(key, plaintext));
    out
}

/// The sender a blob claims, and its sealed part. Claimed, not proven: only a
/// successful [`open_blob`] under that sender's key proves it.
pub fn blob_sender(blob: &[u8]) -> Option<(&str, &[u8])> {
    if blob.len() > MAX_BLOB {
        return None;
    }
    let nl = blob.iter().position(|b| *b == b'\n')?;
    let sender = std::str::from_utf8(&blob[..nl]).ok()?;
    if sender.is_empty() || blob.len() < nl + 1 + 12 + 16 {
        return None;
    }
    Some((sender, &blob[nl + 1..]))
}

/// Open a blob with the key for its claimed sender.
pub fn open_blob(blob: &[u8], key: &[u8; 32]) -> Option<Vec<u8>> {
    let (_, sealed) = blob_sender(blob)?;
    crypto_kit::open(key, sealed)
}

/// The nonce, for replay protection (the Qt build keeps a seen-set of these).
pub fn blob_nonce(blob: &[u8]) -> Option<[u8; 12]> {
    let (_, sealed) = blob_sender(blob)?;
    sealed.get(..12)?.try_into().ok()
}

/// What to make of an X25519 key a peer's DHT record advertised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerKey {
    /// Signed by the peer's pinned Ed25519 key: authenticated.
    Verified,
    /// No pin to check against (a Qt peer, or one never seen on the LAN):
    /// trusted on first use, as the Qt build trusts it.
    Unverified,
    /// A pin exists and the signature is missing or wrong: refused. A record
    /// planted under the peer's key by someone else looks exactly like this.
    Refused,
}

/// The message an `#xsig:` line signs: a domain label and the key, so the
/// signature cannot be replayed as a signature over anything else.
pub fn xsig_message(x_public: &[u8; 32]) -> Vec<u8> {
    let mut m = b"snapclone-mailbox-x25519:".to_vec();
    m.extend_from_slice(x_public);
    m
}

/// Judge an advertised key against what we know of the peer.
pub fn accept_peer_key(x_public: &[u8; 32], xsig: Option<&[u8; 64]>, pinned_ed: Option<&[u8; 32]>) -> PeerKey {
    match pinned_ed {
        None => PeerKey::Unverified,
        Some(ed) => match xsig {
            Some(sig) if crypto_kit::verify(ed, &xsig_message(x_public), sig) => PeerKey::Verified,
            _ => PeerKey::Refused,
        },
    }
}

// ── base64 (standard alphabet, padded) -- the Qt build's `toBase64` ────────────

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 4 != 0 {
        return None;
    }
    let val = |c: u8| -> Option<u32> { B64.iter().position(|x| *x == c).map(|p| p as u32) };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for chunk in s.as_bytes().chunks(4) {
        let pad = chunk.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            let v = if i >= 4 - pad { 0 } else { val(*c)? };
            n = (n << 6) | v;
        }
        out.push((n >> 16) as u8);
        if pad < 2 { out.push((n >> 8) as u8); }
        if pad < 1 { out.push(n as u8); }
    }
    Some(out)
}

/// A 32-byte key from base64, or `None` for anything else.
pub fn b64_key(s: &str) -> Option<[u8; 32]> {
    b64_decode(s)?.try_into().ok()
}

// ── the requester's side: whom to leave a request for, and when ──────────────

/// Per-target republish state.
#[derive(Debug, Clone, Default)]
pub struct Outbox {
    /// (target, first left at ms, last left at ms)
    pending: Vec<(String, u64, u64)>,
}

impl Outbox {
    /// A lookup for `target` found nobody: leave a request now if one is due.
    /// Returns whether to publish.
    pub fn unreachable(&mut self, target: &str, now_ms: u64) -> bool {
        match self.pending.iter_mut().find(|(t, _, _)| t == target) {
            None => {
                self.pending.push((target.to_string(), now_ms, now_ms));
                true
            }
            Some((_, first, last)) => {
                if now_ms.saturating_sub(*first) >= GIVE_UP_MS {
                    return false;
                }
                if now_ms.saturating_sub(*last) >= REPUBLISH_MS {
                    *last = now_ms;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// The target answered (found on the DHT, seen on the LAN, connected):
    /// stop leaving requests.
    pub fn reached(&mut self, target: &str) {
        self.pending.retain(|(t, _, _)| t != target);
    }

    pub fn is_pending(&self, target: &str) -> bool {
        self.pending.iter().any(|(t, _, _)| t == target)
    }
}

/// Replay protection for received requests: nonces already acted on. Bounded,
/// FIFO, like the Qt build's 500-entry set; only nonces of blobs that OPENED are
/// recorded, so garbage cannot evict real entries.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    order: std::collections::VecDeque<[u8; 12]>,
}

impl Seen {
    pub const CAP: usize = 500;

    /// `true` the first time a nonce is offered.
    pub fn first(&mut self, nonce: [u8; 12]) -> bool {
        if self.order.contains(&nonce) {
            return false;
        }
        if self.order.len() >= Self::CAP {
            self.order.pop_front();
        }
        self.order.push_back(nonce);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> ConnReq {
        ConnReq {
            from: "sc-aabbccdd".into(),
            tcp: ("203.0.113.7".into(), 9876),
            punch: ("203.0.113.7".into(), 40123),
            ts: 1_790_000_000,
            lan: Some(("192.168.1.20".into(), 9876)),
        }
    }

    /// The Qt plaintext, exactly -- field order, colons, trailing newlines.
    #[test]
    fn the_plaintext_is_the_qt_builds() {
        assert_eq!(render(&req()),
            "CONNREQ\nsc-aabbccdd\n203.0.113.7:9876\n203.0.113.7:40123\n1790000000\n192.168.1.20:9876\n");
        assert_eq!(parse(&render(&req())), Some(req()));
        let mut five = req();
        five.lan = None;
        assert_eq!(parse("CONNREQ\nsc-aabbccdd\n203.0.113.7:9876\n203.0.113.7:40123\n1790000000\n"), Some(five));
        // IPv6 host: split at the LAST colon, as the Qt `lastIndexOf(':')` does.
        let v6 = parse("CONNREQ\nsc-x\n2001:db8::1:9876\n2001:db8::1:9876\n1\n").unwrap();
        assert_eq!(v6.tcp, ("2001:db8::1".into(), 9876));
        assert!(parse("PEERREFL\nsc-x\n1.2.3.4:5\n1\n").is_none(), "another mailbox kind is not a request");
        assert!(parse("CONNREQ\nsc-x\nnoport\n1.2.3.4:5\n1\n").is_none());
    }

    /// Two installations derive the same key from opposite halves, a sealed blob
    /// opens with it, and not with anybody else's.
    #[test]
    fn a_sealed_request_opens_only_for_its_target() {
        let (a_sec, a_pub) = crypto_kit::x25519_keypair();
        let (b_sec, b_pub) = crypto_kit::x25519_keypair();
        let (c_sec, _) = crypto_kit::x25519_keypair();
        let k_ab = session_key(&a_sec, &b_pub);
        assert_eq!(k_ab, session_key(&b_sec, &a_pub), "the two sides disagree on the key");
        let blob = seal_blob("sc-aaaaaaaa", &k_ab, render(&req()).as_bytes());
        assert_eq!(blob_sender(&blob).unwrap().0, "sc-aaaaaaaa");
        let opened = open_blob(&blob, &session_key(&b_sec, &a_pub)).expect("the target opens it");
        assert_eq!(parse(std::str::from_utf8(&opened).unwrap()), Some(req()));
        assert!(open_blob(&blob, &session_key(&c_sec, &a_pub)).is_none(), "a third party opened it");
        // Layout: sender, newline, 12-byte nonce, ciphertext + 16-byte tag.
        let pt = render(&req()).len();
        assert_eq!(blob.len(), "sc-aaaaaaaa".len() + 1 + 12 + pt + 16);
        assert!(blob_nonce(&blob).is_some());
        // Oversize and truncated blobs are refused before any crypto.
        assert!(blob_sender(&vec![b'a'; MAX_BLOB + 1]).is_none());
        assert!(blob_sender(b"sc-x\nshort").is_none());
    }

    /// The Qt build's `toBase64`: RFC 4648 vectors, and a 32-byte key.
    #[test]
    fn base64_matches_rfc4648() {
        for (plain, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"),
                             ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(b64_encode(plain.as_bytes()), enc);
            assert_eq!(b64_decode(enc).unwrap(), plain.as_bytes());
        }
        let key = [0xabu8; 32];
        assert_eq!(b64_key(&b64_encode(&key)), Some(key));
        assert!(b64_key("Zm9v").is_none(), "a 3-byte value is not a key");
        assert!(b64_decode("Zm9").is_none() && b64_decode("Z!9v").is_none());
    }

    /// A pinned peer's advertised key must be signed by the pinned identity;
    /// an unpinned one is trusted on first use, as the Qt build does.
    #[test]
    fn an_advertised_key_is_checked_against_the_pin() {
        let (ed_sec, ed_pub) = crypto_kit::ed25519_keypair();
        let (_, x_pub) = crypto_kit::x25519_keypair();
        let sig = crypto_kit::sign(&ed_sec, &xsig_message(&x_pub));
        assert_eq!(accept_peer_key(&x_pub, Some(&sig), Some(&ed_pub)), PeerKey::Verified);
        assert_eq!(accept_peer_key(&x_pub, None, None), PeerKey::Unverified);
        assert_eq!(accept_peer_key(&x_pub, None, Some(&ed_pub)), PeerKey::Refused, "a pinned peer with no signature");
        let (_, planted) = crypto_kit::x25519_keypair();
        assert_eq!(accept_peer_key(&planted, Some(&sig), Some(&ed_pub)), PeerKey::Refused, "a signature over another key");
        // The domain label keeps a raw signature over the key bytes from counting.
        let raw = crypto_kit::sign(&ed_sec, &x_pub);
        assert_eq!(accept_peer_key(&x_pub, Some(&raw), Some(&ed_pub)), PeerKey::Refused);
    }

    #[test]
    fn the_outbox_republishes_on_the_qt_cadence_and_gives_up() {
        let mut o = Outbox::default();
        assert!(o.unreachable("sc-b", 0), "the first miss leaves a request");
        assert!(!o.unreachable("sc-b", 60_000), "not again a minute later");
        assert!(o.unreachable("sc-b", REPUBLISH_MS), "again before the 10-minute expiry");
        assert!(REPUBLISH_MS < 10 * 60 * 1000);
        assert!(!o.unreachable("sc-b", GIVE_UP_MS + 1), "stops after a day");
        o.reached("sc-b");
        assert!(!o.is_pending("sc-b"));
        assert!(o.unreachable("sc-b", GIVE_UP_MS + 2), "a new episode starts fresh");
    }

    #[test]
    fn a_nonce_is_acted_on_once_and_the_set_is_bounded() {
        let mut s = Seen::default();
        assert!(s.first([1; 12]));
        assert!(!s.first([1; 12]), "a replay was accepted");
        for i in 0..Seen::CAP as u32 {
            let mut n = [0u8; 12];
            n[..4].copy_from_slice(&i.to_le_bytes());
            n[11] = 9;
            s.first(n);
        }
        assert!(s.order.len() <= Seen::CAP);
    }
}
