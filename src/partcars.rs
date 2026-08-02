//! partcars — cars assembled from other vehicles' part layouts.
//!
//! The library gives a class and a position for every significant part of
//! 178 vehicles. Turning that into a playable car needs two things it does
//! not provide: geometry for each class, and a chassis that obeys the game's
//! rules. This module supplies both.
//!
//! The interesting consequence is that the DONOR shows through. A tank's
//! layout produces a low slab with sponsons and a heavy tail; a mech's
//! produces something upright and panelled; a spaceship's produces a long
//! spine with fins. None of that is authored here — it falls out of where
//! the donor put its parts.

use crate::meshgen;
use crate::model::{Drive, Model, Prim, Tag};
use crate::partdata;
use crate::render::Gpu;
use glam::{Quat, Vec3};

/// Geometry per part class. A class is a hint about SHAPE, not a mesh, so
/// each gets something that reads correctly at car scale: blocks are
/// chamfered volumes, struts are thin bars, discs and barrels are round.
fn part_mesh(class: u8, s: f32) -> (meshgen::MeshData, Vec3) {
    match class {
        // panel: broad and thin, lying along the body
        1 => (meshgen::cbox(0.20), Vec3::new(s * 1.15, s * 0.22, s * 1.05)),
        // strut: a bar, long in Z
        2 => (meshgen::cbox(0.30), Vec3::new(s * 0.22, s * 0.26, s * 1.25)),
        // disc: a flat cylinder standing across the body
        3 => (meshgen::wheel(0.35, 16), Vec3::splat(s * 0.85)),
        // wheel: handled by the chassis, never drawn from the layout
        4 => (meshgen::cbox(0.30), Vec3::splat(s * 0.6)),
        // barrel: a long round shaft
        5 => (meshgen::capsule(0.45, 12, 14), Vec3::new(s * 0.35, s * 1.3, s * 0.35)),
        // greeble: small detail, deliberately crisp
        6 => (meshgen::cbox(0.12), Vec3::splat(s * 0.42)),
        // tread link: a wide low block, like a track shoe
        7 => (meshgen::cbox(0.15), Vec3::new(s * 0.9, s * 0.45, s * 0.5)),
        // block: the default structural volume
        _ => (meshgen::cbox(0.22), Vec3::splat(s * 0.95)),
    }
}

/// Build a car from a donor layout.
///
/// `env` is the car's half-extents; the layout is normalised to -1..1 in
/// each axis, so this is what decides whether the result is a low racer or a
/// tall brick. Parts smaller than `floor` are dropped: the mined set keeps
/// the largest 54, and at car scale the smallest of those are specks.
#[allow(clippy::too_many_arguments)]
pub fn assemble(
    gpu: &mut Gpu,
    layout: &[(u8, f32, f32, f32, f32)],
    env: Vec3,
    palette: [[f32; 4]; 3],
    accent: [f32; 4],
    wheel_r: f32,
    drive: Drive,
    floor: f32,
) -> Model {
    let mut prims = Vec::new();
    let scale = env.length() * 0.20;
    // A donor can be MOSTLY wheel-class: the rocket's are rings down its
    // spine, not road wheels. Dropping them all left it nearly empty, so
    // only the four lowest are treated as running gear (the chassis supplies
    // those anyway) and the rest are drawn as the rings they are.
    let mut wheelish: Vec<usize> = layout
        .iter()
        .enumerate()
        .filter(|(_, p)| p.0 == 4)
        .map(|(i, _)| i)
        .collect();
    wheelish.sort_by(|a, b| layout[*a].2.total_cmp(&layout[*b].2));
    let running: Vec<usize> = wheelish.iter().take(4).copied().collect();
    for (i, (class, x, y, z, s)) in layout.iter().copied().enumerate() {
        if s < floor || running.contains(&i) {
            continue; // the chassis supplies the road wheels
        }
        let class = if class == 4 { 3 } else { class }; // spare wheels are rings
        let at = Vec3::new(x * env.x, y * env.y, z * env.z);
        let (mesh, sz) = part_mesh(class, s * scale);
        // Struts and barrels point along the body; discs stand across it.
        let rot = match class {
            3 => Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            5 => Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            _ => Quat::IDENTITY,
        };
        let col = match class {
            1 => palette[1],
            2 | 6 => palette[2],
            3 | 5 => accent,
            _ => palette[0],
        };
        let m = crate::procars::part(mesh, sz, rot, at);
        prims.push(Prim { mesh_id: gpu.add_mesh(&m), tex_id: None, color: col, tag: Tag::Body, pivot: Vec3::ZERO });
    }
    // A LEGAL CHASSIS. The donor decides the look; the game decides the
    // wheels — four of them, at the corners of the envelope, the right size
    // and able to steer. A layout that happened to have three wheels, or
    // treads, or none, still produces a car you can drive.
    for (zi, front) in [(-1.0f32, true), (1.0, false)] {
        for sx in [-1.0f32, 1.0] {
            let at = Vec3::new(sx * env.x * 0.92, -env.y * 0.55, zi * env.z * 0.62);
            let tyre = crate::procars::part(meshgen::wheel(0.58, 20), Vec3::splat(wheel_r), Quat::IDENTITY, at);
            prims.push(Prim {
                mesh_id: gpu.add_mesh(&tyre),
                tex_id: None,
                color: [0.06, 0.06, 0.07, 1.0],
                tag: if front { Tag::WheelFront } else { Tag::WheelBack },
                pivot: at,
            });
            let hub = crate::procars::part(meshgen::rim(6, 0.24, 18), Vec3::splat(wheel_r * 0.98), Quat::IDENTITY, at);
            prims.push(Prim {
                mesh_id: gpu.add_mesh(&hub),
                tex_id: None,
                color: accent,
                tag: if front { Tag::WheelFront } else { Tag::WheelBack },
                pivot: at,
            });
        }
    }
    Model {
        prims,
        drive,
        hover: None,
        cloud: Vec::new(),
        cloud_n: Vec::new(),
        sdf_half: Vec3::ZERO,
        half_len: env.z,
        half_h: env.y,
        half_w: env.x,
        wheel_r,
    }
}

/// ROLLOUT — from a sci-fi tank. Struts and sponsons over a low slab, with
/// the mass carried at the back where a turret ring would sit.
pub fn rollout(gpu: &mut Gpu) -> Model {
    assemble(
        gpu,
        partdata::ROLLOUT,
        Vec3::new(0.36, 0.20, 0.70),
        [[0.30, 0.33, 0.28, 1.0], [0.22, 0.24, 0.21, 1.0], [0.15, 0.16, 0.14, 1.0]],
        [0.85, 0.62, 0.15, 1.0],
        0.135,
        Drive::Combustion,
        0.10,
    )
}

/// CRYOFIN — from a slick racer. Almost all of its donor's mass is wheels
/// and fins, so what survives the filter is a spine and a shell: the
/// lightest, lowest thing in the collection.
pub fn cryofin(gpu: &mut Gpu) -> Model {
    assemble(
        gpu,
        partdata::CRYOFIN,
        Vec3::new(0.34, 0.15, 0.76),
        [[0.10, 0.45, 0.62, 1.0], [0.14, 0.60, 0.78, 1.0], [0.06, 0.20, 0.30, 1.0]],
        [0.55, 0.95, 1.00, 1.0],
        0.108,
        Drive::Electric,
        0.08,
    )
}

/// TL-4 — from a walking mech. Eighteen panels: it comes out plated and
/// upright, which at car scale reads as an armoured wagon.
pub fn tl4(gpu: &mut Gpu) -> Model {
    assemble(
        gpu,
        partdata::TL4,
        Vec3::new(0.35, 0.26, 0.62),
        [[0.38, 0.36, 0.40, 1.0], [0.28, 0.27, 0.31, 1.0], [0.50, 0.48, 0.52, 1.0]],
        [0.90, 0.25, 0.20, 1.0],
        0.145,
        Drive::Combustion,
        0.12,
    )
}

/// SPEARHEAD — from a rocket. A long spine with rings down it and almost
/// nothing at the sides, so it drives like a dart with wheels bolted on.
pub fn spearhead(gpu: &mut Gpu) -> Model {
    assemble(
        gpu,
        partdata::SPEARHEAD,
        Vec3::new(0.28, 0.17, 0.82),
        [[0.80, 0.80, 0.84, 1.0], [0.65, 0.66, 0.70, 1.0], [0.40, 0.42, 0.46, 1.0]],
        [1.00, 0.45, 0.10, 1.0],
        0.112,
        Drive::Electric,
        0.09,
    )
}

/// OUTBACK — from a sci-fi jeep. The most car-like donor, and it shows: a
/// boxy cabin, discs at the corners and greebles all over it.
pub fn outback(gpu: &mut Gpu) -> Model {
    assemble(
        gpu,
        partdata::OUTBACK,
        Vec3::new(0.37, 0.24, 0.66),
        [[0.52, 0.44, 0.26, 1.0], [0.42, 0.35, 0.20, 1.0], [0.30, 0.26, 0.16, 1.0]],
        [0.95, 0.85, 0.55, 1.0],
        0.150,
        Drive::Combustion,
        0.10,
    )
}
