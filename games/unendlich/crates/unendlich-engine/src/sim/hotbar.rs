//! `sim::hotbar` — the player's quick-select bar and the bridge that moves items
//! between the inventory and the hotbar. It is a thin, pure layer over the
//! existing [`SlotGrid`]/[`ItemStack`] storage plus a held-item **ability**
//! descriptor so the HUD can show what the selected item does (place a block,
//! mine value, drop a decoration, …). The bin drives `select`/`scroll` from
//! input, renders `slots()` + `active_index()`, and reads `active_ability()` for
//! the on-screen "what does this do" readout. Pure: no IO/render/RNG.

use crate::inventory::{ItemStack, SlotGrid};
use crate::world::blocks::BlockId;

/// Number of quick-select slots (the classic 1–9 row).
pub const HOTBAR_SLOTS: usize = 9;

/// What the currently-held item lets the player do — surfaced in the HUD so the
/// operator can *see* an item's purpose without a separate tooltip system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldAbility {
    /// No item in the active slot.
    Empty,
    /// A solid building block — placeable into the world.
    PlaceBlock,
    /// A light-emitting / glowing block — placeable, lights its surroundings.
    PlaceLight,
    /// A plant or decoration — placeable as scenery.
    PlaceDecoration,
    /// An ore — its value is realised by mining/smelting, not placing.
    Mineable,
    /// A liquid — flows when placed.
    PlaceFluid,
}

impl HeldAbility {
    /// A short verb the HUD can show beside the item name.
    pub fn verb(self) -> &'static str {
        match self {
            HeldAbility::Empty => "—",
            HeldAbility::PlaceBlock => "Place",
            HeldAbility::PlaceLight => "Place (glows)",
            HeldAbility::PlaceDecoration => "Place decor",
            HeldAbility::Mineable => "Smelt/refine",
            HeldAbility::PlaceFluid => "Pour",
        }
    }
}

/// Classify what a held block-item does. Pure lookup over [`BlockId`].
pub fn ability_of(b: BlockId) -> HeldAbility {
    use BlockId::*;
    match b {
        Air => HeldAbility::Empty,
        Water | Lava => HeldAbility::PlaceFluid,
        CoalOre | IronOre | GoldOre | CopperOre | DiamondOre => HeldAbility::Mineable,
        Amber | Crystal => HeldAbility::PlaceLight,
        Flower | TallGrass | Fern | DeadBush | Vine | Mushroom | Lichen | Moss => {
            HeldAbility::PlaceDecoration
        }
        _ => HeldAbility::PlaceBlock,
    }
}

/// The quick-select bar.
#[derive(Clone, Debug)]
pub struct Hotbar {
    slots: SlotGrid,
    active: usize,
}

impl Default for Hotbar {
    fn default() -> Self {
        Hotbar::new()
    }
}

impl Hotbar {
    pub fn new() -> Hotbar {
        Hotbar { slots: SlotGrid::new(HOTBAR_SLOTS), active: 0 }
    }

    /// Seed the bar from a list of starter items (left-to-right).
    pub fn with_items(items: &[ItemStack]) -> Hotbar {
        let mut hb = Hotbar::new();
        for (i, s) in items.iter().take(HOTBAR_SLOTS).enumerate() {
            hb.slots.set(i, Some(*s));
        }
        hb
    }

    pub fn slots(&self) -> &SlotGrid {
        &self.slots
    }
    pub fn slots_mut(&mut self) -> &mut SlotGrid {
        &mut self.slots
    }
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// Pick a specific slot (1–9 keys map to 0..9). Out-of-range is ignored.
    pub fn select(&mut self, i: usize) {
        if i < HOTBAR_SLOTS {
            self.active = i;
        }
    }

    /// Mouse-wheel style cycling; wraps around. `dir` is +1 / -1 (any int).
    pub fn scroll(&mut self, dir: i32) {
        let n = HOTBAR_SLOTS as i32;
        let next = ((self.active as i32 + dir) % n + n) % n;
        self.active = next as usize;
    }

    pub fn active_item(&self) -> Option<ItemStack> {
        self.slots.get(self.active)
    }

    /// What the held item does — for the HUD readout.
    pub fn active_ability(&self) -> HeldAbility {
        match self.active_item() {
            Some(s) if !s.is_empty() => ability_of(s.item),
            _ => HeldAbility::Empty,
        }
    }

    pub fn set(&mut self, i: usize, stack: Option<ItemStack>) {
        if i < HOTBAR_SLOTS {
            self.slots.set(i, stack);
        }
    }

    /// Move an item from an inventory grid into a hotbar slot, swapping whatever
    /// was in the hotbar slot back into the inventory slot. This is the
    /// "drag an item onto the bar" gesture, item-count-conserving.
    pub fn assign_from(&mut self, inv: &mut SlotGrid, inv_slot: usize, hb_slot: usize) {
        if hb_slot >= HOTBAR_SLOTS || inv_slot >= inv.len() {
            return;
        }
        let from_inv = inv.get(inv_slot);
        let from_hb = self.slots.get(hb_slot);
        self.slots.set(hb_slot, from_inv);
        inv.set(inv_slot, from_hb);
    }

    /// Consume one unit from the active slot (e.g. after placing a block).
    /// Returns the block that was consumed, or `None` if the slot was empty.
    pub fn consume_active_one(&mut self) -> Option<BlockId> {
        let stack = self.slots.get(self.active)?;
        if stack.is_empty() {
            return None;
        }
        let block = stack.item;
        let taken = self.slots.take(self.active, 1);
        taken.map(|t| t.item).or(Some(block))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(b: BlockId, n: u16) -> ItemStack {
        ItemStack::new(b, n)
    }

    #[test]
    fn select_and_scroll_wrap() {
        let mut hb = Hotbar::new();
        assert_eq!(hb.active_index(), 0);
        hb.select(4);
        assert_eq!(hb.active_index(), 4);
        hb.select(99); // ignored
        assert_eq!(hb.active_index(), 4);
        hb.scroll(-5);
        assert_eq!(hb.active_index(), 8); // (4 - 5) wraps → 8
        hb.select(8);
        hb.scroll(1); // wraps to 0
        assert_eq!(hb.active_index(), 0);
    }

    #[test]
    fn active_ability_reflects_the_held_block() {
        let hb = Hotbar::with_items(&[stack(BlockId::IronOre, 3)]);
        assert_eq!(hb.active_ability(), HeldAbility::Mineable);
        let hb2 = Hotbar::with_items(&[stack(BlockId::Stone, 64)]);
        assert_eq!(hb2.active_ability(), HeldAbility::PlaceBlock);
        let hb3 = Hotbar::with_items(&[stack(BlockId::Flower, 1)]);
        assert_eq!(hb3.active_ability(), HeldAbility::PlaceDecoration);
        let empty = Hotbar::new();
        assert_eq!(empty.active_ability(), HeldAbility::Empty);
    }

    #[test]
    fn assign_from_inventory_swaps_and_conserves() {
        let mut inv = SlotGrid::new(10);
        inv.set(2, Some(stack(BlockId::OakLog, 12)));
        let mut hb = Hotbar::with_items(&[stack(BlockId::Stone, 5)]);
        // drag inventory slot 2 onto hotbar slot 0 (which holds Stone)
        hb.assign_from(&mut inv, 2, 0);
        assert_eq!(hb.slots().get(0).unwrap().item, BlockId::OakLog);
        assert_eq!(inv.get(2).unwrap().item, BlockId::Stone); // swapped back
    }

    #[test]
    fn consuming_a_placed_block_decrements() {
        let mut hb = Hotbar::with_items(&[stack(BlockId::Cobblestone, 2)]);
        assert_eq!(hb.consume_active_one(), Some(BlockId::Cobblestone));
        assert_eq!(hb.active_item().unwrap().count, 1);
    }
}
