//! voice_wire — the socket a call's audio travels on.
//!
//! `voice_bridge` produces `(peer_id, bytes)` to send and consumes
//! `(sender, bytes)` on arrival, and nothing in the tree carried either. A
//! call could be joined and the floor taken with no transport underneath —
//! the same shape as the camera before `capture`.
//!
//! ## Identity comes from the address, and that is a deliberate weakness
//!
//! `voice_bridge`'s security rule is that the asserting peer IS the sender,
//! taken from the transport rather than from inside the packet, because a
//! self-describing floor assert lets anyone claim the floor as anyone. This
//! module is that transport, and today it authenticates by SOURCE ADDRESS
//! against addresses learned from SIGNED gossip.
//!
//! That is weaker than it sounds and the weakness is worth stating plainly:
//! UDP source addresses are forgeable, so on a hostile LAN a spoofed
//! datagram can present as a peer. What it is NOT is anonymous — the
//! attacker must know a current punch address, which only signed gossip
//! hands out, and they cannot read the reply. The real fix is authenticating
//! the media path itself (the handshake module is where that belongs), and
//! until then this refuses everything from an unknown address rather than
//! accepting it as "some peer", which is the difference between a narrow
//! spoofing risk and an open one.
//!
//! ## Unreliable on purpose
//!
//! No retransmit, no ordering, no ack. A voice frame that arrives late is
//! worse than one that never arrives: the play-out cushion has already moved
//! past its slot, so a resend costs bandwidth to deliver something that gets
//! discarded. `sink_cushion` handles gaps; this does not try to prevent
//! them.

use std::net::{SocketAddr, UdpSocket};

use crate::punch_addrs::PeerAddr;

/// Bigger than any frame this app sends (20 ms of Opus is tens of bytes),
/// small enough that a hostile sender cannot make us allocate. Anything
/// larger is truncated by `recv_from` and will fail to decode, which is the
/// correct outcome for a datagram we were never going to understand.
pub const MAX_DATAGRAM_BYTES: usize = 2_048;

/// Who is allowed to speak to us, and where to reach them.
///
/// A separate small type rather than a borrow of `PunchAddrs` because the
/// send loop needs the mapping in BOTH directions — id to address to send,
/// address to id to receive — and the reverse lookup is the security check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Peers {
    /// (id, addr), kept as a vec: a call is a handful of people and a
    /// linear scan beats a hash for both directions at this size.
    entries: Vec<(String, PeerAddr)>,
}

impl Peers {
    pub fn new() -> Peers {
        Peers::default()
    }

    /// Replace the whole set. Called each time the app's view changes rather
    /// than incrementally, so a peer who left cannot linger as an authorised
    /// sender because one removal was missed.
    pub fn set(&mut self, entries: Vec<(String, PeerAddr)>) {
        self.entries = entries;
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn addr_of(&self, id: &str) -> Option<PeerAddr> {
        self.entries.iter().find(|(i, _)| i == id).map(|(_, a)| *a)
    }

    /// The reverse lookup, and the admission check.
    ///
    /// PORT INCLUDED, not just the IP. Two people behind one NAT share an
    /// address and differ only by port; matching on the IP alone would let
    /// either of them assert the other's floor, which is exactly the attack
    /// `voice_bridge`'s identity rule exists to prevent.
    pub fn id_at(&self, from: SocketAddr) -> Option<&str> {
        let SocketAddr::V4(v4) = from else { return None };
        let (ip, port) = (v4.ip().octets(), v4.port());
        self.entries.iter()
            .find(|(_, a)| a.ip == ip && a.port == port)
            .map(|(i, _)| i.as_str())
    }
}

/// Why a datagram was not delivered to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The source address is not one of the call's peers. Counted, not
    /// logged per packet: on a busy or hostile network this is the line that
    /// would fill the log and hide everything else.
    UnknownSender(SocketAddr, Vec<u8>),
    /// Zero bytes. A keepalive or a probe, not audio.
    Empty,
}

/// The voice socket.
pub struct VoiceSocket {
    sock: UdpSocket,
    buf: Vec<u8>,
    /// Datagrams refused since the last report, by reason count. A running
    /// total rather than a log line each, for the reason in `Refused`.
    pub refused_unknown: u64,
}

impl VoiceSocket {
    /// Bind for voice. Port 0 asks the OS for one, which is how tests get
    /// two of these talking.
    pub fn bind(port: u16) -> std::io::Result<VoiceSocket> {
        let sock = UdpSocket::bind(("0.0.0.0", port))?;
        sock.set_nonblocking(true)?;
        Ok(VoiceSocket { sock, buf: vec![0u8; MAX_DATAGRAM_BYTES], refused_unknown: 0 })
    }

    pub fn port(&self) -> std::io::Result<u16> {
        Ok(self.sock.local_addr()?.port())
    }

    /// Send one peer's payload. A failure is returned rather than retried:
    /// see the module docs on why a late voice frame is worse than a missing
    /// one.
    /// A datagram to an arbitrary address — STUN requests and punches,
    /// which go to servers and to addresses not yet in any peer table.
    pub fn send_raw(&self, payload: &[u8], to: SocketAddr) -> std::io::Result<()> {
        self.sock.send_to(payload, to).map(|_| ())
    }
    pub fn send_to_peer(&self, addr: PeerAddr, payload: &[u8]) -> std::io::Result<()> {
        let to = SocketAddr::from((addr.ip, addr.port));
        self.sock.send_to(payload, to).map(|_| ())
    }

    /// Send everything `voice_bridge::outbound` produced.
    ///
    /// Returns the peers with no known address, which is a real state worth
    /// surfacing: someone in the call whose punch address expired hears
    /// everyone and is heard by nobody, and silently dropping their packets
    /// makes that look like their microphone.
    pub fn send_all(&self, peers: &Peers, out: &[(String, Vec<u8>)]) -> Vec<String> {
        let mut unreachable = Vec::new();
        for (id, payload) in out {
            match peers.addr_of(id) {
                Some(a) => {
                    if self.send_to_peer(a, payload).is_err() {
                        unreachable.push(id.clone());
                    }
                }
                None => unreachable.push(id.clone()),
            }
        }
        unreachable
    }

    /// Drain up to `limit` datagrams, admitting only known senders.
    pub fn poll(&mut self, peers: &Peers, limit: usize)
        -> (Vec<(String, Vec<u8>)>, Vec<Refused>) {
        let mut ok = Vec::new();
        let mut refused = Vec::new();
        for _ in 0..limit {
            match self.sock.recv_from(&mut self.buf) {
                Ok((0, _)) => refused.push(Refused::Empty),
                Ok((n, from)) => match peers.id_at(from) {
                    Some(id) => ok.push((id.to_string(), self.buf[..n].to_vec())),
                    None => {
                        // Handed back WITH the bytes: a datagram from a
                        // source we do not know is exactly what a punch
                        // and a STUN reply look like (`nat_traversal`),
                        // and dropping them here would leave the
                        // traversal loop deaf.
                        self.refused_unknown += 1;
                        refused.push(Refused::UnknownSender(from, self.buf[..n].to_vec()));
                    }
                },
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                // Any other error ends this drain, not the socket: on some
                // platforms an ICMP port-unreachable from a peer who hung up
                // surfaces here, and it must not take the call down for
                // everyone else.
                Err(_) => break,
            }
        }
        (ok, refused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(port: u16) -> PeerAddr {
        PeerAddr::new([127, 0, 0, 1], port)
    }

    #[test]
    fn two_sockets_carry_a_frame_between_them() {
        let a = VoiceSocket::bind(0).expect("bind a");
        let mut b = VoiceSocket::bind(0).expect("bind b");
        let (pa, pb) = (a.port().unwrap(), b.port().unwrap());

        let mut a_peers = Peers::new();
        a_peers.set(vec![("sc-bbbbbbbb".into(), addr(pb))]);
        let mut b_peers = Peers::new();
        b_peers.set(vec![("sc-aaaaaaaa".into(), addr(pa))]);

        let unreachable = a.send_all(&a_peers, &[("sc-bbbbbbbb".into(), b"SCV1frame".to_vec())]);
        assert!(unreachable.is_empty(), "{unreachable:?}");

        // Non-blocking both ends, so give the loopback a moment rather than
        // asserting on the first poll — a flaky test here would be blamed on
        // the transport for months.
        let mut got = Vec::new();
        for _ in 0..100 {
            let (ok, _) = b.poll(&b_peers, 8);
            got.extend(ok);
            if !got.is_empty() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got.len(), 1, "nothing arrived");
        assert_eq!(got[0].0, "sc-aaaaaaaa", "the sender came from the address, as it must");
        assert_eq!(got[0].1, b"SCV1frame");
    }

    #[test]
    fn a_datagram_from_an_unknown_address_is_refused_not_attributed() {
        // The alternative — accepting it as "some peer" — hands an
        // unauthenticated sender the floor-assert path, which is the one
        // thing voice_bridge's identity rule exists to prevent.
        let a = VoiceSocket::bind(0).unwrap();
        let mut b = VoiceSocket::bind(0).unwrap();
        let empty = Peers::new();
        a.send_to_peer(addr(b.port().unwrap()), b"SCV1hello").unwrap();

        let mut refused = Vec::new();
        for _ in 0..100 {
            let (ok, r) = b.poll(&empty, 8);
            assert!(ok.is_empty(), "an unknown sender was admitted: {ok:?}");
            refused.extend(r);
            if !refused.is_empty() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(refused.first(), Some(Refused::UnknownSender(..))), "{refused:?}");
        assert_eq!(b.refused_unknown, 1, "the count is what a UI can show");
    }

    #[test]
    fn two_peers_behind_one_nat_are_told_apart_by_port() {
        // Matching on the IP alone would let either of them assert the
        // other's floor. Same address, different ports, different people.
        let mut peers = Peers::new();
        peers.set(vec![
            ("sc-aaaaaaaa".into(), PeerAddr::new([10, 0, 0, 7], 9876)),
            ("sc-bbbbbbbb".into(), PeerAddr::new([10, 0, 0, 7], 9877)),
        ]);
        assert_eq!(peers.id_at("10.0.0.7:9876".parse().unwrap()), Some("sc-aaaaaaaa"));
        assert_eq!(peers.id_at("10.0.0.7:9877".parse().unwrap()), Some("sc-bbbbbbbb"));
        assert_eq!(peers.id_at("10.0.0.7:1".parse().unwrap()), None,
                   "a third port on the same NAT is not either of them");
    }

    #[test]
    fn a_peer_with_no_known_address_is_reported_rather_than_dropped() {
        // Someone in the call whose punch address expired hears everyone and
        // is heard by nobody. Silently dropping their packets makes that look
        // like their microphone.
        let s = VoiceSocket::bind(0).unwrap();
        let peers = Peers::new();
        let out = vec![("sc-cccccccc".to_string(), vec![1, 2, 3])];
        assert_eq!(s.send_all(&peers, &out), vec!["sc-cccccccc".to_string()]);
    }

    #[test]
    fn replacing_the_peer_set_removes_authorisation() {
        // Incremental updates are how someone who left stays an authorised
        // sender because one removal was missed.
        let mut peers = Peers::new();
        peers.set(vec![("sc-aaaaaaaa".into(), addr(1))]);
        assert!(peers.addr_of("sc-aaaaaaaa").is_some());
        peers.set(vec![]);
        assert!(peers.addr_of("sc-aaaaaaaa").is_none());
        assert!(peers.id_at("127.0.0.1:1".parse().unwrap()).is_none());
    }
}
