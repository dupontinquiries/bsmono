//! snap_transport — the socket that finally moves a snap between devices.
//!
//! `snap_wire` is the codec; this is the thing that runs it. One TCP
//! connection carries one snap and closes. That is deliberately the simple
//! shape: session reuse, delta sync and the ack/seq machinery `wire_v3`
//! models are worth having and are not worth having *before* a snap has
//! crossed a wire once. The frame already carries seq and ack, so adding
//! them later does not change the format.
//!
//! ## Identity here is weaker than in voice, and the difference matters
//!
//! `voice_wire` matches a datagram to a peer by IP **and port**, because two
//! people behind one NAT differ only by port. An inbound TCP connection has
//! an EPHEMERAL source port — it tells you nothing about who dialled — so
//! this can only match on IP.
//!
//! The consequence is stated rather than papered over: when two known peers
//! share an address, an inbound snap cannot be attributed to either of them,
//! and [`resolve_sender`] returns [`Sender::Ambiguous`] so the caller can
//! refuse it. Guessing would let one flatmate post snaps as another. The
//! real fix is authenticating the connection — `handshake` — and until that
//! exists, refusing is the honest behaviour.
//!
//! ## Nothing here is encrypted
//!
//! Same caveat as the codec: `Snap.bytes` is "the decrypted bytes" and there
//! is no layer decrypting them yet. A snap crosses the network in the clear.
//! Said here, in the module that puts it on the wire, so it cannot be missed
//! by someone reading only the transport.

use std::io::Write;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::punch_addrs::PeerAddr;
use crate::snap_wire::{self, SnapFrame};

/// How long to wait on a dial or a stalled read before giving up. A peer
/// that accepted a connection and then went quiet must not hold a thread
/// for the life of the process.
pub const TIMEOUT: Duration = Duration::from_secs(20);

/// Who sent an inbound connection, as far as the address can say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sender {
    /// Exactly one known peer has this address.
    Known(String),
    /// More than one known peer shares it — see the module docs. Refused
    /// rather than guessed.
    Ambiguous(Vec<String>),
    /// Nobody known is at this address.
    Unknown,
}

/// Match an inbound connection's address against known peers.
///
/// IP only, and the port is deliberately ignored: an inbound TCP source
/// port is ephemeral and says nothing about who dialled.
pub fn resolve_sender(ip: IpAddr, known: &[(String, PeerAddr)]) -> Sender {
    let IpAddr::V4(v4) = ip else { return Sender::Unknown };
    let octets = v4.octets();
    let hits: Vec<String> = known.iter()
        .filter(|(_, a)| a.ip == octets)
        .map(|(id, _)| id.clone())
        .collect();
    match hits.len() {
        0 => Sender::Unknown,
        1 => Sender::Known(hits.into_iter().next().expect("len checked")),
        _ => Sender::Ambiguous(hits),
    }
}

/// A snap that arrived, with the address it arrived from. Attribution is the
/// caller's, because it needs the peer list and this thread must not borrow
/// the app.
#[derive(Debug, Clone)]
pub struct Arrival {
    pub from: IpAddr,
    pub frame: SnapFrame,
}

/// What the listener thread and the UI thread share.
#[derive(Default)]
pub struct Inbox {
    arrivals: Mutex<Vec<Arrival>>,
    /// Connections that produced no readable frame, counted rather than
    /// logged: a port scanner would otherwise fill the log.
    rejected: Mutex<u64>,
    error: Mutex<Option<String>>,
}

/// A cap on how many undelivered snaps may pile up. Beyond this the listener
/// refuses rather than growing: the UI drains every frame, so a backlog this
/// deep means it is not running, and buffering megabytes for a frame loop
/// that has stopped only turns one problem into an out-of-memory.
pub const MAX_PENDING: usize = 32;

impl Inbox {
    pub fn new() -> Arc<Inbox> {
        Arc::new(Inbox::default())
    }

    /// Everything that arrived since the last call.
    pub fn drain(&self) -> Vec<Arrival> {
        self.arrivals.lock().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default()
    }

    pub fn rejected(&self) -> u64 {
        self.rejected.lock().map(|r| *r).unwrap_or(0)
    }

    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok()?.take()
    }

    fn push(&self, a: Arrival) -> bool {
        let Ok(mut q) = self.arrivals.lock() else { return false };
        if q.len() >= MAX_PENDING {
            return false;
        }
        q.push(a);
        true
    }

    fn note_rejected(&self) {
        if let Ok(mut r) = self.rejected.lock() {
            *r += 1;
        }
    }
}

/// The listening half. Owns a thread; stopped by dropping the flag and
/// dialling ourselves, since `accept` blocks.
pub struct Listener {
    pub inbox: Arc<Inbox>,
    stop: Arc<AtomicBool>,
    port: u16,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Listener {
    /// Bind and start accepting.
    pub fn start(port: u16) -> std::io::Result<Listener> {
        let sock = TcpListener::bind(("0.0.0.0", port))?;
        let port = sock.local_addr()?.port();
        let inbox = Inbox::new();
        let stop = Arc::new(AtomicBool::new(false));
        let (i2, s2) = (inbox.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            for conn in sock.incoming() {
                if s2.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(mut c) = conn else { continue };
                // A peer that connects and then goes quiet must not hold
                // this thread for the life of the process.
                let _ = c.set_read_timeout(Some(TIMEOUT));
                let from = match c.peer_addr() {
                    Ok(a) => a.ip(),
                    Err(_) => continue,
                };
                match snap_wire::read_frame(&mut c) {
                    Ok(frame) => {
                        if !i2.push(Arrival { from, frame }) {
                            i2.note_rejected();
                        }
                    }
                    Err(_) => i2.note_rejected(),
                }
            }
        });
        Ok(Listener { inbox, stop, port, thread: Some(thread) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stop accepting. `accept` blocks, so the flag alone is not enough —
    /// the same two-part stop contract as `capture` and the voice rig, with
    /// a self-dial standing in for the kill.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], self.port)), Duration::from_millis(200));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.stop();
        }
    }
}

/// Dial a peer and hand over one snap.
///
/// Synchronous and short: the caller runs it on a worker, not the frame
/// loop, because a dial to an unreachable phone costs the whole timeout.
pub fn send_snap(addr: PeerAddr, frame: &SnapFrame) -> Result<(), String> {
    let to = SocketAddr::from((addr.ip, addr.port));
    let mut s = TcpStream::connect_timeout(&to, TIMEOUT)
        .map_err(|e| format!("could not reach {to}: {e}"))?;
    s.set_write_timeout(Some(TIMEOUT)).ok();
    snap_wire::write_frame(&mut s, frame).map_err(|e| format!("{to}: {e:?}"))?;
    // Shut the write side down explicitly so the receiver sees the end of
    // the frame rather than waiting on a connection that is merely idle.
    let _ = s.flush();
    let _ = s.shutdown(std::net::Shutdown::Write);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snaps::Media;

    fn addr(ip: [u8; 4], port: u16) -> PeerAddr {
        PeerAddr::new(ip, port)
    }

    fn frame(payload: &[u8]) -> SnapFrame {
        SnapFrame::announce(1, 0, "sc-aabbccdd", "kit", Media::Image, 1_700_000_000, 5,
                            payload.to_vec())
    }

    #[test]
    fn a_snap_crosses_a_real_socket_and_arrives_whole() {
        // The end this app has never had: a snap leaving one device and
        // landing on another.
        let mut l = Listener::start(0).expect("bind");
        let port = l.port();
        let sent = frame(&(0..50_000u32).map(|i| i as u8).collect::<Vec<u8>>());
        send_snap(addr([127, 0, 0, 1], port), &sent).expect("send");

        let mut got = Vec::new();
        for _ in 0..200 {
            got = l.inbox.drain();
            if !got.is_empty() { break; }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got.len(), 1, "nothing arrived");
        assert_eq!(got[0].frame, sent);
        l.stop();
    }

    #[test]
    fn a_connection_carrying_nothing_readable_is_counted_not_crashed() {
        // A port scanner, a wrong-protocol client, a truncated send. None of
        // these may take the listener down, and none deserves a log line
        // each.
        let mut l = Listener::start(0).expect("bind");
        {
            let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], l.port())))
                .expect("connect");
            s.write_all(b"GET / HTTP/1.1\r\n\r\n").ok();
        }
        for _ in 0..200 {
            if l.inbox.rejected() > 0 { break; }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(l.inbox.rejected(), 1, "the junk connection was not counted");
        assert!(l.inbox.drain().is_empty(), "junk reached the feed");

        // Still alive afterwards, which is the actual property.
        let sent = frame(b"real one");
        send_snap(addr([127, 0, 0, 1], l.port()), &sent).expect("send after junk");
        let mut got = Vec::new();
        for _ in 0..200 {
            got = l.inbox.drain();
            if !got.is_empty() { break; }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got.len(), 1, "the listener died on the junk connection");
        l.stop();
    }

    #[test]
    fn two_peers_behind_one_address_are_refused_rather_than_guessed() {
        // TCP source ports are ephemeral, so unlike voice this cannot tell
        // them apart. Guessing would let one flatmate post snaps as another.
        let known = vec![
            ("sc-aaaaaaaa".to_string(), addr([10, 0, 0, 7], 9876)),
            ("sc-bbbbbbbb".to_string(), addr([10, 0, 0, 7], 9877)),
            ("sc-cccccccc".to_string(), addr([10, 0, 0, 8], 9876)),
        ];
        assert_eq!(resolve_sender("10.0.0.8".parse().unwrap(), &known),
                   Sender::Known("sc-cccccccc".into()));
        match resolve_sender("10.0.0.7".parse().unwrap(), &known) {
            Sender::Ambiguous(ids) => assert_eq!(ids.len(), 2, "{ids:?}"),
            other => panic!("two peers at one address resolved to {other:?}"),
        }
        assert_eq!(resolve_sender("10.0.0.9".parse().unwrap(), &known), Sender::Unknown);
    }

    #[test]
    fn a_backlog_is_refused_rather_than_grown() {
        // The UI drains every frame, so a backlog this deep means it is not
        // running. Buffering megabytes for a stopped frame loop turns one
        // problem into an out-of-memory.
        let inbox = Inbox::new();
        for i in 0..(MAX_PENDING + 5) {
            let ok = inbox.push(Arrival {
                from: "127.0.0.1".parse().unwrap(),
                frame: frame(&[i as u8]),
            });
            assert_eq!(ok, i < MAX_PENDING, "the cap was not enforced at {i}");
        }
        assert_eq!(inbox.drain().len(), MAX_PENDING);
    }

    #[test]
    fn an_unreachable_peer_reports_rather_than_hanging() {
        // Port 1 on localhost refuses immediately, which is the fast version
        // of the failure; the slow one is bounded by TIMEOUT.
        let e = send_snap(addr([127, 0, 0, 1], 1), &frame(b"x")).unwrap_err();
        assert!(e.contains("could not reach"), "{e}");
    }

    #[test]
    fn stopping_the_listener_does_not_leave_the_thread_in_accept() {
        // `accept` blocks, so the flag alone would leave a thread parked
        // until the next connection — which on an idle network is forever.
        let mut l = Listener::start(0).expect("bind");
        l.stop();
        assert!(l.thread.is_none(), "the accept thread was never joined");
    }
}
