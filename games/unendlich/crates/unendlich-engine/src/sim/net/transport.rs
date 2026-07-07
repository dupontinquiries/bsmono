//! Transport — the only part of the net stack that touches the outside world.
//!
//! Kept deliberately thin: a [`Transport`] is just "send these bytes" and
//! "give me a datagram if one is waiting, without blocking." The real
//! implementation ([`UdpTransport`]) is a few `std::net` calls and pulls in no
//! external crate, so the engine builds without a `Cargo.toml` change. The
//! protocol and session logic (which hold all the interesting behaviour) are
//! transport-agnostic and tested against [`loopback_pair`] instead of a
//! socket, so the untestable I/O surface stays minimal.

use std::collections::VecDeque;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Mutex;

use crate::sim::physics::WorldQuery;
use crate::sim::rollback::Rollback;

use super::session::{Ingested, Session};

/// A bidirectional, message-oriented, lossy channel to one peer.
pub trait Transport {
    /// Send one datagram. May silently drop (UDP); errors are real I/O faults.
    fn send(&self, bytes: &[u8]) -> io::Result<()>;
    /// Receive one waiting datagram into `buf`, returning its length, or
    /// `None` if nothing is queued. Must not block.
    fn try_recv(&self, buf: &mut [u8]) -> io::Result<Option<usize>>;
}

/// Real UDP transport: non-blocking socket bound locally, sending to a fixed
/// peer address. For the LAN ARM-to-ARM case, bind `0.0.0.0:PORT` and point
/// `peer` at the other machine.
pub struct UdpTransport {
    sock: UdpSocket,
    peer: SocketAddr,
}

impl UdpTransport {
    pub fn bind(local: impl ToSocketAddrs, peer: impl ToSocketAddrs) -> io::Result<UdpTransport> {
        let sock = UdpSocket::bind(local)?;
        sock.set_nonblocking(true)?;
        let peer = peer
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no peer address"))?;
        Ok(UdpTransport { sock, peer })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }
}

impl Transport for UdpTransport {
    fn send(&self, bytes: &[u8]) -> io::Result<()> {
        self.sock.send_to(bytes, self.peer).map(|_| ())
    }

    fn try_recv(&self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        match self.sock.recv_from(buf) {
            Ok((n, _)) => Ok(Some(n)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// In-memory transport for tests and single-process play: a pair whose sends
/// land in the other's receive queue. Lets the full session round-trip be
/// exercised without a socket.
pub struct LoopTransport {
    outbox: std::sync::Arc<Mutex<VecDeque<Vec<u8>>>>,
    inbox: std::sync::Arc<Mutex<VecDeque<Vec<u8>>>>,
}

/// Create two connected [`LoopTransport`]s: bytes sent on one arrive on the other.
pub fn loopback_pair() -> (LoopTransport, LoopTransport) {
    let a = std::sync::Arc::new(Mutex::new(VecDeque::new()));
    let b = std::sync::Arc::new(Mutex::new(VecDeque::new()));
    (
        LoopTransport { outbox: a.clone(), inbox: b.clone() },
        LoopTransport { outbox: b, inbox: a },
    )
}

impl Transport for LoopTransport {
    fn send(&self, bytes: &[u8]) -> io::Result<()> {
        self.outbox.lock().unwrap().push_back(bytes.to_vec());
        Ok(())
    }

    fn try_recv(&self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        match self.inbox.lock().unwrap().pop_front() {
            Some(v) => {
                let n = v.len().min(buf.len());
                buf[..n].copy_from_slice(&v[..n]);
                Ok(Some(n))
            }
            None => Ok(None),
        }
    }
}

/// Drain every datagram currently waiting on `transport` into `session`
/// (feeding remote inputs into `rb`). Returns the outcomes, in order.
pub fn drain<T: Transport, W: WorldQuery>(
    transport: &T,
    session: &mut Session,
    rb: &mut Rollback,
    world: &W,
) -> io::Result<Vec<Ingested>> {
    let mut buf = [0u8; 2048];
    let mut out = Vec::new();
    while let Some(n) = transport.try_recv(&mut buf)? {
        out.push(session.ingest(&buf[..n], rb, world));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Fixed;
    use crate::sim::physics::FlatWorld;
    use crate::sim::state::{Input, SimState};

    #[test]
    fn loopback_delivers_in_order_and_drains_empty() {
        let (a, b) = loopback_pair();
        a.send(b"first").unwrap();
        a.send(b"second").unwrap();

        let mut buf = [0u8; 64];
        let n = b.try_recv(&mut buf).unwrap().unwrap();
        assert_eq!(&buf[..n], b"first");
        let n = b.try_recv(&mut buf).unwrap().unwrap();
        assert_eq!(&buf[..n], b"second");
        assert_eq!(b.try_recv(&mut buf).unwrap(), None);
    }

    #[test]
    fn drain_feeds_a_full_session_round_trip() {
        // Two peers, each its own transport end, exchanging real packets.
        let world = FlatWorld::new();
        let (net0, net1) = loopback_pair();
        let mut s0 = Session::new(0, 2, 8);
        let mut s1 = Session::new(1, 2, 8);
        let mut rb0 = Rollback::new(2, SimState::new(2), 12);
        let mut rb1 = Rollback::new(2, SimState::new(2), 12);

        let fwd = |mx: i64| Input { buttons: 0, move_x: Fixed::from_int(mx), move_z: Fixed::ZERO, yaw: 0 };

        // A peer with nothing left to send returns `None` from
        // outgoing_input_packet — that's correct once everything is confirmed,
        // so we send only when there's a packet.
        for t in 0..50u64 {
            // local inputs
            s0.record_local(t, fwd(1));
            rb0.set_local_input(0, fwd(1), &world);
            s1.record_local(t, fwd(-1));
            rb1.set_local_input(1, fwd(-1), &world);
            // exchange
            if let Some(p) = s0.outgoing_input_packet(rb0.confirmed_tick()) {
                net0.send(&p).unwrap();
            }
            if let Some(p) = s1.outgoing_input_packet(rb1.confirmed_tick()) {
                net1.send(&p).unwrap();
            }
            drain(&net0, &mut s0, &mut rb0, &world).unwrap();
            drain(&net1, &mut s1, &mut rb1, &world).unwrap();
            rb0.advance(&world);
            rb1.advance(&world);
        }
        // flush tails
        for _ in 0..12 {
            if let Some(p) = s0.outgoing_input_packet(rb0.confirmed_tick()) {
                net0.send(&p).unwrap();
            }
            if let Some(p) = s1.outgoing_input_packet(rb1.confirmed_tick()) {
                net1.send(&p).unwrap();
            }
            drain(&net0, &mut s0, &mut rb0, &world).unwrap();
            drain(&net1, &mut s1, &mut rb1, &world).unwrap();
            rb0.advance(&world);
            rb1.advance(&world);
        }

        // Both peers converge to the same confirmed state — the whole point.
        assert_eq!(rb0.confirmed_tick(), rb1.confirmed_tick());
        assert_eq!(rb0.confirmed_checksum(), rb1.confirmed_checksum());
        assert!(rb0.confirmed_tick() >= 50, "session confirmed the full run");
    }
}
