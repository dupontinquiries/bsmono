//! scene — the world the file lives in: a stone floor, a wooden lectern
//! carrying the inscribed page, and the material row behind it (paper, wood,
//! stone, moss plinths — the context-panel materials from RUN_ON_DEVICE,
//! standing where the reader can see what the world is made of).
//!
//! Geometry is built once; per frame only the page quad's UV window moves
//! (the smooth-scroll slide) — `set_page_window` rewrites four UV pairs.

use egui::Color32;
use scene_kit::gpuscene::MAT_INSCRIPTION;
use scene_kit::mesh3d::{cuboid, panel, panel_uv, with_mat, Mesh3};
use scene_kit::spatial::Vec3;

pub struct World {
    pub meshes: Vec<Mesh3>,
    page_ix: usize,
    pub page_center: Vec3,
    pub page_right: Vec3,
    pub page_up: Vec3,
    pub page_w: f32,
    pub page_h: f32,
}

const V: fn(f32, f32, f32) -> Vec3 = Vec3::new;

/// Build the world for a page of `aspect` = visible-window w/h.
pub fn build(aspect: f32) -> World {
    let mut meshes = Vec::new();

    // Stone floor slab, top face at y = 0.
    let floor = with_mat(
        cuboid(V(0.0, -0.6, 0.0), V(46.0, 0.6, 40.0), Color32::from_rgb(126, 124, 120)),
        2.0,
    );
    meshes.push(floor);

    // The lectern: page leaned back ~12°, its top at eye-ish height.
    let lean = 0.21f32;
    let up = V(0.0, lean.cos(), -lean.sin());
    let right = V(1.0, 0.0, 0.0);
    let normal = V(0.0, lean.sin(), lean.cos()); // faces the default camera (+z side)
    let page_h = 13.0;
    let page_w = (page_h * aspect).clamp(10.0, 34.0);
    let center = V(0.0, 8.6, 0.0);

    // Wooden board behind the page: a frame border all around.
    let board = with_mat(
        panel(
            V(center.x - normal.x * 0.22, center.y - normal.y * 0.22, center.z - normal.z * 0.22),
            right,
            up,
            page_w + 1.7,
            page_h + 1.7,
            Color32::from_rgb(112, 76, 42),
        ),
        1.0,
    );
    meshes.push(board);

    // Two wooden legs from the floor to the board's underside.
    for sx in [-1.0f32, 1.0] {
        let leg = with_mat(
            cuboid(V(sx * (page_w * 0.32), 3.4, -1.4), V(0.55, 3.4, 0.55), Color32::from_rgb(96, 64, 36)),
            1.0,
        );
        meshes.push(leg);
    }
    // A foot beam for stance.
    meshes.push(with_mat(
        cuboid(V(0.0, 0.35, -1.4), V(page_w * 0.38, 0.35, 0.9), Color32::from_rgb(96, 64, 36)),
        1.0,
    ));

    // The PAGE — the one inscription face in the scene. Parchment sheet tint;
    // ink comes from `u_page` by UV. Overlay bias so foliage/props can never
    // occlude the reading surface (mesh3d's physical-UI depth pull).
    let mut page = with_mat(
        panel_uv(center, right, up, page_w, page_h, Color32::from_rgb(233, 221, 197)),
        MAT_INSCRIPTION,
    )
    .as_overlay(0.35);
    page.emissive = 0.05; // the sheet itself barely glows; legibility in any light
    let page_ix = meshes.len();
    meshes.push(page);

    // The material row: four plinths behind the lectern — the physical-UI
    // context materials, standing for themselves.
    let mats: [(f32, Color32); 4] = [
        (0.0, Color32::from_rgb(228, 219, 200)), // paper
        (1.0, Color32::from_rgb(122, 84, 48)),   // wood
        (2.0, Color32::from_rgb(120, 120, 122)), // stone
        (4.0, Color32::from_rgb(92, 128, 62)),   // moss
    ];
    for (i, (mat, col)) in mats.into_iter().enumerate() {
        let x = -13.5 + i as f32 * 9.0;
        meshes.push(with_mat(cuboid(V(x, 2.1, -13.0), V(1.55, 2.1, 1.55), col), mat));
        // A faint ember on top of each, so the row reads at dusk.
        let mut cap = with_mat(cuboid(V(x, 4.5, -13.0), V(0.5, 0.28, 0.5), col), mat);
        cap.emissive = 0.5;
        meshes.push(cap);
    }

    World { meshes, page_ix, page_center: center, page_right: right, page_up: up, page_w, page_h }
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

#[cfg(test)]
mod tests {
    use super::*;
    use scene_kit::gpuscene::{pack_sorted, Camera};
    use scene_kit::spatial::OrbitCam;

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
        // Some vertex carries the page's V window; scenery carries (0,0).
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
}
