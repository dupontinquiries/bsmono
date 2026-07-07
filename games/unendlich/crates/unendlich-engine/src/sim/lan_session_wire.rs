//! `sim::lan_session_wire` — the unicast JOIN handshake over the LAN session socket.
//!
//! After discovery (the multicast beacon in `lan_discovery`), a joiner has the
//! host's advertised candidate (`ip:port`). On a LAN the peers are directly
//! reachable — no NAT, no DHT — so the admission handshake is a tiny direct
//! request/reply instead of the mailbox + hole-punch dance the internet path uses
//! (`dht_mailbox` + `net_rendezvous`):
//!
//! ```text
//! joiner --- Join{name} ----------------> host
//! joiner <-- Accept{world_name, seed} --- host     (joiner adopts the host's seed)
//! joiner <-- Deny{reason} --------------- host     (rejected)
//! ```
//!
//! `Accept` carries the host's `world_seed` so the joiner rebuilds the same world
//! (the r346 seed-coherence rule). Versioned, length-prefixed, and defensive:
//! anything malformed parses to `None` (a stray datagram never panics the loop).

const MAGIC: &[u8; 4] = b"ULJ1";
const TAG_JOIN: u8 = 1;
const TAG_ACCEPT: u8 = 2;
const TAG_DENY: u8 = 3;

/// Cap on any embedded string, so a hostile datagram can't make us allocate.
const MAX_STR: usize = 256;

/// A message in the LAN join handshake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LanJoin {
    /// Joiner → host: "let me in, I'm `name`".
    Join { name: String },
    /// Host → joiner: admitted; rebuild `world_name` at `world_seed`.
    Accept { world_name: String, world_seed: u64 },
    /// Host → joiner: rejected.
    Deny { reason: String },
}

fn put_str(o: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    let n = b.len().min(MAX_STR);
    o.extend_from_slice(&(n as u16).to_le_bytes());
    o.extend_from_slice(&b[..n]);
}

fn get_str(b: &[u8], i: &mut usize) -> Option<String> {
    if *i + 2 > b.len() {
        return None;
    }
    let n = u16::from_le_bytes([b[*i], b[*i + 1]]) as usize;
    *i += 2;
    if n > MAX_STR {
        return None;
    }
    let s = b.get(*i..*i + n)?;
    *i += n;
    String::from_utf8(s.to_vec()).ok()
}

fn get_u64(b: &[u8], i: &mut usize) -> Option<u64> {
    let s = b.get(*i..*i + 8)?;
    *i += 8;
    Some(u64::from_le_bytes(s.try_into().ok()?))
}

impl LanJoin {
    /// Serialize to a single datagram.
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(16);
        o.extend_from_slice(MAGIC);
        match self {
            LanJoin::Join { name } => {
                o.push(TAG_JOIN);
                put_str(&mut o, name);
            }
            LanJoin::Accept { world_name, world_seed } => {
                o.push(TAG_ACCEPT);
                put_str(&mut o, world_name);
                o.extend_from_slice(&world_seed.to_le_bytes());
            }
            LanJoin::Deny { reason } => {
                o.push(TAG_DENY);
                put_str(&mut o, reason);
            }
        }
        o
    }

    /// Parse one datagram. `None` on bad magic, unknown tag, truncation, or
    /// non-UTF-8 — never panics.
    pub fn parse(data: &[u8]) -> Option<LanJoin> {
        if data.len() < 5 || &data[..4] != MAGIC {
            return None;
        }
        let tag = data[4];
        let mut i = 5;
        match tag {
            TAG_JOIN => Some(LanJoin::Join { name: get_str(data, &mut i)? }),
            TAG_ACCEPT => {
                let world_name = get_str(data, &mut i)?;
                let world_seed = get_u64(data, &mut i)?;
                Some(LanJoin::Accept { world_name, world_seed })
            }
            TAG_DENY => Some(LanJoin::Deny { reason: get_str(data, &mut i)? }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(m: LanJoin) {
        assert_eq!(LanJoin::parse(&m.encode()).as_ref(), Some(&m));
    }

    #[test]
    fn roundtrips_all_variants() {
        roundtrip(LanJoin::Join { name: "dragon_rider".into() });
        roundtrip(LanJoin::Accept { world_name: "Overworld".into(), world_seed: 0xDEAD_BEEF_1234 });
        roundtrip(LanJoin::Deny { reason: "world full".into() });
    }

    #[test]
    fn accept_preserves_seed_exactly() {
        let seed = 0xFEDC_BA98_7654_3210u64;
        let enc = LanJoin::Accept { world_name: "w".into(), world_seed: seed }.encode();
        match LanJoin::parse(&enc) {
            Some(LanJoin::Accept { world_seed, .. }) => assert_eq!(world_seed, seed),
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    #[test]
    fn rejects_bad_magic_and_truncation() {
        assert!(LanJoin::parse(b"XXXX\x01").is_none());
        assert!(LanJoin::parse(b"ULJ1").is_none()); // no tag
        assert!(LanJoin::parse(&[]).is_none());
        // a Join truncated mid-string
        let mut e = LanJoin::Join { name: "abcdef".into() }.encode();
        e.truncate(7);
        assert!(LanJoin::parse(&e).is_none());
        // unknown tag
        assert!(LanJoin::parse(b"ULJ1\x09").is_none());
    }

    #[test]
    fn oversized_string_is_clamped_not_panic() {
        let big = "x".repeat(10_000);
        let enc = LanJoin::Join { name: big }.encode();
        // clamped to MAX_STR on encode; still parses back cleanly
        match LanJoin::parse(&enc) {
            Some(LanJoin::Join { name }) => assert_eq!(name.len(), MAX_STR),
            other => panic!("expected Join, got {other:?}"),
        }
    }
}
