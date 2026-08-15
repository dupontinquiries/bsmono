//! graph — the commit history as an actual graph, not a flat list.
//! `scene_kit::mesh3d::graph_scene` was built for logscope's conversation
//! graph (a node per turn, an edge per parent, the active tip lit); a git
//! history is exactly that shape (a node per commit, an edge per parent,
//! HEAD or whatever's previewed as the active tip). This module is the
//! layout — turning `Rev`s into positions and index-pairs `graph_scene`
//! can render directly.
//!
//! UPDATE: position now comes from `scene_kit::spatial::flow` — the SAME
//! layout logscope uses for its own conversation graph. `flow` was built
//! around exactly the two gaps this module used to name as deferred:
//! `bond_rest`'s logarithmic time-stretch (a burst of quick commits sits
//! tight; a long gap between work sessions visibly stretches the flow —
//! the graph's own LENGTH now carries information, not just its shape) and
//! real lane separation for forks (siblings fan into their own channels,
//! descendants inherit the lane — actual multi-lane layout, not a single
//! line pretending branches don't exist). `flow` takes ONE parent per node
//! for positioning purposes (`Option<usize>`); a merge commit's SECOND
//! parent edge still renders (this module's own `edges` list carries every
//! real parent, not just the one `flow` used for placement) — flow decides
//! where a node SITS, this module decides what LINES connect them, and
//! those are allowed to disagree slightly for a merge without either being
//! wrong.
//!
//! Honest scope, still: `history()`'s own path-scoped log is usually close
//! to linear anyway (git's default simplification drops most merges that
//! don't uniquely
//! touch the file), so a single lane is an honest fit for what's actually
//! being shown, not a corner cut on something that needed more.

use aperture_core::gitspace::Rev;
use egui::Color32;
use scene_kit::spatial::Vec3;

/// One node's mesh position + tint, and the parent edges among them (index
/// pairs into the same `revs` slice — `graph_scene`'s own contract).
pub struct Graph {
    pub centers: Vec<Vec3>,
    pub colors: Vec<Color32>,
    pub edges: Vec<(usize, usize)>,
    /// `revs`' index of the currently-active node (the previewed revision,
    /// or the newest when live) — `graph_scene`'s `active` param.
    pub active: Option<usize>,
}

/// Checkpoints (gitmind's shadow commits) read amber-ish; ordinary commits
/// read as cool stone — the same visual language `◈` already carries in
/// text everywhere else this session touched revisions.
fn node_color(rev: &Rev) -> Color32 {
    match rev.kind {
        aperture_core::gitspace::RevKind::Checkpoint(_) => Color32::from_rgb(214, 168, 90),
        aperture_core::gitspace::RevKind::Commit => Color32::from_rgb(150, 158, 168),
    }
}

/// Lay `revs` (newest-first, `history()`'s own order) out via
/// `scene_kit::spatial::flow` — time-stretched spacing, lanes for forks —
/// anchored so the flow's own local origin sits at `origin`. `flow`
/// requires a PARENT-BEFORE-CHILD (oldest-first) array with each parent
/// referenced by earlier index; `revs` is the opposite order, so this
/// reverses internally and maps the result back to `revs`' own indices —
/// callers (and `rev_ix`) never need to know the flow ran backwards.
pub fn layout(revs: &[Rev], rev_ix: Option<usize>, origin: Vec3) -> Graph {
    let n = revs.len();
    let colors: Vec<Color32> = revs.iter().map(node_color).collect();
    let mut edges = Vec::new();
    for (i, rev) in revs.iter().enumerate() {
        for p in &rev.parents {
            if let Some(j) = revs.iter().position(|v| &v.oid == p) {
                edges.push((i, j));
            }
        }
    }
    let active = rev_ix.or(if n > 0 { Some(0) } else { None });
    if n == 0 {
        return Graph { centers: Vec::new(), colors, edges, active };
    }

    // Oldest-first for flow's contract. `orig_of[k]` is revs' own index of
    // the k-th oldest entry; `new_of[i]` is the inverse, for translating
    // flow's per-k output back to revs' per-i order.
    let mut orig_of: Vec<usize> = (0..n).collect();
    orig_of.reverse();
    let mut new_of = vec![0usize; n];
    for (k, &i) in orig_of.iter().enumerate() {
        new_of[i] = k;
    }
    // flow's single parent-for-positioning: the FIRST parent oid that's
    // present in this window, mapped to its oldest-first index. Rendering
    // edges (above) already carry every real parent; this is placement only.
    let parents_opt: Vec<Option<usize>> = orig_of
        .iter()
        .map(|&i| {
            revs[i]
                .parents
                .iter()
                .find_map(|p| revs.iter().position(|v| &v.oid == p))
                .map(|j| new_of[j])
        })
        .collect();
    let times: Vec<f64> = orig_of.iter().map(|&i| revs[i].time as f64).collect();
    let flowed = scene_kit::spatial::flow(&parents_opt, &times);

    let centers: Vec<Vec3> = (0..n)
        .map(|i| {
            let p = flowed[new_of[i]];
            Vec3::new(origin.x + p.x, origin.y + p.y, origin.z + p.z)
        })
        .collect();
    Graph { centers, colors, edges, active }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aperture_core::gitspace::RevKind;

    fn rev(oid: &str, parents: &[&str]) -> Rev {
        Rev { oid: oid.into(), time: 0, subject: oid.into(), kind: RevKind::Commit, parents: parents.iter().map(|s| s.to_string()).collect() }
    }

    fn rev_at(oid: &str, time: i64, parents: &[&str]) -> Rev {
        Rev { oid: oid.into(), time, subject: oid.into(), kind: RevKind::Commit, parents: parents.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn nodes_order_oldest_to_newest_along_x_with_equal_steps_at_equal_time_gaps() {
        // `flow`'s core time-stretch math, isolated from its separate
        // "breathing room" relaxation pass (which — despite its own doc
        // comment only promising x-invariance for the pairwise term —
        // ALSO nudges x via a center-clearing push when nodes sit near the
        // origin; a real behavior found by this test's first run, not
        // assumed away). Disabling it via the documented env knob tests
        // the layout math this module actually relies on, in isolation
        // from a physically-motivated pass whose x-perturbation is a
        // documented tunable of logscope's own, not a bug.
        std::env::set_var("LOGSCOPE_TUNE_BREATHE", "0");
        let revs = vec![rev("c", &["b"]), rev("b", &["a"]), rev("a", &[])]; // newest-first
        let g = layout(&revs, None, Vec3::new(0.0, 5.0, 0.0));
        std::env::remove_var("LOGSCOPE_TUNE_BREATHE");
        assert_eq!(g.centers.len(), 3);
        // revs[2]="a" is oldest (smallest x), revs[0]="c" is newest (largest x).
        assert!(g.centers[2].x < g.centers[1].x, "a before b");
        assert!(g.centers[1].x < g.centers[0].x, "b before c");
        let step1 = g.centers[1].x - g.centers[2].x;
        let step2 = g.centers[0].x - g.centers[1].x;
        assert!((step1 - step2).abs() < 1e-4, "equal time gaps: equal steps ({step1} vs {step2})");
        assert!((step1 - 0.9).abs() < 1e-4, "flow's own base reach at dt=0: {step1}");
    }

    #[test]
    fn a_longer_time_gap_stretches_the_flow_further() {
        // Same two-commit shape, wildly different time gaps: the longer
        // gap must produce a LARGER x-step — flow's whole "a long pause
        // stretches the backbone" property, exercised end to end rather
        // than trusting the formula by inspection.
        let quick = vec![rev_at("b", 10, &["a"]), rev_at("a", 0, &[])];
        let slow = vec![rev_at("b", 10_000_000, &["a"]), rev_at("a", 0, &[])];
        let g_quick = layout(&quick, None, Vec3::new(0.0, 0.0, 0.0));
        let g_slow = layout(&slow, None, Vec3::new(0.0, 0.0, 0.0));
        let step_quick = g_quick.centers[0].x - g_quick.centers[1].x;
        let step_slow = g_slow.centers[0].x - g_slow.centers[1].x;
        assert!(step_slow > step_quick, "a ~4-month gap should reach further than a 10s one: {step_slow} vs {step_quick}");
    }

    #[test]
    fn edges_connect_parents_present_in_the_window_and_skip_those_outside_it() {
        // c's parent b is in-window; b's parent a is in-window; but if we
        // trim the window to just [c, b], b's parent a is now OUT of the
        // window and must not produce a dangling edge index.
        let full = vec![rev("c", &["b"]), rev("b", &["a"]), rev("a", &[])];
        let g_full = layout(&full, None, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(g_full.edges.len(), 2, "{:?}", g_full.edges);
        assert!(g_full.edges.contains(&(0, 1)));
        assert!(g_full.edges.contains(&(1, 2)));

        let trimmed = vec![rev("c", &["b"]), rev("b", &["a"])]; // "a" not fetched
        let g_trim = layout(&trimmed, None, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(g_trim.edges, vec![(0, 1)], "b's parent a is outside the window: no dangling edge");
    }

    #[test]
    fn a_merge_commit_produces_two_edges_from_one_node() {
        let revs = vec![rev("m", &["p1", "p2"]), rev("p1", &[]), rev("p2", &[])];
        let g = layout(&revs, None, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(g.edges.len(), 2);
        assert!(g.edges.contains(&(0, 1)) && g.edges.contains(&(0, 2)));
        // flow still places every node (it just uses ONE parent, the first
        // in window, for lane/position purposes) — no node goes missing
        // just because it has two parents.
        assert_eq!(g.centers.len(), 3);
    }

    #[test]
    fn active_defaults_to_newest_when_live_and_follows_rev_ix_when_previewing() {
        let revs = vec![rev("c", &[]), rev("b", &[]), rev("a", &[])];
        let g_live = layout(&revs, None, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(g_live.active, Some(0), "live: the newest is active");
        let g_prev = layout(&revs, Some(2), Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(g_prev.active, Some(2));
    }

    #[test]
    fn empty_history_is_a_true_noop() {
        let g = layout(&[], None, Vec3::new(0.0, 0.0, 0.0));
        assert!(g.centers.is_empty() && g.edges.is_empty() && g.active.is_none());
    }
}
