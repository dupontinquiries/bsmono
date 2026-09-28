//! scene — the worlds a file can live in. Two builders over one vocabulary:
//! [`build`] is the classic single room (a lectern, plinths, a floor — used
//! when a file has no outline to walk), and [`build_rooms`] is the corridor:
//! every outline item a STATION with a plinth in its kind's material and a
//! name plaque from the static atlas (`MAT_INSCRIPTION2`), `mod` items as
//! gates spanning the corridor, and ONE lectern that moves to the focused
//! station — jumping rooms moves the reading stand, not the reader's habits.
//!
//! Geometry rebuilds on jump (station counts are small); per frame only the
//! page quad's UV window moves (`set_page_window`).

use egui::Color32;
use scene_kit::gpuscene::{MAT_INSCRIPTION, MAT_INSCRIPTION2};
use scene_kit::mesh3d::graph_scene;
use scene_kit::mesh3d::{cuboid, panel, panel_uv, with_mat, Mesh3};
use scene_kit::spatial::Vec3;

use crate::graph;
use aperture_core::gitspace::Rev;
use crate::rooms::{room_mat, Station, CORRIDOR_HALF_W, STATION_SPACING};

pub struct World {
    pub meshes: Vec<Mesh3>,
    page_ix: usize,
    pub page_center: Vec3,
    pub page_right: Vec3,
    pub page_up: Vec3,
    pub page_w: f32,
    pub page_h: f32,
    /// Screen-clickable plaques: (station index, mesh index).
    pub plaque_ix: Vec<(usize, usize)>,
    /// Screen-clickable commit-graph nodes: (`revs` index, world center).
    /// Spheres don't have quad corners the way plaques do, so this carries
    /// a raw point for a DISTANCE-based hit test (project it, measure
    /// screen-pixel distance to the click) rather than a contains-test —
    /// its own convention, not a forced reuse of the plaque one.
    pub graph_node_ix: Vec<(usize, Vec3)>,
}

const V: fn(f32, f32, f32) -> Vec3 = Vec3::new;

/// The lectern group at `anchor` (a floor point). Returns the page quad's
/// mesh index and transform.
fn push_lectern(
    meshes: &mut Vec<Mesh3>,
    anchor: Vec3,
    aspect: f32,
) -> (usize, Vec3, Vec3, Vec3, f32, f32) {
    let lean = 0.21f32;
    let up = V(0.0, lean.cos(), -lean.sin());
    let right = V(1.0, 0.0, 0.0);
    let normal = V(0.0, lean.sin(), lean.cos());
    let page_h = 13.0;
    let page_w = (page_h * aspect).clamp(10.0, 34.0);
    let center = V(anchor.x, anchor.y + 8.6, anchor.z);

    // Wooden board with a frame's worth of border behind the sheet.
    meshes.push(with_mat(
        panel(
            V(center.x - normal.x * 0.22, center.y - normal.y * 0.22, center.z - normal.z * 0.22),
            right,
            up,
            page_w + 1.7,
            page_h + 1.7,
            Color32::from_rgb(112, 76, 42),
        ),
        1.0,
    ));
    for sx in [-1.0f32, 1.0] {
        meshes.push(with_mat(
            cuboid(
                V(anchor.x + sx * (page_w * 0.32), anchor.y + 3.4, anchor.z - 1.4),
                V(0.55, 3.4, 0.55),
                Color32::from_rgb(96, 64, 36),
            ),
            1.0,
        ));
    }
    meshes.push(with_mat(
        cuboid(
            V(anchor.x, anchor.y + 0.35, anchor.z - 1.4),
            V(page_w * 0.38, 0.35, 0.9),
            Color32::from_rgb(96, 64, 36),
        ),
        1.0,
    ));

    // The PAGE: parchment sheet, ink from `u_page`, overlay-biased so props
    // can never occlude the reading face.
    let mut page = with_mat(
        panel_uv(center, right, up, page_w, page_h, Color32::from_rgb(233, 221, 197)),
        MAT_INSCRIPTION,
    )
    .as_overlay(0.35);
    page.emissive = 0.05;
    let ix = meshes.len();
    meshes.push(page);
    (ix, center, right, up, page_w, page_h)
}

/// Append the commit-history graph (logscope's `graph_scene`, a node per
/// revision, an edge per parent) as a small cluster at the vestibule — the
/// first thing visible on entering, before walking a single station. Empty
/// or single-node history renders nothing (a lone dot with no edges isn't
/// informative, and an empty graph is just noise).
fn push_history_graph(meshes: &mut Vec<Mesh3>, revs: &[Rev], rev_ix: Option<usize>) -> Vec<(usize, Vec3)> {
    if revs.len() < 2 {
        return Vec::new();
    }
    let g = graph::layout(revs, rev_ix, V(-9.5, 9.0, 8.5));
    let node_ix: Vec<(usize, Vec3)> = g.centers.iter().enumerate().map(|(i, c)| (i, *c)).collect();
    meshes.extend(graph_scene(
        &g.centers,
        &g.colors,
        &g.edges,
        g.active,
        0.42,
        0.10,
        Color32::from_rgb(226, 196, 108),
        Color32::from_rgb(120, 118, 112),
    ));
    node_ix
}

/// The classic single room (no outline): lectern at the origin, the four
/// context-material plinths behind.
pub fn build(aspect: f32) -> World {
    build_with_history(aspect, &[], None)
}

/// [`build`] plus the history graph — kept as a separate entry point so
/// `build`'s existing signature (used by tests and the empty-outline
/// fallback) never has to know about git at all.
pub fn build_with_history(aspect: f32, revs: &[Rev], rev_ix: Option<usize>) -> World {
    let mut meshes = Vec::new();
    meshes.push(with_mat(
        cuboid(V(0.0, -0.6, 0.0), V(46.0, 0.6, 40.0), Color32::from_rgb(126, 124, 120)),
        2.0,
    ));
    let graph_node_ix = push_history_graph(&mut meshes, revs, rev_ix);
    let (page_ix, page_center, page_right, page_up, page_w, page_h) =
        push_lectern(&mut meshes, V(0.0, 0.0, 0.0), aspect);

    let mats: [(f32, Color32); 4] = [
        (0.0, Color32::from_rgb(228, 219, 200)),
        (1.0, Color32::from_rgb(122, 84, 48)),
        (2.0, Color32::from_rgb(120, 120, 122)),
        (4.0, Color32::from_rgb(92, 128, 62)),
    ];
    for (i, (mat, col)) in mats.into_iter().enumerate() {
        let x = -13.5 + i as f32 * 9.0;
        meshes.push(with_mat(cuboid(V(x, 2.1, -13.0), V(1.55, 2.1, 1.55), col), mat));
        let mut cap = with_mat(cuboid(V(x, 4.5, -13.0), V(0.5, 0.28, 0.5), col), mat);
        cap.emissive = 0.5;
        meshes.push(cap);
    }

    World { meshes, page_ix, page_center, page_right, page_up, page_w, page_h, plaque_ix: Vec::new(), graph_node_ix }
}

/// Per-station plaque V window in the label atlas: the atlas is `inscribe`'s
/// stack of single-line rows with a one-cell margin top and bottom.
pub fn atlas_v_window(row: usize, cell_px: f32, tex_h: f32) -> (f32, f32) {
    let v0 = (cell_px + row as f32 * cell_px) / tex_h;
    let v1 = (cell_px + (row + 1) as f32 * cell_px) / tex_h;
    (v0, v1)
}

/// The corridor: one station per outline item, the lectern at `focus`.
pub fn build_rooms(
    stations: &[Station],
    focus: usize,
    aspect: f32,
    atlas_cell_px: f32,
    atlas_h: f32,
    revs: &[Rev],
    rev_ix: Option<usize>,
) -> World {
    build_rooms_with_trail(stations, focus, aspect, atlas_cell_px, atlas_h, revs, rev_ix, &[])
}

/// Floor tiles along the way back: one every this many world units.
pub const TRAIL_STEP: f32 = 2.2;

/// [`build_rooms`], plus the jump trail lit on the floor: `trail` is
/// station indices in the order they were visited (the viewer's `NavPath`,
/// resolved to this corridor). Consecutive stations are joined by a line of
/// glowing tiles, and a visited station's plaque burns a little brighter —
/// "where have I been" readable from the far end of the corridor.
#[allow(clippy::too_many_arguments)]
pub fn build_rooms_with_trail(
    stations: &[Station],
    focus: usize,
    aspect: f32,
    atlas_cell_px: f32,
    atlas_h: f32,
    revs: &[Rev],
    rev_ix: Option<usize>,
    trail: &[usize],
) -> World {
    if stations.is_empty() {
        return build_with_history(aspect, revs, rev_ix);
    }
    let mut meshes = Vec::new();
    let graph_node_ix = push_history_graph(&mut meshes, revs, rev_ix);
    let len = 20.0 + stations.len() as f32 * STATION_SPACING;

    // Corridor floor: one long slab down −z.
    meshes.push(with_mat(
        cuboid(
            V(0.0, -0.6, -len * 0.5 + 8.0),
            V(CORRIDOR_HALF_W + 6.0, 0.6, len * 0.5 + 8.0),
            Color32::from_rgb(122, 120, 118),
        ),
        2.0,
    ));

    let mut plaque_ix = Vec::new();
    for (i, st) in stations.iter().enumerate() {
        let mat = room_mat(st.item.kind);
        let is_gate = st.item.kind == aperture_core::syntax::OutlineKind::Mod;
        if is_gate {
            // A gate across the corridor: two pillars + a lintel.
            for sx in [-1.0f32, 1.0] {
                meshes.push(with_mat(
                    cuboid(V(sx * CORRIDOR_HALF_W, 5.0, st.pos.z), V(1.1, 5.0, 1.1), Color32::from_rgb(96, 122, 70)),
                    mat,
                ));
            }
            meshes.push(with_mat(
                cuboid(V(0.0, 10.4, st.pos.z), V(CORRIDOR_HALF_W + 1.1, 0.9, 1.3), Color32::from_rgb(96, 122, 70)),
                mat,
            ));
        } else {
            // Heat warms the stone toward ember and lends it a glow: you can
            // see this week's work from the far end of the corridor. A
            // station TOUCHED by the diff currently on screen overrides that
            // with git-green — a different question ("did THIS change touch
            // this room") answered with a different color, not a brighter
            // shade of the same one, so the two never get confused at a
            // glance.
            let h = st.heat;
            let col = if st.touched {
                Color32::from_rgb(96, 168, 84)
            } else if st.dormant {
                // Sealed, not just quiet: dimmer and greyer than the ember
                // scale ever goes, so "hasn't moved in months" reads as
                // genuinely different from "cooled off this week" rather
                // than the far end of the same gradient.
                Color32::from_rgb(110, 108, 106)
            } else {
                Color32::from_rgb(
                    (150.0 + 60.0 * h) as u8,
                    (140.0 - 20.0 * h) as u8,
                    (120.0 - 50.0 * h) as u8,
                )
            };
            // large: the plinth itself grows — a bigger item gets a
            // physically bigger room, logscope's Chamber ("token-heavy →
            // larger") with real line-count standing in for token mass.
            let half = if st.large { V(2.3, 2.3, 2.3) } else { V(1.7, 2.0, 1.7) };
            let mut plinth = with_mat(cuboid(V(st.pos.x, half.y, st.pos.z), half, col), mat);
            plinth.emissive = if st.touched { 0.55 } else if st.dormant { 0.0 } else { 0.4 * h };
            meshes.push(plinth);
            if st.touched {
                // A thin marker cap, unmistakable even from far down the
                // corridor before the color read is legible.
                let mut cap = with_mat(
                    cuboid(V(st.pos.x, half.y * 2.2, st.pos.z), V(0.5, 0.18, 0.5), Color32::from_rgb(120, 200, 100)),
                    mat,
                );
                cap.emissive = 0.7;
                meshes.push(cap);
            } else if st.dormant {
                // A capstone, not a cap: flat, wide, unlit — sealing the
                // room shut rather than crowning it. The silhouette itself
                // says "closed off" before any color is legible at all.
                meshes.push(with_mat(
                    cuboid(V(st.pos.x, half.y * 2.05, st.pos.z), V(half.x * 1.15, 0.14, half.x * 1.15), Color32::from_rgb(96, 94, 92)),
                    mat,
                ));
            }
        }
        // Name plaque: atlas row i, standing above the plinth (or hung from
        // the gate's lintel), face aligned down the corridor axis so it reads
        // on approach.
        let (v0, v1) = atlas_v_window(st.atlas_row, atlas_cell_px, atlas_h);
        let (pc, pr) = if is_gate {
            (V(0.0, 8.6, st.pos.z + 1.45), V(1.0, 0.0, 0.0))
        } else {
            (V(st.pos.x, 5.4, st.pos.z + 2.0), V(1.0, 0.0, 0.0))
        };
        let mut plq = with_mat(
            panel_uv(pc, pr, V(0.0, 1.0, 0.0), 9.5, 1.5, Color32::from_rgb(226, 214, 188)),
            MAT_INSCRIPTION2,
        );
        plq.uvs = Some(vec![[0.0, v1], [1.0, v1], [1.0, v0], [0.0, v0]]);
        plq.emissive = if i == focus { 0.35 } else if trail.contains(&i) { 0.22 } else { 0.10 };
        plaque_ix.push((i, meshes.len()));
        meshes.push(plq);
    }

    push_trail(&mut meshes, stations, trail);

    // The one lectern, at the focused station, nudged toward the center line.
    let f = &stations[focus.min(stations.len() - 1)];
    let anchor = V(f.pos.x * 0.35, 0.0, f.pos.z + 4.5);
    let (page_ix, page_center, page_right, page_up, page_w, page_h) =
        push_lectern(&mut meshes, anchor, aspect);

    World { meshes, page_ix, page_center, page_right, page_up, page_w, page_h, plaque_ix, graph_node_ix }
}

/// Glowing floor tiles between consecutive trail stations, on the corridor
/// floor's top face (y = 0), in front of each plinth. Tiles, not a strip:
/// every mesh here is an axis-aligned cuboid, and a diagonal between two
/// stations is a line of small squares rather than a skewed slab.
fn push_trail(meshes: &mut Vec<Mesh3>, stations: &[Station], trail: &[usize]) {
    let at = |i: usize| stations.get(i).map(|s| V(s.pos.x * 0.6, 0.05, s.pos.z + 3.0));
    for w in trail.windows(2) {
        let (Some(a), Some(b)) = (at(w[0]), at(w[1])) else { continue };
        let d = V(b.x - a.x, 0.0, b.z - a.z);
        let len = (d.x * d.x + d.z * d.z).sqrt();
        let n = (len / TRAIL_STEP).ceil().max(1.0) as usize;
        for k in 0..=n {
            let t = k as f32 / n as f32;
            let mut tile = with_mat(
                cuboid(V(a.x + d.x * t, 0.05, a.z + d.z * t), V(0.35, 0.05, 0.35), Color32::from_rgb(236, 196, 110)),
                2.0,
            );
            tile.emissive = 0.6;
            meshes.push(tile);
        }
    }
}

/// Slide the page quad's V window: `v0..v1` in texture space (0 = top row of
/// the raster). U spans the full width. `panel_uv` vert order is BL, BR, TR, TL.
pub fn set_page_window(world: &mut World, v0: f32, v1: f32) {
    world.meshes[world.page_ix].uvs = Some(vec![[0.0, v1], [1.0, v1], [1.0, v0], [0.0, v0]]);
}

/// The page's four corners in world space (BL, BR, TR, TL) — for projecting a
/// screen rect to click on.
pub fn page_corners(w: &World) -> [Vec3; 4] {
    let r = V(w.page_right.x * w.page_w * 0.5, w.page_right.y * w.page_w * 0.5, w.page_right.z * w.page_w * 0.5);
    let u = V(w.page_up.x * w.page_h * 0.5, w.page_up.y * w.page_h * 0.5, w.page_up.z * w.page_h * 0.5);
    let c = w.page_center;
    let add = |a: Vec3, b: Vec3| V(a.x + b.x, a.y + b.y, a.z + b.z);
    let sub = |a: Vec3, b: Vec3| V(a.x - b.x, a.y - b.y, a.z - b.z);
    [sub(sub(c, r), u), sub(add(c, r), u), add(add(c, r), u), add(sub(c, r), u)]
}

/// A plaque quad's four world corners (BL, BR, TR, TL), for click hit-testing.
pub fn plaque_corners(w: &World, mesh_ix: usize) -> [Vec3; 4] {
    let m = &w.meshes[mesh_ix];
    [m.verts[0], m.verts[1], m.verts[2], m.verts[3]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::{layout, room_env};
    use aperture_core::syntax::{OutlineItem, OutlineKind};
    use scene_kit::gpuscene::{pack_sorted, Camera};
    use scene_kit::spatial::OrbitCam;

    fn items() -> Vec<OutlineItem> {
        vec![
            OutlineItem { kind: OutlineKind::Fn, name: "alpha".into(), sig: "fn alpha()".into(), line: 0, end_line: 2 },
            OutlineItem { kind: OutlineKind::Mod, name: "gate".into(), sig: "mod gate".into(), line: 3, end_line: 3 },
            OutlineItem { kind: OutlineKind::Struct, name: "Beta".into(), sig: "struct Beta".into(), line: 4, end_line: 8 },
        ]
    }

    #[test]
    fn packs_14_floats_and_page_uvs_ride_along() {
        let mut w = build(2.2);
        set_page_window(&mut w, 0.1, 0.9);
        let cam = Camera::from_orbit(
            &OrbitCam { yaw: 0.1, pitch: -0.3, dist: 30.0, target: w.page_center, roll: 0.0 },
            1280.0,
            800.0,
            640.0,
            0.5,
            200.0,
        );
        let verts = pack_sorted(&w.meshes, &cam, 0.0);
        assert_eq!(verts.len() % 14, 0, "14-float vertices");
        let uvs: Vec<[f32; 2]> = verts.chunks(14).map(|v| [v[12], v[13]]).collect();
        assert!(uvs.iter().any(|u| (u[1] - 0.9).abs() < 1e-4));
        assert!(uvs.iter().any(|u| u == &[0.0, 0.0]));
    }

    #[test]
    fn page_window_slides_without_rebuilding() {
        let mut w = build(2.2);
        let n = w.meshes.len();
        set_page_window(&mut w, 0.0, 0.8);
        set_page_window(&mut w, 0.2, 1.0);
        assert_eq!(w.meshes.len(), n);
        let uv = w.meshes[w.page_ix].uvs.as_ref().unwrap();
        assert_eq!(uv[2], [1.0, 0.2], "TR carries the new v0");
    }

    #[test]
    fn corridor_has_a_room_per_item_and_a_moving_lectern() {
        let st = layout(items());
        let cell = 16.0;
        let atlas_h = cell * (st.len() as f32 + 2.0);
        let w0 = build_rooms(&st, 0, 2.2, cell, atlas_h, &[], None);
        let w2 = build_rooms(&st, 2, 2.2, cell, atlas_h, &[], None);
        assert_eq!(w0.plaque_ix.len(), 3, "every station has a clickable plaque");
        assert!(w2.page_center.z < w0.page_center.z, "the lectern follows the focus deeper in");
        let uv_of = |w: &World, i: usize| w.meshes[w.plaque_ix[i].1].uvs.clone().unwrap();
        let (a, b) = (uv_of(&w0, 0), uv_of(&w0, 1));
        assert!(a[2][1] < b[2][1], "atlas rows are disjoint and ordered: {a:?} {b:?}");
        assert_eq!(st[0].env, room_env(OutlineKind::Fn, "alpha"), "stations carry deterministic rooms");
        // The focused plaque burns brighter.
        assert!(w0.meshes[w0.plaque_ix[0].1].emissive > w0.meshes[w0.plaque_ix[1].1].emissive);
    }

    #[test]
    fn empty_outline_falls_back_to_the_single_room() {
        let w = build_rooms(&[], 0, 2.2, 16.0, 64.0, &[], None);
        assert!(w.plaque_ix.is_empty());
        assert!(w.page_center.z.abs() < 9.0, "classic room at the origin");
    }
    #[test]
    fn history_graph_appears_with_real_multi_commit_history_and_is_absent_otherwise() {
        let rev = |oid: &str, parents: &[&str]| Rev {
            oid: oid.into(), time: 0, subject: oid.into(),
            kind: aperture_core::gitspace::RevKind::Commit,
            parents: parents.iter().map(|s| s.to_string()).collect(),
        };
        let n_before = build(2.2).meshes.len();

        // Two commits: a graph should appear (at least 2 spheres + 1 tube = 3 meshes over baseline).
        let two = vec![rev("b", &["a"]), rev("a", &[])];
        let w = build_with_history(2.2, &two, None);
        assert!(w.meshes.len() >= n_before + 3, "graph meshes present: {} vs baseline {}", w.meshes.len(), n_before);

        // Zero or one commit: no graph (a lone dot isn't informative).
        assert_eq!(build_with_history(2.2, &[], None).meshes.len(), n_before);
        let one = vec![rev("a", &[])];
        assert_eq!(build_with_history(2.2, &one, None).meshes.len(), n_before);
    }

    #[test]
    fn large_stations_get_a_physically_bigger_plinth_and_dormant_ones_a_capstone() {
        let mk = |name: &str, end_line: usize, heat: f32, dormant: bool, large: bool| Station {
            item: aperture_core::syntax::OutlineItem { kind: aperture_core::syntax::OutlineKind::Fn, name: name.into(), sig: name.into(), line: 0, end_line },
            pos: Vec3::new(-8.0, 0.0, -10.0),
            env: crate::rooms::room_env(aperture_core::syntax::OutlineKind::Fn, name),
            atlas_row: 0,
            heat,
            touched: false,
            dormant,
            large,
        };
        let small = mk("small", 3, 0.5, false, false);
        let big = mk("big", 40, 0.5, false, true);
        let sealed = mk("sealed", 3, 0.0, true, false);

        let w_small = build_rooms(std::slice::from_ref(&small), 0, 2.2, 16.0, 64.0, &[], None);
        let w_big = build_rooms(std::slice::from_ref(&big), 0, 2.2, 16.0, 64.0, &[], None);
        let w_sealed = build_rooms(std::slice::from_ref(&sealed), 0, 2.2, 16.0, 64.0, &[], None);

        // Large: find the plinth mesh (skip the shared corridor floor at index 0) and check its half-extent.
        let plinth_small = &w_small.meshes[1];
        let plinth_big = &w_big.meshes[1];
        assert!(plinth_big.verts.iter().map(|v| v.x.abs()).fold(0.0f32, f32::max)
            > plinth_small.verts.iter().map(|v| v.x.abs()).fold(0.0f32, f32::max),
            "the large station's plinth is physically wider");

        // Dormant: one extra mesh (the capstone) vs a plain, non-touched, non-large, non-dormant station.
        let ordinary = mk("ordinary", 3, 0.5, false, false);
        let w_ordinary = build_rooms(std::slice::from_ref(&ordinary), 0, 2.2, 16.0, 64.0, &[], None);
        assert_eq!(w_sealed.meshes.len(), w_ordinary.meshes.len() + 1, "capstone adds exactly one mesh");
        assert_eq!(w_sealed.meshes[1].emissive, 0.0, "sealed rooms don't glow");
    }

    #[test]
    fn graph_node_ix_has_one_entry_per_node_matching_graph_layout() {
        let rev = |oid: &str, parents: &[&str]| Rev {
            oid: oid.into(), time: 0, subject: oid.into(),
            kind: aperture_core::gitspace::RevKind::Commit,
            parents: parents.iter().map(|s| s.to_string()).collect(),
        };
        let revs = vec![rev("c", &["b"]), rev("b", &["a"]), rev("a", &[])];
        let w = build_with_history(2.2, &revs, Some(1));
        assert_eq!(w.graph_node_ix.len(), 3, "one clickable node per revision");
        // Indices match revs' own indices — jump_to_graph_node(ix) needs
        // this to line up directly with Viewer::revs.
        let ixs: Vec<usize> = w.graph_node_ix.iter().map(|(i, _)| *i).collect();
        assert_eq!(ixs, vec![0, 1, 2]);
        // Centers match graph::layout's own placement exactly (same math).
        let g = graph::layout(&revs, Some(1), V(-9.5, 9.0, 8.5));
        for ((_, wc), gc) in w.graph_node_ix.iter().zip(&g.centers) {
            assert_eq!(wc, gc);
        }
    }

    #[test]
    fn graph_node_ix_is_empty_below_two_commits() {
        assert!(build(2.2).graph_node_ix.is_empty());
        let rev = Rev { oid: "a".into(), time: 0, subject: "a".into(), kind: aperture_core::gitspace::RevKind::Commit, parents: vec![] };
        assert!(build_with_history(2.2, &[rev], None).graph_node_ix.is_empty());
    }


    #[test]
    fn the_jump_trail_is_lit_on_the_floor_and_brightens_visited_plaques() {
        let st = layout(items());
        let cell = 16.0;
        let atlas_h = cell * (st.len() as f32 + 2.0);
        let plain = build_rooms(&st, 0, 2.2, cell, atlas_h, &[], None);
        let lit = build_rooms_with_trail(&st, 0, 2.2, cell, atlas_h, &[], None, &[0, 2, 1]);
        let tiles = lit.meshes.len() - plain.meshes.len();
        assert!(tiles >= 4, "two legs of tiles, got {tiles}");
        let glow = |w: &World, i: usize| w.meshes[w.plaque_ix[i].1].emissive;
        assert!(glow(&lit, 2) > glow(&plain, 2), "a visited plaque burns brighter");
        assert_eq!(glow(&lit, 0), glow(&plain, 0), "the focused one is already brightest");
        // The plaque indices still point at plaques: the trail is added
        // after them, never between.
        assert_eq!(lit.plaque_ix, plain.plaque_ix);
        let one = build_rooms_with_trail(&st, 0, 2.2, cell, atlas_h, &[], None, &[1]);
        assert_eq!(one.meshes.len(), plain.meshes.len(), "one station is not a trail");
    }

}
