//! task_lanes — operation-gated parallelism (r196). Parallel work is
//! admitted ONLY by its access CLASS (see PARALLELISM_BY_OPERATION.md),
//! and the signatures here make a mis-classification a COMPILE error
//! rather than a runtime data race.
//!
//! Class P (pure map):   `&In -> Out`, In: Sync, Out: Send. Disjoint
//!                       owned outputs; merge serially or in key order.
//! Class C (clone+apply): compute an owned `Patch` in parallel, apply it
//!                       to `&mut Target` serially on the main thread.
//! Class S (serial):     not representable here — no parallel entry
//!                       point, by exclusion.
//!
//! Determinism: `map_pure` preserves input order in its output Vec, so a
//! consumer that folds in order is deterministic regardless of how the
//! work was scheduled. `compute_patches` likewise returns patches in
//! input order; `apply_patches` folds them on main in that order.

use rayon::prelude::*;

/// Class P — pure parallel map. `f` must be `Fn(&In) -> Out + Sync` and
/// capture nothing mutable; the borrow checker + `Sync` bound reject a
/// closure that tries to share a `&mut` or a non-Sync cell. Output Vec is
/// in the SAME order as `items` (deterministic merge).
pub fn map_pure<In, Out, F>(items: &[In], f: F) -> Vec<Out>
where
    In: Sync,
    Out: Send,
    F: Fn(&In) -> Out + Sync + Send,
{
    items.par_iter().map(|x| f(x)).collect()
}

/// Class P with a sequential threshold: below `min_parallel` items, run
/// serially (avoids rayon overhead on tiny batches — the per-frame case
/// where a handful of chunks isn't worth a fork).
pub fn map_pure_threshold<In, Out, F>(items: &[In], min_parallel: usize, f: F) -> Vec<Out>
where
    In: Sync,
    Out: Send,
    F: Fn(&In) -> Out + Sync + Send,
{
    if items.len() < min_parallel {
        items.iter().map(|x| f(x)).collect()
    } else {
        items.par_iter().map(|x| f(x)).collect()
    }
}

/// Class C — compute owned patches in parallel (pure: `&In -> Patch`),
/// then the caller applies them on the main thread with `apply_patches`.
/// Splitting compute from apply is the safety: the parallel phase makes
/// only owned values; the only shared write is the serial apply.
pub fn compute_patches<In, Patch, F>(items: &[In], f: F) -> Vec<Patch>
where
    In: Sync,
    Patch: Send,
    F: Fn(&In) -> Patch + Sync + Send,
{
    items.par_iter().map(|x| f(x)).collect()
}

/// Apply patches to a mutable target IN INPUT ORDER on the calling
/// (main) thread. `apply` is the only place shared state is written.
pub fn apply_patches<Patch, Target, A>(patches: Vec<Patch>, target: &mut Target, mut apply: A)
where
    A: FnMut(&mut Target, Patch),
{
    for p in patches {
        apply(target, p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_pure_preserves_order() {
        let xs: Vec<u32> = (0..1000).collect();
        let ys = map_pure(&xs, |&x| x * x);
        for (i, &y) in ys.iter().enumerate() {
            assert_eq!(y, (i as u32) * (i as u32));
        }
    }

    #[test]
    fn threshold_serial_and_parallel_agree() {
        let xs: Vec<u32> = (0..50).collect();
        let a = map_pure_threshold(&xs, 1000, |&x| x + 1); // serial
        let b = map_pure_threshold(&xs, 1, |&x| x + 1); // parallel
        assert_eq!(a, b);
    }

    #[test]
    fn class_c_compute_then_apply_in_order() {
        // patches: each says "add this index's square to the accumulator"
        let xs: Vec<u64> = (0..100).collect();
        let patches = compute_patches(&xs, |&x| x * x);
        let mut total = 0u64;
        apply_patches(patches, &mut total, |t, p| *t += p);
        let expect: u64 = (0..100u64).map(|x| x * x).sum();
        assert_eq!(total, expect);
    }
}
