//! v0.1.758: subgraph group boxes. Mermaid `subgraph ... end` blocks are parsed
//! into MermaidGraph.groups (id, title, member node ids) but were never drawn.
//! This module computes, from the FINAL laid-out card positions, the bounding
//! rectangle for each group so the renderer can draw a labeled container behind
//! its members — the "helps me learn like mermaid" feature.
//!
//! Pure: takes (cards, groups) → Vec<GroupBox>. No layout mutation, no I/O. The
//! actual box drawing is QML on top of this data.

/// A laid-out card the box computation reads (mirrors graph_layout::CardPos's
/// shape but kept local so this module stays dependency-free / testable alone).
#[derive(Debug, Clone)]
pub struct PlacedCard {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub half_w: f32,
    pub half_h: f32,
}

/// A parsed group (mirrors mermaid::SubgraphInfo's relevant fields).
#[derive(Debug, Clone)]
pub struct Group {
    pub id: String,
    pub title: String,
    pub members: Vec<String>,
}

/// The computed container box for one group, in the same coordinate space as
/// the cards. `x,y` is the top-left; `w,h` the size (already padded).
#[derive(Debug, Clone, PartialEq)]
pub struct GroupBox {
    pub id: String,
    pub title: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// nesting depth (0 = outermost); deeper boxes draw on top with tighter
    /// padding so nested groups read as nested.
    pub depth: u32,
    /// how many members were actually found + placed (boxes with <2 are skipped
    /// by compute_group_boxes, but the count is exposed for diagnostics).
    pub member_count: usize,
}

/// Padding (in card-space px) between the member extents and the box edge.
const BASE_PAD: f32 = 18.0;
/// Extra top padding to leave room for the title label.
const TITLE_PAD: f32 = 16.0;
/// Each nesting level tightens padding so nested boxes don't touch the parent.
const DEPTH_STEP: f32 = 8.0;

/// Match a group member id against a placed card id, tolerating the renderer's
/// `gn_` namespace prefix on either side (the layout namespaces ids; the parsed
/// members are raw). Compares the un-prefixed tails.
fn id_matches(member: &str, card_id: &str) -> bool {
    fn strip(s: &str) -> &str { s.strip_prefix("gn_").unwrap_or(s) }
    strip(member) == strip(card_id)
}

/// Compute the nesting depth of each group: a group nested inside another (all
/// its members are also members of the other) gets depth = parent.depth + 1.
/// Returns depth per group index. O(g^2 * m) — fine for the handful of groups a
/// diagram has.
fn is_subset(a: &Group, b: &Group) -> bool {
    // a ⊂ b (strictly fewer members, all of a's in b)
    if a.members.len() >= b.members.len() { return false; }
    a.members.iter().all(|m| b.members.iter().any(|bm| id_matches(m, bm)))
}

/// Compute the nesting depth of each group: a group nested inside another (all
/// its members are also members of the other) gets depth = parent.depth + 1.
/// Returns depth per group index. O(g^2 * m) — fine for the handful of groups a
/// diagram has.
fn group_depths(groups: &[Group]) -> Vec<u32> {
    groups.iter().map(|g| {
        groups.iter().filter(|other| {
            other.id != g.id && is_subset(g, other)
        }).count() as u32
    }).collect()
}

/// Compute a labeled bounding box per group from the laid-out cards. Groups with
/// fewer than 2 placed members are skipped (a box around 0–1 cards adds noise).
/// Boxes are returned sorted outermost-first (ascending depth) so the renderer
/// can paint parents before children.
pub fn compute_group_boxes(cards: &[PlacedCard], groups: &[Group]) -> Vec<GroupBox> {
    let depths = group_depths(groups);
    let mut out: Vec<GroupBox> = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        let mut minx = f32::MAX;
        let mut miny = f32::MAX;
        let mut maxx = f32::MIN;
        let mut maxy = f32::MIN;
        let mut found = 0usize;
        for m in &g.members {
            if let Some(c) = cards.iter().find(|c| id_matches(m, &c.id)) {
                minx = minx.min(c.x - c.half_w);
                miny = miny.min(c.y - c.half_h);
                maxx = maxx.max(c.x + c.half_w);
                maxy = maxy.max(c.y + c.half_h);
                found += 1;
            }
        }
        if found < 2 { continue; }
        let depth = depths[gi];
        let pad = (BASE_PAD - depth as f32 * DEPTH_STEP).max(6.0);
        let x = minx - pad;
        let y = miny - pad - TITLE_PAD;
        let w = (maxx + pad) - x;
        let h = (maxy + pad) - y;
        out.push(GroupBox {
            id: g.id.clone(),
            title: g.title.clone(),
            x, y, w, h, depth,
            member_count: found,
        });
    }
    out.sort_by_key(|b| b.depth);
    out
}

/// Serialize group boxes to the wire format the QML reads, one per line:
/// `id\x1Ftitle\x1Fx\x1Fy\x1Fw\x1Fh\x1Fdepth`. Coordinates rounded to 1 decimal.
pub fn boxes_to_wire(boxes: &[GroupBox]) -> String {
    let mut s = String::new();
    for b in boxes {
        s.push_str(&format!("{}\u{1F}{}\u{1F}{:.1}\u{1F}{:.1}\u{1F}{:.1}\u{1F}{:.1}\u{1F}{}\n",
            b.id, b.title, b.x, b.y, b.w, b.h, b.depth));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, x: f32, y: f32) -> PlacedCard {
        PlacedCard { id: id.to_string(), x, y, half_w: 30.0, half_h: 20.0 }
    }

    #[test]
    fn box_wraps_members_with_padding() {
        let cards = vec![card("a", 100.0, 100.0), card("b", 200.0, 100.0)];
        let groups = vec![Group { id: "g1".into(), title: "Mem".into(),
            members: vec!["a".into(), "b".into()] }];
        let boxes = compute_group_boxes(&cards, &groups);
        assert_eq!(boxes.len(), 1);
        let b = &boxes[0];
        // members span x: 70..230, y: 80..120; +18 pad, top +16 title
        assert_eq!(b.x, 70.0 - 18.0);
        assert_eq!(b.y, 80.0 - 18.0 - 16.0);
        assert!(b.w > 160.0 && b.h > 40.0);
        assert_eq!(b.title, "Mem");
        assert_eq!(b.member_count, 2);
    }

    #[test]
    fn singleton_or_empty_groups_skipped() {
        let cards = vec![card("a", 0.0, 0.0)];
        let groups = vec![
            Group { id: "solo".into(), title: "Solo".into(), members: vec!["a".into()] },
            Group { id: "empty".into(), title: "Empty".into(), members: vec!["zzz".into()] },
        ];
        assert!(compute_group_boxes(&cards, &groups).is_empty());
    }

    #[test]
    fn gn_namespace_prefix_tolerated() {
        // cards carry the gn_ prefix; members are raw — must still match.
        let cards = vec![card("gn_a", 0.0, 0.0), card("gn_b", 100.0, 0.0)];
        let groups = vec![Group { id: "g".into(), title: "T".into(),
            members: vec!["a".into(), "b".into()] }];
        assert_eq!(compute_group_boxes(&cards, &groups).len(), 1);
    }

    #[test]
    fn nested_groups_get_increasing_depth_and_tighter_pad() {
        let cards = vec![card("a", 0.0, 0.0), card("b", 100.0, 0.0), card("c", 200.0, 0.0)];
        let groups = vec![
            Group { id: "outer".into(), title: "Outer".into(),
                members: vec!["a".into(), "b".into(), "c".into()] },
            Group { id: "inner".into(), title: "Inner".into(),
                members: vec!["a".into(), "b".into()] },
        ];
        let boxes = compute_group_boxes(&cards, &groups);
        assert_eq!(boxes.len(), 2);
        // sorted outermost-first
        assert_eq!(boxes[0].id, "outer");
        assert_eq!(boxes[0].depth, 0);
        assert_eq!(boxes[1].id, "inner");
        assert_eq!(boxes[1].depth, 1);
    }

    #[test]
    fn wire_round_trips_fields() {
        let cards = vec![card("a", 100.0, 100.0), card("b", 200.0, 100.0)];
        let groups = vec![Group { id: "g1".into(), title: "Data Memory".into(),
            members: vec!["a".into(), "b".into()] }];
        let boxes = compute_group_boxes(&cards, &groups);
        let wire = boxes_to_wire(&boxes);
        assert!(wire.contains("g1\u{1F}Data Memory\u{1F}"));
        assert!(wire.ends_with('\n'));
        // 7 fields per line
        let line = wire.lines().next().unwrap();
        assert_eq!(line.split('\u{1F}').count(), 7);
    }

    #[test]
    fn empty_inputs_empty_output() {
        assert!(compute_group_boxes(&[], &[]).is_empty());
        assert_eq!(boxes_to_wire(&[]), "");
    }
}
