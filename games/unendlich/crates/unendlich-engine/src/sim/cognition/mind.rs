//! The assembler — a creature's [`Mind`]: the per-tick loop that ties the whole
//! cognitive stack together and emits an [`Intent`] (a goal, its target, and a
//! movement heading) for the deterministic core to act on.
//!
//! Each cognition tick:
//! 1. **appraise** the situation against *prior* memory → [`Signals`]
//!    (so "novelty" means "did I already know this");
//! 2. **observe** this tick's sightings into memory, then **decay** it;
//! 3. **choose** a goal, weighted by [`Disposition`], with hysteresis from the
//!    current goal;
//! 4. pick that goal's **target** (Defend → the threat, Raid → recalled food,
//!    Evade/Flee → retreat / home, Investigate → the novel thing, Patrol →
//!    home);
//! 5. **navigate** to a heading around terrain.
//!
//! The `Mind` is server-side, persistent (it carries the memory and current
//! goal), and not rolled back; the only thing crossing back into the sim is the
//! `Intent` heading, consumed as the creature's input.

use crate::sim::cognition::appraisal::{appraise, SelfState, Sighting};
use crate::sim::cognition::memory::{CellPos, LandmarkTag, Memory};
use crate::sim::cognition::navigate::{heading_toward, Locomotion, NavRegion};
use crate::sim::goals::{choose, Goal};
use crate::sim::personality::Disposition;
use crate::sim::physics::WorldQuery;

/// How far to aim a retreat when evading/fleeing a threat (cells).
const RETREAT_CELLS: i32 = 16;
/// A sighting within this squared distance of a same-tag memory is "recognized"
/// (not novel) — mirrors the memory merge radius.
const RECOGNITION_DIST_SQ: i64 = 64;

/// The decision a `Mind` emits for one tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Intent {
    pub goal: Goal,
    /// World cell the goal wants to move toward/away-to (`None` for pure wander).
    pub target: Option<CellPos>,
    /// Compass heading to move this tick (`None` ⇒ no directed movement; the
    /// creature's own wander behavior takes over).
    pub heading: Option<u8>,
}

/// A creature's persistent cognitive state and the loop that drives it.
#[derive(Clone, Debug)]
pub struct Mind {
    pub memory: Memory,
    /// Currently committed goal (carried for hysteresis).
    pub goal: Goal,
}

impl Mind {
    pub fn new() -> Mind {
        Mind { memory: Memory::with_default_capacity(), goal: Goal::Wander }
    }

    pub fn with_memory(memory: Memory) -> Mind {
        Mind { memory, goal: Goal::Wander }
    }

    /// Run one cognition tick and emit an [`Intent`]. `sightings` are what the
    /// creature perceives now (from the perception layer); `self_state` its
    /// health/hunger; `region`/`loco` how it may move.
    #[allow(clippy::too_many_arguments)]
    pub fn tick<W: WorldQuery>(
        &mut self,
        observer: CellPos,
        sightings: &[Sighting],
        self_state: SelfState,
        disposition: &Disposition,
        region: &NavRegion,
        loco: Locomotion,
        world: &W,
        now: u64,
    ) -> Intent {
        // 1. Appraise against prior memory (before it's updated this tick).
        let signals = appraise(observer, &self.memory, sightings, self_state);
        let novel = self.most_novel(sightings);

        // 2. Learn, then fade.
        for s in sightings {
            self.memory.observe(s.pos, s.tag, s.strength, now);
        }
        self.memory.decay();

        // 3. Decide.
        self.goal = choose(&signals, disposition, self.goal);

        // 4. Where does this goal want to go?
        let threat = self.memory.strongest(LandmarkTag::ThreatStructure).map(|l| l.pos);
        let target = match self.goal {
            Goal::Defend => threat,
            Goal::Raid => self.memory.recall_nearest(LandmarkTag::FoodSource, observer).map(|l| l.pos),
            Goal::Investigate => novel,
            Goal::Patrol => self.patrol_target(observer),
            Goal::Evade => retreat_from(threat, observer),
            Goal::Flee => self
                .memory
                .nest()
                .map(|l| l.pos)
                .or_else(|| retreat_from(threat, observer)),
            Goal::Wander => None,
        };

        // 5. Route to it.
        let heading = target.and_then(|t| heading_toward(observer, t, region, loco, world));
        Intent { goal: self.goal, target, heading }
    }

    /// Position of the most prominent *unrecognized* sighting (Investigate target).
    fn most_novel(&self, sightings: &[Sighting]) -> Option<CellPos> {
        let mut best: Option<(u16, CellPos)> = None;
        for s in sightings {
            let recognized = self
                .memory
                .iter()
                .any(|m| m.tag == s.tag && dist_sq(m.pos, s.pos) <= RECOGNITION_DIST_SQ);
            if !recognized && best.map_or(true, |(bs, _)| s.strength > bs) {
                best = Some((s.strength, s.pos));
            }
        }
        best.map(|(_, p)| p)
    }

    /// Where to patrol: the nest if known, else the nearest natural feature.
    fn patrol_target(&self, observer: CellPos) -> Option<CellPos> {
        self.memory
            .nest()
            .map(|l| l.pos)
            .or_else(|| self.memory.recall_nearest(LandmarkTag::Feature, observer).map(|l| l.pos))
    }
}

impl Default for Mind {
    fn default() -> Mind {
        Mind::new()
    }
}

/// A cell `RETREAT_CELLS` away from `threat`, on the far side of `observer`.
fn retreat_from(threat: Option<CellPos>, observer: CellPos) -> Option<CellPos> {
    let t = threat?;
    let (mut sx, sz) = ((observer.0 - t.0).signum(), (observer.2 - t.2).signum());
    if sx == 0 && sz == 0 {
        sx = 1; // standing on it: pick a direction
    }
    Some((observer.0 + sx * RETREAT_CELLS, observer.1, observer.2 + sz * RETREAT_CELLS))
}

#[inline]
fn dist_sq(a: CellPos, b: CellPos) -> i64 {
    let dx = (a.0 - b.0) as i64;
    let dy = (a.1 - b.1) as i64;
    let dz = (a.2 - b.2) as i64;
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Fixed;
    use crate::sim::physics::SceneWorld;

    fn disp(a: i64, t: i64, c: i64, cu: i64, g: i64) -> Disposition {
        let f = |x: i64| Fixed::from_frac(x, 100);
        Disposition { aggression: f(a), territoriality: f(t), caution: f(c), curiosity: f(cu), greed: f(g) }
    }
    fn state(health: i64, hunger: i64) -> SelfState {
        SelfState { health_frac: Fixed::from_frac(health, 100), hunger: Fixed::from_frac(hunger, 100) }
    }
    fn sight(p: CellPos, tag: LandmarkTag, strength: u16) -> Sighting {
        Sighting { pos: p, tag, strength }
    }
    fn region() -> NavRegion {
        NavRegion::around((0, 0, 0), 32, 3, 1)
    }

    #[test]
    fn sees_food_while_hungry_then_raids_it() {
        let greedy = disp(30, 30, 30, 40, 95);
        let world = SceneWorld::new();
        let mut m = Mind::new();
        let it = m.tick((0, 0, 0), &[sight((20, 0, 0), LandmarkTag::FoodSource, 64)], state(100, 100), &greedy, &region(), Locomotion::Fly, &world, 0);
        assert_eq!(it.goal, Goal::Raid);
        assert_eq!(it.target, Some((20, 0, 0)));
        assert!(it.heading.is_some(), "should have a heading toward the food");
    }

    #[test]
    fn revisits_remembered_food_when_nothing_visible() {
        let greedy = disp(30, 30, 30, 40, 95);
        let world = SceneWorld::new();
        let mut m = Mind::new();
        m.memory.observe((40, 0, 0), LandmarkTag::FoodSource, 50, 0);
        let it = m.tick((0, 0, 0), &[], state(100, 100), &greedy, &region(), Locomotion::Fly, &world, 1);
        assert_eq!(it.goal, Goal::Raid);
        assert_eq!(it.target, Some((40, 0, 0)));
    }

    #[test]
    fn temperament_decides_defend_vs_evade() {
        let bold = disp(90, 90, 10, 50, 50);
        let timid = disp(10, 20, 90, 50, 50);
        let world = SceneWorld::new();

        let mut b = Mind::new();
        b.memory.observe((0, 0, 0), LandmarkTag::Nest, 1, 0);
        let ib = b.tick((0, 0, 0), &[sight((8, 0, 0), LandmarkTag::ThreatStructure, 64)], state(100, 0), &bold, &region(), Locomotion::Fly, &world, 1);
        assert_eq!(ib.goal, Goal::Defend);
        assert_eq!(ib.target, Some((8, 0, 0)));

        let mut t = Mind::new();
        t.memory.observe((0, 0, 0), LandmarkTag::Nest, 1, 0);
        let it = t.tick((10, 0, 10), &[sight((18, 0, 10), LandmarkTag::ThreatStructure, 64)], state(100, 0), &timid, &region(), Locomotion::Fly, &world, 1);
        assert_eq!(it.goal, Goal::Evade);
        // retreat target is on the far side of the observer from the +x threat
        assert!(it.target.unwrap().0 < 10, "should retreat away from the threat");
    }

    #[test]
    fn hurt_creature_flees_home() {
        let timid = disp(10, 20, 90, 50, 50);
        let world = SceneWorld::new();
        let mut m = Mind::new();
        m.memory.observe((100, 0, 100), LandmarkTag::Nest, 1, 0);
        let it = m.tick((50, 0, 50), &[sight((52, 0, 50), LandmarkTag::ThreatStructure, 64)], state(15, 0), &timid, &region(), Locomotion::Fly, &world, 1);
        assert_eq!(it.goal, Goal::Flee);
        assert_eq!(it.target, Some((100, 0, 100)));
    }

    #[test]
    fn curiosity_investigates_the_novel() {
        let curious = disp(30, 30, 30, 95, 30);
        let world = SceneWorld::new();
        let mut m = Mind::new();
        let it = m.tick((0, 0, 0), &[sight((30, 0, 0), LandmarkTag::Feature, 60)], state(100, 0), &curious, &region(), Locomotion::Fly, &world, 0);
        assert_eq!(it.goal, Goal::Investigate);
        assert_eq!(it.target, Some((30, 0, 0)));
    }

    #[test]
    fn memory_persists_then_is_forgotten() {
        let greedy = disp(30, 30, 30, 40, 95);
        let world = SceneWorld::new();
        let mut m = Mind::new();
        m.tick((0, 0, 0), &[sight((20, 0, 0), LandmarkTag::FoodSource, 64)], state(100, 0), &greedy, &region(), Locomotion::Fly, &world, 0);
        assert!(m.memory.recall_nearest(LandmarkTag::FoodSource, (0, 0, 0)).is_some());
        for k in 0..200u64 {
            m.tick((0, 0, 0), &[], state(100, 0), &greedy, &region(), Locomotion::Fly, &world, k + 1);
        }
        assert!(m.memory.recall_nearest(LandmarkTag::FoodSource, (0, 0, 0)).is_none(), "unreinforced food fades");
    }
}
