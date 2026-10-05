//! lockstep.rs — two clans, one simulation, no authority.
//!
//! The battleground is deterministic: `replay.rs` proves it every test run by
//! replaying a whole match to the digest it recorded. That is the expensive
//! precondition for the cheapest kind of multiplayer — **deterministic
//! lockstep**, where peers exchange *inputs* rather than state, each runs the
//! same simulation, and nobody is the server. A match is a few bytes per tick
//! instead of a stream of positions, which is what makes it playable over the
//! kind of link a phone has.
//!
//! This module is the **protocol**, not the transport. It owns the rules — when
//! a tick may advance, what a peer is owed, what happens when one goes quiet,
//! how the two sides notice they have diverged — and knows nothing about
//! sockets, the DHT, or `games/shared/netcode`. That seam is deliberate: the
//! rules are the part worth testing, and they are testable in-process with two
//! `LockstepPeer`s passing packets to each other, which is how the tests below
//! run an entire match between two "players" without opening a port.
//!
//! ## The rules
//!
//! * **Input delay.** A peer's input for tick T is sent at tick T − [`DELAY`]
//!   and both sides apply it at T. Nobody waits on a round trip mid-frame; the
//!   cost is that your own claw lands two hundred milliseconds after you press
//!   it, which is the standard bargain and the reason fighting games use it.
//! * **A tick advances only when both inputs are in.** That is the lockstep
//!   part, and it is why this cannot desync from missing data — only from a
//!   genuine simulation difference.
//! * **Digests are exchanged and compared.** Every [`CHECK_EVERY`] ticks each
//!   peer sends the digest of its own world. A mismatch means the two clans are
//!   no longer in the same match, and the honest response is to say so and stop
//!   rather than to let two players play different games at each other.
//! * **A quiet peer stalls the match, then loses it.** Waiting forever is not a
//!   behaviour; after [`TIMEOUT_TICKS`] with nothing heard, the match ends and
//!   the reason is recorded.

use crate::battleground::Battleground;

/// Ticks of input delay. Six at 30 Hz is 200 ms — enough to cover a domestic
/// link's jitter without the input feeling posted rather than pressed.
pub const DELAY: u32 = 6;
/// How often the peers compare digests. Every second: often enough that a
/// desync is caught while it is still one tick's worth of difference, rare
/// enough that it is not most of the traffic.
pub const CHECK_EVERY: u32 = 30;
/// How long a peer may go quiet before the match is called.
pub const TIMEOUT_TICKS: u32 = 30 * 10;

/// One player's input for one tick — the entire payload of this protocol.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Frame {
    pub mx: f32,
    pub mz: f32,
    pub claw: bool,
    pub ability: Option<usize>,
}

/// What crosses the wire. Small on purpose: a tick of a two-player match is an
/// `Input` each, and a `Check` once a second.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Packet {
    /// "Here is my input for tick `for_tick`."
    Input { for_tick: u32, frame: Frame },
    /// "At tick `at`, my world hashed to `digest`."
    Check { at: u32, digest: u64 },
}

/// Why a match stopped, when it was not the Great Tree falling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Halt {
    /// The two simulations disagree: same tick, different digest.
    Desync { at: u32 },
    /// Nothing heard from the other clan for [`TIMEOUT_TICKS`].
    PeerLost { at: u32 },
}

/// One side of a lockstep match.
pub struct LockstepPeer {
    /// Which clan this peer's player leads: 0 or 1.
    pub side: u8,
    pub bg: Battleground,
    pub tick: u32,
    pub halted: Option<Halt>,
    /// Inputs received from the other side, keyed by the tick they apply to.
    remote: Vec<(u32, Frame)>,
    /// Our own inputs, kept so a late tick can still find the one it needs.
    local: Vec<(u32, Frame)>,
    /// Digests the other side has claimed, and our own — kept because the two
    /// arrive at different times. A peer hashes its world at tick T and the
    /// other peer's claim for T lands a packet-flight later, so comparing only
    /// at the moment of hashing catches nothing. Whichever half arrives second
    /// does the comparing.
    remote_checks: Vec<(u32, u64)>,
    own_checks: Vec<(u32, u64)>,
    silent_for: u32,
    /// Packets this peer wants sent. The transport drains this; the protocol
    /// never touches a socket.
    pub outbox: Vec<Packet>,
}

impl LockstepPeer {
    /// Both peers must be handed a battleground built from the *same* draft,
    /// seed and doctrine — which is exactly what a [`crate::replay::Replay`]
    /// header carries, so a lobby can agree on a match by exchanging one.
    pub fn new(side: u8, bg: Battleground) -> LockstepPeer {
        let mut p = LockstepPeer {
            side,
            bg,
            tick: 0,
            halted: None,
            remote: Vec::new(),
            local: Vec::new(),
            remote_checks: Vec::new(),
            own_checks: Vec::new(),
            silent_for: 0,
            outbox: Vec::new(),
        };
        // The first `DELAY` ticks have no input behind them — somebody has to
        // put an empty frame there or the match cannot start, because a tick
        // needs both sides' input before it may run.
        for t in 0..DELAY {
            p.local.push((t, Frame::default()));
            p.outbox.push(Packet::Input { for_tick: t, frame: Frame::default() });
        }
        p
    }

    /// Offer this peer's input for the tick `DELAY` ahead of now.
    pub fn submit(&mut self, f: Frame) {
        let for_tick = self.tick + DELAY;
        if self.local.iter().any(|&(t, _)| t == for_tick) {
            return; // already spoken for; a second press in the same tick is one press
        }
        self.local.push((for_tick, f));
        self.outbox.push(Packet::Input { for_tick, frame: f });
    }

    pub fn receive(&mut self, p: Packet) {
        self.silent_for = 0;
        match p {
            Packet::Input { for_tick, frame } => {
                if !self.remote.iter().any(|&(t, _)| t == for_tick) {
                    self.remote.push((for_tick, frame));
                }
            }
            Packet::Check { at, digest } => {
                self.remote_checks.push((at, digest));
                if let Some(&(_, mine)) = self.own_checks.iter().find(|&&(t, _)| t == at) {
                    if mine != digest {
                        self.halted = Some(Halt::Desync { at });
                    }
                }
            }
        }
    }

    fn frame_at(list: &[(u32, Frame)], tick: u32) -> Option<Frame> {
        list.iter().find(|&&(t, _)| t == tick).map(|&(_, f)| f)
    }

    /// Can this tick run? Only with both sides' input for it in hand.
    pub fn ready(&self) -> bool {
        self.halted.is_none()
            && Self::frame_at(&self.local, self.tick).is_some()
            && Self::frame_at(&self.remote, self.tick).is_some()
    }

    /// Advance one tick if both inputs are in. Returns whether it ran; a `false`
    /// is a stall, which is normal and brief, or a halt, which is not.
    pub fn step(&mut self) -> bool {
        if self.halted.is_some() {
            return false;
        }
        if !self.ready() {
            self.silent_for += 1;
            if self.silent_for > TIMEOUT_TICKS {
                self.halted = Some(Halt::PeerLost { at: self.tick });
            }
            return false;
        }
        let mine = Self::frame_at(&self.local, self.tick).unwrap();
        let theirs = Self::frame_at(&self.remote, self.tick).unwrap();
        // Each peer applies BOTH frames — its own and the one that arrived —
        // because both worlds must see the same two decisions in the same order.
        // A peer that only drove its own leader and left the other clan's to the
        // AI would be running a different match from the one across the link,
        // which is the classic way a lockstep game desyncs on move one.
        let (a, b) = if self.side == 0 { (mine, theirs) } else { (theirs, mine) };
        self.bg.set_input_for(0, a.mx, a.mz, a.claw, a.ability);
        self.bg.set_input_for(1, b.mx, b.mz, b.claw, b.ability);
        self.bg.step(1.0 / 30.0);
        self.tick += 1;

        if self.tick % CHECK_EVERY == 0 {
            let d = self.bg.digest();
            self.own_checks.push((self.tick, d));
            self.outbox.push(Packet::Check { at: self.tick, digest: d });
            self.compare(self.tick, d);
        }
        true
    }

    fn compare(&mut self, at: u32, mine: u64) {
        if let Some(&(_, theirs)) = self.remote_checks.iter().find(|&&(t, _)| t == at) {
            if theirs != mine {
                self.halted = Some(Halt::Desync { at });
            }
        }
    }

    pub fn drain(&mut self) -> Vec<Packet> {
        std::mem::take(&mut self.outbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ancestor::Ancestor;
    use crate::battleground::LanePolicy;

    fn pair() -> (LockstepPeer, LockstepPeer) {
        let a = [Ancestor::Lynx, Ancestor::Lion, Ancestor::Puma];
        let b = [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Serval];
        (
            LockstepPeer::new(0, Battleground::new_with(a, b, 3, LanePolicy::Kit)),
            LockstepPeer::new(1, Battleground::new_with(a, b, 3, LanePolicy::Kit)),
        )
    }

    /// Move every queued packet across, optionally dropping the link one way.
    fn exchange(x: &mut LockstepPeer, y: &mut LockstepPeer, deliver: bool) {
        let (xs, ys) = (x.drain(), y.drain());
        if deliver {
            for p in xs {
                y.receive(p);
            }
            for p in ys {
                x.receive(p);
            }
        }
    }

    #[test]
    fn two_peers_play_the_same_match() {
        let (mut p, mut q) = pair();
        // hold the terrain lock across the stepping: both sims read the one global
        // height grid, and a parallel test's bake between p's tick and q's would be a
        // desync that is the suite's, not the protocol's
        let _hold = crate::world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for t in 0..600u32 {
            p.submit(Frame { mx: 1.0, mz: 0.0, claw: t % 30 == 0, ability: None });
            q.submit(Frame { mx: -1.0, mz: 0.5, claw: false, ability: None });
            exchange(&mut p, &mut q, true);
            p.step();
            q.step();
        }
        assert_eq!(p.tick, q.tick, "the two sides ran the same number of ticks");
        assert!(p.tick > 500, "and actually ran: {} ticks", p.tick);
        assert_eq!(p.bg.digest(), q.bg.digest(), "same inputs, same match");
        assert_eq!(p.halted, None);
        assert_eq!(q.halted, None);
    }

    #[test]
    fn a_tick_waits_for_both_clans() {
        let (mut p, mut q) = pair();
        // run past the opening frames so the pre-seeded ones are used up
        for _ in 0..DELAY + 2 {
            p.submit(Frame::default());
            q.submit(Frame::default());
            exchange(&mut p, &mut q, true);
            p.step();
            q.step();
        }
        // now cut the link: p keeps pressing, and stalls where q's input ran out
        let stalled_at = p.tick;
        for _ in 0..20 {
            p.submit(Frame { mx: 1.0, ..Frame::default() });
            exchange(&mut p, &mut q, false);
            p.step();
        }
        assert!(
            p.tick <= stalled_at + DELAY,
            "a peer must not run ahead of the inputs it has: {} vs {stalled_at}",
            p.tick
        );
        assert_eq!(p.halted, None, "a brief silence is a stall, not a halt");
    }

    #[test]
    fn a_peer_that_goes_quiet_ends_the_match() {
        let (mut p, mut q) = pair();
        for _ in 0..(TIMEOUT_TICKS + DELAY + 60) {
            p.submit(Frame::default());
            exchange(&mut p, &mut q, false);
            p.step();
        }
        assert!(matches!(p.halted, Some(Halt::PeerLost { .. })), "got {:?}", p.halted);
        // and a halted peer stays halted rather than quietly resuming
        let at = p.tick;
        q.submit(Frame::default());
        exchange(&mut q, &mut p, true);
        assert!(!p.step());
        assert_eq!(p.tick, at);
    }

    #[test]
    fn a_divergence_is_caught_rather_than_played_through() {
        let (mut p, mut q) = pair();
        // q's world is nudged: one warrior a whisker from where p has it. This
        // is what a real desync looks like from the inside — nothing is
        // obviously broken, the two matches simply are not the same one.
        q.bg.warriors[2].x += 0.5;
        for _ in 0..CHECK_EVERY * 2 + DELAY {
            p.submit(Frame::default());
            q.submit(Frame::default());
            exchange(&mut p, &mut q, true);
            p.step();
            q.step();
        }
        assert!(
            matches!(p.halted, Some(Halt::Desync { .. })) || matches!(q.halted, Some(Halt::Desync { .. })),
            "a desync must be noticed: p {:?}, q {:?}",
            p.halted,
            q.halted
        );
    }

    #[test]
    fn each_side_moves_its_own_leader_and_neither_moves_the_others() {
        let (mut p, mut q) = pair();
        let _hold = crate::world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner()); // as above
        let a0 = p.bg.warriors.iter().position(|w| w.team == 0 && w.is_player).unwrap();
        let b0 = p.bg.warriors.iter().position(|w| w.team == 1 && w.is_player).unwrap();
        let start_a = (p.bg.warriors[a0].x, p.bg.warriors[a0].z);
        let start_b = (p.bg.warriors[b0].x, p.bg.warriors[b0].z);
        for _ in 0..90 {
            p.submit(Frame { mx: 1.0, mz: 0.0, claw: false, ability: None });
            q.submit(Frame { mx: 0.0, mz: -1.0, claw: false, ability: None });
            exchange(&mut p, &mut q, true);
            p.step();
            q.step();
        }
        // both worlds agree, and each leader went where its own player pushed
        assert_eq!(p.bg.digest(), q.bg.digest());
        assert!(p.bg.warriors[a0].x - start_a.0 > 2.0, "clan A's leader went east");
        assert!(start_b.1 - p.bg.warriors[b0].z > 2.0, "clan B's leader went north");
        // and the peer that is NOT clan B still shows clan B where clan B is
        assert!((p.bg.warriors[b0].x - q.bg.warriors[b0].x).abs() < 1e-6);
    }

    #[test]
    fn the_wire_is_small() {
        let (mut p, mut q) = pair();
        let mut packets = 0usize;
        for _ in 0..300 {
            p.submit(Frame { mx: 1.0, ..Frame::default() });
            q.submit(Frame::default());
            packets += p.outbox.len() + q.outbox.len();
            exchange(&mut p, &mut q, true);
            p.step();
            q.step();
        }
        // ten seconds of two players: one input each per tick plus a digest a
        // second each. Anything much above that means state is leaking into a
        // protocol that is supposed to carry decisions.
        assert!(
            packets < 300 * 2 + 2 * 12 + DELAY as usize * 2,
            "{packets} packets for 300 ticks is more than inputs and checks"
        );
    }
}
