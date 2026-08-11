//! handshake — the SHAK key exchange from `AppController_connect`.
//!
//! [`crate::wire_v3`] carries snaps once two peers can talk. This is how they
//! start: a fixed-format frame sent on every new TCP connection, carrying the
//! sender's id and public key.
//!
//! ## Two versions, and the second exists for a reason worth stating
//!
//! ```text
//!   SHAK  magic(4) + idLen(2) + id(N) + x25519_pub(32)
//!   SHK2  magic(4) + version(1) + flags(1) + idLen(2) + id(N)
//!         + x25519_pub(32) + pq_len(2) + pq_payload(N)
//! ```
//!
//! v2 carries **hybrid post-quantum KEM material** alongside the classical
//! X25519 key. Hybrid rather than replacement: a PQ scheme that turns out
//! broken leaves X25519 holding the session, and X25519 broken later leaves the
//! PQ half holding it. Shipping only the new one would be trading a known risk
//! for an unknown one.
//!
//! ## The magic is the version, and that is why both are checked
//!
//! `SHAK` and `SHK2` are different four-byte magics, not a version field inside
//! one frame. A v1 peer reading a v2 frame sees an unknown magic and ignores
//! it — which is the correct outcome, because parsing it as v1 would read the
//! version and flags bytes as part of the id length and produce an id of
//! nonsense length.
//!
//! ## Retries are bounded and the schedule is stated
//!
//! *"Send at T=0, 2s, 4s, 6s then give up"* — four attempts. A handshake that
//! retried forever would keep a dead peer's slot warm and, on a phone, keep the
//! radio awake for it.

/// `"SHAK"` — the v1 handshake magic.
pub const MAGIC_V1: u32 = 0x5348_414B;
/// `"SHK2"` — v2, carrying post-quantum material.
pub const MAGIC_V2: u32 = 0x5348_4B32;

pub const SHAK_V2_VERSION: u8 = 0x02;
/// This peer offers a post-quantum KEM.
pub const FLAG_PQ_ENABLED: u8 = 0x01;
/// This peer is answering rather than initiating.
pub const FLAG_IS_RESPONDER: u8 = 0x02;

/// X25519 public key length.
pub const PUBKEY_LEN: usize = 32;

/// Attempts before giving up: T=0, 2s, 4s, 6s.
pub const MAX_SHAK_ATTEMPTS: u32 = 4;
/// Gap between attempts.
pub const SHAK_RETRY_MS: u64 = 2_000;

/// A parsed handshake.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Shak {
    pub version: u8,
    pub id: String,
    pub x25519_pub: Vec<u8>,
    /// Present only on v2 with the PQ flag set.
    pub pq_payload: Option<Vec<u8>>,
    pub is_responder: bool,
}

impl Shak {
    /// Did this peer offer post-quantum material?
    pub fn is_pq(&self) -> bool { self.pq_payload.is_some() }
}

/// Why a frame was not a handshake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShakError {
    /// Not a handshake at all — some other protocol on the same socket.
    NotShak,
    /// The right magic but the frame ends early.
    Truncated,
    /// A length field claims more than the frame holds.
    BadLength,
    /// An id that is not valid UTF-8, or empty.
    BadId,
}

fn u16_at(b: &[u8], i: usize) -> Option<usize> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize)
}

/// Parse a handshake frame.
///
/// Every length is checked against the remaining bytes before it is used. A
/// handshake arrives from an unauthenticated peer — it is the FIRST thing a
/// stranger sends — so a length field that indexes past the buffer is the most
/// obvious attack surface in the protocol.
pub fn parse(buf: &[u8]) -> Result<Shak, ShakError> {
    if buf.len() < 4 { return Err(ShakError::NotShak); }
    let magic = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    match magic {
        MAGIC_V1 => parse_v1(buf),
        MAGIC_V2 => parse_v2(buf),
        _ => Err(ShakError::NotShak),
    }
}

fn parse_v1(buf: &[u8]) -> Result<Shak, ShakError> {
    let id_len = u16_at(buf, 4).ok_or(ShakError::Truncated)?;
    let id_end = 6usize.checked_add(id_len).ok_or(ShakError::BadLength)?;
    let key_end = id_end.checked_add(PUBKEY_LEN).ok_or(ShakError::BadLength)?;
    if buf.len() < key_end { return Err(ShakError::BadLength); }
    let id = std::str::from_utf8(&buf[6..id_end]).map_err(|_| ShakError::BadId)?;
    if id.is_empty() { return Err(ShakError::BadId); }
    Ok(Shak {
        version: 1, id: id.to_string(),
        x25519_pub: buf[id_end..key_end].to_vec(),
        pq_payload: None, is_responder: false,
    })
}

fn parse_v2(buf: &[u8]) -> Result<Shak, ShakError> {
    if buf.len() < 8 { return Err(ShakError::Truncated); }
    let version = buf[4];
    let flags = buf[5];
    let id_len = u16_at(buf, 6).ok_or(ShakError::Truncated)?;
    let id_end = 8usize.checked_add(id_len).ok_or(ShakError::BadLength)?;
    let key_end = id_end.checked_add(PUBKEY_LEN).ok_or(ShakError::BadLength)?;
    let pq_len_end = key_end.checked_add(2).ok_or(ShakError::BadLength)?;
    if buf.len() < pq_len_end { return Err(ShakError::BadLength); }
    let id = std::str::from_utf8(&buf[8..id_end]).map_err(|_| ShakError::BadId)?;
    if id.is_empty() { return Err(ShakError::BadId); }
    let pq_len = u16_at(buf, key_end).ok_or(ShakError::Truncated)?;
    let pq_end = pq_len_end.checked_add(pq_len).ok_or(ShakError::BadLength)?;
    if buf.len() < pq_end { return Err(ShakError::BadLength); }

    // The flag and the payload must agree. A PQ-enabled frame with no payload
    // would have the responder derive a hybrid secret from nothing, and a
    // payload with the flag clear is material nobody will use.
    let pq_payload = if flags & FLAG_PQ_ENABLED != 0 {
        if pq_len == 0 { return Err(ShakError::BadLength); }
        Some(buf[pq_len_end..pq_end].to_vec())
    } else {
        if pq_len != 0 { return Err(ShakError::BadLength); }
        None
    };
    Ok(Shak {
        version, id: id.to_string(),
        x25519_pub: buf[id_end..key_end].to_vec(),
        pq_payload,
        is_responder: flags & FLAG_IS_RESPONDER != 0,
    })
}

/// Build a v2 frame.
pub fn encode_v2(id: &str, x25519_pub: &[u8], pq: Option<&[u8]>, responder: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + id.len());
    out.extend_from_slice(&MAGIC_V2.to_be_bytes());
    out.push(SHAK_V2_VERSION);
    let mut flags = 0u8;
    if pq.is_some() { flags |= FLAG_PQ_ENABLED; }
    if responder { flags |= FLAG_IS_RESPONDER; }
    out.push(flags);
    out.extend_from_slice(&(id.len() as u16).to_be_bytes());
    out.extend_from_slice(id.as_bytes());
    out.extend_from_slice(x25519_pub);
    let pq_bytes = pq.unwrap_or(&[]);
    out.extend_from_slice(&(pq_bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(pq_bytes);
    out
}

/// Which handshake to send to a peer.
///
/// v2 when both sides can, v1 otherwise. Downgrade rather than refuse: an older
/// peer is not an attacker, and a network that splits on an upgrade is worse
/// than one running mixed versions for a while.
pub fn choose_version(we_support_pq: bool, peer_supports_v2: bool) -> u32 {
    if we_support_pq && peer_supports_v2 { MAGIC_V2 } else { MAGIC_V1 }
}

/// When the next retry is due, or `None` once the cap is reached.
///
/// Bounded because a handshake retried forever keeps a dead peer's slot warm
/// and, on a phone, keeps the radio awake for it.
pub fn next_retry_ms(attempt: u32) -> Option<u64> {
    if attempt >= MAX_SHAK_ATTEMPTS { return None; }
    Some(attempt as u64 * SHAK_RETRY_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Vec<u8> { (0..32u8).collect() }

    #[test]
    fn a_v2_frame_round_trips_with_its_pq_material() {
        let pq: Vec<u8> = vec![9; 1184];
        let buf = encode_v2("sc-alice", &key(), Some(&pq), false);
        let s = parse(&buf).expect("parses");
        assert_eq!(s.id, "sc-alice");
        assert_eq!(s.x25519_pub, key());
        assert_eq!(s.pq_payload.as_deref(), Some(&pq[..]));
        assert!(s.is_pq());
    }

    #[test]
    fn hybrid_means_the_classical_key_is_always_present() {
        // A PQ scheme that turns out broken leaves X25519 holding the session,
        // and vice versa. Shipping only the new one trades a known risk for an
        // unknown one.
        let buf = encode_v2("sc-a", &key(), Some(&[1, 2, 3]), false);
        let s = parse(&buf).unwrap();
        assert_eq!(s.x25519_pub.len(), PUBKEY_LEN, "PQ replaced the classical key");
    }

    #[test]
    fn the_responder_flag_round_trips() {
        // Both sides sending as initiator would derive two different session
        // keys and neither would notice until the first decrypt failed.
        let buf = encode_v2("sc-a", &key(), None, true);
        assert!(parse(&buf).unwrap().is_responder);
        let buf = encode_v2("sc-a", &key(), None, false);
        assert!(!parse(&buf).unwrap().is_responder);
    }

    #[test]
    fn a_v1_peer_ignores_a_v2_frame_rather_than_misparsing_it() {
        // Different magics, not a version field inside one frame. Parsing a v2
        // frame as v1 would read the version and flags bytes as part of the id
        // LENGTH and produce an id of nonsense length.
        let v2 = encode_v2("sc-alice", &key(), None, false);
        assert_ne!(u32::from_be_bytes([v2[0], v2[1], v2[2], v2[3]]), MAGIC_V1);
        // A parser that only knew v1 sees an unknown magic:
        let mut wrong = v2.clone();
        wrong[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
        assert_eq!(parse(&wrong), Err(ShakError::NotShak));
    }

    #[test]
    fn some_other_protocol_on_the_socket_is_not_shak() {
        // Payloads are multiplexed; consuming somebody else's frame breaks
        // their feature with no error in either subsystem.
        assert_eq!(parse(b"SNAPmediablob"), Err(ShakError::NotShak));
        assert_eq!(parse(b""), Err(ShakError::NotShak));
        assert_eq!(parse(b"SH"), Err(ShakError::NotShak));
    }

    #[test]
    fn a_length_that_indexes_past_the_buffer_is_refused() {
        // A handshake is the FIRST thing a stranger sends, so a length field
        // pointing off the end is the most obvious attack surface in the
        // protocol.
        let mut buf = encode_v2("sc-alice", &key(), None, false);
        buf[6..8].copy_from_slice(&60_000u16.to_be_bytes());
        assert_eq!(parse(&buf), Err(ShakError::BadLength));
    }

    #[test]
    fn a_pq_length_past_the_buffer_is_refused_too() {
        let mut buf = encode_v2("sc-a", &key(), Some(&[1, 2, 3]), false);
        let at = buf.len() - 3 - 2;
        buf[at..at + 2].copy_from_slice(&50_000u16.to_be_bytes());
        assert_eq!(parse(&buf), Err(ShakError::BadLength));
    }

    #[test]
    fn a_truncated_frame_is_refused_rather_than_read_short() {
        // TCP delivers partial frames routinely; reading a half-arrived key as
        // a whole one would derive a session from padding.
        let full = encode_v2("sc-alice", &key(), None, false);
        for cut in 4..full.len() {
            let r = parse(&full[..cut]);
            assert!(r.is_err(), "accepted a frame cut at {cut}");
        }
        assert!(parse(&full).is_ok());
    }

    #[test]
    fn the_pq_flag_and_the_payload_must_agree() {
        // A PQ-enabled frame with no payload has the responder derive a hybrid
        // secret from nothing; a payload with the flag clear is material nobody
        // will use.
        let mut buf = encode_v2("sc-a", &key(), None, false);
        buf[5] |= FLAG_PQ_ENABLED;
        assert_eq!(parse(&buf), Err(ShakError::BadLength));
    }

    #[test]
    fn an_empty_or_non_utf8_id_is_refused() {
        // The id reaches the routing table and the UI; an empty one would match
        // a peer nobody named.
        let buf = encode_v2("", &key(), None, false);
        assert_eq!(parse(&buf), Err(ShakError::BadId));
        let mut bad = encode_v2("ab", &key(), None, false);
        bad[8] = 0xFF;
        assert_eq!(parse(&bad), Err(ShakError::BadId));
    }

    #[test]
    fn version_choice_downgrades_rather_than_refusing() {
        // An older peer is not an attacker, and a network that splits on an
        // upgrade is worse than one running mixed versions for a while.
        assert_eq!(choose_version(true, true), MAGIC_V2);
        assert_eq!(choose_version(true, false), MAGIC_V1);
        assert_eq!(choose_version(false, true), MAGIC_V1);
    }

    #[test]
    fn retries_follow_the_documented_schedule_and_stop() {
        // "Send at T=0, 2s, 4s, 6s then give up." Retrying forever keeps a dead
        // peer's slot warm and, on a phone, keeps the radio awake for it.
        assert_eq!(next_retry_ms(0), Some(0));
        assert_eq!(next_retry_ms(1), Some(2_000));
        assert_eq!(next_retry_ms(2), Some(4_000));
        assert_eq!(next_retry_ms(3), Some(6_000));
        assert_eq!(next_retry_ms(MAX_SHAK_ATTEMPTS), None);
        assert_eq!(next_retry_ms(99), None);
    }

    #[test]
    fn the_magics_match_the_c_constants() {
        // Two implementations of one handshake: a wrong magic is a peer that
        // never connects, with no error on either side.
        assert_eq!(MAGIC_V1, u32::from_be_bytes(*b"SHAK"));
        assert_eq!(MAGIC_V2, u32::from_be_bytes(*b"SHK2"));
    }

    #[test]
    fn a_long_id_is_carried_intact() {
        // sc-IDs are long, and a u16 length is plenty — but the encoder must
        // not silently truncate one.
        let id = "sc-".to_string() + &"a".repeat(300);
        let buf = encode_v2(&id, &key(), None, false);
        assert_eq!(parse(&buf).unwrap().id, id);
    }
}
