//! clawbound-rs — native papercraft renderer (Rust + wgpu), headless PNG output.

use clawbound_rs::{cat, ecology, hunter, mapeval, objectives, world};
use clawbound_rs::math::{look_at, perspective, v3, Vec3};
use clawbound_rs::mesh::{rgb, shade, Mesh};
use wgpu::util::DeviceExt;

const W: u32 = 1100;
const H: u32 = 680;

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
    cat::build(&mut m, v3(perch.x, perch.y - 0.12, perch.z), 1.0, &cat::climb());

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
    for p in &poses { cat::build(&mut m, v3(x, 0.0, 0.0), 1.0, p); x += 2.4; }
    // climb pose on a branch stub
    let bx = x + 0.4;
    m.segment(v3(bx, 0.0, 0.0), v3(bx, 2.6, 0.7), 0.16, 0.11, 6, rgb(0x6b4b32));
    cat::build(&mut m, v3(bx - 0.05, 1.15, 0.28), 1.0, &cat::climb());

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
        cat::build(&mut m, v3(0.0, 0.0, z), 1.0, &pose);
    }
    // climb the limb in steps, stopping at safe reach (further out would snap)
    let amax = best.safe_reach(5.0).min(0.85);
    for i in 0..3 {
        let along = 0.18 + (i as f32 / 2.0) * (amax - 0.18);
        let p = best.point(along);
        cat::build(&mut m, v3(p.x, p.y - 0.1, p.z), 1.0, &cat::climb());
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
    cat::build(&mut m, v3(catx, 0.0, catz), 1.0, &cat::stand());
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
    m.extend(&world::build_bushes(160));
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
    let status = match g.outcome { Some(Outcome::CatsWin) => "CATS WIN", Some(Outcome::HunterWins) => "HUNTER WINS", None => "in progress" };
    eprintln!("GAME 6v1 [{}]: active {}  escaped {}  caught {}  susp {:.2}", status, g.active_cats(), g.escaped, g.caught, g.hunter.suspicion);

    let mut m = world::build_trees(150);
    m.extend(&world::build_bushes(200));
    m.extend(&world::build_grass(2500));
    m.extend(&world::build_terrain(150));
    m.extend(&world::build_reeds(500));
    m.extend(&build_critters(&g.critters));      // wildlife scattering from the chase

    let mut cx_sum = 0.0; let mut cz_sum = 0.0; let mut n = 0.0;
    for c in g.cats.iter().filter(|c| c.alive) {
        let gy = world::height_at(c.x, c.z);
        let (yoff, pose) = if c.climbing { (4.2, cat::climb()) } else if c.crouched { (0.0, cat::coil()) } else if c.moving { (0.0, cat::walk((g.tick as f32 * 0.1) % 1.0)) } else { (0.0, cat::stand()) };
        cat::build(&mut m, v3(c.x, gy + yoff, c.z), 1.0, &pose);
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

    (m, v3(cx_sum / n, 2.0, cz_sum / n))
}

#[cfg(feature = "app")]
fn build_world_env() -> Mesh {
    world::init_world();
    let mut m = world::build_terrain(150);
    m.extend(&world::build_water());
    m.extend(&world::build_trees(320));
    m.extend(&world::build_bushes(200));
    m.extend(&world::build_grass(3500));
    m.extend(&world::build_rocks(220));
    m.extend(&world::build_reeds(700));
    m
}

#[cfg(feature = "app")]
fn build_entities(g: &clawbound_rs::sim::Game, phase: f32, show_cone: bool) -> Mesh {
    let mut m = Mesh::new();
    m.extend(&build_critters(&g.critters));      // ambient wildlife, fleeing the chase
    for c in g.cats.iter().filter(|c| c.alive) {
        let gy = world::height_at(c.x, c.z);
        let (yoff, pose) = if c.is_carried() { (2.6 + (phase * 6.28).sin() * 0.15, cat::startle()) } // dangling in the talons
            else if c.is_leaping() { (c.leap_arc(), cat::leap()) }   // airborne bound, arcing over the ground
            else if c.climbing { (4.2, cat::climb()) }
            else if c.crouched { (0.0, cat::coil()) } else if c.moving { (0.0, cat::walk(phase)) } else { (0.0, cat::stand()) };
        cat::build(&mut m, v3(c.x, gy + yoff, c.z), 1.0, &pose);
    }
    let hp = g.hunter.kind.params();
    let hgy = world::height_at(g.hunter.x, g.hunter.z);
    if g.hunter.kind.is_shadow() { build_shadow(&mut m, v3(g.hunter.x, hgy, g.hunter.z)); } else { build_fox(&mut m, v3(g.hunter.x, hgy, g.hunter.z)); }
    if show_cone { add_cone(&mut m, g.hunter.x, g.hunter.z, g.hunter.heading, &hp, rgb(hunter::state_color(g.hunter.state)), 0.5); }

    // floating stamina bar + marker over the player's cat (id 0)
    if let Some(pc) = g.cats.iter().find(|c| c.player && c.alive && !c.safe) {
        let gy = world::height_at(pc.x, pc.z) + if pc.climbing { 4.2 } else { 0.0 };
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
        cat::build(&mut m, v3(gx, gy, gz), if i % 2 == 0 { 1.0 } else { -1.0 }, &cat::walk((i as f32 * 0.17) % 1.0));
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
    let mode = std::env::args().nth(2).unwrap_or_default();
    let (mut scene, focus) = match mode.as_str() {
        "poses" => build_poses(),
        "seq" => build_sequence(),
        "senses" => build_senses(),
        "map" => build_scene(),
        "goals" => build_goals(),
        "game" => build_game(),
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
        "poses" => (focus.add(v3(10.0, 4.2, -9.5)), focus.add(v3(-3.0, -0.3, 2.0))),
        "seq" => (focus.add(v3(15.0, 5.5, 1.0)), focus.add(v3(-4.0, 0.0, 6.0))),
        "senses" => (v3(0.0, 78.0, 26.0), v3(0.0, 0.0, 26.0)),
        "map" => (v3(70.0, 82.0, -14.0), v3(4.0, -3.0, 42.0)),
        "goals" => (v3(48.0, 66.0, -24.0), v3(0.0, -2.0, 16.0)),
        "game" => (focus.add(v3(22.0, 40.0, -22.0)), focus.add(v3(0.0, -3.0, 9.0))),
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

  let d = distance(uv, vec2<f32>(0.5, 0.5));
  let vig = 0.55 + nightmare * 0.35;                                           // heavier, closing-in vignette in the nightmare
  col = col * (1.0 - smoothstep(0.5, 0.98, d) * vig);
  let ink = vec3<f32>(0.15, 0.11, 0.09);
  col = mix(col, ink, edge * 0.8);
  return vec4<f32>(col, 1.0);
}
"#;
