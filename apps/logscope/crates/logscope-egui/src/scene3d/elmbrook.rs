//! ELMBROOK — one "living knowledge city", two districts, from the two reference renders.
//!
//! * **Forest Village** (Riverhollow district, evening): a rain-wet cobbled plaza ringed by enormous trees
//!   that carry a whole town in their branches — stacked timber decks with houses on them, balconies,
//!   rope bridges between neighbours, strings of paper lanterns sagging across the square, market awnings
//!   and tables with villagers under them, everything wrapped in vine and lit warm against a blue dusk.
//! * **Lavarock Way** (Riverside district, afternoon): a path of flat black lava-rock slabs winding along a
//!   stream with ducks on it, a glass-domed greenhouse, timber houses stepping up a green hillside under
//!   big broadleaf trees, café umbrellas by the water, cumulus overhead. Same carpentry, same people —
//!   the houses and villagers are shared props — but in daylight.
//!
//! Child module of `scene3d` for the private prop vocabulary (`tree_platform`, `rope_bridge`,
//! `recurse_tree`, `terrain_disc`, `lazy_river`, `critter`, `contact_shadow`, `bench`).
use super::*;
use std::f32::consts::{PI, TAU};

/// A timber house: a body, a pitched roof of two slabs with a ridge beam, a lit window, a door, and a
/// lantern by the door. `face` is the yaw the door faces; `w`,`d`,`h` the body's width, depth and wall
/// height. `lit` warms the windows (evening) or leaves them clear glass (day).
fn timber_house(s: &mut Scene, base: Vec3, face: f32, w: f32, d: f32, h: f32, seed: u32, lit: bool) {
    let fwd = v(face.cos(), 0.0, face.sin());
    let right = v(-face.sin(), 0.0, face.cos());
    let up = v(0.0, 1.0, 0.0);
    let hs = |k: i32| hash(seed as i32, k);
    let wood = mix_rgb(c(0x7e, 0x5c, 0x3a), c(0x5a, 0x42, 0x2a), hs(1) * 0.6);
    let plaster = mix_rgb(c(0xd8, 0xc6, 0xa2), c(0xb0, 0x9e, 0x7e), hs(2));
    let ctr = addv(base, mulv(up, h * 0.5));
    // WALLS — four faces, the front one plastered, the rest dark timber (cheaper than a cuboid + reads as
    // half-timbered from any side). Wind each face outward so back-face culling keeps the outside.
    for (dir, side, ww, col) in [(fwd, right, w, plaster), (mulv(fwd, -1.0), mulv(right, -1.0), w, wood), (right, mulv(fwd, -1.0), d, wood), (mulv(right, -1.0), fwd, d, wood)] {
        let mut wall = m3::panel(addv(ctr, mulv(dir, if ww == w { d * 0.5 } else { w * 0.5 })), mulv(side, -1.0), up, ww, h, col);
        m3::tint_up(&mut wall, mix_rgb(col, c(0xff, 0xf0, 0xd0), 0.1), mix_rgb(col, c(0x10, 0x0c, 0x08), 0.35)); wall.mat = if col == plaster { M::STONE } else { M::WOOD }; wall.emissive = 0.02; s.add(wall, Float::STILL);
    }
    // corner posts + a sill beam
    for (a, b) in [(1.0f32, 1.0f32), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        let p = addv(base, addv(mulv(fwd, a * d * 0.5), mulv(right, b * w * 0.5)));
        let mut post = m3::tube(p, addv(p, mulv(up, h)), 0.09, 5, c(0x4e, 0x38, 0x22)); post.mat = M::WOOD; s.add(post, Float::STILL);
    }
    // ROOF — two slabs meeting at a ridge along `right`, overhanging the walls; dark shingles, moss on top
    let ridge_h = h + w * 0.42;
    let eave = addv(base, mulv(up, h - 0.05));
    for sgn in [-1.0f32, 1.0] {
        let lo = addv(eave, mulv(fwd, sgn * (d * 0.5 + 0.45)));
        let hi = addv(base, mulv(up, ridge_h));
        let slope = normv(v(hi.x - lo.x, hi.y - lo.y, hi.z - lo.z));
        let len = ((hi.x - lo.x).powi(2) + (hi.y - lo.y).powi(2) + (hi.z - lo.z).powi(2)).sqrt();
        let mid = mulv(addv(lo, hi), 0.5);
        let mut slab = m3::panel(mid, mulv(right, -sgn), slope, w + 0.7, len, c(0x5e, 0x4e, 0x40)); // winds so the TOP face survives culling
        m3::tint_up(&mut slab, mix_rgb(c(0x6e, 0x86, 0x4e), c(0x5e, 0x4e, 0x40), 0.5), c(0x42, 0x36, 0x2a)); slab.mat = M::WOOD; slab.emissive = 0.02; s.add(slab, Float::STILL);
        // gable end triangle under the roof
        let g = addv(base, mulv(right, sgn * w * 0.5));
        let mut gable = m3::tri(addv(g, addv(mulv(fwd, -d * 0.5), mulv(up, h))), addv(g, addv(mulv(fwd, d * 0.5), mulv(up, h))), addv(g, mulv(up, ridge_h)), wood);
        if sgn < 0.0 { gable.tris = vec![[0, 2, 1]]; } // wind outward on the far gable
        gable.mat = M::WOOD; s.add(gable, Float::STILL);
    }
    let mut ridge = m3::tube(addv(base, addv(mulv(up, ridge_h), mulv(right, -w * 0.5 - 0.35))), addv(base, addv(mulv(up, ridge_h), mulv(right, w * 0.5 + 0.35))), 0.09, 5, c(0x2a, 0x20, 0x14)); ridge.mat = M::WOOD; s.add(ridge, Float::STILL);
    // CHIMNEY on some houses, a wisp of smoke in the evening
    if hs(3) > 0.5 {
        let cp = addv(base, addv(mulv(right, w * 0.28), addv(mulv(fwd, -d * 0.1), mulv(up, h + w * 0.25))));
        let mut ch = m3::tube(cp, addv(cp, mulv(up, w * 0.35)), 0.16, 5, c(0x5a, 0x4a, 0x40)); ch.mat = M::STONE; s.add(ch, Float::STILL);
        if lit { let mut sm = m3::uv_sphere(addv(cp, mulv(up, w * 0.45)), 0.22, 6, 5, c(0x9a, 0x9a, 0x96)); m3::set_alpha(&mut sm, 70); sm.mat = M::WATER; sm.emissive = 0.05; s.add(sm, Float { amp: 0.3, speed: 0.5, phase: hs(4) * TAU, drift: 0.15 }); }
    }
    // DOOR + WINDOWS on the front face; a lantern on a bracket by the door
    let front = addv(ctr, mulv(fwd, d * 0.5 + 0.02));
    let mut door = m3::panel(addv(front, addv(mulv(right, -w * 0.22), mulv(up, -h * 0.5 + 0.55))), mulv(right, -1.0), up, 0.62, 1.1, c(0x2a, 0x1c, 0x10)); door.mat = M::WOOD; s.add(door, Float::STILL);
    let mut arch = m3::torus(addv(front, addv(mulv(right, -w * 0.22), mulv(up, -h * 0.5 + 1.1))), 0.34, 0.035, 10, 4, c(0x3a, 0x2a, 0x18)); arch.mat = M::WOOD; s.add(arch, Float::STILL);
    let win_col = if lit { c(0xff, 0xc8, 0x70) } else { c(0x8a, 0xa6, 0xb8) };
    for (wx, wy) in [(w * 0.24, 0.1), (w * 0.24, h * 0.5 - 0.55), (-w * 0.22, h * 0.5 - 0.55)] {
        if wy > h - 0.6 { continue; }
        let wc = addv(front, addv(mulv(right, wx), mulv(up, wy)));
        let mut frame = m3::panel(addv(wc, mulv(fwd, -0.005)), mulv(right, -1.0), up, 0.6, 0.6, c(0x2c, 0x1e, 0x10)); frame.mat = M::WOOD; s.add(frame, Float::STILL);
        let mut pane = m3::panel(addv(wc, mulv(fwd, 0.005)), mulv(right, -1.0), up, 0.48, 0.48, win_col); pane.mat = M::PAPER; pane.emissive = if lit { 0.9 } else { 0.15 }; if !lit { m3::set_alpha(&mut pane, 160); } s.add(pane, Float::STILL);
        let mut mullion = m3::tube(addv(wc, addv(mulv(fwd, 0.01), mulv(up, -0.24))), addv(wc, addv(mulv(fwd, 0.01), mulv(up, 0.24))), 0.015, 4, c(0x2c, 0x1e, 0x10)); mullion.mat = M::WOOD; s.add(mullion, Float::STILL);
        let mut flower_box = m3::leaf_cluster(addv(wc, addv(mulv(up, -0.34), mulv(fwd, 0.12))), 0.22, seed.wrapping_add(wx as u32 + 7), c(0x4a, 0x6e, 0x34)); flower_box.mat = M::MOSS; s.add(flower_box, Float { amp: 0.02, speed: 0.6, phase: hs(5) * TAU, drift: 0.02 });
    }
    if lit {
        let lp = addv(front, addv(mulv(right, w * 0.02), addv(mulv(up, -h * 0.5 + 1.5), mulv(fwd, 0.3))));
        let mut br = m3::tube(addv(lp, mulv(fwd, -0.3)), lp, 0.025, 4, c(0x2a, 0x28, 0x26)); br.mat = M::STONE; s.add(br, Float::STILL);
        caged_lantern(s, lp, 0.16, c(0xff, 0xc0, 0x66), seed.wrapping_add(99), Float { amp: 0.01, speed: 0.7, phase: hs(6) * TAU, drift: 0.015 });
    }
    contact_shadow(s, base.x, base.z, w.max(d) * 0.6, base.y, 120);
}

/// A villager: a stylised figure — a cloak (tapered tube), a head, a hood or hat, a hint of arms — in one
/// of a handful of dyed-cloth colours. `seated` folds them onto a bench height. Faces `face`.
fn villager(s: &mut Scene, at: Vec3, face: f32, seated: bool, seed: u32) {
    let fwd = v(face.cos(), 0.0, face.sin());
    let right = v(-face.sin(), 0.0, face.cos());
    let up = v(0.0, 1.0, 0.0);
    let hs = |k: i32| hash(seed as i32, k);
    let cloth = [c(0x7a, 0x4a, 0x3a), c(0x4a, 0x5e, 0x7a), c(0x6a, 0x7a, 0x44), c(0x8a, 0x6a, 0x3a), c(0x5a, 0x4a, 0x6a), c(0xa0, 0x8a, 0x60)][(seed as usize) % 6];
    let skin = mix_rgb(c(0xe2, 0xb8, 0x96), c(0x9a, 0x6e, 0x4e), hs(1));
    let hair = mix_rgb(c(0x2a, 0x1c, 0x12), c(0xb0, 0x8a, 0x50), hs(2) * hs(2));
    let hips = if seated { 0.55 } else { 0.0 };
    let torso_h = if seated { 0.75 } else { 1.35 };
    let base = addv(at, mulv(up, hips));
    let mut body = m3::tube(addv(base, mulv(up, 0.05)), addv(base, mulv(up, torso_h)), 0.26, 8, cloth);
    m3::tint_up(&mut body, mix_rgb(cloth, c(0xff, 0xf0, 0xe0), 0.15), mix_rgb(cloth, c(0x10, 0x0a, 0x08), 0.45)); body.mat = M::PAPER; s.add(body, Float::STILL);
    if seated { // knees forward
        let mut lap = m3::tube(addv(base, addv(mulv(up, 0.1), mulv(fwd, 0.05))), addv(base, addv(mulv(up, 0.12), mulv(fwd, 0.45))), 0.2, 6, mix_rgb(cloth, c(0x10, 0x0a, 0x08), 0.3)); lap.mat = M::PAPER; s.add(lap, Float::STILL);
    }
    let head_c = addv(base, mulv(up, torso_h + 0.22));
    let mut head = m3::uv_sphere(head_c, 0.19, 10, 8, skin); head.mat = M::PAPER; s.add(head, Float { amp: 0.01, speed: 0.4, phase: hs(3) * TAU, drift: 0.0 });
    if hs(4) > 0.45 { // hood
        let mut hood = m3::dome_cap_outside(addv(head_c, mulv(up, 0.02)), 0.24, 0.2, 0.0, 10, 4, cloth); hood.mat = M::PAPER; s.add(hood, Float::STILL);
    } else { // hair + maybe a hat brim
        let mut h = m3::dome_cap_outside(addv(head_c, mulv(up, 0.03)), 0.2, 0.15, 0.0, 10, 4, hair); h.mat = M::PAPER; s.add(h, Float::STILL);
        if hs(5) > 0.6 { let mut brim = m3::disc(addv(head_c, mulv(up, 0.16)), 0.32, 12, c(0x3a, 0x2c, 0x1c)); brim.mat = M::WOOD; s.add(brim, Float::STILL); }
    }
    for sgn in [-1.0f32, 1.0] { // arms: forward when seated (on the table), down when standing
        let sh = addv(base, addv(mulv(up, torso_h - 0.15), mulv(right, sgn * 0.26)));
        let hand = if seated { addv(sh, addv(mulv(fwd, 0.42), mulv(up, -0.25))) } else { addv(sh, mulv(up, -0.55)) };
        let mut arm = m3::tube(sh, hand, 0.06, 5, cloth); arm.mat = M::PAPER; s.add(arm, Float::STILL);
        let mut hd = m3::uv_sphere(hand, 0.06, 6, 5, skin); hd.mat = M::PAPER; s.add(hd, Float::STILL);
    }
    contact_shadow(s, at.x, at.z, 0.35, at.y, 100);
}

/// A round café table with `n` seated villagers around it, and a mug in front of each.
fn cafe_table(s: &mut Scene, at: Vec3, n: u32, seed: u32) {
    let mut top = m3::disc(addv(at, v(0.0, 1.0, 0.0)), 0.7, 16, c(0x4a, 0x36, 0x22)); m3::tint_up(&mut top, c(0x5e, 0x46, 0x2c), c(0x2e, 0x20, 0x12)); top.mat = M::WOOD; s.add(top, Float::STILL);
    let mut rim = m3::torus(addv(at, v(0.0, 0.96, 0.0)), 0.7, 0.04, 16, 4, c(0x3a, 0x2a, 0x18)); rim.mat = M::WOOD; s.add(rim, Float::STILL);
    let mut stem = m3::tube(at, addv(at, v(0.0, 0.96, 0.0)), 0.08, 5, c(0x3a, 0x2a, 0x18)); stem.mat = M::WOOD; s.add(stem, Float::STILL);
    let mut foot = m3::disc(addv(at, v(0.0, 0.02, 0.0)), 0.4, 10, c(0x2e, 0x22, 0x14)); foot.mat = M::WOOD; s.add(foot, Float::STILL);
    for i in 0..n {
        let a = TAU * i as f32 / n as f32 + hash(seed as i32, 1) * TAU;
        let p = addv(at, v(a.cos() * 1.15, 0.0, a.sin() * 1.15));
        let mut stool = m3::tube(addv(p, v(0.0, 0.5, 0.0)), addv(p, v(0.0, 0.56, 0.0)), 0.22, 8, c(0x3e, 0x2c, 0x1a)); stool.mat = M::WOOD; s.add(stool, Float::STILL);
        let mut leg = m3::tube(p, addv(p, v(0.0, 0.5, 0.0)), 0.05, 4, c(0x2e, 0x22, 0x14)); leg.mat = M::WOOD; s.add(leg, Float::STILL);
        villager(s, p, a + PI, true, seed.wrapping_add(i * 17));
        let mp = addv(at, v(a.cos() * 0.42, 1.0, a.sin() * 0.42));
        let mut mug = m3::tube(mp, addv(mp, v(0.0, 0.14, 0.0)), 0.06, 6, c(0xd8, 0xcc, 0xb0)); mug.mat = M::STONE; s.add(mug, Float::STILL);
    }
    contact_shadow(s, at.x, at.z, 0.8, at.y, 100);
}

/// A market awning: a striped canvas on two poles, tilted down toward the front, with a small stall table under it.
fn awning(s: &mut Scene, at: Vec3, face: f32, w: f32, stripe: Color32, seed: u32) {
    let fwd = v(face.cos(), 0.0, face.sin());
    let right = v(-face.sin(), 0.0, face.cos());
    let up = v(0.0, 1.0, 0.0);
    for sgn in [-1.0f32, 1.0] {
        let p = addv(at, addv(mulv(right, sgn * w * 0.5), mulv(fwd, 1.2)));
        let mut pole = m3::tube(p, addv(p, mulv(up, 2.2)), 0.05, 5, c(0x3a, 0x2a, 0x18)); pole.mat = M::WOOD; s.add(pole, Float::STILL);
        let pb = addv(at, addv(mulv(right, sgn * w * 0.5), mulv(fwd, -0.9)));
        let mut pole2 = m3::tube(pb, addv(pb, mulv(up, 2.7)), 0.05, 5, c(0x3a, 0x2a, 0x18)); pole2.mat = M::WOOD; s.add(pole2, Float::STILL);
    }
    let lo = addv(at, addv(mulv(fwd, 1.3), mulv(up, 2.2)));
    let hi = addv(at, addv(mulv(fwd, -1.0), mulv(up, 2.7)));
    let mid = mulv(addv(lo, hi), 0.5);
    let slope = normv(v(hi.x - lo.x, hi.y - lo.y, hi.z - lo.z));
    let nstripe = (w / 0.5) as u32;
    for i in 0..nstripe { // alternating stripes of the dyed cloth and cream
        let x = (i as f32 + 0.5) / nstripe as f32 - 0.5;
        let col = if i % 2 == 0 { stripe } else { c(0xe8, 0xdc, 0xc0) };
        let mut strip = m3::panel(addv(mid, mulv(right, x * w)), mulv(right, -1.0), slope, w / nstripe as f32 + 0.01, 2.4, col);
        m3::tint_up(&mut strip, col, mix_rgb(col, c(0x20, 0x18, 0x10), 0.4)); strip.mat = M::PAPER; strip.emissive = 0.05;
        s.add(strip, Float { amp: 0.02, speed: 0.7, phase: hash(seed as i32, i as i32) * TAU, drift: 0.02 });
    }
    // scalloped valance along the front edge
    for i in 0..(w / 0.3) as u32 {
        let x = (i as f32 + 0.5) / (w / 0.3) - 0.5;
        let mut sc = m3::uv_sphere(addv(lo, addv(mulv(right, x * w), mulv(up, -0.08))), 0.14, 6, 5, stripe); sc.mat = M::PAPER; s.add(sc, Float::STILL);
    }
    // stall table with wares — a few crates and baskets
    let tb = addv(at, mulv(fwd, 0.6));
    let mut table = m3::panel(addv(tb, mulv(up, 0.9)), right, fwd, w * 0.8, 1.0, c(0x5a, 0x42, 0x28)); table.mat = M::WOOD; s.add(table, Float::STILL);
    for sgn in [-1.0f32, 1.0] { let l = addv(tb, mulv(right, sgn * w * 0.36)); let mut leg = m3::tube(l, addv(l, mulv(up, 0.9)), 0.05, 4, c(0x3a, 0x2a, 0x18)); leg.mat = M::WOOD; s.add(leg, Float::STILL); }
    for k in 0..3 {
        let p = addv(tb, addv(mulv(right, (k as f32 - 1.0) * w * 0.26), mulv(up, 1.05)));
        let col = [c(0xc8, 0x8a, 0x40), c(0x8a, 0xa0, 0x50), c(0xb0, 0x60, 0x50)][k];
        let mut basket = m3::tube(addv(p, mulv(up, -0.12)), p, 0.18, 8, c(0x9a, 0x7a, 0x4a)); basket.mat = M::WOOD; s.add(basket, Float::STILL);
        let mut fill = m3::uv_sphere(p, 0.16, 7, 5, col); fill.mat = M::MOSS; fill.emissive = 0.04; s.add(fill, Float::STILL);
    }
    contact_shadow(s, at.x, at.z, w * 0.5, at.y, 90);
}

/// A sagging string of paper lanterns between two anchor points — the evening plaza's lighting.
fn lantern_string(s: &mut Scene, a: Vec3, b: Vec3, n: u32, seed: u32) {
    let segs = n * 2;
    let mut prev = a;
    for i in 1..=segs {
        let t = i as f32 / segs as f32;
        let sag = (t * PI).sin() * 1.1;
        let p = v(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t - sag, a.z + (b.z - a.z) * t);
        let mut wire = m3::tube(prev, p, 0.018, 4, c(0x1c, 0x18, 0x14)); wire.mat = M::WOOD; s.add(wire, Float { amp: 0.02, speed: 0.4, phase: hash(seed as i32, 1) * TAU, drift: 0.02 });
        if i % 2 == 1 {
            let col = [c(0xff, 0xd0, 0x80), c(0xff, 0xb8, 0x66), c(0xf6, 0xe0, 0xa0), c(0xff, 0x9a, 0x6a)][((seed + i) as usize) % 4];
            let lc = addv(p, v(0.0, -0.28, 0.0));
            let sway = Float { amp: 0.02, speed: 0.5 + hash(seed as i32, i as i32) * 0.3, phase: hash(seed as i32, i as i32 + 9) * TAU, drift: 0.04 };
            let mut cord = m3::tube(p, addv(lc, v(0.0, 0.16, 0.0)), 0.012, 4, c(0x1c, 0x18, 0x14)); cord.mat = M::WOOD; s.add(cord, sway);
            let mut body = m3::uv_sphere(v(0.0, 0.0, 0.0), 0.17, 10, 7, col); m3::scale3(&mut body, 1.0, 0.85, 1.0); for q in &mut body.verts { *q = addv(*q, lc); }
            body.mat = M::PAPER; body.emissive = 1.0; s.add(body, sway);
            let mut core = m3::uv_sphere(lc, 0.07, 6, 5, c(0xff, 0xf4, 0xd8)); core.mat = M::MOSS; core.emissive = 1.7; s.add(core, sway);
        }
        prev = p;
    }
}

/// A big broadleaf tree: a stout trunk that forks into a recursive crown. `leaf` is the crown colour.
fn broadleaf(s: &mut Scene, base: Vec3, h: f32, girth: f32, seed: i32, bark_lo: Color32, bark_hi: Color32, leaf_lo: Color32, leaf_hi: Color32) {
    let lean = v((hash(seed, 1) - 0.5) * 0.16, 1.0, (hash(seed, 2) - 0.5) * 0.16);
    recurse_tree(s, base, normv(lean), h, girth, 4, seed, bark_lo, bark_hi, leaf_lo, leaf_hi);
    // buttress roots at the foot, so the trunk grows out of the ground rather than standing on it
    for k in 0..5 {
        let a = TAU * k as f32 / 5.0 + hash(seed, 3 + k) * 0.6;
        let mut root = m3::tube(addv(base, v(0.0, girth * 0.8, 0.0)), addv(base, v(a.cos() * girth * 2.6, -0.1, a.sin() * girth * 2.6)), girth * 0.42, 5, bark_lo);
        m3::tint_up(&mut root, bark_hi, bark_lo); root.mat = M::WOOD; s.add(root, Float::STILL);
    }
    contact_shadow(s, base.x, base.z, girth * 2.4, base.y, 110);
}

/// A duck on the water: a squashed body, a head, an orange bill, bobbing with the ripples.
fn duck(s: &mut Scene, at: Vec3, face: f32, seed: u32) {
    let fwd = v(face.cos(), 0.0, face.sin());
    let bob = Float { amp: 0.03, speed: 1.1, phase: hash(seed as i32, 1) * TAU, drift: 0.05 };
    let (body_col, head_col) = if seed % 3 == 0 { (c(0x8a, 0x6e, 0x4a), c(0x2a, 0x5a, 0x3a)) } else { (c(0xa4, 0x8e, 0x6a), c(0x7a, 0x62, 0x44)) };
    let mut body = m3::uv_sphere(v(0.0, 0.0, 0.0), 0.36, 10, 8, body_col); m3::scale3(&mut body, 1.3, 0.6, 0.8);
    for p in &mut body.verts { *p = addv(addv(at, mulv(fwd, p.x)), v(-fwd.z * p.z, p.y + 0.12, fwd.x * p.z)); }
    body.mat = M::MOSS; s.add(body, bob);
    let hc = addv(at, addv(mulv(fwd, 0.36), v(0.0, 0.5, 0.0)));
    let mut neck = m3::tube(addv(at, addv(mulv(fwd, 0.3), v(0.0, 0.2, 0.0))), hc, 0.08, 5, head_col); neck.mat = M::MOSS; s.add(neck, bob);
    let mut head = m3::uv_sphere(hc, 0.15, 8, 6, head_col); head.mat = M::MOSS; s.add(head, bob);
    let mut bill = m3::tube(addv(hc, mulv(fwd, 0.1)), addv(hc, mulv(fwd, 0.3)), 0.05, 4, c(0xe0, 0x90, 0x30)); bill.mat = M::MOSS; s.add(bill, bob);
    let mut wake = m3::disc(addv(at, v(0.0, 0.02, 0.0)), 0.55, 10, c(0xd0, 0xe8, 0xea)); m3::set_alpha(&mut wake, 40); wake.mat = M::WATER; wake.emissive = 0.1; s.add(wake, bob);
}

/// A barrel: staves + two iron hoops.
fn barrel(s: &mut Scene, at: Vec3, seed: u32) {
    let mut body = m3::tube(at, addv(at, v(0.0, 1.0, 0.0)), 0.36, 10, c(0x5a, 0x42, 0x28)); m3::tint_up(&mut body, c(0x6e, 0x54, 0x36), c(0x3a, 0x2a, 0x18)); body.mat = M::WOOD; s.add(body, Float::STILL);
    for y in [0.22, 0.78] { let mut hoop = m3::torus(addv(at, v(0.0, y, 0.0)), 0.36, 0.025, 10, 4, c(0x2c, 0x2a, 0x28)); hoop.mat = M::STONE; s.add(hoop, Float::STILL); }
    let mut lid = m3::disc(addv(at, v(0.0, 1.0, 0.0)), 0.34, 10, c(0x4a, 0x36, 0x20)); lid.mat = M::WOOD; s.add(lid, Float::STILL);
    contact_shadow(s, at.x, at.z, 0.4, at.y, 90 + (seed % 20) as u8);
}

// ═══════════════════════════════════ FOREST VILLAGE (Riverhollow, evening) ═══════════════════════════════════

pub(super) fn forest_village() -> Scene {
    let mut s = Scene::new();
    let pr = env_params();
    let yf = -3.2_f32;
    let sc = (map_radius() / 64.0).clamp(1.0, 2.0);
    let rr = (34.0 * sc).max((pr.ext_x.max(pr.ext_z) as f32) * 0.62 + 14.0);
    let q = render_quality() * pr.prop_density.clamp(0.4, 2.0);
    let hs = |k: i32, j: i32| hash(k ^ (pr.seed as i32 & 0xffff), j);
    let (bark_lo, bark_hi) = (c(0x4e, 0x3a, 0x26), c(0x86, 0x6a, 0x46));
    let (leaf_lo, leaf_hi) = (c(0x2e, 0x4e, 0x3c), c(0x6e, 0x9a, 0x56)); // dusk-blue greens
    let warm = c(0xff, 0xc4, 0x6e);

    // THE PLAZA — wet cobbles: a dark blue-grey ground with a little relief, cobble discs scattered darker and
    // lighter, and puddles mirroring the lanterns. A lane of paler flags leads out the back.
    // (albedo two stops lighter than the reference looks — this renderer takes anything under ~0x40 to black)
    terrain_disc(&mut s, rr * 1.15, yf, pr.seed ^ 0xE1B, 0.3, 0.07, c(0x3e, 0x44, 0x4e), c(0x62, 0x68, 0x72), M::STONE);
    for k in 0..(160.0 * q) as i32 {
        let a = hs(k, 1) * TAU; let d = hs(k, 2).sqrt() * rr * 0.95;
        let shade = hs(k, 3);
        let col = mix_rgb(c(0x44, 0x48, 0x52), c(0x76, 0x78, 0x80), shade);
        let mut cob = m3::disc(v(a.cos() * d, yf + 0.02, a.sin() * d), (0.35 + hs(k, 4) * 0.35) * sc, 7, col);
        m3::scale_xz(&mut cob, 1.0, 0.7 + hs(k, 5) * 0.5); cob.mat = M::STONE; cob.emissive = 0.02; s.add(cob, Float::STILL);
    }
    for k in 0..(14.0 * q) as i32 { // puddles
        let a = hs(k, 10) * TAU; let d = hs(k, 11).sqrt() * rr * 0.9;
        let mut pud = m3::disc(v(a.cos() * d, yf + 0.03, a.sin() * d), (0.8 + hs(k, 12) * 1.6) * sc, 14, c(0x5a, 0x70, 0x88));
        m3::scale_xz(&mut pud, 1.0, 0.5 + hs(k, 13) * 0.5); m3::set_alpha(&mut pud, 150); pud.mat = M::WATER; pud.emissive = 0.14; s.add(pud, Float::STILL);
    }
    for i in 0..(rr / (1.6 * sc)) as i32 { // the flagged lane, out the back and slightly curving
        let t = i as f32 / (rr / (1.6 * sc));
        let z = -rr * 0.15 - t * rr * 1.0; let x = (t * 3.0).sin() * rr * 0.08;
        let mut flag = m3::panel(v(x, yf + 0.03, z), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0), 2.2 * sc, 1.3 * sc, c(0x74, 0x78, 0x80));
        m3::tint_up(&mut flag, c(0x8a, 0x8e, 0x96), c(0x5a, 0x5e, 0x66)); flag.mat = M::STONE; flag.emissive = 0.03; s.add(flag, Float::STILL);
    }

    // THE GREAT TREES — a ring of them at the plaza's edge, each carrying two or three decks with houses, the
    // decks of neighbours joined by rope bridges. How many trees grows with the conversation.
    // ENVPARAMS shape the town. One great tree per conversation BRANCH, standing at that branch's bearing
    // (`bear`), its height from the branch's depth and the conversation's tree character (`tree_size` ×
    // `tree_height`), its girth from `tree_girth`, its number of decks from the branch's depth, its house
    // from the branch's token mass. Extra trees fill the ring when the talk has fewer branches than the
    // square needs to read as enclosed. `prop_density` scales the market and the lantern strings.
    let (hf, gf) = (pr.tree_size * pr.tree_height, pr.tree_size * pr.tree_girth);
    let ntree = ((pr.branches as i32).max(6) + (pr.nodes / 60) as i32).clamp(6, 10);
    let mut decks: Vec<Vec3> = Vec::new();
    for k in 0..ntree {
        let a = branch_bearing(&pr, k as usize, ntree as usize, 0x7E1) + hs(k, 20) * 0.1;
        let shape = pr.shape.get(k as usize).copied().filter(|sh| sh.len > 0);
        let d = rr * (0.92 + hs(k, 21) * 0.12);
        let base = v(a.cos() * d, yf, a.sin() * d);
        let depth_f = shape.map(|sh| (sh.depth as f32 / 24.0).clamp(0.0, 1.0)).unwrap_or(hs(k, 22));
        let h = (20.0 + depth_f * 12.0) * sc * hf.clamp(0.6, 1.6);
        let girth = (1.5 + hs(k, 23) * 0.5) * sc * gf.clamp(0.6, 1.6);
        broadleaf(&mut s, base, h, girth, 300 + k, bark_lo, bark_hi, leaf_lo, leaf_hi);
        let ndeck = match shape { Some(sh) => (1 + sh.depth as i32 / 6).clamp(1, 3), None => 2 + (hs(k, 24) > 0.5) as i32 };
        let mass = shape.map(|sh| (sh.tokens64 as f32 / 40.0).clamp(0.0, 1.0)).unwrap_or(hs(k, 26));
        for j in 0..ndeck {
            let y = yf + (6.0 + j as f32 * 6.5 + hs(k, 25 + j) * 2.0) * sc;
            let dr = (3.4 + hs(k, 30 + j) * 1.2) * sc;
            // the deck sits on the inward side of the trunk, over the square
            let dc = v(base.x - a.cos() * dr * 0.7, y, base.z - a.sin() * dr * 0.7);
            tree_platform(&mut s, dc.x, dc.z, dc.y, dr, (400 + k * 7 + j) as u32);
            let hw = (2.0 + mass * 1.2 + hs(k, 40 + j) * 0.4) * sc;
            timber_house(&mut s, v(dc.x + a.cos() * dr * 0.25, dc.y, dc.z + a.sin() * dr * 0.25), a + PI + (hs(k, 50 + j) - 0.5) * 0.5, hw, hw * 0.8, 2.3 * sc, (500 + k * 11 + j) as u32, true);
            // a balcony lantern + a villager leaning on the rail
            if hs(k, 60 + j) > 0.4 { villager(&mut s, v(dc.x - a.cos() * (dr - 0.6), dc.y, dc.z - a.sin() * (dr - 0.6)), a + PI, false, (600 + k * 13 + j) as u32); }
            if j == 1 { decks.push(dc); }
            // struts down to the trunk
            for st in 0..3 {
                let sa = a + PI + (st as f32 - 1.0) * 0.7;
                let mut strut = m3::tube(v(dc.x + sa.cos() * dr * 0.9, dc.y - 0.2, dc.z + sa.sin() * dr * 0.9), v(base.x, dc.y - 4.0 * sc, base.z), 0.12, 5, bark_lo); strut.mat = M::WOOD; s.add(strut, Float::STILL);
            }
        }
    }
    for i in 0..decks.len() { // rope bridges between neighbouring mid-height decks
        if decks.len() < 2 { break; }
        let (a, b) = (decks[i], decks[(i + 1) % decks.len()]);
        if hs(i as i32, 70) < 0.3 { continue; } // not every pair — a gap or two reads as a real town
        rope_bridge(&mut s, a, b, 700 + i as u32);
    }

    // LANTERN STRINGS across the square between the trees, at two heights, and hung low around the market
    for k in 0..ntree {
        let a0 = branch_bearing(&pr, k as usize, ntree as usize, 0x7E1) + hs(k, 20) * 0.1;
        let a1 = branch_bearing(&pr, ((k + 2) % ntree) as usize, ntree as usize, 0x7E1) + hs((k + 2) % ntree, 20) * 0.1;
        let (d0, d1) = (rr * 0.86, rr * 0.86);
        let y = yf + (7.0 + hs(k, 80) * 3.0) * sc;
        lantern_string(&mut s, v(a0.cos() * d0, y, a0.sin() * d0), v(a1.cos() * d1, y + 0.6, a1.sin() * d1), (6.0 + q * 4.0) as u32, 800 + k as u32);
    }
    let nposts = 8;
    for k in 0..nposts { // low posts around the market ring, strung between
        let a = TAU * k as f32 / nposts as f32 + 0.2;
        let d = rr * 0.6;
        let p = v(a.cos() * d, yf, a.sin() * d);
        let mut post = m3::tube(p, addv(p, v(0.0, 3.6 * sc, 0.0)), 0.09, 5, c(0x3a, 0x2a, 0x18)); post.mat = M::WOOD; s.add(post, Float::STILL);
        let a2 = TAU * ((k + 1) % nposts) as f32 / nposts as f32 + 0.2;
        lantern_string(&mut s, addv(p, v(0.0, 3.5 * sc, 0.0)), v(a2.cos() * d, yf + 3.5 * sc, a2.sin() * d), 3, 900 + k as u32);
    }

    // MARKET — awnings and café tables with villagers around the plaza's inner ring, barrels and crates,
    // a cat under a table
    let nstall = ((3 + pr.branches as i32) as f32 * pr.prop_density.clamp(0.5, 1.5)).clamp(3.0, 7.0) as i32;
    for k in 0..nstall {
        let a = TAU * k as f32 / nstall as f32 + 0.5 + hs(k, 90) * 0.3;
        let d = rr * 0.66;
        let stripe = [c(0x8a, 0x3e, 0x3a), c(0x3e, 0x5a, 0x7a), c(0x6a, 0x7a, 0x3a), c(0x8a, 0x6a, 0x30)][(k as usize) % 4];
        awning(&mut s, v(a.cos() * d, yf, a.sin() * d), a + PI, (3.0 + hs(k, 91)) * sc, stripe, 1000 + k as u32);
        villager(&mut s, v(a.cos() * (d - 2.6), yf, a.sin() * (d - 2.6)), a, false, 1100 + k as u32); // a customer at the stall
    }
    let ntable = (3.0 + 3.0 * q) as i32;
    for k in 0..ntable {
        let a = TAU * (k as f32 + 0.5) / ntable as f32 + hs(k, 95) * 0.3;
        let d = rr * (0.46 + hs(k, 96) * 0.1);
        cafe_table(&mut s, v(a.cos() * d, yf, a.sin() * d), 2 + (hs(k, 97) > 0.5) as u32, 1200 + k as u32 * 5);
    }
    for k in 0..(6.0 * q) as i32 {
        let a = hs(k, 100) * TAU; let d = rr * (0.74 + hs(k, 101) * 0.14);
        barrel(&mut s, v(a.cos() * d, yf, a.sin() * d), 1300 + k as u32);
    }
    critter(&mut s, 1, v(rr * 0.3, yf, rr * 0.35), 2.2, 1400);
    critter(&mut s, 1, v(-rr * 0.52, yf, rr * 0.2), -0.6, 1401);

    // VINES + PLANTERS everywhere a wall meets the ground, ferns in the corners
    for k in 0..(40.0 * q) as i32 {
        let a = hs(k, 110) * TAU; let d = rr * (0.7 + hs(k, 111) * 0.3);
        let mut fern = m3::leaf_cluster(v(a.cos() * d, yf + 0.3 * sc, a.sin() * d), (0.3 + hs(k, 112) * 0.3) * sc, 1500 + k as u32, c(0x44, 0x66, 0x44));
        m3::tint_up(&mut fern, c(0x6e, 0x96, 0x56), c(0x2e, 0x44, 0x2c)); fern.mat = M::MOSS; fern.emissive = 0.04; s.add(fern, Float { amp: 0.04, speed: 0.5, phase: hs(k, 113) * TAU, drift: 0.05 });
    }
    for k in 0..(24.0 * q) as i32 { firefly(&mut s, v((hs(k, 120) - 0.5) * rr * 1.4, yf + 1.5 + hs(k, 121) * 6.0 * sc, (hs(k, 122) - 0.5) * rr * 1.4), 1600 + k as u32); }

    // CANOPY — the trees' crowns already roof the square; add a high, dense layer with gaps so the dusk sky
    // shows through in places, and a couple of the tallest crowns lit from below by the lanterns
    for k in 0..(26.0 * q) as i32 {
        let a = hs(k, 130) * TAU; let d = hs(k, 131) * rr * 0.9;
        let y = yf + (22.0 + hs(k, 132) * 10.0) * sc;
        let mut crown = m3::leaf_cluster(v(a.cos() * d, y, a.sin() * d), (5.0 + hs(k, 133) * 5.0) * sc, 1700 + k as u32, leaf_lo);
        m3::tint_up(&mut crown, leaf_hi, c(0x26, 0x3c, 0x2e)); crown.mat = M::MOSS; crown.emissive = if hs(k, 134) > 0.7 { 0.1 } else { 0.03 };
        s.add(crown, Float { amp: 0.1, speed: 0.4, phase: hs(k, 135) * TAU, drift: 0.12 });
    }
    // low mist over the wet stones and a soft warm haze under the lantern strings
    for k in 0..8 {
        let mut mist = m3::disc(v((hs(k, 140) - 0.5) * rr * 1.2, yf + 0.4 + hs(k, 141) * 0.5, (hs(k, 142) - 0.5) * rr * 1.2), (4.0 + hs(k, 143) * 5.0) * sc, 14, c(0x6a, 0x78, 0x8a));
        mist.mat = M::WATER; mist.emissive = 0.05; m3::set_alpha(&mut mist, 20); s.add(mist, Float { amp: 0.12, speed: 0.1, phase: hs(k, 144) * TAU, drift: 0.3 });
    }
    let mut haze = m3::disc(v(0.0, yf + 6.5 * sc, 0.0), rr * 0.7, 24, warm); m3::set_alpha(&mut haze, 14); haze.mat = M::MOSS; haze.emissive = 0.4; s.add(haze, Float::STILL);
    organic_boundary(&mut s, rr * 1.3, yf, 9100, c(0x3e, 0x4a, 0x3c), c(0x62, 0x76, 0x58), M::MOSS, leaf_hi, true, sc);
    s
}

// ═══════════════════════════════════ LAVAROCK WAY (Riverside, afternoon) ═══════════════════════════════════

pub(super) fn lavarock_way() -> Scene {
    let mut s = Scene::new();
    let pr = env_params();
    let yf = -3.3_f32;
    let sc = (map_radius() / 64.0).clamp(1.0, 2.0);
    let rr = (38.0 * sc).max((pr.ext_x.max(pr.ext_z) as f32) * 0.62 + 16.0);
    // ENVPARAMS: one house per conversation BRANCH up the hillside, its place along the hill from the branch's
    // bearing and its terrace from the branch's depth; trees sized by the conversation's tree character;
    // ducks by `nodes`; the café, planters and cloud by `prop_density`.
    let q = render_quality() * pr.prop_density.clamp(0.4, 2.0);
    let (hf, gf) = (pr.tree_size * pr.tree_height, pr.tree_size * pr.tree_girth);
    let hs = |k: i32, j: i32| hash(k ^ (pr.seed as i32 & 0xffff), j);
    let (bark_lo, bark_hi) = (c(0x5e, 0x48, 0x30), c(0x9a, 0x7e, 0x56));
    let (leaf_lo, leaf_hi) = (c(0x4a, 0x80, 0x34), c(0xa4, 0xd8, 0x60)); // sunlit greens
    let grass = c(0x4e, 0x76, 0x30);

    // GROUND — a sunlit green with the lava path and the stream cut through it
    let g_seed = pr.seed ^ 0x1A7A;
    terrain_disc(&mut s, rr * 1.2, yf, g_seed, 0.25, 0.06, c(0x2e, 0x4c, 0x20), c(0x58, 0x7e, 0x38), M::GRASS);
    let gy = |x: f32, z: f32| yf + relief(x, z, g_seed, 0.25, 0.06); // props sit ON the relief, not under it
    // THE LAVA-ROCK PATH — flat black basalt slabs, irregular, with pale grout and grass between, winding from
    // the near edge past the graph and up toward the houses at the back
    let path = |t: f32| v((t * 2.2 + 0.4).sin() * rr * 0.22 + rr * 0.1, yf, rr * 0.95 - t * rr * 1.85);
    let nslab = (rr * 1.9 / (2.0 * sc)) as i32;
    for i in 0..nslab {
        let t = (i as f32 + 0.5) / nslab as f32;
        for side in [-1.0f32, 0.0, 1.0] {
            if side != 0.0 && hs(i, 1 + side as i32) < 0.35 { continue; } // the path is two to three slabs wide
            let p0 = path(t); let p1 = path(t + 0.02);
            let along = normv(v(p1.x - p0.x, 0.0, p1.z - p0.z));
            let across = v(-along.z, 0.0, along.x);
            let ctr = addv(p0, mulv(across, side * 2.1 * sc));
            let r = (1.1 + hs(i, 3 + side as i32) * 0.5) * sc;
            let mut slab = m3::rock(v(0.0, 0.0, 0.0), r, (2000 + i * 3 + side as i32) as u32, c(0x46, 0x44, 0x42));
            m3::scale3(&mut slab, 1.0, 0.16, 0.9); for p in &mut slab.verts { *p = addv(*p, v(ctr.x, gy(ctr.x, ctr.z) + 0.14, ctr.z)); }
            m3::tint_up(&mut slab, c(0x6a, 0x66, 0x62), c(0x34, 0x32, 0x30)); slab.mat = M::STONE; slab.emissive = 0.02; s.add(slab, Float::STILL);
            let mut grout = m3::disc(v(ctr.x, gy(ctr.x, ctr.z) + 0.06, ctr.z), r * 1.25, 8, c(0x9a, 0x92, 0x7e)); grout.mat = M::STONE; s.add(grout, Float::STILL);
        }
        if i % 3 == 0 { let (tx, tz) = (path(t).x + (hs(i, 6) - 0.5) * 6.0 * sc, path(t).z); grass_tuft(&mut s, tx, tz, gy(tx, tz), 2100 + i as u32, grass); }
    }

    // THE STREAM — along the left of the path, with a stony bank, ducks, and a little wooden footbridge
    let (sx0, sz0, sx1, sz1) = (-rr * 0.34, rr * 1.1, -rr * 0.16, -rr * 1.1);
    let mut bed = m3::panel(v((sx0 + sx1) * 0.5, yf - 0.12, 0.0), normv(v(sx1 - sx0, 0.0, sz1 - sz0)), v(0.0, 0.0, 1.0), rr * 2.3, 7.0 * sc, c(0x5a, 0x60, 0x58));
    bed.mat = M::STONE; s.add(bed, Float::STILL);
    lazy_river(&mut s, sx0, sz0, sx1, sz1, pr.seed as i32 ^ 77, sc, yf - 0.06);
    for k in 0..(40.0 * q) as i32 { // bank stones
        let t = hs(k, 10); let side = if hs(k, 11) > 0.5 { 1.0 } else { -1.0 };
        let (bx, bz) = (sx0 + (sx1 - sx0) * t + side * (2.6 + hs(k, 12) * 1.4) * sc, sz0 + (sz1 - sz0) * t);
        let mut st = m3::rock(v(bx, yf - 0.1, bz), (0.3 + hs(k, 13) * 0.6) * sc, 2200 + k as u32, c(0x8a, 0x86, 0x7a));
        m3::tint_up(&mut st, c(0xb0, 0xac, 0xa0), c(0x5a, 0x58, 0x50)); st.mat = M::STONE; s.add(st, Float::STILL);
    }
    for k in 0..(3 + (pr.nodes / 60).min(3)) as i32 {
        let t = 0.25 + hs(k, 15) * 0.5;
        duck(&mut s, v(sx0 + (sx1 - sx0) * t + (hs(k, 16) - 0.5) * 2.0 * sc, yf, sz0 + (sz1 - sz0) * t), hs(k, 17) * TAU, 2300 + k as u32);
    }
    { // footbridge over the stream where the path comes closest to it
        let t = 0.42; let bc = v(sx0 + (sx1 - sx0) * t, yf, sz0 + (sz1 - sz0) * t);
        for i in 0..9 {
            let x = (i as f32 - 4.0) * 0.8 * sc;
            let y = 0.9 * sc - (x / (3.5 * sc)).powi(2) * 0.9 * sc;
            let mut plank = m3::panel(addv(bc, v(x, y, 0.0)), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0), 0.7 * sc, 2.4 * sc, c(0x6e, 0x54, 0x36)); plank.mat = M::WOOD; s.add(plank, Float::STILL);
        }
        for sgn in [-1.0f32, 1.0] {
            let mut rail = m3::tube(addv(bc, v(-3.4 * sc, 1.1 * sc, sgn * 1.1 * sc)), addv(bc, v(3.4 * sc, 1.1 * sc, sgn * 1.1 * sc)), 0.05, 4, c(0x4a, 0x36, 0x20)); rail.mat = M::WOOD; s.add(rail, Float::STILL);
            for i in 0..4 { let x = (i as f32 - 1.5) * 2.0 * sc; let p = addv(bc, v(x, 0.8 * sc, sgn * 1.1 * sc)); let mut post = m3::tube(p, addv(p, v(0.0, 0.5 * sc, 0.0)), 0.05, 4, c(0x4a, 0x36, 0x20)); post.mat = M::WOOD; s.add(post, Float::STILL); }
        }
    }

    // THE GREENHOUSE — a glass dome on a low stone wall, ribs of copper, plants inside, on the left bank
    let gh = v(-rr * 0.62, yf, rr * 0.28);
    let gr = 7.0 * sc;
    let mut wall = m3::wall_ring(gh, gr, 1.6 * sc, 24, c(0x9a, 0x92, 0x84)); m3::tint_height(&mut wall, yf, yf + 1.6 * sc, c(0x6a, 0x66, 0x5c), c(0xb8, 0xb0, 0xa0)); wall.mat = M::STONE; s.add(wall, Float::STILL);
    let mut glass = m3::dome_cap_outside(addv(gh, v(0.0, 1.6 * sc, 0.0)), gr, gr * 0.75, 0.0, 28, 8, c(0xc8, 0xe4, 0xec));
    m3::set_alpha(&mut glass, 96); glass.mat = M::WATER; glass.emissive = 0.22; s.add(glass, Float::STILL);
    for k in 0..12 { // meridian ribs
        let a = TAU * k as f32 / 12.0;
        let mut prev = addv(gh, v(a.cos() * gr, 1.6 * sc, a.sin() * gr));
        for i in 1..=6 {
            let t = i as f32 / 6.0 * PI * 0.5;
            let p = addv(gh, v(a.cos() * gr * t.cos(), 1.6 * sc + gr * 0.75 * t.sin(), a.sin() * gr * t.cos()));
            let mut rib = m3::tube(prev, p, 0.06, 4, c(0x8a, 0x5e, 0x3a)); rib.mat = M::STONE; rib.emissive = 0.05; s.add(rib, Float::STILL);
            prev = p;
        }
    }
    let mut ring = m3::torus(addv(gh, v(0.0, 1.6 * sc, 0.0)), gr, 0.07, 28, 4, c(0x8a, 0x5e, 0x3a)); ring.mat = M::STONE; s.add(ring, Float::STILL);
    let mut finial = m3::uv_sphere(addv(gh, v(0.0, 1.6 * sc + gr * 0.75 + 0.3, 0.0)), 0.3, 8, 6, c(0xb0, 0x80, 0x48)); finial.mat = M::STONE; finial.emissive = 0.2; s.add(finial, Float::STILL);
    for k in 0..(12.0 * q) as i32 { // plants inside, tall ones in the middle
        let a = hs(k, 20) * TAU; let d = hs(k, 21) * gr * 0.8;
        let h = (1.0 + (1.0 - d / gr) * 3.0) * sc;
        let p = addv(gh, v(a.cos() * d, 0.0, a.sin() * d));
        let mut stem = m3::tube(p, addv(p, v(0.0, h, 0.0)), 0.06, 4, c(0x3a, 0x5a, 0x28)); stem.mat = M::MOSS; s.add(stem, Float::STILL);
        let mut lf = m3::leaf_cluster(addv(p, v(0.0, h, 0.0)), (0.7 + hs(k, 22) * 0.6) * sc, 2400 + k as u32, leaf_lo); m3::tint_up(&mut lf, leaf_hi, leaf_lo); lf.mat = M::MOSS; lf.emissive = 0.06; s.add(lf, Float { amp: 0.03, speed: 0.5, phase: hs(k, 23) * TAU, drift: 0.03 });
    }

    // THE HILLSIDE — rising to the right and the back, terraced, with houses stepping up it under big trees
    let hill_c = v(rr * 0.55, yf - 2.0, -rr * 0.55);
    for k in 0..6 { // terraces as flattened rocks of decreasing size
        let r = rr * (0.6 - k as f32 * 0.08);
        let mut ter = m3::rock(v(0.0, 0.0, 0.0), r, 2500 + k as u32, c(0x3e, 0x60, 0x2a));
        m3::scale3(&mut ter, 1.0 + (hs(k, 90) - 0.5) * 0.3, 0.16 + k as f32 * 0.02, 1.0 + (hs(k, 91) - 0.5) * 0.3); for p in &mut ter.verts { *p = addv(*p, addv(hill_c, v((hs(k, 92) - 0.5) * 6.0 * sc, 2.0 + k as f32 * 2.6 * sc, (hs(k, 93) - 0.5) * 6.0 * sc))); }
        m3::tint_up(&mut ter, c(0x6e, 0x96, 0x46), c(0x2e, 0x48, 0x20)); ter.mat = M::MOSS; ter.emissive = 0.02; s.add(ter, Float::STILL);
    }
    let nhouse = ((pr.branches as i32).max(4) + (pr.nodes / 80) as i32).clamp(4, 9);
    for k in 0..nhouse {
        let shape = pr.shape.get(k as usize).copied().filter(|sh| sh.len > 0);
        // the branch's bearing, folded onto the hill's arc (the hill spans ~1.6 rad centred on -3π/4)
        let bearing = branch_bearing(&pr, k as usize, nhouse as usize, 0x1A7);
        let along = ((bearing / TAU) * 2.0).rem_euclid(1.0); // 0..1 twice round, so neighbours interleave
        let tier = shape.map(|sh| (sh.depth as f32 / 8.0).clamp(0.0, 3.0).floor()).unwrap_or((k % 4) as f32);
        let a = -PI * 0.75 + (along - 0.5) * 1.6 + (hs(k, 30) - 0.5) * 0.2;
        let d = rr * (0.55 - tier * 0.08) + (hs(k, 31) - 0.5) * 3.0 * sc;
        let base = v(hill_c.x + a.cos() * d, hill_c.y + 2.0 + tier * 2.6 * sc + (0.16 + tier * 0.02) * rr * (0.6 - tier * 0.08) * 0.9, hill_c.z + a.sin() * d);
        let face = (hill_c.x - base.x).atan2(base.z - hill_c.z) + PI * 0.5; // door faces down the hill toward the square
        let mass = shape.map(|sh| (sh.tokens64 as f32 / 40.0).clamp(0.0, 1.0)).unwrap_or(hs(k, 32));
        let w = (2.4 + mass * 1.6) * sc;
        timber_house(&mut s, base, face, w, w * 0.85, (2.4 + hs(k, 33) * 0.6) * sc, 2600 + k as u32, false);
        if hs(k, 34) > 0.5 { broadleaf(&mut s, addv(base, v(w * 1.2, 0.0, -w * 0.4)), (9.0 + hs(k, 35) * 5.0) * sc, 0.5 * sc, 2700 + k, bark_lo, bark_hi, leaf_lo, leaf_hi); }
    }
    // stone steps up the first terrace where the path arrives
    for i in 0..6 {
        let p = path(0.92 + i as f32 * 0.015);
        let mut step = m3::panel(v(p.x, yf + 0.25 + i as f32 * 0.45 * sc, p.z - i as f32 * 0.6 * sc), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0), 4.0 * sc, 0.7 * sc, c(0x9a, 0x92, 0x84)); m3::tint_up(&mut step, c(0xc0, 0xb8, 0xa8), c(0x6a, 0x64, 0x58)); step.mat = M::STONE; s.add(step, Float::STILL);
    }

    // BIG TREES around the square and along the stream, sun-green
    let ntree = (6 + pr.nodes / 40).clamp(6, 11) as i32;
    for k in 0..ntree {
        let a = TAU * k as f32 / ntree as f32 + hs(k, 40) * 0.3;
        let da = ((a - 5.5) % TAU + TAU) % TAU; // bearing from the hill's centre (which sits at ~5.5 rad)
        if da < 1.0 || da > TAU - 1.0 { continue; } // leave the hillside quadrant to the houses
        let d = rr * (0.8 + hs(k, 41) * 0.25);
        broadleaf(&mut s, v(a.cos() * d, yf, a.sin() * d), (14.0 + hs(k, 42) * 8.0) * sc * hf.clamp(0.6, 1.6), (0.9 + hs(k, 43) * 0.5) * sc * gf.clamp(0.6, 1.6), 2800 + k, bark_lo, bark_hi, leaf_lo, leaf_hi);
    }

    // THE CAFÉ — umbrellas and tables between the path and the stream, villagers at them, a musician,
    // planters of herbs, a chalkboard sign
    let ntable = (3.0 + 2.0 * q) as i32;
    for k in 0..ntable {
        let t = 0.3 + (k as f32 + 0.5) / ntable as f32 * 0.35;
        let p = addv(path(t), v(-5.5 * sc + (hs(k, 50) - 0.5) * 2.0 * sc, 0.0, 0.0));
        cafe_table(&mut s, p, 2 + (hs(k, 51) > 0.4) as u32, 2900 + k as u32 * 5);
        // umbrella: a pole and a shallow cone of cream canvas with a coloured rim
        let mut pole = m3::tube(p, addv(p, v(0.0, 2.6 * sc, 0.0)), 0.05, 5, c(0x5a, 0x42, 0x28)); pole.mat = M::WOOD; s.add(pole, Float::STILL);
        let col = [c(0xe8, 0xd8, 0xb8), c(0xd0, 0x6a, 0x50), c(0xe0, 0xc0, 0x60)][(k as usize) % 3];
        let mut canopy = m3::dome_cap_outside(addv(p, v(0.0, 2.25 * sc, 0.0)), 1.9 * sc, 0.55 * sc, 0.0, 12, 3, col); m3::tint_up(&mut canopy, mix_rgb(col, c(0xff, 0xff, 0xf0), 0.3), mix_rgb(col, c(0x40, 0x30, 0x20), 0.35)); canopy.mat = M::PAPER; canopy.emissive = 0.06; s.add(canopy, Float { amp: 0.01, speed: 0.6, phase: hs(k, 52) * TAU, drift: 0.02 });
        let mut rim = m3::torus(addv(p, v(0.0, 2.25 * sc, 0.0)), 1.9 * sc, 0.035, 12, 4, mix_rgb(col, c(0x60, 0x30, 0x20), 0.5)); rim.mat = M::PAPER; s.add(rim, Float::STILL);
        contact_shadow(&mut s, p.x, p.z, 1.9 * sc, yf, 60);
    }
    villager(&mut s, addv(path(0.55), v(3.0 * sc, 0.0, 0.0)), PI, false, 3000); // someone walking the path
    villager(&mut s, addv(path(0.62), v(3.6 * sc, 0.0, 0.0)), PI, false, 3001);
    { // chalkboard sign on an easel by the café
        let p = addv(path(0.3), v(-3.0 * sc, 0.0, 1.0));
        for sgn in [-0.25f32, 0.25] { let mut leg = m3::tube(p, addv(p, v(sgn, 1.5, -0.2)), 0.03, 4, c(0x4a, 0x36, 0x20)); leg.mat = M::WOOD; s.add(leg, Float::STILL); }
        let mut board = m3::panel(addv(p, v(0.0, 0.95, -0.1)), v(-1.0, 0.0, 0.0), normv(v(0.0, 1.0, -0.2)), 0.7, 0.9, c(0x1e, 0x22, 0x1e)); board.mat = M::STONE; s.add(board, Float::STILL);
        for i in 0..3 { let mut chalk = m3::panel(addv(p, v(0.0, 1.2 - i as f32 * 0.22, -0.06 + i as f32 * 0.045)), v(-1.0, 0.0, 0.0), normv(v(0.0, 1.0, -0.2)), 0.45 - i as f32 * 0.1, 0.05, c(0xe8, 0xe4, 0xd8)); chalk.mat = M::PAPER; chalk.emissive = 0.15; s.add(chalk, Float::STILL); }
    }
    for k in 0..(10.0 * q) as i32 { // herb planters along the path
        let t = hs(k, 60) * 0.8 + 0.1;
        let p = addv(path(t), v(if hs(k, 61) > 0.5 { 4.2 } else { -4.2 } * sc, 0.0, 0.0));
        let mut pot = m3::tube(p, addv(p, v(0.0, 0.5, 0.0)), 0.35, 8, c(0xa0, 0x6a, 0x4a)); pot.mat = M::STONE; s.add(pot, Float::STILL);
        let mut herb = m3::leaf_cluster(addv(p, v(0.0, 0.75, 0.0)), 0.5, 3100 + k as u32, c(0x5a, 0x86, 0x3e)); m3::tint_up(&mut herb, c(0x8a, 0xb6, 0x58), c(0x3a, 0x5a, 0x2a)); herb.mat = M::MOSS; s.add(herb, Float { amp: 0.02, speed: 0.6, phase: hs(k, 62) * TAU, drift: 0.02 });
        if hs(k, 63) > 0.5 { flower(&mut s, p.x + 0.3, p.z, yf + 0.5, 3200 + k as u32, [c(0xe8, 0x70, 0x60), c(0xf0, 0xd0, 0x50), c(0xc0, 0x80, 0xd0)][(k as usize) % 3]); }
    }
    let mut sun_rock = m3::rock(addv(path(0.2), v(-4.0 * sc, -0.1, 0.0)), 1.1 * sc, 3301, c(0x8a, 0x86, 0x7a)); m3::tint_up(&mut sun_rock, c(0xb8, 0xb2, 0xa4), c(0x5a, 0x56, 0x4e)); sun_rock.mat = M::STONE; s.add(sun_rock, Float::STILL);
    critter(&mut s, 1, addv(path(0.2), v(-4.0 * sc, 1.0 * sc - 0.1, 0.0)), 0.8, 3300); // a cat sunning on the warm stone

    // SKY — cumulus: clusters of white puffs high up, drifting; a warm sun glow low toward the back
    for k in 0..(9.0 * q) as i32 {
        let cx = (hs(k, 70) - 0.5) * rr * 2.6; let cz = (hs(k, 71) - 0.5) * rr * 2.6; let cy = yf + (34.0 + hs(k, 72) * 14.0) * sc;
        let drift = Float { amp: 0.2, speed: 0.05 + hs(k, 73) * 0.04, phase: hs(k, 74) * TAU, drift: 3.0 };
        for j in 0..5 {
            let r = (3.0 + hs(k * 7 + j, 75) * 4.0) * sc * (1.0 - j as f32 * 0.1);
            let mut puff = m3::uv_sphere(v(cx + (hs(k * 7 + j, 76) - 0.5) * 9.0 * sc, cy + (hs(k * 7 + j, 77) - 0.3) * 3.0 * sc, cz + (hs(k * 7 + j, 78) - 0.5) * 7.0 * sc), r, 10, 8, c(0xf6, 0xf4, 0xf0));
            m3::tint_up(&mut puff, c(0xff, 0xff, 0xfc), c(0xb8, 0xc0, 0xcc)); puff.mat = M::PAPER; puff.emissive = 0.35; s.add(puff, drift);
        }
    }
    for k in 0..(20.0 * q) as i32 { bee(&mut s, v((hs(k, 80) - 0.5) * rr, yf + 1.0 + hs(k, 81) * 2.5, (hs(k, 82) - 0.5) * rr), 3400 + k as u32); }
    for k in 0..(5.0 * q) as i32 { bird(&mut s, v((hs(k, 85) - 0.5) * rr * 1.5, yf + (20.0 + hs(k, 86) * 12.0) * sc, (hs(k, 87) - 0.5) * rr * 1.5), 3500 + k as u32); }
    organic_boundary(&mut s, rr * 1.35, yf - 1.0, 9200, c(0x3e, 0x62, 0x30), c(0x8a, 0xb8, 0x5c), M::MOSS, leaf_hi, true, sc);
    s
}
