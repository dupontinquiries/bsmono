//! `util::batch_kernel` — the one streaming `map`-over-a-slice helper the 60+
//! `sim::batch_*` modules all hand-roll (WID r330-refactor-batch).
//!
//! Every `batch_*` module mirrors a scalar sibling with the same shape: an
//! `#[inline]` kernel, a scalar `*_one`, and a `*_batch` that re-writes the same
//! "clear/reserve an output `Vec` and push `kernel(..)` for each input" loop.
//! That loop is the repetition; this module owns it once.
//!
//! - [`map_into`] is the canonical replacement for a `*_batch` body: it clears
//!   and reserves the caller's `out` then makes a single streaming pass, so the
//!   result is **bit-for-bit identical** to the old per-module loop (same kernel,
//!   same order) while reusing the caller's allocation across frames.
//! - [`map_collect`] is the allocating convenience for the common
//!   `fn *_batch(..) -> Vec<U>` signature; it just routes a fresh `Vec` through
//!   [`map_into`], so there is a single code path.
//! - [`nonneg`] folds the `fn nonneg` several `batch_*` modules re-define
//!   (e.g. `sim::batch_aggro_decay`) into one definition, kept *exactly* as the
//!   `if x > 0.0 { x } else { 0.0 }` form so migrations stay bit-identical.
//!   // TODO fold into `util::mathx` once r330-refactor-math merges (it owns the
//!   //      canonical exact scalar helpers); this local copy exists only so
//!   //      refactor-batch doesn't depend on that merge landing first.
//!
//! Pure, std-only, panic-free, alloc-bounded (one `reserve`, no per-item
//! reallocation). No clock/RNG/IO. Generic over the element/output types, so it
//! serves `f32`, `u8`, tuple and struct kernels alike.

/// Map `f` over `src` into `out`: clear `out`, reserve `src.len()`, then push
/// `f(item)` for each `item` in order. The drop-in for a `*_batch` loop.
///
/// Bit-for-bit equivalent to `for x in src { out.push(f(x)) }` over a fresh
/// `Vec` — same values, same order — while letting the caller keep and reuse
/// `out`'s buffer across calls. Alloc-bounded: at most one growth (the
/// `reserve`); panic-free for any `f` that is itself panic-free.
#[inline]
pub fn map_into<T, U>(src: &[T], out: &mut Vec<U>, f: impl Fn(&T) -> U) {
    out.clear();
    out.reserve(src.len());
    for item in src {
        out.push(f(item));
    }
}

/// Allocating convenience for the `fn *_batch(..) -> Vec<U>` shape: map `f` over
/// `src` into a fresh `Vec`, in order. Routes through [`map_into`].
#[inline]
pub fn map_collect<T, U>(src: &[T], f: impl Fn(&T) -> U) -> Vec<U> {
    let mut out = Vec::new();
    map_into(src, &mut out, f);
    out
}

/// `x` if positive, else `0.0` — the exact shape several `batch_*` modules
/// re-define locally. Kept as `if x > 0.0 { x } else { 0.0 }` (NOT `x.max(0.0)`)
/// to be bit-identical to those local copies on every input.
// TODO fold into `util::mathx::nonneg` on r330-refactor-math merge.
#[inline]
pub fn nonneg(x: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_collect_preserves_values_and_order() {
        let src = [1u32, 2, 3, 4];
        let got = map_collect(&src, |&x| x * 10);
        assert_eq!(got, vec![10, 20, 30, 40]);
    }

    #[test]
    fn empty_slice_yields_empty() {
        let got: Vec<u32> = map_collect::<u32, u32>(&[], |&x| x);
        assert!(got.is_empty());
        let mut out: Vec<u32> = vec![9, 9, 9];
        map_into::<u32, u32>(&[], &mut out, |&x| x);
        assert!(out.is_empty());
    }

    #[test]
    fn map_into_clears_existing_contents() {
        // out starts non-empty / longer than src — must be fully replaced.
        let mut out = vec![100u8, 101, 102, 103, 104];
        map_into(&[1u8, 2, 3], &mut out, |&x| x + 1);
        assert_eq!(out, vec![2, 3, 4]); // cleared, then exactly src-mapped
    }

    #[test]
    fn map_into_reserves_capacity() {
        let mut out: Vec<u32> = Vec::new();
        map_into(&[0u32; 32], &mut out, |&x| x);
        assert!(out.capacity() >= 32, "expected reserve >= 32, got {}", out.capacity());
        assert_eq!(out.len(), 32);
    }

    #[test]
    fn map_into_reuses_buffer_no_realloc_when_same_len() {
        let mut out: Vec<u32> = Vec::with_capacity(8);
        let ptr_before = out.as_ptr();
        map_into(&[0u32; 8], &mut out, |&x| x + 1);
        // clear+reserve(8) on a cap-8 buffer must not reallocate.
        assert_eq!(out.as_ptr(), ptr_before, "buffer should be reused");
        assert_eq!(out, vec![1u32; 8]);
    }

    #[test]
    fn closure_may_capture_environment() {
        // mirrors `*_batch(src, dt)` closing over a shared frame scalar.
        let dt = 0.5f32;
        let got = map_collect(&[(1.0f32, 2.0f32), (3.0, 4.0)], |&(a, b)| (a + b) * dt);
        assert_eq!(got, vec![1.5, 3.5]);
    }

    #[test]
    fn nonneg_matches_local_form_incl_edges() {
        // exact `if x>0 {x} else {0}` form — clamps negatives & NaN to +0.0.
        assert_eq!(nonneg(2.5).to_bits(), 2.5f32.to_bits());
        assert_eq!(nonneg(-1.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(nonneg(0.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(nonneg(-0.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(nonneg(f32::NAN).to_bits(), 0.0f32.to_bits());
        assert_eq!(nonneg(f32::INFINITY), f32::INFINITY);
    }

    #[test]
    fn map_collect_equals_iter_map_collect() {
        // the property migrations rely on: map_collect == .iter().map().collect()
        let src = [(0.1f32, 1u8), (0.2, 2), (0.3, 3), (0.4, 4)];
        let via_helper = map_collect(&src, |&(f, n)| f * n as f32);
        let via_std: Vec<f32> = src.iter().map(|&(f, n)| f * n as f32).collect();
        assert_eq!(via_helper, via_std);
    }
}
