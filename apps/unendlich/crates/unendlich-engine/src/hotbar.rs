//! Helix-inspired modal hotbar control — the *logic* behind fast home-row slot
//! selection and item management. Pure and deterministic; the bin forwards key
//! presses (and swipe directions) into this, and the overlay renders its state.
//!
//! ## Reading of the design (assumptions — correct me)
//!
//! - **Home row = fast slot calls.** Tapping a Dvorak home-row key selects that
//!   hotbar slot without leaving home position:
//!   `a o e u i d h t n s -` → slots 0..=10 ([`HOMEROW`]). The active hotbar
//!   length decides how many are live.
//! - **Modal management** (Helix flavor). A `Quick` mode where home-row taps
//!   *select*, and a `Manage` mode where `h`/`l`/`k`/`j` *act*:
//!     - `h` / `l` — nudge the selected stack one slot left / right (reorder),
//!       carrying the selection with it.
//!     - `k` / `j` — pin **up** / **down**: `k` stashes the selected stack into a
//!       pinned row, `j` pulls a pinned stack back down into the selected slot.
//!   The one key that means two things is `h` (home-row slot 6 *and* Helix-left);
//!   the **mode** disambiguates — in `Manage`, `h`/`l`/`k`/`j` are commands and
//!   every other home-row key still selects.
//!
//! Tab switching (Inventory / Items / SpawnEggs) cycles via [`Hotbar::cycle_tab`]
//! — wire it to Tab/Shift-Tab or to a horizontal swipe. (Detecting a swipe is
//! platform input the engine bin owns; this models only what a swipe *does*.)

use crate::inventory::SlotGrid;
use crate::inventory_screen::Tab;

/// Dvorak home row left→right, the fast-call slot keys. `-` extends to an 11th.
pub const HOMEROW: [char; 11] = ['a', 'o', 'e', 'u', 'i', 'd', 'h', 't', 'n', 's', '-'];

/// The hotbar slot a home-row key maps to, if any.
pub fn homerow_slot(c: char) -> Option<usize> {
    HOMEROW.iter().position(|&k| k == c)
}

/// The home-row key that selects a given slot (for rendering hints), if in range.
pub fn slot_homerow_key(slot: usize) -> Option<char> {
    HOMEROW.get(slot).copied()
}

/// Horizontal direction — reorder nudges and tab/swipe cycling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
}

/// Interaction mode. `Quick` is the default (home-row selects); `Manage` turns
/// `h l k j` into reorder/pin commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotbarMode {
    Quick,
    Manage,
}

/// Pure control state over a hotbar `SlotGrid` (passed in per call, like
/// `InventoryScreen`). Holds the selection, mode, and active tab.
#[derive(Debug, Clone)]
pub struct Hotbar {
    pub selected: usize,
    pub len: usize,
    pub mode: HotbarMode,
    pub tab: Tab,
}

impl Hotbar {
    /// `len` slots, capped to the home-row's reach (11).
    pub fn new(len: usize) -> Self {
        Self { selected: 0, len: len.min(HOMEROW.len()).max(1), mode: HotbarMode::Quick, tab: Tab::Inventory }
    }

    pub fn toggle_manage(&mut self) {
        self.mode = match self.mode {
            HotbarMode::Quick => HotbarMode::Manage,
            HotbarMode::Manage => HotbarMode::Quick,
        };
    }

    /// Select a slot directly (bounds-checked). Returns whether it moved.
    pub fn select(&mut self, slot: usize) -> bool {
        if slot < self.len {
            self.selected = slot;
            true
        } else {
            false
        }
    }

    /// Quick-mode home-row tap → select. Returns true if it hit a live slot.
    pub fn select_homerow(&mut self, c: char) -> bool {
        match homerow_slot(c) {
            Some(s) if s < self.len => {
                self.selected = s;
                true
            }
            _ => false,
        }
    }

    /// Manage-mode `h`/`l`: move the selected stack one slot and follow it.
    /// No-op at the ends or with fewer than two slots. Returns whether it moved.
    pub fn nudge(&mut self, grid: &mut SlotGrid, dir: Dir) -> bool {
        let n = self.len.min(grid.len());
        if n < 2 {
            return false;
        }
        let to = match dir {
            Dir::Left if self.selected > 0 => self.selected - 1,
            Dir::Right if self.selected + 1 < n => self.selected + 1,
            _ => return false,
        };
        grid.swap(self.selected, to);
        self.selected = to;
        true
    }

    /// `k` = pin up: stash the selected stack into the first free `pinned` slot.
    /// `j` = pin down: pull the last-filled pinned stack into the selected slot
    /// (only when it's empty). Returns whether anything moved.
    pub fn pin(&mut self, grid: &mut SlotGrid, pinned: &mut SlotGrid, up: bool) -> bool {
        if up {
            if self.selected >= grid.len() || grid.get(self.selected).is_none() {
                return false;
            }
            if let Some(free) = (0..pinned.len()).find(|&i| pinned.get(i).is_none()) {
                pinned.set(free, grid.get(self.selected));
                grid.set(self.selected, None);
                return true;
            }
            false
        } else {
            if self.selected >= grid.len() || grid.get(self.selected).is_some() {
                return false; // only pull down into an empty slot
            }
            if let Some(src) = (0..pinned.len()).rev().find(|&i| pinned.get(i).is_some()) {
                grid.set(self.selected, pinned.get(src));
                pinned.set(src, None);
                return true;
            }
            false
        }
    }

    /// Switch tab (Tab key or horizontal swipe).
    pub fn cycle_tab(&mut self, dir: Dir) {
        self.tab = match dir {
            Dir::Right => self.tab.next(),
            Dir::Left => self.tab.prev(),
        };
    }

    /// Dispatch a key by mode, resolving the `h` collision. In `Manage`,
    /// `h l k j` are commands; every other key (and all of `Quick`) is a
    /// home-row select. Returns whether the key was consumed.
    pub fn on_key(&mut self, c: char, grid: &mut SlotGrid, pinned: &mut SlotGrid) -> bool {
        if self.mode == HotbarMode::Manage {
            match c {
                'h' => return self.nudge(grid, Dir::Left),
                'l' => return self.nudge(grid, Dir::Right),
                'k' => return self.pin(grid, pinned, true),
                'j' => return self.pin(grid, pinned, false),
                _ => {}
            }
        }
        self.select_homerow(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::ItemStack;
    use crate::world::blocks::BlockId;

    fn stack(b: BlockId, n: u16) -> Option<ItemStack> {
        Some(ItemStack { item: b, count: n })
    }

    #[test]
    fn homerow_maps_dvorak_home_row_to_slots() {
        assert_eq!(homerow_slot('a'), Some(0));
        assert_eq!(homerow_slot('i'), Some(4));
        assert_eq!(homerow_slot('d'), Some(5));
        assert_eq!(homerow_slot('s'), Some(9));
        assert_eq!(homerow_slot('-'), Some(10));
        assert_eq!(homerow_slot('q'), None); // not on the home row
        // Round-trip with the rendering hint.
        for (i, &c) in HOMEROW.iter().enumerate() {
            assert_eq!(slot_homerow_key(i), Some(c));
            assert_eq!(homerow_slot(c), Some(i));
        }
    }

    #[test]
    fn quick_mode_homerow_selects_within_bounds() {
        let mut h = Hotbar::new(9);
        assert!(h.select_homerow('a') && h.selected == 0);
        assert!(h.select_homerow('n') && h.selected == 8); // 9th slot
        // 's' would be slot 9, out of a 9-slot bar → no-op.
        assert!(!h.select_homerow('s'));
        assert_eq!(h.selected, 8);
    }

    #[test]
    fn manage_nudge_reorders_and_follows_selection() {
        let mut g = SlotGrid::new(9);
        g.set(3, stack(BlockId::Stone, 5));
        g.set(4, stack(BlockId::Dirt, 5));
        let mut h = Hotbar::new(9);
        h.select(3);
        h.toggle_manage();
        assert!(h.nudge(&mut g, Dir::Right));
        // Stone followed to slot 4; Dirt swapped back to 3.
        assert_eq!(g.get(4), stack(BlockId::Stone, 5));
        assert_eq!(g.get(3), stack(BlockId::Dirt, 5));
        assert_eq!(h.selected, 4);
        // Right edge is a no-op.
        h.select(8);
        assert!(!h.nudge(&mut g, Dir::Right));
        assert_eq!(h.selected, 8);
    }

    #[test]
    fn pin_up_then_down_round_trips() {
        let mut bar = SlotGrid::new(9);
        let mut pinned = SlotGrid::new(4);
        bar.set(2, stack(BlockId::OakLog, 12));
        let mut h = Hotbar::new(9);
        h.select(2);
        // Pin up: stack leaves the bar for the pinned row.
        assert!(h.pin(&mut bar, &mut pinned, true));
        assert_eq!(bar.get(2), None);
        assert_eq!(pinned.get(0), stack(BlockId::OakLog, 12));
        // Pin up again with an empty slot → nothing to stash.
        assert!(!h.pin(&mut bar, &mut pinned, true));
        // Pin down: pull it back into the now-empty selected slot.
        assert!(h.pin(&mut bar, &mut pinned, false));
        assert_eq!(bar.get(2), stack(BlockId::OakLog, 12));
        assert_eq!(pinned.get(0), None);
        // Pin down into an occupied slot → refused.
        assert!(!h.pin(&mut bar, &mut pinned, false));
    }

    #[test]
    fn on_key_resolves_the_h_collision_by_mode() {
        let mut bar = SlotGrid::new(9);
        let mut pinned = SlotGrid::new(4);
        bar.set(6, stack(BlockId::Sand, 3));
        let mut h = Hotbar::new(9);

        // Quick mode: 'h' selects slot 6 (its home-row position).
        assert!(h.on_key('h', &mut bar, &mut pinned));
        assert_eq!(h.selected, 6);
        assert_eq!(h.mode, HotbarMode::Quick);

        // Manage mode: 'h' nudges left instead of selecting.
        h.toggle_manage();
        assert!(h.on_key('h', &mut bar, &mut pinned));
        assert_eq!(h.selected, 5);
        assert_eq!(bar.get(5), stack(BlockId::Sand, 3)); // sand followed left
        // A non-command home-row key still selects even in Manage mode.
        assert!(h.on_key('a', &mut bar, &mut pinned));
        assert_eq!(h.selected, 0);
    }

    #[test]
    fn tab_cycling_wraps_both_ways() {
        let mut h = Hotbar::new(9);
        assert_eq!(h.tab, Tab::Inventory);
        h.cycle_tab(Dir::Right);
        assert_eq!(h.tab, Tab::Items);
        h.cycle_tab(Dir::Right);
        assert_eq!(h.tab, Tab::SpawnEggs);
        h.cycle_tab(Dir::Right); // wrap
        assert_eq!(h.tab, Tab::Inventory);
        h.cycle_tab(Dir::Left); // wrap back
        assert_eq!(h.tab, Tab::SpawnEggs);
    }
}
