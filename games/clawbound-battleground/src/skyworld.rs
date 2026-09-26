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
    /// A nook in a cliff face under an island's lip, a tiny scene on its
    /// shelf (`skyprops::cliff_nook`), facing out along `facing`.
    Nook { at: Vec3, facing: Vec3, kind: u8, variant: i32 },
    /// A lift pad (`lift`), at its floor's height.
    Pad { at: Vec3, variant: i32 },
    /// A star lantern at a road's edge.
    Lantern { at: Vec3, variant: i32 },
    /// A broken marble column of the old ones.
    Column { at: Vec3, variant: i32 },
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

/// Distance to the nearest linking way (causeway or star-bridge), as it
/// bends (`sky::ways`).
fn causeway_dist(x: f32, z: f32) -> f32 {
    sky::causeway_dist(x, z).min(sky::bridge_dist(x, z))
}

/// Everything placed on the sky map. Computed AS the sky map whatever is
/// active (it mixes the map's geometry with map-dispatched calls).
pub fn placements() -> Vec<Item> {
    crate::map::with(crate::map::MapId::Sky, placements_here)
}

fn placements_here() -> Vec<Item> {
    // the islands' dressing is laid on the islands' floor alone: the Aerie
    // (`sky::AERIE`) is dressed on its own below
    let f = sky::lower_floor();
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
            Item::Nook { at, facing, kind, variant } => Item::Nook { at: flip(at), facing: flip(facing), kind, variant },
            Item::Lantern { at, variant } => Item::Lantern { at: flip(at), variant },
            Item::Column { at, variant } => Item::Column { at: flip(at), variant },
            Item::Pad { at, variant } => Item::Pad { at: flip(at), variant },
        };
        let on_axis = item_at(&it).z.abs() < 0.5;
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

    // Nooks in the cliff faces: walk the blue half's island edges, one
    // every 75 u or so, not at a way onto an island or a ferry pier.
    let mut nooks: Vec<Vec3> = Vec::new();
    let mut z = -lim;
    while z < -8.0 {
        let mut x = -lim;
        while x <= lim {
            let d = f.sdf(x, z);
            if (sky::EDGE - 1.2..sky::EDGE).contains(&d)
                && nooks.iter().all(|n| n.distance(Vec3::new(x, 0.0, z)) > 75.0)
                && sky::causeway_dist(x, z) > sky::CAUSEWAY_R * 1.5 + 8.0
                && sky::bridge_dist(x, z) > 14.0
                && sky::ferry_routes().iter().all(|(a, b)| a.distance(Vec3::new(x, 0.0, z)) > 14.0 && b.distance(Vec3::new(x, 0.0, z)) > 14.0)
                && crate::render::lane_road(x, z) > 6.0
            {
                let e = 0.5;
                let face = Vec3::new(f.sdf(x + e, z) - f.sdf(x - e, z), 0.0, f.sdf(x, z + e) - f.sdf(x, z - e)).normalize_or_zero();
                if face.length() > 0.5 {
                    nooks.push(Vec3::new(x, 0.0, z));
                    // on the face a little under the lip, its back against
                    // the rock (which curls in beneath the lip, ~1 u in at
                    // this depth: `sky::underside`), shelf and hood out in
                    // the air
                    let at = Vec3::new(x, -4.2, z) + face * 0.8;
                    let k = nooks.len();
                    both(&mut out, Item::Nook { at, facing: face, kind: (k % 3) as u8, variant: k as i32 });
                }
            }
            x += 2.0;
        }
        z += 2.0;
    }

    // Star lanterns along both lanes' edges, between the statues.
    let statues: Vec<Vec3> = out.iter().filter_map(|it| if let Item::Statue { at, .. } = it { Some(*at) } else { None }).collect();
    for (li, lane) in [crate::consts::Lane::Top, crate::consts::Lane::Bot].into_iter().enumerate() {
        let pts = lane.polyline();
        let len: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        for (k, fr) in [0.09f32, 0.27, 0.43, 0.57, 0.73, 0.91].into_iter().enumerate() {
            let p = crate::pass::along(&pts, fr * len);
            let q = crate::pass::along(&pts, fr * len + 1.0);
            let dir = (q - p).normalize_or_zero();
            let side = Vec3::new(-dir.z, 0.0, dir.x);
            for s in [1.0f32, -1.0] {
                let at = p + side * s * (sky::LANE_R - 1.2);
                if f.sdf(at.x, at.z) < -0.8
                    && statues.iter().all(|st| st.distance(at) > 7.0)
                    && sky::bridge_dist(at.x, at.z) > sky::BRIDGE_R + 5.0
                    && sky::causeway_dist(at.x, at.z) > sky::CAUSEWAY_R + 5.0
                    && !consts::in_tower_clearing(at.x, at.z)
                {
                    out.push(Item::Lantern { at, variant: (li * 12 + k * 2 + (s > 0.0) as usize) as i32 });
                }
            }
        }
    }

    // Broken marble columns on the glades and the terrace.
    let mut rng = Rng::new(0xC0_1C0);
    for isl in sky::ISLANDS.iter().filter(|i| i.r > 20.0) {
        let mut placed = 0;
        for _ in 0..40 {
            if placed >= 3 {
                break;
            }
            let a = rng.f32() * std::f32::consts::TAU;
            let r = rng.range(6.0, isl.r - 6.0);
            let at = Vec3::new(isl.x + a.cos() * r, 0.0, isl.z + a.sin() * r);
            if clear_of_play(at.x, at.z) && out.iter().all(|it| item_at(it).distance(at) > 4.0) {
                both(&mut out, Item::Column { at, variant: (rng.f32() * 8.0) as i32 });
                placed += 1;
            }
        }
    }

    // The lift pads, both ends of every lift (`sky::lifts` is already
    // both halves).
    for (k, (lo, hi)) in sky::lifts().into_iter().enumerate() {
        out.push(Item::Pad { at: lo, variant: k as i32 * 2 });
        out.push(Item::Pad { at: hi, variant: k as i32 * 2 + 1 });
    }

    // The Aerie: star lanterns round each islet's rim and a broken column
    // on the big one -- kept off its bridges' ends and its lift pad, which
    // is where the walking goes.
    let up = sky::upper_floor();
    let pads: Vec<Vec3> = sky::lifts().into_iter().map(|(_, hi)| hi).collect();
    for (k, &(x, z, r)) in sky::AERIE.iter().enumerate() {
        for j in 0..8 {
            let a = j as f32 / 8.0 * std::f32::consts::TAU + 0.3;
            let at = Vec3::new(x + a.cos() * (r - 1.2), sky::UPPER_Y, z + a.sin() * (r - 1.2));
            // off a bridge: a bridge end widens the floor past the rim
            let open = up.sdf(at.x + a.cos() * 2.5, at.z + a.sin() * 2.5) < 0.0;
            if !open && pads.iter().all(|p| p.distance(at) > crate::lift::PAD_R + 2.0) {
                both(&mut out, Item::Lantern { at, variant: 40 + (k * 8 + j) as i32 });
            }
        }
        if r > 12.0 {
            both(&mut out, Item::Column { at: Vec3::new(x + 4.5, sky::UPPER_Y, z + 3.0), variant: 5 });
        }
    }
    out
}

fn item_at(it: &Item) -> Vec3 {
    match *it {
        Item::Statue { at, .. }
        | Item::Arch { at, .. }
        | Item::Spire { at, .. }
        | Item::Larch { at, .. }
        | Item::Shard { at, .. }
        | Item::Nook { at, .. }
        | Item::Lantern { at, .. }
        | Item::Column { at, .. }
        | Item::Pad { at, .. } => at,
    }
}

/// Turn a mesh made facing +z to face `yaw` (0 = +z) about its origin.
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
            Item::Nook { at, facing, kind, variant } => {
                let yaw = facing.x.atan2(facing.z);
                pieces.push(Piece { mesh: turned(skyprops::cliff_nook(kind, variant), yaw), pos: at, quarter: 0, vsize: skyprops::NOOK_VOX, sway: 0.0 });
            }
            Item::Lantern { at, variant } => {
                blockers.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 0.45 }, top: at.y + 2.8, cover: Cover::NONE });
                pieces.push(Piece { mesh: skyprops::star_lantern(variant), pos: at, quarter: (variant % 4) as u8, vsize: 0.12, sway: 0.0 });
            }
            Item::Column { at, variant } => {
                blockers.push(Blocker { shape: Shape::Circle { x: at.x, z: at.z, r: 1.3 }, top: at.y + 6.0, cover: Cover::NONE });
                pieces.push(Piece { mesh: skyprops::marble(crate::props::ruin_pillar(1.0, variant), variant as f32 * 0.9), pos: at, quarter: (variant % 4) as u8, vsize: V_MED, sway: 0.0 });
            }
            Item::Pad { at, variant } => {
                pieces.push(Piece { mesh: skyprops::lift_pad(crate::lift::PAD_R, variant), pos: at, quarter: 0, vsize: 0.15, sway: 0.0 });
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
        let at = item_at;
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
        let (nooks, lanterns, columns) = (count(&|i| matches!(i, Item::Nook { .. })), count(&|i| matches!(i, Item::Lantern { .. })), count(&|i| matches!(i, Item::Column { .. })));
        assert!((16..=50).contains(&nooks), "nooks in the cliffs {nooks}");
        assert!(lanterns >= 8, "lanterns {lanterns}");
        assert!(columns >= 6, "columns {columns}");
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
                if let Item::Nook { at, facing, .. } = it {
                    // under the lip, set into the face, looking out
                    assert!(at.y < -2.0 && (sky::floor().sdf(at.x, at.z) - sky::EDGE).abs() < 2.5, "a nook on the face at {at}");
                    assert!(sky::floor().sdf(at.x + facing.x * 4.0, at.z + facing.z * 4.0) > sky::floor().sdf(at.x, at.z), "facing out at {at}");
                }
                if let Item::Lantern { at, .. } | Item::Column { at, .. } = it {
                    assert!(sky::floor().fits(at.x, at.z, 0.8), "on the floor at {at}");
                    // standing at its own floor's height, not the islands'
                    // under an Aerie islet or the Aerie's over the islands
                    assert!((at.y - sky::floor_height(at.x, at.z)).abs() < 1e-3, "at its floor's height: {at}");
                }
            }
            // a pad on every lift end, level with its floor, and nothing
            // solid on it or crowding it
            let pads: Vec<Vec3> = items.iter().filter_map(|i| if let Item::Pad { at, .. } = i { Some(*at) } else { None }).collect();
            assert_eq!(pads.len(), sky::lifts().len() * 2);
            let aerie_lanterns = items.iter().filter(|i| matches!(i, Item::Lantern { at, .. } if at.y > 10.0)).count();
            assert!((12..=48).contains(&aerie_lanterns), "lanterns on the Aerie {aerie_lanterns}");
            for p in &pads {
                assert!((p.y - sky::floor_height(p.x, p.z)).abs() < 1e-3, "a pad at its floor's height: {p}");
                for it in &items {
                    if matches!(it, Item::Lantern { .. } | Item::Column { .. } | Item::Statue { .. } | Item::Larch { .. } | Item::Arch { .. }) {
                        let q = at(it);
                        assert!(Vec3::new(q.x - p.x, 0.0, q.z - p.z).length() > crate::lift::PAD_R + 1.0 || (q.y - p.y).abs() > 10.0, "{it:?} crowds the pad at {p}");
                    }
                }
            }
        });
    }
}
