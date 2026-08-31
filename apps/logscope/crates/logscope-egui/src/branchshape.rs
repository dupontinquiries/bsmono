//! branchshape — what each branch of the conversation actually IS, as a small descriptor.
//!
//! ## The seam this fills
//!
//! `scene3d::region_plan` picks a room type per side-wing of the mind palace, and its own comment
//! says what it is doing instead:
//!
//! > SHAPE SOURCE (the seam): real per-branch graph shape — is-loop / depth / length / token-mass /
//! > is-pruned — is not yet plumbed from the graph into `EnvParams` … Until then the type is derived
//! > DETERMINISTICALLY from `(seed, branch index)`.
//!
//! So the palace's rooms come from a hash. They are stable per conversation, which makes them
//! brain-sticky and hides the problem well: the same chat always builds the same palace, so it
//! feels meaningful. It is not. A branch that loops back and a branch that runs deep get whichever
//! rooms their index hashes to, and a pruned dead end is a sealed cave only by luck.
//!
//! That matters more here than a wrong colour would. The whole claim of a memory palace is that
//! PLACE ENCODES CONTENT — you remember where a thing was because the room was shaped by what it
//! held. A palace whose rooms are noise trains the opposite habit: it teaches you that location
//! means nothing, at which point the 3D view is decoration over a list.
//!
//! ## What is measured
//!
//! One descriptor per FORK (a node with more than one child), in fork order, matching the order
//! `bear` is computed in — the wings are placed by bearing and typed by shape, and a mismatch
//! between those two orders would give each wing another branch's character. That ordering is the
//! subtle failure here, so it is asserted in the tests rather than assumed.
//!
//! Everything is counted over the branch's own subtree — the descendants that leave the mainline at
//! that fork — because that is exactly the part of the conversation the wing beyond that door
//! stands for.

/// The shape of one branch, as small as it can be while still being Copy for `EnvParams`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BranchShape {
    /// Longest path from the fork to a leaf of this branch, in nodes. Saturates at 255 — a branch
    /// deeper than that is "very deep" and the exact figure changes nothing downstream.
    pub depth: u8,
    /// Total nodes in the branch. Saturating like `depth`.
    pub len: u8,
    /// Token mass of the branch, in units of 64 tokens, saturating. Coarse on purpose: this feeds a
    /// room-size decision and a cache key, and exact token counts would rebuild the world on every
    /// streamed token.
    pub tokens64: u16,
    /// Any node in the branch refers back to something earlier — the conversation looped.
    pub loops: bool,
    /// The whole branch is human-pruned: kept in the log, removed from the model's context. A dead
    /// end in the most literal sense.
    pub pruned: bool,
}

impl BranchShape {
    /// The "no branch here" value, usable in a `const` context (EnvParams lives in a thread-local
    /// Cell initialised at compile time).
    pub const EMPTY: BranchShape = BranchShape { depth: 0, len: 0, tokens64: 0, loops: false, pruned: false };

    /// Was a real branch measured into this slot? A wing with no shape falls back to the seeded mix
    /// rather than being typed as an empty dead end.
    pub fn is_some(self) -> bool { self.len > 0 }

    /// Is this branch substantial enough to deserve a large room? Token mass rather than node count:
    /// twenty one-line turns are not the same weight as three long ones.
    ///
    /// The threshold is a multiple of the scene cache's bucket (8 units of 64 tokens) on purpose:
    /// the cache quantises `tokens64` to keep a growing branch from rebuilding the world every
    /// token, and a threshold falling mid-bucket would make a wing's type depend on whether the
    /// value had been rounded yet — the same room flickering between Chamber and Terrace.
    pub fn heavy(self) -> bool { self.tokens64 >= 24 } // ~1500 tokens, and 24 = 3 * 8

    /// A branch that went deep rather than wide.
    pub fn deep(self) -> bool { self.depth >= 4 && self.depth * 2 >= self.len }
}

/// Compute one shape per fork, in ascending node id — the same order `chat.rs` computes bearings in.
///
/// `parents[i]` is node i's parent; `tokens[i]` and `pruned[i]` are that node's own values. Slices
/// shorter than `parents` are treated as zero/false rather than panicking: this runs on live UI
/// state while a conversation is streaming, and a torn read must degrade, not crash the frame.
pub fn analyze(parents: &[Option<usize>], tokens: &[usize], pruned: &[bool], looks_back: &[bool]) -> Vec<BranchShape> {
    let n = parents.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, p) in parents.iter().enumerate() {
        if let Some(p) = *p {
            // A parent id out of range, or a self-parent, is corrupt input rather than a tree; skip
            // the edge instead of trusting it into an infinite walk below.
            if p < n && p != i { children[p].push(i); }
        }
    }

    // Subtree rollups, computed leaf-first. Ids are not guaranteed topologically ordered, so this
    // walks children explicitly rather than assuming a parent precedes its children.
    let (mut size, mut depth, mut toks, mut all_pruned, mut any_loop) =
        (vec![1usize; n], vec![1usize; n], vec![0usize; n], vec![true; n], vec![false; n]);
    for i in 0..n {
        toks[i] = tokens.get(i).copied().unwrap_or(0);
        all_pruned[i] = pruned.get(i).copied().unwrap_or(false);
        any_loop[i] = looks_back.get(i).copied().unwrap_or(false);
    }
    for i in postorder(&children, n) {
        for &c in &children[i] {
            size[i] += size[c];
            depth[i] = depth[i].max(depth[c] + 1);
            toks[i] += toks[c];
            all_pruned[i] &= all_pruned[c];
            any_loop[i] |= any_loop[c];
        }
    }

    // The mainline: from the root, always follow the largest subtree. A branch is what LEAVES it.
    let mut mainline = vec![false; n];
    if n > 0 {
        let mut cur = 0usize;
        let mut guard = 0;
        loop {
            mainline[cur] = true;
            guard += 1;
            if guard > n { break; } // corrupt input cannot spin the UI thread
            match children[cur].iter().copied().max_by_key(|&c| size[c]) {
                Some(next) if next != cur => cur = next,
                _ => break,
            }
        }
    }

    let mut out = Vec::new();
    for f in 0..n {
        if children[f].len() < 2 { continue; }
        // Aggregate the off-mainline children: together they are the branch this door leads to.
        let off: Vec<usize> = children[f].iter().copied().filter(|&c| !mainline[c]).collect();
        if off.is_empty() { continue; }
        let len: usize = off.iter().map(|&c| size[c]).sum();
        let d = off.iter().map(|&c| depth[c]).max().unwrap_or(0);
        let t: usize = off.iter().map(|&c| toks[c]).sum();
        out.push(BranchShape {
            depth: d.min(255) as u8,
            len: len.min(255) as u8,
            tokens64: (t / 64).min(u16::MAX as usize) as u16,
            loops: off.iter().any(|&c| any_loop[c]),
            pruned: off.iter().all(|&c| all_pruned[c]),
        });
    }
    out
}

/// Children-first ordering, iteratively. Recursion here would blow the stack on a long conversation
/// — a linear chat of 5000 turns is a 5000-deep tree, and this runs on the UI thread.
fn postorder(children: &[Vec<usize>], n: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(n);
    let mut seen = vec![false; n];
    for root in 0..n {
        if seen[root] { continue; }
        let mut stack = vec![(root, false)];
        while let Some((i, expanded)) = stack.pop() {
            if expanded { out.push(i); continue; }
            if seen[i] { continue; }
            seen[i] = true;
            stack.push((i, true));
            for &c in &children[i] { if !seen[c] { stack.push((c, false)); } }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// parents from a compact edge list; node 0 is the root.
    fn tree(edges: &[(usize, usize)], n: usize) -> Vec<Option<usize>> {
        let mut p = vec![None; n];
        for &(parent, child) in edges { p[child] = Some(parent); }
        p
    }

    #[test]
    fn a_fork_is_measured_over_the_branch_that_leaves_the_mainline() {
        //      0 ── 1 ── 2 ── 3      (mainline: the long side)
        //           └── 4 ── 5       (the branch: 2 nodes, depth 2)
        let parents = tree(&[(0, 1), (1, 2), (2, 3), (1, 4), (4, 5)], 6);
        let toks = vec![10; 6];
        let s = analyze(&parents, &toks, &[false; 6], &[false; 6]);
        assert_eq!(s.len(), 1, "one fork, one branch");
        assert_eq!(s[0].len, 2, "the branch is nodes 4 and 5, not the whole subtree");
        assert_eq!(s[0].depth, 2);
        assert!(!s[0].pruned && !s[0].loops);
    }

    #[test]
    fn depth_and_length_tell_a_deep_branch_from_a_wide_one() {
        // deep: 0-1, 1 forks to a chain of 4
        let deep = analyze(&tree(&[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (1, 6), (6, 7), (7, 8), (8, 9)], 10),
                           &vec![1; 10], &[false; 10], &[false; 10]);
        assert!(deep[0].deep(), "a chain off the mainline is deep: {:?}", deep[0]);
        // wide: 1 forks to four leaves
        let wide = analyze(&tree(&[(0, 1), (1, 2), (2, 3), (1, 4), (1, 5), (1, 6), (1, 7)], 8),
                           &vec![1; 8], &[false; 8], &[false; 8]);
        assert!(!wide[0].deep(), "four siblings are wide, not deep: {:?}", wide[0]);
        assert_eq!(wide[0].len, 4);
        assert_eq!(wide[0].depth, 1);
    }

    /// MAINLINE IS THE LARGEST SUBTREE, so the branch is the smaller side. Stated in a fixture
    /// because the first version of these tests got it backwards: I built two-node-vs-one-node forks
    /// and asserted against the side I had in mind rather than the side the definition picks.
    /// Mainline 0-1-2-3-4-5, branch {6,7} off fork 1.
    fn forked_tree() -> Vec<Option<usize>> {
        tree(&[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (1, 6), (6, 7)], 8)
    }

    #[test]
    fn token_mass_is_the_weight_not_the_node_count() {
        let p = forked_tree();
        let light = analyze(&p, &vec![5; 8], &[false; 8], &[false; 8]);
        assert!(!light[0].heavy());
        let mut toks = vec![5usize; 8];
        toks[7] = 4000; // one long turn deep in the branch
        let heavy = analyze(&p, &toks, &[false; 8], &[false; 8]);
        assert!(heavy[0].heavy(), "one long turn outweighs several short ones: {:?}", heavy[0]);
    }

    #[test]
    fn a_branch_is_pruned_only_when_all_of_it_is() {
        let p = forked_tree();
        let mut pruned = vec![false; 8];
        pruned[6] = true;
        let partial = analyze(&p, &vec![1; 8], &pruned, &[false; 8]);
        assert!(!partial[0].pruned, "a live descendant means the branch is not a dead end");
        pruned[7] = true;
        assert!(analyze(&p, &vec![1; 8], &pruned, &[false; 8])[0].pruned);
    }

    #[test]
    fn a_look_back_anywhere_in_the_branch_makes_it_a_loop() {
        let p = forked_tree();
        let mut lb = vec![false; 8];
        lb[7] = true; // the deep end of the branch refers to something earlier
        assert!(analyze(&p, &vec![1; 8], &[false; 8], &lb)[0].loops);
        // a look-back on the MAINLINE is not this branch's loop
        let mut lb2 = vec![false; 8];
        lb2[3] = true;
        assert!(!analyze(&p, &vec![1; 8], &[false; 8], &lb2)[0].loops);
    }

    /// Fork order is load-bearing: wings are PLACED by `bear` and TYPED by this, both taken in
    /// ascending fork id, so a mismatch gives each wing another branch's character.
    #[test]
    fn forks_come_back_in_ascending_id_order() {
        //  0-1-2-3-4 mainline, with branches off 1 and 3
        let parents = tree(&[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (1, 6), (3, 7), (7, 8)], 9);
        let s = analyze(&parents, &vec![1; 9], &[false; 9], &[false; 9]);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].len, 1, "fork 1's branch is the single node 6");
        assert_eq!(s[1].len, 2, "fork 3's branch is nodes 7 and 8");
    }

    #[test]
    fn a_linear_conversation_has_no_branches_at_all() {
        let parents = tree(&[(0, 1), (1, 2), (2, 3)], 4);
        assert!(analyze(&parents, &vec![1; 4], &[false; 4], &[false; 4]).is_empty());
        assert!(analyze(&[], &[], &[], &[]).is_empty());
    }

    /// This runs on the UI thread against live state, so corrupt or torn input must degrade rather
    /// than hang or panic.
    #[test]
    fn corrupt_input_degrades_instead_of_hanging() {
        // a self-parent and an out-of-range parent
        let parents = vec![None, Some(1), Some(99), Some(0)];
        let s = analyze(&parents, &[1, 1, 1, 1], &[false; 4], &[false; 4]);
        assert!(s.len() <= 1);
        // short auxiliary slices are treated as zero/false, not indexed past
        let parents = tree(&[(0, 1), (1, 2), (1, 3)], 4);
        let s = analyze(&parents, &[], &[], &[]);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].tokens64, 0);
    }

    /// A long linear chat is a deep tree; a recursive rollup would blow the UI thread's stack.
    /// Saturation needs a branch over 255 nodes AND a mainline larger still — otherwise the long
    /// side simply IS the mainline, which is what the first version of this test actually built.
    #[test]
    fn a_very_deep_conversation_does_not_overflow_the_stack() {
        let (main_len, branch_len) = (4000usize, 400usize);
        let n = main_len + branch_len;
        let mut parents = vec![None; n];
        for i in 1..main_len { parents[i] = Some(i - 1); }
        parents[main_len] = Some(1); // the branch forks off node 1
        for i in main_len + 1..n { parents[i] = Some(i - 1); }
        let s = analyze(&parents, &vec![1; n], &[false; 8], &[false; 8]);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].len, 255, "saturates rather than wrapping");
        assert_eq!(s[0].depth, 255);
    }
}
