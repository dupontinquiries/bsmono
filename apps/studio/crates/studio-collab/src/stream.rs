//! stream — a TCP carrier for the packet seam.
//!
//! The Doc, the Session, and the shell all speak `Packet`s through Vecs and
//! return values; this module is the first thing that puts those packets on
//! an actual wire. It is deliberately dumb pipes: two threads per
//! connection (read and write), `Packet::frame`/`Packet::drain` for the
//! bytes, and `mpsc` channels facing the shell — so the shell's `Studio`
//! stays single-threaded and lock-free around the Doc, exactly as its
//! convergence tests exercise it. The carrier never LOOKS at a packet.
//!
//! ## What this is and is not
//!
//! One peer per `StreamPeer`, no reconnect, no discovery, no encryption —
//! the two-people-on-one-beat wire, stated plainly. The datagram shape for
//! the DHT transport lives in [`crate::carrier`]; discovery's network half
//! stays [`crate::dht`]'s named future. When those arrive they will drain
//! the same outbox this does, which is the point of the seam.
//!
//! ## Ordering and loss
//!
//! TCP gives in-order, lossless delivery per connection, but nothing here
//! RELIES on it: ops integrate idempotently in any order, chunks are
//! content-addressed, and rack syncs are LWW. That is why the shell sends
//! `Hello { since: 0 }` on every (re)connect and simply over-fetches — the
//! Doc's `seen` set makes redundancy free, whereas under-asking after a
//! dropped connection would lose edits.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc;

use crate::wire::Packet;

/// What the carrier tells the shell about the connection itself. Everything
/// else it carries is `Packet`s it does not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    Connected(SocketAddr),
    /// The reason, for the status line — a connection that vanishes silently
    /// reads as a hang.
    Disconnected(String),
}

pub struct StreamPeer {
    to_peer: mpsc::Sender<Packet>,
    from_peer: mpsc::Receiver<Packet>,
    pub events: mpsc::Receiver<PeerEvent>,
    /// The locally bound address — what a listener tells the other person to
    /// type. (With port 0 the OS picks; this is where to learn what it chose.)
    pub local: Option<SocketAddr>,
}

impl StreamPeer {
    /// Dial a peer. The sockets and threads start now; `PeerEvent::Connected`
    /// arrives once the stream is up.
    pub fn connect<A: ToSocketAddrs>(addr: A) -> std::io::Result<StreamPeer> {
        let stream = TcpStream::connect(addr)?;
        let local = stream.local_addr().ok();
        let (peer, tx_pkts, tx_events, writer_rx) = Self::shell_side(local);
        Self::spawn_io(stream, tx_pkts, tx_events, writer_rx);
        Ok(peer)
    }

    /// Bind and wait (on a thread) for ONE peer. Packets sent before anyone
    /// arrives simply queue in the channel — the writer starts after accept
    /// and drains the backlog, so "listen, then edit while waiting" works.
    pub fn listen<A: ToSocketAddrs>(addr: A) -> std::io::Result<StreamPeer> {
        let listener = TcpListener::bind(addr)?;
        let local = listener.local_addr().ok();
        let (peer, tx_pkts, tx_events, writer_rx) = Self::shell_side(local);
        std::thread::spawn(move || match listener.accept() {
            Ok((stream, _)) => Self::spawn_io(stream, tx_pkts, tx_events, writer_rx),
            Err(e) => { let _ = tx_events.send(PeerEvent::Disconnected(format!("accept: {e}"))); }
        });
        Ok(peer)
    }

    /// Queue a packet for the peer. `false` once the connection is gone.
    pub fn send(&self, p: Packet) -> bool {
        self.to_peer.send(p).is_ok()
    }

    /// Everything that has arrived since the last poll.
    pub fn poll(&self) -> Vec<Packet> {
        self.from_peer.try_iter().collect()
    }

    // ── plumbing ─────────────────────────────────────────────────────────

    #[allow(clippy::type_complexity)]
    fn shell_side(local: Option<SocketAddr>)
        -> (StreamPeer, mpsc::Sender<Packet>, mpsc::Sender<PeerEvent>, mpsc::Receiver<Packet>)
    {
        let (tx_pkts, from_peer) = mpsc::channel();
        let (tx_events, events) = mpsc::channel();
        let (to_peer, writer_rx) = mpsc::channel();
        (StreamPeer { to_peer, from_peer, events, local }, tx_pkts, tx_events, writer_rx)
    }

    fn spawn_io(
        stream: TcpStream,
        tx_pkts: mpsc::Sender<Packet>,
        tx_events: mpsc::Sender<PeerEvent>,
        writer_rx: mpsc::Receiver<Packet>,
    ) {
        if let Ok(addr) = stream.peer_addr() {
            let _ = tx_events.send(PeerEvent::Connected(addr));
        }
        let write_half = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                let _ = tx_events.send(PeerEvent::Disconnected(format!("clone: {e}")));
                return;
            }
        };
        // Writer: frames whatever the shell queued, in order. On ANY exit —
        // the shell dropped its StreamPeer (channel closed) or the write
        // failed — the socket is shut down, not merely abandoned: an
        // abandoned socket leaves the far side blocked in read() forever,
        // which is exactly how the dropped-peer test first failed.
        std::thread::spawn(move || {
            let mut w = write_half;
            while let Ok(p) = writer_rx.recv() {
                if w.write_all(&p.frame()).is_err() {
                    break;
                }
            }
            let _ = w.shutdown(std::net::Shutdown::Both);
        });
        // Reader: grow a buffer, peel whole frames with Packet::drain — a
        // packet that fails to decode is drain's problem (skipped, stream
        // stays in sync), a closed socket is ours.
        let events = tx_events;
        std::thread::spawn(move || {
            let mut s = stream;
            let mut buf: Vec<u8> = Vec::new();
            let mut tmp = [0u8; 16 * 1024];
            loop {
                match s.read(&mut tmp) {
                    Ok(0) => {
                        let _ = events.send(PeerEvent::Disconnected("peer closed".into()));
                        return;
                    }
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        let (pkts, used) = Packet::drain(&buf);
                        buf.drain(..used);
                        for p in pkts {
                            if tx_pkts.send(p).is_err() {
                                // Shell went away mid-read: same rule as the
                                // writer — tell the wire, don't ghost it.
                                let _ = s.shutdown(std::net::Shutdown::Both);
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = events.send(PeerEvent::Disconnected(e.to_string()));
                        return;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::SourceId;

    fn localhost_pair() -> (StreamPeer, StreamPeer) {
        let a = StreamPeer::listen("127.0.0.1:0").expect("bind");
        let addr = a.local.expect("bound addr");
        let b = StreamPeer::connect(addr).expect("dial");
        (a, b)
    }

    fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> T {
        for _ in 0..200 {
            if let Some(v) = f() { return v; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("timed out");
    }

    #[test]
    fn packets_cross_a_real_socket_both_ways() {
        let (a, b) = localhost_pair();
        assert!(a.send(Packet::AssetRequest { source: SourceId(1) }));
        assert!(b.send(Packet::AssetRequest { source: SourceId(2) }));
        let at_b = wait_for(|| b.poll().pop());
        let at_a = wait_for(|| a.poll().pop());
        assert_eq!(at_b, Packet::AssetRequest { source: SourceId(1) });
        assert_eq!(at_a, Packet::AssetRequest { source: SourceId(2) });
        assert!(a.events.try_iter().any(|e| matches!(e, PeerEvent::Connected(_))));
    }

    #[test]
    fn a_big_packet_survives_being_split_across_reads() {
        // A chunk bigger than the 16K read buffer forces the partial-frame
        // path: several reads accumulate before drain can peel the packet.
        let (a, b) = localhost_pair();
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let id = crate::assets::hash(&bytes);
        a.send(Packet::Chunk { id, bytes: bytes.clone() });
        let got = wait_for(|| b.poll().pop());
        assert_eq!(got, Packet::Chunk { id, bytes });
    }

    #[test]
    fn packets_queued_before_the_peer_arrives_are_delivered_after() {
        let a = StreamPeer::listen("127.0.0.1:0").expect("bind");
        let addr = a.local.unwrap();
        a.send(Packet::AssetRequest { source: SourceId(9) }); // nobody there yet
        let b = StreamPeer::connect(addr).expect("dial");
        let got = wait_for(|| b.poll().pop());
        assert_eq!(got, Packet::AssetRequest { source: SourceId(9) },
                   "listen-then-edit works: the writer drains the backlog");
    }

    #[test]
    fn a_dropped_peer_is_reported_not_hung() {
        let (a, b) = localhost_pair();
        drop(b);
        let ev = wait_for(|| a.events.try_iter()
            .find(|e| matches!(e, PeerEvent::Disconnected(_))));
        match ev {
            PeerEvent::Disconnected(reason) => assert!(!reason.is_empty()),
            other => panic!("expected a disconnect, got {other:?}"),
        }
    }
}
