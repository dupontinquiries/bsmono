//! topk — the per-token scans over the whole vocabulary, written so they are O(n) and so the
//! compiler can vectorize them on BASELINE targets (x86-64 SSE2, AArch64 NEON) with no intrinsics,
//! no `target-cpu`, and no `unsafe`.
//!
//! ## What was here before, and what it cost
//!
//! `sample()` — the default sampler is temperature 0.8, top_k 40, so this is every token of every
//! ordinary chat — did, per token, on a 248,320-entry vocabulary (Ornith / Qwen3.5):
//!
//!   1. `logits.iter().any(|x| !x.is_finite())` — an EARLY-EXIT loop. LLVM does not vectorize
//!      loops with a data-dependent exit, on any architecture, so this is one scalar compare per
//!      logit.
//!   2. `(0..n).collect::<Vec<usize>>()` — a 2 MB allocation.
//!   3. `idx.sort_by(|a, b| logits[b].total_cmp(&logits[a]))` — a FULL stable sort of 248k indices
//!      through an indirection, ~4.5 M cache-hostile comparisons, to keep the first 40.
//!
//! `argmax` (every greedy token that does not use the GPU argmax) is a loop-carried
//! max-with-index, which LLVM also cannot vectorize.
//!
//! ## The idiom
//!
//! A reduction the vectorizer will not touch (early exit, or carrying an index) is split into a
//! VECTORIZABLE FILTER over fixed chunks — a fold with no exit: `acc |= x >= t`, or a lane-wise
//! max — and a scalar body that runs only for the rare chunk the filter flags. The filter is what
//! touches all n elements; the scalar part touches a handful. This is portable Rust and compiles
//! to packed compares on SSE2 and NEON alike.
//!
//! ## Exactness
//!
//! Everything here is EXACT, not approximate: [`top_k_desc`] returns precisely the first `k`
//! entries the stable full sort produced (logit descending by `total_cmp`, ties by index
//! ascending), so sampling is bit-identical for a given RNG state; [`argmax_finite`] returns the
//! same index as the scalar loop it replaces, including its tie rule (FIRST maximum) and its
//! non-finite rule. The reference implementations are kept, `#[cfg(test)]`-free, because the
//! equivalence tests are the licence to use the fast ones.

/// Chunk width for the filters. 64 f32 = 256 bytes: four cache lines, a whole number of NEON (4),
/// SSE (4), AVX2 (8) and AVX-512 (16) lanes, and small enough that a flagged chunk's scalar
/// rescan is cheap.
const CHUNK: usize = 64;

/// True if any value is NaN or ±Inf. No early exit, on purpose: see the module doc.
pub fn any_non_finite(v: &[f32]) -> bool {
    let mut bad = false;
    let mut chunks = v.chunks_exact(CHUNK);
    for c in &mut chunks {
        // `abs() < INF` is false for NaN and for ±Inf; `&=` over a fixed-width chunk is a
        // reduction with no exit, which vectorizes to a packed compare and an AND-reduce.
        let mut ok = true;
        for &x in c { ok &= x.abs() < f32::INFINITY; }
        bad |= !ok;
    }
    for &x in chunks.remainder() { bad |= !(x.abs() < f32::INFINITY); }
    bad
}

/// The scalar loop `any_non_finite` replaces. Kept as the oracle.
pub fn any_non_finite_reference(v: &[f32]) -> bool { v.iter().any(|x| !x.is_finite()) }

/// Index of the FIRST maximum among the finite entries; 0 if none are finite. Same contract as
/// the scalar `argmax` it replaces (which exists so one poisoned logit cannot pin the pick).
pub fn argmax_finite(v: &[f32]) -> u32 {
    // Pass 1, vectorizable: the maximum finite value, as CHUNK independent lanes. A non-finite
    // entry contributes -Inf, the identity — a select, not a branch.
    let mut lanes = [f32::NEG_INFINITY; CHUNK];
    let mut chunks = v.chunks_exact(CHUNK);
    for c in &mut chunks {
        for (l, &x) in lanes.iter_mut().zip(c) {
            let y = if x.abs() < f32::INFINITY { x } else { f32::NEG_INFINITY };
            *l = if y > *l { y } else { *l };
        }
    }
    let mut best = f32::NEG_INFINITY;
    for &l in &lanes { if l > best { best = l; } }
    for &x in chunks.remainder() { if x.abs() < f32::INFINITY && x > best { best = x; } }
    if best == f32::NEG_INFINITY {
        // Either nothing is finite, or every finite entry IS -Inf… which is not finite. So: none.
        return 0;
    }
    // Pass 2: the first index holding it. `==` on the value found in pass 1 is exact, and this
    // stops at the first hit — on average half the vocabulary, with no index carried in pass 1.
    v.iter().position(|&x| x == best).unwrap_or(0) as u32
}

/// The scalar loop `argmax_finite` replaces. Kept as the oracle.
pub fn argmax_reference(v: &[f32]) -> u32 {
    let mut bi = 0usize; let mut bv = f32::NEG_INFINITY; let mut seen = false;
    for (i, &x) in v.iter().enumerate() {
        if x.is_finite() && (!seen || x > bv) { bv = x; bi = i; seen = true; }
    }
    bi as u32
}

/// The indices of the `k` largest logits, best first — EXACTLY `top_k_reference`'s output: logit
/// descending by `total_cmp`, ties by index ascending (what a stable sort of `0..n` gives).
///
/// O(n) for the scan plus O(k) per accepted candidate, no n-sized allocation. The scan keeps a
/// running threshold (the current k-th best); a chunk none of whose values reaches it is skipped
/// by a vectorizable filter, and after the first few hundred tokens of a real distribution that is
/// nearly every chunk.
///
/// Callers must have excluded NaN (`sample` does, via `any_non_finite`): `>=` is not a total
/// order in its presence. ±Inf and ±0 are handled — the filter is deliberately the SUPERSET
/// (`>=`), and the exact `total_cmp` decides.
pub fn top_k_desc(logits: &[f32], k: usize) -> Vec<usize> {
    let n = logits.len();
    let k = k.min(n);
    if k == 0 { return Vec::new(); }
    // (logit, index), best first. Small: k is 40 by default, a few hundred at most.
    let mut best: Vec<(f32, usize)> = Vec::with_capacity(k + 1);
    let mut consider = |best: &mut Vec<(f32, usize)>, x: f32, i: usize| {
        if best.len() == k {
            // Full: x must beat the current worst. A tie loses — `i` is larger than any index
            // already held, and index ascending breaks ties.
            if x.total_cmp(&best[k - 1].0) != std::cmp::Ordering::Greater { return; }
            best.pop();
        }
        // After every element that is >= x (they are better: larger, or equal with a smaller index).
        let at = best.partition_point(|e| e.0.total_cmp(&x) != std::cmp::Ordering::Less);
        best.insert(at, (x, i));
    };
    let mut i = 0usize;
    // Fill phase: until k are held there is no threshold to filter with.
    while i < n && best.len() < k { consider(&mut best, logits[i], i); i += 1; }
    // Scan phase.
    while i + CHUNK <= n {
        let t = best[k - 1].0;
        let c = &logits[i..i + CHUNK];
        let mut hit = false;
        for &x in c { hit |= x >= t; }                 // the filter: no exit, vectorizes
        if hit { for (j, &x) in c.iter().enumerate() { if x >= best[k - 1].0 { consider(&mut best, x, i + j); } } }
        i += CHUNK;
    }
    while i < n { if logits[i] >= best[k - 1].0 { consider(&mut best, logits[i], i); } i += 1; }
    best.into_iter().map(|(_, i)| i).collect()
}

/// What `sample()` did: sort every index, keep `k`. Kept as the oracle.
pub fn top_k_reference(logits: &[f32], k: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..logits.len()).collect();
    idx.sort_by(|&a, &b| logits[b].total_cmp(&logits[a]));
    idx.truncate(k.min(logits.len()));
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(s: &mut u64) -> u64 { *s ^= *s << 13; *s ^= *s >> 7; *s ^= *s << 17; *s }
    /// Logits with the shape that matters: a few large values, a long tail, and DELIBERATE TIES
    /// (quantized to a coarse grid) so the tie rule is exercised on every draw.
    fn logits(n: usize, seed: u64, grid: f32) -> Vec<f32> {
        let mut s = seed | 1;
        (0..n).map(|_| { let u = (rng(&mut s) >> 40) as f32 / (1u64 << 24) as f32; let x = (u * 2.0 - 1.0) * 12.0; if grid > 0.0 { (x / grid).round() * grid } else { x } }).collect()
    }

    #[test]
    fn top_k_is_the_stable_sorts_prefix_exactly() {
        for (n, grid) in [(1usize, 0.0f32), (7, 0.5), (63, 0.0), (64, 0.25), (65, 0.25), (1000, 0.0), (1000, 1.0), (4099, 0.5), (50_000, 0.0), (50_000, 2.0)] {
            for seed in 1..=4u64 {
                let v = logits(n, seed * 7919, grid);
                for k in [1usize, 2, 5, 40, 64, 300, n, n + 9] {
                    assert_eq!(top_k_desc(&v, k), top_k_reference(&v, k), "n={n} grid={grid} seed={seed} k={k}");
                }
            }
        }
        assert!(top_k_desc(&[], 40).is_empty() && top_k_desc(&[1.0, 2.0], 0).is_empty());
    }

    /// The adversarial orders for a threshold scan: ascending (every element beats the threshold —
    /// the worst case for the insert path), descending (none does), all-equal (every compare is a
    /// tie), and the ±0 / ±Inf cases where `>=` and `total_cmp` disagree.
    #[test]
    fn top_k_holds_on_the_orders_that_break_threshold_scans() {
        let asc: Vec<f32> = (0..5000).map(|i| i as f32 * 0.01).collect();
        let desc: Vec<f32> = asc.iter().rev().copied().collect();
        let flat = vec![3.5f32; 5000];
        let mut zeros = vec![0.0f32; 300]; for i in (0..300).step_by(3) { zeros[i] = -0.0; }
        let mut infs = logits(2000, 99, 0.0); infs[17] = f32::INFINITY; infs[1900] = f32::INFINITY; infs[5] = f32::NEG_INFINITY;
        for (name, v) in [("ascending", &asc), ("descending", &desc), ("all equal", &flat), ("signed zeros", &zeros), ("infinities", &infs)] {
            for k in [1usize, 40, 257] { assert_eq!(top_k_desc(v, k), top_k_reference(v, k), "{name} k={k}"); }
        }
        assert_eq!(top_k_desc(&flat, 3), [0, 1, 2], "ties go to the LOWER index");
        assert_eq!(top_k_desc(&zeros, 2), [1, 2], "+0 outranks -0 under total_cmp, as it did under the sort");
    }

    #[test]
    fn the_finite_scan_and_the_argmax_match_their_scalar_oracles() {
        for n in [0usize, 1, 63, 64, 65, 127, 128, 129, 1000, 4099] {
            for seed in 1..=3u64 {
                let mut v = logits(n, seed * 31, 0.5);
                assert_eq!(any_non_finite(&v), any_non_finite_reference(&v));
                assert_eq!(argmax_finite(&v), argmax_reference(&v), "clean n={n}");
                if n == 0 { continue; }
                // poison one position at a time, in the body and in the remainder
                for (at, bad) in [(0usize, f32::NAN), (n / 2, f32::INFINITY), (n - 1, f32::NEG_INFINITY), (n - 1, f32::NAN)] {
                    let keep = v[at]; v[at] = bad;
                    assert!(any_non_finite(&v), "n={n} at={at}");
                    assert_eq!(argmax_finite(&v), argmax_reference(&v), "n={n} poisoned at {at} with {bad}");
                    v[at] = keep;
                }
            }
        }
        // ties: the FIRST maximum, in the chunked body and across a chunk boundary
        let mut v = vec![1.0f32; 200]; v[70] = 9.0; v[71] = 9.0; v[130] = 9.0;
        assert_eq!((argmax_finite(&v), argmax_reference(&v)), (70, 70));
        // a row with nothing finite is index 0, not garbage
        assert_eq!(argmax_finite(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]), argmax_reference(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]));
        assert_eq!(argmax_finite(&vec![f32::NAN; 300]), 0);
        // +Inf must NOT win: it is not finite (the Colibri #369 rule)
        let mut w = logits(500, 5, 0.0); w[3] = f32::INFINITY;
        assert_eq!(argmax_finite(&w), argmax_reference(&w));
        assert_ne!(argmax_finite(&w), 3);
    }
}
