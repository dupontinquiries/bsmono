//! lanwire.rs — the UDP socket that carries `announce` datagrams on a local network.
//!
//! Deliberately thin. Every decision — what a datagram means, whether to believe it, which address to
//! prefer — was made in `announce`, `rendezvous` and `trustkit` and tested without a socket. This binds,
//! sends and receives, and nothing else, because socket code is the part that cannot be exercised
//! properly in CI and should therefore contain no judgement.
//!
//! ## Why multicast rather than broadcast
//!
//! Broadcast reaches every host on the subnet including ones that will never care; multicast reaches
//! only sockets that joined the group. On a phone that difference is battery.
//!
//! ## What this does NOT do
//!
//! It does not retry, back off, or keep state about peers. A discovery datagram is idempotent — the
//! next one supersedes it — so a lost packet costs one interval and nothing more. Adding reliability
//! here would be building TCP badly on top of a protocol chosen for not needing it.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};

/// The group logscope announces on.
///
/// 239.x is the administratively-scoped range — routers do not forward it off the local network, which
/// is the property wanted: a discovery datagram should not leave the building.
pub const GROUP: Ipv4Addr = Ipv4Addr::new(239, 77, 33, 1);
pub const PORT: u16 = 47733;

/// How often to announce.
///
/// A peer that misses one datagram waits this long. Short enough that plugging a laptop in feels
/// immediate; long enough that a dozen machines are not a background load.
pub const INTERVAL_S: u64 = 5;

/// Largest datagram accepted.
///
/// An announcement is well under this. A cap exists because a UDP read into an unbounded buffer is how
/// a listener is made to allocate on a stranger's say-so.
pub const MAX_DATAGRAM: usize = 512;

/// A bound discovery socket.
pub struct LanWire {
    sock: UdpSocket,
    dest: SocketAddr,
}

impl LanWire {
    /// Bind and join the group.
    ///
    /// `SO_REUSEADDR` is not set: two logscopes on one machine is a mistake worth surfacing rather than
    /// quietly permitting, since they would fight over the same identity file.
    pub fn bind() -> std::io::Result<LanWire> {
        let sock = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT))?;
        sock.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
        // Non-blocking: the caller polls from its own loop rather than this owning a thread. A
        // discovery socket that blocks is one that stalls a UI frame.
        sock.set_nonblocking(true)?;
        Ok(LanWire { sock, dest: SocketAddr::from(SocketAddrV4::new(GROUP, PORT)) })
    }

    /// Send one announcement. `sig` is the signature over `a.payload()`.
    pub fn announce(&self, a: &crate::announce::Announce, sig: &[u8; 64]) -> std::io::Result<()> {
        let mut buf = a.payload();
        buf.extend_from_slice(sig);
        if buf.len() > MAX_DATAGRAM {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput,
                "announcement exceeds the datagram cap"));
        }
        self.sock.send_to(&buf, self.dest).map(|_| ())
    }

    /// Drain whatever has arrived. Returns `(body, signature, from)` triples, unparsed.
    ///
    /// Returns raw bytes rather than parsed announcements ON PURPOSE: parsing and believing are
    /// `announce`'s job, and a socket layer that decides what is valid is a socket layer that has to be
    /// tested with a socket.
    pub fn drain(&self) -> Vec<(Vec<u8>, [u8; 64], SocketAddr)> {
        let mut out = Vec::new();
        let mut buf = [0u8; MAX_DATAGRAM];
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) if n > 64 => {
                    let mut sig = [0u8; 64];
                    sig.copy_from_slice(&buf[n - 64..n]);
                    out.push((buf[..n - 64].to_vec(), sig, from));
                }
                // Anything at or under the signature length cannot contain a body; dropped silently
                // because a malformed datagram from a stranger is not an event worth logging per packet.
                Ok(_) => continue,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_group_is_administratively_scoped() {
        // 239.x is not forwarded off the local network by routers, which is the property wanted: a
        // discovery datagram should not leave the building.
        assert_eq!(GROUP.octets()[0], 239);
        assert!(GROUP.is_multicast());
    }

    #[test]
    fn the_datagram_cap_bounds_a_strangers_allocation() {
        // A UDP read into an unbounded buffer is how a listener is made to allocate on someone else's
        // say-so. The cap must still comfortably hold a real announcement plus its signature.
        let a = crate::announce::Announce {
            id: "aaaaa-bbbbb-ccccc-dd".into(),
            addr: "255.255.255.255:65535".into(),
            sent_at_s: u64::MAX,
        };
        assert!(a.payload().len() + 64 < MAX_DATAGRAM,
            "a maximal announcement is {} bytes against a {MAX_DATAGRAM} cap", a.payload().len() + 64);
    }

    #[test]
    fn the_interval_is_short_enough_to_feel_immediate() {
        // A peer that misses one datagram waits this long before the next chance.
        assert!(INTERVAL_S <= 10);
    }

    /// Round-trips a datagram through a real socket pair on loopback.
    ///
    /// Ignored by default: multicast on loopback depends on the container's network configuration, and
    /// a test that fails for that reason teaches people to ignore failures. Run it explicitly on a
    /// machine where the flow is being brought up — which is the only place its result means anything.
    #[test]
    #[ignore]
    fn a_datagram_survives_a_real_socket() {
        let wire = LanWire::bind().expect("bind");
        let a = crate::announce::Announce {
            id: "test-machine".into(), addr: "127.0.0.1:7000".into(), sent_at_s: 42,
        };
        wire.announce(&a, &[7u8; 64]).expect("send");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let got = wire.drain();
        let found = got.iter().any(|(body, sig, _)| {
            crate::announce::Announce::parse(body).map(|p| p == a).unwrap_or(false)
                && *sig == [7u8; 64]
        });
        assert!(found, "sent one announcement and drained {} datagrams", got.len());
    }
}
