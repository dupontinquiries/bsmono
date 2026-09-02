//! DISTRICT — the open ground, authored in occupant heights, addressed by
//! WORLD coordinate so the window can move.
//!
//! Two facts decide this file's shape. A car at 60 mph covers the whole
//! ±1300 m tile in a minute and a half, and this car is faster than
//! that; so for the vehicle side of the game the world cannot be a tile,
//! it has to be a WINDOW that pages as you drive (`world::World::origin`).
//! And a map that is a plan with ends — a hangar capping the road, kerbs
//! down its length, a picket line of spires, nothing past the last
//! arch — is a cage: the player reads the end of the plan as the end of
//! the world. `docs/VEHICLE_COMBAT.md` §cradle has the analysis; the
//! short form is Lynch's five elements (paths, edges, districts, nodes,
//! landmarks) and Appleton's prospect–refuge: an edge must read as MORE
//! world, never as its end, and a body wants to see without being seen.
//!
//! So the district is not a plan, it is a LATTICE over the whole world
//! plane, and `layout(seed, extent, origin)` returns the part of it that
//! falls inside the window at `origin`. Every feature is placed by a
//! hash of its lattice cell, so a spire at world (1820, -540) is the
//! same spire from any window that can see it — that is the property a
//! moving window needs and the test `the_same_place_from_two_windows`
//! pins. Density falls off from the core at world (0, 0) and never
//! reaches zero: city, then outskirts, then a plain with the odd tower,
//! forever. The boulevard runs along world +X without end; cross-streets
//! leave it every third arch so the district is a grid to roam, not a
//! corridor to run; the hangar stands BESIDE the road, a node and not a
//! cap; plazas are refuges with prospect down the road; repetition is
//! broken everywhere because the scenic score in `games/soldier`'s map
//! analysis gates the whole map on it — a kerb line with a third of its
//! posts missing reads as a city, a complete one reads as a fence.
//!
//! ## One unit, one dial
//!
//! Every dimension is in OCCUPANT HEIGHTS (`grandeur.rs`): an arch 4 oh
//! clear, a hangar 20 oh, a lattice cell 60 oh. `DISTRICT_K` multiplies
//! the unit — 4.0 while the body is the sandbox car, 1.0 when it is a
//! robot — and is the only number here that is not a reading off the
//! references.
//!
//! What is still a note: the far horizon (Planetary-tier silhouettes at
//! the fog line) and the terrain skirt past the window, both in
//! `docs/VEHICLE_COMBAT.md` under "the cradle".

use glam::{Quat, Vec3};

use crate::world::{Sdf, M_BOX, M_CAP_TALL, CAP_TALL_HH};

/// The scale placeholder. See the module doc.
pub const DISTRICT_K: f32 = 4.0;
/// One occupant height in engine metres, under the dial.
pub fn oh() -> f32 {
    crate::scale::RL_BODY_H_M * DISTRICT_K
}

// ---- the plan, in oh. Each is a `grandeur.rs` reference or derived from one.
/// the lattice cell: one arch's worth of boulevard
pub const CELL_OH: f32 = 60.0;
/// the boulevard's clear width. GUESS x3 on the 2.5 oh corridor.
pub const BOULEVARD_W_OH: f32 = 8.0;
/// an archway a Transformer walks under grandly
pub const ARCH_CLEAR_OH: f32 = 4.0;
/// spires: Monumental, the Cybertron towers eyeballed at 300 m / 8.5
pub const SPIRE_MIN_OH: f32 = 20.0;
pub const SPIRE_MAX_OH: f32 = 35.0;
/// a building block: a mass, not a tower
pub const BLOCK_MIN_OH: f32 = 5.0;
pub const BLOCK_MAX_OH: f32 = 11.0;
/// the hangar: 20 oh, the Ark's interior
pub const HANGAR_H_OH: f32 = 20.0;
pub const HANGAR_W_OH: f32 = 26.0;
pub const HANGAR_L_OH: f32 = 40.0;
/// a plaza's cover ring, set back from the kerb
pub const PLAZA_SETBACK_OH: f32 = 14.0;
pub const PLAZA_R_OH: f32 = 9.0;
pub const COVER_H_OH: f32 = 1.1;
/// a jump ramp on the straight
pub const RAMP_L_OH: f32 = 9.0;
pub const RAMP_RISE_OH: f32 = 1.6;
/// kerb posts: this often, this tall, and a third of them missing
pub const KERB_EVERY_OH: f32 = 10.0;
pub const KERB_H_OH: f32 = 1.4;
pub const KERB_DROP: f32 = 0.33;
/// the density falloff: 1 at the core, `DENSITY_FLOOR` on the plain
pub const CORE_R_OH: f32 = 300.0;
pub const DENSITY_FLOOR: f32 = 0.12;
/// a cross-street leaves the boulevard every this many arches
pub const CROSS_EVERY: i64 = 3;
/// a plaza sits beside every this many arches, alternating sides
pub const PLAZA_EVERY: i64 = 2;

/// The part of the lattice inside one window. Positions are WINDOW-LOCAL
/// engine metres, y = 0 (the caller seats each piece on its own ground).
#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// where this window sits on the world plane
    pub origin: Vec3,
    pub extent: f32,
    /// the boulevard's local z, and whether any of it is in the window
    pub road_z: f32,
    pub road_in_window: bool,
    pub width: f32,
    pub arches: Vec<Vec3>,
    /// centre and height of each spire
    pub spires: Vec<(Vec3, f32)>,
    /// centre and half extents of each building block
    pub blocks: Vec<(Vec3, Vec3)>,
    pub plazas: Vec<Vec3>,
    pub ramps: Vec<Vec3>,
    /// local x of each cross-street (they run along z)
    pub cross_streets: Vec<f32>,
    pub hangar: Option<Vec3>,
    pub kerbs: Vec<Vec3>,
}

fn hash(seed: u64, i: i64, j: i64, lane: u64) -> f32 {
    let x = seed
        .wrapping_mul(0x9E3779B97F4A7C15)
        .wrapping_add((i as u64).wrapping_mul(0x632BE59BD9B4E019))
        .wrapping_add((j as u64).wrapping_mul(0xD6E8FEB86659FD93))
        .wrapping_add(lane.wrapping_mul(0xA0761D6478BD642F));
    let x = (x ^ (x >> 29)).wrapping_mul(0xBF58476D1CE4E5B9);
    ((x >> 40) & 0xFFFFFF) as f32 / 16_777_215.0
}

/// City, outskirts, plain: 1 at the core, never below the floor.
pub fn density(world_xz: Vec3) -> f32 {
    let r = (world_xz.x * world_xz.x + world_xz.z * world_xz.z).sqrt() / (CORE_R_OH * oh());
    DENSITY_FLOOR + (1.0 - DENSITY_FLOOR) * (-r * r).exp()
}

/// Which cells overlap a window: index range along one axis.
fn cell_range(origin: f32, extent: f32, cell: f32) -> (i64, i64) {
    (((origin - extent) / cell).floor() as i64 - 1, ((origin + extent) / cell).ceil() as i64 + 1)
}

fn inside(local: Vec3, extent: f32) -> bool {
    local.x.abs() <= extent && local.z.abs() <= extent
}

/// Lay out the window at `origin`. `extent_m` is the window's half size
/// (`world::TG_HALF`); features are kept inside 96% of it so nothing is
/// cut by the tile edge (the edge is the skirt's, not the plan's).
pub fn layout(seed: u64, extent_m: f32, origin: Vec3) -> Layout {
    let u = oh();
    let cell = CELL_OH * u;
    let keep = extent_m * 0.96;
    let mut l = Layout { origin, extent: extent_m, road_z: -origin.z, width: BOULEVARD_W_OH * u, ..Default::default() };
    l.road_in_window = l.road_z.abs() <= keep;
    let to_local = |w: Vec3| w - Vec3::new(origin.x, 0.0, origin.z);

    let (i0, i1) = cell_range(origin.x, extent_m, cell);
    let (j0, j1) = cell_range(origin.z, extent_m, cell);

    // ---- the road row: arches, plazas, ramps, cross-streets, kerbs, the hangar
    for i in i0..=i1 {
        let wx_cell = i as f32 * cell;
        let d = density(Vec3::new(wx_cell, 0.0, 0.0));
        // an arch on most core cells, fewer out on the plain; jittered so
        // the rhythm is a city's and not a metronome's
        let arch_here = hash(seed, i, 0, 1) < 0.35 + 0.6 * d;
        let wx = wx_cell + (hash(seed, i, 0, 2) - 0.5) * cell * 0.3;
        let arch = to_local(Vec3::new(wx, 0.0, 0.0));
        if arch_here && inside(arch, keep) && l.road_in_window {
            l.arches.push(Vec3::new(arch.x, 0.0, l.road_z));
        }
        if i.rem_euclid(PLAZA_EVERY) == 0 && hash(seed, i, 0, 3) < 0.25 + 0.75 * d {
            let side = if (i / PLAZA_EVERY).rem_euclid(2) == 0 { 1.0 } else { -1.0 };
            let wz = side * (l.width * 0.5 + PLAZA_SETBACK_OH * u + PLAZA_R_OH * u);
            let p = to_local(Vec3::new(wx, 0.0, wz));
            if inside(p, keep - PLAZA_R_OH * u) {
                l.plazas.push(p);
            }
        }
        if i.rem_euclid(CROSS_EVERY) == 0 && arch_here {
            let x = arch.x;
            if x.abs() <= keep {
                l.cross_streets.push(x);
            }
        }
        // a ramp on the straight between two arches, core cells mostly
        if hash(seed, i, 0, 4) < 0.2 + 0.4 * d && !arch_here {
            let r = to_local(Vec3::new(wx_cell + cell * 0.5, 0.0, 0.0));
            if inside(r, keep) && l.road_in_window {
                l.ramps.push(Vec3::new(r.x, 0.0, l.road_z));
            }
        }
        // kerb posts along this cell's stretch, a third missing, core only
        if d > 0.45 && l.road_in_window {
            let mut x = wx_cell;
            let mut k = 0i64;
            while x < wx_cell + cell {
                if hash(seed, i, 1000 + k, 5) >= KERB_DROP {
                    for side in [-1.0f32, 1.0] {
                        let p = to_local(Vec3::new(x, 0.0, side * (l.width * 0.5 + 0.14 * u)));
                        if inside(p, keep) {
                            l.kerbs.push(p);
                        }
                    }
                }
                x += KERB_EVERY_OH * u;
                k += 1;
            }
        }
    }
    // the hangar: one, beside the road at lattice (2, +1), its mouth on the boulevard
    {
        let wx = 2.0 * cell + cell * 0.5;
        let wz = l.width * 0.5 + HANGAR_W_OH * u * 0.5 + 6.0 * u;
        let h = to_local(Vec3::new(wx, 0.0, wz));
        if inside(h, keep - HANGAR_L_OH * u * 0.5) {
            l.hangar = Some(h);
        }
    }

    // ---- the field: spires and blocks on every other cell, thinning outward
    for j in j0..=j1 {
        if j == 0 {
            continue; // the road row
        }
        for i in i0..=i1 {
            let wc = Vec3::new(i as f32 * cell, 0.0, j as f32 * cell);
            let d = density(wc);
            let roll = hash(seed, i, j, 6);
            if roll > d {
                continue;
            }
            // jitter inside the cell, off its edges
            let jx = (hash(seed, i, j, 7) - 0.5) * cell * 0.6;
            let jz = (hash(seed, i, j, 8) - 0.5) * cell * 0.6;
            let w = wc + Vec3::new(jx, 0.0, jz);
            // keep the road band and the cross-streets clear
            if w.z.abs() < l.width * 0.5 + PLAZA_SETBACK_OH * u + PLAZA_R_OH * u * 2.4 {
                continue;
            }
            let p = to_local(w);
            if !inside(p, keep) {
                continue;
            }
            let on_cross = l.cross_streets.iter().any(|cx| (p.x - cx).abs() < l.width * 0.55);
            if on_cross {
                continue;
            }
            // and out of every plaza
            if l.plazas.iter().any(|pl| (pl.x - p.x).abs() < PLAZA_R_OH * u * 1.6 && (pl.z - p.z).abs() < PLAZA_R_OH * u * 1.6) {
                continue;
            }
            if hash(seed, i, j, 9) < 0.35 * d + 0.1 {
                let h = (SPIRE_MIN_OH + (SPIRE_MAX_OH - SPIRE_MIN_OH) * hash(seed, i, j, 10)) * u * (0.6 + 0.4 * d);
                l.spires.push((p, h));
            } else {
                let h = (BLOCK_MIN_OH + (BLOCK_MAX_OH - BLOCK_MIN_OH) * hash(seed, i, j, 11)) * u * (0.7 + 0.3 * d);
                let hx = (6.0 + 8.0 * hash(seed, i, j, 12)) * u;
                let hz = (6.0 + 8.0 * hash(seed, i, j, 13)) * u;
                l.blocks.push((p, Vec3::new(hx, h * 0.5, hz)));
            }
        }
    }
    l
}

/// One thing to build.
#[derive(Clone, Copy)]
pub struct Piece {
    pub mesh: usize,
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub sdf: Sdf,
    pub color: [f32; 4],
    pub emissive: f32,
    pub spec: f32,
}

fn rbox(pos: Vec3, rot: Quat, half: Vec3, color: [f32; 4], emissive: f32, spec: f32) -> Piece {
    Piece { mesh: M_BOX, pos, rot, scale: half, sdf: Sdf::RBox { half, round: (half.min_element() * 0.15).min(0.3) }, color, emissive, spec }
}

/// A plaza's cover ring: (x, z, angle) for each block, so the field can
/// stand a wave behind the same blocks the world built.
pub fn cover_ring(c: Vec3) -> Vec<(f32, f32, f32)> {
    let u = oh();
    let n = 10;
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            (c.x + a.cos() * PLAZA_R_OH * u, c.z + a.sin() * PLAZA_R_OH * u, a)
        })
        .collect()
}

/// Every cover block in the window, as positions.
pub fn cover_points(l: &Layout) -> Vec<Vec3> {
    l.plazas.iter().flat_map(|c| cover_ring(*c).into_iter().map(|(x, z, _)| Vec3::new(x, 0.0, z))).collect()
}

/// Everything a body should steer around, as (centre, radius) discs on
/// the ground: spires, blocks, the arch pylons, the hangar. The cover
/// blocks are NOT here — a body is meant to stand at them.
pub fn obstacles(l: &Layout) -> Vec<(Vec3, f32)> {
    let u = oh();
    let mut out = Vec::new();
    for (p, h) in &l.spires {
        let r = h / (2.0 * CAP_TALL_HH + 2.0);
        out.push((Vec3::new(p.x, 0.0, p.z), r * 1.15));
    }
    for (p, half) in &l.blocks {
        out.push((Vec3::new(p.x, 0.0, p.z), (half.x * half.x + half.z * half.z).sqrt()));
    }
    for a in &l.arches {
        for side in [-1.0f32, 1.0] {
            out.push((Vec3::new(a.x, 0.0, a.z + side * (l.width * 0.5 + 0.9 * u)), 1.1 * u));
        }
    }
    if let Some(h) = l.hangar {
        out.push((Vec3::new(h.x, 0.0, h.z), HANGAR_L_OH * u * 0.55));
    }
    out
}

/// The window as geometry, each piece seated on `th` (ground height at
/// local x, z). `stone`, `accent` and `dark` are the style's palette.
pub fn pieces(l: &Layout, th: &dyn Fn(f32, f32) -> f32, stone: [f32; 4], accent: [f32; 4], dark: [f32; 4]) -> Vec<Piece> {
    let u = oh();
    let mut out = Vec::new();

    // ---- arches: two pylons and a lintel, clear height 4 oh, a glow line
    for a in &l.arches {
        let clear = ARCH_CLEAR_OH * u;
        let pylon_half = Vec3::new(0.7 * u, (clear + 1.2 * u) * 0.5, 0.9 * u);
        for side in [-1.0f32, 1.0] {
            let z = a.z + side * (l.width * 0.5 + pylon_half.z);
            let g = th(a.x, z);
            out.push(rbox(Vec3::new(a.x, g + pylon_half.y, z), Quat::IDENTITY, pylon_half, stone, 0.02, 0.25));
        }
        let g = th(a.x, a.z);
        let lintel_half = Vec3::new(0.6 * u, 0.6 * u, l.width * 0.5 + pylon_half.z * 2.0);
        out.push(rbox(Vec3::new(a.x, g + clear + lintel_half.y, a.z), Quat::IDENTITY, lintel_half, stone, 0.02, 0.3));
        let line_half = Vec3::new(0.08 * u, 0.06 * u, lintel_half.z * 0.96);
        out.push(rbox(Vec3::new(a.x, g + clear - line_half.y, a.z), Quat::IDENTITY, line_half, accent, 0.9, 0.2));
    }

    // ---- spires: tall capsules, a lit band near the top
    for (p, h) in &l.spires {
        let r = h / (2.0 * CAP_TALL_HH + 2.0);
        let half = r * CAP_TALL_HH;
        let g = th(p.x, p.z);
        out.push(Piece { mesh: M_CAP_TALL, pos: Vec3::new(p.x, g + half, p.z), rot: Quat::IDENTITY, scale: Vec3::splat(r), sdf: Sdf::Capsule { r, half }, color: dark, emissive: 0.04, spec: 0.45 });
        let band = Vec3::new(r * 1.06, 0.12 * u, r * 1.06);
        out.push(rbox(Vec3::new(p.x, g + half * 1.7, p.z), Quat::IDENTITY, band, accent, 0.8, 0.3));
    }

    // ---- blocks: building masses, a lit strip at the roofline
    for (p, half) in &l.blocks {
        let g = th(p.x, p.z);
        out.push(rbox(Vec3::new(p.x, g + half.y, p.z), Quat::IDENTITY, *half, mix4(stone, dark, 0.5), 0.0, 0.3));
        let strip = Vec3::new(half.x * 0.9, 0.05 * u, 0.08 * u);
        out.push(rbox(Vec3::new(p.x, g + half.y * 2.0 - strip.y, p.z - half.z), Quat::IDENTITY, strip, accent, 0.5, 0.2));
    }

    // ---- plazas: a ring of low cover, a lit marker in the middle
    for c in &l.plazas {
        for (x, z, a) in cover_ring(*c) {
            let g = th(x, z);
            let half = Vec3::new(1.2 * u, COVER_H_OH * u * 0.5, 0.45 * u);
            let rot = Quat::from_rotation_y(-a);
            out.push(rbox(Vec3::new(x, g + half.y, z), rot, half, stone, 0.0, 0.2));
        }
        let g = th(c.x, c.z);
        let mark = Vec3::new(0.5 * u, 0.06 * u, 0.5 * u);
        out.push(rbox(Vec3::new(c.x, g + mark.y, c.z), Quat::IDENTITY, mark, accent, 0.7, 0.2));
    }

    // ---- ramps: a long low wedge on the crown, pitched up along +X
    for r in &l.ramps {
        let len = RAMP_L_OH * u;
        let rise = RAMP_RISE_OH * u;
        let pitch = (rise / len).atan();
        let half = Vec3::new(len * 0.5, 0.12 * u, l.width * 0.32);
        let g = th(r.x, r.z);
        let rot = Quat::from_rotation_z(pitch);
        out.push(rbox(Vec3::new(r.x, g + rise * 0.5 + half.y * 0.5, r.z), rot, half, stone, 0.02, 0.3));
    }

    // ---- the hangar: a block beside the road, its mouth lit toward it
    if let Some(hp) = l.hangar {
        let hh = Vec3::new(HANGAR_L_OH * u * 0.5, HANGAR_H_OH * u * 0.5, HANGAR_W_OH * u * 0.5);
        let g = th(hp.x, hp.z);
        out.push(rbox(Vec3::new(hp.x, g + hh.y, hp.z), Quat::IDENTITY, hh, dark, 0.03, 0.4));
        let mouth = Vec3::new(l.width * 0.5, 0.15 * u, 0.15 * u);
        let toward = if hp.z > l.road_z { -1.0 } else { 1.0 };
        out.push(rbox(Vec3::new(hp.x, g + ARCH_CLEAR_OH * u * 1.6, hp.z + toward * (hh.z + mouth.z)), Quat::IDENTITY, mouth, accent, 0.9, 0.2));
    }

    // ---- kerb posts
    for k in &l.kerbs {
        let g = th(k.x, k.z);
        let half = Vec3::new(0.12 * u, KERB_H_OH * u * 0.5, 0.12 * u);
        out.push(rbox(Vec3::new(k.x, g + half.y, k.z), Quat::IDENTITY, half, stone, 0.0, 0.2));
        let cap = Vec3::new(0.16 * u, 0.05 * u, 0.16 * u);
        out.push(rbox(Vec3::new(k.x, g + half.y * 2.0 + cap.y, k.z), Quat::IDENTITY, cap, accent, 0.6, 0.2));
    }
    out
}

fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, 1.0]
}

/// Is `p` (window-local) on the boulevard: within its clear width.
pub fn on_boulevard(l: &Layout, p: Vec3) -> bool {
    l.road_in_window && (p.z - l.road_z).abs() <= l.width * 0.5 && p.x.abs() <= l.extent
}

// ------------------------------------------------------------------ tests ---

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(_x: f32, _z: f32) -> f32 {
        0.0
    }
    fn pal() -> ([f32; 4], [f32; 4], [f32; 4]) {
        ([0.5, 0.45, 0.4, 1.0], [1.0, 0.85, 0.5, 1.0], [0.2, 0.2, 0.22, 1.0])
    }
    const EXT: f32 = 1300.0;

    #[test]
    fn the_same_place_from_two_windows() {
        // the property a moving window needs: a feature's WORLD position
        // does not depend on which window generated it
        let a = layout(7, EXT, Vec3::ZERO);
        let b = layout(7, EXT, Vec3::new(700.0, 0.0, -300.0));
        let wa: Vec<Vec3> = a.spires.iter().map(|(p, _)| *p + a.origin).collect();
        let wb: Vec<Vec3> = b.spires.iter().map(|(p, _)| *p + b.origin).collect();
        // everything in the overlap of the two kept areas must match
        let keep = EXT * 0.96;
        let in_both = |w: Vec3| (w - a.origin).x.abs() <= keep && (w - a.origin).z.abs() <= keep && (w - b.origin).x.abs() <= keep && (w - b.origin).z.abs() <= keep;
        let mut checked = 0;
        for w in wa.iter().filter(|w| in_both(**w)) {
            assert!(wb.iter().any(|v| (*v - *w).length() < 1e-3), "spire at world {w:?} seen from A, missing from B");
            checked += 1;
        }
        for w in wb.iter().filter(|w| in_both(**w)) {
            assert!(wa.iter().any(|v| (*v - *w).length() < 1e-3), "spire at world {w:?} seen from B, missing from A");
        }
        assert!(checked > 5, "the overlap held only {checked} spires; the test proves little");
        // the road too: same world z, same arches
        assert!((a.road_z + a.origin.z - (b.road_z + b.origin.z)).abs() < 1e-3);
        let aa: Vec<f32> = a.arches.iter().map(|p| p.x + a.origin.x).filter(|x| (x - b.origin.x).abs() <= keep).collect();
        for x in aa {
            assert!(b.arches.iter().any(|p| (p.x + b.origin.x - x).abs() < 1e-3), "arch at world x {x:.1} missing from B");
        }
    }

    #[test]
    fn density_falls_off_from_the_core_and_never_reaches_zero() {
        let l = layout(3, EXT, Vec3::ZERO);
        let u = oh();
        let count_in = |lo: f32, hi: f32| l.spires.iter().map(|(p, _)| *p).chain(l.blocks.iter().map(|(p, _)| *p)).filter(|p| {
            let r = p.length() / u;
            r >= lo && r < hi
        }).count();
        // per unit AREA: the outer annulus is ten times the core disc
        let core = count_in(0.0, 200.0) as f32 / (200.0f32 * 200.0);
        let outer = count_in(500.0, 800.0) as f32 / (800.0f32 * 800.0 - 500.0 * 500.0);
        assert!(core > outer * 2.0, "core {core:.5}/oh² vs outer {outer:.5}/oh²");
        let outer = count_in(500.0, 800.0);
        assert!(outer > 0, "the plain went empty: a cage with the walls hidden");
        assert!(density(Vec3::new(1.0e6, 0.0, 0.0)) >= DENSITY_FLOOR - 1e-6);
        // a window far out on the plain still gets something to stand next to
        let far = layout(3, EXT, Vec3::new(20_000.0, 0.0, 9_000.0));
        assert!(far.spires.len() + far.blocks.len() > 3, "nothing at all 20 km out");
    }

    #[test]
    fn the_boulevard_is_clear_and_every_arch_can_be_driven_under() {
        let l = layout(3, EXT, Vec3::ZERO);
        let u = oh();
        let (stone, accent, dark) = pal();
        let ps = pieces(&l, &flat, stone, accent, dark);
        for p in &ps {
            if let Sdf::RBox { half, .. } = p.sdf {
                let dz = (p.pos.z - l.road_z).abs();
                let over_road = dz - half.z < l.width * 0.5;
                let low = p.pos.y - half.y < ARCH_CLEAR_OH * u - 1e-3;
                let is_ramp = half.x > 3.0 * u && half.y < 0.2 * u;
                let is_mark = half.y <= 0.08 * u;
                if over_road && low && !is_ramp && !is_mark {
                    panic!("piece in the road at {:?} half {:?}", p.pos, half);
                }
            }
        }
        let lintels = ps.iter().filter(|p| matches!(p.sdf, Sdf::RBox { half, .. } if half.z > l.width * 0.5 && half.y > 0.3 * u && half.y < 1.0 * u && (p.pos.z - l.road_z).abs() < 1e-3)).count();
        assert_eq!(lintels, l.arches.len());
        // and the hangar is beside the road, not across it
        let h = l.hangar.expect("no hangar in the core window");
        assert!((h.z - l.road_z).abs() > l.width * 0.5 + HANGAR_W_OH * u * 0.5 - 1e-3, "the hangar caps the road again");
        assert!(!l.cross_streets.is_empty(), "no cross-streets: a corridor, not a grid");
        for cx in &l.cross_streets {
            for (p, _) in &l.spires {
                assert!((p.x - cx).abs() >= l.width * 0.55 - 1e-3, "a spire stands in a cross-street");
            }
        }
    }

    #[test]
    fn plazas_stand_off_the_road_and_out_of_the_field() {
        let l = layout(11, EXT, Vec3::ZERO);
        let u = oh();
        assert!(!l.plazas.is_empty());
        for p in &l.plazas {
            assert!(!on_boulevard(&l, *p));
            assert!((p.z - l.road_z).abs() - PLAZA_R_OH * u >= l.width * 0.5 + PLAZA_SETBACK_OH * u - 1e-3);
            for (s, _) in &l.spires {
                assert!((*s - *p).length() > PLAZA_R_OH * u * 1.2, "spire {s:?} in plaza {p:?}");
            }
            for (b, _) in &l.blocks {
                assert!((*b - *p).length() > PLAZA_R_OH * u * 1.2, "block {b:?} in plaza {p:?}");
            }
        }
    }

    #[test]
    fn the_kerb_is_broken_and_the_rhythm_is_not_a_metronome() {
        let l = layout(5, EXT, Vec3::ZERO);
        let u = oh();
        // a complete kerb would have 2 posts every KERB_EVERY along the core
        let core_len = 2.0 * CORE_R_OH * u;
        let complete = (core_len / (KERB_EVERY_OH * u)) as usize * 2;
        assert!(l.kerbs.len() < complete, "{} kerb posts of a possible {complete}: a fence", l.kerbs.len());
        // arch gaps vary
        let mut xs: Vec<f32> = l.arches.iter().map(|a| a.x).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let gaps: Vec<f32> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        let mean = gaps.iter().sum::<f32>() / gaps.len().max(1) as f32;
        let var = gaps.iter().map(|g| (g - mean).powi(2)).sum::<f32>() / gaps.len().max(1) as f32;
        assert!(var.sqrt() > mean * 0.05, "arch gaps are a metronome: sd {:.1} on mean {:.1}", var.sqrt(), mean);
    }

    #[test]
    fn the_world_generator_builds_the_window_and_keeps_its_landmarks() {
        let style = crate::world::styles().iter().find(|s| s.name == "DISTRICT").expect("no DISTRICT style");
        let w = crate::world::World::generate(42, style, 0.19, 1.0);
        assert!(!w.plazas.is_empty(), "no plazas on the world");
        assert!(w.boulevard.is_some());
        let lay = layout(42, crate::world::TG_HALF, Vec3::ZERO);
        assert_eq!(w.plazas.len(), lay.plazas.len());
        let ps = pieces(&lay, &|_, _| 0.0, [0.5; 4], [1.0; 4], [0.2; 4]);
        assert!(w.shapes.len() >= ps.len(), "{} shapes for {} pieces", w.shapes.len(), ps.len());
        let (a, b, _) = w.boulevard.unwrap();
        for i in 0..40 {
            let t = i as f32 / 39.0;
            let x = a.x + (b.x - a.x) * t;
            if lay.ramps.iter().any(|r| (r.x - x).abs() <= RAMP_L_OH * oh() * 0.6) {
                continue;
            }
            let g = w.ground_h(x, lay.road_z);
            let d = w.sdf(Vec3::new(x, g + 0.6, lay.road_z));
            assert!(d > 0.2, "something solid on the boulevard at x={x:.0}: sdf {d:.2}");
        }
    }

    #[test]
    fn the_district_palette_has_value_structure() {
        let style = crate::world::styles().iter().find(|s| s.name == "DISTRICT").unwrap();
        let v = crate::valuestruct::Values::of(style);
        assert!(v.stops() >= crate::valuestruct::MIN_STOPS, "{}", crate::valuestruct::report(style));
    }

    #[test]
    fn the_dial_scales_everything_and_only_the_dial() {
        let u = oh();
        assert!((u - crate::scale::RL_BODY_H_M * DISTRICT_K).abs() < 1e-6);
        assert!((ARCH_CLEAR_OH * u - 6.08).abs() < 0.01);
    }
}
