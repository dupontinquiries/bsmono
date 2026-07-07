//! Inventory screen state — open/close, tab switching, and the cursor-held-stack
//! mouse interactions that sit *above* the storage model in `inventory.rs`. This
//! is the pure state machine behind the egui rendering: the egui layer draws what
//! this says and forwards clicks back into it, so the interaction logic is
//! testable without a display.
//!
//! Mouse model is the familiar one: left-click picks up a whole stack onto the
//! cursor, places it into an empty slot, merges into a matching stack, or swaps
//! with a different one; right-click picks up half, or drops one at a time.
//! Opening the screen frees the mouse for the UI; closing recaptures it for
//! mouse-look, and any stack still on the cursor is handed back to the caller to
//! drop into the world.
//!
//! Pure, deterministic.

use crate::inventory::{ItemStack, SlotGrid};

/// Which panel is showing. `Inventory` is the player's hotbar+storage; `Items`
/// and `SpawnEggs` are creative palettes (populated separately).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// The player's own live hotbar + storage grid.
    Inventory,
    // r638: the creative palette is now split by category (was just Items/SpawnEggs).
    // Each maps to a `sim::creative_items::CreativeCategory`; the item list per tab
    // comes from `item::creative_palette`.
    All,
    Building,
    Natural,
    Stone,
    Ingots,
    Tools,
    Weapons,
    Creatures,
}

impl Tab {
    /// Every tab, in display order (left→right). Drives cycling for keys and
    /// swipe gestures.
    pub const ALL: [Tab; 9] = [
        Tab::Inventory,
        Tab::All,
        Tab::Building,
        Tab::Natural,
        Tab::Stone,
        Tab::Ingots,
        Tab::Tools,
        Tab::Weapons,
        Tab::Creatures,
    ];

    /// The creative category this tab shows, or `None` for the live inventory.
    pub fn to_category(self) -> Option<crate::sim::creative_items::CreativeCategory> {
        use crate::sim::creative_items::CreativeCategory as C;
        Some(match self {
            Tab::Inventory => return None,
            Tab::All => C::All,
            Tab::Building => C::Building,
            Tab::Natural => C::Natural,
            Tab::Stone => C::Stone,
            Tab::Ingots => C::Ingots,
            Tab::Tools => C::Tools,
            Tab::Weapons => C::Weapons,
            Tab::Creatures => C::Creatures,
        })
    }

    /// Short label for the tab strip.
    pub fn label(self) -> &'static str {
        match self {
            Tab::Inventory => "Inventory",
            Tab::All => "All",
            Tab::Building => "Building",
            Tab::Natural => "Natural",
            Tab::Stone => "Stone",
            Tab::Ingots => "Ingots",
            Tab::Tools => "Tools",
            Tab::Weapons => "Weapons",
            Tab::Creatures => "Creatures",
        }
    }

    fn index(self) -> usize {
        Tab::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }

    /// Next tab to the right, wrapping (swipe-left / Tab key).
    pub fn next(self) -> Tab {
        Tab::ALL[(self.index() + 1) % Tab::ALL.len()]
    }

    /// Previous tab to the left, wrapping (swipe-right / Shift-Tab).
    pub fn prev(self) -> Tab {
        Tab::ALL[(self.index() + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }
}

/// Screen state layered over the storage grids.
#[derive(Debug, Clone)]
pub struct InventoryScreen {
    pub open: bool,
    pub tab: Tab,
    /// The stack currently "on the cursor" (mid-drag / picked up).
    pub held: Option<ItemStack>,
    /// Slot index under the pointer in the active grid, for highlight.
    pub hovered: Option<usize>,
}

impl Default for InventoryScreen {
    fn default() -> Self {
        Self { open: false, tab: Tab::Inventory, held: None, hovered: None }
    }
}

impl InventoryScreen {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bin captures the mouse for look only while the screen is closed.
    pub fn wants_mouse_captured(&self) -> bool {
        !self.open
    }

    pub fn open(&mut self) {
        self.open = true;
    }

    /// Close the screen. Returns any stack still on the cursor so the caller can
    /// drop it into the world (Minecraft-style).
    pub fn close(&mut self) -> Option<ItemStack> {
        self.open = false;
        self.hovered = None;
        self.held.take()
    }

    /// Toggle open/closed. When closing, returns a leftover held stack (as `close`).
    pub fn toggle(&mut self) -> Option<ItemStack> {
        if self.open {
            self.close()
        } else {
            self.open();
            None
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.hovered = None;
    }

    pub fn hover(&mut self, slot: Option<usize>) {
        self.hovered = slot;
    }

    /// Left-click a slot: pick up / place / merge / swap.
    pub fn left_click(&mut self, grid: &mut SlotGrid, slot: usize) {
        match (self.held, grid.get(slot)) {
            (None, Some(_)) => {
                // Pick up the whole stack.
                self.held = grid.get(slot);
                grid.set(slot, None);
            }
            (None, None) => {}
            (Some(h), None) => {
                // Place all.
                grid.set(slot, Some(h));
                self.held = None;
            }
            (Some(h), Some(mut dst)) => {
                if dst.item == h.item {
                    // Merge into the slot, keep any leftover on the cursor.
                    let leftover = dst.merge(h.count);
                    grid.set(slot, Some(dst));
                    self.held = (leftover > 0).then_some(ItemStack { item: h.item, count: leftover });
                } else {
                    // Swap cursor and slot.
                    grid.set(slot, Some(h));
                    self.held = Some(dst);
                }
            }
        }
    }

    /// Right-click a slot: pick up half, or drop a single item.
    pub fn right_click(&mut self, grid: &mut SlotGrid, slot: usize) {
        match (self.held, grid.get(slot)) {
            (None, Some(mut s)) => {
                // Pick up half (ceil).
                let half = (s.count + 1) / 2;
                let taken = s.split(half);
                self.held = Some(taken);
                grid.set(slot, if s.is_empty() { None } else { Some(s) });
            }
            (None, None) => {}
            (Some(mut h), None) => {
                // Drop one into the empty slot.
                grid.set(slot, Some(ItemStack { item: h.item, count: 1 }));
                h.count -= 1;
                self.held = (h.count > 0).then_some(h);
            }
            (Some(mut h), Some(mut dst)) => {
                if dst.item == h.item && dst.space() > 0 {
                    // Drop one onto the matching stack.
                    dst.merge(1);
                    grid.set(slot, Some(dst));
                    h.count -= 1;
                    self.held = (h.count > 0).then_some(h);
                }
                // Different item, or full: right-click does nothing.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::ItemStack;
    use crate::world::blocks::BlockId;

    fn grid_with(slots: &[(usize, BlockId, u16)], n: usize) -> SlotGrid {
        let mut g = SlotGrid::new(n);
        for &(i, item, count) in slots {
            g.set(i, Some(ItemStack { item, count }));
        }
        g
    }

    #[test]
    fn opening_frees_the_mouse_closing_recaptures() {
        let mut s = InventoryScreen::new();
        assert!(s.wants_mouse_captured(), "captured for look when closed");
        s.toggle();
        assert!(s.open && !s.wants_mouse_captured(), "freed for UI when open");
        s.toggle();
        assert!(!s.open && s.wants_mouse_captured());
    }

    #[test]
    fn left_click_picks_up_a_whole_stack_then_places_it() {
        let mut g = grid_with(&[(0, BlockId::Stone, 30)], 4);
        let mut s = InventoryScreen::new();
        s.left_click(&mut g, 0);
        assert_eq!(s.held.unwrap().count, 30);
        assert!(g.get(0).is_none(), "source slot emptied");
        s.left_click(&mut g, 2);
        assert!(s.held.is_none());
        assert_eq!(g.get(2).unwrap().count, 30, "placed into the empty slot");
    }

    #[test]
    fn left_click_merges_matching_stacks_and_keeps_overflow_on_the_cursor() {
        // Cursor holds 50 stone, slot has 30 → 64 in slot, 16 left on cursor.
        let mut g = grid_with(&[(0, BlockId::Stone, 30)], 4);
        let mut s = InventoryScreen::new();
        s.held = Some(ItemStack { item: BlockId::Stone, count: 50 });
        s.left_click(&mut g, 0);
        assert_eq!(g.get(0).unwrap().count, 64);
        assert_eq!(s.held.unwrap().count, 16, "overflow stays on the cursor");
    }

    #[test]
    fn left_click_swaps_different_items() {
        let mut g = grid_with(&[(0, BlockId::Dirt, 10)], 4);
        let mut s = InventoryScreen::new();
        s.held = Some(ItemStack { item: BlockId::Stone, count: 5 });
        s.left_click(&mut g, 0);
        assert_eq!(g.get(0).unwrap().item, BlockId::Stone);
        assert_eq!(s.held.unwrap().item, BlockId::Dirt, "swapped onto the cursor");
    }

    #[test]
    fn right_click_picks_up_half_rounding_up() {
        let mut g = grid_with(&[(0, BlockId::Stone, 9)], 4);
        let mut s = InventoryScreen::new();
        s.right_click(&mut g, 0);
        assert_eq!(s.held.unwrap().count, 5, "ceil(9/2)");
        assert_eq!(g.get(0).unwrap().count, 4);
    }

    #[test]
    fn right_click_drops_one_at_a_time() {
        let mut g = SlotGrid::new(4);
        let mut s = InventoryScreen::new();
        s.held = Some(ItemStack { item: BlockId::Stone, count: 3 });
        s.right_click(&mut g, 1);
        assert_eq!(g.get(1).unwrap().count, 1);
        assert_eq!(s.held.unwrap().count, 2, "one dropped, rest on cursor");
        s.right_click(&mut g, 1);
        assert_eq!(g.get(1).unwrap().count, 2);
        assert_eq!(s.held.unwrap().count, 1);
    }

    #[test]
    fn closing_hands_back_a_held_stack_to_drop() {
        let mut s = InventoryScreen::new();
        s.open();
        s.held = Some(ItemStack { item: BlockId::Stone, count: 7 });
        let dropped = s.close();
        assert_eq!(dropped.unwrap().count, 7, "caller drops it into the world");
        assert!(s.held.is_none() && !s.open);
    }

    #[test]
    fn switching_tabs_clears_hover() {
        let mut s = InventoryScreen::new();
        s.hover(Some(3));
        s.set_tab(Tab::Creatures);
        assert_eq!(s.tab, Tab::Creatures);
        assert!(s.hovered.is_none());
    }
}
