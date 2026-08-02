//! THE RIG: glTF skins and animation clips, evaluated at a time.
//!
//! `model.rs` bakes node transforms into the vertices at load and hands the
//! GPU something that never moves again. That is right for a car and wrong
//! for a machine whose whole point is that it folds — so this module keeps
//! what `model.rs` throws away: the node hierarchy, the animation channels,
//! and the skin.
//!
//! What the reference transformations actually need, measured rather than
//! assumed (Optimus "Change form", 83 joints, 8.33 s):
//!
//!   * 82 of 83 joints are animated, and NO animated node carries a mesh
//!     directly — every mesh is a child that inherits its motion through the
//!     hierarchy. So a flat list of per-prim matrices cannot reproduce it;
//!     the tree has to be walked.
//!   * 61% of the geometry is genuinely skinned (four joints per vertex),
//!     39% rides a single node rigidly. Both paths are needed.
//!   * 143 of 228 animated nodes are rotation/translation only. Of the 85
//!     that touch scale, 58 are CONSTANT — rig setup, not animation — and
//!     the 27 that vary stay between 0.32 and 1.0. Nothing vanishes.
//!
//! That last point is the interesting one. It is why this exists instead of
//! a pose strip: the motion is overwhelmingly rigid, so it interpolates
//! correctly at any t, and sampling it at runtime costs a matrix per joint
//! rather than a whole model per frame of the fold.
//!
//! Scale is still carried through faithfully. Refusing to (foldview proves
//! every part rigid and rejects anything else) is a stricter rule than the
//! reference animations follow, and the 27 telescoping joints here are the
//! evidence: a real transformation retracts parts into themselves.

use std::path::Path;

use glam::{Mat4, Quat, Vec3};

/// One animation channel: a node, a property, and the keys that drive it.
struct Channel {
    node: usize,
    path: Path_,
    times: Vec<f32>,
    /// 3 floats per key for translation/scale, 4 for rotation
    vals: Vec<f32>,
    step: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Path_ {
    T,
    R,
    S,
}

pub struct Clip {
    pub name: String,
    pub duration: f32,
    chans: Vec<Channel>,
}

struct Skin {
    joints: Vec<usize>,
    ibm: Vec<Mat4>,
}

/// A prim as the rig sees it: bind-pose positions, plus either a skin
/// binding or the single node it rides.
pub struct RigPrim {
    pub pos: Vec<[f32; 3]>,
    pub idx: Vec<u32>,
    pub color: [f32; 4],
    /// (skin, joints, weights) when skinned
    skin: Option<usize>,
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
    /// the node this prim hangs off (its own transform when unskinned)
    node: usize,
}

pub struct Rig {
    parent: Vec<Option<usize>>,
    /// bind-pose local transform of every node
    local_t: Vec<Vec3>,
    local_r: Vec<Quat>,
    local_s: Vec<Vec3>,
    /// parents strictly before children, so one pass composes the tree
    order: Vec<usize>,
    skins: Vec<Skin>,
    pub clips: Vec<Clip>,
    pub prims: Vec<RigPrim>,
}

fn decompose(n: &gltf::Node) -> (Vec3, Quat, Vec3) {
    match n.transform() {
        gltf::scene::Transform::Matrix { matrix } => {
            let m = Mat4::from_cols_array_2d(&matrix);
            let (s, r, t) = m.to_scale_rotation_translation();
            (t, r, s)
        }
        gltf::scene::Transform::Decomposed {
            translation,
            rotation,
            scale,
        } => (
            Vec3::from_array(translation),
            Quat::from_array(rotation),
            Vec3::from_array(scale),
        ),
    }
}

impl Rig {
    /// Load `dir/scene.gltf` (or `dir.glb`) keeping the hierarchy intact.
    pub fn load(dir: &Path) -> Option<Rig> {
        let glb = dir.with_extension("glb");
        let (doc, bin) = if glb.is_file() {
            let d = gltf::Gltf::open(&glb).ok()?;
            let b = d.blob.clone().unwrap_or_default();
            (d, b)
        } else {
            let f = dir.join("scene.gltf");
            let d = gltf::Gltf::open(&f).ok()?;
            let uri = d.buffers().next().and_then(|b| match b.source() {
                gltf::buffer::Source::Uri(u) => Some(u.to_string()),
                _ => None,
            })?;
            let b = std::fs::read(dir.join(uri)).ok()?;
            (d, b)
        };
        let bin: &[u8] = &bin;

        let n = doc.nodes().count();
        let mut parent = vec![None; n];
        let (mut lt, mut lr, mut ls) = (vec![Vec3::ZERO; n], vec![Quat::IDENTITY; n], vec![Vec3::ONE; n]);
        for node in doc.nodes() {
            let i = node.index();
            let (t, r, s) = decompose(&node);
            lt[i] = t;
            lr[i] = r;
            ls[i] = s;
            for c in node.children() {
                parent[c.index()] = Some(i);
            }
        }
        // topological order: repeatedly take nodes whose parent is placed
        let mut order = Vec::with_capacity(n);
        let mut placed = vec![false; n];
        let mut guard = 0;
        while order.len() < n && guard <= n {
            for i in 0..n {
                if placed[i] {
                    continue;
                }
                if parent[i].map_or(true, |p| placed[p]) {
                    placed[i] = true;
                    order.push(i);
                }
            }
            guard += 1;
        }
        if order.len() != n {
            return None; // a cycle: not a tree, and nothing here can pose it
        }

        let mut skins = Vec::new();
        for s in doc.skins() {
            let r = s.reader(|_| Some(bin));
            let ibm: Vec<Mat4> = match r.read_inverse_bind_matrices() {
                Some(it) => it.map(|m| Mat4::from_cols_array_2d(&m)).collect(),
                None => vec![Mat4::IDENTITY; s.joints().count()],
            };
            skins.push(Skin {
                joints: s.joints().map(|j| j.index()).collect(),
                ibm,
            });
        }

        let mut clips = Vec::new();
        for a in doc.animations() {
            let mut chans = Vec::new();
            let mut duration: f32 = 0.0;
            for c in a.channels() {
                let path = match c.target().property() {
                    gltf::animation::Property::Translation => Path_::T,
                    gltf::animation::Property::Rotation => Path_::R,
                    gltf::animation::Property::Scale => Path_::S,
                    // morph weights: no morph targets in either reference
                    // model, and nothing downstream could consume them
                    gltf::animation::Property::MorphTargetWeights => continue,
                };
                let r = c.reader(|_| Some(bin));
                let times: Vec<f32> = match r.read_inputs() {
                    Some(it) => it.collect(),
                    None => continue,
                };
                let vals: Vec<f32> = match r.read_outputs() {
                    Some(gltf::animation::util::ReadOutputs::Translations(it)) => {
                        it.flat_map(|v| v.into_iter()).collect()
                    }
                    Some(gltf::animation::util::ReadOutputs::Scales(it)) => {
                        it.flat_map(|v| v.into_iter()).collect()
                    }
                    Some(gltf::animation::util::ReadOutputs::Rotations(it)) => {
                        it.into_f32().flat_map(|v| v.into_iter()).collect()
                    }
                    _ => continue,
                };
                if let Some(&last) = times.last() {
                    duration = duration.max(last);
                }
                chans.push(Channel {
                    node: c.target().node().index(),
                    path,
                    times,
                    vals,
                    step: matches!(
                        c.sampler().interpolation(),
                        gltf::animation::Interpolation::Step
                    ),
                });
            }
            clips.push(Clip {
                name: a.name().unwrap_or("clip").to_string(),
                duration,
                chans,
            });
        }

        // prims, in BIND POSE — no world transform baked in, because the
        // whole point is that the transform changes every frame
        let mut prims = Vec::new();
        for node in doc.nodes() {
            let Some(mesh) = node.mesh() else { continue };
            for p in mesh.primitives() {
                let r = p.reader(|_| Some(bin));
                let Some(pos) = r.read_positions() else { continue };
                let pos: Vec<[f32; 3]> = pos.collect();
                let idx: Vec<u32> = match r.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    None => (0..pos.len() as u32).collect(),
                };
                let joints: Vec<[u16; 4]> = r.read_joints(0).map(|j| j.into_u16().collect()).unwrap_or_default();
                let weights: Vec<[f32; 4]> = r.read_weights(0).map(|w| w.into_f32().collect()).unwrap_or_default();
                let c = p
                    .material()
                    .pbr_metallic_roughness()
                    .base_color_factor();
                prims.push(RigPrim {
                    pos,
                    idx,
                    color: c,
                    skin: node.skin().map(|s| s.index()),
                    joints,
                    weights,
                    node: node.index(),
                });
            }
        }

        Some(Rig {
            parent,
            local_t: lt,
            local_r: lr,
            local_s: ls,
            order,
            skins,
            clips,
            prims,
        })
    }

    /// Global transform of every node, with `clip` evaluated at `t` seconds.
    pub fn pose(&self, clip: usize, t: f32) -> Vec<Mat4> {
        let (mut tt, mut rr, mut ss) =
            (self.local_t.clone(), self.local_r.clone(), self.local_s.clone());
        if let Some(c) = self.clips.get(clip) {
            for ch in &c.chans {
                sample(ch, t, &mut tt, &mut rr, &mut ss);
            }
        }
        let mut g = vec![Mat4::IDENTITY; tt.len()];
        for &i in &self.order {
            let local = Mat4::from_scale_rotation_translation(ss[i], rr[i], tt[i]);
            g[i] = match self.parent[i] {
                Some(p) => g[p] * local,
                None => local,
            };
        }
        g
    }

    /// Skin (or rigidly place) every prim into world space at this pose.
    ///
    /// CPU side on purpose. GPU skinning wants a joint palette, two more
    /// vertex attributes and a shader change; this needs none of that, runs
    /// only while a transform is actually playing, and produces exactly the
    /// vertex buffer the existing pipeline already knows how to draw.
    pub fn skin(&self, g: &[Mat4]) -> Vec<(Vec<[f32; 3]>, Vec<u32>, [f32; 4])> {
        let mut out = Vec::with_capacity(self.prims.len());
        for p in &self.prims {
            let mut pos = Vec::with_capacity(p.pos.len());
            match p.skin {
                Some(si) if !p.joints.is_empty() => {
                    let sk = &self.skins[si];
                    // joint matrix = global(joint) * inverseBind. The skinned
                    // node's own transform is deliberately not applied: per
                    // the glTF spec it is ignored, and honouring it here
                    // doubles the root motion.
                    let jm: Vec<Mat4> = sk
                        .joints
                        .iter()
                        .zip(&sk.ibm)
                        .map(|(&j, ibm)| g[j] * *ibm)
                        .collect();
                    for (v, (js, ws)) in p.pos.iter().zip(p.joints.iter().zip(&p.weights)) {
                        let vp = Vec3::from_array(*v);
                        let mut acc = Vec3::ZERO;
                        let mut wsum = 0.0;
                        for k in 0..4 {
                            let w = ws[k];
                            if w == 0.0 {
                                continue;
                            }
                            if let Some(m) = jm.get(js[k] as usize) {
                                acc += m.transform_point3(vp) * w;
                                wsum += w;
                            }
                        }
                        // a vertex with no usable weights would collapse to
                        // the origin and spike a triangle across the model
                        let f = if wsum > 1e-6 { acc / wsum } else { g[p.node].transform_point3(vp) };
                        pos.push(f.to_array());
                    }
                }
                _ => {
                    let m = g[p.node];
                    for v in &p.pos {
                        pos.push(m.transform_point3(Vec3::from_array(*v)).to_array());
                    }
                }
            }
            out.push((pos, p.idx.clone(), p.color));
        }
        out
    }
}

fn sample(ch: &Channel, t: f32, tt: &mut [Vec3], rr: &mut [Quat], ss: &mut [Vec3]) {
    let n = ch.times.len();
    if n == 0 {
        return;
    }
    let comps = if ch.path == Path_::R { 4 } else { 3 };
    if ch.vals.len() < n * comps {
        return; // CUBICSPLINE stores 3 values per key; not in either reference
    }
    let (i0, i1, f) = if t <= ch.times[0] {
        (0, 0, 0.0)
    } else if t >= ch.times[n - 1] {
        (n - 1, n - 1, 0.0)
    } else {
        let mut k = 0;
        while k + 1 < n && ch.times[k + 1] < t {
            k += 1;
        }
        let (a, b) = (ch.times[k], ch.times[k + 1]);
        let f = if b > a { (t - a) / (b - a) } else { 0.0 };
        (k, k + 1, if ch.step { 0.0 } else { f })
    };
    let at = |i: usize| -> &[f32] { &ch.vals[i * comps..i * comps + comps] };
    let (a, b) = (at(i0), at(i1));
    match ch.path {
        Path_::T => {
            tt[ch.node] = Vec3::from_slice(a).lerp(Vec3::from_slice(b), f);
        }
        Path_::S => {
            ss[ch.node] = Vec3::from_slice(a).lerp(Vec3::from_slice(b), f);
        }
        Path_::R => {
            let qa = Quat::from_xyzw(a[0], a[1], a[2], a[3]).normalize();
            let qb = Quat::from_xyzw(b[0], b[1], b[2], b[3]).normalize();
            // shortest arc: a raw lerp across an antipodal pair spins the
            // long way round, which on a 228-node rig is instantly obvious
            rr[ch.node] = qa.slerp(if qa.dot(qb) < 0.0 { -qb } else { qb }, f);
        }
    }
}

/// Poses a rig into the format the harnesses and the loader already read.
pub fn pose_bounds(prims: &[(Vec<[f32; 3]>, Vec<u32>, [f32; 4])]) -> (Vec3, Vec3) {
    let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (pos, _, _) in prims {
        for v in pos {
            let p = Vec3::from_array(*v);
            mn = mn.min(p);
            mx = mx.max(p);
        }
    }
    (mn, mx)
}
