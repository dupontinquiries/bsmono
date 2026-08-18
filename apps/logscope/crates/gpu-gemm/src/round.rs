//! round.rs — splitting a round's slots between fork breadth and sibling depth.
//!
//! `probe` needs at least `MIN_EFFECTIVE_N` distinct forks before it will name a layer, because
//! siblings of one fork are not independent observations. With 32 slots that reads as a tax: eight
//! forks means four children each, and the population design wanted deep fanout from the promising
//! checkpoints. This module is the arithmetic that decides whether the tax is real.
//!
//! It is not. Under the obvious model, spreading never loses.
//!
//! ## The model
//!
//! A fork point is VIABLE with probability `q` — the checkpoint is somewhere a solution can be
//! reached from at all. Given a viable fork, each child independently succeeds with probability `p`.
//! Then for `f` forks of `c` children each:
//!
//! ```text
//! P(fork yields nothing) = 1 - q * (1 - (1-p)^c)
//! P(round yields nothing) = [1 - q * (1 - (1-p)^c)] ^ f
//! ```
//!
//! At `q = 1` — every fork viable — this collapses to `1 - (1-p)^(f*c)`, identical for every split
//! of the same slot count. Grouping cannot matter when nothing is riding on the choice of parent.
//!
//! At `q < 1` spreading strictly wins, and the reason is diminishing returns: the 32nd child of one
//! parent is competing against the 31 that already sampled that neighbourhood, while the 1st child
//! of a new parent samples a neighbourhood nobody has touched. So the fork floor `probe` asks for is
//! not a tax on search. It is what search should be doing anyway, and the measurement comes free.
//!
//! Two caveats, both pushing the same way. Siblings share a prefix, so their outcomes correlate and
//! the within-fork returns diminish FASTER than independence assumes — strengthening the case for
//! breadth. And the model says nothing about sequential refinement, where child `n+1` builds on what
//! child `n` learned; that is a real argument for depth and it is not parallel fanout, so it belongs
//! to `recover`'s patch loop rather than here.
//!
//! ## Reaping deepens, it does not broaden
//!
//! `reap`'s slot turnover produces more children per slot over a round, all from the same fork
//! point the slot was seeded with. So reaping buys depth. Breadth has to come from the layout, which
//! is why the two live in different modules and the layout is chosen first.
//!
//! Pure: no wgpu, no model.

/// How a round's slots are divided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    /// Distinct fork points seeded this round.
    pub forks: usize,
    /// Slots per fork.
    pub per_fork: usize,
}

impl Layout {
    pub fn slots(&self) -> usize { self.forks * self.per_fork }

    /// Rounds until `probe` has seen enough distinct forks to evaluate a layer.
    ///
    /// Forks accumulate: the probe does not need them simultaneous, only distinct. This is what
    /// actually dissolves the apparent conflict with deep fanout — the constraint is on time to a
    /// first validated direction, not on the shape of any one round.
    ///
    /// The thing it does NOT license is accumulating across tasks. Forks from different problems fit
    /// a direction that is claimed to hold globally, which is a much stronger claim than the local
    /// chart this was supposed to be. Accumulate within a task.
    pub fn rounds_to_probe(&self, min_forks: usize) -> usize {
        if self.forks == 0 { return usize::MAX; }
        (min_forks + self.forks - 1) / self.forks
    }
}

/// Probability that a round produces at least one success.
///
/// `q` is per-fork viability, `p` is per-child success given a viable fork.
pub fn p_success(l: Layout, q: f32, p: f32) -> f32 {
    let (q, p) = (q.clamp(0.0, 1.0), p.clamp(0.0, 1.0));
    let per_fork_miss = 1.0 - q * (1.0 - (1.0 - p).powi(l.per_fork as i32));
    1.0 - per_fork_miss.powi(l.forks as i32)
}

/// Probability of at least one success when the forks are NOT interchangeable.
///
/// `q` gives each parent's viability in rank order. The uniform model above says 32 forks of one
/// child each is optimal at every parameter setting, which is an artifact of two things it leaves
/// out: forks have to come from somewhere, and the ones further down the ranking are worse. A round
/// can only seed as many forks as the previous round left survivors, and survivor 8 is not survivor
/// 1.
pub fn p_success_ranked(q: &[f32], per_fork: usize, p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    let child_miss = (1.0 - p).powi(per_fork as i32);
    let mut miss = 1.0f32;
    for qi in q { miss *= 1.0 - qi.clamp(0.0, 1.0) * (1.0 - child_miss); }
    1.0 - miss
}

/// Viability by rank: `top * decay^i`. `decay = 1.0` is the uniform model.
pub fn q_by_rank(top: f32, decay: f32, k: usize) -> Vec<f32> {
    (0..k).map(|i| top * decay.powi(i as i32)).collect()
}

/// How many survivors to keep, given `slots` to spend on their children.
///
/// This is the round-shape decision the population design left as "keep top 4, fork x8". Keeping
/// more parents buys fork diversity — which the probe needs and which the uniform model says is
/// always good — but each extra parent is less likely to be viable AND takes slots from better ones.
/// With `decay < 1` that produces an interior optimum instead of "keep everything".
///
/// Returns `(k, per_fork)`, with `per_fork = slots / k`.
pub fn best_k(slots: usize, top: f32, decay: f32, p: f32, min_forks: usize) -> (usize, usize) {
    const TIE: f32 = 1e-6;
    let (mut bk, mut bp) = (1usize, -1.0f32);
    for k in 1..=slots {
        if slots % k != 0 { continue; }
        if k < min_forks && k < slots { continue; }
        let s = p_success_ranked(&q_by_rank(top, decay, k), slots / k, p);
        if s > bp + TIE || (s > bp - TIE && k > bk) { bk = k; bp = s; }
    }
    (bk, slots / bk)
}

/// Every layout of exactly `slots` slots, breadth-first.
pub fn layouts(slots: usize) -> Vec<Layout> {
    (1..=slots).filter(|f| slots % f == 0)
        .map(|forks| Layout { forks, per_fork: slots / forks })
        .collect()
}

/// The layout maximising `p_success`, optionally subject to a fork floor.
///
/// Ties go to MORE forks: when two layouts are equally good for search, the one that also feeds the
/// probe is free.
pub fn best_layout(slots: usize, q: f32, p: f32, min_forks: usize) -> Layout {
    // Tolerance is 1e-6, not 1e-9: `p_success` is f32 and at a probability near 0.8 one ulp is
    // about 6e-8, so two layouts that are mathematically identical (the q=1 case) differ by more
    // than 1e-9 and the tiebreak never fires. The first version of this returned the DEEPEST layout
    // for every tie, which is the opposite of what it says it does.
    const TIE: f32 = 1e-6;
    let mut best = Layout { forks: slots.max(1), per_fork: 1 };
    let mut best_p = -1.0f32;
    for l in layouts(slots) {
        if l.forks < min_forks { continue; }
        let s = p_success(l, q, p);
        if s > best_p + TIE || (s > best_p - TIE && l.forks > best.forks) {
            best = l; best_p = s;
        }
    }
    best
}

/// What the fork floor costs, as a fraction of the unconstrained optimum's success probability.
/// Zero means free.
pub fn floor_cost(slots: usize, q: f32, p: f32, min_forks: usize) -> f32 {
    let free = p_success(best_layout(slots, q, p, 1), q, p);
    let held = p_success(best_layout(slots, q, p, min_forks), q, p);
    if free <= 0.0 { return 0.0; }
    ((free - held) / free).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(forks: usize, per_fork: usize) -> Layout { Layout { forks, per_fork } }

    #[test]
    fn every_layout_of_32_slots_holds_32_slots() {
        for x in layouts(32) { assert_eq!(x.slots(), 32); }
        assert!(layouts(32).contains(&l(8, 4)));
        assert!(layouts(32).contains(&l(1, 32)));
        assert_eq!(layouts(32).len(), 6, "32 has six divisors");
    }

    #[test]
    fn with_every_fork_viable_the_split_makes_no_difference() {
        // q = 1 collapses to 1 - (1-p)^(f*c). The grouping cannot matter, and that is the boundary
        // case the whole argument pivots on.
        let a = p_success(l(1, 32), 1.0, 0.05);
        let b = p_success(l(8, 4), 1.0, 0.05);
        let c = p_success(l(32, 1), 1.0, 0.05);
        assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        assert!((a - c).abs() < 1e-5, "{a} vs {c}");
    }

    #[test]
    fn when_fork_viability_is_uncertain_spreading_wins() {
        let deep = p_success(l(1, 32), 0.3, 0.10);
        let wide = p_success(l(8, 4), 0.3, 0.10);
        assert!(wide > deep * 1.5, "wide {wide} deep {deep}");
    }

    #[test]
    fn spreading_never_loses_across_the_whole_parameter_range() {
        // The claim the module rests on. If this fails, the fork floor IS a tax and the layout has
        // to be argued for rather than assumed.
        for qi in 1..=10 {
            for pi in 1..=20 {
                let (q, p) = (qi as f32 / 10.0, pi as f32 / 200.0);
                let deep = p_success(l(1, 32), q, p);
                let wide = p_success(l(8, 4), q, p);
                assert!(wide >= deep - 1e-6, "q={q} p={p}: wide {wide} < deep {deep}");
            }
        }
    }

    #[test]
    fn the_probe_fork_floor_costs_the_search_nothing() {
        for qi in 1..=10 {
            for pi in 1..=20 {
                let (q, p) = (qi as f32 / 10.0, pi as f32 / 200.0);
                assert!(floor_cost(32, q, p, 8) < 1e-5, "q={q} p={p} cost {}", floor_cost(32, q, p, 8));
            }
        }
    }

    #[test]
    fn ties_are_broken_toward_more_forks_because_measurement_is_then_free() {
        // q = 1 makes every layout equal for search; the tiebreak should still feed the probe.
        let b = best_layout(32, 1.0, 0.05, 1);
        assert!(b.forks >= 8, "got {b:?}");
    }

    #[test]
    fn forks_accumulate_so_a_narrow_round_only_costs_time() {
        assert_eq!(l(2, 16).rounds_to_probe(8), 4);
        assert_eq!(l(8, 4).rounds_to_probe(8), 1);
        assert_eq!(l(3, 10).rounds_to_probe(8), 3);
        assert_eq!(l(0, 0).rounds_to_probe(8), usize::MAX);
    }

    #[test]
    fn with_uniform_viability_the_ranked_model_agrees_with_the_uniform_one() {
        let a = p_success(l(8, 4), 0.6, 0.05);
        let b = p_success_ranked(&q_by_rank(0.6, 1.0, 8), 4, 0.05);
        assert!((a - b).abs() < 1e-5, "{a} vs {b}");
    }

    #[test]
    fn declining_viability_produces_an_interior_optimum_instead_of_keep_everything() {
        // The uniform model says 32 forks of one child. Once survivor 8 is worse than survivor 1,
        // that stops being true and there is a real number to pick.
        let (k, per) = best_k(32, 0.8, 0.55, 0.10, 1);
        assert!(k > 1 && k < 32, "k={k} per_fork={per}");
        assert_eq!(k * per, 32);
    }

    #[test]
    fn a_steeper_decline_keeps_fewer_parents() {
        let (gentle, _) = best_k(32, 0.8, 0.9, 0.10, 1);
        let (steep, _) = best_k(32, 0.8, 0.3, 0.10, 1);
        assert!(steep <= gentle, "steep {steep} should not exceed gentle {gentle}");
    }

    #[test]
    fn uniform_viability_recovers_keep_everything() {
        let (k, _) = best_k(32, 0.8, 1.0, 0.10, 1);
        assert_eq!(k, 32, "with no ranking penalty, breadth is free and unbounded");
    }

    #[test]
    fn a_floor_above_the_slot_count_still_returns_a_usable_layout() {
        // Asking for more forks than there are slots cannot be satisfied; the fallback must be the
        // widest available rather than a panic or a zero-fork layout.
        let b = best_layout(8, 0.5, 0.1, 64);
        assert_eq!(b.slots(), 8);
        assert!(b.forks >= 1);
    }
}
