//! `sim::stack_deposit` — stack-aware deposit (auto-stack) planner.
//!
//! Plans where incoming items should land in an inventory without mutating it.
//! Uses the same per-slot stack-cap semantics as [`sim::stack_merge`]:
//! `stack_cap` is the maximum number of units a single slot may hold.
//!
//! ## Deposit order
//! 1. **Top-up pass** — existing slots holding `item_id` that are below
//!    `stack_cap`, visited in ascending index order, are filled first.
//! 2. **Empty-slot pass** — remaining amount spills into empty slots
//!    (`item_id == None`), again in ascending index order.
//! 3. **Overflow** — anything that still doesn't fit is reported in
//!    `DepositPlan::overflow`; no placement is emitted for it.
//!
//! The function is **pure**: it reads `inv` but never mutates it.
//! Conservation invariant: `sum(placements.1) + overflow == amount` always.

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single inventory slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// `None` means the slot is empty; `Some(id)` holds item type `id`.
    pub item_id: Option<u32>,
    /// How many units are currently in this slot.
    pub count: u32,
}

/// The result of a deposit plan.
#[derive(Debug, PartialEq, Eq)]
pub struct DepositPlan {
    /// `(slot_index, units_added)` pairs in the order they were planned.
    /// No slot appears more than once. `units_added` is always > 0.
    pub placements: Vec<(usize, u32)>,
    /// Units that did not fit anywhere in the inventory.
    pub overflow: u32,
}

// ---------------------------------------------------------------------------
// Core function
// ---------------------------------------------------------------------------

/// Plan depositing `amount` of `item_id` into `inv` with `stack_cap` per slot.
///
/// - `stack_cap == 0`: nothing can be deposited; all `amount` overflows.
/// - `amount == 0`: returns empty placements and `overflow = 0`.
/// - Slots holding a *different* `item_id` are never targeted.
/// - Pure: `inv` is not mutated. Deterministic. No allocation beyond the plan.
pub fn deposit(inv: &[Slot], item_id: u32, amount: u32, stack_cap: u32) -> DepositPlan {
    if amount == 0 || stack_cap == 0 {
        return DepositPlan {
            placements: Vec::new(),
            overflow: amount,
        };
    }

    let mut remaining = amount;
    let mut placements: Vec<(usize, u32)> = Vec::new();

    // Pass 1: top up existing same-item slots in index order.
    for (idx, slot) in inv.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        if slot.item_id == Some(item_id) && slot.count < stack_cap {
            let space = stack_cap - slot.count;
            let add = remaining.min(space);
            placements.push((idx, add));
            remaining -= add;
        }
    }

    // Pass 2: spill into empty slots in index order.
    for (idx, slot) in inv.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        if slot.item_id.is_none() {
            let add = remaining.min(stack_cap);
            placements.push((idx, add));
            remaining -= add;
        }
    }

    DepositPlan {
        placements,
        overflow: remaining,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers -----------------------------------------------------------

    fn empty(n: usize) -> Vec<Slot> {
        vec![Slot { item_id: None, count: 0 }; n]
    }

    fn slot(id: u32, count: u32) -> Slot {
        Slot { item_id: Some(id), count }
    }

    fn empty_slot() -> Slot {
        Slot { item_id: None, count: 0 }
    }

    /// Conservation invariant: sum(placements) + overflow == amount.
    fn assert_conserved(plan: &DepositPlan, amount: u32) {
        let placed: u64 = plan.placements.iter().map(|&(_, n)| n as u64).sum();
        assert_eq!(
            placed + plan.overflow as u64,
            amount as u64,
            "conservation violated: placed={placed} overflow={} amount={amount}",
            plan.overflow
        );
    }

    // ---- unit tests --------------------------------------------------------

    #[test]
    fn partial_same_item_slots_fill_before_empties() {
        // slot 0: item 1, count 50 (has 14 space at cap 64)
        // slot 1: empty
        let inv = vec![slot(1, 50), empty_slot()];
        let plan = deposit(&inv, 1, 20, 64);
        // First placement must be slot 0 (top-up), then slot 1.
        assert_eq!(plan.placements[0], (0, 14), "should top up slot 0 first");
        assert_eq!(plan.placements[1], (1, 6),  "remainder into slot 1");
        assert_eq!(plan.overflow, 0);
        assert_conserved(&plan, 20);
    }

    #[test]
    fn spills_into_empty_slots() {
        let inv = empty(3);
        let plan = deposit(&inv, 7, 150, 64);
        // slot 0: 64, slot 1: 64, slot 2: 22, overflow 0
        assert_eq!(plan.placements, vec![(0, 64), (1, 64), (2, 22)]);
        assert_eq!(plan.overflow, 0);
        assert_conserved(&plan, 150);
    }

    #[test]
    fn full_inventory_all_overflow() {
        let inv = vec![slot(1, 64), slot(1, 64), slot(2, 64)];
        let plan = deposit(&inv, 1, 50, 64);
        assert!(plan.placements.is_empty(), "no room → no placements");
        assert_eq!(plan.overflow, 50);
        assert_conserved(&plan, 50);
    }

    #[test]
    fn quantity_conservation_random() {
        // LCG-based property sweep (seed R20-style, documented: 0xD9A3_C28F).
        let mut seed = 0xD9A3_C28Fu32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed >> 8
        };

        for _ in 0..2000 {
            let n_slots = (lcg() % 8 + 1) as usize;
            let cap = lcg() % 64 + 1;
            let amount = lcg() % 200;
            let item_id = lcg() % 4;

            let inv: Vec<Slot> = (0..n_slots)
                .map(|_| {
                    let r = lcg() % 4;
                    if r == 0 {
                        empty_slot()
                    } else {
                        let id = lcg() % 4;
                        let cnt = lcg() % (cap + 1);
                        Slot { item_id: Some(id), count: cnt }
                    }
                })
                .collect();

            let plan = deposit(&inv, item_id, amount, cap);
            assert_conserved(&plan, amount);
        }
    }

    #[test]
    fn no_placement_exceeds_stack_cap() {
        let inv = vec![slot(3, 60), empty_slot(), empty_slot()];
        let plan = deposit(&inv, 3, 200, 64);
        for &(idx, added) in &plan.placements {
            let before = inv[idx].count;
            assert!(
                before + added <= 64,
                "slot {idx} would exceed cap: {before} + {added} > 64"
            );
        }
        assert_conserved(&plan, 200);
    }

    #[test]
    fn never_targets_different_item_id() {
        let inv = vec![slot(5, 10), slot(6, 10), empty_slot()];
        let plan = deposit(&inv, 5, 30, 64);
        for &(idx, _) in &plan.placements {
            let s = inv[idx];
            assert!(
                s.item_id == Some(5) || s.item_id.is_none(),
                "targeted slot {idx} holds item {:?}, not 5 or empty", s.item_id
            );
        }
        assert_conserved(&plan, 30);
    }

    #[test]
    fn amount_zero_returns_empty_plan() {
        let inv = vec![slot(1, 10), empty_slot()];
        let plan = deposit(&inv, 1, 0, 64);
        assert!(plan.placements.is_empty());
        assert_eq!(plan.overflow, 0);
        assert_conserved(&plan, 0);
    }

    #[test]
    fn stack_cap_zero_all_overflow() {
        let inv = vec![empty_slot(), empty_slot()];
        let plan = deposit(&inv, 2, 99, 0);
        assert!(plan.placements.is_empty());
        assert_eq!(plan.overflow, 99);
        assert_conserved(&plan, 99);
    }

    #[test]
    fn deterministic() {
        let inv = vec![slot(1, 30), empty_slot(), slot(1, 50)];
        let a = deposit(&inv, 1, 80, 64);
        let b = deposit(&inv, 1, 80, 64);
        assert_eq!(a.placements, b.placements);
        assert_eq!(a.overflow,   b.overflow);
    }

    #[test]
    fn top_up_index_order() {
        // Two partial same-item slots: should fill lower index first.
        let inv = vec![slot(9, 50), slot(9, 40), empty_slot()];
        let plan = deposit(&inv, 9, 100, 64);
        // slot 0 gets 14, slot 1 gets 24, slot 2 gets 62, overflow 0
        assert_eq!(plan.placements[0].0, 0, "should target slot 0 first");
        assert_eq!(plan.placements[1].0, 1, "should target slot 1 second");
        assert_conserved(&plan, 100);
    }

    #[test]
    fn exactly_fills_to_cap() {
        let inv = vec![slot(2, 60)];
        let plan = deposit(&inv, 2, 4, 64);
        assert_eq!(plan.placements, vec![(0, 4)]);
        assert_eq!(plan.overflow, 0);
        assert_conserved(&plan, 4);
    }

    #[test]
    fn empty_inventory_all_overflow() {
        let plan = deposit(&[], 1, 50, 64);
        assert!(plan.placements.is_empty());
        assert_eq!(plan.overflow, 50);
        assert_conserved(&plan, 50);
    }
}
