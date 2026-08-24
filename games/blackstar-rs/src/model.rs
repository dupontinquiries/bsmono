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
    /// per-prim pull-in for ROBOT MODE: how far each prim moves toward
    /// the body to stop a wingspan from swallowing the bot
    /// (`skeleton::compact`). Zero for a body with nothing sticking out,
    /// and applied only when folded to robot — a vehicle's wings are the
    /// right size for a vehicle.
    /// UPRIGHTING FOR THIS POSE, on top of the chassis frame.
    ///
    /// The chassis frame has to be shared by every pose or the fold
    /// tumbles — and the fold is visibly coherent, so that rule is right.
    /// But a coherent fold can still END sideways: an artist may author the
    /// robot standing along a different axis from the one the vehicle
    /// drives along, and the interpolation carries it there smoothly and
    /// correctly. One frame cannot fix that without breaking the fold.
    /// This is the leftover rotation that stands THIS pose up, applied
    /// ramped by fold progress so the animation stays continuous and only
    /// the destination moves.
    pub settle: Quat,
    pub tuck: Vec<Vec3>,
    /// per-prim pull-in for VEHICLE MODE: what it takes to get each limb
    /// inside the hull envelope, so a folded body reads as a solid object
    /// instead of a skeleton in a box (`skeleton::contain`). Zero for
    /// anything already tidy.
    pub pack: Vec<Vec3>,
    /// what this hull carries: a turret to aim and treads to roll
    /// (`turret::find`). Indices are into `prims`, so the draw path can
    /// give the turret its own rotation without a second model.
    pub fittings: crate::turret::Fittings,
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
    /// Distance from the body origin to its LOWEST point, static-pose
    /// equivalent of `xform::Morph::bounds_at`'s second return value.
    ///
    /// `da831f98` ("seat bots on their feet, not their bounding-box
    /// centre") fixed this for the RUNTIME smooth-fold path — `bounds_at`
    /// samples every prim at the current blend `t` and returns `-lo.y`
    /// alongside the half-extents, every frame, while a fold is actively
    /// playing. It never touched this struct, which is the STATIC
    /// per-pose load path: `apply_car_dims` (main.rs) reads `half_h` off
    /// whichever `Model` is currently displayed, and that read is the
    /// ONLY seating input for (a) any chassis whose morph strip was
    /// refused — the stepping fallback swaps `Model`s and never calls
    /// `bounds_at` at all, so it never had the fix — and (b) the instant
    /// a smooth chassis is first loaded or switched to, before
    /// `advance_xform`'s morph branch has run even once. `car.ride_h`
    /// defaults to 0.0 and the seating code falls back to `half_h`
    /// whenever it is `<= 0.0`, so both of those cases were silently
    /// using the pre-fix, bounding-box-centred number the whole time —
    /// the same sink `da831f98` measured, just in the other loading path.
    ///
    /// `lo`/`hi` are already computed in `load_raw_at` for exactly this
    /// purpose (`half = (hi - lo) * 0.5`); this is the other half of the
    /// same bounding box, at zero extra sampling cost. Same sign
    /// convention as `bounds_at`: negative means the pose sits entirely
    /// above the origin (a hovering body), which the seating code's own
    /// `> 0.0` check already treats as "leave it, don't plant it."
    pub ride_h: f32,
}

pub struct RawPrim {
    pub verts: Vec<Vertex>,
    pub idx: Vec<u32>,
    pub color: [f32; 4],
    pub tex_uri: Option<String>,
    pub tag: Tag,
    /// the axle, when this prim came out of a wheel split
    pub pivot: Option<Vec3>,
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
            let (mut cy, mut cz, cx) = (ys[ys.len() / 2], zs[zs.len() / 2], xs[xs.len() / 2]);
            // FIT THE CIRCLE, do not just take the middle. A median is pulled
            // off the axle by whatever bodywork the cluster caught — an arch
            // above the tyre drags the centre up, and the wheel then turns
            // about a point that is not its own, which is the wobble. A tyre
            // is a circle in the side view, so fit one: two rounds of
            // least-squares, keeping only the points that agree with the
            // first, which drops the attached bodywork before the second.
            for round in 0..2 {
                let (mut sx, mut sy, mut sxx, mut syy, mut sxy, mut sr2x, mut sr2y, mut n) =
                    (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
                // on the second round, keep points near the first estimate
                let mut rs: Vec<f32> = c
                    .iter()
                    .map(|p| ((p.y - cy).powi(2) + (p.z - cz).powi(2)).sqrt())
                    .collect();
                rs.sort_by(f32::total_cmp);
                let rmed = rs[rs.len() / 2];
                for p in c.iter() {
                    let (u, v) = ((p.y - cy) as f64, (p.z - cz) as f64);
                    if round == 1 {
                        let d = ((u * u + v * v).sqrt() as f32 - rmed).abs();
                        if d > rmed * 0.45 {
                            continue; // not on the tyre: an arch, a fender, a strut
                        }
                    }
                    let r2 = u * u + v * v;
                    sx += u;
                    sy += v;
                    sxx += u * u;
                    syy += v * v;
                    sxy += u * v;
                    sr2x += r2 * u;
                    sr2y += r2 * v;
                    n += 1.0;
                }
                if n < 24.0 {
                    break;
                }
                // solve the 2x2 normal equations for the circle centre
                let (a11, a12, a22) = (sxx - sx * sx / n, sxy - sx * sy / n, syy - sy * sy / n);
                let (b1, b2) = (0.5 * (sr2x - (sxx + syy) * sx / n), 0.5 * (sr2y - (sxx + syy) * sy / n));
                let det = a11 * a22 - a12 * a12;
                if det.abs() < 1e-9 {
                    break;
                }
                let du = (b1 * a22 - b2 * a12) / det;
                let dv = (a11 * b2 - a12 * b1) / det;
                if du.is_finite() && dv.is_finite() && du.abs() < 1e3 && dv.abs() < 1e3 {
                    cy += du as f32;
                    cz += dv as f32;
                }
            }
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
            // ON THE TYRE, not merely near it. A wheel is an annulus in the
            // side view — a triangle well inside the hub or well outside the
            // tread belongs to something else, and taking it made brackets
            // and arches turn with the wheel.
            let in_slab = (c.x - centres[i].x).abs() < radii[i] * 0.70;
            let on_wheel = d < radii[i] * 0.92 && d > radii[i] * 0.10;
            if same_side && on_wheel && in_slab && c.x.abs() > half.x * 0.40 {
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

/// The orientation sidecar that speaks for the whole strip.
///
/// Prefers a pose whose sidecar says `foldview`, because that one was written
/// by the exporter that knows the answer rather than guessed from geometry.
/// Falls back to the FIRST pose — f00, the vehicle — where "long, low, wheels
/// down" is a fair heuristic, and never to a mid-fold pose where it is not.
///
/// Returns None when `dir` is not inside a chassis directory, so a standalone
/// model still reads its own sidecar.
fn strip_orientation_text(pose_dir: &Path) -> Option<String> {
    let chassis = pose_dir.parent()?;
    let mut poses: Vec<std::path::PathBuf> = std::fs::read_dir(chassis)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("orient.json").is_file())
        .collect();
    if poses.len() < 2 {
        return None; // not a strip; the caller reads the pose's own sidecar
    }
    poses.sort();
    let mut first: Option<String> = None;
    for p in &poses {
        let txt = std::fs::read_to_string(p.join("orient.json")).ok()?;
        if txt.contains("\"foldview\"") {
            return Some(txt);
        }
        if first.is_none() {
            first = Some(txt);
        }
    }
    first
}

/// Read `orient.json` beside the geometry, if the asset ships one.
///
/// Returns the rotation that carries the asset's declared frame onto the
/// engine's: +Y up, nose at -Z. Malformed or degenerate files are ignored
/// rather than fatal — a bad sidecar should cost the guess, not the load.
/// The GUESSED orientation for a whole strip, decided once.
///
/// `declared_orientation` already enforces "orientation is a chassis
/// property, not a pose property" for chassis whose sidecars say so. The
/// GUESS never got the same rule: `find_orientation` ran per pose, so a
/// five-pose strip got up to five different frames and the body tumbled as
/// it folded. That is what "the bot is sideways in robot mode but fine as a
/// vehicle" is — not a robot-pose bug, a DISAGREEMENT between poses.
///
/// Measured on `truck_barrage_aka_m270_multiple_launch_rocket_system`:
/// f00 guesses up=+Y, f01/f03/f04 guess up=-Z, f02 guesses up=-Y. One body,
/// three answers, and the fold walks between them.
///
/// The answer is taken from the VEHICLE pose, for the reason the search's
/// own comment gives: `wheel_score` is a car heuristic, and a mid-fold or
/// standing robot gives it nothing to find, so it "picks whatever reads
/// most car-like, which is the robot lying on its side". The first pose is
/// the vehicle by the strip's own naming (f00), which is the same pose
/// `strip_orientation_text` falls back to.
///
/// Memoised per chassis: without it, loading a five-pose strip would read
/// the vehicle pose five extra times.
fn strip_guess(pose_dir: &Path) -> Option<Quat> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MEMO: OnceLock<Mutex<HashMap<std::path::PathBuf, Option<Quat>>>> = OnceLock::new();

    let chassis = pose_dir.parent()?.to_path_buf();
    let memo = MEMO.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = memo.lock().ok().and_then(|m| m.get(&chassis).copied()) {
        return hit;
    }
    let mut poses: Vec<std::path::PathBuf> = std::fs::read_dir(&chassis)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && (p.join("scene.gltf").is_file() || p.with_extension("glb").is_file()))
        .collect();
    if poses.len() < 2 {
        // not a strip: the caller guesses from the pose in hand, as before
        if let Ok(mut m) = memo.lock() {
            m.insert(chassis, None);
        }
        return None;
    }
    poses.sort();
    let ans = (|| {
        let raws = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            load_raw_unoriented(&poses[0])
        }))
        .ok()?;
        // the same debris rejection the caller does, for the same reason:
        // a flung panel drags the frame search with it
        let pts: Vec<Vec<Vec3>> = raws
            .iter()
            .map(|r| r.verts.iter().map(|v| Vec3::from_array(v.pos)).collect())
            .collect();
        let refs: Vec<&[Vec3]> = pts.iter().map(|v| v.as_slice()).collect();
        let mass = crate::massfilter::classify(&refs);
        let all: Vec<Vertex> = raws
            .iter()
            .enumerate()
            .filter(|(i, _)| mass.keep.get(*i).copied().unwrap_or(true))
            .flat_map(|(_, r)| r.verts.iter().copied())
            .collect();
        if all.is_empty() {
            return None;
        }
        Some(nose_corrected(find_orientation(&all), &all))
    })();
    if let Ok(mut m) = memo.lock() {
        m.insert(chassis, ans);
    }
    ans
}

/// A SECOND, INDEPENDENT VOTE ON WHICH END IS THE FRONT.
///
/// `wheel_score`'s fore/aft term is one heuristic — "a car is lower at the
/// front" — and it is a coin flip on anything that is not shaped like a
/// car. An M270 carries a tall cab and a low stowed launcher, which is the
/// heuristic exactly backwards, and the truck arrives with its cab facing
/// the camera.
///
/// `cockpit::signed_forward` answers the same question from a different
/// measurement: where the crew sits. On a truck that is the cab; on a jet
/// the canopy. When it is confident and it disagrees, it wins — and when it
/// is not, nothing changes and the old heuristic stands.
fn nose_corrected(q: Quat, verts: &[Vertex]) -> Quat {
    let cloud: Vec<Vec3> = verts.iter().map(|v| q * Vec3::from_array(v.pos)).collect();
    // in the canonical frame the body is Y-up and faces -Z
    let g = crate::cockpit::signed_forward(&cloud, Vec3::Y, Vec3::NEG_Z);
    if g.confident && g.forward.z > 0.0 {
        // the cab is at +Z: the frame is backwards, yaw it round
        Quat::from_rotation_y(std::f32::consts::PI) * q
    } else {
        q
    }
}

/// The frame the LOADER will apply to this chassis, and where it came
/// from. Exposed for the `--orient check` driver, which used to print only
/// `orient::guess`'s per-pose second opinion — a different function from
/// the one the game runs, so a disagreeing printout proved nothing either
/// way.
pub fn loader_frame(pose_dir: &Path) -> Option<(Quat, &'static str)> {
    let heuristic = sidecar_is_heuristic(pose_dir);
    if let Some(q) = declared_orientation(pose_dir) {
        if !heuristic {
            return Some((q, "declared sidecar"));
        }
    }
    strip_guess(pose_dir).map(|q| {
        (
            q,
            if heuristic {
                "guessed: the sidecar was only a heuristic"
            } else {
                "guessed from the vehicle pose"
            },
        )
    })
}

/// Was this chassis' sidecar written by an exporter that knew, or by an
/// older run of the same heuristic? See the call site for why it matters.
fn sidecar_is_heuristic(pose_dir: &Path) -> bool {
    let txt = match strip_orientation_text(pose_dir) {
        Some(t) => t,
        None => match std::fs::read_to_string(pose_dir.join("orient.json")) {
            Ok(t) => t,
            Err(_) => return false,
        },
    };
    // an authored answer names its source; anything calling itself a
    // heuristic is a guess we can now make better
    txt.contains("heuristic") && !txt.contains("foldview") && !txt.contains("rigpose")
}

fn declared_orientation(dir: &Path) -> Option<Quat> {
    // ORIENTATION IS A CHASSIS PROPERTY, NOT A POSE PROPERTY.
    //
    // A fold strip is one body in several positions, so every pose must be
    // rotated the SAME way — rotate them differently and the strip tumbles as
    // it folds, which is what "the bots were turned sideways" looked like.
    //
    // 48 of 221 chassis disagree with themselves: analog declares five
    // different orientations across its own poses, blackstar six. They are the
    // ones whose sidecars say `orient_heuristic_v1` rather than `foldview` —
    // the heuristic ran per pose and a mid-fold robot gives it nothing to work
    // with ("a legless robot has no wheels to find, so the search picks
    // whatever reads most car-like, which is the robot lying on its side").
    //
    // So the strip's answer is taken from ONE pose and applied to all of them.
    let txt = match strip_orientation_text(dir) {
        Some(t) => t,
        None => std::fs::read_to_string(dir.join("orient.json")).ok()?,
    };
    let num3 = |key: &str| -> Option<Vec3> {
        let i = txt.find(key)?;
        let open = txt[i..].find('[')? + i;
        let close = txt[open..].find(']')? + open;
        let v: Vec<f32> = txt[open + 1..close]
            .split(',')
            .filter_map(|t| t.trim().parse::<f32>().ok())
            .collect();
        (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
    };
    let up = num3("\"up\"")?.normalize_or_zero();
    let fwd = num3("\"forward\"")?.normalize_or_zero();
    if up.length_squared() < 0.5 || fwd.length_squared() < 0.5 {
        return None;
    }
    // orthonormalize: the forward the asset declares need not be exactly
    // perpendicular to its up, and a skew basis would shear the model
    let f = (fwd - up * fwd.dot(up)).normalize_or_zero();
    if f.length_squared() < 0.5 {
        return None; // forward parallel to up: no frame to build
    }
    let r = f.cross(up).normalize_or_zero();
    // rows are the asset's axes, so the matrix maps asset space -> engine
    // space, where up becomes +Y and forward becomes -Z
    let m = Mat3::from_cols(r, up, -f).transpose();
    Some(Quat::from_mat3(&m).normalize())
}

/// Everything `load` does before touching the GPU: parse (.gltf folder or
/// .glb), bake node transforms, find the car's frame, split the wheels,
/// orient, centre and scale. Harnesses and tools can accept an asset exactly
/// the way the engine does, with no device.
///
/// Returns the normalized prims and the model's half-extents in metres.
pub fn load_raw(dir: &Path, target_len: f32) -> (Vec<RawPrim>, Vec3) {
    let (raws, half, _, _, _) = load_raw_at(dir, target_len, false);
    (raws, half)
}

/// The gltf-read step alone: raw source vertices, no orientation, no
/// centering, no scale. Everything downstream in `load_raw_at` runs the
/// (potentially wrong) orientation guess before returning anything, which
/// makes it useless for diagnosing THAT guess. This is the same read,
/// stopped before that step — the ground truth `--orient` checks against.
pub fn load_raw_unoriented(dir: &Path) -> Vec<RawPrim> {
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
        let d = gltf::Gltf::open(dir.join("scene.gltf")).unwrap_or_else(|e| panic!("open {:?}: {e}", dir));
        let buffer_uri = d
            .buffers()
            .next()
            .and_then(|b| match b.source() {
                gltf::buffer::Source::Uri(u) => Some(u.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| "scene.bin".into());
        // scene.bin, or the quantised pack that replaced it.
        //
        // tools/pack_bots.py folds a whole fold strip into one bots.pack in the
        // CHASSIS directory — one quantisation box across every pose, because a
        // per-pose box would make the same world position decode differently in
        // different poses and warp the fold. Decoding here rather than at a
        // higher level means everything downstream sees the float32 buffer it
        // always saw.
        let bytes = match std::fs::read(dir.join(&buffer_uri)) {
            Ok(b) => b,
            Err(_) => crate::botpack::pose_buffer(dir)
                .unwrap_or_else(|| panic!("no scene.bin and no bots.pack for {dir:?}")),
        };
        (d, bytes)
    };
    let mut raws: Vec<RawPrim> = Vec::new();
    let scene = doc.default_scene().or_else(|| doc.scenes().next()).expect("no scene");
    for node in scene.nodes() {
        collect_node(&node, Mat4::IDENTITY, Tag::Body, &bin, &mut raws);
    }
    raws
}

/// `load_raw` plus the two values the upload path still needs to place wheel
/// pivots: the pre-scale centre and the scale factor.
pub fn load_raw_at(dir: &Path, target_len: f32, flip: bool) -> (Vec<RawPrim>, Vec3, Vec3, f32, f32) {
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
        // Panic with the path actually OPENED, not the directory. Printing
        // `dir` made a doubled join — a caller passing `.../w00/scene.gltf`
        // where a directory was expected — report as
        // `open ".../w00/scene.gltf": Not a directory`, which reads like a
        // missing asset and sent the reader looking for a file that was there
        // all along.
        let f = dir.join("scene.gltf");
        let d = gltf::Gltf::open(&f).unwrap_or_else(|e| {
            if !dir.is_dir() {
                panic!("model dir {dir:?} is not a directory (caller passed a file?): {e}");
            }
            panic!("open {f:?}: {e}")
        });
        let buffer_uri = d
            .buffers()
            .next()
            .and_then(|b| match b.source() {
                gltf::buffer::Source::Uri(u) => Some(u.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| "scene.bin".into());
        // scene.bin, or the quantised pack that replaced it.
        //
        // tools/pack_bots.py folds a whole fold strip into one bots.pack in the
        // CHASSIS directory — one quantisation box across every pose, because a
        // per-pose box would make the same world position decode differently in
        // different poses and warp the fold. Decoding here rather than at a
        // higher level means everything downstream sees the float32 buffer it
        // always saw.
        let bytes = match std::fs::read(dir.join(&buffer_uri)) {
            Ok(b) => b,
            Err(_) => crate::botpack::pose_buffer(dir)
                .unwrap_or_else(|| panic!("no scene.bin and no bots.pack for {dir:?}")),
        };
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
        // LET THE ASSET SAY WHICH WAY IS UP.
        //
        // `find_orientation` scores candidate frames by how well four
        // same-size wheels fall out at the corners. That is a good guess for
        // a car and a bad one for anything else: a legless robot has no
        // wheels to find, so the search picks whatever reads most car-like,
        // which is the robot lying on its side — long and low scores well.
        //
        // An `orient.json` beside the geometry ends the guessing. It is two
        // vectors in the asset's own space, and the exporter that knows the
        // answer writes it: `rigpose` reads up straight off the skeleton
        // (feet to head) and right off the hips, so a rigged model never
        // has to be guessed about at all.
        //
        //     { "up": [0,1,0], "forward": [0,0,-1] }
        //
        // Absent, nothing changes and the search runs as before.
        // DEBRIS FIRST. `find_orientation` scores candidate frames over
        // every vertex, so a mis-parented panel flung off the body drags
        // the up/forward guess with it and the pose arrives lying down or
        // facing backwards — one of the three symptoms that share this
        // cause (see `massfilter`). Rejecting before the search means the
        // frame is chosen from the body, and rejecting HERE rather than
        // after means the same prims are gone for the bounds below too.
        {
            let pts: Vec<Vec<Vec3>> = raws
                .iter()
                .map(|r| r.verts.iter().map(|v| Vec3::from_array(v.pos)).collect())
                .collect();
            let refs: Vec<&[Vec3]> = pts.iter().map(|v| v.as_slice()).collect();
            let mass = crate::massfilter::classify(&refs);
            if mass.dropped > 0 {
                let mut keep = mass.keep.iter();
                raws.retain(|_| keep.next().copied().unwrap_or(true));
            }
        }
        // ONE FRAME PER CHASSIS, whether it was declared or guessed. The
        // declared path has enforced that since the sidecars landed; the
        // guessed path did not, and a strip that guesses per pose tumbles
        // as it folds (see `strip_guess`).
        // A SIDECAR IS ONLY WORTH TRUSTING IF SOMETHING KNEW THE ANSWER.
        //
        // `orient.json` comes in two kinds. `foldview` and `rigpose` were
        // written by exporters that could see the skeleton — those are
        // facts. `orient_heuristic_v1` was written by THIS SAME GUESS run
        // offline, per pose, before the strip rule existed: 48 of 221
        // chassis disagree with themselves and every one of them is that
        // kind. Reading a stale copy of a guess we have since improved is
        // strictly worse than guessing again, so heuristic sidecars are
        // skipped and the strip guess (which also gets the debris filter
        // and the cockpit nose vote) is used instead.
        let q = match declared_orientation(dir).filter(|_| !sidecar_is_heuristic(dir)) {
            Some(q) => q,
            None => match strip_guess(dir) {
                Some(q) => q,
                None => {
                    let all: Vec<Vertex> =
                        raws.iter().flat_map(|r| r.verts.iter().copied()).collect();
                    nose_corrected(find_orientation(&all), &all)
                }
            },
        };
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

    // MEASURE THE BODY WE ACTUALLY BUILT, not the one we were handed.
    //
    // `ext` is the bounding box of the SOURCE vertices, taken before the
    // orientation search rotates them. For a car that arrives upright and
    // nose-aft the two agree, which is why this went unnoticed. For anything
    // the search actually turns they do not, and every dimension downstream
    // is then wrong about which way is which: half_h, half_w and half_len
    // feed the hitbox, the SDF box and the lift that keeps a model out of
    // the floor.
    //
    // Caught on a rigged robot pose, where the loader reported
    // [0.66 2.10 1.27] for geometry that measures [0.66 1.27 2.10] — a body
    // lying down being described as standing up. The blackstar bot poses had
    // width and height swapped the same way.
    // MEASURE THE MASS, not every stray vertex. The debris pass above
    // already dropped what it was sure about; this second call catches
    // what survived it (a prim that is far but not slight enough to
    // delete still should not define the hitbox) WITHOUT removing
    // geometry — bounds and rendering are allowed to disagree, and this
    // is the one place where they should.
    let pts: Vec<Vec<Vec3>> = raws
        .iter()
        .map(|r| r.verts.iter().map(|v| Vec3::from_array(v.pos)).collect())
        .collect();
    let refs: Vec<&[Vec3]> = pts.iter().map(|v| v.as_slice()).collect();
    let mass = crate::massfilter::classify(&refs);
    let (half, ride_h) = match crate::massfilter::bounds(&refs, &mass.keep) {
        Some(b) => b,
        None => {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for r in raws.iter() {
                for v in r.verts.iter() {
                    let p = Vec3::from_array(v.pos);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
            bounds_from_extent(lo, hi, ext * s * 0.5)
        }
    };
    (raws, half, center, s, ride_h)
}

/// Half-extents and the drop from the origin to the lowest point, from a
/// measured bounding box — the static-load half of `da831f98`'s fix,
/// factored out so it is checkable without a real chassis directory.
/// Every real one on disk is packed now (`scene.bin` is gone, and
/// `load_raw_at` needs it), so a filesystem-backed test can't reach this
/// computation any more; the formula itself still can.
///
/// `lo > hi` on any axis means no vertices were actually measured (an
/// empty or degenerate raw set) — `ext_half` is the caller's fallback
/// half-extent for that case, and the drop falls back to `ext_half.y`,
/// the symmetric assumption, rather than a nonsensical number computed
/// from an inverted box.
///
/// Same convention as `xform::Morph::bounds_at`: the drop (`-lo.y`)
/// equals the half-extent only when the box is centred on the origin,
/// which a vehicle pose is and a standing robot pose is not.
fn bounds_from_extent(lo: Vec3, hi: Vec3, ext_half: Vec3) -> (Vec3, f32) {
    if hi.x >= lo.x {
        ((hi - lo) * 0.5, -lo.y)
    } else {
        (ext_half, ext_half.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE BUG THE MASS FILTER FIXES, as a unit: one panel flung below a
    /// body used to define `ride_h`, so physics seated the chassis on
    /// debris and the visible body hung in the air (or, when the stray
    /// went sideways, the hitbox doubled and the body caught on nothing).
    /// `bounds_from_extent` still exists and still measures a box; what
    /// changed is WHICH points reach it.
    #[test]
    fn a_flung_panel_no_longer_sets_the_ride_height() {
        let body: Vec<Vec3> = (0..300)
            .map(|i| {
                let t = i as f32 * 0.7;
                Vec3::new(t.sin() * 0.8, (t * 1.3).cos() * 1.2, (t * 0.6).sin() * 2.0)
            })
            .collect();
        let junk: Vec<Vec3> = (0..6).map(|i| Vec3::new(0.1 * i as f32, -18.0, 0.0)).collect();
        let refs: Vec<&[Vec3]> = vec![&body, &junk];
        let m = crate::massfilter::classify(&refs);
        let (_, ride) = crate::massfilter::bounds(&refs, &m.keep).expect("a body was measured");
        assert!(ride < 2.0, "debris still owns the ride height: {ride}");
        // and the naive measurement is what it used to be, so the fixture
        // is proving a difference rather than an absence
        let (_, naive) = crate::massfilter::bounds(&refs, &[true, true]).unwrap();
        assert!(naive > 17.0, "the fixture does not reproduce the old behaviour");
    }

    #[test]
    fn the_drop_matches_half_only_when_the_box_is_centred() {
        // da831f98's own measurement on `analog`, reused here as the
        // ground truth: f00 (vehicle) is centred, f04 (robot) is not.
        //   f00 vehicle   y -0.239..0.239   half 0.239, feet at 0.239
        //   f04 robot     y -0.400..0.355   half 0.377, feet at 0.400
        let (half_veh, ride_veh) = bounds_from_extent(
            Vec3::new(-1.0, -0.239, -1.0), Vec3::new(1.0, 0.239, 1.0), Vec3::ONE);
        assert!((half_veh.y - 0.239).abs() < 1e-6);
        assert!((ride_veh - 0.239).abs() < 1e-6, "a centred box should have drop == half");

        let (half_bot, ride_bot) = bounds_from_extent(
            Vec3::new(-1.0, -0.400, -1.0), Vec3::new(1.0, 0.355, 1.0), Vec3::ONE);
        assert!((half_bot.y - 0.3775).abs() < 1e-4, "half should be the OLD (wrong) number");
        assert!((ride_bot - 0.400).abs() < 1e-6, "drop should be the real distance to the feet");
        assert!(ride_bot > half_bot.y, "this is exactly the sink da831f98 measured and fixed");
    }

    #[test]
    fn a_body_floating_entirely_above_the_origin_gets_a_negative_drop() {
        // da831f98's other named case: flyer_deathstroke and
        // spaceship_horizon sit entirely above y=0. The seating code's
        // `> 0.0` check is what turns a negative drop into "leave it be"
        // — this only checks that the formula actually produces one.
        let (_, ride) = bounds_from_extent(
            Vec3::new(-1.0, 0.170, -1.0), Vec3::new(1.0, 0.345, 1.0), Vec3::ONE);
        assert!(ride < 0.0, "a body entirely above the origin should get a negative drop, got {ride}");
    }

    #[test]
    fn a_degenerate_box_falls_back_to_the_symmetric_assumption() {
        // hi < lo means nothing was actually measured (empty raw set) --
        // the fallback ext_half is what the caller already had, and the
        // drop falls back to matching it rather than computing garbage
        // from an inverted box.
        let ext_half = Vec3::new(0.5, 0.3, 0.8);
        let (half, ride) = bounds_from_extent(Vec3::splat(f32::MAX), Vec3::splat(f32::MIN), ext_half);
        assert_eq!(half, ext_half);
        assert_eq!(ride, ext_half.y);
    }
}

/// As `load`, but `flip` turns the body around its vertical axis. The nose
/// is inferred from the geometry, and inference is not always right — the
/// Guardian arrived facing backwards, which is invisible until you drive it.
pub fn load_oriented(dir: &Path, gpu: &mut Gpu, target_len: f32, flip: bool) -> Model {
    let (raws, half, center, s, ride_h) = load_raw_at(dir, target_len, flip);

    // ---- upload: dedupe textures per model, build prims ----
    let mut tex_cache: HashMap<String, usize> = HashMap::new();
    let mut prims = Vec::new();
    let mut cloud_all: Vec<Vec3> = Vec::new();
    let mut cloud_nrm: Vec<Vec3> = Vec::new();
    let mut wheel_bottom = f32::MAX;
    let mut wheel_top = f32::MIN;
    // per-prim points, kept only long enough to find the fittings: the
    // verts move into the gpu below and the indices have to line up with
    // `prims`, so this is the one moment both exist
    let mut prim_pts: Vec<Vec<Vec3>> = Vec::new();
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
        let mut mine: Vec<Vec3> = Vec::with_capacity(r.verts.len());
        for v in &r.verts {
            let p = Vec3::from_array(v.pos);
            cloud_all.push(p);
            cloud_nrm.push(Vec3::from_array(v.nrm).normalize_or(Vec3::Y));
            mine.push(p);
        }
        prim_pts.push(mine);
        let mesh_id = gpu.add_mesh(&MeshData { verts: r.verts, idx: r.idx });
        prims.push(Prim { mesh_id, tex_id, color: r.color, tag: r.tag, pivot });
    }

    // WHAT THIS HULL CARRIES. Measured in the oriented frame, so "on top"
    // and "along the side" mean what they say (`turret::find`).
    let refs: Vec<&[Vec3]> = prim_pts.iter().map(|v| v.as_slice()).collect();
    let fittings = crate::turret::find(&refs);
    // AND HOW MUCH OF IT IS IN THE WAY. The skeleton finds the core and
    // the appendages; `compact` shortens each overhanging one along its
    // own axis to half its reach, which is the number that keeps a jet's
    // wings from being the whole silhouette when it stands up.
    // What is still not upright after the chassis frame? Ask the same
    // search again on the ORIENTED points: an answer other than identity
    // means this pose stands along its own axis.
    let settle = {
        let vs: Vec<Vertex> = raws.iter().flat_map(|r| r.verts.iter().copied()).collect();
        if vs.is_empty() { Quat::IDENTITY } else { find_orientation(&vs) }
    };
    let (tuck, pack) = {
        let sk = crate::skeleton::skeletonize(&refs);
        (
            crate::skeleton::compact(&sk, refs.len(), 0.5),
            // vehicle mode is stricter: contain rather than shorten, with
            // a quarter of the core's own size as slack so a bonnet still
            // reads as a bonnet
            crate::skeleton::contain(&sk, &refs, 0.25),
        )
    };

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
    Model { prims, drive: Drive::Combustion, hover: None, settle, tuck, pack, fittings, cloud, cloud_n, sdf_half: Vec3::ZERO,
            half_len: half.z, half_h: half.y, half_w: half.x, wheel_r,
            // Same sign convention as xform::Morph::bounds_at: -lo.y, the
            // distance from the origin to the pose's actual lowest point,
            // not the symmetric half-extent. See the field's own doc
            // comment for why this matters (da831f98 fixed this for the
            // runtime blend path only; this is the static load path).
            ride_h }
}
