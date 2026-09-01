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
const POSE_BYTES: usize = 44;

pub struct EngineScene {
    pub width: usize,
    pub height: usize,
    pub mesh: SceneMesh,
    pub poses: Vec<CameraPose>,
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
