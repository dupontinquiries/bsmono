//! group_graph — turns `Project::group_tangles()` into node positions and
//! edges, pure and deterministic, so the LAYOUT can be tested without a
//! display (see `tests` below) and the egui painting in `paint` is a thin,
//! separately-verified layer on top (checked with uikit-egui's headless
//! `ctx.run` harness instead, since paint code itself has no meaningful
//! return value to assert on).

use studio_core::{ClipId, GroupId, GroupTangle};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodePos {
    pub id: GroupId,
    pub x: f32,
    pub y: f32,
    /// Node radius scales with member count -- a bigger composition reads
    /// as a bigger circle, no legend needed.
    pub radius: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeSpec {
    pub a: GroupId,
    pub b: GroupId,
    /// Every clip id shared between `a` and `b` -- usually one, but
    /// nothing stops two groups sharing several clips at once.
    pub shared: Vec<ClipId>,
}

pub struct Layout {
    pub nodes: Vec<NodePos>,
    pub edges: Vec<EdgeSpec>,
}

/// One tangle per horizontal band (bands stacked top to bottom, `band_h`
/// apart); within a band, a tangle of N groups sits on a circle of radius
/// proportional to N so a bigger tangle doesn't cram its nodes together.
/// A tangle of exactly one group sits centered on its band with no circle
/// needed. Pure function of `tangles` and the canvas size -- same input,
/// same output, always, which is what makes it testable and what makes
/// the egui layer's animation (interpolating toward THESE targets) stable
/// frame to frame.
pub fn layout(tangles: &[GroupTangle], canvas_w: f32, canvas_h: f32) -> Layout {
    let mut nodes = Vec::new();
    let edges = Vec::new();
    if tangles.is_empty() {
        return Layout { nodes, edges };
    }

    let band_h = canvas_h / tangles.len() as f32;
    for (band_i, t) in tangles.iter().enumerate() {
        let cy = band_h * (band_i as f32 + 0.5);
        let cx = canvas_w * 0.5;
        let n = t.groups.len().max(1);
        let ring_r = if n <= 1 { 0.0 } else { (band_h * 0.35).min(canvas_w * 0.25).max(30.0) };
        let node_r = (10.0 + t.clips.len() as f32 * 1.5).min(band_h * 0.3);

        for (i, gid) in t.groups.iter().enumerate() {
            let angle = if n <= 1 { 0.0 } else { (i as f32 / n as f32) * std::f32::consts::TAU };
            let x = cx + ring_r * angle.cos();
            let y = cy + ring_r * angle.sin();
            nodes.push(NodePos { id: *gid, x, y, radius: node_r });
        }
    }

    Layout { nodes, edges }
}

/// The version `paint` actually calls: same node layout as [`layout`], but
/// with CORRECT per-pair edges (needs the `Project` to know which clips
/// each specific pair of groups shares, not just the tangle-wide union).
pub fn layout_with_project(project: &studio_core::Project, tangles: &[GroupTangle], canvas_w: f32, canvas_h: f32) -> Layout {
    let mut out = layout(tangles, canvas_w, canvas_h);
    for t in tangles {
        for i in 0..t.groups.len() {
            for j in (i + 1)..t.groups.len() {
                let (a, b) = (t.groups[i], t.groups[j]);
                let (Some(ga), Some(gb)) = (project.groups.get(&a), project.groups.get(&b)) else { continue };
                let shared: Vec<ClipId> = ga.clips.iter().filter(|c| gb.clips.contains(c) && project.clips.contains_key(c)).copied().collect();
                if !shared.is_empty() {
                    out.edges.push(EdgeSpec { a, b, shared });
                }
            }
        }
    }
    out
}

/// Draws the graph into `ui`, animating every node toward its layout
/// target position rather than snapping -- so adding/removing a
/// composition, or a shared clip appearing/disappearing, reads as motion
/// (something changed HERE) instead of a jump-cut redraw. Uses egui's own
/// per-widget animation memory (`ctx.animate_value_with_time`, keyed by
/// each node's `GroupId`), so no extra state needs to live on `App` --
/// the animation survives exactly as long as the panel stays open, which
/// is exactly the lifetime it should have.
pub fn paint(ui: &mut egui::Ui, project: &studio_core::Project, colors: &uikit_egui::PaletteColors) {
    let tangles = project.group_tangles();
    let rect = ui.available_rect_before_wrap();
    let target = layout_with_project(project, &tangles, rect.width().max(1.0), rect.height().max(1.0));

    if tangles.is_empty() {
        ui.centered_and_justified(|ui| ui.colored_label(colors.text_weak, "no compositions in this project"));
        return;
    }

    let painter = ui.painter_at(rect);
    let anim_secs = 0.35;

    let mut resolved: std::collections::BTreeMap<studio_core::GroupId, (f32, f32, f32)> = Default::default();
    let mut still_animating = false;
    for n in &target.nodes {
        let base = rect.min;
        let id_x = egui::Id::new(("group_graph_x", n.id));
        let id_y = egui::Id::new(("group_graph_y", n.id));
        let (tx, ty) = (base.x + n.x, base.y + n.y);
        let x = ui.ctx().animate_value_with_time(id_x, tx, anim_secs);
        let y = ui.ctx().animate_value_with_time(id_y, ty, anim_secs);
        if (x - tx).abs() > 0.5 || (y - ty).abs() > 0.5 { still_animating = true; }
        resolved.insert(n.id, (x, y, n.radius));
    }
    if still_animating { ui.ctx().request_repaint(); }

    for e in &target.edges {
        let (Some(&(ax, ay, _)), Some(&(bx, by, _))) = (resolved.get(&e.a), resolved.get(&e.b)) else { continue };
        painter.line_segment([egui::pos2(ax, ay), egui::pos2(bx, by)], egui::Stroke::new(1.5_f32, colors.text_weak));
        let mid = egui::pos2((ax + bx) * 0.5, (ay + by) * 0.5);
        let label = e.shared.iter().map(|c| c.0.to_string()).collect::<Vec<_>>().join(",");
        painter.text(mid, egui::Align2::CENTER_CENTER, format!("clip {label}"), egui::FontId::proportional(11.0), colors.text_weak);
    }

    for t in &tangles {
        for gid in &t.groups {
            let Some(&(x, y, r)) = resolved.get(gid) else { continue };
            let Some(g) = project.groups.get(gid) else { continue };
            let fill = if t.shared_clips.is_empty() { colors.accent } else { colors.warning };
            painter.circle_filled(egui::pos2(x, y), r, fill);
            painter.circle_stroke(egui::pos2(x, y), r, egui::Stroke::new(1.0_f32, colors.text));
            painter.text(egui::pos2(x, y), egui::Align2::CENTER_CENTER, &g.name, egui::FontId::proportional(12.0), colors.text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::GroupTangle;

    fn tangle(groups: Vec<u64>, clips: Vec<u64>) -> GroupTangle {
        GroupTangle {
            groups: groups.into_iter().map(GroupId).collect(),
            clips: clips.into_iter().map(ClipId).collect(),
            shared_clips: Vec::new(),
        }
    }

    #[test]
    fn empty_tangles_produce_an_empty_layout() {
        let l = layout(&[], 800.0, 600.0);
        assert!(l.nodes.is_empty() && l.edges.is_empty());
    }

    #[test]
    fn a_solo_group_sits_centered_with_no_ring_offset() {
        let l = layout(&[tangle(vec![1], vec![1, 2])], 800.0, 600.0);
        assert_eq!(l.nodes.len(), 1);
        assert_eq!(l.nodes[0].x, 400.0, "centered horizontally");
    }

    #[test]
    fn multiple_tangles_land_in_separate_non_overlapping_bands() {
        let l = layout(&[tangle(vec![1], vec![1]), tangle(vec![2], vec![2]), tangle(vec![3], vec![3])], 800.0, 600.0);
        assert_eq!(l.nodes.len(), 3);
        let ys: Vec<f32> = l.nodes.iter().map(|n| n.y).collect();
        // Each band is 200px tall (600/3); centers should be 100, 300, 500.
        assert_eq!(ys, vec![100.0, 300.0, 500.0]);
    }

    #[test]
    fn a_tangle_of_several_groups_spreads_them_around_a_ring_not_stacked() {
        let l = layout(&[tangle(vec![1, 2, 3], vec![1, 2, 3, 4])], 800.0, 600.0);
        assert_eq!(l.nodes.len(), 3);
        // No two nodes should land on the exact same point.
        for i in 0..3 {
            for j in (i + 1)..3 {
                let (a, b) = (l.nodes[i], l.nodes[j]);
                let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
                assert!(d > 1.0, "nodes {i} and {j} are stacked on top of each other");
            }
        }
    }

    #[test]
    fn bigger_compositions_get_bigger_nodes() {
        let l = layout(&[tangle(vec![1], vec![1, 2]), tangle(vec![2], (1..=20).collect())], 800.0, 600.0);
        assert!(l.nodes[1].radius > l.nodes[0].radius, "20-clip composition should draw larger than a 2-clip one");
    }

    /// The paint function actually runs and draws SOMETHING, checked the
    /// way this monorepo checks headless egui panes: rasterise a real
    /// `egui::Context` pass and measure "ink" (fraction of non-background
    /// pixels) -- the failure a compile check alone cannot catch (a pane
    /// that runs cleanly but draws zero shapes).
    #[test]
    fn paint_actually_draws_something_for_a_tangled_project() {
        use studio_core::{Clip, ClipId, GroupId, Op, Project, Source, SourceId, Ticks, Track, TrackId, TrackKind};
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "t".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "s.wav".into(),
            len: Ticks(1_000_000), sample_rate: 48_000, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        for i in 0..3u64 {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(i + 1), source: SourceId(1), track: TrackId(1),
                pos: Ticks(i as i64 * 1000), src_in: Ticks::ZERO, len: Ticks(500), gain: 1.0, effects: vec![] } }).unwrap();
        }
        p.apply(&Op::SetGroup { id: GroupId(1), name: "hook".into(), clips: vec![ClipId(1), ClipId(2)] }).unwrap();
        p.apply(&Op::SetGroup { id: GroupId(2), name: "bridge".into(), clips: vec![ClipId(2), ClipId(3)] }).unwrap();

        let colors = uikit_egui::PaletteColors::from_palette(&snapclone_theme::palette::DARK);
        let size = egui::Vec2::new(400.0, 300.0);
        let frame = uikit_egui::uiverify::render(size, colors.bg_base, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                paint(ui, &p, &colors);
            });
        });

        let ink = frame.ink(colors.bg_base);
        assert!(ink > 0.0, "paint() drew nothing at all for a two-group tangle -- expected circles, edges and labels");
    }

    /// The empty case must not panic either, and should draw the
    /// "no compositions" message (some ink), not a blank pane that looks
    /// broken rather than empty-on-purpose.
    #[test]
    fn paint_handles_an_empty_project_without_panicking() {
        let p = studio_core::Project::default();
        let colors = uikit_egui::PaletteColors::from_palette(&snapclone_theme::palette::DARK);
        let size = egui::Vec2::new(400.0, 300.0);
        let frame = uikit_egui::uiverify::render(size, colors.bg_base, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                paint(ui, &p, &colors);
            });
        });
        let _ = frame.ink(colors.bg_base); // just proving this doesn't panic
    }
}
