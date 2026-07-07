//! Command-buffer partitioning — the load-balancer for parallel command
//! recording. Recording is single-threaded today: ~1.5ms on every frame, the
//! hard floor that caps the engine around 240Hz and blocks 500Hz no matter what
//! else is optimized. The fix is recording into N secondary command buffers on N
//! threads, but that only helps if the threads finish together — the frame waits
//! on the slowest. This splits the draw list into N buckets of roughly equal
//! cost (index count is the proxy for record + GPU work) so the buckets straggle
//! as little as possible.
//!
//! Greedy longest-processing-time-first (LPT): a classic makespan minimizer.
//! Pure, deterministic.

/// One unit of recordable work — a (coalesced) draw and its cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawItem {
    pub draw_index: u32,
    /// Cost proxy — index count works well (drives both record time and GPU work).
    pub cost: u64,
}

/// Partition `items` across `n` recording threads, minimizing the heaviest
/// bucket (LPT: assign the largest remaining item to the currently-lightest
/// bucket). Returns, per bucket, the draw indices to record there. Deterministic:
/// items sorted by cost desc then index; bucket ties broken by lowest index.
pub fn partition(items: &[DrawItem], n: usize) -> Vec<Vec<u32>> {
    let n = n.max(1);
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut loads: Vec<u64> = vec![0; n];

    let mut order: Vec<&DrawItem> = items.iter().collect();
    order.sort_by(|a, b| b.cost.cmp(&a.cost).then(a.draw_index.cmp(&b.draw_index)));

    for it in order {
        // Lightest bucket; ties → lowest index for determinism.
        let mut best = 0usize;
        for j in 1..n {
            if loads[j] < loads[best] {
                best = j;
            }
        }
        buckets[best].push(it.draw_index);
        loads[best] += it.cost;
    }
    buckets
}

/// Cost of the heaviest bucket — the critical path of a parallel record pass.
pub fn makespan(buckets: &[Vec<u32>], items: &[DrawItem]) -> u64 {
    use std::collections::HashMap;
    let cost: HashMap<u32, u64> = items.iter().map(|i| (i.draw_index, i.cost)).collect();
    buckets
        .iter()
        .map(|b| b.iter().map(|d| cost.get(d).copied().unwrap_or(0)).sum::<u64>())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn item(i: u32, c: u64) -> DrawItem {
        DrawItem { draw_index: i, cost: c }
    }

    #[test]
    fn single_thread_gets_everything() {
        let items = [item(0, 5), item(1, 3), item(2, 8)];
        let b = partition(&items, 1);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].len(), 3);
        assert_eq!(makespan(&b, &items), 16);
    }

    #[test]
    fn every_draw_assigned_exactly_once() {
        let items: Vec<DrawItem> = (0..50).map(|i| item(i, (i as u64 % 7) + 1)).collect();
        let b = partition(&items, 4);
        let seen: Vec<u32> = b.iter().flatten().copied().collect();
        let set: HashSet<u32> = seen.iter().copied().collect();
        assert_eq!(seen.len(), 50, "no duplicates");
        assert_eq!(set.len(), 50, "all 50 present");
    }

    #[test]
    fn lpt_balances_better_than_a_fifth_of_total() {
        // [5,4,3,2,1] over 2 buckets: total 15, ideal 7.5; LPT gives 8.
        let items = [item(0, 5), item(1, 4), item(2, 3), item(3, 2), item(4, 1)];
        let b = partition(&items, 2);
        let m = makespan(&b, &items);
        assert_eq!(m, 8, "near-optimal balance");
        // Both buckets within 1 of each other.
        let loads: Vec<u64> = b.iter().map(|bk| bk.iter().map(|d| items[*d as usize].cost).sum()).collect();
        let (lo, hi) = (*loads.iter().min().unwrap(), *loads.iter().max().unwrap());
        assert!(hi - lo <= 1, "balanced within one unit: {loads:?}");
    }

    #[test]
    fn makespan_is_bounded_by_total_and_ideal() {
        let items: Vec<DrawItem> = (0..37).map(|i| item(i, (i as u64 * 13 % 17) + 1)).collect();
        let total: u64 = items.iter().map(|i| i.cost).sum();
        let n = 6;
        let m = makespan(&partition(&items, n), &items);
        assert!(m >= total / n as u64, "can't beat the perfect split");
        assert!(m <= total, "never worse than serial");
    }

    #[test]
    fn more_threads_than_draws_is_fine() {
        let items = [item(0, 3), item(1, 5)];
        let b = partition(&items, 8);
        assert_eq!(b.len(), 8);
        assert_eq!(b.iter().filter(|bk| !bk.is_empty()).count(), 2, "two non-empty, rest idle");
        assert_eq!(makespan(&b, &items), 5, "biggest single draw sets the floor");
    }

    #[test]
    fn empty_input_yields_empty_buckets() {
        let b = partition(&[], 4);
        assert_eq!(b.len(), 4);
        assert!(b.iter().all(|bk| bk.is_empty()));
        assert_eq!(makespan(&b, &[]), 0);
    }
}
