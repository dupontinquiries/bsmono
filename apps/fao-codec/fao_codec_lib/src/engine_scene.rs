//! # engine_scene — the seam an engine hands its world across
//!
//! `bin/engine_lab` measured what this is for: given a correct canonical
//! and correct poses, citing them beat self-coding by 31% of the bytes
//! AND 5.5 dB at once, while the same canonical at wrong poses was 21%
//! WORSE than self-coding. So the value is entirely in getting geometry
//! and camera right, and an engine already knows both exactly.
//!
//! ## What this file is, and what it deliberately is not
//!
//! It is a **format plus a reader**, not an integration. The engine side
//! must not depend on this crate: a game should not link a video codec
//! to be recordable, and a codec should not track a game's types. So the
//! wire format is plain, self-describing, and documented here well
//! enough that an engine can emit it with `std::io::Write` and nothing
//! else — which is what keeps this a seam rather than a coupling.
//!
//! ## Format (`.faoscene`, little-endian throughout)
//!
//! ```text
//! magic   "FAOS"          4 bytes
//! version u16 = 1
//! flags   u16             reserved, must be 0
//! frames  u32             number of pose records
//! width   u32             render width the poses were authored for
//! height  u32             render height
//! mesh    u32 length, then that many bytes of `mesh_pack` payload
//! poses   frames x 44 bytes:
//!           position  3 x f32   world space
//!           rotation  4 x f32   quaternion [w, x, y, z]
//!           focal     2 x f32   pixels
//!           principal 2 x f32   pixels
//! ```
//!
//! Two conventions are load-bearing and are stated because an engine
//! author cannot infer them from the layout:
//!
//! * the quaternion is `[w, x, y, z]`, NOT `[x, y, z, w]`. The codec has
//!   already shipped one bug from exactly this ambiguity — a decoder
//!   read the other order and every frame rendered behind the camera,
//!   silently falling back to source video — so the order is fixed here
//!   and a scene that gets it wrong will render nothing rather than
//!   render subtly wrong;
//! * `focal` is in PIXELS at the stated `width`/`height`. An engine
//!   holding a vertical FOV converts with
//!   `focal_y = (height / 2) / tan(fov_y / 2)`, and `focal_x` is
//!   `focal_y * aspect` for square pixels.
//!
//! ## Version 2 (2026-09-01): meshes, instances, renderer, stages
//!
//! v1 carries one mesh and one camera, which is a MAP. The measured
//! payoff (SESSION_LOG, soldier: 9x under AV1) needs the actors as
//! entities — a mesh in the library plus a pose per frame — and the
//! fog result needs the atmosphere as a stage the renderer evaluates
//! with transmitted parameters. v2 adds exactly those, after the v1
//! header fields so a v1 reader's refusal still triggers on the
//! version number:
//!
//! ```text
//! magic     "FAOS"   version u16 = 2   flags u16 = 0
//! frames    u32      width u32         height u32
//! renderer  u16 len, utf-8 identity ("early-fps/render@1"): the
//!           decoder must hold THIS renderer; a generic rasteriser
//!           cannot reproduce per-vertex fog or the far cull, and a
//!           stage is parameters the renderer reads
//! meshes    u32 count, each u32 len + mesh_pack payload
//! stages    u32 count, each: u8 kind, u16 len, payload — SHOT level
//!           kind 1 = fog: rgb u8 x3, start f32, end f32
//! poses     frames x 44 bytes, as v1 (the camera)
//! instances per frame: u16 count, each u16 mesh, position 3 x f32,
//!           rotation quaternion [w, x, y, z] 4 x f32     (30 bytes)
//! overrides per frame: u8 count, each a stage record — FRAME level,
//!           replaces the shot-level stage of the same kind
//! ```
//!
//! Rates are the design: a mesh is written once, a shot-level stage
//! once, a camera and an instance per frame, a stage override only
//! when it changes. That is the library / shot / frame factorisation
//! the photometric work measured, made a wire format.
//!
//! ## Why the mesh is `mesh_pack` and not a mesh format
//!
//! `mesh_pack` is already the codec's measured wire form for geometry
//! (one AABB, quantised plane-split positions, delta indices — 10 bits
//! measured as the knee at 29% of naive). Embedding it means the scene
//! file inherits that work instead of adding a second geometry
//! representation to keep in sync.

use crate::mesh_pack;
use crate::v11_plate::Yuv420;
use crate::v8_rasterizer::render_mesh;
use crate::v8_reconstruction_codec::{CameraPose, SceneMesh};
use crate::geometry::Vec3;

pub const MAGIC: &[u8; 4] = b"FAOS";
pub const VERSION: u16 = 1;
pub const VERSION2: u16 = 2;
const POSE_BYTES: usize = 44;
const INSTANCE_BYTES: usize = 30;

pub struct EngineScene {
    pub width: usize,
    pub height: usize,
    pub mesh: SceneMesh,
    pub poses: Vec<CameraPose>,
}

/// One rendered thing in one frame: which library mesh, where.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    pub mesh: u16,
    pub position: Vec3,
    /// [w, x, y, z], as the camera's.
    pub rotation: [f32; 4],
}

impl Instance {
    pub fn identity(mesh: u16) -> Self {
        Instance { mesh, position: Vec3::new(0.0, 0.0, 0.0), rotation: [1.0, 0.0, 0.0, 0.0] }
    }
}

/// A stage record: parameters the renderer reads. `kind` names the
/// stage; the payload layout is per kind and documented at the top.
#[derive(Clone, Debug, PartialEq)]
pub struct Stage {
    pub kind: u8,
    pub payload: Vec<u8>,
}

pub const STAGE_FOG: u8 = 1;

impl Stage {
    pub fn fog(color: [u8; 3], start: f32, end: f32) -> Self {
        let mut p = Vec::with_capacity(11);
        p.extend_from_slice(&color);
        p.extend_from_slice(&start.to_le_bytes());
        p.extend_from_slice(&end.to_le_bytes());
        Stage { kind: STAGE_FOG, payload: p }
    }
    pub fn as_fog(&self) -> Option<([u8; 3], f32, f32)> {
        if self.kind != STAGE_FOG || self.payload.len() != 11 {
            return None;
        }
        let p = &self.payload;
        let f = |o: usize| f32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]]);
        Some(([p[0], p[1], p[2]], f(3), f(7)))
    }
}

pub struct EngineScene2 {
    pub width: usize,
    pub height: usize,
    pub renderer: String,
    pub meshes: Vec<SceneMesh>,
    pub stages: Vec<Stage>,
    pub poses: Vec<CameraPose>,
    /// Per frame, the instances to render, in draw order.
    pub instances: Vec<Vec<Instance>>,
    /// Per frame, stage overrides (usually empty).
    pub overrides: Vec<Vec<Stage>>,
}

fn put_stage(out: &mut Vec<u8>, st: &Stage) {
    out.push(st.kind);
    out.extend_from_slice(&(st.payload.len() as u16).to_le_bytes());
    out.extend_from_slice(&st.payload);
}

fn get_stage(data: &[u8], o: &mut usize) -> Result<Stage, String> {
    if *o + 3 > data.len() {
        return Err("truncated stage".into());
    }
    let kind = data[*o];
    let len = u16::from_le_bytes([data[*o + 1], data[*o + 2]]) as usize;
    *o += 3;
    if *o + len > data.len() {
        return Err("truncated stage payload".into());
    }
    let payload = data[*o..*o + len].to_vec();
    *o += len;
    Ok(Stage { kind, payload })
}

fn put_pose(out: &mut Vec<u8>, p: &CameraPose) {
    for v in [p.position.x, p.position.y, p.position.z] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in p.rotation {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in p.focal {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in p.principal {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn get_pose(data: &[u8], o: usize) -> CameraPose {
    let f = |k: usize| f32::from_le_bytes([data[o + k], data[o + k + 1], data[o + k + 2], data[o + k + 3]]);
    CameraPose {
        position: Vec3::new(f(0), f(4), f(8)),
        rotation: [f(12), f(16), f(20), f(24)],
        focal: [f(28), f(32)],
        principal: [f(36), f(40)],
        distortion: 0.0,
    }
}

pub fn write2(scene: &EngineScene2, bits: u8) -> Vec<u8> {
    let frames = scene.poses.len();
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION2.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(frames as u32).to_le_bytes());
    out.extend_from_slice(&(scene.width as u32).to_le_bytes());
    out.extend_from_slice(&(scene.height as u32).to_le_bytes());
    out.extend_from_slice(&(scene.renderer.len() as u16).to_le_bytes());
    out.extend_from_slice(scene.renderer.as_bytes());
    out.extend_from_slice(&(scene.meshes.len() as u32).to_le_bytes());
    for m in &scene.meshes {
        let (wire, _) = mesh_pack::encode(m, &mesh_pack::MeshPackOpts { pos_bits: bits, keep_colors: true });
        out.extend_from_slice(&(wire.len() as u32).to_le_bytes());
        out.extend_from_slice(&wire);
    }
    out.extend_from_slice(&(scene.stages.len() as u32).to_le_bytes());
    for st in &scene.stages {
        put_stage(&mut out, st);
    }
    for p in &scene.poses {
        put_pose(&mut out, p);
    }
    for fi in 0..frames {
        let inst = scene.instances.get(fi).map(|v| v.as_slice()).unwrap_or(&[]);
        out.extend_from_slice(&(inst.len() as u16).to_le_bytes());
        for i in inst {
            out.extend_from_slice(&i.mesh.to_le_bytes());
            for v in [i.position.x, i.position.y, i.position.z] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            for v in i.rotation {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        let ov = scene.overrides.get(fi).map(|v| v.as_slice()).unwrap_or(&[]);
        out.push(ov.len() as u8);
        for st in ov {
            put_stage(&mut out, st);
        }
    }
    out
}

/// Reads v2, and lifts v1 into it (one mesh, one identity instance
/// per frame, no renderer, no stages) so one code path serves both.
pub fn read2(data: &[u8]) -> Result<EngineScene2, String> {
    if data.len() < 24 || &data[0..4] != MAGIC {
        return Err("not a FAOS scene".into());
    }
    let ver = u16::from_le_bytes([data[4], data[5]]);
    if ver == VERSION {
        let s = read(data)?;
        let frames = s.poses.len();
        return Ok(EngineScene2 {
            width: s.width,
            height: s.height,
            renderer: String::new(),
            meshes: vec![s.mesh],
            stages: Vec::new(),
            poses: s.poses,
            instances: vec![vec![Instance::identity(0)]; frames],
            overrides: vec![Vec::new(); frames],
        });
    }
    if ver != VERSION2 {
        return Err(format!("scene version {ver}, this build reads {VERSION} and {VERSION2}"));
    }
    let g32 = |o: usize| -> Result<usize, String> {
        if o + 4 > data.len() {
            return Err("truncated header".into());
        }
        Ok(u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize)
    };
    let frames = g32(8)?;
    let width = g32(12)?;
    let height = g32(16)?;
    let mut o = 20;
    if o + 2 > data.len() {
        return Err("truncated renderer".into());
    }
    let rl = u16::from_le_bytes([data[o], data[o + 1]]) as usize;
    o += 2;
    if o + rl > data.len() {
        return Err("truncated renderer".into());
    }
    let renderer = String::from_utf8(data[o..o + rl].to_vec()).map_err(|_| "renderer not utf-8")?;
    o += rl;
    let n_meshes = g32(o)?;
    o += 4;
    let mut meshes = Vec::with_capacity(n_meshes);
    for _ in 0..n_meshes {
        let len = g32(o)?;
        o += 4;
        if o + len > data.len() {
            return Err("truncated mesh".into());
        }
        meshes.push(mesh_pack::decode(&data[o..o + len]).ok_or("mesh decode failed")?);
        o += len;
    }
    let n_stages = g32(o)?;
    o += 4;
    let mut stages = Vec::with_capacity(n_stages);
    for _ in 0..n_stages {
        stages.push(get_stage(data, &mut o)?);
    }
    if o + frames * POSE_BYTES > data.len() {
        return Err("truncated poses".into());
    }
    let poses: Vec<CameraPose> = (0..frames).map(|i| get_pose(data, o + i * POSE_BYTES)).collect();
    o += frames * POSE_BYTES;
    let mut instances = Vec::with_capacity(frames);
    let mut overrides = Vec::with_capacity(frames);
    for _ in 0..frames {
        if o + 2 > data.len() {
            return Err("truncated instances".into());
        }
        let cnt = u16::from_le_bytes([data[o], data[o + 1]]) as usize;
        o += 2;
        if o + cnt * INSTANCE_BYTES > data.len() {
            return Err("truncated instances".into());
        }
        let mut v = Vec::with_capacity(cnt);
        for _ in 0..cnt {
            let f = |k: usize| f32::from_le_bytes([data[o + k], data[o + k + 1], data[o + k + 2], data[o + k + 3]]);
            let mesh = u16::from_le_bytes([data[o], data[o + 1]]);
            if mesh as usize >= meshes.len() {
                return Err(format!("instance refers to mesh {mesh}, scene has {}", meshes.len()));
            }
            v.push(Instance {
                mesh,
                position: Vec3::new(f(2), f(6), f(10)),
                rotation: [f(14), f(18), f(22), f(26)],
            });
            o += INSTANCE_BYTES;
        }
        instances.push(v);
        if o >= data.len() {
            return Err("truncated overrides".into());
        }
        let oc = data[o] as usize;
        o += 1;
        let mut ov = Vec::with_capacity(oc);
        for _ in 0..oc {
            ov.push(get_stage(data, &mut o)?);
        }
        overrides.push(ov);
    }
    Ok(EngineScene2 { width, height, renderer, meshes, stages, poses, instances, overrides })
}

impl EngineScene2 {
    /// The stage of `kind` in force at frame `fi`: the frame override
    /// if present, else the shot-level record.
    pub fn stage_at(&self, fi: usize, kind: u8) -> Option<&Stage> {
        self.overrides
            .get(fi)
            .and_then(|v| v.iter().find(|s| s.kind == kind))
            .or_else(|| self.stages.iter().find(|s| s.kind == kind))
    }

    /// All instances of frame `fi` merged into one world-space mesh,
    /// for a renderer that takes a single mesh (the codec's own).
    pub fn merged_mesh(&self, fi: usize) -> SceneMesh {
        let mut out = SceneMesh::default();
        let Some(inst) = self.instances.get(fi) else { return out };
        for i in inst {
            let m = &self.meshes[i.mesh as usize];
            let base = out.vertices.len() as u32;
            for v in &m.vertices {
                let r = rotate(i.rotation, *v);
                out.vertices.push(Vec3::new(r.x + i.position.x, r.y + i.position.y, r.z + i.position.z));
            }
            out.vertex_colors.extend_from_slice(&m.vertex_colors);
            out.normals.extend(m.normals.iter().map(|n| rotate(i.rotation, *n)));
            out.indices.extend(m.indices.iter().map(|k| k + base));
        }
        out
    }

    /// Render with the codec's generic rasteriser. Correct for
    /// geometry and vertex colour; NOT for per-vertex fog or a far
    /// cull — those need `renderer`, which is why the field exists.
    pub fn predict(&self, fi: usize, w: usize, h: usize) -> Yuv420 {
        let one = EngineScene {
            width: self.width,
            height: self.height,
            mesh: self.merged_mesh(fi),
            poses: self.poses.clone(),
        };
        one.predict(fi, w, h)
    }
}

/// [w, x, y, z] quaternion rotation of a vector.
fn rotate(q: [f32; 4], v: Vec3) -> Vec3 {
    let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
    // v' = v + 2 q_v x (q_v x v + w v)
    let (cx, cy, cz) = (y * v.z - z * v.y + w * v.x, z * v.x - x * v.z + w * v.y, x * v.y - y * v.x + w * v.z);
    Vec3::new(
        v.x + 2.0 * (y * cz - z * cy),
        v.y + 2.0 * (z * cx - x * cz),
        v.z + 2.0 * (x * cy - y * cx),
    )
}

pub fn write(scene: &EngineScene, bits: u8) -> Vec<u8> {
    let (mesh_wire, _) = mesh_pack::encode(
        &scene.mesh,
        &mesh_pack::MeshPackOpts { pos_bits: bits, keep_colors: true },
    );
    let mut out = Vec::with_capacity(mesh_wire.len() + scene.poses.len() * POSE_BYTES + 24);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(scene.poses.len() as u32).to_le_bytes());
    out.extend_from_slice(&(scene.width as u32).to_le_bytes());
    out.extend_from_slice(&(scene.height as u32).to_le_bytes());
    out.extend_from_slice(&(mesh_wire.len() as u32).to_le_bytes());
    out.extend_from_slice(&mesh_wire);
    for p in &scene.poses {
        for v in [p.position.x, p.position.y, p.position.z] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in p.rotation {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in p.focal {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in p.principal {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

pub fn read(data: &[u8]) -> Result<EngineScene, String> {
    if data.len() < 24 || &data[0..4] != MAGIC {
        return Err("not a FAOS scene".into());
    }
    let ver = u16::from_le_bytes([data[4], data[5]]);
    if ver != VERSION {
        // Refused, not guessed. A future layout read as v1 would produce
        // plausible geometry at wrong poses, which is the failure mode
        // engine_lab measured as 21% WORSE than not citing at all — an
        // error is strictly better than that.
        return Err(format!("scene version {ver}, this build reads {VERSION}"));
    }
    let g32 = |o: usize| -> usize {
        u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize
    };
    let frames = g32(8);
    let width = g32(12);
    let height = g32(16);
    let mesh_len = g32(20);
    let mesh_end = 24 + mesh_len;
    if data.len() < mesh_end + frames * POSE_BYTES {
        return Err("truncated scene".into());
    }
    let mesh = mesh_pack::decode(&data[24..mesh_end]).ok_or("mesh decode failed")?;
    let f32_at = |o: usize| f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    let mut poses = Vec::with_capacity(frames);
    for i in 0..frames {
        let o = mesh_end + i * POSE_BYTES;
        poses.push(CameraPose {
            position: Vec3::new(f32_at(o), f32_at(o + 4), f32_at(o + 8)),
            rotation: [f32_at(o + 12), f32_at(o + 16), f32_at(o + 20), f32_at(o + 24)],
            focal: [f32_at(o + 28), f32_at(o + 32)],
            principal: [f32_at(o + 36), f32_at(o + 40)],
            distortion: 0.0,
        });
    }
    Ok(EngineScene { width, height, mesh, poses })
}

impl EngineScene {
    /// Render the canonical at frame `fi`'s pose, as the prediction
    /// provider `encode_gop_v3_core` consumes. Unrasterised pixels come
    /// back neutral gray so the mode decision routes them to prev-MC or
    /// intra exactly as it does for a plate miss — a scene that covers
    /// only part of the frame degrades instead of failing.
    pub fn predict(&self, fi: usize, w: usize, h: usize) -> Yuv420 {
        let mut out = Yuv420::new(w, h);
        for p in out.y.iter_mut() {
            *p = 128;
        }
        for p in out.u.iter_mut().chain(out.v.iter_mut()) {
            *p = 128;
        }
        let Some(pose) = self.poses.get(fi) else { return out };
        // Poses are authored for the scene's own resolution; rescale the
        // intrinsics rather than refusing, so one capture can serve
        // several encode resolutions.
        let (sx, sy) = (w as f32 / self.width as f32, h as f32 / self.height as f32);
        let scaled = CameraPose {
            focal: [pose.focal[0] * sx, pose.focal[1] * sy],
            principal: [pose.principal[0] * sx, pose.principal[1] * sy],
            ..*pose
        };
        let Some(rgb) = render_mesh(&self.mesh, None, &scaled, w, h) else { return out };
        let cw = w / 2;
        let mut us = vec![0u32; cw * (h / 2)];
        let mut vs = vec![0u32; cw * (h / 2)];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                let (r, g, b) = (rgb[i] as f32, rgb[i + 1] as f32, rgb[i + 2] as f32);
                let (r, g, b) = if rgb[i] == 0 && rgb[i + 1] == 0 && rgb[i + 2] == 0 {
                    (128.0, 128.0, 128.0)
                } else {
                    (r, g, b)
                };
                out.y[y * w + x] =
                    (0.299 * r + 0.587 * g + 0.114 * b).round().clamp(0.0, 255.0) as u8;
                let ci = (y / 2) * cw + x / 2;
                us[ci] += (-0.168736 * r - 0.331264 * g + 0.5 * b + 128.0).round().clamp(0.0, 255.0) as u32;
                vs[ci] += (0.5 * r - 0.418688 * g - 0.081312 * b + 128.0).round().clamp(0.0, 255.0) as u32;
            }
        }
        for (o, s) in out.u.iter_mut().zip(&us) {
            *o = (s / 4) as u8;
        }
        for (o, s) in out.v.iter_mut().zip(&vs) {
            *o = (s / 4) as u8;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> EngineScene {
        let mut mesh = SceneMesh::default();
        for i in 0..24u32 {
            let f = i as f32;
            mesh.vertices.push(Vec3::new(f * 3.0 - 30.0, f.sin() * 20.0, 200.0 + f * 4.0));
            mesh.vertex_colors.push([(i % 5) as f32 / 5.0, 0.4, (i % 3) as f32 / 3.0]);
        }
        for t in 0..20u32 {
            mesh.indices.extend_from_slice(&[t, t + 1, t + 2]);
        }
        let poses = (0..6)
            .map(|i| CameraPose {
                position: Vec3::new(i as f32 * 5.0, 3.0, -10.0),
                rotation: [1.0, 0.0, 0.0, 0.0],
                focal: [320.0, 320.0],
                principal: [160.0, 96.0],
                distortion: 0.0,
            })
            .collect();
        EngineScene { width: 320, height: 192, mesh, poses }
    }

    /// Round-trip must preserve topology and poses. An engine emitting
    /// this file has no way to check the codec read it correctly, so the
    /// contract has to be tested from the codec side.
    #[test]
    fn scene_roundtrips() {
        let s = scene();
        let wire = write(&s, 16);
        let got = read(&wire).expect("must read back");
        assert_eq!(got.width, s.width);
        assert_eq!(got.height, s.height);
        assert_eq!(got.mesh.indices, s.mesh.indices);
        assert_eq!(got.poses.len(), s.poses.len());
        for (a, b) in got.poses.iter().zip(&s.poses) {
            assert!((a.position.x - b.position.x).abs() < 1e-5);
            assert_eq!(a.rotation, b.rotation);
            assert_eq!(a.focal, b.focal);
        }
    }

    fn scene2() -> EngineScene2 {
        let s = scene();
        let mut actor = SceneMesh::default();
        for i in 0..8u32 {
            let f = i as f32;
            actor.vertices.push(Vec3::new((f % 2.0) - 0.5, ((f / 2.0) % 2.0) - 0.5, (f / 4.0) - 0.5));
            actor.vertex_colors.push([0.8, 0.3, 0.2]);
        }
        actor.indices.extend_from_slice(&[0, 1, 2, 1, 3, 2, 4, 5, 6]);
        let frames = s.poses.len();
        let instances = (0..frames)
            .map(|fi| {
                vec![
                    Instance::identity(0),
                    Instance { mesh: 1, position: Vec3::new(fi as f32, 1.0, 210.0), rotation: [1.0, 0.0, 0.0, 0.0] },
                ]
            })
            .collect();
        let mut overrides = vec![Vec::new(); frames];
        overrides[3] = vec![Stage::fog([18, 21, 27], 12.0, 60.0)];
        EngineScene2 {
            width: s.width,
            height: s.height,
            renderer: "early-fps/render@1".into(),
            meshes: vec![s.mesh, actor],
            stages: vec![Stage::fog([18, 21, 27], 55.0, 280.0)],
            poses: s.poses,
            instances,
            overrides,
        }
    }

    /// v2 round trip: meshes, renderer identity, stages, instances
    /// and per-frame overrides all survive, and the override wins on
    /// its frame only.
    #[test]
    fn scene2_roundtrips_and_stages_resolve() {
        let s = scene2();
        let wire = write2(&s, 16);
        let got = read2(&wire).expect("v2 must read back");
        assert_eq!(got.renderer, "early-fps/render@1");
        assert_eq!(got.meshes.len(), 2);
        assert_eq!(got.meshes[1].indices, s.meshes[1].indices);
        assert_eq!(got.instances.len(), s.poses.len());
        assert_eq!(got.instances[2], s.instances[2]);
        assert_eq!(got.stage_at(0, STAGE_FOG).and_then(|f| f.as_fog()), Some(([18, 21, 27], 55.0, 280.0)));
        assert_eq!(got.stage_at(3, STAGE_FOG).and_then(|f| f.as_fog()), Some(([18, 21, 27], 12.0, 60.0)));
        assert_eq!(got.stage_at(4, STAGE_FOG).and_then(|f| f.as_fog()), Some(([18, 21, 27], 55.0, 280.0)));
        // merged geometry: map + actor, actor translated per frame
        let m2 = got.merged_mesh(2);
        assert_eq!(m2.vertices.len(), s.meshes[0].vertices.len() + 8);
        let a0 = m2.vertices[s.meshes[0].vertices.len()];
        assert!((a0.x - (2.0 - 0.5)).abs() < 1e-5 && (a0.z - 209.5).abs() < 1e-5);
    }

    /// A v1 file reads through the v2 path as a map with one identity
    /// instance per frame — the old seam is a special case of the new.
    #[test]
    fn v1_lifts_into_v2() {
        let s = scene();
        let wire = write(&s, 16);
        let got = read2(&wire).expect("v1 must lift");
        assert_eq!(got.meshes.len(), 1);
        assert!(got.renderer.is_empty());
        assert!(got.stages.is_empty());
        assert!(got.instances.iter().all(|v| v == &[Instance::identity(0)]));
    }

    /// v2 refuses truncation and dangling instance references.
    #[test]
    fn v2_refuses_bad_input() {
        let s = scene2();
        let wire = write2(&s, 16);
        assert!(read2(&wire[..wire.len() - 7]).is_err());
        let mut dangling = s;
        dangling.instances[1][1].mesh = 7;
        let bad = write2(&dangling, 16);
        match read2(&bad) {
            Err(e) => assert!(e.contains("mesh 7"), "got {e}"),
            Ok(_) => panic!("dangling mesh reference must be refused"),
        }
    }

    /// Bad input must be REFUSED, never guessed. engine_lab measured a
    /// canonical at wrong poses as 21% worse than not citing at all, so
    /// silently accepting a malformed or future scene is worse than an
    /// error.
    #[test]
    fn malformed_scenes_are_refused() {
        let s = scene();
        let wire = write(&s, 16);
        assert!(read(b"nope").is_err(), "bad magic must be refused");
        let mut bad_ver = wire.clone();
        bad_ver[4] = 9;
        match read(&bad_ver) {
            Err(e) => assert!(e.contains("version 9"), "got {e}"),
            Ok(_) => panic!("a future version must be refused, not guessed"),
        }
        assert!(read(&wire[..wire.len() - 20]).is_err(), "truncation must be caught");
        assert!(read(&wire).is_ok(), "the valid scene must still read");
    }

    /// The provider must produce a real prediction, and must rescale
    /// intrinsics when asked for a different resolution than the scene
    /// was authored at.
    #[test]
    fn provider_renders_and_rescales() {
        let s = read(&write(&scene(), 16)).unwrap();
        let native = s.predict(0, 320, 192);
        assert!(native.y.iter().any(|&v| v != 128), "nothing rasterised");
        let half = s.predict(0, 160, 96);
        assert_eq!(half.y.len(), 160 * 96);
        assert!(half.y.iter().any(|&v| v != 128), "rescaled render is empty");
        // Out-of-range frame yields neutral gray rather than panicking:
        // a pose stream shorter than the GOP is an engine's problem to
        // notice, not a reason to abort an encode.
        let past_end = s.predict(999, 320, 192);
        assert!(past_end.y.iter().all(|&v| v == 128));
    }
}
