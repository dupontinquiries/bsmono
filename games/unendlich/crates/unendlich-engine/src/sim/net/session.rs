//! Session logic — pure, transport-agnostic glue between the wire protocol
//! and the [`Rollback`] engine. No sockets here: it consumes and produces
//! byte slices, so it can be exhaustively tested against a real [`Rollback`]
//! with an in-memory channel (see `transport::loopback_pair`).
//!
//! Responsibilities:
//! - Keep a small history of the local player's recent inputs and resend the
//!   last `redundancy` of them in every packet, so packet loss self-heals.
//! - Track the peer's `ack` (its confirmed tick) and prune local history the
//!   peer no longer needs.
//! - Feed received remote inputs into [`Rollback::set_input`], where stale and
//!   duplicate resends are ignored idempotently.
//! - Record our own confirmed-tick checksums and compare them against the
//!   peer's `Sync` packets to detect a divergence.

use std::collections::BTreeMap;

use crate::sim::physics::WorldQuery;
use crate::sim::rollback::{PlayerId, Rollback};
use crate::sim::state::Input;

use super::wire::{decode, encode_input_packet, encode_sync_packet, Packet};

/// Outcome of feeding one received datagram to [`Session::ingest`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ingested {
    /// An input batch was applied; carries how many inputs were fed in.
    Inputs(usize),
    /// A sync packet that matched (or referenced a tick we can't compare yet).
    Synced,
    /// A sync packet whose checksum disagreed with ours at that tick.
    Desync { tick: u64, ours: u64, theirs: u64 },
    /// The bytes did not decode to a valid packet.
    Malformed,
}

/// Per-peer session state. One of these per remote peer (two-player sessions
/// have a single one).
pub struct Session {
    local_player: PlayerId,
    num_players: usize,
    redundancy: usize,
    /// Local inputs the peer has not yet confirmed, keyed by tick (contiguous).
    local_history: BTreeMap<u64, Input>,
    /// Highest confirmed tick the peer has reported (its `ack`).
    remote_ack: u64,
    /// Our confirmed-tick checksums, awaiting comparison against peer syncs.
    local_checksums: BTreeMap<u64, u64>,
    desync: Option<(u64, u64, u64)>,
}

impl Session {
    /// `redundancy` is how many recent local inputs to repeat per packet —
    /// the largest packet-loss burst that still self-heals. 8–16 is typical.
    pub fn new(local_player: PlayerId, num_players: usize, redundancy: usize) -> Session {
        Session {
            local_player,
            num_players,
            redundancy: redundancy.max(1),
            local_history: BTreeMap::new(),
            remote_ack: 0,
            local_checksums: BTreeMap::new(),
            desync: None,
        }
    }

    /// Remember a local input so it can be (re)sent. Call once per tick with
    /// the same value you fed to [`Rollback::set_local_input`].
    pub fn record_local(&mut self, tick: u64, input: Input) {
        self.local_history.insert(tick, input);
    }

    /// Build the outgoing input datagram: the last `redundancy` local inputs
    /// plus `ack` (pass [`Rollback::confirmed_tick`]). `None` if we have not
    /// recorded any local input yet.
    pub fn outgoing_input_packet(&self, ack: u64) -> Option<Vec<u8>> {
        if self.local_history.is_empty() {
            return None;
        }
        let entries: Vec<(&u64, &Input)> = self.local_history.iter().collect();
        let start_idx = entries.len().saturating_sub(self.redundancy);
        let window = &entries[start_idx..];
        let start_tick = *window[0].0;
        let inputs: Vec<Input> = window.iter().map(|(_, i)| **i).collect();
        Some(encode_input_packet(self.local_player as u8, ack, start_tick, &inputs))
    }

    /// Build a sync datagram advertising our confirmed checksum.
    pub fn outgoing_sync_packet(&self, rb: &Rollback) -> Vec<u8> {
        encode_sync_packet(rb.confirmed_tick(), rb.confirmed_checksum())
    }

    /// Record our confirmed-tick checksum so later peer syncs can be checked.
    /// Call after advancing the rollback each frame.
    pub fn note_confirmed(&mut self, rb: &Rollback) {
        self.local_checksums.insert(rb.confirmed_tick(), rb.confirmed_checksum());
        self.prune_checksums();
    }

    /// Decode one datagram and apply it. Remote inputs flow into `rb`.
    pub fn ingest<W: WorldQuery>(
        &mut self,
        bytes: &[u8],
        rb: &mut Rollback,
        world: &W,
    ) -> Ingested {
        match decode(bytes) {
            None => Ingested::Malformed,
            Some(Packet::Input { player, ack, start_tick, inputs }) => {
                if ack > self.remote_ack {
                    self.remote_ack = ack;
                    self.prune_history();
                    self.prune_checksums();
                }
                let p = player as usize;
                // Only a peer's own player's inputs are authoritative, and we
                // never let the wire overwrite our local prediction authority.
                if p >= self.num_players || p == self.local_player {
                    return Ingested::Inputs(0);
                }
                let mut applied = 0;
                for (i, inp) in inputs.iter().enumerate() {
                    rb.set_input(start_tick + i as u64, p, *inp, world);
                    applied += 1;
                }
                Ingested::Inputs(applied)
            }
            Some(Packet::Sync { tick, checksum }) => match self.local_checksums.get(&tick) {
                Some(&ours) if ours != checksum => {
                    self.desync = Some((tick, ours, checksum));
                    Ingested::Desync { tick, ours, theirs: checksum }
                }
                _ => Ingested::Synced,
            },
        }
    }

    /// The peer's last reported confirmed tick.
    pub fn remote_ack(&self) -> u64 {
        self.remote_ack
    }

    /// The first detected desync, if any: `(tick, ours, theirs)`.
    pub fn desync(&self) -> Option<(u64, u64, u64)> {
        self.desync
    }

    fn prune_history(&mut self) {
        let cutoff = self.remote_ack;
        loop {
            match self.local_history.keys().next().copied() {
                Some(t) if t < cutoff => {
                    self.local_history.remove(&t);
                }
                _ => break,
            }
        }
    }

    fn prune_checksums(&mut self) {
        let cutoff = self.remote_ack;
        loop {
            match self.local_checksums.keys().next().copied() {
                Some(t) if t < cutoff => {
                    self.local_checksums.remove(&t);
                }
                _ => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Fixed;
    use crate::sim::physics::FlatWorld;
    use crate::sim::state::{checksum, step, SimState, BTN_JUMP};

    fn mv(mx: i64, buttons: u32) -> Input {
        Input { buttons, move_x: Fixed::from_int(mx), move_z: Fixed::ZERO, yaw: 0 }
    }

    /// Offline reference: both players' inputs known on time, no networking.
    fn offline(p0: &[Input], p1: &[Input], world: &FlatWorld) -> u64 {
        let mut s = SimState::new(2);
        for t in 0..p0.len() {
            s = step(&s, &[p0[t], p1[t]], world);
        }
        checksum(&s)
    }

    /// One peer (player 0) drives a rollback while player 1's inputs arrive
    /// over the wire in redundant packets through a lossy channel. As long as
    /// no loss burst exceeds the redundancy window, every input is delivered
    /// and the result matches the offline run.
    #[test]
    fn redundant_packets_survive_bursty_loss() {
        let world = FlatWorld::new();
        let ticks = 200usize;
        let redundancy = 8;
        let p0: Vec<Input> = (0..ticks).map(|t| mv(1, if t % 20 == 0 { BTN_JUMP } else { 0 })).collect();
        let p1: Vec<Input> = (0..ticks).map(|t| mv(if (t / 5) % 2 == 0 { 1 } else { -1 }, 0)).collect();

        // The remote peer's session: it is player 1, resending the last 8.
        let mut remote = Session::new(1, 2, redundancy);
        // The local peer's session + rollback: it is player 0.
        let mut local = Session::new(0, 2, redundancy);
        let mut rb = Rollback::new(2, SimState::new(2), (redundancy + 4) as u64);

        // Deterministic pseudo-random loss: drop ~40% of packets, but never
        // `redundancy` in a row (that is the documented breaking point).
        let mut lcg: u64 = 0x1234_5678;
        let mut consecutive_drops = 0;
        let mut next_drop = |consecutive: &mut i32| {
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let drop = (lcg >> 40) % 10 < 4; // ~40%
            if drop && *consecutive < (redundancy as i32 - 1) {
                *consecutive += 1;
                true
            } else {
                *consecutive = 0;
                false
            }
        };

        for t in 0..ticks {
            // Local player records + applies its own input on time.
            local.record_local(t as u64, p0[t]);
            rb.set_local_input(0, p0[t], &world);

            // Remote peer records its input and emits a redundant packet.
            remote.record_local(t as u64, p1[t]);
            let pkt = remote.outgoing_input_packet(0).unwrap();

            // Lossy channel.
            if !next_drop(&mut consecutive_drops) {
                local.ingest(&pkt, &mut rb, &world);
            }

            rb.advance(&world);
        }
        // Flush: deliver the final window so the tail confirms.
        let final_pkt = remote.outgoing_input_packet(0).unwrap();
        local.ingest(&final_pkt, &mut rb, &world);
        // Drain any remaining prediction window.
        while rb.confirmed_tick() < ticks as u64 {
            if !rb.advance(&world) {
                break;
            }
        }

        assert_eq!(rb.confirmed_tick(), ticks as u64, "all inputs delivered despite loss");
        assert_eq!(rb.checksum(), offline(&p0, &p1, &world), "matches the on-time run");
        let _ = consecutive_drops;
    }

    #[test]
    fn ack_prunes_local_history() {
        let mut s = Session::new(0, 2, 4);
        for t in 0..10u64 {
            s.record_local(t, mv(1, 0));
        }
        // Peer acks through tick 6: history below 6 should be dropped on ingest.
        let ack_pkt = encode_input_packet(1, 6, 0, &[mv(0, 0)]);
        let world = FlatWorld::new();
        let mut rb = Rollback::new(2, SimState::new(2), 16);
        s.ingest(&ack_pkt, &mut rb, &world);
        assert_eq!(s.remote_ack(), 6);
        // Earliest retained local input is now tick 6.
        let pkt = s.outgoing_input_packet(0).unwrap();
        match decode(&pkt).unwrap() {
            Packet::Input { start_tick, .. } => assert!(start_tick >= 6),
            _ => panic!("expected input packet"),
        }
    }

    #[test]
    fn matching_sync_is_fine_mismatch_is_a_desync() {
        let world = FlatWorld::new();
        let mut s = Session::new(0, 2, 4);
        let mut rb = Rollback::new(2, SimState::new(2), 16);
        // Confirm a few ticks so we have a recorded checksum at tick 0+.
        for t in 0..3u64 {
            rb.set_input(t, 0, mv(1, 0), &world);
            rb.set_input(t, 1, mv(0, 0), &world);
            rb.advance(&world);
            s.note_confirmed(&rb);
        }
        let good_tick = 1u64;
        let ours = *s.local_checksums.get(&good_tick).unwrap();
        // A correct sync from a peer in lockstep:
        assert_eq!(
            s.ingest(&encode_sync_packet(good_tick, ours), &mut rb, &world),
            Ingested::Synced
        );
        // A diverged peer:
        let bad = s.ingest(&encode_sync_packet(good_tick, ours ^ 0xff), &mut rb, &world);
        assert_eq!(bad, Ingested::Desync { tick: good_tick, ours, theirs: ours ^ 0xff });
        assert!(s.desync().is_some());
    }

    #[test]
    fn garbage_bytes_are_reported_not_panicked() {
        let world = FlatWorld::new();
        let mut s = Session::new(0, 2, 4);
        let mut rb = Rollback::new(2, SimState::new(2), 16);
        assert_eq!(s.ingest(&[1, 2, 3], &mut rb, &world), Ingested::Malformed);
        assert_eq!(s.ingest(&[], &mut rb, &world), Ingested::Malformed);
    }
}
