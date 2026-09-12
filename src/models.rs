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
const H: i32 = 42; // grid height in voxels

pub fn build_god(def: &GodDef, team: Team) -> VoxelMesh {
    let pal = god_palette(def, team);
    let pelt = pal[PAL_BODY];
    let collar = pal[PAL_CLOTH];
    let second = pal[PAL_METAL];
    let accent = pal[PAL_ACCENT];
    let dark = mix(pelt, [0.05, 0.04, 0.05], 0.7);

    let bulk: i32 = match def.role {
        Role::Guardian => 2,
        Role::Warrior => 1,
        Role::Mage => 0,
        Role::Hunter => 0,
        Role::Assassin => -1,
    };
    let bf = bulk as f32;
    let crag = matches!(def.pelt, PeltKind::Crag);
    let extra = if crag { 1.0 } else { 0.0 };

    let mut g = Grid::new(30, H as usize, 48);
    let cx = 15;
    let cz = 24;
    let (cxf, czf) = (cx as f32, cz as f32);

    // ── legs + paws (y 0..18) ────────────────────────────────────────────
    let lt = 1 + (bulk > 0) as i32; // half-thickness
    let lx = 5 + bulk.max(0);
    let leg_col = solid(mix(pelt, dark, 0.15));
    for (sx, sz) in [(-1, 11), (1, 11), (-1, -10), (1, -10)] {
        let x = cx + sx * lx;
        let z = cz + sz;
        g.filled_box(x - lt, 0, z - lt - 1, x + lt, 19, z + lt + 1, leg_col);
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
    for sx in [-1i32, 1] {
        let x = cx + sx * lx;
        g.filled_box(x - lt - 1, 9, cz + 11 - lt - 1, x + lt + 1, 10, cz + 11 + lt + 1, solid(team.rgb()));
    }

    g.mesh(GV, true)
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

pub fn build_minion(caster: bool, team: Team) -> VoxelMesh {
    let tc = team.rgb();
    let cloth = mix(tc, [0.35, 0.35, 0.4], 0.45);
    let metal = mix(tc, [0.75, 0.75, 0.8], 0.55);
    let skin = [0.8, 0.62, 0.45];

    let mut g = Grid::new(12, 14, 12);
    let cx = 6;
    let cz = 6;

    // legs
    g.filled_box(cx - 2, 0, cz - 1, cx - 1, 4, cz + 1, solid(mix(cloth, [0.1, 0.1, 0.1], 0.4)));
    g.filled_box(cx + 1, 0, cz - 1, cx + 2, 4, cz + 1, solid(mix(cloth, [0.1, 0.1, 0.1], 0.4)));
    // torso
    g.filled_box(cx - 3, 5, cz - 2, cx + 3, 9, cz + 2, solid(cloth));
    // arms
    g.filled_box(cx - 4, 5, cz - 1, cx - 4, 8, cz + 1, solid(skin));
    g.filled_box(cx + 4, 5, cz - 1, cx + 4, 8, cz + 1, solid(skin));
    // head
    g.ellipsoid(cx as f32, 11.0, cz as f32, 2.4, 2.4, 2.4, solid(skin));
    // helmet
    g.ellipsoid(cx as f32, 12.0, cz as f32, 2.6, 1.8, 2.6, solid(metal));
    g.set(cx - 1, 11, cz + 2, solid(tc));
    g.set(cx + 1, 11, cz + 2, solid(tc));

    if caster {
        // hood robe flare + floating orb hands
        g.cone_y(cx as f32, cz as f32, 4.0, 3.0, 2, 6, solid(cloth));
        g.ellipsoid(cx as f32 + 5.0, 8.5, cz as f32, 1.2, 1.2, 1.2, solid(mix(tc, [1.0, 1.0, 1.0], 0.3)));
    } else {
        // little club
        g.filled_box(cx + 4, 6, cz - 1, cx + 5, 12, cz, solid([0.4, 0.28, 0.16]));
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

pub fn build_titan(team: Team) -> (VoxelMesh, VoxelMesh) {
    let tc = team.rgb();
    let armor = mix(tc, [0.55, 0.55, 0.6], 0.45);
    let armor_dark = mix(tc, [0.2, 0.2, 0.25], 0.6);
    let gold = [0.85, 0.68, 0.3];
    let glow = mix(tc, [1.0, 1.0, 1.0], 0.4);

    let mut g = Grid::new(40, 42, 30);
    let cx = 20;
    let cz = 15;

    // legs
    g.filled_box(cx - 8, 0, cz - 3, cx - 3, 14, cz + 3, solid(armor_dark));
    g.filled_box(cx + 3, 0, cz - 3, cx + 8, 14, cz + 3, solid(armor_dark));
    g.filled_box(cx - 9, 0, cz - 4, cx - 2, 3, cz + 4, solid(armor));
    g.filled_box(cx + 2, 0, cz - 4, cx + 9, 3, cz + 4, solid(armor));
    // skirt
    g.cone_y(cx as f32, cz as f32, 9.0, 10.0, 12, 18, solid(armor_dark));
    // torso
    g.filled_box(cx - 9, 18, cz - 5, cx + 9, 30, cz + 5, solid(armor));
    g.filled_box(cx - 6, 22, cz - 2, cx + 6, 29, cz + 2, solid(mix(armor, gold, 0.35)));
    // pauldrons
    g.ellipsoid((cx - 10) as f32, 30.0, cz as f32, 4.0, 3.0, 4.5, solid(armor));
    g.ellipsoid((cx + 10) as f32, 30.0, cz as f32, 4.0, 3.0, 4.5, solid(armor));
    // arms
    g.filled_box(cx - 13, 16, cz - 3, cx - 10, 27, cz + 3, solid(armor_dark));
    g.filled_box(cx + 10, 16, cz - 3, cx + 13, 27, cz + 3, solid(armor_dark));
    g.filled_box(cx - 14, 12, cz - 4, cx - 10, 16, cz + 4, solid(armor)); // fists
    g.filled_box(cx + 10, 12, cz - 4, cx + 14, 16, cz + 4, solid(armor));
    // head + horns
    g.ellipsoid(cx as f32, 33.0, cz as f32, 4.0, 3.6, 4.0, solid(armor_dark));
    g.set(cx - 2, 34, cz + 3, solid(glow));
    g.set(cx + 2, 34, cz + 3, solid(glow));
    for s in [-1i32, 1] {
        for i in 0..6 {
            g.set(cx + s * (4 + i / 2), 35 + i / 2, cz + 1 - i / 3, solid(gold));
        }
    }
    // crown spikes
    for i in -2..=2i32 {
        g.filled_box(cx + i * 3, 36, cz - 1, cx + i * 3, 38 - i.abs(), cz + 1, solid(gold));
    }
    let body = g.mesh(XV, true);

    // giant mace (weapon)
    let mut wg = Grid::new(20, 44, 20);
    let wx = 10;
    let wz = 10;
    wg.filled_box(wx, 0, wz, wx, 30, wz, solid([0.3, 0.22, 0.15]));
    wg.ellipsoid(wx as f32, 36.0, wz as f32, 6.0, 6.0, 6.0, solid(armor_dark));
    for a in 0..10 {
        let ang = a as f32 / 10.0 * std::f32::consts::TAU;
        let x = wx as f32 + ang.cos() * 6.0;
        let z = wz as f32 + ang.sin() * 6.0;
        wg.ellipsoid(x, 36.0, z, 1.6, 1.6, 1.6, solid(mix(tc, [1.0, 1.0, 1.0], 0.3)));
    }
    let weapon = wg.mesh(XV, false);

    (body, weapon)
}

// ---------------------------------------------------------------------------
// Projectiles
// ---------------------------------------------------------------------------

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
