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
    Crag { at: Vec3, size: f32, lean: f32, facing: Vec3, variant: i32, coarse: bool, spiky: bool },
    /// A glowing gem cluster at the foot of a wall (`props::gem_cluster`).
    Gem { at: Vec3, size: f32, hue: u8, variant: i32 },
    /// A natural stone arch over a corridor; `yaw` is the corridor's
    /// direction. Its legs stand in the rock, so it is scenery.
    Arch { at: Vec3, span: f32, height: f32, yaw: f32, variant: i32 },
    /// An animal god, facing along quarter turn `quarter`.
    Statue { at: Vec3, owl: bool, quarter: u8 },
}

/// Distinct crag meshes built; placements share them.
pub const CRAG_VARIANTS: i32 = 10;
/// Directions a crag can lean, round the compass.
pub const CRAG_YAWS: i32 = 16;

/// The lean-direction bucket for an outward direction (0 = +z, as the mesh
/// builds it, increasing toward +x).
pub fn yaw_bucket(facing: Vec3) -> i32 {
    let a = facing.x.atan2(facing.z).rem_euclid(std::f32::consts::TAU);
    ((a / std::f32::consts::TAU * CRAG_YAWS as f32).round() as i32).rem_euclid(CRAG_YAWS)
}

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

/// Everything placed on Twinfang Pass. Always computed AS the pass: it mixes
/// the pass's own geometry with map-dispatched calls (`Team::base_pos`,
/// `lane_clearance`, tower clearings), and run with the jungle active -- the
/// thread default, as in a test -- it put the base statues at the jungle's
/// bases, inside the pass's rock (`statues_arches_and_gems_...` caught it).
pub fn placements() -> Vec<Item> {
    crate::map::with(crate::map::MapId::Pass, placements_here)
}

fn placements_here() -> Vec<Item> {
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
                // judged again at the crag's footing below: the sample point
                // can be outside a choke's radius while the footing it walks
                // out to is inside it
                let tight_here = near_choke(jx, jz);
                if (0.6..2.4).contains(&d) && free(jx, jz, 8.5) {
                    let facing = out_of(jx, jz);
                    // Walk out to the face's foot, then find the face at the
                    // crag's height by inverting the cliff's rise profile
                    // (h = top * (1 - (1 - d/RISE)^3)): a cliff recedes as it
                    // rises, and a crag rooted at ground level then lifted
                    // hung in front of the rock with its shards dangling.
                    let mut foot = Vec3::new(jx, 0.0, jz);
                    for _ in 0..40 {
                        if f.sdf(foot.x, foot.z) <= 0.3 {
                            break;
                        }
                        foot += facing * 0.1;
                    }
                    let tight = tight_here || near_choke(foot.x, foot.z);
                    let top = pass::cliff_top(foot.x, foot.z).max(2.0);
                    let face_at = |y: f32| {
                        let u = (y / top).clamp(0.0, 0.95);
                        pass::CLIFF_RISE * (1.0 - (1.0 - u).powf(1.0 / 3.0))
                    };
                    // The face's character comes from the ground up: tall
                    // blades rising against the base of the wall, rooted in
                    // the floor-rock junction -- a seam the eye accepts.
                    // Mid-face crags only high on tall faces, seen from
                    // across the canyon: at eye level a blade meeting a
                    // smooth sloped face shows its seam (the choke render,
                    // twice: hanging shards, then buried ones).
                    if rng.f32() < 0.7 || tight {
                        // Ribs on the face line, nearly upright: half in the
                        // rock, rising visibly from the floor. At a choke
                        // they stay behind the line (small, and 0.8 u in).
                        let (sz, ln, back) = if tight { (rng.range(0.55, 0.8), rng.range(0.05, 0.15), 0.8) } else { (rng.range(0.8, 1.35), rng.range(0.05, 0.25), -0.3) };
                        let mut at = foot - facing * back;
                        if tight {
                            // Walk in until the rock is really this deep: a
                            // fixed step along a direction measured elsewhere
                            // left one choke crag proud of a curved funnel
                            // wall (the test caught it).
                            for _ in 0..60 {
                                if f.sdf(at.x, at.z) >= 0.9 {
                                    break;
                                }
                                at -= facing * 0.1;
                            }
                        }
                        let at = at + Vec3::Y * -0.6;
                        out.push(Item::Crag { at, size: sz, lean: ln, facing, variant: v, coarse: false, spiky: false });
                    } else if top > 18.0 && !tight {
                        let y = rng.range(10.0, top * 0.75);
                        let at = foot - facing * (face_at(y) + 0.6) + Vec3::Y * (y - 0.6);
                        out.push(Item::Crag { at, size: rng.range(0.8, 1.2), lean: rng.range(0.3, 0.5), facing, variant: v, coarse: true, spiky: false });
                    }
                } else if d > 5.0 && d < 60.0 && rng.f32() < 0.03 && free(jx, jz, 16.0) {
                    let top = pass::terrain_height(jx, jz);
                    if top < 60.0 {
                        out.push(Item::Crag { at: Vec3::new(jx, top - 0.8, jz), size: rng.range(1.0, 1.6), lean: rng.range(0.0, 0.3), facing: out_of(jx, jz), variant: v, coarse: true, spiky: false });
                    }
                }
                x += 2.5;
            }
            z += 2.5;
        }
    }

    // Gate crags: at every choke, big spiky clusters on both walls either
    // side of the narrows, jutting out over the approach. They frame the
    // choke; slim ribs there only decorated it (compared side by side).
    // Rooted in the rock (sdf >= 0.8) with the lean built in, so the gap
    // itself -- `pass::CHOKE_R` -- is never narrowed at floor level.
    for (ci, (p, dir)) in pass::choke_points().into_iter().enumerate() {
        let side = Vec3::new(-dir.z, 0.0, dir.x);
        for (k, (along, sgn)) in [(-9.0f32, 1.0f32), (-15.0, -1.0), (8.0, -1.0), (14.0, 1.0), (-4.0, -1.0), (4.0, 1.0)].into_iter().enumerate() {
            // from the lane's centre, out to that wall, then into the rock
            let mut q = p + dir * along;
            for _ in 0..200 {
                if f.sdf(q.x, q.z) >= 0.0 {
                    break;
                }
                q += side * sgn * 0.1;
            }
            for _ in 0..40 {
                if f.sdf(q.x, q.z) >= 1.0 {
                    break;
                }
                q += side * sgn * 0.1;
            }
            // a thin wall with another corridor behind it (a slot passes
            // close to a mid choke): no solid rock to root in, no gate crag
            if f.sdf(q.x, q.z) < 0.8 {
                continue;
            }
            let facing = -side * sgn;
            let y = if k >= 4 { 3.5 } else { rng.range(0.5, 2.5) };
            out.push(Item::Crag { at: Vec3::new(q.x, y - 0.6, q.z), size: rng.range(1.05, 1.45), lean: rng.range(0.45, 0.7), facing, variant: (ci as i32 * 7 + k as i32) % CRAG_VARIANTS, coarse: false, spiky: true });
        }
    }

    // Animal gods. Great cats flank each base, facing out toward the lanes;
    // owls stand in the back walls of the boss grounds, facing in.
    for t in [Team::Blue, Team::Red] {
        let b = t.base_pos();
        for sx in [-1.0f32, 1.0] {
            let at = Vec3::new(sx * 21.0, 0.0, b.z - t.push_z() * 10.0);
            out.push(Item::Statue { at, owl: false, quarter: if t == Team::Blue { 0 } else { 2 } });
        }
    }
    for p in pass::pit_positions() {
        let out_dir = Vec3::new(p.x.signum(), 0.0, 0.0);
        let mut at = p;
        while f.sdf(at.x, at.z) < 1.2 {
            at += out_dir * 0.25;
        }
        // facing back in: +z turns to +x on quarter 1, -x on quarter 3
        out.push(Item::Statue { at, owl: true, quarter: if p.x < 0.0 { 1 } else { 3 } });
    }

    // Arches over the flank slots and the shoulder ledges, spanning each
    // square-on; kept only if both legs stand in the rock.
    {
        let pick: [(bool, usize); 4] = [(true, 1), (true, 4), (false, 0), (false, 2)];
        for (is_slot, i) in pick {
            let ((ax, az), (bx, bz)) = if is_slot { pass::SLOTS[i] } else { pass::LEDGES[i] };
            for zs in [1.0f32, -1.0] {
                let a = Vec3::new(ax * pass::S, 0.0, az * zs * pass::S);
                let b = Vec3::new(bx * pass::S, 0.0, bz * zs * pass::S);
                let mid = (a + b) * 0.5;
                let dir = (b - a).normalize_or_zero();
                let across = Vec3::new(-dir.z, 0.0, dir.x);
                let half = if is_slot { pass::SLOT_R } else { pass::LEDGE_R };
                let span = half * 2.0 + 2.2;
                let legs = [mid + across * (span * 0.5 + 1.3), mid - across * (span * 0.5 + 1.3)];
                if legs.iter().all(|l| f.sdf(l.x, l.z) >= 0.0) {
                    out.push(Item::Arch { at: mid, span, height: if is_slot { 10.5 } else { 8.5 }, yaw: dir.x.atan2(dir.z), variant: i as i32 * 2 + (zs > 0.0) as i32 });
                }
            }
        }
    }

    // Gems along the side paths: every ~10 u on a ledge or slot, at the
    // foot of alternating walls, half in the rock. Blue half, mirrored.
    {
        let mut k = 0;
        let paths: Vec<((f32, f32), (f32, f32), f32)> = pass::LEDGES.iter().map(|&(a, b)| (a, b, pass::LEDGE_R)).chain(pass::SLOTS.iter().map(|&(a, b)| (a, b, pass::SLOT_R))).collect();
        for ((ax, az), (bx, bz), _half) in paths {
            let a = Vec3::new(ax * pass::S, 0.0, az * pass::S);
            let b = Vec3::new(bx * pass::S, 0.0, bz * pass::S);
            let len = (b - a).length();
            let dir = (b - a).normalize_or_zero();
            let across = Vec3::new(-dir.z, 0.0, dir.x);
            let mut s = 6.0;
            while s < len - 6.0 {
                let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                let mut q = a + dir * s;
                for _ in 0..120 {
                    if f.sdf(q.x, q.z) >= 0.3 {
                        break;
                    }
                    q += across * side * 0.1;
                }
                if (0.2..1.0).contains(&f.sdf(q.x, q.z)) && q.z < -0.5 && !crate::bosses::in_pit(q.x, q.z) {
                    let (size, hue, variant) = (rng.range(0.7, 1.05), (k % 3 == 2) as u8, k);
                    out.push(Item::Gem { at: q, size, hue, variant });
                    out.push(Item::Gem { at: Vec3::new(q.x, 0.0, -q.z), size, hue, variant });
                }
                k += 1;
                s += 10.0;
            }
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

fn lifted(it: Item) -> Item {
    let up = |p: Vec3| Vec3::new(p.x, p.y + pass::floor_height(p.x, p.z), p.z);
    match it {
        Item::Prop { .. } | Item::HighPine { .. } | Item::Gem { .. } => it,
        Item::Crag { at, .. } if pass::floor().sdf(at.x, at.z) > 4.0 => it, // a plateau spire
        Item::Crag { at, size, lean, facing, variant, coarse, spiky } => Item::Crag { at: up(at), size, lean, facing, variant, coarse, spiky },
        Item::Fall { at, height, facing } => Item::Fall { at: up(at), height, facing },
        Item::Arch { at, span, height, yaw, variant } => Item::Arch { at: up(at), span, height, yaw, variant },
        Item::Statue { at, owl, quarter } => Item::Statue { at: up(at), owl, quarter },
    }
}

/// Meshes and collision for everything in [`placements`].
pub fn build() -> (Vec<Piece>, Blockers) {
    let mut blockers = Blockers::new(consts::arena_radius() + 20.0, 6.0);
    blockers.set_floor(pass::floor());
    // Under a cave roof you are harder to see: cover-only entries (no
    // collision, `top` = -inf) every 4 u along each cave.
    for cave in pass::caves() {
        let len = (cave.b - cave.a).length();
        let n = (len / 4.0).ceil() as usize;
        for k in 0..=n {
            let p = cave.a.lerp(cave.b, k as f32 / n as f32);
            blockers.push(crate::blockers::Blocker {
                shape: crate::blockers::Shape::Circle { x: p.x, z: p.z, r: 0.0 },
                top: f32::NEG_INFINITY,
                cover: crate::blockers::Cover { x: p.x, z: p.z, radius: cave.half + 0.5, amount: 0.55 },
            });
        }
    }
    let mut pieces = Vec::new();
    // crag meshes by (variant, size step, lean step): a few dozen grids
    // for hundreds of placements
    let mut crags: std::collections::HashMap<(i32, i32, i32), crate::voxel::VoxelMesh> = Default::default();
    for (i, it) in placements().into_iter().enumerate() {
        let v = i as i32;
        // Lift what stands on the floor onto the floor's height there: the
        // Top lane's shelf (`pass::floor_height`). Gems, plateau pines and
        // plateau spires already stand on the terrain's height.
        let it = lifted(it);
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
                let y = pass::floor_height(x, z);
                if matches!(kind, PropKind::Rock | PropKind::BoulderCluster) {
                    blockers.push_placed(kind, x, z, y, quarter, 1.0);
                } else {
                    blockers.push_prop(kind, x, z, y, 1.0);
                }
                pieces.push(Piece { mesh, pos: Vec3::new(x, y, z), quarter, vsize: vs, sway });
            }
            Item::HighPine { x, z, y, scale } => {
                pieces.push(Piece { mesh: weathered(props::pine(scale), 0.4, 0.75), pos: Vec3::new(x, y, z), quarter: 0, vsize: V_MED, sway: 0.4 });
            }
            Item::Crag { at, size, lean, facing, variant, coarse, spiky } => {
                // a rib standing proud of the face has a base cats must
                // not walk through
                if !coarse && !spiky && at.y < 0.0 && pass::floor().sdf(at.x, at.z) < 0.5 {
                    let b = at + facing * (0.5 * size);
                    blockers.push(crate::blockers::Blocker { shape: crate::blockers::Shape::Circle { x: b.x, z: b.z, r: 0.8 * size }, top: 4.0, cover: crate::blockers::Cover::NONE });
                }
                // The lean is built into the mesh at one of CRAG_YAWS
                // directions (22.5 deg): quarter turns were up to 45 deg off
                // a curved wall's real normal, and the blades read as pasted
                // on sideways.
                let yb = yaw_bucket(facing);
                let key = (variant * 4 + coarse as i32 + 2 * spiky as i32, (size * 4.0).round() as i32, (lean * 4.0).round() as i32 * 64 + yb);
                let vox = if coarse { 1.2 } else { 0.6 };
                let yaw = yb as f32 / CRAG_YAWS as f32 * std::f32::consts::TAU;
                let mesh = crags.entry(key).or_insert_with(|| props::crag(key.1 as f32 / 4.0, key.0 / 4, (key.2 / 64) as f32 / 4.0, vox, yaw, spiky)).clone();
                pieces.push(Piece { mesh, pos: at, quarter: 0, vsize: vox, sway: 0.0 });
            }
            Item::Gem { at, size, hue, variant } => {
                // seated on the ground actually there (a stream bed dips
                // half a unit: a fixed height left one floating)
                let y = pass::terrain_height(at.x, at.z).min(0.3) - 0.35;
                pieces.push(Piece { mesh: props::gem_cluster(size, variant, hue), pos: Vec3::new(at.x, y, at.z), quarter: (variant % 4) as u8, vsize: V_SM, sway: 0.0 });
            }
            Item::Arch { at, span, height, yaw, variant } => {
                pieces.push(Piece { mesh: props::stone_arch(span, height, yaw, variant), pos: at, quarter: 0, vsize: V_MED, sway: 0.0 });
            }
            Item::Statue { at, owl, quarter } => {
                let mesh = if owl { props::statue_owl(1.0, 3) } else { props::statue_cat(1.0, 1) };
                // a statue on the floor is solid; one set into a wall is not
                if pass::floor().sdf(at.x, at.z) < 0.0 {
                    blockers.push(crate::blockers::Blocker { shape: crate::blockers::Shape::Circle { x: at.x, z: at.z, r: 4.2 }, top: 14.0, cover: crate::blockers::Cover::NONE });
                }
                pieces.push(Piece { mesh, pos: at, quarter, vsize: V_MED, sway: 0.0 });
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

    /// Run the test as the pass: its checks (lane_clearance, bases) are
    /// map-dispatched, like the placement they check.
    struct OnPass(crate::map::MapId);
    impl Drop for OnPass {
        fn drop(&mut self) {
            crate::map::set_active(self.0);
        }
    }
    fn on_pass() -> OnPass {
        let prev = crate::map::active();
        crate::map::set_active(crate::map::MapId::Pass);
        OnPass(prev)
    }

    #[test]
    fn nothing_blocks_a_lane_or_stands_in_the_rock() {
        let _pass = on_pass();
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
        let _pass = on_pass();
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

    /// Crags are rooted at the wall: every footing on the face line or
    /// behind it, and none near a choke leans out over it or stands proud
    /// of its wall (a choke's width is set by `pass::CHOKE_R`, not scenery).
    #[test]
    fn crags_stand_in_the_rock_and_keep_off_the_chokes() {
        let _pass = on_pass();
        let f = pass::floor();
        let chokes = pass::choke_points();
        let (mut n, mut gates) = (0, 0);
        for it in placements() {
            if let Item::Crag { at, lean, spiky: true, .. } = it {
                // gate crags: rooted in the rock, never on the floor
                assert!(f.sdf(at.x, at.z) >= 0.8, "gate crag not rooted in the rock at {at}");
                gates += 1;
                continue;
            }
            if let Item::Crag { at, lean, .. } = it {
                n += 1;
                // rooted at the wall: on the face line or behind it
                assert!(f.sdf(at.x, at.z) >= -0.5, "crag standing out on the floor at {at} (sdf {})", f.sdf(at.x, at.z));
                if chokes.iter().any(|(p, _)| (p.x - at.x).hypot(p.z - at.z) < 20.0) {
                    assert!(lean <= 0.3 + 1e-3, "a crag leaning {lean} over the choke near {at}");
                    assert!(f.sdf(at.x, at.z) >= 0.5, "a crag standing proud at a choke, near {at}");
                }
            }
        }
        assert!(n > 200, "the walls should be broken up: {n} crags");
        assert!(gates >= 4 * pass::CHOKES.len(), "gate crags at every choke: {gates}");
    }

    /// Statues on the floor are clear of every lane and solid; arches stand
    /// with both legs in the rock; gems sit at the foot of a wall; all of it
    /// mirrored like the rest of the map.
    #[test]
    fn statues_arches_and_gems_stand_where_they_should() {
        let _pass = on_pass();
        let f = pass::floor();
        let items = placements();
        let (mut statues, mut arches, mut gems) = (0, 0, 0);
        for it in &items {
            match *it {
                Item::Statue { at, owl, .. } => {
                    statues += 1;
                    if f.sdf(at.x, at.z) < 0.0 {
                        assert!(f.fits(at.x, at.z, 4.2), "statue half off the floor at {at}");
                        assert!(lane_clearance(at.x, at.z) > 4.2 + 3.0, "statue on a lane at {at}");
                    } else {
                        assert!(owl, "only the owls are set into walls");
                    }
                }
                Item::Arch { at, span, yaw, .. } => {
                    arches += 1;
                    let across = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
                    for l in [at + across * (span * 0.5 + 1.3), at - across * (span * 0.5 + 1.3)] {
                        assert!(f.sdf(l.x, l.z) >= 0.0, "arch leg on the floor at {l}");
                    }
                    assert!(f.sdf(at.x, at.z) < 0.0, "an arch spans a corridor, its middle is floor");
                }
                Item::Gem { at, .. } => {
                    gems += 1;
                    assert!((0.2..1.0).contains(&f.sdf(at.x, at.z)), "gem not at a wall's foot: {at}");
                }
                _ => {}
            }
        }
        assert_eq!(statues, 6, "four cats at the bases, two owls at the boss grounds");
        assert!(arches >= 4, "{arches} arches");
        assert!(gems >= 20 && gems % 2 == 0, "{gems} gems (mirrored pairs)");
    }

    /// Caves lie over floor, their roof's lowest point (the mouth lip and
    /// stalactites: height - 1.4 - 0.9 - 1.4) still clears a standing cat,
    /// inside a cave you are in cover and outside it you are not, and the
    /// roof is cut away exactly while the player is under it.
    #[test]
    fn caves_cover_the_path_hide_who_is_in_them_and_cut_away_for_the_player() {
        let _pass = on_pass();
        let f = pass::floor();
        let (_, blockers) = build();
        let caves = pass::caves();
        assert_eq!(caves.len(), 4);
        for c in &caves {
            for k in 0..=10 {
                let p = c.a.lerp(c.b, k as f32 / 10.0);
                assert!(f.fits(p.x, p.z, 1.0), "cave over rock at {p}");
            }
            assert!(c.height - 1.4 - 0.9 - 1.4 > 2.0, "a cat does not fit under the lowest point");
            let m = c.mid();
            assert!(blockers.concealment_at(m.x, m.z) >= 0.5, "no cover in the cave at {m}");
            assert!(crate::scene::cave_drawn(c, Vec3::new(0.0, 0.0, 0.0)), "drawn for a player elsewhere");
            assert!(!crate::scene::cave_drawn(c, m), "cut away for the player under it");
        }
        // cover is the cave's, not the whole map's
        let mid = pass::along(&crate::consts::Lane::Mid.polyline(), 20.0);
        assert!(blockers.concealment_at(mid.x, mid.z) < 0.1);
    }
}
