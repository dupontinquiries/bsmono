//! atlantisworld — what stands in (and over, and around) the Drowned Halls.
//!
//! `atlantis` is the ground; this is the dressing, for `scene` to draw and
//! `Blockers` to collide against. Placements are data ([`placements`],
//! tested without a GPU); [`build`] turns them into meshes and collision.
//!
//! The halls: a fluted colonnade down both walls of the lane hall, bronze
//! ribs across it every few paces with glass vaulting between them,
//! stained glass in the bays, ribbed glass domes over the two bases and the
//! Library, the Library's ring of bookcases round an orichalcum heart,
//! statues and sea lanterns in the galleries. Outside, on the sea floor:
//! kelp beds and coral.

use glam::Vec3;

use crate::atlantis::{self as at, BASE_R, LANE_R, LIBRARY_R, LIBRARY_X, WALL_H};
use crate::atlantisprops as ap;
use crate::blockers::{Blocker, Blockers, Cover, Shape};
use crate::consts::{self, Lane, Team};
use crate::scene::{Piece, V_MED};
use crate::sim::Rng;

/// How far the ribs and vaults rise above the wall top.
pub const VAULT_RISE: f32 = 3.2;
/// Ribs along the lane hall, this far apart.
pub const RIB_STEP: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    Column { at: Vec3, variant: i32 },
    /// A rib across a hall at `at` (its middle, at floor level), the hall
    /// running along `yaw` (0 = +z), `span` wide.
    Rib { at: Vec3, yaw: f32, span: f32 },
    /// Glass vaulting from one rib to the next, centred at `at`.
    Vault { at: Vec3, yaw: f32, span: f32, len: f32 },
    /// A glass dome over a rotunda of radius `r`.
    Dome { at: Vec3, r: f32 },
    /// Stained glass against a wall, facing `yaw`.
    Window { at: Vec3, yaw: f32, gold: bool, seed: f32 },
    /// A bookcase against the Library's wall, facing its centre.
    Books { at: Vec3, yaw: f32, seed: f32 },
    Statue { at: Vec3, yaw: f32 },
    Lantern { at: Vec3, variant: i32 },
    Heart { at: Vec3 },
    Kelp { at: Vec3, h: f32, variant: i32 },
    Coral { at: Vec3, size: f32, variant: i32 },
}

pub fn item_at(it: &Item) -> Vec3 {
    match *it {
        Item::Column { at, .. }
        | Item::Rib { at, .. }
        | Item::Vault { at, .. }
        | Item::Dome { at, .. }
        | Item::Window { at, .. }
        | Item::Books { at, .. }
        | Item::Statue { at, .. }
        | Item::Lantern { at, .. }
        | Item::Heart { at }
        | Item::Kelp { at, .. }
        | Item::Coral { at, .. } => at,
    }
}

fn flip(it: Item) -> Item {
    let f = |p: Vec3| Vec3::new(p.x, p.y, -p.z);
    let fy = |y: f32| std::f32::consts::PI - y;
    match it {
        Item::Column { at, variant } => Item::Column { at: f(at), variant },
        Item::Rib { at, yaw, span } => Item::Rib { at: f(at), yaw: fy(yaw), span },
        Item::Vault { at, yaw, span, len } => Item::Vault { at: f(at), yaw: fy(yaw), span, len },
        Item::Dome { at, r } => Item::Dome { at: f(at), r },
        Item::Window { at, yaw, gold, seed } => Item::Window { at: f(at), yaw: fy(yaw), gold, seed },
        Item::Books { at, yaw, seed } => Item::Books { at: f(at), yaw: fy(yaw), seed },
        Item::Statue { at, yaw } => Item::Statue { at: f(at), yaw: fy(yaw) },
        Item::Lantern { at, variant } => Item::Lantern { at: f(at), variant },
        Item::Heart { at } => Item::Heart { at: f(at) },
        Item::Kelp { at, h, variant } => Item::Kelp { at: f(at), h, variant },
        Item::Coral { at, size, variant } => Item::Coral { at: f(at), size, variant },
    }
}

/// Everything placed on the Drowned Halls. Computed AS that map whatever
/// is active.
pub fn placements() -> Vec<Item> {
    crate::map::with(crate::map::MapId::Atlantis, placements_here)
}

fn placements_here() -> Vec<Item> {
    let f = at::floor();
    let mut out = Vec::new();
    let mut both = |out: &mut Vec<Item>, it: Item| {
        let on_axis = item_at(&it).z.abs() < 0.5;
        out.push(it);
        if !on_axis {
            out.push(flip(it));
        }
    };

    // the colonnade (blue half; `colonnade` walks the blue half)
    for (k, c) in at::colonnade(9.0).into_iter().enumerate() {
        both(&mut out, Item::Column { at: c, variant: k as i32 % 4 });
    }

    // ribs across the lane hall, and glass between them
    let pts = at::lane_polyline(Lane::Mid);
    let total: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let half = total * 0.5;
    let dir_at = |s: f32| {
        let a = crate::pass::along(&pts, (s - 0.5).max(0.0));
        let b = crate::pass::along(&pts, s + 0.5);
        (b - a).normalize_or(Vec3::Z)
    };
    // start past the base rotunda's dome; step to the middle
    // (the hall's polyline starts at the base mouth, z = -80; the base's
    // rotunda reaches to -BASE_Z + BASE_R)
    let mut s = (-at::BASE_Z + BASE_R) - pts[0].z + 2.0;
    let mut ribs = Vec::new();
    while s <= half + 0.01 {
        let p = crate::pass::along(&pts, s);
        let d = dir_at(s);
        ribs.push((s, p, d.x.atan2(d.z)));
        s += RIB_STEP;
    }
    for (i, &(_, p, yaw)) in ribs.iter().enumerate() {
        both(&mut out, Item::Rib { at: p, yaw, span: LANE_R * 2.0 + 1.0 });
        if let Some(&(_, q, _)) = ribs.get(i + 1) {
            let mid = (p + q) * 0.5;
            let d = q - p;
            both(&mut out, Item::Vault { at: mid, yaw: d.x.atan2(d.z), span: LANE_R * 2.0 + 1.0, len: d.length() });
        }
    }
    // (the middle bay, across the axis: from the last rib to its mirror)
    if let Some(&(_, p, _)) = ribs.last() {
        let q = Vec3::new(p.x, 0.0, -p.z);
        let d = q - p;
        if d.length() > 1.0 {
            out.push(Item::Vault { at: (p + q) * 0.5, yaw: d.x.atan2(d.z), span: LANE_R * 2.0 + 1.0, len: d.length() });
        }
    }

    // stained glass in every other bay, both walls, facing in
    for (i, &(_, p, yaw)) in ribs.iter().enumerate() {
        if i % 2 == 1 {
            continue;
        }
        let d = Vec3::new(yaw.sin(), 0.0, yaw.cos());
        let side = Vec3::new(-d.z, 0.0, d.x);
        for sgn in [-1.0f32, 1.0] {
            let w = p + d * (RIB_STEP * 0.5) + side * sgn * (LANE_R - 0.25);
            // only against wall (not a passage mouth)
            if f.sdf(w.x + side.x * sgn * 2.0, w.z + side.z * sgn * 2.0) > 0.5 {
                let face = (-side * sgn).x.atan2((-side * sgn).z);
                both(&mut out, Item::Window { at: w + Vec3::Y * 2.2, yaw: face, gold: i % 4 == 0, seed: i as f32 + sgn });
            }
        }
    }

    // domes: the bases and the Library
    out.push(Item::Dome { at: Vec3::new(LIBRARY_X, WALL_H - 0.4, 0.0), r: LIBRARY_R + 1.0 });
    both(&mut out, Item::Dome { at: Vec3::new(0.0, WALL_H - 0.4, -at::BASE_Z), r: BASE_R + 1.0 });

    // the Library: a ring of bookcases at its wall (not across a passage),
    // and its orichalcum heart
    let n = 22;
    for k in 0..n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        let p = Vec3::new(LIBRARY_X + a.cos() * (LIBRARY_R - 0.6), 0.0, a.sin() * (LIBRARY_R - 0.6));
        if p.z > 0.2 {
            continue; // the blue half and the axis; mirrored
        }
        let outward = Vec3::new(a.cos(), 0.0, a.sin());
        if f.sdf(p.x + outward.x * 2.5, p.z + outward.z * 2.5) < 0.3 {
            continue; // a passage opens here
        }
        let face = (-outward).x.atan2(-outward.z);
        both(&mut out, Item::Books { at: p, yaw: face, seed: k as f32 });
    }
    out.push(Item::Heart { at: Vec3::new(LIBRARY_X - 6.0, 0.0, 0.0) });
    both(&mut out, Item::Lantern { at: Vec3::new(LIBRARY_X - 10.0, 0.0, -8.0), variant: 1 });

    // the galleries: a statue at the back, lanterns by the neck
    let (gx, gz, gr) = at::GALLERY;
    both(&mut out, Item::Statue { at: Vec3::new(gx - gr + 3.2, 0.0, gz), yaw: std::f32::consts::FRAC_PI_2 });
    both(&mut out, Item::Lantern { at: Vec3::new(gx + 3.0, 0.0, gz - 4.5), variant: 2 });
    // lanterns in the bases' rotundas, either side of the door to the hall
    for sx in [-1.0f32, 1.0] {
        both(&mut out, Item::Lantern { at: Vec3::new(sx * 7.0, 0.0, -at::BASE_Z + 14.0), variant: 3 });
    }

    // the sea floor: kelp beds and coral, blue half then mirrored
    let mut rng = Rng::new(0xA71A_2715);
    let lim = at::ARENA_RADIUS + 30.0;
    let mut z = -lim;
    while z < -0.5 {
        let mut x = -lim;
        while x <= lim {
            let (jx, jz) = (x + rng.range(-1.5, 1.5), (z + rng.range(-1.5, 1.5)).min(-0.6));
            if at::on_seabed(jx, jz) {
                let y = at::terrain_height(jx, jz);
                // kelp in beds: where a slow noise says so
                let bed = ((jx * 0.05).sin() * (jz * 0.043 + 1.3).cos()) > 0.2;
                if bed && rng.f32() < 0.35 {
                    both(&mut out, Item::Kelp { at: Vec3::new(jx, y - 0.2, jz), h: rng.range(6.0, 13.0), variant: (rng.f32() * 6.0) as i32 });
                } else if rng.f32() < 0.05 {
                    both(&mut out, Item::Coral { at: Vec3::new(jx, y - 0.1, jz), size: rng.range(0.8, 1.8), variant: (rng.f32() * 8.0) as i32 });
                }
            }
            x += 3.5;
        }
        z += 3.5;
    }
    out
}

fn turned(mut m: crate::voxel::VoxelMesh, yaw: f32) -> crate::voxel::VoxelMesh {
    let (s, c) = yaw.sin_cos();
    for i in m.inst.iter_mut() {
        let (x, z) = (i.off_ao[0], i.off_ao[2]);
        i.off_ao[0] = x * c + z * s;
        i.off_ao[2] = -x * s + z * c;
    }
    m
}

/// Meshes and collision for everything in [`placements`].
pub fn build() -> (Vec<Piece>, Blockers) {
    let mut blockers = Blockers::new(consts::arena_radius() + 20.0, 6.0);
    blockers.set_floor(at::floor());
    let mut pieces = Vec::new();
    let col = ap::column(WALL_H - 0.8, 0);
    let books: Vec<_> = (0..4).map(|k| ap::bookcase(3.6, k as f32)).collect();
    let mut kelps: std::collections::HashMap<(i32, i32), crate::voxel::VoxelMesh> = Default::default();
    for it in placements() {
        match it {
            Item::Column { at, variant } => {
                blockers.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 1.1 }, top: WALL_H, cover: Cover::NONE });
                pieces.push(Piece { mesh: col.clone(), pos: at, quarter: (variant % 4) as u8, vsize: 0.25, sway: 0.0 });
            }
            Item::Rib { at, yaw, span } => {
                // the mesh spans x; the hall runs along its z: turn by yaw
                pieces.push(Piece { mesh: turned(ap::rib(span, VAULT_RISE, 0.9), yaw), pos: at + Vec3::Y * (WALL_H - 0.2), quarter: 0, vsize: 0.3, sway: 0.0 });
            }
            Item::Vault { at, yaw, span, len } => {
                pieces.push(Piece { mesh: turned(ap::vault_glass(span, VAULT_RISE, (len - 0.9).max(0.7)), yaw), pos: at + Vec3::Y * (WALL_H - 0.2), quarter: 0, vsize: 0.7, sway: 0.0 });
            }
            Item::Dome { at, r } => {
                pieces.push(Piece { mesh: ap::dome(r, r * 0.55), pos: at, quarter: 0, vsize: 0.7, sway: 0.0 });
            }
            Item::Window { at, yaw, gold, seed } => {
                pieces.push(Piece { mesh: turned(ap::stained_glass(2.2, 5.2, gold, seed), yaw), pos: at, quarter: 0, vsize: 0.2, sway: 0.0 });
            }
            Item::Books { at, yaw, seed } => {
                blockers.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 1.0 }, top: 4.2, cover: Cover::NONE });
                pieces.push(Piece { mesh: turned(books[seed as usize % 4].clone(), yaw), pos: at, quarter: 0, vsize: 0.2, sway: 0.0 });
            }
            Item::Statue { at, yaw } => {
                let q = (((yaw / std::f32::consts::FRAC_PI_2).round() as i32).rem_euclid(4)) as u8;
                blockers.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 2.6 }, top: 8.5, cover: Cover::NONE });
                pieces.push(Piece { mesh: ap::statue(0.55, 1), pos: at, quarter: q, vsize: V_MED, sway: 0.0 });
            }
            Item::Lantern { at, variant } => {
                blockers.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 0.45 }, top: 2.8, cover: Cover::NONE });
                pieces.push(Piece { mesh: ap::sea_lantern(variant), pos: at, quarter: (variant % 4) as u8, vsize: 0.12, sway: 0.0 });
            }
            Item::Heart { at } => {
                blockers.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 2.4 }, top: 7.0, cover: Cover::NONE });
                pieces.push(Piece { mesh: ap::orichalcum(0), pos: at, quarter: 0, vsize: 0.25, sway: 0.0 });
            }
            Item::Kelp { at, h, variant } => {
                let key = ((h * 0.5).round() as i32, variant % 4);
                let mesh = kelps.entry(key).or_insert_with(|| ap::kelp(key.0 as f32 * 2.0, key.1)).clone();
                pieces.push(Piece { mesh, pos: at, quarter: (variant % 4) as u8, vsize: 0.25, sway: 0.7 });
            }
            Item::Coral { at, size, variant } => {
                pieces.push(Piece { mesh: ap::coral(size, variant), pos: at, quarter: (variant % 4) as u8, vsize: 0.2, sway: 0.0 });
            }
        }
    }
    let _ = Team::Blue;
    (pieces, blockers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dressing_is_fair_and_keeps_the_halls_clear() {
        let items = placements();
        let count = |f: &dyn Fn(&Item) -> bool| items.iter().filter(|i| f(i)).count();
        let columns = count(&|i| matches!(i, Item::Column { .. }));
        let ribs = count(&|i| matches!(i, Item::Rib { .. }));
        let vaults = count(&|i| matches!(i, Item::Vault { .. }));
        let domes = count(&|i| matches!(i, Item::Dome { .. }));
        let books = count(&|i| matches!(i, Item::Books { .. }));
        let kelp = count(&|i| matches!(i, Item::Kelp { .. }));
        let windows = count(&|i| matches!(i, Item::Window { .. }));
        assert!((20..120).contains(&columns), "columns {columns}");
        assert!((10..40).contains(&ribs) && vaults + 1 >= ribs, "ribs {ribs}, vaults {vaults}");
        assert_eq!(domes, 3, "two bases and the Library");
        assert!((8..40).contains(&books), "bookcases {books}");
        assert!(kelp > 60, "kelp {kelp}");
        assert!(windows >= 6, "windows {windows}");
        // mirrored
        for it in &items {
            let p = item_at(it);
            if p.z.abs() > 0.5 {
                assert!(items.iter().any(|o| (item_at(o) - Vec3::new(p.x, p.y, -p.z)).length() < 1e-3), "{it:?} has no twin");
            }
        }
        crate::map::with(crate::map::MapId::Atlantis, || {
            let f = at::floor();
            for it in &items {
                let p = item_at(it);
                match it {
                    // solid things stand on the floor at its edge, never in
                    // the lane's middle
                    Item::Column { .. } | Item::Books { .. } | Item::Statue { .. } | Item::Lantern { .. } | Item::Heart { .. } => {
                        assert!(f.fits(p.x, p.z, 0.3), "{it:?} is off the floor");
                        assert!(crate::render::lane_road(p.x, p.z) > 4.0 || f.sdf(p.x, p.z) > -3.0, "{it:?} stands in the lane");
                        assert!(!crate::bosses::in_pit(p.x, p.z) || matches!(it, Item::Books { .. }), "{it:?} in a boss pit");
                    }
                    // sea life only out on the sea floor
                    Item::Kelp { .. } | Item::Coral { .. } => assert!(at::on_seabed(p.x, p.z), "{it:?} is not on the sea floor"),
                    _ => {}
                }
            }
            // nothing solid near a tower, the Elders or the fountains
            for team in [Team::Blue, Team::Red] {
                let spots = [team.fountain(), team.titan_pos(), consts::tower_pos(Lane::Mid, team, true), consts::tower_pos(Lane::Mid, team, false)];
                for it in &items {
                    if matches!(it, Item::Column { .. } | Item::Statue { .. } | Item::Heart { .. } | Item::Lantern { .. }) {
                        for s in spots {
                            assert!((item_at(it) - s).length() > 4.0, "{it:?} crowds {s}");
                        }
                    }
                }
            }
        });
    }
}
