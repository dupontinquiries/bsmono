//! Landmark memory — a creature's spatial "cortex": a bounded set of
//! remembered salient places, reinforced when re-seen and decaying toward
//! forgetting when not. This is what lets a dragon remember the village it
//! raids and forget the keep that was torn down.
//!
//! Server-side: this is **not** part of [`crate::sim::state::SimState`] and is
//! never rolled back. It is written with integer/[`Fixed`] state anyway so it
//! stays replay-reproducible and unit-testable.
//!
//! Lifecycle: perception calls [`Memory::observe`] for each salient feature it
//! sees this cognition tick (reinforcing or inserting), then [`Memory::decay`]
//! is called once to fade everything unreinforced. Appraisal and goal-target
//! selection then read the memory via [`Memory::recall_nearest`] /
//! [`Memory::strongest`] / [`Memory::nest`].

use crate::sim::fixed::Fixed;

/// Integer world-cell position of a remembered place.
pub type CellPos = (i32, i32, i32);

/// What a landmark means to the creature.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LandmarkTag {
    /// Home anchor — defines territory.
    Nest,
    /// Village / livestock — a raid target.
    FoodSource,
    /// Player fortification — to defend against or evade.
    ThreatStructure,
    /// Accumulated loot site.
    Hoard,
    /// Natural navigation landmark (cliff, lake, peak).
    Feature,
}

/// One remembered place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Landmark {
    pub pos: CellPos,
    pub tag: LandmarkTag,
    /// `[0, 1]`; reinforced on re-sighting, decays otherwise.
    pub salience: Fixed,
    /// Tick of the most recent sighting.
    pub last_seen: u64,
    /// Crude size/value estimate (pigs counted, blocks, …).
    pub strength: u16,
}

// ── tunables (see §9 of the cognition design doc) ─────────────────────────
/// Default per-creature memory capacity.
pub const DEFAULT_CAPACITY: usize = 32;
/// Salience a freshly recorded landmark starts at.
const INITIAL_SALIENCE: Fixed = Fixed::from_frac(34, 100);
/// Salience added each time a landmark is re-seen (saturating at 1).
const REINFORCE_BOOST: Fixed = Fixed::from_frac(34, 100);
/// Multiplicative salience decay per [`Memory::decay`] step. ~0.98 ⇒ a
/// once-seen landmark fades in ~95 steps, a saturated one in ~148.
const DECAY_FACTOR: Fixed = Fixed::from_frac(98, 100);
/// Below this salience a landmark is forgotten.
const FORGET_FLOOR: Fixed = Fixed::from_frac(5, 100);
/// Squared distance (blocks²) within which a same-tag sighting reinforces an
/// existing landmark instead of creating a near-duplicate (8 blocks).
const MERGE_DIST_SQ: i64 = 64;

#[inline]
fn dist_sq(a: CellPos, b: CellPos) -> i64 {
    let dx = (a.0 - b.0) as i64;
    let dy = (a.1 - b.1) as i64;
    let dz = (a.2 - b.2) as i64;
    dx * dx + dy * dy + dz * dz
}

/// A creature's bounded landmark memory.
#[derive(Clone, Debug)]
pub struct Memory {
    landmarks: Vec<Landmark>,
    capacity: usize,
}

impl Memory {
    pub fn new(capacity: usize) -> Memory {
        Memory { landmarks: Vec::new(), capacity: capacity.max(1) }
    }

    pub fn with_default_capacity() -> Memory {
        Memory::new(DEFAULT_CAPACITY)
    }

    pub fn len(&self) -> usize {
        self.landmarks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.landmarks.is_empty()
    }
    /// Iterate all landmarks (for debug overlays / appraisal scans).
    pub fn iter(&self) -> impl Iterator<Item = &Landmark> {
        self.landmarks.iter()
    }

    /// Record a sighting. If a same-tag landmark already sits within the merge
    /// radius, reinforce it (boost salience, refresh `last_seen`, take the
    /// larger `strength`); otherwise insert a new one — evicting the weakest if
    /// at capacity, and only if the newcomer is stronger than that weakest.
    pub fn observe(&mut self, pos: CellPos, tag: LandmarkTag, strength: u16, now: u64) {
        if let Some(m) = self
            .landmarks
            .iter_mut()
            .find(|m| m.tag == tag && dist_sq(m.pos, pos) <= MERGE_DIST_SQ)
        {
            m.salience = (m.salience + REINFORCE_BOOST).min(Fixed::ONE);
            m.last_seen = now;
            m.strength = m.strength.max(strength);
            return;
        }

        let fresh = Landmark { pos, tag, salience: INITIAL_SALIENCE, last_seen: now, strength };
        if self.landmarks.len() < self.capacity {
            self.landmarks.push(fresh);
            return;
        }
        // At capacity: replace the weakest, but only if the newcomer beats it
        // (a full memory of strong places won't be displaced by a faint one).
        if let Some((wi, weakest)) = self
            .landmarks
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.salience.cmp(&b.salience))
            .map(|(i, m)| (i, m.salience))
        {
            if fresh.salience > weakest {
                self.landmarks[wi] = fresh;
            }
        }
    }

    /// Apply one decay step (call once per cognition tick). Returns how many
    /// landmarks dropped below the forget floor and were removed.
    pub fn decay(&mut self) -> usize {
        for m in &mut self.landmarks {
            m.salience = m.salience * DECAY_FACTOR;
        }
        let before = self.landmarks.len();
        self.landmarks.retain(|m| m.salience >= FORGET_FLOOR);
        before - self.landmarks.len()
    }

    /// The nearest remembered landmark of `tag` to `from`, if any.
    pub fn recall_nearest(&self, tag: LandmarkTag, from: CellPos) -> Option<&Landmark> {
        self.landmarks
            .iter()
            .filter(|m| m.tag == tag)
            .min_by_key(|m| dist_sq(m.pos, from))
    }

    /// The most salient landmark of `tag`, if any (e.g. the worst threat).
    pub fn strongest(&self, tag: LandmarkTag) -> Option<&Landmark> {
        self.landmarks
            .iter()
            .filter(|m| m.tag == tag)
            .max_by(|a, b| a.salience.cmp(&b.salience))
    }

    /// The creature's nest (territory anchor), if one is remembered.
    pub fn nest(&self) -> Option<&Landmark> {
        self.landmarks.iter().find(|m| m.tag == LandmarkTag::Nest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resighting_reinforces_without_duplicating() {
        let mut m = Memory::new(8);
        m.observe((100, 60, 200), LandmarkTag::FoodSource, 10, 0);
        let after_one = m.iter().next().unwrap().salience;
        // within 8 blocks, same tag → merge (not a second entry)
        m.observe((103, 60, 202), LandmarkTag::FoodSource, 14, 5);
        assert_eq!(m.len(), 1);
        let lm = m.iter().next().unwrap();
        assert!(lm.salience > after_one, "re-sighting should raise salience");
        assert_eq!(lm.strength, 14, "strength takes the larger estimate");
        assert_eq!(lm.last_seen, 5);
        // a different tag at the same spot is a distinct landmark
        m.observe((100, 60, 200), LandmarkTag::ThreatStructure, 30, 6);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn reinforcement_saturates() {
        let mut m = Memory::new(8);
        for k in 0..5 {
            m.observe((0, 0, 0), LandmarkTag::Nest, 1, k);
        }
        assert!(m.nest().unwrap().salience > Fixed::from_frac(95, 100));
        assert!(m.nest().unwrap().salience <= Fixed::ONE);
    }

    #[test]
    fn unreinforced_landmarks_are_forgotten() {
        let mut m = Memory::new(8);
        m.observe((0, 0, 0), LandmarkTag::ThreatStructure, 5, 0);
        let mut steps = 0;
        while !m.is_empty() {
            m.decay();
            steps += 1;
            assert!(steps < 1000, "must eventually forget");
        }
        assert!((80..130).contains(&steps), "fade time ~95 steps, got {steps}");
    }

    #[test]
    fn known_landmarks_persist_longer() {
        let mut faint = Memory::new(8);
        faint.observe((0, 0, 0), LandmarkTag::Feature, 1, 0);
        let mut s_faint = 0;
        while !faint.is_empty() { faint.decay(); s_faint += 1; }

        let mut known = Memory::new(8);
        for k in 0..5 { known.observe((0, 0, 0), LandmarkTag::Feature, 1, k); }
        let mut s_known = 0;
        while !known.is_empty() { known.decay(); s_known += 1; }

        assert!(s_known > s_faint, "a well-known place fades slower than a glimpsed one");
    }

    #[test]
    fn capacity_evicts_the_weakest() {
        let mut m = Memory::new(4);
        for i in 0..4 {
            m.observe((i * 20, 0, 0), LandmarkTag::Feature, 1, 0);
        }
        for _ in 0..3 {
            m.decay(); // all four fade equally
        }
        // a fresh sighting (higher salience than the decayed four) displaces one
        m.observe((999, 0, 0), LandmarkTag::FoodSource, 1, 100);
        assert_eq!(m.len(), 4, "stays at capacity");
        assert!(m.iter().any(|l| l.tag == LandmarkTag::FoodSource), "fresh landmark was kept");
    }

    #[test]
    fn recall_returns_the_nearest_of_a_tag() {
        let mut m = Memory::new(8);
        m.observe((10, 0, 0), LandmarkTag::FoodSource, 1, 0);
        m.observe((50, 0, 0), LandmarkTag::FoodSource, 1, 0);
        m.observe((5, 0, 5), LandmarkTag::ThreatStructure, 1, 0);
        let n = m.recall_nearest(LandmarkTag::FoodSource, (0, 0, 0)).unwrap();
        assert_eq!(n.pos, (10, 0, 0));
        assert!(m.recall_nearest(LandmarkTag::Hoard, (0, 0, 0)).is_none());
    }
}
