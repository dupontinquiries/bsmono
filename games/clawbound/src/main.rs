//! clawbound-rs — native papercraft renderer (Rust + wgpu), headless PNG output.

use clawbound_rs::{cat, ecology, hunter, mapeval, objectives, world};
use clawbound_rs::math::{look_at, perspective, v3, Vec3};
use clawbound_rs::mesh::{rgb, shade, Mesh};
use wgpu::util::DeviceExt;

const BASE_W: u32 = 1100;
const BASE_H: u32 = 680;

/// AAA / high-res mode toggle (env CLAW_HIRES): 2x supersampling + denser geometry.
fn hires() -> bool { std::env::var("CLAW_HIRES").is_ok() }
/// Scale a geometry/particle count by the quality setting.
fn q(n: usize) -> usize { if hires() { (n as f32 * 1.8) as usize } else { n } }

// ── papercraft actors ──────────────────────────────────────────────────────
fn build_shadow(m: &mut Mesh, at: Vec3) {
    let dark = rgb(0x241830);
    let dark2 = rgb(0x160f22);
    let y = at.y;
    // a taller, hunched spectral cat
    m.boxx(v3(at.x, y + 0.65, at.z - 0.4), v3(0.36, 0.34, 0.5), dark2);
    m.boxx(v3(at.x, y + 0.72, at.z + 0.32), v3(0.34, 0.34, 0.46), dark);
    m.boxx(v3(at.x, y + 1.0, at.z + 0.82), v3(0.28, 0.26, 0.27), dark);
    // ears + glowing eyes
    m.cone(v3(at.x - 0.16, y + 1.28, at.z + 0.82), 0.11, 0.3, 4, dark2);
    m.cone(v3(at.x + 0.16, y + 1.28, at.z + 0.82), 0.11, 0.3, 4, dark2);
    m.boxx(v3(at.x - 0.09, y + 1.02, at.z + 1.02), v3(0.04, 0.03, 0.02), rgb(0xff3838));
    m.boxx(v3(at.x + 0.09, y + 1.02, at.z + 1.02), v3(0.04, 0.03, 0.02), rgb(0xff3838));
    // a wisp of otherworld rising off it (translucent)
    m.set_op(0.4);
    m.cone(v3(at.x, y + 1.4, at.z), 0.5, 1.6, 6, rgb(0x6a3d8a));
    m.set_op(1.0);
    // long shadow tail
    m.segment(v3(at.x, y + 0.7, at.z - 0.82), v3(at.x + 0.3, y + 0.3, at.z - 1.5), 0.09, 0.03, 5, dark2);
}

fn build_fox(m: &mut Mesh, at: Vec3) {
    let fur = rgb(0xb5642f);
    let dark = rgb(0x2a2320);
    let y = at.y;
    m.boxx(v3(at.x, y + 0.75, at.z), v3(0.42, 0.36, 1.0), fur);
    m.boxx(v3(at.x, y + 0.9, at.z + 0.95), v3(0.32, 0.30, 0.32), fur);
    m.boxx(v3(at.x, y + 0.78, at.z + 1.3), v3(0.16, 0.15, 0.2), fur); // snout
    m.cone(v3(at.x - 0.22, y + 1.15, at.z + 0.85), 0.13, 0.32, 4, dark);
    m.cone(v3(at.x + 0.22, y + 1.15, at.z + 0.85), 0.13, 0.32, 4, dark);
    for &(lx, lz) in &[(-0.3, 0.6), (0.3, 0.6), (-0.3, -0.6), (0.3, -0.6)] {
        m.boxx(v3(at.x + lx, y + 0.28, at.z + lz), v3(0.11, 0.3, 0.11), dark);
    }
    m.boxx(v3(at.x, y + 0.75, at.z - 1.1), v3(0.16, 0.16, 0.5), shade(fur, 1.1));
}

/// Draw one ambient animal as a little papercraft creature, posed by its FSM state.
fn build_critter(m: &mut Mesh, c: &ecology::Critter) {
    use ecology::{CritterState, Species};
    let sz = c.species.size();
    let col = rgb(c.species.color());
    let dark = shade(col, 0.78);
    let gy = world::height_at(c.x, c.z);
    let fleeing = c.state == CritterState::Flee;
    // bob: brisk when fleeing, gentle idle otherwise
    let bob = if fleeing { c.phase.sin().abs() * 0.14 * sz } else { c.phase.sin() * 0.03 * sz };
    let by = gy + sz * 0.42 + bob;
    let (fx, fz) = (c.heading.cos(), c.heading.sin());
    let cx = c.x; let cz = c.z;
    // body
    m.boxx(v3(cx, by, cz), v3(sz * 0.32, sz * 0.30, sz * 0.46), col);
    // head, forward
    let hx = cx + fx * sz * 0.5; let hz = cz + fz * sz * 0.5; let hy = by + sz * 0.16;
    m.boxx(v3(hx, hy, hz), v3(sz * 0.2, sz * 0.2, sz * 0.2), col);

    match c.species {
        Species::Hare => {
            // tall ears + a bright scut
            m.boxx(v3(hx - fz * sz * 0.08, hy + sz * 0.34, hz + fx * sz * 0.02), v3(sz * 0.05, sz * 0.3, sz * 0.05), col);
            m.boxx(v3(hx + fz * sz * 0.08, hy + sz * 0.34, hz - fx * sz * 0.02), v3(sz * 0.05, sz * 0.3, sz * 0.05), col);
            m.boxx(v3(cx - fx * sz * 0.5, by, cz - fz * sz * 0.5), v3(sz * 0.12, sz * 0.12, sz * 0.12), rgb(0xf0ece0));
        }
        Species::Deer => {
            // slim legs + branching antlers
            for &s in &[-1.0f32, 1.0] {
                let ox = -fz * s * sz * 0.22; let oz = fx * s * sz * 0.22;
                m.boxx(v3(cx + fx * sz * 0.3 + ox, gy + sz * 0.24, cz + fz * sz * 0.3 + oz), v3(sz * 0.06, sz * 0.26, sz * 0.06), dark);
                m.boxx(v3(cx - fx * sz * 0.3 + ox, gy + sz * 0.24, cz - fz * sz * 0.3 + oz), v3(sz * 0.06, sz * 0.26, sz * 0.06), dark);
            }
            m.boxx(v3(hx, hy + sz * 0.34, hz), v3(sz * 0.04, sz * 0.26, sz * 0.04), rgb(0x6a5030));
            for &s in &[-1.0f32, 1.0] {
                m.boxx(v3(hx + (-fz * s) * sz * 0.14, hy + sz * 0.5, hz + (fx * s) * sz * 0.14), v3(sz * 0.03, sz * 0.14, sz * 0.03), rgb(0x6a5030));
            }
        }
        Species::Squirrel => {
            // big curled tail arcing over the back
            m.boxx(v3(cx - fx * sz * 0.5, by + sz * 0.3, cz - fz * sz * 0.5), v3(sz * 0.14, sz * 0.36, sz * 0.14), shade(col, 1.1));
        }
        Species::Heron => {
            // long legs, long neck, dagger beak
            m.boxx(v3(cx, gy + sz * 0.4, cz), v3(sz * 0.04, sz * 0.4, sz * 0.04), rgb(0x9a8a3a));
            m.boxx(v3(hx, hy + sz * 0.3, hz), v3(sz * 0.06, sz * 0.34, sz * 0.06), col);
            m.boxx(v3(hx + fx * sz * 0.28, hy + sz * 0.5, hz + fz * sz * 0.28), v3(sz * 0.16, sz * 0.03, sz * 0.03), rgb(0xd8b23a));
        }
        Species::Owl => {
            // upright, big flat face, ear tufts
            m.boxx(v3(hx, hy + sz * 0.08, hz), v3(sz * 0.24, sz * 0.22, sz * 0.14), col);
            for &s in &[-1.0f32, 1.0] {
                m.boxx(v3(hx + (-fz * s) * sz * 0.16, hy + sz * 0.32, hz + (fx * s) * sz * 0.16), v3(sz * 0.05, sz * 0.1, sz * 0.05), col);
                m.boxx(v3(hx + (-fz * s) * sz * 0.1 + fx * sz * 0.16, hy + sz * 0.12, hz + (fx * s) * sz * 0.1 + fz * sz * 0.16), v3(sz * 0.05, sz * 0.05, sz * 0.02), rgb(0xf2c53a));
            }
        }
        Species::Duck | Species::Grouse => {
            // plump body, little beak; wings flare when flushed
            m.boxx(v3(hx + fx * sz * 0.2, hy, hz + fz * sz * 0.2), v3(sz * 0.1, sz * 0.06, sz * 0.06), rgb(0xd8a83a));
            let spread = if fleeing { sz * 0.34 } else { sz * 0.12 };
            for &s in &[-1.0f32, 1.0] {
                m.boxx(v3(cx + (-fz * s) * spread, by, cz + (fx * s) * spread), v3(sz * 0.18, sz * 0.05, sz * 0.28), dark);
            }
        }
        Species::Frog => {
            // squat, bulging eyes
            for &s in &[-1.0f32, 1.0] {
                m.boxx(v3(hx + (-fz * s) * sz * 0.12, hy + sz * 0.12, hz + (fx * s) * sz * 0.12), v3(sz * 0.06, sz * 0.06, sz * 0.06), rgb(0xf2d24a));
            }
        }
    }
}

/// Build a whole population of ambient wildlife.
fn build_critters(critters: &[ecology::Critter]) -> Mesh {
    let mut m = Mesh::new();
    for c in critters { build_critter(&mut m, c); }
    m
}

fn build_scene() -> (Mesh, Vec3) {
    world::init_world();                         // bake eroded heightfield + river
    let mut m = world::build_terrain(150);
    m.extend(&world::build_water());
    m.extend(&world::build_trees(320));
    m.extend(&world::build_bushes(200));
    m.extend(&world::build_grass(4200));
    m.extend(&world::build_rocks(220));
    m.extend(&world::build_reeds(700));

    m.extend(&build_critters(&ecology::populate(320, false)));  // ambient wildlife

    // a hero broadleaf tree to climb, with individual-leaf detail
    let (hx, hz) = (5.0, 9.0);
    let gy = world::height_at(hx, hz);
    let leaf = rgb(world::biome_at(hx, hz).leaf);
    let branches = world::gen_tree(&mut m, hx, hz, 1, leaf, 1.3, 5.0, true);

    // perch the cat partway out the best climbable limb (within its safe reach)
    let mut best = branches[0];
    let mut best_score = -1.0;
    for b in &branches {
        let score = b.len() * b.safe_reach(5.0);
        if score > best_score { best_score = score; best = *b; }
    }
    let along = best.safe_reach(5.0).min(0.7).max(0.3);
    let perch = best.point(along);
    cat::build(&mut m, v3(perch.x, perch.y - 0.12, perch.z), 1.0, &cat::climb(), clawbound_rs::class::Class::Cat);

    // a fox prowling the ground below
    let fy = world::height_at(13.0, 20.0);
    build_fox(&mut m, v3(13.0, fy, 20.0));

    (m, v3(hx, gy + 4.2, hz))
}

/// A pose/gait sheet — the rig posed across its states on flat ground.
fn build_poses() -> (Mesh, Vec3) {
    let mut m = Mesh::new();
    let g = rgb(0x4f8036);
    let s = 22.0;
    let a = v3(-s, 0.0, -s); let b = v3(s, 0.0, -s); let c = v3(s, 0.0, s); let d = v3(-s, 0.0, s);
    m.quad(a, d, c, b, g); // up-facing ground

    let poses: [cat::Pose; 8] = [
        cat::stand(), cat::walk(0.0), cat::walk(0.35), cat::bound(0.2),
        cat::bound(0.7), cat::coil(), cat::leap(), cat::startle(),
    ];
    let mut x = -8.4;
    for p in &poses { cat::build(&mut m, v3(x, 0.0, 0.0), 1.0, p, clawbound_rs::class::Class::Cat); x += 2.4; }
    // climb pose on a branch stub
    let bx = x + 0.4;
    m.segment(v3(bx, 0.0, 0.0), v3(bx, 2.6, 0.7), 0.16, 0.11, 6, rgb(0x6b4b32));
    cat::build(&mut m, v3(bx - 0.05, 1.15, 0.28), 1.0, &cat::climb(), clawbound_rs::class::Class::Cat);

    (m, v3((x - 8.4) * 0.5 + 0.5, 0.7, 0.2))
}

/// A motion strip — the cat sampled along a path with the gait cycling live,
/// then climbing a limb up to its safe reach (stroboscopic, one frame).
fn build_sequence() -> (Mesh, Vec3) {
    let mut m = Mesh::new();
    let g = rgb(0x4f8036);
    let s = 26.0;
    let a = v3(-s, 0.0, -s); let b = v3(s, 0.0, -s); let c = v3(s, 0.0, s); let d = v3(-s, 0.0, s);
    m.quad(a, d, c, b, g); // up-facing ground

    // a tree at the end of the run to climb
    let ztree = 11.0;
    let leaf = rgb(world::biome_at(0.0, ztree).leaf);
    let branches = world::gen_tree(&mut m, 0.0, ztree, 1, leaf, 1.3, 5.0, true);
    let mut best = branches[0];
    let mut sc = -1.0;
    for br in &branches { let s2 = br.len() * br.safe_reach(5.0); if s2 > sc { sc = s2; best = *br; } }

    // walk → break into a bound as it approaches the trunk
    let n = 6;
    for i in 0..n {
        let t = i as f32 / (n as f32);
        let z = -12.0 + t * ((ztree - 3.5) - (-12.0));
        let ph = (t * 3.2).fract();
        let pose = if t < 0.62 { cat::walk(ph) } else { cat::bound(ph) };
        cat::build(&mut m, v3(0.0, 0.0, z), 1.0, &pose, clawbound_rs::class::Class::Cat);
    }
    // climb the limb in steps, stopping at safe reach (further out would snap)
    let amax = best.safe_reach(5.0).min(0.85);
    for i in 0..3 {
        let along = 0.18 + (i as f32 / 2.0) * (amax - 0.18);
        let p = best.point(along);
        cat::build(&mut m, v3(p.x, p.y - 0.1, p.z), 1.0, &cat::climb(), clawbound_rs::class::Class::Cat);
    }

    (m, v3(0.0, 1.0, 1.5))
}

/// Flat cascading cone (+ omni disc) drawn on the ground for the sensory chart.
fn add_cone(m: &mut Mesh, hx: f32, hz: f32, heading: f32, p: &hunter::Params, col: [f32; 3], op: f32) {
    m.set_op(op);
    let y = 0.06;
    let (fx, fz) = (heading.sin(), heading.cos());
    let (rx, rz) = (heading.cos(), -heading.sin());
    let apex = v3(hx + fx * p.range, y, hz + fz * p.range);
    let n = 12;
    for i in 0..n {
        let f0 = -p.near_half + 2.0 * p.near_half * i as f32 / n as f32;
        let f1 = -p.near_half + 2.0 * p.near_half * (i + 1) as f32 / n as f32;
        let b0 = v3(hx + rx * f0, y, hz + rz * f0);
        let b1 = v3(hx + rx * f1, y, hz + rz * f1);
        m.tri(apex, b1, b0, col); // up-facing
    }
    if p.omni > 0.0 {
        let c = v3(hx, y, hz);
        let seg = 20;
        for i in 0..seg {
            let a0 = i as f32 / seg as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / seg as f32 * std::f32::consts::TAU;
            let r0 = v3(hx + a0.cos() * p.omni, y, hz + a0.sin() * p.omni);
            let r1 = v3(hx + a1.cos() * p.omni, y, hz + a1.sin() * p.omni);
            m.tri(c, r1, r0, shade(col, 0.85));
        }
    }
    m.set_op(1.0);
}

fn build_senses() -> (Mesh, Vec3) {
    let mut m = Mesh::new();
    // ground patch
    let g = rgb(0x4f8036);
    let (x0, x1, z0, z1) = (-42.0, 42.0, -6.0, 64.0);
    m.quad(v3(x0, 0.0, z0), v3(x0, 0.0, z1), v3(x1, 0.0, z1), v3(x1, 0.0, z0), g);

    let kinds = [hunter::Kind::Fox, hunter::Kind::Dog, hunter::Kind::Badger, hunter::Kind::Hawk, hunter::Kind::Snake];
    let xs = [-32.0, -16.0, 0.0, 16.0, 32.0];
    // one cat stalks in front of the fox (triggers its FSM to Chase → red cone)
    let (catx, catz) = (-32.0, 11.0);

    for (i, &k) in kinds.iter().enumerate() {
        let p = k.params();
        let (hx, hz) = (xs[i], 0.0);
        let seen = hunter::detects(k, hx, hz, 0.0, catx, catz, true, false);
        let state = if seen { hunter::HState::Chase } else { hunter::HState::Patrol };
        let col = rgb(hunter::state_color(state));
        add_cone(&mut m, hx, hz, 0.0, &p, col, 0.5);
        // hunter marker (a small block + a facing nub)
        let by = world_flat(hx, hz);
        m.set_op(1.0);
        m.boxx(v3(hx, by + 0.4, hz), v3(0.5, 0.4, 0.7), rgb(0x2a2320));
        m.boxx(v3(hx, by + 0.4, hz + 0.7), v3(0.2, 0.2, 0.3), rgb(0x2a2320));
    }

    // the cat, its scent trail drifting downwind, and a wind arrow
    cat::build(&mut m, v3(catx, 0.0, catz), 1.0, &cat::stand(), clawbound_rs::class::Class::Cat);
    // scent dots trailing back + drifted -x (downwind)
    for i in 0..7 {
        let t = i as f32;
        let sx = catx - t * 0.6 - 1.2;
        let sz = catz - t * 1.4;
        m.boxx(v3(sx, 0.15, sz), v3(0.18, 0.05, 0.18), rgb(0x9a6fd0));
    }
    // wind arrow (blowing toward -x)
    let wy = 0.1;
    m.segment(v3(24.0, wy, 54.0), v3(12.0, wy, 54.0), 0.2, 0.2, 5, rgb(0x6fa8d0));
    m.cone(v3(11.0, wy, 54.0), 1.0, 0.1, 4, rgb(0x6fa8d0)); // tiny flat head marker

    (m, v3(0.0, 0.0, 26.0))
}

fn world_flat(_x: f32, _z: f32) -> f32 { 0.0 }

/// The world with fairness-placed objective beacons (Reach/Rendezvous/Forage/Survive).
fn build_goals() -> (Mesh, Vec3) {
    world::init_world();
    let mut m = world::build_terrain(150);
    m.extend(&world::build_water());
    m.extend(&world::build_trees(320));
    m.extend(&world::build_bushes(200));
    m.extend(&world::build_grass(3000));
    m.extend(&world::build_reeds(600));
    m.extend(&world::build_rocks(200));
    let tau = std::f32::consts::TAU;
    for o in objectives::place() {
        let gy = world::height_at(o.x, o.z);
        let col = rgb(objectives::color(o.kind));
        m.set_op(0.9);
        m.segment(v3(o.x, gy, o.z), v3(o.x, gy + 17.0, o.z), 0.85, 0.25, 7, col);
        m.set_op(1.0);
        let c = v3(o.x, gy + 0.12, o.z);
        for i in 0..20 {
            let a0 = i as f32 / 20.0 * tau; let a1 = (i + 1) as f32 / 20.0 * tau;
            m.tri(c, v3(o.x + a1.cos() * 2.8, gy + 0.12, o.z + a1.sin() * 2.8),
                     v3(o.x + a0.cos() * 2.8, gy + 0.12, o.z + a0.sin() * 2.8), col);
        }
    }
    (m, v3(0.0, 0.0, 10.0))
}

/// A ground-level showcase of the ambient wildlife, framed low so the animals read.
fn build_wildlife() -> (Mesh, Vec3) {
    world::init_world();
    let mut m = world::build_terrain(150);
    m.extend(&world::build_water());
    m.extend(&world::build_trees(220));
    m.extend(&world::build_bushes(200));
    m.extend(&world::build_grass(3200));
    m.extend(&world::build_reeds(650));
    m.extend(&world::build_rocks(200));
    m.extend(&build_critters(&ecology::populate(360, false)));

    // a curated near-camera line-up so every species is legible in one frame
    let focus = v3(0.0, 0.0, 22.0);
    use ecology::Species::*;
    let cast = [Deer, Hare, Heron, Owl, Duck, Squirrel, Grouse, Frog];
    let mut demo = Vec::new();
    for (i, sp) in cast.iter().enumerate() {
        let x = -10.5 + i as f32 * 3.0;
        let z = focus.z + 2.0 + (i % 2) as f32 * 1.4;
        let mut c = ecology::Critter::new(*sp, x, z, 100 + i as u32);
        c.heading = -1.2;                       // face roughly toward the camera
        if i % 3 == 0 { c.state = ecology::CritterState::Flee; c.phase = 1.2; }  // some mid-bound
        demo.push(c);
    }
    m.extend(&build_critters(&demo));
    (m, focus)
}

/// Play out a 6-vs-1 match for a few seconds and render the moment.
/// A row of floating attribute bars above a point, one per clan trait, coloured by
/// trait (climb=green, sprint=gold, swim=blue, stealth=violet), length ∝ value.
fn trait_bars(m: &mut Mesh, x: f32, base_y: f32, z: f32, t: &clawbound_rs::clan::ClanTrait) {
    let rows = [(t.climb, 0x5aa054u32), (t.sprint, 0xd8a53c), (t.swim, 0x3d7fd8), (t.stealth, 0x9a6cff), (t.scent, 0xe08040)];
    for (i, (v, col)) in rows.iter().enumerate() {
        let by = base_y - i as f32 * 0.62;
        m.set_op(0.6); m.boxx(v3(x, by, z), v3(1.25, 0.24, 0.05), rgb(0x1a1a1a)); m.set_op(1.0); // track
        let f = ((v - 0.6) / 1.05).clamp(0.06, 1.0);          // map ~0.6..1.65 → 0..1
        m.boxx(v3(x - 1.25 + 1.25 * f, by, z - 0.03), v3(1.25 * f, 0.2, 0.07), rgb(*col));
    }
}

/// Showcase: the four clans lined up, each under a floating profile of trait bars.
fn build_clans() -> (Mesh, Vec3) {
    use clawbound_rs::clan;
    let mut m = world::build_terrain(120);
    m.extend(&world::build_grass(1600));
    m.extend(&world::build_trees(40));
    for c in 0..clan::CLAN_COUNT as u8 {
        let x = (c as f32 - 2.5) * 5.6;
        let z = 0.0;
        let gy = world::height_at(x, z);
        let t = clan::trait_of(c);
        cat::build(&mut m, v3(x, gy, z), 0.0, &cat::idle(0.2 + c as f32 * 0.3), clawbound_rs::class::Class::Cat);
        m.boxx(v3(x, gy + 2.15, z), v3(0.55, 0.2, 0.55), rgb(t.color));   // clan banner
        trait_bars(&mut m, x, gy + 6.4, z, &t);
        eprintln!("{} ({}) — {}", t.name, clan::specialty(c), t.home);
    }
    (m, v3(0.0, 3.2, 0.0))
}

fn build_game() -> (Mesh, Vec3) {
    use clawbound_rs::sim::{Game, Mode, Outcome};
    let kind = match std::env::args().nth(4).as_deref() {
        Some("dog") => hunter::Kind::Dog, Some("badger") => hunter::Kind::Badger,
        Some("hawk") => hunter::Kind::Hawk, Some("snake") => hunter::Kind::Snake, Some("shadow") => hunter::Kind::Shadow,
        _ => hunter::Kind::Fox,
    };
    let mode = match std::env::args().nth(5).as_deref() { Some("survive") => Mode::Survive, _ => Mode::Rendezvous };
    let mut g = Game::new_match(kind, mode);
    for _ in 0..300 { if g.outcome.is_some() { break; } g.step(1.0 / 30.0); }
    if let Some(sky) = sky_override() { g.weather.sky = sky; }   // CLAW_SKY=rain|snow|storm|fog for headless previews
    eprintln!("weather: {} ({}), wind {:.2}", g.weather.sky.name(), g.weather.season.name(), g.weather.total_wind());
    let status = match g.outcome { Some(Outcome::CatsWin) => "CATS WIN", Some(Outcome::HunterWins) => "HUNTER WINS", None => "in progress" };
    eprintln!("GAME 6v1 [{}]: active {}  escaped {}  caught {}  susp {:.2}", status, g.active_cats(), g.escaped, g.caught, g.hunter.suspicion);

    let mut m = world::build_trees(q(150));
    m.extend(&world::build_bushes(q(200)));
    m.extend(&world::build_logs(q(90)));
    m.extend(&world::build_grass(q(9000)));
    m.extend(&world::build_litter(q(4000)));
    m.extend(&world::build_terrain(if hires() { 240 } else { 150 }));
    m.extend(&world::build_reeds(q(500)));
    m.extend(&build_critters(&g.critters));      // wildlife scattering from the chase

    let mut cx_sum = 0.0; let mut cz_sum = 0.0; let mut n = 0.0;
    for c in g.cats.iter().filter(|c| c.alive) {
        let gy = world::height_at(c.x, c.z);
        let (yoff, pose) = if c.climb_t > 0.01 { (c.climb_height(), cat::climb()) } else if c.crouched { (0.0, cat::coil()) } else if c.moving { (0.0, cat::gait(c.speed, g.tick as f32 * 0.05, c.id as f32 * 0.37)) } else { (0.0, cat::idle(g.tick as f32 * 0.03 + c.id as f32 * 0.3)) };
        cat::build(&mut m, v3(c.x, gy + yoff, c.z), c.heading, &pose, clawbound_rs::class::Class::Cat);
        cx_sum += c.x; cz_sum += c.z; n += 1.0;
    }
    let hp = g.hunter.kind.params();
    let hgy = world::height_at(g.hunter.x, g.hunter.z);
    if g.hunter.kind.is_shadow() { build_shadow(&mut m, v3(g.hunter.x, hgy, g.hunter.z)); } else { build_fox(&mut m, v3(g.hunter.x, hgy, g.hunter.z)); }
    let col = rgb(hunter::state_color(g.hunter.state));
    add_cone(&mut m, g.hunter.x, g.hunter.z, g.hunter.heading, &hp, col, 0.5);
    cx_sum += g.hunter.x; cz_sum += g.hunter.z; n += 1.0;

    let (ox, oz) = g.objective;
    let ogy = world::height_at(ox, oz);
    m.set_op(0.9);
    m.segment(v3(ox, ogy, oz), v3(ox, ogy + 14.0, oz), 0.7, 0.2, 6, rgb(0xffd24a));
    m.set_op(1.0);

    // weather particles around the pack's centre
    let focus = v3(cx_sum / n, world::height_at(cx_sum / n, cz_sum / n), cz_sum / n);
    m.extend(&build_weather(&g.weather, 0.4, focus));

    // paw prints pressed into the ground where the pack has trodden (the scent trail)
    for s in &g.scent {
        let gy = world::height_at(s.x, s.z);
        let a = (s.strength * 0.45).clamp(0.0, 0.4);
        m.set_op(a);
        m.boxx(v3(s.x, gy + 0.03, s.z), v3(0.1, 0.008, 0.16), rgb(0x37291d));
        m.set_op(1.0);
    }
    m.extend(&build_scent_wisps(&g, 0.4));   // scent made visible on the wind

    (m, v3(cx_sum / n, 2.0, cz_sum / n))
}

/// Worn dirt paths (lanes) drawn as a ribbon of flat discs hugging the terrain.
fn build_lanes(m: &mut Mesh, lanes: &[(f32, f32, f32, f32)]) {
    let dirt = rgb(0x8a7550);
    for &(ax, az, bx, bz) in lanes {
        let len = ((bx - ax).powi(2) + (bz - az).powi(2)).sqrt();
        let steps = (len / 3.5).max(1.0) as i32;
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let (x, z) = (ax + (bx - ax) * t, az + (bz - az) * t);
            let gy = world::height_at(x, z);
            m.cone(v3(x, gy + 0.12, z), 5.2, 0.22, 8, dirt);
        }
    }
}

/// A grand, swaying ancestor tree: tiered canopy on a heavy trunk with flared roots and
/// an aura ring — gold when claimable, green once it's a claimed staging ground, dim while
/// its buff is resting on cooldown. `sway` animates the canopy lean.
fn build_ancestor(m: &mut Mesh, x: f32, z: f32, gy: f32, sway: f32, ready: bool, owner: Option<u8>) {
    let bark = rgb(0x5b4636);
    let leaf = if owner.is_some() { rgb(0x93c46f) } else { rgb(0x6f9a54) };
    m.segment(v3(x, gy, z), v3(x, gy + 13.0, z), 1.7, 1.1, 8, bark);          // trunk
    for k in 0..6 {                                                           // flared roots
        let a = k as f32 / 6.0 * std::f32::consts::TAU;
        m.segment(v3(x, gy + 0.6, z), v3(x + a.cos() * 3.2, gy - 0.4, z + a.sin() * 3.2), 0.8, 0.3, 4, bark);
    }
    let (sx, sz) = (sway.sin() * 0.9, (sway * 0.8).cos() * 0.6);              // canopy sway
    for (i, (h, r)) in [(11.0f32, 8.5f32), (14.5, 6.6), (17.6, 4.3)].iter().enumerate() {
        let lean = (i as f32 + 1.0) * 0.5;
        m.cone(v3(x + sx * lean, gy + *h, z + sz * lean), *r, 5.2, 9, leaf);
    }
    // aura: the owning clan's colour, gold if it's wild but ready to claim, else dim
    let (ac, op) = if let Some(c) = owner { (rgb(clawbound_rs::prowl::clan_color(c)), 0.36) }
        else if ready { (rgb(0xffcf5a), 0.42) } else { (rgb(0x555560), 0.16) };
    m.set_op(op);
    for k in 0..40 {                                                          // aura ring
        let a = k as f32 / 40.0 * std::f32::consts::TAU;
        let (rx, rz) = (x + a.cos() * 11.0, z + a.sin() * 11.0);
        m.segment(v3(rx, gy - 0.5, rz), v3(rx, gy + 1.3, rz), 0.32, 0.32, 3, ac);
    }
    m.set_op(1.0);
}

fn build_prowl() -> (Mesh, Vec3) {
    use clawbound_rs::prowl::{Prowl, ProwlObj};
    use clawbound_rs::class::Class;
    let hk = match std::env::args().nth(4).as_deref() {
        Some("dog") => hunter::Kind::Dog, Some("badger") => hunter::Kind::Badger, Some("hawk") => hunter::Kind::Hawk,
        Some("snake") => hunter::Kind::Snake, Some("shadow") => hunter::Kind::Shadow, _ => hunter::Kind::Fox,
    };
    let cl = match std::env::args().nth(5).as_deref() {
        Some("squirrel") => Class::Squirrel, Some("frog") => Class::Frog, Some("goat") => Class::Goat,
        Some("hawk") => Class::Hawk, Some("cat") => Class::Cat, _ => Class::Otter,
    };
    let mut g = Prowl::new(cl, hk);
    for _ in 0..(6 * 30) {
        let (mut tx, mut tz) = (g.obj.tx - g.x, g.obj.tz - g.z);
        let hd = ((g.hx - g.x).powi(2) + (g.hz - g.z).powi(2)).sqrt();
        let dodge = hd < 6.0;
        if dodge { tx = g.x - g.hx; tz = g.z - g.hz; }
        g.set_input(tx, tz, false, dodge, false);
        g.step(1.0 / 30.0);
    }
    eprintln!("PROWL [{}]: {}  | prey {}  caught {}  escapes {}", g.class.name(), g.objective_text(), g.prey_caught, g.times_caught, g.escapes);
    // showcase: stand the prowler at the first ancestor tree (claimed) so the shot frames it
    if let Some(a) = g.ancestors.first().copied() {
        g.ancestors[0].claimed_by = Some(0);
        g.x = a.x + 7.0; g.z = a.z + 3.0; g.heading = 3.6;
    }

    let mut m = world::build_terrain(150);
    m.extend(&world::build_water());
    m.extend(&world::build_trees(140));
    m.extend(&world::build_grass(2400));
    m.extend(&world::build_reeds(450));
    m.extend(&build_critters(&g.critters));
    build_lanes(&mut m, &g.lanes);   // worn paths between the staging grounds

    // point-of-interest pillars, and a bright beacon on the live objective
    for &(px, pz, _n) in &g.pois {
        let gy = world::height_at(px, pz);
        m.set_op(0.5);
        m.segment(v3(px, gy, pz), v3(px, gy + 10.0, pz), 0.4, 0.1, 5, rgb(0x8fd0ff));
        m.set_op(1.0);
    }
    if matches!(g.obj.kind, ProwlObj::Reach | ProwlObj::Rendezvous) {
        let (ox, oz) = (g.obj.tx, g.obj.tz);
        let gy = world::height_at(ox, oz);
        m.set_op(0.85);
        m.segment(v3(ox, gy, oz), v3(ox, gy + 22.0, oz), 0.9, 0.25, 8, rgb(0x8fd0ff));
        m.set_op(1.0);
    }
    // the shrinking hunt-ring — a translucent violet storm wall at the current radius
    {
        let rr = g.ring.radius();
        m.set_op(0.3);
        let rc = rgb(0x9a6cff);
        for k in 0..72 {
            let a = k as f32 / 72.0 * std::f32::consts::TAU;
            let (rx, rz) = (g.ring.cx + a.cos() * rr, g.ring.cz + a.sin() * rr);
            let gy = world::height_at(rx, rz);
            m.segment(v3(rx, gy - 1.0, rz), v3(rx, gy + 7.0, rz), 0.35, 0.35, 4, rc);
        }
        m.set_op(1.0);
    }

    // ancestor trees — grand swaying staging grounds
    for a in &g.ancestors {
        let gy = world::height_at(a.x, a.z);
        build_ancestor(&mut m, a.x, a.z, gy, g.time * 1.1 + a.x * 0.05, a.ready(), a.claimed_by);
    }

    // power-up pickups
    for (px, pz, kind) in &g.pickups {
        let gy = world::height_at(*px, *pz) + 1.4;
        m.boxx(v3(*px, gy, *pz), v3(0.5, 0.5, 0.5), rgb(kind.color()));
    }

    // clan minions — small cats on border patrol, tinted by clan
    for mm in &g.minions {
        let gy = world::height_at(mm.x, mm.z);
        let pose = cat::walk((g.time * 5.0 + mm.x) % 1.0);
        cat::build(&mut m, v3(mm.x, gy, mm.z), mm.heading, &pose, clawbound_rs::class::Class::Cat);
        m.boxx(v3(mm.x, gy + 1.3, mm.z), v3(0.28, 0.12, 0.28), rgb(clawbound_rs::prowl::clan_color(mm.clan)));
    }
    // rival clan prowlers — full-size clan-tinted cats
    for rv in &g.rivals {
        let gy = world::height_at(rv.x, rv.z);
        let pose = cat::walk((g.time * 5.0 + rv.x) % 1.0);
        cat::build(&mut m, v3(rv.x, gy, rv.z), rv.heading, &pose, rv.class);
        m.boxx(v3(rv.x, gy + 2.1, rv.z), v3(0.46, 0.2, 0.46), rgb(clawbound_rs::prowl::clan_color(rv.clan)));
    }

    // the prowler (player) + a class-tint marker; then the stalking hunter
    let pgy = world::height_at(g.x, g.z);
    let pose = if g.leap_t > 0.0 { cat::leap() } else if g.crouched { cat::coil() } else if g.moving { cat::walk((g.time * 6.0) % 1.0) } else { cat::stand() };
    cat::build(&mut m, v3(g.x, pgy + g.leap_t.max(0.0) * 2.0, g.z), g.heading, &pose, g.class);
    m.boxx(v3(g.x, pgy + 2.2, g.z), v3(0.4, 0.18, 0.4), rgb(g.class.def().color));

    let hgy = world::height_at(g.hx, g.hz);
    if g.hunter_kind.is_shadow() { build_shadow(&mut m, v3(g.hx, hgy, g.hz)); } else { build_fox(&mut m, v3(g.hx, hgy, g.hz)); }

    (m, v3(g.x, 2.0, g.z))
}

fn build_capture() -> (Mesh, Vec3) {
    use clawbound_rs::capture::{Capture, BASE_R, TAWNY, SILVER};
    use clawbound_rs::class::Class;
    let pc = match std::env::args().nth(4).as_deref() {
        Some("squirrel") => Class::Squirrel, Some("otter") => Class::Otter, Some("frog") => Class::Frog,
        Some("goat") => Class::Goat, Some("hawk") => Class::Hawk, _ => Class::Cat,
    };
    let mut g = Capture::new(pc);
    // play a few seconds so mobs spread out, yarn is in motion, power-ups appear
    let mut t = 0;
    while g.outcome.is_none() && t < 12 * 30 {
        // nudge the player toward the nearest loose yarn so the shot shows a carry
        let m = &g.mobs[0];
        let (mut tx, mut tz) = (0.0f32, 0.0f32);
        if m.carrying { let b = g.base(TAWNY); tx = b.0 - m.x; tz = b.1 - m.z; }
        else { let mut best = (f32::MAX, 0.0f32, 0.0f32);
            for &(yx, yz) in &g.yarn_free { let d = ((yx - m.x).powi(2) + (yz - m.z).powi(2)).sqrt(); if d < best.0 { best = (d, yx, yz); } }
            if best.0 < f32::MAX { tx = best.1 - m.x; tz = best.2 - m.z; } }
        g.set_input(tx, tz, t % 50 == 0, false);
        g.step(1.0 / 30.0); t += 1;
    }
    eprintln!("CAPTURE [period {}]: Tawny {} — Silver {}  loose-yarn {}  powerups {}", g.period, g.score[0], g.score[1], g.yarn_free.len(), g.powerups.len());

    let mut m = world::build_terrain(150);
    m.extend(&world::build_water());
    m.extend(&world::build_trees(90));
    m.extend(&world::build_grass(2200));
    m.extend(&world::build_reeds(400));

    // team bases: a broad low disc + a beacon in team colour
    for team in [TAWNY, SILVER] {
        let (bx, bz) = g.base(team);
        let col = if team == TAWNY { rgb(0xff7030) } else { rgb(0x3070ff) };
        let gy = world::height_at(bx, bz);
        m.set_op(0.5);
        m.cone(v3(bx, gy + 0.15, bz), BASE_R, 0.3, 22, col);   // flat pad
        m.set_op(1.0);
        m.segment(v3(bx, gy, bz), v3(bx, gy + 16.0, bz), 0.6, 0.15, 6, col);
    }
    // yarn balls — pink papercraft puffs
    for &(yx, yz) in &g.yarn_free {
        let gy = world::height_at(yx, yz);
        m.cone(v3(yx, gy + 0.8, yz), 0.8, 1.4, 6, rgb(0xff6acf));
    }
    // power-ups — floating coloured markers
    for p in &g.powerups {
        let gy = world::height_at(p.x, p.z);
        m.boxx(v3(p.x, gy + 1.6, p.z), v3(0.5, 0.5, 0.5), rgb(p.kind.color()));
    }
    // creatures — cat rig tinted per team, a class-coloured back marker, yarn if carried
    for mob in &g.mobs {
        let gy = world::height_at(mob.x, mob.z);
        let pose = if mob.stunned() { cat::startle() } else { cat::stand() };
        cat::build(&mut m, v3(mob.x, gy, mob.z), mob.heading, &pose, mob.class);
        let team_col = if mob.team == TAWNY { rgb(0xff7030) } else { rgb(0x3070ff) };
        m.boxx(v3(mob.x, gy + 2.1, mob.z), v3(0.5, 0.18, 0.5), team_col);          // team collar/flag
        m.boxx(v3(mob.x, gy + 1.2, mob.z), v3(0.32, 0.32, 0.32), rgb(mob.class.def().color)); // class tint
        if mob.carrying { m.cone(v3(mob.x, gy + 1.9, mob.z), 0.45, 0.7, 6, rgb(0xff6acf)); }
    }

    (m, v3(0.0, 0.0, 0.0))
}

#[cfg(feature = "app")]
fn build_world_env() -> Mesh {
    world::init_world();
    let hi = hires();
    let mut m = world::build_terrain(if hi { 240 } else { 150 });
    m.extend(&world::build_water());
    m.extend(&world::build_trees(if hi { 460 } else { 320 }));
    m.extend(&world::build_bushes(q(200)));
    m.extend(&world::build_logs(q(110)));
    m.extend(&world::build_grass(q(11000)));
    m.extend(&world::build_litter(q(5000)));
    m.extend(&world::build_rocks(q(220)));
    m.extend(&world::build_reeds(q(700)));
    m
}

/// Deterministic hash → [0,1) for scattering weather particles.
fn whash(mut h: u32) -> f32 {
    h ^= h >> 16; h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15; h = h.wrapping_mul(0x846ca68b);
    h ^= h >> 16; (h as f32) / (u32::MAX as f32)
}

/// Optional weather override for headless renders via the CLAW_SKY env var.
fn sky_override() -> Option<clawbound_rs::weather::Sky> {
    use clawbound_rs::weather::Sky;
    match std::env::var("CLAW_SKY").ok().as_deref() {
        Some("rain") => Some(Sky::Rain), Some("storm") => Some(Sky::Storm),
        Some("snow") => Some(Sky::Snow), Some("fog") => Some(Sky::Fog),
        Some("overcast") => Some(Sky::Overcast), Some("clear") => Some(Sky::Clear),
        _ => None,
    }
}

/// Scent made visible — faint motes drifting downwind. Your own trail fading behind you,
/// a warm thread from the nearest prey you can smell (follow it upwind to the catch), and
/// the predator's reek when you're downwind of it (a warning drifting to your nose).
fn build_scent_wisps(g: &clawbound_rs::sim::Game, t: f32) -> Mesh {
    use clawbound_rs::mesh::{rgb, Mesh};
    let mut m = Mesh::new();
    let (wx, wz) = g.weather.wind_vec();
    let wstr = (wx * wx + wz * wz).sqrt().max(0.05);
    let (wux, wuz) = (wx / wstr, wz / wstr);
    // your own scent trail: pale motes lifting and drifting downwind, fading as it thins
    for (i, s) in g.scent.iter().enumerate() {
        let drift = (t * 0.5 + i as f32 * 0.31).rem_euclid(1.0);
        let a = (s.strength * 0.2 * (1.0 - drift)).clamp(0.0, 0.16);
        if a < 0.02 { continue; }
        let (px, pz) = (s.x + wux * drift * 1.6, s.z + wuz * drift * 1.6);
        let gy = world::height_at(px, pz) + 0.4 + drift * 0.9;
        m.set_op(a); m.boxx(v3(px, gy, pz), v3(0.09, 0.09, 0.09), rgb(0xbfc8d6)); m.set_op(1.0);
    }
    let ws = g.read_wind();
    // the nearest prey you can smell: a warm thread of scent riding the wind to your nose
    if let (Some((dx, dz, dist, _)), Some(pc)) = (ws.prey, g.cats.iter().find(|c| c.player && c.alive)) {
        let (prx, prz) = (pc.x + dx * dist, pc.z + dz * dist);   // prey sits upwind
        for k in 0..9 {
            let f = (t * 0.6 + k as f32 * 0.12).rem_euclid(1.0);
            let (mx, mz) = (prx + wux * f * dist, prz + wuz * f * dist);
            let gy = world::height_at(mx, mz) + 0.5 + (f * 6.28).sin() * 0.2;
            m.set_op(0.13 * (1.0 - f)); m.boxx(v3(mx, gy, mz), v3(0.1, 0.1, 0.1), rgb(0xd8b060)); m.set_op(1.0);
        }
    }
    // the predator's reek, drifting to you when you're downwind
    if ws.predator_reek > 0.05 {
        for k in 0..7 {
            let f = (t * 0.72 + k as f32 * 0.16).rem_euclid(1.0);
            let (mx, mz) = (g.hunter.x + wux * f * 32.0, g.hunter.z + wuz * f * 32.0);
            let gy = world::height_at(mx, mz) + 0.5 + (f * 6.28 + 1.0).sin() * 0.25;
            m.set_op(0.13 * ws.predator_reek * (1.0 - f)); m.boxx(v3(mx, gy, mz), v3(0.11, 0.11, 0.11), rgb(0xc05040)); m.set_op(1.0);
        }
    }
    m
}

/// Rain streaks / snow / storm particles in a column around `focus`, slanted by wind.
fn build_weather(w: &clawbound_rs::weather::Weather, t: f32, focus: Vec3) -> Mesh {
    use clawbound_rs::weather::Sky;
    let mut m = Mesh::new();
    let (wx, wz) = w.wind_vec();
    let span = 48.0;
    match w.sky {
        Sky::Rain | Sky::Storm => {
            let n = if w.sky == Sky::Storm { 460 } else { 240 };
            let col = if w.sky == Sky::Storm { rgb(0x8aa0bc) } else { rgb(0xa6bdd6) };
            for i in 0..n {
                let (h, h2) = (whash(i as u32), whash(i as u32 ^ 0x9e37));
                let x = focus.x + (h - 0.5) * span;
                let z = focus.z + (h2 - 0.5) * span;
                let fall = (t * 26.0 + h * 40.0).rem_euclid(24.0);
                let y = focus.y + 24.0 - fall;
                let top = v3(x, y + 1.7, z);
                let bot = v3(x + wx * 0.7, y, z + wz * 0.7);
                m.segment(top, bot, 0.03, 0.02, 3, col);
            }
        }
        Sky::Snow => {
            for i in 0..300 {
                let (h, h2) = (whash(i as u32), whash(i as u32 ^ 0x33a1));
                let sway = (t * 0.8 + h * 6.28).sin() * 1.3;
                let x = focus.x + (h - 0.5) * span + sway;
                let z = focus.z + (h2 - 0.5) * span;
                let fall = (t * 3.2 + h * 14.0).rem_euclid(14.0);
                let y = focus.y + 20.0 - fall;
                m.boxx(v3(x, y, z), v3(0.12, 0.12, 0.12), rgb(0xf2f6ff));
            }
        }
        _ => {}
    }
    m
}

#[cfg(feature = "app")]
fn build_entities(g: &clawbound_rs::sim::Game, anim_t: f32, show_cone: bool) -> Mesh {
    let mut m = Mesh::new();
    m.extend(&build_critters(&g.critters));      // ambient wildlife, fleeing the chase
    for c in g.cats.iter().filter(|c| c.alive) {
        let gy = world::height_at(c.x, c.z);
        let off = c.id as f32 * 0.37;                       // per-cat gait offset so they don't march in lockstep
        let (yoff, pose) = if let Some((mv, t)) = g.player_move.filter(|_| c.player) {
            let ph = (t / mv.total()).clamp(0.0, 1.0);
            let air = if matches!(mv, clawbound_rs::combat::Move::Pounce | clawbound_rs::combat::Move::Jump) { c.leap_arc() } else { 0.0 };
            (air, cat::move_pose(mv, ph))
        }
            else if c.is_carried() { (2.6 + (anim_t * 6.28).sin() * 0.15, cat::startle()) } // dangling in the talons
            else if c.is_leaping() { (c.leap_arc(), cat::leap()) }   // airborne bound, arcing over the ground
            else if c.climb_t > 0.01 { (c.climb_height(), cat::climb()) }
            else if c.crouched { (0.0, cat::coil()) }
            else if c.moving {
                let cover = world::cover_at(c.x, c.z);
                if cover > 0.42 { (0.0, cat::wade(c.speed, anim_t, off, cover)) }   // pushing through the brush
                else { (0.0, cat::gait(c.speed, anim_t, off)) }                     // walk → trot → bound by speed
            }
            else if c.player && g.danger > 0.35 { (0.0, cat::taste_air(anim_t * 0.9)) }   // frozen, reading the wind
            else { (0.0, cat::idle(anim_t * 0.6 + off)) };   // breathing idle when standing still
        cat::build(&mut m, v3(c.x, gy + yoff, c.z), c.heading, &pose, clawbound_rs::class::Class::Cat);
        m.boxx(v3(c.x, gy + yoff + 1.55, c.z), v3(0.22, 0.1, 0.22), rgb(clawbound_rs::clan::color(g.clan))); // clan collar
    }
    let hp = g.hunter.kind.params();
    let hgy = world::height_at(g.hunter.x, g.hunter.z);
    if g.hunter.kind.is_shadow() { build_shadow(&mut m, v3(g.hunter.x, hgy, g.hunter.z)); } else { build_fox(&mut m, v3(g.hunter.x, hgy, g.hunter.z)); }
    if show_cone { add_cone(&mut m, g.hunter.x, g.hunter.z, g.hunter.heading, &hp, rgb(hunter::state_color(g.hunter.state)), 0.5); }

    // floating stamina bar + marker over the player's cat (id 0)
    if let Some(pc) = g.cats.iter().find(|c| c.player && c.alive && !c.safe) {
        let gy = world::height_at(pc.x, pc.z) + pc.climb_height();
        let by = gy + 2.7;
        m.boxx(v3(pc.x, by, pc.z), v3(0.36, 0.05, 0.12), rgb(0x232323));       // bar backing
        let s = pc.stamina.clamp(0.0, 1.0);
        let col = if s > 0.5 { 0x6ad06a } else if s > 0.2 { 0xd6c24a } else { 0xd6584a };
        let fw = 0.34 * s;
        m.boxx(v3(pc.x - 0.34 + fw, by + 0.02, pc.z), v3(fw.max(0.001), 0.06, 0.13), rgb(col));
        m.cone(v3(pc.x, by + 0.5, pc.z), 0.18, 0.42, 4, rgb(0x5fd0ff));         // "you" marker
    }
    let (ox, oz) = g.objective; let ogy = world::height_at(ox, oz);
    m.set_op(0.9); m.segment(v3(ox, ogy, oz), v3(ox, ogy + 12.0, oz), 0.6, 0.18, 6, rgb(0xffd24a)); m.set_op(1.0);

    // weather: rain / snow / storm particles falling around the player
    if let Some(pc) = g.cats.iter().find(|c| c.player) {
        let f = v3(pc.x, world::height_at(pc.x, pc.z), pc.z);
        m.extend(&build_weather(&g.weather, anim_t, f));
    }
    // paw prints in the dirt where the pack has trodden (the scent trail)
    for s in &g.scent {
        let gy = world::height_at(s.x, s.z);
        let a = (s.strength * 0.45).clamp(0.0, 0.4);
        m.set_op(a);
        m.boxx(v3(s.x, gy + 0.03, s.z), v3(0.1, 0.008, 0.16), rgb(0x37291d));
        m.set_op(1.0);
    }
    m.extend(&build_scent_wisps(g, anim_t));   // scent made visible, drifting on the wind
    m
}

#[cfg(feature = "app")]
mod app;
#[cfg(feature = "app")]
mod egui_render;

/// Otherworld dressing: long shadows crawling from the trees and the faint shapes
/// of cats moving in the underworld. Appended over any scene during a nightmare.
fn build_nightmare_extras(center: Vec3) -> Mesh {
    let mut m = Mesh::new();
    let (sdx, sdz) = (0.62, 0.78);           // low-sun shadow direction
    let (px, pz) = (-sdz, sdx);              // perpendicular
    let shade_col = rgb(0x080610);
    m.set_op(0.42);
    for (tx, tz) in world::tree_positions(120) {
        if (tx - center.x).hypot(tz - center.z) > 78.0 { continue; }
        let gy = world::height_at(tx, tz) + 0.06;
        let len = 13.0; let w = 1.5;
        let a = v3(tx + px * w, gy, tz + pz * w);
        let b = v3(tx - px * w, gy, tz - pz * w);
        let c = v3(tx + sdx * len - px * w * 0.25, gy, tz + sdz * len - pz * w * 0.25);
        let d = v3(tx + sdx * len + px * w * 0.25, gy, tz + sdz * len + pz * w * 0.25);
        m.quad(a, d, c, b, shade_col);       // up-facing long shadow streak
    }
    // spectral cats drifting through the underworld (very faint)
    m.set_op(0.22);
    for i in 0..7 {
        let a = i as f32 * 0.92;
        let r = 16.0 + (i as f32 * 5.0) % 22.0;
        let gx = center.x + a.cos() * r;
        let gz = center.z + a.sin() * r + i as f32 * 3.0;
        let gy = world::height_at(gx, gz) + 0.4 + (i as f32 * 0.7).sin() * 0.6; // hovering
        cat::build(&mut m, v3(gx, gy, gz), if i % 2 == 0 { 1.0 } else { -1.0 }, &cat::walk((i as f32 * 0.17) % 1.0), clawbound_rs::class::Class::Cat);
    }
    m.set_op(1.0);
    m
}

fn main() {
    #[cfg(all(feature = "app", not(target_arch = "wasm32")))]
    {
        if std::env::args().any(|a| a == "--play") { app::run(); return; }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pollster::block_on(run());
}

#[cfg(not(target_arch = "wasm32"))]
async fn run() {
    let scale: f32 = if hires() { 1.85 } else { 1.0 };   // supersample for AAA mode (fits a 2048 texture cap)
    #[allow(non_snake_case)]
    let (W, H) = ((BASE_W as f32 * scale) as u32, (BASE_H as f32 * scale) as u32);
    let mode = std::env::args().nth(2).unwrap_or_default();
    let (mut scene, focus) = match mode.as_str() {
        "poses" => build_poses(),
        "clans" => build_clans(),
        "seq" => build_sequence(),
        "senses" => build_senses(),
        "map" => build_scene(),
        "goals" => build_goals(),
        "game" => build_game(),
        "prowl" => build_prowl(),
        "capture" => build_capture(),
        "wildlife" => build_wildlife(),
        _ => build_scene(),
    };
    if std::env::args().nth(3).as_deref() == Some("nightmare") {
        world::init_world();
        scene.extend(&build_nightmare_extras(focus));
    }
    let vcount = scene.count();
    eprintln!("scene: {} vertices  (mode: {})", vcount, if mode.is_empty() { "world" } else { &mode });
    if mode == "goals" {
        let s = mapeval::score();
        eprintln!("MAP SCORE: grade {}  total {:.0}/100  | openness {:.2} flow {:.2} cover {:.2} vert {:.2} | FAIRNESS {:.0}/100",
            s.grade(), s.total, s.openness, s.flow, s.cover, s.verticality, s.fairness);
        for o in objectives::place() {
            eprintln!("  objective {:<11} at ({:.0}, {:.0})", objectives::name(o.kind), o.x, o.z);
        }
    }

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::all(), ..Default::default() });
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::None, compatible_surface: None, force_fallback_adapter: false,
    }).await.expect("no adapter");
    eprintln!("adapter: {} / {:?}", adapter.get_info().name, adapter.get_info().backend);
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: None, required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(), memory_hints: wgpu::MemoryHints::default(),
    }, None).await.expect("no device");

    // camera — pose sheet gets a side-on sweep; world gets the hero-tree framing
    let (eye, target) = match mode.as_str() {
        "clans" => (focus.add(v3(0.0, 8.5, -17.0)), focus.add(v3(0.0, 4.2, 1.0))),
        "poses" => (focus.add(v3(10.0, 4.2, -9.5)), focus.add(v3(-3.0, -0.3, 2.0))),
        "seq" => (focus.add(v3(15.0, 5.5, 1.0)), focus.add(v3(-4.0, 0.0, 6.0))),
        "senses" => (v3(0.0, 78.0, 26.0), v3(0.0, 0.0, 26.0)),
        "map" => (v3(70.0, 82.0, -14.0), v3(4.0, -3.0, 42.0)),
        "goals" => (v3(48.0, 66.0, -24.0), v3(0.0, -2.0, 16.0)),
        "game" => (focus.add(v3(22.0, 40.0, -22.0)), focus.add(v3(0.0, -3.0, 9.0))),
        "prowl" => (focus.add(v3(20.0, 34.0, -20.0)), focus.add(v3(0.0, -3.0, 8.0))),
        "capture" => (v3(92.0, 104.0, -30.0), v3(-6.0, -10.0, 6.0)),
        "wildlife" => (focus.add(v3(0.5, 2.3, -7.5)), focus.add(v3(0.0, 0.9, 3.2))),
        _ => (focus.add(v3(-13.0, 8.0, -15.0)), focus.add(v3(3.0, -3.5, 7.0))),
    };
    let up = if mode == "senses" { v3(0.0, 0.0, 1.0) } else { v3(0.0, 1.0, 0.0) };
    let proj = perspective(56f32.to_radians(), W as f32 / H as f32, 0.1, 900.0);
    let view = look_at(eye, target, up);
    let mvp = proj.mul(&view);

    // time of day (arg 3): day | dusk | night | nightmare — drives light, fill, sky
    let tod = std::env::args().nth(3).unwrap_or_else(|| "day".into());
    let (light, key, amb, bg) = match tod.as_str() {
        "dusk" => ([0.85, 0.42, 0.30, 0.0], [1.15, 0.62, 0.40, 0.0], [0.40, 0.34, 0.44, 0.0], [0.66, 0.50, 0.46]),
        "night" => ([0.30, 0.75, 0.55, 0.0], [0.42, 0.54, 0.82, 0.0], [0.20, 0.24, 0.40, 0.0], [0.16, 0.20, 0.32]),
        // the otherworld: a low, cold, sourceless light over a near-black violet sky
        "nightmare" => ([0.7, 0.28, 0.4, 0.0], [0.40, 0.34, 0.58, 0.0], [0.22, 0.19, 0.30, 0.0], [0.06, 0.04, 0.10]),
        _ => ([0.40, 1.0, 0.35, 0.0], [1.05, 0.97, 0.85, 0.0], [0.52, 0.53, 0.50, 0.0], [0.86, 0.81, 0.70]),
    };
    let night = if tod == "night" || tod == "nightmare" { 1.0f32 } else if tod == "dusk" { 0.6 } else { 0.0 };
    let nightmare = if tod == "nightmare" { 1.0f32 } else { 0.0 };

    #[repr(C)]
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    struct U { mvp: [f32; 16], light: [f32; 4], key: [f32; 4], amb: [f32; 4] }
    let ubo = U { mvp: mvp.0, light, key, amb };
    let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("u"), contents: bytemuck::bytes_of(&ubo), usage: wgpu::BufferUsages::UNIFORM,
    });
    let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("verts"), contents: bytemuck::cast_slice(&scene.verts), usage: wgpu::BufferUsages::VERTEX,
    });

    // targets
    let scene_fmt = wgpu::TextureFormat::Rgba8Unorm;
    let mktex = |usage, fmt| device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: fmt,
        usage, view_formats: &[],
    });
    let scene_tex = mktex(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, scene_fmt);
    let depth_tex = mktex(wgpu::TextureUsages::RENDER_ATTACHMENT, wgpu::TextureFormat::Depth32Float);
    let final_tex = mktex(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, scene_fmt);
    let scene_view = scene_tex.create_view(&Default::default());
    let depth_view = depth_tex.create_view(&Default::default());
    let final_view = final_tex.create_view(&Default::default());

    // ── scene pipeline (Lambert warm key; depth packed into alpha) ──
    let scene_sh = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None, source: wgpu::ShaderSource::Wgsl(SCENE_WGSL.into()),
    });
    let ubgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None, entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        }],
    });
    let ubg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None, layout: &ubgl, entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
    });
    let scene_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&ubgl], push_constant_ranges: &[] });
    let vlayout = wgpu::VertexBufferLayout {
        array_stride: 40, step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute { offset: 0, shader_location: 0, format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: 12, shader_location: 1, format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: 24, shader_location: 2, format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: 36, shader_location: 3, format: wgpu::VertexFormat::Float32 },
        ],
    };
    let scene_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None, layout: Some(&scene_pl),
        vertex: wgpu::VertexState { module: &scene_sh, entry_point: "vs", buffers: &[vlayout], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState { module: &scene_sh, entry_point: "fs",
            targets: &[Some(wgpu::ColorTargetState { format: scene_fmt, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default() }),
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::Less, stencil: Default::default(), bias: Default::default() }),
        multisample: Default::default(), multiview: None, cache: None,
    });

    // ── post pipeline (paper + ink + grain + vignette) ──
    let post_src = POST_WGSL.replace("TWf", &format!("{:.8}", 1.0 / W as f32))
        .replace("THf", &format!("{:.8}", 1.0 / H as f32))
        .replace("WPIX", &format!("{:.1}", W as f32))
        .replace("HPIX", &format!("{:.1}", H as f32));
    let post_sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(post_src.into()) });
    let samp = device.create_sampler(&wgpu::SamplerDescriptor { label: None, ..Default::default() });
    let fxbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("fx"), contents: bytemuck::cast_slice(&[nightmare, night, 0.0f32, 0.0f32]), usage: wgpu::BufferUsages::UNIFORM,
    });
    let pbgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None, entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ],
    });
    let pbg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None, layout: &pbgl, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(&samp) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&scene_view) },
            wgpu::BindGroupEntry { binding: 2, resource: fxbuf.as_entire_binding() },
        ],
    });
    let post_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&pbgl], push_constant_ranges: &[] });
    let post_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None, layout: Some(&post_pl),
        vertex: wgpu::VertexState { module: &post_sh, entry_point: "vs", buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState { module: &post_sh, entry_point: "fs",
            targets: &[Some(wgpu::ColorTargetState { format: scene_fmt, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview: None, cache: None,
    });

    // ── record + submit ──
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &scene_view, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: bg[0] as f64, g: bg[1] as f64, b: bg[2] as f64, a: 1.0 }), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
            timestamp_writes: None, occlusion_query_set: None,
        });
        rp.set_pipeline(&scene_pipe); rp.set_bind_group(0, &ubg, &[]); rp.set_vertex_buffer(0, vbuf.slice(..)); rp.draw(0..vcount, 0..1);
    }
    {
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &final_view, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None,
        });
        rp.set_pipeline(&post_pipe); rp.set_bind_group(0, &pbg, &[]); rp.draw(0..3, 0..1);
    }
    // readback (bytes_per_row must be 256-aligned)
    let bpr = W * 4;
    let padded = (bpr + 255) & !255u32;
    let rbuf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (padded * H) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    enc.copy_texture_to_buffer(
        wgpu::ImageCopyTexture { texture: &final_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::ImageCopyBuffer { buffer: &rbuf, layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(H) } },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 });
    queue.submit(Some(enc.finish()));

    let slice = rbuf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::Maintain::Wait);
    let mapped = slice.get_mapped_range();
    let mut data = vec![0u8; (bpr * H) as usize];
    for row in 0..H as usize {
        let src = row * padded as usize;
        let dst = row * bpr as usize;
        data[dst..dst + bpr as usize].copy_from_slice(&mapped[src..src + bpr as usize]);
    }
    let path = std::env::args().nth(1).unwrap_or_else(|| "shot.png".into());
    if let Some(parent) = std::path::Path::new(&path).parent() {
        if !parent.as_os_str().is_empty() { let _ = std::fs::create_dir_all(parent); }
    }
    if let Err(e) = image::save_buffer(&path, &data, W, H, image::ColorType::Rgba8) {
        eprintln!("failed to write {path}: {e}");
        std::process::exit(1);
    }
    eprintln!("wrote {}", path);
}

const SCENE_WGSL: &str = r#"
struct U { mvp: mat4x4<f32>, light: vec4<f32>, key: vec4<f32>, amb: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
struct VO { @builtin(position) pos: vec4<f32>, @location(0) nrm: vec3<f32>, @location(1) col: vec3<f32>, @location(2) op: f32 };
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec3<f32>, @location(3) o: f32) -> VO {
  var out: VO; out.pos = u.mvp * vec4<f32>(p, 1.0); out.nrm = n; out.col = c; out.op = o; return out;
}
fn bayer4(p: vec2<u32>) -> f32 {
  var m = array<f32,16>(0.0,8.0,2.0,10.0, 12.0,4.0,14.0,6.0, 3.0,11.0,1.0,9.0, 15.0,7.0,13.0,5.0);
  let idx = (p.y % 4u) * 4u + (p.x % 4u);
  return (m[idx] + 0.5) / 16.0;
}
@fragment fn fs(i: VO) -> @location(0) vec4<f32> {
  if (i.op < 0.999) {
    let th = bayer4(vec2<u32>(u32(i.pos.x), u32(i.pos.y)));
    if (i.op < th) { discard; }
  }
  let L = normalize(u.light.xyz);
  let ndl = max(dot(normalize(i.nrm), L), 0.0);
  let lit = i.col * (u.amb.xyz + u.key.xyz * ndl);
  return vec4<f32>(lit, i.pos.z);
}
"#;

const POST_WGSL: &str = r#"
@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> fx: vec4<f32>;   // (nightmare, night, time, 0)
struct VO { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) vi: u32) -> VO {
  var p = array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0), vec2<f32>(3.0,-1.0), vec2<f32>(-1.0,3.0));
  var o: VO; let q = p[vi]; o.pos = vec4<f32>(q, 0.0, 1.0);
  o.uv = vec2<f32>((q.x + 1.0) * 0.5, 1.0 - (q.y + 1.0) * 0.5); return o;
}
fn h21(p: vec2<f32>) -> f32 {
  var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
  p3 = p3 + dot(p3, vec3<f32>(p3.y, p3.z, p3.x) + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}
// ordered 4x4 Bayer threshold (0..1) at a pixel, for dithering
fn bayer4(c: vec2<f32>) -> f32 {
  var m = array<f32,16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
  let x = u32(c.x) & 3u;
  let y = u32(c.y) & 3u;
  return (m[y * 4u + x] + 0.5) / 16.0;
}
@fragment fn fs(i: VO) -> @location(0) vec4<f32> {
  let uv = i.uv;
  let nightmare = fx.x; let night = fx.y; let t = fx.z;
  let texel = vec2<f32>(TWf, THf);
  let c = textureSample(tex, samp, uv);
  let dC = c.a;
  let dR = textureSample(tex, samp, uv + vec2<f32>(texel.x, 0.0)).a;
  let dD = textureSample(tex, samp, uv + vec2<f32>(0.0, texel.y)).a;
  let dL = textureSample(tex, samp, uv - vec2<f32>(texel.x, 0.0)).a;
  let dU = textureSample(tex, samp, uv - vec2<f32>(0.0, texel.y)).a;
  let edge = clamp((abs(dC-dR)+abs(dC-dD)+abs(dC-dL)+abs(dC-dU)) * 350.0, 0.0, 1.0);
  let px = uv * vec2<f32>(WPIX, HPIX);
  let paper = 0.94 + 0.06 * h21(floor(px * 0.5));
  let grain = 0.965 + 0.035 * h21(px);
  var col = c.rgb * paper * grain;

  // ── night sky: stars in the far background (dC ~ 1 = nothing drawn), high up ──
  let is_sky = step(0.985, dC) * (1.0 - smoothstep(0.4, 0.78, uv.y));
  let cell = floor(px / 3.0);
  let hs = h21(cell);
  let star = step(0.984, hs) * (0.55 + 0.45 * h21(cell + 5.0) + 0.15 * sin(t * 2.0 + hs * 30.0));
  // the otherworld blots the stars out and swallows the sky
  let star_vis = is_sky * star * (1.0 - nightmare) * max(night, 0.2);
  col = col + vec3<f32>(0.9, 0.92, 1.0) * star_vis;

  // ── otherworld: darken, drain color, long creeping shadow from below ──
  if (nightmare > 0.001) {
    let g = dot(col, vec3<f32>(0.299, 0.587, 0.114));
    col = mix(col, vec3<f32>(g * 0.72, g * 0.66, g * 0.9), nightmare * 0.7);    // desaturate to a sickly grey-violet
    col = col * (1.0 - nightmare * 0.34);                                        // sky goes dark, not pitch black
    let creep = smoothstep(0.2, 0.98, uv.y) * (0.28 + 0.22 * h21(vec2<f32>(cell.x, floor(t * 1.3))));
    col = col * (1.0 - creep * nightmare * 0.42);                               // long shadows crawl up from the ground
    col = col + vec3<f32>(0.06, 0.03, 0.09) * nightmare;                        // faint violet underglow so shapes read
  }

  // ── suspense: a heartbeat that pounds faster and harder as danger (fx.w) rises,
  //    closing the vignette in on each beat and bleeding the edges red near a catch ──
  let danger = fx.w;
  let hr = 3.0 + danger * 6.5;
  let beat = pow(max(sin(t * hr), 0.0), 6.0);
  let pulse = danger * (0.35 + 0.65 * beat);
  let d = distance(uv, vec2<f32>(0.5, 0.5));
  let vig = 0.55 + nightmare * 0.35 + pulse * 0.55;
  col = col * (1.0 - smoothstep(0.42 - pulse * 0.12, 0.98, d) * vig);
  let edgeglow = smoothstep(0.44, 0.95, d) * pulse;
  col = mix(col, vec3<f32>(0.52, 0.06, 0.05), edgeglow * 0.55);
  let ink = vec3<f32>(0.15, 0.11, 0.09);
  col = mix(col, ink, edge * 0.8);
  // ordered (Bayer) dithering: quantise the colour along a 4x4 threshold pattern,
  // which breaks up smooth-gradient banding and lays down a printed-paper texture.
  let bay = bayer4(px);
  let levels = 26.0;
  col = floor(col * levels + (bay - 0.5)) / levels;
  return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;
