//! Sketchfab glTF loader. Reads scene.gltf + scene.bin directly (no gltf image
//! machinery, so the unused normal/roughness maps we didn't ship are never
//! touched), bakes the full node hierarchy transform into vertices, tags wheel
//! sub-trees by the "- FR / - FL / - BR / - BL" naming convention these rips
//! use, and normalizes the model: nose to -Z (front/back wheel centroids tell
//! us which end is which), centered, uniformly scaled to a target length.

use std::collections::HashMap;
use std::path::Path;

use glam::{Mat3, Mat4, Quat, Vec3};

use crate::meshgen::{MeshData, Vertex};
use crate::render::Gpu;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tag {
    Body,
    WheelFront,
    WheelBack,
}

pub struct Prim {
    pub mesh_id: usize,
    pub tex_id: Option<usize>,
    pub color: [f32; 4],
    pub tag: Tag,
    pub pivot: Vec3, // wheel spin pivot in normalized model space
}

/// How a car puts its power down. Combustion cars build speed; an electric
/// one has its torque from a standstill and trades a little top end for it —
/// which in this game means EVs win the first two car-lengths off a kickoff
/// and lose a long straight.
#[derive(Clone, Copy, PartialEq)]
pub enum Drive {
    Combustion,
    Electric,
}

impl Drive {
    /// multiplier on acceleration, strongest from a stop
    pub fn torque(self, speed_frac: f32) -> f32 {
        match self {
            Drive::Combustion => 1.0,
            // flat torque curve: 1.35x off the line, fading to parity
            Drive::Electric => 1.0 + 0.35 * (1.0 - speed_frac.clamp(0.0, 1.0)).powi(2),
        }
    }
    /// multiplier on top speed
    pub fn top(self) -> f32 {
        match self {
            Drive::Combustion => 1.0,
            Drive::Electric => 0.94,
        }
    }
}

pub struct Model {
    pub prims: Vec<Prim>,
    /// drivetrain character (electric cars pull harder off the line)
    pub drive: Drive,
    /// downsampled surface points (final meters) for the reflection SDF bake
    pub cloud: Vec<Vec3>,
    /// outward normal for each cloud point — this is what makes the baked
    /// field SIGNED (solid inside) instead of a hollow shell
    pub cloud_n: Vec<Vec3>,
    /// symmetric half-extents of the baked SDF box (set by the bake)
    pub sdf_half: Vec3,
    pub half_len: f32,
    pub half_h: f32,
    #[allow(dead_code)]
    pub half_w: f32,
    pub wheel_r: f32,
}

struct RawPrim {
    verts: Vec<Vertex>,
    idx: Vec<u32>,
    color: [f32; 4],
    tex_uri: Option<String>,
    tag: Tag,
}

fn detect_tag(name: Option<&str>) -> Option<Tag> {
    let n = name?;
    if n.contains("- FR") || n.contains("- FL") || n.contains("-FR") || n.contains("-FL") {
        Some(Tag::WheelFront)
    } else if n.contains("- BR") || n.contains("- BL") || n.contains("-BR") || n.contains("-BL") {
        Some(Tag::WheelBack)
    } else {
        None
    }
}

fn collect_node(
    node: &gltf::Node,
    parent: Mat4,
    inherited: Tag,
    bin: &[u8],
    out: &mut Vec<RawPrim>,
) {
    let local = Mat4::from_cols_array_2d(&node.transform().matrix());
    let world = parent * local;
    let tag = detect_tag(node.name()).unwrap_or(inherited);

    if let Some(mesh) = node.mesh() {
        let nmat = Mat3::from_mat4(world).inverse().transpose();
        for prim in mesh.primitives() {
            let reader = prim.reader(|_| Some(bin));
            let Some(pos_iter) = reader.read_positions() else { continue };
            let positions: Vec<[f32; 3]> = pos_iter.collect();
            let normals: Option<Vec<[f32; 3]>> = reader.read_normals().map(|it| it.collect());
            let uvs: Option<Vec<[f32; 2]>> =
                reader.read_tex_coords(0).map(|t| t.into_f32().collect());
            let idx: Vec<u32> = match reader.read_indices() {
                Some(ind) => ind.into_u32().collect(),
                None => (0..positions.len() as u32).collect(),
            };

            let mut verts: Vec<Vertex> = Vec::with_capacity(positions.len());
            for (i, p) in positions.iter().enumerate() {
                let wp = world.transform_point3(Vec3::from_array(*p));
                let n = match &normals {
                    Some(ns) => (nmat * Vec3::from_array(ns[i])).normalize_or_zero(),
                    None => Vec3::Y,
                };
                let uv = uvs.as_ref().map(|u| u[i]).unwrap_or([0.0, 0.0]);
                verts.push(Vertex { pos: wp.to_array(), nrm: n.to_array(), uv });
            }
            if normals.is_none() {
                // accumulate face normals
                let mut acc = vec![Vec3::ZERO; verts.len()];
                for t in idx.chunks_exact(3) {
                    let a = Vec3::from_array(verts[t[0] as usize].pos);
                    let b = Vec3::from_array(verts[t[1] as usize].pos);
                    let c = Vec3::from_array(verts[t[2] as usize].pos);
                    let n = (b - a).cross(c - a);
                    for &vi in t {
                        acc[vi as usize] += n;
                    }
                }
                for (v, n) in verts.iter_mut().zip(acc) {
                    v.nrm = n.normalize_or(Vec3::Y).to_array();
                }
            }

            let mat = prim.material();
            let pbr = mat.pbr_metallic_roughness();
            let color = pbr.base_color_factor();
            let tex_uri = pbr.base_color_texture().and_then(|info| {
                match info.texture().source().source() {
                    gltf::image::Source::Uri { uri, .. } => Some(uri.to_string()),
                    _ => None,
                }
            });
            out.push(RawPrim { verts, idx, color, tex_uri, tag });
        }
    }
    for child in node.children() {
        collect_node(&child, world, tag, bin, out);
    }
}

/// Load a model folder (scene.gltf + scene.bin + textures/), normalized so its
/// longest run (nose axis if wheels exist) spans `target_len` along Z with the
/// nose at -Z, centered at the origin.
pub fn load(dir: &Path, gpu: &mut Gpu, target_len: f32) -> Model {
    let doc = gltf::Gltf::open(dir.join("scene.gltf"))
        .unwrap_or_else(|e| panic!("open {:?}: {e}", dir));
    let buffer_uri = doc
        .buffers()
        .next()
        .and_then(|b| match b.source() {
            gltf::buffer::Source::Uri(u) => Some(u.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "scene.bin".into());
    let bin = std::fs::read(dir.join(&buffer_uri)).expect("read scene.bin");

    let mut raws: Vec<RawPrim> = Vec::new();
    let scene = doc.default_scene().or_else(|| doc.scenes().next()).expect("no scene");
    for node in scene.nodes() {
        collect_node(&node, Mat4::IDENTITY, Tag::Body, &bin, &mut raws);
    }
    assert!(!raws.is_empty(), "model {:?} had no primitives", dir);

    // ---- orientation from wheels (front centroid - back centroid) ----
    let centroid = |tag: Tag| -> Option<Vec3> {
        let mut sum = Vec3::ZERO;
        let mut n = 0usize;
        for r in raws.iter().filter(|r| r.tag == tag) {
            for v in &r.verts {
                sum += Vec3::from_array(v.pos);
                n += 1;
            }
        }
        (n > 0).then(|| sum / n as f32)
    };
    let fwd = match (centroid(Tag::WheelFront), centroid(Tag::WheelBack)) {
        (Some(f), Some(b)) => {
            let mut d = f - b;
            d.y = 0.0;
            d.normalize_or(Vec3::NEG_Z)
        }
        _ => {
            // no wheels (ball / display piece): point the longest horizontal axis at -Z
            let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for r in &raws {
                for v in &r.verts {
                    let p = Vec3::from_array(v.pos);
                    mn = mn.min(p);
                    mx = mx.max(p);
                }
            }
            let e = mx - mn;
            if e.x >= e.z { Vec3::X } else { Vec3::NEG_Z }
        }
    };
    // rotate about Y so `fwd` lands on -Z
    let cur = fwd.x.atan2(fwd.z);
    let rot = Quat::from_rotation_y(std::f32::consts::PI - cur);

    for r in raws.iter_mut() {
        for v in r.verts.iter_mut() {
            v.pos = (rot * Vec3::from_array(v.pos)).to_array();
            v.nrm = (rot * Vec3::from_array(v.nrm)).to_array();
        }
    }

    // ---- center + scale ----
    let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for r in &raws {
        for v in &r.verts {
            let p = Vec3::from_array(v.pos);
            mn = mn.min(p);
            mx = mx.max(p);
        }
    }
    let ext = mx - mn;
    let center = (mn + mx) * 0.5;
    let long = ext.z.max(ext.x).max(ext.y).max(1e-6);
    let s = target_len / long;
    for r in raws.iter_mut() {
        for v in r.verts.iter_mut() {
            v.pos = ((Vec3::from_array(v.pos) - center) * s).to_array();
        }
    }

    // ---- upload: dedupe textures per model, build prims ----
    let mut tex_cache: HashMap<String, usize> = HashMap::new();
    let mut prims = Vec::new();
    let mut cloud_all: Vec<Vec3> = Vec::new();
    let mut cloud_nrm: Vec<Vec3> = Vec::new();
    let mut wheel_bottom = f32::MAX;
    let mut wheel_top = f32::MIN;
    for r in raws {
        let tex_id = r.tex_uri.as_ref().map(|uri| {
            *tex_cache.entry(uri.clone()).or_insert_with(|| {
                let bytes = std::fs::read(dir.join(uri))
                    .unwrap_or_else(|e| panic!("texture {uri}: {e}"));
                let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                    .expect("decode png")
                    .to_rgba8();
                gpu.add_texture(img.width(), img.height(), &img)
            })
        });
        let mut pivot = Vec3::ZERO;
        if r.tag != Tag::Body {
            for v in &r.verts {
                let p = Vec3::from_array(v.pos);
                pivot += p;
                wheel_bottom = wheel_bottom.min(p.y);
                wheel_top = wheel_top.max(p.y);
            }
            pivot /= r.verts.len().max(1) as f32;
        }
        for v in &r.verts {
            cloud_all.push(Vec3::from_array(v.pos));
            cloud_nrm.push(Vec3::from_array(v.nrm).normalize_or(Vec3::Y));
        }
        let mesh_id = gpu.add_mesh(&MeshData { verts: r.verts, idx: r.idx });
        prims.push(Prim { mesh_id, tex_id, color: r.color, tag: r.tag, pivot });
    }

    let half = ext * s * 0.5;
    let wheel_r = if wheel_bottom < wheel_top {
        (wheel_top - wheel_bottom) * 0.5
    } else {
        half.y * 0.55
    };
    // 520 points over a whole car left 5-8 cm between samples, and the bake
    // inflated that into a puffy blob — the "bubble" in the mirror. Denser
    // sampling plus a smaller inflation gives back the silhouette.
    let stride = (cloud_all.len() / 2600).max(1);
    let cloud: Vec<Vec3> = cloud_all.into_iter().step_by(stride).collect();
    let cloud_n: Vec<Vec3> = cloud_nrm.into_iter().step_by(stride).collect();
    Model { prims, drive: Drive::Combustion, cloud, cloud_n, sdf_half: Vec3::ZERO, half_len: half.z, half_h: half.y, half_w: half.x, wheel_r }
}
