//! Inventory **display sort** ordering (R38, INV) — the order the UI shows
//! items in. R33 `stack_deposit` / `inventory_ops` handle stacking and
//! movement; this is the separate (previously absent) display-order core.
//!
//! Sort key, applied to non-empty slots: **category ascending**, then
//! **rarity descending** (rarest first within a category), then **name_key
//! ascending** (interned display name), then **item_id ascending** as a fully
//! deterministic final tiebreak. Empty slots (`item_id == 0`) always sort to
//! the END. Ties on the full key preserve the slots' original order (stable).
//!
//! Mobile: this sorts an **index permutation** into `out: Vec<u32>` and never
//! moves or copies the `Slot` structs. The UI reorders by indirection, so
//! re-sorting a large bag on an 8GB mobile SoC costs only a `u32` array shuffle
//! — not item-data churn — which matters on a memory-bandwidth-bound device.
//! No allocation beyond `out` (the permutation is built and sorted in place).
//!
//! Pure & deterministic: same slots → same order, every call. No RNG, I/O, GPU,
//! wall-clock; never panics.

use core::cmp::Ordering;

/// One inventory slot. `item_id == 0` marks an empty slot. `name_key` is an
/// interned display-name id (not the name text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub item_id: u32,
    pub category: u8,
    pub rarity: u8,
    pub quantity: u32,
    pub name_key: u32,
}

/// Total ordering between two slots for display. Defined unconditionally (used
/// on the normal sort path, including release builds).
///
/// `Equal` is returned only when the full key matches; a stable sort then keeps
/// such slots in their original relative order.
fn slot_cmp(a: &Slot, b: &Slot) -> Ordering {
    let a_empty = a.item_id == 0;
    let b_empty = b.item_id == 0;
    match (a_empty, b_empty) {
        (true, true) => Ordering::Equal,    // both empty → stable keeps input order
        (true, false) => Ordering::Greater, // empties sink to the end
        (false, true) => Ordering::Less,
        (false, false) => a
            .category
            .cmp(&b.category) // category ascending
            .then_with(|| b.rarity.cmp(&a.rarity)) // rarity DESCENDING
            .then_with(|| a.name_key.cmp(&b.name_key)) // name ascending
            .then_with(|| a.item_id.cmp(&b.item_id)), // deterministic final tiebreak
    }
}

/// Produce the stable display order as a permutation of indices into `slots`,
/// written into `out` (cleared first).
///
/// `out` ends up containing each index `0..slots.len()` exactly once, ordered
/// by [`slot_cmp`]. Empty input yields empty `out`.
pub fn sort_order(slots: &[Slot], out: &mut Vec<u32>) {
    out.clear();
    out.reserve(slots.len());
    for i in 0..slots.len() {
        out.push(i as u32);
    }
    // Stable sort over the index permutation — no Slot data is moved, only u32
    // indices, and full-key ties retain their original order.
    out.sort_by(|&i, &j| slot_cmp(&slots[i as usize], &slots[j as usize]));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(item_id: u32, category: u8, rarity: u8, name_key: u32) -> Slot {
        Slot { item_id, category, rarity, quantity: 1, name_key }
    }

    fn order(slots: &[Slot]) -> Vec<u32> {
        let mut out = Vec::new();
        sort_order(slots, &mut out);
        out
    }

    fn is_permutation(out: &[u32], n: usize) -> bool {
        if out.len() != n {
            return false;
        }
        let mut seen = vec![false; n];
        for &i in out {
            let k = i as usize;
            if k >= n || seen[k] {
                return false;
            }
            seen[k] = true;
        }
        seen.iter().all(|&s| s)
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(order(&[]).is_empty());
    }

    #[test]
    fn output_is_a_permutation() {
        let slots = [
            slot(5, 2, 1, 30),
            slot(0, 0, 0, 0), // empty
            slot(9, 0, 3, 10),
            slot(7, 2, 1, 30),
            slot(0, 0, 0, 0), // empty
        ];
        let out = order(&slots);
        assert!(is_permutation(&out, slots.len()));
    }

    #[test]
    fn out_is_cleared_first() {
        let slots = [slot(1, 0, 0, 0)];
        let mut out = vec![999, 998, 997];
        sort_order(&slots, &mut out);
        assert_eq!(out, vec![0]);
    }

    #[test]
    fn category_then_rarity_desc_then_name_then_id() {
        // cat 0 before cat 1; within a category, higher rarity first; then
        // name_key asc; then item_id asc.
        let slots = [
            slot(10, 1, 5, 100), // cat1
            slot(11, 0, 2, 50),  // cat0 rarity2
            slot(12, 0, 9, 70),  // cat0 rarity9 (rarer → first in cat0)
            slot(13, 0, 9, 70),  // same key as 12 except id 13 → after it
        ];
        let out = order(&slots);
        // cat0 first: rarity9 items (idx2 then idx3 by id), then rarity2 (idx1),
        // then cat1 (idx0).
        assert_eq!(out, vec![2, 3, 1, 0]);
    }

    #[test]
    fn rarity_is_descending_within_category() {
        let slots = [
            slot(1, 0, 1, 5),
            slot(2, 0, 9, 5),
            slot(3, 0, 4, 5),
        ];
        let out = order(&slots);
        // rarity 9, 4, 1
        assert_eq!(out, vec![1, 2, 0]);
    }

    #[test]
    fn empty_slots_sort_to_the_end() {
        let slots = [
            slot(0, 0, 0, 0),  // empty
            slot(5, 3, 1, 9),  // real
            slot(0, 7, 7, 7),  // empty (item_id 0 → empty regardless of other fields)
            slot(6, 1, 1, 2),  // real
        ];
        let out = order(&slots);
        // reals first (idx3 cat1, idx1 cat3), then the two empties in input order.
        assert_eq!(out, vec![3, 1, 0, 2]);
        // last two are the empty slots
        assert!(slots[out[2] as usize].item_id == 0 && slots[out[3] as usize].item_id == 0);
    }

    #[test]
    fn stable_across_full_key_ties() {
        // Five slots identical in the full key (incl. item_id) keep input order.
        let slots = [
            slot(8, 1, 1, 1),
            slot(8, 1, 1, 1),
            slot(8, 1, 1, 1),
            slot(8, 1, 1, 1),
            slot(8, 1, 1, 1),
        ];
        assert_eq!(order(&slots), vec![0, 1, 2, 3, 4]);
        // And empties tied among themselves are also stable.
        let empties = [slot(0, 0, 0, 0), slot(0, 0, 0, 0), slot(0, 0, 0, 0)];
        assert_eq!(order(&empties), vec![0, 1, 2]);
    }

    #[test]
    fn deterministic() {
        let slots = [
            slot(5, 2, 1, 30),
            slot(9, 0, 3, 10),
            slot(0, 0, 0, 0),
            slot(7, 2, 1, 30),
            slot(3, 1, 9, 4),
        ];
        let first = order(&slots);
        for _ in 0..100 {
            assert_eq!(order(&slots), first);
        }
        assert!(is_permutation(&first, slots.len()));
    }

    #[test]
    fn no_panic_on_large_bag_and_remains_permutation() {
        // Build a big bag with repeated keys and empties; just must not panic
        // and stay a valid permutation.
        let mut slots = Vec::new();
        for i in 0..1000u32 {
            let empty = i % 7 == 0;
            slots.push(Slot {
                item_id: if empty { 0 } else { i + 1 },
                category: (i % 5) as u8,
                rarity: (i % 11) as u8,
                quantity: 1,
                name_key: i % 13,
            });
        }
        let out = order(&slots);
        assert!(is_permutation(&out, slots.len()));
        // every empty index lands in the trailing block
        let first_empty_pos = out
            .iter()
            .position(|&i| slots[i as usize].item_id == 0)
            .unwrap_or(out.len());
        for &i in &out[first_empty_pos..] {
            assert_eq!(slots[i as usize].item_id, 0, "non-empty after first empty");
        }
    }
}
