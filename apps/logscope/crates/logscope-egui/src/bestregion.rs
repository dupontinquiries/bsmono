//! bestregion.rs — pick the subtree the verify oracle likes best, the way the summary agent does it.
//! Pure: takes the tree as a parent list + a per-node verdict, returns the region (root + members) with
//! the most *verified-good* nodes. This is the real, tested version of the mockup's "summarize best
//! region" selection; wiring it to the live graph + a verdict source (the sandbox `cargo check` oracle,
//! or a cheap heuristic until that's per-node) is a thin layer on top. The principle, same as the
//! depth-search framing: rank by what actually held up, not by how confident the model sounded.

/// Per-node verdict. Maps onto the sandbox oracle's classify() output; `None` = not (yet) judged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict { Pass, Partial, Fail, None }

#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub root: usize,
    pub members: Vec<usize>, // the subtree under `root`, root first
    pub pass: usize,         // verified-good nodes in the subtree
    pub total: usize,        // verified (non-None) nodes in the subtree
    pub max_depth: usize,    // deepest node in the region (depth-search reach)
}
impl Region {
    pub fn ratio(&self) -> f64 { if self.total == 0 { 0.0 } else { self.pass as f64 / self.total as f64 } }
}

fn children(parents: &[Option<usize>], id: usize) -> Vec<usize> {
    (0..parents.len()).filter(|&i| parents[i] == Some(id)).collect()
}

/// Depth of each node (root = 0), using the recorded parent (no linear fallback — callers pass explicit
/// parents; the chat layer resolves the `or(prev)` rule before calling).
fn depths(parents: &[Option<usize>]) -> Vec<usize> {
    let n = parents.len();
    let mut d = vec![0usize; n];
    // parents may not be topologically ordered; iterate to a fixpoint (tree → at most depth passes,
    // bounded by n so a malformed cycle can't hang).
    for _ in 0..n {
        let mut changed = false;
        for i in 0..n {
            if let Some(p) = parents[i] {
                if p < n && d[i] != d[p] + 1 { d[i] = d[p] + 1; changed = true; }
            }
        }
        if !changed { break; }
    }
    d
}

/// Walk the subtree rooted at `root`, collecting members + verdict tallies.
fn subtree(parents: &[Option<usize>], verdict: &[Verdict], depth: &[usize], root: usize) -> Region {
    let mut members = Vec::new();
    let mut stack = vec![root];
    let (mut pass, mut total, mut max_depth) = (0usize, 0usize, depth.get(root).copied().unwrap_or(0));
    while let Some(id) = stack.pop() {
        members.push(id);
        max_depth = max_depth.max(depth.get(id).copied().unwrap_or(0));
        match verdict.get(id).copied().unwrap_or(Verdict::None) {
            Verdict::None => {}
            Verdict::Pass => { pass += 1; total += 1; }
            Verdict::Partial | Verdict::Fail => { total += 1; }
        }
        for c in children(parents, id) { stack.push(c); }
    }
    members.sort_unstable();
    Region { root, members, pass, total, max_depth }
}

/// Pick the best region: among subtrees with at least `min_verified` verified nodes and pass-ratio at
/// least `min_ratio`, the one with the most passing nodes (tie → higher ratio → deeper → smaller root id
/// for determinism). Returns None if nothing qualifies (e.g. nothing verified yet). Favors a *substantial
/// mostly-green* region over a tiny perfect one — the agent wants the branch that held up at scale.
pub fn best_region(parents: &[Option<usize>], verdict: &[Verdict], min_verified: usize, min_ratio: f64) -> Option<Region> {
    let n = parents.len();
    if n == 0 { return None; }
    let depth = depths(parents);
    let mut best: Option<Region> = None;
    for root in 0..n {
        // a region begins at a node that was itself verified — the conversation's user-turn root (and any
        // not-yet-judged node) shouldn't be reported as "the region that compiled best".
        if verdict.get(root).copied().unwrap_or(Verdict::None) == Verdict::None { continue; }
        let r = subtree(parents, verdict, &depth, root);
        if r.total < min_verified.max(1) || r.ratio() < min_ratio { continue; }
        let better = match &best {
            None => true,
            Some(b) => {
                use std::cmp::Ordering::*;
                matches!(
                    r.pass.cmp(&b.pass)
                        .then(r.ratio().partial_cmp(&b.ratio()).unwrap_or(Equal))
                        .then(r.max_depth.cmp(&b.max_depth))
                        .then(b.root.cmp(&r.root)), // smaller root id wins, for determinism
                    Greater
                )
            }
        };
        if better { best = Some(r); }
    }
    best
}

/// Every branch tip (no node lists it as a parent). Mirrors the chat layer's `leaf_ids`, kept here so the
/// module is self-contained/testable; callers pass parents with the linear fallback already resolved.
pub fn leaf_ids(parents: &[Option<usize>]) -> Vec<usize> {
    let n = parents.len();
    let mut is_parent = vec![false; n];
    for i in 0..n { if let Some(p) = parents[i] { if p < n { is_parent[p] = true; } } }
    (0..n).filter(|&i| !is_parent[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use Verdict::{Pass, Partial, Fail};

    // tree:  0 ──1(pass)──2(pass)──3(pass)        (a deep green line)
    //        └──4(fail)──5(partial)               (a weak branch)
    //        └──6(pass)                           (a lone green leaf)
    fn fixture() -> (Vec<Option<usize>>, Vec<Verdict>) {
        let parents = vec![None, Some(0), Some(1), Some(2), Some(0), Some(4), Some(0)];
        let verdict = vec![Verdict::None, Pass, Pass, Pass, Fail, Partial, Pass];
        (parents, verdict)
    }

    #[test]
    fn picks_the_deep_green_line() {
        let (p, v) = fixture();
        let r = best_region(&p, &v, 2, 0.6).expect("a region qualifies");
        // subtree at 1 has 3 verified, all pass (ratio 1.0) and is the deepest → best
        assert_eq!(r.root, 1);
        assert_eq!((r.pass, r.total), (3, 3));
        assert_eq!(r.max_depth, 3);
    }

    #[test]
    fn rejects_when_nothing_verified() {
        let p = vec![None, Some(0), Some(1)];
        let v = vec![Verdict::None, Verdict::None, Verdict::None];
        assert!(best_region(&p, &v, 1, 0.5).is_none());
    }

    #[test]
    fn ratio_gate_excludes_mostly_red() {
        let p = vec![None, Some(0), Some(1), Some(2)];
        let v = vec![Verdict::None, Fail, Fail, Pass]; // subtree at 1: 1/3 pass = 0.33 ratio
        assert!(best_region(&p, &v, 3, 0.6).is_none(), "below ratio gate → no region");
        // lower the gate and it qualifies (root 1, the only ≥3-verified subtree)
        assert_eq!(best_region(&p, &v, 3, 0.3).unwrap().root, 1);
    }

    #[test]
    fn prefers_substantial_region_over_tiny_perfect_one() {
        // root 1: 4 verified, 3 pass (0.75).  root 5: 1 verified, 1 pass (1.0).
        let p = vec![None, Some(0), Some(1), Some(2), Some(3), Some(0)];
        let v = vec![Verdict::None, Pass, Pass, Partial, Pass, Pass];
        let r = best_region(&p, &v, 1, 0.5).unwrap();
        assert_eq!(r.root, 1, "more passing nodes wins over a lone perfect leaf");
        assert_eq!(r.pass, 3);
    }

    #[test]
    fn leaves_are_branch_tips() {
        let (p, _) = fixture();
        let mut l = leaf_ids(&p); l.sort_unstable();
        assert_eq!(l, vec![3, 5, 6]);
    }
}
