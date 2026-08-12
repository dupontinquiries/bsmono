//! graph — the commit history as an actual graph, not a flat list.
//! `scene_kit::mesh3d::graph_scene` was built for logscope's conversation
//! graph (a node per turn, an edge per parent, the active tip lit); a git
//! history is exactly that shape (a node per commit, an edge per parent,
//! HEAD or whatever's previewed as the active tip). This module is the
//! layout — turning `Rev`s into positions and index-pairs `graph_scene`
//! can render directly.
//!
//! Honest scope: this is a SINGLE LANE. Position is purely by time order
//! (newest at one end); an edge is drawn whenever a parent oid is also
//! present in the same fetched window, however far apart in time they land.
//! Real multi-lane DAG layout (git-log-graph's own lane-packing, keeping
//! parallel branches visually parallel) is real, harder, and not this pass
//! — `history()`'s own path-scoped log is usually close to linear anyway
//! (git's default simplification drops most merges that don't uniquely
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

/// Lay `revs` (newest-first, `history()`'s own order) along one axis
/// centered at `origin`, `spacing` apart, oldest at the far end. `rev_ix`
/// (the viewer's own `Option<usize>`, `None` = live/newest) becomes the
/// active node.
pub fn layout(revs: &[Rev], rev_ix: Option<usize>, origin: Vec3, spacing: f32, right: Vec3) -> Graph {
    let n = revs.len();
    let r = norm(right);
    let centers: Vec<Vec3> = (0..n)
        .map(|i| Vec3::new(origin.x + r.x * i as f32 * spacing, origin.y + r.y * i as f32 * spacing, origin.z + r.z * i as f32 * spacing))
        .collect();
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
    Graph { centers, colors, edges, active }
}

fn norm(v: Vec3) -> Vec3 {
    let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if len < 1e-6 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(v.x / len, v.y / len, v.z / len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aperture_core::gitspace::RevKind;

    fn rev(oid: &str, parents: &[&str]) -> Rev {
        Rev { oid: oid.into(), time: 0, subject: oid.into(), kind: RevKind::Commit, parents: parents.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn lays_out_evenly_spaced_along_the_given_axis() {
        let revs = vec![rev("c", &["b"]), rev("b", &["a"]), rev("a", &[])];
        let g = layout(&revs, None, Vec3::new(0.0, 5.0, 0.0), 2.0, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(g.centers.len(), 3);
        assert_eq!(g.centers[0], Vec3::new(0.0, 5.0, 0.0));
        assert_eq!(g.centers[1], Vec3::new(2.0, 5.0, 0.0));
        assert_eq!(g.centers[2], Vec3::new(4.0, 5.0, 0.0));
    }

    #[test]
    fn edges_connect_parents_present_in_the_window_and_skip_those_outside_it() {
        // c's parent b is in-window; b's parent a is in-window; but if we
        // trim the window to just [c, b], b's parent a is now OUT of the
        // window and must not produce a dangling edge index.
        let full = vec![rev("c", &["b"]), rev("b", &["a"]), rev("a", &[])];
        let g_full = layout(&full, None, Vec3::new(0.0, 0.0, 0.0), 1.0, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(g_full.edges.len(), 2, "{:?}", g_full.edges);
        assert!(g_full.edges.contains(&(0, 1)));
        assert!(g_full.edges.contains(&(1, 2)));

        let trimmed = vec![rev("c", &["b"]), rev("b", &["a"])]; // "a" not fetched
        let g_trim = layout(&trimmed, None, Vec3::new(0.0, 0.0, 0.0), 1.0, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(g_trim.edges, vec![(0, 1)], "b's parent a is outside the window: no dangling edge");
    }

    #[test]
    fn a_merge_commit_produces_two_edges_from_one_node() {
        let revs = vec![rev("m", &["p1", "p2"]), rev("p1", &[]), rev("p2", &[])];
        let g = layout(&revs, None, Vec3::new(0.0, 0.0, 0.0), 1.0, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(g.edges.len(), 2);
        assert!(g.edges.contains(&(0, 1)) && g.edges.contains(&(0, 2)));
    }

    #[test]
    fn active_defaults_to_newest_when_live_and_follows_rev_ix_when_previewing() {
        let revs = vec![rev("c", &[]), rev("b", &[]), rev("a", &[])];
        let g_live = layout(&revs, None, Vec3::new(0.0, 0.0, 0.0), 1.0, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(g_live.active, Some(0), "live: the newest is active");
        let g_prev = layout(&revs, Some(2), Vec3::new(0.0, 0.0, 0.0), 1.0, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(g_prev.active, Some(2));
    }

    #[test]
    fn empty_history_is_a_true_noop() {
        let g = layout(&[], None, Vec3::new(0.0, 0.0, 0.0), 1.0, Vec3::new(1.0, 0.0, 0.0));
        assert!(g.centers.is_empty() && g.edges.is_empty() && g.active.is_none());
    }
}
