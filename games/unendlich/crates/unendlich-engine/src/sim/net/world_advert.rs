//! `sim::net::world_advert` — DHT world-advert wire codec (roadmap **item 1**, §A;
//! WID r334-coordinator).
//!
//! When a world is **Opened to DHT** (the `sim::world_management` host flow), the host
//! publishes a [`session_mode::WorldAdvert`] under its DHT key; browsers on the Join
//! screen observe the published bytes and decode them back into adverts for
//! `session_mode::ServerBrowser`. This module is the **wire (de)serialization** of that
//! advert — the bytes that actually sit in the DHT record:
//! `{ host_username, world_name, players, max_players, last_seen_ms, candidates[] }`,
//! each candidate `{ host:[u8;4], port, kind }` (mirrors `net_candidates::Candidate`).
//!
//! Self-contained over the DHT-record representation ([`AdvertWire`] / [`CandWire`]); the
//! `session_mode::WorldAdvert ⇄ AdvertWire` mapping is trivial field glue (the bin builds
//! an `AdvertWire` from the live `WorldAdvert` before publish, and back after observe —
//! see the handoff). The DHT put/get IO itself is bin glue.
//!
//! Pure: fixed little-endian layout mirroring `sim::net::wire` (magic `0x554E` "UN", v1);
//! strings/counts are length-capped so a hostile record can't blow allocation; UTF-8 is
//! validated; `decode_advert` never panics (returns `None` on any malformed input).

/// Datagram family ("UN"), same as `wire::PROTOCOL_MAGIC`.
pub const ADVERT_MAGIC: u16 = 0x554E;
pub const ADVERT_VERSION: u8 = 1;

/// Candidate kind tags — must match `net_candidates::CandKind` order (Lan, Public).
pub const KIND_LAN: u8 = 0;
pub const KIND_PUBLIC: u8 = 1;

/// Byte cap on the username / world-name strings (encode truncates on a char boundary,
/// decode rejects an over-cap declared length).
pub const MAX_NAME_LEN: usize = 64;
/// Cap on advertised endpoints (encode truncates, decode rejects over-cap).
pub const MAX_CANDIDATES: usize = 16;

/// One advertised endpoint (wire form of `net_candidates::Candidate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandWire {
    pub host: [u8; 4],
    pub port: u16,
    /// `KIND_LAN` or `KIND_PUBLIC`.
    pub kind: u8,
}

/// Wire form of `session_mode::WorldAdvert` (the DHT record payload).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdvertWire {
    pub host_username: String,
    pub world_name: String,
    pub players: u16,
    pub max_players: u16,
    pub last_seen_ms: u64,
    pub candidates: Vec<CandWire>,
}

impl AdvertWire {
    /// Mirrors `session_mode::WorldAdvert::is_full`.
    pub fn is_full(&self) -> bool {
        self.max_players > 0 && self.players >= self.max_players
    }
    /// Dedupe identity, mirrors `WorldAdvert::key`.
    pub fn key(&self) -> (&str, &str) {
        (self.host_username.as_str(), self.world_name.as_str())
    }
}

/// Largest char-boundary length `<= cap` for `s` (so truncation never splits a char).
fn char_floor(s: &str, cap: usize) -> usize {
    if s.len() <= cap {
        return s.len();
    }
    let mut n = cap;
    while n > 0 && !s.is_char_boundary(n) {
        n -= 1;
    }
    n
}

fn write_str(b: &mut Vec<u8>, s: &str) {
    let n = char_floor(s, MAX_NAME_LEN);
    b.push(n as u8); // n <= MAX_NAME_LEN (64) fits a u8
    b.extend_from_slice(&s.as_bytes()[..n]);
}

/// Encode an advert to its DHT-record bytes.
pub fn encode_advert(a: &AdvertWire) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&ADVERT_MAGIC.to_le_bytes());
    b.push(ADVERT_VERSION);
    write_str(&mut b, &a.host_username);
    write_str(&mut b, &a.world_name);
    b.extend_from_slice(&a.players.to_le_bytes());
    b.extend_from_slice(&a.max_players.to_le_bytes());
    b.extend_from_slice(&a.last_seen_ms.to_le_bytes());
    let n = a.candidates.len().min(MAX_CANDIDATES);
    b.push(n as u8);
    for c in &a.candidates[..n] {
        b.extend_from_slice(&c.host);
        b.extend_from_slice(&c.port.to_le_bytes());
        b.push(c.kind);
    }
    b
}

/// Decode an advert. `None` on bad magic/version, truncation, an over-cap string/candidate
/// length, invalid UTF-8, or an unknown candidate kind tag. Never panics.
pub fn decode_advert(buf: &[u8]) -> Option<AdvertWire> {
    let mut r = Reader::new(buf);
    if r.u16()? != ADVERT_MAGIC {
        return None;
    }
    if r.u8()? != ADVERT_VERSION {
        return None;
    }
    let host_username = r.string()?;
    let world_name = r.string()?;
    let players = r.u16()?;
    let max_players = r.u16()?;
    let last_seen_ms = r.u64()?;
    let n = r.u8()? as usize;
    if n > MAX_CANDIDATES {
        return None;
    }
    let mut candidates = Vec::with_capacity(n);
    for _ in 0..n {
        let host = [r.u8()?, r.u8()?, r.u8()?, r.u8()?];
        let port = r.u16()?;
        let kind = r.u8()?;
        if kind != KIND_LAN && kind != KIND_PUBLIC {
            return None; // unknown transport kind
        }
        candidates.push(CandWire { host, port, kind });
    }
    Some(AdvertWire {
        host_username,
        world_name,
        players,
        max_players,
        last_seen_ms,
        candidates,
    })
}

// ── bounded reader (mirrors wire::Reader; never panics) ─────────────────────

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        let s = self.take(2)?;
        Some(u16::from_le_bytes([s[0], s[1]]))
    }
    fn u64(&mut self) -> Option<u64> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Some(u64::from_le_bytes(a))
    }
    /// A u8-length-prefixed UTF-8 string, length-capped.
    fn string(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        if n > MAX_NAME_LEN {
            return None;
        }
        let s = self.take(n)?;
        core::str::from_utf8(s).ok().map(|x| x.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AdvertWire {
        AdvertWire {
            host_username: "dragonlord".to_string(),
            world_name: "Skyhold".to_string(),
            players: 3,
            max_players: 8,
            last_seen_ms: 1_700_000_000_123,
            candidates: vec![
                CandWire { host: [192, 168, 1, 7], port: 47000, kind: KIND_LAN },
                CandWire { host: [203, 0, 113, 9], port: 47001, kind: KIND_PUBLIC },
            ],
        }
    }

    #[test]
    fn round_trips_full_advert() {
        let a = sample();
        assert_eq!(decode_advert(&encode_advert(&a)).as_ref(), Some(&a));
    }

    #[test]
    fn round_trips_no_candidates() {
        let mut a = sample();
        a.candidates.clear();
        assert_eq!(decode_advert(&encode_advert(&a)).as_ref(), Some(&a));
    }

    #[test]
    fn decode_rejects_bad_magic_and_version() {
        let good = encode_advert(&sample());
        let mut bad = good.clone();
        bad[0] ^= 0xFF;
        assert!(decode_advert(&bad).is_none());
        let mut bad = good.clone();
        bad[2] = 9;
        assert!(decode_advert(&bad).is_none());
    }

    #[test]
    fn decode_rejects_truncation() {
        let good = encode_advert(&sample());
        assert!(decode_advert(&good[..good.len() - 1]).is_none());
        assert!(decode_advert(&[]).is_none());
        assert!(decode_advert(&[0x4E]).is_none());
    }

    #[test]
    fn long_names_truncated_on_char_boundary() {
        let mut a = sample();
        a.host_username = "x".repeat(200); // > MAX_NAME_LEN
        let dec = decode_advert(&encode_advert(&a)).expect("decodes");
        assert_eq!(dec.host_username.len(), MAX_NAME_LEN);
        // multibyte: ensure no split char (é = 2 bytes); 33×é = 66 bytes > 64 → floor to 64 (32 chars)
        let mut a = sample();
        a.world_name = "é".repeat(33);
        let dec = decode_advert(&encode_advert(&a)).expect("decodes");
        assert!(dec.world_name.len() <= MAX_NAME_LEN);
        assert_eq!(dec.world_name.chars().count(), 32, "floored to whole chars");
    }

    #[test]
    fn decode_rejects_over_cap_declared_string_len() {
        // hand-craft a record claiming a 200-byte name
        let mut b = Vec::new();
        b.extend_from_slice(&ADVERT_MAGIC.to_le_bytes());
        b.push(ADVERT_VERSION);
        b.push(200); // > MAX_NAME_LEN
        b.extend_from_slice(&[b'x'; 200]);
        assert!(decode_advert(&b).is_none());
    }

    #[test]
    fn candidate_count_capped_and_over_cap_rejected() {
        let mut a = sample();
        a.candidates = (0..40)
            .map(|i| CandWire { host: [10, 0, 0, i as u8], port: 1, kind: KIND_LAN })
            .collect();
        let dec = decode_advert(&encode_advert(&a)).expect("decodes");
        assert_eq!(dec.candidates.len(), MAX_CANDIDATES, "encode capped the list");
        // a hand-built record claiming 99 candidates is refused
        let mut b = encode_advert(&sample());
        // locate the candidate-count byte: 3 (hdr) + (1+u)+(1+w) + 2+2+8 ; just rebuild minimally
        let mut hb = Vec::new();
        hb.extend_from_slice(&ADVERT_MAGIC.to_le_bytes());
        hb.push(ADVERT_VERSION);
        hb.push(0);
        hb.push(0);
        hb.extend_from_slice(&0u16.to_le_bytes());
        hb.extend_from_slice(&0u16.to_le_bytes());
        hb.extend_from_slice(&0u64.to_le_bytes());
        hb.push(99); // > MAX_CANDIDATES
        assert!(decode_advert(&hb).is_none());
        let _ = &mut b;
    }

    #[test]
    fn unknown_candidate_kind_rejected() {
        let mut a = sample();
        a.candidates[0].kind = 7; // not Lan/Public
        assert!(decode_advert(&encode_advert(&a)).is_none());
    }

    #[test]
    fn is_full_mirrors_session_semantics() {
        let mut a = sample();
        a.players = 8;
        a.max_players = 8;
        assert!(a.is_full());
        a.max_players = 0; // unlimited
        assert!(!a.is_full());
    }

    #[test]
    fn deterministic() {
        let a = sample();
        assert_eq!(encode_advert(&a), encode_advert(&a));
    }
}
