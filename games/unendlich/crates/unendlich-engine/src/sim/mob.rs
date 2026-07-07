//! Mob entities and their deterministic "brain" — Phase 2, brick M1+M2 logic.
//!
//! # What this is (and isn't)
//!
//! A mob is a [`crate::sim::state::Character`] body plus a kind, health, and a
//! tiny deterministic AI. The AI's only job is to produce, each tick, the same
//! [`Input`] a player would — so a mob is driven through the *identical*
//! [`crate::sim::physics::step_character`] kernel as a player. No separate
//! movement code, no special cases.
//!
//! This module is **self-contained on purpose**: it does not modify
//! [`crate::sim::state::SimState`], [`crate::sim::state::step`], or `checksum`.
//! That keeps the validated, compiling sim core untouched while the AI is built
//! and proven. Integration is one small, well-scoped follow-up brick (see
//! "Integration" below) to be done once the Phase 0 driver has a hardware pass.
//!
//! # Determinism
//!
//! Server-authoritative: mob AI runs only on the host. But it is written fully
//! deterministic anyway (integer-only, [`splitmix64`] PRNG seeded from
//! `world_seed`/`mob_id`/`tick`) so replays and headless tests reproduce
//! exactly, and so it could later be cross-checked. No floats, no clock, no
//! global RNG.
//!
//! Movement is **world-space**: `step_character` uses `move_x`/`move_z`
//! directly as world-axis velocity targets (yaw is facing only), so the brain
//! emits a world-space heading vector directly — no rotation needed.
//!
//! # Integration (done — bricks M1b + M2)
//!
//! Mobs are now first-class in the rolled-back sim:
//! 1. `SimState` carries `mobs: Vec<Mob>` + a `world_seed` (empty/0 default →
//!    backward compatible; states without mobs checksum identically).
//! 2. `step` advances each mob after players:
//!    `let input = mob.brain(world_seed, id, tick, players, prey, predators, world); mob.body =
//!    step_character(&mob.body, &input, world);`
//! 3. `checksum` folds each mob's [`Mob::checksum_words`] (empty-invariant).
//! 4. `SimDriver::mob_pose(i)` mirrors `render_pose` for interpolation.

use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::physics::WorldQuery;
use crate::sim::state::{Character, Input};
use crate::sim::vision::{can_see, FOV_COS2_90DEG};

/// Quarter-turn-eighth: `u16` yaw step between the 8 compass headings
/// (`65536 / 8`).
const YAW_STEP: u16 = 8192;

/// The 8 compass headings as world-space unit vectors `(move_x, move_z)` in
/// raw [`Fixed`] units (`ONE = 65536`). Diagonals use `0.70710678 * 65536 ≈
/// 46341`, so every heading is unit-magnitude and mobs move at one consistent
/// speed regardless of direction.
const DIR: [(i64, i64); 8] = [
    (65536, 0),        // 0  E   (+x)
    (46341, 46341),    // 1  NE
    (0, 65536),        // 2  N   (+z)
    (-46341, 46341),   // 3  NW
    (-65536, 0),       // 4  W
    (-46341, -46341),  // 5  SW
    (0, -65536),       // 6  S
    (46341, -46341),   // 7  SE
];

/// How close (horizontally, in blocks) a player must be for a [`MobKind::Seeker`]
/// to lock on.
const SEEKER_RANGE: Fixed = Fixed::from_int(12);

/// How close (horizontally, in blocks) a player must be for a [`MobKind::Grazer`]
/// to bolt. Shorter than [`SEEKER_RANGE`] and omnidirectional (no FOV cone), so
/// grazers are skittish at close quarters but don't stampede from across the
/// map.
const FLEE_RANGE: Fixed = Fixed::from_int(8);

/// How close (horizontally) a [`MobKind::Seeker`] senses prey ([`MobKind::Grazer`]).
/// Omnidirectional (scent, not the forward vision cone used for players), so a
/// predator tracks prey it can't see. Matched to `SEEKER_RANGE` for symmetry.
const HUNT_RANGE: Fixed = Fixed::from_int(12);

/// Contact range at which a Seeker bites a Grazer it has caught.
pub(crate) const BITE_RANGE: Fixed = Fixed::from_int(2);

/// HP a Seeker removes from a caught Grazer per tick of contact. A default
/// 10-HP grazer dies after ~5 ticks in a predator's jaws.
pub(crate) const BITE_DAMAGE: i16 = 2;

/// How far a Grazer looks for herdmates when deciding which way to drift while
/// calmly wandering. Larger than FLEE_RANGE so a settled herd stays cohesive.
const HERD_RANGE: Fixed = Fixed::from_int(10);

/// Height of a mob's eye above its feet, for line-of-sight origin/target.
const EYE_HEIGHT: Fixed = Fixed::from_frac(15, 10); // 1.5 blocks

/// The nearest player this seeker can actually *see* from `eye` while facing
/// `(facing_x, facing_z)`: within [`SEEKER_RANGE`], inside the forward FOV cone,
/// and not occluded by terrain. Returns the player's body position to home in on.
fn nearest_visible_player<W: WorldQuery>(
    eye: Vec3,
    facing_x: Fixed,
    facing_z: Fixed,
    players: &[Character],
    world: &W,
) -> Option<Vec3> {
    let mut best: Option<(Fixed, Vec3)> = None;
    for p in players {
        let target_eye = Vec3 { x: p.pos.x, y: p.pos.y + EYE_HEIGHT, z: p.pos.z };
        if !can_see(eye, facing_x, facing_z, target_eye, SEEKER_RANGE, FOV_COS2_90DEG, world) {
            continue;
        }
        // Visible ⇒ within range, so the squared horizontal distance is bounded.
        let dx = p.pos.x - eye.x;
        let dz = p.pos.z - eye.z;
        let d2 = dx * dx + dz * dz;
        if best.map_or(true, |(b, _)| d2 < b) {
            best = Some((d2, p.pos));
        }
    }
    best.map(|(_, p)| p)
}

/// The compass heading (index into [`DIR`]) pointing most directly from `from`
/// toward `to`. Chosen by maximizing the dot product against each unit heading —
/// no trig, no `atan2`, all integer fixed-point.
fn seek_dir(from: Vec3, to: Vec3) -> u8 {
    let dx = to.x - from.x;
    let dz = to.z - from.z;
    let mut best = 0u8;
    let mut best_dot = Fixed(i64::MIN);
    for (i, &(mx, mz)) in DIR.iter().enumerate() {
        let dot = Fixed(mx) * dx + Fixed(mz) * dz;
        if dot > best_dot {
            best_dot = dot;
            best = i as u8;
        }
    }
    best
}

/// Nearest player within `radius` (horizontal) of `pos`, omnidirectional — prey
/// senses a threat from any direction, unlike a Seeker's forward cone. Returns
/// the threat position to flee from, or `None` if no player is close enough.
fn nearest_player_in_radius(pos: Vec3, players: &[Character], radius: Fixed) -> Option<Vec3> {
    let r2 = radius * radius;
    let mut best: Option<(Fixed, Vec3)> = None;
    for p in players {
        let dx = p.pos.x - pos.x;
        let dz = p.pos.z - pos.z;
        let d2 = dx * dx + dz * dz;
        if d2 <= r2 && best.map_or(true, |(b, _)| d2 < b) {
            best = Some((d2, p.pos));
        }
    }
    best.map(|(_, p)| p)
}

/// Heading pointing directly AWAY from `threat` — the heading from the threat
/// toward the mob. Reuses [`seek_dir`] with arguments swapped.
fn flee_dir(from: Vec3, threat: Vec3) -> u8 {
    seek_dir(threat, from)
}

/// Nearest of a set of world positions within `radius` (horizontal) of `pos`,
/// omnidirectional. Used for predator scent (Seeker → prey) and prey threat
/// sensing (Grazer → predator), where detection isn't limited to a view cone.
fn nearest_pos_in_radius(pos: Vec3, candidates: &[Vec3], radius: Fixed) -> Option<Vec3> {
    let r2 = radius * radius;
    let mut best: Option<(Fixed, Vec3)> = None;
    for &c in candidates {
        let dx = c.x - pos.x;
        let dz = c.z - pos.z;
        let d2 = dx * dx + dz * dz;
        // Skip a candidate at the exact same position (a mob sensing itself).
        if d2 <= r2 && d2 > Fixed(0) && best.map_or(true, |(b, _)| d2 < b) {
            best = Some((d2, c));
        }
    }
    best.map(|(_, c)| c)
}

/// Whichever of two candidate targets is horizontally closer to `from`.
/// `None`s drop out; ties favor `a`.
fn closer_of(from: Vec3, a: Option<Vec3>, b: Option<Vec3>) -> Option<Vec3> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(pa), Some(pb)) => {
            let d = |t: Vec3| {
                let dx = t.x - from.x;
                let dz = t.z - from.z;
                dx * dx + dz * dz
            };
            if d(pa) <= d(pb) { Some(pa) } else { Some(pb) }
        }
    }
}

/// True if any position in `seekers` is within [`BITE_RANGE`] (horizontal) of
/// `pos` — i.e. a predator has closed to contact. Unlike prey/threat sensing
/// this includes an exact overlap (distance 0): a predator standing on its prey
/// is biting it.
pub(crate) fn within_bite_range(pos: Vec3, seekers: &[Vec3]) -> bool {
    let r2 = BITE_RANGE * BITE_RANGE;
    seekers.iter().any(|s| {
        let dx = s.x - pos.x;
        let dz = s.z - pos.z;
        dx * dx + dz * dz <= r2
    })
}

/// Centroid of herdmates within `range` of `pos`, excluding `pos` itself (a
/// herdmate at exact distance 0 is the grazer's own entry in the snapshot).
/// `None` if the grazer is alone. Drives gentle herd cohesion while wandering.
fn herd_centroid(pos: Vec3, herd: &[Vec3], range: Fixed) -> Option<Vec3> {
    let r2 = range * range;
    let mut sx = Fixed(0);
    let mut sz = Fixed(0);
    let mut n: i64 = 0;
    for &h in herd {
        let dx = h.x - pos.x;
        let dz = h.z - pos.z;
        let d2 = dx * dx + dz * dz;
        if d2 > Fixed(0) && d2 <= r2 {
            sx = sx + h.x;
            sz = sz + h.z;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    Some(Vec3 { x: Fixed(sx.0 / n), y: pos.y, z: Fixed(sz.0 / n) })
}

/// What kind of creature this is. Drives spawn rules and, later, model +
/// behavior tuning. `Flyer` is the seed for dragons (brick M6) — it will use a
/// 3D-movement variant of the same brain.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MobKind {
    #[default]
    Walker,
    /// Chases the nearest player within range, else wanders (brick M2).
    Seeker,
    Flyer,
    /// Calm wanderer that bolts directly away from any player within
    /// [`FLEE_RANGE`]. Adds prey dynamics: where a Seeker closes in, a Grazer
    /// scatters. Senses omnidirectionally (no forward cone) — prey notices a
    /// threat from any direction.
    Grazer,
}

/// The mob's current intent.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AiMode {
    #[default]
    Idle,
    Wander,
    /// Moving straight at a locked-on player (brick M2).
    Seek,
    /// Moving straight away from a nearby threat (Grazer prey behavior).
    Flee,
}

/// Compact, `Copy`, checksum-friendly AI state. `ticks_left` counts down; at
/// zero the brain repicks a fresh intent.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct AiState {
    pub mode: AiMode,
    /// Heading index `0..8` when `mode == Wander`.
    pub dir: u8,
    pub ticks_left: u16,
}

/// A simulated mob: a physics body plus kind, health, and AI.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mob {
    pub body: Character,
    pub kind: MobKind,
    /// Integer HP (deterministic, simple). Switch to `Fixed` later if you want
    /// fractional damage.
    pub health: i16,
    pub ai: AiState,
    /// A compass heading (0..8) commanded by the cognition layer for this tick,
    /// or `None` to fall back to the built-in wander/seek reflex. This is the
    /// channel the server-side `Mind` writes its `Intent` heading into; being
    /// part of the mob state, it is snapshotted and replayed under rollback.
    pub commanded: Option<u8>,
}

impl Mob {
    /// A ground-wandering walker at `pos` with `health` HP, ready to think on
    /// its first tick (`ticks_left == 0` triggers an immediate intent pick).
    pub fn new_walker(pos: Vec3, health: i16) -> Mob {
        Mob {
            body: Character { pos, ..Character::default() },
            kind: MobKind::Walker,
            health,
            ai: AiState::default(),
            commanded: None,
        }
    }

    /// A player-chasing seeker at `pos` with `health` HP.
    pub fn new_seeker(pos: Vec3, health: i16) -> Mob {
        Mob {
            body: Character { pos, ..Character::default() },
            kind: MobKind::Seeker,
            health,
            ai: AiState::default(),
            commanded: None,
        }
    }

    /// A skittish grazer at `pos` with `health` HP: wanders calmly, flees any
    /// player within [`FLEE_RANGE`].
    pub fn new_grazer(pos: Vec3, health: i16) -> Mob {
        Mob {
            body: Character { pos, ..Character::default() },
            kind: MobKind::Grazer,
            health,
            ai: AiState::default(),
            commanded: None,
        }
    }

    /// Decide this tick's [`Input`], advancing the AI state. Deterministic given
    /// `(world_seed, mob_id, tick)` and the `players` snapshot. The returned
    /// input is fed to `step_character` exactly like a player's.
    ///
    /// Seekers lock onto the nearest player they can actually *see* (range +
    /// forward FOV cone + unobstructed line of sight) and re-aim every tick;
    /// with no visible target (or any other kind) they fall back to the wander
    /// state machine.
    pub fn brain<W: WorldQuery>(
        &mut self,
        world_seed: u64,
        mob_id: u64,
        tick: u64,
        players: &[Character],
        prey: &[Vec3],
        predators: &[Vec3],
        world: &W,
    ) -> Input {
        // A heading from the cognition layer overrides reflex behavior: the
        // creature's Mind has already perceived, decided, and pathed, so it just
        // walks where it's told. `None` falls back to the built-in reflex below.
        if let Some(dir) = self.commanded {
            self.ai.mode = AiMode::Seek;
            self.ai.dir = dir & 7;
            return self.mode_input();
        }
        // Grazer prey reflex: bolt away from the nearest player within
        // FLEE_RANGE (omnidirectional), else wander like a walker. Checked
        // before the seeker logic since a Grazer never seeks.
        if self.kind == MobKind::Grazer {
            // Prey reflex: flee whichever is closer — a player or a predator
            // (Seeker) — when either is within FLEE_RANGE. Else wander.
            let player_threat = nearest_player_in_radius(self.body.pos, players, FLEE_RANGE);
            let pred_threat = nearest_pos_in_radius(self.body.pos, predators, FLEE_RANGE);
            if let Some(threat) = closer_of(self.body.pos, player_threat, pred_threat) {
                self.ai.mode = AiMode::Flee;
                self.ai.dir = flee_dir(self.body.pos, threat);
                // Fleeing is continuous; tick down without triggering a
                // wander repick while a threat is near.
                self.ai.ticks_left = self.ai.ticks_left.saturating_sub(1);
            } else {
                self.wander_tick(world_seed, mob_id, tick);
                // Herd cohesion: while loosely wandering (not idle/paused),
                // drift toward the local herd centroid so grazers gather into
                // herds. A predator (flee overrides this branch) scatters them.
                if self.ai.mode == AiMode::Wander {
                    if let Some(c) = herd_centroid(self.body.pos, prey, HERD_RANGE) {
                        self.ai.dir = seek_dir(self.body.pos, c);
                    }
                }
            }
            return self.mode_input();
        }
        let target = if self.kind == MobKind::Seeker {
            let eye = Vec3 { x: self.body.pos.x, y: self.body.pos.y + EYE_HEIGHT, z: self.body.pos.z };
            // Facing comes from the current heading (8-way), so a seeker only
            // notices players within its forward cone — it must be looking.
            let (fxr, fzr) = DIR[((self.body.yaw / YAW_STEP) & 7) as usize];
            let player_target = nearest_visible_player(eye, Fixed(fxr), Fixed(fzr), players, world);
            // Prey is sensed omnidirectionally (scent) within HUNT_RANGE. The
            // seeker pursues whichever target — player or prey — is closer.
            let prey_target = nearest_pos_in_radius(self.body.pos, prey, HUNT_RANGE);
            closer_of(self.body.pos, player_target, prey_target)
        } else {
            None
        };
        match target {
            Some(t) => {
                self.ai.mode = AiMode::Seek;
                self.ai.dir = seek_dir(self.body.pos, t);
                // Seeking is continuous; tick the timer down without letting it
                // trigger a wander repick while locked on.
                self.ai.ticks_left = self.ai.ticks_left.saturating_sub(1);
            }
            None => self.wander_tick(world_seed, mob_id, tick),
        }
        self.mode_input()
    }

    /// The wander state machine: on `ticks_left == 0`, repick idle-or-heading and
    /// a fresh hold time, then count down. This is the original walker AI,
    /// unchanged.
    fn wander_tick(&mut self, world_seed: u64, mob_id: u64, tick: u64) {
        if self.ai.ticks_left == 0 {
            let mut s = mix(world_seed, mob_id, tick);
            let r = splitmix64(&mut s);
            // 1-in-4 chance to pause; otherwise wander a random heading.
            if r % 4 == 0 {
                self.ai.mode = AiMode::Idle;
                self.ai.dir = 0;
            } else {
                self.ai.mode = AiMode::Wander;
                self.ai.dir = ((r >> 3) % 8) as u8;
            }
            // Hold the intent for 0.5–2.0 s (30..120 ticks @ 60 Hz).
            let r2 = splitmix64(&mut s);
            self.ai.ticks_left = 30 + (r2 % 90) as u16;
        }
        self.ai.ticks_left -= 1;
    }

    /// Translate the current AI mode into a movement [`Input`]. Wander and Seek
    /// both move along the chosen `DIR` heading; Idle holds still.
    fn mode_input(&self) -> Input {
        match self.ai.mode {
            AiMode::Idle => Input {
                buttons: 0,
                move_x: Fixed::ZERO,
                move_z: Fixed::ZERO,
                yaw: self.body.yaw, // hold facing while paused
            },
            AiMode::Wander | AiMode::Seek | AiMode::Flee => {
                let (mx, mz) = DIR[(self.ai.dir & 7) as usize];
                Input {
                    buttons: 0,
                    move_x: Fixed(mx),
                    move_z: Fixed(mz),
                    yaw: (self.ai.dir as u16) * YAW_STEP,
                }
            }
        }
    }

    /// The mob's forward unit vector (8-way) from its current yaw octant — the
    /// facing used for vision cones, shared with the cognition perception pass.
    pub fn facing(&self) -> (Fixed, Fixed) {
        let (fx, fz) = DIR[((self.body.yaw / YAW_STEP) & 7) as usize];
        (Fixed(fx), Fixed(fz))
    }

    /// The integer words to fold into the sim checksum when mobs join
    /// `SimState`. Keeps the checksum extension a one-liner per mob.
    pub fn checksum_words(&self) -> [u64; 10] {
        [
            self.body.pos.x.0 as u64,
            self.body.pos.y.0 as u64,
            self.body.pos.z.0 as u64,
            self.body.vel.x.0 as u64,
            self.body.vel.y.0 as u64,
            self.body.vel.z.0 as u64,
            self.body.yaw as u64,
            self.health as u64,
            // AI: mode in the low 2 bits, dir in the next 3, ticks_left above.
            (self.ai.mode as u64)
                | ((self.ai.dir as u64) << 2)
                | ((self.ai.ticks_left as u64) << 5),
            // Commanded heading from cognition: None = 0, Some(d) = 1 + d.
            match self.commanded {
                None => 0,
                Some(d) => 1 + (d as u64),
            },
        ]
    }
}

/// SplitMix64 — a tiny, fast, well-distributed deterministic PRNG. Advances
/// `state` and returns the next value.
#[inline]
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Seed a decision from the world seed, the mob's id, and the tick — so every
/// mob has an independent, reproducible stream.
#[inline]
fn mix(world_seed: u64, mob_id: u64, tick: u64) -> u64 {
    world_seed
        ^ mob_id.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ tick.wrapping_mul(0xD1B5_4A32_D192_ED03)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walker() -> Mob {
        Mob::new_walker(Vec3::default(), 10)
    }

    fn stream(world_seed: u64, mob_id: u64, n: u64) -> Vec<Input> {
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = walker();
        (0..n).map(|t| m.brain(world_seed, mob_id, t, &[], &[], &[], &w)).collect()
    }

    #[test]
    fn brain_is_deterministic() {
        assert_eq!(stream(0xABCDEF, 7, 300), stream(0xABCDEF, 7, 300));
    }

    #[test]
    fn grazer_flees_directly_away_from_close_player() {
        // Player to the east (+x) within FLEE_RANGE -> grazer bolts west (-x).
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = Mob::new_grazer(Vec3::default(), 10);
        let player = Character {
            pos: Vec3 { x: Fixed::from_int(5), y: Fixed::ZERO, z: Fixed::ZERO },
            ..Character::default()
        };
        let input = m.brain(0, 0, 0, &[player], &[], &[], &w);
        assert_eq!(m.ai.mode, AiMode::Flee, "grazer near a player should flee");
        assert!(input.move_x < Fixed::ZERO, "should move away (west) from eastern threat: {:?}", input.move_x);
    }

    #[test]
    fn grazer_wanders_when_no_threat_in_range() {
        // Player well beyond FLEE_RANGE -> grazer ignores it and wanders/idles.
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = Mob::new_grazer(Vec3::default(), 10);
        let far = Character {
            pos: Vec3 { x: Fixed::from_int(50), y: Fixed::ZERO, z: Fixed::ZERO },
            ..Character::default()
        };
        m.brain(0, 0, 0, &[far], &[], &[], &w);
        assert_ne!(m.ai.mode, AiMode::Flee, "distant player must not trigger flee");
    }

    #[test]
    fn seeker_hunts_nearby_grazer() {
        // No players, but a grazer sits to the east within HUNT_RANGE. The
        // seeker senses it omnidirectionally and pursues (+x), even though no
        // player is around.
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = Mob::new_seeker(Vec3::default(), 10);
        let grazer = Vec3 { x: Fixed::from_int(4), y: Fixed::ZERO, z: Fixed::ZERO };
        let input = m.brain(0, 0, 0, &[], &[grazer], &[], &w);
        assert_eq!(m.ai.mode, AiMode::Seek, "seeker should hunt a grazer in range");
        assert!(input.move_x > Fixed::ZERO, "should move toward the +x prey: {:?}", input.move_x);
    }

    #[test]
    fn seeker_ignores_grazer_out_of_hunt_range() {
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = Mob::new_seeker(Vec3::default(), 10);
        let far_grazer = Vec3 { x: Fixed::from_int(40), y: Fixed::ZERO, z: Fixed::ZERO };
        m.brain(0, 0, 0, &[], &[far_grazer], &[], &w);
        assert_ne!(m.ai.mode, AiMode::Seek, "prey beyond HUNT_RANGE must not trigger a hunt");
    }

    #[test]
    fn grazer_flees_nearby_seeker_predator() {
        // No players, but a predator (seeker) is east within FLEE_RANGE. The
        // grazer bolts west, same as it would from a player.
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = Mob::new_grazer(Vec3::default(), 10);
        let predator = Vec3 { x: Fixed::from_int(5), y: Fixed::ZERO, z: Fixed::ZERO };
        let input = m.brain(0, 0, 0, &[], &[], &[predator], &w);
        assert_eq!(m.ai.mode, AiMode::Flee, "grazer should flee a nearby predator");
        assert!(input.move_x < Fixed::ZERO, "should bolt away (west) from the +x predator: {:?}", input.move_x);
    }

    #[test]
    fn closer_of_picks_nearest_and_handles_none() {
        let origin = Vec3::default();
        let near = Vec3 { x: Fixed::from_int(2), y: Fixed::ZERO, z: Fixed::ZERO };
        let far = Vec3 { x: Fixed::from_int(9), y: Fixed::ZERO, z: Fixed::ZERO };
        assert_eq!(closer_of(origin, Some(near), Some(far)), Some(near));
        assert_eq!(closer_of(origin, Some(far), Some(near)), Some(near));
        assert_eq!(closer_of(origin, None, Some(far)), Some(far));
        assert_eq!(closer_of(origin, Some(near), None), Some(near));
        assert_eq!(closer_of(origin, None, None), None);
    }

    #[test]
    fn seeker_self_position_in_prey_list_is_ignored() {
        // A seeker that is also listed in prey (shouldn't happen, but be safe)
        // must not lock onto its own position (distance 0 is skipped).
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = Mob::new_seeker(Vec3::default(), 10);
        let self_pos = m.body.pos;
        m.brain(0, 0, 0, &[], &[self_pos], &[], &w);
        assert_ne!(m.ai.mode, AiMode::Seek, "a mob must not hunt its own position");
    }

    #[test]
    fn herd_centroid_excludes_self_and_averages() {
        let pos = Vec3::default();
        // Two herdmates at +x=2 and +x=4 (plus our own entry at 0, excluded).
        let herd = [
            pos,
            Vec3 { x: Fixed::from_int(2), y: Fixed::ZERO, z: Fixed::ZERO },
            Vec3 { x: Fixed::from_int(4), y: Fixed::ZERO, z: Fixed::ZERO },
        ];
        let c = herd_centroid(pos, &herd, Fixed::from_int(10)).expect("herd present");
        assert_eq!(c.x, Fixed::from_int(3), "centroid x should average to 3");
        assert_eq!(c.z, Fixed::ZERO);
        // Lone grazer (only its own entry) → no centroid.
        assert!(herd_centroid(pos, &[pos], Fixed::from_int(10)).is_none());
        // Herdmates out of range → none.
        let far = [pos, Vec3 { x: Fixed::from_int(40), y: Fixed::ZERO, z: Fixed::ZERO }];
        assert!(herd_centroid(pos, &far, Fixed::from_int(10)).is_none());
    }

    #[test]
    fn grazing_grazer_drifts_toward_herd() {
        // No threat, but herdmates sit to the east. Whenever the grazer is in
        // its loose Wander state, cohesion should steer it toward the herd
        // centroid (east), not a random heading.
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = Mob::new_grazer(Vec3::default(), 10);
        let herd = [
            m.body.pos, // own entry (excluded)
            Vec3 { x: Fixed::from_int(6), y: Fixed::ZERO, z: Fixed::ZERO },
            Vec3 { x: Fixed::from_int(8), y: Fixed::ZERO, z: Fixed::ZERO },
        ];
        let expect_dir = seek_dir(m.body.pos, Vec3 { x: Fixed::from_int(7), y: Fixed::ZERO, z: Fixed::ZERO });
        let mut wandered = false;
        for t in 0..300 {
            m.brain(0xBEEF_u64, 4, t, &[], &herd, &[], &w);
            if m.ai.mode == AiMode::Wander {
                wandered = true;
                assert_eq!(m.ai.dir, expect_dir,
                    "a wandering grazer should drift toward the herd (east)");
            }
        }
        assert!(wandered, "grazer should have wandered at least once over 300 ticks");
    }

    #[test]
    fn grazer_brain_is_deterministic() {
        let run = || {
            let w = crate::sim::physics::FlatWorld::new();
            let mut m = Mob::new_grazer(Vec3::default(), 10);
            let p = Character {
                pos: Vec3 { x: Fixed::from_int(3), y: Fixed::ZERO, z: Fixed::ZERO },
                ..Character::default()
            };
            (0..120).map(|t| m.brain(0x5EED, 9, t, &[p], &[], &[], &w)).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn different_mobs_diverge() {
        assert_ne!(stream(0xABCDEF, 7, 300), stream(0xABCDEF, 8, 300));
    }

    #[test]
    fn commanded_heading_overrides_reflex() {
        // A seeker with a player right in front would normally chase it (+x);
        // with a command set it instead walks the commanded heading (North/+z).
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = Mob::new_seeker(Vec3::default(), 10);
        m.commanded = Some(2); // North
        let player = Character {
            pos: Vec3 { x: Fixed::from_int(1), y: Fixed::ZERO, z: Fixed::ZERO },
            ..Character::default()
        };
        let input = m.brain(0, 0, 0, &[player], &[], &[], &w);
        assert_eq!(input.move_x, Fixed::ZERO);
        assert_eq!(input.move_z, Fixed(65536)); // due North, not toward the +x player
        assert_eq!(m.ai.dir, 2);
    }

    #[test]
    fn no_command_falls_back_to_reflex() {
        // Without a command, a seeker still reflex-chases a visible player (+x).
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = Mob::new_seeker(Vec3::default(), 10);
        assert_eq!(m.commanded, None);
        let player = Character {
            pos: Vec3 { x: Fixed::from_int(3), y: Fixed::ZERO, z: Fixed::ZERO },
            ..Character::default()
        };
        let input = m.brain(0, 0, 0, &[player], &[], &[], &w);
        assert!(input.move_x > Fixed::ZERO, "should reflex-seek toward the +x player");
    }

    #[test]
    fn commanded_encodes_into_checksum_words() {
        let mut m = walker();
        let base = m.checksum_words();
        assert_eq!(base[9], 0, "no command encodes as 0");
        m.commanded = Some(3);
        assert_eq!(m.checksum_words()[9], 4, "Some(3) encodes as 1 + 3");
        assert_ne!(m.checksum_words(), base);
    }

    #[test]
    fn explores_all_headings_and_idles_sometimes() {
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = walker();
        let mut dirs = [false; 8];
        let (mut idle, mut picks) = (0u32, 0u32);
        for t in 0..40_000u64 {
            let was_due = m.ai.ticks_left == 0;
            m.brain(0x1234, 3, t, &[], &[], &[], &w);
            if was_due {
                picks += 1;
                match m.ai.mode {
                    AiMode::Idle => idle += 1,
                    AiMode::Wander => dirs[(m.ai.dir & 7) as usize] = true,
                    AiMode::Seek => {} // unreachable for a walker (no players)
                    AiMode::Flee => {} // unreachable for a walker (only Grazers flee)
                }
            }
        }
        assert!(dirs.iter().all(|&d| d), "every heading should occur");
        let frac = idle as f64 / picks as f64;
        assert!((0.18..0.32).contains(&frac), "idle fraction {frac} off ~0.25");
    }

    #[test]
    fn headings_are_unit_magnitude() {
        const ONE_SQ: i64 = 65536 * 65536;
        for (mx, mz) in DIR {
            let mag_sq = mx * mx + mz * mz;
            let err = (mag_sq - ONE_SQ).abs();
            // diagonals round to 46341 → within ~0.005% of unit.
            assert!(err * 10_000 / ONE_SQ < 2, "heading ({mx},{mz}) not unit");
        }
    }

    #[test]
    fn wander_yaw_matches_heading() {
        // dir d should face d * 45°.
        for d in 0u16..8 {
            let yaw = d * YAW_STEP;
            let deg = yaw as f32 / 65536.0 * 360.0;
            assert!((deg - d as f32 * 45.0).abs() < 0.01);
        }
    }

    #[test]
    fn seek_dir_points_at_target() {
        let o = Vec3::default();
        let at = |x: i64, z: i64| Vec3 { x: Fixed::from_int(x), y: Fixed::ZERO, z: Fixed::from_int(z) };
        assert_eq!(seek_dir(o, at(5, 0)), 0); // +x  → E
        assert_eq!(seek_dir(o, at(5, 5)), 1); // +x+z → NE
        assert_eq!(seek_dir(o, at(0, 5)), 2); // +z  → N
        assert_eq!(seek_dir(o, at(-5, 0)), 4); // -x → W
        assert_eq!(seek_dir(o, at(0, -5)), 6); // -z → S
    }

    #[test]
    fn seeker_heads_toward_player() {
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = Mob::new_seeker(Vec3 { x: Fixed::ZERO, y: Fixed::from_int(4), z: Fixed::ZERO }, 20);
        let player = Character {
            pos: Vec3 { x: Fixed::from_int(10), y: Fixed::ZERO, z: Fixed::ZERO },
            ..Character::default()
        };
        let start_x = m.body.pos.x;
        for t in 0..120u64 {
            let input = m.brain(0, 0, t, std::slice::from_ref(&player), &[], &[], &w);
            m.body = crate::sim::physics::step_character(&m.body, &input, &w);
        }
        assert!(m.body.pos.x > start_x, "seeker should advance toward the +x player");
        assert_eq!(m.ai.mode, AiMode::Seek);
    }

    #[test]
    fn seeker_ignores_far_player() {
        let w = crate::sim::physics::FlatWorld::new();
        let mut m = Mob::new_seeker(Vec3::default(), 20);
        let far = Character {
            pos: Vec3 { x: Fixed::from_int(1000), y: Fixed::ZERO, z: Fixed::ZERO },
            ..Character::default()
        };
        let mut saw_seek = false;
        for t in 0..200u64 {
            m.brain(0x55, 1, t, std::slice::from_ref(&far), &[], &[], &w);
            if m.ai.mode == AiMode::Seek {
                saw_seek = true;
            }
        }
        assert!(!saw_seek, "out-of-range seeker must not lock on");
    }

    #[test]
    fn seeker_needs_line_of_sight_and_facing() {
        use crate::sim::physics::{BlockPhysics, SceneWorld};
        let eye = Vec3 { x: Fixed::from_f64(0.5), y: Fixed::from_f64(1.5), z: Fixed::from_f64(0.5) };
        let player = Character {
            pos: Vec3 { x: Fixed::from_f64(5.5), y: Fixed::ZERO, z: Fixed::from_f64(0.5) },
            ..Character::default()
        };
        let (fx, fz) = DIR[0]; // facing +x
        // Clear and ahead → seen.
        let clear = SceneWorld::new();
        assert!(nearest_visible_player(eye, Fixed(fx), Fixed(fz), std::slice::from_ref(&player), &clear).is_some());
        // A wall on the sightline → blind, even though in range and in cone.
        let mut walled = SceneWorld::new();
        walled.set(2, 1, 0, BlockPhysics::solid(Fixed::ZERO));
        assert!(nearest_visible_player(eye, Fixed(fx), Fixed(fz), std::slice::from_ref(&player), &walled).is_none());
        // A player directly behind (facing +x, player at -x) → outside the cone.
        let behind = Character {
            pos: Vec3 { x: Fixed::from_f64(-5.5), y: Fixed::ZERO, z: Fixed::from_f64(0.5) },
            ..Character::default()
        };
        assert!(nearest_visible_player(eye, Fixed(fx), Fixed(fz), std::slice::from_ref(&behind), &clear).is_none());
    }
}
