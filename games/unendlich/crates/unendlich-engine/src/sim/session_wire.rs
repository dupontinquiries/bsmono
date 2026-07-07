//! `sim::session_wire` — wire format for LAN host discovery.
//!
//! The session layer ([`session_mode`](crate::sim::session_mode)) already models
//! what a host advertises ([`WorldAdvert`]) and how a joiner browses
//! ([`ServerBrowser`]); what was missing is turning an advert into a datagram a
//! host can multicast and parsing one back on the joiner. This is that wire
//! layer, modelled on the reference P2P stack's gossip discovery (periodic JOIN
//! announcements on a UDP multicast group): a host beacons [`encode_join`] on a
//! timer, every listener runs [`parse`] on each datagram and feeds the result to
//! `ServerBrowser::observe`.
//!
//! Pure and panic-free. Datagrams arrive from the open network, so parsing is
//! strictly defensive: an oversized, wrong-version, truncated, or malformed
//! packet yields `None`, never a panic or an unbounded allocation. The encoded
//! form is plain text (versioned, tab-delimited) so it needs no serialization
//! dependency and stays debuggable on the wire.

use crate::sim::net_candidates::{CandKind, Candidate};
use crate::sim::session_mode::WorldAdvert;

/// Wire version tag — bump on any format change so old/new peers reject rather
/// than misparse each other.
pub const MAGIC: &str = "UNDL1";

/// Hard cap on an accepted datagram. A safe-ish UDP payload that stays well
/// under a typical 1500-byte MTU, and bounds the work any single packet can
/// cause. Larger datagrams are rejected before any parsing or allocation (the
/// reference does the same to avoid OOM on crafted input).
pub const MAX_DATAGRAM: usize = 1200;

/// Cap on the variable-length parts so one packet can't describe an absurd
/// advert. (Matches what fits sanely in `MAX_DATAGRAM` anyway.)
const MAX_STR: usize = 64;
const MAX_CANDS: usize = 8;

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            ';' => out.push_str("\\s"),
            _ => out.push(c),
        }
    }
    out
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('\\') => out.push('\\'),
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('s') => out.push(';'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn cand_str(c: &Candidate) -> String {
    let k = match c.kind {
        CandKind::Lan => 'L',
        CandKind::Public => 'P',
    };
    format!("{},{}.{}.{}.{},{}", k, c.host[0], c.host[1], c.host[2], c.host[3], c.port)
}

fn parse_cand(s: &str) -> Option<Candidate> {
    let mut parts = s.split(',');
    let kind = match parts.next()? {
        "L" => CandKind::Lan,
        "P" => CandKind::Public,
        _ => return None,
    };
    let ip = parts.next()?;
    let port: u16 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None; // trailing junk
    }
    let mut oct = ip.split('.');
    let host = [
        oct.next()?.parse().ok()?,
        oct.next()?.parse().ok()?,
        oct.next()?.parse().ok()?,
        oct.next()?.parse().ok()?,
    ];
    if oct.next().is_some() {
        return None;
    }
    Some(Candidate { host, port, kind })
}

/// Serialize an advert into a JOIN datagram. `last_seen_ms` is deliberately not
/// transmitted — it's a receiver-clock stamp the listener sets on arrival, so a
/// host's clock skew can't poison freshness ordering in the browser.
pub fn encode_join(a: &WorldAdvert) -> Vec<u8> {
    let cands = a
        .candidates
        .iter()
        .take(MAX_CANDS)
        .map(cand_str)
        .collect::<Vec<_>>()
        .join(";");
    let s = format!(
        "{MAGIC}\tJOIN\t{}\t{}\t{}\t{}\t{}\t{}",
        esc(&a.host_username),
        esc(&a.world_name),
        a.players,
        a.max_players,
        a.world_seed,
        cands,
    );
    s.into_bytes()
}

/// Parse a datagram into an advert, stamping `last_seen_ms = now_ms`. Returns
/// `None` for anything oversized, wrong-version, truncated, or malformed — the
/// caller can feed the `Some` straight to `ServerBrowser::observe`.
pub fn parse(data: &[u8], now_ms: u64) -> Option<WorldAdvert> {
    if data.is_empty() || data.len() > MAX_DATAGRAM {
        return None;
    }
    let text = std::str::from_utf8(data).ok()?;
    // Exactly 8 fields: MAGIC, JOIN, user, world, players, max, seed, cands.
    // splitn keeps the candidate blob intact even though it may be empty.
    let f: Vec<&str> = text.splitn(8, '\t').collect();
    if f.len() != 8 || f[0] != MAGIC || f[1] != "JOIN" {
        return None;
    }
    let host_username = unesc(f[2]);
    let world_name = unesc(f[3]);
    if host_username.chars().count() > MAX_STR || world_name.chars().count() > MAX_STR {
        return None;
    }
    let players: u16 = f[4].parse().ok()?;
    let max_players: u16 = f[5].parse().ok()?;
    let world_seed: u64 = f[6].parse().ok()?;

    let mut candidates = Vec::new();
    if !f[7].is_empty() {
        for cs in f[7].split(';') {
            if candidates.len() >= MAX_CANDS {
                return None; // too many → treat as malformed rather than silently truncating
            }
            candidates.push(parse_cand(cs)?);
        }
    }

    Some(WorldAdvert {
        host_username,
        world_name,
        players,
        max_players,
        last_seen_ms: now_ms,
        candidates,
        world_seed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advert() -> WorldAdvert {
        WorldAdvert {
            host_username: "kessler".into(),
            world_name: "Dragon Vale".into(),
            players: 1,
            max_players: 4,
            last_seen_ms: 999, // not transmitted
            candidates: vec![
                Candidate { host: [192, 168, 2, 178], port: 9876, kind: CandKind::Lan },
                Candidate { host: [203, 0, 113, 7], port: 40123, kind: CandKind::Public },
            ],
            world_seed: 0x5EED_BEEF,
        }
    }

    #[test]
    fn roundtrip_preserves_everything_but_last_seen() {
        let a = advert();
        let got = parse(&encode_join(&a), 1234).expect("parses");
        assert_eq!(got.host_username, a.host_username);
        assert_eq!(got.world_name, a.world_name);
        assert_eq!(got.players, a.players);
        assert_eq!(got.max_players, a.max_players);
        assert_eq!(got.world_seed, a.world_seed);
        assert_eq!(got.candidates, a.candidates);
        assert_eq!(got.last_seen_ms, 1234, "receiver stamps its own clock");
    }

    #[test]
    fn empty_candidates_ok() {
        let mut a = advert();
        a.candidates.clear();
        let got = parse(&encode_join(&a), 5).expect("parses");
        assert!(got.candidates.is_empty());
    }

    #[test]
    fn oversized_is_rejected_before_parse() {
        let big = vec![b'x'; MAX_DATAGRAM + 1];
        assert!(parse(&big, 0).is_none());
    }

    #[test]
    fn wrong_version_or_type_rejected() {
        assert!(parse(b"UNDL0\tJOIN\ta\tb\t0\t0\t0\t", 0).is_none());
        assert!(parse(b"UNDL1\tLEAVE\ta\tb\t0\t0\t0\t", 0).is_none());
    }

    #[test]
    fn truncated_and_malformed_rejected() {
        assert!(parse(b"UNDL1\tJOIN\ta\tb\t0", 0).is_none()); // too few fields
        assert!(parse(b"UNDL1\tJOIN\ta\tb\tNOTNUM\t0\t0\t", 0).is_none()); // bad int
        assert!(parse(b"", 0).is_none());
        // bad candidate (missing port)
        assert!(parse(b"UNDL1\tJOIN\ta\tb\t1\t4\t7\tL,1.2.3.4", 0).is_none());
        // bad ip (3 octets)
        assert!(parse(b"UNDL1\tJOIN\ta\tb\t1\t4\t7\tL,1.2.3,5", 0).is_none());
    }

    #[test]
    fn delimiters_in_names_survive() {
        let mut a = advert();
        a.host_username = "a\tb\nc\\d;e".into();
        a.world_name = "my;world\tname".into();
        let got = parse(&encode_join(&a), 0).expect("parses");
        assert_eq!(got.host_username, "a\tb\nc\\d;e");
        assert_eq!(got.world_name, "my;world\tname");
    }

    #[test]
    fn too_many_candidates_rejected() {
        // Hand-build a packet with MAX_CANDS+1 candidates.
        let one = "L,1.1.1.1,1";
        let blob = std::iter::repeat(one).take(MAX_CANDS + 1).collect::<Vec<_>>().join(";");
        let pkt = format!("{MAGIC}\tJOIN\tu\tw\t1\t4\t7\t{blob}");
        assert!(parse(pkt.as_bytes(), 0).is_none());
    }

    #[test]
    fn non_utf8_rejected() {
        let bad = [b'U', b'N', b'D', b'L', b'1', b'\t', 0xFF, 0xFE];
        assert!(parse(&bad, 0).is_none());
    }

    #[test]
    fn feeds_server_browser() {
        use crate::sim::session_mode::ServerBrowser;
        let mut br = ServerBrowser::new();
        if let Some(adv) = parse(&encode_join(&advert()), 100) {
            br.observe(adv);
        }
        assert_eq!(br.len(), 1);
    }
}
