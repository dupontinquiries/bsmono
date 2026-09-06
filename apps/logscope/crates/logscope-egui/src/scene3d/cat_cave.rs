//! CAT CAVE — a lantern-lit grotto. A dark stone bowl with a still pond at its heart, crossed by stepping
//! stones; a waterfall pouring off a ledge on the left; a round skylight in the cave roof with roots and vines
//! trailing down through it and one broad shaft of daylight falling to the water; stalactites; shelves of
//! jars cut into the wall; caged lanterns on posts; a writing desk in the foreground; and cats — one on a warm
//! rock at the water's edge, one asleep on the desk. Everything the reference render shows except the text.
//!
//! Built as a child module of `scene3d` so it can use the private prop vocabulary (`waterfall`,
//! `stalactite`, `critter`, `light_shaft`, `reading_desk`, `caged_lantern`) without exporting any of it.
use super::*;

/// Scale every mesh added since `mark` about `pivot` — the props are built at one fixed size and the
/// reference cats are foreground-sized, not house-cat-sized.
fn scale_since(s: &mut Scene, mark: usize, pivot: Vec3, k: f32) {
    for m in &mut s.meshes[mark..] {
        for p in &mut m.verts { *p = addv(pivot, mulv(v(p.x - pivot.x, p.y - pivot.y, p.z - pivot.z), k)); }
    }
}

/// A glass jar on a shelf: a short translucent cylinder with a dark lid and a faint glow of whatever is
/// inside (herbs, fireflies, pickled somethings) — the cave's shelves are full of them.
fn jar(s: &mut Scene, p: Vec3, r: f32, h: f32, fill: Color32, seed: u32) {
    let mut body = m3::tube(p, addv(p, v(0.0, h, 0.0)), r, 8, mix_rgb(fill, c(0xd8, 0xe4, 0xe0), 0.45));
    m3::set_alpha(&mut body, 150); body.mat = M::WATER; body.emissive = 0.18 + hash(seed as i32, 1) * 0.2; s.add(body, Float::STILL);
    let mut lid = m3::disc(addv(p, v(0.0, h + 0.01, 0.0)), r * 1.05, 8, c(0x3a, 0x2e, 0x22)); lid.mat = M::WOOD; s.add(lid, Float::STILL);
    let mut inner = m3::uv_sphere(addv(p, v(0.0, h * 0.45, 0.0)), r * 0.55, 6, 5, fill); inner.mat = M::MOSS; inner.emissive = 0.5; s.add(inner, Float::STILL);
}

/// A rock-cut shelf with jars along it, set into the cave wall at bearing `ang` (the shelf runs tangent to the
/// wall, facing inward). `tiers` shelves stacked.
fn jar_shelf(s: &mut Scene, cx: f32, cz: f32, ang: f32, len: f32, yb: f32, tiers: u32, seed: u32) {
    let along = v(-ang.sin(), 0.0, ang.cos());
    let inward = v(-ang.cos(), 0.0, -ang.sin());
    for t in 0..tiers {
        let y = yb + 1.2 + t as f32 * 1.1;
        let ctr = v(cx, y, cz);
        let mut plank = m3::panel(addv(ctr, mulv(inward, 0.3)), along, inward, len, 0.6, c(0x4a, 0x38, 0x24));
        m3::tint_up(&mut plank, c(0x5e, 0x48, 0x2e), c(0x30, 0x24, 0x16)); plank.mat = M::WOOD; s.add(plank, Float::STILL);
        let mut lip = m3::tube(addv(ctr, addv(mulv(inward, 0.6), mulv(along, -len * 0.5))), addv(ctr, addv(mulv(inward, 0.6), mulv(along, len * 0.5))), 0.04, 4, c(0x30, 0x24, 0x16)); lip.mat = M::WOOD; s.add(lip, Float::STILL);
        for b in [-1.0f32, 1.0] { // brackets under each end
            let e = addv(ctr, addv(mulv(along, b * len * 0.42), mulv(inward, 0.55)));
            let mut br = m3::tube(e, addv(e, addv(mulv(inward, -0.5), v(0.0, -0.45, 0.0))), 0.035, 4, c(0x2c, 0x2a, 0x28)); br.mat = M::STONE; s.add(br, Float::STILL);
        }
        let njar = (len / 0.55) as u32;
        for j in 0..njar {
            let k = seed.wrapping_add(t * 31 + j * 7);
            if hash(k as i32, 3) < 0.18 { continue; }
            let x = (j as f32 + 0.5) / njar as f32 - 0.5;
            let p = addv(ctr, addv(mulv(along, x * len), mulv(inward, 0.28 + hash(k as i32, 4) * 0.12)));
            let fills = [c(0xd4, 0x9a, 0x44), c(0x6a, 0x9a, 0x50), c(0xc0, 0x70, 0x50), c(0x8a, 0xb0, 0x9a), c(0xe0, 0xc0, 0x70)];
            jar(s, addv(p, v(0.0, 0.01, 0.0)), 0.1 + hash(k as i32, 5) * 0.08, 0.28 + hash(k as i32, 6) * 0.3, fills[(k as usize) % 5], k);
        }
    }
}

/// A trailing root/vine dropping through the skylight: a jointed tube that wanders as it falls, with leaf
/// tufts every so often and a bare frayed tip, swaying as one.
fn hanging_vine(s: &mut Scene, top: Vec3, len: f32, seed: u32, sway: Float) {
    let segs = (len / 1.4).max(3.0) as u32;
    let mut p = top;
    let mut dir = v((hash(seed as i32, 1) - 0.5) * 0.3, -1.0, (hash(seed as i32, 2) - 0.5) * 0.3);
    for i in 0..segs {
        let l = len / segs as f32;
        let q = addv(p, mulv(normv(dir), l));
        let r = (0.22 - i as f32 * 0.02).max(0.05);
        let mut seg = m3::tube(p, q, r, 5, c(0x4a, 0x3a, 0x26));
        m3::tint_up(&mut seg, c(0x5e, 0x4c, 0x32), c(0x2e, 0x24, 0x18)); seg.mat = M::WOOD; s.add(seg, sway);
        if i % 2 == 1 && i + 1 < segs {
            let mut lf = m3::leaf_cluster(q, 1.1 + hash(seed as i32 + i as i32, 4) * 0.9, seed.wrapping_add(i * 13), c(0x4e, 0x72, 0x36));
            m3::tint_up(&mut lf, c(0x74, 0x9a, 0x48), c(0x2a, 0x40, 0x1e)); lf.mat = M::MOSS; lf.emissive = 0.06; s.add(lf, sway);
        }
        dir = v(dir.x + (hash(seed as i32 + i as i32, 5) - 0.5) * 0.35, -1.0, dir.z + (hash(seed as i32 + i as i32, 6) - 0.5) * 0.35);
        p = q;
    }
}

pub(super) fn cat_cave() -> Scene {
    use std::f32::consts::{PI, TAU};
    let mut s = Scene::new();
    let pr = env_params();
    let yf = -3.6_f32;
    let sc = (map_radius() / 64.0).clamp(1.0, 2.0);
    let rr = (30.0 * sc).max((pr.ext_x.max(pr.ext_z) as f32) * 0.62 + 12.0); // cave radius: holds the graph + a shoreline
    // ENVPARAMS drive the cave: `nodes` how well-lit it is (lanterns), `branches` + `bear` how many shelves are
    // cut into the wall and WHERE (one per conversation branch, at that branch's bearing), `prop_density` the
    // scatter (stalactites, ferns, motes), `seed` every placement. A different talk is a different cave.
    let q = render_quality() * pr.prop_density.clamp(0.4, 2.0);
    let hs = |k: i32, j: i32| hash(k ^ (pr.seed as i32 & 0xffff), j);
    // ALBEDO NOTE: this renderer's exposure chain turns anything darker than ~0x40 to black (the Alley's
    // asphalt already reads that way), so the cave's stone is drawn two stops lighter than the reference
    // looks and lets the lighting and the bg colour make it dark.
    let (rock_lo, rock_hi) = (c(0x46, 0x40, 0x36), c(0x84, 0x7a, 0x6a));
    let moss = c(0x3e, 0x5a, 0x2c);
    let amber = c(0xff, 0xb8, 0x5a);

    // FLOOR — a dark stone bowl with a little relief, mossy in the damp
    terrain_disc(&mut s, rr * 1.12, yf, pr.seed ^ 0xCA7, 0.12, 0.09, c(0x3a, 0x34, 0x2c), c(0x6a, 0x62, 0x54), M::STONE);
    let pond_y = yf + 0.16; // above the floor's relief, so the water is not buried under it
    for k in 0..(40.0 * q) as i32 { // moss patches on the floor and low rocks
        let a = hs(k, 1) * TAU; let d = rr * (0.45 + hs(k, 2) * 0.55);
        let mut mp = m3::disc(v(a.cos() * d, yf + 0.03, a.sin() * d), (0.8 + hs(k, 3) * 1.6) * sc, 10, moss);
        m3::scale_xz(&mut mp, 1.0, 0.5 + hs(k, 4) * 0.7); m3::set_alpha(&mut mp, 200); mp.mat = M::MOSS; mp.emissive = 0.03; s.add(mp, Float::STILL);
    }

    // THE POND — still, dark, teal-black, reflecting the lantern light. Slightly below the floor so the rim
    // reads as a shore; ripple rings where the fall lands.
    let pond_r = rr * 0.44;
    let pond_c = v(0.0, pond_y, rr * 0.05);
    // (WATER blazes under this renderer — the Atlantis notes say the same — so the pond is drawn far darker
    // than it will read, and with no emissive to speak of)
    let mut pond = m3::disc(pond_c, pond_r, 48, c(0x16, 0x30, 0x38));
    m3::tint_up(&mut pond, c(0x20, 0x44, 0x4c), c(0x0e, 0x1e, 0x24)); pond.mat = M::WATER; pond.emissive = 0.04; m3::set_alpha(&mut pond, 236); s.add(pond, Float::STILL);
    let mut shore = m3::annulus(v(0.0, pond_y + 0.02, pond_c.z), pond_r * 0.97, pond_r * 1.12, 48, c(0x5a, 0x56, 0x4a)); m3::tint_up(&mut shore, c(0x6a, 0x74, 0x58), c(0x3e, 0x3a, 0x32)); shore.mat = M::STONE; s.add(shore, Float::STILL);
    for k in 0..(26.0 * q) as i32 { // shoreline rocks, half in the water
        let a = hs(k, 10) * TAU;
        let d = pond_r * (1.0 + (hs(k, 11) - 0.5) * 0.12);
        let r = (0.5 + hs(k, 12) * 1.1) * sc;
        let mut rk = m3::rock(v(a.cos() * d, pond_y - r * 0.4, a.sin() * d + pond_c.z), r, k as u32 + 900, rock_lo);
        m3::tint_up(&mut rk, mix_rgb(rock_hi, moss, 0.45), rock_lo); rk.mat = M::STONE; rk.emissive = 0.02; s.add(rk, Float::STILL);
    }

    // STEPPING STONES — a line of flat mossy slabs from the near shore to the far, wandering a little, so the
    // eye walks across the water toward the light
    let nstep = (pond_r * 2.0 / (2.2 * sc)) as i32 + 1;
    for i in 0..nstep {
        let t = (i as f32 + 0.5) / nstep as f32;
        let z = pond_c.z + pond_r * (0.95 - 1.9 * t);
        let x = (t * 5.0).sin() * pond_r * 0.14 + (hs(i, 20) - 0.5) * 0.6 * sc;
        let r = (0.9 + hs(i, 21) * 0.5) * sc;
        let mut st = m3::rock(v(0.0, 0.0, 0.0), r, i as u32 + 300, c(0x3a, 0x38, 0x32));
        m3::scale3(&mut st, 1.0, 0.28, 0.85); for p in &mut st.verts { *p = addv(*p, v(x, pond_y + 0.06, z)); }
        m3::tint_up(&mut st, mix_rgb(c(0x5a, 0x58, 0x4e), moss, 0.35), c(0x24, 0x22, 0x1e)); st.mat = M::STONE; st.emissive = 0.03; s.add(st, Float::STILL);
        ripple_rings(&mut s, x, z, pond_y + 0.01, r * 1.35, 1, c(0x22, 0x3e, 0x44));
    }

    // WATERFALL — off a rock ledge on the left, into the pond
    let (wx, wz) = (-rr * 0.52, pond_c.z - rr * 0.12);
    let ledge_y = yf + 8.5 * sc;
    for k in 0..7 { // the ledge: a stack of big rocks with a flat top
        let mut lr = m3::rock(v(wx + (hs(k, 30) - 0.5) * 6.0 * sc, yf + k as f32 * 1.3 * sc, wz + (hs(k, 31) - 0.5) * 6.0 * sc), (3.0 + hs(k, 32) * 2.0) * sc, 400 + k as u32, rock_lo);
        m3::tint_up(&mut lr, mix_rgb(rock_hi, moss, 0.5), rock_lo); lr.mat = M::STONE; s.add(lr, Float::STILL);
    }
    let mut lip = m3::panel(v(wx, ledge_y, wz), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0), 5.0 * sc, 3.0 * sc, c(0x5a, 0x6a, 0x54)); lip.mat = M::STONE; s.add(lip, Float::STILL);
    waterfall(&mut s, wx + 1.2 * sc, wz, ledge_y - 0.1, pond_y, 2.4 * sc, c(0xc4, 0xe0, 0xe8), 0);
    ripple_rings(&mut s, wx + 2.0 * sc, wz, pond_y + 0.01, 4.0 * sc, 4, c(0x9a, 0xd0, 0xd6));
    for k in 0..(8.0 * q) as i32 { // a mist haze at the foot of the fall
        let mut fog = m3::disc(v(wx + 2.0 * sc + (hs(k, 35) - 0.5) * 5.0 * sc, yf + 0.6 + hs(k, 36) * 2.0 * sc, wz + (hs(k, 37) - 0.5) * 5.0 * sc), (2.0 + hs(k, 38) * 2.5) * sc, 12, c(0xb0, 0xc8, 0xcc));
        fog.mat = M::WATER; fog.emissive = 0.12; m3::set_alpha(&mut fog, 28); s.add(fog, Float { amp: 0.25, speed: 0.15, phase: hs(k, 39) * TAU, drift: 0.4 });
    }

    // CAVE WALLS + ROOF — a rough rock ring, craggy boulders piled against it, a dome overhead with a round
    // SKYLIGHT at the crown
    let wall_h = 20.0 * sc;
    let mut wall = m3::wall_ring(v(0.0, yf - 1.0, 0.0), rr * 1.1, wall_h + 1.0, 56, rock_lo);
    m3::tint_height(&mut wall, yf, yf + wall_h, c(0x40, 0x3a, 0x30), c(0x72, 0x6a, 0x5e)); wall.mat = M::STONE; wall.emissive = 0.03; s.add(wall, Float::STILL);
    for k in 0..(48.0 * q) as i32 { // crags against the wall, at all heights, so the wall is not a cylinder
        let a = hs(k, 40) * TAU;
        let d = rr * (1.0 + (hs(k, 41) - 0.5) * 0.18);
        let y = yf + hs(k, 42) * wall_h * 0.9;
        let r = (2.0 + hs(k, 43) * 4.0) * sc;
        let mut cr = m3::rock(v(a.cos() * d, y, a.sin() * d), r, 500 + k as u32, rock_lo);
        m3::tint_up(&mut cr, if y < yf + 3.0 { mix_rgb(rock_hi, moss, 0.5) } else { rock_hi }, rock_lo); cr.mat = M::STONE; cr.emissive = 0.02; s.add(cr, Float::STILL);
    }
    let sky_r = rr * 0.22;
    let mut roof = m3::dome_cap(v(0.0, yf + wall_h, 0.0), rr * 1.1, 9.0 * sc, sky_r, 40, 8, c(0x3e, 0x38, 0x30));
    m3::tint_up(&mut roof, c(0x4a, 0x44, 0x3a), c(0x2e, 0x2a, 0x24)); roof.mat = M::STONE; s.add(roof, Float::STILL);
    // the SKYLIGHT: bright overcast sky seen through the hole, a rim of roots and grass, and the shaft of day
    let sky_y = yf + wall_h + 9.0 * sc;
    let mut sky = m3::disc_down(v(0.0, sky_y + 0.4, 0.0), sky_r * 1.1, 32, c(0xe8, 0xee, 0xe0)); sky.mat = M::PAPER; sky.emissive = 1.5; s.add(sky, Float::STILL); // seen from below
    let mut rim = m3::torus(v(0.0, sky_y, 0.0), sky_r, 1.2 * sc, 28, 7, c(0x3a, 0x34, 0x28)); m3::tint_up(&mut rim, c(0x5a, 0x6e, 0x3a), c(0x2a, 0x24, 0x1c)); rim.mat = M::MOSS; s.add(rim, Float::STILL);
    for k in 0..(14.0 * q) as i32 { // roots + vines trailing down through the hole
        let a = hs(k, 50) * TAU;
        let sway = Float { amp: 0.06, speed: 0.35 + hs(k, 51) * 0.2, phase: hs(k, 52) * TAU, drift: 0.18 };
        hanging_vine(&mut s, v(a.cos() * sky_r * (0.6 + hs(k, 53) * 0.5), sky_y - 0.2, a.sin() * sky_r * (0.6 + hs(k, 53) * 0.5)), (8.0 + hs(k, 54) * 14.0) * sc, 600 + k as u32, sway);
    }
    for k in 0..(10.0 * q) as i32 { // grass tufts on the rim, seen against the sky
        let a = hs(k, 55) * TAU;
        grass_tuft(&mut s, a.cos() * sky_r * 1.05, a.sin() * sky_r * 1.05, sky_y + 0.3, 700 + k as u32, c(0x6a, 0x8a, 0x40));
    }
    // (a translucent shaft tube + a pool disc were tried for the light column; both painter-sorted into black
    // wedges over the water. The motes carry the shaft on their own.)
    for k in 0..(60.0 * q) as i32 { // dust motes drifting in the shaft
        let a = hs(k, 60) * TAU; let d = hs(k, 61) * sky_r * 0.8;
        let mut mote = m3::uv_sphere(v(a.cos() * d, yf + 1.0 + hs(k, 62) * (sky_y - yf - 2.0), a.sin() * d + pond_c.z * 0.5), 0.05 + hs(k, 63) * 0.04, 5, 4, c(0xf4, 0xf0, 0xdc));
        mote.mat = M::MOSS; mote.emissive = 0.9; s.add(mote, Float { amp: 0.3, speed: 0.12 + hs(k, 64) * 0.15, phase: hs(k, 65) * TAU, drift: 0.5 });
    }
    // STALACTITES — hanging from the dome, more of them nearer the rim where the roof is lower
    for k in 0..(32.0 * q) as i32 {
        let a = hs(k, 70) * TAU;
        let d = rr * (0.35 + hs(k, 71) * 0.7);
        let top = yf + wall_h + 9.0 * sc * (1.0 - (d / (rr * 1.1)).powi(2)).max(0.0) - 0.4;
        stalactite(&mut s, a.cos() * d, a.sin() * d, top, (1.5 + hs(k, 72) * 4.5) * sc, (0.3 + hs(k, 73) * 0.5) * sc, rock_lo, mix_rgb(rock_hi, c(0x9a, 0xa0, 0x9a), 0.3));
    }

    // SHELVES OF JARS cut into the wall — one per conversation BRANCH, at that branch's bearing, deeper branches
    // getting more tiers. Never on the near side (where the reader stands) or behind the waterfall.
    let nshelf = (pr.branches as usize).clamp(2, 6);
    for i in 0..nshelf {
        let mut ang = branch_bearing(&pr, i, nshelf, 0x5E1F);
        if ang.sin() > 0.55 { ang = PI * 1.5 - (ang - PI * 0.5).abs().min(PI * 0.8); } // fold the near arc onto the far wall
        if (ang - PI).abs() < 0.35 { ang += 0.5; } // clear of the fall
        let tiers = match pr.shape.get(i) { Some(sh) if sh.depth > 0 => (1 + sh.depth as u32 / 4).clamp(1, 4), _ => 2 + (i as u32 % 2) };
        let d = rr * 1.0;
        jar_shelf(&mut s, ang.cos() * d, ang.sin() * d, ang + PI * 0.5, (4.0 + hs(i as i32, 80) * 3.0 + tiers as f32) * sc, yf + 0.4, tiers, 800 + i as u32 * 17);
    }

    // CAGED LANTERNS — on iron posts around the shore and hung off the crags, the cave's warm light. Count
    // grows with the conversation: a bigger talk is a better-lit cave.
    let nlan = ((8 + pr.nodes / 30) as f32 * pr.prop_density.clamp(0.5, 1.6)).clamp(6.0, 22.0) as i32;
    for k in 0..nlan {
        let a = (k as f32 + 0.5) / nlan as f32 * TAU + hs(k, 90) * 0.3;
        let d = pond_r * 1.28 + hs(k, 91) * rr * 0.28;
        let (lx, lz) = (a.cos() * d, a.sin() * d + pond_c.z);
        let h = (2.4 + hs(k, 92) * 1.4) * sc;
        let mut post = m3::tube(v(lx, yf, lz), v(lx, yf + h, lz), 0.07, 5, c(0x2a, 0x28, 0x26)); post.mat = M::STONE; s.add(post, Float::STILL);
        let mut arm = m3::tube(v(lx, yf + h, lz), v(lx + a.cos() * -0.5, yf + h + 0.1, lz + a.sin() * -0.5), 0.04, 4, c(0x2a, 0x28, 0x26)); arm.mat = M::STONE; s.add(arm, Float::STILL);
        let sway = Float { amp: 0.015, speed: 0.6 + hs(k, 93) * 0.3, phase: hs(k, 94) * TAU, drift: 0.03 };
        caged_lantern(&mut s, v(lx + a.cos() * -0.5, yf + h + 0.05, lz + a.sin() * -0.5), 0.42 * sc, amber, 1000 + k as u32, sway);
        let mut glow = m3::disc(v(lx + a.cos() * -0.5, yf + 0.1, lz + a.sin() * -0.5), 3.0 * sc, 18, amber); m3::set_alpha(&mut glow, 48); glow.mat = M::MOSS; glow.emissive = 0.7; s.add(glow, Float::STILL);
        contact_shadow(&mut s, lx, lz, 0.4, yf, 90);
    }

    // THE DESK — the foreground writing table, with an open book, a mug, a small globe, a stack of tomes
    let desk = v(rr * 0.2, yf, rr * 0.72);
    reading_desk(&mut s, desk.x, desk.z, yf);
    let mut mug = m3::tube(addv(desk, v(-0.55, 0.92, 0.2)), addv(desk, v(-0.55, 1.2, 0.2)), 0.13, 8, c(0x3a, 0x30, 0x2a)); mug.mat = M::STONE; s.add(mug, Float::STILL);
    let mut handle = m3::torus(addv(desk, v(-0.7, 1.06, 0.2)), 0.08, 0.02, 8, 4, c(0x3a, 0x30, 0x2a)); handle.mat = M::STONE; s.add(handle, Float::STILL);
    let mut steam = m3::uv_sphere(addv(desk, v(-0.55, 1.35, 0.2)), 0.1, 6, 5, c(0xd0, 0xd8, 0xd4)); m3::set_alpha(&mut steam, 60); steam.mat = M::WATER; steam.emissive = 0.2; s.add(steam, Float { amp: 0.12, speed: 0.9, phase: 0.0, drift: 0.04 });
    let gb = addv(desk, v(0.55, 0.92, -0.3));
    let mut gstem = m3::tube(gb, addv(gb, v(0.0, 0.28, 0.0)), 0.03, 4, c(0x8a, 0x6a, 0x30)); gstem.mat = M::STONE; s.add(gstem, Float::STILL);
    let mut globe = m3::uv_sphere(addv(gb, v(0.0, 0.42, 0.0)), 0.17, 10, 8, c(0x4a, 0x6a, 0x8a)); m3::tint_up(&mut globe, c(0x8a, 0x9a, 0x6a), c(0x2a, 0x40, 0x5a)); globe.mat = M::STONE; globe.emissive = 0.08; s.add(globe, Float::STILL);
    let mut gring = m3::torus(addv(gb, v(0.0, 0.42, 0.0)), 0.2, 0.015, 12, 4, c(0xb0, 0x90, 0x50)); gring.mat = M::STONE; gring.emissive = 0.2; s.add(gring, Float::STILL);
    let mut lamp_post = m3::tube(addv(desk, v(-1.4, 0.0, -0.4)), addv(desk, v(-1.4, 2.2, -0.4)), 0.06, 5, c(0x2a, 0x28, 0x26)); lamp_post.mat = M::STONE; s.add(lamp_post, Float::STILL);
    caged_lantern(&mut s, addv(desk, v(-1.4, 2.15, -0.1)), 0.34, amber, 1200, Float { amp: 0.01, speed: 0.5, phase: 1.0, drift: 0.02 });

    // CATS — one on a flat warm rock by the water watching the fall, one curled on the desk. Reference-sized.
    let cat_rock = v(rr * 0.34, yf, pond_c.z + pond_r * 0.9);
    let mut cr = m3::rock(v(0.0, 0.0, 0.0), 1.6 * sc, 1300, c(0x3e, 0x3a, 0x34));
    m3::scale3(&mut cr, 1.0, 0.4, 1.0); for p in &mut cr.verts { *p = addv(*p, addv(cat_rock, v(0.0, 0.2, 0.0))); }
    m3::tint_up(&mut cr, mix_rgb(c(0x5e, 0x5a, 0x50), moss, 0.3), c(0x26, 0x24, 0x20)); cr.mat = M::STONE; s.add(cr, Float::STILL);
    let mk = s.mark();
    critter(&mut s, 1, addv(cat_rock, v(0.0, 0.62 * sc, 0.0)), PI * 0.5 + 0.6, 1400); // faces the water
    scale_since(&mut s, mk, addv(cat_rock, v(0.0, 0.62 * sc, 0.0)), 2.2 * sc);
    let mk = s.mark();
    critter(&mut s, 1, addv(desk, v(0.1, 0.92, 0.35)), -PI * 0.5, 1500); // asleep on the desk, facing the reader
    scale_since(&mut s, mk, addv(desk, v(0.1, 0.92, 0.35)), 1.5);

    // FERNS + FLOWERS in the damp corners; fireflies over the water at the dark end
    for k in 0..(30.0 * q) as i32 {
        let a = hs(k, 100) * TAU; let d = pond_r * 1.15 + hs(k, 101) * rr * 0.4;
        let (fx, fz) = (a.cos() * d, a.sin() * d + pond_c.z);
        let mut fern = m3::leaf_cluster(v(fx, yf + 0.3 * sc, fz), (0.3 + hs(k, 102) * 0.35) * sc, 1600 + k as u32, c(0x3a, 0x5e, 0x2e)); // leaf_cluster's r is LEAF size
        m3::tint_up(&mut fern, c(0x6a, 0x96, 0x44), c(0x22, 0x36, 0x1a)); fern.mat = M::MOSS; fern.emissive = 0.05; s.add(fern, Float { amp: 0.04, speed: 0.5, phase: hs(k, 103) * TAU, drift: 0.05 });
        if hs(k, 104) > 0.7 { flower(&mut s, fx + 0.6, fz, yf, 1700 + k as u32, c(0xe8, 0xd0, 0x70)); }
    }
    for k in 0..(18.0 * q) as i32 {
        let a = PI + (hs(k, 110) - 0.5) * PI * 0.9; let d = hs(k, 111) * pond_r * 0.8;
        firefly(&mut s, v(a.cos() * d, yf + 0.6 + hs(k, 112) * 2.5, a.sin() * d + pond_c.z), 1800 + k as u32);
    }
    // a slow-breathing low mist over the far half of the pond
    for k in 0..6 {
        let mut mist = m3::disc(v((hs(k, 120) - 0.5) * pond_r, yf + 0.5 + hs(k, 121) * 0.6, pond_c.z - pond_r * 0.4 - hs(k, 122) * pond_r * 0.4), (3.0 + hs(k, 123) * 4.0) * sc, 14, c(0x8a, 0xa0, 0xa4));
        mist.mat = M::WATER; mist.emissive = 0.06; m3::set_alpha(&mut mist, 22); s.add(mist, Float { amp: 0.15, speed: 0.1, phase: hs(k, 124) * TAU, drift: 0.3 });
    }
    s
}
