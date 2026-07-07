//! Foraging drive — the food half of the brain.
//!
//! The sentinel turns senses into a threat reveal; this is its complement. A
//! hunger that rises over time and, once high enough, turns the creature's food
//! memory (`cognition::memory`) into a goal: the nearest remembered
//! `FoodSource`. Eating sates the hunger and reinforces that spot, so a herd
//! drifts back to reliable feeding grounds while forgotten ones fade with the
//! memory's own decay. Pure + `Copy`; the memory it reads and writes is the
//! shared cognition store, so what one creature learns can seed a herd's.

use crate::sim::cognition::memory::{CellPos, LandmarkTag, Memory};

/// Per-creature hunger + foraging policy. `hunger` climbs each tick toward
/// `max`; at or above `seek_at` the creature wants to find food.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Forager {
    pub hunger: u16,
    pub seek_at: u16,
    pub max: u16,
}

impl Forager {
    /// Starts fed; wants food once hunger reaches `seek_at`, starves at `2×`.
    pub fn new(seek_at: u16) -> Forager {
        Forager { hunger: 0, seek_at, max: seek_at.saturating_mul(2).max(seek_at) }
    }

    pub fn is_hungry(&self) -> bool {
        self.hunger >= self.seek_at
    }

    pub fn is_starving(&self) -> bool {
        self.hunger >= self.max
    }

    /// One tick of metabolism — hunger creeps up to the cap.
    pub fn tick(&mut self) {
        self.hunger = self.hunger.saturating_add(1).min(self.max);
    }

    /// Record a food source the creature has noticed (reinforces memory).
    pub fn remember_food(&self, mem: &mut Memory, pos: CellPos, strength: u16, now: u64) {
        mem.observe(pos, LandmarkTag::FoodSource, strength, now);
    }

    /// The creature's current goal: when hungry, the nearest remembered food
    /// from `from`; `None` when sated or nothing is remembered.
    pub fn goal(&self, mem: &Memory, from: CellPos) -> Option<CellPos> {
        if !self.is_hungry() {
            return None;
        }
        mem.recall_nearest(LandmarkTag::FoodSource, from).map(|l| l.pos)
    }

    /// Reached and ate food at `pos`: hunger resets and the spot is reinforced so
    /// the creature — and, via the shared store, its herd — remembers to return.
    pub fn eat(&mut self, mem: &mut Memory, pos: CellPos, strength: u16, now: u64) {
        self.hunger = 0;
        self.remember_food(mem, pos, strength, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunger_rises_to_a_cap_and_crosses_the_seek_threshold() {
        let mut f = Forager::new(10);
        assert!(!f.is_hungry());
        for _ in 0..10 {
            f.tick();
        }
        assert!(f.is_hungry(), "hungry once hunger reaches seek_at");
        for _ in 0..100 {
            f.tick();
        }
        assert_eq!(f.hunger, f.max, "hunger saturates at the cap");
        assert!(f.is_starving());
    }

    #[test]
    fn a_full_creature_has_no_goal() {
        let f = Forager::new(10);
        let mut mem = Memory::with_default_capacity();
        f.remember_food(&mut mem, (5, 0, 0), 3, 0);
        assert_eq!(f.goal(&mem, (0, 0, 0)), None, "sated → no foraging goal");
    }

    #[test]
    fn a_hungry_creature_seeks_the_nearest_remembered_food() {
        let mut f = Forager::new(2);
        let mut mem = Memory::with_default_capacity();
        f.remember_food(&mut mem, (50, 0, 0), 3, 0);
        f.remember_food(&mut mem, (8, 0, 6), 3, 0); // nearer
        f.tick();
        f.tick();
        assert!(f.is_hungry());
        assert_eq!(f.goal(&mem, (0, 0, 0)), Some((8, 0, 6)), "heads for the closest food");
    }

    #[test]
    fn hungry_with_no_memory_has_no_goal() {
        let mut f = Forager::new(1);
        let mem = Memory::with_default_capacity();
        f.tick();
        assert!(f.is_hungry());
        assert_eq!(f.goal(&mem, (0, 0, 0)), None, "nothing remembered → nowhere to go");
    }

    #[test]
    fn eating_sates_and_reinforces_the_spot() {
        let mut f = Forager::new(3);
        let mut mem = Memory::with_default_capacity();
        for _ in 0..5 {
            f.tick();
        }
        assert!(f.is_hungry());
        f.eat(&mut mem, (12, 0, 4), 5, 7);
        assert_eq!(f.hunger, 0, "eating sates");
        assert!(!f.is_hungry());
        // The eaten spot is now a remembered food source to return to.
        assert_eq!(mem.recall_nearest(LandmarkTag::FoodSource, (0, 0, 0)).map(|l| l.pos), Some((12, 0, 4)));
    }
}
