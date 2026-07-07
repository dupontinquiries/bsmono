//! Cat rig — a posable papercraft cat. Two body units (fore/hind), a head, tail,
//! and four two-bone legs (thigh+shank) that pivot at the hip. A `Pose` captures
//! per-leg swing/knee plus body bob/pitch/crouch/tail/head, so the same builder
//! renders any gait phase or special pose (leap, startle, climb).

use crate::math::{v3, Vec3};
use crate::mesh::{rgb, Mesh};

const TAU: f32 = std::f32::consts::TAU;
const PI: f32 = std::f32::consts::PI;

#[derive(Clone, Copy)]
pub struct Pose {
    pub swing: [f32; 4], // per-leg fore/back angle (FL, FR, HL, HR)
    pub knee: [f32; 4],  // knee flex
    pub bob: f32,        // vertical body bounce
    pub pitch: f32,      // nose up(+)/down(−)
    pub crouch: f32,     // 0..1 lowers the body, folds the legs
    pub tail: f32,       // tail sway/lift
    pub head: f32,       // head pitch
}

fn base() -> Pose { Pose { swing: [0.0; 4], knee: [0.35; 4], bob: 0.0, pitch: 0.0, crouch: 0.0, tail: 0.2, head: 0.0 } }

pub fn stand() -> Pose { base() }

/// Four-beat walk (lateral sequence).
pub fn walk(t: f32) -> Pose {
    let off = [0.0, 0.5, 0.75, 0.25];
    let mut p = base();
    for i in 0..4 {
        let ph = (t + off[i]) * TAU;
        p.swing[i] = 0.5 * ph.sin();
        p.knee[i] = 0.28 + 0.45 * (-ph.cos()).max(0.0);
    }
    p.bob = 0.04 * (2.0 * t * TAU).sin();
    p.tail = 0.2 * (t * TAU).sin();
    p
}

/// Gallop/bound — fore pair together, hind pair together, big amplitude.
pub fn bound(t: f32) -> Pose {
    let mut p = base();
    let ph = t * TAU;
    for i in 0..4 {
        let a = if i < 2 { ph } else { ph + PI };
        p.swing[i] = 0.75 * a.sin();
        p.knee[i] = 0.5 + 0.5 * (-a.cos()).max(0.0);
    }
    p.bob = 0.13 * ph.sin();
    p.pitch = 0.25 * ph.cos();
    p.tail = 0.3;
    p
}

/// Mid-leap — body stretched, legs extended.
pub fn leap() -> Pose {
    let mut p = base();
    p.swing = [0.9, 0.9, -0.8, -0.8];
    p.knee = [0.1; 4];
    p.pitch = 0.4; p.bob = 0.25; p.tail = 0.45; p.head = 0.3;
    p
}

/// Anticipation crouch before a leap.
pub fn coil() -> Pose {
    let mut p = base();
    p.crouch = 1.0; p.knee = [1.0; 4]; p.pitch = -0.12; p.tail = 0.1; p.head = -0.2;
    p
}

/// Startle — arched back, tail up, about to bolt.
pub fn startle() -> Pose {
    let mut p = base();
    p.crouch = 0.5; p.pitch = 0.3; p.tail = 1.0; p.knee = [0.7; 4]; p.head = 0.35;
    p
}

/// Clinging to a trunk/branch, angled upward, gripping.
pub fn climb() -> Pose {
    let mut p = base();
    p.pitch = 0.7;
    p.swing = [0.85, 0.85, -0.5, -0.5];
    p.knee = [0.6, 0.6, 0.5, 0.5];
    p.tail = -0.3; p.head = 0.4;
    p
}

pub fn build(m: &mut Mesh, at: Vec3, face: f32, p: &Pose) {
    let pelt = rgb(0x9a7b53);
    let dark = rgb(0x6f5638);
    let fz = face;
    let stand_h = 0.52;
    let body_y = at.y + stand_h * (1.0 - p.crouch * 0.35) + p.bob;

    let fore = v3(at.x, body_y + p.pitch * 0.25 + 0.03, at.z + 0.30 * fz);
    let hind = v3(at.x, body_y - p.pitch * 0.25, at.z - 0.35 * fz);
    m.boxx(hind, v3(0.34, 0.30, 0.45), pelt);
    m.boxx(fore, v3(0.32, 0.30, 0.42), pelt);

    let head = v3(at.x, fore.y + 0.22 + p.head * 0.12, at.z + 0.78 * fz);
    m.boxx(head, v3(0.27, 0.24, 0.25), pelt);
    m.cone(v3(head.x - 0.15, head.y + 0.24, head.z), 0.1, 0.22, 4, dark);
    m.cone(v3(head.x + 0.15, head.y + 0.24, head.z), 0.1, 0.22, 4, dark);

    let tb = v3(at.x, hind.y + 0.05, at.z - 0.78 * fz);
    let ttip = v3(tb.x + p.tail * 0.3, tb.y + 0.2 + p.tail.abs() * 0.25, tb.z - 0.5 * fz);
    m.segment(tb, ttip, 0.08, 0.04, 5, pelt);

    let hips = [
        v3(at.x - 0.22, body_y, at.z + 0.32 * fz),
        v3(at.x + 0.22, body_y, at.z + 0.32 * fz),
        v3(at.x - 0.22, body_y, at.z - 0.32 * fz),
        v3(at.x + 0.22, body_y, at.z - 0.32 * fz),
    ];
    let (lt, ls) = (0.28, 0.28);
    for i in 0..4 {
        let sw = p.swing[i];
        let kn = p.knee[i] + p.crouch * 0.4;
        let hip = hips[i];
        let knee = v3(hip.x, hip.y - lt * sw.cos(), hip.z + lt * sw.sin() * fz);
        let foot = v3(knee.x, knee.y - ls * (sw + kn).cos(), knee.z + ls * (sw + kn).sin() * fz);
        m.segment(hip, knee, 0.09, 0.075, 5, dark);
        m.segment(knee, foot, 0.075, 0.05, 5, dark);
    }
}
