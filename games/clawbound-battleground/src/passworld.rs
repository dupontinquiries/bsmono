//! passworld — what stands on Twinfang Pass.
//!
//! `pass` is the ground; this is the scenery on it, for `scene` to batch
//! and draw and for `Blockers` to collide against. Sparse on purpose: a
//! bleak pass has little on it, and what there is has to earn its place --
//! scree at the foot of the cliffs, a few wind-bent pines and tough scrub in
//! the clearings, grass only where it is sheltered, pines clinging to the
//! plateau above, and a waterfall at the head of every stream.
//!
//! Every placement goes through [`placements`] (data, testable without a
//! GPU or a mesh); [`build`] turns them into meshes and collision.

use glam::Vec3;

use crate::blockers::{Blockers, PropKind};
use crate::consts::{self, lane_clearance, Team};
use crate::pass;
use crate::props;
use crate::scene::{Piece, V_MED, V_SM};
use crate::sim::Rng;

/// What goes where. `Scenery` kinds are drawn but are not collision (they
/// stand on the plateau, where nobody can reach).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    Prop { kind: PropKind, x: f32, z: f32, scale: f32, quarter: u8 },
    /// A pine on the rock above the floor, at that height.
    HighPine { x: f32, z: f32, y: f32, scale: f32 },
    /// A waterfall at a cliff face: `at` is the foot of the face, `height`
    /// the drop, `facing` the unit vector from the face out over the floor.
    Fall { at: Vec3, height: f32, facing: Vec3 },
    /// Broken rock jutting out of a cliff or standing on the plateau
    /// (`props::crag`). Rooted in the rock: scenery, not collision -- the
    /// floor's edge is already the wall.
    Crag { at: Vec3, size: f32, lean: f32, facing: Vec3, variant: i32, coarse: bool },
}

/// Distinct crag meshes built; placements share them.
pub const CRAG_VARIANTS: i32 = 10;

/// Keep off anything a match needs clear: lanes, bases, towers, streams,
/// boss grounds.
fn clear_of_play(x: f32, z: f32, lane_margin: f32) -> bool {
    lane_clearance(x, z) > lane_margin
        && pass::river_dist(x, z) > pass::STREAM_HALF_WIDTH + 1.5
        && !crate::bosses::in_pit(x, z)
        && !consts::in_tower_clearing(x, z)
        && [Team::Blue, Team::Red].iter().all(|t| (t.base_pos() - Vec3::new(x, 0.0, z)).length() > 26.0)
        // the pools under the falls are for looking at
        && pass::streams().iter().all(|s| (s[1] - Vec3::new(x, 0.0, z)).length() > 7.0)
}

/// Fold a z onto the blue half, clear of the axis: what is placed there is
/// then mirrored, so a clearing that straddles z = 0 (the boss grounds) is
/// as fair as one that does not.
fn blue_half(z: f32) -> f32 {
    -(z.abs().max(0.6))
}

pub fn placements() -> Vec<Item> {
    let f = pass::floor();
    let mut rng = Rng::new(0x7F1A_F00D);
    let mut out = Vec::new();

    // Scree: rocks fallen at the foot of the cliffs, on the floor's edge.
    // Mirrored in z so the halves are fair.
    let lim = 165.0 * pass::S;
    let mut z = -lim;
    while z <= 0.0 {
        let mut x = -lim;
        while x <= lim {
            let (jx, jz) = (x + rng.range(-1.2, 1.2), z + rng.range(-1.2, 1.2));
            let d = f.sdf(jx, jz);
            if (-3.0..=-1.4).contains(&d) && clear_of_play(jx, jz, 6.5) {
                let roll = rng.f32();
                let q = (rng.f32() * 4.0) as u8;
                let item = if roll < 0.12 {
                    Some(Item::Prop { kind: PropKind::Rock, x: jx, z: jz, scale: rng.range(0.6, 1.1), quarter: q })
                } else if roll < 0.16 {
                    Some(Item::Prop { kind: PropKind::BoulderCluster, x: jx, z: jz, scale: rng.range(0.7, 1.0), quarter: q })
                } else {
                    None
                };
                if let Some(Item::Prop { kind, x, z, scale, quarter }) = item {
                    out.push(Item::Prop { kind, x, z, scale, quarter });
                    if z < -0.5 {
                        out.push(Item::Prop { kind, x, z: -z, scale, quarter });
                    }
                }
            }
            x += 3.0;
        }
        z += 3.0;
    }

    // The clearings: a pine or two, scrub near the edge, a little grass.
    for (cx, cz, r) in pass::clearings() {
        if cz > 0.5 {
            continue; // placed with the blue half and mirrored below
        }
        let mut here = Vec::new();
        let pines = if crate::bosses::in_pit(cx, cz) { 0 } else { 1 + (rng.f32() * 2.0) as usize };
        for _ in 0..pines {
            let (a, d) = (rng.f32() * std::f32::consts::TAU, r * rng.range(0.35, 0.7));
            let (x, z) = (cx + a.cos() * d, blue_half(cz + a.sin() * d));
            if f.fits(x, z, 3.0) && clear_of_play(x, z, 5.0) {
                here.push(Item::Prop { kind: PropKind::Pine, x, z, scale: rng.range(0.7, 1.05), quarter: 0 });
            }
        }
        for k in 0..(4 + (rng.f32() * 5.0) as usize) {
            let (a, d) = (rng.f32() * std::f32::consts::TAU, r * rng.range(0.55, 0.95));
            let (x, z) = (cx + a.cos() * d, blue_half(cz + a.sin() * d));
            if f.fits(x, z, 0.8) && clear_of_play(x, z, 4.0) {
                let kind = if k % 3 == 0 { PropKind::Bush } else { PropKind::Fern };
                here.push(Item::Prop { kind, x, z, scale: 1.0, quarter: 0 });
            }
        }
        for it in here {
            out.push(it);
            if let Item::Prop { kind, x, z, scale, quarter } = it {
                if z < -0.5 {
                    out.push(Item::Prop { kind, x, z: -z, scale, quarter });
                }
            }
        }
    }

    // Occasional grass and scrub along the lanes' margins: very occasional.
    let mut z = -155.0 * pass::S;
    while z <= 0.0 {
        let mut x = -lim;
        while x <= lim {
            let d = f.sdf(x, z);
            if (-2.2..=-1.0).contains(&d) && rng.f32() < 0.07 && clear_of_play(x, z, 4.5) {
                let kind = if rng.f32() < 0.3 { PropKind::Bush } else { PropKind::Fern };
                out.push(Item::Prop { kind, x, z, scale: 1.0, quarter: 0 });
                if z < -0.5 {
                    out.push(Item::Prop { kind, x, z: -z, scale: 1.0, quarter: 0 });
                }
            }
            x += 4.0;
        }
        z += 4.0;
    }

    // Pines clinging to the plateau: out of reach, scenery only, thinning
    // with height and gone above the treeline.
    let hl = consts::arena_radius() + 30.0;
    let mut z = -hl;
    while z <= hl {
        let mut x = -hl;
        while x <= hl {
            let (jx, jz) = (x + rng.range(-3.0, 3.0), z + rng.range(-3.0, 3.0));
            let d = f.sdf(jx, jz);
            let h = pass::terrain_height(jx, jz);
            let chance = 0.22 * (1.0 - ((h - 14.0) / 16.0).clamp(0.0, 1.0));
            if d > 3.5 && h < 30.0 && Vec3::new(jx, 0.0, jz).length() < consts::arena_radius() + 25.0 && rng.f32() < chance {
                out.push(Item::HighPine { x: jx, z: jz, y: h - 0.3, scale: rng.range(0.6, 1.0) });
            }
            x += 8.0;
        }
        z += 8.0;
    }

    // Crags: the walls broken into rock. Three kinds of place:
    //  - at the foot of a wall, erupting from the ground against it;
    //  - partway up a face, jutting out and leaning over the floor;
    //  - on the plateau above, spires on the skyline.
    // All rooted in the rock (floor sdf > 0.6), so no lane, choke or
    // clearing gets narrower. Spaced by a coarse hash so they cluster
    // naturally without piling up.
    {
        let out_of = |x: f32, z: f32| {
            // from the rock toward the floor: down the SDF's slope
            let e = 0.5;
            Vec3::new(f.sdf(x - e, z) - f.sdf(x + e, z), 0.0, f.sdf(x, z - e) - f.sdf(x, z + e)).normalize_or_zero()
        };
        let mut taken: std::collections::HashSet<(i32, i32)> = Default::default();
        let mut free = |x: f32, z: f32, cell: f32| {
            let k = ((x / cell).floor() as i32, (z / cell).floor() as i32);
            taken.insert(k)
        };
        let lim = consts::arena_radius() + 20.0;
        let chokes = pass::choke_points();
        let near_choke = |x: f32, z: f32| chokes.iter().any(|(p, _)| (p.x - x).hypot(p.z - z) < 20.0);
        let mut z = -lim;
        while z <= lim {
            let mut x = -lim;
            while x <= lim {
                let (jx, jz) = (x + rng.range(-1.0, 1.0), z + rng.range(-1.0, 1.0));
                let d = f.sdf(jx, jz);
                let v = (rng.f32() * CRAG_VARIANTS as f32) as i32;
                // Near a choke the crags stay small and upright: one leaning
                // out over a 7.6 u narrows hides the gap the choke exists to
                // show (seen in the first render).
                let tight = near_choke(jx, jz);
                if (0.6..2.4).contains(&d) && free(jx, jz, 8.5) {
                    let top = pass::terrain_height(jx, jz).max(2.0);
                    let facing = out_of(jx, jz);
                    if rng.f32() < 0.55 || tight {
                        // at the foot
                        let (sz, ln) = if tight { (rng.range(0.55, 0.8), rng.range(0.1, 0.3)) } else { (rng.range(0.65, 1.05), rng.range(0.25, 0.6)) };
                        out.push(Item::Crag { at: Vec3::new(jx, -0.4, jz), size: sz, lean: ln, facing, variant: v, coarse: false });
                    } else if top > 6.0 {
                        // up the face, jutting out
                        let y = rng.range(2.5, top * 0.7);
                        out.push(Item::Crag { at: Vec3::new(jx, y, jz), size: rng.range(0.7, 1.25), lean: rng.range(0.7, 1.0), facing, variant: v, coarse: y > 9.0 });
                    }
                } else if d > 5.0 && d < 60.0 && rng.f32() < 0.03 && free(jx, jz, 16.0) {
                    let top = pass::terrain_height(jx, jz);
                    if top < 60.0 {
                        out.push(Item::Crag { at: Vec3::new(jx, top - 0.8, jz), size: rng.range(1.0, 1.6), lean: rng.range(0.0, 0.3), facing: out_of(jx, jz), variant: v, coarse: true });
                    }
                }
                x += 2.5;
            }
            z += 2.5;
        }
    }

    // A waterfall at the head of every stream: find the cliff face on the
    // line from the pool back up to the lip.
    for fall in pass::falls() {
        let dir = (fall.foot - fall.lip).normalize_or_zero();
        let mut p = fall.foot;
        for _ in 0..80 {
            if f.sdf(p.x, p.z) > -0.4 {
                break;
            }
            p -= dir * 0.2;
        }
        out.push(Item::Fall { at: Vec3::new(p.x, 0.0, p.z), height: fall.height, facing: dir });
    }
    out
}

/// Cold and drained: the jungle's plant meshes pulled toward a grey-green
/// and darkened. The pass reuses the props; it should not reuse their
/// colour -- the first render had jungle-bright ferns on grey stone.
fn weathered(mut m: crate::voxel::VoxelMesh, grey: f32, dark: f32) -> crate::voxel::VoxelMesh {
    for i in m.inst.iter_mut() {
        let [r, g, b, a] = i.color;
        let l = r * 0.3 + g * 0.55 + b * 0.15;
        let mix = |c: f32| (c + (l - c) * grey) * dark;
        i.color = [mix(r), mix(g), mix(b) * 1.03, a];
    }
    m
}

/// Meshes and collision for everything in [`placements`].
pub fn build() -> (Vec<Piece>, Blockers) {
    let mut blockers = Blockers::new(consts::arena_radius() + 20.0, 6.0);
    blockers.set_floor(pass::floor());
    let mut pieces = Vec::new();
    // crag meshes by (variant, size step, lean step): a few dozen grids
    // for hundreds of placements
    let mut crags: std::collections::HashMap<(i32, i32, i32), crate::voxel::VoxelMesh> = Default::default();
    for (i, it) in placements().into_iter().enumerate() {
        let v = i as i32;
        match it {
            Item::Prop { kind, x, z, scale, quarter } => {
                let (mesh, vs, sway) = match kind {
                    PropKind::Rock => (props::rock(scale), V_MED, 0.0),
                    PropKind::BoulderCluster => (props::boulder_cluster(scale, v), V_MED, 0.0),
                    PropKind::Pine => (weathered(props::pine(scale), 0.35, 0.8), V_MED, 0.5),
                    PropKind::Bush => (weathered(props::bush(v), 0.5, 0.78), V_SM, 1.4),
                    PropKind::Fern => (weathered(props::fern(v), 0.55, 0.8), V_SM, 2.2),
                    other => panic!("passworld places {other:?}, which it has no mesh for"),
                };
                if matches!(kind, PropKind::Rock | PropKind::BoulderCluster) {
                    blockers.push_placed(kind, x, z, 0.0, quarter, 1.0);
                } else {
                    blockers.push_prop(kind, x, z, 0.0, 1.0);
                }
                pieces.push(Piece { mesh, pos: Vec3::new(x, 0.0, z), quarter, vsize: vs, sway });
            }
            Item::HighPine { x, z, y, scale } => {
                pieces.push(Piece { mesh: weathered(props::pine(scale), 0.4, 0.75), pos: Vec3::new(x, y, z), quarter: 0, vsize: V_MED, sway: 0.4 });
            }
            Item::Crag { at, size, lean, facing, variant, coarse } => {
                let key = (variant * 2 + coarse as i32, (size * 4.0).round() as i32, (lean * 4.0).round() as i32);
                let vox = if coarse { 1.2 } else { 0.6 };
                let mesh = crags.entry(key).or_insert_with(|| props::crag(key.1 as f32 / 4.0, key.0 / 2, key.2 as f32 / 4.0, vox)).clone();
                // the mesh leans toward local +z; turn it to lean out of the
                // wall (a quarter turn sends +z to +x, then -z, then -x)
                let dirs = [Vec3::Z, Vec3::X, Vec3::NEG_Z, Vec3::NEG_X];
                let q = (0..4).max_by(|&a, &b| dirs[a].dot(facing).partial_cmp(&dirs[b].dot(facing)).unwrap()).unwrap_or(0) as u8;
                pieces.push(Piece { mesh, pos: at, quarter: q, vsize: vox, sway: 0.0 });
            }
            Item::Fall { at, height, facing } => {
                // The mesh's sheet faces -z (it is built against z = 1 of a
                // 7-deep grid); pick the quarter turn nearest the facing.
                let a = facing.z.atan2(facing.x);
                let q = ((((-a - std::f32::consts::FRAC_PI_2) / std::f32::consts::FRAC_PI_2).round() as i32).rem_euclid(4)) as u8;
                pieces.push(Piece { mesh: props::waterfall(height, v), pos: at, quarter: q, vsize: V_SM, sway: 0.15 });
            }
        }
    }
    (pieces, blockers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_blocks_a_lane_or_stands_in_the_rock() {
        let items = placements();
        let f = pass::floor();
        let mut props = 0;
        for it in &items {
            if let Item::Prop { kind, x, z, .. } = *it {
                props += 1;
                assert!(f.sdf(x, z) < -0.5, "{kind:?} in the rock at ({x},{z})");
                if matches!(kind, PropKind::Rock | PropKind::BoulderCluster | PropKind::Pine) {
                    assert!(lane_clearance(x, z) > 4.0, "{kind:?} on a lane at ({x},{z})");
                }
            }
        }
        assert!(props > 150, "a bleak pass, not an empty one: {props} props");
    }

    #[test]
    fn it_is_fair_and_every_stream_has_its_fall() {
        let items = placements();
        let props: Vec<_> = items.iter().filter_map(|i| if let Item::Prop { kind, x, z, .. } = *i { Some((kind, x, z)) } else { None }).collect();
        for &(k, x, z) in &props {
            assert!(props.iter().any(|&(k2, x2, z2)| k2 == k && (x2 - x).abs() < 1e-3 && (z2 + z).abs() < 1e-3), "{k:?} at ({x},{z}) has no mirror");
        }
        let falls = items.iter().filter(|i| matches!(i, Item::Fall { .. })).count();
        assert_eq!(falls, pass::streams().len());
        for it in &items {
            if let Item::Fall { at, height, .. } = *it {
                assert!(height > 4.0);
                let d = pass::floor().sdf(at.x, at.z);
                assert!(d > -1.0 && d < 0.5, "a fall stands at the cliff face, not {d} from it");
            }
        }
    }

    /// Crags are scenery rooted in the rock: every one's footing is behind
    /// a cliff face (no lane, choke or clearing narrower for them), and none
    /// near a choke leans out over it.
    #[test]
    fn crags_stand_in_the_rock_and_keep_off_the_chokes() {
        let f = pass::floor();
        let chokes = pass::choke_points();
        let mut n = 0;
        for it in placements() {
            if let Item::Crag { at, lean, .. } = it {
                n += 1;
                assert!(f.sdf(at.x, at.z) >= 0.6, "crag rooted on the floor at {at}");
                if chokes.iter().any(|(p, _)| (p.x - at.x).hypot(p.z - at.z) < 20.0) {
                    assert!(lean <= 0.3 + 1e-3, "a crag leaning {lean} over the choke near {at}");
                }
            }
        }
        assert!(n > 200, "the walls should be broken up: {n} crags");
    }
}
