//! LOOT — what a raid pays out, and why it is never a random number.
//!
//! Every piece here is already decided by systems that exist. A raid happens
//! on a world with a temperature, an atmosphere and a star that flares. The
//! bosses have classes. The player has four slots and an envelope. So a drop
//! is not "roll a rarity" — it is: what did this place demand, and what
//! would have made it survivable?
//!
//! Three rules, each of which a test enforces:
//!
//! **A drop reflects where it was won.** A cryogenic world pays out cold
//! tolerance. A flare-swept one pays shielding. That is not flavour: it
//! means clearing a hard world is how you get the gear for the NEXT hard
//! world, and it makes the map a progression without anyone drawing one.
//!
//! **A drop must be equippable by someone in the fireteam.** Paying gene
//! therapy to an all-machine squad is a wasted drop, and the player reads it
//! as the game not knowing what it gave them.
//!
//! **Nothing is strictly better than everything.** Slots are the constraint
//! that makes the whole loadout system a choice; a drop that dominates its
//! slot cost quietly deletes that choice.

use crate::loadout::{Effect, Mod, MODS};
use crate::sun::{Orbit, Surface};
use crate::survival::Body;

/// How good a drop is. Not a damage multiplier — a SLOT DISCOUNT and a
/// magnitude bump, because those are the two currencies the loadout system
/// actually spends.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Grade {
    Salvage,
    Standard,
    Refined,
    Relic,
}

impl Grade {
    /// (magnitude multiplier, slots saved)
    pub fn spec(self) -> (f32, u8) {
        match self {
            Grade::Salvage => (0.7, 0),
            Grade::Standard => (1.0, 0),
            Grade::Refined => (1.25, 0),
            // a relic is the same effect for one slot less, which is worth
            // far more than a bigger number: it buys back a CHOICE
            Grade::Relic => (1.4, 1),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Grade::Salvage => "SALVAGE",
            Grade::Standard => "STANDARD",
            Grade::Refined => "REFINED",
            Grade::Relic => "RELIC",
        }
    }
}

/// A thing a raid gave you.
#[derive(Clone, Copy, Debug)]
pub struct Drop {
    pub base: Mod,
    pub grade: Grade,
    /// the world it came off, for the name and for the player's memory of it
    pub from: &'static str,
}

impl Drop {
    /// The mod as actually fitted, grade applied.
    pub fn fitted(&self) -> Mod {
        let (mult, save) = self.grade.spec();
        Mod {
            magnitude: self.base.magnitude * mult,
            slots: self.base.slots.saturating_sub(save).max(1),
            ..self.base
        }
    }
    /// Can anyone in this fireteam wear it?
    pub fn usable_by(&self, team: &[Body]) -> bool {
        team.iter().any(|b| self.base.fits.accepts(*b))
    }
}

/// WHAT THIS WORLD DEMANDS. The drop table is derived from the hazard rather
/// than authored per destination, so a generated world has a loot table the
/// moment it has an orbit.
pub fn demanded(orbit: &Orbit) -> Effect {
    let flare = orbit.flare_severity();
    match orbit.surface() {
        Surface::Molten | Surface::Scorched if flare < 0.5 => Effect::HeatCeiling,
        Surface::Cryogenic | Surface::Frozen if flare < 0.5 => Effect::ColdFloor,
        _ if flare >= 0.5 => Effect::Shielding,
        // a temperate world with a quiet star tests neither, so it pays out
        // the thing that lets you STAY: endurance, or the speed to cross it
        Surface::Temperate => Effect::Endurance,
        _ => Effect::Shielding,
    }
}

/// Roll a drop. Deterministic in `seed`, so a raid's payout is a property of
/// that raid rather than of when you happened to open it.
pub fn roll(orbit: &Orbit, world_name: &'static str, team: &[Body], seed: u32, boss_tier: u8) -> Option<Drop> {
    let want = demanded(orbit);
    // candidates: the right effect, and wearable by someone present
    let mut pool: Vec<&Mod> = MODS
        .iter()
        .filter(|m| m.effect == want && team.iter().any(|b| m.fits.accepts(*b)))
        .collect();
    if pool.is_empty() {
        // nothing of the demanded kind fits this team — fall back to
        // anything they CAN wear rather than paying out a wasted drop
        pool = MODS
            .iter()
            .filter(|m| team.iter().any(|b| m.fits.accepts(*b)))
            .collect();
    }
    if pool.is_empty() {
        return None;
    }
    let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(0x9E37_79B9) | 1;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s >> 8) as f32 / 16_777_216.0
    };
    let base = *pool[(next() * pool.len() as f32) as usize % pool.len()];
    // GRADE RISES WITH THE BOSS AND WITH THE WORLD'S HAZARD, because a relic
    // should feel like it came off something that nearly killed you
    let hazard = orbit.flare_severity().max(match orbit.surface() {
        Surface::Molten => 0.9,
        Surface::Scorched | Surface::Cryogenic => 0.6,
        Surface::Frozen => 0.4,
        Surface::Temperate => 0.15,
    });
    let r = next() * 0.55 + hazard * 0.3 + boss_tier as f32 * 0.12;
    let grade = if r > 0.92 {
        Grade::Relic
    } else if r > 0.66 {
        Grade::Refined
    } else if r > 0.30 {
        Grade::Standard
    } else {
        Grade::Salvage
    };
    Some(Drop { base, grade, from: world_name })
}

/// Would this drop change anything? A drop the team already outclasses is
/// noise, and telling the player so is better than letting them find out by
/// equipping it.
pub fn improves(d: &Drop, current: &[Mod], body: Body) -> bool {
    if !d.base.fits.accepts(body) {
        return false;
    }
    let f = d.fitted();
    let best = current
        .iter()
        .filter(|m| m.effect == f.effect && m.fits.accepts(body))
        .map(|m| m.magnitude / m.slots.max(1) as f32)
        .fold(0.0f32, f32::max);
    f.magnitude / f.slots.max(1) as f32 > best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sun::StarClass;

    const MACHINE: [Body; 1] = [Body::Cybertronian];
    const MIXED: [Body; 2] = [Body::Cybertronian, Body::HumanSuited];

    fn world(au: f32, albedo: f32, air: f32, star: StarClass) -> Orbit {
        Orbit { star, au, albedo, air }
    }

    #[test]
    fn a_drop_reflects_the_world_it_was_won_on() {
        // this is what makes the map a progression without anyone drawing one
        let hot = world(0.15, 0.1, 0.0, StarClass::G);
        let cold = world(30.0, 0.5, 0.0, StarClass::G);
        let flarey = world(0.05, 0.2, 0.0, StarClass::M);
        assert_eq!(demanded(&hot), Effect::HeatCeiling, "{} K", hot.surface_k());
        assert_eq!(demanded(&cold), Effect::ColdFloor);
        assert_eq!(demanded(&flarey), Effect::Shielding);
    }

    #[test]
    fn a_flare_swept_world_pays_shielding_even_when_it_is_also_freezing() {
        // 0.12 AU, not 0.35. Flare severity falls with distance as
        // sqrt(1/au), so an M dwarf at 0.35 only reaches 0.38 — genuinely
        // not flare-swept, and the model was right to say so. The severity
        // threshold is where "the thing that will kill you first" changes,
        // and it is a real crossing rather than a label.
        let both = world(0.12, 0.4, 0.0, StarClass::M);
        assert!(both.flare_severity() >= 0.5, "{}", both.flare_severity());
        assert_eq!(demanded(&both), Effect::Shielding);
        // and just outside it, the cold wins instead
        let colder = world(0.5, 0.4, 0.0, StarClass::M);
        assert!(colder.flare_severity() < 0.5);
        assert_eq!(demanded(&colder), Effect::ColdFloor);
    }

    #[test]
    fn nothing_drops_that_nobody_present_can_wear() {
        // paying gene therapy to an all-machine squad reads as the game not
        // knowing what it gave you
        let w = world(1.0, 0.3, 0.3, StarClass::G);
        for seed in 0..80 {
            let d = roll(&w, "TEST", &MACHINE, seed, 1).expect("no drop at all");
            assert!(d.usable_by(&MACHINE), "{} cannot be worn by a machine", d.base.name);
            assert_ne!(d.base.fits, BodyFit::Human);
        }
    }

    #[test]
    fn a_relic_buys_back_a_slot_which_is_worth_more_than_a_bigger_number() {
        let base = *MODS.iter().find(|m| m.slots >= 2).unwrap();
        let relic = Drop { base, grade: Grade::Relic, from: "X" }.fitted();
        let std = Drop { base, grade: Grade::Standard, from: "X" }.fitted();
        assert_eq!(relic.slots, std.slots - 1);
        assert!(relic.magnitude > std.magnitude);
        // and it can never drop below one slot, or it would be free
        let one = *MODS.iter().find(|m| m.slots == 1).unwrap();
        assert_eq!(Drop { base: one, grade: Grade::Relic, from: "X" }.fitted().slots, 1);
    }

    #[test]
    fn hard_worlds_and_hard_bosses_pay_better() {
        // a relic should feel like it came off something that nearly killed
        // you, which means the distribution has to actually shift
        let easy = world(1.0, 0.3, 0.4, StarClass::G);
        let brutal = world(0.05, 0.15, 0.0, StarClass::M);
        let count = |w: &Orbit, tier: u8| {
            (0..300)
                .filter_map(|s| roll(w, "T", &MIXED, s, tier))
                .filter(|d| d.grade >= Grade::Refined)
                .count()
        };
        let lo = count(&easy, 0);
        let hi = count(&brutal, 3);
        assert!(hi > lo * 2, "easy {lo} vs brutal {hi}");
    }

    #[test]
    fn the_same_raid_always_pays_the_same_thing() {
        // a payout is a property of the raid, not of when you opened it
        let w = world(0.6, 0.3, 0.2, StarClass::K);
        let a = roll(&w, "T", &MIXED, 4242, 2).unwrap();
        let b = roll(&w, "T", &MIXED, 4242, 2).unwrap();
        assert_eq!(a.base.name, b.base.name);
        assert_eq!(a.grade, b.grade);
    }

    #[test]
    fn a_drop_the_team_already_outclasses_is_reported_as_such() {
        let base = *MODS
            .iter()
            .find(|m| m.effect == Effect::Shielding && m.fits.accepts(Body::Cybertronian))
            .unwrap();
        let held = [Mod { magnitude: base.magnitude * 3.0, ..base }];
        let weak = Drop { base, grade: Grade::Salvage, from: "X" };
        let strong = Drop { base, grade: Grade::Relic, from: "X" };
        assert!(!improves(&weak, &held, Body::Cybertronian));
        // and a relic of the same mod still loses to something three times
        // its size, because a slot saved is not infinite value
        assert!(!improves(&strong, &held, Body::Cybertronian));
        // against nothing, anything wearable improves
        assert!(improves(&weak, &[], Body::Cybertronian));
        // and it never improves a body that cannot wear it
        assert!(!improves(&weak, &[], Body::HumanBare) || base.fits.accepts(Body::HumanBare));
    }

    #[test]
    fn a_generated_world_has_a_loot_table_the_moment_it_has_an_orbit() {
        // no per-destination authoring: the hazard IS the table
        for (au, star) in [(0.03, StarClass::G), (0.9, StarClass::K), (44.0, StarClass::M)] {
            let w = world(au, 0.25, 0.1, star);
            let d = roll(&w, "UNCHARTED", &MIXED, 7, 1);
            assert!(d.is_some(), "a world at {au} AU paid out nothing");
        }
    }
}
