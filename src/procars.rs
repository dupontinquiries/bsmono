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

pub fn part(base: MeshData, scale: Vec3, rot: Quat, off: Vec3) -> MeshData {
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
    Model { prims, drive: Drive::Electric, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.56, half_h: 0.26, half_w: 0.31, wheel_r: 0.125 }
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
    Model { prims, drive: Drive::Electric, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.78, half_h: 0.17, half_w: 0.38, wheel_r: 0.105 }
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
    Model { prims, drive: Drive::Electric, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.66, half_h: 0.28, half_w: 0.34, wheel_r: 0.145 }
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
    Model { prims, drive: Drive::Electric, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.52, half_h: 0.30, half_w: 0.33, wheel_r: 0.155 }
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
    Model { prims, drive: Drive::Electric, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.62, half_h: 0.34, half_w: 0.32, wheel_r: 0.125 }
}

// ======================= ELECTRIFICATION =================================
// Every car in the game can be had as an EV. Rather than model fourteen more
// vehicles, `electrify` takes a finished model and converts it: it keeps the
// body exactly as it is and adds the small set of cues that actually say
// "electric" — a full-width light bar at each end, sill strips along the
// battery floor, a blanked grille where the intake was, and an ION THRUSTER
// ring at the tail in place of an exhaust. The drivetrain flips with it, so
// an EV variant also pulls harder off the line.

/// Turn any model into its electric variant.
pub fn electrify(gpu: &mut Gpu, base: &Model, accent: [f32; 4]) -> Model {
    let mut prims = base.prims.clone();
    let (hl, hh) = (base.half_len, base.half_h);

    // WHERE THE BODY ACTUALLY IS.
    //
    // Placing the cues at the bounding box put them on the widest thing the
    // model contains — which on most cars is the WHEELS, not the flank. The
    // sills then floated in the air beside the doors, and any asymmetry in
    // the model made one side look worse than the other.
    //
    // Measure the real flank instead: take the surface points along the
    // middle of the car, at door height, and use the 90th percentile of
    // their lateral distance. That is the door skin, wheels excluded.
    let flank = {
        let mut xs: Vec<f32> = base
            .cloud
            .iter()
            .filter(|p| p.z.abs() < hl * 0.45 && p.y > -hh * 0.35 && p.y < hh * 0.8)
            .map(|p| p.x.abs())
            .collect();
        if xs.len() < 32 {
            base.half_w * 0.88
        } else {
            xs.sort_by(f32::total_cmp);
            xs[xs.len() * 9 / 10]
        }
    };
    let hw = flank;

    // light bars: full width, front white and rear red
    prims.push(body_prim(
        gpu,
        part(meshgen::cbox(0.40), Vec3::new(hw * 0.80, 0.012, 0.018), Quat::IDENTITY, Vec3::new(0.0, hh * 0.15, -hl * 0.98)),
        [0.86, 0.94, 1.0, 1.0],
    ));
    prims.push(body_prim(
        gpu,
        part(meshgen::cbox(0.40), Vec3::new(hw * 0.82, 0.012, 0.018), Quat::IDENTITY, Vec3::new(0.0, hh * 0.25, hl * 0.96)),
        [1.0, 0.24, 0.20, 1.0],
    ));
    // blanked grille: an EV has no radiator to feed
    prims.push(body_prim(
        gpu,
        part(meshgen::cbox(0.30), Vec3::new(hw * 0.42, hh * 0.22, 0.016), Quat::IDENTITY, Vec3::new(0.0, -hh * 0.12, -hl * 0.97)),
        [0.10, 0.11, 0.13, 1.0],
    ));
    // sill strips: the giveaway that the floor is a battery
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(
            gpu,
            part(meshgen::cbox(0.28), Vec3::new(0.010, 0.013, hl * 0.42), Quat::IDENTITY, Vec3::new(sx * (hw + 0.006), -hh * 0.30, 0.0)),
            accent,
        ));
    }
    // ION THRUSTER: a glowing ring with a dark throat, where the pipes were
    let ring = part(meshgen::torus(0.34, 20, 10), Vec3::splat(hh * 0.42), Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Vec3::new(0.0, hh * 0.02, hl * 0.99));
    prims.push(body_prim(gpu, ring, accent));
    prims.push(body_prim(
        gpu,
        part(meshgen::sphere(10, 14), Vec3::new(hh * 0.30, hh * 0.30, 0.03), Quat::IDENTITY, Vec3::new(0.0, hh * 0.02, hl * 1.0)),
        [0.05, 0.06, 0.09, 1.0],
    ));
    Model {
        prims,
        drive: Drive::Electric,
        hover: None,
        cloud: base.cloud.clone(),
        cloud_n: base.cloud_n.clone(),
        sdf_half: base.sdf_half,
        half_len: hl,
        half_h: hh,
        half_w: hw,
        wheel_r: base.wheel_r,
    }
}

// ===================== CONCEPT CARS =====================================
// Two vehicles built from a written specification rather than by eye, and
// built in the order that specification insists on: WHEEL HARDPOINTS FIRST,
// then the chassis between them, then the body volumes around it. That
// ordering is the whole point — it is what stops a car ending up beautiful
// with its wheels somewhere impossible.
//
// The two use opposite design grammars, and the code is meant to show it:
//
//   VIPERA  continuous aerodynamic shell, soft edges, teardrop silhouette.
//           Built from high-segment lofts so the surface curves.
//   AEGIS   low wedge of interlocking armour plates, sharp edges, arrowhead
//           silhouette. Built from SEVEN-segment lofts, so the same
//           generator produces flat facets instead of curves.

/// A hardpoint: where a wheel goes, decided before any bodywork exists.
struct Hardpoint {
    at: Vec3,
    r: f32,
    front: bool,
}

fn hardpoints(wheelbase: f32, track: f32, r: f32, ride: f32) -> Vec<Hardpoint> {
    let mut v = Vec::new();
    for (z, front) in [(-wheelbase * 0.5, true), (wheelbase * 0.5, false)] {
        for sx in [-1.0f32, 1.0] {
            v.push(Hardpoint { at: Vec3::new(sx * track * 0.5, ride, z), r, front });
        }
    }
    v
}

fn mount_wheels(gpu: &mut Gpu, prims: &mut Vec<Prim>, hp: &[Hardpoint], rim: [f32; 4], spokes: u32) {
    for h in hp {
        let tyre = part(meshgen::wheel(0.58, 22), Vec3::splat(h.r), Quat::IDENTITY, h.at);
        prims.push(Prim {
            mesh_id: gpu.add_mesh(&tyre),
            tex_id: None,
            color: [0.06, 0.06, 0.07, 1.0],
            tag: if h.front { Tag::WheelFront } else { Tag::WheelBack },
            pivot: h.at,
        });
        // deep-dish rim: a disc plus radial spokes, so it reads as machined
        let dish = part(meshgen::sphere(8, 14), Vec3::new(h.r * 0.26, h.r * 0.56, h.r * 0.56), Quat::IDENTITY, h.at);
        prims.push(Prim { mesh_id: gpu.add_mesh(&dish), tex_id: None, color: rim, tag: if h.front { Tag::WheelFront } else { Tag::WheelBack }, pivot: h.at });
        for k in 0..spokes {
            let a = k as f32 / spokes as f32 * std::f32::consts::TAU;
            let m = part(
                meshgen::cbox(0.30),
                Vec3::new(h.r * 0.10, h.r * 0.40, h.r * 0.055),
                Quat::from_rotation_x(a),
                h.at + Vec3::new(h.at.x.signum() * h.r * 0.10, 0.0, 0.0),
            );
            prims.push(Prim { mesh_id: gpu.add_mesh(&m), tex_id: None, color: rim, tag: if h.front { Tag::WheelFront } else { Tag::WheelBack }, pivot: h.at });
        }
    }
}

/// VIPERA — the green streamlined cyber-supercar.
///
/// One continuous shell: the cockpit is a dark canopy sunk INTO the body
/// rather than a cabin sitting on top of it, the front fenders flare out of
/// a narrow nose, and the rear haunches carry visibly more volume than the
/// front. Proportions from the spec (4.6 : 2.15 : 1.0), scaled to the
/// game's car space.
pub fn vipera(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    let paint = [0.06, 0.34, 0.16, 1.0];
    let trim = [0.78, 0.80, 0.76, 1.0];
    let (half_len, half_w, wheel_r) = (0.77f32, 0.36f32, 0.130f32);

    // 1-4: hardpoints and wheels first, then everything else answers to them
    let hp = hardpoints(1.05, 0.60, wheel_r, -0.055);

    // 5: chassis slab between the hardpoints (battery floor, mostly hidden)
    prims.push(body_prim(
        gpu,
        part(meshgen::cbox(0.16), Vec3::new(0.26, 0.045, 0.58), Quat::IDENTITY, Vec3::new(0.0, -0.045, 0.0)),
        [0.07, 0.08, 0.09, 1.0],
    ));

    // 7: the primary silhouette — a teardrop that starts narrow and low at
    // the nose, swells through the front fenders, and carries its mass into
    // the rear haunches
    let shell = [
        (-half_len, 0.13, -0.035, 0.010, 0.95),
        (-0.56, 0.24, -0.055, 0.045, 0.85),
        (-0.30, 0.34, -0.065, 0.085, 0.70),
        (0.02, 0.36, -0.065, 0.105, 0.65),
        (0.34, 0.35, -0.060, 0.095, 0.70),
        (0.60, 0.30, -0.050, 0.060, 0.80),
        (half_len, 0.22, -0.035, 0.020, 0.95),
    ];
    prims.push(body_prim(gpu, part(meshgen::loft(&shell, 26), Vec3::ONE, Quat::IDENTITY, Vec3::ZERO), paint));

    // 8: the canopy, sunk into the shell — a single dark volume, steeply
    // raked at the windscreen, blending imperceptibly into the rear deck
    let canopy = [
        (-0.34, 0.14, 0.055, 0.075, 0.95),
        (-0.16, 0.23, 0.050, 0.150, 0.90),
        (0.10, 0.24, 0.045, 0.160, 0.90),
        (0.36, 0.17, 0.040, 0.110, 0.95),
    ];
    prims.push(body_prim(gpu, part(meshgen::loft(&canopy, 22), Vec3::ONE, Quat::IDENTITY, Vec3::ZERO), [0.05, 0.07, 0.08, 1.0]));

    // 9: fenders — bulbous volumes that WRAP the wheels and flare wider than
    // the nose. Front soft and round, rear heavier and squarer.
    for h in hp.iter() {
        let (w, hgt, len, round) = if h.front { (0.10, 0.115, 0.20, 0.80) } else { (0.115, 0.135, 0.23, 0.65) };
        let arch = [
            (h.at.z - len, w * 0.7, -0.05, hgt * 0.55, round),
            (h.at.z, w, -0.06, hgt, round),
            (h.at.z + len, w * 0.7, -0.05, hgt * 0.55, round),
        ];
        let m = part(meshgen::loft(&arch, 18), Vec3::ONE, Quat::IDENTITY, Vec3::new(h.at.x * 1.02, 0.0, 0.0));
        prims.push(body_prim(gpu, m, paint));
    }

    // 12: the kidney side cavity, bordered by a bright structural trim that
    // is real extruded geometry rather than paint
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(
            gpu,
            part(meshgen::loft(&[(-0.20, 0.030, -0.030, 0.055, 0.9), (0.02, 0.045, -0.045, 0.075, 0.9), (0.24, 0.030, -0.030, 0.050, 0.9)], 16), Vec3::ONE, Quat::IDENTITY, Vec3::new(sx * 0.345, 0.0, 0.0)),
            [0.03, 0.03, 0.04, 1.0],
        ));
        prims.push(body_prim(
            gpu,
            part(meshgen::cbox(0.32), Vec3::new(0.010, 0.012, 0.24), Quat::from_rotation_x(0.06), Vec3::new(sx * 0.352, -0.052, 0.02)),
            trim,
        ));
    }

    // 14: lights as recessed geometry — the amber blade across the nose
    for k in 0..5 {
        let x = -0.16 + k as f32 * 0.08;
        prims.push(body_prim(gpu, part(meshgen::cbox(0.35), Vec3::new(0.030, 0.008, 0.012), Quat::IDENTITY, Vec3::new(x, -0.010, -half_len + 0.03)), [1.0, 0.80, 0.15, 1.0]));
    }
    prims.push(body_prim(gpu, part(meshgen::cbox(0.35), Vec3::new(0.30, 0.010, 0.015), Quat::IDENTITY, Vec3::new(0.0, 0.030, half_len - 0.02)), [1.0, 0.22, 0.16, 1.0]));

    // 16: a thin blade spoiler off the rear shoulders, and the boost aperture
    prims.push(body_prim(gpu, part(meshgen::cbox(0.28), Vec3::new(0.33, 0.010, 0.055), Quat::from_rotation_x(0.14), Vec3::new(0.0, 0.135, 0.66)), [0.10, 0.11, 0.12, 1.0]));
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(gpu, part(meshgen::cbox(0.25), Vec3::new(0.012, 0.038, 0.028), Quat::IDENTITY, Vec3::new(sx * 0.26, 0.100, 0.655)), [0.10, 0.11, 0.12, 1.0]));
    }
    prims.push(body_prim(gpu, part(meshgen::sphere(10, 14), Vec3::new(0.055, 0.045, 0.030), Quat::IDENTITY, Vec3::new(0.0, 0.005, half_len - 0.01)), [0.10, 0.09, 0.08, 1.0]));

    // 15: underbody — a dark floor with a diffuser, so it reads from below
    prims.push(body_prim(gpu, part(meshgen::cbox(0.14), Vec3::new(0.28, 0.012, 0.20), Quat::from_rotation_x(-0.12), Vec3::new(0.0, -0.075, 0.50)), [0.05, 0.05, 0.06, 1.0]));

    mount_wheels(gpu, &mut prims, &hp, [0.16, 0.17, 0.19, 1.0], 6);
    Model { prims, drive: Drive::Electric, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len, half_h: 0.175, half_w, wheel_r }
}

/// AEGIS — the silver armoured wedge.
///
/// The same loft generator, but at SEVEN segments a section stops being a
/// curve and becomes a set of flat facets — so the identical machinery
/// yields planar armour instead of aerodynamic skin. A central ridge runs
/// from the front tip to the windscreen and is the car's visual axis; the
/// flanks are three layered plates at different angles; the headlights are
/// triangular recesses in an inverted V.
pub fn aegis(gpu: &mut Gpu) -> Model {
    let mut prims = Vec::new();
    let steel = [0.42, 0.44, 0.47, 1.0];
    let dark = [0.16, 0.17, 0.19, 1.0];
    let glow = [0.95, 0.97, 1.0, 1.0];
    let copper = [0.85, 0.45, 0.18, 1.0];
    let (half_len, half_w, wheel_r) = (0.80f32, 0.37f32, 0.137f32);

    let hp = hardpoints(1.08, 0.62, wheel_r, -0.045);

    prims.push(body_prim(
        gpu,
        part(meshgen::cbox(0.10), Vec3::new(0.26, 0.040, 0.60), Quat::IDENTITY, Vec3::new(0.0, -0.040, 0.0)),
        [0.09, 0.09, 0.10, 1.0],
    ));

    // the wedge: a sharp point at the nose, a raised centre ridge, and a
    // body that is wider at the bottom than the top
    // FULLER. At half the height of the others this read as a plate with
    // wheels rather than a car — the wedge silhouette is the point, but a
    // wedge still needs volume behind it. Roughly 60% more section depth,
    // carried mostly in the middle where a cabin and a floor would be.
    let wedge = [
        (-half_len, 0.08, -0.030, 0.012, 0.05),
        (-0.58, 0.22, -0.058, 0.055, 0.05),
        (-0.28, 0.34, -0.075, 0.115, 0.05),
        (0.06, 0.37, -0.082, 0.140, 0.05),
        (0.40, 0.36, -0.075, 0.125, 0.05),
        (0.66, 0.30, -0.058, 0.080, 0.05),
        (half_len, 0.18, -0.035, 0.030, 0.05),
    ];
    prims.push(body_prim(gpu, part(meshgen::loft(&wedge, 7), Vec3::ONE, Quat::IDENTITY, Vec3::ZERO), steel));

    // the centre ridge — the primary visual axis, nose tip to windscreen
    prims.push(body_prim(
        gpu,
        part(meshgen::loft(&[(-half_len + 0.02, 0.022, 0.008, 0.024, 0.05), (-0.30, 0.058, 0.070, 0.128, 0.05), (-0.02, 0.048, 0.110, 0.152, 0.05)], 5), Vec3::ONE, Quat::IDENTITY, Vec3::ZERO),
        steel,
    ));

    // cockpit: a flattened black trapezoidal prism, not a bubble
    prims.push(body_prim(
        gpu,
        part(meshgen::loft(&[(-0.20, 0.17, 0.105, 0.140, 0.10), (0.04, 0.23, 0.100, 0.190, 0.10), (0.34, 0.17, 0.095, 0.155, 0.10)], 7), Vec3::ONE, Quat::IDENTITY, Vec3::ZERO),
        [0.04, 0.05, 0.06, 1.0],
    ));

    // triangular headlight recesses, meeting in an inverted V
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(gpu, part(meshgen::loft(&[(-half_len + 0.04, 0.075, -0.010, 0.030, 0.05), (-half_len + 0.16, 0.030, 0.000, 0.018, 0.05)], 4), Vec3::ONE, Quat::from_rotation_y(sx * 0.22), Vec3::new(sx * 0.16, 0.0, 0.0)), dark));
        prims.push(body_prim(gpu, part(meshgen::cbox(0.30), Vec3::new(0.075, 0.008, 0.010), Quat::from_rotation_y(sx * 0.30) * Quat::from_rotation_z(sx * 0.22), Vec3::new(sx * 0.17, 0.012, -half_len + 0.075)), glow));
    }

    // three layered side plates, each at its own angle
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(gpu, part(meshgen::cbox(0.06), Vec3::new(0.014, 0.030, 0.30), Quat::from_rotation_z(sx * 0.20), Vec3::new(sx * 0.335, 0.055, 0.05)), steel));
        prims.push(body_prim(gpu, part(meshgen::cbox(0.06), Vec3::new(0.016, 0.045, 0.34), Quat::from_rotation_z(-sx * 0.14), Vec3::new(sx * 0.352, 0.000, 0.02)), [0.34, 0.36, 0.39, 1.0]));
        prims.push(body_prim(gpu, part(meshgen::cbox(0.06), Vec3::new(0.012, 0.020, 0.32), Quat::from_rotation_z(sx * 0.30), Vec3::new(sx * 0.340, -0.055, 0.02)), [0.10, 0.10, 0.11, 1.0]));
    }

    // angular fenders: armour plates around a wheel, not a curved arch
    for h in hp.iter() {
        let (w, hgt, len) = if h.front { (0.095, 0.105, 0.19) } else { (0.125, 0.140, 0.24) };
        let arch = [
            (h.at.z - len, w * 0.6, -0.045, hgt * 0.5, 0.05),
            (h.at.z - len * 0.3, w, -0.055, hgt, 0.05),
            (h.at.z + len * 0.3, w, -0.055, hgt, 0.05),
            (h.at.z + len, w * 0.6, -0.045, hgt * 0.5, 0.05),
        ];
        prims.push(body_prim(gpu, part(meshgen::loft(&arch, 6), Vec3::ONE, Quat::IDENTITY, Vec3::new(h.at.x * 1.03, 0.0, 0.0)), if h.front { steel } else { [0.36, 0.38, 0.41, 1.0] }));
    }

    // rear quarter structures: big triangular volumes emerging from the body
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(gpu, part(meshgen::loft(&[(0.34, 0.10, -0.020, 0.090, 0.05), (0.62, 0.055, -0.010, 0.045, 0.05)], 5), Vec3::ONE, Quat::from_rotation_y(sx * 0.18), Vec3::new(sx * 0.28, 0.0, 0.0)), steel));
    }
    // central diffuser and twin fins
    prims.push(body_prim(gpu, part(meshgen::cbox(0.08), Vec3::new(0.20, 0.030, 0.055), Quat::from_rotation_x(-0.18), Vec3::new(0.0, -0.035, 0.66)), [0.08, 0.08, 0.09, 1.0]));
    for sx in [-1.0f32, 1.0] {
        prims.push(body_prim(gpu, part(meshgen::cbox(0.12), Vec3::new(0.010, 0.045, 0.075), Quat::from_rotation_z(sx * 0.12), Vec3::new(sx * 0.13, 0.055, 0.62)), dark));
    }
    prims.push(body_prim(gpu, part(meshgen::cbox(0.30), Vec3::new(0.24, 0.010, 0.014), Quat::IDENTITY, Vec3::new(0.0, 0.035, half_len - 0.02)), copper));

    mount_wheels(gpu, &mut prims, &hp, copper, 6);
    Model { prims, drive: Drive::Combustion, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len, half_h: 0.205, half_w, wheel_r }
}

/// The house recipe, in one place.
///
/// Every car in the catalogue is now built the same way the references are:
/// a CONTINUOUS lofted shell with a shoulder crease, a darker glasshouse set
/// INTO it rather than perched on top, and a dark tub underneath. Octane and
/// Dominus are four body meshes each — chassis, paint, body, window — and
/// that is the structure, not a coincidence: the shell reads as one surface
/// because it IS one, and the hard lines come from where the parts meet.
pub fn shell_car(
    gpu: &mut Gpu,
    prims: &mut Vec<Prim>,
    sections: &[(f32, f32, f32, f32, f32)],
    paint: [f32; 4],
    cabin: (f32, f32, f32, f32),
    ring: u32,
) {
    prims.push(body_prim(gpu, part(meshgen::loft(sections, ring), Vec3::ONE, Quat::IDENTITY, Vec3::ZERO), paint));
    // the glasshouse: its own loft, sunk into the shell so the join is a
    // line rather than a lip
    let (z0, z1, w, y) = cabin;
    let glass = [
        (z0, w * 0.70, y - 0.02, y + 0.02, 0.92),
        (z0 + (z1 - z0) * 0.38, w, y - 0.03, y + 0.075, 0.86),
        (z1, w * 0.78, y - 0.03, y + 0.045, 0.92),
    ];
    prims.push(body_prim(gpu, part(meshgen::loft(&glass, ring.max(16)), Vec3::ONE, Quat::IDENTITY, Vec3::ZERO), [0.06, 0.08, 0.11, 1.0]));
    // and the tub: a dark floor that grounds the shell the way a chassis does
    let nose = sections[0];
    let tail = sections[sections.len() - 1];
    let mid = sections[sections.len() / 2];
    let tub = [
        (nose.0 * 0.86, mid.1 * 0.62, mid.2 - 0.012, mid.2 + 0.030, 0.55),
        (0.0, mid.1 * 0.80, mid.2 - 0.016, mid.2 + 0.040, 0.45),
        (tail.0 * 0.86, mid.1 * 0.66, mid.2 - 0.012, mid.2 + 0.030, 0.55),
    ];
    prims.push(body_prim(gpu, part(meshgen::loft(&tub, 14), Vec3::ONE, Quat::IDENTITY, Vec3::ZERO), [0.09, 0.09, 0.11, 1.0]));
}

pub fn breadbox(gpu: &mut Gpu) -> Model {
    // tall, square-shouldered and stubby: the van of the catalogue. Boxy is
    // a PROFILE choice (low roundness), not a reason to build it from boxes.
    let mut prims = Vec::new();
    shell_car(
        gpu,
        &mut prims,
        &[
            (-0.50, 0.24, -0.075, 0.145, 0.35),
            (-0.28, 0.33, -0.105, 0.205, 0.22),
            (0.02, 0.34, -0.115, 0.215, 0.18),
            (0.30, 0.33, -0.105, 0.200, 0.22),
            (0.50, 0.26, -0.080, 0.150, 0.35),
        ],
        [0.86, 0.74, 0.42, 1.0],
        (-0.30, 0.16, 0.30, 0.185),
        20,
    );
    wheels(gpu, &mut prims, 0.115, 0.30, -0.32, 0.32, -0.09);
    Model { prims, drive: Drive::Combustion, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.56, half_h: 0.25, half_w: 0.36, wheel_r: 0.115 }
}


pub fn wedge(gpu: &mut Gpu) -> Model {
    // a doorstop: the nose is almost flat to the floor and the whole body is
    // one rising plane to the tail
    let mut prims = Vec::new();
    shell_car(
        gpu,
        &mut prims,
        &[
            (-0.80, 0.16, -0.055, -0.020, 0.55),
            (-0.44, 0.33, -0.075, 0.010, 0.40),
            (-0.02, 0.40, -0.085, 0.055, 0.30),
            (0.42, 0.39, -0.080, 0.085, 0.30),
            (0.80, 0.28, -0.060, 0.060, 0.55),
        ],
        [0.13, 0.13, 0.15, 1.0],
        (-0.10, 0.44, 0.30, 0.070),
        22,
    );
    // the stripe, as a raised spine rather than paint
    prims.push(body_prim(gpu, part(meshgen::cbox(0.30), Vec3::new(0.045, 0.012, 0.62), Quat::from_rotation_x(-0.06), Vec3::new(0.0, 0.055, -0.05)), [0.85, 0.16, 0.16, 1.0]));
    wheels(gpu, &mut prims, 0.095, 0.36, -0.52, 0.52, -0.06);
    Model { prims, drive: Drive::Combustion, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.80, half_h: 0.125, half_w: 0.42, wheel_r: 0.095 }
}


pub fn roller(gpu: &mut Gpu) -> Model {
    // round in every section: a pebble on wheels, and the roundest thing
    // the profile can produce
    let mut prims = Vec::new();
    shell_car(
        gpu,
        &mut prims,
        &[
            (-0.42, 0.14, -0.070, 0.040, 1.0),
            (-0.22, 0.28, -0.130, 0.130, 1.0),
            (0.02, 0.32, -0.145, 0.160, 1.0),
            (0.24, 0.28, -0.130, 0.130, 1.0),
            (0.42, 0.15, -0.075, 0.045, 1.0),
        ],
        [0.92, 0.44, 0.22, 1.0],
        (-0.18, 0.20, 0.24, 0.130),
        24,
    );
    wheels(gpu, &mut prims, 0.13, 0.29, -0.28, 0.28, -0.08);
    Model { prims, drive: Drive::Combustion, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.48, half_h: 0.24, half_w: 0.34, wheel_r: 0.13 }
}


pub fn longboy(gpu: &mut Gpu) -> Model {
    // absurdly long and low: a limousine hitbox, and the profile barely
    // changes down its whole length
    let mut prims = Vec::new();
    shell_car(
        gpu,
        &mut prims,
        &[
            (-1.00, 0.13, -0.045, 0.010, 0.70),
            (-0.55, 0.25, -0.065, 0.065, 0.50),
            (0.00, 0.27, -0.070, 0.080, 0.45),
            (0.55, 0.26, -0.068, 0.072, 0.50),
            (1.00, 0.16, -0.050, 0.020, 0.70),
        ],
        [0.24, 0.30, 0.52, 1.0],
        (-0.34, 0.40, 0.22, 0.068),
        20,
    );
    wheels(gpu, &mut prims, 0.09, 0.27, -0.72, 0.72, -0.05);
    Model { prims, drive: Drive::Combustion, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 1.00, half_h: 0.115, half_w: 0.30, wheel_r: 0.09 }
}


pub fn cube(gpu: &mut Gpu) -> Model {
    // the cube: equal in every direction, wrong in every situation. touches
    // are chaos, dribbles are a dare
    let mut prims = Vec::new();
    // WHEEL ARCHES. A solid box with wheels bolted to its faces has them
    // half-buried in the bodywork, which looks like a mistake rather than a
    // joke. Build the lower body from four posts instead, leaving a genuine
    // opening at each corner for the wheel to sit in.
    let body = [0.90, 0.35, 0.30, 1.0];
    // upper block: everything above the arches
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.30, 0.17, 0.30), Quat::IDENTITY, Vec3::new(0.0, 0.18, 0.0)), body));
    // four corner posts and two sills, framing an arch on each side
    for sx in [-1.0f32, 1.0] {
        for sz in [-1.0f32, 1.0] {
            prims.push(body_prim(
                gpu,
                part(meshgen::boxx(), Vec3::new(0.075, 0.13, 0.075), Quat::IDENTITY, Vec3::new(sx * 0.225, -0.09, sz * 0.225)),
                body,
            ));
        }
        // the sill between the two arches on this side
        prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.055, 0.055, 0.30), Quat::IDENTITY, Vec3::new(sx * 0.245, -0.16, 0.0)), body));
    }
    // and a spine down the middle so it still reads as one solid object
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.17, 0.13, 0.30), Quat::IDENTITY, Vec3::new(0.0, -0.09, 0.0)), body));
    prims.push(body_prim(gpu, part(meshgen::boxx(), Vec3::new(0.31, 0.06, 0.31), Quat::IDENTITY, Vec3::new(0.0, 0.36, 0.0)), [0.98, 0.92, 0.85, 1.0]));
    wheels(gpu, &mut prims, 0.11, 0.30, -0.225, 0.225, -0.14);
    Model { prims, drive: Drive::Combustion, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.34, half_h: 0.34, half_w: 0.34, wheel_r: 0.11 }
}

pub fn shark(gpu: &mut Gpu) -> Model {
    // a streamliner: round in section, tapered at both ends, with a dorsal
    // fin that is the whole silhouette
    let mut prims = Vec::new();
    shell_car(
        gpu,
        &mut prims,
        &[
            (-0.66, 0.11, -0.055, 0.020, 0.95),
            (-0.34, 0.26, -0.105, 0.090, 0.90),
            (0.02, 0.30, -0.120, 0.130, 0.88),
            (0.36, 0.27, -0.105, 0.105, 0.90),
            (0.66, 0.14, -0.060, 0.040, 0.95),
        ],
        [0.35, 0.48, 0.62, 1.0],
        (-0.24, 0.24, 0.22, 0.115),
        24,
    );
    prims.push(body_prim(gpu, part(meshgen::cbox(0.34), Vec3::new(0.022, 0.13, 0.15), Quat::from_rotation_x(0.28), Vec3::new(0.0, 0.22, 0.14)), [0.30, 0.42, 0.55, 1.0]));
    prims.push(body_prim(gpu, part(meshgen::cbox(0.40), Vec3::new(0.24, 0.014, 0.02), Quat::IDENTITY, Vec3::new(0.0, -0.02, -0.63)), [0.92, 0.92, 0.95, 1.0]));
    wheels(gpu, &mut prims, 0.10, 0.28, -0.44, 0.44, -0.09);
    Model { prims, drive: Drive::Combustion, hover: None, cloud: Vec::new(), cloud_n: Vec::new(), sdf_half: Vec3::ZERO, half_len: 0.66, half_h: 0.20, half_w: 0.31, wheel_r: 0.10 }
}

