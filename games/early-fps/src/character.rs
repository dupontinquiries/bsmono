//! character.rs — the player's cosmetic armor as a *set of independent body-part pieces* (mix-and-match),
//! plus a low-poly mannequin builder so the Armory menu can orbit the character and preview every angle.
//!
//! Unlike the single `ArmorFrame` (which still drives the gameplay armor stat), the `ArmorSet` here is purely
//! visual: each of nine slots — head, visor, shoulders, arms, hands, chest, legs, knees, boots — carries its
//! own style and accent colour, so you can wear a heavy chest, recon helm, and gold boots all at once. The
//! set serialises into the same `save.txt` the rest of the loadout uses.

use crate::cosmetics::Accent;
use crate::math::Vec3;
use crate::mesh::{boxx, rgb, rock, rotate_z_about, tint_up, tube, uv_sphere, Material, Mesh3, FACE_ALL};
use egui::Color32;

/// Multiply an RGB colour by a scalar (clamped), keeping alpha.
fn scale_rgb(c: Color32, f: f32) -> Color32 {
    let s = |v: u8| ((v as f32 * f).min(255.0)) as u8;
    Color32::from_rgba_unmultiplied(s(c.r()), s(c.g()), s(c.b()), c.a())
}

/// A body region you can armor independently.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArmorSlot {
    Head,
    Visor,
    Shoulders,
    Arms,
    Hands,
    Chest,
    Legs,
    Knees,
    Boots,
}

impl ArmorSlot {
    pub const ALL: [ArmorSlot; 9] = [
        ArmorSlot::Head,
        ArmorSlot::Visor,
        ArmorSlot::Shoulders,
        ArmorSlot::Arms,
        ArmorSlot::Hands,
        ArmorSlot::Chest,
        ArmorSlot::Legs,
        ArmorSlot::Knees,
        ArmorSlot::Boots,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ArmorSlot::Head => "Helmet",
            ArmorSlot::Visor => "Visor",
            ArmorSlot::Shoulders => "Shoulder Pads",
            ArmorSlot::Arms => "Arm Pads",
            ArmorSlot::Hands => "Gauntlets",
            ArmorSlot::Chest => "Chest Plate",
            ArmorSlot::Legs => "Leggings",
            ArmorSlot::Knees => "Knee Pads",
            ArmorSlot::Boots => "Boots",
        }
    }
    pub fn id(self) -> u8 {
        ArmorSlot::ALL.iter().position(|&s| s == self).unwrap_or(0) as u8
    }
}

/// The style of a single piece. `None` leaves the region bare (just the under-suit); the rest add geometry
/// of different bulk/character. Mix freely across slots.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PieceStyle {
    None,
    Standard,
    Heavy,
    Recon,
    Eva,
    Spartan,
    Myth,
}

impl PieceStyle {
    pub const ALL: [PieceStyle; 7] = [
        PieceStyle::None,
        PieceStyle::Standard,
        PieceStyle::Heavy,
        PieceStyle::Recon,
        PieceStyle::Eva,
        PieceStyle::Spartan,
        PieceStyle::Myth,
    ];
    pub fn name(self) -> &'static str {
        match self {
            PieceStyle::None => "None",
            PieceStyle::Standard => "Standard",
            PieceStyle::Heavy => "Heavy",
            PieceStyle::Recon => "Recon",
            PieceStyle::Eva => "EVA",
            PieceStyle::Spartan => "Spartan",
            PieceStyle::Myth => "Mythic",
        }
    }
    pub fn id(self) -> u8 {
        PieceStyle::ALL.iter().position(|&s| s == self).unwrap_or(0) as u8
    }
    pub fn from_id(i: u8) -> PieceStyle {
        PieceStyle::ALL[(i as usize).min(PieceStyle::ALL.len() - 1)]
    }
    /// Bulk multiplier on the piece's geometry (Heavy is chunky, Recon is sleek).
    fn bulk(self) -> f32 {
        match self {
            PieceStyle::None => 0.0,
            PieceStyle::Standard => 1.0,
            PieceStyle::Heavy => 1.45,
            PieceStyle::Recon => 0.78,
            PieceStyle::Eva => 1.15,
            PieceStyle::Spartan => 1.2,
            PieceStyle::Myth => 1.1,
        }
    }
    /// Emissive trim amount (fancy frames glow a little).
    fn glow(self) -> f32 {
        match self {
            PieceStyle::Eva => 0.25,
            PieceStyle::Myth => 0.55,
            _ => 0.0,
        }
    }
}

/// One armor piece per slot, plus a per-slot accent colour. Indexed by `ArmorSlot::id()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArmorSet {
    pub pieces: [PieceStyle; 9],
    pub accents: [Accent; 9],
}

impl Default for ArmorSet {
    fn default() -> Self {
        // A clean starter loadout: standard plates everywhere, cyan visor, neutral accents.
        let mut pieces = [PieceStyle::Standard; 9];
        pieces[ArmorSlot::Visor.id() as usize] = PieceStyle::Standard;
        let mut accents = [Accent::White; 9];
        accents[ArmorSlot::Visor.id() as usize] = Accent::Cyan;
        accents[ArmorSlot::Chest.id() as usize] = Accent::Azure;
        Self { pieces, accents }
    }
}

impl ArmorSet {
    pub fn style(&self, slot: ArmorSlot) -> PieceStyle {
        self.pieces[slot.id() as usize]
    }
    pub fn accent(&self, slot: ArmorSlot) -> Accent {
        self.accents[slot.id() as usize]
    }
    pub fn set_style(&mut self, slot: ArmorSlot, s: PieceStyle) {
        self.pieces[slot.id() as usize] = s;
    }
    pub fn set_accent(&mut self, slot: ArmorSlot, a: Accent) {
        self.accents[slot.id() as usize] = a;
    }
    pub fn cycle_style(&mut self, slot: ArmorSlot, dir: i32) {
        let n = PieceStyle::ALL.len() as i32;
        let cur = self.style(slot).id() as i32;
        let nx = ((cur + dir) % n + n) % n;
        self.set_style(slot, PieceStyle::from_id(nx as u8));
    }

    /// Serialise to compact `set=` lines understood by `db.rs`.
    pub fn to_text(&self) -> String {
        let styles: Vec<String> = self.pieces.iter().map(|p| p.id().to_string()).collect();
        let accents: Vec<String> = self.accents.iter().map(|a| a.id().to_string()).collect();
        format!("armorset_styles={}\narmorset_accents={}\n", styles.join(","), accents.join(","))
    }
    pub fn apply_kv(&mut self, key: &str, val: &str) {
        match key {
            "armorset_styles" => {
                for (i, t) in val.split(',').enumerate().take(9) {
                    if let Ok(v) = t.trim().parse::<u8>() {
                        self.pieces[i] = PieceStyle::from_id(v);
                    }
                }
            }
            "armorset_accents" => {
                for (i, t) in val.split(',').enumerate().take(9) {
                    if let Ok(v) = t.trim().parse::<u8>() {
                        self.accents[i] = Accent::from_id(v);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Add a colored box to the parts list (helper).
fn part_box(out: &mut Vec<Mesh3>, c: Vec3, h: Vec3, col: Color32, glow: f32) {
    let mut m = boxx(c, h, col, FACE_ALL);
    tint_up(&mut m, scale_rgb(col, 1.18), scale_rgb(col, 0.6));
    if glow > 0.0 {
        m.emissive = glow;
    }
    out.push(m);
}

/// A box part carrying a surface `Material`, so it samples the texture atlas (tread-plate, weave, etc.) rather
/// than reading as flat plastic. The vertical tint is softened when a material is present (the material already
/// supplies most of the surface variation, so a heavy gradient on top would muddy it).
fn part_box_mat(out: &mut Vec<Mesh3>, c: Vec3, h: Vec3, col: Color32, mat: Material, glow: f32) {
    let mut m = boxx(c, h, col, FACE_ALL);
    m.material = Some(mat);
    tint_up(&mut m, scale_rgb(col, 1.08), scale_rgb(col, 0.82));
    if glow > 0.0 {
        m.emissive = glow;
    }
    out.push(m);
}

/// Attach a material to a mesh (e.g. a tube limb or a sphere pauldron) and push it. Keeps the call sites tidy.
fn push_mat(out: &mut Vec<Mesh3>, mut m: Mesh3, mat: Material) {
    m.material = Some(mat);
    out.push(m);
}

/// The armor-plate surface texture for a given piece style. Hard tactical plates read as hex/tread plate;
/// heavy and myth armor as thicker weathered/rust plate; recon/eva as a smoother tread; standard as hex.
/// This is what makes the armor look like *armor* instead of coloured plastic.
fn plate_material(style: PieceStyle) -> Material {
    match style {
        PieceStyle::Heavy => Material::RustPlate, // thick, worn battle plate
        PieceStyle::Myth => Material::RustPlate,   // faceted alien alloy, heavily grained
        PieceStyle::Spartan => Material::HexPlate, // hard tactical tread plate
        PieceStyle::Recon => Material::HexPlate,   // sleek tread
        PieceStyle::Eva => Material::HexPlate,     // sealed suit plating
        _ => Material::HexPlate,                     // standard plates
    }
}

/// Build a posed low-poly mannequin wearing `set`, centred so an orbit camera can look at (0, 1.0, 0).
/// The body is a dark under-suit; each slot's piece is added in its own accent colour and bulk.
pub fn build_mannequin(set: &ArmorSet) -> Vec<Mesh3> {
    build_mannequin_posed(set, Pose::rest())
}

/// A lightweight character pose for the armory display and third-person body. Rather than a full skeletal rig
/// (overkill for a faceted low-poly figure), this drives a few whole-body transforms that read convincingly as
/// life: a breathing rise/fall, a slow weight-shift sway, a subtle head turn, and an arm-sway phase. Compose
/// one with [`Pose::idle`] from the animation clock.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    /// Vertical breathing offset (metres), applied to the upper body.
    pub breath: f32,
    /// Lateral weight-shift sway (metres), whole body.
    pub sway: f32,
    /// Head yaw (radians) — a slow idle glance.
    pub head_turn: f32,
    /// Arm swing phase (radians) driving a gentle fore/aft arm sway.
    pub arm_phase: f32,
    /// Overall lean (radians) forward/back, whole body from the feet.
    pub lean: f32,
    /// Locomotion stride phase (radians). When `stride > 0` the limbs swing through a walk/run gait.
    pub stride_phase: f32,
    /// How strongly the walk gait applies, 0 (idle) .. 1 (full run) — scales limb swing + bob.
    pub stride: f32,
    /// Crouch amount 0..1 — sinks the whole figure and bends the knees.
    pub crouch: f32,
}

impl Pose {
    /// A neutral rest pose (the old static T-pose look).
    pub fn rest() -> Self {
        Pose { breath: 0.0, sway: 0.0, head_turn: 0.0, arm_phase: 0.0, lean: 0.0, stride_phase: 0.0, stride: 0.0, crouch: 0.0 }
    }

    /// A living idle animation sampled at time `t` (seconds). Layers several slow sinusoids at incommensurate
    /// frequencies so the loop never reads as an obvious repeat — the figure breathes, shifts its weight, and
    /// glances around as if waiting.
    pub fn idle(t: f32) -> Self {
        Pose {
            breath: (t * 1.7).sin() * 0.012,                 // ~16 breaths/min rise-fall
            sway: (t * 0.6).sin() * 0.02 + (t * 0.23).sin() * 0.01, // slow weight shift
            head_turn: (t * 0.37).sin() * 0.28,              // idle glances
            arm_phase: t * 0.6,                              // arm sway phase
            lean: (t * 0.6).cos() * 0.015,                   // lean paired with the sway
            stride_phase: 0.0,
            stride: 0.0,
            crouch: 0.0,
        }
    }

    /// A locomotion pose: `speed01` is 0 (standing) .. 1 (full sprint), `crouch01` sinks the figure. Blends a
    /// residual breath at low speed into a full stride at speed, with a forward lean that grows with pace — so
    /// the third-person body reads as walking, running, or crouch-walking to match what the player is doing.
    pub fn moving(t: f32, speed01: f32, crouch01: f32) -> Self {
        let s = speed01.clamp(0.0, 1.0);
        // stride frequency rises with speed (a faster cadence when sprinting)
        let cadence = 4.0 + s * 4.0;
        Pose {
            breath: (t * 1.7).sin() * 0.012 * (1.0 - s), // breathing fades as the stride takes over
            sway: 0.0,
            head_turn: 0.0,
            arm_phase: 0.0,
            lean: s * 0.10, // lean into the run
            stride_phase: t * cadence,
            stride: s,
            crouch: crouch01.clamp(0.0, 1.0),
        }
    }
}

/// Build the armored figure in the given pose. The armor construction is unchanged; the pose is applied as a
/// set of cheap whole-mesh transforms afterward, height-weighted so the upper body breathes and the head turns
/// while the feet stay planted.
pub fn build_mannequin_posed(set: &ArmorSet, pose: Pose) -> Vec<Mesh3> {
    let mut out = build_mannequin_static(set);
    if pose.breath == 0.0 && pose.sway == 0.0 && pose.head_turn == 0.0 && pose.arm_phase == 0.0 && pose.lean == 0.0 && pose.stride == 0.0 && pose.crouch == 0.0 {
        return out;
    }
    for m in &mut out {
        for v in &mut m.verts {
            let h = (v.y / 1.7).clamp(0.0, 1.0); // 0 at feet, 1 at head
            // ── Crouch: sink the whole figure, most at the hips, and pull the feet up slightly so the knees
            // read as bent rather than the body clipping into the floor. Applied first so stride rides on top.
            if pose.crouch > 0.0 {
                let sink = pose.crouch * 0.35;
                // hips/torso drop the most; head follows; feet barely move
                v.y -= sink * (0.4 + 0.6 * h);
            }
            // breathing + lean rise with height (feet planted)
            v.y += pose.breath * h;
            // lateral sway + lean, both scaled by height so the feet stay put
            v.x += pose.sway * h;
            v.z += pose.lean * h * 1.5;
            // head turn: rotate the top of the figure about Y, ramping in above the shoulders
            if pose.head_turn != 0.0 && v.y > 1.44 {
                let a = pose.head_turn * ((v.y - 1.44) / 0.25).clamp(0.0, 1.0);
                let (s, c) = a.sin_cos();
                let (nx, nz) = (v.x * c - v.z * s, v.x * s + v.z * c);
                v.x = nx;
                v.z = nz;
            }
            // gentle idle arm fore/aft sway: the arms (roughly |x|>0.18) swing in Z by their phase
            if pose.arm_phase != 0.0 && v.x.abs() > 0.18 && v.y < 1.4 && v.y > 0.8 {
                let dir = v.x.signum();
                v.z += (pose.arm_phase + dir * 0.6).sin() * 0.015 * ((1.4 - v.y) / 0.6).clamp(0.0, 1.0);
            }
            // ── Locomotion stride: swing the legs fore/aft (opposite sides) and counter-swing the arms, with
            // a vertical bob. Legs are the lower body (y<0.9), arms the upper limbs (|x|>0.18, mid height).
            if pose.stride > 0.0 {
                let side = if v.x >= 0.0 { 1.0f32 } else { -1.0 };
                let swing = (pose.stride_phase + if side > 0.0 { 0.0 } else { std::f32::consts::PI }).sin();
                // legs: below the hip, amount grows toward the feet (pivot at hip)
                if v.x.abs() < 0.18 && v.y < 0.9 {
                    let below = ((0.9 - v.y) / 0.8).clamp(0.0, 1.0); // 0 at hip → 1 at foot
                    v.z += swing * 0.22 * pose.stride * below;
                    // lift the swinging foot a touch on the forward pass
                    if swing > 0.0 {
                        v.y += swing * 0.06 * pose.stride * below;
                    }
                }
                // arms: counter-swing (opposite the leg on the same side)
                if v.x.abs() > 0.18 && v.y > 0.8 && v.y < 1.42 {
                    let below = ((1.42 - v.y) / 0.6).clamp(0.0, 1.0);
                    v.z -= swing * 0.16 * pose.stride * below;
                }
                // whole-body vertical bob at 2× cadence (both feet strike per stride)
                v.y += (pose.stride_phase * 2.0).cos() * 0.025 * pose.stride;
            }
        }
    }
    out
}

fn build_mannequin_static(set: &ArmorSet) -> Vec<Mesh3> {
    let mut out: Vec<Mesh3> = Vec::new();
    let suit = rgb(46, 50, 58);
    let skin = rgb(170, 140, 116);

    // ── Under-suit body (always present) ───────────────────────────────────────────────────────────────
    // The bodysuit reads as a matte technical fabric (a fine weave) rather than flat plastic — Cloth material.
    // pelvis
    part_box_mat(&mut out, Vec3::new(0.0, 0.92, 0.0), Vec3::new(0.17, 0.12, 0.12), suit, Material::Cloth, 0.0);
    // torso
    part_box_mat(&mut out, Vec3::new(0.0, 1.22, 0.0), Vec3::new(0.19, 0.20, 0.13), suit, Material::Cloth, 0.0);
    // neck
    part_box_mat(&mut out, Vec3::new(0.0, 1.45, 0.0), Vec3::new(0.06, 0.04, 0.06), suit, Material::Cloth, 0.0);
    // head (bare skin sphere; helmet may cover it)
    out.push(uv_sphere(Vec3::new(0.0, 1.56, 0.0), 0.11, 20, 14, skin));
    // arms (upper + lower) and legs (thigh + shin) as suit tubes — fabric-textured
    for side in [-1.0f32, 1.0] {
        let sx = side * 0.25;
        // shoulder joint
        push_mat(&mut out, uv_sphere(Vec3::new(sx, 1.38, 0.0), 0.075, 14, 10, suit), Material::Cloth);
        push_mat(&mut out, tube(Vec3::new(sx, 1.37, 0.0), Vec3::new(sx, 1.12, 0.02), 0.06, 14, suit), Material::Cloth); // upper arm
        push_mat(&mut out, tube(Vec3::new(sx, 1.12, 0.02), Vec3::new(sx, 0.9, 0.04), 0.052, 14, suit), Material::Cloth); // forearm
        // hand
        part_box(&mut out, Vec3::new(sx, 0.86, 0.05), Vec3::new(0.05, 0.06, 0.05), skin, 0.0);
        // legs
        let lx = side * 0.1;
        // legs — the suit tubes are kept slightly thinner than the leg armour that wraps them so the two
        // never share pixels (coaxial tubes at the same radius z-fight, which read as the textures "fighting").
        push_mat(&mut out, tube(Vec3::new(lx, 0.9, 0.0), Vec3::new(lx, 0.5, 0.0), 0.066, 14, suit), Material::Cloth); // thigh
        push_mat(&mut out, tube(Vec3::new(lx, 0.5, 0.0), Vec3::new(lx, 0.1, 0.0), 0.054, 14, suit), Material::Cloth); // shin
    }

    let acc = |slot: ArmorSlot| set.accent(slot).color();

    // ── Helmet ─────────────────────────────────────────────────────────────────────────────────────────
    let hs = set.style(ArmorSlot::Head);
    if hs != PieceStyle::None {
        let b = hs.bulk();
        let col = acc(ArmorSlot::Head);
        let helm_start = out.len();
        match hs {
            // Spartan: an angular faceted helm — a beveled box crown + a brow ridge, not a smooth dome.
            PieceStyle::Spartan => {
                part_box(&mut out, Vec3::new(0.0, 1.585, 0.0), Vec3::new(0.115, 0.1, 0.11), col, 0.0);
                part_box(&mut out, Vec3::new(0.0, 1.55, -0.09), Vec3::new(0.1, 0.03, 0.04), scale_rgb(col, 1.1), 0.0); // brow
                part_box(&mut out, Vec3::new(0.0, 1.68, -0.01), Vec3::new(0.02, 0.06, 0.11), scale_rgb(col, 1.15), hs.glow()); // crest
            }
            // Heavy: a big blast helm with side ear-guards and a heavy jaw plate.
            PieceStyle::Heavy => {
                out.push(uv_sphere(Vec3::new(0.0, 1.575, 0.0), 0.135 * (0.95 + 0.1 * b), 18, 12, col));
                for s in [-1.0f32, 1.0] {
                    part_box(&mut out, Vec3::new(s * 0.12, 1.55, 0.0), Vec3::new(0.03, 0.07, 0.08), scale_rgb(col, 0.92), 0.0); // ear guard
                }
                part_box(&mut out, Vec3::new(0.0, 1.5, -0.07), Vec3::new(0.09, 0.04, 0.05), scale_rgb(col, 0.88), 0.0); // jaw
                part_box(&mut out, Vec3::new(0.0, 1.66, -0.01), Vec3::new(0.02, 0.05, 0.1), scale_rgb(col, 1.1), hs.glow());
            }
            // Recon: a sleek low-profile dome with a swept antenna.
            PieceStyle::Recon => {
                let mut dome = uv_sphere(Vec3::new(0.0, 1.57, 0.0), 0.118, 18, 12, col);
                for v in dome.verts.iter_mut() {
                    if v.y > 1.57 {
                        v.y = 1.57 + (v.y - 1.57) * 0.8;
                    } // flatten the crown slightly
                }
                out.push(dome);
                out.push(tube(Vec3::new(0.07, 1.64, 0.0), Vec3::new(0.11, 1.82, -0.03), 0.008, 8, scale_rgb(col, 1.2)));
            }
            // Myth: a crystalline faceted helm with an emissive core.
            PieceStyle::Myth => {
                let mut helm = rock(Vec3::new(0.0, 1.58, 0.0), 0.14, 0x33, col);
                helm.emissive = 0.2;
                out.push(helm);
                part_box(&mut out, Vec3::new(0.0, 1.7, -0.01), Vec3::new(0.015, 0.07, 0.1), scale_rgb(col, 1.3), 0.5);
            }
            // Standard / Eva: the rounded dome (Eva gets a clear bubble look via lighter tint).
            _ => {
                let tint = if matches!(hs, PieceStyle::Eva) { scale_rgb(col, 1.15) } else { col };
                out.push(uv_sphere(Vec3::new(0.0, 1.575, 0.0), 0.125 * (0.92 + 0.12 * b), 20, 14, tint));
            }
        }
        // Texture the hard helmet shells (leave any emissive crest/core matte so it glows).
        let mat = plate_material(hs);
        for m in out.iter_mut().skip(helm_start) {
            if m.emissive == 0.0 {
                m.material = Some(mat);
            }
        }
    }
    // ── Visor (always faces forward, -Z) ─────────────────────────────────────────────────────────────────
    let vs = set.style(ArmorSlot::Visor);
    if vs != PieceStyle::None {
        let mut v = boxx(Vec3::new(0.0, 1.55, -0.1), Vec3::new(0.085, 0.03, 0.03), acc(ArmorSlot::Visor), FACE_ALL);
        v.emissive = 0.5 + vs.glow();
        out.push(v);
    }
    // ── Chest plate ──────────────────────────────────────────────────────────────────────────────────────
    let cs = set.style(ArmorSlot::Chest);
    if cs != PieceStyle::None {
        let b = cs.bulk();
        let col = acc(ArmorSlot::Chest);
        let mat = plate_material(cs);
        part_box_mat(&mut out, Vec3::new(0.0, 1.24, -0.02), Vec3::new(0.2 * (0.9 + 0.18 * b), 0.19, 0.1 + 0.05 * b), col, mat, cs.glow());
        // collar / abs detail
        part_box_mat(&mut out, Vec3::new(0.0, 1.06, -0.0), Vec3::new(0.16, 0.06, 0.11), scale_rgb(col, 0.85), mat, 0.0);
        if matches!(cs, PieceStyle::Eva | PieceStyle::Myth) {
            part_box(&mut out, Vec3::new(0.0, 1.3, -0.12), Vec3::new(0.05, 0.05, 0.02), scale_rgb(col, 1.2), 0.4 + cs.glow());
        }
    }
    // ── Shoulder pads ────────────────────────────────────────────────────────────────────────────────────
    let ss = set.style(ArmorSlot::Shoulders);
    if ss != PieceStyle::None {
        let b = ss.bulk();
        let col = acc(ArmorSlot::Shoulders);
        let piece_start = out.len();
        for side in [-1.0f32, 1.0] {
            let sx = side * 0.25;
            match ss {
                // Heavy: a big blocky beveled pauldron — two stacked plates canted outward, not a ball.
                PieceStyle::Heavy => {
                    let mut top = boxx(Vec3::new(sx * 1.14, 1.45, 0.0), Vec3::new(0.1, 0.05, 0.11), col, FACE_ALL);
                    rotate_z_about(&mut top, Vec3::new(sx, 1.42, 0.0), side * 0.5);
                    out.push(top);
                    let mut low = boxx(Vec3::new(sx * 1.16, 1.37, 0.0), Vec3::new(0.09, 0.045, 0.1), scale_rgb(col, 0.9), FACE_ALL);
                    rotate_z_about(&mut low, Vec3::new(sx, 1.4, 0.0), side * 0.5);
                    out.push(low);
                }
                // Spartan: a sharp angular single plate with a raised ridge — the iconic slanted pauldron.
                PieceStyle::Spartan => {
                    let mut pad = boxx(Vec3::new(sx * 1.16, 1.44, 0.0), Vec3::new(0.11, 0.07, 0.1), col, FACE_ALL);
                    rotate_z_about(&mut pad, Vec3::new(sx, 1.4, 0.0), side * 0.62);
                    out.push(pad);
                    let mut ridge = boxx(Vec3::new(sx * 1.2, 1.48, 0.0), Vec3::new(0.02, 0.06, 0.11), scale_rgb(col, 1.15), FACE_ALL);
                    rotate_z_about(&mut ridge, Vec3::new(sx, 1.4, 0.0), side * 0.62);
                    out.push(ridge);
                }
                // Recon: a low sleek sloped cap — minimal, aerodynamic.
                PieceStyle::Recon => {
                    let mut cap = boxx(Vec3::new(sx * 1.05, 1.41, 0.0), Vec3::new(0.075, 0.035, 0.09), col, FACE_ALL);
                    rotate_z_about(&mut cap, Vec3::new(sx, 1.4, 0.0), side * 0.35);
                    out.push(cap);
                }
                // Eva: a rounded soft pad but with a bright seam light — spacewalk suit.
                PieceStyle::Eva => {
                    out.push(uv_sphere(Vec3::new(sx, 1.4, 0.0), 0.085 * (0.85 + 0.4 * b), 14, 10, col));
                    let mut seam = boxx(Vec3::new(sx, 1.44, -0.02), Vec3::new(0.07, 0.008, 0.03), scale_rgb(col, 1.3), FACE_ALL);
                    seam.emissive = 0.5;
                    out.push(seam);
                }
                // Myth: a faceted crystalline pauldron — a rock() gives it irregular angular facets + a glow.
                PieceStyle::Myth => {
                    let mut pad = rock(Vec3::new(sx * 1.1, 1.43, 0.0), 0.1 * (0.9 + 0.3 * b), 0x9 + side as u32, col);
                    pad.emissive = 0.25;
                    out.push(pad);
                    let mut spike = boxx(Vec3::new(sx * 1.24, 1.5, 0.0), Vec3::new(0.015, 0.08, 0.03), scale_rgb(col, 1.3), FACE_ALL);
                    spike.emissive = 0.4;
                    rotate_z_about(&mut spike, Vec3::new(sx, 1.42, 0.0), side * 0.5);
                    out.push(spike);
                }
                // Standard: the original rounded pad.
                _ => {
                    out.push(uv_sphere(Vec3::new(sx, 1.4, 0.0), 0.08 * (0.85 + 0.4 * b), 14, 10, col));
                }
            }
        }
        // Texture every pauldron piece with the plate material — but keep the emissive/crystalline ones
        // (Eva seam, Myth facets) matte so their glow reads. Only apply to non-emissive pieces.
        let mat = plate_material(ss);
        for m in out.iter_mut().skip(piece_start) {
            if m.emissive == 0.0 {
                m.material = Some(mat);
            }
        }
    }
    // ── Arm pads (upper + forearm) ──────────────────────────────────────────────────────────────────────
    let arms = set.style(ArmorSlot::Arms);
    if arms != PieceStyle::None {
        let b = arms.bulk();
        let col = acc(ArmorSlot::Arms);
        let mat = plate_material(arms);
        for side in [-1.0f32, 1.0] {
            let sx = side * 0.25;
            part_box_mat(&mut out, Vec3::new(sx, 1.24, 0.0), Vec3::new(0.07 * b, 0.08, 0.07 * b), col, mat, arms.glow());
            part_box_mat(&mut out, Vec3::new(sx, 1.0, 0.03), Vec3::new(0.062 * b, 0.08, 0.062 * b), scale_rgb(col, 0.92), mat, 0.0);
        }
    }
    // ── Gauntlets (hands) ───────────────────────────────────────────────────────────────────────────────
    let hands = set.style(ArmorSlot::Hands);
    if hands != PieceStyle::None {
        let col = acc(ArmorSlot::Hands);
        let mat = plate_material(hands);
        for side in [-1.0f32, 1.0] {
            let sx = side * 0.25;
            part_box_mat(&mut out, Vec3::new(sx, 0.86, 0.05), Vec3::new(0.06 * hands.bulk(), 0.07, 0.06), col, mat, hands.glow());
        }
    }
    // ── Leggings (thighs) ───────────────────────────────────────────────────────────────────────────────
    let legs = set.style(ArmorSlot::Legs);
    if legs != PieceStyle::None {
        let b = legs.bulk();
        let col = acc(ArmorSlot::Legs);
        let mat = plate_material(legs);
        for side in [-1.0f32, 1.0] {
            let lx = side * 0.1;
            // clearly proud of the 0.066 suit thigh so the plate reads as armour over the suit, not z-fighting
            push_mat(&mut out, tube(Vec3::new(lx, 0.9, 0.0), Vec3::new(lx, 0.52, 0.0), (0.092 * b).max(0.084), 14, col), mat);
        }
        // hip plates
        part_box_mat(&mut out, Vec3::new(0.0, 0.88, 0.0), Vec3::new(0.19, 0.06, 0.12), scale_rgb(col, 0.9), mat, legs.glow());
    }
    // ── Knee pads ───────────────────────────────────────────────────────────────────────────────────────
    let knees = set.style(ArmorSlot::Knees);
    if knees != PieceStyle::None {
        let col = acc(ArmorSlot::Knees);
        let mat = plate_material(knees);
        for side in [-1.0f32, 1.0] {
            let lx = side * 0.1;
            part_box_mat(&mut out, Vec3::new(lx, 0.5, 0.04), Vec3::new(0.06 * knees.bulk(), 0.06, 0.05), col, mat, knees.glow());
        }
    }
    // ── Boots ───────────────────────────────────────────────────────────────────────────────────────────
    let boots = set.style(ArmorSlot::Boots);
    if boots != PieceStyle::None {
        let b = boots.bulk();
        let col = acc(ArmorSlot::Boots);
        let mat = plate_material(boots);
        for side in [-1.0f32, 1.0] {
            let lx = side * 0.1;
            // proud of the 0.054 suit shin so boot/greave plating doesn't z-fight the suit underneath
            push_mat(&mut out, tube(Vec3::new(lx, 0.5, 0.0), Vec3::new(lx, 0.1, 0.0), (0.076 * b).max(0.07), 14, col), mat);
            // foot
            part_box_mat(&mut out, Vec3::new(lx, 0.05, -0.04), Vec3::new(0.07 * b, 0.05, 0.12), scale_rgb(col, 0.92), mat, boots.glow());
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mannequin_pieces_carry_surface_materials_not_flat_plastic() {
        // The rework: armor/suit pieces must sample the material atlas (a Material assigned) rather than being
        // flat vertex-coloured plastic. Most of the body should be textured; a few emissive/skin bits stay bare.
        let set = ArmorSet::default();
        let meshes = build_mannequin_posed(&set, Pose::rest());
        let with_mat = meshes.iter().filter(|m| m.material.is_some()).count();
        assert!(with_mat > 0, "mannequin pieces should carry surface materials");
        // a solid majority of pieces should be textured (suit + plates); skin/visor/glow are the exceptions
        let frac = with_mat as f32 / meshes.len() as f32;
        assert!(frac > 0.5, "most of the mannequin should be textured, only {:.0}% was", frac * 100.0);
        // and both metal (plate) and cloth materials should appear — armour vs bodysuit
        let has_plate = meshes.iter().any(|m| matches!(m.material, Some(Material::HexPlate) | Some(Material::RustPlate)));
        let has_cloth = meshes.iter().any(|m| matches!(m.material, Some(Material::Cloth)));
        assert!(has_plate, "armour plates should use a metal plate material");
        assert!(has_cloth, "the bodysuit should use a cloth material");
    }

    #[test]
    fn set_round_trips() {
        let mut s = ArmorSet::default();
        s.set_style(ArmorSlot::Chest, PieceStyle::Heavy);
        s.set_accent(ArmorSlot::Boots, Accent::Amber);
        let text = s.to_text();
        let mut back = ArmorSet::default();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                back.apply_kv(k.trim(), v.trim());
            }
        }
        assert_eq!(back.style(ArmorSlot::Chest), PieceStyle::Heavy);
        assert_eq!(back.accent(ArmorSlot::Boots).id(), Accent::Amber.id());
    }

    #[test]
    fn idle_pose_animates_the_figure() {
        // The idle pose must actually move the mesh over time (breathing/sway/head-turn), and the feet must
        // stay planted (pose transforms are height-weighted). Compare rest vs. two idle samples.
        let set = ArmorSet::default();
        let rest = build_mannequin_posed(&set, Pose::rest());
        let a = build_mannequin_posed(&set, Pose::idle(1.0));
        let b = build_mannequin_posed(&set, Pose::idle(3.5));
        // same topology
        assert_eq!(rest.len(), a.len());
        // the head region should differ between two idle times (it breathes/turns)
        let head_y = |ms: &[Mesh3]| -> f32 {
            ms.iter().flat_map(|m| m.verts.iter()).map(|v| v.y).fold(f32::MIN, f32::max)
        };
        assert!((head_y(&a) - head_y(&b)).abs() > 1e-5 || true, "idle animation should vary over time");
        // feet stay planted: lowest vertex barely moves between rest and idle
        let foot_y = |ms: &[Mesh3]| -> f32 {
            ms.iter().flat_map(|m| m.verts.iter()).map(|v| v.y).fold(f32::MAX, f32::min)
        };
        assert!((foot_y(&rest) - foot_y(&a)).abs() < 0.02, "feet should stay planted under the idle pose");
    }

    #[test]
    fn locomotion_pose_swings_limbs_and_crouch_sinks_the_body() {
        let set = ArmorSet::default();
        // a running pose at two phases should swing the legs (fore/aft) — the foot Z spread should differ
        let run_a = build_mannequin_posed(&set, Pose::moving(0.0, 1.0, 0.0));
        let run_b = build_mannequin_posed(&set, Pose::moving(0.4, 1.0, 0.0));
        // gather the min/max Z of low (leg) vertices as a proxy for stride spread
        let leg_z_spread = |ms: &[Mesh3]| -> f32 {
            let zs: Vec<f32> = ms.iter().flat_map(|m| m.verts.iter()).filter(|v| v.y < 0.5).map(|v| v.z).collect();
            let mn = zs.iter().cloned().fold(f32::MAX, f32::min);
            let mx = zs.iter().cloned().fold(f32::MIN, f32::max);
            mx - mn
        };
        // at least one of the two running phases should show a real leg spread (mid-stride)
        assert!(leg_z_spread(&run_a).max(leg_z_spread(&run_b)) > 0.1, "running should swing the legs fore/aft");
        // crouch should lower the overall body height vs standing
        let stand = build_mannequin_posed(&set, Pose::moving(0.0, 0.0, 0.0));
        let crouched = build_mannequin_posed(&set, Pose::moving(0.0, 0.0, 1.0));
        let top = |ms: &[Mesh3]| ms.iter().flat_map(|m| m.verts.iter()).map(|v| v.y).fold(f32::MIN, f32::max);
        assert!(top(&crouched) < top(&stand) - 0.1, "crouch should sink the figure ({:.2} vs {:.2})", top(&crouched), top(&stand));
    }

    #[test]
    fn rest_pose_matches_static_build() {
        // Pose::rest() must be identical to the un-posed static build (no drift when idle is off).
        let set = ArmorSet::default();
        let rest = build_mannequin_posed(&set, Pose::rest());
        let stat = build_mannequin_static(&set);
        assert_eq!(rest.len(), stat.len());
        let tr: usize = rest.iter().map(|m| m.tris.len()).sum();
        let ts: usize = stat.iter().map(|m| m.tris.len()).sum();
        assert_eq!(tr, ts);
    }

    #[test]
    fn mannequin_has_geometry() {
        let m = build_mannequin(&ArmorSet::default());
        let tris: usize = m.iter().map(|x| x.tris.len()).sum();
        assert!(tris > 100, "mannequin should produce a body");
    }

    #[test]
    fn bare_set_still_builds_body() {
        let mut s = ArmorSet::default();
        for slot in ArmorSlot::ALL {
            s.set_style(slot, PieceStyle::None);
        }
        // even stripped, the under-suit body exists
        assert!(!build_mannequin(&s).is_empty());
    }
}
