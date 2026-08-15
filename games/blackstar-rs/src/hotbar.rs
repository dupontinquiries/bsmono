//! THE ACTION BAR — a Minecraft-style hotbar for the bearer's kit.
//!
//! `gear.rs` already has three fixed rails (kinetic/energy/power, one
//! item each) — that models what the Watch Bearer is ARMED with. This
//! is a different question: what is quick to reach. Nine numbered
//! slots, any item in any slot, in any order, switched with a number
//! key or a scroll — the exact shape Minecraft's own hotbar has, and
//! for the same reason: quick access should be something the player
//! arranges, not something the game assigns.
//!
//! "Fully customizable" means two things here, not one:
//! * slot CONTENTS and ORDER are entirely the player's — assign, swap,
//!   clear, any slot, any item, no fixed categories;
//! * each slot also carries its own SIDE and AMMO TYPE
//!   (`gear::Side`, `gear::AmmoType`), defaulted from the weapon's own
//!   `WeaponSpec` but overridable per slot. The same rifle can sit in
//!   two different slots configured for opposite hands, or declared to
//!   draw a different rail than its own catalog entry says — a real
//!   override, not just a label, because a slot's ammo type is what a
//!   caller (a HUD, a firing check) actually reads.
//!
//! The active slot is the only one "equipped" — selecting a slot with
//! no item in it is a no-op, same as Minecraft leaving an empty hotbar
//! cell selected rather than refusing the click.

use crate::gear::{self, AmmoType, Item, Side};

pub const SLOTS: usize = 9;

#[derive(Clone, Debug)]
pub struct HotbarSlot {
    pub item: Item,
    pub side: Side,
    pub ammo: AmmoType,
}

pub struct Hotbar {
    slots: [Option<HotbarSlot>; SLOTS],
    active: usize,
}

impl Default for Hotbar {
    fn default() -> Hotbar {
        Hotbar { slots: Default::default(), active: 0 }
    }
}

impl Hotbar {
    /// Assign an item to a slot, with side/ammo defaulted from the
    /// weapon catalog when the item matches a known `WeaponSpec`, or
    /// from `gear::rail_of` otherwise (armor and unlisted weapons still
    /// get a sensible ammo type rather than an empty one).
    pub fn assign(&mut self, i: usize, item: Item) -> Result<(), String> {
        if i >= SLOTS {
            return Err(format!("slots run 0..{}", SLOTS - 1));
        }
        let (side, ammo) = defaults_for(&item);
        self.slots[i] = Some(HotbarSlot { item, side, ammo });
        Ok(())
    }

    pub fn clear(&mut self, i: usize) -> Option<Item> {
        self.slots.get_mut(i).and_then(|s| s.take()).map(|s| s.item)
    }

    /// Swap two slots wholesale — item AND its side/ammo overrides move
    /// together, since a customization belongs to the loadout the
    /// player built, not to the slot number.
    pub fn swap(&mut self, a: usize, b: usize) -> Result<(), String> {
        if a >= SLOTS || b >= SLOTS {
            return Err(format!("slots run 0..{}", SLOTS - 1));
        }
        self.slots.swap(a, b);
        Ok(())
    }

    /// Override a slot's side or ammo without touching what is in it —
    /// the "fully" in fully customizable. Refuses on an empty slot:
    /// there is nothing here to customize yet.
    pub fn set_side(&mut self, i: usize, side: Side) -> Result<(), String> {
        self.slots
            .get_mut(i)
            .and_then(|s| s.as_mut())
            .map(|s| s.side = side)
            .ok_or_else(|| format!("slot {i} is empty"))
    }
    pub fn set_ammo(&mut self, i: usize, ammo: AmmoType) -> Result<(), String> {
        self.slots
            .get_mut(i)
            .and_then(|s| s.as_mut())
            .map(|s| s.ammo = ammo)
            .ok_or_else(|| format!("slot {i} is empty"))
    }

    /// Select a slot outright. An empty slot is a legal selection (the
    /// player's hands are simply empty), matching Minecraft's own rule.
    pub fn select(&mut self, i: usize) -> Result<(), String> {
        if i >= SLOTS {
            return Err(format!("slots run 0..{}", SLOTS - 1));
        }
        self.active = i;
        Ok(())
    }

    /// Scroll-wheel style cycling. Wraps, and — unlike `select` — skips
    /// empty slots when at least one slot is occupied, so a scroll
    /// always lands on something to fight with.
    pub fn cycle(&mut self, dir: i32) {
        if self.slots.iter().all(|s| s.is_none()) {
            return;
        }
        let step = if dir >= 0 { 1i32 } else { -1 };
        let mut i = self.active as i32;
        for _ in 0..SLOTS {
            i = (i + step).rem_euclid(SLOTS as i32);
            if self.slots[i as usize].is_some() {
                self.active = i as usize;
                return;
            }
        }
    }

    pub fn active_index(&self) -> usize {
        self.active
    }
    pub fn active(&self) -> Option<&HotbarSlot> {
        self.slots[self.active].as_ref()
    }
    pub fn slot(&self, i: usize) -> Option<&HotbarSlot> {
        self.slots.get(i).and_then(|s| s.as_ref())
    }
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.is_none())
    }

    /// `slot0code;side;ammo,slot1code;side;ammo,...` — `-` for an empty
    /// slot, active index appended last. A semicolon separates the
    /// hotbar's own fields from the item code deliberately: `Item::
    /// code()` already uses colons internally (`w:verb:element[:from]`,
    /// a variable field count when a warden provenance tag is present),
    /// so appending `:side:ammo` onto it would collide with `Item::
    /// parse`'s own splitting — caught by the roundtrip test below
    /// before it ever reached a save file. Lenient otherwise: junk
    /// fields fall back to the item's own defaults rather than
    /// corrupting the load.
    pub fn line(&self) -> String {
        let cells: Vec<String> = self
            .slots
            .iter()
            .map(|s| match s {
                Some(hs) => format!("{};{};{}", hs.item.code(), hs.side.name(), hs.ammo.name()),
                None => "-".into(),
            })
            .collect();
        format!("{}|{}", cells.join(","), self.active)
    }

    pub fn from_line(line: &str) -> Hotbar {
        let mut hb = Hotbar::default();
        let (cells_part, active_part) = line.split_once('|').unwrap_or((line, "0"));
        for (i, cell) in cells_part.split(',').take(SLOTS).enumerate() {
            if cell == "-" || cell.is_empty() {
                continue;
            }
            let mut parts = cell.splitn(3, ';');
            let Some(item) = parts.next().and_then(Item::parse) else { continue };
            let (default_side, default_ammo) = defaults_for(&item);
            let side = parts.next().and_then(Side::parse).unwrap_or(default_side);
            let ammo = parts.next().and_then(AmmoType::parse).unwrap_or(default_ammo);
            hb.slots[i] = Some(HotbarSlot { item, side, ammo });
        }
        hb.active = active_part.trim().parse().unwrap_or(0);
        if hb.active >= SLOTS {
            hb.active = 0;
        }
        hb
    }
}

/// A weapon's catalog defaults when it matches a known `WeaponSpec`
/// (matched by verb+element, since `Item` carries no name of its own);
/// otherwise fall back to `rail_of`'s ammo type and an unopinionated
/// side. Armor has no rail at all, so it defaults to `Melee`/`Either`
/// — inert values a caller can simply ignore for a worn item.
fn defaults_for(item: &Item) -> (Side, AmmoType) {
    if let Item::Weapon { verb, element, .. } = item {
        if let Some(spec) = gear::WEAPONS.iter().find(|w| w.verb == *verb && w.element == *element) {
            return (spec.side, spec.ammo);
        }
    }
    match item {
        Item::Weapon { verb, .. } => {
            let ammo = match gear::rail_of(*verb) {
                gear::Rail::Kinetic => AmmoType::Kinetic,
                gear::Rail::Energy => AmmoType::Energy,
                gear::Rail::Power => AmmoType::Power,
            };
            (Side::Either, ammo)
        }
        Item::Armor(_) => (Side::Either, AmmoType::Melee),
    }
}

/// What arming a hotbar slot actually means. `weapons::Form` is the
/// WEAPON (a name and a verb); `weapons::Element` is a separate,
/// orthogonal attunement the Watch holds and applies at fire time. An
/// `Item::Weapon` carries both, so arming a slot is two distinct
/// requests, not one lookup:
/// * the form, matched by VERB alone (TABLE has no element field — an
///   earlier version of this function assumed it did and failed to
///   compile, which is the correct outcome for a wrong mental model);
/// * the element, which must go through `Watch::switch` rather than
///   being written directly, because switching attunement costs a real
///   delay (`ATTUNE_SEC`) and is refused mid-attune. Arming a slot
///   should not be a way to dodge that cost.
///
/// Returns the form index if the verb has a TABLE entry. An empty slot
/// or an armor item returns None, and the caller leaves the current
/// weapon alone rather than disarming the player mid-fight.
pub fn form_for(item: &Item) -> Option<usize> {
    let Item::Weapon { verb, .. } = item else { return None };
    crate::weapons::TABLE.iter().position(|f| f.verb == *verb)
}

/// The element a slot wants the Watch attuned to, if it names one.
pub fn element_for(item: &Item) -> Option<crate::weapons::Element> {
    match item {
        Item::Weapon { element, .. } if *element != crate::weapons::Element::None => Some(*element),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weapons::{Element, Verb};

    #[test]
    fn assignment_is_free_form_and_defaults_come_from_the_armory() {
        let mut hb = Hotbar::default();
        let ashpulse = gear::weapon_item(gear::weapon_named("ASHPULSE").unwrap());
        hb.assign(4, ashpulse).unwrap();
        let slot = hb.slot(4).unwrap();
        assert_eq!(slot.ammo, AmmoType::Energy, "the plasma blaster did not default to energy");
        assert_eq!(slot.side, Side::Right);
        assert!(hb.assign(SLOTS, gear::weapon_item(gear::weapon_named("FIRSTEDGE").unwrap())).is_err());
    }

    #[test]
    fn a_slot_can_be_customized_without_touching_what_is_in_it() {
        let mut hb = Hotbar::default();
        let rifle = gear::weapon_item(gear::weapon_named("LONGCOIL").unwrap());
        hb.assign(0, rifle).unwrap();
        assert_eq!(hb.slot(0).unwrap().side, Side::Right);
        hb.set_side(0, Side::Left).unwrap();
        assert_eq!(hb.slot(0).unwrap().side, Side::Left, "the override did not take");
        assert!(
            matches!(hb.slot(0).unwrap().item, Item::Weapon { verb: Verb::Chain, .. }),
            "customizing moved the item"
        );
        hb.set_ammo(0, AmmoType::Melee).unwrap();
        assert_eq!(hb.slot(0).unwrap().ammo, AmmoType::Melee);
        // an empty slot has nothing to customize
        assert!(hb.set_side(5, Side::Left).is_err());
    }

    #[test]
    fn swap_carries_the_customization_with_the_item() {
        let mut hb = Hotbar::default();
        hb.assign(0, gear::weapon_item(gear::weapon_named("SNAPBURN").unwrap())).unwrap();
        hb.set_side(0, Side::Left).unwrap();
        hb.swap(0, 1).unwrap();
        assert!(hb.slot(0).is_none());
        let moved = hb.slot(1).unwrap();
        assert_eq!(moved.side, Side::Left, "the customization did not travel with the swap");
        assert!(matches!(moved.item, Item::Weapon { verb: Verb::Pierce, .. }));
    }

    #[test]
    fn select_allows_empty_but_cycle_always_lands_on_something() {
        let mut hb = Hotbar::default();
        hb.select(3).unwrap(); // legal even though slot 3 is empty
        assert_eq!(hb.active_index(), 3);
        assert!(hb.active().is_none());
        hb.assign(6, gear::weapon_item(gear::weapon_named("GREYFALL").unwrap())).unwrap();
        hb.cycle(1);
        assert_eq!(hb.active_index(), 6, "cycling skipped past the only occupied slot");
        // wraps correctly both directions
        hb.cycle(1);
        assert_eq!(hb.active_index(), 6, "with only one item, cycling should land back on it");
        hb.cycle(-1);
        assert_eq!(hb.active_index(), 6);
    }

    #[test]
    fn cycling_an_entirely_empty_bar_does_nothing() {
        let mut hb = Hotbar::default();
        hb.select(2).unwrap();
        hb.cycle(1);
        assert_eq!(hb.active_index(), 2, "cycling moved with nothing to land on");
    }

    #[test]
    fn arming_a_slot_resolves_a_form_by_verb_and_an_element_separately() {
        // the distinction this locks in: TABLE forms carry a verb, NOT an
        // element -- element is the Watch's orthogonal attunement. Every
        // armory weapon must resolve to a real form.
        for w in &gear::WEAPONS {
            let item = gear::weapon_item(w);
            let form = form_for(&item).unwrap_or_else(|| panic!("{} has no TABLE form for its verb", w.name));
            assert_eq!(crate::weapons::TABLE[form].verb, w.verb);
            // and the element comes back separately, when it names one
            match w.element {
                Element::None => assert_eq!(element_for(&item), None, "{} named an element it does not have", w.name),
                e => assert_eq!(element_for(&item), Some(e), "{}", w.name),
            }
        }
        // armor arms nothing at all
        let armor = Item::Armor(crate::loadout::MODS.iter().find(|m| m.name == "AEROGEL LINER").unwrap());
        assert_eq!(form_for(&armor), None);
        assert_eq!(element_for(&armor), None);
    }

    #[test]
    fn the_line_roundtrips_contents_customization_and_the_active_slot() {
        let mut hb = Hotbar::default();
        hb.assign(0, gear::weapon_item(gear::weapon_named("RAILFANG").unwrap())).unwrap();
        hb.set_side(0, Side::Right).unwrap(); // override away from the catalog default (Left)
        hb.assign(8, Item::Weapon { verb: Verb::Beam, element: Element::Water, from: Some("hussar".into()) })
            .unwrap();
        hb.select(8).unwrap();
        let line = hb.line();
        let back = Hotbar::from_line(&line);
        assert_eq!(back.active_index(), 8);
        assert_eq!(back.slot(0).unwrap().side, Side::Right, "the override did not survive a save round-trip");
        assert!(matches!(&back.slot(8).unwrap().item, Item::Weapon { from: Some(f), .. } if f == "hussar"));
        assert_eq!(back.line(), line);
        // junk survives without panicking and falls back to sane defaults
        let junk = Hotbar::from_line("not:a:real:line,,,,,,,,|99");
        assert_eq!(junk.active_index(), 0, "an out-of-range active index was not caught");
    }
}
