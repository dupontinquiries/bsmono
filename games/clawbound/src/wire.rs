//! wire.rs — putting lockstep packets on an actual wire.
//!
//! [`crate::lockstep`] deliberately knows nothing about sockets: it owns when a
//! tick may run, and that is the part worth testing without a network. This is
//! the other side of that seam — a byte format small enough to be worth having,
//! and a UDP transport that carries it.
//!
//! The format, and why it is not JSON or bincode:
//!
//! * **Eight bytes for an input, five for a check.** A tick of a two-player
//!   match costs sixteen bytes of payload. That is the number that decides
//!   whether this is playable on a handset's uplink, and it is worth writing a
//!   codec by hand for.
//! * **Fixed-point movement.** A move is two axes of i8, a hundred and
//!   twenty-seventh of a length each way, because the sim normalises the vector
//!   anyway — sending f32 would be four times the bytes to express a direction
//!   that gets scaled to unit length on arrival. It also removes a whole class
//!   of desync: two peers that agree on the *bytes* of an input agree on the
//!   input, with no float parsing in between.
//! * **No allocation on the read path.** `decode` takes a slice and returns a
//!   packet; nothing in a receive loop touches the heap.
//!
//! The transport is deliberately dumb: UDP, no retransmit, no ordering. Lockstep
//! needs neither — a tick is only run when both inputs are present, so a lost
//! packet stalls rather than corrupts, and a duplicate is ignored because
//! [`crate::lockstep::LockstepPeer::receive`] already refuses a second input for
//! a tick it has. What a real deployment needs on top is resend-until-acked for
//! the *last few* frames, which is a transport concern and belongs here rather
//! than in the protocol.

use crate::lockstep::{Frame, Packet};

/// Bytes on the wire for one input packet.
pub const INPUT_LEN: usize = 8;
/// Bytes on the wire for one digest check.
pub const CHECK_LEN: usize = 13;

const TAG_INPUT: u8 = b'i';
const TAG_CHECK: u8 = b'c';

fn to_i8(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}
fn from_i8(v: i8) -> f32 {
    v as f32 / 127.0
}

/// Serialise a packet. Returns how many bytes were written, or None if the
/// buffer is too small — a receive loop's buffer is fixed and this should not
/// panic on a short one.
pub fn encode(p: &Packet, out: &mut [u8]) -> Option<usize> {
    match *p {
        Packet::Input { for_tick, frame } => {
            if out.len() < INPUT_LEN {
                return None;
            }
            out[0] = TAG_INPUT;
            out[1..5].copy_from_slice(&for_tick.to_le_bytes());
            out[5] = to_i8(frame.mx) as u8;
            out[6] = to_i8(frame.mz) as u8;
            // one byte carries the claw and the ability slot: a cat has three
            // abilities and two paws' worth of intent, not a whole octet each
            out[7] = (frame.claw as u8) | match frame.ability {
                Some(s) => ((s.min(2) as u8) + 1) << 1,
                None => 0,
            };
            Some(INPUT_LEN)
        }
        Packet::Check { at, digest } => {
            if out.len() < CHECK_LEN {
                return None;
            }
            out[0] = TAG_CHECK;
            out[1..5].copy_from_slice(&at.to_le_bytes());
            out[5..13].copy_from_slice(&digest.to_le_bytes());
            Some(CHECK_LEN)
        }
    }
}

/// Parse a packet from bytes. Returns None on anything malformed rather than
/// guessing — a peer that invents a frame from a corrupt datagram desyncs, and
/// a dropped one only stalls.
pub fn decode(b: &[u8]) -> Option<Packet> {
    match *b.first()? {
        TAG_INPUT if b.len() >= INPUT_LEN => {
            let for_tick = u32::from_le_bytes([b[1], b[2], b[3], b[4]]);
            let bits = b[7];
            let ability = match bits >> 1 {
                0 => None,
                s => Some((s - 1).min(2) as usize),
            };
            Some(Packet::Input {
                for_tick,
                frame: Frame {
                    mx: from_i8(b[5] as i8),
                    mz: from_i8(b[6] as i8),
                    claw: bits & 1 == 1,
                    ability,
                },
            })
        }
        TAG_CHECK if b.len() >= CHECK_LEN => Some(Packet::Check {
            at: u32::from_le_bytes([b[1], b[2], b[3], b[4]]),
            digest: u64::from_le_bytes([
                b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12],
            ]),
        }),
        _ => None,
    }
}

/// A UDP link for one match. Bind, tell it the other paw's address, and pump.
///
/// This is the layer `games/shared/netcode` hands off to: that crate finds the
/// other player (profiles, the DHT, a session token) and ends with two peers
/// holding each other's socket address, which is exactly what this needs and
/// all it needs. Kept separate so a match can be played over a plain address
/// pair with no discovery at all — which is what the tests do, and what a LAN
/// game between two handsets on the same wifi should not have to pay for.
pub struct UdpLink {
    sock: std::net::UdpSocket,
    peer: Option<std::net::SocketAddr>,
    buf: [u8; 64],
}

impl UdpLink {
    pub fn bind(addr: &str) -> std::io::Result<UdpLink> {
        let sock = std::net::UdpSocket::bind(addr)?;
        sock.set_nonblocking(true)?;
        Ok(UdpLink { sock, peer: None, buf: [0; 64] })
    }

    pub fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.sock.local_addr()
    }

    pub fn connect(&mut self, peer: std::net::SocketAddr) {
        self.peer = Some(peer);
    }

    /// Send everything a peer has queued. Failures are counted, not raised: a
    /// dropped datagram is a stall the protocol already handles, and a match
    /// should not end because one `send_to` returned EAGAIN.
    pub fn flush(&mut self, packets: &[Packet]) -> usize {
        let Some(peer) = self.peer else { return 0 };
        let mut sent = 0;
        let mut b = [0u8; 32];
        for p in packets {
            if let Some(n) = encode(p, &mut b) {
                if self.sock.send_to(&b[..n], peer).is_ok() {
                    sent += 1;
                }
            }
        }
        sent
    }

    /// Drain whatever has arrived. Learns the peer's address from the first
    /// datagram, so the side that was dialled does not need to be told.
    pub fn poll(&mut self) -> Vec<Packet> {
        let mut out = Vec::new();
        loop {
            match self.sock.recv_from(&mut self.buf) {
                Ok((n, from)) => {
                    if self.peer.is_none() {
                        self.peer = Some(from);
                    }
                    if let Some(p) = decode(&self.buf[..n]) {
                        out.push(p);
                    }
                }
                Err(_) => break, // would-block, or a datagram we cannot use
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ancestor::Ancestor;
    use crate::battleground::{Battleground, LanePolicy};
    use crate::lockstep::LockstepPeer;

    #[test]
    fn a_packet_survives_the_wire() {
        let cases = [
            Packet::Input { for_tick: 0, frame: Frame::default() },
            Packet::Input {
                for_tick: 4_000_000,
                frame: Frame { mx: -1.0, mz: 1.0, claw: true, ability: Some(2) },
            },
            Packet::Input {
                for_tick: 7,
                frame: Frame { mx: 0.5, mz: -0.25, claw: false, ability: Some(0) },
            },
            Packet::Check { at: 30, digest: 0xdead_beef_0bad_f00d },
        ];
        let mut b = [0u8; 32];
        for c in cases {
            let n = encode(&c, &mut b).expect("encodes");
            let back = decode(&b[..n]).expect("decodes");
            match (c, back) {
                (Packet::Input { for_tick: t1, frame: f1 }, Packet::Input { for_tick: t2, frame: f2 }) => {
                    assert_eq!(t1, t2);
                    assert_eq!((f1.claw, f1.ability), (f2.claw, f2.ability));
                    // movement is fixed-point: it comes back within a step of
                    // the ladder, which is all a normalised direction needs
                    assert!((f1.mx - f2.mx).abs() < 0.01, "{f1:?} vs {f2:?}");
                    assert!((f1.mz - f2.mz).abs() < 0.01);
                }
                (a, b) => assert_eq!(a, b),
            }
        }
    }

    #[test]
    fn a_tick_of_two_players_is_sixteen_bytes() {
        let mut b = [0u8; 32];
        let n = encode(
            &Packet::Input { for_tick: 12, frame: Frame { mx: 1.0, ..Frame::default() } },
            &mut b,
        )
        .unwrap();
        assert_eq!(n, 8, "an input is eight bytes, so a tick of two players is sixteen");
        assert_eq!(
            encode(&Packet::Check { at: 30, digest: 1 }, &mut b).unwrap(),
            13
        );
    }

    #[test]
    fn rubbish_is_dropped_rather_than_guessed_at() {
        assert!(decode(&[]).is_none());
        assert!(decode(b"i").is_none(), "a truncated input must not be invented");
        assert!(decode(&[TAG_CHECK, 1, 2, 3]).is_none());
        assert!(decode(b"hello").is_none(), "netcode's handshake datagram is not a frame");
        // and a short buffer refuses to encode rather than writing past it
        let mut tiny = [0u8; 4];
        assert!(encode(&Packet::Check { at: 1, digest: 2 }, &mut tiny).is_none());
    }

    #[test]
    fn two_peers_play_a_match_over_real_sockets() {
        let a = [Ancestor::Lynx, Ancestor::Lion, Ancestor::Puma];
        let b = [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Serval];
        let (Ok(mut la), Ok(mut lb)) =
            (UdpLink::bind("127.0.0.1:0"), UdpLink::bind("127.0.0.1:0"))
        else {
            eprintln!("no loopback sockets here — skipping");
            return;
        };
        let (aa, ba) = (la.local_addr().unwrap(), lb.local_addr().unwrap());
        la.connect(ba);
        lb.connect(aa);

        let mut p = LockstepPeer::new(0, Battleground::new_with(a, b, 3, LanePolicy::Kit));
        let mut q = LockstepPeer::new(1, Battleground::new_with(a, b, 3, LanePolicy::Kit));
        // both sims read the one global height grid: hold the terrain lock across the
        // stepping so a parallel test's bake cannot swap the country between p's tick
        // and q's (it did, one run in eight: "ticks: 30 and 30", a desync at the first
        // checkpoint that was the suite's, not the wire's)
        let _hold = crate::world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // two builds of one draft are one battleground; if they are not, the desync below
        // is the suite's (a knob flipped under a build), not the wire's -- say which
        let d0 = (p.bg.digest(), q.bg.digest());
        assert_eq!(d0.0, d0.1, "the two peers were built on different ground: camps {:?} vs {:?}", p.bg.camps, q.bg.camps);
        let mut stalls = 0u32;

        for t in 0..240u32 {
            p.submit(Frame { mx: 1.0, mz: 0.0, claw: t % 30 == 0, ability: None });
            q.submit(Frame { mx: 0.0, mz: -1.0, claw: false, ability: None });
            let (ps, qs) = (p.drain(), q.drain());
            la.flush(&ps);
            lb.flush(&qs);
            // loopback is not instant; give the datagrams a moment, then pump
            // until both sides can run the tick
            for _ in 0..50 {
                for pk in lb.poll() {
                    q.receive(pk);
                }
                for pk in la.poll() {
                    p.receive(pk);
                }
                if p.ready() && q.ready() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
            let (sp, sq) = (p.step(), q.step());
            if !sp || !sq { stalls += 1; }
        }
        assert!(p.tick > 200 && q.tick > 200, "ticks: {} and {}; stalls {}; halted {:?} {:?}", p.tick, q.tick, stalls, p.halted, q.halted);
        assert_eq!(p.tick, q.tick);
        assert_eq!(p.halted, None, "no desync over the wire");
        assert_eq!(q.halted, None);
        assert_eq!(
            p.bg.digest(),
            q.bg.digest(),
            "two processes, one match — the whole point"
        );
    }
}

