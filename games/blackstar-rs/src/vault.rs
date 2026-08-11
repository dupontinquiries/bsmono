//! VAULT — the chest on the ship.
//!
//! Minecraft's answer to a small inventory: a box you walk back to. One
//! per player's ship (one per save — the relay's multiplayer carries a
//! profile name, and a vault follows the profile), 27 slots in the
//! chest's own 3x9 grid, one item per slot because gear here is
//! discrete kit, not stackable cobblestone — the grid and the walk are
//! the Minecraft of it, not the stacking.
//!
//! The vault holds `gear::Item`s and trades them with the bag:
//! `deposit` and `withdraw` move one thing each way, `sort` packs the
//! grid weapons-first then armor, alphabetical inside each — the
//! shift-click tidy without the shift-click. Persistence is one `vault`
//! save line of item codes with `-` holding empty slots, so a sorted
//! chest STAYS where you sorted it.

use crate::gear::{Gear, Item};

pub const ROWS: usize = 3;
pub const COLS: usize = 9;
pub const SLOTS: usize = ROWS * COLS;

#[derive(Clone)]
pub struct Vault {
    slots: Vec<Option<Item>>,
}

impl Default for Vault {
    fn default() -> Vault {
        Vault { slots: vec![None; SLOTS] }
    }
}

impl Vault {
    pub fn from_line(line: &str) -> Vault {
        let mut v = Vault::default();
        for (i, code) in line.split(',').take(SLOTS).enumerate() {
            v.slots[i] = Item::parse(code);
        }
        v
    }
    /// `-` per empty slot; trailing emptiness trimmed so an untouched
    /// vault writes nothing at all.
    pub fn line(&self) -> String {
        let last = self.slots.iter().rposition(|s| s.is_some());
        match last {
            None => String::new(),
            Some(n) => self.slots[..=n]
                .iter()
                .map(|s| s.as_ref().map(|i| i.code()).unwrap_or_else(|| "-".into()))
                .collect::<Vec<_>>()
                .join(","),
        }
    }

    pub fn count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Look at a slot without moving anything — the hold's console reads
    /// the chest this way.
    pub fn peek(&self, i: usize) -> Option<&Item> {
        self.slots.get(i).and_then(|s| s.as_ref())
    }

    /// Bag item -> first empty slot.
    pub fn deposit(&mut self, g: &mut Gear, bag_i: usize) -> Result<String, String> {
        if bag_i >= g.bag.len() {
            return Err(format!("no bag item {bag_i}"));
        }
        let Some(free) = self.slots.iter().position(|s| s.is_none()) else {
            return Err(format!("the vault is full ({SLOTS})"));
        };
        let it = g.bag.remove(bag_i);
        let label = it.label();
        self.slots[free] = Some(it);
        Ok(format!("{label} -> vault slot {free}"))
    }

    /// Vault slot -> bag, if the bag has room.
    pub fn withdraw(&mut self, g: &mut Gear, slot: usize) -> Result<String, String> {
        if slot >= SLOTS {
            return Err(format!("slots run 0..{}", SLOTS - 1));
        }
        let Some(it) = self.slots[slot].take() else {
            return Err(format!("slot {slot} is empty"));
        };
        match g.take(it.clone()) {
            Ok(()) => Ok(format!("{} -> bag", it.label())),
            Err(e) => {
                self.slots[slot] = Some(it); // put it back; nothing is lost
                Err(e)
            }
        }
    }

    /// Weapons first, then armor, alphabetical by label inside each —
    /// packed to the top-left like a tidied chest.
    pub fn sort(&mut self) {
        let mut items: Vec<Item> = self.slots.iter_mut().filter_map(|s| s.take()).collect();
        items.sort_by_key(|i| (matches!(i, Item::Armor(_)), i.label()));
        for (i, it) in items.into_iter().enumerate() {
            self.slots[i] = Some(it);
        }
    }

    /// The grid, drawn: three rows of nine cells, numbered, `..` empty.
    pub fn grid(&self) -> String {
        let mut out = String::from("THE SHIP'S VAULT\n");
        for r in 0..ROWS {
            let mut cells = String::new();
            let mut names = String::new();
            for c in 0..COLS {
                let i = r * COLS + c;
                match &self.slots[i] {
                    Some(_) => cells.push_str(&format!("[{i:2}]")),
                    None => cells.push_str("[..]"),
                }
            }
            for c in 0..COLS {
                let i = r * COLS + c;
                if let Some(it) = &self.slots[i] {
                    names.push_str(&format!("  {i:2} {}\n", it.label()));
                }
            }
            out.push_str(&cells);
            out.push('\n');
            out.push_str(&names);
        }
        out.push_str(&format!("{} of {SLOTS} slots held\n", self.count()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weapons::{Element, Verb};

    fn w(v: Verb, e: Element) -> Item {
        Item::Weapon { verb: v, element: e, from: None }
    }

    #[test]
    fn deposit_withdraw_and_nothing_is_ever_lost() {
        let mut vault = Vault::default();
        let mut g = Gear::default();
        g.take(w(Verb::Beam, Element::Ice)).unwrap();
        vault.deposit(&mut g, 0).unwrap();
        assert_eq!(g.bag.len(), 0);
        assert_eq!(vault.count(), 1);
        // fill the bag, then a withdraw must refuse AND keep the item
        for _ in 0..crate::gear::BAG_CAP {
            g.take(w(Verb::Push, Element::Water)).unwrap();
        }
        assert!(vault.withdraw(&mut g, 0).is_err());
        assert_eq!(vault.count(), 1, "a refused withdraw ate the item");
        g.bag.pop();
        vault.withdraw(&mut g, 0).unwrap();
        assert_eq!(vault.count(), 0);
    }

    #[test]
    fn the_line_keeps_slot_positions() {
        let mut vault = Vault::default();
        let mut g = Gear::default();
        g.take(w(Verb::Flak, Element::Fire)).unwrap();
        g.take(w(Verb::Beam, Element::Ice)).unwrap();
        vault.deposit(&mut g, 0).unwrap();
        vault.deposit(&mut g, 0).unwrap();
        // hollow out slot 0: slot 1 must STAY slot 1 across a save
        g.bag.clear();
        vault.withdraw(&mut g, 0).unwrap();
        let back = Vault::from_line(&vault.line());
        assert!(back.slots[0].is_none() && back.slots[1].is_some());
        assert_eq!(back.line(), vault.line());
        assert_eq!(Vault::default().line(), "", "an untouched vault should write nothing");
    }

    #[test]
    fn sort_packs_weapons_then_armor() {
        use crate::loadout::MODS;
        let mut vault = Vault::default();
        let mut g = Gear::default();
        g.take(Item::Armor(&MODS[5])).unwrap();
        g.take(w(Verb::Vortex, Element::Water)).unwrap();
        for _ in 0..2 {
            vault.deposit(&mut g, 0).unwrap();
        }
        // scatter: move the weapon deep
        vault.slots[20] = vault.slots[1].take();
        vault.sort();
        assert!(matches!(vault.slots[0], Some(Item::Weapon { .. })), "weapons lead the chest");
        assert!(matches!(vault.slots[1], Some(Item::Armor(_))));
        assert_eq!(vault.count(), 2);
    }

    #[test]
    fn the_grid_never_overflows() {
        let mut vault = Vault::default();
        let mut g = Gear::default();
        for i in 0..SLOTS {
            g.take(w(Verb::Push, Element::Water)).unwrap();
            vault.deposit(&mut g, 0).expect(&format!("slot {i}"));
        }
        g.take(w(Verb::Push, Element::Water)).unwrap();
        assert!(vault.deposit(&mut g, 0).is_err(), "a 28th item went into a 27-slot chest");
        assert_eq!(g.bag.len(), 1, "the refused deposit ate the item");
    }
}
