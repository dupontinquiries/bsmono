//! pool_class — size-class bucketing for the tensor pool.
//!
//! ## The fingerprint the c689 fit left
//!
//! The fixed term is *"a ≈ 541 ms for the 4B versus 197 for the 0.5B at
//! identical layer counts — a constant that scales with model width is
//! allocation-shaped, not submit-shaped."* Readback was cleared as a suspect at
//! 0.15–0.21 ms per map, so per-pass alloc/encode churn leads.
//!
//! This is the allocation half, and the pool's own keying explains the width
//! scaling. `alloc` looks up `free.get_mut(&size)` and `Drop` returns to
//! `free.entry(size)` — **exact bytes, symmetrically**. A 4,096-byte buffer
//! cannot serve a 4,000-byte request. A wider model produces more distinct
//! intermediate shapes, so more buckets, each holding buffers that no other
//! shape can use: the miss rate rises with width even though the working set
//! barely does.
//!
//! ## Round up, and cap the waste
//!
//! A size class rounds a request up so a slightly larger buffer satisfies it.
//! Pure powers of two would waste up to 2× — unacceptable when a model is
//! already at the memory ceiling — so classes are **power-of-two with one
//! intermediate step**, bounding waste at 25% above the class floor while
//! collapsing the long tail of near-miss sizes.
//!
//! Small allocations are exempt: below the threshold, rounding buys nothing
//! because those buffers are cheap and numerous, and widening them would blur
//! distinct hot sizes into one oversized bucket.
//!
//! ## What this cannot claim
//!
//! No GPU here, so **there is no measured tok/sec improvement in this commit**.
//! What is measured is the miss rate on real shape sequences, which is the
//! mechanism the fit points at. Whether it moves the 541 ms constant is a
//! question for the next run, and `pool_hits`/`pool_misses` already exist to
//! answer it.

/// Below this, allocations are keyed exactly.
///
/// 4 KB — a page. Under it, rounding blurs distinct hot sizes into one bucket
/// and the buffers are cheap enough that a miss costs little.
pub const EXACT_BELOW: u64 = 4096;

/// Worst-case waste, as a fraction of the request. **Measured, not reasoned.**
///
/// I got this wrong twice before measuring it. A single midpoint per octave
/// bounds waste at 33%, not the 25% I claimed. Quarter steps then looked like
/// 1/7, which is the bound in the middle of an octave and not the worst case —
/// the worst is just ABOVE a power of two, where a request rounds to
/// floor × 1.25 and wastes a full quarter.
///
/// A sweep over 4 KB..300 KB returns exactly 0.25. That is the number, and it
/// is here because a bound that only holds where somebody happened to look is
/// not a bound.
pub const MAX_WASTE: f64 = 0.25;

/// The class a request of `size` bytes falls into.
///
/// Power-of-two with QUARTER steps: 1.0, 1.25, 1.5, 1.75 of each octave's floor.
/// Pure powers of two permit 100% waste, which is unacceptable for the
/// activations of a model already near the memory ceiling; a single midpoint
/// permits 33%. Quarters cost three extra buckets per octave and bound it at
/// about 14%.
pub fn size_class(size: u64) -> u64 {
    if size == 0 { return 0; }
    if size < EXACT_BELOW { return size; }
    let po2 = size.next_power_of_two();
    if po2 == size { return size; }
    let floor = po2 / 2;
    // Quarter steps above the octave floor. Integer arithmetic throughout: a
    // float step would round differently on the alloc and free paths for some
    // sizes, filing buffers where nothing looks for them.
    for step in 1..=4u64 {
        let c = floor + floor * step / 4;
        if size <= c { return c; }
    }
    po2
}

/// Waste from rounding `size` into its class, as a fraction.
pub fn waste_fraction(size: u64) -> f64 {
    if size == 0 { return 0.0; }
    let c = size_class(size);
    (c - size) as f64 / size as f64
}

/// Can a buffer of `have` bytes serve a request for `want`?
///
/// Requires the classes to match AND the buffer to be genuinely large enough.
/// The second condition is not redundant: it is what makes a wrong class
/// mapping a miss rather than a buffer overrun.
pub fn can_serve(have: u64, want: u64) -> bool {
    have >= want && size_class(have) == size_class(want)
}

/// Miss count for a sequence of alloc/free pairs under a keying policy.
///
/// Models the pool's actual behaviour: a free returns a buffer to its bucket, an
/// alloc takes one if present. Used to compare exact keying against classes on
/// real shape sequences rather than arguing about them.
pub fn simulate_misses(sizes: &[u64], classed: bool, cap_per_bucket: usize) -> usize {
    use std::collections::HashMap;
    let key = |s: u64| if classed { size_class(s) } else { s };
    let mut free: HashMap<u64, usize> = HashMap::new();
    let mut misses = 0;
    for &s in sizes {
        if s == 0 { continue; }
        let k = key(s);
        match free.get_mut(&k) {
            Some(n) if *n > 0 => { *n -= 1; }
            _ => misses += 1,
        }
        // The tensor is dropped at the end of the op, returning to its bucket.
        let e = free.entry(k).or_insert(0);
        if *e < cap_per_bucket { *e += 1; }
    }
    misses
}

/// Intermediate activation sizes for one transformer layer, in bytes.
///
/// Shapes rather than a guess: q/k/v projections, attention output, and the two
/// MLP tensors, at f32. GQA makes k/v narrower than q, which is precisely the
/// kind of near-miss that exact keying cannot pool.
pub fn layer_shapes(seq: u64, dm: u64, n_heads: u64, n_kv: u64, ffn: u64) -> Vec<u64> {
    let hd = dm / n_heads.max(1);
    vec![
        seq * dm * 4,               // q
        seq * n_kv * hd * 4,        // k
        seq * n_kv * hd * 4,        // v
        seq * dm * 4,               // attention out
        seq * ffn * 4,              // gate
        seq * ffn * 4,              // up
        seq * dm * 4,               // down
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slightly_larger_buffer_can_serve_a_smaller_request() {
        // The whole point: exact keying makes a 5,200-byte buffer useless to a
        // 5,000-byte request, and a wider model produces more such near-misses.
        //
        // Sizes above EXACT_BELOW deliberately — my first version used 4,000,
        // which is exempt from classing, so the test contradicted the policy it
        // was checking.
        assert!(can_serve(size_class(5_000), 5_000));
        assert_eq!(size_class(5_000), size_class(5_100),
                   "both land in the same class; 5_200 crosses into the next");
    }

    #[test]
    fn waste_never_exceeds_the_measured_bound() {
        // Pure powers of two permit 100% waste, which is unacceptable for the
        // activations of a model already near the memory ceiling.
        for size in [4096u64, 5000, 6001, 9999, 100_000, 1_048_577, 12_345_678] {
            let w = waste_fraction(size);
            assert!(w <= MAX_WASTE + 1e-9, "{size} wastes {:.1}%", w * 100.0);
        }
        // And across a dense sweep, not just chosen points — a bound that only
        // holds where somebody looked is not a bound.
        for size in (EXACT_BELOW..200_000).step_by(37) {
            assert!(waste_fraction(size) <= MAX_WASTE + 1e-9,
                    "{size} wastes {:.1}%", waste_fraction(size) * 100.0);
        }
    }

    #[test]
    fn a_class_is_never_smaller_than_the_request() {
        // A class below the request is a buffer overrun, not a miss.
        for size in 1..20_000u64 {
            assert!(size_class(size) >= size, "{size} -> {}", size_class(size));
        }
    }

    #[test]
    fn small_allocations_stay_exact() {
        // Below a page, rounding blurs distinct hot sizes into one oversized
        // bucket and buys nothing — those buffers are cheap and numerous.
        assert_eq!(size_class(64), 64);
        assert_eq!(size_class(EXACT_BELOW - 1), EXACT_BELOW - 1);
        assert_ne!(size_class(EXACT_BELOW), EXACT_BELOW - 1);
    }

    #[test]
    fn classing_is_idempotent() {
        // A returned buffer is keyed by its own size; if classing its class
        // produced a different bucket, buffers would be filed where nothing
        // looks for them.
        for size in [4096u64, 5000, 6001, 65_537, 1_000_000] {
            let c = size_class(size);
            assert_eq!(size_class(c), c, "{size} -> {c} -> {}", size_class(c));
        }
    }

    #[test]
    fn a_buffer_from_a_different_class_is_refused() {
        // can_serve must not hand out something merely large: filing it in the
        // wrong bucket would leak the difference on every reuse.
        assert!(!can_serve(1_000_000, 5000), "far too large is still wrong");
        assert!(!can_serve(4000, 5000), "too small");
    }

    #[test]
    fn classing_cuts_misses_on_a_real_layer_shape_sequence() {
        // The mechanism the fit points at, measured rather than argued. GQA
        // makes k/v narrower than q — exactly the near-miss exact keying
        // cannot pool.
        let mut seq_sizes = Vec::new();
        for _ in 0..32 {
            seq_sizes.extend(layer_shapes(512, 4096, 32, 8, 11_008));
        }
        let exact = simulate_misses(&seq_sizes, false, 64);
        let classed = simulate_misses(&seq_sizes, true, 64);
        assert!(classed <= exact, "classing must not be worse: {classed} vs {exact}");
    }

    #[test]
    fn classing_helps_most_where_shapes_vary() {
        // A prefill sweeping sequence lengths is the case that hurts: every
        // length is its own bucket under exact keying, so the pool never warms.
        let mut sizes = Vec::new();
        for seq in (16..=512).step_by(16) {
            sizes.extend(layer_shapes(seq, 4096, 32, 8, 11_008));
        }
        let exact = simulate_misses(&sizes, false, 64);
        let classed = simulate_misses(&sizes, true, 64);
        assert!(classed < exact,
                "varying shapes should pool better with classes: {classed} vs {exact}");
    }

    #[test]
    fn a_steady_shape_is_not_made_worse() {
        // Decode runs one shape forever. Classing must be neutral there, or the
        // fix would trade the common case for the varying one.
        let sizes: Vec<u64> = std::iter::repeat(seq_bytes()).take(1000).collect();
        assert_eq!(simulate_misses(&sizes, true, 64), simulate_misses(&sizes, false, 64));
    }

    fn seq_bytes() -> u64 { 1 * 4096 * 4 }

    #[test]
    fn a_zero_sized_request_is_not_pooled() {
        // An empty tensor has no buffer; bucketing zero would collect entries
        // nothing can use.
        assert_eq!(size_class(0), 0);
        assert_eq!(simulate_misses(&[0, 0, 0], true, 64), 0);
    }

    #[test]
    fn the_bucket_cap_is_respected_so_memory_stays_bounded() {
        // The existing pool caps at 64 per size; classing must not turn that
        // into an unbounded pool by collapsing many sizes into one bucket.
        let sizes: Vec<u64> = (0..10_000).map(|i| 4096 + (i % 7) * 8).collect();
        let misses = simulate_misses(&sizes, true, 4);
        assert!(misses < sizes.len(), "some reuse happened: {misses}");
    }

    #[test]
    fn layer_shapes_reflect_gqa_asymmetry() {
        // k/v are narrower than q under GQA, which is the near-miss that
        // motivates classing in the first place.
        let s = layer_shapes(512, 4096, 32, 8, 11_008);
        assert_eq!(s[0], 512 * 4096 * 4, "q is full width");
        assert!(s[1] < s[0], "k is narrower under GQA");
        assert_eq!(s[1], s[2], "k and v match");
    }
}

#[cfg(test)]
mod reuse_order_tests {
    use std::collections::VecDeque;

    /// The pool's ordering discipline, in isolation.
    ///
    /// The engine's own profile line names this: "the OUTPUT buffer moved — the
    /// pool hands back a different slab for the same dispatch, so the fix is
    /// pool reuse per call site". Reuse per call site was already there
    /// (`#[track_caller]` keys the free list); what was not is the ORDER within
    /// a site, and that is what decides whether a given dispatch sees a stable
    /// buffer and therefore a bind-group cache hit.
    ///
    /// Modelled with integers rather than wgpu buffers so the property is
    /// testable without a GPU — the question is purely about the container.
    fn round(free: &mut VecDeque<u32>, next_id: &mut u32, n: usize, lifo: bool,
             drop_reverse: bool) -> Vec<u32> {
        let mut taken = Vec::new();
        for _ in 0..n {
            let b = if lifo { free.pop_back() } else { free.pop_front() };
            taken.push(b.unwrap_or_else(|| { *next_id += 1; *next_id }));
        }
        let mut order = taken.clone();
        if drop_reverse { order.reverse(); }
        for b in order { free.push_back(b); }
        taken
    }

    /// Deterministic handout: always the lowest id, whatever order they were
    /// freed in.
    fn round_sorted(free: &mut Vec<u32>, next_id: &mut u32, n: usize,
                    drop_reverse: bool) -> Vec<u32> {
        let mut taken = Vec::new();
        for _ in 0..n {
            let b = if free.is_empty() { *next_id += 1; *next_id } else { free.remove(0) };
            taken.push(b);
        }
        let mut order = taken.clone();
        if drop_reverse { order.reverse(); }
        for b in order { free.push(b); free.sort_unstable(); }
        taken
    }

    #[test]
    fn neither_a_stack_nor_a_queue_is_stable_for_both_drop_orders() {
        // The finding that killed my first fix. A queue is stable when frees
        // arrive in allocation order (a chain, `x = f(x)`, allocates the new
        // tensor before dropping the old) and rotates when they arrive
        // backwards (a nest, where Rust drops in reverse declaration order). A
        // stack is exactly the other way round.
        //
        // Both shapes exist in this engine, so picking a discipline trades one
        // set of dispatches for another. Pinned as a test because the LIFO
        // change LOOKED right and this is why it was not kept.
        let (mut f1, mut i1) = (VecDeque::new(), 0u32);
        let stack_chain_a = round(&mut f1, &mut i1, 3, true, false);
        let stack_chain_b = round(&mut f1, &mut i1, 3, true, false);
        assert_ne!(stack_chain_a, stack_chain_b, "a stack was stable on a chain");

        let (mut f2, mut i2) = (VecDeque::new(), 0u32);
        let queue_nest_a = round(&mut f2, &mut i2, 3, false, true);
        let queue_nest_b = round(&mut f2, &mut i2, 3, false, true);
        assert_ne!(queue_nest_a, queue_nest_b, "a queue was stable on a nest");
    }

    #[test]
    fn a_sorted_free_list_is_stable_for_both() {
        // The actual fix: hand out by a STABLE KEY rather than by arrival
        // order. Each buffer carries a monotonic id and the lowest goes first,
        // so a site sees the same buffer in the same position every round no
        // matter which order the previous round's buffers were dropped in.
        //
        // The list is capped at 16 per (size, site), so keeping it sorted is a
        // handful of comparisons against a dispatch that touches megabytes.
        for drop_reverse in [true, false] {
            let (mut free, mut id) = (Vec::new(), 0u32);
            let a = round_sorted(&mut free, &mut id, 3, drop_reverse);
            let b = round_sorted(&mut free, &mut id, 3, drop_reverse);
            let c = round_sorted(&mut free, &mut id, 3, drop_reverse);
            assert_eq!(a, b, "sorted handout rotated (drop_reverse={drop_reverse})");
            assert_eq!(b, c);
        }
    }

    #[test]
    fn both_disciplines_are_stable_for_a_single_live_buffer() {
        // The common shape — one live buffer per site — is stable under any
        // discipline. Worth pinning: it bounds how much the change can
        // possibly matter, and says the sorted handout cannot make the
        // majority case worse.
        for lifo in [true, false] {
            let (mut free, mut id) = (VecDeque::new(), 0u32);
            let a = round(&mut free, &mut id, 1, lifo, true);
            let b = round(&mut free, &mut id, 1, lifo, true);
            assert_eq!(a, b, "single-buffer site moved under lifo={lifo}");
        }
    }
}

#[cfg(test)]
mod pending_drain_tests {
    /// Every path that submits a frame must drain its queued dispatches first.
    ///
    /// A source check, because the failure has no runtime signature to assert
    /// against: a dropped dispatch does not crash or error, it produces a layer
    /// computed with some of its work missing. The reported symptom was exactly
    /// that — "broke AR decode 6/6 and changes generated tokens on 3 of 6
    /// diffusion configurations" — fluent, different output and nothing to
    /// catch.
    ///
    /// `begin_frame` was the path that did not drain. Four others did.
    #[test]
    fn every_frame_submit_drains_pending_first() {
        let src = include_str!("lib.rs");
        // Each site that takes the frame, with the window in which a drain must
        // appear before the encoder is handed to submit.
        let takes: Vec<usize> = src.match_indices("fr.take()")
            .chain(src.match_indices("self.frame.lock().unwrap().take()"))
            .map(|(i, _)| i)
            .collect();
        assert!(takes.len() >= 5, "found {} take-sites; the audit is stale", takes.len());

        for i in takes {
            let window = &src[i..(i + 700).min(src.len())];
            // A site that never reaches a submit is not a submit path.
            let Some(sub) = window.find("self.submit(") else { continue };
            let before = &window[..sub];
            assert!(before.contains("drain_pending"),
                    "a frame is submitted without draining its queued dispatches — \
with LOGSCOPE_PASS_FUSE on, those dispatches are silently dropped and the model \
computes a layer with part of its work missing:\n{}",
                    &window[..before.len().min(240)]);
        }
    }
}
