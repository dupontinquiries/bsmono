//! sim::inventory_transfer — move stacks *between* two inventory grids.
//!
//! [`crate::sim::inventory_grid`] backs both the player inventory and chests and
//! handles everything *within* one grid (add, move, swap, split, take). What it
//! doesn't do is move items **between** two grids — the shift-click into a
//! chest, "deposit all", "loot all" the game needs the moment a container is
//! opened. This module is exactly that layer, built on the grid's own conserving
//! mutators so it inherits their guarantees: every transfer fills existing
//! partial stacks in the destination before using empty slots, leaves whatever
//! doesn't fit in the source, and never creates or destroys an item — the sum of
//! counts across the two grids is invariant. Pure, deterministic, rustc-testable.

use crate::sim::inventory_grid::{InventoryGrid, ItemStack};

/// Move as much of the stack in `src[slot]` as will fit into `dst`, filling
/// existing stacks of the same item first, then empty slots (the destination's
/// own `add` policy). Returns the count actually moved; any remainder stays in
/// `src[slot]`. Moving from an empty slot is a no-op returning 0.
pub fn quick_move_slot(src: &mut InventoryGrid, slot: usize, dst: &mut InventoryGrid) -> u32 {
    let stack = match src.get(slot) {
        Some(s) => s,
        None => return 0,
    };
    let leftover = dst.add(stack);
    let moved = stack.count - leftover;
    if moved == 0 {
        return 0;
    }
    if leftover == 0 {
        src.set(slot, None);
    } else {
        src.set(slot, Some(ItemStack::new(stack.id, leftover)));
    }
    moved
}

/// Quick-move every occupied slot of `src` into `dst`, in slot order ("deposit
/// all" / "loot all"). Returns the total count moved; anything that didn't fit
/// remains in `src`.
pub fn transfer_all(src: &mut InventoryGrid, dst: &mut InventoryGrid) -> u32 {
    let mut moved = 0;
    for slot in 0..src.capacity() {
        moved += quick_move_slot(src, slot, dst);
    }
    moved
}

/// Quick-move only the slots of `src` holding item `id` into `dst`. Returns the
/// total count moved.
pub fn transfer_matching(
    src: &mut InventoryGrid,
    dst: &mut InventoryGrid,
    id: crate::sim::inventory_grid::ItemId,
) -> u32 {
    let mut moved = 0;
    for slot in 0..src.capacity() {
        if src.get(slot).map(|s| s.id) == Some(id) {
            moved += quick_move_slot(src, slot, dst);
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::inventory_grid::{ItemId, ItemStack};

    fn grid(rows: usize, cols: usize) -> InventoryGrid {
        InventoryGrid::new(rows, cols)
    }
    fn stack(id: u16, n: u32) -> ItemStack {
        ItemStack::new(ItemId(id), n)
    }

    #[test]
    fn a_slot_moves_wholesale_into_an_empty_chest() {
        let mut src = grid(1, 3);
        let mut dst = grid(1, 3);
        src.set(0, Some(stack(7, 10)));
        let moved = quick_move_slot(&mut src, 0, &mut dst);
        assert_eq!(moved, 10);
        assert!(src.get(0).is_none(), "source slot emptied");
        assert_eq!(dst.count_of(ItemId(7)), 10);
    }

    #[test]
    fn moving_from_empty_is_a_noop() {
        let mut src = grid(1, 3);
        let mut dst = grid(1, 3);
        assert_eq!(quick_move_slot(&mut src, 1, &mut dst), 0);
        assert_eq!(dst.total(), 0);
    }

    #[test]
    fn a_partial_fit_leaves_the_remainder_in_the_source() {
        // dst can only take a little: one 1×1 grid already near max.
        let mut src = grid(1, 1);
        let mut dst = grid(1, 1);
        let cap = crate::sim::inventory_grid::max_stack(ItemId(3));
        dst.set(0, Some(stack(3, cap - 2))); // room for 2 more of item 3
        src.set(0, Some(stack(3, 10)));
        let moved = quick_move_slot(&mut src, 0, &mut dst);
        assert_eq!(moved, 2, "only the 2 that fit move");
        assert_eq!(src.get(0).unwrap().count, 8, "remainder stays in source");
        assert_eq!(dst.get(0).unwrap().count, cap);
    }

    #[test]
    fn transfer_fills_existing_stacks_before_empty_slots() {
        // dst slot 0 has some of item 5; a fresh deposit should top it up first.
        let mut src = grid(1, 2);
        let mut dst = grid(1, 2);
        dst.set(0, Some(stack(5, 4)));
        src.set(0, Some(stack(5, 3)));
        quick_move_slot(&mut src, 0, &mut dst);
        assert_eq!(dst.get(0).unwrap().count, 7, "topped up the existing stack");
        assert!(dst.get(1).is_none(), "didn't waste an empty slot");
    }

    #[test]
    fn deposit_all_empties_the_source_when_there_is_room() {
        let mut src = grid(2, 2);
        let mut dst = grid(4, 4);
        src.set(0, Some(stack(1, 5)));
        src.set(2, Some(stack(2, 8)));
        src.set(3, Some(stack(1, 3)));
        let before = src.total() + dst.total();
        let moved = transfer_all(&mut src, &mut dst);
        assert_eq!(moved, 16);
        assert_eq!(src.total(), 0, "everything left the source");
        assert_eq!(src.total() + dst.total(), before, "item count conserved");
        assert_eq!(dst.count_of(ItemId(1)), 8);
        assert_eq!(dst.count_of(ItemId(2)), 8);
    }

    #[test]
    fn deposit_all_into_a_tight_chest_conserves_and_keeps_the_overflow() {
        let mut src = grid(2, 2);
        let mut dst = grid(1, 1); // one slot only
        src.set(0, Some(stack(9, 20)));
        src.set(1, Some(stack(4, 5))); // won't fit — different item, no empty slot
        let before = src.total() + dst.total();
        transfer_all(&mut src, &mut dst);
        assert_eq!(src.total() + dst.total(), before, "no items lost or duplicated");
        assert!(dst.count_of(ItemId(9)) > 0, "the first item partially/fully deposited");
        assert_eq!(src.count_of(ItemId(4)), 5, "the item with nowhere to go stays put");
    }

    #[test]
    fn transfer_matching_moves_only_the_requested_item() {
        let mut src = grid(1, 3);
        let mut dst = grid(2, 3);
        src.set(0, Some(stack(1, 4)));
        src.set(1, Some(stack(2, 6)));
        src.set(2, Some(stack(1, 2)));
        let moved = transfer_matching(&mut src, &mut dst, ItemId(1));
        assert_eq!(moved, 6, "both item-1 slots (4+2)");
        assert_eq!(dst.count_of(ItemId(1)), 6);
        assert_eq!(dst.count_of(ItemId(2)), 0, "item 2 untouched");
        assert_eq!(src.count_of(ItemId(2)), 6, "item 2 still in source");
    }

    #[test]
    fn transfer_is_deterministic() {
        let build = || {
            let mut s = grid(2, 2);
            s.set(0, Some(stack(1, 5)));
            s.set(3, Some(stack(2, 7)));
            s
        };
        let (mut s1, mut d1) = (build(), grid(3, 3));
        let (mut s2, mut d2) = (build(), grid(3, 3));
        let m1 = transfer_all(&mut s1, &mut d1);
        let m2 = transfer_all(&mut s2, &mut d2);
        assert_eq!(m1, m2);
        for i in 0..d1.capacity() {
            assert_eq!(d1.get(i), d2.get(i), "slot {i} identical");
        }
    }
}
