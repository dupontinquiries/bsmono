//! `sim::lan_discovery` — UDP multicast transport for LAN host discovery.
//!
//! The I/O companion to [`session_wire`](crate::sim::session_wire): that module
//! turns a [`WorldAdvert`] into a datagram and back; this one puts the datagrams
//! on the wire. A host calls [`LanBeacon::announce`] on a timer; every running
//! instance calls [`LanBeacon::poll`] each frame and feeds the results to
//! `session_mode::ServerBrowser::observe`, so the Join screen fills with the
//! hosts visible on the local network. Modelled on the reference P2P stack's
//! gossip layer (periodic JOIN beacons on a multicast group).
//!
//! This reaches hosts on the **same LAN** only. Crossing NAT to the wider
//! internet needs a DHT + hole-punching layer (the reference uses OpenDHT); that
//! is a separate, larger build and is intentionally not attempted here.
//!
//! The socket is non-blocking, so `poll` never stalls the game loop, and all
//! parsing goes through `session_wire` (size-capped, malformed-rejecting), so a
//! hostile packet can't panic or exhaust memory.

use crate::sim::session_mode::WorldAdvert;
use crate::sim::session_wire;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

/// UDP port for the gossip beacon (matches the reference's gossip port).
pub const GOSSIP_PORT: u16 = 9877;

/// Multicast group for beacons. 239.x.x.x is the locally-administered scope, so
/// this won't collide with mDNS (224.0.0.251) or other well-known groups.
pub const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 77, 1);

/// A bound multicast socket for announcing and discovering LAN hosts.
pub struct LanBeacon {
    sock: UdpSocket,
}

impl LanBeacon {
    /// Bind the gossip socket, join the multicast group, and go non-blocking.
    /// Returns an `io::Error` if the port is taken or multicast is unavailable —
    /// the caller should treat that as "LAN discovery off" rather than fatal.
    pub fn bind() -> std::io::Result<LanBeacon> {
        let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, GOSSIP_PORT))?;
        sock.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
        // Hear our own beacons too — harmless (the browser dedupes by key) and it
        // lets a single host verify its beacon is live.
        let _ = sock.set_multicast_loop_v4(true);
        sock.set_nonblocking(true)?;
        Ok(LanBeacon { sock })
    }

    /// Multicast one JOIN beacon for `advert`. Cheap; call on a timer (~1/sec)
    /// while hosting.
    pub fn announce(&self, advert: &WorldAdvert) -> std::io::Result<()> {
        let pkt = session_wire::encode_join(advert);
        self.sock
            .send_to(&pkt, SocketAddrV4::new(GROUP, GOSSIP_PORT))
            .map(|_| ())
    }

    /// Drain every pending datagram, returning the adverts that parsed (stamped
    /// `now_ms` as `last_seen_ms`). Never blocks; silently drops malformed or
    /// oversized packets. Feed the results straight to `ServerBrowser::observe`.
    pub fn poll(&self, now_ms: u64) -> Vec<WorldAdvert> {
        let mut out = Vec::new();
        let mut buf = [0u8; session_wire::MAX_DATAGRAM];
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, _src)) => {
                    if let Some(a) = session_wire::parse(&buf[..n], now_ms) {
                        out.push(a);
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        out
    }
}

/// Best-effort discovery of this host's primary LAN IPv4. Opens a UDP socket and
/// "connects" it to an off-subnet address — no packet is sent; `connect` on a UDP
/// socket only sets the default peer and makes the OS choose the source IP of the
/// default route, which is this machine's LAN address. Returns `None` if there's
/// no routable IPv4 (loopback-only host).
pub fn local_lan_ipv4() -> Option<[u8; 4]> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    // 192.0.2.1 is TEST-NET-1 (RFC 5737) — reserved, never actually routed.
    sock.connect("192.0.2.1:9").ok()?;
    match sock.local_addr().ok()? {
        std::net::SocketAddr::V4(a) if !a.ip().is_loopback() && !a.ip().is_unspecified() => {
            Some(a.ip().octets())
        }
        _ => None,
    }
}

/// Build a LAN connection candidate for this host's `ip:port` (the endpoint a
/// joiner on the same network connects to). Always [`CandKind::Lan`].
pub fn lan_candidate(host: [u8; 4], port: u16) -> crate::sim::net_candidates::Candidate {
    crate::sim::net_candidates::Candidate {
        host,
        port,
        kind: crate::sim::net_candidates::CandKind::Lan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::net_candidates::CandKind;

    #[test]
    fn lan_candidate_carries_addr_and_lan_kind() {
        let c = lan_candidate([192, 168, 1, 5], 47777);
        assert_eq!(c.host, [192, 168, 1, 5]);
        assert_eq!(c.port, 47777);
        assert_eq!(c.kind, CandKind::Lan);
    }
}
