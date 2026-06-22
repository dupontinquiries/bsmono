//! `sim::lauscher_lair` — what the Lauscher *leaves behind*. The Lauscher is a
//! grounded, deceptive echo-predator (see `animal_voice_lauscher`): it stores
//! player phrases, withdraws to its cave, and replays them to lure. This module
//! gives it a HOME it slowly builds and decorates, so the moment a player stumbles
//! into a Lauscher's lair is unsettling: climb structures it shouldn't be able to
//! make (vine-ropes, lashed ladders, a carved tunnel toward the surface) and a
//! collection of little tools/trinkets arranged with too much intent.
//!
//! Everything is deterministic from `(home, seed)` + the lair's age, so re-entering
//! a cave reproduces the identical lair (no popping/re-rolling). The world/bin reads
//! [`LauscherLair::block_edits`] to stamp it into the chunk, and [`unease`] to drive
//! the discovery ambience. Pure: no IO/render/clock/RNG-beyond-seed.

use crate::world::blocks::BlockId;

/// A vertical traversal structure the Lauscher builds to climb out of its cave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClimbKind {
    /// A hanging rope of vines down a shaft — climbed hand-over-hand.
    VineRope,
    /// Rungs lashed to a wall (vines pinned at intervals) — a crude ladder.
    LashedLadder,
    /// A narrow carved passage angling up toward the surface (cells hollowed out).
    Tunnel,
}

impl ClimbKind {
    /// The block this structure is made of (Tunnels are carved = Air).
    pub fn block(self) -> BlockId {
        match self {
            ClimbKind::VineRope | ClimbKind::LashedLadder => BlockId::Vine,
            ClimbKind::Tunnel => BlockId::Air,
        }
    }
}

/// A little tool/trinket the Lauscher leaves around its home as decoration —
/// the "too much intent" evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trinket {
    BoneCharm,    // gnawed bones arranged in a pattern
    AmberBead,    // collected amber, placed like an offering
    CrystalShard, // a glinting shard set into the wall
    Webwork,      // dense cobweb lattice — strung deliberately
    GlowLichen,   // lichen cultivated for a dim light
    Fungus,       // a tended mushroom cluster
}

impl Trinket {
    pub fn block(self) -> BlockId {
        match self {
            Trinket::BoneCharm => BlockId::Bone,
            Trinket::AmberBead => BlockId::Amber,
            Trinket::CrystalShard => BlockId::Crystal,
            Trinket::Webwork => BlockId::Cobweb,
            Trinket::GlowLichen => BlockId::Lichen,
            Trinket::Fungus => BlockId::Mushroom,
        }
    }
    fn from_roll(r: u64) -> Trinket {
        match r % 6 {
            0 => Trinket::BoneCharm,
            1 => Trinket::AmberBead,
            2 => Trinket::CrystalShard,
            3 => Trinket::Webwork,
            4 => Trinket::GlowLichen,
            _ => Trinket::Fungus,
        }
    }
}

/// One climb structure: a base cell, a height, and a horizontal step per level
/// (so ladders/tunnels can angle rather than go straight up).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClimbStructure {
    pub kind: ClimbKind,
    pub base: [i32; 3],
    pub height: i32,
    pub step: [i32; 2], // (dx, dz) added per level climbed
}

/// One placed decoration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoration {
    pub trinket: Trinket,
    pub pos: [i32; 3],
}

/// The Lauscher's home, grown over time.
#[derive(Clone, Debug)]
pub struct LauscherLair {
    home: [i32; 3],
    seed: u64,
    climbs: Vec<ClimbStructure>,
    decor: Vec<Decoration>,
    stored_echoes: u32,
    age_ticks: u64,
}

const BUILD_INTERVAL: u64 = 600; // a new lair element roughly this often
const MAX_CLIMBS: usize = 3;
const MAX_DECOR: usize = 14;
const CLIMB_TARGET_HEIGHT: i32 = 18; // how high it works toward the surface

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl LauscherLair {
    pub fn new(home: [i32; 3], seed: u64) -> LauscherLair {
        LauscherLair {
            home,
            seed,
            climbs: Vec::new(),
            decor: Vec::new(),
            stored_echoes: 0,
            age_ticks: 0,
        }
    }

    /// A lair as it would be after `age_ticks` of habitation — the deterministic
    /// way the world should instantiate a found lair (rather than ticking from 0).
    pub fn aged(home: [i32; 3], seed: u64, age_ticks: u64) -> LauscherLair {
        let mut l = LauscherLair::new(home, seed);
        l.rebuild_to_age(age_ticks);
        l
    }

    pub fn home(&self) -> [i32; 3] {
        self.home
    }
    pub fn climbs(&self) -> &[ClimbStructure] {
        &self.climbs
    }
    pub fn decorations(&self) -> &[Decoration] {
        &self.decor
    }
    pub fn stored_echoes(&self) -> u32 {
        self.stored_echoes
    }

    /// The Lauscher captured a player phrase — feeds the lure system AND deepens
    /// the lair's unease (a cave that whispers back in your own voice).
    pub fn record_echo(&mut self) {
        self.stored_echoes = self.stored_echoes.saturating_add(1);
    }

    /// Advance habitation; periodically adds a climb structure or a decoration.
    pub fn tick(&mut self, dt_ticks: u64) {
        let before = self.age_ticks / BUILD_INTERVAL;
        self.age_ticks = self.age_ticks.saturating_add(dt_ticks);
        let after = self.age_ticks / BUILD_INTERVAL;
        for milestone in (before + 1)..=after {
            self.add_element(milestone);
        }
    }

    fn rebuild_to_age(&mut self, age_ticks: u64) {
        self.climbs.clear();
        self.decor.clear();
        let milestones = age_ticks / BUILD_INTERVAL;
        for m in 1..=milestones {
            self.add_element(m);
        }
        self.age_ticks = age_ticks;
    }

    /// Deterministically add the `n`-th lair element. Early milestones favour a
    /// primary climb route (it wants OUT); later ones accrete decoration.
    fn add_element(&mut self, n: u64) {
        let h = splitmix(self.seed ^ n.wrapping_mul(0x1000_0001));
        // First milestone always starts the primary escape climb.
        if self.climbs.is_empty() {
            self.climbs.push(self.make_climb(h, true));
            return;
        }
        // Roughly 1-in-4 later elements extend climbing; the rest decorate.
        if self.climbs.len() < MAX_CLIMBS && (h % 4 == 0) {
            self.climbs.push(self.make_climb(h, false));
        } else if self.decor.len() < MAX_DECOR {
            self.decor.push(self.make_decor(h));
        } else if let Some(first) = self.climbs.first_mut() {
            // home is "finished": keep raising the main route toward the surface
            if first.height < CLIMB_TARGET_HEIGHT {
                first.height += 1;
            }
        }
    }

    fn make_climb(&self, h: u64, primary: bool) -> ClimbStructure {
        let kind = match h % 3 {
            0 => ClimbKind::VineRope,
            1 => ClimbKind::LashedLadder,
            _ => ClimbKind::Tunnel,
        };
        // primary climb starts at home; secondary ones offset a little.
        let off_x = if primary { 0 } else { ((h >> 3) % 5) as i32 - 2 };
        let off_z = if primary { 0 } else { ((h >> 6) % 5) as i32 - 2 };
        let height = if primary { 6 } else { 3 + ((h >> 9) % 5) as i32 };
        // tunnels angle; ropes/ladders go mostly straight up.
        let step = match kind {
            ClimbKind::Tunnel => [((h >> 12) % 3) as i32 - 1, ((h >> 15) % 3) as i32 - 1],
            _ => [0, 0],
        };
        ClimbStructure {
            kind,
            base: [self.home[0] + off_x, self.home[1], self.home[2] + off_z],
            height,
            step,
        }
    }

    fn make_decor(&self, h: u64) -> Decoration {
        // arranged in a tight ring around home — deliberate, not scattered.
        let ring = 1 + (self.decor.len() as i32 % 3);
        let ang = (h % 8) as i32;
        let (dx, dz) = match ang {
            0 => (ring, 0),
            1 => (ring, ring),
            2 => (0, ring),
            3 => (-ring, ring),
            4 => (-ring, 0),
            5 => (-ring, -ring),
            6 => (0, -ring),
            _ => (ring, -ring),
        };
        let dy = ((h >> 5) % 2) as i32; // floor or one up (set into the wall)
        Decoration {
            trinket: Trinket::from_roll(h >> 8),
            pos: [self.home[0] + dx, self.home[1] + dy, self.home[2] + dz],
        }
    }

    /// All block placements that realise this lair in the world, as
    /// `(world_pos, block)`. Climb structures expand into their per-level cells.
    pub fn block_edits(&self) -> Vec<([i32; 3], BlockId)> {
        let mut out = Vec::new();
        for c in &self.climbs {
            let b = c.kind.block();
            for level in 0..c.height {
                out.push((
                    [
                        c.base[0] + c.step[0] * level,
                        c.base[1] + level,
                        c.base[2] + c.step[1] * level,
                    ],
                    b,
                ));
            }
        }
        for d in &self.decor {
            out.push((d.pos, d.trinket.block()));
        }
        out
    }

    /// 0..1 "wrongness" of the lair for the discovery moment. Grows with how much
    /// the Lauscher has built, decorated, and — most of all — how many of the
    /// player's own words it has stored to whisper back.
    pub fn unease(&self) -> f32 {
        let structure = self.climbs.len() as f32 / MAX_CLIMBS as f32; // it built things
        let decor = self.decor.len() as f32 / MAX_DECOR as f32; // arranged with intent
        let echoes = (self.stored_echoes as f32 / 6.0).min(1.0); // it has your voice
        (0.25 * structure + 0.35 * decor + 0.40 * echoes).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_lair_is_empty_and_calm() {
        let l = LauscherLair::new([0, 40, 0], 7);
        assert!(l.climbs().is_empty());
        assert!(l.decorations().is_empty());
        assert_eq!(l.unease(), 0.0);
        assert!(l.block_edits().is_empty());
    }

    #[test]
    fn habitation_builds_a_climb_first_then_decorates() {
        let mut l = LauscherLair::new([10, 30, -4], 99);
        l.tick(BUILD_INTERVAL); // first milestone
        assert_eq!(l.climbs().len(), 1, "it starts an escape climb before anything else");
        l.tick(BUILD_INTERVAL * 12);
        assert!(!l.decorations().is_empty(), "then it decorates the home");
        assert!(l.unease() > 0.0);
        // the primary climb starts at home
        assert_eq!(l.climbs()[0].base, [10, 30, -4]);
    }

    #[test]
    fn aged_is_deterministic_and_matches_ticking() {
        let aged = LauscherLair::aged([3, 50, 9], 0xABCD, BUILD_INTERVAL * 10);
        let mut ticked = LauscherLair::new([3, 50, 9], 0xABCD);
        ticked.tick(BUILD_INTERVAL * 10);
        assert_eq!(aged.climbs(), ticked.climbs());
        assert_eq!(aged.decorations(), ticked.decorations());
        // same seed/home/age elsewhere → identical lair (no re-roll on re-entry)
        let again = LauscherLair::aged([3, 50, 9], 0xABCD, BUILD_INTERVAL * 10);
        assert_eq!(aged.block_edits(), again.block_edits());
    }

    #[test]
    fn climb_edits_expand_into_a_vertical_run() {
        let l = LauscherLair::aged([0, 20, 0], 1, BUILD_INTERVAL);
        let c = l.climbs()[0];
        let edits = l.block_edits();
        // the climb contributes `height` cells rising from its base
        let climb_cells = edits.iter().filter(|(p, _)| p[1] >= c.base[1] && p[1] < c.base[1] + c.height).count();
        assert!(climb_cells >= c.height as usize);
    }

    #[test]
    fn stored_echoes_drive_unease_hard() {
        let mut l = LauscherLair::aged([0, 20, 0], 5, BUILD_INTERVAL * 6);
        let calm = l.unease();
        for _ in 0..6 {
            l.record_echo();
        }
        assert!(l.unease() > calm, "a cave that knows your voice is worse");
        assert_eq!(l.stored_echoes(), 6);
    }
}

// ── service registration (S3) ──
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

struct LauscherLairService(LauscherLair);
impl Service for LauscherLairService {
    fn name(&self) -> &'static str {
        "lauscher_lair"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        self.0.tick(1); // advance one lair tick per frame
    }
}
#[distributed_slice(SERVICES)]
static LAUSCHER_LAIR_SERVICE: ServiceEntry = ServiceEntry {
    name: "lauscher_lair",
    make: || Box::new(LauscherLairService(LauscherLair::new([0, 0, 0], 0x5EED_1A1C))),
};
