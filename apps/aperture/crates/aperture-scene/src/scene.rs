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
use scene_kit::mesh3d::{cuboid, panel, panel_uv, with_mat, Mesh3};
use scene_kit::spatial::Vec3;

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

/// The classic single room (no outline): lectern at the origin, the four
/// context-material plinths behind.
pub fn build(aspect: f32) -> World {
    let mut meshes = Vec::new();
    meshes.push(with_mat(
        cuboid(V(0.0, -0.6, 0.0), V(46.0, 0.6, 40.0), Color32::from_rgb(126, 124, 120)),
        2.0,
    ));
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

    World { meshes, page_ix, page_center, page_right, page_up, page_w, page_h, plaque_ix: Vec::new() }
}

/// Per-station plaque V window in the label atlas: the atlas is `inscribe`'s
/// stack of single-line rows with a one-cell margin top and bottom.
pub fn atlas_v_window(row: usize, cell_px: f32, tex_h: f32) -> (f32, f32) {
    let v0 = (cell_px + row as f32 * cell_px) / tex_h;
    let v1 = (cell_px + (row + 1) as f32 * cell_px) / tex_h;
    (v0, v1)
}

/// The corridor: one station per outline item, the lectern at `focus`.
pub fn build_rooms(stations: &[Station], focus: usize, aspect: f32, atlas_cell_px: f32, atlas_h: f32) -> World {
    if stations.is_empty() {
        return build(aspect);
    }
    let mut meshes = Vec::new();
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
            } else {
                Color32::from_rgb(
                    (150.0 + 60.0 * h) as u8,
                    (140.0 - 20.0 * h) as u8,
                    (120.0 - 50.0 * h) as u8,
                )
            };
            let mut plinth = with_mat(cuboid(V(st.pos.x, 2.0, st.pos.z), V(1.7, 2.0, 1.7), col), mat);
            plinth.emissive = if st.touched { 0.55 } else { 0.4 * h };
            meshes.push(plinth);
            if st.touched {
                // A thin marker cap, unmistakable even from far down the
                // corridor before the color read is legible.
                let mut cap = with_mat(
                    cuboid(V(st.pos.x, 4.4, st.pos.z), V(0.5, 0.18, 0.5), Color32::from_rgb(120, 200, 100)),
                    mat,
                );
                cap.emissive = 0.7;
                meshes.push(cap);
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
        plq.emissive = if i == focus { 0.35 } else { 0.10 };
        plaque_ix.push((i, meshes.len()));
        meshes.push(plq);
    }

    // The one lectern, at the focused station, nudged toward the center line.
    let f = &stations[focus.min(stations.len() - 1)];
    let anchor = V(f.pos.x * 0.35, 0.0, f.pos.z + 4.5);
    let (page_ix, page_center, page_right, page_up, page_w, page_h) =
        push_lectern(&mut meshes, anchor, aspect);

    World { meshes, page_ix, page_center, page_right, page_up, page_w, page_h, plaque_ix }
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
        let w0 = build_rooms(&st, 0, 2.2, cell, atlas_h);
        let w2 = build_rooms(&st, 2, 2.2, cell, atlas_h);
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
        let w = build_rooms(&[], 0, 2.2, 16.0, 64.0);
        assert!(w.plaque_ix.is_empty());
        assert!(w.page_center.z.abs() < 9.0, "classic room at the origin");
    }
}
