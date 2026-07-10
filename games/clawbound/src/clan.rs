//! clan.rs — the clans and their **environment-adapted traits**, shared across modes.
//!
//! Straight out of the books: each clan grew up in a different corner of the territory
//! and adapted to it, so they diverge across five attributes —
//!   * **climb**   — scrambling up and holding a tree (the oak-canopy clan excels),
//!   * **sprint**  — running down open ground (the moor clan excels),
//!   * **swim**    — moving through water (the river clan excels),
//!   * **stealth** — lurking, crouching, going unseen (the shadowed-pine clan excels),
//!   * **scent**   — keen noses that smell the predator coming from far off (the fen clan
//!                   excels; the mangrove ocelots hunt fish by scent in the shallows).
//! Traits are multipliers around 1.0; a clan is strongest in its home element and pays
//! for it elsewhere, so which clan you play changes how you use the land.

pub const CLAN_COUNT: usize = 6;

#[derive(Clone, Copy)]
pub struct ClanTrait {
    pub name: &'static str,
    pub home: &'static str,
    pub climb: f32,
    pub sprint: f32,
    pub swim: f32,
    pub stealth: f32,
    pub scent: f32,
    pub color: u32,
}

/// Trait profile for a clan index (0..5; higher indices wrap).
pub fn trait_of(clan: u8) -> ClanTrait {
    match clan % CLAN_COUNT as u8 {
        // Emberclan — the shadowed pines: ambush hunters, lurk and vanish
        0 => ClanTrait { name: "Emberclan", home: "the shadowed pines", climb: 1.0, sprint: 1.0, swim: 0.85, stealth: 1.5, scent: 0.95, color: 0xd8623c },
        // Tideclan — the reed-choked river: born swimmers
        1 => ClanTrait { name: "Tideclan", home: "the reed-choked river", climb: 0.8, sprint: 0.95, swim: 1.6, stealth: 0.95, scent: 1.05, color: 0x3d7fd8 },
        // Mossclan — the oak canopy: climbers who live in the branches
        2 => ClanTrait { name: "Mossclan", home: "the oak canopy", climb: 1.5, sprint: 0.9, swim: 0.7, stealth: 1.0, scent: 0.9, color: 0x5aa054 },
        // Amberclan — the open moor: long-legged runners of the plains
        3 => ClanTrait { name: "Amberclan", home: "the open moor", climb: 0.8, sprint: 1.45, swim: 0.8, stealth: 0.9, scent: 0.85, color: 0xd8a53c },
        // Fenclan — the misted fen: keenest noses, smell danger long before it arrives
        4 => ClanTrait { name: "Fenclan", home: "the misted fen", climb: 0.85, sprint: 0.95, swim: 1.05, stealth: 1.05, scent: 1.7, color: 0x7a9a8c },
        // Mangroveclan — the tangled mangroves: fish-eating ocelots, swim and scent
        _ => ClanTrait { name: "Mangroveclan", home: "the tangled mangroves", climb: 1.1, sprint: 0.85, swim: 1.4, stealth: 1.05, scent: 1.3, color: 0x3f9a78 },
    }
}

pub fn name(clan: u8) -> &'static str { trait_of(clan).name }
pub fn color(clan: u8) -> u32 { trait_of(clan).color }

/// The attribute this clan is best at, e.g. for a one-word tag ("climbers").
pub fn specialty(clan: u8) -> &'static str {
    let t = trait_of(clan);
    let vals = [(t.climb, "climbers"), (t.sprint, "runners"), (t.swim, "swimmers"), (t.stealth, "lurkers"), (t.scent, "trackers")];
    vals.iter().copied().fold((0.0, ""), |best, v| if v.0 > best.0 { v } else { best }).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_clan_is_best_in_its_element() {
        let clans: Vec<u8> = (0..CLAN_COUNT as u8).collect();
        assert!(clans.iter().all(|&c| trait_of(2).climb >= trait_of(c).climb), "Mossclan should climb best");
        assert!(clans.iter().all(|&c| trait_of(3).sprint >= trait_of(c).sprint), "Amberclan should sprint best");
        assert!(clans.iter().all(|&c| trait_of(1).swim >= trait_of(c).swim), "Tideclan should swim best");
        assert!(clans.iter().all(|&c| trait_of(0).stealth >= trait_of(c).stealth), "Emberclan should lurk best");
        assert!(clans.iter().all(|&c| trait_of(4).scent >= trait_of(c).scent), "Fenclan should have the keenest nose");
    }

    #[test]
    fn specialty_matches_the_strongest_trait() {
        assert_eq!(specialty(2), "climbers");
        assert_eq!(specialty(3), "runners");
        assert_eq!(specialty(1), "swimmers");
        assert_eq!(specialty(0), "lurkers");
        assert_eq!(specialty(4), "trackers");            // Fenclan
        assert_eq!(specialty(5), "swimmers");            // Mangroveclan fishes by swimming
    }

    #[test]
    fn traits_are_balanced_tradeoffs() {
        // five traits now; no clan is strong at everything — each pays for its specialty
        for c in 0..CLAN_COUNT as u8 {
            let t = trait_of(c);
            let sum = t.climb + t.sprint + t.swim + t.stealth + t.scent;
            assert!(sum < 6.0, "clan {} is over-tuned (sum {sum})", t.name);
            assert!(sum > 4.4, "clan {} is under-tuned (sum {sum})", t.name);
        }
    }
}
