//! skyworld — what stands on Starwell Glacier.
//!
//! `sky` is the ground; this is the dressing, for `scene` to draw and
//! `Blockers` to collide against. Placements are data ([`placements`],
//! tested without a GPU); [`build`] turns them into meshes and collision.
//!
//! What is here: the old ones' marble statues along both lanes and in a
//! ring round the Moonwell, marble arches where the star-bridges and the
//! ferries come in to the well, ice spires crowding every island's lip,
//! golden spirit larches on the islands (never on a road), and shards of
//! fallen starlight at the edges.

use glam::Vec3;

use crate::blockers::{Blocker, Blockers, Cover, PropKind, Shape};
use crate::consts::{self, Team};
use crate::scene::{Piece, V_MED, V_SM};
use crate::sim::Rng;
use crate::sky;
use crate::skyprops;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    /// A marble cat (or owl) of the old ones, `scale` x the jungle statue,
    /// facing `yaw` (0 = +z).
    Statue { at: Vec3, yaw: f32, owl: bool, scale: f32 },
    /// A marble arch spanning a way in; `yaw` is the way's direction.
    Arch { at: Vec3, span: f32, height: f32, yaw: f32 },
    /// Glacier ice on an island's lip, leaning out over the sky.
    Spire { at: Vec3, size: f32, lean: f32, yaw: f32, variant: i32 },
    /// A spirit larch.
    Larch { at: Vec3, scale: f32, variant: i32 },
    /// Fallen starlight.
    Shard { at: Vec3, size: f32, variant: i32 },
}

/// Keep off what a match needs clear: roads, bases, towers, the pool, the
/// boss grounds, the heal springs, the ferry piers.
fn clear_of_play(x: f32, z: f32) -> bool {
    let p = Vec3::new(x, 0.0, z);
    crate::render::lane_road(x, z) > sky::LANE_R + 3.0
        && x.hypot(z) > sky::POOL_R + 14.0
        && !crate::bosses::in_pit(x, z)
        && !consts::in_tower_clearing(x, z)
        && [Team::Blue, Team::Red].iter().all(|t| (t.base_pos() - p).length() > 40.0 && (t.fountain() - p).length() > 22.0)
        && sky::heal_tree_positions().iter().all(|h| (*h - p).length() > 6.0)
        && sky::ferry_routes().iter().all(|(a, b)| (*a - p).length() > 9.0 && (*b - p).length() > 9.0)
        && causeway_dist(x, z) > sky::CAUSEWAY_R + 2.0
}

fn causeway_dist(x: f32, z: f32) -> f32 {
    let p = Vec3::new(x, 0.0, z);
    let mut best = f32::MAX;
    for zs in [1.0f32, -1.0] {
        for &((ax, az), (bx, bz)) in &sky::CAUSEWAYS {
            let (a, b) = (Vec3::new(ax, 0.0, az * zs), Vec3::new(bx, 0.0, bz * zs));
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            best = best.min((a + ab * t - p).length());
        }
    }
    for &((ax, az), (bx, bz)) in &sky::BRIDGES {
        let (a, b) = (Vec3::new(ax, 0.0, az), Vec3::new(bx, 0.0, bz));
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
        best = best.min((a + ab * t - p).length());
    }
    best
}

/// Everything placed on the sky map. Computed AS the sky map whatever is
/// active (it mixes the map's geometry with map-dispatched calls).
pub fn placements() -> Vec<Item> {
    crate::map::with(crate::map::MapId::Sky, placements_here)
}

fn placements_here() -> Vec<Item> {
    let f = sky::floor();
    let mut rng = Rng::new(0x5CA1_AB1E);
    let mut out = Vec::new();
    let mut both = |out: &mut Vec<Item>, it: Item| {
        let flip = |p: Vec3| Vec3::new(p.x, p.y, -p.z);
        let twin = match it {
            Item::Statue { at, yaw, owl, scale } => Item::Statue { at: flip(at), yaw: std::f32::consts::PI - yaw, owl, scale },
            Item::Arch { at, span, height, yaw } => Item::Arch { at: flip(at), span, height, yaw: std::f32::consts::PI - yaw },
            Item::Spire { at, size, lean, yaw, variant } => Item::Spire { at: flip(at), size, lean, yaw: std::f32::consts::PI - yaw, variant },
            Item::Larch { at, scale, variant } => Item::Larch { at: flip(at), scale, variant },
            Item::Shard { at, size, variant } => Item::Shard { at: flip(at), size, variant },
        };
        let on_axis = match it {
            Item::Statue { at, .. } | Item::Arch { at, .. } | Item::Spire { at, .. } | Item::Larch { at, .. } | Item::Shard { at, .. } => at.z.abs() < 0.5,
        };
        out.push(it);
        if !on_axis {
            out.push(twin);
        }
    };

    // The old ones: statues already mirrored by `sky::statues`. The lane
    // statues are colossal (0.9); the ring round the well smaller (0.7) and
    // alternating cat and owl.
    for (k, (at, yaw)) in sky::statues().into_iter().enumerate() {
        let ring = at.length() < sky::POOL_R + 12.0;
        out.push(Item::Statue { at, yaw, owl: ring && k % 2 == 1, scale: if ring { 0.7 } else { 0.9 } });
    }

    // Marble arches where the star-bridges land on the well island.
    for &(_, (bx, _)) in &sky::BRIDGES {
        let at = Vec3::new(bx - bx.signum() * 3.0, 0.0, 0.0);
        out.push(Item::Arch { at, span: sky::BRIDGE_R * 2.0 + 3.0, height: 11.0, yaw: std::f32::consts::FRAC_PI_2 });
    }

    // Ice on every lip, blue half then mirrored.
    let lim = sky::ARENA_RADIUS + 10.0;
    let mut z = -lim;
    while z < -0.5 {
        let mut x = -lim;
        while x <= lim {
            let (jx, jz) = (x + rng.range(-1.5, 1.5), (z + rng.range(-1.5, 1.5)).min(-0.6));
            let d = f.sdf(jx, jz);
            if (1.2..4.5).contains(&d) && rng.f32() < 0.07 && x.hypot(z) > sky::WELL_ISLAND_R + 8.0 && sky::ferry_routes().iter().all(|(a, b)| (*a - Vec3::new(jx, 0.0, jz)).length() > 10.0 && (*b - Vec3::new(jx, 0.0, jz)).length() > 10.0) {
                // lean out, away from the floor
                let e = 0.5;
                let g = Vec3::new(f.sdf(jx + e, jz) - f.sdf(jx - e, jz), 0.0, f.sdf(jx, jz + e) - f.sdf(jx, jz - e)).normalize_or_zero();
                let yaw = g.x.atan2(g.z);
                both(&mut out, Item::Spire { at: Vec3::new(jx, sky::RIM_H * 0.3, jz), size: rng.range(0.8, 1.8), lean: rng.range(0.15, 0.5), yaw, variant: (rng.f32() * 6.0) as i32 });
            }
            // larches on the islands, never on a road
            if d < -5.0 && clear_of_play(jx, jz) && jx.hypot(jz) > sky::WELL_ISLAND_R + 2.0 && rng.f32() < 0.08 {
                both(&mut out, Item::Larch { at: Vec3::new(jx, 0.0, jz), scale: rng.range(0.9, 1.35), variant: (rng.f32() * 8.0) as i32 });
            }
            // starlight at the edges
            if (-3.5..-1.0).contains(&d) && clear_of_play(jx, jz) && rng.f32() < 0.25 {
                both(&mut out, Item::Shard { at: Vec3::new(jx, 0.0, jz), size: rng.range(0.6, 1.0), variant: (rng.f32() * 6.0) as i32 });
            }
            x += 4.0;
        }
        z += 4.0;
    }
    out
}

/// Meshes and collision for everything in [`placements`].
pub fn build() -> (Vec<Piece>, Blockers) {
    let mut blockers = Blockers::new(consts::arena_radius() + 20.0, 6.0);
    blockers.set_floor(sky::floor());
    let mut pieces = Vec::new();
    let mut spires: std::collections::HashMap<(i32, i32, i32, i32), crate::voxel::VoxelMesh> = Default::default();
    let mut larches: std::collections::HashMap<(i32, i32), crate::voxel::VoxelMesh> = Default::default();
    for (i, it) in placements().into_iter().enumerate() {
        match it {
            Item::Statue { at, yaw, owl, scale } => {
                let mesh = if owl { skyprops::marble_owl(scale, i as i32 % 4) } else { skyprops::marble_cat(scale, i as i32 % 4) };
                // the mesh faces +z; turn it by the nearest quarter
                let q = (((yaw / std::f32::consts::FRAC_PI_2).round() as i32).rem_euclid(4)) as u8;
                blockers.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 4.2 * scale }, top: 14.0 * scale, cover: Cover::NONE });
                pieces.push(Piece { mesh, pos: at, quarter: q, vsize: V_MED, sway: 0.0 });
            }
            Item::Arch { at, span, height, yaw } => {
                pieces.push(Piece { mesh: skyprops::marble_arch(span, height, yaw, i as i32 % 3), pos: at, quarter: 0, vsize: V_MED, sway: 0.0 });
            }
            Item::Spire { at, size, lean, yaw, variant } => {
                let yb = ((yaw / std::f32::consts::TAU * 16.0).round() as i32).rem_euclid(16);
                let key = (variant, (size * 4.0).round() as i32, (lean * 4.0).round() as i32, yb);
                let mesh = spires
                    .entry(key)
                    .or_insert_with(|| skyprops::ice_spire(key.1 as f32 / 4.0, key.0, key.2 as f32 / 4.0, yb as f32 / 16.0 * std::f32::consts::TAU))
                    .clone();
                pieces.push(Piece { mesh, pos: at, quarter: 0, vsize: 0.6, sway: 0.0 });
            }
            Item::Larch { at, scale, variant } => {
                let key = ((scale * 4.0).round() as i32, variant % 4);
                let mesh = larches.entry(key).or_insert_with(|| skyprops::spirit_larch(key.0 as f32 / 4.0, key.1)).clone();
                blockers.push_prop(PropKind::Pine, at.x, at.z, 0.0, 1.0);
                pieces.push(Piece { mesh, pos: at, quarter: (variant % 4) as u8, vsize: V_MED, sway: 0.4 });
            }
            Item::Shard { at, size, variant } => {
                pieces.push(Piece { mesh: skyprops::star_shard(size, variant), pos: at - Vec3::Y * 0.3, quarter: (variant % 4) as u8, vsize: V_SM, sway: 0.0 });
            }
        }
    }
    (pieces, blockers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dressing_is_fair_and_keeps_off_the_roads() {
        let items = placements();
        let at = |it: &Item| match *it {
            Item::Statue { at, .. } | Item::Arch { at, .. } | Item::Spire { at, .. } | Item::Larch { at, .. } | Item::Shard { at, .. } => at,
        };
        let count = |f: &dyn Fn(&Item) -> bool| items.iter().filter(|i| f(i)).count();
        let (statues, spires, larches, shards) = (
            count(&|i| matches!(i, Item::Statue { .. })),
            count(&|i| matches!(i, Item::Spire { .. })),
            count(&|i| matches!(i, Item::Larch { .. })),
            count(&|i| matches!(i, Item::Shard { .. })),
        );
        assert!(statues >= 20, "statues {statues}");
        assert!((60..1500).contains(&spires), "spires {spires}");
        assert!((20..600).contains(&larches), "larches {larches}");
        assert!(shards >= 10, "shards {shards}");
        // every item off the axis has a mirror twin
        for it in &items {
            let p = at(it);
            if p.z.abs() > 0.5 {
                assert!(items.iter().any(|o| (at(o) - Vec3::new(p.x, p.y, -p.z)).length() < 1e-3), "{it:?} has no twin");
            }
        }
        crate::map::with(crate::map::MapId::Sky, || {
            for it in &items {
                if let Item::Larch { at, .. } = it {
                    assert!(crate::render::lane_road(at.x, at.z) > sky::LANE_R, "a larch on a road at {at}");
                    assert!(sky::floor().fits(at.x, at.z, 1.0), "a larch over the sky at {at}");
                }
                if let Item::Spire { at, .. } = it {
                    assert!(sky::floor().sdf(at.x, at.z) > 0.5, "a spire on the floor at {at}");
                }
            }
        });
    }
}
