//! `lod_upload` — cull / LOD upload-budget policy (PERF #4). **OPTIONAL.**
//!
//! Worker-03 / WORLD-PERF pure core. Given a render distance, a per-frame
//! upload budget, and a set of candidate chunks (each with a distance and an
//! importance), decide which to upload, which to defer to a later frame, and
//! which to cull (out of range). Nearer chunks are prioritized; ties break by
//! higher importance, then by id for a fully deterministic order.
//!
//! Pure & deterministic. Self-contained for standalone `rustc --test`.
#![allow(dead_code)]

/// A chunk competing for the per-frame upload budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Stable identity; also the final tiebreak so ordering is deterministic.
    pub id: u64,
    /// Chebyshev distance in chunks from the player.
    pub distance: i32,
    /// Higher = more worth uploading at equal distance (e.g. edited / visible).
    pub importance: u32,
}

/// Per-chunk outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Upload,
    Defer,
    Cull,
}

/// Decide upload / defer / cull for each candidate within `budget`.
///
/// * Chunks with `distance > render_distance` are **culled**.
/// * In-range chunks are ranked (nearer first, then higher importance, then
///   lower id); the first `budget` are **uploaded**, the rest **deferred**.
///
/// Decisions are returned in the SAME order as `candidates`. Pure &
/// deterministic.
pub fn plan_uploads(candidates: &[Candidate], render_distance: i32, budget: usize) -> Vec<Decision> {
    let mut out = vec![Decision::Cull; candidates.len()];

    // In-range candidates default to Defer; the budget promotes the top ones.
    let mut in_range: Vec<usize> = Vec::new();
    for (i, c) in candidates.iter().enumerate() {
        if c.distance <= render_distance {
            out[i] = Decision::Defer;
            in_range.push(i);
        }
    }

    // Deterministic priority: nearer, then more important, then lower id.
    in_range.sort_by(|&a, &b| {
        let ca = candidates[a];
        let cb = candidates[b];
        ca.distance
            .cmp(&cb.distance)
            .then(cb.importance.cmp(&ca.importance))
            .then(ca.id.cmp(&cb.id))
    });

    for &i in in_range.iter().take(budget) {
        out[i] = Decision::Upload;
    }
    out
}

/// Convenience: total uploads a plan would issue (≤ budget, ≤ in-range count).
pub fn upload_count(decisions: &[Decision]) -> usize {
    decisions.iter().filter(|d| **d == Decision::Upload).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: u64, distance: i32, importance: u32) -> Candidate {
        Candidate { id, distance, importance }
    }

    #[test]
    fn out_of_range_is_culled() {
        let cands = [c(0, 5, 0), c(1, 11, 9)];
        let plan = plan_uploads(&cands, 10, 8);
        assert_eq!(plan[0], Decision::Upload); // in range, fits budget
        assert_eq!(plan[1], Decision::Cull); // distance 11 > rd 10
    }

    #[test]
    fn budget_respected() {
        let cands: Vec<Candidate> = (0..20).map(|i| c(i, (i % 8) as i32, 0)).collect();
        let plan = plan_uploads(&cands, 100, 5);
        assert_eq!(upload_count(&plan), 5);
    }

    #[test]
    fn in_range_over_budget_defers_not_culls() {
        let cands: Vec<Candidate> = (0..10).map(|i| c(i, 1, 0)).collect();
        let plan = plan_uploads(&cands, 100, 3);
        assert_eq!(upload_count(&plan), 3);
        // The remaining 7 are in range, so deferred (never culled).
        assert_eq!(plan.iter().filter(|d| **d == Decision::Defer).count(), 7);
        assert_eq!(plan.iter().filter(|d| **d == Decision::Cull).count(), 0);
    }

    #[test]
    fn nearer_is_prioritized_over_farther() {
        // Far chunk listed first, near chunk second; budget 1 → near wins.
        let cands = [c(0, 9, 100), c(1, 1, 0)];
        let plan = plan_uploads(&cands, 100, 1);
        assert_eq!(plan[1], Decision::Upload); // distance 1
        assert_eq!(plan[0], Decision::Defer); // distance 9
    }

    #[test]
    fn prop_no_deferred_chunk_outranks_an_uploaded_one() {
        // For a deterministic spread of candidates, every Uploaded chunk must
        // rank at least as high as every Deferred chunk (nearer, or = distance
        // with >= importance) — i.e. the budget cut never skips a nearer chunk.
        for seed in 0..200u64 {
            let n = 16;
            let cands: Vec<Candidate> = (0..n)
                .map(|i| {
                    let h = (i as u64).wrapping_mul(2654435761).wrapping_add(seed.wrapping_mul(40503));
                    c(i as u64, (h % 12) as i32, (h >> 5) as u32 % 4)
                })
                .collect();
            let budget = (seed as usize % (n as usize + 1)).max(1);
            let plan = plan_uploads(&cands, 100, budget);

            let rank = |x: &Candidate| (x.distance, core::cmp::Reverse(x.importance), x.id);
            for (i, di) in plan.iter().enumerate() {
                if *di != Decision::Upload {
                    continue;
                }
                for (j, dj) in plan.iter().enumerate() {
                    if *dj == Decision::Defer {
                        // uploaded i must rank <= deferred j (better or equal)
                        assert!(
                            rank(&cands[i]) <= rank(&cands[j]),
                            "deferred chunk outranks an uploaded one (seed {seed})"
                        );
                    }
                }
            }
            assert!(upload_count(&plan) <= budget);
        }
    }

    #[test]
    fn prop_deterministic() {
        let cands: Vec<Candidate> = (0..32).map(|i| c(i, (i % 7) as i32, (i % 3) as u32)).collect();
        let a = plan_uploads(&cands, 100, 10);
        let b = plan_uploads(&cands, 100, 10);
        assert_eq!(a, b);
    }
}
