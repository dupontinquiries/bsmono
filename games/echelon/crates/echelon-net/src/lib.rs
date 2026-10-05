//! Lockstep multiplayer core, transport-agnostic.
//!
//! Model: simulation ticks are grouped into fixed-size *command turns*.
//! Each client submits its local commands for turn `T + delay` while turn
//! `T` executes, so the network has `delay` turns of slack. A turn executes
//! only when every player's commands for it have arrived; merging is
//! deterministic (player 0's commands first, each player's own order kept).
//! Clients periodically exchange state hashes; any mismatch flags a desync
//! with the earliest offending checkpoint.
//!
//! This crate owns no sockets in its core — feed [`Lockstep::recv`] from any
//! transport (the runner's `--lockstep` mode wires two sessions back-to-back
//! in memory). [`udp::UdpLink`] is the UDP one: the same `Msg` values over a
//! datagram socket, re-sent until acknowledged.

pub mod udp;

use echelon_sim::PlayerCmd;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Msg {
    Cmds {
        turn: u32,
        player: u8,
        cmds: Vec<PlayerCmd>,
    },
    Hash {
        /// keyed by sim tick of the checkpoint
        tick: u32,
        player: u8,
        hash: u64,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct NetCfg {
    pub players: u8,
    /// sim ticks per command turn
    pub turn_len: u32,
    /// input delay in turns (>= 1)
    pub delay: u32,
}

pub struct Lockstep {
    pub cfg: NetCfg,
    pub me: u8,
    /// next turn to execute
    exec_turn: u32,
    /// next local turn to submit (already offset by delay on send)
    sent_turn: u32,
    cmds: BTreeMap<u32, Vec<Option<Vec<PlayerCmd>>>>,
    hashes: BTreeMap<u32, Vec<Option<u64>>>,
    pub desync: Option<u32>,
    outbox: Vec<Msg>,
}

impl Lockstep {
    pub fn new(cfg: NetCfg, me: u8) -> Lockstep {
        assert!(cfg.delay >= 1, "input delay must be >= 1 turn");
        let mut s = Lockstep {
            cfg,
            me,
            exec_turn: 0,
            sent_turn: 0,
            cmds: BTreeMap::new(),
            hashes: BTreeMap::new(),
            desync: None,
            outbox: Vec::new(),
        };
        // bootstrap: the first `delay` turns have no inputs by definition
        for t in 0..cfg.delay {
            let slot = s.slot(t);
            for p in 0..cfg.players as usize {
                slot[p] = Some(Vec::new());
            }
        }
        s
    }

    fn slot(&mut self, turn: u32) -> &mut Vec<Option<Vec<PlayerCmd>>> {
        let n = self.cfg.players as usize;
        self.cmds.entry(turn).or_insert_with(|| vec![None; n])
    }

    /// stage this client's commands for its next send-turn and queue the
    /// message for the transport. Call exactly once per local turn.
    pub fn submit_local(&mut self, cmds: Vec<PlayerCmd>) {
        let turn = self.sent_turn + self.cfg.delay;
        self.sent_turn += 1;
        let me = self.me;
        self.slot(turn)[me as usize] = Some(cmds.clone());
        self.outbox.push(Msg::Cmds {
            turn,
            player: me,
            cmds,
        });
    }

    pub fn recv(&mut self, msg: Msg) {
        match msg {
            Msg::Cmds { turn, player, cmds } => {
                // a turn already executed is a transport's re-send (`udp`), not input: it
                // must not grow the map back
                if player != self.me && player < self.cfg.players && turn >= self.exec_turn {
                    self.slot(turn)[player as usize] = Some(cmds);
                }
            }
            Msg::Hash { tick, player, hash } => {
                self.record_hash(tick, player, hash);
            }
        }
    }

    fn record_hash(&mut self, tick: u32, player: u8, hash: u64) {
        let n = self.cfg.players as usize;
        let e = self.hashes.entry(tick).or_insert_with(|| vec![None; n]);
        e[player as usize] = Some(hash);
        if e.iter().all(|h| h.is_some()) {
            let h0 = e[0].unwrap();
            if e.iter().any(|h| h.unwrap() != h0) && self.desync.is_none() {
                self.desync = Some(tick);
            }
            // bounded memory: drop confirmed checkpoints
            self.hashes.remove(&tick);
        }
    }

    /// report this client's state hash for a checkpoint tick.
    pub fn report_hash(&mut self, tick: u32, hash: u64) {
        let me = self.me;
        self.record_hash(tick, me, hash);
        self.outbox.push(Msg::Hash {
            tick,
            player: me,
            hash,
        });
    }

    pub fn ready(&self) -> bool {
        match self.cmds.get(&self.exec_turn) {
            Some(v) => v.iter().all(|c| c.is_some()),
            None => false,
        }
    }

    /// if every player's input for the next turn is here, pop it as one
    /// deterministic command list.
    pub fn next_turn(&mut self) -> Option<(u32, Vec<PlayerCmd>)> {
        if !self.ready() {
            return None;
        }
        let turn = self.exec_turn;
        let v = self.cmds.remove(&turn).unwrap();
        self.exec_turn += 1;
        let mut merged = Vec::new();
        for c in v.into_iter() {
            merged.extend(c.unwrap());
        }
        Some((turn, merged))
    }

    pub fn take_outbox(&mut self) -> Vec<Msg> {
        std::mem::take(&mut self.outbox)
    }

    /// The next turn to execute.
    pub fn exec_turn(&self) -> u32 {
        self.exec_turn
    }

    /// how many turns ahead of execution the local input stream is
    pub fn lead(&self) -> u32 {
        (self.sent_turn + self.cfg.delay).saturating_sub(self.exec_turn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echelon_sim::Cmd;

    fn cmd(p: u8, x: i32) -> PlayerCmd {
        PlayerCmd {
            player: p,
            cmd: Cmd::Move {
                ids: vec![0],
                mx: x,
                my: 0,
            },
        }
    }

    /// A re-sent `Cmds` for a turn already executed must not grow the map back (a transport
    /// re-sends until acked); an out-of-range player is ignored rather than indexed.
    #[test]
    fn a_stale_or_foreign_cmds_message_is_dropped() {
        let cfg = NetCfg { players: 2, turn_len: 4, delay: 1 };
        let mut a = Lockstep::new(cfg, 0);
        a.submit_local(vec![cmd(0, 1)]);
        a.recv(Msg::Cmds { turn: 1, player: 1, cmds: vec![cmd(1, 2)] });
        assert!(a.next_turn().is_some(), "turn 0, the bootstrap");
        assert!(a.next_turn().is_some(), "turn 1");
        assert_eq!(a.exec_turn(), 2);
        assert!(a.cmds.is_empty());
        a.recv(Msg::Cmds { turn: 1, player: 1, cmds: vec![cmd(1, 2)] });
        assert!(a.cmds.is_empty(), "the stale re-send grew nothing back");
        a.recv(Msg::Cmds { turn: 5, player: 7, cmds: vec![] });
        assert!(a.cmds.is_empty(), "a player off the roster is ignored");
    }

    #[test]
    fn two_clients_merge_identically() {
        let cfg = NetCfg {
            players: 2,
            turn_len: 4,
            delay: 2,
        };
        let mut a = Lockstep::new(cfg, 0);
        let mut b = Lockstep::new(cfg, 1);
        let mut got_a = Vec::new();
        let mut got_b = Vec::new();
        for turn in 0..20u32 {
            let ca = if turn % 3 == 0 {
                vec![cmd(0, turn as i32)]
            } else {
                vec![]
            };
            let cb = if turn % 4 == 0 {
                vec![cmd(1, -(turn as i32))]
            } else {
                vec![]
            };
            a.submit_local(ca);
            b.submit_local(cb);
            // b's packets arrive one exchange late to exercise buffering
            for m in a.take_outbox() {
                b.recv(m);
            }
            if turn > 0 {
                for m in b.take_outbox() {
                    a.recv(m);
                }
            }
            while let Some(t) = a.next_turn() {
                got_a.push(t);
            }
            while let Some(t) = b.next_turn() {
                got_b.push(t);
            }
        }
        assert!(got_a.len() >= 15, "turns executed: {}", got_a.len());
        let fmt = |v: &Vec<(u32, Vec<PlayerCmd>)>| format!("{:?}", v);
        // b is one exchange behind, so compare the common prefix
        let n = got_a.len().min(got_b.len());
        assert_eq!(fmt(&got_a[..n].to_vec()), fmt(&got_b[..n].to_vec()));
    }

    #[test]
    fn desync_detected() {
        let cfg = NetCfg {
            players: 2,
            turn_len: 4,
            delay: 1,
        };
        let mut a = Lockstep::new(cfg, 0);
        let mut b = Lockstep::new(cfg, 1);
        a.report_hash(224, 0xdead);
        b.report_hash(224, 0xbeef);
        for m in a.take_outbox() {
            b.recv(m);
        }
        for m in b.take_outbox() {
            a.recv(m);
        }
        assert_eq!(a.desync, Some(224));
        assert_eq!(b.desync, Some(224));
    }

    #[test]
    fn matching_hashes_pass() {
        let cfg = NetCfg {
            players: 2,
            turn_len: 4,
            delay: 1,
        };
        let mut a = Lockstep::new(cfg, 0);
        let mut b = Lockstep::new(cfg, 1);
        for tick in [224u32, 448, 672] {
            a.report_hash(tick, 0x1234 + tick as u64);
            b.report_hash(tick, 0x1234 + tick as u64);
            for m in a.take_outbox() {
                b.recv(m);
            }
            for m in b.take_outbox() {
                a.recv(m);
            }
        }
        assert_eq!(a.desync, None);
        assert_eq!(b.desync, None);
    }
}
