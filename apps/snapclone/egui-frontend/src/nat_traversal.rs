//! nat_traversal — `NatTraversal.cpp`'s loop, ported.
//!
//! `nat_punch` is the *strategy* from that file (classify, decide);
//! `stun_servers` is its server list; `nat-kit` is the STUN codec. What
//! none of them did was RUN: the binary learned peers' punch addresses
//! from records and gossip and sent voice to them, and never sent a STUN
//! request, never announced a mapped port, never punched. On a LAN that
//! is invisible. Across a carrier NAT it is the difference between the
//! Qt build, which got through, and this one, which could not.
//!
//! ## What crossed CGNAT, and why
//!
//! Three things, all on ONE socket:
//!
//! 1. **STUN from the punch socket.** A NAT assigns mappings per socket,
//!    so the mapped address announced in our record must be the mapping
//!    of the socket peers will actually send to. The C++ comment calls
//!    this out as the bug the whole file fixed.
//! 2. **A volley, then maintenance.** Sixty punches at 150 ms — nine
//!    seconds — open the filter on both sides when both peers do it; then
//!    one every twenty seconds keeps the carrier's mapping from timing out
//!    while the user is pocketed. Halved to thirty when the device is
//!    conserving and the user has not just acted.
//! 3. **The peer's REAL source address.** A carrier NAT may map our
//!    peer to a port other than the one STUN told them, and the punch that
//!    arrives carries the truth in its source. `registerPunchedPeer`
//!    rewrote the peer's address from the first punch received, and voice
//!    then went to where the packets actually come from.
//!
//! Symmetric NAT — a fresh mapping per destination — defeats all three,
//! and the source says so: TURN is the fallback, and it was on the roadmap
//! rather than in the tree. Same here.
//!
//! ## Sans-io, like the rest of the port
//!
//! [`Traversal`] owns no socket. It hands back datagrams to send and
//! events to act on; the binary owns the punch socket (which is the voice
//! socket — the same one, deliberately) and the clock. Every timing rule
//! above is therefore testable on a fake clock, which is where the C++
//! version's behaviour lived only in a log.

use std::net::{IpAddr, SocketAddr};

/// What a punch packet starts with. The rest is the sender's sc-ID, so
/// the receiver can attribute the source address to a peer.
pub const PUNCH_MAGIC: &[u8] = b"SNAPCLONE_PUNCH_V1\n";

/// `Timings::kPunchBurstIntervalMs` / `kPunchBurstCount`: 60 × 150 ms.
pub const BURST_INTERVAL_MS: u64 = 150;
pub const BURST_COUNT: u32 = 60;
/// The keep-open cadence once the volley is done. Under the common 30 s
/// UDP mapping lifetime; the same figure the C++ used.
pub const MAINTENANCE_MS: u64 = 20_000;
/// Per STUN server before rotating to the next.
pub const STUN_TIMEOUT_MS: u64 = 1_500;

/// A datagram the caller must send.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Out {
    pub to: SocketAddr,
    pub bytes: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Event {
    /// Our punch socket's mapped address, from a STUN server.
    Reflexive(SocketAddr),
    /// Every server timed out; no mapped address this round.
    StunFailed,
    /// The volley to this peer is complete: the path is as open as
    /// punching makes it. Emitted once per target.
    PunchReady { peer: String, addr: SocketAddr },
    /// A punch arrived from `from` claiming to be `peer`. `from` is where
    /// that peer's packets ACTUALLY come from and is the address to use.
    PunchArrived { peer: String, from: SocketAddr },
}

/// Why `initiate` declined.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    NoAddress,
    /// Loopback, our own id, or our own reflexive address — punching
    /// ourselves is a classic way to "succeed" at reaching nobody.
    OurselvesOrLoopback,
}

struct Stun {
    servers: Vec<SocketAddr>,
    idx: usize,
    tx: nat_kit::TxId,
    sent_ms: u64,
}

struct Punch {
    peer: String,
    addr: SocketAddr,
    sent: u32,
    burst: u32,
    next_ms: u64,
    ready: bool,
}

pub struct Traversal {
    self_id: String,
    reflexive: Option<SocketAddr>,
    stun: Option<Stun>,
    punches: Vec<Punch>,
    seed: u64,
}

impl Traversal {
    pub fn new(self_id: &str) -> Traversal {
        Traversal { self_id: self_id.to_string(), reflexive: None, stun: None,
                    punches: Vec::new(), seed: 0x9E37_79B9_7F4A_7C15 }
    }

    pub fn reflexive(&self) -> Option<SocketAddr> { self.reflexive }

    /// The punch packet we send: magic, then our id.
    pub fn punch_payload(&self) -> Vec<u8> {
        let mut b = PUNCH_MAGIC.to_vec();
        b.extend_from_slice(self.self_id.as_bytes());
        b
    }

    /// The sender's id, if `bytes` is a punch.
    pub fn parse_punch(bytes: &[u8]) -> Option<&str> {
        let id = bytes.strip_prefix(PUNCH_MAGIC)?;
        let id = std::str::from_utf8(id).ok()?.trim();
        (!id.is_empty()).then_some(id)
    }

    fn next_tx(&mut self) -> nat_kit::TxId {
        // Not a security boundary: a transaction id only needs to differ
        // between requests so a late reply is not taken for a new one.
        let mut tx = [0u8; 12];
        for b in tx.iter_mut() {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            *b = (self.seed >> 24) as u8;
        }
        tx
    }

    /// Begin (or restart) STUN discovery through `servers`, first one
    /// first. Returns the first request to send.
    pub fn start_stun(&mut self, servers: Vec<SocketAddr>, now_ms: u64) -> Vec<Out> {
        if servers.is_empty() {
            self.stun = None;
            return Vec::new();
        }
        let tx = self.next_tx();
        let to = servers[0];
        self.stun = Some(Stun { servers, idx: 0, tx, sent_ms: now_ms });
        vec![Out { to, bytes: nat_kit::Msg::BindingRequest { tx }.encode() }]
    }

    /// Start punching `peer` at `addr`. A second call for the same peer
    /// and address is a no-op; a different address restarts the volley
    /// at the new target, as the C++ did when a fresher record arrived.
    pub fn initiate(&mut self, peer: &str, addr: SocketAddr, conservative: bool, now_ms: u64)
        -> Result<(), Refusal> {
        if addr.port() == 0 || addr.ip().is_unspecified() {
            return Err(Refusal::NoAddress);
        }
        let self_reflexive = self.reflexive.is_some_and(|r| r.ip() == addr.ip());
        if addr.ip().is_loopback() || peer == self.self_id || self_reflexive {
            return Err(Refusal::OurselvesOrLoopback);
        }
        if let Some(p) = self.punches.iter_mut().find(|p| p.peer == peer) {
            if p.addr == addr { return Ok(()); }
            self.punches.retain(|p| p.peer != peer);
        }
        // Computed ONCE at the start of the volley, as the C++ captured
        // it by value: a context flip mid-burst must not change its mind
        // halfway.
        let burst = if conservative { BURST_COUNT / 2 } else { BURST_COUNT };
        self.punches.push(Punch { peer: peer.to_string(), addr, sent: 0, burst, next_ms: now_ms, ready: false });
        Ok(())
    }

    pub fn stop(&mut self, peer: &str) {
        self.punches.retain(|p| p.peer != peer);
    }

    /// Where we are currently punching `peer`, if anywhere.
    pub fn target_of(&self, peer: &str) -> Option<SocketAddr> {
        self.punches.iter().find(|p| p.peer == peer).map(|p| p.addr)
    }

    /// Everything due at `now_ms`: punches, STUN retries. Call every frame;
    /// idle frames cost a few comparisons.
    pub fn tick(&mut self, now_ms: u64) -> (Vec<Out>, Vec<Event>) {
        let mut out = Vec::new();
        let mut events = Vec::new();
        let payload = self.punch_payload();
        for p in self.punches.iter_mut() {
            if now_ms < p.next_ms { continue; }
            out.push(Out { to: p.addr, bytes: payload.clone() });
            p.sent += 1;
            if p.sent >= p.burst {
                p.next_ms = now_ms + MAINTENANCE_MS;
                if !p.ready {
                    p.ready = true;
                    events.push(Event::PunchReady { peer: p.peer.clone(), addr: p.addr });
                }
            } else {
                p.next_ms = now_ms + BURST_INTERVAL_MS;
            }
        }
        // STUN: rotate on timeout; give up after the last server.
        if let Some(s) = self.stun.as_mut() {
            if now_ms.saturating_sub(s.sent_ms) >= STUN_TIMEOUT_MS {
                s.idx += 1;
                if s.idx >= s.servers.len() {
                    self.stun = None;
                    events.push(Event::StunFailed);
                } else {
                    let to = s.servers[s.idx];
                    let tx = {
                        let mut x = [0u8; 12];
                        for (i, b) in x.iter_mut().enumerate() { *b = s.tx[i].wrapping_add(1 + i as u8); }
                        x
                    };
                    s.tx = tx;
                    s.sent_ms = now_ms;
                    out.push(Out { to, bytes: nat_kit::Msg::BindingRequest { tx }.encode() });
                }
            }
        }
        (out, events)
    }

    /// A datagram from a source the voice path did not recognise. Punches
    /// and STUN replies both arrive that way — one from a peer whose real
    /// address we are about to learn, one from a server.
    pub fn on_datagram(&mut self, from: SocketAddr, bytes: &[u8]) -> Option<Event> {
        if let Some(peer) = Self::parse_punch(bytes) {
            if peer == self.self_id { return None; }
            return Some(Event::PunchArrived { peer: peer.to_string(), from });
        }
        let s = self.stun.as_ref()?;
        if from != s.servers[s.idx] { return None; }
        match nat_kit::Msg::decode(bytes).ok()? {
            nat_kit::Msg::BindingSuccess { tx, addr } if tx == s.tx => {
                let mapped = match addr {
                    nat_kit::Addr::V4(ip, port) => SocketAddr::new(IpAddr::from(ip), port),
                    nat_kit::Addr::V6(ip, port) => SocketAddr::new(IpAddr::from(ip), port),
                };
                self.reflexive = Some(mapped);
                self.stun = None;
                Some(Event::Reflexive(mapped))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(s: &str) -> SocketAddr { s.parse().unwrap() }

    #[test]
    fn a_volley_is_sixty_punches_at_150ms_then_one_every_twenty_seconds() {
        let mut t = Traversal::new("me");
        t.initiate("bob", sa("203.0.113.5:40000"), false, 0).unwrap();
        let mut sent = 0;
        let mut ready_at = None;
        let mut now = 0;
        while now <= 30_000 {
            let (out, ev) = t.tick(now);
            sent += out.len();
            if ev.iter().any(|e| matches!(e, Event::PunchReady { .. })) { ready_at = Some(now); }
            now += 50;
        }
        // 60 in the first 9 s, then one at ~9 s + 20 s = 29 s.
        assert_eq!(sent, 61, "sixty in the volley plus one maintenance by 30 s");
        assert_eq!(ready_at, Some(59 * BURST_INTERVAL_MS as u64), "ready on the sixtieth punch");
        let payload = t.punch_payload();
        assert!(payload.starts_with(PUNCH_MAGIC) && payload.ends_with(b"me"));
    }

    #[test]
    fn conserving_halves_the_volley_and_keeps_the_maintenance() {
        let mut t = Traversal::new("me");
        t.initiate("bob", sa("203.0.113.5:40000"), true, 0).unwrap();
        let mut sent = 0;
        for now in (0..=30_000).step_by(50) { sent += t.tick(now).0.len(); }
        assert_eq!(sent, 31, "thirty in the volley plus one maintenance");
    }

    #[test]
    fn ourselves_and_loopback_are_refused_and_a_retarget_restarts() {
        let mut t = Traversal::new("me");
        assert_eq!(t.initiate("me", sa("203.0.113.5:1"), false, 0), Err(Refusal::OurselvesOrLoopback));
        assert_eq!(t.initiate("bob", sa("127.0.0.1:9"), false, 0), Err(Refusal::OurselvesOrLoopback));
        assert_eq!(t.initiate("bob", sa("0.0.0.0:0"), false, 0), Err(Refusal::NoAddress));
        t.reflexive = Some(sa("198.51.100.7:5000"));
        assert_eq!(t.initiate("bob", sa("198.51.100.7:6000"), false, 0), Err(Refusal::OurselvesOrLoopback),
                   "a peer at our own reflexive ip is behind our NAT with us, or is us");

        t.initiate("bob", sa("203.0.113.5:40000"), false, 0).unwrap();
        for now in (0..3_000).step_by(50) { t.tick(now); }   // twenty punches in
        t.initiate("bob", sa("203.0.113.5:40000"), false, 3_000).unwrap();  // same: no-op
        assert_eq!(t.punches[0].sent, 20);
        t.initiate("bob", sa("203.0.113.5:41000"), false, 3_000).unwrap();  // fresher record
        assert_eq!(t.punches.len(), 1);
        assert_eq!(t.punches[0].sent, 0, "a new target starts a new volley");
        assert_eq!(t.target_of("bob"), Some(sa("203.0.113.5:41000")));
        t.stop("bob");
        assert!(t.target_of("bob").is_none());
    }

    #[test]
    fn stun_rotates_servers_on_timeout_and_learns_the_mapping_from_the_right_one() {
        let mut t = Traversal::new("me");
        let servers = vec![sa("158.69.23.138:3478"), sa("74.125.197.127:19302")];
        let first = t.start_stun(servers.clone(), 0);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].to, servers[0]);
        let req = nat_kit::Msg::decode(&first[0].bytes).unwrap();
        let nat_kit::Msg::BindingRequest { tx: tx0 } = req else { panic!() };

        // A reply from somebody else, or with the wrong tx: ignored.
        let fake = nat_kit::Msg::BindingSuccess { tx: tx0, addr: nat_kit::Addr::V4([9, 9, 9, 9], 9) }.encode();
        assert_eq!(t.on_datagram(sa("1.2.3.4:5"), &fake), None);
        let wrong_tx = nat_kit::Msg::BindingSuccess { tx: [7; 12], addr: nat_kit::Addr::V4([9, 9, 9, 9], 9) }.encode();
        assert_eq!(t.on_datagram(servers[0], &wrong_tx), None);

        // Timeout: the second server is asked, with a different tx.
        let (out, ev) = t.tick(STUN_TIMEOUT_MS);
        assert!(ev.is_empty());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to, servers[1]);
        let nat_kit::Msg::BindingRequest { tx: tx1 } = nat_kit::Msg::decode(&out[0].bytes).unwrap() else { panic!() };
        assert_ne!(tx0, tx1);

        // The real reply: our mapping for THIS socket.
        let ok = nat_kit::Msg::BindingSuccess { tx: tx1, addr: nat_kit::Addr::V4([203, 0, 113, 77], 51234) }.encode();
        assert_eq!(t.on_datagram(servers[1], &ok), Some(Event::Reflexive(sa("203.0.113.77:51234"))));
        assert_eq!(t.reflexive(), Some(sa("203.0.113.77:51234")));

        // Every server silent: said so, once.
        let mut t = Traversal::new("me");
        t.start_stun(servers, 0);
        assert!(t.tick(STUN_TIMEOUT_MS).1.is_empty());
        assert_eq!(t.tick(2 * STUN_TIMEOUT_MS).1, vec![Event::StunFailed]);
        assert!(t.tick(3 * STUN_TIMEOUT_MS).1.is_empty());
    }

    #[test]
    fn a_punch_that_arrives_names_the_peer_and_its_real_source() {
        let mut t = Traversal::new("me");
        let mut bytes = PUNCH_MAGIC.to_vec();
        bytes.extend_from_slice(b"bob");
        // The carrier mapped bob to 40012, not the 40000 his record said.
        assert_eq!(t.on_datagram(sa("203.0.113.5:40012"), &bytes),
                   Some(Event::PunchArrived { peer: "bob".into(), from: sa("203.0.113.5:40012") }));
        let mut own = PUNCH_MAGIC.to_vec();
        own.extend_from_slice(b"me");
        assert_eq!(t.on_datagram(sa("203.0.113.5:1"), &own), None, "our own punch reflected back is not a peer");
        assert_eq!(Traversal::parse_punch(b"SNAPCLONE_PUNCH_V1\n"), None);
        assert_eq!(t.on_datagram(sa("203.0.113.5:1"), b"\x00\x01garbage"), None);
    }
}
