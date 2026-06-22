//! retrieval.rs — the asymmetric retrieval-set objective.
//!
//! Three-state lattice per fact: Miss < Partial < Hit. Two rules:
//!   HARD: never regress a *pinned* hit (a hit the exact layer — context_graph/rubric — confirmed).
//!         Pinned hits are un-swappable, so the no-regression guarantee is real, not aspirational.
//!   SOFT: accept an update only if it nets upward — more Miss→Partial than Partial→Miss.
//! A small fuzzy model proposes swaps but only on the `frontier` (everything that isn't a pinned hit),
//! so it can only help. This is strictly stronger than summarization, which *accepts* hit loss.
#![allow(dead_code)]

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Recall { Miss = 0, Partial = 1, Hit = 2 }

pub fn level(r: Recall) -> i32 { r as i32 }

/// Net lattice movement before→after: +1 per step up, −1 per step down.
pub fn net_movement(before: &[Recall], after: &[Recall]) -> i32 {
    before.iter().zip(after).map(|(&b, &a)| level(a) - level(b)).sum()
}

/// Did any pinned hit drop below Hit? The cardinal sin.
pub fn regressed_pinned_hit(before: &[Recall], after: &[Recall], pinned: &[bool]) -> bool {
    before.iter().zip(after).zip(pinned).any(|((&b, &a), &p)| p && b == Recall::Hit && a != Recall::Hit)
}

/// Accept iff no pinned hit regressed AND the update nets positive upward movement.
pub fn accept_update(before: &[Recall], after: &[Recall], pinned: &[bool]) -> bool {
    !regressed_pinned_hit(before, after, pinned) && net_movement(before, after) > 0
}

/// Indices a fuzzy pass may touch: everything that ISN'T a pinned hit (the partial/miss frontier).
pub fn frontier(state: &[Recall], pinned: &[bool]) -> Vec<usize> {
    state.iter().zip(pinned).enumerate()
        .filter(|(_, (&s, &p))| !(p && s == Recall::Hit))
        .map(|(i, _)| i).collect()
}

/// Global-compression noise bar: retained noise must stay within budget (default 5%).
pub fn within_noise_budget(noise: usize, total: usize, budget: f32) -> bool {
    total == 0 || (noise as f32 / total as f32) <= budget
}

pub fn gate_retrieval() {
    use Recall::*;
    let before = [Hit, Partial, Miss];
    let pinned = [true, false, false]; // slot 0 is a confirmed hit → un-swappable

    let good = [Hit, Hit, Partial];      // partial→hit, miss→partial, pinned kept : +2
    let regress = [Miss, Hit, Hit];      // pinned hit → miss : forbidden no matter the other gains
    let net_zero = [Hit, Miss, Partial]; // partial→miss (−1) + miss→partial (+1) : 0
    let net_pos = [Hit, Miss, Hit];      // partial→miss (−1) + miss→hit (+2) : +1

    let checks = [
        ("net-positive, no regression → accept", accept_update(&before, &good, &pinned)),
        ("regresses pinned hit → reject", !accept_update(&before, &regress, &pinned)),
        ("net-zero (one up, one down) → reject", !accept_update(&before, &net_zero, &pinned)),
        ("more up than down → accept", accept_update(&before, &net_pos, &pinned)),
        ("frontier excludes the pinned hit", frontier(&before, &pinned) == vec![1, 2]),
        ("5% noise budget", within_noise_budget(1, 20, 0.05) && !within_noise_budget(2, 20, 0.05)),
    ];
    for (label, ok) in &checks { println!("  [{}] {}", if *ok { "\u{2713}" } else { "\u{2717} FAIL" }, label); }
    let all = checks.iter().all(|&(_, ok)| ok);
    println!("gate-retrieval: {}", if all { "asymmetric objective checks \u{2713}" } else { "FAIL \u{2717}" });
}
