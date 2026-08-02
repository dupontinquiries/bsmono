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
    // squashed sphere as a tire, baked at its pivot
    let m = part(meshgen::sphere(12, 16), Vec3::new(r * 0.55, r, r), Quat::IDENTITY, at);
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

/// VOLT — a compact electric hatch. Short, tall-ish, friendly: the EV that
/// dribbles like an Octane.
pub fn volt(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    let body = [0.20, 0.62, 0.86, 1.0];
    // lower hull, chamfered, with a slight nose-down rake
    prims.push(body_prim(gpu, part(meshgen::cbox(0.22), Vec3::new(0.30, 0.14, 0.52), Quat::from_rotation_x(-0.04), Vec3::new(0.0, 0.02, 0.0)), body));
    // shoulder line and glasshouse
    prims.push(body_prim(gpu, part(meshgen::cbox(0.30), Vec3::new(0.26, 0.11, 0.30), Quat::IDENTITY, Vec3::new(0.0, 0.17, 0.04)), [0.10, 0.13, 0.17, 1.0]));
    // full-width light bar, front and rear
    prims.push(body_prim(gpu, part(meshgen::cbox(0.35), Vec3::new(0.27, 0.018, 0.02), Quat::IDENTITY, Vec3::new(0.0, 0.08, -0.52)), [0.85, 0.95, 1.0, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::cbox(0.35), Vec3::new(0.27, 0.016, 0.02), Quat::IDENTITY, Vec3::new(0.0, 0.09, 0.52)), [1.0, 0.25, 0.20, 1.0]));
    // sill strips, the giveaway that the floor is a battery
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(gpu, part(meshgen::cbox(0.25), Vec3::new(0.015, 0.02, 0.40), Quat::IDENTITY, Vec3::new(sx * 0.30, -0.08, 0.0)), [0.35, 0.85, 0.95, 1.0]));
    }
    ev_wheels(gpu, &mut prims, 0.125, 0.28, -0.36, 0.36, -0.07, [0.72, 0.76, 0.80, 1.0]);
    Model { prims, drive: Drive::Electric, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.56, half_h: 0.26, half_w: 0.31, wheel_r: 0.125 }
}

/// AXION — a low electric hypercar. Long, flat, fast: the flick blade.
pub fn axion(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    let body = [0.08, 0.09, 0.12, 1.0];
    prims.push(body_prim(gpu, part(meshgen::cbox(0.18), Vec3::new(0.36, 0.085, 0.74), Quat::from_rotation_x(-0.07), Vec3::new(0.0, 0.0, 0.0)), body));
    // cab canopy set back, and a spoiler on struts
    prims.push(body_prim(gpu, part(meshgen::cbox(0.34), Vec3::new(0.24, 0.075, 0.26), Quat::IDENTITY, Vec3::new(0.0, 0.11, 0.10)), [0.12, 0.16, 0.22, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::cbox(0.30), Vec3::new(0.34, 0.012, 0.07), Quat::from_rotation_x(0.16), Vec3::new(0.0, 0.19, 0.62)), [0.16, 0.16, 0.18, 1.0]));
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(gpu, part(meshgen::cbox(0.25), Vec3::new(0.012, 0.05, 0.03), Quat::IDENTITY, Vec3::new(sx * 0.24, 0.15, 0.62)), [0.16, 0.16, 0.18, 1.0]));
    }
    // a single blade of light across the nose, and a bar across the tail
    prims.push(body_prim(gpu, part(meshgen::cbox(0.40), Vec3::new(0.32, 0.012, 0.02), Quat::IDENTITY, Vec3::new(0.0, 0.03, -0.74)), [0.75, 0.90, 1.0, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::cbox(0.40), Vec3::new(0.33, 0.014, 0.02), Quat::IDENTITY, Vec3::new(0.0, 0.10, 0.68)), [1.0, 0.20, 0.30, 1.0]));
    ev_wheels(gpu, &mut prims, 0.105, 0.34, -0.50, 0.50, -0.045, [0.55, 0.58, 0.62, 1.0]);
    Model { prims, drive: Drive::Electric, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.78, half_h: 0.17, half_w: 0.38, wheel_r: 0.105 }
}

/// HAULER — an electric pickup, all straight lines and shoulders. The
/// heaviest-feeling of the three, with a flat bed the ball sits in.
pub fn hauler(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    let body = [0.72, 0.74, 0.78, 1.0];
    prims.push(body_prim(gpu, part(meshgen::cbox(0.12), Vec3::new(0.33, 0.13, 0.62), Quat::IDENTITY, Vec3::new(0.0, 0.03, 0.0)), body));
    // slab cab, set forward, with a deep windscreen rake
    prims.push(body_prim(gpu, part(meshgen::cbox(0.16), Vec3::new(0.30, 0.13, 0.26), Quat::from_rotation_x(-0.12), Vec3::new(0.0, 0.19, -0.14)), [0.16, 0.18, 0.21, 1.0]));
    // bed walls: a shelf the ball settles into
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(gpu, part(meshgen::cbox(0.25), Vec3::new(0.03, 0.07, 0.24), Quat::IDENTITY, Vec3::new(sx * 0.30, 0.14, 0.34)), body));
    }
    prims.push(body_prim(gpu, part(meshgen::cbox(0.25), Vec3::new(0.32, 0.07, 0.03), Quat::IDENTITY, Vec3::new(0.0, 0.14, 0.58)), body));
    prims.push(body_prim(gpu, part(meshgen::cbox(0.40), Vec3::new(0.32, 0.016, 0.02), Quat::IDENTITY, Vec3::new(0.0, 0.10, -0.62)), [0.90, 0.96, 1.0, 1.0]));
    ev_wheels(gpu, &mut prims, 0.145, 0.31, -0.40, 0.40, -0.06, [0.28, 0.30, 0.33, 1.0]);
    Model { prims, drive: Drive::Electric, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.66, half_h: 0.28, half_w: 0.34, wheel_r: 0.145 }
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
