//! **World authority** — the command-sourced world model behind persistence and
//! anti-cheat. Clients send *commands* ("I placed block X at Y"), never world
//! state. The authority (host, or in P2P the quorum-agreed history) validates
//! each command against *canonical* state and applies it; clients keep only a
//! predicted replica for responsiveness.
//!
//! Why commands, not state: merging divergent world *data* from clients is the
//! lazy-merge trap — it's where exploits live. With commands, a build survives
//! iff its commands were legal against canonical state, independent of the
//! client's (possibly diverged or cheated) local copy. A desync-flagged player is
//! resynced and their commands re-validated against truth — over-spends they only
//! "had" locally get rejected on replay; legit builds stand. See the tests.
//!
//! ## Saving
//! Persist a [`Snapshot`] (canonical state + tick + digest) plus the command log
//! after it. [`replay`] folds the log over a snapshot and reproduces the world
//! exactly — so a save is a snapshot + tail log, replayable and auditable, not a
//! merge of client worlds. Host migration seeds the new host from the last
//! snapshot + relayed log.
//!
//! Determinism: state is kept in `BTreeMap` (canonical key order), and [`apply`]
//! is integer-only, so [`digest`] agrees across peers and replay is exact — the
//! same boundary the rollback netcode relies on.

use crate::sim::gpu_bench::{bench, BenchResult};
use std::collections::BTreeMap;

pub type PlayerId = u64;
pub type BlockId = u16;
pub type Pos = (i32, i32, i32);

/// Placeholder flat cost per block (a real game keys cost by block/recipe).
pub const COST: u32 = 1;

/// A client-issued command. The authority validates it against canonical state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldCmd {
    Place { pos: Pos, block: BlockId },
    Remove { pos: Pos },
    /// Resource credited to a player (already authority-validated, e.g. a gather).
    Grant { amount: u32 },
}

/// Why a command was rejected (state left unchanged).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    NoResource,
    Occupied,
    Empty,
}

/// Authoritative world state. `BTreeMap` (not `HashMap`) so iteration — and thus
/// [`digest`](CanonicalWorld::digest) and replay — is canonical and deterministic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CanonicalWorld {
    pub blocks: BTreeMap<Pos, BlockId>,
    pub resources: BTreeMap<PlayerId, u32>,
}

#[inline]
fn mix(h: u64, x: u64) -> u64 {
    let mut z = h ^ x.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl CanonicalWorld {
    pub fn new() -> Self {
        CanonicalWorld::default()
    }

    fn res(&self, player: PlayerId) -> u32 {
        *self.resources.get(&player).unwrap_or(&0)
    }

    /// Validate `cmd` from `player` against canonical state and apply it. On
    /// `Err`, state is unchanged. Checks resource before occupancy (deterministic
    /// order).
    pub fn apply(&mut self, player: PlayerId, cmd: WorldCmd) -> Result<(), Reject> {
        match cmd {
            WorldCmd::Place { pos, block } => {
                if self.res(player) < COST {
                    return Err(Reject::NoResource);
                }
                if self.blocks.contains_key(&pos) {
                    return Err(Reject::Occupied);
                }
                self.blocks.insert(pos, block);
                self.resources.insert(player, self.res(player) - COST);
                Ok(())
            }
            WorldCmd::Remove { pos } => {
                if self.blocks.remove(&pos).is_none() {
                    return Err(Reject::Empty);
                }
                self.resources.insert(player, self.res(player) + COST);
                Ok(())
            }
            WorldCmd::Grant { amount } => {
                self.resources.insert(player, self.res(player) + amount);
                Ok(())
            }
        }
    }

    /// Canonical 64-bit digest over the (sorted) blocks and resources — the same
    /// kind of proof the rollback consensus compares.
    pub fn digest(&self) -> u64 {
        let mut h = 0xCBF2_9CE4_8422_2325u64;
        for (pos, bid) in &self.blocks {
            h = mix(h, pos.0 as u32 as u64);
            h = mix(h, pos.1 as u32 as u64);
            h = mix(h, pos.2 as u32 as u64);
            h = mix(h, *bid as u64);
        }
        for (p, r) in &self.resources {
            h = mix(h, *p);
            h = mix(h, *r as u64);
        }
        h
    }
}

/// A persisted point: canonical state + tick + digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub tick: u64,
    pub world: CanonicalWorld,
    pub digest: u64,
}

impl Snapshot {
    pub fn of(tick: u64, world: &CanonicalWorld) -> Self {
        Snapshot { tick, world: world.clone(), digest: world.digest() }
    }
}

/// Fold a command log over a snapshot, reproducing canonical state. Returns the
/// resulting world and the list of rejected commands (audit trail). Deterministic.
pub fn replay(snapshot: &CanonicalWorld, log: &[(PlayerId, WorldCmd)]) -> (CanonicalWorld, Vec<(PlayerId, WorldCmd, Reject)>) {
    let mut world = snapshot.clone();
    let mut rejects = Vec::new();
    for &(player, cmd) in log {
        if let Err(r) = world.apply(player, cmd) {
            rejects.push((player, cmd, r));
        }
    }
    (world, rejects)
}

/// Benchmark a full replay over `iters` runs.
pub fn bench_replay(snapshot: &CanonicalWorld, log: &[(PlayerId, WorldCmd)], iters: usize) -> BenchResult {
    bench(iters, || {
        let (w, _) = replay(snapshot, log);
        std::hint::black_box(&w);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(p: Pos, b: BlockId) -> WorldCmd {
        WorldCmd::Place { pos: p, block: b }
    }

    fn sample_log() -> Vec<(PlayerId, WorldCmd)> {
        vec![
            (1, WorldCmd::Grant { amount: 3 }),
            (1, place((0, 0, 0), 7)),
            (1, place((1, 0, 0), 7)),
            (2, WorldCmd::Grant { amount: 1 }),
            (2, place((5, 5, 5), 9)),
        ]
    }

    #[test]
    fn replay_deterministic_and_digest_stable() {
        let (a, _) = replay(&CanonicalWorld::new(), &sample_log());
        let (b, _) = replay(&CanonicalWorld::new(), &sample_log());
        assert_eq!(a, b);
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn place_without_resource_rejected() {
        let (w, rej) = replay(&CanonicalWorld::new(), &[(1, place((0, 0, 0), 7))]);
        assert_eq!(rej.len(), 1);
        assert_eq!(rej[0].2, Reject::NoResource);
        assert!(w.blocks.is_empty());
    }

    #[test]
    fn place_on_occupied_rejected() {
        let log = vec![(1, WorldCmd::Grant { amount: 2 }), (1, place((0, 0, 0), 7)), (1, place((0, 0, 0), 8))];
        let (w, rej) = replay(&CanonicalWorld::new(), &log);
        assert!(rej.iter().any(|r| r.2 == Reject::Occupied));
        assert_eq!(w.blocks.get(&(0, 0, 0)), Some(&7)); // original block kept
    }

    #[test]
    fn remove_empty_rejected_and_refund_works() {
        let (_, rej) = replay(&CanonicalWorld::new(), &[(1, WorldCmd::Remove { pos: (9, 9, 9) })]);
        assert_eq!(rej[0].2, Reject::Empty);
        let log = vec![(1, WorldCmd::Grant { amount: 2 }), (1, place((0, 0, 0), 1)), (1, WorldCmd::Remove { pos: (0, 0, 0) })];
        let (w, _) = replay(&CanonicalWorld::new(), &log);
        assert_eq!(w.resources.get(&1), Some(&2)); // spent then refunded
        assert!(w.blocks.is_empty());
    }

    #[test]
    fn snapshot_plus_tail_equals_full_replay() {
        let log = sample_log();
        let (full, _) = replay(&CanonicalWorld::new(), &log);
        let (mid, _) = replay(&CanonicalWorld::new(), &log[..3]);
        let snap = Snapshot::of(3, &mid);
        let (tail, _) = replay(&snap.world, &log[3..]);
        assert_eq!(tail, full);
        assert_eq!(tail.digest(), full.digest());
    }

    #[test]
    fn cheated_overspend_rejected_on_replay_legit_survives() {
        // canonical grant is only 1, so the 2nd place (no resource left) is a cheat.
        let log = vec![
            (3, WorldCmd::Grant { amount: 1 }),
            (3, place((2, 2, 2), 5)), // legit
            (3, place((2, 3, 2), 5)), // over-spend
        ];
        let (w, rej) = replay(&CanonicalWorld::new(), &log);
        assert_eq!(w.blocks.get(&(2, 2, 2)), Some(&5), "legit build stands");
        assert!(!w.blocks.contains_key(&(2, 3, 2)), "cheat discarded");
        assert!(rej.iter().any(|r| r.1 == place((2, 3, 2), 5) && r.2 == Reject::NoResource));
    }

    #[test]
    fn digest_is_sensitive() {
        let (a, _) = replay(&CanonicalWorld::new(), &[(1, WorldCmd::Grant { amount: 1 }), (1, place((0, 0, 0), 1))]);
        let (b, _) = replay(&CanonicalWorld::new(), &[(1, WorldCmd::Grant { amount: 1 }), (1, place((0, 0, 0), 2))]);
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn bench_runs() {
        let log = sample_log();
        let br = bench_replay(&CanonicalWorld::new(), &log, 3);
        assert_eq!(br.samples.len(), 3);
        assert!(br.metric_line("world_replay", None).contains("world_replay n=3"));
    }
}
