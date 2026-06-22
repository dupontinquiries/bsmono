//! `sim::viewmodel` — first-person held-item ("model in hand") geometry (WID I1).
//!
//! Pure math that places a held item/tool in front of the camera with a walk
//! bob and a look sway, plus a tiny box model per [`ItemForm`]. Data + math only:
//! the bin feeds the camera basis + walk phase + look delta and draws the result;
//! this module never touches input or the draw path.
//!
//! VIEW-SPACE CONVENTION (matches a standard FPS viewmodel pass): the camera sits
//! at the origin looking down **−Z**, with **+X right** and **+Y up**. A held item
//! therefore lives at small −Z (in front), +X (right hand), −Y (below centre).
//! [`place_in_world`] maps a view-space point out to world space through a
//! [`CameraBasis`] the bin fills from its real camera.
//!
//! Pure / deterministic / panic-free: no `crate::render`, no GPU, no I/O, no
//! clock (phase is an argument), no RNG, no `.unwrap()`/`.expect()` in non-test
//! code. Release-safe (no `#[cfg]`-gated helpers used in asserts).

use crate::sim::elemental::ItemForm;

type V3 = [f32; 3];

// ── tiny vec helpers (dependency-free) ──
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// The camera frame the bin supplies (world space). `forward` points where the
/// camera looks; `right`/`up` complete a right-handed basis. The bin can fill
/// this straight from its glm camera (`pos`, `forward()`, `right()`, `up()`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraBasis {
    pub pos: V3,
    pub forward: V3,
    pub right: V3,
    pub up: V3,
}

impl CameraBasis {
    /// Identity basis at the origin: looks down −Z, +X right, +Y up (i.e. world
    /// space already equals view space). Handy for tests / the headless collage.
    pub fn identity() -> CameraBasis {
        CameraBasis { pos: [0.0, 0.0, 0.0], forward: [0.0, 0.0, -1.0], right: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0] }
    }
}

/// A posed transform for the viewmodel, expressed in VIEW space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewmodelPose {
    /// Translation of the item's grip in view space.
    pub offset: V3,
    /// Small orientation (radians): yaw about +Y, pitch about +X, roll about +Z.
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    /// Uniform scale of the model.
    pub scale: f32,
}

/// One oriented box of the local item model (item space: +Z is the length the
/// item points along, origin at the grip).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemBox {
    pub center: V3,
    pub half: V3,
    pub color: [u8; 3],
}

// ── tuning (all bounded; documented) ──
const BOB_X: f32 = 0.015; // lateral bob amplitude (m) at full walk speed
const BOB_Y: f32 = 0.022; // vertical bob amplitude (m)
const SWAY_K: f32 = 0.06; // look-delta → sway gain
const SWAY_MAX: f32 = 0.06; // sway clamp (m)
const SWAY_ROLL: f32 = 0.5; // look-delta → roll gain (rad)
const ROLL_MAX: f32 = 0.18; // roll clamp (rad)

/// Per-form base pose (offset, orientation, scale) before bob/sway. Items are
/// modelled along +Z and yawed ≈π so their length points into the screen (−Z
/// view = forward), angled slightly across the view for the held look.
fn base_pose(form: ItemForm) -> ViewmodelPose {
    const PI: f32 = std::f32::consts::PI;
    match form {
        // long staff held more upright, pushed forward
        ItemForm::Staff => ViewmodelPose { offset: [0.24, -0.26, -0.30], yaw: PI - 0.10, pitch: 0.18, roll: 0.05, scale: 1.0 },
        // axe: heavier, lower, canted
        ItemForm::Axe => ViewmodelPose { offset: [0.30, -0.34, -0.30], yaw: PI - 0.25, pitch: 0.05, roll: 0.18, scale: 1.0 },
        // blade: angled across, forward
        ItemForm::Blade => ViewmodelPose { offset: [0.28, -0.30, -0.32], yaw: PI - 0.35, pitch: 0.10, roll: 0.12, scale: 1.0 },
        // tool: small, close, low
        ItemForm::Tool => ViewmodelPose { offset: [0.26, -0.32, -0.30], yaw: PI - 0.18, pitch: 0.00, roll: 0.08, scale: 0.85 },
    }
}

/// Compute the viewmodel pose for this frame.
/// * `walk_phase` — the gait/anim phase in radians (reuse the forge anim phase).
/// * `speed01` — normalized move speed `[0,1]`; scales the bob (0 = no bob).
/// * `look_delta` — `[dx, dy]` look change this frame; drives a bounded sway/roll.
///
/// Bob traces a lemniscate (a horizontal wobble at the step rate, a vertical
/// bounce at twice that). Sway lags opposite the look so the item trails the
/// turn. Both are clamped so the model never leaves its corner of the screen.
pub fn viewmodel_pose(form: ItemForm, walk_phase: f32, speed01: f32, look_delta: [f32; 2]) -> ViewmodelPose {
    let base = base_pose(form);
    let s = speed01.clamp(0.0, 1.0);

    // bob (bounded by BOB_X / BOB_Y · s)
    let bob_x = BOB_X * s * walk_phase.sin();
    let bob_y = BOB_Y * s * (2.0 * walk_phase).sin();

    // sway: trail opposite the look delta, clamped
    let sway_x = (-look_delta[0] * SWAY_K).clamp(-SWAY_MAX, SWAY_MAX);
    let sway_y = (look_delta[1] * SWAY_K).clamp(-SWAY_MAX, SWAY_MAX);
    let roll = base.roll + (-look_delta[0] * SWAY_ROLL).clamp(-ROLL_MAX, ROLL_MAX);

    ViewmodelPose {
        offset: add(base.offset, [bob_x + sway_x, bob_y + sway_y, 0.0]),
        yaw: base.yaw,
        pitch: base.pitch,
        roll,
        scale: base.scale,
    }
}

/// Apply euler (yaw·Y, pitch·X, roll·Z) to a vector, then scale.
fn rotate_scale(p: V3, pose: &ViewmodelPose) -> V3 {
    let p = scale(p, pose.scale);
    // roll about Z
    let (sr, cr) = (pose.roll.sin(), pose.roll.cos());
    let p = [p[0] * cr - p[1] * sr, p[0] * sr + p[1] * cr, p[2]];
    // pitch about X
    let (sp, cp) = (pose.pitch.sin(), pose.pitch.cos());
    let p = [p[0], p[1] * cp - p[2] * sp, p[1] * sp + p[2] * cp];
    // yaw about Y
    let (sy, cy) = (pose.yaw.sin(), pose.yaw.cos());
    [p[0] * cy + p[2] * sy, p[1], -p[0] * sy + p[2] * cy]
}

/// Transform a local item-space point by the pose into VIEW space.
pub fn place_in_view(pose: &ViewmodelPose, local: V3) -> V3 {
    add(rotate_scale(local, pose), pose.offset)
}

/// Map a VIEW-space point out to WORLD space through the camera basis.
/// View `+X→right`, `+Y→up`, `−Z→forward` (so a point at view `z<0` is in front).
pub fn place_in_world(view: V3, cam: &CameraBasis) -> V3 {
    let mut w = cam.pos;
    w = add(w, scale(cam.right, view[0]));
    w = add(w, scale(cam.up, view[1]));
    w = add(w, scale(cam.forward, -view[2])); // −Z is forward
    w
}

/// The local box model for an item form (item space, +Z = length, grip at origin).
pub fn item_model(form: ItemForm) -> Vec<ItemBox> {
    let wood: [u8; 3] = [120, 84, 52];
    let steel: [u8; 3] = [180, 188, 200];
    let dark: [u8; 3] = [70, 74, 82];
    match form {
        ItemForm::Staff => vec![
            ItemBox { center: [0.0, 0.0, 0.45], half: [0.018, 0.018, 0.55], color: wood }, // shaft
            ItemBox { center: [0.0, 0.0, 1.02], half: [0.045, 0.045, 0.06], color: steel }, // tip/focus
        ],
        ItemForm::Axe => vec![
            ItemBox { center: [0.0, 0.0, 0.34], half: [0.022, 0.022, 0.40], color: wood }, // handle
            ItemBox { center: [0.06, 0.0, 0.70], half: [0.10, 0.16, 0.04], color: steel }, // head (offset blade)
            ItemBox { center: [0.0, 0.0, 0.72], half: [0.03, 0.05, 0.05], color: dark }, // eye
        ],
        ItemForm::Blade => vec![
            ItemBox { center: [0.0, 0.0, 0.10], half: [0.020, 0.020, 0.10], color: wood }, // grip
            ItemBox { center: [0.0, 0.0, 0.21], half: [0.075, 0.022, 0.02], color: dark }, // guard
            ItemBox { center: [0.0, 0.0, 0.62], half: [0.028, 0.010, 0.42], color: steel }, // blade
        ],
        ItemForm::Tool => vec![
            ItemBox { center: [0.0, 0.0, 0.22], half: [0.020, 0.020, 0.26], color: wood }, // handle
            ItemBox { center: [0.0, 0.05, 0.50], half: [0.06, 0.06, 0.05], color: steel }, // head
        ],
    }
}

/// The 8 view-space corners of a box (for a rasteriser / the bin), with its color.
pub fn box_corners_in_view(pose: &ViewmodelPose, b: &ItemBox) -> ([V3; 8], [u8; 3]) {
    let mut out = [[0.0f32; 3]; 8];
    let mut k = 0;
    for sx in [-1.0f32, 1.0] {
        for sy in [-1.0f32, 1.0] {
            for sz in [-1.0f32, 1.0] {
                let local = [b.center[0] + sx * b.half[0], b.center[1] + sy * b.half[1], b.center[2] + sz * b.half[2]];
                out[k] = place_in_view(pose, local);
                k += 1;
            }
        }
    }
    (out, b.color)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMS: [ItemForm; 4] = [ItemForm::Staff, ItemForm::Axe, ItemForm::Blade, ItemForm::Tool];

    #[test]
    fn zero_speed_zero_look_is_base_pose() {
        for &f in &FORMS {
            let p = viewmodel_pose(f, 1.234, 0.0, [0.0, 0.0]);
            let b = base_pose(f);
            assert_eq!(p.offset, b.offset, "no bob/sway at rest");
            assert_eq!((p.yaw, p.pitch, p.roll, p.scale), (b.yaw, b.pitch, b.roll, b.scale));
        }
    }

    #[test]
    fn bob_and_sway_are_bounded() {
        for &f in &FORMS {
            let base = base_pose(f);
            let mut phase = 0.0f32;
            while phase < 20.0 {
                for &spd in &[0.0f32, 0.5, 1.0, 2.0] {
                    for &ld in &[[0.0f32, 0.0], [50.0, -30.0], [-80.0, 40.0]] {
                        let p = viewmodel_pose(f, phase, spd, ld);
                        let dx = p.offset[0] - base.offset[0];
                        let dy = p.offset[1] - base.offset[1];
                        assert!(dx.abs() <= BOB_X + SWAY_MAX + 1e-6, "x excursion {dx} bounded");
                        assert!(dy.abs() <= BOB_Y + SWAY_MAX + 1e-6, "y excursion {dy} bounded");
                        assert!(p.offset.iter().all(|v| v.is_finite()));
                        assert!((p.roll - base.roll).abs() <= ROLL_MAX + 1e-6, "roll bounded");
                    }
                }
                phase += 0.37;
            }
        }
    }

    #[test]
    fn sway_trails_opposite_the_look() {
        let b = base_pose(ItemForm::Blade);
        // look right (+x) → item lags left (offset.x below base)
        let pr = viewmodel_pose(ItemForm::Blade, 0.0, 0.0, [60.0, 0.0]);
        assert!(pr.offset[0] < b.offset[0], "looking right trails the item left");
        assert!(pr.roll < b.roll, "looking right rolls it the trailing way");
        // look left mirrors
        let pl = viewmodel_pose(ItemForm::Blade, 0.0, 0.0, [-60.0, 0.0]);
        assert!(pl.offset[0] > b.offset[0]);
    }

    #[test]
    fn bob_is_periodic_and_grows_with_speed() {
        use std::f32::consts::TAU;
        let a = viewmodel_pose(ItemForm::Staff, 0.8, 1.0, [0.0, 0.0]);
        let b = viewmodel_pose(ItemForm::Staff, 0.8 + TAU, 1.0, [0.0, 0.0]);
        for i in 0..3 {
            assert!((a.offset[i] - b.offset[i]).abs() < 1e-4, "bob has period 2π");
        }
        // amplitude grows with speed
        let slow = viewmodel_pose(ItemForm::Staff, 1.5, 0.25, [0.0, 0.0]).offset;
        let fast = viewmodel_pose(ItemForm::Staff, 1.5, 1.0, [0.0, 0.0]).offset;
        let base = base_pose(ItemForm::Staff).offset;
        let amp_slow = (slow[1] - base[1]).abs();
        let amp_fast = (fast[1] - base[1]).abs();
        assert!(amp_fast >= amp_slow - 1e-6 && amp_fast > 0.0, "faster walk bobs more");
    }

    #[test]
    fn deterministic() {
        let p1 = viewmodel_pose(ItemForm::Axe, 2.1, 0.7, [12.0, -5.0]);
        let p2 = viewmodel_pose(ItemForm::Axe, 2.1, 0.7, [12.0, -5.0]);
        assert_eq!(p1, p2);
        let m = item_model(ItemForm::Axe);
        let c1 = box_corners_in_view(&p1, &m[0]);
        let c2 = box_corners_in_view(&p2, &m[0]);
        assert_eq!(c1, c2);
    }

    #[test]
    fn models_are_nonempty_finite_and_in_front() {
        for &f in &FORMS {
            let m = item_model(f);
            assert!(m.len() >= 2, "{f:?} has a multi-part model");
            let pose = viewmodel_pose(f, 0.0, 0.0, [0.0, 0.0]);
            for b in &m {
                let (corners, _) = box_corners_in_view(&pose, b);
                for c in &corners {
                    assert!(c.iter().all(|v| v.is_finite()), "finite corner");
                    assert!(c[2] < 0.0, "held item sits in front of the camera (view z<0), got z={}", c[2]);
                }
            }
        }
    }

    #[test]
    fn place_in_world_uses_the_basis() {
        // identity basis: world == view
        let id = CameraBasis::identity();
        let v = [0.1, -0.2, -0.5];
        assert_eq!(place_in_view(&base_pose(ItemForm::Tool), [0.0, 0.0, 0.0]), base_pose(ItemForm::Tool).offset);
        let w = place_in_world(v, &id);
        for i in 0..3 {
            assert!((w[i] - v[i]).abs() < 1e-6, "identity basis: world equals view, {w:?} vs {v:?}");
        }

        // a translated, +Z-looking camera: a point 2 in front lands 2·forward away
        let cam = CameraBasis { pos: [5.0, 1.0, 0.0], forward: [0.0, 0.0, 1.0], right: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0] };
        let front = place_in_world([0.0, 0.0, -2.0], &cam);
        assert!((front[0] - 5.0).abs() < 1e-6 && (front[1] - 1.0).abs() < 1e-6 && (front[2] - 2.0).abs() < 1e-6, "{front:?}");
    }
}
