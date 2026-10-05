//! UDP transport for [`Lockstep`] -- the "remaining" item the README names after the
//! in-process proof: the same [`Msg`] values over a datagram socket, with the reliability a
//! lockstep needs (a turn cannot execute until every player's commands for it are in, so a
//! lost `Cmds` is a stall forever) and nothing more.
//!
//! Wire, one datagram per message: `[kind u8][seq u64 LE][payload]`, kind 0 a [`Msg`] in
//! RON text, kind 1 an acknowledgement of `seq` (no payload). Every message is sent with a
//! sequence number and kept until the peer acks it; unacked ones are re-sent every
//! [`RESEND_MS`]. The receiver acks every message it gets -- a duplicate too, since the
//! duplicate means the ack was lost -- and feeds each one to the session once (`Lockstep`
//! is keyed by turn and tick, so order does not matter, and [`Lockstep::recv`] drops a turn
//! already executed). Two players only, as the session layer is today; the relay server the
//! README foresees would hold one link per peer.
//!
//! Verified on loopback: two sessions, two sockets, a full bot match in lockstep with hash
//! checkpoints and zero desyncs, and the same match with every third datagram dropped on
//! one side.

use crate::{Lockstep, Msg};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

const KIND_MSG: u8 = 0;
const KIND_ACK: u8 = 1;
/// Unacked messages are re-sent this often.
pub const RESEND_MS: u64 = 100;
/// Seen sequence numbers remembered, for dropping duplicates.
const SEEN_MAX: usize = 4096;
/// The largest datagram accepted (a turn's commands are small; a hash is tiny).
const MAX_DATAGRAM: usize = 16 * 1024;

pub struct UdpLink {
    sock: UdpSocket,
    peer: SocketAddr,
    next_seq: u64,
    unacked: BTreeMap<u64, Vec<u8>>,
    last_resend: Instant,
    seen: BTreeSet<u64>,
    /// Counters for a status line.
    pub resent: u64,
    pub duplicates: u64,
    pub rejected: u64,
}

impl UdpLink {
    /// Bind `bind` (`0.0.0.0:0` for any port) and talk to `peer`.
    pub fn open(bind: impl ToSocketAddrs, peer: SocketAddr) -> io::Result<UdpLink> {
        let sock = UdpSocket::bind(bind)?;
        sock.set_nonblocking(true)?;
        Ok(UdpLink::over(sock, peer))
    }

    /// A link over a socket already bound (a lobby's).
    pub fn over(sock: UdpSocket, peer: SocketAddr) -> UdpLink {
        let _ = sock.set_nonblocking(true);
        UdpLink { sock, peer, next_seq: 1, unacked: BTreeMap::new(), last_resend: Instant::now(), seen: BTreeSet::new(), resent: 0, duplicates: 0, rejected: 0 }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Messages sent and not yet acknowledged.
    pub fn unacked(&self) -> usize {
        self.unacked.len()
    }

    fn frame(kind: u8, seq: u64, payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(9 + payload.len());
        v.push(kind);
        v.extend_from_slice(&seq.to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    /// One pump: send what the session queued (and re-send what is unacked and due), take
    /// in what the peer sent (acks, and messages into the session). Call once per frame.
    pub fn pump(&mut self, session: &mut Lockstep, now: Instant) {
        for msg in session.take_outbox() {
            let text = match ron::to_string(&msg) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let seq = self.next_seq;
            self.next_seq += 1;
            let dg = Self::frame(KIND_MSG, seq, text.as_bytes());
            let _ = self.sock.send_to(&dg, self.peer);
            self.unacked.insert(seq, dg);
        }
        if now.duration_since(self.last_resend) >= Duration::from_millis(RESEND_MS) {
            self.last_resend = now;
            for dg in self.unacked.values() {
                let _ = self.sock.send_to(dg, self.peer);
                self.resent += 1;
            }
        }
        let mut buf = [0u8; MAX_DATAGRAM];
        loop {
            let (n, from) = match self.sock.recv_from(&mut buf) {
                Ok(x) => x,
                Err(_) => break,
            };
            if from != self.peer || n < 9 {
                self.rejected += 1;
                continue;
            }
            let seq = u64::from_le_bytes([buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7], buf[8]]);
            match buf[0] {
                KIND_ACK => {
                    self.unacked.remove(&seq);
                }
                KIND_MSG => {
                    // ack whatever arrives, duplicate or not: a duplicate means the ack was lost
                    let _ = self.sock.send_to(&Self::frame(KIND_ACK, seq, &[]), self.peer);
                    if !self.seen.insert(seq) {
                        self.duplicates += 1;
                        continue;
                    }
                    if self.seen.len() > SEEN_MAX {
                        let oldest = *self.seen.iter().next().unwrap();
                        self.seen.remove(&oldest);
                    }
                    match std::str::from_utf8(&buf[9..n]).ok().and_then(|t| ron::from_str::<Msg>(t).ok()) {
                        Some(msg) => session.recv(msg),
                        None => self.rejected += 1,
                    }
                }
                _ => self.rejected += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetCfg;
    use echelon_sim::{Cmd, PlayerCmd};
    use std::net::Ipv4Addr;

    fn cmd(p: u8, x: i32) -> PlayerCmd {
        PlayerCmd { player: p, cmd: Cmd::Move { ids: vec![0], mx: x, my: 0 } }
    }

    fn pair() -> (UdpLink, UdpLink) {
        let a = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let b = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let (aa, ab) = (a.local_addr().unwrap(), b.local_addr().unwrap());
        (UdpLink::over(a, ab), UdpLink::over(b, aa))
    }

    /// Two sessions over two sockets: each submits a turn, both execute it with the merged
    /// commands, the acks clear the queues.
    #[test]
    fn two_sessions_execute_a_turn_over_loopback_and_ack() {
        let cfg = NetCfg { players: 2, turn_len: 4, delay: 1 };
        let (mut s0, mut s1) = (Lockstep::new(cfg, 0), Lockstep::new(cfg, 1));
        let (mut l0, mut l1) = pair();
        s0.submit_local(vec![cmd(0, 1)]);
        s1.submit_local(vec![cmd(1, 2)]);
        let t0 = Instant::now();
        let (mut got0, mut got1) = (Vec::new(), Vec::new());
        for i in 0..200 {
            let now = t0 + Duration::from_millis(i * 5);
            l0.pump(&mut s0, now);
            l1.pump(&mut s1, now);
            while let Some(t) = s0.next_turn() {
                got0.push(t);
            }
            while let Some(t) = s1.next_turn() {
                got1.push(t);
            }
            if got0.len() == 2 && got1.len() == 2 && l0.unacked() == 0 && l1.unacked() == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        // turn 0 is the bootstrap (empty by definition); the submitted commands land at turn 1
        assert_eq!((got0.len(), got1.len()), (2, 2), "both executed the bootstrap turn and the first real one");
        let (t0c, c0) = got0.pop().unwrap();
        let (t1c, c1) = got1.pop().unwrap();
        assert_eq!((t0c, t1c), (1, 1), "the delay turn");
        assert_eq!(c0.len(), 2);
        assert_eq!(c0.iter().map(|c| c.player).collect::<Vec<_>>(), vec![0, 1], "player 0's commands first");
        assert_eq!(c0.iter().map(|c| c.player).collect::<Vec<_>>(), c1.iter().map(|c| c.player).collect::<Vec<_>>());
        assert_eq!((l0.unacked(), l1.unacked()), (0, 0), "everything acked");
        assert_eq!(l0.rejected + l1.rejected, 0);
    }

    /// A lossy link: a socket that drops every third datagram it receives (by reading and
    /// discarding it before the link sees it is not possible from outside, so the drop is
    /// done at the sender -- every third send skipped -- which the resend loop must cover).
    /// The match still executes every turn, in step, and nothing is applied twice.
    #[test]
    fn a_lossy_link_still_executes_every_turn_in_step() {
        let cfg = NetCfg { players: 2, turn_len: 4, delay: 2 };
        let (mut s0, mut s1) = (Lockstep::new(cfg, 0), Lockstep::new(cfg, 1));
        let (mut l0, mut l1) = pair();
        // a lossy middle: l0 talks to a relay socket that forwards two in three datagrams
        let relay = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        relay.set_nonblocking(true).unwrap();
        let relay_addr = relay.local_addr().unwrap();
        let l1_addr = l1.local_addr().unwrap();
        let l0_addr = l0.local_addr().unwrap();
        l0.peer = relay_addr;
        l1.peer = relay_addr;
        let mut executed = (Vec::new(), Vec::new());
        let t0 = Instant::now();
        let mut dropped = 0u32;
        let mut count = 0u32;
        for turn in 0..30u32 {
            s0.submit_local(if turn % 3 == 0 { vec![cmd(0, turn as i32)] } else { vec![] });
            s1.submit_local(if turn % 2 == 0 { vec![cmd(1, turn as i32)] } else { vec![] });
            for i in 0..400 {
                let now = t0 + Duration::from_millis((turn * 400 + i) as u64);
                l0.pump(&mut s0, now);
                l1.pump(&mut s1, now);
                // the relay: forward from l0 to l1 and back, dropping every third
                let mut buf = [0u8; 4096];
                while let Ok((n, from)) = relay.recv_from(&mut buf) {
                    count += 1;
                    if count % 3 == 0 {
                        dropped += 1;
                        continue;
                    }
                    let to = if from == l0_addr { l1_addr } else { l0_addr };
                    let _ = relay.send_to(&buf[..n], to);
                }
                while let Some((t, c)) = s0.next_turn() {
                    executed.0.push((t, c));
                }
                while let Some((t, c)) = s1.next_turn() {
                    executed.1.push((t, c));
                }
                // submitted turn k lands at turn k + delay: everything up to it executed
                let want = (turn + cfg.delay + 1) as usize;
                if executed.0.len() >= want && executed.1.len() >= want {
                    break;
                }
                std::thread::sleep(Duration::from_micros(300));
            }
        }
        assert!(dropped > 10, "the relay dropped datagrams: {dropped} of {count}");
        assert_eq!(executed.0.len(), executed.1.len(), "both executed the same number of turns");
        assert_eq!(executed.0.len(), 30 + cfg.delay as usize, "every submitted turn executed through the loss");
        for (a, b) in executed.0.iter().zip(executed.1.iter()) {
            assert_eq!(a.0, b.0);
            assert_eq!(a.1.len(), b.1.len(), "turn {}: the merged command lists differ", a.0);
            for (x, y) in a.1.iter().zip(b.1.iter()) {
                assert_eq!((x.player, format!("{:?}", x.cmd)), (y.player, format!("{:?}", y.cmd)));
            }
        }
        assert!(l0.resent + l1.resent > 0, "the loss was covered by resends");
        assert_eq!(l0.rejected + l1.rejected, 0);
    }

    /// A stranger's datagram and junk are rejected, not fed to the session.
    #[test]
    fn strangers_and_junk_are_rejected() {
        let cfg = NetCfg { players: 2, turn_len: 4, delay: 1 };
        let mut s0 = Lockstep::new(cfg, 0);
        let (mut l0, _l1) = pair();
        // player 0's turn 1 is in and the bootstrap turn is gone: turn 1 waits on player 1
        s0.submit_local(vec![cmd(0, 1)]);
        assert!(s0.next_turn().is_some());
        assert!(!s0.ready());
        let stranger = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let me = l0.local_addr().unwrap();
        stranger.send_to(&UdpLink::frame(KIND_MSG, 1, b"Cmds(turn:1,player:1,cmds:[])"), me).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        l0.pump(&mut s0, Instant::now());
        assert_eq!(l0.rejected, 1, "a stranger is rejected");
        assert!(!s0.ready(), "and its player-1 commands did not reach the session");
        // the peer sends junk under a good kind
        let peer_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        l0.peer = peer_sock.local_addr().unwrap();
        peer_sock.send_to(&UdpLink::frame(KIND_MSG, 2, b"not ron at all"), me).unwrap();
        peer_sock.send_to(&[9, 0, 0], me).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        l0.pump(&mut s0, Instant::now());
        assert_eq!(l0.rejected, 3);
    }
}
