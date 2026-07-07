//! The inventory grid: a flat list of slots, the first `hotbar_size` of which
//! form the hotbar. All operations are deterministic and integer-only.
//!
//! Slot indexing convention: `0 .. hotbar_size` is the hotbar (index 0 is the
//! leftmost hotbar slot); the rest is the main grid. The *selected* hotbar
//! slot's contents are what [`Inventory::held`] returns — the item a
//! place/break handler should act with.

use super::{ItemId, ItemRegistry, ItemStack};

/// A fixed-size collection of item slots with a selectable hotbar.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Inventory {
    slots: Vec<Option<ItemStack>>,
    hotbar_size: usize,
    selected: usize,
}

impl Inventory {
    /// `size` total slots, the first `hotbar_size` of which are the hotbar.
    /// Both are clamped to sane minimums (`size >= 1`, `1 <= hotbar <= size`).
    pub fn new(size: usize, hotbar_size: usize) -> Inventory {
        let size = size.max(1);
        let hotbar_size = hotbar_size.clamp(1, size);
        Inventory { slots: vec![None; size], hotbar_size, selected: 0 }
    }

    /// Minecraft-ish default: a 9-slot hotbar plus 27 main slots (36 total).
    pub fn standard() -> Inventory {
        Inventory::new(36, 9)
    }

    // ── inspection ──────────────────────────────────────────────────────

    pub fn len(&self) -> usize {
        self.slots.len()
    }
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.is_none())
    }
    pub fn hotbar_size(&self) -> usize {
        self.hotbar_size
    }
    pub fn slots(&self) -> &[Option<ItemStack>] {
        &self.slots
    }

    /// r449: reorder the BACKPACK (slots past the hotbar) into display order — empties to the end,
    /// grouped by item — using the pure `sim::inventory_sort` core. The hotbar is left untouched so
    /// the player's quick-access layout is preserved. Stable for equal items.
    pub fn sort_display(&mut self) {
        use crate::sim::inventory_sort::{sort_order, Slot};
        let start = self.hotbar_size.min(self.slots.len());
        let descr: Vec<Slot> = self.slots[start..]
            .iter()
            .map(|s| match s {
                Some(st) => Slot {
                    item_id: st.item.0,
                    category: 0,
                    rarity: 0,
                    quantity: st.count as u32,
                    name_key: st.item.0,
                },
                None => Slot { item_id: 0, category: 0, rarity: 0, quantity: 0, name_key: 0 },
            })
            .collect();
        let mut order = Vec::new();
        sort_order(&descr, &mut order);
        let reordered: Vec<Option<ItemStack>> =
            order.iter().map(|&i| self.slots[start + i as usize]).collect();
        for (k, st) in reordered.into_iter().enumerate() {
            self.slots[start + k] = st;
        }
    }
    /// Contents of slot `i`, or `None` if empty or out of range.
    pub fn slot(&self, i: usize) -> Option<ItemStack> {
        self.slots.get(i).copied().flatten()
    }

    // ── hotbar selection ────────────────────────────────────────────────

    pub fn selected(&self) -> usize {
        self.selected
    }
    /// Select a hotbar slot (clamped into the hotbar range).
    pub fn set_selected(&mut self, i: usize) {
        self.selected = i.min(self.hotbar_size - 1);
    }
    /// Scroll the hotbar selection by `delta`, wrapping (mouse wheel).
    pub fn scroll(&mut self, delta: i32) {
        let n = self.hotbar_size as i32;
        self.selected = (self.selected as i32 + delta).rem_euclid(n) as usize;
    }
    /// The stack in the selected hotbar slot — the "held" item.
    pub fn held(&self) -> Option<ItemStack> {
        self.slot(self.selected)
    }

    // ── bulk add / remove ───────────────────────────────────────────────

    /// Add `count` of `item`, topping up existing partial stacks first (low
    /// index first), then filling empty slots. Returns the count that did
    /// **not** fit.
    pub fn add_item<R: ItemRegistry>(&mut self, item: ItemId, count: u16, reg: &R) -> u16 {
        let max = reg.max_stack(item);
        if max == 0 || count == 0 {
            return count;
        }
        let mut remaining = count;

        // Pass 1: top up matching partial stacks.
        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                break;
            }
            if let Some(s) = slot {
                if s.item == item && s.count < max {
                    let add = (max - s.count).min(remaining);
                    s.count += add;
                    remaining -= add;
                }
            }
        }
        // Pass 2: fill empty slots.
        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                break;
            }
            if slot.is_none() {
                let add = max.min(remaining);
                *slot = Some(ItemStack::new(item, add));
                remaining -= add;
            }
        }
        remaining
    }

    /// Add a whole stack; returns the leftover stack that did not fit, if any.
    pub fn add_stack<R: ItemRegistry>(&mut self, stack: ItemStack, reg: &R) -> Option<ItemStack> {
        let left = self.add_item(stack.item, stack.count, reg);
        if left == 0 {
            None
        } else {
            Some(ItemStack::new(stack.item, left))
        }
    }

    /// Remove up to `count` of `item` across slots (low index first). Returns
    /// how many were actually removed.
    pub fn remove_item(&mut self, item: ItemId, count: u16) -> u16 {
        let mut remaining = count;
        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                break;
            }
            if let Some(s) = slot {
                if s.item == item {
                    let take = s.count.min(remaining);
                    s.count -= take;
                    remaining -= take;
                    if s.count == 0 {
                        *slot = None;
                    }
                }
            }
        }
        count - remaining
    }

    /// Total count of `item` held across all slots.
    pub fn count(&self, item: ItemId) -> u32 {
        self.slots
            .iter()
            .filter_map(|s| s.as_ref())
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    }

    /// Whether at least `count` of `item` is present.
    pub fn contains(&self, item: ItemId, count: u32) -> bool {
        self.count(item) >= count
    }

    // ── slot-level ops (for UI drag/drop) ───────────────────────────────

    /// Remove and return the contents of slot `i`.
    pub fn take(&mut self, i: usize) -> Option<ItemStack> {
        match self.slots.get_mut(i) {
            Some(slot) => slot.take(),
            None => None,
        }
    }

    /// Set slot `i`'s contents directly (a `count == 0` stack clears it).
    pub fn set(&mut self, i: usize, stack: Option<ItemStack>) {
        if let Some(slot) = self.slots.get_mut(i) {
            *slot = stack.filter(|s| s.count > 0);
        }
    }

    /// Swap two slots.
    pub fn swap(&mut self, a: usize, b: usize) {
        if a != b && a < self.slots.len() && b < self.slots.len() {
            self.slots.swap(a, b);
        }
    }

    /// Move/merge the stack in `from` onto `to`: into an empty target it moves
    /// wholesale; onto the same item it merges up to the stack ceiling and
    /// leaves any remainder in `from`; onto a different item it swaps. The
    /// primary left-click drag-and-drop operation.
    pub fn move_into<R: ItemRegistry>(&mut self, from: usize, to: usize, reg: &R) {
        if from == to || from >= self.slots.len() || to >= self.slots.len() {
            return;
        }
        match (self.slots[from], self.slots[to]) {
            (None, _) => {}
            (Some(s), None) => {
                self.slots[to] = Some(s);
                self.slots[from] = None;
            }
            (Some(s), Some(d)) if d.item == s.item => {
                let max = reg.max_stack(s.item);
                let moved = max.saturating_sub(d.count).min(s.count);
                self.slots[to] = Some(ItemStack::new(d.item, d.count + moved));
                let left = s.count - moved;
                self.slots[from] = if left > 0 { Some(ItemStack::new(s.item, left)) } else { None };
            }
            (Some(s), Some(d)) => {
                self.slots[to] = Some(s);
                self.slots[from] = Some(d);
            }
        }
    }

    /// Split slot `i`, returning the larger (ceil) half and leaving the rest —
    /// the right-click "grab half" gesture. A single item is taken whole.
    pub fn split_off(&mut self, i: usize) -> Option<ItemStack> {
        let s = self.slot(i)?;
        if s.count <= 1 {
            return self.take(i);
        }
        let grabbed = (s.count + 1) / 2; // ceil half
        let keep = s.count - grabbed;
        self.slots[i] = if keep > 0 { Some(ItemStack::new(s.item, keep)) } else { None };
        Some(ItemStack::new(s.item, grabbed))
    }

    /// Empty every slot.
    pub fn clear(&mut self) {
        for slot in self.slots.iter_mut() {
            *slot = None;
        }
    }

    // ── determinism ─────────────────────────────────────────────────────

    /// Stable 64-bit checksum (FNV-1a fold over slot contents + selection).
    /// Matches the sim's checksum style so an inventory can join desync checks
    /// when it eventually becomes networked sim state.
    pub fn checksum(&self) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        #[inline]
        fn fold(h: u64, v: u64) -> u64 {
            (h ^ v).wrapping_mul(PRIME)
        }
        let mut h = OFFSET;
        h = fold(h, self.slots.len() as u64);
        h = fold(h, self.selected as u64);
        for slot in &self.slots {
            match slot {
                Some(s) => {
                    h = fold(h, s.item.0 as u64);
                    h = fold(h, s.count as u64);
                }
                None => {
                    h = fold(h, u64::MAX); // distinct from any real item id
                }
            }
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{TableRegistry, UniformRegistry};

    const STONE: ItemId = ItemId(1);
    const DIRT: ItemId = ItemId(3);
    const PICK: ItemId = ItemId(100);

    fn reg64() -> UniformRegistry {
        UniformRegistry(64)
    }

    #[test]
    fn add_stacks_into_empty_slots_respecting_max() {
        let mut inv = Inventory::new(10, 9);
        let left = inv.add_item(STONE, 100, &reg64());
        assert_eq!(left, 0);
        assert_eq!(inv.slot(0), Some(ItemStack::new(STONE, 64)));
        assert_eq!(inv.slot(1), Some(ItemStack::new(STONE, 36)));
        assert_eq!(inv.count(STONE), 100);
    }

    #[test]
    fn add_returns_overflow_when_full() {
        let mut inv = Inventory::new(2, 1);
        let left = inv.add_item(STONE, 200, &reg64());
        assert_eq!(left, 200 - 128, "two 64-stacks fit, 72 overflow");
        assert_eq!(inv.count(STONE), 128);
    }

    #[test]
    fn add_tops_up_existing_partial_before_using_new_slot() {
        let mut inv = Inventory::new(4, 4);
        inv.set(0, Some(ItemStack::new(STONE, 60)));
        let left = inv.add_item(STONE, 10, &reg64());
        assert_eq!(left, 0);
        assert_eq!(inv.slot(0), Some(ItemStack::new(STONE, 64)), "topped to max first");
        assert_eq!(inv.slot(1), Some(ItemStack::new(STONE, 6)), "rest into next slot");
    }

    #[test]
    fn non_stackable_items_take_one_slot_each() {
        let mut reg = TableRegistry::new(64);
        reg.set(PICK, 1);
        let mut inv = Inventory::new(4, 4);
        let left = inv.add_item(PICK, 3, &reg);
        assert_eq!(left, 0);
        assert_eq!(inv.slot(0), Some(ItemStack::new(PICK, 1)));
        assert_eq!(inv.slot(1), Some(ItemStack::new(PICK, 1)));
        assert_eq!(inv.slot(2), Some(ItemStack::new(PICK, 1)));
        assert_eq!(inv.slot(3), None);
    }

    #[test]
    fn remove_pulls_across_slots() {
        let mut inv = Inventory::new(4, 4);
        inv.add_item(STONE, 100, &reg64()); // 64 + 36
        let got = inv.remove_item(STONE, 70);
        assert_eq!(got, 70);
        assert_eq!(inv.count(STONE), 30);
    }

    #[test]
    fn move_into_empty_moves_all_same_merges_diff_swaps() {
        let mut inv = Inventory::new(4, 4);
        inv.set(0, Some(ItemStack::new(STONE, 40)));
        // move to empty
        inv.move_into(0, 1, &reg64());
        assert_eq!(inv.slot(0), None);
        assert_eq!(inv.slot(1), Some(ItemStack::new(STONE, 40)));
        // merge same item with remainder
        inv.set(0, Some(ItemStack::new(STONE, 40)));
        inv.move_into(0, 1, &reg64()); // 40 onto 40, max 64 → 64 + 16 left
        assert_eq!(inv.slot(1), Some(ItemStack::new(STONE, 64)));
        assert_eq!(inv.slot(0), Some(ItemStack::new(STONE, 16)));
        // swap different items
        inv.set(0, Some(ItemStack::new(STONE, 16)));
        inv.set(1, Some(ItemStack::new(DIRT, 5)));
        inv.move_into(0, 1, &reg64());
        assert_eq!(inv.slot(0), Some(ItemStack::new(DIRT, 5)));
        assert_eq!(inv.slot(1), Some(ItemStack::new(STONE, 16)));
    }

    #[test]
    fn split_off_grabs_ceil_half() {
        let mut inv = Inventory::new(2, 2);
        inv.set(0, Some(ItemStack::new(STONE, 7)));
        let grabbed = inv.split_off(0);
        assert_eq!(grabbed, Some(ItemStack::new(STONE, 4)), "ceil(7/2)=4 grabbed");
        assert_eq!(inv.slot(0), Some(ItemStack::new(STONE, 3)), "floor stays");
        // single item: taken whole
        inv.set(1, Some(ItemStack::new(DIRT, 1)));
        assert_eq!(inv.split_off(1), Some(ItemStack::new(DIRT, 1)));
        assert_eq!(inv.slot(1), None);
    }

    #[test]
    fn hotbar_selection_wraps() {
        let mut inv = Inventory::standard(); // hotbar 9
        inv.set_selected(8);
        assert_eq!(inv.selected(), 8);
        inv.scroll(1);
        assert_eq!(inv.selected(), 0, "wrapped forward");
        inv.scroll(-1);
        assert_eq!(inv.selected(), 8, "wrapped backward");
        inv.set_selected(100); // clamps into hotbar
        assert_eq!(inv.selected(), 8);
    }

    #[test]
    fn held_reflects_selected_slot() {
        let mut inv = Inventory::standard();
        inv.set(3, Some(ItemStack::new(STONE, 10)));
        inv.set_selected(3);
        assert_eq!(inv.held(), Some(ItemStack::new(STONE, 10)));
    }

    #[test]
    fn checksum_is_stable_and_sensitive() {
        let mut a = Inventory::standard();
        let mut b = Inventory::standard();
        a.add_item(STONE, 50, &reg64());
        b.add_item(STONE, 50, &reg64());
        assert_eq!(a.checksum(), b.checksum(), "same contents → same checksum");
        b.add_item(DIRT, 1, &reg64());
        assert_ne!(a.checksum(), b.checksum(), "different contents → different checksum");
    }

    #[test]
    fn out_of_range_slot_ops_are_safe() {
        let mut inv = Inventory::new(4, 4);
        assert_eq!(inv.slot(99), None);
        assert_eq!(inv.take(99), None);
        inv.set(99, Some(ItemStack::new(STONE, 1))); // no-op, no panic
        inv.swap(0, 99); // no-op
        inv.move_into(0, 99, &reg64()); // no-op
        assert!(inv.is_empty());
    }
}
