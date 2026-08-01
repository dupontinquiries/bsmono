//! branchtree.rs — enumerate every branch of a chat, not just the active one.
//!
//! A workspace export walked `active_leaf` back through `parent` and wrote that single chain. Every
//! other branch — the alternatives a fan-out produced, the ones a rollback abandoned — was silently
//! dropped. The export reported success and the count of turns it saved, so nothing indicated that most
//! of the session was missing.
//!
//! That is the worst shape for a data-loss bug: it succeeds, it reports a number, and the number is
//! about the part that survived.
//!
//! Branches are what a fan-out is FOR. Exporting one of them is exporting the conclusion and discarding
//! the comparison that justified it.

/// A message's position in the tree.
pub trait Node {
    fn parent(&self) -> Option<usize>;
}

/// One root-to-leaf path.
#[derive(Clone, PartialEq, Debug)]
pub struct Branch {
    /// Message indices, root first.
    pub path: Vec<usize>,
    /// The leaf, which names the branch.
    pub leaf: usize,
    /// Index at which this path last agreed with the active branch, if it diverges from it.
    pub diverged_at: Option<usize>,
}

/// Every leaf in the tree: a message that is nobody's parent.
///
/// Order is by index so two exports of the same chat produce the same layout — a directory name that
/// moves between runs makes diffs useless, which is most of what an exported tree is for.
pub fn leaves<N: Node>(nodes: &[N]) -> Vec<usize> {
    let mut is_parent = vec![false; nodes.len()];
    for n in nodes {
        if let Some(p) = n.parent() {
            if p < is_parent.len() { is_parent[p] = true; }
        }
    }
    (0..nodes.len()).filter(|i| !is_parent[*i]).collect()
}

/// Walk a leaf back to its root.
///
/// Guards against a cycle rather than trusting the tree: a parent pointer corrupted by an edit would
/// otherwise hang the export, and an export that never returns is harder to diagnose than one that
/// returns a short path.
pub fn path_to_root<N: Node>(nodes: &[N], leaf: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cur = Some(leaf);
    while let Some(i) = cur {
        if i >= nodes.len() || !seen.insert(i) { break; }
        out.push(i);
        cur = nodes[i].parent();
    }
    out.reverse();
    out
}

/// Every branch, with divergence marked against the active one.
pub fn branches<N: Node>(nodes: &[N], active_leaf: Option<usize>) -> Vec<Branch> {
    let active_path = active_leaf.map(|l| path_to_root(nodes, l)).unwrap_or_default();
    leaves(nodes).into_iter().map(|leaf| {
        let path = path_to_root(nodes, leaf);
        // Where this path stops agreeing with the active one. None when it IS the active path, so a
        // reader can tell the main line from an alternative without comparing leaf indices.
        let diverged_at = if Some(leaf) == active_leaf { None } else {
            path.iter().zip(active_path.iter()).position(|(a, b)| a != b).or(Some(path.len().min(active_path.len())))
        };
        Branch { path, leaf, diverged_at }
    }).collect()
}

/// A stable directory name for a branch.
///
/// `main` for the active branch and `branch-<leaf>` for the rest, so the interesting one is obvious in
/// a listing and the others sort together.
pub fn dir_name(b: &Branch) -> String {
    match b.diverged_at {
        None => "main".to_string(),
        Some(_) => format!("branch-{:03}", b.leaf),
    }
}

/// A summary line per branch, for a manifest written beside the export.
///
/// Without this an exported tree is a set of directories with no indication of how they relate — which
/// is the same loss as before, moved one level up.
pub fn manifest(branches: &[Branch]) -> String {
    let mut s = format!("{} branch(es)\n", branches.len());
    for b in branches {
        match b.diverged_at {
            None => s.push_str(&format!("  {:12} {} turns (active)\n", dir_name(b), b.path.len())),
            Some(d) => s.push_str(&format!("  {:12} {} turns, diverges after turn {}\n",
                                           dir_name(b), b.path.len(), d)),
        }
    }
    s
}

/// One turn, with EVERY variant it carries.
///
/// A message holds `variants: Vec<String>` and renders `variants[cur]`. An export that writes `text()`
/// keeps the selected regeneration and drops the rest — so a turn that was regenerated four times
/// exports as though it were written once, and the comparison that made someone pick the fourth is gone.
///
/// The same loss as dropping branches, one level down: the alternatives ARE the record of the decision.
#[derive(Clone, PartialEq, Debug)]
pub struct Turn {
    pub index: usize,
    pub role: String,
    /// All variants, in the order they were produced.
    pub variants: Vec<String>,
    /// Which one was selected when the export ran.
    pub selected: usize,
}

impl Turn {
    pub fn selected_text(&self) -> &str {
        self.variants.get(self.selected).map(String::as_str).unwrap_or("")
    }
    /// Did this turn have alternatives that an export would otherwise lose?
    pub fn has_alternatives(&self) -> bool { self.variants.len() > 1 }

    /// Filenames for this turn: the selected one plain, the others suffixed.
    ///
    /// The selected variant keeps the unadorned name so a reader opening the directory sees the
    /// conversation as it stood, and has to go looking for the alternatives rather than the reverse.
    pub fn file_names(&self, ext: &str) -> Vec<(String, &str)> {
        self.variants.iter().enumerate().map(|(i, v)| {
            let n = if i == self.selected { format!("{:03}-{}.{ext}", self.index, self.role) }
                    else { format!("{:03}-{}.alt{}.{ext}", self.index, self.role, i) };
            (n, v.as_str())
        }).collect()
    }
}

/// Count what an export would drop if it wrote only selected variants.
///
/// Reported in the manifest, because the original bug succeeded silently — a number about what was
/// saved, with nothing about what was not.
pub fn dropped_if_selected_only(turns: &[Turn]) -> usize {
    turns.iter().map(|t| t.variants.len().saturating_sub(1)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct M(Option<usize>);
    impl Node for M { fn parent(&self) -> Option<usize> { self.0 } }

    /// 0 -> 1 -> 2, and 1 -> 3. Two leaves: 2 and 3.
    fn forked() -> Vec<M> { vec![M(None), M(Some(0)), M(Some(1)), M(Some(1))] }

    #[test]
    fn every_leaf_is_found_not_just_the_active_one() {
        // The bug: the export walked one chain and reported the turns it saved, so nothing indicated
        // that the alternatives — which are what a fan-out is FOR — had been dropped.
        assert_eq!(leaves(&forked()), vec![2, 3]);
    }

    #[test]
    fn each_branch_is_a_full_path_from_the_root() {
        let bs = branches(&forked(), Some(2));
        let b3 = bs.iter().find(|b| b.leaf == 3).unwrap();
        assert_eq!(b3.path, vec![0, 1, 3], "shared prefix is included, not just the divergent tail");
    }

    #[test]
    fn the_active_branch_is_named_main_and_the_others_are_not() {
        // So the interesting one is obvious in a listing rather than requiring a leaf index lookup.
        let bs = branches(&forked(), Some(2));
        let active = bs.iter().find(|b| b.leaf == 2).unwrap();
        assert_eq!(dir_name(active), "main");
        assert!(active.diverged_at.is_none());
        let other = bs.iter().find(|b| b.leaf == 3).unwrap();
        assert_eq!(dir_name(other), "branch-003");
        assert_eq!(other.diverged_at, Some(2), "agrees through index 1, differs at position 2");
    }

    #[test]
    fn a_cycle_in_the_parents_terminates_instead_of_hanging() {
        // A corrupted parent pointer would otherwise make the export never return, which is harder to
        // diagnose than a short path.
        let cyc = vec![M(Some(1)), M(Some(0))];
        let p = path_to_root(&cyc, 0);
        assert!(p.len() <= 2, "terminated: {p:?}");
    }

    #[test]
    fn a_linear_chat_produces_exactly_one_branch() {
        // The common case must not gain a directory level it does not need.
        let lin = vec![M(None), M(Some(0)), M(Some(1))];
        let bs = branches(&lin, Some(2));
        assert_eq!(bs.len(), 1);
        assert_eq!(dir_name(&bs[0]), "main");
    }

    #[test]
    fn the_manifest_says_where_each_branch_diverges() {
        // Without it, an exported tree is directories with no stated relationship — the same loss as
        // before, moved one level up.
        let m = manifest(&branches(&forked(), Some(2)));
        assert!(m.contains("2 branch(es)"));
        assert!(m.contains("main") && m.contains("(active)"));
        assert!(m.contains("diverges after turn"));
    }

    fn turn(i: usize, n: usize, sel: usize) -> Turn {
        Turn { index: i, role: "assistant".into(),
               variants: (0..n).map(|k| format!("v{k}")).collect(), selected: sel }
    }

    #[test]
    fn every_variant_gets_a_file_not_just_the_selected_one() {
        // The second layer of the same bug. A turn regenerated four times exports as though it were
        // written once, and the comparison that made someone pick the fourth is gone.
        let t = turn(7, 3, 1);
        let f = t.file_names("md");
        assert_eq!(f.len(), 3);
        assert!(f.iter().any(|(n, _)| n == "007-assistant.md"), "selected keeps the plain name");
        assert!(f.iter().any(|(n, _)| n == "007-assistant.alt0.md"));
        assert!(f.iter().any(|(n, _)| n == "007-assistant.alt2.md"));
    }

    #[test]
    fn the_selected_variant_keeps_the_unadorned_name() {
        // So a reader opening the directory sees the conversation as it stood, and has to go looking
        // for the alternatives rather than the reverse.
        let t = turn(2, 4, 3);
        let (name, body) = t.file_names("md").into_iter().find(|(n, _)| !n.contains(".alt")).unwrap();
        assert_eq!(name, "002-assistant.md");
        assert_eq!(body, "v3", "and it is the one that was selected, not variant 0");
    }

    #[test]
    fn a_single_variant_turn_gains_no_alt_files() {
        // The common case must not acquire clutter.
        let t = turn(1, 1, 0);
        assert!(!t.has_alternatives());
        assert_eq!(t.file_names("md").len(), 1);
    }

    #[test]
    fn the_count_of_what_would_be_dropped_is_reportable() {
        // The original bug succeeded silently — a number about what was saved, with nothing about what
        // was not. This is the number that would have shown it.
        let ts = vec![turn(0, 1, 0), turn(1, 4, 2), turn(2, 2, 0)];
        assert_eq!(dropped_if_selected_only(&ts), 4, "3 alts on turn 1 plus 1 on turn 2");
    }

    #[test]
    fn a_selected_index_past_the_end_yields_empty_not_a_panic() {
        // Corrupt state must not take the export down with it — a partial export is recoverable, a
        // crash during one loses everything including what was already written.
        let t = Turn { index: 0, role: "user".into(), variants: vec![], selected: 3 };
        assert_eq!(t.selected_text(), "");
    }

    #[test]
    fn ordering_is_stable_across_runs() {
        // A directory name that moves between exports makes diffs useless, which is most of what an
        // exported tree is for.
        let a = branches(&forked(), Some(2));
        let b = branches(&forked(), Some(2));
        assert_eq!(a.iter().map(dir_name).collect::<Vec<_>>(),
                   b.iter().map(dir_name).collect::<Vec<_>>());
    }
}
