//! Loot — what a slain creature yields, tied back into the item system. Drops
//! are conditioned on the body: a carcass always gives its base materials, and
//! each part still *intact* at death contributes its own material. Parts you
//! destroyed in the fight (via `mob::strike`) are ruined and drop nothing — so
//! a dragon downed by shearing both wings yields no wing-membrane, while one
//! killed with a clean bite to the jaw still drops its full wingspan.
//!
//! This links three systems: `spawn` builds the anatomy, `mob` damages it, and
//! `loot` reads the survivors into `ItemKind` stacks the inventory can hold.
//! Pure; spawning the dropped items as pickups in the world is engine wiring.

use crate::creature::{Anatomy, Part};
use crate::item::{CreatureKind, ItemKind};
use crate::world::blocks::BlockId;

/// One stack a creature drops on death.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemDrop {
    pub item: ItemKind,
    pub count: u16,
}

fn block(b: BlockId) -> ItemKind {
    ItemKind::Block(b)
}

/// Coalescing accumulator — merges repeated items into one stack, drops zeros.
#[derive(Default)]
struct Loot(Vec<ItemDrop>);
impl Loot {
    fn add(&mut self, item: ItemKind, count: u16) {
        if count == 0 {
            return;
        }
        if let Some(d) = self.0.iter_mut().find(|d| d.item == item) {
            d.count = d.count.saturating_add(count);
        } else {
            self.0.push(ItemDrop { item, count });
        }
    }
}

/// How many parts of a kind are still intact (undamaged-to-zero).
fn surviving(anatomy: &Anatomy, kind: Part) -> u16 {
    anatomy.parts.iter().filter(|p| p.part == kind && !p.is_destroyed()).count() as u16
}

/// Everything a creature of `kind` drops, given the state of its body at death
/// (`None` = unknown body, base materials only).
pub fn drops(kind: CreatureKind, anatomy: Option<&Anatomy>) -> Vec<ItemDrop> {
    let mut loot = Loot::default();
    match kind {
        CreatureKind::Dragon => {
            // The woody carcass + skeleton, always.
            loot.add(block(BlockId::OakLog), 3);
            loot.add(block(BlockId::Bone), 2);
            if let Some(a) = anatomy {
                loot.add(block(BlockId::Vine), surviving(a, Part::Wing)); // membrane
                loot.add(block(BlockId::Amber), surviving(a, Part::Eye)); // golden eyes
                loot.add(block(BlockId::Bone), surviving(a, Part::Jaw));  // fang
                loot.add(block(BlockId::OakLog), surviving(a, Part::Tail));
            }
        }
        CreatureKind::Lauscher => {
            loot.add(block(BlockId::Bone), 1);
            loot.add(block(BlockId::Lichen), 1);
            if let Some(a) = anatomy {
                loot.add(block(BlockId::Crystal), surviving(a, Part::Eye)); // keen eyes
            }
        }
        CreatureKind::Spaeher => {
            loot.add(block(BlockId::Bone), 1);
            if let Some(a) = anatomy {
                loot.add(block(BlockId::Vine), surviving(a, Part::Wing)); // wing membrane
                loot.add(block(BlockId::Amber), surviving(a, Part::Eye)); // bright eyes
            }
        }
        CreatureKind::Fauna => {
            // A wild animal's carcass: bone, plus a fang from an intact jaw.
            loot.add(block(BlockId::Bone), 1);
            if let Some(a) = anatomy {
                loot.add(block(BlockId::Bone), surviving(a, Part::Jaw));
            }
        }
        CreatureKind::Dinosaur => {
            // A great skeleton — a lot of bone, big fangs, and amber eyes.
            loot.add(block(BlockId::Bone), 4);
            if let Some(a) = anatomy {
                loot.add(block(BlockId::Bone), surviving(a, Part::Jaw));
                loot.add(block(BlockId::Amber), surviving(a, Part::Eye));
            }
        }
    }
    loot.0
}

/// Base drops with no body information — the carcass materials only.
pub fn base_drops(kind: CreatureKind) -> Vec<ItemDrop> {
    drops(kind, None)
}

// ── r330-swords ────────────────────────────────────────────────────────────
/// Like [`drops`], but a `roll_seed` + drop context may append a single
/// **legendary weapon** drop on a rare roll (via `sim::legendary_loot`).
///
/// Kept separate from [`drops`]/[`base_drops`] so their signatures and every
/// existing caller stay untouched — this is the additive hook the bin loot path
/// calls when it wants the legendary roll. Pure and deterministic: the only
/// entropy is the passed-in `roll_seed`. A legendary stacks to 1 (count fixed),
/// and is appended after the carcass materials so existing drop order is stable.
pub fn drops_with_legendary(
    kind: CreatureKind,
    anatomy: Option<&crate::creature::Anatomy>,
    roll_seed: u64,
    ctx: crate::sim::legendary_loot::DropContext,
) -> Vec<ItemDrop> {
    let mut out = drops(kind, anatomy);
    if let Some(spec) = crate::sim::legendary_loot::roll(roll_seed, ctx) {
        out.push(ItemDrop { item: ItemKind::Legendary(spec), count: 1 });
    }
    out
}
// ── end r330-swords ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spawn::spawn;
    use crate::mob::strike;

    fn count_of(drops: &[ItemDrop], b: BlockId) -> u16 {
        drops.iter().find(|d| d.item == ItemKind::Block(b)).map_or(0, |d| d.count)
    }

    #[test]
    fn an_intact_dragon_drops_both_wings_membrane_and_both_eyes_amber() {
        let d = spawn(CreatureKind::Dragon, [0.0; 3]);
        let l = drops(CreatureKind::Dragon, d.anatomy.as_ref());
        assert_eq!(count_of(&l, BlockId::Vine), 2, "two intact wings -> two membrane");
        assert_eq!(count_of(&l, BlockId::Amber), 2, "two intact eyes -> two amber");
        assert!(count_of(&l, BlockId::OakLog) >= 3);
    }

    #[test]
    fn shearing_both_wings_yields_no_membrane() {
        let mut d = spawn(CreatureKind::Dragon, [0.0; 3]);
        strike(&mut d, [0.0, 0.8, 2.5], 100);   // wing 1
        strike(&mut d, [0.0, 0.8, -2.5], 100);  // wing 2
        let l = drops(CreatureKind::Dragon, d.anatomy.as_ref());
        assert_eq!(count_of(&l, BlockId::Vine), 0, "ruined wings drop nothing");
        // Eyes untouched, so amber still drops.
        assert_eq!(count_of(&l, BlockId::Amber), 2);
    }

    #[test]
    fn one_destroyed_eye_halves_the_amber() {
        let mut d = spawn(CreatureKind::Dragon, [0.0; 3]);
        strike(&mut d, [3.0, 0.6, 0.6], 100); // eye 1
        let l = drops(CreatureKind::Dragon, d.anatomy.as_ref());
        assert_eq!(count_of(&l, BlockId::Amber), 1);
    }

    #[test]
    fn base_drops_are_carcass_only() {
        let l = base_drops(CreatureKind::Dragon);
        assert_eq!(count_of(&l, BlockId::Vine), 0, "no body -> no part bonuses");
        assert!(count_of(&l, BlockId::OakLog) >= 3 && count_of(&l, BlockId::Bone) >= 2);
    }

    #[test]
    fn lauscher_loot_differs_from_the_dragon() {
        let lau = base_drops(CreatureKind::Lauscher);
        assert!(count_of(&lau, BlockId::OakLog) == 0, "lauscher isn't woody");
        assert!(count_of(&lau, BlockId::Bone) >= 1 && count_of(&lau, BlockId::Lichen) >= 1);
    }

    #[test]
    fn drops_never_carry_a_zero_stack_or_duplicate_item() {
        let d = spawn(CreatureKind::Dragon, [0.0; 3]);
        let l = drops(CreatureKind::Dragon, d.anatomy.as_ref());
        assert!(l.iter().all(|d| d.count > 0));
        for i in 0..l.len() {
            for j in (i + 1)..l.len() {
                assert_ne!(l[i].item, l[j].item, "items are coalesced, no dupes");
            }
        }
    }

    // ── r330-swords ─────────────────────────────────────────────────────────
    use crate::sim::legendary_loot::DropContext;

    #[test]
    fn legendary_hook_appends_exactly_one_legendary_on_a_full_roll() {
        let base = base_drops(CreatureKind::Dragon);
        // 100% chance, no rarity gate → exactly one legendary appended.
        let with = drops_with_legendary(
            CreatureKind::Dragon,
            None,
            0xABCD,
            DropContext::ungated(1.0, 0.0, 0.0),
        );
        let legendaries = with.iter().filter(|d| matches!(d.item, ItemKind::Legendary(_))).count();
        assert_eq!(legendaries, 1, "one legendary on a guaranteed roll");
        assert_eq!(with.len(), base.len() + 1, "carcass drops preserved, one item added");
        // The carcass prefix is unchanged (legendary is appended, not interleaved).
        assert_eq!(&with[..base.len()], &base[..]);
    }

    #[test]
    fn legendary_hook_adds_nothing_when_chance_is_zero() {
        let base = base_drops(CreatureKind::Lauscher);
        let with = drops_with_legendary(
            CreatureKind::Lauscher,
            None,
            0xABCD,
            DropContext::ungated(0.0, 0.0, 0.0),
        );
        assert_eq!(with, base, "0% chance leaves the drop list identical");
    }

    #[test]
    fn legendary_hook_is_deterministic() {
        let ctx = DropContext::ungated(0.5, 1.0, 0.0);
        let a = drops_with_legendary(CreatureKind::Dinosaur, None, 99, ctx);
        let b = drops_with_legendary(CreatureKind::Dinosaur, None, 99, ctx);
        assert_eq!(a, b);
    }
}
