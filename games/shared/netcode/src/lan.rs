//! lan — find a game on the local network without typing anything.
//!
//! `multiplayer` joins a friend BY ID through the DHT (or a shared file, on
//! one machine). Neither answers "who on this network is hosting right
//! now?", which is the whole of a LAN screen. This does: a host broadcasts a
//! small [`Beacon`] over UDP once a second; a [`Browser`] listens, keeps what
//! it has heard recently, and forgets a host that goes quiet.
//!
//! Control plane only, like the rest of this crate: a beacon carries where
//! the host's game socket is, never gameplay bytes. std only.
//!
//! The wire format is one line of tab-separated text behind a magic, so a
//! packet capture is readable and a stray datagram from anything else on the
//! port is rejected rather than parsed.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

pub const LAN_PORT: u16 = 47_624;
pub const MAGIC: &str = "BSLAN1";
/// A host not heard from for this long is gone.
pub const STALE_AFTER: Duration = Duration::from_millis(3500);
pub const ANNOUNCE_EVERY: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beacon {
    /// Registry id: "clawbound", "soldier", ... A browser only lists its own.
    pub game: String,
    /// Stable per host, so a re-announce updates a row instead of adding one.
    pub host_id: String,
    pub name: String,
    /// The host's GAME socket. The address is taken from the datagram.
    pub port: u16,
    pub players: u8,
    pub slots: u8,
    /// Hosts and joiners must agree on this, or the row is shown as unjoinable.
    pub proto: u16,
}

impl Beacon {
    pub fn encode(&self) -> Vec<u8> {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ").chars().take(48).collect::<String>();
        format!("{MAGIC}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", clean(&self.game), clean(&self.host_id), clean(&self.name), self.port, self.players, self.slots, self.proto).into_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Option<Beacon> {
        if bytes.len() > 512 {
            return None;
        }
        let text = std::str::from_utf8(bytes).ok()?;
        let f: Vec<&str> = text.split('\t').collect();
        if f.len() != 8 || f[0] != MAGIC || f[1].is_empty() || f[2].is_empty() {
            return None;
        }
        let b = Beacon { game: f[1].into(), host_id: f[2].into(), name: f[3].into(), port: f[4].parse().ok()?, players: f[5].parse().ok()?, slots: f[6].parse().ok()?, proto: f[7].parse().ok()? };
        (b.players <= b.slots && b.port != 0).then_some(b)
    }
}

/// The hosting side.
pub struct Announcer {
    sock: UdpSocket,
    to: Vec<SocketAddr>,
    last: Option<Instant>,
}

impl Announcer {
    /// Broadcasts to the LAN, and to loopback so a second copy on the same
    /// machine sees it too.
    pub fn new() -> io::Result<Announcer> {
        Self::to(vec![(Ipv4Addr::BROADCAST, LAN_PORT).into(), (Ipv4Addr::LOCALHOST, LAN_PORT).into()])
    }
    pub fn to(to: Vec<SocketAddr>) -> io::Result<Announcer> {
        let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        sock.set_broadcast(true)?;
        Ok(Announcer { sock, to, last: None })
    }
    /// Call every frame; sends at most once per [`ANNOUNCE_EVERY`]. Returns
    /// whether it sent. A destination that fails (no route, broadcast
    /// refused) is skipped: the others may still work.
    pub fn tick(&mut self, b: &Beacon, now: Instant) -> bool {
        if self.last.is_some_and(|t| now.duration_since(t) < ANNOUNCE_EVERY) {
            return false;
        }
        self.last = Some(now);
        let bytes = b.encode();
        self.to.iter().filter(|a| self.sock.send_to(&bytes, a).is_ok()).count() > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub beacon: Beacon,
    /// Where to reach the host's game socket.
    pub addr: SocketAddr,
    pub heard: Instant,
}

/// The browsing side.
pub struct Browser {
    sock: UdpSocket,
    game: String,
    proto: u16,
    own_id: String,
    found: Vec<Found>,
}

impl Browser {
    pub fn new(game: &str, proto: u16, own_id: &str) -> io::Result<Browser> {
        Self::on_port(game, proto, own_id, LAN_PORT)
    }
    /// std cannot set SO_REUSEADDR, so two browsers on one machine cannot
    /// share [`LAN_PORT`]: the second gets `AddrInUse`. Callers should say so
    /// rather than show an empty list.
    pub fn on_port(game: &str, proto: u16, own_id: &str, port: u16) -> io::Result<Browser> {
        let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
        sock.set_nonblocking(true)?;
        Ok(Browser { sock, game: game.into(), proto, own_id: own_id.into(), found: Vec::new() })
    }
    pub fn port(&self) -> u16 {
        self.sock.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// Drain the socket, drop the stale, return what is left, newest first.
    pub fn poll(&mut self, now: Instant) -> &[Found] {
        let mut buf = [0u8; 600];
        // Bounded: a flood on the port must not hold the frame.
        for _ in 0..64 {
            let Ok((n, from)) = self.sock.recv_from(&mut buf) else { break };
            let Some(b) = Beacon::decode(&buf[..n]) else { continue };
            if b.game != self.game || b.host_id == self.own_id {
                continue;
            }
            let addr = SocketAddr::new(from.ip(), b.port);
            match self.found.iter_mut().find(|f| f.beacon.host_id == b.host_id) {
                Some(f) => *f = Found { beacon: b, addr, heard: now },
                None => self.found.push(Found { beacon: b, addr, heard: now }),
            }
        }
        self.found.retain(|f| now.duration_since(f.heard) < STALE_AFTER);
        self.found.sort_by(|a, b| b.heard.cmp(&a.heard).then(a.beacon.name.cmp(&b.beacon.name)));
        &self.found
    }

    pub fn joinable(&self, f: &Found) -> bool {
        f.beacon.proto == self.proto && f.beacon.players < f.beacon.slots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beacon(id: &str, players: u8) -> Beacon {
        Beacon { game: "clawbound".into(), host_id: id.into(), name: format!("{id}'s den"), port: 40_000, players, slots: 10, proto: 1 }
    }
    fn hear(b: &mut Browser, now: Instant) -> Vec<Found> {
        // UDP on loopback is fast, not instant.
        for _ in 0..50 {
            if !b.poll(now).is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        b.poll(now).to_vec()
    }

    #[test]
    fn a_beacon_survives_the_wire_and_junk_does_not() {
        let b = beacon("ash", 3);
        assert_eq!(Beacon::decode(&b.encode()), Some(b.clone()));
        let nasty = Beacon { name: "tab\there\nnewline".into(), ..b.clone() };
        let back = Beacon::decode(&nasty.encode()).expect("separators in a name must not break the frame");
        assert_eq!(back.name, "tab here newline");
        for junk in [&b"hello"[..], b"", b"BSLAN1\tclawbound", b"BSLAN9\tclawbound\tx\tn\t1\t0\t1\t1", &[0xff, 0xfe, 0x00][..], &vec![b'a'; 2000][..]] {
            assert_eq!(Beacon::decode(junk), None);
        }
        assert_eq!(Beacon::decode(&Beacon { players: 11, ..b.clone() }.encode()), None, "more players than slots is not a lobby");
        assert_eq!(Beacon::decode(&Beacon { port: 0, ..b }.encode()), None);
    }

    /// Real sockets, on loopback: announce, hear, update in place, go stale.
    #[test]
    fn a_host_is_heard_updated_in_place_and_forgotten_when_it_goes_quiet() {
        let mut br = Browser::on_port("clawbound", 1, "me", 0).unwrap();
        let mut an = Announcer::to(vec![(Ipv4Addr::LOCALHOST, br.port()).into()]).unwrap();
        let t0 = Instant::now();
        assert!(an.tick(&beacon("ash", 1), t0));
        assert!(!an.tick(&beacon("ash", 1), t0 + Duration::from_millis(200)), "at most once a second");
        let got = hear(&mut br, t0);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].beacon.players, got[0].addr.port()), (1, 40_000), "the row points at the GAME socket, not the beacon's source port");
        assert!(br.joinable(&got[0]));

        // A second announce from the same host is the same row.
        let t1 = t0 + ANNOUNCE_EVERY;
        assert!(an.tick(&beacon("ash", 4), t1));
        std::thread::sleep(Duration::from_millis(30));
        let got = br.poll(t1).to_vec();
        assert_eq!((got.len(), got[0].beacon.players), (1, 4));

        // Silence.
        assert_eq!(br.poll(t1 + STALE_AFTER - Duration::from_millis(10)).len(), 1, "not yet");
        assert!(br.poll(t1 + STALE_AFTER + Duration::from_millis(10)).is_empty(), "a host that stops announcing disappears");
    }

    #[test]
    fn other_games_myself_and_mismatched_builds_are_handled() {
        let mut br = Browser::on_port("clawbound", 2, "me", 0).unwrap();
        let mut an = Announcer::to(vec![(Ipv4Addr::LOCALHOST, br.port()).into()]).unwrap();
        let now = Instant::now();
        let tick = |an: &mut Announcer, b: Beacon, k: u32| assert!(an.tick(&b, now + ANNOUNCE_EVERY * k));
        tick(&mut an, Beacon { game: "soldier".into(), ..beacon("sol", 1) }, 0);
        tick(&mut an, beacon("me", 1), 1);
        tick(&mut an, beacon("old", 1), 2); // proto 1 against our 2
        tick(&mut an, Beacon { proto: 2, players: 10, ..beacon("full", 0) }, 3);
        tick(&mut an, Beacon { proto: 2, ..beacon("ok", 2) }, 4);
        std::thread::sleep(Duration::from_millis(40));
        let got = hear(&mut br, now).to_vec();
        let ids: Vec<&str> = got.iter().map(|f| f.beacon.host_id.as_str()).collect();
        assert!(!ids.contains(&"sol") && !ids.contains(&"me"), "another game and my own beacon are not rows: {ids:?}");
        assert_eq!(ids.len(), 3, "{ids:?}");
        let can = |id: &str| br.joinable(got.iter().find(|f| f.beacon.host_id == id).unwrap());
        assert!(can("ok") && !can("old") && !can("full"), "listed, but only a matching build with a free slot can be joined");
    }
}
