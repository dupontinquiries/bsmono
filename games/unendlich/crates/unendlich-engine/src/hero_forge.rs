//! r538: HERO FORGE — engine-native voxel "hero" characters.
//!
//! The character-forge HTML ships five bespoke humanoid voxel heroes
//! (Heliographer, Windwalker, Tidecaller, Stormweaver, Gravitybender), each a
//! hand-placed rig with element-themed crests, plating and palettes. This module
//! is the engine port: a single *parameterized* humanoid builder that produces a
//! distinct hero per [`HeroKind`], shaped live by the player's [`BuildParams`]
//! sliders (height, bulk/muscularity, head, shoulders, arms, legs).
//!
//! It is an engine-native interpretation rather than a byte-for-byte copy of the
//! JS geometry — the proportions, palettes and element features are matched, but
//! the exact per-voxel placement is rebuilt to fit our entity vertex pipeline.
//! Output is a plain `(Vec<EntityVertex>, Vec<u32>)` cube soup that flows through
//! the same bake path (coverage fade, alpha, head clip) as any other avatar.

use crate::player_character::BuildParams;
use crate::world::entity::EntityVertex;

/// The five forge heroes. Kept separate from the 4-way bending element because
/// Stormweaver and Windwalker share the Air element but must look different.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeroKind {
    Heliographer, // sun / Fire
    Windwalker,   // wind / Air
    Tidecaller,   // tide / Water (female build)
    Stormweaver,  // storm / Air
    Gravitybender, // gravity / Earth
    Brinekeeper,  // r683: brine / mineral (stocky, Carrier aesthetic)
    Luminant,     // r683: information / light (lean scholar)
}

impl HeroKind {
    /// Case-insensitive name → hero, matching the forge tabs. `None` for ordinary
    /// (non-hero) characters, which keep the generic blueprint avatar.
    pub fn from_name(name: &str) -> Option<HeroKind> {
        let n = name.to_ascii_lowercase();
        if n.contains("helio") {
            Some(HeroKind::Heliographer)
        } else if n.contains("tide") {
            Some(HeroKind::Tidecaller)
        } else if n.contains("wind") {
            Some(HeroKind::Windwalker)
        } else if n.contains("storm") {
            Some(HeroKind::Stormweaver)
        } else if n.contains("gravity") {
            Some(HeroKind::Gravitybender)
        } else if n.contains("brine") || n.contains("carrier") {
            Some(HeroKind::Brinekeeper)
        } else if n.contains("lumin") || n.contains("scholar") || n.contains("info") {
            Some(HeroKind::Luminant)
        } else {
            None
        }
    }

    /// Real per-part colours, ported from the character-forge builders' palettes
    /// + their `buildBody(...)` colour mapping (legs/foot/torso/collar/skin/eye/
    /// sleeve/fore/brow/beard) plus element accents.
    fn colors(self) -> HeroColors {
        match self {
            HeroKind::Heliographer => HeroColors {
                legs: rgb(0xcca658), foot: rgb(0xbfa066), torso: rgb(0xd0ad5e), collar: rgb(0xb28a44),
                skin: rgb(0xcda86a), eye: rgb(0xffe89a), sleeve: rgb(0xcca658), fore: rgb(0xb28a44),
                brow: rgb(0x5a4424), hair: rgb(0x8a6a34), accent: rgb(0xf2cf68), glow: rgb(0xffe682),
                beard: Some(rgb(0x8a6a34)), gender: 0.0, nolegs: false,
            },
            HeroKind::Windwalker => HeroColors {
                legs: rgb(0x4f5563), foot: rgb(0x6b5436), torso: rgb(0xddddd3), collar: rgb(0x9fb0c4),
                skin: rgb(0xc9a583), eye: rgb(0xeaf6ff), sleeve: rgb(0xcccdc2), fore: rgb(0x7a5e38),
                brow: rgb(0x1a140d), hair: rgb(0x483524), accent: rgb(0x4a7fb5), glow: rgb(0x9fccff),
                beard: None, gender: 0.0, nolegs: false,
            },
            HeroKind::Tidecaller => HeroColors {
                legs: rgb(0x294a5e), foot: rgb(0x1c2738), torso: rgb(0x2f6e6e), collar: rgb(0x3f8a86),
                skin: rgb(0x7a4e34), eye: rgb(0xc8f2ff), sleeve: rgb(0x2f6e6e), fore: rgb(0xb08a4a),
                brow: rgb(0x0d0b08), hair: rgb(0x140f0b), accent: rgb(0x6fd6ff), glow: rgb(0xc8f2ff),
                beard: None, gender: 1.0, nolegs: false,
            },
            HeroKind::Stormweaver => HeroColors {
                legs: rgb(0x2c2f36), foot: rgb(0x1b1e24), torso: rgb(0x2c2f36), collar: rgb(0x8e97a4),
                skin: rgb(0xcbbaa8), eye: rgb(0xeaf6ff), sleeve: rgb(0x3a3e46), fore: rgb(0x2c2f36),
                brow: rgb(0x141518), hair: rgb(0x202329), accent: rgb(0xd9a838), glow: rgb(0x7cc6ff),
                beard: None, gender: 0.0, nolegs: false,
            },
            HeroKind::Gravitybender => HeroColors {
                legs: rgb(0x554c42), foot: rgb(0x3a342c), torso: rgb(0x6b6256), collar: rgb(0x554c42),
                skin: rgb(0x6b6256), eye: rgb(0xc89cff), sleeve: rgb(0x6b6256), fore: rgb(0x3a342c),
                brow: rgb(0x2a241c), hair: rgb(0x2a2433), accent: rgb(0xb89243), glow: rgb(0x9a5cff),
                beard: None, gender: 0.0, nolegs: true,
            },
            // r683: Brine — mineral teal-grey, salt-crusted, stocky.
            HeroKind::Brinekeeper => HeroColors {
                legs: rgb(0x3a4a4a), foot: rgb(0x2a3636), torso: rgb(0x4a6a68), collar: rgb(0x5a8a86),
                skin: rgb(0x8a6a4a), eye: rgb(0xaaf0e0), sleeve: rgb(0x4a6a68), fore: rgb(0x6a8a4a),
                brow: rgb(0x1a1610), hair: rgb(0x2a2420), accent: rgb(0x5ad0c0), glow: rgb(0x8af0e0),
                beard: Some(rgb(0x2a2420)), gender: 0.0, nolegs: false,
            },
            // r683: Information — pale luminous scholar, light accents.
            HeroKind::Luminant => HeroColors {
                legs: rgb(0x4a4a5a), foot: rgb(0x35354a), torso: rgb(0xd8d8e8), collar: rgb(0xb0b0d0),
                skin: rgb(0xd0b090), eye: rgb(0xffffff), sleeve: rgb(0xc8c8e0), fore: rgb(0x9090b0),
                brow: rgb(0x2a2a3a), hair: rgb(0x6a6a5a), accent: rgb(0xa0e0ff), glow: rgb(0xe0f0ff),
                beard: None, gender: 0.0, nolegs: false,
            },
        }
    }

    /// Real proportions (voxel units), ported from each builder's `dim`.
    fn dims(self) -> HeroDims {
        match self {
            HeroKind::Heliographer => HeroDims { sh: 0.78, arm_w: 6.0, leg_u: 7.0, leg_l: 6.0, leg_ul: 15.0, leg_ll: 12.5, t_w: 16.0, t_d: 8.5, t_h: 17.0, head: 7.0 },
            HeroKind::Windwalker   => HeroDims { sh: 0.78, arm_w: 6.2, leg_u: 8.5, leg_l: 7.5, leg_ul: 15.5, leg_ll: 13.5, t_w: 14.0, t_d: 7.5, t_h: 17.0, head: 6.6 },
            HeroKind::Tidecaller   => HeroDims { sh: 0.88, arm_w: 8.5, leg_u: 11.5, leg_l: 8.8, leg_ul: 15.0, leg_ll: 13.0, t_w: 19.0, t_d: 11.0, t_h: 17.0, head: 8.2 },
            HeroKind::Stormweaver  => HeroDims { sh: 0.76, arm_w: 7.0, leg_u: 7.0, leg_l: 6.0, leg_ul: 15.75, leg_ll: 12.0, t_w: 17.0, t_d: 9.5, t_h: 17.0, head: 7.0 },
            HeroKind::Gravitybender=> HeroDims { sh: 0.94, arm_w: 10.0, leg_u: 9.0, leg_l: 8.0, leg_ul: 15.0, leg_ll: 12.5, t_w: 23.0, t_d: 13.0, t_h: 18.0, head: 8.0 },
            HeroKind::Brinekeeper  => HeroDims { sh: 0.86, arm_w: 9.0, leg_u: 9.0, leg_l: 7.5, leg_ul: 15.0, leg_ll: 12.5, t_w: 20.0, t_d: 11.5, t_h: 17.5, head: 7.6 },
            HeroKind::Luminant     => HeroDims { sh: 0.80, arm_w: 6.5, leg_u: 8.0, leg_l: 7.0, leg_ul: 15.5, leg_ll: 13.0, t_w: 15.0, t_d: 8.0, t_h: 17.0, head: 7.4 },
        }
    }
}

#[derive(Clone, Copy)]
struct HeroColors {
    legs: [f32; 3], foot: [f32; 3], torso: [f32; 3], collar: [f32; 3], skin: [f32; 3],
    eye: [f32; 3], sleeve: [f32; 3], fore: [f32; 3], brow: [f32; 3], hair: [f32; 3],
    accent: [f32; 3], glow: [f32; 3], beard: Option<[f32; 3]>, gender: f32, nolegs: bool,
}

#[derive(Clone, Copy)]
struct HeroDims {
    sh: f32, arm_w: f32, leg_u: f32, leg_l: f32, leg_ul: f32, leg_ll: f32,
    t_w: f32, t_d: f32, t_h: f32, head: f32,
}

/// World units per voxel-step — matches the character-forge `u`.
const U: f32 = 0.072;

#[inline]
fn rgb(hex: u32) -> [f32; 3] {
    [((hex >> 16) & 0xff) as f32 / 255.0, ((hex >> 8) & 0xff) as f32 / 255.0, (hex & 0xff) as f32 / 255.0]
}

// ───────────────────────── tiny transform rig ─────────────────────────
// A pivot's world transform maps a local point p → r·p + t. Children compose
// with their parent: world = parent ∘ local. This gives the forge's compounding
// joint hierarchy (shoulder→elbow→hand, hip→knee→foot) that a single-pivot box
// emitter can't express.
#[derive(Clone, Copy)]
struct Xf { r: [[f32; 3]; 3], t: [f32; 3] }

impl Xf {
    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        [
            self.r[0][0] * p[0] + self.r[0][1] * p[1] + self.r[0][2] * p[2] + self.t[0],
            self.r[1][0] * p[0] + self.r[1][1] * p[1] + self.r[1][2] * p[2] + self.t[1],
            self.r[2][0] * p[0] + self.r[2][1] * p[1] + self.r[2][2] * p[2] + self.t[2],
        ]
    }
    // self ∘ child
    fn compose(&self, c: &Xf) -> Xf {
        let mut r = [[0.0f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                r[i][j] = self.r[i][0] * c.r[0][j] + self.r[i][1] * c.r[1][j] + self.r[i][2] * c.r[2][j];
            }
        }
        let t = self.apply(c.t);
        Xf { r, t }
    }
}

fn mat_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut r = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    r
}

// Euler XYZ rotation (Rx·Ry·Rz). Poses here are mostly single-axis, so order is
// not sensitive.
fn euler(e: [f32; 3]) -> [[f32; 3]; 3] {
    let (sx, cx) = e[0].sin_cos();
    let (sy, cy) = e[1].sin_cos();
    let (sz, cz) = e[2].sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    mat_mul(rx, mat_mul(ry, rz))
}

// Pivot indices.
const ROOT: usize = 0;
const HIPS: usize = 1;
const CHEST: usize = 2;
const NECK: usize = 3;
const HEAD: usize = 4;
const SHL: usize = 5;
const SHR: usize = 6;
const ELL: usize = 7;
const ELR: usize = 8;
const HAL: usize = 9;
const HAR: usize = 10;
const HIPL: usize = 11;
const HIPR: usize = 12;
const KNL: usize = 13;
const KNR: usize = 14;
const NPIV: usize = 15;

// (pivot, parent, local-translation) — parents always precede children.
fn pivot_table(dim: &HeroDims, base_y: f32) -> [(usize, i32, [f32; 3]); NPIV] {
    [
        (ROOT, -1, [0.0, base_y, 0.0]),
        (HIPS, ROOT as i32, [0.0, 2.02, 0.0]),
        (CHEST, HIPS as i32, [0.0, 0.0, 0.0]),
        (NECK, CHEST as i32, [0.0, 1.36, 0.0]),
        (HEAD, NECK as i32, [0.0, 0.18, 0.0]),
        (SHL, CHEST as i32, [-dim.sh, 1.12, 0.0]),
        (SHR, CHEST as i32, [dim.sh, 1.12, 0.0]),
        (ELL, SHL as i32, [0.0, -0.72, 0.0]),
        (ELR, SHR as i32, [0.0, -0.72, 0.0]),
        (HAL, ELL as i32, [0.0, -0.66, 0.0]),
        (HAR, ELR as i32, [0.0, -0.66, 0.0]),
        (HIPL, HIPS as i32, [-0.30, 0.0, 0.0]),
        (HIPR, HIPS as i32, [0.30, 0.0, 0.0]),
        (KNL, HIPL as i32, [0.0, -dim.leg_ul * U, 0.0]),
        (KNR, HIPR as i32, [0.0, -dim.leg_ul * U, 0.0]),
    ]
}

// Compose world transforms from per-pivot local rotations (euler) + the table.
fn rig_world(dim: &HeroDims, base_y: f32, rot: &[[f32; 3]; NPIV]) -> [Xf; NPIV] {
    let table = pivot_table(dim, base_y);
    let mut world = [Xf { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0, 0.0, 0.0] }; NPIV];
    for &(idx, parent, t) in table.iter() {
        let local = Xf { r: euler(rot[idx]), t };
        world[idx] = if parent < 0 {
            local
        } else {
            world[parent as usize].compose(&local)
        };
    }
    world
}

// ───────────────────────── box emitter ─────────────────────────
// Accumulates engine-local geometry (body yaw applied; origin added at finalize so
// the feet can be grounded to y=0 for any dims/pose). Three→engine axis remap:
// the forge character faces +Z, so engine forward(X)=three Z, up(Y)=three Y,
// width(Z)=−three X (a proper 90° turn so the hero faces the look direction).
struct Body { verts: Vec<EntityVertex>, indices: Vec<u32>, cy: f32, sy: f32 }

impl Body {
    fn new(yaw: f32) -> Self {
        Body { verts: Vec::new(), indices: Vec::new(), cy: yaw.cos(), sy: yaw.sin() }
    }
    /// Emit one box: `size` in voxel units, `center` in the pivot's local frame
    /// (world units), transformed by the pivot's world `xf`.
    fn part(&mut self, xf: &Xf, size: [f32; 3], color: [f32; 3], center: [f32; 3]) {
        let h = [size[0] * U * 0.5, size[1] * U * 0.5, size[2] * U * 0.5];
        let base = self.verts.len() as u32;
        for &dx in &[-1.0f32, 1.0] {
            for &dy in &[-1.0f32, 1.0] {
                for &dz in &[-1.0f32, 1.0] {
                    let lp = [center[0] + dx * h[0], center[1] + dy * h[1], center[2] + dz * h[2]];
                    let w = xf.apply(lp); // three-space world
                    // remap three → engine: ex = tz, ey = ty, ez = -tx
                    let (ex, ey, ez) = (w[2], w[1], -w[0]);
                    // body yaw about engine Y
                    let rx = ex * self.cy - ez * self.sy;
                    let rz = ex * self.sy + ez * self.cy;
                    self.verts.push(EntityVertex { position: [rx, ey, rz], color, alpha: 1.0 });
                }
            }
        }
        const FACES: [[u32; 6]; 6] = [
            [0, 1, 3, 0, 3, 2], [4, 6, 7, 4, 7, 5], [0, 4, 5, 0, 5, 1],
            [2, 3, 7, 2, 7, 6], [0, 2, 6, 0, 6, 4], [1, 5, 7, 1, 7, 3],
        ];
        for f in &FACES {
            for &i in f {
                self.indices.push(base + i);
            }
        }
    }
    /// Ground the feet to y=0, uniformly scale the model to `target_height`
    /// blocks (about the rig's local origin, which is already centred), add the
    /// `bob`, and translate to the origin.
    fn finalize(mut self, origin: [f32; 3], bob: f32, target_height: f32) -> (Vec<EntityVertex>, Vec<u32>) {
        let (mut min_y, mut max_y) = (f32::MAX, f32::MIN);
        for v in &self.verts {
            min_y = min_y.min(v.position[1]);
            max_y = max_y.max(v.position[1]);
        }
        if !min_y.is_finite() {
            min_y = 0.0;
            max_y = 0.0;
        }
        let span = (max_y - min_y).max(1e-3);
        let scale = (target_height / span).max(1e-3);
        for v in self.verts.iter_mut() {
            v.position[0] = origin[0] + v.position[0] * scale;
            v.position[1] = origin[1] + bob + (v.position[1] - min_y) * scale;
            v.position[2] = origin[2] + v.position[2] * scale;
        }
        (self.verts, self.indices)
    }
}

// Port of the forge `buildBody`: fills the rig pivots with per-part voxel boxes.
fn build_body(b: &mut Body, w: &[Xf; NPIV], dim: &HeroDims, col: &HeroColors) {
    let ff = dim.head * U / 2.0; // head front face (+z in three space)
    let g = col.gender;
    let mouth = rgb(0x3a2a24);

    // legs (a levitating Gravitybender has none)
    if !col.nolegs {
        for &(hip, kn) in &[(HIPL, KNL), (HIPR, KNR)] {
            b.part(&w[hip], [dim.leg_u, dim.leg_ul, dim.leg_u], col.legs, [0.0, -dim.leg_ul * U / 2.0, 0.0]);
            b.part(&w[kn], [dim.leg_l, dim.leg_ll, dim.leg_l], col.legs, [0.0, -dim.leg_ll * U / 2.0, 0.0]);
            b.part(&w[kn], [5.0, 2.0, 8.0], col.foot, [0.0, -dim.leg_ll * U - 0.05, 0.12]);
        }
    }

    // torso
    b.part(&w[CHEST], [dim.t_w * (1.0 - 0.06 * g), dim.t_h, dim.t_d], col.torso, [0.0, 0.62, 0.0]);
    if g > 0.45 {
        b.part(&w[CHEST], [(dim.t_w * 0.54).max(6.0), 5.0, 3.6], col.torso, [0.0, 0.55, ff - 0.16]);
        b.part(&w[HIPS], [dim.t_w * 1.08, 5.0, dim.t_d * 1.08], col.legs, [0.0, -0.08, 0.0]);
    } else {
        b.part(&w[CHEST], [dim.t_w * 0.66, 5.2, 1.6], col.torso, [0.0, 0.95, ff - 0.12]);
        b.part(&w[SHL], [dim.arm_w * 0.7, 2.4, dim.arm_w * 0.7], col.sleeve, [0.0, 0.12, 0.0]);
        b.part(&w[SHR], [dim.arm_w * 0.7, 2.4, dim.arm_w * 0.7], col.sleeve, [0.0, 0.12, 0.0]);
    }
    b.part(&w[NECK], [9.0 - 1.6 * g, 3.2, 9.0 - 1.6 * g], col.collar, [0.0, -0.18, 0.0]);
    b.part(&w[NECK], [3.4, 3.0, 3.4], col.skin, [0.0, 0.02, 0.0]);

    // head
    b.part(&w[HEAD], [dim.head, dim.head, dim.head], col.skin, [0.0, 0.18, 0.0]);
    b.part(&w[HEAD], [dim.head * (0.8 - 0.12 * g), 2.4, dim.head * 0.52], col.skin, [0.0, -0.30, ff - 0.16]);
    b.part(&w[HEAD], [1.3, 0.7 + 0.2 * g, 0.5], col.eye, [-0.12, 0.20, ff + 0.01]);
    b.part(&w[HEAD], [1.3, 0.7 + 0.2 * g, 0.5], col.eye, [0.12, 0.20, ff + 0.01]);
    b.part(&w[HEAD], [5.2 - 1.0 * g, 0.7 - 0.2 * g, 0.6], col.brow, [0.0, 0.32, ff - 0.02]);
    b.part(&w[HEAD], [0.55, 0.95, 0.6], col.skin, [0.0, 0.05, ff + 0.02]); // nose
    b.part(&w[HEAD], [1.0, 0.32, 0.4], mouth, [0.0, -0.16, ff + 0.015]); // mouth
    b.part(&w[HEAD], [1.0, 1.3 - 0.3 * g, 0.9], col.skin, [0.0, 0.10, ff + 0.03]); // chin
    if let Some(beard) = col.beard {
        if g < 0.4 {
            b.part(&w[HEAD], [5.6, 3.2, 2.6], beard, [0.0, -0.16, ff - 0.10]);
        }
    }
    // hair cap
    b.part(&w[HEAD], [dim.head * 1.02, dim.head * 0.5, dim.head * 1.02], col.hair, [0.0, 0.18 + dim.head * U * 0.42, 0.0]);

    // arms
    for &(sh, el, ha) in &[(SHL, ELL, HAL), (SHR, ELR, HAR)] {
        let aw = dim.arm_w * (1.0 + 0.06 * (1.0 - g));
        b.part(&w[sh], [aw, 9.0, aw], col.sleeve, [0.0, -0.34, 0.0]);
        b.part(&w[el], [dim.arm_w, 9.0, dim.arm_w], col.fore, [0.0, -0.32, 0.0]);
        b.part(&w[ha], [3.2, 3.2, 3.2], col.skin, [0.0, -0.14, 0.0]);
    }
}

/// r551: per-hero signature props (ported from the character-forge builders).
/// Attached either to a hand pivot (so they ride the arm) or to world/root space
/// (so they stand upright and turn with the body yaw). `anim_t` drives slow spins.
fn dress(kind: HeroKind, b: &mut Body, w: &[Xf; NPIV], dim: &HeroDims, col: &HeroColors, pose: &HeroPose) {
    let id = Xf { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0, 0.0, 0.0] };
    let t = pose.anim_t;
    let ff = dim.head * U / 2.0;
    match kind {
        HeroKind::Heliographer => {
            // leather cap + dark sunglasses visor + gold brow rim (head)
            b.part(&w[HEAD], [dim.head * 1.06, 3.0, dim.head * 1.06], rgb(0xbfa066), [0.0, 0.42, 0.0]);
            b.part(&w[HEAD], [5.4, 1.5, 0.6], rgb(0x16161c), [0.0, 0.20, ff + 0.02]);
            b.part(&w[HEAD], [5.6, 0.4, 0.5], col.accent, [0.0, 0.50, ff + 0.03]);
            // flowing cape down the back (behind = -z)
            b.part(&w[CHEST], [dim.t_w * 1.12, 26.0, 1.8], rgb(0xb28a44), [0.0, -0.5, -ff - 0.10]);
            b.part(&w[CHEST], [dim.t_w * 0.6, 18.0, 2.0], rgb(0x9c7a3c), [0.0, -1.5, -ff - 0.16]);
            // sun-orb staff - vertical at the left side (world/root space, turns with yaw)
            let sx = -1.05;
            let sz = 0.30;
            b.part(&id, [1.7, 46.0, 1.7], rgb(0xb28a44), [sx, 2.18, sz]); // shaft
            for &yy in &[-1.2f32, -0.6, 0.0, 0.6, 1.2] {
                b.part(&id, [2.2, 0.9, 2.2], col.accent, [sx, 2.18 + yy, sz]); // gold bands
            }
            let fy = 2.18 + 1.50; // finial centre
            let spin = t * 0.5;
            for i in 0..12 {
                let a = i as f32 / 12.0 * std::f32::consts::TAU + spin;
                b.part(&id, [0.55, 0.55, 0.55], col.accent, [sx + a.cos() * 0.62, fy + 0.74 + a.sin() * 0.78, sz]);
            }
            b.part(&id, [2.0, 2.0, 0.7], col.accent, [sx, fy + 0.74, sz - 0.04]); // sun disk
            b.part(&id, [1.6, 1.7, 1.0], col.glow, [sx, fy + 0.74, sz + 0.06]); // glowing core
            b.part(&id, [0.8, 0.95, 0.6], col.accent, [sx, fy + 1.95, sz]); // top finial
            // lantern hanging from the RIGHT hand (rides the arm swing)
            b.part(&w[HAR], [3.0, 0.55, 3.0], rgb(0xb28a44), [0.0, -0.55, 0.0]); // cage top
            b.part(&w[HAR], [3.0, 0.55, 3.0], rgb(0xb28a44), [0.0, -1.45, 0.0]); // cage bottom
            for &(cx, cz) in &[(-0.52f32, -0.52f32), (0.52, -0.52), (0.52, 0.52), (-0.52, 0.52)] {
                b.part(&w[HAR], [0.42, 3.2, 0.42], rgb(0xb28a44), [cx, -1.0, cz]); // bars
            }
            b.part(&w[HAR], [1.7, 2.3, 1.7], rgb(0xffc862), [0.0, -1.0, 0.0]); // caged light
        }
        HeroKind::Gravitybender => {
            // hooded cloak over the head + back
            b.part(&w[HEAD], [dim.head * 1.12, dim.head * 1.1, dim.head * 1.12], rgb(0x2a2433), [0.0, 0.30, -0.10]);
            b.part(&w[CHEST], [dim.t_w * 1.18, 22.0, 2.2], rgb(0x2a2433), [0.0, -0.4, -ff - 0.08]);
            // floating debris platform it levitates on (becomes the grounded base)
            for &(dx, dz, s) in &[(0.0f32, 0.0f32, 7.0f32), (-3.0, 1.5, 4.0), (3.2, -1.0, 4.5), (0.5, 3.0, 3.5), (-2.0, -2.5, 3.0)] {
                let sp = (t * 0.6 + dx).sin() * 0.4;
                b.part(&id, [s, 2.2, s], rgb(0x554c42), [dx * U * 4.0, 1.35 + sp, dz * U * 4.0]);
            }
            // purple gravity orb in front of the chest + orbiting rock cubes
            let oz = ff + 0.55;
            b.part(&w[CHEST], [2.0, 2.0, 2.0], col.glow, [0.0, 0.7, oz]); // orb
            for i in 0..3 {
                let a = i as f32 / 3.0 * std::f32::consts::TAU + t * 1.1;
                b.part(&w[CHEST], [0.9, 0.9, 0.9], rgb(0x6b6256),
                    [a.cos() * 0.7, 0.7 + a.sin() * 0.25, oz + a.sin() * 0.7]);
            }
        }
        HeroKind::Windwalker => {
            // AIR — an aviator's scarf that streams and flutters behind, gliding
            // wind-wisps orbiting the torso, and light air currents off the ankles.
            b.part(&w[NECK], [6.0, 1.8, 1.4], col.accent, [0.0, 0.05, ff * 0.35]); // scarf knot
            for i in 0..5 {
                let f = i as f32;
                let s = (t * 2.1 + f * 0.6).sin();
                let c = if i % 2 == 0 { col.accent } else { rgb(0xe4e4da) };
                b.part(&w[CHEST], [3.0 - f * 0.35, 1.3, 1.0], c,
                    [s * (0.05 + 0.035 * f), 0.15 - f * 0.30, -ff - 0.05 - f * 0.11]); // trailing tail
            }
            // wind-wisps circling the body (world space, so they orbit as it turns)
            for i in 0..3 {
                let a = i as f32 / 3.0 * std::f32::consts::TAU + t * 1.7;
                let yy = 1.95 + (t * 1.3 + i as f32).sin() * 0.18;
                b.part(&id, [1.5, 0.45, 0.45], col.glow, [a.cos() * 0.9, yy, a.sin() * 0.9]);
            }
            // faint updraft off each foot
            b.part(&w[HAL], [0.5, 2.4, 0.5], col.glow, [0.0, -0.5, 0.0]);
            b.part(&w[HAR], [0.5, 2.4, 0.5], col.glow, [0.0, -0.5, 0.0]);
        }
        HeroKind::Tidecaller => {
            // WATER — a wave-crest circlet, a flowing tidal train down the back, and
            // a water orb cupped in the hand with droplets in orbit.
            let hr = dim.head * U * 0.5;
            for i in 0..6 {
                let a = i as f32 / 6.0 * std::f32::consts::TAU;
                let crest = 1.4 + (i % 2) as f32 * 0.9;
                b.part(&w[HEAD], [0.9, crest, 0.9], col.accent, [a.cos() * hr, 0.30, a.sin() * hr]);
            }
            // rippling tidal train (behind, widening downward)
            for i in 0..4 {
                let f = i as f32;
                let s = (t * 1.6 + f * 0.7).sin();
                let c = if i % 2 == 0 { col.torso } else { col.accent };
                b.part(&w[CHEST], [(dim.t_w * 0.9 - f * 1.4).max(3.0), 7.0, 1.5], c,
                    [s * 0.05, -0.3 - f * 0.30, -ff - 0.05 - f * 0.06]);
            }
            // water orb in the right hand + orbiting droplets
            b.part(&w[HAR], [3.0, 3.0, 3.0], col.glow, [0.0, -0.5, 0.0]);
            for i in 0..4 {
                let a = i as f32 / 4.0 * std::f32::consts::TAU + t * 1.9;
                let yy = -0.5 + (t * 2.0 + i as f32).sin() * 0.12;
                b.part(&w[HAR], [0.7, 0.7, 0.7], col.accent, [a.cos() * 0.36, yy, a.sin() * 0.36]);
            }
        }
        HeroKind::Stormweaver => {
            // STORM — a dark cloud crackling overhead, and a charged staff whose orb
            // throws lightning arcs that strobe on and off.
            let flash = if (t * 7.0).sin() > 0.6 { 1.0 } else { 0.0 };
            for &(dx, dz, s) in &[(0.0f32, 0.0f32, 6.5f32), (-1.7, 0.4, 4.6), (1.8, -0.3, 4.9), (0.4, 1.3, 3.9)] {
                let bob = (t * 0.9 + dx).sin() * 0.06;
                b.part(&id, [s, 2.6, s * 0.8], rgb(0x3a3e46), [dx * 0.18, 2.98 + bob, -0.10 + dz * 0.12]);
            }
            if flash > 0.5 {
                for i in 0..3 {
                    let a = i as f32 / 3.0 * std::f32::consts::TAU + t;
                    b.part(&id, [0.32, 1.9, 0.32], col.glow, [a.cos() * 0.5, 2.35, -0.05 + a.sin() * 0.4]);
                }
            }
            // charged staff on the right hand
            b.part(&w[HAR], [1.0, 20.0, 1.0], rgb(0x4a4038), [0.0, 0.1, 0.0]); // shaft
            b.part(&w[HAR], [2.3, 2.3, 2.3], col.accent, [0.0, 0.80, 0.0]); // crown band
            b.part(&w[HAR], [1.6, 1.7, 1.6], col.glow, [0.0, 0.88, 0.0]); // electric orb
            if flash > 0.5 {
                b.part(&w[HAR], [0.3, 0.3, 3.2], col.glow, [0.0, 0.88, 0.0]); // arc z
                b.part(&w[HAR], [3.2, 0.3, 0.3], col.glow, [0.0, 0.88, 0.0]); // arc x
            }
        }
        // r683: Brine — salt-crust pauldron + a brine trident on the right hand.
        HeroKind::Brinekeeper => {
            b.part(&w[CHEST], [dim.t_w * 0.55, 3.0, dim.t_d * 1.1], col.accent, [-dim.t_w * 0.30 * U, 1.2, 0.0]); // pauldron
            b.part(&w[CHEST], [dim.t_w * 1.06, 2.0, dim.t_d * 1.06], rgb(0x3a5a58), [0.0, 0.9, 0.0]); // collar
            b.part(&w[HAR], [0.9, 22.0, 0.9], rgb(0x4a5a58), [0.0, 0.1, 0.0]); // trident shaft
            for &px in &[-0.9f32, 0.0, 0.9] {
                b.part(&w[HAR], [0.5, 4.0, 0.5], col.accent, [px, 1.15, 0.0]); // three prongs
            }
            b.part(&w[HAR], [1.5, 1.4, 1.5], col.glow, [0.0, 0.85, 0.0]); // brine core
            let a = t * 0.8;
            b.part(&w[CHEST], [0.8, 0.9, 0.8], col.glow, [a.cos() * 0.6, 0.7, ff + 0.4 + a.sin() * 0.3]); // orbiting droplet
        }
        // r683: Information — a scholar's circlet, orbiting glyph-lights, held tome.
        HeroKind::Luminant => {
            b.part(&w[HEAD], [dim.head * 1.06, 1.2, dim.head * 1.06], col.accent, [0.0, 0.48, 0.0]); // circlet
            b.part(&w[HEAD], [1.6, 0.5, 0.4], col.glow, [0.0, 0.30, ff + 0.02]); // brow light
            for i in 0..4 {
                let a = i as f32 / 4.0 * std::f32::consts::TAU + t * 0.7;
                b.part(&w[HEAD], [0.6, 0.6, 0.6], col.glow, [a.cos() * 1.05, 0.55 + (a * 2.0).sin() * 0.12, a.sin() * 1.05]); // glyphs
            }
            b.part(&w[HAR], [3.4, 0.6, 2.4], rgb(0x9090b0), [0.0, 0.0, 0.0]); // tome covers
            b.part(&w[HAR], [3.0, 0.5, 2.0], rgb(0xf0f0ff), [0.0, 0.35, 0.0]); // pages
            b.part(&w[HAR], [0.4, 0.5, 2.1], rgb(0x9090b0), [0.0, 0.40, 0.0]); // spine
            b.part(&w[HAR], [1.4, 1.0, 1.0], col.glow, [0.0, 0.9, 0.0]); // rising light
        }
    }
}

/// r549/r550: per-frame hero pose/anim inputs. Copy so `build` can read it freely.
#[derive(Clone, Copy, Debug)]
pub struct HeroPose {
    pub yaw: f32,
    pub gaze_pitch: f32,
    /// raw advancing walk phase (radians) + amplitude (0 standing … ~0.5 running).
    /// `build` derives the limb swing, vertical bob (2×), torso twist, etc. from these.
    pub walk_phase: f32,
    pub walk_amp: f32,
    /// continuously-advancing time for idle breathing/sway.
    pub anim_t: f32,
    /// equipped item colour to show in the hand; `None` = empty-handed.
    pub held_color: Option<[f32; 3]>,
    /// r570: 0 = grounded … 1 = fully airborne (drives a fall tuck — knees up,
    /// arms out for balance).
    pub airborne: f32,
    /// r570: signed vertical velocity (m/s); falling (negative) leans the torso
    /// back, rising leans it forward.
    pub vertical_vel: f32,
    /// r570: transient landing crouch (0..1), set on touchdown and decayed by the
    /// caller — deep knee/hip flex + arms forward to absorb the impact.
    pub land_crouch: f32,
    /// r572: 0 = upright … 1 = horizontal "superman" flight (body pitches forward,
    /// arms stream ahead, legs trail back). Set by the caller while actively flying.
    pub flight01: f32,
    /// r573: signed bank angle (radians) for flight turns — the body rolls into the
    /// turn so changing heading reads as a banked arc. Only applied while flying.
    pub bank: f32,
}

impl Default for HeroPose {
    fn default() -> Self {
        HeroPose { yaw: 0.0, gaze_pitch: 0.0, walk_phase: 0.0, walk_amp: 0.0, anim_t: 0.0, held_color: None,
            airborne: 0.0, vertical_vel: 0.0, land_crouch: 0.0, flight01: 0.0, bank: 0.0 }
    }
}

/// Overall hero height in **blocks**. `BuildParams.height` is now the stature in
/// blocks directly (default 1.8, slider 0.9..15.0 — see `docs/STATURE.md`); this
/// just clamps it. The bin uses it (and `sim::stature::derive`) to size the
/// model, camera eye height, and the player collider/mass so they all agree.
pub fn hero_total_height(params: &BuildParams) -> f32 {
    params.height.clamp(
        crate::sim::stature::MIN_HEIGHT,
        crate::sim::stature::MAX_HEIGHT,
    )
}

/// r663: reasonable default body proportions for each hero archetype — a distinct
/// silhouette per element (Gravitybender short + heavy, Windwalker tall + lean,
/// Tidecaller slighter, …). The character customiser seeds its sliders from this
/// when a hero is picked, then lets the player adjust from there.
pub fn default_params(kind: HeroKind) -> BuildParams {
    match kind {
        // Fire — balanced, sturdy, broad-shouldered.
        HeroKind::Heliographer => BuildParams { height: 1.85, bulk: 1.05, head: 1.0, shoulders: 1.10, arm: 1.05, leg: 1.0 },
        // Air — tall, lean, long-limbed.
        HeroKind::Windwalker => BuildParams { height: 2.00, bulk: 0.85, head: 0.98, shoulders: 0.95, arm: 0.95, leg: 1.15 },
        // Water — shorter, slighter (female build).
        HeroKind::Tidecaller => BuildParams { height: 1.70, bulk: 0.90, head: 1.00, shoulders: 0.90, arm: 0.92, leg: 1.02 },
        // Storm — tall, wiry, broad-shouldered.
        HeroKind::Stormweaver => BuildParams { height: 1.95, bulk: 0.95, head: 0.98, shoulders: 1.20, arm: 1.00, leg: 1.08 },
        // Earth — short, heavy, powerful.
        HeroKind::Gravitybender => BuildParams { height: 1.75, bulk: 1.35, head: 1.05, shoulders: 1.25, arm: 1.20, leg: 0.90 },
        // Brine — stocky, mineral-dense, broad.
        HeroKind::Brinekeeper => BuildParams { height: 1.78, bulk: 1.20, head: 1.02, shoulders: 1.15, arm: 1.10, leg: 0.96 },
        // Information — lean scholar, slightly larger head, long legs.
        HeroKind::Luminant => BuildParams { height: 1.82, bulk: 0.90, head: 1.05, shoulders: 0.98, arm: 0.96, leg: 1.05 },
    }
}

/// r663: suggested per-hero height range in **blocks** for a slider clamp — all
/// inside the global stature limits, `min < default_params(kind).height < max`.
pub fn height_range(kind: HeroKind) -> (f32, f32) {
    match kind {
        HeroKind::Heliographer => (1.6, 2.2),
        HeroKind::Windwalker => (1.8, 2.4),
        HeroKind::Tidecaller => (1.5, 2.0),
        HeroKind::Stormweaver => (1.7, 2.3),
        HeroKind::Gravitybender => (1.5, 2.1),
        HeroKind::Brinekeeper => (1.55, 2.05),
        HeroKind::Luminant => (1.65, 2.15),
    }
}

/// Build a forge hero: a jointed voxel humanoid (ported skeleton + buildBody),
/// posed by the walk gait + an idle sway, coloured per [`HeroKind`]. `origin` =
/// feet position; feet are auto-grounded to it. Returns engine entity geometry.
pub fn build(
    kind: HeroKind,
    params: &BuildParams,
    origin: [f32; 3],
    pose: &HeroPose,
) -> (Vec<EntityVertex>, Vec<u32>) {
    let col = kind.colors();
    let mut dim = kind.dims();
    // BuildParams sliders scale the ported proportions.
    // r558: overall height now comes from ONE uniform scale at finalize, so the
    // model defaults to ~2 blocks (was ~4.2) and the slider reaches ~4. The other
    // sliders adjust SHAPE (proportions) only, not the overall height.
    let bulk = params.bulk.clamp(0.7, 1.5);
    let head_s = params.head.clamp(0.7, 1.4);
    let sh_s = params.shoulders.clamp(0.7, 1.5);
    let arm_s = params.arm.clamp(0.7, 1.5);
    let leg_s = params.leg.clamp(0.7, 1.4);
    dim.t_w *= bulk;
    dim.t_d *= bulk;
    dim.arm_w *= arm_s * (0.6 + 0.4 * bulk);
    dim.head *= head_s;
    dim.sh *= sh_s;
    dim.leg_ul *= leg_s;
    dim.leg_ll *= leg_s;
    dim.leg_u *= leg_s.sqrt();
    dim.leg_l *= leg_s.sqrt();

    // ---- pose + animation as per-pivot euler rotations ----
    let amp = pose.walk_amp;
    let ph = pose.walk_phase;
    let gait = ph.sin() * amp; // primary fore/aft limb swing
    let stride2 = (ph * 2.0).sin(); // 2× stride — vertical bounce
    let t = pose.anim_t;
    let idle = (1.0 - (amp * 5.0)).clamp(0.0, 1.0); // 1 standing, 0 walking
    // walking dynamics
    let twist = gait * 0.55; // shoulders/chest counter-rotate the hips (about Y)
    let pelvis_tilt = (ph.cos()) * amp * 0.18; // hip drop side-to-side
    let lean = amp * 0.16; // lean into the stride
    let knee_f = 0.5 + 0.5 * amp; // deeper knee flex at speed
    // idle life
    let breathe = (t * 1.6).sin();
    let sway = (t * 0.8).sin() * 0.025 * idle;
    let weight = (t * 0.8).cos() * 0.04 * idle; // subtle weight shift (hip z-tilt)
    let head_turn = (t * 0.3).sin() * 0.12 * idle;
    let head_nod = (t * 0.5 + 0.7).sin() * 0.04 * idle;

    let mut rot = [[0.0f32; 3]; NPIV];
    rot[ROOT] = [0.0, 0.0, sway + pelvis_tilt * 0.4];
    rot[HIPS] = [0.0, -twist * 0.45, pelvis_tilt + weight];
    rot[CHEST] = [lean, twist, -pelvis_tilt * 0.5];
    rot[NECK] = [-lean * 0.5, 0.0, 0.0];
    rot[HEAD] = [-pose.gaze_pitch * 0.8 + head_nod, head_turn - twist * 0.5, 0.0];
    // legs swing fore/aft (about three-X); knees flex on the back/lift swing.
    rot[HIPL] = [gait, 0.0, 0.0];
    rot[HIPR] = [-gait, 0.0, 0.0];
    rot[KNL] = [(-gait).max(0.0) * knee_f * 1.6, 0.0, 0.0];
    rot[KNR] = [(gait).max(0.0) * knee_f * 1.6, 0.0, 0.0];
    // arms counter-swing the legs, larger throw + idle drift + elbow flex through swing.
    let arm = gait * 1.25;
    let drift = breathe * 0.04 * idle;
    rot[SHL] = [-arm + drift, 0.0, 0.13 + 0.05 * idle];
    rot[SHR] = [arm + drift, 0.0, -0.13 - 0.05 * idle];
    rot[ELL] = [-0.25 - (arm).max(0.0) * 0.8, 0.0, 0.0];
    rot[ELR] = [-0.25 - (-arm).max(0.0) * 0.8, 0.0, 0.0];

    // r570: AIRBORNE + LANDING poses, blended additively onto the walk rig (the
    // walk amp is ~0 mid-air anyway, so these read cleanly). Airborne → a fall
    // tuck: knees draw up, thighs flex, arms abduct out for balance, and the torso
    // leans back the faster you drop. land_crouch → a transient touchdown absorb:
    // deep knee/hip flex with the arms swung forward. Both are 0 on the ground.
    let air = pose.airborne.clamp(0.0, 1.0);
    let crouch = pose.land_crouch.clamp(0.0, 1.0);
    if air > 0.001 || crouch > 0.001 {
        let dropping = (-pose.vertical_vel / 12.0).clamp(0.0, 1.0); // 0..1 fall speed
        let rising = (pose.vertical_vel / 12.0).clamp(0.0, 1.0); // 0..1 ascend (fly/jump)
        // fall tuck
        let knee_tuck = air * 0.9;
        let hip_flex = air * 0.35;
        let arm_out = air * 0.7;
        let torso_lean = air * (0.20 * dropping - 0.14 * rising); // back falling, fwd rising
        // landing absorb
        let c_knee = crouch * 1.1;
        let c_hip = crouch * 0.5;
        let c_arm = crouch * 0.85;
        rot[KNL][0] += knee_tuck + c_knee;
        rot[KNR][0] += knee_tuck + c_knee;
        rot[HIPL][0] -= hip_flex + c_hip;
        rot[HIPR][0] -= hip_flex + c_hip;
        rot[CHEST][0] += torso_lean - crouch * 0.30; // fold forward on landing
        rot[SHL][2] += arm_out; // abduct arms outward (z-roll)
        rot[SHR][2] -= arm_out;
        rot[SHL][0] -= c_arm; // ...and swing them forward to brace on touchdown
        rot[SHR][0] -= c_arm;
        rot[ELL][0] -= air * 0.3; // slight elbow flex aloft
        rot[ELR][0] -= air * 0.3;
    }

    // r572/r573: FLIGHT pose — when the caller signals active flight, pitch the body
    // FORWARD into a face-down "superman" (r573: sign flipped — was belly-up), with
    // arms streaming ahead, legs trailing straight back, and the head lifting to keep
    // the gaze along the flight line. The body also ROLLS into turns by `bank` so a
    // heading change reads as a banked arc (turn radius). Blended by flight01.
    let fly = pose.flight01.clamp(0.0, 1.0);
    if fly > 0.001 {
        let lerp = |a: f32, b: f32| a * (1.0 - fly) + b * fly;
        rot[ROOT][0] += 1.45 * fly; // tilt torso forward to near-horizontal, face leading
        rot[ROOT][2] += pose.bank * fly; // bank/roll into the turn
        rot[NECK][0] = lerp(rot[NECK][0], -0.7); // lift head back up to the flight line
        rot[HEAD][0] = lerp(rot[HEAD][0], -0.6);
        // arms stream forward overhead
        rot[SHL][0] = lerp(rot[SHL][0], -2.6);
        rot[SHR][0] = lerp(rot[SHR][0], -2.6);
        rot[SHL][2] = lerp(rot[SHL][2], 0.12);
        rot[SHR][2] = lerp(rot[SHR][2], -0.12);
        rot[ELL][0] = lerp(rot[ELL][0], 0.0); // straighten elbows
        rot[ELR][0] = lerp(rot[ELR][0], 0.0);
        // legs straighten and trail back together
        rot[HIPL][0] = lerp(rot[HIPL][0], -0.22);
        rot[HIPR][0] = lerp(rot[HIPR][0], -0.22);
        rot[KNL][0] = lerp(rot[KNL][0], 0.0);
        rot[KNR][0] = lerp(rot[KNR][0], 0.0);
    }

    let base_y = 0.0; // feet auto-grounded at finalize
    let world = rig_world(&dim, base_y, &rot);

    let mut body = Body::new(pose.yaw);
    build_body(&mut body, &world, &dim, &col);
    dress(kind, &mut body, &world, &dim, &col, pose);

    // held item in the right hand (engine forward +X of the hand), tracks the arm.
    if let Some(hc) = pose.held_color {
        body.part(&world[HAR], [1.7, 5.2, 1.7], hc, [0.0, -0.10, ff_depth(&dim)]);
    }

    // a small vertical bounce on the walk (applied after grounding so the planted
    // pose still rests at the origin at mid-stride).
    let bob = (stride2 * 0.5 + 0.5) * amp * 0.10;
    body.finalize(origin, bob, hero_total_height(params))
}

#[inline]
fn ff_depth(dim: &HeroDims) -> f32 {
    dim.head * U / 2.0 + 0.05
}

/// r541/r549: 2D front-profile projection of a hero for the character menu preview.
/// Builds the hero at rest and normalises into the `[0,1]` unit square (+y up) that
/// the menu painter expects — same convention as `creature_viewer::project`.
pub fn preview_boxes(
    kind: HeroKind,
    params: &BuildParams,
) -> Vec<crate::creature_viewer::ProjectedBox> {
    let (verts, _idx) = build(kind, params, [0.0, 0.0, 0.0], &HeroPose::default());
    struct Raw { minx: f32, maxx: f32, miny: f32, maxy: f32, z: f32, color: [f32; 3] }
    let mut raws: Vec<Raw> = Vec::new();
    let (mut gminx, mut gmaxx) = (f32::MAX, f32::MIN);
    let (mut gminy, mut gmaxy) = (f32::MAX, f32::MIN);
    for c in verts.chunks(8) {
        if c.len() < 8 {
            break;
        }
        // FRONT view: width is along Z, so project Z→horizontal, Y→vertical, X→depth key.
        let (mut minx, mut maxx) = (f32::MAX, f32::MIN);
        let (mut miny, mut maxy) = (f32::MAX, f32::MIN);
        let mut zsum = 0.0;
        for v in c {
            minx = minx.min(v.position[2]);
            maxx = maxx.max(v.position[2]);
            miny = miny.min(v.position[1]);
            maxy = maxy.max(v.position[1]);
            zsum += v.position[0];
        }
        gminx = gminx.min(minx);
        gmaxx = gmaxx.max(maxx);
        gminy = gminy.min(miny);
        gmaxy = gmaxy.max(maxy);
        raws.push(Raw { minx, maxx, miny, maxy, z: zsum / 8.0, color: c[0].color });
    }
    let spanx = (gmaxx - gminx).max(1e-3);
    let spany = (gmaxy - gminy).max(1e-3);
    let span = spanx.max(spany);
    let offx = (span - spanx) * 0.5;
    let offy = (span - spany) * 0.5;
    let mut out: Vec<crate::creature_viewer::ProjectedBox> = raws
        .iter()
        .map(|r| crate::creature_viewer::ProjectedBox {
            x: (r.minx - gminx + offx) / span,
            y: (r.miny - gminy + offy) / span,
            w: (r.maxx - r.minx) / span,
            h: (r.maxy - r.miny) / span,
            color: r.color,
            depth: r.z,
            role: 0,
            bumpiness: 0.0,
        })
        .collect();
    out.sort_by(|a, b| a.depth.partial_cmp(&b.depth).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod default_param_tests {
    use super::*;

    const ALL: [HeroKind; 5] = [
        HeroKind::Heliographer,
        HeroKind::Windwalker,
        HeroKind::Tidecaller,
        HeroKind::Stormweaver,
        HeroKind::Gravitybender,
    ];

    #[test]
    fn defaults_are_distinct_and_in_range() {
        for k in ALL {
            let p = default_params(k);
            let (lo, hi) = height_range(k);
            assert!(lo < p.height && p.height < hi, "{k:?} height {} not in ({lo},{hi})", p.height);
            assert!(p.bulk > 0.0 && p.head > 0.0 && p.leg > 0.0);
        }
        // Air is tallest, Water/Earth are shorter; Earth is the bulkiest.
        assert!(default_params(HeroKind::Windwalker).height > default_params(HeroKind::Tidecaller).height);
        for k in ALL {
            if k != HeroKind::Gravitybender {
                assert!(default_params(HeroKind::Gravitybender).bulk > default_params(k).bulk);
            }
        }
    }
}
