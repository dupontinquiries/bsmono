//! Deterministic simulation state and the pure step function.
//!
//! The simulation advances in fixed ticks (60 Hz). A tick is a pure
//! function `step(state, inputs, world) -> state`: given the same prior
//! state, inputs, and (read-only, deterministic) world it always yields the
//! same next state, bit-for-bit, on
//! any machine (everything is [`Fixed`], never float). That purity is what
//! makes rollback possible — see [`crate::sim::rollback`].
//!
//! Scope: the rolled-back simulation holds **entities** (characters now;
//! mobs/projectiles later), never the voxel world. Block edits will enter
//! as deterministic events in the input stream; the terrain itself is
//! synced separately and is not snapshotted per tick.
//!
//! Per-character physics (gravity, movement, voxel collision) lives in
//! [`crate::sim::physics`]; this module owns the tick/input/state types and
//! the orchestrating [`step`], which is generic over a read-only
//! [`WorldQuery`] so collision can read real terrain while staying pure.

use super::fixed::{Fixed, Vec3};
use super::physics::WorldQuery;

/// Simulation rate. One tick is `1/TICK_HZ` seconds. Physics constants
/// below are expressed *per tick*, so the step never multiplies by a
/// `dt` value — fewer fixed-point ops, and no place for a stray dt to
/// drift between peers.
pub const TICK_HZ: i64 = 60;

// ── Placeholder physics constants (per tick) ───────────────────────────
// Tuning values, not final feel. Velocities are in blocks/tick; gravity
// is the per-tick change to vertical velocity. All derived via integer
// `from_frac` so they're identical on every platform.

/// Gravity applied to vertical velocity each tick (~20 blocks/s²).
pub const GRAVITY_PER_TICK: Fixed = Fixed::from_frac(-20, TICK_HZ * TICK_HZ);
/// Maximum horizontal speed (~5 blocks/s).
pub const MAX_SPEED: Fixed = Fixed::from_frac(5, TICK_HZ);
/// Horizontal acceleration toward the input target per tick
/// (reaches `MAX_SPEED` in ~8 ticks).
pub const MOVE_ACCEL: Fixed = Fixed::from_frac(5, TICK_HZ * 8);
/// Upward velocity imparted by a jump (~7 blocks/s launch).
pub const JUMP_VEL: Fixed = Fixed::from_frac(7, TICK_HZ);

/// A simulation tick index.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Tick(pub u64);

impl Tick {
    #[inline]
    pub const fn next(self) -> Tick {
        Tick(self.0 + 1)
    }
}

// ── Input ──────────────────────────────────────────────────────────────

/// Button bit: jump.
pub const BTN_JUMP: u32 = 1 << 0;
/// Button bit: sprint.
pub const BTN_SPRINT: u32 = 1 << 1;
/// Button bit: primary action (place/use). Reserved for later bricks.
pub const BTN_ACTION_PRIMARY: u32 = 1 << 2;
/// Button bit: secondary action (break). Reserved for later bricks.
pub const BTN_ACTION_SECONDARY: u32 = 1 << 3;

/// One player's intent for one tick.
///
/// This is the single source of truth that travels over the network, so it
/// must be fully deterministic. The horizontal move direction is supplied
/// as fixed-point components already resolved from the camera by the
/// *presentation* layer — the sim never runs trig. Components are expected
/// in `[-1, 1]`; `yaw` is the facing angle quantized to the full `u16`
/// range (`0..=65535` maps to `0..2π`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Input {
    pub buttons: u32,
    pub move_x: Fixed,
    pub move_z: Fixed,
    pub yaw: u16,
}

impl Input {
    #[inline]
    pub const fn is_held(self, btn: u32) -> bool {
        (self.buttons & btn) != 0
    }
}

// ── Entities & state ───────────────────────────────────────────────────

/// A simulated character (player or, later, an AI-driven entity).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Character {
    pub pos: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    pub yaw: u16,
}

/// The complete rolled-back simulation state at a given tick.
///
/// Cheap to clone (a handful of entities) — that's what lets the rollback
/// engine snapshot every tick. Deliberately does **not** contain the voxel
/// world.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SimState {
    pub tick: Tick,
    pub players: Vec<Character>,
    /// AI-driven entities, advanced after players each tick. Empty by default,
    /// so states with no mobs behave (and checksum) exactly as before.
    pub mobs: Vec<crate::sim::mob::Mob>,
    /// Per-world seed feeding deterministic mob brains. Constant for a run;
    /// part of the state so it snapshots and rolls back with everything else.
    pub world_seed: u64,
}

impl SimState {
    /// A fresh state with `n` characters at the origin, ready for tick 0.
    pub fn new(n: usize) -> SimState {
        SimState {
            tick: Tick(0),
            players: vec![Character::default(); n],
            mobs: Vec::new(),
            world_seed: 0,
        }
    }

    /// Like [`SimState::new`] but with an explicit `world_seed` for mob AI.
    pub fn new_seeded(n: usize, world_seed: u64) -> SimState {
        SimState { world_seed, ..SimState::new(n) }
    }
}

/// Advance the simulation by exactly one tick against the voxel `world`.
///
/// Pure: depends only on `state`, `inputs`, and the read-only, deterministic
/// `world`. `inputs[i]` drives `state.players[i]`; a missing entry (shorter
/// slice) is treated as the default (idle) input, so the function is total.
/// The per-character physics kernel lives in
/// [`crate::sim::physics::step_character`].
pub fn step<W: WorldQuery>(state: &SimState, inputs: &[Input], world: &W) -> SimState {
    let mut players = state.players.clone();
    for (i, ch) in players.iter_mut().enumerate() {
        let input = inputs.get(i).copied().unwrap_or_default();
        *ch = super::physics::step_character(ch, &input, world);
    }
    // Mobs decide their own input (deterministic brain) and move through the
    // identical character kernel. The mob's index is its stable id for the
    // tick's PRNG seed.
    let mut mobs = state.mobs.clone();
    // Start-of-tick position snapshots for predator-prey sensing. Taken before
    // the mutable loop so every mob perceives the same frame (no order
    // dependence) and there's no aliasing while iterating `mobs` mutably.
    let prey: Vec<super::fixed::Vec3> = state.mobs.iter()
        .filter(|m| m.kind == super::mob::MobKind::Grazer)
        .map(|m| m.body.pos)
        .collect();
    let predators: Vec<super::fixed::Vec3> = state.mobs.iter()
        .filter(|m| m.kind == super::mob::MobKind::Seeker)
        .map(|m| m.body.pos)
        .collect();
    for (i, m) in mobs.iter_mut().enumerate() {
        let input = m.brain(state.world_seed, i as u64, state.tick.0, &players, &prey, &predators, world);
        m.body = super::physics::step_character(&m.body, &input, world);
    }
    // Predation: a Seeker in contact (BITE_RANGE) with a Grazer bites it, from a
    // post-move snapshot of seeker positions so resolution is order-independent.
    // Then dead mobs (health <= 0) are removed. Both are pure functions of the
    // post-move state, so determinism / rollback equivalence is preserved (the
    // checksum already folds health, and a removal deterministically shifts the
    // index-seeded PRNG identically on every replay).
    let seeker_positions: Vec<super::fixed::Vec3> = mobs.iter()
        .filter(|m| m.kind == super::mob::MobKind::Seeker)
        .map(|m| m.body.pos)
        .collect();
    if !seeker_positions.is_empty() {
        for m in mobs.iter_mut() {
            if m.kind == super::mob::MobKind::Grazer
                && super::mob::within_bite_range(m.body.pos, &seeker_positions)
            {
                m.health = m.health.saturating_sub(super::mob::BITE_DAMAGE);
            }
        }
    }
    mobs.retain(|m| m.health > 0);
    SimState {
        tick: state.tick.next(),
        players,
        mobs,
        world_seed: state.world_seed,
    }
}

/// A deterministic 64-bit checksum of a simulation state.
///
/// Used to detect desyncs (peers exchange checksums for confirmed ticks)
/// and to assert rollback equivalence in tests. This is a fixed FNV-1a-style
/// fold over the integer fields — *not* `std::hash::Hash`, whose output
/// isn't guaranteed stable across compiler versions.
pub fn checksum(state: &SimState) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    #[inline]
    fn fold(h: u64, v: u64) -> u64 {
        (h ^ v).wrapping_mul(FNV_PRIME)
    }

    let mut h = FNV_OFFSET;
    h = fold(h, state.tick.0);
    h = fold(h, state.players.len() as u64);
    for ch in &state.players {
        h = fold(h, ch.pos.x.0 as u64);
        h = fold(h, ch.pos.y.0 as u64);
        h = fold(h, ch.pos.z.0 as u64);
        h = fold(h, ch.vel.x.0 as u64);
        h = fold(h, ch.vel.y.0 as u64);
        h = fold(h, ch.vel.z.0 as u64);
        h = fold(h, ch.on_ground as u64);
        h = fold(h, ch.yaw as u64);
    }
    // Mobs fold in after players. An empty mob list folds nothing, so states
    // without mobs keep their exact prior checksum (backward compatible).
    for m in &state.mobs {
        for w in m.checksum_words() {
            h = fold(h, w);
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::physics::FlatWorld;

    fn idle() -> Input {
        Input::default()
    }

    fn move_input(mx: i64, mz: i64, buttons: u32) -> Input {
        Input {
            buttons,
            move_x: Fixed::from_int(mx),
            move_z: Fixed::from_int(mz),
            yaw: 0,
        }
    }

    #[test]
    fn mobs_step_and_are_reproducible() {
        let w = FlatWorld::new();
        let base = {
            let mut s = SimState::new(0);
            s.mobs.push(crate::sim::mob::Mob::new_walker(
                Vec3 { x: Fixed::ZERO, y: Fixed::from_int(6), z: Fixed::ZERO },
                20,
            ));
            s
        };
        let run = |start: &SimState| {
            let mut s = start.clone();
            for _ in 0..120 {
                s = step(&s, &[], &w);
            }
            s
        };
        assert_eq!(run(&base), run(&base), "same start + ticks must replay identically");
        assert_ne!(
            run(&base).mobs[0].body.pos,
            base.mobs[0].body.pos,
            "the mob should have moved/fallen"
        );
    }

    #[test]
    fn seeker_bites_and_eats_adjacent_grazer() {
        let w = FlatWorld::new();
        let mut s = SimState::new(0);
        // Seeker and grazer one block apart on flat ground (within BITE_RANGE).
        s.mobs.push(crate::sim::mob::Mob::new_seeker(
            Vec3 { x: Fixed::ZERO, y: Fixed::from_int(6), z: Fixed::ZERO }, 30));
        s.mobs.push(crate::sim::mob::Mob::new_grazer(
            Vec3 { x: Fixed::from_int(1), y: Fixed::from_int(6), z: Fixed::ZERO }, 10));

        // One step: the grazer takes a single bite.
        let s1 = step(&s, &[], &w);
        let hp = s1.mobs.iter()
            .find(|m| m.kind == crate::sim::mob::MobKind::Grazer)
            .map(|m| m.health);
        assert_eq!(hp, Some(10 - crate::sim::mob::BITE_DAMAGE), "grazer should be bitten once");

        // Enough steps: the grazer is eaten (removed); the seeker remains.
        let mut s = s;
        for _ in 0..60 { s = step(&s, &[], &w); }
        assert!(s.mobs.iter().all(|m| m.kind != crate::sim::mob::MobKind::Grazer),
            "grazer should have been eaten and removed");
        assert!(s.mobs.iter().any(|m| m.kind == crate::sim::mob::MobKind::Seeker),
            "seeker should remain after eating");
        // Predation is deterministic: replay matches.
        let replay = {
            let mut r = SimState::new(0);
            r.mobs.push(crate::sim::mob::Mob::new_seeker(
                Vec3 { x: Fixed::ZERO, y: Fixed::from_int(6), z: Fixed::ZERO }, 30));
            r.mobs.push(crate::sim::mob::Mob::new_grazer(
                Vec3 { x: Fixed::from_int(1), y: Fixed::from_int(6), z: Fixed::ZERO }, 10));
            let mut r = r;
            for _ in 0..60 { r = step(&r, &[], &w); }
            r
        };
        assert_eq!(checksum(&s), checksum(&replay), "predation must replay identically");
    }

    #[test]
    fn lone_grazer_is_not_bitten() {
        // No predator → a grazer keeps full health (predation never fires).
        let w = FlatWorld::new();
        let mut s = SimState::new(0);
        s.mobs.push(crate::sim::mob::Mob::new_grazer(
            Vec3 { x: Fixed::ZERO, y: Fixed::from_int(6), z: Fixed::ZERO }, 10));
        let mut s = s;
        for _ in 0..30 { s = step(&s, &[], &w); }
        assert_eq!(s.mobs.len(), 1, "lone grazer should survive");
        assert_eq!(s.mobs[0].health, 10, "lone grazer should keep full health");
    }

    #[test]
    fn checksum_is_empty_mob_invariant_and_mob_sensitive() {
        // No mobs → identical checksum to a players-only state (empty fold).
        let players_only = SimState::new(2);
        let mut also_empty = SimState::new(2);
        also_empty.mobs.clear();
        assert_eq!(checksum(&players_only), checksum(&also_empty));
        // Adding a mob changes the checksum.
        let mut with_mob = SimState::new(2);
        with_mob.mobs.push(crate::sim::mob::Mob::new_walker(
            Vec3 { x: Fixed::from_int(1), y: Fixed::from_int(2), z: Fixed::ZERO },
            20,
        ));
        assert_ne!(checksum(&players_only), checksum(&with_mob));
    }

    #[test]
    fn step_is_deterministic() {
        let w = FlatWorld::new();
        let s = SimState::new(2);
        let ins = [move_input(1, 0, BTN_JUMP), idle()];
        let a = step(&s, &ins, &w);
        let b = step(&s, &ins, &w);
        assert_eq!(checksum(&a), checksum(&b));
        assert_eq!(a, b);
    }

    #[test]
    fn long_run_is_reproducible() {
        // Same input sequence from the same start → identical checksum.
        let inputs: Vec<[Input; 1]> = (0..300)
            .map(|t| [move_input(1, 0, if t % 40 == 0 { BTN_JUMP } else { 0 })])
            .collect();

        let run = || {
            let w = FlatWorld::new();
            let mut s = SimState::new(1);
            for ins in &inputs {
                s = step(&s, ins, &w);
            }
            checksum(&s)
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn the_tick_advances_and_inputs_dispatch_per_player() {
        // Two players, different inputs: their states must diverge, and the
        // tick counter must advance by one per step.
        let w = FlatWorld::new();
        let mut s = SimState::new(2);
        s.players[0].on_ground = true;
        s.players[1].on_ground = true;
        s = step(&s, &[move_input(1, 0, 0), idle()], &w);
        assert_eq!(s.tick, Tick(1));
        assert!(s.players[0].vel.x > s.players[1].vel.x, "input drove player 0 only");
    }

    #[test]
    fn a_missing_input_is_treated_as_idle() {
        // Fewer inputs than players → the extra player gets the default.
        let w = FlatWorld::new();
        let s = SimState::new(2);
        let a = step(&s, &[move_input(1, 0, 0)], &w); // only player 0 supplied
        let b = step(&s, &[move_input(1, 0, 0), idle()], &w);
        assert_eq!(checksum(&a), checksum(&b));
    }
}
