//! `sim::inventory_txn` — atomic multi-slot inventory transactions.
//!
//! Composes the slot primitives in `crate::inventory` (`ItemStack::{merge,split,
//! space}`, `SlotGrid::{get,set,add,take,swap,total,remove,len}`) into
//! multi-slot operations applied **all-or-nothing**: quick-move (shift-click)
//! across regions, split-evenly across N slots, consolidate/auto-stack, and
//! equip-swap. Each returns enough accounting to reverse it. This does NOT
//! reimplement sorting (`sim::inventory_sort`) or crafting (`sim::crafting` /
//! `craft_resolve`) — it only re-sequences the existing slot ops as
//! transactions.
//!
//! Regions are a slice of mutable `SlotGrid`s (e.g. `[&mut inv.hotbar.grid,
//! &mut inv.main, &mut container]`); an [`Addr`] is `(region, slot)`. Pure:
//! data in → data out, no IO/clock/RNG, no `unwrap`/`expect` in non-test code
//! (R1). Release-safe (no `#[cfg(debug_assertions)]` helpers).

use crate::inventory::{ItemStack, SlotGrid};
use crate::world::blocks::BlockId;

/// A slot address: which region, which slot within it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Addr {
    pub region: usize,
    pub slot: usize,
}

impl Addr {
    pub fn new(region: usize, slot: usize) -> Addr {
        Addr { region, slot }
    }
}

/// Result of [`quick_move`] — enough to undo via [`undo_quick_move`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Moved {
    pub from: Addr,
    pub item: BlockId,
    /// How many items actually left `from` and landed in the targets.
    pub total_moved: u16,
    /// How many stayed at `from` because the targets were full.
    pub leftover: u16,
    /// Target regions, in the priority order they were filled.
    pub targets: Vec<usize>,
}

impl Moved {
    fn none(from: Addr, item: BlockId) -> Moved {
        Moved { from, item, total_moved: 0, leftover: 0, targets: Vec::new() }
    }
    /// Did anything move?
    pub fn moved_any(&self) -> bool {
        self.total_moved > 0
    }
}

/// Result of [`split_even`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SplitResult {
    pub applied: bool,
    /// Per-slot share counts written (length `n` when applied).
    pub shares: Vec<u16>,
    /// Slots that received a share (the source slot first, then the empties).
    pub slots: Vec<usize>,
}

// ─── snapshot / restore (atomicity) ─────────────────────────────────
fn snapshot(regions: &[&mut SlotGrid]) -> Vec<SlotGrid> {
    regions.iter().map(|g| (**g).clone()).collect()
}
fn restore(regions: &mut [&mut SlotGrid], snap: Vec<SlotGrid>) {
    for (g, s) in regions.iter_mut().zip(snap.into_iter()) {
        **g = s;
    }
}

/// Shift-click: pull the whole stack out of `from` and pour it into the
/// `targets` regions in priority order (each tops up matching stacks, then fills
/// empties — `SlotGrid::add`'s policy). Any remainder is put back at `from`.
/// Item count is conserved. Returns the accounting; never partially corrupts.
pub fn quick_move(regions: &mut [&mut SlotGrid], from: Addr, targets: &[usize]) -> Moved {
    // Read source.
    let src = match regions.get(from.region) {
        Some(g) => g.get(from.slot),
        None => return Moved::none(from, BlockId::Air),
    };
    let src = match src {
        Some(s) if !s.is_empty() => s,
        _ => return Moved::none(from, BlockId::Air),
    };
    let item = src.item;

    // Lift the whole stack out of `from`.
    let taken = match regions[from.region].take(from.slot, u16::MAX) {
        Some(t) => t,
        None => return Moved::none(from, item),
    };
    let total = taken.count;
    let mut remaining = total;

    // Pour into each target region in order (fills then spills to the next).
    let mut used_targets: Vec<usize> = Vec::new();
    for &t in targets {
        if remaining == 0 {
            break;
        }
        if t >= regions.len() || t == from.region {
            // skip out-of-range and the source region itself (a quick-move
            // moves *between* regions; self-target would just no-op the take)
            if t == from.region {
                used_targets.push(t);
            }
            continue;
        }
        let leftover = regions[t].add(ItemStack { item, count: remaining });
        if leftover < remaining {
            used_targets.push(t);
        }
        remaining = leftover;
    }

    // Put any remainder back where it came from.
    if remaining > 0 {
        regions[from.region].set(from.slot, Some(ItemStack { item, count: remaining }));
    }

    Moved {
        from,
        item,
        total_moved: total - remaining,
        leftover: remaining,
        targets: used_targets,
    }
}

/// Reverse a [`quick_move`]: remove the moved items from the targets and return
/// them to `from`. Restores per-item totals and the source slot's contents
/// (exact target slot layout is not guaranteed — totals are).
pub fn undo_quick_move(regions: &mut [&mut SlotGrid], m: &Moved) {
    if m.total_moved == 0 {
        return;
    }
    let mut to_return = 0u16;
    for &t in &m.targets {
        if to_return >= m.total_moved {
            break;
        }
        if let Some(g) = regions.get_mut(t) {
            let want = m.total_moved - to_return;
            to_return += g.remove(m.item, want);
        }
    }
    if to_return > 0 {
        if let Some(g) = regions.get_mut(m.from.region) {
            // back to the source slot first, spilling within that region.
            let existing = g.get(m.from.slot);
            match existing {
                Some(s) if s.item == m.item => {
                    let mut s = s;
                    let spill = s.merge(to_return);
                    g.set(m.from.slot, Some(s));
                    if spill > 0 {
                        let _ = g.add(ItemStack { item: m.item, count: spill });
                    }
                }
                None => {
                    g.set(m.from.slot, Some(ItemStack { item: m.item, count: to_return }));
                }
                Some(_) => {
                    let _ = g.add(ItemStack { item: m.item, count: to_return });
                }
            }
        }
    }
}

/// Split the stack at `at` evenly across `n` slots in the same region: the
/// source slot keeps the first share, and `n-1` empty slots in that region take
/// the rest. The remainder (`count % n`) is spread one-each over the first
/// shares. Atomic: if there aren't `n-1` empty slots, nothing changes and
/// `applied == false`.
pub fn split_even(regions: &mut [&mut SlotGrid], at: Addr, n: u16) -> SplitResult {
    let fail = SplitResult { applied: false, shares: Vec::new(), slots: Vec::new() };
    if n == 0 || at.region >= regions.len() {
        return fail;
    }
    let src = match regions[at.region].get(at.slot) {
        Some(s) if !s.is_empty() => s,
        _ => return fail,
    };
    let item = src.item;
    let total = src.count;

    // even shares + remainder spread over the first `rem` slots
    let base = total / n;
    let rem = total % n;
    let mut shares: Vec<u16> = (0..n).map(|i| base + if i < rem { 1 } else { 0 }).collect();
    // a zero total or n>total leaves trailing zero shares — drop empty shares,
    // they need no slot.
    while matches!(shares.last(), Some(&0)) {
        shares.pop();
    }
    if shares.is_empty() {
        return fail;
    }
    let need_extra = shares.len() - 1; // besides the source slot

    let snap = snapshot(regions);

    // gather empty slots in the region (excluding the source slot)
    let mut empties: Vec<usize> = Vec::new();
    {
        let g = &regions[at.region];
        for i in 0..g.len() {
            if i != at.slot && g.get(i).is_none() {
                empties.push(i);
                if empties.len() == need_extra {
                    break;
                }
            }
        }
    }
    if empties.len() < need_extra {
        // not enough room — all-or-nothing
        return fail;
    }

    // apply: source slot gets shares[0], empties get the rest
    let mut slots: Vec<usize> = Vec::with_capacity(shares.len());
    let g = &mut regions[at.region];
    g.set(at.slot, Some(ItemStack { item, count: shares[0] }));
    slots.push(at.slot);
    for (k, &si) in empties.iter().enumerate() {
        g.set(si, Some(ItemStack { item, count: shares[k + 1] }));
        slots.push(si);
    }

    // conservation guard: shares must sum to the original total
    let sum: u32 = shares.iter().map(|&x| x as u32).sum();
    if sum != total as u32 {
        restore(regions, snap);
        return fail;
    }
    SplitResult { applied: true, shares, slots }
}

/// Auto-stack within every region: merge partial stacks of the same item and
/// compact toward the front (re-runs `SlotGrid::add`'s top-up-then-fill on each
/// region's own contents). Item counts are conserved. Returns the number of
/// occupied slots freed (>= 0).
pub fn consolidate(regions: &mut [&mut SlotGrid]) -> usize {
    let mut freed = 0usize;
    for g in regions.iter_mut() {
        let n = g.len();
        let before = (0..n).filter(|&i| g.get(i).is_some()).count();
        // drain
        let mut stacks: Vec<ItemStack> = Vec::new();
        for i in 0..n {
            if let Some(s) = g.get(i) {
                stacks.push(s);
            }
            g.set(i, None);
        }
        // re-add (tops up matching, then fills empties → consolidated + compact)
        for s in stacks {
            let _leftover = g.add(s); // capacity is unchanged, so it all fits back
        }
        let after = (0..n).filter(|&i| g.get(i).is_some()).count();
        freed += before.saturating_sub(after);
    }
    freed
}

/// Equip-swap: swap the contents of `slot_a` and `equip_b`, but only if the item
/// currently at `slot_a` is accepted by the equip slot (`accepts(item)`); an
/// empty `slot_a` (un-equip) is always allowed. Atomic: on rejection nothing
/// changes and it returns `false`. Item counts are conserved.
pub fn equip_swap(
    regions: &mut [&mut SlotGrid],
    slot_a: Addr,
    equip_b: Addr,
    accepts: impl Fn(BlockId) -> bool,
) -> bool {
    if slot_a.region >= regions.len() || equip_b.region >= regions.len() {
        return false;
    }
    let a_stack = regions[slot_a.region].get(slot_a.slot);
    // moving a's item into the equip slot must be accepted (empty = un-equip, ok)
    if let Some(s) = a_stack {
        if !s.is_empty() && !accepts(s.item) {
            return false;
        }
    }
    let snap = snapshot(regions);
    let b_stack = regions[equip_b.region].get(equip_b.slot);

    // same-region swap can use SlotGrid::swap; cross-region does a set/set swap.
    if slot_a.region == equip_b.region {
        regions[slot_a.region].swap(slot_a.slot, equip_b.slot);
    } else {
        regions[slot_a.region].set(slot_a.slot, b_stack);
        regions[equip_b.region].set(equip_b.slot, a_stack);
    }

    // guard (defensive; the ops above can't lose items, but keep it atomic)
    let conserved = total_all(regions) == total_all_snap(&snap);
    if !conserved {
        restore(regions, snap);
        return false;
    }
    true
}

// ─── conservation helpers (unconditional — release-safe) ─────────────
fn grid_total(g: &SlotGrid) -> u32 {
    (0..g.len()).filter_map(|i| g.get(i)).map(|s| s.count as u32).sum()
}
fn total_all(regions: &[&mut SlotGrid]) -> u32 {
    regions.iter().map(|g| grid_total(g)).sum()
}
fn total_all_snap(snap: &[SlotGrid]) -> u32 {
    snap.iter().map(grid_total).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The delivered module imports crate::inventory + world::blocks. For the
    // standalone `rustc --test` self-check, the harness file
    // (inventory_txn_standalone_check.rs) mirrors ItemStack/SlotGrid/BlockId
    // verbatim from src/inventory.rs and runs THESE tests against them, proving
    // the transaction logic against the real primitive semantics. The
    // coordinator's `cargo build` is the in-crate gate.
    use crate::world::blocks::BlockId;

    fn st(item: BlockId, n: u16) -> ItemStack {
        ItemStack::new(item, n)
    }
    fn total2(a: &SlotGrid, b: &SlotGrid) -> u32 {
        grid_total(a) + grid_total(b)
    }

    #[test]
    fn quick_move_fills_then_spills_and_conserves() {
        let mut hot = SlotGrid::new(2);
        let mut main = SlotGrid::new(4);
        // 100 stone at hotbar slot 0; MAX_STACK 64 ⇒ target hotbar slot can't
        // hold a 2nd stone stack here (it's the source region), so move to main.
        hot.set(0, Some(st(BlockId::Stone, 100)));
        let before = total2(&hot, &main);
        let m = {
            let mut regions: [&mut SlotGrid; 2] = [&mut hot, &mut main];
            quick_move(&mut regions, Addr::new(0, 0), &[1])
        };
        assert_eq!(m.total_moved, 100, "all 100 should move to main");
        assert_eq!(total2(&hot, &main), before, "count conserved");
        // main now holds 64 + 36 across two slots (add fills then spills)
        assert_eq!(main.total(BlockId::Stone), 100);
        assert!(hot.get(0).is_none(), "source emptied");
    }

    #[test]
    fn quick_move_spills_back_when_targets_full() {
        let mut hot = SlotGrid::new(1);
        let mut main = SlotGrid::new(1);
        hot.set(0, Some(st(BlockId::Stone, 50)));
        main.set(0, Some(st(BlockId::Stone, 40))); // room for 24 only
        let before = total2(&hot, &main);
        let m = {
            let mut regions: [&mut SlotGrid; 2] = [&mut hot, &mut main];
            quick_move(&mut regions, Addr::new(0, 0), &[1])
        };
        assert_eq!(m.total_moved, 24);
        assert_eq!(m.leftover, 26);
        assert_eq!(hot.get(0).map(|s| s.count), Some(26), "remainder back at source");
        assert_eq!(total2(&hot, &main), before, "conserved");
    }

    #[test]
    fn undo_quick_move_restores_totals() {
        let mut hot = SlotGrid::new(2);
        let mut main = SlotGrid::new(4);
        hot.set(0, Some(st(BlockId::Dirt, 30)));
        let before_hot = hot.total(BlockId::Dirt);
        let m = {
            let mut regions: [&mut SlotGrid; 2] = [&mut hot, &mut main];
            quick_move(&mut regions, Addr::new(0, 0), &[1])
        };
        assert!(m.moved_any());
        {
            let mut regions: [&mut SlotGrid; 2] = [&mut hot, &mut main];
            undo_quick_move(&mut regions, &m);
        }
        assert_eq!(hot.total(BlockId::Dirt), before_hot, "undo returns items to source");
        assert_eq!(main.total(BlockId::Dirt), 0, "targets cleared by undo");
    }

    #[test]
    fn split_even_remainder_and_conservation() {
        let mut g = SlotGrid::new(4);
        g.set(0, Some(st(BlockId::Stone, 10))); // 10 over 3 → 4,3,3
        let r = {
            let mut regions: [&mut SlotGrid; 1] = [&mut g];
            split_even(&mut regions, Addr::new(0, 0), 3)
        };
        assert!(r.applied);
        assert_eq!(r.shares, vec![4, 3, 3]);
        assert_eq!(grid_total(&g), 10, "conserved");
        let placed: u32 = r.slots.iter().filter_map(|&i| g.get(i)).map(|s| s.count as u32).sum();
        assert_eq!(placed, 10);
    }

    #[test]
    fn split_even_atomic_rollback_when_no_room() {
        let mut g = SlotGrid::new(2); // only 1 empty besides source
        g.set(0, Some(st(BlockId::Stone, 9)));
        let before = g.clone();
        let r = {
            let mut regions: [&mut SlotGrid; 1] = [&mut g];
            split_even(&mut regions, Addr::new(0, 0), 3) // needs 2 empties, has 1
        };
        assert!(!r.applied, "should fail — not enough empty slots");
        assert_eq!(g, before, "atomic: nothing changed on failure");
    }

    #[test]
    fn consolidate_merges_and_conserves() {
        let mut g = SlotGrid::new(5);
        g.set(0, Some(st(BlockId::Stone, 20)));
        g.set(1, Some(st(BlockId::Dirt, 5)));
        g.set(2, Some(st(BlockId::Stone, 30)));
        g.set(3, Some(st(BlockId::Stone, 10)));
        let before = grid_total(&g);
        let freed = {
            let mut regions: [&mut SlotGrid; 1] = [&mut g];
            consolidate(&mut regions)
        };
        assert_eq!(grid_total(&g), before, "conserved");
        // 60 stone → one full 64? no, 60 < 64 ⇒ one slot of 60; dirt one slot.
        assert_eq!(g.total(BlockId::Stone), 60);
        assert_eq!(g.total(BlockId::Dirt), 5);
        assert!(freed >= 1, "stone stacks consolidated, freeing slots");
    }

    #[test]
    fn equip_swap_accepts_rejects_and_conserves() {
        let mut inv = SlotGrid::new(2);
        let mut equip = SlotGrid::new(1);
        inv.set(0, Some(st(BlockId::Stone, 1)));
        let before = grid_total(&inv) + grid_total(&equip);
        // reject: equip slot only accepts Dirt
        let ok = {
            let mut regions: [&mut SlotGrid; 2] = [&mut inv, &mut equip];
            equip_swap(&mut regions, Addr::new(0, 0), Addr::new(1, 0), |b| b == BlockId::Dirt)
        };
        assert!(!ok, "stone rejected by a dirt-only equip slot");
        assert_eq!(inv.get(0).map(|s| s.item), Some(BlockId::Stone), "unchanged on reject");
        // accept: equip slot accepts Stone
        let ok2 = {
            let mut regions: [&mut SlotGrid; 2] = [&mut inv, &mut equip];
            equip_swap(&mut regions, Addr::new(0, 0), Addr::new(1, 0), |b| b == BlockId::Stone)
        };
        assert!(ok2);
        assert_eq!(equip.get(0).map(|s| s.item), Some(BlockId::Stone));
        assert_eq!(grid_total(&inv) + grid_total(&equip), before, "conserved");
    }
}
