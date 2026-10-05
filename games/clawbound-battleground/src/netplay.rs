//! netplay -- lockstep inputs over an unreliable network, tuned for
//! reaction time.
//!
//! `lockstep` is the rule (advance a tick only when every player's input
//! for it is in; checksums catch desyncs). This is the wire, and the three
//! things that decide how quickly a press becomes an action on every
//! machine:
//!
//! 1. **Redundancy, not resends.** Every packet -- one per frame -- carries
//!    the sender's last [`REDUNDANCY`] ticks of input. A lost packet is
//!    covered by the next one a frame later; nothing waits a round trip for
//!    a resend, so loss costs at most a frame, not a stall.
//! 2. **Adaptive input delay.** A press is scheduled `delay` ticks ahead so
//!    it reaches everyone before that tick runs. A fixed delay is either
//!    sluggish on a LAN or stalls on Wi-Fi; here it is the smallest that the
//!    MEASURED round trip and jitter allow (TCP-style estimators on echoed
//!    timestamps), changed one tick at a time, safely: shrinking it never
//!    drops a press (a cast or leap carries to the next stamp), growing it
//!    never leaves a tick without an input (the gap repeats the input).
//! 3. **Time dilation.** A machine ahead of its peers runs a few percent
//!    slow until they converge, so a small skew is absorbed smoothly instead
//!    of by a stall.
//!
//! Headless: [`Session`] takes packets and a clock and returns packets; the
//! caller owns the socket (or [`NetSim`], the in-process network with
//! latency, jitter and loss the tests and `examples/netbench` play over).

use std::collections::{BTreeMap, VecDeque};

use crate::lockstep::{checksum, Input, Lockstep, WIRE_LEN};
use crate::sim::Game;

/// Ticks of input history in every packet.
pub const REDUNDANCY: usize = 8;
pub const TICK_MS: f32 = 1000.0 / 60.0;
/// Bounds on the adaptive input delay, in ticks.
pub const MIN_DELAY: u32 = 1;
pub const MAX_DELAY: u32 = 10;
/// Extra headroom in the delay estimate, milliseconds: scheduling and the
/// one-frame gap between sends.
pub const MARGIN_MS: f32 = 6.0;
const MAGIC: [u8; 4] = *b"CBNP";

/// One datagram from one player to another.
#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub from: u8,
    /// The tick of `inputs[0]`; the rest follow consecutively.
    pub first_tick: u32,
    pub inputs: Vec<Input>,
    /// Sender's clock (ms) when sent, and the latest such stamp the sender
    /// had received from the recipient, with how long it had held it.
    pub stamp: u32,
    pub echo: u32,
    pub echo_hold: u16,
    /// A checksum of the sender's world at a tick, for desync detection.
    pub check: Option<(u32, u64)>,
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(32 + self.inputs.len() * WIRE_LEN);
        b.extend_from_slice(&MAGIC);
        b.push(self.from);
        b.extend_from_slice(&self.first_tick.to_le_bytes());
        b.push(self.inputs.len() as u8);
        for i in &self.inputs {
            b.extend_from_slice(&i.encode());
        }
        b.extend_from_slice(&self.stamp.to_le_bytes());
        b.extend_from_slice(&self.echo.to_le_bytes());
        b.extend_from_slice(&self.echo_hold.to_le_bytes());
        match self.check {
            Some((t, c)) => {
                b.push(1);
                b.extend_from_slice(&t.to_le_bytes());
                b.extend_from_slice(&c.to_le_bytes());
            }
            None => b.push(0),
        }
        b
    }

    pub fn decode(b: &[u8]) -> Option<Packet> {
        let mut r = b;
        let mut take = |n: usize| -> Option<&[u8]> {
            if r.len() < n {
                return None;
            }
            let (a, rest) = r.split_at(n);
            r = rest;
            Some(a)
        };
        if take(4)? != MAGIC {
            return None;
        }
        let from = take(1)?[0];
        let first_tick = u32::from_le_bytes(take(4)?.try_into().ok()?);
        let n = take(1)?[0] as usize;
        if n > REDUNDANCY {
            return None;
        }
        let mut inputs = Vec::with_capacity(n);
        for _ in 0..n {
            inputs.push(Input::decode(take(WIRE_LEN)?)?);
        }
        let stamp = u32::from_le_bytes(take(4)?.try_into().ok()?);
        let echo = u32::from_le_bytes(take(4)?.try_into().ok()?);
        let echo_hold = u16::from_le_bytes(take(2)?.try_into().ok()?);
        let check = match take(1)?[0] {
            0 => None,
            1 => Some((u32::from_le_bytes(take(4)?.try_into().ok()?), u64::from_le_bytes(take(8)?.try_into().ok()?))),
            _ => return None,
        };
        Some(Packet { from, first_tick, inputs, stamp, echo, echo_hold, check })
    }
}

/// Round-trip estimate, TCP style (RFC 6298): smoothed RTT and variance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rtt {
    pub srtt: f32,
    pub rttvar: f32,
    pub samples: u32,
}

impl Default for Rtt {
    fn default() -> Rtt {
        // until measured: assume a modest LAN
        Rtt { srtt: 20.0, rttvar: 10.0, samples: 0 }
    }
}

impl Rtt {
    pub fn sample(&mut self, rtt: f32) {
        if !(rtt.is_finite() && rtt >= 0.0 && rtt < 5000.0) {
            return;
        }
        if self.samples == 0 {
            self.srtt = rtt;
            self.rttvar = rtt / 2.0;
        } else {
            self.rttvar = 0.75 * self.rttvar + 0.25 * (self.srtt - rtt).abs();
            self.srtt = 0.875 * self.srtt + 0.125 * rtt;
        }
        self.samples += 1;
    }

    /// The input delay (ticks) these measurements call for: the one-way
    /// trip, two deviations of jitter, and a margin, rounded up to ticks.
    pub fn delay_ticks(&self) -> u32 {
        let ms = self.srtt / 2.0 + 2.0 * self.rttvar + MARGIN_MS;
        ((ms / TICK_MS).ceil() as u32).clamp(MIN_DELAY, MAX_DELAY)
    }
}

/// The baseline this is measured against: what a first implementation does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    /// Inputs per packet (1 = no redundancy).
    pub redundancy: usize,
    /// Fixed delay, or None to adapt.
    pub fixed_delay: Option<u32>,
    /// Without redundancy: resend unacknowledged inputs after this long.
    pub resend_ms: u32,
    pub dilation: bool,
}

impl Tuning {
    pub const NAIVE: Tuning = Tuning { redundancy: 1, fixed_delay: Some(6), resend_ms: 100, dilation: false };
    pub const TUNED: Tuning = Tuning { redundancy: REDUNDANCY, fixed_delay: None, resend_ms: 0, dilation: true };
}

/// Latency accounting for the presses this machine made.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    /// ms from each press to the tick it takes effect running here.
    pub reaction_ms: Vec<f32>,
    /// Frames this machine wanted to advance but waited for input.
    pub stalled_frames: u32,
    pub packets_sent: u32,
    pub bytes_sent: u64,
    pub desync: Option<u32>,
}

pub struct Session {
    pub me: usize,
    pub lockstep: Lockstep,
    pub tuning: Tuning,
    pub rtt: Rtt,
    pub stats: Stats,
    delay: u32,
    /// The last tick this machine stamped an input for.
    last_stamp: Option<u32>,
    /// A press that could not be stamped (the delay shrank) rides the next.
    carry: Option<Input>,
    /// This machine's stamped inputs, newest last, for the redundant window.
    history: VecDeque<(u32, Input)>,
    /// Per peer: the latest stamp received and when, for the echo.
    heard: Vec<Option<(u32, u32)>>,
    /// Per peer: the highest tick received from them (for dilation).
    peer_tick: Vec<u32>,
    /// Naive tuning: when each input was last sent.
    sent_at: BTreeMap<u32, u32>,
    /// Presses awaiting their tick: (tick, pressed at ms).
    pending_press: VecDeque<(u32, u32)>,
    my_checks: BTreeMap<u32, u64>,
    their_checks: BTreeMap<u32, u64>,
    last_delay_change: u32,
}

impl Session {
    pub fn new(me: usize, players: usize, tuning: Tuning) -> Session {
        let delay = tuning.fixed_delay.unwrap_or(4);
        Session {
            me,
            lockstep: Lockstep::new((0..players).collect(), delay),
            tuning,
            rtt: Rtt::default(),
            stats: Stats::default(),
            delay,
            last_stamp: None,
            carry: None,
            history: VecDeque::new(),
            heard: vec![None; players],
            peer_tick: vec![0; players],
            sent_at: BTreeMap::new(),
            pending_press: VecDeque::new(),
            my_checks: BTreeMap::new(),
            their_checks: BTreeMap::new(),
            last_delay_change: 0,
        }
    }

    pub fn delay(&self) -> u32 {
        self.delay
    }

    /// Stamp this frame's input for tick + delay and record it. Called
    /// once per tick this machine simulates. `pressed` marks a press (for
    /// the latency accounting) made at `now`.
    pub fn stamp(&mut self, input: Input, pressed: bool, now: u32) {
        // adapt the delay: one tick per half-second at most
        if self.tuning.fixed_delay.is_none() && self.rtt.samples >= 4 && now.saturating_sub(self.last_delay_change) >= 500 {
            let want = self.rtt.delay_ticks();
            if want != self.delay {
                self.delay = if want > self.delay { self.delay + 1 } else { self.delay - 1 };
                self.last_delay_change = now;
            }
        }
        let mut input = input;
        if let Some(c) = self.carry.take() {
            if input.cast.is_none() {
                input.cast = c.cast;
            }
            input.leap |= c.leap;
        }
        let target = self.lockstep.tick + self.delay;
        let next = self.last_stamp.map_or(target, |l| l + 1);
        if target < next {
            // the delay shrank: this tick is already stamped; carry the
            // press's edges (a cast, a leap) to the next stamp
            self.carry = Some(input);
            return;
        }
        // the delay grew (or first stamp): fill any gap with this input so
        // no tick is left without one
        for t in next..=target {
            self.lockstep.submit(self.me, t, input);
            self.history.push_back((t, input));
        }
        self.last_stamp = Some(target);
        while self.history.len() > REDUNDANCY * 4 {
            self.history.pop_front();
        }
        if pressed {
            self.pending_press.push_back((target, now));
        }
    }

    /// The packets to send to `peer` now. Tuned: ONE packet with the last N
    /// stamped inputs, every frame. Naive: a packet per input -- each new
    /// input once, and each not yet simulated here resent every `resend_ms`
    /// (a timeout, since it has no acknowledgements).
    pub fn packets_for(&mut self, peer: usize, now: u32) -> Vec<Packet> {
        if self.tuning.redundancy > 1 {
            return self.packet_for(peer, now).into_iter().collect();
        }
        let due: Vec<(u32, Input)> = self
            .history
            .iter()
            .copied()
            // any recent input the peer may still need: NOT only ticks this
            // machine has yet to run -- a lost input for a tick already run
            // here is exactly the one the peer is stalled on (resending only
            // `>= own tick` deadlocks both, found by the benchmark)
            .filter(|(t, _)| t + REDUNDANCY as u32 * 4 >= self.lockstep.tick && self.sent_at.get(t).map_or(true, |&at| now.saturating_sub(at) >= self.tuning.resend_ms))
            .collect();
        let mut out = Vec::new();
        for (t, inp) in due {
            self.sent_at.insert(t, now);
            out.push(self.wrap(peer, now, vec![(t, inp)]));
        }
        out
    }

    fn wrap(&mut self, peer: usize, now: u32, inputs: Vec<(u32, Input)>) -> Packet {
        let (echo, echo_hold) = match self.heard[peer] {
            Some((stamp, at)) => (stamp, now.saturating_sub(at).min(u16::MAX as u32 - 1) as u16),
            None => (0, u16::MAX),
        };
        let check = self.my_checks.iter().next_back().map(|(t, c)| (*t, *c));
        let p = Packet { from: self.me as u8, first_tick: inputs.first().map_or(self.lockstep.tick, |x| x.0), inputs: inputs.iter().map(|x| x.1).collect(), stamp: now, echo, echo_hold, check };
        self.stats.packets_sent += 1;
        self.stats.bytes_sent += p.encode().len() as u64 + 28; // + UDP/IP headers
        p
    }

    /// The tuned packet: the last REDUNDANCY stamped inputs.
    pub fn packet_for(&mut self, peer: usize, now: u32) -> Option<Packet> {
        let inputs: Vec<(u32, Input)> = self.history.iter().rev().take(self.tuning.redundancy.max(1)).rev().copied().collect();
        Some(self.wrap(peer, now, inputs))
    }

    pub fn receive(&mut self, p: &Packet, now: u32) {
        let from = p.from as usize;
        if from >= self.heard.len() || from == self.me {
            return;
        }
        self.heard[from] = Some((p.stamp, now));
        if p.echo_hold != u16::MAX && p.echo > 0 {
            self.rtt.sample(now.saturating_sub(p.echo).saturating_sub(p.echo_hold as u32) as f32);
        }
        for (k, inp) in p.inputs.iter().enumerate() {
            let t = p.first_tick + k as u32;
            self.lockstep.submit(from, t, *inp);
            self.peer_tick[from] = self.peer_tick[from].max(t);
        }
        if let Some((t, c)) = p.check {
            self.their_checks.insert(t, c);
            if let Some(mine) = self.my_checks.get(&t) {
                if *mine != c && self.stats.desync.is_none() {
                    self.stats.desync = Some(t);
                }
            }
        }
    }

    pub fn ready(&self) -> bool {
        self.lockstep.ready()
    }

    /// Advance one tick if every input is in; records reaction latency for
    /// presses that take effect on it, and a checksum every 30 ticks.
    pub fn step(&mut self, game: &mut Game, dt: f32, now: u32) -> bool {
        let tick = self.lockstep.tick;
        if self.lockstep.step(game, dt).is_none() {
            self.stats.stalled_frames += 1;
            return false;
        }
        while let Some(&(t, at)) = self.pending_press.front() {
            if t > tick {
                break;
            }
            self.stats.reaction_ms.push(now.saturating_sub(at) as f32);
            self.pending_press.pop_front();
        }
        if tick % 30 == 0 {
            let c = checksum(game);
            self.my_checks.insert(tick, c);
            if let Some(theirs) = self.their_checks.get(&tick) {
                if *theirs != c && self.stats.desync.is_none() {
                    self.stats.desync = Some(tick);
                }
            }
            while self.my_checks.len() > 8 {
                let first = *self.my_checks.keys().next().unwrap();
                self.my_checks.remove(&first);
            }
        }
        true
    }

    /// How fast this machine should run, 0.94..=1.0: slower while it is
    /// ahead of the slowest peer, so skew is absorbed without a stall.
    pub fn speed(&self) -> f32 {
        if !self.tuning.dilation {
            return 1.0;
        }
        let me = self.lockstep.tick;
        let slowest = (0..self.peer_tick.len())
            .filter(|&p| p != self.me)
            .map(|p| self.peer_tick[p].saturating_sub(self.delay))
            .min()
            .unwrap_or(me);
        let ahead = me.saturating_sub(slowest) as f32;
        (1.0 - 0.02 * (ahead - 1.0).max(0.0)).clamp(0.94, 1.0)
    }
}

/// An in-process network: per-link one-way latency with jitter, loss and
/// the reordering jitter causes. Deterministic from its seed.
pub struct NetSim {
    pub latency_ms: f32,
    pub jitter_ms: f32,
    pub loss: f32,
    rng: u64,
    /// (deliver at ms, to, bytes)
    queue: Vec<(u32, usize, Vec<u8>)>,
}

impl NetSim {
    pub fn new(latency_ms: f32, jitter_ms: f32, loss: f32, seed: u64) -> NetSim {
        NetSim { latency_ms, jitter_ms, loss, rng: seed | 1, queue: Vec::new() }
    }
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f32 / (1u64 << 53) as f32
    }
    pub fn send(&mut self, to: usize, bytes: Vec<u8>, now: u32) {
        if self.rand() < self.loss {
            return;
        }
        // jitter: a skewed spread above the base latency
        let j = self.rand().powf(1.5) * self.jitter_ms * 2.5;
        self.queue.push(((now as f32 + self.latency_ms + j) as u32, to, bytes));
    }
    pub fn deliver(&mut self, now: u32) -> Vec<(usize, Vec<u8>)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.queue.len() {
            if self.queue[i].0 <= now {
                let (_, to, b) = self.queue.swap_remove(i);
                out.push((to, b));
            } else {
                i += 1;
            }
        }
        out
    }
}

/// The result of a simulated match between two machines.
#[derive(Debug, Clone)]
pub struct Trial {
    pub reaction_ms: Vec<f32>,
    pub stalled_frames: u32,
    pub frames: u32,
    pub desync: bool,
    pub ticks: [u32; 2],
    pub final_delay: [u32; 2],
    pub bytes_per_sec: f32,
}

impl Trial {
    pub fn pct(&self, p: f32) -> f32 {
        let mut v = self.reaction_ms.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if v.is_empty() {
            return f32::NAN;
        }
        v[((v.len() - 1) as f32 * p).round() as usize]
    }
}

/// Two machines play `secs` of a real 1v1 over `net` with `tuning`. Each
/// runs frames at 60 Hz of its own clock (the second 0.3% fast: real clocks
/// drift); presses change every half second. Returns the latency and stall
/// accounting, and whether the two worlds stayed identical.
pub fn trial(tuning: Tuning, mut net: NetSim, secs: f32) -> Trial {
    let mut games = [Game::new_teams(&[0], &[1]), Game::new_teams(&[0], &[1])];
    for g in games.iter_mut() {
        for god in g.gods.iter_mut() {
            god.ai = None;
        }
    }
    let mut s = [Session::new(0, 2, tuning), Session::new(1, 2, tuning)];
    let frame_ms = [TICK_MS, TICK_MS / 1.003];
    let mut next_frame = [0.0f32, 3.0];
    let mut acc = [0.0f32; 2];
    let mut frames = 0u32;
    let end = (secs * 1000.0) as u32;
    let mut now = 0u32;
    let mut rng = 0x2545F4914F6CDD1Du64;
    while now < end {
        for (to, bytes) in net.deliver(now) {
            if let Some(p) = Packet::decode(&bytes) {
                s[to].receive(&p, now);
            }
        }
        for m in 0..2 {
            if (now as f32) < next_frame[m] {
                continue;
            }
            next_frame[m] += frame_ms[m];
            frames += (m == 0) as u32;
            // time dilation: a machine ahead accumulates sim time slower
            acc[m] += s[m].speed();
            while acc[m] >= 1.0 {
                if !s[m].ready() {
                    s[m].stats.stalled_frames += 1;
                    acc[m] = acc[m].min(1.0);
                    break;
                }
                // this frame's input: a new press every ~0.5 s
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                let tick = s[m].lockstep.tick;
                let press = tick % 30 == (m as u32 * 7) % 30;
                let inp = Input { mv: [((rng >> 8) as i8) / 2, ((rng >> 16) as i8) / 2], attack: rng & 3 == 0, sneak: false, leap: false, cast: None };
                s[m].stamp(inp, press, now);
                s[m].step(&mut games[m], 1.0 / 60.0, now);
                games[m].events.clear(); // nobody draws them here
                acc[m] -= 1.0;
            }
            let peer = 1 - m;
            for p in s[m].packets_for(peer, now) {
                net.send(peer, p.encode(), now);
            }
        }
        now += 1;
    }
    let ok = s[0].lockstep.tick.min(s[1].lockstep.tick) > 0 && s[0].stats.desync.is_none() && s[1].stats.desync.is_none();
    let mut reaction = s[0].stats.reaction_ms.clone();
    reaction.extend(s[1].stats.reaction_ms.iter());
    Trial {
        reaction_ms: reaction,
        stalled_frames: s[0].stats.stalled_frames + s[1].stats.stalled_frames,
        frames: frames * 2,
        desync: !ok,
        ticks: [s[0].lockstep.tick, s[1].lockstep.tick],
        final_delay: [s[0].delay(), s[1].delay()],
        bytes_per_sec: s[0].stats.bytes_sent as f32 / secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packet_round_trips_and_garbage_is_refused() {
        let p = Packet {
            from: 1,
            first_tick: 1234,
            inputs: vec![Input { mv: [5, -7], attack: true, sneak: false, leap: true, cast: Some((2, [100, -40])) }; 3],
            stamp: 99_000,
            echo: 98_000,
            echo_hold: 12,
            check: Some((1200, 0xDEAD_BEEF_0123)),
        };
        assert_eq!(Packet::decode(&p.encode()), Some(p.clone()));
        let b = p.encode();
        assert!(Packet::decode(&b[..b.len() - 1]).is_none(), "truncated");
        assert!(Packet::decode(b"CBNX0000").is_none(), "bad magic");
        assert!(p.encode().len() <= 4 + 1 + 4 + 1 + 3 * WIRE_LEN + 10 + 13);
    }

    #[test]
    fn the_rtt_estimate_and_its_delay() {
        let mut r = Rtt::default();
        for _ in 0..30 {
            r.sample(4.0);
        }
        assert!((r.srtt - 4.0).abs() < 0.1 && r.delay_ticks() == 1, "LAN: {:?} -> {}", r, r.delay_ticks());
        let mut r = Rtt::default();
        for k in 0..60 {
            r.sample(if k % 2 == 0 { 50.0 } else { 90.0 });
        }
        let d = r.delay_ticks();
        assert!((3..=6).contains(&d), "70 +- 20 ms: {d} ticks");
    }

    /// Shrinking the delay never drops a press; growing it never leaves a
    /// tick without an input.
    #[test]
    fn delay_changes_keep_every_press_and_every_tick() {
        let mut s = Session::new(0, 1, Tuning { fixed_delay: Some(4), ..Tuning::TUNED });
        let mut game = Game::new_teams(&[0], &[1]);
        let cast = Input { cast: Some((1, [8, 8])), ..Default::default() };
        s.stamp(Input::default(), false, 0);
        s.step(&mut game, 1.0 / 60.0, 0);
        // shrink 4 -> 2 while pressing a cast: it must still be stamped
        s.delay = 2;
        s.stamp(cast, true, 16);
        assert!(s.carry.is_some(), "the cast is carried, not dropped");
        // it rides along until the stamps catch up with the shorter delay
        // (two ticks were already stamped), then lands exactly once
        for k in 0..3 {
            s.step(&mut game, 1.0 / 60.0, 16 + k);
            s.stamp(Input::default(), false, 33 + k);
        }
        assert_eq!(s.history.iter().filter(|(_, i)| i.cast.is_some()).count(), 1, "the carried cast was stamped exactly once");
        // grow 2 -> 6: every tick up to the new stamp has an input
        s.delay = 6;
        for k in 0..3 {
            s.step(&mut game, 1.0 / 60.0, 40 + k);
            s.stamp(Input::default(), false, 40 + k);
        }
        let last = s.last_stamp.unwrap();
        for t in s.lockstep.tick..=last {
            assert!(s.history.iter().any(|(ht, _)| *ht == t), "tick {t} left without an input");
        }
    }

    /// Two machines over a perfect link stay in lockstep with identical
    /// worlds, and the tuned stack settles on the minimum delay.
    #[test]
    fn a_clean_link_stays_in_sync_at_minimum_delay() {
        let t = trial(Tuning::TUNED, NetSim::new(1.0, 0.5, 0.0, 7), 20.0);
        assert!(!t.desync, "{t:?}");
        assert!(t.ticks[0] > 1000 && t.ticks[1] > 1000, "{:?}", t.ticks);
        assert_eq!(t.final_delay, [1, 1]);
        assert!(t.pct(0.5) <= 2.0 * TICK_MS + 1.0, "median reaction {} ms", t.pct(0.5));
    }

    /// The point of the tuning, on Wi-Fi-like conditions (15 ms +- jitter,
    /// 2% loss): faster reactions than the naive stack AND fewer stalls,
    /// and still in sync.
    #[test]
    fn tuned_beats_naive_on_wifi() {
        let wifi = || NetSim::new(15.0, 8.0, 0.02, 11);
        let naive = trial(Tuning::NAIVE, wifi(), 30.0);
        let tuned = trial(Tuning::TUNED, wifi(), 30.0);
        assert!(!naive.desync && !tuned.desync);
        assert!(tuned.pct(0.5) < naive.pct(0.5), "median: tuned {} vs naive {}", tuned.pct(0.5), naive.pct(0.5));
        assert!(tuned.pct(0.95) < naive.pct(0.95), "p95: tuned {} vs naive {}", tuned.pct(0.95), naive.pct(0.95));
        assert!(tuned.stalled_frames <= naive.stalled_frames, "stalls: tuned {} vs naive {}", tuned.stalled_frames, naive.stalled_frames);
    }
}
