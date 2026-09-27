// Procedural voxel character builders — 13 feline ancestors (per-pelt
// signatures, crest colours), minions, towers, titans, projectiles.
// Everything is authored in voxel units then meshed at a per-model voxel size.

use crate::consts::{Team, VOXEL_SIZE};
use crate::gods::{GodDef, PeltKind, Role, WeaponKind, PAL_ACCENT, PAL_BODY, PAL_CLOTH, PAL_METAL};
use crate::voxel::{Grid, VoxelMesh};

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn solid(c: [f32; 3]) -> [f32; 4] {
    [c[0], c[1], c[2], 1.0]
}

/// Team-tinted ancestor palette: the collar takes most of the team colour,
/// the crest keeps its own identity (that's the "who" axis; see gods.rs).
pub fn god_palette(def: &GodDef, team: Team) -> [[f32; 3]; 4] {
    let mut p = def.palette;
    let tc = team.rgb();
    p[PAL_CLOTH] = mix(p[PAL_CLOTH], tc, 0.65);
    p
}

// ---------------------------------------------------------------------------
// Ancestors — quadruped felines, ~36 voxels tall at the ear (voxel 0.05 →
// ~1.8u), 44 voxels nose-to-tail. Model faces +Z. Meshed at `GV` and drawn at
// `GOD_DRAW_VOXEL` (a hair larger than the spacing, so cubes seal without the
// 2× overlap the old humanoids used) — twice the linear voxel density of the
// original 0.07/0.14 bodies.
// ---------------------------------------------------------------------------

/// Ancestor voxel spacing (world units).
pub const GV: f32 = 0.05;
/// Cube edge used when drawing ancestor meshes.
pub const GOD_DRAW_VOXEL: f32 = 0.055;
// Grid: 30 x 50 x 56, body centred at (CX, _, CZ). It was 30 x 42 x 48, which
// silently clipped two things `Grid::set` drops without a word: the antlers
// above y = 41 (Stillsmoke lost the top four voxels of each beam) and the
// tail tip below z = 0. `cz` stays at nz / 2 so the mesh origin is unmoved.
const H: i32 = 50;
const NZ: usize = 56;
const CX: i32 = 15;
const CZ: i32 = 28;

fn role_bulk(role: Role) -> i32 {
    match role {
        Role::Guardian => 2,
        Role::Warrior => 1,
        Role::Mage | Role::Hunter => 0,
        Role::Assassin => -1,
    }
}

pub fn build_god(def: &GodDef, team: Team) -> VoxelMesh {
    god_grid(def, team).mesh(GV, true)
}

/// An ancestor cut into posable parts, with the rig that poses them.
pub struct RiggedGod {
    pub meshes: [VoxelMesh; crate::rigged::SLOTS],
    pub rig: crate::rigged::QuadRig,
}

pub fn build_god_rigged(def: &GodDef, team: Team) -> RiggedGod {
    let bulk = role_bulk(def.role);
    let plan = feline_cut_plan(bulk, matches!(def.pelt, PeltKind::Antlers));
    RiggedGod { meshes: plan.cut(&god_grid(def, team), GV), rig: feline_rig(bulk) }
}

/// Leg column geometry shared by the authoring, the cut and the rig, so the
/// three cannot disagree about where a leg is.
/// Returns `(x, z, half_thickness)` per leg in `[FL, FR, HL, HR]` order.
/// The model faces +Z, which makes its LEFT side +X.
fn leg_columns(bulk: i32) -> [(i32, i32, i32); 4] {
    let lt = 1 + (bulk > 0) as i32;
    let lx = 5 + bulk.max(0);
    [(CX + lx, CZ + 11, lt), (CX - lx, CZ + 11, lt), (CX + lx, CZ - 10, lt), (CX - lx, CZ - 10, lt)]
}
/// Legs are authored 0..=LEG_TOP; the belly starts a little above CUT_Y.
const LEG_TOP: i32 = 19;
const LEG_CUT_Y: i32 = 14;

pub fn feline_cut_plan(bulk: i32, antlers: bool) -> crate::rigged::CutPlan {
    use crate::rigged::{Region, Slot, LEG_SLOTS, TAIL_SLOTS};
    let mut regions = Vec::new();
    for (i, (x, z, lt)) in leg_columns(bulk).into_iter().enumerate() {
        let (x0, x1, z0, z1) = (x - lt - 1, x + lt + 1, z - lt - 1, z + lt + 3);
        regions.push(Region { slot: LEG_SLOTS[i], min: [x0, 0, z0], max: [x1, LEG_CUT_Y, z1], exclusive: true });
        // The shoulder: in the leg AND the torso, so neither shows a stump.
        regions.push(Region { slot: LEG_SLOTS[i], min: [x0, LEG_CUT_Y + 1, z0], max: [x1, LEG_TOP, z1], exclusive: false });
    }
    regions.push(Region { slot: Slot::Head, min: [0, 18, CZ + 17], max: [29, H - 1, NZ as i32 - 1], exclusive: true });
    regions.push(Region { slot: Slot::Head, min: [0, 22, CZ + 14], max: [29, H - 1, CZ + 16], exclusive: false });
    if antlers {
        // The beams sweep BACK past the head cut; without this the head
        // turns and leaves half its antlers behind.
        regions.push(Region { slot: Slot::Head, min: [0, 36, CZ + 10], max: [29, H - 1, CZ + 16], exclusive: true });
    }
    // Tail: four bands matching the rig's four chain segments. The first
    // voxels of the tail stay buried in the rump as torso.
    // The authored tail runs z = CZ-17 .. CZ-25 with a 1.6 radius, and the
    // rump covers it down to CZ-18, so eight voxels show: two per segment.
    let bands = [(CZ - 20, CZ - 19), (CZ - 22, CZ - 21), (CZ - 24, CZ - 23), (0, CZ - 25)];
    for (k, (z0, z1)) in bands.into_iter().enumerate() {
        regions.push(Region { slot: TAIL_SLOTS[k], min: [0, 18, z0], max: [29, H - 1, z1], exclusive: true });
    }
    crate::rigged::CutPlan { regions }
}

pub fn feline_rig(bulk: i32) -> crate::rigged::QuadRig {
    // voxel -> rig frame (X forward, Y up, Z right; model left is +X).
    const BODY_Y: f32 = 24.0;
    let at = |x: f32, y: f32, z: f32| [(z - CZ as f32) * GV, (y - BODY_Y) * GV, -(x - CX as f32) * GV];
    let cols = leg_columns(bulk);
    let leg = |i: usize| {
        let (x, z, lt) = cols[i];
        let top = LEG_TOP as f32 + 1.0;
        (at(x as f32 + 0.5, top * 0.5, z as f32 + 0.5), [(lt as f32 + 1.5) * GV, top * 0.5 * GV, (lt as f32 + 0.5) * GV])
    };
    crate::rigged::QuadRig::new(
        glam::Vec3::new(0.0, BODY_Y * GV, 0.0),
        [18.0 * GV, 7.5 * GV, (7.0 + bulk as f32) * GV],
        leg(0), leg(1), leg(2), leg(3),
        (at(CX as f32, 32.5, CZ as f32 + 21.5), [5.5 * GV, 5.0 * GV, 5.0 * GV]),
        (at(CX as f32, 27.0, CZ as f32 - 22.5), [4.0 * GV, 1.5 * GV, 1.5 * GV]),
    )
}

fn god_grid(def: &GodDef, team: Team) -> Grid {
    let pal = god_palette(def, team);
    let pelt = pal[PAL_BODY];
    let collar = pal[PAL_CLOTH];
    let second = pal[PAL_METAL];
    let accent = pal[PAL_ACCENT];
    let dark = mix(pelt, [0.05, 0.04, 0.05], 0.7);

    let bulk = role_bulk(def.role);
    let bf = bulk as f32;
    let crag = matches!(def.pelt, PeltKind::Crag);
    let extra = if crag { 1.0 } else { 0.0 };

    let mut g = Grid::new(30, H as usize, NZ);
    let cx = CX;
    let cz = CZ;
    let (cxf, czf) = (cx as f32, cz as f32);

    // ── legs + paws (y 0..18) ────────────────────────────────────────────
    let leg_col = solid(mix(pelt, dark, 0.15));
    for (x, z, lt) in leg_columns(bulk) {
        g.filled_box(x - lt, 0, z - lt - 1, x + lt, LEG_TOP, z + lt + 1, leg_col);
        // paw: wider, flatter, secondary-toned toes
        g.filled_box(x - lt - 1, 0, z - lt - 1, x + lt + 1, 2, z + lt + 3, solid(mix(pelt, second, 0.35)));
        for tx in -1..=1 {
            g.set(x + tx, 0, z + lt + 3, solid(dark)); // claws
        }
    }

    // ── torso: chest + hindquarters + belly ──────────────────────────────
    g.ellipsoid(cxf, 24.0, czf + 8.0, 7.0 + bf + extra, 7.0 + extra, 10.0, solid(pelt));
    g.ellipsoid(cxf, 23.0, czf - 8.0, 6.5 + bf + extra, 6.5 + extra, 10.0, solid(pelt));
    g.ellipsoid(cxf, 20.0, czf, 5.0 + bf, 3.0, 13.0, solid(second));

    // ── tail: arcs back and up, ring near the base in team colour ────────
    for i in 0..16 {
        let t = i as f32 / 15.0;
        let sway = (t * 3.0).sin() * 1.5;
        let x = cxf + sway;
        let y = 22.0 + t * t * 11.0;
        let z = czf - 17.0 - t * 8.0;
        let c = if i >= 14 { accent } else if (2..=3).contains(&i) { collar } else { pelt };
        g.ellipsoid(x, y, z, 1.4, 1.4, 1.6, solid(c));
    }

    // ── mane / shoulders (before the head so the face is re-carved on top) ─
    match def.pelt {
        PeltKind::Mane => {
            g.ellipsoid(cxf, 29.0, czf + 15.0, 9.0 + bf, 9.0, 7.0, solid(mix(second, dark, 0.55)));
            g.ellipsoid(cxf, 27.0, czf + 12.0, 8.0 + bf, 6.0, 8.0, solid(mix(second, dark, 0.35)));
        }
        PeltKind::Hulk => {
            g.ellipsoid(cxf, 28.0, czf + 9.0, 10.0 + bf, 6.5, 9.0, solid(mix(pelt, dark, 0.25)));
            // ragged mane rows
            for i in 0..6 {
                let z = czf + 6.0 + i as f32 * 2.0;
                g.ellipsoid(cxf, 33.0 + (i % 2) as f32, z, 4.0, 2.5, 1.2, solid(mix(second, dark, 0.5)));
            }
        }
        _ => {}
    }

    // ── neck + team collar ───────────────────────────────────────────────
    g.ellipsoid(cxf, 28.0, czf + 16.0, 4.5 + bf * 0.5, 4.5, 5.0, solid(pelt));
    g.ellipsoid(cxf, 27.0, czf + 14.0, 5.5 + bf * 0.5, 5.5, 1.5, solid(collar));

    // ── head, muzzle, nose, eyes ─────────────────────────────────────────
    let hy = 32.0;
    let hz = czf + 21.0;
    g.ellipsoid(cxf, hy, hz, 5.0 + bf * 0.5, 5.0, 5.5, solid(pelt));
    g.ellipsoid(cxf, hy - 2.0, hz + 4.5, 3.0, 2.6, 3.0, solid(second));
    g.set(cx, hy as i32 - 1, hz as i32 + 7, solid([0.08, 0.05, 0.06]));
    g.set(cx - 1, hy as i32 - 1, hz as i32 + 7, solid([0.08, 0.05, 0.06]));
    // eyes: accent with a bright core, read at distance
    let eye = [accent[0].max(0.55), accent[1].max(0.55), accent[2].max(0.35)];
    for sx in [-1i32, 1] {
        g.set(cx + sx * 2, hy as i32 + 1, hz as i32 + 5, solid(eye));
        g.set(cx + sx * 3, hy as i32 + 1, hz as i32 + 4, solid(dark));
    }
    // brow ridge
    g.filled_box(cx - 3, hy as i32 + 2, hz as i32 + 4, cx + 3, hy as i32 + 2, hz as i32 + 4, solid(mix(pelt, dark, 0.3)));

    // ── ears ─────────────────────────────────────────────────────────────
    let (ear_h, ear_w, tuft): (i32, i32, bool) = match def.pelt {
        PeltKind::EarTufts => (5, 1, true),
        PeltKind::BigEars => (7, 2, false),
        // long, lean ears (one notched, below)
        PeltKind::Hollow => (5, 1, false),
        // a sand cat's ears: broad and low, set wide
        PeltKind::Lamp => (3, 2, false),
        _ => (3, 1, false),
    };
    for sx in [-1i32, 1] {
        let ex = cx + sx * (3 + bulk.max(0) / 2);
        let ez = hz as i32 - 1;
        for i in 0..ear_h {
            let w = (ear_w - i / 3).max(0);
            g.filled_box(ex - w, hy as i32 + 4 + i, ez - 1, ex + w, hy as i32 + 4 + i, ez + 1, solid(pelt));
            if w >= 1 {
                g.set(ex, hy as i32 + 4 + i, ez + 1, solid(second)); // inner ear
            }
        }
        if tuft {
            g.set(ex, hy as i32 + 4 + ear_h, ez, solid(dark));
            g.set(ex, hy as i32 + 5 + ear_h, ez, solid(dark));
        }
    }

    // ── pelt signatures ──────────────────────────────────────────────────
    match def.pelt {
        PeltKind::Stripes => {
            let stripe = solid(mix(dark, [0.1, 0.06, 0.04], 0.5));
            paint_shell(&mut g, |x, y, z| {
                let flank = (z - cz).abs() < 18 && y > 15 && (x - cx).abs() >= 3;
                flank && ((z + (y / 6)) % 4 == 0)
            }, stripe);
        }
        PeltKind::Rosettes => {
            let spot = solid(mix(dark, pelt, 0.25));
            paint_shell(&mut g, |x, y, z| {
                if y < 14 || (z - cz).abs() > 20 {
                    return false;
                }
                let h = ((x * 7 + z * 13 + y * 3) as u32).wrapping_mul(2654435761) >> 24;
                let cell = ((z.rem_euclid(5)) == 2) && ((x + y).rem_euclid(5) == 2);
                cell && h > 60
            }, spot);
        }
        PeltKind::Shadow => {
            // faint moonlight rim on the spine only; the rest stays black
            paint_shell(&mut g, |_x, y, z| y >= 29 && (z - cz).abs() < 12, solid(mix(pelt, [0.35, 0.35, 0.5], 0.25)));
        }
        PeltKind::Sabers => {
            for sx in [-1i32, 1] {
                g.filled_box(cx + sx * 2, hy as i32 - 6, hz as i32 + 6, cx + sx * 2, hy as i32 - 3, hz as i32 + 6, solid([0.96, 0.95, 0.9]));
                g.set(cx + sx * 2, hy as i32 - 7, hz as i32 + 6, solid([0.9, 0.85, 0.8]));
            }
        }
        PeltKind::SunDisc => {
            // a floating disc of noon light above the brow + a sun glyph
            for a in 0..28 {
                let ang = a as f32 / 28.0 * std::f32::consts::TAU;
                let x = cxf + ang.cos() * 5.0;
                let y = hy + 8.0 + ang.sin() * 5.0;
                g.set(x.round() as i32, y.round() as i32, hz as i32 - 2, solid(accent));
            }
            g.set(cx, hy as i32 + 3, hz as i32 + 4, solid(accent));
        }
        PeltKind::Comet => {
            // stars on the back: accent specks on the upper shell
            paint_shell(&mut g, |x, y, z| {
                if y < 24 || (z - cz).abs() > 18 {
                    return false;
                }
                let h = ((x * 31 + z * 17 + y * 7) as u32).wrapping_mul(2654435761) >> 24;
                h > 238
            }, solid(accent));
            // a starmark blaze on the brow
            g.set(cx, hy as i32 + 3, hz as i32 + 4, solid(accent));
            g.set(cx, hy as i32 + 4, hz as i32 + 3, solid(accent));
            // the tail streams light: a comet's plume rising off its tip,
            // fading from the accent to the belly's pale silver (up, not
            // back: the grid ends a few cells behind the tail)
            for i in 0..8 {
                let t = i as f32 / 7.0;
                let (x, y, z) = (cxf + (t * 5.0).sin() * 0.8, 33.0 + t * 7.0, czf - 24.0 - t * 1.5);
                let r = 1.4 * (1.0 - t * 0.6);
                g.ellipsoid(x, y, z, r, r, r * 1.3, solid(mix(accent, second, t)));
            }
        }
        PeltKind::Lamp => {
            // a brass lantern hung at the breast from the collar: a small
            // frame of brass round a bright flame, a ring on top; faint
            // bars on the legs (a sand cat's)
            let brass = solid([0.72, 0.52, 0.20]);
            let lz = czf as i32 + 19;
            g.filled_box(cx - 1, 20, lz, cx + 1, 23, lz + 2, brass);
            g.filled_box(cx, 21, lz + 1, cx, 22, lz + 2, solid([1.0, 0.92, 0.55]));
            g.set(cx, 24, lz + 1, brass);
            g.set(cx, 25, lz + 1, brass);
            paint_shell(&mut g, |_x, y, _z| (6..=12).contains(&y) && y % 3 == 0, solid(mix(pelt, dark, 0.35)));
        }
        PeltKind::Hollow => {
            // a hood of dark fur over the brow and the eyes, falling back
            // over the crown; two pale eyes looking out of it; the left
            // ear notched; a thin dark line down the spine
            let hood = solid(mix(dark, pelt, 0.12));
            g.ellipsoid(cxf, hy + 2.2, hz + 0.5, 5.3 + bf * 0.5, 2.6, 5.2, hood);
            g.ellipsoid(cxf, hy + 1.0, hz + 3.8, 3.8, 1.5, 1.8, hood);
            for sx in [-1i32, 1] {
                g.set(cx + sx * 2, hy as i32 + 1, hz as i32 + 5, solid([0.86, 0.93, 1.0]));
                g.set(cx + sx * 2, hy as i32 + 1, hz as i32 + 6, solid([0.86, 0.93, 1.0]));
            }
            let ex = cx - (3 + bulk.max(0) / 2);
            g.carve_sphere(ex as f32, hy + 6.5, hz - 0.5, 0.9);
            paint_shell(&mut g, |x, y, _z| (x - cx).abs() <= 0 && y >= 27, solid(dark));
        }
        PeltKind::Kit => {
            // a kitten: the head rounder and bigger than any grown cat's
            // (still inside the head's cut), a white bib and muzzle, eyes
            // huge and bright with a catchlight, a soft fluff on the crown
            // -- and a green sprout of two leaves growing out of it
            g.ellipsoid(cxf, hy + 0.5, hz - 0.5, 6.4, 6.0, 6.0, solid(pelt));
            g.ellipsoid(cxf, hy - 2.5, hz + 4.5, 3.4, 2.8, 2.6, solid(second));
            g.ellipsoid(cxf, 26.0, czf + 17.0, 4.0, 3.5, 2.5, solid(second));
            g.set(cx, hy as i32 - 1, hz as i32 + 6, solid([0.95, 0.55, 0.60]));
            let white = [0.98, 0.98, 1.0];
            // big eyes set in the front of the round head: a dark rim, a
            // wide bright iris, a catchlight
            for sx in [-1i32, 1] {
                let ex = cx + sx * 3;
                let ez = hz as i32 + 4;
                g.filled_box(ex - 1, hy as i32 - 1, ez + 1, ex + 1, hy as i32 + 2, ez + 1, solid([0.06, 0.05, 0.08]));
                g.filled_box(ex.min(ex - sx), hy as i32, ez + 2, ex.max(ex - sx), hy as i32 + 1, ez + 2, solid(eye));
                g.set(ex, hy as i32, ez + 2, solid([0.04, 0.04, 0.06]));
                g.set(ex - sx, hy as i32 + 1, ez + 2, solid(white));
            }
            g.ellipsoid(cxf, hy + 5.5, hz - 1.0, 2.2, 1.4, 2.0, solid(mix(pelt, second, 0.4)));
            // the sprout: a stem and two round leaves
            let leaf = solid(accent);
            let stem = solid(mix(accent, [0.2, 0.4, 0.1], 0.5));
            for y in 0..4 {
                g.set(cx, hy as i32 + 7 + y, hz as i32 - 1, stem);
            }
            g.ellipsoid(cxf - 1.8, hy + 11.0, hz - 1.0, 1.8, 0.8, 1.2, leaf);
            g.ellipsoid(cxf + 1.8, hy + 11.5, hz - 1.0, 1.8, 0.8, 1.2, leaf);
        }
        PeltKind::Antlers => {
            // antlers: a main beam sweeping up and back with two tines each
            for sx in [-1i32, 1] {
                let bx = cx + sx * 3;
                for i in 0..9 {
                    let x = bx + sx * (i / 3);
                    let y = hy as i32 + 5 + i;
                    let z = hz as i32 - 2 - i / 2;
                    g.set(x, y, z, solid([0.55, 0.45, 0.3]));
                    if i == 3 || i == 6 {
                        for k in 1..=2 {
                            g.set(x + sx * k, y + k, z + 1, solid([0.62, 0.52, 0.36]));
                        }
                    }
                }
                // saber fangs, like the ninth son's father
                g.filled_box(cx + sx * 2, hy as i32 - 5, hz as i32 + 6, cx + sx * 2, hy as i32 - 3, hz as i32 + 6, solid([0.95, 0.93, 0.88]));
            }
            // scales down the back in a checker of the crest colour
            paint_shell(&mut g, |x, y, z| y >= 27 && (z - cz).abs() < 16 && (x + z).rem_euclid(2) == 0, solid(mix(pelt, accent, 0.45)));
        }
        _ => {}
    }

    // ── crest: a tall accent ridge along the spine, the "who" at distance ─
    if !matches!(def.pelt, PeltKind::SunDisc) {
        for z in (cz - 10..=cz + 12).step_by(2) {
            let mut top = None;
            for y in (0..H).rev() {
                if g.get(cx, y, z).is_some() {
                    top = Some(y);
                    break;
                }
            }
            if let Some(top) = top {
                let h = if (z - cz).abs() < 5 { 2 } else { 1 };
                g.filled_box(cx, top + 1, z, cx, top + h, z, solid(accent));
            }
        }
    }

    // team war-paint: a band on the front legs
    for (x, z, lt) in &leg_columns(bulk)[0..2] {
        g.filled_box(x - lt - 1, 9, z - lt - 1, x + lt + 1, 10, z + lt + 1, solid(team.rgb()));
    }

    g
}

#[cfg(test)]
mod rig_tests {
    use super::*;
    use crate::gods::ROSTER;
    use crate::rigged::{Slot, SLOTS};

    /// Every ancestor, both teams: no slot empty, and the cut loses no
    /// voxel. An empty slot draws a cat with a missing leg; a lost voxel is
    /// a hole that only shows in one pose.
    #[test]
    fn every_ancestor_cuts_into_ten_nonempty_parts_and_loses_nothing() {
        assert_eq!(ROSTER.len(), 17);
        for def in ROSTER.iter() {
            let grid = god_grid(def, Team::Blue);
            let plan = feline_cut_plan(role_bulk(def.role), matches!(def.pelt, PeltKind::Antlers));
            let parts = plan.cut(&grid, GV);
            for (i, m) in parts.iter().enumerate() {
                assert!(m.inst.len() >= 8, "{}: slot {i} has {} voxels", def.name, m.inst.len());
            }
            let mut filled = 0usize;
            for z in 0..grid.nz as i32 {
                for y in 0..grid.ny as i32 {
                    for x in 0..grid.nx as i32 {
                        if grid.get(x, y, z).is_some() {
                            filled += 1;
                            let owners = (0..SLOTS).filter(|s| plan.in_slot(slot(*s), x, y, z)).count();
                            assert!(owners >= 1, "{}: voxel ({x},{y},{z}) belongs to no part", def.name);
                        }
                    }
                }
            }
            assert!(filled > 5000, "{}: only {filled} voxels -- did the grid build?", def.name);
            // Hull culling should more than pay for the duplicated joints.
            let cut_total: usize = parts.iter().map(|m| m.inst.len()).sum();
            assert!(cut_total < filled, "{}: cut {cut_total} vs solid {filled}", def.name);
        }
    }

    fn slot(i: usize) -> Slot {
        [Slot::Torso, Slot::LegFL, Slot::LegFR, Slot::LegHL, Slot::LegHR, Slot::Head, Slot::Tail0, Slot::Tail1, Slot::Tail2, Slot::Tail3][i]
    }

    /// The legs the cut plan carves and the legs the rig pivots must be the
    /// same legs: the hip sits on the column's axis at the top of the box,
    /// and the soles are on the floor.
    #[test]
    fn rig_hips_sit_on_the_authored_leg_columns() {
        for bulk in [-1, 0, 1, 2] {
            let rig = feline_rig(bulk);
            let cols = leg_columns(bulk);
            for i in 0..4 {
                let hip = rig.hip(i);
                let world_y = rig.body_centre.y + hip[1];
                assert!((world_y - (LEG_TOP as f32 + 1.0) * GV).abs() < 1e-4, "hip {i} at y {world_y}");
                assert!((rig.body_centre.y + rig.sole(i)).abs() < 1e-4, "sole {i} is not on the floor");
                // FL/HL are LEFT: rig z < 0, model x > CX.
                assert_eq!(hip[2] < 0.0, cols[i].0 > CX, "leg {i} is on the wrong side");
                assert_eq!(hip[0] > 0.0, i < 2, "leg {i} is at the wrong end");
            }
            let ph = crate::anim::Gait::Walk.leg_phases();
            assert_eq!(ph.len(), rig.parts().iter().filter(|p| p.role == creature_rig::Role::Leg).count());
        }
    }

    /// The grid used to clip the antlers at y = 41 and the tail at z = 0.
    #[test]
    fn nothing_is_authored_against_the_grid_wall() {
        for def in ROSTER.iter() {
            let g = god_grid(def, Team::Red);
            let mut top = 0;
            let mut back = i32::MAX;
            for z in 0..g.nz as i32 {
                for y in 0..g.ny as i32 {
                    for x in 0..g.nx as i32 {
                        if g.get(x, y, z).is_some() {
                            top = top.max(y);
                            back = back.min(z);
                        }
                    }
                }
            }
            assert!(top < g.ny as i32 - 1, "{}: touches the ceiling at y {top}", def.name);
            assert!(back > 0, "{}: touches the back wall", def.name);
        }
    }
}

/// Recolour existing *surface* voxels (any with an empty neighbour) where
/// `pred` holds — used for stripes, rosettes and scales so markings follow
/// the body's silhouette instead of being painted through it.
fn paint_shell(g: &mut Grid, pred: impl Fn(i32, i32, i32) -> bool, c: [f32; 4]) {
    let mut hits = Vec::new();
    for z in 0..48 {
        for y in 0..H {
            for x in 0..30 {
                if g.get(x, y, z).is_none() || !pred(x, y, z) {
                    continue;
                }
                let surface = g.get(x + 1, y, z).is_none()
                    || g.get(x - 1, y, z).is_none()
                    || g.get(x, y + 1, z).is_none()
                    || g.get(x, y, z + 1).is_none()
                    || g.get(x, y, z - 1).is_none();
                if surface {
                    hits.push((x, y, z));
                }
            }
        }
    }
    for (x, y, z) in hits {
        g.set(x, y, z, c);
    }
}

// ---------------------------------------------------------------------------
// Prey — a hare (voxel 0.05, ~0.45u at the ear) and a stag (voxel 0.07,
// ~1.7u at the antler). Both face +Z like the ancestors.
// ---------------------------------------------------------------------------

pub const HARE_VOXEL: f32 = 0.05;
pub const STAG_VOXEL: f32 = 0.07;

pub fn build_hare() -> VoxelMesh {
    let mut g = Grid::new(10, 12, 14);
    let (cx, cz) = (5, 7);
    let fur = [0.58, 0.48, 0.36];
    let belly = [0.85, 0.8, 0.7];
    let dark = [0.25, 0.2, 0.15];
    g.ellipsoid(cx as f32, 3.5, cz as f32 - 1.0, 2.6, 2.4, 4.0, solid(fur));
    g.ellipsoid(cx as f32, 2.4, cz as f32 - 1.0, 1.8, 1.2, 3.0, solid(belly));
    g.ellipsoid(cx as f32, 5.0, cz as f32 + 4.0, 1.9, 1.9, 2.2, solid(fur));
    // ears
    for sx in [-1i32, 1] {
        g.filled_box(cx + sx, 6, cz + 3, cx + sx, 10, cz + 3, solid(fur));
        g.set(cx + sx, 8, cz + 4, solid([0.9, 0.7, 0.7]));
    }
    // eyes, nose, tail, feet
    g.set(cx - 2, 5, cz + 6, solid(dark));
    g.set(cx + 2, 5, cz + 6, solid(dark));
    g.set(cx, 4, cz + 6, solid([0.9, 0.55, 0.6]));
    g.set(cx, 4, cz - 5, solid([0.95, 0.95, 0.9]));
    for (sx, sz) in [(-2, 2), (2, 2), (-2, -3), (2, -3)] {
        g.filled_box(cx + sx, 0, cz + sz, cx + sx, 1, cz + sz + 1, solid(fur));
    }
    g.mesh(HARE_VOXEL, true)
}

pub fn build_stag() -> VoxelMesh {
    let mut g = Grid::new(14, 26, 24);
    let (cx, cz) = (7, 12);
    let hide = [0.55, 0.4, 0.26];
    let belly = [0.78, 0.7, 0.58];
    let dark = [0.22, 0.16, 0.1];
    let bone = [0.8, 0.72, 0.55];
    // legs
    for (sx, sz) in [(-3, 6), (3, 6), (-3, -6), (3, -6)] {
        g.filled_box(cx + sx, 0, cz + sz, cx + sx, 11, cz + sz, solid(hide));
        g.set(cx + sx, 0, cz + sz, solid(dark));
    }
    // body + belly + neck + head
    g.ellipsoid(cx as f32, 14.0, cz as f32, 3.6, 3.2, 8.5, solid(hide));
    g.ellipsoid(cx as f32, 12.0, cz as f32, 2.6, 1.6, 6.5, solid(belly));
    g.ellipsoid(cx as f32, 17.5, cz as f32 + 7.5, 1.8, 3.5, 2.2, solid(hide));
    g.ellipsoid(cx as f32, 20.5, cz as f32 + 9.5, 2.0, 1.9, 3.2, solid(hide));
    g.set(cx, 20, cz + 12, solid(dark));
    g.set(cx - 1, 21, cz + 11, solid(dark));
    g.set(cx + 1, 21, cz + 11, solid(dark));
    // ears + antlers
    for sx in [-1i32, 1] {
        g.filled_box(cx + sx * 2, 22, cz + 8, cx + sx * 2, 23, cz + 8, solid(hide));
        let bx = cx + sx;
        for i in 0..4 {
            g.set(bx + sx * (i / 2), 22 + i, cz + 8 - i / 2, solid(bone));
        }
        g.set(bx + sx * 2, 24, cz + 8, solid(bone));
        g.set(bx + sx * 2, 25, cz + 9, solid(bone));
        g.set(bx, 25, cz + 6, solid(bone));
    }
    // tail
    g.set(cx, 16, cz - 9, solid([0.9, 0.88, 0.8]));
    g.mesh(STAG_VOXEL, true)
}

pub const FISH_VOXEL: f32 = 0.05;

/// The glowfish: a hand-long, lantern-bright river fish (drawn emissive).
pub fn build_glowfish() -> VoxelMesh {
    let mut g = Grid::new(8, 8, 16);
    let (cx, cz) = (4, 8);
    let body = [0.55, 0.95, 0.85];
    let fin = [0.35, 0.8, 0.9];
    g.ellipsoid(cx as f32, 3.5, cz as f32, 1.6, 2.0, 5.0, solid(body));
    g.ellipsoid(cx as f32, 3.5, cz as f32 + 3.0, 1.2, 1.4, 2.5, solid([0.75, 1.0, 0.9]));
    // tail fin + dorsal
    for i in 0..3 {
        g.set(cx, 2 + i, cz - 6 - i / 2, solid(fin));
        g.set(cx, 4 + i, cz - 6 - i / 2, solid(fin));
    }
    g.filled_box(cx, 5, cz - 2, cx, 6, cz + 1, solid(fin));
    g.set(cx - 1, 4, cz + 4, solid([0.1, 0.2, 0.2]));
    g.set(cx + 1, 4, cz + 4, solid([0.1, 0.2, 0.2]));
    g.mesh(FISH_VOXEL, true)
}

// ---------------------------------------------------------------------------
// Objectives — the Elder Boar (voxel 0.09, ~3.2u at the crest) and the Storm
// Drake (voxel 0.1, ~4.5u to the horn tips, wings spread). Both face +Z.
// ---------------------------------------------------------------------------

pub const BOAR_VOXEL: f32 = 0.09;
pub const DRAKE_VOXEL: f32 = 0.10;

pub fn build_boar() -> VoxelMesh {
    let mut g = Grid::new(20, 36, 40);
    let (cx, cz) = (10, 20);
    let (cxf, czf) = (cx as f32, cz as f32);
    let hide = [0.36, 0.26, 0.18];
    let dark = [0.2, 0.14, 0.1];
    let bristle = [0.55, 0.42, 0.28];
    let tusk = [0.95, 0.9, 0.78];
    // legs, thick
    for (sx, sz) in [(-5, 10), (5, 10), (-5, -10), (5, -10)] {
        g.filled_box(cx + sx - 2, 0, cz + sz - 2, cx + sx + 2, 14, cz + sz + 2, solid(hide));
        g.filled_box(cx + sx - 2, 0, cz + sz - 2, cx + sx + 2, 2, cz + sz + 2, solid(dark));
    }
    // barrel body, humped shoulders
    g.ellipsoid(cxf, 20.0, czf - 4.0, 8.5, 8.0, 14.0, solid(hide));
    g.ellipsoid(cxf, 24.0, czf + 6.0, 8.0, 7.0, 9.0, solid(hide));
    g.ellipsoid(cxf, 16.0, czf, 6.5, 3.0, 12.0, solid(mix(hide, bristle, 0.4)));
    // bristle crest along the spine
    for z in (cz - 12..=cz + 12).step_by(2) {
        let h = if (z - cz).abs() < 6 { 4 } else { 2 };
        g.filled_box(cx, 29, z, cx, 29 + h, z, solid(bristle));
        g.set(cx - 1, 30, z, solid(bristle));
    }
    // head + snout + ears + tusks + eyes
    g.ellipsoid(cxf, 22.0, czf + 15.0, 6.0, 6.0, 6.0, solid(hide));
    g.ellipsoid(cxf, 19.5, czf + 20.0, 3.5, 3.0, 4.0, solid(dark));
    for sx in [-1i32, 1] {
        g.filled_box(cx + sx * 4, 26, cz + 13, cx + sx * 5, 30, cz + 14, solid(hide));
        for i in 0..6 {
            g.set(cx + sx * (3 + i / 3), 19 + i, cz + 22 + (i % 2), solid(tusk));
        }
        g.set(cx + sx * 3, 24, cz + 20, solid([0.9, 0.2, 0.1]));
    }
    g.mesh(BOAR_VOXEL, true)
}

pub fn build_drake() -> VoxelMesh {
    let mut g = Grid::new(44, 46, 40);
    let (cx, cz) = (22, 20);
    let (cxf, czf) = (cx as f32, cz as f32);
    let scale = [0.26, 0.32, 0.42];
    let belly = [0.55, 0.6, 0.66];
    let wing = [0.35, 0.4, 0.6];
    let bone = [0.85, 0.85, 0.8];
    let glow = [0.6, 0.75, 1.0];
    // legs
    for (sx, sz) in [(-5, 6), (5, 6), (-5, -8), (5, -8)] {
        g.filled_box(cx + sx - 1, 0, cz + sz - 1, cx + sx + 1, 12, cz + sz + 1, solid(scale));
        g.filled_box(cx + sx - 2, 0, cz + sz - 1, cx + sx + 2, 1, cz + sz + 3, solid(bone));
    }
    // body, belly, tail
    g.ellipsoid(cxf, 18.0, czf, 6.5, 6.0, 15.0, solid(scale));
    g.ellipsoid(cxf, 15.0, czf, 4.5, 3.0, 12.0, solid(belly));
    for i in 0..16 {
        let t = i as f32;
        g.ellipsoid(cxf + (t * 0.4).sin() * 2.0, 17.0 + t * 0.5, czf - 14.0 - t * 1.2, 2.2 - t * 0.1, 2.2 - t * 0.1, 1.5, solid(scale));
    }
    // neck + head + horns + jaw
    for i in 0..8 {
        g.ellipsoid(cxf, 20.0 + i as f32 * 2.0, czf + 12.0 + i as f32 * 1.4, 3.2, 3.0, 3.5, solid(scale));
    }
    g.ellipsoid(cxf, 36.0, czf + 25.0, 4.0, 3.5, 6.5, solid(scale));
    g.ellipsoid(cxf, 34.0, czf + 30.0, 2.5, 1.6, 3.5, solid(belly));
    for sx in [-1i32, 1] {
        for i in 0..8 {
            g.set(cx + sx * (3 + i / 2), 38 + i, cz + 22 - i / 3, solid(bone));
        }
        g.set(cx + sx * 2, 37, cz + 30, solid(glow));
    }
    // wings: membrane fans on two bones each
    for sx in [-1i32, 1] {
        for i in 0..16 {
            let x = cx + sx * (4 + i);
            let y = 24 + (i as f32 * 0.9) as i32 - (i * i) as i32 / 30;
            g.filled_box(x, y, cz - 2, x, y + 1, cz - 1, solid(bone));
            let span = (14 - i).max(2);
            g.filled_box(x, y - 1, cz - 2 - span, x, y - 1, cz - 3, solid(wing));
        }
    }
    // spine glow ridge
    for z in (cz - 12..=cz + 10).step_by(3) {
        g.set(cx, 25, z, solid(glow));
    }
    g.mesh(DRAKE_VOXEL, true)
}

// ---------------------------------------------------------------------------
// Heal Tree — Clawbound's sanctuary tree: pale bark, a wide low crown, and
// glowing seed-fruit that read as a beacon (drawn with emissive).
// ---------------------------------------------------------------------------

pub const HEAL_TREE_VOXEL: f32 = 0.5;

pub fn build_heal_tree() -> (VoxelMesh, VoxelMesh) {
    let mut g = Grid::new(22, 26, 22);
    let c = 11.0;
    let bark = [0.72, 0.68, 0.58];
    let bark_d = [0.55, 0.5, 0.42];
    g.cyl_y(c, c, 2.2, 0, 10, solid(bark));
    g.cyl_y(c, c, 1.6, 10, 15, solid(bark_d));
    for k in 0..5 {
        let a = k as f32 * std::f32::consts::TAU / 5.0;
        for i in 0..4 {
            let r = 2.2 + i as f32 * 1.3;
            let x = (c + a.cos() * r) as i32;
            let z = (c + a.sin() * r) as i32;
            g.filled_box(x - 1, 0, z - 1, x + 1, (5 - i).max(1), z + 1, solid(bark_d));
        }
    }
    let leaf_a = [0.42, 0.62, 0.36];
    let leaf_b = [0.55, 0.75, 0.42];
    g.ellipsoid(c, 18.0, c, 9.5, 3.0, 9.5, solid(leaf_a));
    g.ellipsoid(c, 21.0, c, 6.5, 2.6, 6.5, solid(leaf_b));
    g.ellipsoid(c, 23.5, c, 3.2, 1.8, 3.2, solid(leaf_a));
    let crown = g.mesh(HEAL_TREE_VOXEL, true);
    // fruit: separate mesh so it can be drawn emissive
    let mut f = Grid::new(22, 26, 22);
    for k in 0..14 {
        let a = k as f32 * 2.399;
        let r = 4.0 + (k % 3) as f32 * 2.0;
        let x = c + a.cos() * r;
        let z = c + a.sin() * r;
        let y = 16.0 + (k % 4) as f32 * 1.5;
        f.ellipsoid(x, y, z, 0.9, 0.9, 0.9, solid([1.0, 0.92, 0.55]));
    }
    (crown, f.mesh(HEAL_TREE_VOXEL, true))
}

// ---------------------------------------------------------------------------
// Weapons — ancestors carry none; kept as a seam for future held items.
// ---------------------------------------------------------------------------

pub fn build_weapon(kind: WeaponKind, _def: &GodDef, _team: Team) -> Option<VoxelMesh> {
    match kind {
        WeaponKind::None => None,
    }
}

/// Right-side attachment point (model-space world units) — the jaw.
pub fn weapon_anchor(def: &GodDef) -> [f32; 3] {
    let bulk: f32 = match def.role {
        Role::Guardian => 2.0,
        Role::Warrior => 1.0,
        _ => 0.0,
    };
    [(6.0 + bulk) * GV, 1.45, 1.3]
}

/// Weapons are drawn 1 voxel further out and rotated during swings.
pub const WEAPON_PIVOT_VOX: f32 = 0.0;

// ---------------------------------------------------------------------------
// Minions — 14 voxel tall (voxel 0.09)
// ---------------------------------------------------------------------------

const MV: f32 = 0.09;

/// Minions are **mini cats**, not soldiers.
///
/// This was a humanoid — legs, torso, arms in skin tone, a metal helmet and a
/// wooden club. It is now a quadruped: there are no people anywhere in this
/// game, and a game called Clawbound fielding little humans with clubs was
/// the one place that slipped.
///
/// Built facing **+Z**, matching `build_hare` and `build_boar`, so the
/// existing yaw handling in `scene.rs` needs no change. Roughly 1.0u tall
/// against the old humanoid's 1.26u — a mini cat reads as small next to the
/// gods, which is what a minion should do. The hp bar in `ui.rs` sits at a
/// fixed `Y * 1.5`, lowered to suit.
pub fn build_minion(caster: bool, team: Team) -> VoxelMesh {
    let tc = team.rgb();
    // Team colour carried in the FUR rather than in cloth and armour: with no
    // clothing left to tint, the coat is the only thing that can read as
    // allegiance at gameplay distance.
    let fur = mix(tc, [0.42, 0.38, 0.34], 0.55);
    let belly = mix(fur, [0.92, 0.88, 0.80], 0.6);
    let dark = mix(fur, [0.10, 0.09, 0.12], 0.55);
    let accent = mix(tc, [1.0, 1.0, 1.0], 0.35);

    let mut g = Grid::new(12, 11, 20);
    let (cx, cz) = (6i32, 9i32);
    let (cxf, czf) = (cx as f32, cz as f32);

    // four legs — front pair slightly forward of the shoulder
    for (sx, sz) in [(-3i32, 5i32), (3, 5), (-3, -5), (3, -5)] {
        g.filled_box(cx + sx - 1, 0, cz + sz - 1, cx + sx, 3, cz + sz + 1, solid(fur));
        // dark paws, the way the hare and boar both foot their legs
        g.filled_box(cx + sx - 1, 0, cz + sz - 1, cx + sx, 0, cz + sz + 1, solid(dark));
    }

    // body: long barrel, pale underside
    g.ellipsoid(cxf, 5.5, czf, 3.0, 2.8, 6.0, solid(fur));
    g.ellipsoid(cxf, 4.2, czf, 2.2, 1.6, 5.0, solid(belly));

    // head
    g.ellipsoid(cxf, 7.0, czf + 7.0, 2.6, 2.5, 2.6, solid(fur));
    // muzzle + nose. Kept a voxel clear of the z=19 grid edge: `Grid::set`
    // drops out-of-range writes silently, so overshooting here would shave
    // the front off the nose with no error to notice.
    g.ellipsoid(cxf, 6.2, czf + 8.6, 1.3, 1.0, 1.2, solid(belly));
    g.set(cx, 6, cz + 9, solid([0.92, 0.55, 0.60]));
    // ears — the silhouette that makes it read as a cat at distance
    for sx in [-1i32, 1] {
        g.filled_box(cx + sx * 2, 9, cz + 6, cx + sx * 2, 10, cz + 7, solid(fur));
        g.set(cx + sx * 2, 9, cz + 7, solid(accent));
    }
    // eyes, on the head's front face rather than out on the muzzle
    g.set(cx - 1, 7, cz + 8, solid(dark));
    g.set(cx + 1, 7, cz + 8, solid(dark));

    // tail, curling up behind
    for i in 0..5i32 {
        let y = 6 + i.min(3);
        let z = (cz - 6 - i / 2).max(0);
        g.set(cx, y, z, solid(fur));
    }
    g.set(cx, 9, (cz - 8).max(0), solid(accent));

    if caster {
        // A caster carries a floating orb rather than a hood: a robe needs
        // shoulders, and this one is on four legs.
        g.ellipsoid(cxf, 9.0, czf + 3.0, 1.3, 1.3, 1.3, solid(accent));
        // collar tuft marking it apart from the melee line at a glance
        for sx in [-2i32, -1, 0, 1, 2] {
            g.set(cx + sx, 8, cz + 4, solid(accent));
        }
    } else {
        // Melee: shoulder spaulder and bared claws — armour on the animal,
        // not a weapon in a hand it does not have.
        g.filled_box(cx - 3, 7, cz + 2, cx + 3, 7, cz + 4, solid(dark));
        for sx in [-3i32, 3] {
            g.set(cx + sx, 1, cz + 6, solid([0.95, 0.93, 0.86]));
        }
    }

    g.mesh(MV, true)
}

// ---------------------------------------------------------------------------
// Towers — 5.6u tall (voxel 0.16)
// ---------------------------------------------------------------------------

const TV: f32 = 0.16;

pub fn build_tower(team: Team) -> (VoxelMesh, VoxelMesh) {
    let tc = team.rgb();
    let stone = [0.42, 0.40, 0.38];
    let stone_dark = [0.30, 0.29, 0.28];
    let trim = mix(tc, [0.9, 0.9, 0.9], 0.15);

    let mut g = Grid::new(26, 36, 26);
    let cx = 13;
    let cz = 13;

    // base plinth
    g.filled_box(cx - 11, 0, cz - 11, cx + 11, 3, cz + 11, solid(stone_dark));
    g.filled_box(cx - 9, 3, cz - 9, cx + 9, 5, cz + 9, solid(stone));
    // shaft, tapering
    g.cone_y(cx as f32, cz as f32, 7.0, 5.0, 5, 28, solid(stone));
    // brick banding
    for y in (7..28).step_by(3) {
        g.cyl_y(cx as f32, cz as f32, 7.2 - (y as f32 - 5.0) * 0.08, y, y, solid(stone_dark));
    }
    // capital + banner plates
    g.filled_box(cx - 8, 28, cz - 8, cx + 8, 30, cz + 8, solid(stone));
    g.filled_box(cx - 10, 30, cz - 2, cx - 8, 34, cz + 2, solid(trim));
    g.filled_box(cx + 8, 30, cz - 2, cx + 10, 34, cz + 2, solid(trim));
    let body = g.mesh(TV, true);

    // floating crystal (separate mesh for emissive pulsing)
    let mut cg = Grid::new(10, 14, 10);
    cg.cone_y(5.0, 5.0, 3.2, 0.8, 0, 6, solid(mix(tc, [1.0, 1.0, 1.0], 0.35)));
    cg.cone_y(5.0, 5.0, 0.8, 3.2, 6, 12, solid(mix(tc, [1.0, 1.0, 1.0], 0.35)));
    let crystal = cg.mesh(TV, false);

    (body, crystal)
}

// ---------------------------------------------------------------------------
// Titans — 6u tall (voxel 0.145)
// ---------------------------------------------------------------------------

const XV: f32 = 0.145;

/// The titan: a six-legged winged sabre-ancestor, not a man in armour.
///
/// This was a humanoid — legs, skirt, torso, pauldrons, arms, fists, a mace.
/// Fully armoured with no flesh showing, so it read as a golem rather than a
/// person, but it was still the one human silhouette left in a game whose
/// gods and minions are all cats. Now it is what a titan in this world should
/// be: the ancestor the ancestors descend from.
///
/// Six legs rather than four is the whole silhouette. Doubling a cat's size
/// reads as a big cat; adding a third pair reads as something older than cats
/// — the same trick the roster's megafauna camps use, where a leader is
/// recognisable but wrong-scaled. Wings are baked into the body rather than
/// animated: they are a canopy the thing stands under, and a titan that
/// flapped would leave the lane.
///
/// Faces **+Z**, like every other creature here. The second mesh is the right
/// FORELEG CLAW, which is why it needs no change in `scene.rs`: the existing
/// weapon transform offsets +1.85 on X and rotates about X, and for a
/// +Z-facing body that is exactly a forward swipe on the right side. A mace
/// in a fist and a claw on a foreleg happen to want the same matrix.
pub fn build_titan(team: Team) -> (VoxelMesh, VoxelMesh) {
    let tc = team.rgb();
    let armor = mix(tc, [0.55, 0.55, 0.6], 0.45);
    let armor_dark = mix(tc, [0.2, 0.2, 0.25], 0.6);
    let pelt = mix(tc, [0.30, 0.26, 0.24], 0.5);
    let gold = [0.85, 0.68, 0.3];
    let glow = mix(tc, [1.0, 1.0, 1.0], 0.4);
    let fang = [0.94, 0.92, 0.84];

    // Longer than it is tall: a hexapod is read end-on, so the grid is deep.
    let mut g = Grid::new(40, 42, 56);
    let (cx, cz) = (20i32, 28i32);
    let (cxf, czf) = (cx as f32, cz as f32);

    // ── six legs, three pairs ────────────────────────────────────────────
    // Front pair tallest and set widest: the shoulders carry the wings, and
    // a hexapod that tapers front-to-back reads as built for pouncing rather
    // than as a centipede.
    for (i, (zoff, h, spread)) in [(14i32, 17i32, 9i32), (0, 15, 8), (-14, 14, 8)].into_iter().enumerate() {
        for s in [-1i32, 1] {
            let lx = cx + s * spread;
            let lz = cz + zoff;
            g.filled_box(lx - 3, 0, lz - 3, lx + 3, h, lz + 3, solid(armor_dark));
            // greave
            g.filled_box(lx - 4, h - 5, lz - 4, lx + 4, h - 2, lz + 4, solid(armor));
            // paw + claws
            g.filled_box(lx - 4, 0, lz - 4, lx + 4, 2, lz + 4, solid(pelt));
            for c in -1..=1i32 {
                g.set(lx + c * 2, 0, lz + 4, solid(fang));
            }
            // A gold band on the front pair only, so the eye finds the head.
            if i == 0 {
                g.filled_box(lx - 4, h - 1, lz - 4, lx + 4, h, lz + 4, solid(gold));
            }
        }
    }

    // ── barrel body ──────────────────────────────────────────────────────
    g.ellipsoid(cxf, 22.0, czf, 9.0, 7.0, 21.0, solid(pelt));
    // armoured spine and flank plates over it
    g.ellipsoid(cxf, 24.0, czf - 2.0, 8.0, 6.5, 16.0, solid(armor));
    g.filled_box(cx - 7, 28, cz - 16, cx + 7, 30, cz + 10, solid(armor_dark));
    g.filled_box(cx - 3, 30, cz - 14, cx + 3, 31, cz + 8, solid(gold));

    // ── wings: swept, armoured, static ───────────────────────────────────
    // Ribbed rather than solid so they read as a frame with a membrane, and
    // so a 40-wide grid can hold a span that still looks like a wing.
    for s in [-1i32, 1] {
        for r in 0..6i32 {
            let t = r as f32 / 5.0;
            let x = cx + s * (9 + (t * 9.0) as i32);
            let y = 30 + (t * 9.0) as i32;
            let z0 = cz + 6 - (t * 16.0) as i32;
            let z1 = cz + 10 - (t * 4.0) as i32;
            g.filled_box(x, y, z0, x, y + 1, z1, solid(if r % 2 == 0 { armor } else { armor_dark }));
        }
        // leading edge, and a gold tip so the span is legible against sky
        g.filled_box(cx + s * 9, 30, cz + 8, cx + s * 18, 31, cz + 9, solid(armor));
        g.set(cx + s * 18, 39, cz + 6, solid(gold));
    }

    // ── head: sabre skull ────────────────────────────────────────────────
    let hz = czf + 19.0;
    g.ellipsoid(cxf, 26.0, hz, 6.0, 5.5, 6.0, solid(pelt));
    // helm over the crown, open at the muzzle
    g.ellipsoid(cxf, 28.0, hz - 1.0, 6.2, 4.4, 5.6, solid(armor));
    g.ellipsoid(cxf, 24.0, hz + 5.0, 3.4, 3.0, 3.2, solid(pelt));
    g.set(cx, 24, cz + 24, solid(armor_dark)); // nose
    // sabre fangs -- the one feature that has to survive at lane distance
    for s in [-1i32, 1] {
        for i in 0..7i32 {
            g.set(cx + s * 2, 22 - i, cz + 23 - i / 3, solid(fang));
        }
    }
    // eyes, and ears folded back under the helm
    g.set(cx - 3, 27, cz + 24, solid(glow));
    g.set(cx + 3, 27, cz + 24, solid(glow));
    for s in [-1i32, 1] {
        g.filled_box(cx + s * 4, 31, cz + 14, cx + s * 5, 34, cz + 16, solid(armor_dark));
        g.set(cx + s * 5, 34, cz + 16, solid(gold));
    }
    // crown horns, sweeping back over the shoulders
    for s in [-1i32, 1] {
        for i in 0..8i32 {
            g.set(cx + s * (5 + i / 3), 32 + i / 2, cz + 12 - i * 2, solid(gold));
        }
    }

    // ── tail: armoured, rising ───────────────────────────────────────────
    for i in 0..10i32 {
        let z = (cz - 21 - i).max(0);
        let y = 20 + i;
        if y >= 42 {
            break;
        }
        let r = 3 - i / 4;
        g.filled_box(cx - r, y, z, cx + r, y + 1, z, solid(if i % 3 == 0 { armor } else { pelt }));
    }
    let body = g.mesh(XV, true);

    // ── the right foreleg's siege claw (the "weapon" slot) ───────────────
    // Built around its own origin so the existing swing transform pivots it
    // at the shoulder rather than through its middle.
    let mut wg = Grid::new(20, 40, 24);
    let (wx, wz) = (10i32, 12i32);
    // armoured forearm down to the wrist
    wg.filled_box(wx - 3, 18, wz - 3, wx + 3, 34, wz + 3, solid(armor_dark));
    wg.filled_box(wx - 4, 30, wz - 4, wx + 4, 33, wz + 4, solid(armor));
    wg.filled_box(wx - 4, 20, wz - 4, wx + 4, 22, wz + 4, solid(gold));
    // the paw
    wg.ellipsoid(wx as f32, 16.0, wz as f32, 4.5, 4.0, 4.5, solid(pelt));
    // four blades, splayed and curving forward -- the reason this is a claw
    // and not a mace.
    for (i, s) in [-3i32, -1, 1, 3].into_iter().enumerate() {
        let lean = i as i32 % 2;
        for j in 0..11i32 {
            let x = wx + s;
            let y = (14 - j).max(0);
            let z = wz + 3 + j / 2 + lean;
            if z < 24 {
                wg.set(x, y, z, solid(fang));
            }
        }
    }
    // a dewclaw high on the inside, the way a real cat carries one
    for j in 0..5i32 {
        wg.set(wx - 5, 22 - j, wz + 1 + j / 2, solid(fang));
    }
    let weapon = wg.mesh(XV, false);

    (body, weapon)
}

// ---------------------------------------------------------------------------
// Projectiles
// ---------------------------------------------------------------------------

/// Voxel size of Sproutkit's glitter-ball.
pub const DISCO_VOX: f32 = 0.1;

/// Sproutkit's glitter-ball: a sphere of small mirror tiles, each a
/// slightly different silver (some catch the light: emissive), under a
/// little cap and hook. Centred on the ball.
pub fn disco_ball() -> VoxelMesh {
    let n = 28;
    let mut g = Grid::new(n, n + 4, n);
    let c = n as f32 * 0.5;
    g.ellipsoid(c, c, c, 12.0, 12.0, 12.0, [0.8, 0.82, 0.86, 1.0]);
    g.filled_box(c as i32 - 2, n as i32 - 3, c as i32 - 2, c as i32 + 1, n as i32 - 1, c as i32 + 1, [0.35, 0.33, 0.30, 1.0]);
    g.set(c as i32, n as i32, c as i32, [0.35, 0.33, 0.30, 1.0]);
    let mut m = g.mesh(DISCO_VOX, false);
    for (k, i) in m.inst.iter_mut().enumerate() {
        if i.color[0] > 0.7 {
            // mirror tiles: most dark (they reflect the night), a few
            // catching the light
            let h = ((k as u32).wrapping_mul(2654435761) >> 24) as f32 / 255.0;
            let v = 0.25 + 0.55 * h;
            i.color = [v, v, v * 1.08, 1.0];
            if h > 0.86 {
                i.color = [1.0, 0.97, 0.9, 1.0];
                i.misc[2] = 3.0;
            }
        }
    }
    m
}

pub fn build_orb(color: [f32; 3]) -> VoxelMesh {
    let mut g = Grid::new(7, 7, 7);
    g.ellipsoid(3.0, 3.0, 3.0, 2.6, 2.6, 2.6, [color[0], color[1], color[2], 0.35]);
    g.ellipsoid(3.0, 3.0, 3.0, 1.4, 1.4, 1.4, [1.0, 1.0, 1.0, 0.9]);
    let mut m = g.mesh(0.09, false);
    // boost core brightness
    for i in m.inst.iter_mut() {
        i.color[3] = i.color[3];
    }
    m
}

pub fn build_arrow(team: Team) -> VoxelMesh {
    let tc = team.rgb();
    let mut g = Grid::new(3, 3, 9);
    g.filled_box(1, 1, 0, 1, 1, 6, solid([0.5, 0.38, 0.22]));
    g.filled_box(1, 1, 7, 1, 1, 8, solid(tc));
    g.filled_box(1, 1, 0, 1, 1, 0, solid([0.9, 0.9, 0.9]));
    g.mesh(0.07, false)
}

pub fn build_boulder() -> VoxelMesh {
    let mut g = Grid::new(8, 8, 8);
    g.ellipsoid(4.0, 4.0, 4.0, 3.4, 3.0, 3.2, solid([0.45, 0.4, 0.34]));
    g.ellipsoid(3.0, 5.0, 3.0, 1.6, 1.4, 1.6, solid([0.55, 0.5, 0.42]));
    g.mesh(0.11, false)
}

pub fn god_voxel() -> f32 {
    VOXEL_SIZE
}
