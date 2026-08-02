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

#[derive(Clone, Copy)]
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
    /// hover chassis, if this is a hovercar rather than a wheeled one
    pub hover: Option<crate::physics::Hover>,
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
    /// the axle, when this prim came out of a wheel split
    pivot: Option<Vec3>,
}

/// SPLIT THE WHEELS OFF A MERGED BODY.
///
/// A model exported as one mesh has no wheels as far as the game is
/// concerned — nothing spins, nothing steers, and a car rolling on frozen
/// tyres reads as a toy no matter how good the bodywork is. Named nodes
/// solve it when they exist; these do not have them, so find the wheels
/// geometrically instead.
///
/// The method: take the vertices in the lower half of the car, split them by
/// which corner they sit in, and for each corner find the tightest
/// concentration in the side view. A wheel is the one thing in a car that is
/// round and centred on its own axle, so a cluster centre plus a radius
/// describes it well — and triangles are then assigned whole, by their
/// centroid, so a tyre never ends up half body.
/// FIND THE CAR'S OWN FRAME.
///
/// An exported model can arrive in any orientation — on its side, nose-aft,
/// Z-up — and the game cannot tell by looking at a vertex buffer. But a car
/// has a giveaway: FOUR WHEELS, the same size, low down, at the corners. So
/// try every axis-aligned orientation, score each by how well a wheel set
/// falls out of it, and keep the best. That fixes a rolled or reversed
/// import without anyone having to eyeball it and type in a rotation.
///
/// Returns the rotation that puts the car upright and nose at -Z.
pub fn find_orientation(verts: &[Vertex]) -> Quat {
    // the 24 axis-aligned rotations, as (up axis, forward axis) pairs
    const AXES: [Vec3; 6] = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];
    let mut best = (f32::MIN, Quat::IDENTITY);
    for up in AXES {
        for fwd in AXES {
            if up.dot(fwd).abs() > 0.5 {
                continue; // not perpendicular
            }
            // the rotation taking this candidate frame to (Y up, -Z forward)
            let right = fwd.cross(up).normalize();
            let m = glam::Mat3::from_cols(right, up, -fwd).transpose();
            let q = Quat::from_mat3(&m);
            let score = wheel_score(verts, q);
            if score > best.0 {
                best = (score, q);
            }
        }
    }
    best.1
}

/// How much this orientation looks like an upright car: four low clusters at
/// the corners, of similar size, wider than they are tall.
pub fn wheel_score(verts: &[Vertex], q: Quat) -> f32 {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in verts {
        let p = q * Vec3::from_array(v.pos);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let half = (hi - lo) * 0.5;
    let mid = (lo + hi) * 0.5;
    if half.y < 1e-4 || half.z < 1e-4 || half.x < 1e-4 {
        return f32::MIN;
    }
    // a car is longer than it is wide, and wider than it is tall
    let mut score = 0.0f32;
    score += (half.z / half.x).clamp(0.0, 3.0) * 2.0; // long
    score += (half.x / half.y).clamp(0.0, 4.0); // wide, not tall
    // and its mass sits low: more of the body below the middle than above
    let mut below = 0u32;
    let mut corners = [0u32; 4];
    for v in verts {
        let p = q * Vec3::from_array(v.pos) - mid;
        if p.y < 0.0 {
            below += 1;
        }
        if p.y < -half.y * 0.15 && p.x.abs() > half.x * 0.45 && p.z.abs() > half.z * 0.25 {
            let ci = (if p.x > 0.0 { 1 } else { 0 }) + (if p.z > 0.0 { 2 } else { 0 });
            corners[ci] += 1;
        }
    }
    score += (below as f32 / verts.len().max(1) as f32) * 3.0;
    // four populated corners is the strongest signal there is
    let filled = corners.iter().filter(|c| **c > 40).count();
    score += filled as f32 * 2.5;
    // ...and they should be evenly filled: a lopsided set means the guess is
    // catching bodywork on one side rather than wheels on both
    let mx = *corners.iter().max().unwrap_or(&1) as f32;
    let mn = *corners.iter().min().unwrap_or(&0) as f32;
    if mx > 0.0 {
        score += (mn / mx) * 2.0;
    }
    // NOSE OR TAIL. Everything above is symmetric front-to-back, so a car
    // and the same car reversed score identically — which is a coin flip on
    // every 180-degree yaw, and half the imports came out backwards. A car
    // is LOWER AT THE FRONT and carries its bulk behind the cabin, so
    // compare the mean height of each half and reward the frame whose -Z end
    // is the low one.
    let (mut front_h, mut front_n, mut back_h, mut back_n) = (0.0f64, 0u32, 0.0f64, 0u32);
    for v in verts {
        let p = q * Vec3::from_array(v.pos) - mid;
        if p.z < -half.z * 0.35 {
            front_h += p.y as f64;
            front_n += 1;
        } else if p.z > half.z * 0.35 {
            back_h += p.y as f64;
            back_n += 1;
        }
    }
    if front_n > 20 && back_n > 20 {
        let fa = (front_h / front_n as f64) as f32;
        let ba = (back_h / back_n as f64) as f32;
        // normalised by the car's height so it is comparable between models
        score += ((ba - fa) / half.y).clamp(-1.5, 1.5) * 2.5;
    }
    score
}

fn split_wheels(raws: &mut Vec<RawPrim>) -> bool {
    // only worth attempting on a single merged prim with no wheels tagged
    if raws.len() != 1 || raws[0].tag != Tag::Body {
        return false;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in &raws[0].verts {
        let p = Vec3::from_array(v.pos);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let mid = (lo + hi) * 0.5;
    let half = (hi - lo) * 0.5;
    if half.x < 1e-4 || half.z < 1e-4 {
        return false;
    }
    // SEARCH FOR THE THRESHOLD, do not assume one. How far out a wheel sits
    // varies between cars: a single fixed cut found four wheels on six of
    // seven models and, on the seventh, caught a piece of bodywork in one
    // corner instead (a "wheel" of radius 0.04). Try several cuts and take
    // the first that yields four plausible wheels everywhere.
    let mut corners: [Vec<Vec3>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut centres = [Vec3::ZERO; 4];
    let mut radii = [0.0f32; 4];
    let mut found_cut = false;
    for cut in [0.45f32, 0.35, 0.55, 0.28, 0.62] {
        corners = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for v in &raws[0].verts {
            let p = Vec3::from_array(v.pos) - mid;
            if p.y > half.y * 0.15 || p.x.abs() < half.x * cut || p.z.abs() < half.z * 0.25 {
                continue;
            }
            let ci = (if p.x > 0.0 { 1 } else { 0 }) + (if p.z > 0.0 { 2 } else { 0 });
            corners[ci].push(p);
        }
        if corners.iter().any(|c| c.len() < 60) {
            continue;
        }
        let mut ok = true;
        for (i, c) in corners.iter().enumerate() {
            let mut ys: Vec<f32> = c.iter().map(|p| p.y).collect();
            let mut zs: Vec<f32> = c.iter().map(|p| p.z).collect();
            let mut xs: Vec<f32> = c.iter().map(|p| p.x).collect();
            ys.sort_by(f32::total_cmp);
            zs.sort_by(f32::total_cmp);
            xs.sort_by(f32::total_cmp);
            let (cy, cz, cx) = (ys[ys.len() / 2], zs[zs.len() / 2], xs[xs.len() / 2]);
            centres[i] = Vec3::new(cx, cy, cz);
            let mut rs: Vec<f32> = c
                .iter()
                .map(|p| ((p.y - cy).powi(2) + (p.z - cz).powi(2)).sqrt())
                .collect();
            rs.sort_by(f32::total_cmp);
            radii[i] = rs[rs.len() * 85 / 100];
            if radii[i] < half.y * 0.16 || radii[i] > half.y * 1.7 {
                ok = false;
            }
        }
        // the four wheels of a car are the same size: a set that disagrees
        // wildly means one of them is not a wheel
        let rmin = radii.iter().cloned().fold(f32::MAX, f32::min);
        let rmax = radii.iter().cloned().fold(0.0f32, f32::max);
        if ok && rmax < rmin * 1.9 {
            found_cut = true;
            break;
        }
    }
    if !found_cut {
        return false;
    }
    for (i, c) in corners.iter().enumerate() {
        if c.len() < 60 {
            return false;
        }
        let _ = i;
        {
        }
    }
    // assign triangles whole, by centroid, so no tyre is half body
    let src = raws.remove(0);
    let mut out: Vec<RawPrim> = Vec::new();
    let mut body = RawPrim { verts: src.verts.clone(), idx: Vec::new(), color: src.color, tex_uri: src.tex_uri.clone(), tag: Tag::Body, pivot: None };
    let mut wheels: Vec<RawPrim> = (0..4)
        .map(|i| RawPrim {
            verts: src.verts.clone(),
            idx: Vec::new(),
            color: src.color,
            tex_uri: src.tex_uri.clone(),
            tag: if centres[i].z < 0.0 { Tag::WheelFront } else { Tag::WheelBack },
            pivot: None,
        })
        .collect();
    for t in src.idx.chunks_exact(3) {
        let c = (Vec3::from_array(src.verts[t[0] as usize].pos)
            + Vec3::from_array(src.verts[t[1] as usize].pos)
            + Vec3::from_array(src.verts[t[2] as usize].pos))
            / 3.0
            - mid;
        let mut which: Option<usize> = None;
        for i in 0..4 {
            let d = ((c.y - centres[i].y).powi(2) + (c.z - centres[i].z).powi(2)).sqrt();
            let same_side = c.x.signum() == centres[i].x.signum();
            // TIGHTER. At 1.05 the disc reached past the tyre and swept in
            // bumper and arch — Orion's bumper turned with its wheels. A
            // wheel also lives in a slab of its own along the axle, so the
            // triangle has to be near that too, not merely outboard.
            let in_slab = (c.x - centres[i].x).abs() < radii[i] * 0.75;
            if same_side && d < radii[i] * 0.92 && in_slab && c.x.abs() > half.x * 0.40 {
                which = Some(i);
                break;
            }
        }
        match which {
            Some(i) => wheels[i].idx.extend_from_slice(t),
            None => body.idx.extend_from_slice(t),
        }
    }
    // COMPACT each piece to the vertices it actually uses. Without this
    // every wheel carries a copy of the whole car's vertex buffer — five
    // uploads of 130k vertices for one model — and the pivot, which is the
    // mean of a prim's vertices, would come out as the centre of the car
    // rather than the centre of the axle.
    let compact = |pr: &mut RawPrim| {
        let mut remap: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let mut verts: Vec<Vertex> = Vec::new();
        let mut idx: Vec<u32> = Vec::with_capacity(pr.idx.len());
        for &i in &pr.idx {
            let ni = *remap.entry(i).or_insert_with(|| {
                verts.push(pr.verts[i as usize]);
                (verts.len() - 1) as u32
            });
            idx.push(ni);
        }
        pr.verts = verts;
        pr.idx = idx;
    };

    // a split that finds almost nothing is not a split
    let found: usize = wheels.iter().map(|w| w.idx.len()).sum();
    if found < src.idx.len() / 50 {
        raws[0] = src;
        return false;
    }
    compact(&mut body);
    out.push(body);
    for (i, mut w) in wheels.into_iter().enumerate() {
        if w.idx.is_empty() {
            continue;
        }
        compact(&mut w);
        w.color = src.color;
        // THE PIVOT IS THE AXLE, not the average of the prim's vertices.
        // Averaging is fine for a clean wheel and wrong for one that caught
        // a little bodywork: the centre shifts off the axle and the wheel
        // visibly wobbles as it turns. The cluster centre found above IS the
        // axle, so carry it out rather than recomputing something else.
        w.pivot = Some(centres[i] + mid);
        out.push(w);
    }
    *raws = out;
    true
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
            out.push(RawPrim { verts, idx, color, tex_uri, tag, pivot: None });
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
    load_oriented(dir, gpu, target_len, false)
}

/// As `load`, but `flip` turns the body around its vertical axis. The nose
/// is inferred from the geometry, and inference is not always right — the
/// Guardian arrived facing backwards, which is invisible until you drive it.
pub fn load_oriented(dir: &Path, gpu: &mut Gpu, target_len: f32, flip: bool) -> Model {
    // Two packagings of the same format. A .gltf folder keeps its buffer and
    // textures as sibling files; a .glb is one binary with the buffer inline
    // (and usually the images too). Take whichever is present, so a model
    // can be dropped in either way.
    let glb = dir.with_extension("glb");
    let (doc, bin) = if glb.is_file() {
        let d = gltf::Gltf::open(&glb).unwrap_or_else(|e| panic!("open {:?}: {e}", glb));
        let blob = d.blob.clone().unwrap_or_default();
        (d, blob)
    } else if dir.join("slim.glb").is_file() {
        let f = dir.join("slim.glb");
        let d = gltf::Gltf::open(&f).unwrap_or_else(|e| panic!("open {:?}: {e}", f));
        let blob = d.blob.clone().unwrap_or_default();
        (d, blob)
    } else {
        let d = gltf::Gltf::open(dir.join("scene.gltf"))
            .unwrap_or_else(|e| panic!("open {:?}: {e}", dir));
        let buffer_uri = d
            .buffers()
            .next()
            .and_then(|b| match b.source() {
                gltf::buffer::Source::Uri(u) => Some(u.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| "scene.bin".into());
        let bytes = std::fs::read(dir.join(&buffer_uri)).expect("read scene.bin");
        (d, bytes)
    };

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
    // CENTRE ON THE CAR, NOT ITS BOUNDING BOX.
    //
    // A car is left-right symmetric, so its lateral centre is where its mass
    // balances. The bounding box is not that: one wing mirror, aerial or
    // one-sided intake shifts it, and the whole body then sits off the axis.
    // (Assael measured 5.1% off, which is visible.) Y and Z keep the box —
    // there is no symmetry to appeal to along those — but X takes the
    // centroid when the two disagree meaningfully.
    let mut center = (mn + mx) * 0.5;
    {
        let mut sum = 0.0f64;
        let mut cnt = 0u64;
        for r in raws.iter() {
            for v in r.verts.iter() {
                sum += v.pos[0] as f64;
                cnt += 1;
            }
        }
        if cnt > 0 {
            let centroid_x = (sum / cnt as f64) as f32;
            let span = (mx.x - mn.x).max(1e-6);
            if (center.x - centroid_x).abs() > span * 0.02 {
                center.x = centroid_x;
            }
        }
    }
    let long = ext.z.max(ext.x).max(ext.y).max(1e-6);
    let s = target_len / long;
    let yaw = if flip { std::f32::consts::PI } else { 0.0 };
    // ---- ORIENT FIRST ----
    // Everything downstream — the wheel split, the hitbox, the nose — assumes
    // an upright car facing -Z. Several of these models do not arrive that
    // way (on their side, nose-aft, back upward), so put them in that frame
    // before anything reads them.
    {
        let all: Vec<Vertex> = raws.iter().flat_map(|r| r.verts.iter().copied()).collect();
        let q = find_orientation(&all);
        if q != Quat::IDENTITY {
            for r in raws.iter_mut() {
                for v in r.verts.iter_mut() {
                    v.pos = (q * Vec3::from_array(v.pos)).to_array();
                    v.nrm = (q * Vec3::from_array(v.nrm)).to_array();
                }
            }
        }
    }

    // A merged export has no wheels the game can see: find them.
    let split = split_wheels(&mut raws);
    if split {
        // (silent success — the count is reported by `--sim glb`)
    }
    let turn = Quat::from_rotation_y(yaw);
    for r in raws.iter_mut() {
        for v in r.verts.iter_mut() {
            let p = (Vec3::from_array(v.pos) - center) * s;
            v.pos = (turn * p).to_array();
            v.nrm = (turn * Vec3::from_array(v.nrm)).to_array();
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
            // ...unless the split told us where the AXLE is. Averaging a
            // prim's vertices is right for a clean wheel and wrong for one
            // that caught a little bodywork — the centre shifts off the axle
            // and the wheel visibly wobbles as it turns.
            if let Some(ax) = r.pivot {
                pivot = (ax - center) * s;
            }
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
    Model { prims, drive: Drive::Combustion, hover: None, cloud, cloud_n, sdf_half: Vec3::ZERO, half_len: half.z, half_h: half.y, half_w: half.x, wheel_r }
}
