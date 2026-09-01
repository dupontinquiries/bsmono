//! DISTRICT — the open ground, authored in occupant heights.
//!
//! The brief has two demands that the inherited maps cannot meet at
//! once: keep truly open space for the vehicle side of the game, and
//! build architecture that reads as GRAND against a body the size of a
//! Transformer — archways, spires, the Ark's hangar, a district you fight
//! over. skydribble's thirty-six styles scatter rings and balloons for a
//! car and a ball; none of them has a road, and none of them was sized
//! against a body at all.
//!
//! This is a map style that is nothing but a plan: one boulevard the
//! length of the world, grand arches across it at intervals, spires in a
//! grid either side, ramps on the straight for the vehicle enjoyers, a
//! hangar block closing one end, and PLAZAS — rings of low cover set
//! back from the road where a wave stands (`field.rs` musters at the
//! nearest one). The road itself is the terrain between two kerb lines,
//! so it rolls with the ground instead of floating over it.
//!
//! ## One unit, one dial
//!
//! Every dimension here is in OCCUPANT HEIGHTS (`grandeur.rs`): an arch
//! is 4 oh clear, a corridor is 2.5 oh, a hangar is 20 oh, the district
//! is 250 oh across. That is what makes the plan survive the scale
//! decision `docs/PRIORITIES.md` §2 prices and `--grandeur` measures:
//! the body is a 0.38 m car standing in for an 8.5 m robot, and until it
//! grows, a district authored at true oh would be a scale model the car
//! could not drive. So one dial, `DISTRICT_K`, multiplies the unit —
//! today 4.0, so the boulevard is drivable at car speeds; when the body
//! is re-derived to its real size, it goes to 1.0 and nothing else in
//! this file changes. It is the only number here that is not a reading
//! off the references.
//!
//! ## What is a piece
//!
//! `pieces()` returns plain geometry — mesh id, pose, SDF, colour — and
//! `world.rs` pushes each one with its own `push`. The layout is pure
//! (`layout(seed)`), so the tests below can check the boulevard is clear,
//! every arch is tall enough to drive under, and the plazas sit off the
//! road, without a `World` in hand.

use glam::{Quat, Vec3};

use crate::world::{Sdf, M_BOX, M_CAP_TALL, CAP_TALL_HH};

/// The scale placeholder. 4.0 while the body is the sandbox car; 1.0 when
/// it is a robot. See the module doc.
pub const DISTRICT_K: f32 = 4.0;
/// One occupant height in engine metres, under the dial.
pub fn oh() -> f32 {
    crate::scale::RL_BODY_H_M * DISTRICT_K
}

// ---- the plan, in oh. Each is a `grandeur.rs` reference or derived from one.
/// the district across: between Chicago's set piece and a patrol zone
pub const DISTRICT_OH: f32 = 250.0;
/// the boulevard's clear width: a grand corridor is 2.5 oh two abreast;
/// a boulevard robots fight down is wider than that. GUESS x3.
pub const BOULEVARD_W_OH: f32 = 8.0;
/// an archway a Transformer walks under grandly
pub const ARCH_CLEAR_OH: f32 = 4.0;
pub const ARCH_EVERY_OH: f32 = 60.0;
/// spires: Monumental, the Cybertron towers eyeballed at 300 m / 8.5
pub const SPIRE_MIN_OH: f32 = 20.0;
pub const SPIRE_MAX_OH: f32 = 35.0;
pub const SPIRE_EVERY_OH: f32 = 45.0;
/// how far off the boulevard the spire rows stand
pub const SPIRE_SETBACK_OH: f32 = 22.0;
/// the hangar: 20 oh, the Ark's interior
pub const HANGAR_H_OH: f32 = 20.0;
pub const HANGAR_W_OH: f32 = 26.0;
pub const HANGAR_L_OH: f32 = 40.0;
/// a plaza's cover ring: set back from the kerb, front/mid/back within
/// `field.rs`'s zone depths (0..40 m under K=4 is 0..26 oh)
pub const PLAZA_SETBACK_OH: f32 = 14.0;
pub const PLAZA_R_OH: f32 = 9.0;
pub const COVER_H_OH: f32 = 1.1;
/// a jump ramp on the straight: long, low, a vehicle's fun
pub const RAMP_L_OH: f32 = 9.0;
pub const RAMP_RISE_OH: f32 = 1.6;
pub const RAMP_EVERY_OH: f32 = 110.0;
/// kerb posts: every so often, this tall
pub const KERB_EVERY_OH: f32 = 10.0;
pub const KERB_H_OH: f32 = 1.4;

/// The plan. Positions are engine metres, y = 0 (the caller seats each
/// piece on its own ground height).
#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// the boulevard runs along +X from `boulevard.0` to `boulevard.1`
    pub boulevard: (Vec3, Vec3),
    pub width: f32,
    pub arches: Vec<Vec3>,
    /// centre and height (metres) of each spire
    pub spires: Vec<(Vec3, f32)>,
    /// plaza centres, alternating sides, one per two arches
    pub plazas: Vec<Vec3>,
    pub ramps: Vec<Vec3>,
    pub hangar: Vec3,
    pub kerbs: Vec<Vec3>,
}

fn hash(seed: u64, k: u64) -> f32 {
    let x = seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(k.wrapping_mul(0x632BE59BD9B4E019));
    let x = (x ^ (x >> 29)).wrapping_mul(0xBF58476D1CE4E5B9);
    ((x >> 40) & 0xFFFFFF) as f32 / 16_777_215.0
}

/// Lay the district out. `extent_m` is how far the world reaches from the
/// origin on each axis (`world::TG_HALF`), and the plan stays inside 85%
/// of it.
pub fn layout(seed: u64, extent_m: f32) -> Layout {
    let u = oh();
    let half = (DISTRICT_OH * 0.5 * u).min(extent_m * 0.85);
    let mut l = Layout { boulevard: (Vec3::new(-half, 0.0, 0.0), Vec3::new(half, 0.0, 0.0)), width: BOULEVARD_W_OH * u, ..Default::default() };

    // arches down the boulevard, the first a little in from the end
    let mut x = -half + ARCH_EVERY_OH * u * 0.5;
    let mut k = 0u64;
    while x < half - HANGAR_L_OH * u {
        l.arches.push(Vec3::new(x, 0.0, 0.0));
        // a plaza every second arch, alternating sides, set back from the kerb
        if k % 2 == 1 {
            let side = if (k / 2) % 2 == 0 { 1.0 } else { -1.0 };
            let z = side * (l.width * 0.5 + PLAZA_SETBACK_OH * u + PLAZA_R_OH * u);
            l.plazas.push(Vec3::new(x, 0.0, z));
        }
        x += ARCH_EVERY_OH * u;
        k += 1;
    }

    // spires in two rows, jittered so they do not read as a fence
    let mut x = -half + SPIRE_EVERY_OH * u * 0.7;
    let mut k = 100u64;
    while x < half - HANGAR_L_OH * u * 1.2 {
        for side in [-1.0f32, 1.0] {
            let jx = (hash(seed, k) - 0.5) * SPIRE_EVERY_OH * u * 0.35;
            let jz = hash(seed, k + 1) * SPIRE_SETBACK_OH * u * 0.6;
            let h = (SPIRE_MIN_OH + (SPIRE_MAX_OH - SPIRE_MIN_OH) * hash(seed, k + 2)) * u;
            let z = side * (l.width * 0.5 + SPIRE_SETBACK_OH * u + PLAZA_R_OH * u * 2.4 + jz);
            // a spire never stands in a plaza
            let pos = Vec3::new(x + jx, 0.0, z);
            if l.plazas.iter().all(|p| (p.x - pos.x).abs() > PLAZA_R_OH * u * 1.6 || (p.z - pos.z).abs() > PLAZA_R_OH * u * 1.6) {
                l.spires.push((pos, h));
            }
            k += 3;
        }
        x += SPIRE_EVERY_OH * u;
    }

    // ramps on the straight, between arches, on the crown of the road
    let mut x = -half + RAMP_EVERY_OH * u * 0.6;
    while x < half - HANGAR_L_OH * u * 1.5 {
        if l.arches.iter().all(|a| (a.x - x).abs() > RAMP_L_OH * u * 1.5) {
            l.ramps.push(Vec3::new(x, 0.0, 0.0));
        }
        x += RAMP_EVERY_OH * u;
    }

    // the hangar closes the +X end, its mouth on the boulevard
    l.hangar = Vec3::new(half - HANGAR_L_OH * u * 0.5, 0.0, 0.0);

    // kerb posts, both sides, the length of the road
    let mut x = -half;
    while x < half - HANGAR_L_OH * u {
        for side in [-1.0f32, 1.0] {
            // just OUTSIDE the clear width: the posts mark the kerb, they
            // do not stand in the lane (a test drove into the first one)
            l.kerbs.push(Vec3::new(x, 0.0, side * (l.width * 0.5 + 0.14 * u)));
        }
        x += KERB_EVERY_OH * u;
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

/// The district as geometry, each piece seated on `th` (ground height at
/// x, z). `stone`, `accent` and `dark` are the style's palette: the warm
/// gold column, if this is Earth.
pub fn pieces(l: &Layout, th: &dyn Fn(f32, f32) -> f32, stone: [f32; 4], accent: [f32; 4], dark: [f32; 4]) -> Vec<Piece> {
    let u = oh();
    let mut out = Vec::new();

    // ---- arches: two pylons and a lintel, clear height 4 oh, a glow line
    for a in &l.arches {
        let clear = ARCH_CLEAR_OH * u;
        let pylon_half = Vec3::new(0.7 * u, (clear + 1.2 * u) * 0.5, 0.9 * u);
        for side in [-1.0f32, 1.0] {
            let z = side * (l.width * 0.5 + pylon_half.z);
            let g = th(a.x, z);
            out.push(rbox(Vec3::new(a.x, g + pylon_half.y, z), Quat::IDENTITY, pylon_half, stone, 0.02, 0.25));
        }
        let g = th(a.x, 0.0);
        let lintel_half = Vec3::new(0.6 * u, 0.6 * u, l.width * 0.5 + pylon_half.z * 2.0);
        out.push(rbox(Vec3::new(a.x, g + clear + lintel_half.y, 0.0), Quat::IDENTITY, lintel_half, stone, 0.02, 0.3));
        // the elegant part: a thin accent line under the lintel, lit
        let line_half = Vec3::new(0.08 * u, 0.06 * u, lintel_half.z * 0.96);
        out.push(rbox(Vec3::new(a.x, g + clear - line_half.y, 0.0), Quat::IDENTITY, line_half, accent, 0.9, 0.2));
    }

    // ---- spires: tall capsules, a lit band near the top
    for (p, h) in &l.spires {
        // a tall capsule is 10 r high (2 half + 2 r at CAP_TALL_HH = 4)
        let r = h / (2.0 * CAP_TALL_HH + 2.0);
        let half = r * CAP_TALL_HH;
        let g = th(p.x, p.z);
        out.push(Piece { mesh: M_CAP_TALL, pos: Vec3::new(p.x, g + half, p.z), rot: Quat::IDENTITY, scale: Vec3::splat(r), sdf: Sdf::Capsule { r, half }, color: dark, emissive: 0.04, spec: 0.45 });
        let band = Vec3::new(r * 1.06, 0.12 * u, r * 1.06);
        out.push(rbox(Vec3::new(p.x, g + half * 1.7, p.z), Quat::IDENTITY, band, accent, 0.8, 0.3));
    }

    // ---- plazas: a ring of low cover, a lit marker in the middle
    for c in &l.plazas {
        let n = 10;
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let x = c.x + a.cos() * PLAZA_R_OH * u;
            let z = c.z + a.sin() * PLAZA_R_OH * u;
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
        // pivot at the low end: the box's centre rises by half the rise
        let rot = Quat::from_rotation_z(pitch);
        out.push(rbox(Vec3::new(r.x, g + rise * 0.5 + half.y * 0.5, r.z), rot, half, stone, 0.02, 0.3));
    }

    // ---- the hangar: a block with its mouth cut toward the boulevard
    {
        let hh = Vec3::new(HANGAR_L_OH * u * 0.5, HANGAR_H_OH * u * 0.5, HANGAR_W_OH * u * 0.5);
        let g = th(l.hangar.x, l.hangar.z);
        out.push(rbox(Vec3::new(l.hangar.x, g + hh.y, l.hangar.z), Quat::IDENTITY, hh, dark, 0.03, 0.4));
        // the mouth: a cutter would carve it; without CSG here, the lit
        // lintel over the opening does the reading
        let mouth = Vec3::new(0.15 * u, 0.15 * u, l.width * 0.5);
        out.push(rbox(Vec3::new(l.hangar.x - hh.x - mouth.x, g + ARCH_CLEAR_OH * u * 1.6, l.hangar.z), Quat::IDENTITY, mouth, accent, 0.9, 0.2));
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

/// Is `p` on the boulevard: within its clear width, between its ends.
pub fn on_boulevard(l: &Layout, p: Vec3) -> bool {
    p.x >= l.boulevard.0.x && p.x <= l.boulevard.1.x && p.z.abs() <= l.width * 0.5
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

    #[test]
    fn the_plan_is_in_oh_and_fits_the_world() {
        let l = layout(7, 1300.0);
        let u = oh();
        let across = l.boulevard.1.x - l.boulevard.0.x;
        assert!((across - DISTRICT_OH * u).abs() < 1.0 || across <= 1300.0 * 0.85 * 2.0 + 1.0);
        assert!((l.width - BOULEVARD_W_OH * u).abs() < 1e-3);
        assert!(!l.arches.is_empty() && !l.spires.is_empty() && !l.plazas.is_empty() && !l.ramps.is_empty());
        for (_, h) in &l.spires {
            assert!(*h >= SPIRE_MIN_OH * u - 1e-3 && *h <= SPIRE_MAX_OH * u + 1e-3);
        }
        // deterministic
        let m = layout(7, 1300.0);
        assert_eq!(l.spires.len(), m.spires.len());
        assert_eq!(l.spires[0].0, m.spires[0].0);
        let n = layout(8, 1300.0);
        assert_ne!(l.spires[0].0, n.spires[0].0, "the seed does nothing");
    }

    #[test]
    fn the_boulevard_is_clear_and_every_arch_can_be_driven_under() {
        let l = layout(3, 1300.0);
        let u = oh();
        let (stone, accent, dark) = pal();
        let ps = pieces(&l, &flat, stone, accent, dark);
        for p in &ps {
            // anything whose footprint crosses the road must be above the arch clearance
            if let Sdf::RBox { half, .. } = p.sdf {
                let over_road = (p.pos.z - half.z).abs() < l.width * 0.5 || p.pos.z.abs() < l.width * 0.5;
                let low = p.pos.y - half.y < ARCH_CLEAR_OH * u - 1e-3;
                let is_ramp = half.x > 3.0 * u && half.y < 0.2 * u && p.pos.z.abs() < 1e-3;
                let is_mark = half.y <= 0.08 * u;
                if over_road && low && !is_ramp && !is_mark && p.pos.x < l.hangar.x - HANGAR_L_OH * u * 0.5 {
                    // pylons stand OUTSIDE the width: their inner face is at the kerb
                    assert!(p.pos.z.abs() - half.z >= l.width * 0.5 - 1e-3, "piece in the road at {:?} half {:?}", p.pos, half);
                }
            }
        }
        // the lintels sit at exactly the clearance
        // a lintel spans the road and is slab-thin; the hangar spans it too and is not
        let lintels: Vec<&Piece> = ps.iter().filter(|p| matches!(p.sdf, Sdf::RBox { half, .. } if half.z > l.width * 0.5 && half.y > 0.3 * u && half.y < 1.0 * u)).collect();
        assert_eq!(lintels.len(), l.arches.len());
        for p in lintels {
            if let Sdf::RBox { half, .. } = p.sdf {
                assert!((p.pos.y - half.y - ARCH_CLEAR_OH * u).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn plazas_stand_off_the_road_and_inside_the_field_zones() {
        let l = layout(11, 1300.0);
        let u = oh();
        for p in &l.plazas {
            assert!(!on_boulevard(&l, *p));
            assert!(p.z.abs() - PLAZA_R_OH * u >= l.width * 0.5 + PLAZA_SETBACK_OH * u - 1e-3);
            // a wave musters within field::DEPTH's 40 m of its anchor; the
            // cover ring is inside that at K = 4
            assert!(PLAZA_R_OH * u < 40.0, "cover ring {:.1} m wider than the back zone", PLAZA_R_OH * u);
        }
        // spires keep out of plazas
        for (s, _) in &l.spires {
            for p in &l.plazas {
                let d = (*s - *p).length();
                assert!(d > PLAZA_R_OH * u * 1.2, "spire {s:?} in plaza {p:?}");
            }
        }
    }

    #[test]
    fn the_world_generator_builds_the_district_and_keeps_its_landmarks() {
        let style = crate::world::styles().iter().find(|s| s.name == "DISTRICT").expect("no DISTRICT style");
        let w = crate::world::World::generate(42, style, 0.19, 1.0);
        assert!(!w.plazas.is_empty(), "no plazas on the world");
        assert!(w.boulevard.is_some());
        let lay = layout(42, crate::world::TG_HALF);
        assert_eq!(w.plazas.len(), lay.plazas.len());
        // the plan's pieces are in the shape list (plus the generator's own spawn plinths etc.)
        let ps = pieces(&lay, &|_, _| 0.0, [0.5; 4], [1.0; 4], [0.2; 4]);
        assert!(w.shapes.len() >= ps.len(), "{} shapes for {} pieces", w.shapes.len(), ps.len());
        // and the boulevard is drivable: ground under the road is the terrain, nothing solid at head height
        let (a, b, _) = w.boulevard.unwrap();
        for i in 0..40 {
            let t = i as f32 / 39.0;
            let x = a.x + (b.x - a.x) * t;
            if x > lay.hangar.x - HANGAR_L_OH * oh() * 0.5 {
                break;
            }
            // the ramps are ON the road by design: a wedge you drive up
            if lay.ramps.iter().any(|r| (r.x - x).abs() <= RAMP_L_OH * oh() * 0.6) {
                continue;
            }
            let g = w.ground_h(x, 0.0);
            let d = w.sdf(Vec3::new(x, g + 0.6, 0.0));
            assert!(d > 0.2, "something solid on the boulevard at x={x:.0}: sdf {d:.2}");
        }
    }

    #[test]
    fn the_district_palette_has_value_structure() {
        // C13's audit: MEADOW read as a demo at 0.58 stops; the floor is 0.8.
        // The Earth column is warm and gold, and warm is not the same as flat.
        let style = crate::world::styles().iter().find(|s| s.name == "DISTRICT").unwrap();
        let v = crate::valuestruct::Values::of(style);
        assert!(v.stops() >= crate::valuestruct::MIN_STOPS, "{}", crate::valuestruct::report(style));
    }

    #[test]
    fn the_dial_scales_everything_and_only_the_dial() {
        // every dimension is oh times a constant: at K=4 the arch is 4 oh
        // clear, which is 4 * 0.38 * 4 = 6.08 m
        let u = oh();
        assert!((u - crate::scale::RL_BODY_H_M * DISTRICT_K).abs() < 1e-6);
        assert!((ARCH_CLEAR_OH * u - 6.08).abs() < 0.01);
    }
}
