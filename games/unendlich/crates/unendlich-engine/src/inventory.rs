//! Inventory and item-bar (hotbar) model — pure data and operations. What the
//! player holds, how stacks merge and split, which hotbar slot is selected, and
//! how the scroll wheel cycles it. The input layer (the `Hotbar1..N` keybindings
//! and the scroll action) drives these methods; the renderer reads the selected
//! slot to draw the item bar. No I/O, no rendering, fully unit-tested.
//!
//! Items are blocks for now (a placeable world is the first use); the model is
//! shaped so non-block items (tools, food) can slot in later via a wider item
//! type and a per-item `max_stack`.

use crate::world::blocks::BlockId;

/// Default maximum count in one stack.
pub const MAX_STACK: u16 = 64;

/// A stack of identical items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemStack {
    pub item: BlockId,
    pub count: u16,
}

impl ItemStack {
    pub fn new(item: BlockId, count: u16) -> Self {
        Self { item, count }
    }

    /// A single item.
    pub fn single(item: BlockId) -> Self {
        Self { item, count: 1 }
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Max stack for this item (uniform today; per-item later).
    pub fn max_stack(&self) -> u16 {
        MAX_STACK
    }

    /// Free room left in this stack.
    pub fn space(&self) -> u16 {
        self.max_stack().saturating_sub(self.count)
    }

    /// Pour up to `incoming` of the *same* item into this stack. Returns the
    /// leftover that didn't fit (0 if it all fit).
    pub fn merge(&mut self, incoming: u16) -> u16 {
        let room = self.space();
        let taken = incoming.min(room);
        self.count += taken;
        incoming - taken
    }

    /// Split `n` items off into a new stack, reducing this one. `n` is clamped
    /// to what's available.
    pub fn split(&mut self, n: u16) -> ItemStack {
        let taken = n.min(self.count);
        self.count -= taken;
        ItemStack { item: self.item, count: taken }
    }
}

/// A fixed row/grid of slots, each empty or holding one stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotGrid {
    pub slots: Vec<Option<ItemStack>>,
}

impl SlotGrid {
    pub fn new(n: usize) -> Self {
        Self { slots: vec![None; n] }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.is_none())
    }

    pub fn get(&self, i: usize) -> Option<ItemStack> {
        self.slots.get(i).copied().flatten()
    }

    pub fn set(&mut self, i: usize, stack: Option<ItemStack>) {
        if let Some(slot) = self.slots.get_mut(i) {            // Normalize an empty stack to None.
            *slot = stack.filter(|s| !s.is_empty());
        }
    }

    /// r449: reorder this grid into display order (empties to the end, grouped by item) using the
    /// pure `sim::inventory_sort` core. Stable, so equal stacks keep their relative order.
    pub fn sort_display(&mut self) {
        use crate::sim::inventory_sort::{sort_order, Slot};
        let slots: Vec<Slot> = self
            .slots
            .iter()
            .map(|s| match s {
                Some(st) => Slot {
                    item_id: st.item as u32,
                    category: 0,
                    rarity: 0,
                    quantity: st.count as u32,
                    name_key: st.item as u32,
                },
                None => Slot { item_id: 0, category: 0, rarity: 0, quantity: 0, name_key: 0 },
            })
            .collect();
        let mut order = Vec::new();
        sort_order(&slots, &mut order);
        let reordered: Vec<Option<ItemStack>> =
            order.iter().map(|&i| self.slots[i as usize]).collect();
        self.slots = reordered;
    }

    /// Insert a stack, filling existing matching stacks first (top up), then
    /// empty slots. Returns the leftover count that didn't fit anywhere.
    pub fn add(&mut self, mut stack: ItemStack) -> u16 {
        if stack.is_empty() {
            return 0;
        }
        // Pass 1: top up existing stacks of the same item.
        for slot in self.slots.iter_mut() {
            if let Some(s) = slot {
                if s.item == stack.item {
                    stack.count = s.merge(stack.count);
                    if stack.count == 0 {
                        return 0;
                    }
                }
            }
        }
        // Pass 2: drop into empty slots.
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                let put = stack.count.min(stack.max_stack());
                *slot = Some(ItemStack { item: stack.item, count: put });
                stack.count -= put;
                if stack.count == 0 {
                    return 0;
                }
            }
        }
        stack.count // leftover that didn't fit
    }

    /// Take up to `n` items from slot `i`. Returns what was removed (or `None`
    /// if the slot was empty). Empties the slot when it hits zero.
    pub fn take(&mut self, i: usize, n: u16) -> Option<ItemStack> {
        let slot = self.slots.get_mut(i)?;
        let s = slot.as_mut()?;
        let out = s.split(n);
        if s.is_empty() {
            *slot = None;
        }
        Some(out)
    }

    pub fn swap(&mut self, a: usize, b: usize) {
        if a < self.slots.len() && b < self.slots.len() && a != b {
            self.slots.swap(a, b);
        }
    }

    /// Move the contents of `from` into `to`: merge if the same item (leaving
    /// any overflow behind in `from`), otherwise swap. The drag-and-drop op.
    pub fn move_into(&mut self, from: usize, to: usize) {
        if from == to || from >= self.slots.len() || to >= self.slots.len() {
            return;
        }
        match (self.get(from), self.get(to)) {
            (Some(src), Some(dst)) if src.item == dst.item => {
                let mut dst = dst;
                let leftover = dst.merge(src.count);
                self.set(to, Some(dst));
                self.set(from, if leftover == 0 {
                    None
                } else {
                    Some(ItemStack { item: src.item, count: leftover })
                });
            }
            _ => self.swap(from, to),
        }
    }

    /// Total count of `item` across all slots.
    pub fn total(&self, item: BlockId) -> u32 {
        self.slots
            .iter()
            .filter_map(|s| *s)
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    }

    /// Remove up to `n` of `item` across all slots (emptying slots as they hit
    /// zero). Returns the number actually removed.
    pub fn remove(&mut self, item: BlockId, n: u16) -> u16 {
        let mut remaining = n;
        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                break;
            }
            if let Some(s) = slot {
                if s.item == item {
                    let taken = remaining.min(s.count);
                    s.count -= taken;
                    remaining -= taken;
                    if s.is_empty() {
                        *slot = None;
                    }
                }
            }
        }
        n - remaining
    }

    /// True if no slot can accept more of `item` (no matching stack with room,
    /// no empty slot).
    pub fn is_full_for(&self, item: BlockId) -> bool {
        !self.slots.iter().any(|s| match s {
            None => true,
            Some(st) => st.item == item && st.space() > 0,
        })
    }
}

/// The item bar: a slot row plus the currently selected index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotbar {
    pub grid: SlotGrid,
    pub selected: usize,
}

impl Hotbar {
    pub fn new(n: usize) -> Self {
        Self { grid: SlotGrid::new(n.max(1)), selected: 0 }
    }

    /// Select a slot by index (e.g. number keys). Out-of-range is clamped.
    pub fn select(&mut self, i: usize) {
        self.selected = i.min(self.grid.len() - 1);
    }

    /// Scroll-wheel cycling: `delta` slots, wrapping around both ends.
    pub fn scroll(&mut self, delta: i32) {
        let n = self.grid.len() as i32;
        let next = (self.selected as i32 + delta).rem_euclid(n);
        self.selected = next as usize;
    }

    pub fn selected_stack(&self) -> Option<ItemStack> {
        self.grid.get(self.selected)
    }

    /// Consume one of the selected item (placing a block). Returns the block
    /// placed, or `None` if the selected slot is empty.
    pub fn consume_selected(&mut self) -> Option<BlockId> {
        let taken = self.grid.take(self.selected, 1)?;
        Some(taken.item)
    }
}

/// Full inventory: the always-visible hotbar plus a larger backing store.
/// Picked-up items fill the hotbar first (so the player can use them), then
/// spill into main storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    pub hotbar: Hotbar,
    pub main: SlotGrid,
}

impl Inventory {
    /// `hotbar_slots` visible slots + `main_slots` backing slots.
    pub fn new(hotbar_slots: usize, main_slots: usize) -> Self {
        Self { hotbar: Hotbar::new(hotbar_slots), main: SlotGrid::new(main_slots) }
    }

    /// Pick up a stack: hotbar first, then main storage. Returns the leftover
    /// that didn't fit anywhere (0 if it all fit).
    pub fn pickup(&mut self, stack: ItemStack) -> u16 {
        let leftover = self.hotbar.grid.add(stack);
        if leftover == 0 {
            return 0;
        }
        self.main.add(ItemStack { item: stack.item, count: leftover })
    }

    /// Total of `item` across hotbar + main.
    pub fn total(&self, item: BlockId) -> u32 {
        self.hotbar.grid.total(item) + self.main.total(item)
    }

    /// Remove exactly `n` of `item` if at least that many are present
    /// (hotbar + main). All-or-nothing: returns `true` having removed them, or
    /// `false` having changed nothing. The consume side of crafting/placement.
    pub fn remove(&mut self, item: BlockId, n: u16) -> bool {
        if self.total(item) < n as u32 {
            return false;
        }
        let from_hotbar = self.hotbar.grid.remove(item, n);
        let rest = n - from_hotbar;
        if rest > 0 {
            self.main.remove(item, rest);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::blocks::BlockId;

    #[test]
    fn stack_merge_respects_max_and_reports_leftover() {
        let mut s = ItemStack::new(BlockId::Stone, 60);
        let leftover = s.merge(10); // room for 4, 6 left over
        assert_eq!(s.count, 64);
        assert_eq!(leftover, 6);
        // A full stack accepts nothing.
        assert_eq!(s.merge(5), 5);
    }

    #[test]
    fn stack_split_clamps() {
        let mut s = ItemStack::new(BlockId::Dirt, 10);
        let off = s.split(3);
        assert_eq!((off.count, s.count), (3, 7));
        // Splitting more than present takes only what's there.
        let rest = s.split(99);
        assert_eq!((rest.count, s.count), (7, 0));
        assert!(s.is_empty());
    }

    #[test]
    fn grid_add_stacks_then_fills_then_overflows() {
        let mut g = SlotGrid::new(2);
        assert_eq!(g.add(ItemStack::new(BlockId::Stone, 50)), 0);
        // Tops up the existing stone stack (50→64), then opens a new slot.
        assert_eq!(g.add(ItemStack::new(BlockId::Stone, 20)), 0);
        assert_eq!(g.total(BlockId::Stone), 70);
        // Two slots: one full (64), one with 6. A big add overflows.
        let leftover = g.add(ItemStack::new(BlockId::Stone, 100));
        // Room was (64-64)=0 in slot0 + (64-6)=58 in slot1 → 58 taken, 42 left.
        assert_eq!(leftover, 42);
        assert!(g.is_full_for(BlockId::Stone));
    }

    #[test]
    fn grid_take_empties_slot() {
        let mut g = SlotGrid::new(1);
        g.add(ItemStack::new(BlockId::Sand, 5));
        let got = g.take(0, 2).unwrap();
        assert_eq!(got.count, 2);
        assert_eq!(g.get(0).unwrap().count, 3);
        g.take(0, 99); // takes the rest
        assert!(g.get(0).is_none(), "emptied slot becomes None");
    }

    #[test]
    fn grid_move_merges_same_swaps_different() {
        let mut g = SlotGrid::new(2);
        g.set(0, Some(ItemStack::new(BlockId::Stone, 30)));
        g.set(1, Some(ItemStack::new(BlockId::Stone, 20)));
        g.move_into(0, 1); // merge into slot 1
        assert!(g.get(0).is_none());
        assert_eq!(g.get(1).unwrap().count, 50);

        // Different items swap.
        g.set(0, Some(ItemStack::new(BlockId::Dirt, 10)));
        g.move_into(0, 1);
        assert_eq!(g.get(0).unwrap().item, BlockId::Stone);
        assert_eq!(g.get(1).unwrap().item, BlockId::Dirt);
    }

    #[test]
    fn move_merge_overflow_leaves_remainder_behind() {
        let mut g = SlotGrid::new(2);
        g.set(0, Some(ItemStack::new(BlockId::Stone, 40)));
        g.set(1, Some(ItemStack::new(BlockId::Stone, 50)));
        g.move_into(0, 1); // slot1 has room for 14; 26 stay in slot0
        assert_eq!(g.get(1).unwrap().count, 64);
        assert_eq!(g.get(0).unwrap().count, 26);
    }

    #[test]
    fn hotbar_select_clamps_and_scroll_wraps() {
        let mut h = Hotbar::new(9);
        h.select(3);
        assert_eq!(h.selected, 3);
        h.select(99); // clamped to last
        assert_eq!(h.selected, 8);
        h.scroll(1); // wrap forward 8→0
        assert_eq!(h.selected, 0);
        h.scroll(-1); // wrap backward 0→8
        assert_eq!(h.selected, 8);
        h.scroll(11); // 8 + 11 = 19 mod 9 = 1
        assert_eq!(h.selected, 1);
    }

    #[test]
    fn hotbar_consume_selected_decrements_and_clears() {
        let mut h = Hotbar::new(9);
        h.grid.set(0, Some(ItemStack::new(BlockId::OakPlanks, 2)));
        h.select(0);
        assert_eq!(h.consume_selected(), Some(BlockId::OakPlanks));
        assert_eq!(h.selected_stack().unwrap().count, 1);
        assert_eq!(h.consume_selected(), Some(BlockId::OakPlanks));
        assert!(h.selected_stack().is_none(), "slot empties");
        assert_eq!(h.consume_selected(), None, "nothing left to place");
    }

    #[test]
    fn inventory_pickup_fills_hotbar_then_main() {
        // Tiny inventory: 1 hotbar slot + 1 main slot, so a big pickup spills.
        let mut inv = Inventory::new(1, 1);
        // 64 fills the hotbar slot exactly; another 64 fills main; 10 overflow.
        let leftover = inv.pickup(ItemStack::new(BlockId::Sandstone, 138));
        assert_eq!(leftover, 138 - 128);
        assert_eq!(inv.total(BlockId::Sandstone), 128);
        assert_eq!(inv.hotbar.grid.get(0).unwrap().count, 64);
        assert_eq!(inv.main.get(0).unwrap().count, 64);
    }
}
