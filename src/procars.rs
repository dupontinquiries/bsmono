//! Procedural cars. The GLTF trio maps a spectrum — Octane (tall balanced
//! hitbox: the dribble all-rounder), Fennec (same hitbox, flat visual top:
//! ball-control readability), Dominus (long + low: the flick/aerial plank).
//! These three fill the gaps that spectrum leaves open:
//!   BREADBOX — taller and shorter than Octane: maximum "sponge", the ball
//!     settles on the roof shelf; slowest-feeling, comfiest dribbler.
//!   WEDGE    — flatter and longer than Dominus: a doorstop. The ultimate
//!     flip-reset plank and power-flick blade, twitchy to balance on.
//!   ROLLER   — a dome pod on a skateboard: tiny contact patch, hard mode,
//!     but the round top gives weirdly soft first touches.
//! Built from baked+merged primitive meshes; one prim per color, wheels baked
//! at their pivots so the existing spin/steer matrices just work.

use glam::{Quat, Vec3};

use crate::meshgen::{self, MeshData};
use crate::model::{Drive, Model, Prim, Tag};
use crate::render::Gpu;

fn part(base: MeshData, scale: Vec3, rot: Quat, off: Vec3) -> MeshData {
    let mut m = base;
    meshgen::bake(&mut m, scale, rot, off);
    m
}

fn body_prim(gpu: &mut Gpu, mesh: MeshData, color: [f32; 4]) -> Prim {
    Prim { mesh_id: gpu.add_mesh(&mesh), tex_id: None, color, tag: Tag::Body, pivot: Vec3::ZERO }
}

fn wheel_prim(gpu: &mut Gpu, r: f32, at: Vec3, front: bool) -> Prim {
    // A real tyre, baked at its pivot. This used to be a squashed sphere,
    // which is most of why the older cars read as toys — every one of them
    // gets the upgrade for free by sharing this function.
    let m = part(meshgen::wheel(0.55, 18), Vec3::splat(r), Quat::IDENTITY, at);
    Prim {
        mesh_id: gpu.add_mesh(&m),
        tex_id: None,
        color: [0.08, 0.08, 0.10, 1.0],
        tag: if front { Tag::WheelFront } else { Tag::WheelBack },
        pivot: at,
    }
}

fn wheels(gpu: &mut Gpu, prims: &mut Vec<Prim>, r: f32, half_w: f32, front_z: f32, back_z: f32, y: f32) {
    for (z, front) in [(front_z, true), (back_z, false)] {
        for sx in [-1.0f32, 1.0] {
            prims.push(wheel_prim(gpu, r, Vec3::new(sx * half_w, y, z), front));
        }
    }
}

// ------------------------------------------------------ electric cars ----
// Built to a higher standard than the block cars above: chamfered hulls
// that catch a highlight on every edge, a proper tyre-and-rim wheel, a
// smoked glasshouse, and a light bar that actually emits. They also DRIVE
// differently — an electric motor has its torque at zero revs, so these
// win the first two car-lengths off a kickoff and give a little back on a
// long straight.

fn ev_wheel(gpu: &mut Gpu, r: f32, at: Vec3, front: bool, rim: [f32; 4]) -> Vec<Prim> {
    let tyre = part(meshgen::wheel(0.55, 20), Vec3::splat(r), Quat::from_rotation_z(0.0), at);
    let disc = part(meshgen::sphere(8, 14), Vec3::new(r * 0.30, r * 0.62, r * 0.62), Quat::IDENTITY, at);
    vec![
        Prim { mesh_id: gpu.add_mesh(&tyre), tex_id: None, color: [0.07, 0.07, 0.09, 1.0], tag: if front { Tag::WheelFront } else { Tag::WheelBack }, pivot: at },
        Prim { mesh_id: gpu.add_mesh(&disc), tex_id: None, color: rim, tag: if front { Tag::WheelFront } else { Tag::WheelBack }, pivot: at },
    ]
}

fn ev_wheels(gpu: &mut Gpu, prims: &mut Vec<Prim>, r: f32, half_w: f32, fz: f32, bz: f32, y: f32, rim: [f32; 4]) {
    for (z, front) in [(fz, true), (bz, false)] {
        for sx in [-1.0f32, 1.0] {
            prims.extend(ev_wheel(gpu, r, Vec3::new(sx * half_w, y, z), front, rim));
        }
    }
}

/// The shared recipe: a lofted hull, a glasshouse set into it, wheel arches,
/// a light bar at each end and sill strips along the battery floor. Sections
/// run nose (-z) to tail (+z) as (z, half width, floor, roof, roundness).
fn ev_body(
    gpu: &mut Gpu,
    prims: &mut Vec<Prim>,
    sections: &[(f32, f32, f32, f32, f32)],
    paint: [f32; 4],
    glass: (f32, f32, f32, f32),
    front_light: [f32; 4],
    tail_light: [f32; 4],
) {
    let hull = part(meshgen::loft(sections, 24), Vec3::ONE, Quat::IDENTITY, Vec3::ZERO);
    prims.push(body_prim(gpu, hull, paint));
    // glasshouse: a smaller loft riding on top, smoked
    let (gz0, gz1, gw, gy) = glass;
    let cabin = [
        (gz0, gw * 0.72, gy, gy + 0.02, 0.9),
        (gz0 + (gz1 - gz0) * 0.35, gw, gy - 0.01, gy + 0.085, 0.85),
        (gz1, gw * 0.80, gy - 0.01, gy + 0.055, 0.9),
    ];
    prims.push(body_prim(
        gpu,
        part(meshgen::loft(&cabin, 20), Vec3::ONE, Quat::IDENTITY, Vec3::ZERO),
        [0.09, 0.12, 0.16, 1.0],
    ));
    let nose = sections[0];
    let tail = sections[sections.len() - 1];
    prims.push(body_prim(
        gpu,
        part(meshgen::cbox(0.40), Vec3::new(nose.1 * 0.86, 0.013, 0.02), Quat::IDENTITY, Vec3::new(0.0, (nose.2 + nose.3) * 0.5 + 0.02, nose.0 + 0.01)),
        front_light,
    ));
    prims.push(body_prim(
        gpu,
        part(meshgen::cbox(0.40), Vec3::new(tail.1 * 0.88, 0.013, 0.02), Quat::IDENTITY, Vec3::new(0.0, (tail.2 + tail.3) * 0.5 + 0.02, tail.0 - 0.01)),
        tail_light,
    ));
    // sills: the giveaway that the floor is a battery
    let mid = sections[sections.len() / 2];
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(
            gpu,
            part(meshgen::cbox(0.28), Vec3::new(0.012, 0.016, mid.0.abs().max(0.30)), Quat::IDENTITY, Vec3::new(sx * mid.1 * 0.97, mid.2 + 0.03, 0.0)),
            [front_light[0] * 0.5, front_light[1] * 0.8, front_light[2], 1.0],
        ));
    }
}

/// Wheel arches: a flared band over each wheel, so the body has haunches.
fn arches(gpu: &mut Gpu, prims: &mut Vec<Prim>, r: f32, half_w: f32, fz: f32, bz: f32, y: f32, col: [f32; 4]) {
    for z in [fz, bz] {
        for sx in [-1.0f32, 1.0] {
            let m = part(
                meshgen::torus(0.30, 18, 8),
                Vec3::new(r * 1.30, r * 1.30, r * 1.30),
                Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                Vec3::new(sx * (half_w * 0.92), y, z),
            );
            prims.push(body_prim(gpu, m, col));
        }
    }
}

/// VOLT — a compact electric hatch. Short, tall-ish, friendly: the EV that
/// dribbles like an Octane.
pub fn volt(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    let paint = [0.20, 0.62, 0.86, 1.0];
    ev_body(
        gpu,
        &mut prims,
        &[
            (-0.52, 0.20, -0.05, 0.09, 0.75),
            (-0.34, 0.28, -0.10, 0.15, 0.55),
            (0.02, 0.30, -0.12, 0.17, 0.45),
            (0.34, 0.29, -0.11, 0.16, 0.50),
            (0.52, 0.22, -0.06, 0.10, 0.75),
        ],
        paint,
        (-0.24, 0.26, 0.24, 0.16),
        [0.85, 0.95, 1.0, 1.0],
        [1.0, 0.25, 0.20, 1.0],
    );
    arches(gpu, &mut prims, 0.125, 0.30, -0.36, 0.36, -0.07, [0.13, 0.14, 0.16, 1.0]);
    ev_wheels(gpu, &mut prims, 0.125, 0.28, -0.36, 0.36, -0.07, [0.72, 0.76, 0.80, 1.0]);
    Model { prims, drive: Drive::Electric, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.56, half_h: 0.26, half_w: 0.31, wheel_r: 0.125 }
}

/// AXION — a low electric hypercar. Long, flat, fast: the flick blade.
pub fn axion(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    let paint = [0.08, 0.09, 0.12, 1.0];
    ev_body(
        gpu,
        &mut prims,
        &[
            (-0.74, 0.20, -0.06, 0.00, 0.85),
            (-0.48, 0.32, -0.08, 0.04, 0.55),
            (-0.10, 0.36, -0.09, 0.07, 0.40),
            (0.34, 0.35, -0.08, 0.06, 0.45),
            (0.62, 0.28, -0.06, 0.03, 0.70),
        ],
        paint,
        (-0.10, 0.30, 0.26, 0.06),
        [0.75, 0.90, 1.0, 1.0],
        [1.0, 0.20, 0.30, 1.0],
    );
    // spoiler on struts
    prims.push(body_prim(gpu, part(meshgen::cbox(0.30), Vec3::new(0.34, 0.011, 0.06), Quat::from_rotation_x(0.16), Vec3::new(0.0, 0.16, 0.60)), [0.16, 0.16, 0.18, 1.0]));
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(gpu, part(meshgen::cbox(0.25), Vec3::new(0.011, 0.05, 0.025), Quat::IDENTITY, Vec3::new(sx * 0.24, 0.12, 0.60)), [0.16, 0.16, 0.18, 1.0]));
    }
    arches(gpu, &mut prims, 0.105, 0.36, -0.50, 0.50, -0.045, [0.11, 0.11, 0.13, 1.0]);
    ev_wheels(gpu, &mut prims, 0.105, 0.34, -0.50, 0.50, -0.045, [0.55, 0.58, 0.62, 1.0]);
    Model { prims, drive: Drive::Electric, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.78, half_h: 0.17, half_w: 0.38, wheel_r: 0.105 }
}

/// HAULER — an electric pickup, all shoulders. The heaviest-feeling of the
/// three, with a flat bed the ball settles into.
pub fn hauler(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    let paint = [0.72, 0.74, 0.78, 1.0];
    ev_body(
        gpu,
        &mut prims,
        &[
            (-0.62, 0.26, -0.06, 0.10, 0.35),
            (-0.40, 0.33, -0.10, 0.16, 0.20),
            (0.00, 0.33, -0.11, 0.14, 0.15),
            (0.36, 0.32, -0.10, 0.12, 0.15),
            (0.60, 0.29, -0.07, 0.10, 0.30),
        ],
        paint,
        (-0.40, -0.02, 0.29, 0.15),
        [0.90, 0.96, 1.0, 1.0],
        [1.0, 0.35, 0.22, 1.0],
    );
    // bed walls: a shelf the ball settles into
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(gpu, part(meshgen::cbox(0.22), Vec3::new(0.028, 0.06, 0.22), Quat::IDENTITY, Vec3::new(sx * 0.30, 0.16, 0.32)), paint));
    }
    prims.push(body_prim(gpu, part(meshgen::cbox(0.22), Vec3::new(0.31, 0.06, 0.028), Quat::IDENTITY, Vec3::new(0.0, 0.16, 0.56)), paint));
    arches(gpu, &mut prims, 0.145, 0.33, -0.40, 0.40, -0.06, [0.20, 0.21, 0.24, 1.0]);
    ev_wheels(gpu, &mut prims, 0.145, 0.31, -0.40, 0.40, -0.06, [0.28, 0.30, 0.33, 1.0]);
    Model { prims, drive: Drive::Electric, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.66, half_h: 0.28, half_w: 0.34, wheel_r: 0.145 }
}

/// PULSE — a rally EV on tall soft tyres: short overhangs, high stance, a
/// light pod on the roof. Lands well and shrugs off bad ones.
pub fn pulse(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    let paint = [0.96, 0.55, 0.12, 1.0];
    ev_body(
        gpu,
        &mut prims,
        &[
            (-0.46, 0.24, -0.02, 0.12, 0.55),
            (-0.28, 0.31, -0.06, 0.18, 0.40),
            (0.04, 0.32, -0.07, 0.19, 0.35),
            (0.32, 0.30, -0.06, 0.17, 0.45),
            (0.48, 0.25, -0.03, 0.12, 0.60),
        ],
        paint,
        (-0.22, 0.20, 0.26, 0.19),
        [1.0, 0.95, 0.75, 1.0],
        [1.0, 0.30, 0.18, 1.0],
    );
    // roof light pod, four lamps
    prims.push(body_prim(gpu, part(meshgen::cbox(0.30), Vec3::new(0.20, 0.020, 0.05), Quat::IDENTITY, Vec3::new(0.0, 0.31, -0.14)), [0.14, 0.15, 0.17, 1.0]));
    for k in 0..4 {
        let x = -0.135 + k as f32 * 0.09;
        prims.push(body_prim(gpu, part(meshgen::sphere(8, 12), Vec3::splat(0.030), Quat::IDENTITY, Vec3::new(x, 0.325, -0.185)), [1.0, 0.97, 0.85, 1.0]));
    }
    arches(gpu, &mut prims, 0.155, 0.32, -0.32, 0.34, -0.02, [0.15, 0.15, 0.17, 1.0]);
    ev_wheels(gpu, &mut prims, 0.155, 0.30, -0.32, 0.34, -0.02, [0.85, 0.86, 0.88, 1.0]);
    Model { prims, drive: Drive::Electric, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.52, half_h: 0.30, half_w: 0.33, wheel_r: 0.155 }
}

/// TRAM — an electric shuttle: a long slab of glass and aluminium with a
/// roof rail. Enormous roof shelf, so the ball just sits there.
pub fn tram(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    let paint = [0.86, 0.88, 0.91, 1.0];
    ev_body(
        gpu,
        &mut prims,
        &[
            (-0.58, 0.24, -0.06, 0.20, 0.50),
            (-0.36, 0.30, -0.10, 0.26, 0.25),
            (0.02, 0.31, -0.11, 0.27, 0.18),
            (0.38, 0.30, -0.10, 0.26, 0.25),
            (0.58, 0.25, -0.06, 0.21, 0.50),
        ],
        paint,
        (-0.44, 0.44, 0.28, 0.24),
        [0.80, 0.92, 1.0, 1.0],
        [1.0, 0.28, 0.24, 1.0],
    );
    // roof rail
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(gpu, part(meshgen::cbox(0.35), Vec3::new(0.014, 0.014, 0.34), Quat::IDENTITY, Vec3::new(sx * 0.22, 0.30, 0.02)), [0.30, 0.32, 0.35, 1.0]));
    }
    arches(gpu, &mut prims, 0.125, 0.31, -0.38, 0.40, -0.07, [0.22, 0.23, 0.26, 1.0]);
    ev_wheels(gpu, &mut prims, 0.125, 0.29, -0.38, 0.40, -0.07, [0.62, 0.65, 0.70, 1.0]);
    Model { prims, drive: Drive::Electric, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.62, half_h: 0.34, half_w: 0.32, wheel_r: 0.125 }
}

pub fn breadbox(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    // tall van body + darker cab face + mint roof rail
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.30, 0.26, 0.52), Quat::IDENTITY, Vec3::new(0.0, 0.10, 0.02)), [0.62, 0.86, 0.72, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.27, 0.10, 0.10), Quat::IDENTITY, Vec3::new(0.0, 0.16, -0.50)), [0.16, 0.20, 0.24, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.24, 0.02, 0.40), Quat::IDENTITY, Vec3::new(0.0, 0.38, 0.04)), [0.93, 0.90, 0.80, 1.0]));
    wheels(gpu, &mut prims, 0.115, 0.27, -0.36, 0.36, -0.10);
    Model { prims, drive: Drive::Combustion, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.56, half_h: 0.30, half_w: 0.31, wheel_r: 0.115 }
}

pub fn wedge(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    // long low doorstop: main slab pitched nose-down + tail block + red stripe
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.40, 0.075, 0.78), Quat::from_rotation_x(-0.10), Vec3::new(0.0, 0.015, 0.0)), [0.13, 0.13, 0.15, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.34, 0.055, 0.16), Quat::IDENTITY, Vec3::new(0.0, 0.10, 0.58)), [0.16, 0.16, 0.19, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.05, 0.078, 0.70), Quat::from_rotation_x(-0.10), Vec3::new(0.0, 0.02, 0.0)), [0.85, 0.16, 0.16, 1.0]));
    wheels(gpu, &mut prims, 0.095, 0.36, -0.52, 0.52, -0.06);
    Model { prims, drive: Drive::Combustion, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.80, half_h: 0.125, half_w: 0.42, wheel_r: 0.095 }
}

pub fn roller(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    // skateboard deck + glass dome pod + banana stripe
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.24, 0.035, 0.46), Quat::IDENTITY, Vec3::new(0.0, -0.02, 0.0)), [0.94, 0.80, 0.30, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::sphere(16, 22), Vec3::new(0.20, 0.20, 0.22), Quat::IDENTITY, Vec3::new(0.0, 0.16, -0.02)), [0.55, 0.85, 0.92, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.25, 0.012, 0.10), Quat::IDENTITY, Vec3::new(0.0, 0.012, 0.34)), [0.95, 0.45, 0.25, 1.0]));
    wheels(gpu, &mut prims, 0.13, 0.22, -0.30, 0.30, -0.05);
    Model { prims, drive: Drive::Combustion, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.48, half_h: 0.22, half_w: 0.26, wheel_r: 0.13 }
}

pub fn longboy(gpu: &mut Gpu) -> Model {
    // the limousine plank: past-Wedge length, comedic reach, wobbly to spin
    let mut prims = Vec::new();
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.30, 0.085, 1.02), Quat::IDENTITY, Vec3::new(0.0, 0.02, 0.0)), [0.14, 0.15, 0.20, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.26, 0.07, 0.30), Quat::IDENTITY, Vec3::new(0.0, 0.13, -0.30)), [0.10, 0.10, 0.13, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.31, 0.012, 1.00), Quat::IDENTITY, Vec3::new(0.0, 0.105, 0.0)), [0.85, 0.78, 0.30, 1.0]));
    wheels(gpu, &mut prims, 0.095, 0.27, -0.74, 0.74, -0.055);
    Model { prims, drive: Drive::Combustion, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 1.04, half_h: 0.14, half_w: 0.32, wheel_r: 0.095 }
}

pub fn cube(gpu: &mut Gpu) -> Model {
    // the cube: equal in every direction, wrong in every situation. touches
    // are chaos, dribbles are a dare
    let mut prims = Vec::new();
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::splat(0.30), Quat::IDENTITY, Vec3::new(0.0, 0.05, 0.0)), [0.90, 0.35, 0.30, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.31, 0.06, 0.31), Quat::IDENTITY, Vec3::new(0.0, 0.36, 0.0)), [0.98, 0.92, 0.85, 1.0]));
    wheels(gpu, &mut prims, 0.11, 0.28, -0.24, 0.24, -0.16);
    Model { prims, drive: Drive::Combustion, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.34, half_h: 0.34, half_w: 0.34, wheel_r: 0.11 }
}

pub fn shark(gpu: &mut Gpu) -> Model {
    // streamliner with a dorsal fin: mid hitbox, unmistakable silhouette
    let mut prims = Vec::new();
    prims.push(body_prim(gpu, part(meshgen::sphere(14, 20), Vec3::new(0.30, 0.16, 0.64), Quat::IDENTITY, Vec3::new(0.0, 0.03, 0.0)), [0.35, 0.48, 0.62, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.025, 0.17, 0.16), Quat::from_rotation_x(0.28), Vec3::new(0.0, 0.24, 0.14)), [0.30, 0.42, 0.55, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.30, 0.02, 0.10), Quat::IDENTITY, Vec3::new(0.0, -0.02, -0.60)), [0.92, 0.92, 0.95, 1.0]));
    wheels(gpu, &mut prims, 0.10, 0.28, -0.44, 0.44, -0.09);
    Model { prims, drive: Drive::Combustion, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.66, half_h: 0.20, half_w: 0.31, wheel_r: 0.10 }
}
