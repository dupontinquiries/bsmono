//! lan — a session that says its name on the local network, and an id a
//! person can read to a friend.
//!
//! ## The id
//!
//! A session is keyed by 20 bytes ([`crate::carrier::session_key`]). Nobody
//! dictates 40 hex digits over a phone, so [`SessionId`] renders the key as
//! 32 characters of a confusable-free base-32 alphabet (no `0/O`, `1/I/L`),
//! grouped in fours: `q7xk-3mfp-...`. It round-trips exactly; case and
//! dashes are ignored on the way back in. The palette shows it, and "join
//! session" accepts it.
//!
//! ## The beacon
//!
//! Two machines on one LAN should not need to exchange ids at all. Every
//! open session multicasts a [`Beacon`] once a second — the key, the
//! project name, who is hosting — and every Studio listens, so the palette
//! can offer "join Alice's 'demo'" with no typing. UDP multicast on
//! `239.87.83.1:8781` (`0x5753` = "SW" reversed; nothing else uses it).
//! The beacon carries no secret: the key is the address of a session,
//! not its password, exactly as the DHT already treats it. A LAN that can
//! see the beacon can already see the DHT traffic.
//!
//! [`Discovery`] owns the socket; [`Beacon`] is the packet and is pure.
//! Sockets are std, non-blocking, drained once per frame like everything
//! else in the shell.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

// ── the id ─────────────────────────────────────────────────────────────────

const ALPHABET: &[u8; 32] = b"abcdefghjkmnpqrstuvwxyz23456789_";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionId(pub [u8; 20]);

impl SessionId {
    /// `xxxx-xxxx-xxxx-xxxx-xxxx-xxxx-xxxx-xxxx`.
    pub fn render(&self) -> String {
        let mut out = String::with_capacity(39);
        let mut acc: u64 = 0;
        let mut bits = 0;
        let mut n = 0;
        for &byte in &self.0 {
            acc = (acc << 8) | byte as u64;
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                let idx = ((acc >> bits) & 31) as usize;
                if n > 0 && n % 4 == 0 { out.push('-'); }
                out.push(ALPHABET[idx] as char);
                n += 1;
            }
        }
        out
    }

    /// Short form for a status bar: the first two groups.
    pub fn short(&self) -> String {
        self.render()[..9].to_string()
    }

    /// Lenient: ignores dashes, spaces and case; accepts the `0/1/l` a
    /// person types for `o/i`? No — those are not in the alphabet, and
    /// silently mapping them would make a wrong id look valid.
    pub fn parse(text: &str) -> Option<SessionId> {
        let mut acc: u64 = 0;
        let mut bits = 0;
        let mut out = Vec::with_capacity(20);
        let mut n = 0;
        for ch in text.chars() {
            if ch == '-' || ch.is_whitespace() { continue; }
            let c = ch.to_ascii_lowercase() as u8;
            let idx = ALPHABET.iter().position(|&a| a == c)? as u64;
            acc = (acc << 5) | idx;
            bits += 5;
            n += 1;
            if bits >= 8 {
                bits -= 8;
                out.push(((acc >> bits) & 0xFF) as u8);
            }
        }
        if n != 32 || out.len() != 20 { return None; }
        Some(SessionId(out.try_into().ok()?))
    }
}

// ── the beacon ─────────────────────────────────────────────────────────────

pub const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 87, 83, 1);
pub const PORT: u16 = 8781;
const MAGIC: &[u8; 4] = b"STDB";
const VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beacon {
    pub key: [u8; 20],
    pub project: String,
    pub host: String,
    /// The host's site id, so a machine does not list its own session.
    pub site: u64,
}

impl Beacon {
    /// `STDB | ver | key(20) | site(8 LE) | plen | project | hlen | host`.
    pub fn encode(&self) -> Vec<u8> {
        let p = self.project.as_bytes();
        let h = self.host.as_bytes();
        let mut out = Vec::with_capacity(4 + 1 + 20 + 8 + 1 + p.len().min(255) + 1 + h.len().min(255));
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.key);
        out.extend_from_slice(&self.site.to_le_bytes());
        out.push(p.len().min(255) as u8);
        out.extend_from_slice(&p[..p.len().min(255)]);
        out.push(h.len().min(255) as u8);
        out.extend_from_slice(&h[..h.len().min(255)]);
        out
    }

    pub fn decode(b: &[u8]) -> Option<Beacon> {
        if b.len() < 4 + 1 + 20 + 8 + 2 || &b[..4] != MAGIC || b[4] != VERSION { return None; }
        let mut i = 5;
        let key: [u8; 20] = b[i..i + 20].try_into().ok()?; i += 20;
        let site = u64::from_le_bytes(b[i..i + 8].try_into().ok()?); i += 8;
        let pl = *b.get(i)? as usize; i += 1;
        let project = String::from_utf8(b.get(i..i + pl)?.to_vec()).ok()?; i += pl;
        let hl = *b.get(i)? as usize; i += 1;
        let host = String::from_utf8(b.get(i..i + hl)?.to_vec()).ok()?; i += hl;
        if i != b.len() { return None; }
        Some(Beacon { key, project, host, site })
    }
}

/// A session seen on the LAN, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub beacon: Beacon,
    pub from: std::net::SocketAddr,
    pub last_seen_ms: i64,
}

/// Sessions drop off the list after this long without a beacon.
pub const EXPIRE_MS: i64 = 5_000;

/// The socket and the table. `None` fields when the network refused; the
/// palette then simply has no LAN entries, and says so.
pub struct Discovery {
    socket: Option<UdpSocket>,
    seen: BTreeMap<[u8; 20], Sighting>,
    last_announce_ms: i64,
    pub error: Option<String>,
}

impl Discovery {
    /// Bind the multicast listener. Failure is recorded, not fatal.
    pub fn open() -> Discovery {
        let mut d = Discovery { socket: None, seen: BTreeMap::new(), last_announce_ms: 0, error: None };
        match Self::bind(PORT) {
            Ok(s) => d.socket = Some(s),
            Err(e) => d.error = Some(e),
        }
        d
    }

    /// Bind on a specific port with no multicast — for tests on a loopback.
    pub fn open_local(port: u16) -> Discovery {
        let mut d = Discovery { socket: None, seen: BTreeMap::new(), last_announce_ms: 0, error: None };
        match UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)) {
            Ok(s) => { let _ = s.set_nonblocking(true); d.socket = Some(s); }
            Err(e) => d.error = Some(e.to_string()),
        }
        d
    }

    fn bind(port: u16) -> Result<UdpSocket, String> {
        let s = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).map_err(|e| format!("bind {port}: {e}"))?;
        s.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED).map_err(|e| format!("join multicast: {e}"))?;
        s.set_nonblocking(true).map_err(|e| e.to_string())?;
        let _ = s.set_multicast_loop_v4(true);
        Ok(s)
    }

    /// Send our beacon if a second has passed. Call every frame.
    pub fn announce(&mut self, beacon: &Beacon, now_ms: i64) {
        self.announce_to(beacon, now_ms, std::net::SocketAddr::V4(SocketAddrV4::new(MULTICAST_ADDR, PORT)));
    }

    pub fn announce_to(&mut self, beacon: &Beacon, now_ms: i64, to: std::net::SocketAddr) {
        if now_ms - self.last_announce_ms < 1000 { return; }
        self.last_announce_ms = now_ms;
        if let Some(s) = &self.socket {
            if let Err(e) = s.send_to(&beacon.encode(), to) {
                self.error = Some(format!("announce: {e}"));
            }
        }
    }

    /// Drain received beacons into the table and expire old ones. Call
    /// every frame; returns the number of new arrivals.
    pub fn poll(&mut self, now_ms: i64) -> usize {
        let mut fresh = 0;
        if let Some(s) = &self.socket {
            let mut buf = [0u8; 1024];
            while let Ok((n, from)) = s.recv_from(&mut buf) {
                if let Some(b) = Beacon::decode(&buf[..n]) {
                    let key = b.key;
                    let is_new = !self.seen.contains_key(&key);
                    self.seen.insert(key, Sighting { beacon: b, from, last_seen_ms: now_ms });
                    if is_new { fresh += 1; }
                }
            }
        }
        self.seen.retain(|_, s| now_ms - s.last_seen_ms <= EXPIRE_MS);
        fresh
    }

    /// What is on the LAN right now, minus our own session.
    pub fn sessions(&self, own_site: u64) -> Vec<&Sighting> {
        self.seen.values().filter(|s| s.beacon.site != own_site).collect()
    }

    pub fn is_open(&self) -> bool { self.socket.is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_id_renders_in_groups_and_parses_back_leniently() {
        let key = crate::carrier::session_key("team/demo");
        let id = SessionId(key);
        let text = id.render();
        assert_eq!(text.len(), 32 + 7, "{text}");
        assert_eq!(text.split('-').count(), 8);
        assert!(!text.contains(['0', '1', 'o', 'i', 'l']), "{text}");
        assert_eq!(SessionId::parse(&text), Some(id));
        assert_eq!(SessionId::parse(&text.to_uppercase().replace('-', " ")), Some(id));
        assert_eq!(id.short().len(), 9);
        assert_eq!(SessionId::parse("q7xk-3mfp"), None, "too short");
        let mut confusable = text.clone();
        confusable.replace_range(0..1, "0");
        assert_eq!(SessionId::parse(&confusable), None, "confusable characters are refused, not mapped");
    }

    #[test]
    fn every_key_survives_the_id_round_trip() {
        for i in 0..64u8 {
            let key = crate::carrier::session_key(&format!("k{i}"));
            assert_eq!(SessionId::parse(&SessionId(key).render()), Some(SessionId(key)));
        }
        assert_eq!(SessionId::parse(&SessionId([0; 20]).render()), Some(SessionId([0; 20])));
        assert_eq!(SessionId::parse(&SessionId([255; 20]).render()), Some(SessionId([255; 20])));
    }

    #[test]
    fn a_beacon_round_trips_and_rejects_junk() {
        let b = Beacon { key: [7; 20], project: "demo".into(), host: "kes (phone)".into(), site: 42 };
        let bytes = b.encode();
        assert_eq!(Beacon::decode(&bytes), Some(b.clone()));
        assert_eq!(Beacon::decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(Beacon::decode(b"STDB\x02"), None);
        let long = Beacon { key: [1; 20], project: "x".repeat(400), host: String::new(), site: 1 };
        assert_eq!(Beacon::decode(&long.encode()).unwrap().project.len(), 255, "names cap at a byte");
    }

    #[test]
    fn two_sockets_on_loopback_see_each_other_and_not_themselves() {
        let mut a = Discovery::open_local(0);
        let mut b = Discovery::open_local(0);
        let (Some(sa), Some(sb)) = (&a.socket, &b.socket) else {
            eprintln!("skipped: loopback UDP unavailable ({:?} / {:?})", a.error, b.error);
            return;
        };
        let addr_a = sa.local_addr().unwrap();
        let addr_b = sb.local_addr().unwrap();
        let ba = Beacon { key: [1; 20], project: "alpha".into(), host: "a".into(), site: 1 };
        let bb = Beacon { key: [2; 20], project: "beta".into(), host: "b".into(), site: 2 };
        a.announce_to(&ba, 1000, addr_b);
        b.announce_to(&bb, 1000, addr_a);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(a.poll(1100), 1);
        assert_eq!(b.poll(1100), 1);
        assert_eq!(a.sessions(1)[0].beacon.project, "beta");
        assert!(b.sessions(2).iter().all(|s| s.beacon.site != 2));
        // Rate limit: a second announce inside a second is dropped.
        a.announce_to(&ba, 1500, addr_b);
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert_eq!(b.poll(1600), 0);
        // Expiry.
        assert_eq!(a.poll(1100 + EXPIRE_MS + 1), 0);
        assert!(a.sessions(1).is_empty());
    }
}
