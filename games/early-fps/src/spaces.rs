//! spaces.rs — **functional space layout**. A space isn't a random pile of crates; it has a *function*, and
//! the function implies a coherent structure (with variation). A stronghold gets defensive cover wrapped
//! around a raised hold; a crossing gets a barricade with one flanking gap; a plaza gets a central island
//! ringed by approach cover; a nest is a lone perch with a sightline notch; a gauntlet staggers cover down its
//! long axis for cover-to-cover movement. Each stamps a coherent arrangement into a footprint so the space
//! reads as a *designed place* rather than scatter — and crucially every prop is one object: the mesh you see
//! is the exact box you collide with and now stand on.
//!
//! The choice of function comes from the space's role in the map (a graph node's role), which is itself a
//! proxy for the weights that matter — transit vs. destination, coverage need, high-ground value. Destinations
//! (strongholds, nests) get defensive, holdable cover; transit spaces (gauntlets, crossings) get
//! sightline-breaking cover you weave through.

use crate::level::{Obstacle, Palette};
use crate::math::Vec3;
use crate::mesh::{boxx, dither, rgb, Mesh3, FACE_ALL};
use crate::mapgraph::NodeRole;
use egui::Color32;

/// Darken/scale an RGB colour (the `scale_rgb` in level.rs is private).
fn scale_rgb(c: Color32, f: f32) -> Color32 {
    Color32::from_rgb((c.r() as f32 * f) as u8, (c.g() as f32 * f) as u8, (c.b() as f32 * f) as u8)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpaceFunction {
    /// Open spawn apron — kept mostly clear so you aren't fragged stepping out.
    Muster,
    /// Defensive hold: cover wrapped around a raised crate you can stand and fight on.
    Stronghold,
    /// Chokepoint: a barricade across the middle with a single flanking gap.
    Crossing,
    /// Crossroads: a central island ringed by approach cover.
    Plaza,
    /// Sniper perch: a lone raised stack with a low sightline notch in front.
    Nest,
    /// Transit lane: cover staggered down the long axis for cover-to-cover movement.
    Gauntlet,
}

impl SpaceFunction {
    /// Map a topology role to the function its space should perform. This is where "this area has a function,
    /// so it most likely has this structure" lives.
    pub fn from_role(role: NodeRole) -> Self {
        match role {
            NodeRole::Spawn => SpaceFunction::Muster,
            NodeRole::Tower => SpaceFunction::Nest,
            NodeRole::Trench => SpaceFunction::Gauntlet,
            NodeRole::Junction => SpaceFunction::Plaza,
            NodeRole::Cover => SpaceFunction::Crossing,
            NodeRole::Room => SpaceFunction::Stronghold,
        }
    }
}

/// One prop = one box, so the collision obstacle and the rendered mesh are the same dimensions at the same
/// place. After the support fix you can stand on its top, and 3-D collision blocks it only across its own
/// height, so low ones you shoot over and tall ones break the line.
fn block(center: Vec3, half: Vec3, col: Color32, key: i32) -> (Obstacle, Mesh3) {
    (Obstacle { center, half }, boxx(center, half, dither(col, key, 0.12), FACE_ALL))
}

const LOW: f32 = 0.55; // waist-high: shoot over
const TALL: f32 = 0.95; // chest/shoulder: full cover

/// Stamp the coherent structure for `function` into a footprint centred at (cx, cz) on the floor at `base_y`,
/// with world half-extents (hw, hh). Returns aligned (obstacle, mesh) pairs. Deterministic in `seed`, with a
/// couple of structural variants per function so repeated spaces don't read identically.
pub fn stamp(function: SpaceFunction, cx: f32, cz: f32, hw: f32, hh: f32, base_y: f32, pal: Palette, seed: u32) -> (Vec<Obstacle>, Vec<Mesh3>) {
    let mut obs = Vec::new();
    let mut mesh = Vec::new();
    let cover = pal.cover;
    let hard = scale_rgb(pal.trim, 0.9);
    let r = seed.wrapping_mul(2_654_435_761);
    let variant = (r >> 13) & 3;
    let push = |o: (Obstacle, Mesh3), obs: &mut Vec<Obstacle>, mesh: &mut Vec<Mesh3>| {
        obs.push(o.0);
        mesh.push(o.1);
    };
    // keep props inside the footprint with a margin so they never poke into the surrounding walls
    let ix = (hw - 1.2).max(0.6);
    let iz = (hh - 1.2).max(0.6);

    match function {
        SpaceFunction::Muster => {
            // mostly clear; one low crate to the side for an opening peek
            let sx = if variant & 1 == 0 { -ix * 0.6 } else { ix * 0.6 };
            push(block(Vec3::new(cx + sx, base_y + LOW, cz), Vec3::new(0.7, LOW, 0.7), cover, 1), &mut obs, &mut mesh);
        }
        SpaceFunction::Stronghold => {
            // a raised central hold (two stacked crates → a standable perch) wrapped by a U of cover
            push(block(Vec3::new(cx, base_y + 0.5, cz), Vec3::new(1.0, 0.5, 1.0), hard, 2), &mut obs, &mut mesh);
            push(block(Vec3::new(cx, base_y + 1.25, cz), Vec3::new(0.7, 0.25, 0.7), hard, 3), &mut obs, &mut mesh);
            // U opening toward a different side per variant
            let open = variant; // 0:N 1:S 2:E 3:W
            let walls = [
                (0.0f32, -iz, ix, 0.35), // north
                (0.0, iz, ix, 0.35),     // south
                (ix, 0.0, 0.35, iz),     // east
                (-ix, 0.0, 0.35, iz),    // west
            ];
            for (i, &(ox, oz, ex, ez)) in walls.iter().enumerate() {
                if i as u32 == open {
                    continue;
                }
                push(block(Vec3::new(cx + ox, base_y + TALL, cz + oz), Vec3::new(ex, TALL, ez), cover, 10 + i as i32), &mut obs, &mut mesh);
            }
        }
        SpaceFunction::Crossing => {
            // a tall barricade across the middle with one gap, plus a low block on each approach
            let along_x = variant & 1 == 0;
            let gap = if variant < 2 { -1.0 } else { 1.0 }; // which side the gap sits
            if along_x {
                push(block(Vec3::new(cx - ix * 0.5 + gap * 0.3, base_y + TALL, cz), Vec3::new(ix * 0.4, TALL, 0.4), hard, 20), &mut obs, &mut mesh);
                push(block(Vec3::new(cx + ix * 0.5 + gap * 0.3, base_y + TALL, cz), Vec3::new(ix * 0.3, TALL, 0.4), hard, 21), &mut obs, &mut mesh);
                push(block(Vec3::new(cx, base_y + LOW, cz - iz * 0.6), Vec3::new(0.8, LOW, 0.6), cover, 22), &mut obs, &mut mesh);
                push(block(Vec3::new(cx, base_y + LOW, cz + iz * 0.6), Vec3::new(0.8, LOW, 0.6), cover, 23), &mut obs, &mut mesh);
            } else {
                push(block(Vec3::new(cx, base_y + TALL, cz - iz * 0.5 + gap * 0.3), Vec3::new(0.4, TALL, iz * 0.4), hard, 20), &mut obs, &mut mesh);
                push(block(Vec3::new(cx, base_y + TALL, cz + iz * 0.5 + gap * 0.3), Vec3::new(0.4, TALL, iz * 0.3), hard, 21), &mut obs, &mut mesh);
                push(block(Vec3::new(cx - ix * 0.6, base_y + LOW, cz), Vec3::new(0.6, LOW, 0.8), cover, 22), &mut obs, &mut mesh);
                push(block(Vec3::new(cx + ix * 0.6, base_y + LOW, cz), Vec3::new(0.6, LOW, 0.8), cover, 23), &mut obs, &mut mesh);
            }
        }
        SpaceFunction::Plaza => {
            // central island (a planter / monument block) ringed by four low approach blocks
            let tall_island = variant & 1 == 0;
            let ih = if tall_island { 1.2 } else { 0.7 };
            push(block(Vec3::new(cx, base_y + ih, cz), Vec3::new(0.9, ih, 0.9), hard, 30), &mut obs, &mut mesh);
            for (i, &(ox, oz)) in [(-ix * 0.7, -iz * 0.7), (ix * 0.7, -iz * 0.7), (-ix * 0.7, iz * 0.7), (ix * 0.7, iz * 0.7)].iter().enumerate() {
                push(block(Vec3::new(cx + ox, base_y + LOW, cz + oz), Vec3::new(0.6, LOW, 0.6), cover, 31 + i as i32), &mut obs, &mut mesh);
            }
        }
        SpaceFunction::Nest => {
            // a lone raised perch (standable stack) with a low cover notch in front to shoot over
            push(block(Vec3::new(cx, base_y + 0.55, cz), Vec3::new(0.95, 0.55, 0.95), hard, 40), &mut obs, &mut mesh);
            push(block(Vec3::new(cx, base_y + 1.35, cz), Vec3::new(0.7, 0.25, 0.7), hard, 41), &mut obs, &mut mesh);
            let fz = if variant & 1 == 0 { -iz * 0.7 } else { iz * 0.7 };
            push(block(Vec3::new(cx, base_y + LOW, cz + fz), Vec3::new(0.9, LOW, 0.5), cover, 42), &mut obs, &mut mesh);
        }
        SpaceFunction::Gauntlet => {
            // staggered cover down the long axis, alternating sides and heights — a cover-to-cover weave
            let along_x = hw >= hh;
            let n = 4;
            for k in 0..n {
                let t = (k as f32 + 0.5) / n as f32 * 2.0 - 1.0; // -1..1
                let side = if k % 2 == 0 { -0.55 } else { 0.55 };
                let h = if k % 2 == 0 { LOW } else { TALL };
                let (px, pz) = if along_x { (cx + t * ix, cz + side * iz) } else { (cx + side * ix, cz + t * iz) };
                push(block(Vec3::new(px, base_y + h, pz), Vec3::new(0.7, h, 0.7), cover, 50 + k), &mut obs, &mut mesh);
            }
        }
    }
    let _ = rgb; // keep import available for future palette work
    // ── Art-director pass: give each function its own material identity ──────────────────────────────
    // A stronghold's cover should *read* fortified, a nest's should read grown-over, a plaza's should read
    // dressed — the same crate silhouette in a different material sells the space's role at a glance, the
    // same way a level designer would reach for a different prop kit per room type. These are discrete
    // props (crates, barricades, a perch), not enclosing walls, so texturing them doesn't crowd the space.
    let accent = match function {
        SpaceFunction::Muster => None,                         // open assembly point — stays plain
        SpaceFunction::Stronghold => Some(crate::mesh::Material::Concrete), // fortified hold
        SpaceFunction::Crossing => Some(crate::mesh::Material::RustPlate), // industrial choke barricade
        SpaceFunction::Plaza => Some(crate::mesh::Material::Tile),        // dressed, ornamental island
        SpaceFunction::Nest => Some(crate::mesh::Material::Organic),      // grown-over lookout
        SpaceFunction::Gauntlet => Some(crate::mesh::Material::Gravel),   // rough staggered cover
    };
    if let Some(mat) = accent {
        for me in &mut mesh {
            me.material = Some(mat);
        }
    }
    (obs, mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::MapTheme;

    fn pal() -> Palette {
        MapTheme::Industrial.palette()
    }

    #[test]
    fn every_function_stamps_aligned_mesh_and_collision() {
        for f in [
            SpaceFunction::Muster,
            SpaceFunction::Stronghold,
            SpaceFunction::Crossing,
            SpaceFunction::Plaza,
            SpaceFunction::Nest,
            SpaceFunction::Gauntlet,
        ] {
            let (obs, mesh) = stamp(f, 30.0, 30.0, 4.0, 4.0, 0.0, pal(), 7);
            assert_eq!(obs.len(), mesh.len(), "{:?}: one mesh per collision box", f);
            assert!(!obs.is_empty(), "{:?}: should place structure", f);
            // every prop sits inside the footprint and on/above the floor
            for o in &obs {
                assert!((o.center.x - 30.0).abs() <= 4.0 && (o.center.z - 30.0).abs() <= 4.0, "{:?}: prop escaped footprint", f);
                assert!(o.center.y - o.half.y >= -0.01, "{:?}: prop sunk below floor", f);
            }
        }
    }

    #[test]
    fn destinations_hold_more_cover_than_a_spawn_apron() {
        let (muster, _) = stamp(SpaceFunction::Muster, 0.0, 0.0, 4.0, 4.0, 0.0, pal(), 1);
        let (hold, _) = stamp(SpaceFunction::Stronghold, 0.0, 0.0, 4.0, 4.0, 0.0, pal(), 1);
        assert!(hold.len() > muster.len(), "a stronghold should be more fortified than a spawn");
    }

    #[test]
    fn a_nest_has_a_standable_perch() {
        let (obs, _) = stamp(SpaceFunction::Nest, 0.0, 0.0, 4.0, 4.0, 0.0, pal(), 3);
        // the stacked perch reaches above a single crate's height → you can climb/stand on it
        let top = obs.iter().map(|o| o.center.y + o.half.y).fold(0.0f32, f32::max);
        assert!(top > 1.4, "nest perch should be a real raised stand, top {:.2}", top);
    }
}
