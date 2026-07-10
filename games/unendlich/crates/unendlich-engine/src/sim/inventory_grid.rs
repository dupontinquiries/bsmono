//! inventory_grid — a `rows × cols` grid of optional item stacks, the shared
//! backing for chests and the player inventory. Every mutator is total and
//! conserves item count: nothing in this module creates or destroys items, it
//! only shuffles counts between slots (or, for [`InventoryGrid::take`], hands a
//! stack out and removes exactly that many). Stacks never exceed a per-item
//! maximum ([`max_stack`]) and a count-zero slot is always normalised to empty,
//! so "is this slot occupied" and "does it hold >0" are the same question.
//!
//! Pure and rustc-testable in isolation (R1): it owns a placeholder [`max_stack`]
//! table rather than reaching into item config. The conservation and
//! never-exceed-max invariants are pinned by `prop_*` tests so the grid can back
//! a networked, rolled-back inventory without silently leaking items.

/// Item kind id. The real id space comes from item config; here it is just a
/// handle the grid groups and stacks by.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u16);

/// A non-empty pile of one item kind. Invariant maintained by this module:
/// `count >= 1` and `count <= max_stack(id)` for any stack stored in a grid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemStack {
    pub id: ItemId,
    pub count: u32,
}

impl ItemStack {
    pub fn new(id: ItemId, count: u32) -> ItemStack {
        ItemStack { id, count }
    }
}

/// Maximum stack size for an item kind. Placeholder rule until item config wires
/// in real limits: ids `>= 200` are tools/armour/weapons (stack to 1), ids in
/// `100..200` are mid-stack throwables/pearls (16), everything else stacks to 64.
/// Never returns 0 — a zero max would make [`InventoryGrid::add`] unable to make
/// progress.
pub fn max_stack(id: ItemId) -> u32 {
    match id.0 {
        v if v >= 200 => 1,
        v if v >= 100 => 16,
        _ => 64,
    }
}

/// A fixed-size grid of optional stacks. Slots are addressed by a flat index
/// `0..rows*cols`; [`InventoryGrid::index`] converts `(row, col)`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InventoryGrid {
    rows: usize,
    cols: usize,
    slots: Vec<Option<ItemStack>>,
}

impl InventoryGrid {
    /// An empty `rows × cols` grid.
    pub fn new(rows: usize, cols: usize) -> InventoryGrid {
        InventoryGrid {
            rows,
            cols,
            slots: vec![None; rows * cols],
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }
    pub fn cols(&self) -> usize {
        self.cols
    }
    /// Number of slots (`rows * cols`).
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Flat slot index for `(row, col)`, or `None` if out of range.
    pub fn index(&self, row: usize, col: usize) -> Option<usize> {
        if row < self.rows && col < self.cols {
            Some(row * self.cols + col)
        } else {
            None
        }
    }

    /// The stack at a slot (copied out), or `None` if empty / out of range.
    pub fn get(&self, slot: usize) -> Option<ItemStack> {
        match self.slots.get(slot) {
            Some(s) => *s,
            None => None,
        }
    }

    /// Directly place (or clear) a slot — for setup. A `Some(stack)` is clamped
    /// to the item's max and a zero count is normalised to empty. Returns false
    /// if the slot is out of range.
    pub fn set(&mut self, slot: usize, stack: Option<ItemStack>) -> bool {
        if slot >= self.slots.len() {
            return false;
        }
        self.slots[slot] = match stack {
            Some(s) if s.count == 0 => None,
            Some(s) => Some(ItemStack {
                id: s.id,
                count: s.count.min(max_stack(s.id)),
            }),
            None => None,
        };
        true
    }

    /// Total item count across every slot.
    pub fn total(&self) -> u64 {
        self.slots
            .iter()
            .map(|s| s.map_or(0u64, |st| st.count as u64))
            .sum()
    }

    /// Total count of a single item kind.
    pub fn count_of(&self, id: ItemId) -> u64 {
        self.slots
            .iter()
            .map(|s| match s {
                Some(st) if st.id == id => st.count as u64,
                _ => 0,
            })
            .sum()
    }

    /// True if no slot is empty (a fresh stack can't be placed; partial same-id
    /// slots may still accept a top-up via [`InventoryGrid::add`]).
    pub fn is_full(&self) -> bool {
        self.slots.iter().all(|s| s.is_some())
    }

    /// Insert `incoming`, topping up existing same-id stacks first and then
    /// filling empty slots, every stack capped at [`max_stack`]. Returns the
    /// leftover count that didn't fit (0 = fully placed). Adding into a grid with
    /// no room leaves it unchanged and returns the full incoming count.
    pub fn add(&mut self, incoming: ItemStack) -> u32 {
        let mut remaining = incoming.count;
        if remaining == 0 {
            return 0;
        }
        let cap = max_stack(incoming.id);
        // Pass 1: top up partial stacks of the same kind.
        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                break;
            }
            if let Some(s) = slot {
                if s.id == incoming.id && s.count < cap {
                    let moved = (cap - s.count).min(remaining);
                    s.count += moved;
                    remaining -= moved;
                }
            }
        }
        // Pass 2: open new stacks in empty slots.
        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                break;
            }
            if slot.is_none() {
                let moved = cap.min(remaining);
                *slot = Some(ItemStack {
                    id: incoming.id,
                    count: moved,
                });
                remaining -= moved;
            }
        }
        remaining
    }

    fn both_in_range(&self, a: usize, b: usize) -> bool {
        a < self.slots.len() && b < self.slots.len()
    }

    /// Move the whole stack from `from` into an EMPTY `to`. No-op (returns false)
    /// if `from == to`, either index is out of range, `from` is empty, or `to` is
    /// occupied. Conserves total.
    pub fn move_to(&mut self, from: usize, to: usize) -> bool {
        if from == to || !self.both_in_range(from, to) {
            return false;
        }
        let moving = match self.slots[from] {
            Some(s) => s,
            None => return false,
        };
        if self.slots[to].is_some() {
            return false;
        }
        self.slots[to] = Some(moving);
        self.slots[from] = None;
        true
    }

    /// Swap the contents of two slots (either or both may be empty). No-op
    /// (returns false) only if an index is out of range. Conserves total.
    pub fn swap(&mut self, a: usize, b: usize) -> bool {
        if !self.both_in_range(a, b) {
            return false;
        }
        self.slots.swap(a, b);
        true
    }

    /// Stack `from` onto `to`: if both hold the same kind, move as much as fits
    /// (up to `to`'s remaining room), leaving any overflow in `from`. Returns
    /// false if indices are bad, kinds differ, a slot is empty, or `to` is
    /// already full. Conserves total.
    pub fn stack_onto(&mut self, from: usize, to: usize) -> bool {
        if from == to || !self.both_in_range(from, to) {
            return false;
        }
        let src = match self.slots[from] {
            Some(s) => s,
            None => return false,
        };
        let dst = match self.slots[to] {
            Some(s) => s,
            None => return false,
        };
        if src.id != dst.id {
            return false;
        }
        let cap = max_stack(dst.id);
        if dst.count >= cap {
            return false;
        }
        let moved = (cap - dst.count).min(src.count);
        self.slots[to] = Some(ItemStack {
            id: dst.id,
            count: dst.count + moved,
        });
        let left = src.count - moved;
        self.slots[from] = if left == 0 {
            None
        } else {
            Some(ItemStack {
                id: src.id,
                count: left,
            })
        };
        true
    }

    /// Move up to `n` units from `from` into `to`, where `to` is empty or holds
    /// the same kind, respecting [`max_stack`]. Returns false if nothing could
    /// move (bad indices, `from` empty, kind mismatch, `to` full, or `n == 0`).
    /// Conserves total — items stay in the grid.
    pub fn split(&mut self, from: usize, to: usize, n: u32) -> bool {
        if n == 0 || from == to || !self.both_in_range(from, to) {
            return false;
        }
        let src = match self.slots[from] {
            Some(s) => s,
            None => return false,
        };
        let want = n.min(src.count);
        let moved = match self.slots[to] {
            None => want.min(max_stack(src.id)),
            Some(dst) => {
                if dst.id != src.id {
                    return false;
                }
                let cap = max_stack(dst.id);
                if dst.count >= cap {
                    return false;
                }
                want.min(cap - dst.count)
            }
        };
        if moved == 0 {
            return false;
        }
        let dst_count = self.slots[to].map_or(0, |d| d.count);
        self.slots[to] = Some(ItemStack {
            id: src.id,
            count: dst_count + moved,
        });
        let left = src.count - moved;
        self.slots[from] = if left == 0 {
            None
        } else {
            Some(ItemStack {
                id: src.id,
                count: left,
            })
        };
        true
    }

    /// Remove up to `n` units from a slot and return them as a stack (the
    /// "pick up" op). Returns `None` if the slot is empty/out of range or `n == 0`.
    /// The grid's total drops by exactly the returned count, so
    /// `grid.total() + taken.count` equals the pre-call total.
    pub fn take(&mut self, slot: usize, n: u32) -> Option<ItemStack> {
        if n == 0 || slot >= self.slots.len() {
            return None;
        }
        let src = match self.slots[slot] {
            Some(s) => s,
            None => return None,
        };
        let taken = n.min(src.count);
        let left = src.count - taken;
        self.slots[slot] = if left == 0 {
            None
        } else {
            Some(ItemStack {
                id: src.id,
                count: left,
            })
        };
        Some(ItemStack {
            id: src.id,
            count: taken,
        })
    }

    /// Merge scattered partial stacks of the same item into fuller stacks (up to
    /// [`max_stack`]), packing counts toward earlier slots and freeing later ones.
    /// Returns the number of slots freed. Conserves the total count of every item
    /// kind and is order-stable (earlier slots fill first). This is the "tidy" /
    /// auto-stack operation a UI button or a pickup path wants after many small
    /// additions have fragmented the grid.
    pub fn consolidate(&mut self) -> u32 {
        let n = self.slots.len();
        let mut freed = 0u32;
        for i in 0..n {
            let (id, mut count) = match self.slots[i] {
                Some(s) => (s.id, s.count),
                None => continue,
            };
            let cap = max_stack(id);
            if count >= cap {
                continue; // already full — nothing to pull in
            }
            for j in (i + 1)..n {
                if count >= cap {
                    break;
                }
                if let Some(s) = self.slots[j] {
                    if s.id != id {
                        continue;
                    }
                    let moved = (cap - count).min(s.count);
                    count += moved;
                    let left = s.count - moved;
                    if left == 0 {
                        self.slots[j] = None;
                        freed += 1;
                    } else {
                        self.slots[j] = Some(ItemStack { id, count: left });
                    }
                }
            }
            self.slots[i] = Some(ItemStack { id, count });
        }
        freed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    fn id(v: u16) -> ItemId {
        ItemId(v)
    }

    /// A grid with each non-empty slot holding a valid (1..=max) stack, seeded.
    fn random_grid(seed: &mut u32, rows: usize, cols: usize) -> InventoryGrid {
        let mut g = InventoryGrid::new(rows, cols);
        let kinds = [id(1), id(2), id(105), id(210)];
        for i in 0..g.capacity() {
            if lcg(seed) % 3 != 0 {
                let k = kinds[(lcg(seed) as usize) % kinds.len()];
                let cap = max_stack(k);
                let c = 1 + lcg(seed) % cap;
                g.set(i, Some(ItemStack::new(k, c)));
            }
        }
        g
    }

    fn invariants_hold(g: &InventoryGrid) -> bool {
        (0..g.capacity()).all(|i| match g.get(i) {
            Some(s) => s.count >= 1 && s.count <= max_stack(s.id),
            None => true,
        })
    }

    #[test]
    fn add_merges_then_fills_then_overflows() {
        let mut g = InventoryGrid::new(1, 2); // two slots, cap 64 for id(1)
        assert_eq!(g.add(ItemStack::new(id(1), 30)), 0);
        assert_eq!(g.add(ItemStack::new(id(1), 50)), 0); // 30->64 (top up), 16 into slot 2
        assert_eq!(g.total(), 80);
        // both slots: 64 + 16; now overflow 100 -> fits 48 more (64-16=48), 52 leftover
        assert_eq!(g.add(ItemStack::new(id(1), 100)), 52);
        assert_eq!(g.total(), 128);
        assert!(invariants_hold(&g));
    }

    #[test]
    fn add_into_full_grid_is_clean() {
        let mut g = InventoryGrid::new(1, 2);
        g.set(0, Some(ItemStack::new(id(1), 64)));
        g.set(1, Some(ItemStack::new(id(1), 64)));
        let before = g.total();
        // different kind, no empty slot, no same-kind room -> all leftover, unchanged
        let leftover = g.add(ItemStack::new(id(2), 10));
        assert_eq!(leftover, 10);
        assert_eq!(g.total(), before);
    }

    #[test]
    fn move_swap_stack_split_behaviour() {
        let mut g = InventoryGrid::new(1, 3);
        g.set(0, Some(ItemStack::new(id(1), 20)));
        g.set(2, Some(ItemStack::new(id(1), 50)));

        assert!(g.move_to(0, 1)); // into empty
        assert_eq!(g.get(0), None);
        assert_eq!(g.get(1), Some(ItemStack::new(id(1), 20)));
        assert!(!g.move_to(1, 2)); // dest occupied -> refused

        assert!(g.swap(1, 0));
        assert_eq!(g.get(0), Some(ItemStack::new(id(1), 20)));

        // stack 20 onto 50 -> 64 + 6 leftover
        assert!(g.stack_onto(0, 2));
        assert_eq!(g.get(2), Some(ItemStack::new(id(1), 64)));
        assert_eq!(g.get(0), Some(ItemStack::new(id(1), 6)));

        // split 4 from slot 0 into empty slot 1
        assert!(g.split(0, 1, 4));
        assert_eq!(g.get(0), Some(ItemStack::new(id(1), 2)));
        assert_eq!(g.get(1), Some(ItemStack::new(id(1), 4)));
    }

    #[test]
    fn take_extracts_and_clears() {
        let mut g = InventoryGrid::new(1, 1);
        g.set(0, Some(ItemStack::new(id(7), 5)));
        let before = g.total();
        let taken = g.take(0, 2);
        assert_eq!(taken, Some(ItemStack::new(id(7), 2)));
        assert_eq!(g.total() + 2, before);
        let rest = g.take(0, 99);
        assert_eq!(rest, Some(ItemStack::new(id(7), 3)));
        assert_eq!(g.get(0), None);
        assert_eq!(g.take(0, 1), None); // empty now
    }

    #[test]
    fn out_of_range_ops_are_no_ops() {
        let mut g = InventoryGrid::new(2, 2);
        g.set(0, Some(ItemStack::new(id(1), 5)));
        assert!(!g.move_to(0, 99));
        assert!(!g.swap(0, 99));
        assert!(!g.stack_onto(0, 99));
        assert!(!g.split(0, 99, 1));
        assert_eq!(g.take(99, 1), None);
        assert_eq!(g.total(), 5); // untouched
    }

    #[test]
    fn prop_ingrid_ops_conserve_total() {
        let mut s = 0xBEEF_0001u32;
        for _ in 0..2000 {
            let mut g = random_grid(&mut s, 3, 3);
            let before = g.total();
            for _ in 0..20 {
                let a = (lcg(&mut s) as usize) % g.capacity();
                let b = (lcg(&mut s) as usize) % g.capacity();
                match lcg(&mut s) % 4 {
                    0 => {
                        g.move_to(a, b);
                    }
                    1 => {
                        g.swap(a, b);
                    }
                    2 => {
                        g.stack_onto(a, b);
                    }
                    _ => {
                        g.split(a, b, 1 + lcg(&mut s) % 40);
                    }
                }
                assert_eq!(g.total(), before, "in-grid op changed total");
                assert!(invariants_hold(&g), "stack exceeded max or held 0");
            }
        }
    }

    #[test]
    fn prop_take_conserves_with_returned() {
        let mut s = 0xBEEF_0002u32;
        for _ in 0..3000 {
            let mut g = random_grid(&mut s, 2, 3);
            let before = g.total();
            let slot = (lcg(&mut s) as usize) % g.capacity();
            let n = 1 + lcg(&mut s) % 80;
            let taken = g.take(slot, n).map_or(0u64, |st| st.count as u64);
            assert_eq!(g.total() + taken, before);
            assert!(invariants_hold(&g));
        }
    }

    #[test]
    fn prop_add_never_exceeds_max_and_is_deterministic() {
        let mut s = 0xBEEF_0003u32;
        let kinds = [id(1), id(2), id(150), id(255)];
        for _ in 0..3000 {
            let rows = 1 + (lcg(&mut s) as usize) % 3;
            let cols = 1 + (lcg(&mut s) as usize) % 3;
            let mut g1 = InventoryGrid::new(rows, cols);
            let mut g2 = InventoryGrid::new(rows, cols);
            let mut leftovers1 = 0u32;
            let mut leftovers2 = 0u32;
            for _ in 0..8 {
                let k = kinds[(lcg(&mut s) as usize) % kinds.len()];
                let c = 1 + lcg(&mut s) % 200;
                leftovers1 += g1.add(ItemStack::new(k, c));
                leftovers2 += g2.add(ItemStack::new(k, c));
            }
            // never-exceed-max
            assert!(invariants_hold(&g1));
            // determinism: identical inputs -> identical grids and leftovers
            assert_eq!(g1, g2);
            assert_eq!(leftovers1, leftovers2);
        }
    }

    #[test]
    fn consolidate_merges_partials_and_conserves() {
        let mut g = InventoryGrid::new(1, 4);
        // Three partial stacks of id 1 (max 64): 40 + 30 + 20 = 90 → one 64 + one 26.
        g.set(0, Some(ItemStack::new(id(1), 40)));
        g.set(1, Some(ItemStack::new(id(1), 30)));
        g.set(2, Some(ItemStack::new(id(1), 20)));
        let before = g.total();
        let freed = g.consolidate();
        assert_eq!(g.total(), before, "count conserved");
        assert_eq!(freed, 1, "three partials → two stacks, one slot freed");
        assert_eq!(g.get(0), Some(ItemStack::new(id(1), 64)));
        assert_eq!(g.get(1), Some(ItemStack::new(id(1), 26)));
        assert_eq!(g.get(2), None);
        assert!(invariants_hold(&g));
    }

    #[test]
    fn consolidate_leaves_distinct_ids_alone() {
        let mut g = InventoryGrid::new(1, 3);
        g.set(0, Some(ItemStack::new(id(1), 10)));
        g.set(1, Some(ItemStack::new(id(2), 10)));
        g.set(2, Some(ItemStack::new(id(1), 10)));
        let freed = g.consolidate();
        assert_eq!(freed, 1); // the two id(1) stacks merge; id(2) untouched
        assert_eq!(g.get(0), Some(ItemStack::new(id(1), 20)));
        assert_eq!(g.get(1), Some(ItemStack::new(id(2), 10)));
        assert_eq!(g.count_of(id(1)), 20);
    }

    #[test]
    fn consolidate_never_stacks_unstackables() {
        // ids >= 200 stack to 1 — two of them must stay in separate slots.
        let mut g = InventoryGrid::new(1, 2);
        g.set(0, Some(ItemStack::new(id(210), 1)));
        g.set(1, Some(ItemStack::new(id(210), 1)));
        let freed = g.consolidate();
        assert_eq!(freed, 0);
        assert_eq!(g.count_of(id(210)), 2);
    }

    #[test]
    fn consolidate_conserves_on_random_grids() {
        let mut s = 0xC0FFEEu32;
        for _ in 0..200 {
            let mut g = random_grid(&mut s, 3, 4);
            let before = g.total();
            g.consolidate();
            assert_eq!(g.total(), before);
            assert!(invariants_hold(&g));
            // Idempotent: a second pass frees nothing.
            assert_eq!(g.consolidate(), 0);
        }
    }
}
