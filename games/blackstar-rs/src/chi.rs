//! CHI — what a human body can hold of the seven elements.
//!
//! The bots run on energon (`energon.rs`); the WATCH-BEARER does not.
//! A human ice-clone shapeshifter carrying the Watch levels the way a
//! monk does, not the way a machine does: attunement is EARNED per
//! element, in the element, and the body's worthiness to wear serious
//! kit follows from it. Mana by way of chi: seven pools, leveled on a
//! widening curve, spent nowhere — chi is not ammunition, it is what
//! the body has become.
//!
//! The bearer is learning what an Ice Clone already knows without
//! trying: per the treatment (`Clones.md`, and the worldbuilding
//! addendum `TF_CLONE_CIVILIZATIONS_AND_CONVERGENCE.md`), Ice Clones
//! and the elemental Clone races generally do not age the way anything
//! else in this cast does — a native of one of these worlds who has
//! stood watch for a thousand years is not old by their own reckoning.
//! Each people mostly keeps to its own world or outpost; contact
//! between them is occasional, not constant. `arena_element` below —
//! frostline/kuiper/pluto to Ice, emberpit/unicron to Fire, trench to
//! Water, and so on — is that geography: every arena a suited raid
//! visits is somebody's home, not a neutral battlefield, and the chi
//! banked there is the body being taught by the world itself rather
//! than by a teacher who bothered to travel.
//!
//! Where chi comes from (both wired into the sims today):
//! * a NIGHTFALL cleared banks chi in the burn element — the Watch
//!   attunes to whatever ran hot that week;
//! * a raid cleared IN A SUIT banks chi in the arena's element — the
//!   deep sites each have a nature, and standing in it as a human is
//!   the training. Machines earn energon there instead; a body that
//!   walked frostline in a suit earned something else.
//!
//! What chi is FOR:
//! * WORTHINESS — human-fit armor mods (`loadout.rs`) carry an
//!   elemental governor: LEAD-WEAVE SHELL answers to Rock, RADIATOR
//!   VANES to Fire. Heavier mods (more slots) demand higher levels.
//!   `worthy` is the gate the gear menu shows the reason for.
//! * THE WATCH — attunement level shortens the attune delay toward
//!   that element (`attune_mul`), which is the shapeshifter's craft:
//!   the ice clone does not WAIT for ice.
//! * THE SHIFT — at ice level 3 the bearer can hold the ice-clone
//!   form (`can_shift`); the model itself lives in `clones.rs`.
//!
//! Persistence: one lenient `chi` save line, `elem:points` pairs.

use crate::content::Ability;
use crate::loadout::{BodyFit, Mod};
use crate::weapons::{Element, ELEMENTS};

/// Points needed to STAND at each level: a widening staircase. Level 5
/// is mastery and the table is the whole curve — no formula to misread.
pub const LEVELS: [u32; 6] = [0, 3, 8, 15, 25, 40];

/// The seven pools.
#[derive(Clone, Default)]
pub struct Chi {
    points: [u32; 7],
}

fn slot(e: Element) -> Option<usize> {
    ELEMENTS.iter().position(|x| *x == e)
}

impl Chi {
    pub fn from_line(line: &str) -> Chi {
        let mut c = Chi::default();
        for part in line.split(',') {
            let Some((name, pts)) = part.trim().split_once(':') else { continue };
            let Some(e) = ELEMENTS.iter().find(|e| e.name() == name.trim()) else { continue };
            if let Ok(p) = pts.trim().parse() {
                c.points[slot(*e).unwrap()] = p;
            }
        }
        c
    }
    /// Canonical: only non-zero pools, ring order.
    pub fn line(&self) -> String {
        ELEMENTS
            .iter()
            .filter(|e| self.points[slot(**e).unwrap()] > 0)
            .map(|e| format!("{}:{}", e.name(), self.points[slot(*e).unwrap()]))
            .collect::<Vec<_>>()
            .join(",")
    }
    pub fn points(&self, e: Element) -> u32 {
        slot(e).map(|i| self.points[i]).unwrap_or(0)
    }
    pub fn bank(&mut self, e: Element, pts: u32) {
        if let Some(i) = slot(e) {
            self.points[i] += pts;
        }
    }
    /// Level from the staircase: the highest step the points reach.
    pub fn level(&self, e: Element) -> u8 {
        let p = self.points(e);
        LEVELS.iter().rposition(|t| p >= *t).unwrap_or(0) as u8
    }

    /// The shapeshifter's threshold: holding the ice-clone form is an
    /// ice discipline, and it is not an opener.
    pub fn can_shift(&self) -> bool {
        self.level(Element::Ice) >= 3
    }

    /// The Watch attunes faster toward elements the body has earned:
    /// 8% off the delay per level, floored — mastery still takes a
    /// beat, because the beat is the cost that stops a hot-swap meta.
    pub fn attune_mul(&self, toward: Element) -> f32 {
        (1.0 - 0.08 * self.level(toward) as f32).max(0.60)
    }
}

/// Which element GOVERNS a mod, read off what the mod does: heat gear
/// answers to Fire, cold gear to Ice, shielding to Rock (mass is mass),
/// endurance to Vine (the sustaining element), mobility to Air.
pub fn governor(m: &Mod) -> Element {
    use crate::loadout::Effect;
    match m.effect {
        Effect::HeatCeiling => Element::Fire,
        Effect::ColdFloor => Element::Ice,
        Effect::Shielding => Element::Rock,
        Effect::Endurance => Element::Vine,
        Effect::Mobility => Element::Air,
    }
}

/// The level a mod demands: its slot weight, straight across. A 1-slot
/// genetic tweak asks level 1; the 3-slot LEAD-WEAVE SHELL asks Rock 3.
/// Machine-fit mods ask nothing of chi — plating is bolted, not earned.
pub fn required(m: &Mod) -> u8 {
    if m.fits == BodyFit::Machine {
        0
    } else {
        m.slots
    }
}

/// The worthiness gate, with the reason on the outside: Err carries the
/// line the gear menu prints.
pub fn worthy(chi: &Chi, m: &Mod) -> Result<(), String> {
    let need = required(m);
    if need == 0 {
        return Ok(());
    }
    let gov = governor(m);
    let have = chi.level(gov);
    if have >= need {
        Ok(())
    } else {
        Err(format!("unworthy: {} asks {} level {}, the body holds {}", m.name, gov.name(), need, have))
    }
}

/// What a deep site trains, for the suited-raid chi bank. The mapping
/// follows each arena's weather and premise in `raidcon.rs`; patrol
/// sites train the generalist's element.
pub fn arena_element(arena: &str) -> Element {
    match arena {
        "frostline" | "kuiper" | "pluto" => Element::Ice,
        "emberpit" | "unicron" => Element::Fire,
        "trench" => Element::Water,
        "shield_edge" => Element::Electric,
        "outer_rim" => Element::Air,
        "cybertron" => Element::Rock,
        _ => Element::Water,
    }
}

/// What one clear banks.
pub const PER_NIGHTFALL: u32 = 2;
pub const PER_SUITED_RAID: u32 = 3;

/// One line for a status screen: leveled pools only.
pub fn sheet(chi: &Chi) -> String {
    let mut parts: Vec<String> = ELEMENTS
        .iter()
        .filter(|e| chi.points(**e) > 0)
        .map(|e| format!("{} L{} ({})", e.name(), chi.level(*e), chi.points(*e)))
        .collect();
    if parts.is_empty() {
        parts.push("nothing yet -- the body is a stranger to all seven".into());
    }
    parts.join(", ")
}

/// The abilities module already names actives; expose the chi read the
/// shell will want next to each: does the bearer's discipline discount
/// this ability's element? (Abilities have no element today, so this is
/// the seam, stated: it returns the watch discount for the element the
/// CALLER says the ability channels.)
pub fn ability_mul(chi: &Chi, _a: &Ability, channels: Element) -> f32 {
    chi.attune_mul(channels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loadout::MODS;

    #[test]
    fn the_line_roundtrips_and_tolerates_junk() {
        let mut c = Chi::from_line("ice:9, fire:2, plasma:9, rock:abc,,vine:1");
        assert_eq!(c.points(Element::Ice), 9);
        assert_eq!(c.points(Element::Fire), 2);
        assert_eq!(c.points(Element::Vine), 1);
        assert_eq!(c.points(Element::Rock), 0, "junk points banked");
        c.bank(Element::Ice, 1);
        let back = Chi::from_line(&c.line());
        assert_eq!(back.points(Element::Ice), 10);
        assert_eq!(back.line(), c.line());
    }

    #[test]
    fn levels_climb_the_staircase() {
        let mut c = Chi::default();
        assert_eq!(c.level(Element::Ice), 0);
        c.bank(Element::Ice, 3);
        assert_eq!(c.level(Element::Ice), 1);
        c.bank(Element::Ice, 37); // 40 total
        assert_eq!(c.level(Element::Ice), 5);
        assert!(c.can_shift());
        assert!(!Chi::default().can_shift());
        // the watch discount follows, floored
        assert!((c.attune_mul(Element::Ice) - 0.60).abs() < 1e-6);
        assert!((c.attune_mul(Element::Fire) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn worthiness_gates_the_heavy_human_kit_and_ignores_plating() {
        let chi = Chi::from_line("rock:8"); // level 2
        let shell = MODS.iter().find(|m| m.name == "LEAD-WEAVE SHELL").unwrap();
        assert!(worthy(&chi, shell).is_err(), "a level-2 body wore the 3-slot shell");
        let earned = Chi::from_line("rock:15"); // level 3
        assert!(worthy(&earned, shell).is_ok());
        // machine plating never asks
        let plating = MODS.iter().find(|m| m.name == "ABLATIVE PLATING").unwrap();
        assert!(worthy(&Chi::default(), plating).is_ok());
        // every human mod has a governor and a nonzero ask
        for m in MODS.iter().filter(|m| m.fits != BodyFit::Machine) {
            assert!(required(m) >= 1, "{} asks nothing of the body", m.name);
        }
    }

    #[test]
    fn every_arena_trains_something() {
        for a in crate::raidcon::ARENAS {
            let _ = arena_element(a.id); // total over the roster's arenas
        }
        assert_eq!(arena_element("frostline"), Element::Ice);
        assert_eq!(arena_element("unicron"), Element::Fire);
    }
}
