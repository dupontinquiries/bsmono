//! class.rs — playable creature **classes**, shared across game modes.
//!
//! Ported from the browser prototypes (Pop Island 2 "Tails of the Wild" and the
//! Prowl/SMITE-2 mode). Each class is a distinct creature with terrain-specialised
//! movement: it moves at one speed on land, another in water, another on the
//! mountains, so *where* you are on the world heightfield changes who's fastest.
//! Flying classes ignore the terrain entirely. This is the key shared mechanic —
//! it reads the same `world` heightfield the rest of the engine uses.

use crate::world;

/// Height above which terrain counts as "mountain" for movement (matches the
/// MOUNTAINS biome cutoff in worldgen); below `world::WATER_Y` counts as water.
pub const MTN_Y: f32 = 1.9;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class { Cat, Squirrel, Otter, Frog, Goat, Hawk }

pub const CLASSES: [Class; 6] = [Class::Cat, Class::Squirrel, Class::Otter, Class::Frog, Class::Goat, Class::Hawk];

/// Static per-class data. Speeds are in world units/second on each terrain type.
#[derive(Clone, Copy)]
pub struct ClassDef {
    pub name: &'static str,
    pub land: f32, pub water: f32, pub mtn: f32,
    pub jump: f32,      // leap power
    pub stun: f32,      // how long this class stays stunned when hit (lower = tougher)
    pub fly: bool,      // ignores terrain, moves at a constant pace everywhere
    pub color: u32,
    pub trait_: &'static str,
}

/// Each class's signature active ability.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ability { SecondWind, Scamper, Dive, Bound, Charge, Stoop }

impl Ability {
    pub fn name(self) -> &'static str {
        match self {
            Ability::SecondWind => "Second Wind", Ability::Scamper => "Scamper", Ability::Dive => "Dive",
            Ability::Bound => "Bound", Ability::Charge => "Charge", Ability::Stoop => "Stoop",
        }
    }
    /// One-line description for a HUD/tooltip.
    pub fn blurb(self) -> &'static str {
        match self {
            Ability::SecondWind => "catch your breath — refill stamina, briefly untouchable",
            Ability::Scamper => "a blazing sprint",
            Ability::Dive => "slip away fast and untouchable",
            Ability::Bound => "a huge airborne leap",
            Ability::Charge => "an unstoppable charge",
            Ability::Stoop => "a screaming dive",
        }
    }
    pub fn cooldown(self) -> f32 {
        match self { Ability::Bound => 8.0, Ability::Scamper | Ability::Charge | Ability::Stoop => 10.0, _ => 12.0 }
    }
    /// How long the ability's effect lasts once triggered.
    pub fn duration(self) -> f32 {
        match self { Ability::SecondWind | Ability::Bound => 0.6, Ability::Charge | Ability::Stoop => 1.5, Ability::Scamper => 2.0, Ability::Dive => 3.0 }
    }
    /// Movement multiplier while active.
    pub fn speed_mult(self) -> f32 {
        match self { Ability::SecondWind => 1.0, Ability::Dive => 1.5, Ability::Scamper => 1.9, Ability::Charge => 2.2, Ability::Stoop => 2.6, Ability::Bound => 3.0 }
    }
    /// Whether you're uncatchable while it's active.
    pub fn invuln(self) -> bool {
        matches!(self, Ability::SecondWind | Ability::Dive | Ability::Bound | Ability::Stoop)
    }
    /// Whether triggering it refills stamina.
    pub fn refills(self) -> bool { matches!(self, Ability::SecondWind) }
}

/// Each class's passive trait — an always-on edge that plays into its terrain
/// specialisation, distinct from its active ability. Gives every class a second
/// point of differentiation (SMITE-style kit depth: passive + active).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Passive { NineLives, Cache, Slipstream, Camouflage, SureFooted, KeenEyes }

impl Passive {
    pub fn name(self) -> &'static str {
        match self {
            Passive::NineLives => "Nine Lives", Passive::Cache => "Cache",
            Passive::Slipstream => "Slipstream", Passive::Camouflage => "Camouflage",
            Passive::SureFooted => "Sure-Footed", Passive::KeenEyes => "Keen Eyes",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Passive::NineLives => "cheats death the first time it would be caught each match",
            Passive::Cache => "claims ancestor trees 30% faster — quick paws",
            Passive::Slipstream => "never tires while sprinting in water",
            Passive::Camouflage => "suspicion melts away faster while hidden",
            Passive::SureFooted => "harder to spot while up on the mountains",
            Passive::KeenEyes => "always senses the hunter's position from the air",
        }
    }
}

impl Class {
    /// The class's passive trait — always on, complementing its active ability.
    pub fn passive(self) -> Passive {
        match self {
            Class::Cat => Passive::NineLives,
            Class::Squirrel => Passive::Cache,
            Class::Otter => Passive::Slipstream,
            Class::Frog => Passive::Camouflage,
            Class::Goat => Passive::SureFooted,
            Class::Hawk => Passive::KeenEyes,
        }
    }

    pub fn def(self) -> ClassDef {
        match self {
            // land, water, mtn, jump, stun, fly, color, trait  — ported values
            Class::Cat      => ClassDef { name: "Cat",      land: 7.0, water: 3.0,  mtn: 5.0,  jump: 6.0,  stun: 1.2, fly: false, color: 0xc88a52, trait_: "Balanced all-rounder" },
            Class::Squirrel => ClassDef { name: "Squirrel", land: 9.5, water: 2.0,  mtn: 7.0,  jump: 9.0,  stun: 1.4, fly: false, color: 0xa05a28, trait_: "Blazing fast on land, agile jumper" },
            Class::Otter    => ClassDef { name: "Otter",    land: 4.5, water: 11.0, mtn: 3.0,  jump: 3.0,  stun: 1.1, fly: false, color: 0x6a5340, trait_: "King of the river — slow ashore" },
            Class::Frog     => ClassDef { name: "Frog",     land: 6.5, water: 9.0,  mtn: 4.5,  jump: 13.0, stun: 1.3, fly: false, color: 0x5a8a3a, trait_: "Huge leaps, strong swimmer" },
            Class::Goat     => ClassDef { name: "Goat",     land: 5.5, water: 2.0,  mtn: 10.5, jump: 5.0,  stun: 0.7, fly: false, color: 0xc8c0b0, trait_: "Mountain sprinter, tough to stun" },
            Class::Hawk     => ClassDef { name: "Hawk",     land: 6.0, water: 6.0,  mtn: 6.0,  jump: 0.0,  stun: 2.0, fly: true,  color: 0x8a6230, trait_: "Flies over all terrain, but fragile" },
        }
    }

    pub fn name(self) -> &'static str { self.def().name }

    /// The class's signature active ability.
    pub fn ability(self) -> Ability {
        match self {
            Class::Cat => Ability::SecondWind,
            Class::Squirrel => Ability::Scamper,
            Class::Otter => Ability::Dive,
            Class::Frog => Ability::Bound,
            Class::Goat => Ability::Charge,
            Class::Hawk => Ability::Stoop,
        }
    }

    /// Terrain kind under a point: 0 = water, 1 = land, 2 = mountain.
    pub fn terrain_at(x: f32, z: f32) -> u8 {
        let y = world::height_at(x, z);
        if y < world::WATER_Y { 0 } else if y > MTN_Y { 2 } else { 1 }
    }

    /// Movement speed for this class at a world position. Flyers are terrain-agnostic
    /// (they cruise at their land speed everywhere); everyone else uses the speed of
    /// the terrain they're standing on.
    pub fn speed_at(self, x: f32, z: f32) -> f32 {
        let d = self.def();
        if d.fly { return d.land; }
        match Self::terrain_at(x, z) {
            0 => d.water,
            2 => d.mtn,
            _ => d.land,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_is_complete_and_named() {
        assert_eq!(CLASSES.len(), 6);
        assert_eq!(Class::Otter.name(), "Otter");
        assert!(Class::Hawk.def().fly);
        assert!(!Class::Cat.def().fly);
    }

    #[test]
    fn each_class_is_fastest_on_its_terrain() {
        // otter rules the water, goat the mountains, squirrel the land
        let o = Class::Otter.def();
        assert!(o.water > o.land && o.water > o.mtn, "otter should be fastest in water");
        let g = Class::Goat.def();
        assert!(g.mtn > g.land && g.mtn > g.water, "goat should be fastest on mountains");
        let s = Class::Squirrel.def();
        assert!(s.land > s.water && s.land > s.mtn, "squirrel should be fastest on land");
    }

    #[test]
    fn flyer_speed_is_terrain_independent() {
        world::init_world();
        // scan a range of terrains; the hawk's speed never changes, a ground class's does
        let mut hawk_speeds = std::collections::HashSet::new();
        let mut otter_varies = false;
        let base = Class::Otter.speed_at(0.0, 0.0);
        for i in -40..40 {
            let (x, z) = (i as f32 * 4.0, (i as f32 * 2.7).sin() * 30.0);
            hawk_speeds.insert(Class::Hawk.speed_at(x, z).to_bits());
            if (Class::Otter.speed_at(x, z) - base).abs() > 0.5 { otter_varies = true; }
        }
        assert_eq!(hawk_speeds.len(), 1, "the hawk flies at one speed everywhere");
        assert!(otter_varies, "a ground class's speed should change with terrain");
    }

    #[test]
    fn terrain_classifier_matches_heightfield() {
        world::init_world();
        // a point below the water line classifies as water
        let mut saw_water = false;
        for i in -60..60 {
            let (x, z) = (i as f32 * 3.0, 0.0);
            if world::height_at(x, z) < world::WATER_Y { saw_water = true; assert_eq!(Class::terrain_at(x, z), 0); }
        }
        assert!(saw_water, "the scan should cross some water");
    }

    #[test]
    fn every_class_has_a_distinct_ability() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for c in CLASSES {
            let a = c.ability();
            assert!(seen.insert(a), "abilities should be unique per class");
            assert!(a.cooldown() > 0.0 && a.duration() > 0.0 && a.speed_mult() >= 1.0);
            assert!(!a.name().is_empty());
        }
        assert_eq!(seen.len(), 6);
        assert!(Class::Frog.ability().speed_mult() >= 3.0, "the frog's Bound is the big leap");
        assert!(Class::Cat.ability().refills(), "the cat's Second Wind refills stamina");
    }

    #[test]
    fn every_class_has_a_distinct_passive() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for c in CLASSES {
            let p = c.passive();
            assert!(seen.insert(p), "passives should be unique per class");
            assert!(!p.name().is_empty() && !p.blurb().is_empty());
        }
        assert_eq!(seen.len(), 6);
        assert_eq!(Class::Otter.passive(), Passive::Slipstream, "the river king never tires in water");
        assert_eq!(Class::Hawk.passive(), Passive::KeenEyes, "the flyer always senses the hunter");
    }
}
