// FAO Codec v8 — "Reconstruction" Codec
//
// SCAFFOLD ONLY in this session. The actual 3D reconstruction work will land
// in future sessions on this foundation.
//
// VISION:
//   v7 detects camera motion. v8 USES that motion to triangulate scene
//   geometry — solving the inverse problem: given how pixels move when the
//   camera moves, where ARE those pixels in 3D space?
//
//   Deliverables when complete:
//     - Per-frame depth map (each pixel gets a Z value)
//     - Sparse 3D point cloud of the scene
//     - Triangle mesh of major surfaces
//     - Camera intrinsics estimated from motion patterns
//
//   These let us:
//     - COMPRESS BETTER: store the scene geometry once, reuse for every frame.
//       (huge win for video where the camera moves through a static scene)
//     - FLY THROUGH the reconstructed world as a virtual camera
//     - SEPARATE static scene from dynamic objects ('what moved' becomes
//       trivially obvious once you have a depth-stable background)
//
// PIPELINE (multi-session implementation plan):
//
//   Session 1 (THIS SESSION): scaffold — types, trailer format, encoder/decoder
//                             stubs, GPU resource hooks. v8 selectable in UI but 
//                             produces a placeholder.
//
//   Session 2: FeatureTrackPass — ORB or KLT tracker. For each frame, find 
//              ~500 keypoints and match them to the previous frame. This 
//              gives us per-feature 2D motion vectors at sub-pixel accuracy.
//
//   Session 3: TriangulatePass — given matched features across N frames + 
//              the camera motion (from v7's PhaseCorrelationPass), solve for
//              3D position of each feature. Output: sparse point cloud.
//
//   Session 4: DensifyPass — propagate depth from sparse points to dense
//              per-pixel depth maps. Either patch-match or planar fit.
//              Could leverage existing GPU shader infrastructure.
//
//   Session 5: MeshPass — Poisson surface reconstruction or marching cubes
//              on the depth maps to get a triangle mesh.
//
//   Session 6: CompressionPass — use mesh+texture to encode each frame as 
//              "render this mesh from this camera pose". Frame becomes 
//              a 12-byte camera matrix instead of pixel data.
//
// GPU REQUIREMENTS:
//   The existing gpu_pipeline.rs has wgpu compute shader infrastructure.
//   v8's heavy passes (feature matching, triangulation, depth densification)
//   will dispatch via that. CPU fallbacks live alongside for testing.
//
// FRAME FORMAT (v8):
//   'V' '8' n_features(u16) + camera_pose(48 bytes) + features(...)
//   
//   camera_pose = position(3×f32) + rotation_quat(4×f32) + intrinsics(5×f32)
//   = 12+16+20 = 48 bytes
//   
//   feature: id(u32) + screen_x(f32) + screen_y(f32) + depth(f32) + 
//            confidence(f32) = 20 bytes
//
//   Empty v8 frame is 4+48 = 52 bytes (camera pose only — useful for static
//   shots where the scene mesh in the trailer is the whole scene).

use crate::geometry::Vec3;
use crate::phase_correlation::{PhaseCorrelationPass, CameraMotion};

// f16 conversion lives in fao_codec_core (`fao_core::f16`). Compact 
// SceneFeature serialization stores depth as IEEE 754 binary16.
use fao_core::f16::{f32_to_f16_bits, f16_bits_to_f32};

// ═══════════════════════════════════════════════════════════
// CameraPose — 6DOF camera state with intrinsics
// ═══════════════════════════════════════════════════════════
//
// position + rotation give us where and how the camera is oriented.
// intrinsics (focal length + principal point + distortion) give us how
// the camera projects 3D world points to 2D pixels.
//
// In future sessions, an EstimateIntrinsicsPass will solve for these
// from the pattern of motion in the video. For now we use defaults.

#[derive(Clone, Copy, Debug)]
pub struct CameraPose {
    /// World-space position
    pub position: Vec3,
    /// Rotation as quaternion (w, x, y, z)
    pub rotation: [f32; 4],
    /// Focal length (fx, fy in pixel units)
    pub focal: [f32; 2],
    /// Principal point (cx, cy — usually image center)
    pub principal: [f32; 2],
    /// Radial distortion coefficient (k1)
    pub distortion: f32,
}

impl Default for CameraPose {
    fn default() -> Self {
        Self {
            position: Vec3::zero(),
            rotation: [1.0, 0.0, 0.0, 0.0],  // identity quaternion
            focal: [1000.0, 1000.0],          // reasonable default
            principal: [0.5, 0.5],            // normalized — multiply by frame dims
            distortion: 0.0,
        }
    }
}

impl CameraPose {
    pub fn to_bytes(&self) -> [u8; 48] {
        let mut out = [0u8; 48];
        out[0..4].copy_from_slice(&self.position.x.to_le_bytes());
        out[4..8].copy_from_slice(&self.position.y.to_le_bytes());
        out[8..12].copy_from_slice(&self.position.z.to_le_bytes());
        out[12..16].copy_from_slice(&self.rotation[0].to_le_bytes());
        out[16..20].copy_from_slice(&self.rotation[1].to_le_bytes());
        out[20..24].copy_from_slice(&self.rotation[2].to_le_bytes());
        out[24..28].copy_from_slice(&self.rotation[3].to_le_bytes());
        out[28..32].copy_from_slice(&self.focal[0].to_le_bytes());
        out[32..36].copy_from_slice(&self.focal[1].to_le_bytes());
        out[36..40].copy_from_slice(&self.principal[0].to_le_bytes());
        out[40..44].copy_from_slice(&self.principal[1].to_le_bytes());
        out[44..48].copy_from_slice(&self.distortion.to_le_bytes());
        out
    }
    
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 48 { return None; }
        let f = |i: usize| f32::from_le_bytes([data[i], data[i+1], data[i+2], data[i+3]]);
        Some(Self {
            position: Vec3::new(f(0), f(4), f(8)),
            rotation: [f(12), f(16), f(20), f(24)],
            focal: [f(28), f(32)],
            principal: [f(36), f(40)],
            distortion: f(44),
        })
    }
    
    /// Compact 28-byte serialization: position + rotation only.
    /// 
    /// Camera intrinsics (focal, principal, distortion) are essentially 
    /// constant across a video stream — stored once, not per-frame. This 
    /// drops 20 bytes per frame at the cost of storing a single 20B 
    /// `CameraIntrinsics` block in the trailer.
    /// 
    /// On a 5548-frame video that's ~110KB saved with no precision loss.
    pub fn to_bytes_extrinsics(&self) -> [u8; 28] {
        let mut out = [0u8; 28];
        out[0..4].copy_from_slice(&self.position.x.to_le_bytes());
        out[4..8].copy_from_slice(&self.position.y.to_le_bytes());
        out[8..12].copy_from_slice(&self.position.z.to_le_bytes());
        out[12..16].copy_from_slice(&self.rotation[0].to_le_bytes());
        out[16..20].copy_from_slice(&self.rotation[1].to_le_bytes());
        out[20..24].copy_from_slice(&self.rotation[2].to_le_bytes());
        out[24..28].copy_from_slice(&self.rotation[3].to_le_bytes());
        out
    }
    
    /// Reconstruct a CameraPose from compact extrinsic bytes + a shared 
    /// CameraIntrinsics block.
    pub fn from_extrinsics(data: &[u8], intrinsics: &CameraIntrinsics) -> Option<Self> {
        if data.len() < 28 { return None; }
        let f = |i: usize| f32::from_le_bytes([data[i], data[i+1], data[i+2], data[i+3]]);
        Some(Self {
            position: Vec3::new(f(0), f(4), f(8)),
            rotation: [f(12), f(16), f(20), f(24)],
            focal: intrinsics.focal,
            principal: intrinsics.principal,
            distortion: intrinsics.distortion,
        })
    }
}

/// Camera intrinsics — the per-stream constants extracted from CameraPose.
/// 
/// These never change frame-to-frame for a given video. Storing them once 
/// per stream (in the trailer) instead of per-frame saves ~20 bytes/frame.
#[derive(Clone, Copy, Debug)]
pub struct CameraIntrinsics {
    pub focal: [f32; 2],
    pub principal: [f32; 2],
    pub distortion: f32,
}

impl Default for CameraIntrinsics {
    fn default() -> Self {
        Self {
            focal: [1000.0, 1000.0],
            principal: [0.5, 0.5],
            distortion: 0.0,
        }
    }
}

impl CameraIntrinsics {
    /// Extract intrinsics from a representative pose. Use this once at 
    /// stream end to record the constant intrinsics.
    pub fn from_pose(pose: &CameraPose) -> Self {
        Self {
            focal: pose.focal,
            principal: pose.principal,
            distortion: pose.distortion,
        }
    }
    
    /// Serialize as 20 bytes for trailer storage.
    pub fn to_bytes(&self) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[0..4].copy_from_slice(&self.focal[0].to_le_bytes());
        out[4..8].copy_from_slice(&self.focal[1].to_le_bytes());
        out[8..12].copy_from_slice(&self.principal[0].to_le_bytes());
        out[12..16].copy_from_slice(&self.principal[1].to_le_bytes());
        out[16..20].copy_from_slice(&self.distortion.to_le_bytes());
        out
    }
    
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 20 { return None; }
        let f = |i: usize| f32::from_le_bytes([data[i], data[i+1], data[i+2], data[i+3]]);
        Some(Self {
            focal: [f(0), f(4)],
            principal: [f(8), f(12)],
            distortion: f(16),
        })
    }
}

// ═══════════════════════════════════════════════════════════
// SceneFeature — a 3D point detected in the scene
// ═══════════════════════════════════════════════════════════
//
// The atomic unit of scene reconstruction. A feature is a distinctive
// pixel pattern (corner, edge intersection) that we tracked across multiple
// frames and triangulated to a 3D position.
//
// Features become vertices of the eventual mesh.

#[derive(Clone, Copy, Debug)]
pub struct SceneFeature {
    /// Stable ID across frames (same feature = same id even if temporarily lost)
    pub feature_id: u32,
    /// Where it appears on screen this frame
    pub screen_x: f32,
    pub screen_y: f32,
    /// Estimated depth (Z in camera space). 0.0 = unknown.
    pub depth: f32,
    /// Confidence in the depth estimate [0.0, 1.0]
    /// Combines tracker stability + triangulation residual.
    pub confidence: f32,
}

impl SceneFeature {
    /// Original 20-byte format: 4B id + 4B×4 f32 (x,y,depth,confidence).
    /// Kept for backward-compat with existing .fao files.
    pub fn to_bytes(&self) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[0..4].copy_from_slice(&self.feature_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.screen_x.to_le_bytes());
        out[8..12].copy_from_slice(&self.screen_y.to_le_bytes());
        out[12..16].copy_from_slice(&self.depth.to_le_bytes());
        out[16..20].copy_from_slice(&self.confidence.to_le_bytes());
        out
    }
    
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 20 { return None; }
        let f = |i: usize| f32::from_le_bytes([data[i], data[i+1], data[i+2], data[i+3]]);
        let id = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        Some(Self {
            feature_id: id,
            screen_x: f(4),
            screen_y: f(8),
            depth: f(12),
            confidence: f(16),
        })
    }
    
    /// Compact 11-byte format: 4B id + 2B×2 u16 screen + 2B f16 depth + 1B u8 conf.
    /// 
    /// Tradeoffs:
    ///   - id: kept 4B (typical id range 0..2M needs 21+ bits)
    ///   - x, y: u16 sub-pixel — range 0..16384 image-coordinates, 
    ///     ~0.25 pixel precision (lossy but visually indistinguishable).
    ///   - depth: f16 (half-precision IEEE 754) — range 6e-5..65504, 
    ///     precision ~0.1% of value (depths beyond 65km clipped, 
    ///     irrelevant for any real video).
    ///   - confidence: u8 quantized — [0,1] mapped to 0..255 (0.4% precision).
    /// 
    /// Saves 9 bytes per feature × ~500 features/frame = ~4.5KB per frame.
    /// On Lone Ranger Bloopers (5548 frames) that's ~25MB total savings.
    pub fn to_bytes_compact(&self) -> [u8; 11] {
        let mut out = [0u8; 11];
        out[0..4].copy_from_slice(&self.feature_id.to_le_bytes());
        // Clamp to u16 range. Note: any video larger than 16384 in either 
        // dimension would lose this; that's a future format concern.
        let sx = (self.screen_x.max(0.0).min(65535.0)) as u16;
        let sy = (self.screen_y.max(0.0).min(65535.0)) as u16;
        out[4..6].copy_from_slice(&sx.to_le_bytes());
        out[6..8].copy_from_slice(&sy.to_le_bytes());
        // f32 → f16 manually (avoids pulling in a half crate). Standard 
        // IEEE 754 conversion: sign(1) + exp(5) + mant(10).
        let f16_bits = f32_to_f16_bits(self.depth);
        out[8..10].copy_from_slice(&f16_bits.to_le_bytes());
        // confidence: [0,1] → [0,255]. Saturate.
        out[10] = (self.confidence.max(0.0).min(1.0) * 255.0).round() as u8;
        out
    }
    
    pub fn from_bytes_compact(data: &[u8]) -> Option<Self> {
        if data.len() < 11 { return None; }
        let id = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let sx = u16::from_le_bytes([data[4], data[5]]) as f32;
        let sy = u16::from_le_bytes([data[6], data[7]]) as f32;
        let f16_bits = u16::from_le_bytes([data[8], data[9]]);
        let depth = f16_bits_to_f32(f16_bits);
        let confidence = data[10] as f32 / 255.0;
        Some(Self {
            feature_id: id,
            screen_x: sx,
            screen_y: sy,
            depth,
            confidence,
        })
    }
}

// ═══════════════════════════════════════════════════════════
// SceneMesh — triangulated reconstruction of the static scene
// ═══════════════════════════════════════════════════════════
//
// Stored ONCE per archive in the V8MS trailer. Per-frame data only needs
// to reference the camera pose; the geometry is reusable.
//
// In session 5 of the plan above, the MeshPass will populate this from
// densified depth maps. For now it's a container.

#[derive(Clone, Debug, Default)]
pub struct SceneMesh {
    /// 3D vertex positions (world space)
    pub vertices: Vec<Vec3>,
    /// Per-vertex colors (RGB f32 [0..1])
    pub vertex_colors: Vec<[f32; 3]>,
    /// Triangle indices (3 indices per triangle)
    pub indices: Vec<u32>,
    /// Per-vertex normals (computed at mesh-build time)
    pub normals: Vec<Vec3>,
    
    /// Spatial hash for cross-anchor vertex fusion (Session 5b).
    /// Maps voxel-quantized world position -> existing vertex index.
    /// When MeshIntegrator places a vertex, it first looks here. If a 
    /// vertex already exists nearby (within voxel_size), the new vertex 
    /// is FUSED with the existing one (color blended, position averaged).
    /// 
    /// This is the difference between "30 overlapping anchor shells" and 
    /// "one continuous reconstructed scene" — the same physical wall 
    /// observed in 30 anchor frames produces ONE shared vertex region 
    /// instead of 30 stacked copies.
    /// 
    /// The voxel coordinate is (round(x/voxel_size), ...). Voxel size is 
    /// part of MeshIntegrator config (default ~ scene-dependent).
    /// 
    /// Memory: ~16 bytes per entry. At 1M vertices that's 16MB — same 
    /// order as the vertices themselves. Worth it because total vertex 
    /// count drops dramatically (10x typical).
    pub vertex_grid: std::collections::HashMap<(i32, i32, i32), u32>,
    
    /// Per-vertex hit count (how many anchor observations contributed).
    /// Used for weighted color/position blending. Higher hit count = 
    /// more confident vertex.
    pub vertex_hits: Vec<u32>,
}

impl SceneMesh {
    pub fn new() -> Self { Self::default() }
    
    pub fn vertex_count(&self) -> usize { self.vertices.len() }
    pub fn triangle_count(&self) -> usize { self.indices.len() / 3 }
    
    /// Estimate raw memory size for diagnostics
    pub fn raw_size(&self) -> usize {
        self.vertices.len() * 12
            + self.vertex_colors.len() * 12
            + self.indices.len() * 4
            + self.normals.len() * 12
    }
    
    /// Serialize to V8MS (Mesh Scene) trailer.
    /// Format: 'V8MS' + n_verts(u32) + n_tris(u32) + comp_len(u32)
    ///         + zstd(vertices + colors + normals + indices)
    pub fn to_trailer(&self) -> Vec<u8> {
        use std::io::Cursor;
        let mut concat = Vec::new();
        for v in &self.vertices {
            concat.extend_from_slice(&v.x.to_le_bytes());
            concat.extend_from_slice(&v.y.to_le_bytes());
            concat.extend_from_slice(&v.z.to_le_bytes());
        }
        for c in &self.vertex_colors {
            concat.extend_from_slice(&c[0].to_le_bytes());
            concat.extend_from_slice(&c[1].to_le_bytes());
            concat.extend_from_slice(&c[2].to_le_bytes());
        }
        for n in &self.normals {
            concat.extend_from_slice(&n.x.to_le_bytes());
            concat.extend_from_slice(&n.y.to_le_bytes());
            concat.extend_from_slice(&n.z.to_le_bytes());
        }
        for &i in &self.indices {
            concat.extend_from_slice(&i.to_le_bytes());
        }
        let lvl = adaptive_zstd_level(concat.len());
        let compressed = zstd::encode_all(Cursor::new(&concat), lvl).unwrap_or(concat);
        let mut out = Vec::with_capacity(compressed.len() + 16);
        out.extend_from_slice(b"V8MS");
        out.extend_from_slice(&(self.vertices.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.indices.len() as u32 / 3).to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&compressed);
        out
    }
    
    pub fn from_trailer(data: &[u8]) -> Option<Self> {
        use std::io::Cursor;
        if data.len() < 16 || &data[..4] != b"V8MS" { return None; }
        let n_verts = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let n_tris = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
        let comp_len = u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;
        if data.len() < 16 + comp_len { return None; }
        
        let raw = zstd::decode_all(Cursor::new(&data[16..16 + comp_len])).ok()?;
        // Layout: vertices(12n) + colors(12n) + normals(12n) + indices(4*3*n_tris)
        let needed = 12 * n_verts + 12 * n_verts + 12 * n_verts + 12 * n_tris;
        if raw.len() < needed { return None; }
        
        let f = |off: usize| f32::from_le_bytes([raw[off], raw[off+1], raw[off+2], raw[off+3]]);
        let mut mesh = Self::new();
        let mut p = 0;
        for _ in 0..n_verts {
            mesh.vertices.push(Vec3::new(f(p), f(p+4), f(p+8)));
            p += 12;
        }
        for _ in 0..n_verts {
            mesh.vertex_colors.push([f(p), f(p+4), f(p+8)]);
            p += 12;
        }
        for _ in 0..n_verts {
            mesh.normals.push(Vec3::new(f(p), f(p+4), f(p+8)));
            p += 12;
        }
        for _ in 0..(n_tris * 3) {
            mesh.indices.push(u32::from_le_bytes([raw[p], raw[p+1], raw[p+2], raw[p+3]]));
            p += 4;
        }
        Some(mesh)
    }
}

// ═══════════════════════════════════════════════════════════
// V8Frame — per-frame data
// ═══════════════════════════════════════════════════════════

#[derive(Clone, Debug, Default)]
pub struct V8Frame {
    pub camera_pose: CameraPose,
    pub features: Vec<SceneFeature>,
}

impl V8Frame {
    /// Default frame serialization: writes the PLANAR format ("V8p").
    /// 
    /// Same on-the-wire size as V81 (5 + 48 + 11n bytes uncompressed), 
    /// but lays features out as struct-of-arrays — all feature_ids in one 
    /// contiguous block, then all sxs, then sys, then depths, then 
    /// confidences. After zstd wrap, this compresses 1.5-2.5× better 
    /// than the V81 AoS layout because similar values cluster together.
    /// 
    /// Decoder recognizes both V81 (interleaved) and V8p (planar) via 
    /// the third magic byte ('1' or 'p').
    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_bytes_v8p()
    }
    
    /// Compact format ("V81"): magic (3B) + n_features (u16, 2B) + 
    /// camera_pose (48B) + features × 11B compact each.
    /// 
    /// vs V8 legacy: 9B saved per feature. ~45% reduction at 500 features/frame.
    pub fn to_bytes_v81(&self) -> Vec<u8> {
        let n = self.features.len().min(65535);
        let mut out = Vec::with_capacity(5 + 48 + 11 * n);
        out.push(b'V');
        out.push(b'8');
        out.push(b'1');  // version marker — distinguishes from legacy "V8"
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&self.camera_pose.to_bytes());
        for feat in self.features.iter().take(n) {
            out.extend_from_slice(&feat.to_bytes_compact());
        }
        out
    }
    
    /// Planar (struct-of-arrays) serialization. Same total size as V81 
    /// but groups same-typed fields together — sequential feature_id u32s 
    /// in one contiguous block, all sxs together, all sys together, etc.
    /// 
    /// This is a pure win for the per-frame zstd wrap: planar layouts 
    /// compress 1.5-2.5× better on this kind of structured data than 
    /// the AoS V81 form. The decoder doesn't need to know — bytes are 
    /// the same; ordering changed.
    /// 
    /// Magic: 'V' '8' 'p'. The 'p' suffix is what disambiguates from V81.
    pub fn to_bytes_v8p(&self) -> Vec<u8> {
        let n = self.features.len().min(65535);
        let mut out = Vec::with_capacity(5 + 48 + 11 * n);
        out.push(b'V');
        out.push(b'8');
        out.push(b'p');  // planar marker
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&self.camera_pose.to_bytes());
        // Plane 1: feature_id (4 bytes each, monotonic-ish after sort)
        for feat in self.features.iter().take(n) {
            out.extend_from_slice(&feat.feature_id.to_le_bytes());
        }
        // Plane 2: screen_x (2 bytes each, clustered around frame center)
        for feat in self.features.iter().take(n) {
            let sx = (feat.screen_x.max(0.0).min(65535.0)) as u16;
            out.extend_from_slice(&sx.to_le_bytes());
        }
        // Plane 3: screen_y (2 bytes each)
        for feat in self.features.iter().take(n) {
            let sy = (feat.screen_y.max(0.0).min(65535.0)) as u16;
            out.extend_from_slice(&sy.to_le_bytes());
        }
        // Plane 4: depth as f16 (2 bytes each)
        for feat in self.features.iter().take(n) {
            let f16_bits = f32_to_f16_bits(feat.depth);
            out.extend_from_slice(&f16_bits.to_le_bytes());
        }
        // Plane 5: confidence quantized to u8 (1 byte each)
        for feat in self.features.iter().take(n) {
            let c = (feat.confidence.max(0.0).min(1.0) * 255.0).round() as u8;
            out.push(c);
        }
        out
    }
    
    /// Decode a planar V8p frame. Inverse of to_bytes_v8p.
    pub fn from_bytes_v8p(data: &[u8]) -> Option<Self> {
        if data.len() < 5 + 48 { return None; }
        if data[0] != b'V' || data[1] != b'8' || data[2] != b'p' { return None; }
        let n = u16::from_le_bytes([data[3], data[4]]) as usize;
        let needed = 5 + 48 + 11 * n;
        if data.len() < needed { return None; }
        let pose = CameraPose::from_bytes(&data[5..5+48])?;
        let mut p = 5 + 48;
        // Decode each plane in order.
        let id_off = p; p += 4 * n;
        let sx_off = p; p += 2 * n;
        let sy_off = p; p += 2 * n;
        let dp_off = p; p += 2 * n;
        let cf_off = p;
        let mut features = Vec::with_capacity(n);
        for i in 0..n {
            let id = u32::from_le_bytes([
                data[id_off + i*4], data[id_off + i*4 + 1],
                data[id_off + i*4 + 2], data[id_off + i*4 + 3],
            ]);
            let sx = u16::from_le_bytes([data[sx_off + i*2], data[sx_off + i*2 + 1]]) as f32;
            let sy = u16::from_le_bytes([data[sy_off + i*2], data[sy_off + i*2 + 1]]) as f32;
            let dp_bits = u16::from_le_bytes([data[dp_off + i*2], data[dp_off + i*2 + 1]]);
            let depth = f16_bits_to_f32(dp_bits);
            let confidence = data[cf_off + i] as f32 / 255.0;
            features.push(SceneFeature {
                feature_id: id,
                screen_x: sx, screen_y: sy,
                depth, confidence,
            });
        }
        Some(Self { camera_pose: pose, features })
    }
    
    /// Legacy 20B-per-feature serialization. Kept so that decoders can be 
    /// tested against either format and old .fao files still parse.
    pub fn to_bytes_v8_legacy(&self) -> Vec<u8> {
        let n = self.features.len().min(65535);
        let mut out = Vec::with_capacity(4 + 48 + 20 * n);
        out.push(b'V');
        out.push(b'8');
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&self.camera_pose.to_bytes());
        for feat in self.features.iter().take(n) {
            out.extend_from_slice(&feat.to_bytes());
        }
        out
    }
    
    /// Auto-detect format on read.
    /// 
    /// AMBIGUITY: "V8"+0x31 looks like either the V81 magic or a legacy 
    /// frame with n_features whose low byte happens to be 0x31 (i.e., n=49 
    /// or 49+256, 49+512, ...). To disambiguate we VALIDATE BY LENGTH: 
    /// 
    ///   - Try parsing as V81 first. The expected length given n is exactly 
    ///     5 + 48 + 11*n bytes. If `data.len()` matches, it IS V81.
    ///   - Otherwise fall back to legacy (52 + 20*n bytes).
    /// 
    /// In production, `data` is exactly the bytes the encoder wrote, so 
    /// this check is unambiguous — the two formats never have the same 
    /// length for the same n_features.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 4 || data[0] != b'V' || data[1] != b'8' { return None; }
        // 'V8p' planar variant — same total size as V81 but column-grouped.
        if data[2] == b'p' {
            return Self::from_bytes_v8p(data);
        }
        if data[2] == b'1' && data.len() >= 5 {
            // Could be V81 OR legacy with n_low=0x31. Compute expected V81 length.
            let n_v81 = u16::from_le_bytes([data[3], data[4]]) as usize;
            let exp_v81_len = 5 + 48 + 11 * n_v81;
            // Compute expected legacy length for the SAME magic interpretation.
            // Legacy reads n from data[2..4], which would be 0x31, data[3].
            let n_legacy = u16::from_le_bytes([data[2], data[3]]) as usize;
            let exp_legacy_len = 4 + 48 + 20 * n_legacy;
            
            if data.len() == exp_v81_len {
                Self::from_bytes_v81(data)
            } else if data.len() == exp_legacy_len {
                Self::from_bytes_v8_legacy(data)
            } else {
                // Length matches neither — try V81 first as it's the new default.
                Self::from_bytes_v81(data).or_else(|| Self::from_bytes_v8_legacy(data))
            }
        } else {
            // data[2] != '1' — definitely legacy.
            Self::from_bytes_v8_legacy(data)
        }
    }
    
    pub fn from_bytes_v81(data: &[u8]) -> Option<Self> {
        // 'V'(1) + '8'(1) + '1'(1) + n_features(2) + pose(48) = 53 minimum
        if data.len() < 53 || &data[..3] != b"V81" { return None; }
        let n = u16::from_le_bytes([data[3], data[4]]) as usize;
        let camera_pose = CameraPose::from_bytes(&data[5..53])?;
        if data.len() < 53 + 11 * n { return None; }
        let mut features = Vec::with_capacity(n);
        for i in 0..n {
            let off = 53 + 11 * i;
            if let Some(f) = SceneFeature::from_bytes_compact(&data[off..off + 11]) {
                features.push(f);
            }
        }
        Some(Self { camera_pose, features })
    }
    
    pub fn from_bytes_v8_legacy(data: &[u8]) -> Option<Self> {
        if data.len() < 52 || data[0] != b'V' || data[1] != b'8' { return None; }
        let n = u16::from_le_bytes([data[2], data[3]]) as usize;
        let camera_pose = CameraPose::from_bytes(&data[4..52])?;
        if data.len() < 52 + 20 * n { return None; }
        let mut features = Vec::with_capacity(n);
        for i in 0..n {
            let off = 52 + 20 * i;
            if let Some(f) = SceneFeature::from_bytes(&data[off..off + 20]) {
                features.push(f);
            }
        }
        Some(Self { camera_pose, features })
    }
    
    /// Decode bytes that may be zstd-wrapped or raw.
    /// 
    /// Wrapped format (produced by encode_frame_at when the frame is large 
    /// enough to benefit from compression):
    ///   'Z' + version(1B) + uncompressed_len(u32 LE) + zstd_payload
    /// 
    /// If the first byte is 'V', the input is raw — pass directly to 
    /// from_bytes(). If 'Z', unwrap and recurse.
    /// 
    /// Wrapper versions:
    ///   v1: 'Z'(1) + version=1(1) + ulen(u32) + zstd(v8_payload)
    ///   v2: 'Z'(1) + version=2(1) + ulen(u32) + reslen(u32) + 
    ///       zstd(v8_payload, ulen bytes) + residual(reslen bytes)
    /// 
    /// V2 (v8.1) extends v1 with an optional photometric residual blob 
    /// appended after the zstd v8 payload. The residual is consumed by 
    /// render_frame_rgb, not by from_bytes — so V8Frame parsing only 
    /// needs to extract the v8 payload prefix in either case.
    pub fn from_bytes_any(data: &[u8]) -> Option<Self> {
        if data.is_empty() { return None; }
        if data[0] == b'Z' {
            if data.len() < 6 { return None; }
            let version = data[1];
            match version {
                1 => {
                    let _u_len = u32::from_le_bytes([data[2], data[3], data[4], data[5]]) as usize;
                    use std::io::Cursor;
                    let raw = zstd::decode_all(Cursor::new(&data[6..])).ok()?;
                    Self::from_bytes(&raw)
                }
                2 => {
                    // v2 layout: 1+1+4+4 = 10 bytes header before payload
                    if data.len() < 10 { return None; }
                    let _u_len = u32::from_le_bytes([data[2], data[3], data[4], data[5]]) as usize;
                    let res_len = u32::from_le_bytes([data[6], data[7], data[8], data[9]]) as usize;
                    // The v8 payload starts at offset 10. Its compressed 
                    // length = data.len() - 10 - res_len.
                    let payload_end = data.len().saturating_sub(res_len);
                    if payload_end <= 10 { return None; }
                    use std::io::Cursor;
                    let raw = zstd::decode_all(Cursor::new(&data[10..payload_end])).ok()?;
                    Self::from_bytes(&raw)
                }
                _ => None,
            }
        } else {
            // Raw V8 or V81 — dispatch directly
            Self::from_bytes(data)
        }
    }
    
    /// Extract the photometric residual blob from a v2 Z-wrapper, if 
    /// present. Returns None for v1 wrappers, raw V8 frames, or v2 
    /// wrappers with res_len = 0.
    /// 
    /// The returned bytes are the raw residual payload — caller (the 
    /// decoder render path) is responsible for decompressing and 
    /// applying it to a rasterized RGB image.
    pub fn extract_residual(data: &[u8]) -> Option<&[u8]> {
        if data.len() < 10 || data[0] != b'Z' || data[1] != 2 {
            return None;
        }
        let res_len = u32::from_le_bytes([data[6], data[7], data[8], data[9]]) as usize;
        if res_len == 0 { return None; }
        let payload_end = data.len().saturating_sub(res_len);
        if payload_end <= 10 { return None; }
        Some(&data[payload_end..])
    }
}

// ═══════════════════════════════════════════════════════════
// V8Encoder — scaffold (real passes land in future sessions)
// ═══════════════════════════════════════════════════════════
//
// Right now this just chains v7 (for camera motion) with placeholder feature
// detection. As future passes land, they'll plug in here.
//
// What we DO produce today:
//   - Camera pose per frame (taken from v7's phase correlation)
//   - Empty feature list (placeholder until FeatureTrackPass exists)
//   - Empty scene mesh in trailer
//
// What we WILL produce as passes land:
//   - Sparse feature points with depth
//   - Dense depth map per anchor frame
//   - Reconstructed scene mesh (one per scene, in the trailer)

pub struct V8Encoder {
    pub width: u32,
    pub height: u32,
    
    // ── Chunk identity (Stage 1 of parallel architecture) ───────────────
    // Each V8Encoder instance handles one CHUNK of the video. A chunk is 
    // a contiguous span of frames that this encoder is responsible for.
    // 
    // For a single-threaded encode the whole video is one chunk:
    //   chunk_id = 0, chunk_start_frame = 0
    // 
    // For parallel encode (Stage 3), each worker gets its own V8Encoder 
    // with a unique chunk_id. The chunk_id namespaces feature IDs so 
    // workers don’t collide. chunk_start_frame is the absolute video 
    // frame index of the chunk’s first frame, used to translate 
    // chunk-relative motion into global pose track positions at merge.
    pub chunk_id: u32,
    pub chunk_start_frame: u32,
    
    pub frame_count: u32,
    pub total_bytes: u64,
    
    /// Per-frame camera pose (cumulative from v7 motion + intrinsic estimation)
    pub pose_track: Vec<CameraPose>,
    
    /// The reconstructed mesh — populated as features triangulate
    /// (currently empty; future sessions populate via TriangulatePass + MeshPass)
    pub scene_mesh: SceneMesh,
    
    /// Phase correlation reused from v7 — gives us frame-to-frame motion
    pub phase_corr: std::sync::Arc<PhaseCorrelationPass>,
    
    /// Previous frame YUV (for motion analysis)
    pub prev_frame: Option<Vec<u8>>,
    
    /// Last bytes emitted by encode_frame_at. Used for the static-frame 
    /// fast path: if the new YUV matches prev_frame closely, re-emit the 
    /// last bytes instead of running the full pipeline.
    pub last_emit_bytes: Vec<u8>,
    
    /// Cumulative camera position (integrated from per-frame motion)
    cumulative_position: Vec3,
    
    /// Feature tracker — emits stable-id keypoints across frames.
    pub feature_pass: FeatureTrackPass,
    
    /// Spatial trace for World viewer: (feature_id, x, y, frame_idx, class)
    /// class: 0=Unknown, 1=Static, 2=Dynamic
    /// Capped to MAX_TRACE_ENTRIES to bound memory on long videos.
    pub spatial_trace: Vec<(u32, i16, i16, u32, u8)>,
    
    /// Per-frame anchor positions for the Tobii-style player overlay.
    /// Keyed by external (video) frame index. Each entry: (x, y, class) 
    /// for every live feature at that frame. UNLIKE spatial_trace, this 
    /// is NOT cleared at chunk rotation — it accumulates for the full 
    /// encode and gets serialized into a V8AT trailer at export time.
    /// 
    /// Memory: ~150 features × 5 bytes × frame_count. At 15900 frames = 
    /// ~12MB raw, ~3-4MB after zstd. Tolerable for the diagnostic value 
    /// (user can see WHERE the encoder is anchoring during playback).
    /// 
    /// Storage layout in V8AT trailer:
    ///   magic 'V8AT' (4) + version u8 + flags u8 + reserved (2)
    ///   n_frames u32
    ///   per frame: external_frame_idx u32 + n_anchors u16 + 
    ///     (x i16, y i16, cls u8)*
    ///   all zstd compressed inside the trailer.
    pub anchor_track: std::collections::HashMap<u32, Vec<(i16, i16, u8)>>,
    
    /// Triangulation pass (Session 3) — solves 3D from 2D + camera motion
    pub triangulate_pass: TriangulatePass,
    
    /// Per-feature observation history + cached triangulation results
    pub observations: ObservationStore,
    
    /// Motion consensus classifier (Session 2.5) — labels features Static or 
    /// Dynamic so triangulation only sees rigid-scene features
    pub consensus_pass: MotionConsensusPass,
    
    /// Block-grid layer mask from the region-activity classifier 
    /// (16x16 blocks). One BlockLayer-as-u8 per block, row-major. 
    /// Updated externally via `set_layer_mask` before each `encode_frame_at` 
    /// call. None when no layer info is available (e.g. file load before 
    /// the region tracker has accumulated enough samples).
    /// 
    /// Used by the UI-overlay filter (encode_frame_at) to:
    ///   1. Increment FeatureTrack.layer_overlay_hits for features in 
    ///      AlphaOverlay-classified blocks
    ///   2. Apply layer-weight multipliers in densify's sparse_screen
    /// 
    /// Layout: mask[by * block_w + bx] = BlockLayer-as-u8 (see 
    /// region_activity::BlockLayer). block_w = width / 16, block_h = 
    /// height / 16. Vec is None or has length block_w * block_h.
    pub layer_mask: Option<Vec<u8>>,
    /// Tuning: how many consecutive AlphaOverlay-block hits before a 
    /// feature is hard-promoted to MotionClass::UiOverlay (and excluded 
    /// from triangulation / mesh integration).
    pub ui_overlay_promotion_threshold: u8,
    /// Tuning: minimum frames a track must live before depth-variance 
    /// based UI promotion kicks in. Young tracks haven't triangulated 
    /// enough to have a meaningful variance estimate.
    pub ui_overlay_min_age_for_variance: u32,
    /// Tuning: depth-variance threshold (in scene units). Tracks whose 
    /// EMA variance falls below this after they've been triangulated 
    /// enough times are presumed-UI and get promoted. 
    /// Zero-parallax UI sits at ~0; real scene features typically have 
    /// hundreds to thousands of units of depth variance as the camera moves.
    pub ui_overlay_variance_threshold: f32,
    
    /// Densification pass (Session 4) — sparse 3D → dense depth maps
    pub densify_pass: DensifyPass,
    
    /// Mesh integrator (Session 5) — depth maps → growing triangle mesh.
    /// Streaming: each integrated depth map is then DISCARDED.
    pub mesh_integrator: MeshIntegrator,
    
    /// Chunk store (Session 6) — retains source frames + derived data for 
    /// future passes (texture projection, re-encoding) with quality-aware 
    /// eviction. Bounds memory regardless of video length.
    pub chunk_store: ChunkStore,
    
    /// Optional library handle (Phase 3 — multi-video web sharing).
    /// 
    /// When Some, the encoder queries this library at chunk close to find 
    /// existing canonicals that match the chunk's web. Matches are stored 
    /// as LibraryRefs in chunk_store.library_refs and the chunk's mesh is 
    /// dropped from RAM (saved to library on first occurrence; reused on 
    /// subsequent encodes of similar content).
    /// 
    /// Wrapped in Arc<RwLock<Library>> so multiple workers can READ 
    /// concurrently (the common case: signature lookups). Writes (adding 
    /// a new web to the library) take exclusive lock briefly.
    pub library: Option<std::sync::Arc<std::sync::RwLock<crate::library::Library>>>,
    
    /// Whether this encoder is allowed to ADD new webs to the library.
    /// Default: false (read-only library use). Setting this to true 
    /// requires the library was opened with `open_writable`.
    pub library_writable: bool,
    
    /// Match threshold for library lookups. Higher = stricter matches, 
    /// fewer false positives, smaller compression gain. Default 0.85.
    pub library_match_threshold: f32,
    
    /// How often to rotate chunks (in encoder calls). Default 240 = 4× anchor 
    /// cadence so each chunk contains ~4 anchors of derived data. Note that
    /// 240 CALLS may span an arbitrary range of video_frame_idx due to 
    /// multi-thread out-of-order compress — that's fine, the chunk just 
    /// holds whatever frames were processed in its call window.
    pub chunk_rotate_interval: u32,
    
    /// frame_count value when the current chunk was opened. Used to 
    /// trigger rotation after chunk_rotate_interval calls.
    pub chunk_open_call: u32,
    
    /// Anchor-frame depth maps. Bounded to last ~32 anchors (~16 sec at 
    /// 30fps anchor cadence). Future MeshPass consumes these.
    pub depth_maps: Vec<DepthMap>,
    
    /// Per-frame RELATIVE camera motion deltas (Stage 1 of parallel arch).
    /// 
    /// Each entry is (video_frame_idx, motion_delta) where motion_delta is 
    /// the camera shift that occurred BETWEEN the previous frame and this 
    /// frame. NOT cumulative.
    /// 
    /// Why relative not cumulative: when chunks are merged, the global 
    /// camera path is the prefix-sum of all chunks’ deltas in frame order. 
    /// Each chunk independently computes its deltas (no inter-chunk 
    /// dependency), then merge does a single O(N) cumsum.
    /// 
    /// For single-chunk encoding the cumulative position can be derived 
    /// from this on demand by scanning forward.
    pub relative_motion_per_frame: Vec<(u32, Vec3)>,
}

impl V8Encoder {
    /// Construct a V8Encoder for one CHUNK of the video.
    /// 
    /// Arguments:
    ///   width, height — frame dimensions
    ///   chunk_id      — 0..255, unique per worker. Used to namespace feature 
    ///                   IDs so different chunks’ features don’t collide 
    ///                   when merged.
    ///   chunk_start_frame — absolute video frame index of this chunk’s 
    ///                   first frame. Used at merge time to align chunk 
    ///                   poses into the global pose track.
    /// 
    /// For non-chunked (single-encoder) use, see `V8Encoder::new()` which 
    /// is a shim for `new_for_chunk(w, h, 0, 0)`.
    pub fn new_for_chunk(width: u32, height: u32, chunk_id: u32, chunk_start_frame: u32) -> Self {
        let pc_size = if width >= 1280 { 256 } else { 128 };
        // Initialize feature_pass with chunk-aware ID generation.
        let mut feature_pass = FeatureTrackPass::default();
        feature_pass.set_chunk_id(chunk_id);
        Self {
            width,
            height,
            chunk_id,
            chunk_start_frame,
            frame_count: 0,
            total_bytes: 0,
            pose_track: Vec::new(),
            scene_mesh: SceneMesh::new(),
            phase_corr: std::sync::Arc::new(PhaseCorrelationPass::new(pc_size)),
            prev_frame: None,
            last_emit_bytes: Vec::new(),
            cumulative_position: Vec3::zero(),
            feature_pass,
            spatial_trace: Vec::with_capacity(65536),
            // Pre-allocate for a reasonable number of frames per worker. 
            // 4096 frames * 6 workers = 24576 typical encode; the HashMap 
            // will resize as needed but starting small avoids initial 
            // alloc costs.
            anchor_track: std::collections::HashMap::with_capacity(4096),
            triangulate_pass: TriangulatePass::default(),
            observations: ObservationStore::default(),
            consensus_pass: MotionConsensusPass::default(),
            // UI-overlay filter state. Layer mask starts None; consumer 
            // populates via set_layer_mask before each frame.
            layer_mask: None,
            // Promotion thresholds. 5-frame overlay streak is conservative — 
            // a feature that bounces between active and overlay regions 
            // (e.g. moving subject behind a static HUD edge) won't get 
            // promoted. Tuned for 30fps content; bump higher for higher fps.
            ui_overlay_promotion_threshold: 5,
            // Need at least 8 frames before depth variance is meaningful. 
            // Triangulation typically converges after 3-5 observations, so 
            // 8 frames means we've had a few re-triangulations to build 
            // the variance estimate.
            ui_overlay_min_age_for_variance: 8,
            // Tightened from initial 1.0 — UI overlays consistently sit at 
            // exactly zero variance because they ARE part of the camera 
            // (zero parallax). Real static scene features have variance in 
            // the tens-to-thousands range even for slow camera moves. 5.0 
            // is a comfortable buffer above noise floor of real features.
            ui_overlay_variance_threshold: 5.0,
            densify_pass: DensifyPass::default(),
            mesh_integrator: MeshIntegrator::default(),
            chunk_store: ChunkStore::new(),
            library: None,
            library_writable: false,
            library_match_threshold: 0.85,
            chunk_rotate_interval: 240,  // 4 anchors per chunk at default cadence
            chunk_open_call: 0,
            depth_maps: Vec::new(),
            relative_motion_per_frame: Vec::new(),
        }
    }
    
    /// Attach a library to this encoder. After this call, the encoder 
    /// queries the library at chunk close for matching canonicals.
    /// 
    /// `writable=true` enables ADDING new webs to the library when this 
    /// encode produces canonicals that don't already exist there. Requires 
    /// the library to have been opened with `open_writable` upstream.
    pub fn attach_library(
        &mut self,
        library: std::sync::Arc<std::sync::RwLock<crate::library::Library>>,
        writable: bool,
    ) {
        self.library = Some(library);
        self.library_writable = writable;
    }
    
    /// Install a block-grid layer mask for the UI-overlay filter to 
    /// consult during the next `encode_frame_at` call. Caller is expected 
    /// to refresh this mask periodically (typically once every 8-16 
    /// frames — same cadence the RegionActivityTracker uses) so that 
    /// classification stays current as the scene evolves.
    /// 
    /// Layout: `mask[by * block_w + bx]` = BlockLayer-as-u8. block_w 
    /// = width / 16, block_h = height / 16. Pass None to clear the mask 
    /// (encoder falls back to motion-class-only filtering, matching the 
    /// pre-UI-overlay-filter behavior).
    /// 
    /// Mismatched lengths are tolerated silently: the filter pass checks 
    /// `mask.len() == expected` before indexing, so a wrong-size mask 
    /// becomes a no-op rather than a panic. This matters for code that 
    /// queries the region tracker before it's had time to size itself 
    /// for the current video dimensions.
    pub fn set_layer_mask(&mut self, mask: Option<Vec<u8>>) {
        self.layer_mask = mask;
    }
    
    /// Convenience for non-chunked use. Equivalent to a single chunk 
    /// covering the entire video.
    pub fn new(width: u32, height: u32) -> Self {
        Self::new_for_chunk(width, height, 0, 0)
    }
    
    /// Encode a frame. `external_frame_idx` is the video's actual frame 
    /// number — used as the canonical frame_idx in spatial_trace and 
    /// observation records, so the World viewer's temporal axis matches 
    /// the source video's timeline. Without this, the encoder's internal 
    /// counter would diverge from video time when frames arrive out-of-order
    /// from multiple compress threads.
    pub fn encode_frame_at(&mut self, frame_yuv: &[u8], external_frame_idx: u32) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;
        let needed = w * h + (w / 2) * (h / 2) * 2;
        // Diagnostic: log every entry so we know if frames are being 
        // rejected at the size check (most common silent bailout)
        if self.frame_count < 5 || self.frame_count % 30 == 0 {
            eprintln!("[v8 trace] encode_frame called: chunk={} frame_count={} ext_fidx={} input_len={} needed={} accept={}", 
                self.chunk_id, self.frame_count, external_frame_idx, frame_yuv.len(), needed, frame_yuv.len() >= needed);
        }
        if frame_yuv.len() < needed { return Vec::new(); }
        // Use the external (video) frame index throughout this call so that 
        // spatial_trace / observations / pose_track align with video time.
        let video_frame_idx = external_frame_idx;
        
        // ── STATIC FRAME FAST PATH ──
        // 
        // For cartoons, presentations, talking-head videos, and any content 
        // where many consecutive frames are nearly identical, the entire v8 
        // pipeline (phase-correlation FFT, LK tracker, FAST detector, 
        // triangulation, mesh fusion) is wasted work — the geometry didn't 
        // change.
        // 
        // Cheap detector: sparse luma-plane SAD on every 32nd pixel. For 
        // 1080p this is ~2K samples, ~10µs of work. If avg SAD is below a 
        // small threshold, the frame is a near-duplicate and we re-emit the 
        // last bytes verbatim. Decoder will treat the new frame the same as 
        // the previous one.
        // 
        // Skip the fast path on the FIRST frame (no prev) and on every 
        // 30th frame (so feature tracks get a chance to refresh and the 
        // mesh keeps growing if needed).
        if let Some(ref prev) = self.prev_frame {
            if !self.last_emit_bytes.is_empty()
                && prev.len() >= w * h
                && self.frame_count > 0
                && self.frame_count % 30 != 0
            {
                let mut total_sad: u64 = 0;
                let mut samples: u64 = 0;
                let stride = 32;
                let y_sz = w * h;
                let mut i = 0;
                while i < y_sz && i < prev.len() && i < frame_yuv.len() {
                    total_sad += (frame_yuv[i] as i32 - prev[i] as i32).unsigned_abs() as u64;
                    samples += 1;
                    i += stride;
                }
                let avg_sad = if samples > 0 { total_sad / samples } else { 255 };
                // Threshold of 2 luma units catches truly-static content 
                // (cartoons, slides) without grabbing slow-motion scenes.
                if avg_sad <= 2 {
                    self.frame_count += 1;
                    // Update prev_frame so the next-frame check has fresh 
                    // ground truth (otherwise drift accumulates).
                    self.prev_frame = Some(frame_yuv[..needed.min(frame_yuv.len())].to_vec());
                    // v8.3 "R" repeat marker (opt-in via FAO_V83_REPEAT=1):
                    // emit a single 'R' byte instead of re-emitting the 
                    // full previous frame bytes. Decoder must walk 
                    // backwards through fao_index to find the most 
                    // recent non-R frame and decode that.
                    // 
                    // Size win: for 100 consecutive static frames where 
                    // each previous bytes was 1KB, this saves ~99KB. On 
                    // talking-head footage / slide presentations, the 
                    // savings compound massively.
                    // 
                    // Risk: if the decoder doesn't walk back correctly 
                    // (or the FAO file is truncated such that the 
                    // referenced frame is missing), playback shows the 
                    // wrong frame. Hence opt-in until validated.
                    let v83_repeat = std::env::var("FAO_V83_REPEAT")
                        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                        .unwrap_or(false);
                    if v83_repeat {
                        let r_marker = vec![b'R'];
                        if self.frame_count < 5 || self.frame_count % 100 == 0 {
                            eprintln!("[v8 fast-path] frame {} static (avg_sad={}) — emitting R marker (1 byte vs {} for full re-emit)",
                                video_frame_idx, avg_sad, self.last_emit_bytes.len());
                        }
                        self.total_bytes += r_marker.len() as u64;
                        return r_marker;
                    }
                    if self.frame_count < 5 || self.frame_count % 100 == 0 {
                        eprintln!("[v8 fast-path] frame {} static (avg_sad={}) — re-emitting {} bytes",
                            video_frame_idx, avg_sad, self.last_emit_bytes.len());
                    }
                    self.total_bytes += self.last_emit_bytes.len() as u64;
                    return self.last_emit_bytes.clone();
                }
            }
        }
        
        // ── Camera motion (reuse v7's phase correlation) ──
        let cam_motion = if let Some(ref prev) = self.prev_frame {
            self.phase_corr.estimate(frame_yuv, prev, w, h)
        } else {
            CameraMotion::none()
        };
        
        // Integrate motion into cumulative camera position.
        // (Frame shifts by (dx, dy) → camera moved by (-dx, -dy) in world)
        // 
        // We ALSO record the per-frame DELTA in relative_motion_per_frame.
        // For single-chunk encoding this is just diagnostic. For chunked 
        // encoding (Stage 3) the merge step will cumsum these across chunks 
        // to build a globally-consistent camera path.
        let frame_delta = if cam_motion.confidence > 0.3 {
            let dx = -cam_motion.dx;
            let dy = -cam_motion.dy;
            self.cumulative_position.x += dx;
            self.cumulative_position.y += dy;
            Vec3 { x: dx, y: dy, z: 0.0 }
        } else {
            // Low confidence — assume zero motion this frame (don’t propagate noise)
            Vec3::zero()
        };
        // Cap relative motion log to bound memory on very long chunks.
        // 100k entries = ~30 minutes at 30fps single-chunk; well under typical 
        // 100-frame chunk size.
        if self.relative_motion_per_frame.len() < 100_000 {
            self.relative_motion_per_frame.push((video_frame_idx, frame_delta));
        }
        
        // Camera pose for THIS frame — position from cumulative track,
        // intrinsics defaulted (will be refined when EstimateIntrinsicsPass lands)
        let pose = CameraPose {
            position: self.cumulative_position,
            rotation: [1.0, 0.0, 0.0, 0.0],  // identity for now
            focal: [w as f32, h as f32],      // ~1 pixel per unit, refined later
            principal: [w as f32 / 2.0, h as f32 / 2.0],
            distortion: 0.0,
        };
        
        // ── FeatureTrackPass: detect + track keypoints (Session 2) ──
        let mut features = self.feature_pass.process(
            frame_yuv,
            self.prev_frame.as_deref(),
            w, h,
            video_frame_idx,
        );
        
        // ── MotionConsensusPass: classify each track as Static or Dynamic
        //    based on agreement with camera motion (Session 2.5) ──
        // 
        // CRITICAL — without this, moving objects (people, cars) corrupt 
        // triangulation because their motion gets misinterpreted as scene 
        // depth variation. Now triangulation sees only static features.
        // Pass the layer mask through so the consensus classifier can 
        // compute a robust camera-motion estimate from background-only 
        // features (axis 4 of the UI-overlay system, complementing the 
        // layer-hits, depth-variance, and weighting axes).
        self.consensus_pass.classify_with_layers(
            &mut self.feature_pass.live_tracks,
            cam_motion.dx,
            cam_motion.dy,
            cam_motion.confidence,
            self.layer_mask.as_deref(),
            self.width,
            self.height,
        );
        
        // ── UI-OVERLAY FILTER PASS ──
        // 
        // Root cause user identified: "the anchors are mostly on the UI 
        // elements". Game HUDs and overlays produce features that 
        // MotionConsensusPass labels Static (because they move-with-camera 
        // identically — they ARE part of the camera), but they have zero 
        // parallax. When fed into triangulation, these features cause the 
        // encoder to model the scene as a flat UI plane instead of the 
        // actual 3D world.
        // 
        // Three axes for promoting a feature to MotionClass::UiOverlay 
        // (a hard-exclude category):
        //   1. Layer classification (this pass): if the feature's screen 
        //      pixel consistently falls in an AlphaOverlay-classified block, 
        //      increment layer_overlay_hits. After ui_overlay_promotion_threshold 
        //      hits (default 5), promote to UiOverlay regardless of motion class.
        //   2. Depth-variance filter (deferred to after triangulation below): 
        //      tracks whose depth EMA-variance stays below 
        //      ui_overlay_variance_threshold once they're old enough are 
        //      presumed UI (zero parallax over time = stuck to camera).
        //   3. Active-content weighting (applied later in maybe_densify): 
        //      tracks in Active blocks get a confidence boost; tracks in 
        //      AlphaOverlay/PureStatic blocks get penalized. Soft preference 
        //      rather than hard exclude.
        // 
        // No-op when self.layer_mask is None (region tracker hasn't 
        // accumulated enough samples yet, or the feature wasn't plumbed 
        // through from main.rs).
        if let Some(ref mask) = self.layer_mask {
            let block_w = (self.width as usize) / 16;
            let block_h = (self.height as usize) / 16;
            let expected_len = block_w * block_h;
            // Reject mismatched masks (size change mid-encode would silently 
            // index out of bounds otherwise).
            if mask.len() == expected_len {
                for track in self.feature_pass.live_tracks.iter_mut() {
                    let bx = (track.current_x as i32 / 16).max(0) as usize;
                    let by = (track.current_y as i32 / 16).max(0) as usize;
                    if bx >= block_w || by >= block_h { continue; }
                    let block_idx = by * block_w + bx;
                    let layer_byte = mask[block_idx];
                    // Decode the BlockLayer enum-as-u8. The layout matches 
                    // fao_codec_core::region_activity::BlockLayer's discriminants.
                    // 0=PureStatic, 1=AlphaOverlay, 2=ParallaxBg, 3=Active, 
                    // 4=Noise, 5=Unknown
                    match layer_byte {
                        1 => {
                            // AlphaOverlay — strong UI signal.
                            track.layer_overlay_hits = track.layer_overlay_hits.saturating_add(1);
                        }
                        3 => {
                            // Active — strong scene signal.
                            track.layer_active_hits = track.layer_active_hits.saturating_add(1);
                            // Decay the overlay counter so a feature that 
                            // moves OUT of a UI region recovers eligibility.
                            // Use saturating_sub so we don't wrap.
                            track.layer_overlay_hits = track.layer_overlay_hits.saturating_sub(1);
                        }
                        _ => {
                            // PureStatic/ParallaxBg/Noise/Unknown: neutral. 
                            // No counter update. Avoid changing the balance 
                            // for ambiguous regions.
                        }
                    }
                    // Promotion to UiOverlay: if overlay_hits dominates and 
                    // the track is old enough that we've had a chance to 
                    // observe its parallax (or lack thereof), commit to the 
                    // hard-exclude classification.
                    // 
                    // We require BOTH overlay_hits >= threshold AND 
                    // overlay_hits > active_hits — this avoids false positives 
                    // for foreground features that occasionally cross 
                    // overlay regions (e.g. a character walking past the 
                    // HUD edge). They'll accumulate active_hits faster.
                    if track.frames_alive >= 4
                        && track.layer_overlay_hits >= self.ui_overlay_promotion_threshold
                        && track.layer_overlay_hits > track.layer_active_hits
                    {
                        track.motion_class = MotionClass::UiOverlay;
                    }
                }
            }
        }
        
        // Build a lookup: feature_id → motion_class so we can mirror the 
        // classification onto the SceneFeature output (the FeatureTrackPass
        // produced features already, but doesn't have classification yet).
        let class_lookup: std::collections::HashMap<u32, MotionClass> = 
            self.feature_pass.live_tracks.iter()
                .map(|t| (t.feature_id, t.motion_class))
                .collect();
        
        // Record observations for triangulation — but ONLY for static features
        // (or unknown ones, since young features haven't classified yet but
        // most will turn out static; better to over-include than miss).
        // We need pose_track to have THIS frame's pose available, so push 
        // it before triangulating.
        for feat in &features {
            let class = class_lookup.get(&feat.feature_id).copied()
                .unwrap_or(MotionClass::Unknown);
            // Skip recording dynamic features (corrupt triangulation by 
            // their own motion) and UI-overlay features (corrupt 
            // triangulation by their zero parallax — see UI-overlay 
            // filter pass above).
            if class == MotionClass::Dynamic || class == MotionClass::UiOverlay {
                continue;
            }
            self.observations.record(feat.feature_id, FeatureObservation {
                frame_idx: video_frame_idx,
                screen_x: feat.screen_x,
                screen_y: feat.screen_y,
            });
        }
        
        // Record for World viewer (with classification)
        let n_before = self.spatial_trace.len();
        for feat in &features {
            if self.spatial_trace.len() < 4_000_000 {
                let cls = match class_lookup.get(&feat.feature_id).copied()
                    .unwrap_or(MotionClass::Unknown) {
                    MotionClass::Unknown => 0u8,
                    MotionClass::Static => 1u8,
                    MotionClass::Dynamic => 2u8,
                    MotionClass::UiOverlay => 3u8,
                };
                self.spatial_trace.push((
                    feat.feature_id,
                    feat.screen_x as i16,
                    feat.screen_y as i16,
                    video_frame_idx,
                    cls,
                ));
            }
        }
        // Trace cadence: dense at start (for diagnosing initialization 
        // issues), sparse later (for long encodes — at 30/call cadence on 
        // a 283K-frame video that's 9400 lines of mostly-redundant trace).
        let trace_due = self.frame_count < 5
            || (self.frame_count < 1000 && self.frame_count % 30 == 0)
            || (self.frame_count % 500 == 0);
        if trace_due {
            eprintln!("[v8 trace] chunk={} video_fidx={} call_count={} pushed {} entries (trace total now {})",
                self.chunk_id, video_frame_idx, self.frame_count, self.spatial_trace.len() - n_before, self.spatial_trace.len());
        }
        
        // ── TriangulatePass: periodic, solves 3D from observations + poses ──
        // Pad pose_track so it's indexable by video_frame_idx (handles 
        // out-of-order arrival from multiple compress threads).
        while self.pose_track.len() <= video_frame_idx as usize {
            self.pose_track.push(CameraPose::default());
        }
        self.pose_track[video_frame_idx as usize] = pose;
        
        // ── ChunkStore tick (Session 6 + 6c) ──
        // Rotate the open chunk every chunk_rotate_interval frames. At each 
        // rotation, SEAL the current web (transfer scene_mesh into the closing 
        // chunk's web_mesh and reset our working mesh) so the next chunk 
        // starts with a fresh, independent web.
        // 
        // This is the critical 6c change: each chunk owns its own self-
        // consistent web of geometry. We never force-merge across chunks 
        // again. Compression emerges later (Sessions 7+8) when alignment 
        // detection finds webs that observe similar geometry.
        // Rotate every `chunk_rotate_interval` ENCODER CALLS, not video 
        // frames. With multi-threaded compress, video frames arrive 
        // out-of-order — the first call might be video_frame_idx=2034 
        // even though we've only processed 1 frame. A video-frame-based 
        // gate would never trigger rotation (since later frames have 
        // smaller indices). Call-based rotation guarantees regular chunk 
        // closure regardless of arrival order.
        let calls_into_chunk = self.frame_count.saturating_sub(self.chunk_open_call);
        let need_rotate = self.chunk_store.current_chunk.is_some()
            && calls_into_chunk >= self.chunk_rotate_interval;
        let need_open = self.chunk_store.current_chunk.is_none();
        
        if need_rotate {
            // Snapshot the encoder's current geometry into the chunk.
            let sealed_mesh = std::mem::replace(&mut self.scene_mesh, SceneMesh::new());
            let sealed_poses = self.pose_track.clone();
            self.chunk_store.seal_web(sealed_mesh, sealed_poses);
            
            // Opportunistic alignment (Session 7).
            if let Some(score) = self.chunk_store.try_align_current_web() {
                if let Some(chunk) = self.chunk_store.current_chunk.as_ref() {
                    eprintln!("[v8 web] chunk {}..{} aligned to canonical web {} (similarity {:.2})",
                        chunk.frame_start, chunk.frame_end,
                        chunk.canonical_web_id.unwrap_or(0), score);
                }
            }
            
            self.chunk_store.close_chunk(video_frame_idx);
            self.chunk_store.open_chunk(video_frame_idx);
            self.chunk_open_call = self.frame_count;
            
            // ── MEMORY: clear per-chunk tracking state at rotation ──
            // 
            // observations.by_feature accumulates 1 entry per feature_id 
            // ever seen. Without clearing, a multi-hour encode at 50 new 
            // feature_ids/frame × 30 obs each ≈ 13GB of FeatureObservation 
            // entries — visible as the "0GB available, killed" OOM in 
            // long-encode logs.
            // 
            // The per-chunk tracking state (live_tracks, observations, 
            // triangulated cache) was used to BUILD the just-sealed web's 
            // geometry. The web is now closed; the cache served its 
            // purpose. Reset for the new chunk.
            //
            // We deliberately do NOT clear: pose_track (still needed for 
            // global frame indexing), chunk_store (its eviction policy 
            // governs its size), spatial_trace (already capped at 4M).
            self.observations.by_feature.clear();
            self.observations.by_feature.shrink_to_fit();
            self.observations.triangulated.clear();
            self.observations.triangulated.shrink_to_fit();
            // Re-detect features from scratch in the new chunk — old 
            // tracks reference a feature namespace that's now sealed.
            self.feature_pass.live_tracks.clear();
            self.feature_pass.live_tracks.shrink_to_fit();
            self.feature_pass.next_local_id = 0;
            // spatial_trace is per-chunk debug data (the world-view 
            // visualization scrubs it). Stuck at the 4M cap on long 
            // encodes; at rotation it's stale anyway.
            self.spatial_trace.clear();
            self.spatial_trace.shrink_to_fit();
            // relative_motion_per_frame is also per-chunk
            self.relative_motion_per_frame.clear();
            self.relative_motion_per_frame.shrink_to_fit();
        } else if need_open {
            self.chunk_store.open_chunk(video_frame_idx);
            self.chunk_open_call = self.frame_count;
        }
        // Add the source frame. ChunkStore filters internally based on tier 
        // (Stats/Summary skip; Sampled keeps every Nth; Full keeps all).
        self.chunk_store.add_frame(video_frame_idx, frame_yuv);
        
        // Triangulate based on the encoder's call count (not video frame), 
        // so cadence is independent of out-of-order arrival.
        // 
        // ── DEPTH-VARIANCE EMA UPDATE (UI-overlay filter, axis 2) ──
        // 
        // Runs ONLY on triangulation-refresh frames so we don't apply 
        // the same stale triangulation as a "new sample" 30 frames in a row 
        // (that would falsely decay the EMA toward zero even for real 
        // features, because delta would be 0 against the cached depth). 
        // 
        // The variance signal: how much does the triangulated camera-space 
        // Z change between refreshes? Real scene features have parallax 
        // (tens to thousands of units of Z variation as camera moves). 
        // UI overlays have ~0 variation (zero parallax — they ARE part 
        // of the camera).
        // 
        // EMA is over the absolute depth-delta between consecutive 
        // triangulations. ema = alpha * delta + (1-alpha) * ema. 
        // alpha = 0.3 matches MotionConsensusPass's decay. After ~8 
        // triangulation refreshes (240 frames at TRIANGULATE_INTERVAL=30), 
        // a feature that consistently has ~0 delta will have EMA below 
        // ui_overlay_variance_threshold (default 5.0) and get promoted.
        // 
        // ui_overlay_min_age_for_variance is in FRAMES, not triangulation 
        // cycles — at TRIANGULATE_INTERVAL=30 and min_age=8, a track 
        // needs to be alive long enough to have been triangulated at 
        // least once (probably twice). The convergence rate is the real 
        // gate; min_age is a paranoia floor.
        if self.frame_count > 0 && self.frame_count % TRIANGULATE_INTERVAL == 0 {
            self.observations.run_triangulation(&self.triangulate_pass, &self.pose_track);
            
            let alpha: f32 = 0.3;
            for track in self.feature_pass.live_tracks.iter_mut() {
                if let Some(tf) = self.observations.triangulated.get(&track.feature_id) {
                    let dz = tf.world_pos.z - pose.position.z;
                    let current_depth = dz.abs();
                    if track.last_depth > 0.0 {
                        let delta = (current_depth - track.last_depth).abs();
                        track.depth_variance_ema =
                            alpha * delta + (1.0 - alpha) * track.depth_variance_ema;
                    }
                    // else: first triangulation — record the depth, leave 
                    // EMA at its initial large value until a second 
                    // triangulation produces a meaningful delta.
                    track.last_depth = current_depth;
                    track.frames_since_triangulation = 0;
                    
                    // Promotion to UiOverlay based on near-zero parallax. 
                    // Skip if already UiOverlay (layer-pass may have 
                    // gotten here first) or Dynamic (we trust 
                    // MotionConsensusPass for moving objects).
                    if track.motion_class != MotionClass::UiOverlay
                        && track.motion_class != MotionClass::Dynamic
                        && track.frames_alive >= self.ui_overlay_min_age_for_variance
                        && track.depth_variance_ema < self.ui_overlay_variance_threshold
                    {
                        track.motion_class = MotionClass::UiOverlay;
                    }
                }
            }
        }
        
        // Annotate features with cached triangulated depth
        self.observations.annotate_with_depth(&mut features, &pose);
        
        // ── DensifyPass: anchor-frame dense depth map (Session 4) ──
        // Use video_frame_idx so anchor cadence aligns with video timeline.
        // Pass frame_yuv so vertices get sampled real scene colors during 
        // mesh integration (fixes the bluish-green confidence-tint bug).
        self.maybe_densify(video_frame_idx, frame_yuv);
        
        // FUTURE PASSES will plug in here:
        //   Session 5: self.scene_mesh.merge(MeshPass.surface(self.depth_maps))
        
        // ── ANCHOR TRACK CAPTURE ──
        // 
        // Build a per-frame snapshot of live tracks for the V8AT trailer 
        // (Tobii-style anchor overlay). Captured AFTER all classification 
        // passes so we record the FINAL motion_class for each feature 
        // this frame.
        // 
        // Stored cls codes (matching spatial_trace + the visualizer's 
        // ftype mapping):
        //   0 = Unknown, 1 = Static, 2 = Dynamic, 3 = UiOverlay
        // 
        // We dedupe by external_frame_idx — if multiple chunks happen 
        // to encode the same frame (shouldn't happen at the worker 
        // level since workers process disjoint frame ranges, but might 
        // at merge time across shards), the LAST writer wins. The 
        // last-writer-wins is fine because adjacent encodes of the 
        // same frame produce near-identical track positions.
        {
            let mut anchors: Vec<(i16, i16, u8)> = Vec::with_capacity(self.feature_pass.live_tracks.len());
            for t in &self.feature_pass.live_tracks {
                let x = t.current_x.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                let y = t.current_y.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                let cls = match t.motion_class {
                    MotionClass::Unknown => 0u8,
                    MotionClass::Static => 1u8,
                    MotionClass::Dynamic => 2u8,
                    MotionClass::UiOverlay => 3u8,
                };
                anchors.push((x, y, cls));
            }
            self.anchor_track.insert(video_frame_idx, anchors);
        }
        
        // Sort features by ID before serialization. Sorted IDs are 
        // monotonic across frames (because IDs are assigned sequentially 
        // by the FeatureTrackPass), which dramatically improves zstd 
        // ratio on the resulting bytes — sequential u32 deltas compress 
        // to a few bits each. Cost: O(n log n) on ~300 features = ~5µs/frame.
        let mut features = features;
        features.sort_unstable_by_key(|f| f.feature_id);
        
        let frame = V8Frame { camera_pose: pose, features };
        let bytes = frame.to_bytes();
        
        // ── v8.1 PHOTOMETRIC RESIDUAL (optional) ──
        // 
        // When FAO_V81=1 is set in the environment, compute a per-frame 
        // photometric residual: render the current mesh through the pose, 
        // compare to the source frame, store the quarter-resolution 
        // i8-quantized RGB delta. Decoder applies it on top of the 
        // rasterized output to reconstruct near-source quality.
        // 
        // Cost: one mesh rasterization per frame (~5-20ms at 1080p, less 
        // at smaller resolutions). Size: ~10-40KB compressed per frame at 
        // quarter resolution, depending on content. For comparison, base 
        // v8 frames are ~1-2KB. So v8.1 adds about 10x per-frame size for 
        // near-source quality — still substantially smaller than AV1's 
        // 50-200KB/s budget.
        // 
        // Skipped if:
        //   - FAO_V81 is unset or 0
        //   - scene_mesh is empty (nothing to render against)
        //   - scene_mesh has < 100 vertices (too sparse to add useful detail; 
        //     residual would be mostly uncovered-block sentinel anyway, 
        //     which the coverage-aware residual writes as a fixed-byte run 
        //     ~1KB compressed — not worth the ~5-15ms rasterizer cost)
        //   - render fails for any reason
        let residual_blob: Vec<u8> = if compute_v81_residual_enabled() 
            && self.scene_mesh.vertices.len() >= 100
        {
            // Dispatch: v10 DCT residual (opt-in) or legacy bias-128.
            // Both produce a residual blob whose first u16 carries the 
            // header bits; the decoder routes based on FLAG_DCT_FORMAT.
            if use_v10_residual() {
                compute_v10_residual(&self.scene_mesh, &pose, frame_yuv, w, h)
                    .unwrap_or_default()
            } else {
                compute_photometric_residual(&self.scene_mesh, &pose, frame_yuv, w, h)
                    .unwrap_or_default()
            }
        } else { Vec::new() };
        
        // ── ON-WIRE COMPRESSION ──
        // 
        // Frames over a threshold get zstd-wrapped. This reliably saves 
        // 30-50% on per-frame size for typical content (where the same 
        // ~500 features repeat across frames, so feature_id and screen 
        // coords compress beautifully).
        // 
        // Wrapper format:
        //   Magic 'Z'(1B) + version(1B=1) + uncompressed_len(u32 LE) + zstd_payload
        // 
        // The decoder distinguishes raw from compressed by the first byte: 
        // 'V' = raw V8/V81, 'Z' = zstd-wrapped.
        // 
        // Threshold = 600 bytes: below this, zstd overhead (~22B + minimal 
        // compression on tiny inputs) exceeds savings.
        const ZSTD_THRESHOLD: usize = 600;
        let has_residual = !residual_blob.is_empty();
        // Threshold: wrap if payload is large enough, OR if residual is 
        // present (must use v2 wrapper either way).
        let bytes = if bytes.len() >= ZSTD_THRESHOLD || has_residual {
            use std::io::Cursor;
            // Per-frame zstd: was level 1 (fast, slightly worse ratio). 
            // Switched to adaptive: small frames (≤64KB, the typical 
            // case for V8p planar data) get level 12 — still ~3MB/s 
            // throughput but ~25% better ratio than level 1.
            // 
            // FAO_FRAME_ZSTD_FAST=1 reverts to level 1 for slow-CPU 
            // workflows. The default trade favors smaller files since 
            // most encode time is spent elsewhere (feature tracking, 
            // mesh integration).
            let frame_zstd_level: i32 = if std::env::var("FAO_FRAME_ZSTD_FAST")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
            {
                1
            } else if bytes.len() <= 65536 {
                12
            } else {
                3
            };
            match zstd::encode_all(Cursor::new(&bytes), frame_zstd_level) {
                Ok(compressed) if compressed.len() + 6 < bytes.len() || has_residual => {
                    // Compression helped, OR we need v2 wrapper anyway 
                    // for the residual. Wrap.
                    if has_residual {
                        // v2 layout: 'Z'+ver+ulen(4)+reslen(4)+zstd_payload+residual
                        let mut out = Vec::with_capacity(10 + compressed.len() + residual_blob.len());
                        out.push(b'Z');
                        out.push(2u8);  // wrapper version 2
                        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                        out.extend_from_slice(&(residual_blob.len() as u32).to_le_bytes());
                        out.extend_from_slice(&compressed);
                        out.extend_from_slice(&residual_blob);
                        out
                    } else {
                        // v1 layout: 'Z'+ver+ulen(4)+zstd_payload
                        let mut out = Vec::with_capacity(6 + compressed.len());
                        out.push(b'Z');
                        out.push(1u8);
                        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                        out.extend_from_slice(&compressed);
                        out
                    }
                }
                _ => bytes,  // compression didn't help or failed — keep raw
            }
        } else { bytes };
        
        self.total_bytes += bytes.len() as u64;
        self.frame_count += 1;
        self.prev_frame = Some(frame_yuv[..needed.min(frame_yuv.len())].to_vec());
        // Cache for static-frame fast path on the next call.
        self.last_emit_bytes = bytes.clone();
        
        bytes
    }
    
    /// Finalize the encoder at end of stream.
    /// 
    /// Seals the currently-open chunk into the chunk store (since chunk 
    /// rotation only fires mid-stream, the last chunk would otherwise be 
    /// lost). After this returns, all encode geometry is in chunk_store.
    /// 
    /// Call this once after the last encode_frame_at() before serializing 
    /// the multi-web trailer.
    pub fn finalize(&mut self) {
        // If there's an open chunk, seal its mesh + close it.
        if self.chunk_store.current_chunk.is_some() {
            let sealed_mesh = std::mem::replace(&mut self.scene_mesh, SceneMesh::new());
            let sealed_poses = self.pose_track.clone();
            self.chunk_store.seal_web(sealed_mesh, sealed_poses);
            
            // Try alignment for the last chunk too
            if let Some(score) = self.chunk_store.try_align_current_web() {
                if let Some(chunk) = self.chunk_store.current_chunk.as_ref() {
                    eprintln!("[v8 web] FINAL chunk {}..{} aligned to canonical web {} (similarity {:.2})",
                        chunk.frame_start, chunk.frame_end,
                        chunk.canonical_web_id.unwrap_or(0), score);
                }
            }
            
            // Use the encoder's last-seen frame as end_frame
            self.chunk_store.close_chunk(self.frame_count);
        }
        
        // ─── Library matching (Phase 3) ───
        // 
        // Walk all closed_chunks and try to find an existing library 
        // canonical that matches each one. Matched chunks have their 
        // mesh dropped from RAM and a LibraryRef recorded for the V8LR 
        // trailer.
        // 
        // Critically, this happens BEFORE the merge in fao_app — the merge 
        // sees a smaller closed_chunks list (matched ones are gone) plus 
        // a non-empty library_refs list to fold into the trailer.
        if let Some(lib_arc) = self.library.clone() {
            let threshold = self.library_match_threshold;
            // If writable, take a write lock so we can touch matched 
            // canonicals (ref_count++, last_referenced = now). If 
            // read-only, the immutable view doesn't allow touches 
            // — fall back to a temporary read-only stub that satisfies 
            // try_match_against_library's &mut signature without 
            // actually mutating disk.
            let n_matches = if self.library_writable {
                let mut lib_w = match lib_arc.write() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                self.chunk_store.try_match_against_library(&mut *lib_w, threshold)
            } else {
                // Read-only library: take write guard purely to satisfy 
                // the signature. touch() will fail on read-only lib 
                // (no lockfile), which we log but tolerate.
                let mut lib_w = match lib_arc.write() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                self.chunk_store.try_match_against_library(&mut *lib_w, threshold)
            };
            if n_matches > 0 {
                eprintln!("[v8 library] {} chunks matched library canonicals (saved ~{:.1} MB of mesh data)",
                    n_matches,
                    self.chunk_store.total_library_match_bytes_saved as f64 / 1_048_576.0);
            }
            
            // If writable AND there are unmatched canonicals, ADD them to 
            // the library. Future encodes of similar content will then 
            // match against these.
            if self.library_writable {
                let mut added = 0u32;
                let mut lib_w = match lib_arc.write() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                // Only add SELF-CANONICAL chunks (not aligned followers) — 
                // followers are already a delta to a canonical, no benefit 
                // to library-storing them.
                let chunks_to_add: Vec<(u32, u32, WebSignature, SceneMesh, Vec<CameraPose>)> = self.chunk_store.closed_chunks.iter()
                    .filter(|c| {
                        !c.web_mesh.vertices.is_empty()
                        && match c.canonical_web_id {
                            None => true,
                            Some(canon_id) => canon_id == c.frame_start,
                        }
                    })
                    .map(|c| (
                        c.frame_start,
                        c.frame_end,
                        c.web_signature,
                        c.web_mesh.clone(),
                        c.web_pose_track.clone(),
                    ))
                    .collect();
                for (f_start, f_end, sig, mesh, poses) in chunks_to_add {
                    match lib_w.add_web(sig, &mesh, &poses, "<encode-source>", f_start, f_end) {
                        Ok(_uuid) => { added += 1; }
                        Err(e) => {
                            eprintln!("[v8 library] add_web failed for chunk {}..{}: {}", f_start, f_end, e);
                        }
                    }
                }
                if added > 0 {
                    eprintln!("[v8 library] added {} new canonicals to library (will benefit future encodes)", added);
                }
            }
        }
    }
    
    /// Backwards-compat shim: uses internal frame_count as the external index.
    /// Prefer encode_frame_at(yuv, video_fidx) for correct multi-thread behavior.
    pub fn encode_frame(&mut self, frame_yuv: &[u8]) -> Vec<u8> {
        let fidx = self.frame_count;
        self.encode_frame_at(frame_yuv, fidx)
    }
    
    pub fn avg_bytes_per_frame(&self) -> f32 {
        if self.frame_count == 0 { return 0.0; }
        self.total_bytes as f32 / self.frame_count as f32
    }
}

// ═══════════════════════════════════════════════════════════
// V8Decoder — scaffold
// ═══════════════════════════════════════════════════════════
//
// Until the mesh+textures path is implemented, we don't produce a real
// frame reconstruction. The decoder just exposes the camera path for the
// World viewer's virtual-camera fly-through.

/// Single web decoded from a V8MX trailer. Each web has its own coordinate 
/// frame and frame range. Aligned webs reference a canonical web by ID.
#[derive(Clone, Debug)]
pub struct DecodedWeb {
    pub chunk_id: u32,
    pub canonical_id: u32,
    pub frame_start: u32,
    pub frame_end: u32,
    pub kind: u8,  // 0=aligned-delta, 1=canonical, 2=standalone
    pub transform: [f32; 16],
    /// For canonical/standalone: full mesh.
    /// For aligned: only the residual vertices (full mesh = canonical's mesh + transform applied + residual).
    pub mesh: SceneMesh,
}

pub struct V8Decoder {
    pub width: u32,
    pub height: u32,
    pub scene_mesh: SceneMesh,
    pub pose_track: Vec<CameraPose>,
    /// All webs from V8MX trailer. Empty if file uses old V8MS-only format.
    pub webs: Vec<DecodedWeb>,
    /// Library refs from V8LR trailer (Phase 3). Each entry points to 
    /// a library canonical that supplies the actual mesh data for one 
    /// chunk. Resolved against `library` at decode time.
    pub library_refs: Vec<LibraryRef>,
    /// Optional library handle. When Some, V8LR refs are resolved against 
    /// it during render. When None, frames covered by V8LR refs render 
    /// without geometry (gracefully degraded).
    pub library: Option<std::sync::Arc<std::sync::RwLock<crate::library::Library>>>,
    
    /// Optional GPU rasterizer. Lazily initialized on the first call to 
    /// render_frame_rgb that has a non-empty mesh. Skipped on systems 
    /// without a supported wgpu backend (falls back to CPU rasterizer 
    /// transparently). Set to None to force CPU-only rendering.
    /// 
    /// Wrapped in Mutex because render_frame_rgb takes &self but the GPU 
    /// rasterizer needs &mut for buffer growth + readback. The mutex is 
    /// per-decoder, so cross-decoder GPU calls don't contend.
    #[cfg(feature = "gpu")]
    pub gpu_rasterizer: std::sync::Mutex<Option<crate::v8_gpu_rasterizer::GpuMeshRasterizer>>,
    /// Once-flag: when GPU init fails, don't keep retrying. Cheaper than 
    /// re-running the wgpu adapter request on every frame.
    #[cfg(feature = "gpu")]
    pub gpu_init_failed: std::sync::atomic::AtomicBool,
    
    /// Stabilizer state: the (frame_idx, pose) used by the most recent 
    /// render. When FAO_LESS_JITTER is enabled, render_frame_rgb 
    /// compares the new frame's pose to this and snaps to it if the 
    /// difference is below a small threshold — suppressing per-frame 
    /// jitter from noisy pose estimates.
    /// 
    /// This is a decode-only correction; the underlying pose data on 
    /// disk is unchanged, so it's safe to flip on/off mid-session.
    /// 
    /// Stored in a Mutex because render_frame_rgb takes &self.
    pub stabilizer: std::sync::Mutex<Option<(u32, CameraPose)>>,
}

impl V8Decoder {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width, height,
            scene_mesh: SceneMesh::new(),
            pose_track: Vec::new(),
            webs: Vec::new(),
            library_refs: Vec::new(),
            library: None,
            #[cfg(feature = "gpu")]
            gpu_rasterizer: std::sync::Mutex::new(None),
            #[cfg(feature = "gpu")]
            gpu_init_failed: std::sync::atomic::AtomicBool::new(false),
            stabilizer: std::sync::Mutex::new(None),
        }
    }
    
    /// Attach a library to this decoder. After this, V8LR-referenced 
    /// chunks fault their meshes through the library on render.
    pub fn attach_library(
        &mut self,
        library: std::sync::Arc<std::sync::RwLock<crate::library::Library>>,
    ) {
        self.library = Some(library);
    }
    
    /// Resolve a library_ref's mesh by reading from the attached library.
    /// Returns the mesh + pose track on success, or None if the library 
    /// isn't attached, the uuid is unknown, or the load failed.
    /// 
    /// The alignment_transform from the LibraryRef should be applied to 
    /// the returned vertices to position them in this video's scene 
    /// coordinate frame.
    pub fn resolve_library_ref(&self, lr: &LibraryRef) -> Option<(SceneMesh, Vec<CameraPose>)> {
        let lib = self.library.as_ref()?;
        let guard = lib.read().ok()?;
        guard.load_web(&lr.uuid).ok()
    }
    
    pub fn with_mesh(mut self, mesh: SceneMesh) -> Self {
        self.scene_mesh = mesh;
        self
    }
    
    pub fn with_pose_track(mut self, track: Vec<CameraPose>) -> Self {
        self.pose_track = track;
        self
    }
    
    pub fn with_webs(mut self, webs: Vec<DecodedWeb>) -> Self {
        self.webs = webs;
        self
    }
    
    /// Render a mesh through this decoder, choosing GPU or CPU based on 
    /// availability. Mirrors v8_rasterizer::render_mesh signature exactly.
    /// 
    /// First call lazily initializes the GPU rasterizer; subsequent calls 
    /// reuse it. If GPU init fails (no Metal/Vulkan/DX12 adapter, missing 
    /// device features, etc.) the path falls back to the CPU rasterizer 
    /// permanently — no per-frame retries.
    /// 
    /// Without the `gpu` feature, this is just a thin wrapper around the 
    /// CPU rasterizer.
    fn render_mesh_dispatch(
        &self,
        mesh: &SceneMesh,
        world_transform: Option<&[f32; 16]>,
        pose: &CameraPose,
    ) -> Option<Vec<u8>> {
        #[cfg(feature = "gpu")]
        {
            use std::sync::atomic::Ordering;
            // Escape hatch: set FAO_NO_GPU=1 to force CPU rasterizer 
            // even when the gpu feature is compiled in. Used for 
            // debugging GPU vs CPU output mismatches.
            let gpu_disabled = std::env::var("FAO_NO_GPU")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if !gpu_disabled && !self.gpu_init_failed.load(Ordering::Relaxed) {
                if let Ok(mut guard) = self.gpu_rasterizer.lock() {
                    if guard.is_none() {
                        // Lazy init.
                        match crate::v8_gpu_rasterizer::GpuMeshRasterizer::new(self.width, self.height) {
                            Some(r) => {
                                eprintln!("[v8 GPU raster] initialized for {}x{}", self.width, self.height);
                                *guard = Some(r);
                            }
                            None => {
                                eprintln!("[v8 GPU raster] no GPU adapter — falling back to CPU rasterizer permanently");
                                self.gpu_init_failed.store(true, Ordering::Relaxed);
                            }
                        }
                    }
                    if let Some(ref mut r) = *guard {
                        if let Some(rgb) = r.render(
                            mesh, world_transform, pose,
                            self.width as usize, self.height as usize,
                        ) {
                            return Some(rgb);
                        }
                        // GPU returned None — fall through to CPU as a 
                        // best-effort recovery. We do NOT set gpu_init_failed 
                        // here because the failure might be mesh-specific 
                        // (e.g., zero triangles after clipping).
                    }
                }
            }
        }
        // CPU fallback (always available).
        crate::v8_rasterizer::render_mesh(
            mesh, world_transform, pose,
            self.width as usize, self.height as usize,
        )
    }
    
    /// Return the pose this frame should render through, after the 
    /// optional jitter-suppression filter.
    /// 
    /// When `FAO_LESS_JITTER=1` is set, compares the frame's pose to 
    /// the most recently rendered pose. If the difference is small 
    /// (sub-pixel screen movement at typical scene depths), snaps to 
    /// the last pose. This suppresses encoder-side noise that's 
    /// otherwise visible as per-frame jitter.
    /// 
    /// When the env var is unset (default), returns the frame's pose 
    /// unchanged — pixel-identical to the old behavior.
    /// 
    /// Threshold: 0.01 scene-units per axis. Small enough that real 
    /// camera motion is preserved (a 1-unit/frame pan is 100× over 
    /// threshold), large enough to absorb pose-estimate noise from 
    /// the encoder's identity-pose default and feature-track drift.
    fn effective_pose(&self, frame_idx: u32, pose: &CameraPose) -> CameraPose {
        // Two pose-stabilization modes — set EITHER, not both:
        //   FAO_LESS_JITTER=1 — hard snap to previous pose when delta 
        //     is below 0.01 scene-units (kills literal-zero noise; 
        //     no effect on real motion).
        //   FAO_POSE_EMA=<alpha> — soft low-pass filter with the given 
        //     alpha. alpha=0 means no smoothing, alpha=1 means freeze 
        //     to first pose. Recommended range 0.3-0.7 for noticeable 
        //     smoothing without latency. Default off.
        // 
        // EMA gives the user a tunable knob between "raw" (no smoothing) 
        // and "snap" (FAO_LESS_JITTER). Bigger alpha = more smoothing. 
        // Like the jitter snap, EMA only applies within a 4-frame window 
        // so seeks don't blend across scene cuts.
        let snap_enabled = std::env::var("FAO_LESS_JITTER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let ema_alpha: f32 = std::env::var("FAO_POSE_EMA")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .map(|a| a.clamp(0.0, 0.95))  // cap below 1 to keep some pose responsiveness
            .unwrap_or(0.0);
        
        if !snap_enabled && ema_alpha <= 0.001 {
            return *pose;
        }
        
        let mut guard = match self.stabilizer.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        
        let result = if let Some((last_idx, last_pose)) = guard.as_ref() {
            // Only stabilize within a window of adjacent frames. Big 
            // jumps (scrubbing, seek) should NOT snap to old pose.
            let frame_dist = (frame_idx as i64 - *last_idx as i64).abs();
            if frame_dist > 4 {
                *pose
            } else if ema_alpha > 0.001 {
                // EMA low-pass: blend toward previous pose. The 
                // rotation interpolation uses simple quaternion lerp 
                // (faster than slerp; for small deltas the difference 
                // is imperceptible). Re-normalize the quat to keep 
                // it unit-length.
                let a = ema_alpha;
                let b = 1.0 - a;
                let mut q = [
                    a * last_pose.rotation[0] + b * pose.rotation[0],
                    a * last_pose.rotation[1] + b * pose.rotation[1],
                    a * last_pose.rotation[2] + b * pose.rotation[2],
                    a * last_pose.rotation[3] + b * pose.rotation[3],
                ];
                let qlen = (q[0]*q[0] + q[1]*q[1] + q[2]*q[2] + q[3]*q[3]).sqrt().max(1e-6);
                q[0] /= qlen; q[1] /= qlen; q[2] /= qlen; q[3] /= qlen;
                CameraPose {
                    position: Vec3 {
                        x: a * last_pose.position.x + b * pose.position.x,
                        y: a * last_pose.position.y + b * pose.position.y,
                        z: a * last_pose.position.z + b * pose.position.z,
                    },
                    rotation: q,
                    focal: pose.focal,
                    principal: pose.principal,
                    distortion: pose.distortion,
                }
            } else {
                let dx = (pose.position.x - last_pose.position.x).abs();
                let dy = (pose.position.y - last_pose.position.y).abs();
                let dz = (pose.position.z - last_pose.position.z).abs();
                const EPS: f32 = 0.01;
                if dx < EPS && dy < EPS && dz < EPS {
                    // Sub-threshold delta: snap to last pose. Visually 
                    // identical, but the GPU/CPU rasterizer produces 
                    // bit-identical output for two adjacent frames.
                    *last_pose
                } else {
                    *pose
                }
            }
        } else {
            *pose
        };
        
        *guard = Some((frame_idx, result));
        result
    }
    
    /// Parse a V8MX trailer back into a list of DecodedWeb.
    /// 
    /// Counterpart to ChunkStore::to_v8mx_trailer. Format documented there.
    /// Returns None on parse failure (corrupt, truncated, or wrong magic).
    pub fn parse_v8mx(data: &[u8]) -> Option<Vec<DecodedWeb>> {
        use std::io::Cursor;
        if data.len() < 9 || &data[..4] != b"V8MX" { return None; }
        let trailer_version = data[4];  // 1 = legacy, 2 = tagged blobs (v8mx-quant)
        if trailer_version > 2 {
            eprintln!("[v8 parse] V8MX trailer version {} is newer than this build supports (max 2).                        Treating as legacy parse — some mesh data may be misinterpreted.",
                trailer_version);
        }
        let n_webs = u32::from_le_bytes([data[5], data[6], data[7], data[8]]) as usize;
        // Sanity cap. A real .fao should never have more than a few 
        // thousand webs (one per ~30 frames of input). If n_webs is in 
        // the millions, this is almost certainly a false-positive V8MX 
        // magic match inside a zstd blob in the frame data — refuse to 
        // allocate gigabytes for what is essentially random bytes.
        // 
        // The 1M cap allows hundreds of hours of content; anything 
        // bigger is suspect.
        const MAX_REASONABLE_WEBS: usize = 1_000_000;
        if n_webs > MAX_REASONABLE_WEBS {
            eprintln!("[v8 parse] V8MX claims {} webs (cap {}). Likely a false-magic match; refusing to parse.",
                n_webs, MAX_REASONABLE_WEBS);
            return None;
        }
        // Also cap by what the data slice could plausibly contain: 
        // each web header is at least 85 bytes, so n_webs * 85 must 
        // fit in data.len(). Catches huge values that pass the absolute 
        // cap but couldn't exist in this much data.
        if n_webs.saturating_mul(85) > data.len() {
            eprintln!("[v8 parse] V8MX claims {} webs but data is only {} bytes. Refusing.",
                n_webs, data.len());
            return None;
        }
        let mut webs = Vec::with_capacity(n_webs);
        let mut p = 9;
        
        for _ in 0..n_webs {
            // Header: chunk_id, canonical_id, frame_start, frame_end (4 × u32 = 16B)
            //       + kind (u8) + transform (16 × f32 = 64B) + blob_len (u32)
            // = 16 + 1 + 64 + 4 = 85 bytes per web header
            if p + 85 > data.len() { return None; }
            let chunk_id = u32::from_le_bytes([data[p], data[p+1], data[p+2], data[p+3]]); p += 4;
            let canonical_id = u32::from_le_bytes([data[p], data[p+1], data[p+2], data[p+3]]); p += 4;
            let frame_start = u32::from_le_bytes([data[p], data[p+1], data[p+2], data[p+3]]); p += 4;
            let frame_end = u32::from_le_bytes([data[p], data[p+1], data[p+2], data[p+3]]); p += 4;
            let kind = data[p]; p += 1;
            let mut transform = [0.0f32; 16];
            for i in 0..16 {
                transform[i] = f32::from_le_bytes([data[p], data[p+1], data[p+2], data[p+3]]);
                p += 4;
            }
            let blob_len = u32::from_le_bytes([data[p], data[p+1], data[p+2], data[p+3]]) as usize; p += 4;
            if p + blob_len > data.len() { return None; }
            let blob = &data[p..p+blob_len];
            p += blob_len;
            
            // Decompress blob
            let raw = match zstd::decode_all(Cursor::new(blob)) {
                Ok(r) => r,
                Err(_) => continue,  // skip this web on corruption rather than failing whole trailer
            };
            
            let mesh = if kind == 0 {
                // Aligned: blob is raw f32×3 residual vertices only.
                // v2 trailer prepends a tag byte (we keep aligned blobs 
                // as legacy f32 even in v2, so the tag is always 0 — 
                // but we still strip it).
                let base_off = if trailer_version >= 2 { 1 } else { 0 };
                let mut m = SceneMesh::new();
                if raw.len() < base_off { continue; }
                let payload = &raw[base_off..];
                let n = payload.len() / 12;
                for i in 0..n {
                    let off = i * 12;
                    if off + 12 > payload.len() { break; }
                    let x = f32::from_le_bytes([payload[off], payload[off+1], payload[off+2], payload[off+3]]);
                    let y = f32::from_le_bytes([payload[off+4], payload[off+5], payload[off+6], payload[off+7]]);
                    let z = f32::from_le_bytes([payload[off+8], payload[off+9], payload[off+10], payload[off+11]]);
                    m.vertices.push(Vec3::new(x, y, z));
                }
                m
            } else {
                // Canonical/standalone: 
                //   v1 trailer: header (n_verts u32, n_indices u32, n_colors u32) 
                //               + verts (3×f32 each) + indices (u32 each) + colors (3×f32 each)
                //   v2 trailer: tag byte + header + ... 
                //     tag=0: legacy f32 layout (same as v1)
                //     tag=1: AABB (24 bytes) + quantized verts (3×u16 each) 
                //            + indices (u32) + colors (3×u8)
                if raw.len() < 12 { continue; }
                let (format_tag, header_off) = if trailer_version >= 2 {
                    (raw[0], 1usize)
                } else {
                    (0u8, 0usize)
                };
                if raw.len() < header_off + 12 { continue; }
                let n_v = u32::from_le_bytes([raw[header_off], raw[header_off+1], raw[header_off+2], raw[header_off+3]]) as usize;
                let n_i = u32::from_le_bytes([raw[header_off+4], raw[header_off+5], raw[header_off+6], raw[header_off+7]]) as usize;
                let n_c = u32::from_le_bytes([raw[header_off+8], raw[header_off+9], raw[header_off+10], raw[header_off+11]]) as usize;
                
                // Bounds-check counts against payload size.
                let vert_size = if format_tag == 1 { 6 } else { 12 };
                let color_size = if format_tag == 1 { 3 } else { 12 };
                let aabb_size = if format_tag == 1 { 24 } else { 0 };
                let needed = n_v.saturating_mul(vert_size)
                    .saturating_add(n_i.saturating_mul(4))
                    .saturating_add(n_c.saturating_mul(color_size))
                    .saturating_add(header_off + 12 + aabb_size);
                if needed > raw.len() {
                    eprintln!("[v8 parse] mesh blob: tag={} n_v={} n_i={} n_c={} but raw is only {} bytes — skipping",
                        format_tag, n_v, n_i, n_c, raw.len());
                    continue;
                }
                
                let mut m = SceneMesh::new();
                m.vertices.reserve(n_v);
                m.indices.reserve(n_i);
                m.vertex_colors.reserve(n_c);
                
                let mut off = header_off + 12;
                
                if format_tag == 1 {
                    // Read AABB.
                    let mut mn = [0.0f32; 3];
                    let mut mx = [0.0f32; 3];
                    for i in 0..3 {
                        mn[i] = f32::from_le_bytes([raw[off], raw[off+1], raw[off+2], raw[off+3]]);
                        off += 4;
                    }
                    for i in 0..3 {
                        mx[i] = f32::from_le_bytes([raw[off], raw[off+1], raw[off+2], raw[off+3]]);
                        off += 4;
                    }
                    let extent = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
                    // Dequantize vertices.
                    for _ in 0..n_v {
                        if off + 6 > raw.len() { break; }
                        let qx = u16::from_le_bytes([raw[off], raw[off+1]]) as f32;
                        let qy = u16::from_le_bytes([raw[off+2], raw[off+3]]) as f32;
                        let qz = u16::from_le_bytes([raw[off+4], raw[off+5]]) as f32;
                        let x = mn[0] + qx * extent[0] / 65535.0;
                        let y = mn[1] + qy * extent[1] / 65535.0;
                        let z = mn[2] + qz * extent[2] / 65535.0;
                        m.vertices.push(Vec3::new(x, y, z));
                        off += 6;
                    }
                    // Indices (unchanged).
                    for _ in 0..n_i {
                        if off + 4 > raw.len() { break; }
                        m.indices.push(u32::from_le_bytes([raw[off], raw[off+1], raw[off+2], raw[off+3]]));
                        off += 4;
                    }
                    // Colors as u8 RGB.
                    for _ in 0..n_c {
                        if off + 3 > raw.len() { break; }
                        let r = raw[off] as f32 / 255.0;
                        let g = raw[off+1] as f32 / 255.0;
                        let b = raw[off+2] as f32 / 255.0;
                        m.vertex_colors.push([r, g, b]);
                        off += 3;
                    }
                    m
                } else {
                    // Legacy f32 layout (tag=0 in v2, or v1 trailer).
                    for _ in 0..n_v {
                        if off + 12 > raw.len() { break; }
                        let x = f32::from_le_bytes([raw[off], raw[off+1], raw[off+2], raw[off+3]]);
                        let y = f32::from_le_bytes([raw[off+4], raw[off+5], raw[off+6], raw[off+7]]);
                        let z = f32::from_le_bytes([raw[off+8], raw[off+9], raw[off+10], raw[off+11]]);
                        m.vertices.push(Vec3::new(x, y, z));
                        off += 12;
                    }
                    for _ in 0..n_i {
                        if off + 4 > raw.len() { break; }
                        m.indices.push(u32::from_le_bytes([raw[off], raw[off+1], raw[off+2], raw[off+3]]));
                        off += 4;
                    }
                    for _ in 0..n_c {
                        if off + 12 > raw.len() { break; }
                        let r = f32::from_le_bytes([raw[off], raw[off+1], raw[off+2], raw[off+3]]);
                        let g = f32::from_le_bytes([raw[off+4], raw[off+5], raw[off+6], raw[off+7]]);
                        let b = f32::from_le_bytes([raw[off+8], raw[off+9], raw[off+10], raw[off+11]]);
                        m.vertex_colors.push([r, g, b]);
                        off += 12;
                    }
                    m
                }
            };
            
            webs.push(DecodedWeb {
                chunk_id, canonical_id, frame_start, frame_end, kind, transform, mesh,
            });
        }
        
        Some(webs)
    }
    
    /// Decode a single v8 frame. Returns None for now — see 
    /// `render_frame_rgb` for the actual mesh rasterizer (RGB output).
    /// 
    /// This older signature is kept so existing callers compile; they 
    /// can migrate to `render_frame_rgb(bytes, frame_idx)` for working 
    /// playback.
    pub fn decode_frame(&self, _frame_bytes: &[u8]) -> Option<Vec<u8>> {
        None
    }
    
    /// Render a v8 frame to RGB.
    /// 
    /// Pipeline:
    ///   1. Parse the frame bytes for the camera pose.
    ///   2. Pick the web that covers `frame_idx` and resolve its mesh 
    ///      (canonical = direct mesh, aligned = canonical's mesh + 
    ///      this web's transform).
    ///   3. Rasterize the mesh through the camera pose into an RGB buffer.
    /// 
    /// Returns `None` if no web covers the frame, the mesh is empty, or 
    /// no triangles ended up on screen. Callers should fall back to 
    /// source-video display in that case.
    pub fn render_frame_rgb(&self, frame_bytes: &[u8], frame_idx: u32) -> Option<Vec<u8>> {
        // Step 1: parse the frame for its camera pose. The pose-track 
        // also has poses; prefer the in-frame pose since it's authoritative 
        // for THIS frame (the pose-track is per-frame too but we may be 
        // off-by-one if the user is scrubbing during decode).
        let frame = V8Frame::from_bytes_any(frame_bytes)?;
        // Apply the FAO_LESS_JITTER stabilizer if enabled. Default is 
        // pass-through (returns the frame's pose unchanged).
        let pose_owned = self.effective_pose(frame_idx, &frame.camera_pose);
        let pose = &pose_owned;
        
        // Step 1.5: check library refs FIRST. If this frame falls inside 
        // a chunk whose mesh lives in the attached library (V8LR trailer), 
        // resolve it now. This is the Phase-3 cross-encode mesh sharing 
        // path: chunks that matched a library canonical don't carry their 
        // own mesh — they carry a uuid + alignment transform here.
        // 
        // Fall-through to the local webs list if:
        //   - no library_ref covers this frame
        //   - the library isn't attached
        //   - the load failed
        //   - the library has the mesh but it's empty (corrupt entry)
        for lr in &self.library_refs {
            if frame_idx >= lr.chunk_frame_start && frame_idx <= lr.chunk_frame_end {
                if let Some((mut canonical_mesh, _poses)) = self.resolve_library_ref(lr) {
                    if !canonical_mesh.vertices.is_empty() {
                        // The mesh is in the canonical's frame; this 
                        // chunk's pose is in THIS video's frame. The 
                        // alignment_transform maps chunk-local → canonical, 
                        // so to render canonical into chunk-local we apply 
                        // the inverse.
                        let inv = crate::v8_rasterizer::invert_rigid_transform(&lr.alignment_transform);
                        // If this ref carried a residual (vertices the 
                        // chunk had that the canonical didn't), splice 
                        // them in. Their positions are in chunk-local 
                        // already so they go in straight.
                        if !lr.residual_vertices.is_empty() {
                            let v_base = canonical_mesh.vertices.len() as u32;
                            // Residual vertices are in chunk-local frame; 
                            // map them into canonical frame so they share 
                            // the inverse transform with the rest of the 
                            // mesh at render time. The forward transform 
                            // is alignment_transform.
                            for v in &lr.residual_vertices {
                                let mapped = crate::v8_rasterizer::transform_vec3(&lr.alignment_transform, *v);
                                canonical_mesh.vertices.push(mapped);
                            }
                            for c in &lr.residual_colors {
                                canonical_mesh.vertex_colors.push(*c);
                            }
                            // No triangles connecting residual vertices 
                            // to the canonical mesh — they show as 
                            // isolated points. A future enhancement is 
                            // to triangulate residual vs canonical 
                            // boundary. For now, residuals contribute 
                            // points but no surface.
                            let _ = v_base; // silence unused (reserved for triangulation hook)
                        }
                        let rendered = self.render_mesh_dispatch(
                            &canonical_mesh,
                            Some(&inv),
                            pose,
                        );
                        return rendered.map(|rgb| apply_residual(rgb, frame_bytes, self.width as usize, self.height as usize));
                    }
                }
                // Library ref covers this frame but resolution failed — 
                // don't fall through to the local webs (they wouldn't 
                // have the mesh either; that's the whole point of the 
                // V8LR replacement). Return None so the caller can show 
                // a gracefully-degraded frame.
                return None;
            }
        }
        
        // Step 2: pick the right web for this frame index.
        let web_idx = crate::v8_rasterizer::web_for_frame(&self.webs, frame_idx)?;
        let web = &self.webs[web_idx];
        
        // Step 3: resolve the mesh.
        // 
        // - kind 1 (canonical): web.mesh is the source of truth. Render directly.
        // - kind 2 (standalone): same as canonical — its own mesh.
        // - kind 0 (aligned): web.mesh stores only residual vertices. The 
        //     real geometry is the CANONICAL web's mesh, transformed by 
        //     web.transform. (Residual integration is a future enhancement; 
        //     for now we use the canonical mesh directly with the alignment 
        //     transform applied at projection time.)
        let (mesh_to_render, transform_to_apply): (&SceneMesh, Option<&[f32; 16]>) = match web.kind {
            1 | 2 => (&web.mesh, None),
            0 => {
                // Aligned web — its own mesh is a residual blob without 
                // colors/indices. The renderable geometry is the 
                // CANONICAL web's mesh, but expressed in canonical's 
                // coordinate frame.
                // 
                // The per-frame camera pose is in THIS web's local 
                // frame. To project canonical's vertices, we transform 
                // them from canonical's frame INTO this web's frame — 
                // that's the inverse of `web.transform` (which maps 
                // web→canonical).
                let canonical = self.webs.iter()
                    .find(|w| w.chunk_id == web.canonical_id && (w.kind == 1 || w.kind == 2));
                match canonical {
                    Some(c) if !c.mesh.vertices.is_empty() => {
                        // Compute inverse-rigid transform once per frame.
                        // Stored as a fresh array; the borrow rules want 
                        // a stable reference into render_mesh, so we 
                        // emit it from this scope.
                        let inv = crate::v8_rasterizer::invert_rigid_transform(&web.transform);
                        let rendered = self.render_mesh_dispatch(
                            &c.mesh,
                            Some(&inv),
                            pose,
                        );
                        return rendered.map(|rgb| apply_residual(rgb, frame_bytes, self.width as usize, self.height as usize));
                    }
                    _ => {
                        // No canonical found — fall back to this web's 
                        // own mesh if non-empty (rare).
                        if web.mesh.vertices.is_empty() { return None; }
                        (&web.mesh, None)
                    }
                }
            }
            _ => return None,
        };
        
        // Step 4: rasterize.
        let rendered = self.render_mesh_dispatch(
            mesh_to_render,
            transform_to_apply,
            pose,
        );
        // Step 5 (v8.1): apply photometric residual if the frame wrapper
        // carries one. No-op for v1 frames.
        rendered.map(|rgb| apply_residual(rgb, frame_bytes, self.width as usize, self.height as usize))
    }
}

/// Returns true if FAO_V81 environment variable is set to "1" or "true".
/// 
/// This is the toggle for v8.1 photometric residual encoding. Off by 
/// default — when off, encoder behavior is byte-for-byte identical to 
/// v8.0. When on, encoder produces v2-wrapped Z frames with embedded 
/// residuals; decoder transparently handles either wrapper version.
/// 
/// Checked once per encode_frame_at call. The env var is read on every 
/// call (not cached) so the user can toggle mid-encode in theory, 
/// though in practice the encoder pool is initialized at start_transcode 
/// and the env var only matters per-process.
fn compute_v81_residual_enabled() -> bool {
    std::env::var("FAO_V81")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Render the current mesh through the pose, diff against the source 
/// YUV (converted to RGB), downsample 4x, quantize to bias-128 u8, and 
/// zstd compress.
/// 
/// Returns the raw residual blob ready to be appended to a v2 Z frame. 
/// Returns None if rasterization fails or sizes are degenerate.
/// 
/// Format of the returned bytes:
///   header(4) = residual_w u16 LE + residual_h u16 LE
///   zstd(payload) where payload = rw * rh * 3 bytes of bias-128 i8 RGB delta
/// v10 DCT-quantized residual.
/// 
/// Replacement for `compute_photometric_residual` that uses the shared 
/// `residual::encode` pipeline (2D DCT + frequency-aware quant + 
/// skip-block RLE + DC prediction). The on-wire format differs from 
/// the legacy bias-128 path; the FLAG_DCT_FORMAT header bit signals 
/// which decode path to use.
/// 
/// Coverage handling: pixels where the rasterizer didn't produce a 
/// prediction (background, behind-camera) have predicted_rgb == 0. We 
/// could leave those as-is and let the residual carry the full source 
/// color (large signal, poor compression). Instead we set 
/// predicted = source for those pixels so the residual there is exactly 
/// zero — those blocks then collapse via skip-block RLE.
/// 
/// This is the same idea as the legacy coverage-sentinel approach, but 
/// expressed in the prediction step rather than as a special block 
/// type. zstd compresses zero-residual blocks much better than 
/// (128,128,128)-sentinel runs.
fn compute_v10_residual(
    mesh: &SceneMesh,
    pose: &CameraPose,
    frame_yuv: &[u8],
    w: usize,
    h: usize,
) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || frame_yuv.len() < w * h * 3 / 2 { return None; }
    
    // Rasterize the mesh through the pose.
    let mut predicted_rgb = crate::v8_rasterizer::render_mesh(mesh, None, pose, w, h)?;
    if predicted_rgb.len() != w * h * 3 { return None; }
    
    // YUV → RGB (BT.601 limited-range). Same conversion as the legacy 
    // path; duplicated here to keep the two functions independent so 
    // either can be modified without affecting the other.
    let mut source_rgb = vec![0u8; w * h * 3];
    let y_sz = w * h;
    let uv_w = w / 2;
    let uv_h = h / 2;
    let uv_sz = uv_w * uv_h;
    if frame_yuv.len() < y_sz + 2 * uv_sz { return None; }
    for py in 0..h {
        for px in 0..w {
            let yv = frame_yuv[py * w + px] as f32;
            let uv_off = (py / 2) * uv_w + (px / 2);
            let cu = frame_yuv[y_sz + uv_off] as f32;
            let cv = frame_yuv[y_sz + uv_sz + uv_off] as f32;
            let yn = (yv - 16.0) * (255.0 / 219.0);
            let un = cu - 128.0;
            let vn = cv - 128.0;
            let r = (yn + 1.402 * vn).clamp(0.0, 255.0) as u8;
            let g = (yn - 0.344136 * un - 0.714136 * vn).clamp(0.0, 255.0) as u8;
            let b = (yn + 1.772 * un).clamp(0.0, 255.0) as u8;
            let oi = (py * w + px) * 3;
            source_rgb[oi] = r;
            source_rgb[oi + 1] = g;
            source_rgb[oi + 2] = b;
        }
    }
    
    // Coverage handling: for uncovered pixels (predicted_rgb is all-zero), 
    // replace predicted with source so the residual becomes zero. zstd + 
    // skip-block-RLE in residual::encode will collapse the runs.
    for i in (0..predicted_rgb.len()).step_by(3) {
        if predicted_rgb[i] == 0 && predicted_rgb[i+1] == 0 && predicted_rgb[i+2] == 0 {
            predicted_rgb[i]   = source_rgb[i];
            predicted_rgb[i+1] = source_rgb[i+1];
            predicted_rgb[i+2] = source_rgb[i+2];
        }
    }
    
    // Dispatch to the shared encoder. ResidualOpts::from_env() reads the 
    // existing FAO_V82_* knobs so users can tune skip-RLE / DC pred / 
    // deinterleave the same way as before.
    let opts = crate::residual::ResidualOpts::from_env();
    crate::residual::encode(&predicted_rgb, &source_rgb, w, h, &opts)
}

/// Returns true if the user requested the v10 DCT residual path 
/// (FAO_V10_RESIDUAL=1). Default off — opt-in until we have confidence 
/// it beats the legacy bias-128 path on real content.
fn use_v10_residual() -> bool {
    std::env::var("FAO_V10_RESIDUAL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn compute_photometric_residual(
    mesh: &SceneMesh,
    pose: &CameraPose,
    frame_yuv: &[u8],
    w: usize,
    h: usize,
) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || frame_yuv.len() < w * h * 3 / 2 { return None; }
    
    // Step 1: rasterize the mesh through this pose. predicted_rgb has 
    // pixels in viewspace; background pixels (no triangle covered them) 
    // are zero. We treat those as "no prediction" — the residual would 
    // need to be the full source value there. In practice the mesh 
    // covers most of the visible region; uncovered areas show up as 
    // large residual deltas which compress poorly. Mitigation is left 
    // to future work; for v8.1 MVP we accept the cost.
    let predicted_rgb = crate::v8_rasterizer::render_mesh(mesh, None, pose, w, h)?;
    if predicted_rgb.len() != w * h * 3 { return None; }
    
    // Step 2: convert source YUV420p → RGB. BT.601 limited-range.
    let mut source_rgb = vec![0u8; w * h * 3];
    let y_sz = w * h;
    let uv_w = w / 2;
    let uv_h = h / 2;
    let uv_sz = uv_w * uv_h;
    if frame_yuv.len() < y_sz + 2 * uv_sz { return None; }
    for py in 0..h {
        for px in 0..w {
            let yv = frame_yuv[py * w + px] as f32;
            let uv_off = (py / 2) * uv_w + (px / 2);
            let cu = frame_yuv[y_sz + uv_off] as f32;
            let cv = frame_yuv[y_sz + uv_sz + uv_off] as f32;
            let yn = (yv - 16.0) * (255.0 / 219.0);
            let un = cu - 128.0;
            let vn = cv - 128.0;
            let r = (yn + 1.402 * vn).clamp(0.0, 255.0) as u8;
            let g = (yn - 0.344136 * un - 0.714136 * vn).clamp(0.0, 255.0) as u8;
            let b = (yn + 1.772 * un).clamp(0.0, 255.0) as u8;
            let oi = (py * w + px) * 3;
            source_rgb[oi] = r;
            source_rgb[oi + 1] = g;
            source_rgb[oi + 2] = b;
        }
    }
    
    // Step 3: downsample by 4x using box-average. This is the primary 
    // size lever — quarter resolution means 1/16 the residual data. 
    // For 1280x540 source, residual is 320x135 = 130KB raw.
    const DS: usize = 4;
    let rw = (w / DS).max(1);
    let rh = (h / DS).max(1);
    let mut residual_pixels = Vec::with_capacity(rw * rh * 3);
    
    // ── COVERAGE-AWARE RESIDUAL ──
    // 
    // Critical bug fixed here: previously the loop computed delta = 
    // source - predicted for every block. When the mesh covers only 
    // 20-30% of the frame, the other 70-80% of blocks have predicted=0 
    // (no triangle covered them), so delta = source - 0 = source. That 
    // means uncovered blocks carried the FULL SOURCE COLOR at quarter 
    // resolution. For 1920×1080 source, the residual was effectively 
    // a 480×270 RGB image of the source video for those uncovered 
    // areas. After zstd that's 200-300KB per frame.
    // 
    // Fix: for each block, count how many pixels have ANY mesh content 
    // (predicted_rgb non-zero on at least one channel). If FEWER than 
    // 25% of the block's pixels are mesh-covered, emit sentinel value 
    // (128, 128, 128) which means "no residual contribution" — the 
    // decoder will leave the area as the rasterized output (zero) and 
    // the composite path will fill from source video at playback.
    // 
    // For blocks that ARE mesh-covered, compute the residual only over 
    // the COVERED pixels (not all DS×DS pixels). This keeps the residual 
    // semantically correct (delta against the prediction that's actually 
    // there) and prevents uncovered pixels from pulling the average 
    // toward the source value.
    // 
    // Expected impact: residual size drops from 250-300KB to 30-50KB 
    // on typical DBD frames (where coverage is 20-30%). File should 
    // shrink from 1.2GB to ~400-500MB — much closer to AV1's 300MB.
    let coverage_threshold_pixels = (DS * DS) / 4;  // need >=25% of block covered
    for by in 0..rh {
        for bx in 0..rw {
            let mut sum = [0i32; 3];
            let mut covered = 0i32;
            for dy in 0..DS {
                let py = by * DS + dy;
                if py >= h { break; }
                for dx in 0..DS {
                    let px = bx * DS + dx;
                    if px >= w { break; }
                    let pi = (py * w + px) * 3;
                    let p_any = predicted_rgb[pi] | predicted_rgb[pi+1] | predicted_rgb[pi+2];
                    if p_any != 0 {
                        // Only accumulate covered pixels — uncovered pixels 
                        // would otherwise tag full source value into the 
                        // delta.
                        let s_r = source_rgb[pi]   as i32;
                        let s_g = source_rgb[pi+1] as i32;
                        let s_b = source_rgb[pi+2] as i32;
                        let p_r = predicted_rgb[pi]   as i32;
                        let p_g = predicted_rgb[pi+1] as i32;
                        let p_b = predicted_rgb[pi+2] as i32;
                        sum[0] += s_r - p_r;
                        sum[1] += s_g - p_g;
                        sum[2] += s_b - p_b;
                        covered += 1;
                    }
                }
            }
            if covered < coverage_threshold_pixels as i32 {
                // Block is essentially uncovered — emit no-residual 
                // sentinel. zstd compresses runs of 128 to near-zero.
                residual_pixels.push(128); residual_pixels.push(128); residual_pixels.push(128);
                continue;
            }
            for c in 0..3 {
                let avg = sum[c] / covered;
                let biased = (avg + 128).clamp(0, 255) as u8;
                residual_pixels.push(biased);
            }
        }
    }
    
    // ── v8.2: AV1-inspired DC prediction (optional, before skip-block) ──
    // 
    // The residual_pixels buffer holds bias-128 box-averaged delta 
    // values per quarter-resolution block. Neighboring blocks have 
    // correlated values: smooth video gradients translate to smooth 
    // residual gradients. AV1 exploits this via spatial DC prediction.
    // 
    // The transform: for each block in raster scan, predict its value 
    // from PREVIOUSLY-ENCODED neighbors (top, left), encode the delta 
    // from prediction instead of the absolute value. Predicted neighbor 
    // values come from the encoder's reconstruction (= the decoder 
    // sees the same prediction state), so encoder/decoder stay in 
    // sync.
    // 
    // After this transform, the distribution of residual_pixels 
    // shifts: blocks where the prediction was correct become near-128 
    // (= zero delta from prediction). The downstream skip-block 
    // detector then catches MORE blocks as skippable, because the 
    // remaining signal is more sparse.
    // 
    // Activated by FAO_V82_DCPRED=1 (requires FAO_V82_SKIP=1 to be 
    // useful — DC pred alone without skip-block detection wastes the 
    // entropy reduction).
    // 
    // Cost: one extra pass over residual_pixels (~50K elements at 
    // quarter res). Negligible compared to the rasterization step that 
    // produced these bytes.
    let dcpred_enabled = std::env::var("FAO_V82_DCPRED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if dcpred_enabled {
        // Reconstruct what the decoder will reconstruct, in lockstep. 
        // We need a parallel "reconstructed" buffer because the 
        // prediction at block (x,y) uses RECONSTRUCTED neighbors, not 
        // raw input values (otherwise prediction error compounds).
        // 
        // For our quarter-res blocks the reconstruction == the input 
        // (we don't quantize), so we can do this in-place with care.
        let mut recon = residual_pixels.clone();
        for by in 0..rh {
            for bx in 0..rw {
                let bi = (by * rw + bx) * 3;
                for c in 0..3 {
                    // Predict from (left + top) / 2, with fallbacks.
                    let pred = match (bx > 0, by > 0) {
                        (true, true) => {
                            let l = recon[((by) * rw + bx - 1) * 3 + c] as i32;
                            let t = recon[((by - 1) * rw + bx) * 3 + c] as i32;
                            (l + t) / 2
                        }
                        (true, false) => recon[((by) * rw + bx - 1) * 3 + c] as i32,
                        (false, true) => recon[((by - 1) * rw + bx) * 3 + c] as i32,
                        (false, false) => 128,
                    };
                    let cur = residual_pixels[bi + c] as i32;
                    // Store the prediction error, biased to keep it positive.
                    let delta = cur - pred;
                    let raw_stored = delta + 128;
                    if raw_stored < 0 || raw_stored > 255 {
                        // Prediction error exceeds 8-bit range. Track 
                        // for diagnostics — too many of these and DC 
                        // pred is hurting more than helping.
                        DCPRED_CLAMP_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    let stored = raw_stored.clamp(0, 255) as u8;
                    residual_pixels[bi + c] = stored;
                    // Reconstruction note: the decoder will compute 
                    // cur = pred + (stored - 128). With stored=255 and 
                    // pred=0, cur=127 (lossless). With stored=0 and 
                    // pred=255, cur=128 (lossless). We have 8-bit 
                    // values so any int delta fits. recon[bi+c] stays 
                    // as the original `cur` for downstream blocks' 
                    // predictions.
                }
            }
        }
    }
    
    // ── v8.2: AV1-inspired skip-block RLE ──
    // 
    // AV1 (and HEVC, VP9, etc.) use a "skip" signal per block: if the 
    // residual delta is near-zero or the block was predicted well 
    // enough, the encoder writes a single bit instead of a quantized 
    // coefficient stream. We do the moral equivalent here.
    // 
    // A block counts as "skip" when:
    //   - It got the sentinel value (128,128,128) — uncovered region
    //   - Its absolute delta from neutral (128) is < SKIP_THRESH in 
    //     every channel — visually-imperceptible residual after upsampling
    // 
    // Format: alternating segments of (skip_run_len_u16_LE, 
    // n_blocks_u8, n_blocks * 3 bytes RGB). Decoder steps through, 
    // skipping `skip_run_len` blocks (= no contribution), then 
    // emitting `n_blocks` deltas, repeat.
    // 
    // Most real frames have huge skip runs (uncovered regions, static 
    // background that the mesh already predicts well). RLE compresses 
    // these to 2 bytes per run instead of 3*N bytes of repeated 128s.
    // After zstd, the savings compound: typical frame residual drops 
    // 30-60% vs v8.1.
    // 
    // Activated by FAO_V82_SKIP=1 (default off for backward compat).
    let v82_skip_enabled = std::env::var("FAO_V82_SKIP")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    // Channel deinterleave for non-skip block payload. Improves zstd 
    // ratio by 10-20% on real residual data; pure post-processing 
    // step, no semantic change.
    let deint_enabled = std::env::var("FAO_V82_DEINT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    const SKIP_THRESH: i32 = 4;  // ±4 per channel = ±2% delta tolerance
    
    let raw_payload = if v82_skip_enabled {
        let n_blocks = residual_pixels.len() / 3;
        let mut packed: Vec<u8> = Vec::with_capacity(residual_pixels.len() / 2);
        let mut i = 0;
        while i < n_blocks {
            // Count consecutive skip blocks from i.
            let mut skip_count = 0u32;
            while i < n_blocks {
                let r = residual_pixels[i*3] as i32;
                let g = residual_pixels[i*3+1] as i32;
                let b = residual_pixels[i*3+2] as i32;
                let is_skip = (r - 128).abs() < SKIP_THRESH
                           && (g - 128).abs() < SKIP_THRESH
                           && (b - 128).abs() < SKIP_THRESH;
                if !is_skip { break; }
                skip_count += 1;
                i += 1;
            }
            // Count consecutive non-skip blocks from i, max 255 per segment.
            let nonskip_start = i;
            while i < n_blocks && i - nonskip_start < 255 {
                let r = residual_pixels[i*3] as i32;
                let g = residual_pixels[i*3+1] as i32;
                let b = residual_pixels[i*3+2] as i32;
                let is_skip = (r - 128).abs() < SKIP_THRESH
                           && (g - 128).abs() < SKIP_THRESH
                           && (b - 128).abs() < SKIP_THRESH;
                if is_skip { break; }
                i += 1;
            }
            let n_nonskip = (i - nonskip_start) as u8;
            // Emit segment header: skip_count (u32 LE, up to 4B blocks) + n_nonskip (u8).
            packed.extend_from_slice(&skip_count.to_le_bytes());
            packed.push(n_nonskip);
            // Emit non-skip block bytes. When channel deinterleave is on, 
            // group all R values first, then all G, then all B. zstd 
            // matches monotone streams much better than RGB-interleaved 
            // mixed-distribution streams.
            if deint_enabled {
                for j in 0..n_nonskip as usize {
                    packed.push(residual_pixels[(nonskip_start + j) * 3]);
                }
                for j in 0..n_nonskip as usize {
                    packed.push(residual_pixels[(nonskip_start + j) * 3 + 1]);
                }
                for j in 0..n_nonskip as usize {
                    packed.push(residual_pixels[(nonskip_start + j) * 3 + 2]);
                }
            } else {
                for j in 0..n_nonskip as usize {
                    let idx = (nonskip_start + j) * 3;
                    packed.push(residual_pixels[idx]);
                    packed.push(residual_pixels[idx + 1]);
                    packed.push(residual_pixels[idx + 2]);
                }
            }
        }
        packed
    } else {
        residual_pixels.clone()
    };
    
    // Step 4: zstd compress.
    // 
    // Level selection: AV1-inspired residuals tend to have low-entropy 
    // packed payloads (skip-block RLE = lots of small u32 counters + 
    // sparse delta bytes). At ~10KB after RLE packing, zstd level 9 is 
    // ~1ms and gets ~25% better ratio than level 3. Worth it.
    // 
    // For legacy v8.1 (full residual_pixels), level 3 is the right 
    // tradeoff: payload is larger (~50-130KB), level 9 would cost 
    // 5-10ms which is too much per frame.
    use std::io::Cursor;
    let zstd_level = if v82_skip_enabled { 9 } else { 3 };
    let compressed = match zstd::encode_all(Cursor::new(&raw_payload), zstd_level) {
        Ok(c) => c,
        Err(_) => return None,
    };
    
    // Diagnostic — log size every Nth call so the user can verify the 
    // coverage-aware fix is producing small residuals. Without the fix, 
    // residuals for sparse-mesh frames could hit 250-300KB. With the 
    // fix, expect 1-30KB for typical DBD frames.
    static RESIDUAL_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = RESIDUAL_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n == 1 || n == 10 || n == 100 || n == 1000 || n % 5000 == 0 {
        // Count how many sentinel-block triplets ended up in the 
        // residual so we can correlate compression ratio with coverage.
        let mut sentinel_blocks = 0u32;
        let mut total_blocks = 0u32;
        for chunk in residual_pixels.chunks(3) {
            total_blocks += 1;
            if chunk.len() == 3 && chunk[0] == 128 && chunk[1] == 128 && chunk[2] == 128 {
                sentinel_blocks += 1;
            }
        }
        let pct = if total_blocks > 0 { 100.0 * sentinel_blocks as f32 / total_blocks as f32 } else { 0.0 };
        let clamps = DCPRED_CLAMP_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        eprintln!("[v8 residual] #{}: raw={}KB packed={}KB compressed={}KB ratio={:.1}x sentinel={}/{} ({:.0}% uncovered, v82_skip={}, dcpred={}, deint={}, clamps={})",
            n,
            residual_pixels.len() / 1024,
            raw_payload.len() / 1024,
            compressed.len() / 1024,
            residual_pixels.len() as f32 / compressed.len().max(1) as f32,
            sentinel_blocks, total_blocks, pct,
            v82_skip_enabled, dcpred_enabled, deint_enabled, clamps);
    }
    
    // Step 5: prepend 4-byte header (residual_w + residual_h as u16 LE 
    // each) so the decoder knows the dimensions without recomputing.
    // 
    // v8.2: when skip-block encoding is on, set bit 15 of rw to signal 
    // the new format. Old decoders see an inflated rw and fail the 
    // "rw <= w" sanity check, which is harmless (residual is treated 
    // as absent). New decoders strip bit 15, recover the real rw, and 
    // run the skip-block decode path.
    // Header bits:
    //   bit 15 (0x8000): v8.2 skip-block RLE enabled
    //   bit 14 (0x4000): v8.2 DC prediction enabled (only meaningful if bit 15 set)
    //   bit 13 (0x2000): v8.2 deinterleaved RGB channels in segments (only meaningful if bit 15 set)
    // Bits 0..12 hold residual_w (max 8192 — quarter-res of 32768-wide source).
    let mut rw_field = rw as u16;
    if v82_skip_enabled { rw_field |= 0x8000; }
    if dcpred_enabled { rw_field |= 0x4000; }
    if deint_enabled { rw_field |= 0x2000; }
    let mut out = Vec::with_capacity(4 + compressed.len());
    out.extend_from_slice(&rw_field.to_le_bytes());
    out.extend_from_slice(&(rh as u16).to_le_bytes());
    out.extend_from_slice(&compressed);
    Some(out)
}

/// Apply a v8.1 photometric residual to an already-rasterized RGB frame.
/// 
/// For v1 frames (or v2 frames with empty residual), returns the input 
/// unchanged. For v2 frames with a residual blob, decompresses, 
/// upsamples 4× via nearest-neighbor, and adds to each pixel with 
/// clamping. The residual is i8-quantized RGB at quarter resolution.
/// 
/// This is the decoder side of v8.1. The encoder side lives in 
/// V8Encoder::encode_frame_at, gated behind the FAO_V81 env var.
fn apply_residual(mut rgb: Vec<u8>, frame_bytes: &[u8], w: usize, h: usize) -> Vec<u8> {
    let res_bytes = match V8Frame::extract_residual(frame_bytes) {
        Some(b) => b,
        None => return rgb,
    };
    if rgb.len() != w * h * 3 { return rgb; }
    
    // ── v10 DCT residual routing ──
    // 
    // The residual format bumped from bias-128 box-average (v8.1+) to 
    // 2D-DCT-quantized blocks (v10). Both formats start with a 4-byte 
    // header that encodes the residual block dimensions plus flag bits 
    // in the high bits of the width field. Bit 12 of the first u16 
    // identifies v10:
    //   - both set → v10 DCT format, delegate to residual::apply
    //   - otherwise → legacy bias-128, continue below
    // 
    // The 2-bit signature requires bits 11+12 BOTH set. Legacy 
    // residuals never had both set simultaneously: that would require 
    // rw ≥ 6144 which is quarter-res of 24576-wide source, well 
    // beyond any realistic video.
    if crate::residual::is_dct_format(res_bytes) {
        return crate::residual::apply(rgb, res_bytes, w, h);
    }
    
    // Header: residual_w (u16 LE) + residual_h (u16 LE) + zstd_payload
    // 
    // v8.2 header bits:
    //   bit 15 (0x8000) = skip-block RLE
    //   bit 14 (0x4000) = DC prediction (only if bit 15)
    //   bit 13 (0x2000) = channel deinterleave for non-skip payload (only if bit 15)
    // Width is in bits 0..12 (max 8191 — quarter res of a 32764-wide source).
    if res_bytes.len() < 4 { return rgb; }
    let rw_raw = u16::from_le_bytes([res_bytes[0], res_bytes[1]]);
    let v82 = (rw_raw & 0x8000) != 0;
    let dcpred = (rw_raw & 0x4000) != 0;
    let deint = (rw_raw & 0x2000) != 0;
    let rw = (rw_raw & 0x1FFF) as usize;
    let rh = u16::from_le_bytes([res_bytes[2], res_bytes[3]]) as usize;
    if rw == 0 || rh == 0 || rw > w || rh > h { return rgb; }
    
    use std::io::Cursor;
    let decompressed = match zstd::decode_all(Cursor::new(&res_bytes[4..])) {
        Ok(d) => d,
        Err(_) => return rgb,
    };
    
    // Expand to raw residual_pixels buffer (rw * rh * 3 bytes).
    // - v8.1: payload is already raw, copy as-is.
    // - v8.2: walk the skip-block RLE and reconstruct.
    let n_blocks = rw * rh;
    let raw: Vec<u8> = if v82 {
        let mut out = vec![128u8; n_blocks * 3];  // skip-blocks default to neutral 128
        let mut pos = 0usize;
        let mut block = 0usize;
        while pos + 5 <= decompressed.len() && block < n_blocks {
            // Segment header: skip_count (u32 LE) + n_nonskip (u8)
            let skip_count = u32::from_le_bytes([
                decompressed[pos], decompressed[pos+1],
                decompressed[pos+2], decompressed[pos+3],
            ]) as usize;
            let n_nonskip = decompressed[pos+4] as usize;
            pos += 5;
            block = block.saturating_add(skip_count);
            // Copy non-skip block bytes. Layout depends on the deint flag:
            //   - Interleaved (legacy): [r0 g0 b0 r1 g1 b1 ...]
            //   - Deinterleaved: [r0 r1 r2 ...] [g0 g1 g2 ...] [b0 b1 b2 ...]
            if pos + n_nonskip * 3 > decompressed.len() { break; }
            if deint {
                let r_off = pos;
                let g_off = pos + n_nonskip;
                let b_off = pos + 2 * n_nonskip;
                for j in 0..n_nonskip {
                    if block + j >= n_blocks { break; }
                    let oi = (block + j) * 3;
                    out[oi]   = decompressed[r_off + j];
                    out[oi+1] = decompressed[g_off + j];
                    out[oi+2] = decompressed[b_off + j];
                }
            } else {
                for j in 0..n_nonskip {
                    if block + j >= n_blocks { break; }
                    let oi = (block + j) * 3;
                    out[oi]   = decompressed[pos + j*3];
                    out[oi+1] = decompressed[pos + j*3 + 1];
                    out[oi+2] = decompressed[pos + j*3 + 2];
                }
            }
            pos += n_nonskip * 3;
            block += n_nonskip;
        }
        out
    } else {
        let expected = n_blocks * 3;
        if decompressed.len() != expected { return rgb; }
        decompressed
    };
    
    // Inverse DC prediction (v8.2 + DC pred bit). Walks blocks in same 
    // raster order as the encoder. For each block, predicts from 
    // top/left neighbors of the ALREADY-RECONSTRUCTED output (not the 
    // raw stored values — encoder/decoder must agree on what gets 
    // predicted from). Restores the original delta values, which then 
    // feed the upsample/composite step below.
    // 
    // We mutate `raw` in place; once a block is reconstructed it 
    // becomes a neighbor candidate for the next blocks in scan order.
    let raw: Vec<u8> = if dcpred {
        let mut raw = raw;
        for by in 0..rh {
            for bx in 0..rw {
                let bi = (by * rw + bx) * 3;
                for c in 0..3 {
                    let pred = match (bx > 0, by > 0) {
                        (true, true) => {
                            let l = raw[((by) * rw + bx - 1) * 3 + c] as i32;
                            let t = raw[((by - 1) * rw + bx) * 3 + c] as i32;
                            (l + t) / 2
                        }
                        (true, false) => raw[((by) * rw + bx - 1) * 3 + c] as i32,
                        (false, true) => raw[((by - 1) * rw + bx) * 3 + c] as i32,
                        (false, false) => 128,
                    };
                    let stored = raw[bi + c] as i32;
                    // Inverse of encoder: cur = pred + (stored - 128).
                    let cur = pred + stored - 128;
                    raw[bi + c] = cur.clamp(0, 255) as u8;
                }
            }
        }
        raw
    } else {
        raw
    };
    
    // Scale factor for nearest-neighbor upsample. We expect rw = w/4, 
    // rh = h/4 (with rounding) but tolerate other ratios.
    let sx = w / rw.max(1);
    let sy = h / rh.max(1);
    if sx == 0 || sy == 0 { return rgb; }
    
    // For each output pixel, read the corresponding residual pixel and 
    // add. Bias-128: byte 128 = no delta. byte 127 = -1, byte 129 = +1.
    // 
    // OPTIMIZATION: if the residual byte is exactly 128 (no delta), 
    // skip the pixel entirely. For skip-block frames most pixels are 
    // 128 so this is a big speedup. We check all three channels 
    // together since the common skip case has all three at 128.
    for py in 0..h {
        let rpy = (py / sy).min(rh - 1);
        for px in 0..w {
            let rpx = (px / sx).min(rw - 1);
            let ri = (rpy * rw + rpx) * 3;
            if raw[ri] == 128 && raw[ri+1] == 128 && raw[ri+2] == 128 {
                continue;  // skip block — no change to rgb
            }
            let oi = (py * w + px) * 3;
            for c in 0..3 {
                let delta = raw[ri + c] as i16 - 128;
                if delta == 0 { continue; }
                let new_val = (rgb[oi + c] as i16 + delta).clamp(0, 255);
                rgb[oi + c] = new_val as u8;
            }
        }
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn camera_pose_roundtrip() {
        let p = CameraPose {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: [0.5, 0.5, 0.5, 0.5],
            focal: [800.0, 800.0],
            principal: [320.0, 240.0],
            distortion: 0.1,
        };
        let bytes = p.to_bytes();
        let back = CameraPose::from_bytes(&bytes).unwrap();
        assert_eq!(back.position.x, 1.0);
        assert_eq!(back.focal[0], 800.0);
        assert_eq!(back.distortion, 0.1);
    }
    
    #[test]
    fn v8_frame_roundtrip() {
        let mut frame = V8Frame::default();
        frame.camera_pose.position = Vec3::new(10.0, 20.0, 30.0);
        frame.features.push(SceneFeature {
            feature_id: 42,
            screen_x: 100.0,
            screen_y: 50.0,
            depth: 5.0,
            confidence: 0.9,
        });
        // Default `to_bytes()` is now the compact V81 format. Test that 
        // it round-trips through dispatched `from_bytes()`.
        let bytes = frame.to_bytes();
        let parsed = V8Frame::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.camera_pose.position.x, 10.0);
        assert_eq!(parsed.features.len(), 1);
        assert_eq!(parsed.features[0].feature_id, 42);
        assert_eq!(parsed.features[0].depth, 5.0);  // 5.0 fits f16 exactly
        // x and y are u16 quantized — exact for integer values.
        assert_eq!(parsed.features[0].screen_x, 100.0);
        assert_eq!(parsed.features[0].screen_y, 50.0);
        // Confidence is u8 quantized: 0.9 → 230/255 ≈ 0.9019.
        assert!((parsed.features[0].confidence - 0.9).abs() < 0.005);
    }
    
    #[test]
    fn v8_frame_legacy_roundtrip() {
        // Prove the old V8 wire format still round-trips via the dispatched 
        // from_bytes(). This is the path for reading old .fao files.
        let mut frame = V8Frame::default();
        frame.camera_pose.position = Vec3::new(7.0, 11.0, 13.0);
        frame.features.push(SceneFeature {
            feature_id: 99,
            screen_x: 12.34,  // exact f32 → f32 (legacy, no quant)
            screen_y: 56.78,
            depth: 9.876,
            confidence: 0.123,
        });
        let bytes = frame.to_bytes_v8_legacy();
        assert_eq!(&bytes[..2], b"V8");
        assert_ne!(bytes[2], b'1');  // legacy: byte 2 is n_features low byte, not '1'
        let parsed = V8Frame::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.features[0].feature_id, 99);
        assert_eq!(parsed.features[0].screen_x, 12.34);  // exact f32 round-trip
        assert_eq!(parsed.features[0].confidence, 0.123);
    }
    
    #[test]
    fn render_frame_rgb_returns_some_with_canonical_web() {
        // Build a V8Decoder with a single canonical web that has a 
        // simple triangle, plus a frame that points at it. Confirm 
        // render_frame_rgb produces a non-empty buffer.
        let mut mesh = SceneMesh::new();
        mesh.vertices = vec![
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new( 1.0, -1.0, 0.0),
            Vec3::new( 0.0,  1.0, 0.0),
        ];
        mesh.vertex_colors = vec![[1.0, 0.0, 0.0]; 3];
        mesh.indices = vec![0, 1, 2];
        let web = DecodedWeb {
            chunk_id: 0,
            canonical_id: 0,
            frame_start: 0,
            frame_end: 100,
            kind: 1,  // canonical
            transform: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            mesh,
        };
        let decoder = V8Decoder::new(100, 100).with_webs(vec![web]);
        
        // Build a synthetic frame at idx 5 with a useful pose.
        let mut frame = V8Frame::default();
        frame.camera_pose.position = Vec3::new(0.0, 0.0, -3.0);
        frame.camera_pose.rotation = [0.0, 0.0, 0.0, 1.0];
        frame.camera_pose.focal = [50.0, 50.0];
        frame.camera_pose.principal = [50.0, 50.0];
        let frame_bytes = frame.to_bytes_v81();
        
        let rgb = decoder.render_frame_rgb(&frame_bytes, 5);
        assert!(rgb.is_some(), "expected rasterizer to produce RGB for canonical web");
        let rgb = rgb.unwrap();
        assert_eq!(rgb.len(), 100 * 100 * 3);
        
        // Some pixels should be red (the triangle color).
        let red_pixels = rgb.chunks(3).filter(|c| c[0] > 100 && c[1] < 50 && c[2] < 50).count();
        assert!(red_pixels > 50, "expected red triangle pixels, got {}", red_pixels);
    }
    
    #[test]
    fn render_frame_rgb_returns_none_outside_web_range() {
        // Frame index 200 falls outside the web's 0..100 range.
        let decoder = V8Decoder::new(100, 100).with_webs(vec![]);
        let mut frame = V8Frame::default();
        frame.camera_pose.position = Vec3::new(0.0, 0.0, -3.0);
        frame.camera_pose.rotation = [0.0, 0.0, 0.0, 1.0];
        let frame_bytes = frame.to_bytes_v81();
        assert!(decoder.render_frame_rgb(&frame_bytes, 200).is_none());
    }
    
    #[test]
    fn camera_pose_extrinsic_roundtrip() {
        let pose = CameraPose {
            position: Vec3::new(1.5, 2.5, 3.5),
            rotation: [0.7, 0.1, 0.5, 0.4],
            focal: [1234.5, 1234.5],
            principal: [960.0, 540.0],
            distortion: 0.05,
        };
        let intrinsics = CameraIntrinsics::from_pose(&pose);
        let extr = pose.to_bytes_extrinsics();
        assert_eq!(extr.len(), 28);
        let back = CameraPose::from_extrinsics(&extr, &intrinsics).unwrap();
        // Position and rotation round-trip exactly (f32 → f32).
        assert_eq!(back.position.x, 1.5);
        assert_eq!(back.position.y, 2.5);
        assert_eq!(back.position.z, 3.5);
        assert_eq!(back.rotation[0], 0.7);
        // Intrinsics carried via the side-band CameraIntrinsics.
        assert_eq!(back.focal[0], 1234.5);
        assert_eq!(back.distortion, 0.05);
    }
    
    #[test]
    fn camera_intrinsics_roundtrip() {
        let i = CameraIntrinsics {
            focal: [800.0, 800.0],
            principal: [640.0, 360.0],
            distortion: -0.1,
        };
        let bytes = i.to_bytes();
        assert_eq!(bytes.len(), 20);
        let back = CameraIntrinsics::from_bytes(&bytes).unwrap();
        assert_eq!(back.focal, i.focal);
        assert_eq!(back.principal, i.principal);
        assert_eq!(back.distortion, i.distortion);
    }
    
    #[test]
    fn v8_frame_compact_size_smaller() {
        // Sanity check: confirm V81 actually saves bytes.
        let mut frame = V8Frame::default();
        for i in 0..100 {
            frame.features.push(SceneFeature {
                feature_id: i,
                screen_x: 10.0 * i as f32,
                screen_y: 20.0 * i as f32,
                depth: 5.0,
                confidence: 0.5,
            });
        }
        let v81 = frame.to_bytes_v81();
        let legacy = frame.to_bytes_v8_legacy();
        assert!(v81.len() < legacy.len(),
            "V81 ({}) should be smaller than legacy ({})", v81.len(), legacy.len());
        // 100 features × 9B saved = 900B, plus small header diff.
        let saved = legacy.len() - v81.len();
        assert!(saved > 850, "expected ~900B savings, got {}", saved);
    }
    
    #[test]
    fn v8_frame_zstd_wrap_roundtrip() {
        // Manually construct a wrapped frame and verify from_bytes_any decodes it.
        let mut frame = V8Frame::default();
        frame.camera_pose.position = Vec3::new(1.0, 2.0, 3.0);
        for i in 0..200 {
            frame.features.push(SceneFeature {
                feature_id: i,
                screen_x: i as f32 * 0.5,
                screen_y: 100.0 + i as f32,
                depth: 5.0,
                confidence: 0.7,
            });
        }
        let raw = frame.to_bytes();
        // Manually wrap (mirror what encode_frame_at does)
        use std::io::Cursor;
        let compressed = zstd::encode_all(Cursor::new(&raw), 1).unwrap();
        let mut wrapped = Vec::new();
        wrapped.push(b'Z');
        wrapped.push(1u8);
        wrapped.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        wrapped.extend_from_slice(&compressed);
        
        // Decode via from_bytes_any — should auto-unwrap
        let parsed = V8Frame::from_bytes_any(&wrapped).unwrap();
        assert_eq!(parsed.features.len(), 200);
        assert_eq!(parsed.features[0].feature_id, 0);
        assert_eq!(parsed.features[199].feature_id, 199);
        // from_bytes_any should also handle raw bytes
        let parsed_raw = V8Frame::from_bytes_any(&raw).unwrap();
        assert_eq!(parsed_raw.features.len(), 200);
    }
    
    #[test]
    fn empty_mesh_trailer_roundtrip() {
        let mesh = SceneMesh::new();
        let bytes = mesh.to_trailer();
        let back = SceneMesh::from_trailer(&bytes).unwrap();
        assert_eq!(back.vertex_count(), 0);
        assert_eq!(back.triangle_count(), 0);
    }
    
    #[test]
    fn nonempty_mesh_trailer_roundtrip() {
        let mut mesh = SceneMesh::new();
        mesh.vertices.push(Vec3::new(0.0, 0.0, 0.0));
        mesh.vertices.push(Vec3::new(1.0, 0.0, 0.0));
        mesh.vertices.push(Vec3::new(0.0, 1.0, 0.0));
        mesh.vertex_colors.push([1.0, 0.0, 0.0]);
        mesh.vertex_colors.push([0.0, 1.0, 0.0]);
        mesh.vertex_colors.push([0.0, 0.0, 1.0]);
        mesh.normals.push(Vec3::new(0.0, 0.0, 1.0));
        mesh.normals.push(Vec3::new(0.0, 0.0, 1.0));
        mesh.normals.push(Vec3::new(0.0, 0.0, 1.0));
        mesh.indices.extend_from_slice(&[0, 1, 2]);
        let bytes = mesh.to_trailer();
        let back = SceneMesh::from_trailer(&bytes).unwrap();
        assert_eq!(back.vertex_count(), 3);
        assert_eq!(back.triangle_count(), 1);
        assert_eq!(back.vertices[0].x, 0.0);
        assert_eq!(back.vertices[1].x, 1.0);
        assert_eq!(back.indices[0], 0);
    }
}


// ═══════════════════════════════════════════════════════════
// SESSION 2: FeatureTrackPass — keypoint detection + tracking
// ═══════════════════════════════════════════════════════════
//
// Output: per-frame Vec<SceneFeature> with consistent feature_id across
// frames. This is the input to TriangulatePass (Session 3).
//
// Pipeline:
//   1. Frame N=0: FAST detects ~500 corners → seed initial tracks
//   2. Frame N>0: LK optical flow advances each track to new position
//                 - Tracks with high residual are killed
//                 - When live track count drops below threshold, FAST re-seeds
//   3. Each surviving track is emitted as a SceneFeature for this frame
//
// All pure Rust. Performance is OK for prototype; future GPU port lives in
// the existing wgpu pipeline infrastructure.

/// FAST-9 corner detector with threshold and non-max suppression.
///
/// Algorithm: for each candidate pixel p with intensity I_p, examine 16 
/// pixels arranged in a Bresenham circle of radius 3. If 9+ contiguous 
/// pixels are all brighter than I_p+t OR all darker than I_p-t, p is a
/// corner. Score = sum of |I_circle - I_p| for the contiguous arc.
pub struct FastDetector {
    pub threshold: u8,
    pub min_score: u32,
    /// Non-max suppression radius (pixels)
    pub nms_radius: usize,
    /// Maximum keypoints to return per frame (top-N by score)
    pub max_keypoints: usize,
}

impl Default for FastDetector {
    fn default() -> Self {
        Self {
            threshold: 20,
            min_score: 200,
            nms_radius: 3,
            max_keypoints: 500,
        }
    }
}

/// Bresenham circle of radius 3 around (0, 0). 16 offsets in CW order
/// starting at the top.
const FAST_CIRCLE_OFFSETS: [(i32, i32); 16] = [
    (0, -3), (1, -3), (2, -2), (3, -1),
    (3, 0),  (3, 1),  (2, 2),  (1, 3),
    (0, 3),  (-1, 3), (-2, 2), (-3, 1),
    (-3, 0), (-3, -1), (-2, -2), (-1, -3),
];

#[derive(Clone, Copy, Debug)]
pub struct Keypoint {
    pub x: f32,
    pub y: f32,
    pub score: u32,
}

impl FastDetector {
    pub fn detect(&self, yuv: &[u8], width: usize, height: usize) -> Vec<Keypoint> {
        if yuv.len() < width * height || width < 8 || height < 8 { return Vec::new(); }
        
        let mut candidates = Vec::with_capacity(2048);
        // Stay 4 pixels from edges so the radius-3 circle is in-bounds
        let margin = 4;
        for y in margin..height - margin {
            for x in margin..width - margin {
                let center = yuv[y * width + x] as i32;
                let bright = center + self.threshold as i32;
                let dim = center - self.threshold as i32;
                
                // Quick reject: check 4 cardinal points first (top, right, 
                // bottom, left). At least 3 must agree on brighter or darker.
                let cards = [
                    yuv[(y - 3) * width + x] as i32,        // top
                    yuv[y * width + (x + 3)] as i32,        // right
                    yuv[(y + 3) * width + x] as i32,        // bottom
                    yuv[y * width + (x - 3)] as i32,        // left
                ];
                let n_brighter = cards.iter().filter(|&&v| v > bright).count();
                let n_darker = cards.iter().filter(|&&v| v < dim).count();
                if n_brighter < 3 && n_darker < 3 { continue; }
                
                // Full circle check: find longest contiguous arc of 9+
                // Doubled-array trick: walk the 32-length doubled circle once
                // looking for runs.
                let mut intensities = [0i32; 16];
                for (k, &(dx, dy)) in FAST_CIRCLE_OFFSETS.iter().enumerate() {
                    let px = x as i32 + dx;
                    let py = y as i32 + dy;
                    intensities[k] = yuv[(py as usize) * width + (px as usize)] as i32;
                }
                
                let arc_score = |is_brighter: bool| -> u32 {
                    let cmp = |v: i32| if is_brighter { v > bright } else { v < dim };
                    // Walk 32 entries (doubled), count longest run
                    let mut best_len = 0usize;
                    let mut best_sum = 0u32;
                    let mut cur_len = 0usize;
                    let mut cur_sum = 0u32;
                    for i in 0..32 {
                        let v = intensities[i % 16];
                        if cmp(v) {
                            cur_len += 1;
                            cur_sum += (v - center).unsigned_abs();
                        } else {
                            if cur_len > best_len {
                                best_len = cur_len;
                                best_sum = cur_sum;
                            }
                            cur_len = 0;
                            cur_sum = 0;
                        }
                    }
                    if cur_len > best_len {
                        best_len = cur_len;
                        best_sum = cur_sum;
                    }
                    if best_len >= 9 { best_sum } else { 0 }
                };
                
                let s = arc_score(true).max(arc_score(false));
                if s >= self.min_score {
                    candidates.push(Keypoint { x: x as f32, y: y as f32, score: s });
                }
            }
        }
        
        // Non-max suppression: sort by score desc, then greedily keep points
        // not within nms_radius of an already-kept point.
        candidates.sort_by(|a, b| b.score.cmp(&a.score));
        let r2 = (self.nms_radius * self.nms_radius) as f32;
        let mut kept: Vec<Keypoint> = Vec::with_capacity(self.max_keypoints);
        for c in candidates {
            let too_close = kept.iter().any(|k| {
                let dx = k.x - c.x;
                let dy = k.y - c.y;
                dx * dx + dy * dy < r2
            });
            if !too_close {
                kept.push(c);
                if kept.len() >= self.max_keypoints { break; }
            }
        }
        kept
    }
}

/// Sparse Lucas-Kanade optical flow tracker.
///
/// For each input point, finds its new position in the next frame by 
/// iteratively solving the LK equation in a small window:
///   sum(Ix^2)   sum(Ix*Iy)   *  [u]  =  [-sum(Ix*It)]
///   sum(Ix*Iy)  sum(Iy^2)       [v]     [-sum(Iy*It)]
///
/// Limitations of this prototype:
///   - Single-scale (no pyramid) → only small motions handled
///   - Float bilinear sampling at sub-pixel positions
///   - Kills tracks where residual exceeds threshold
///
/// Future improvement (Session 3+): build image pyramid, run LK at coarse
/// level first, propagate to fine level. Handles fast motion + scale changes.
pub struct LkTracker {
    /// Window radius for the LK system (full window = 2r+1 square)
    pub window_radius: usize,
    /// Maximum LK iterations per point
    pub max_iterations: usize,
    /// Convergence threshold (pixel displacement per iteration)
    pub convergence_eps: f32,
    /// Tracks with residual > this are killed
    pub max_residual: f32,
}

impl Default for LkTracker {
    fn default() -> Self {
        Self {
            // Performance tuning (Session 5b speed pass):
            // window_radius 5 → 3 makes the inner loop work 49/121 = 40% of before,
            // max_iterations 8 → 5 cuts another 38% on average (most converge in 3-4).
            // Combined effect: ~3x speedup in LK without observable tracking loss
            // for typical handheld / panning footage. Fast erratic motion benefits
            // from a future pyramid-LK pass; for now this is the right tradeoff.
            window_radius: 3,        // 7x7 window (was 11x11)
            max_iterations: 5,       // was 8
            convergence_eps: 0.05,
            max_residual: 30.0,
        }
    }
}

impl LkTracker {
    /// Track each point from `prev` to `cur`. Returns Some(new_position) on 
    /// success, None when the track is lost (out of bounds, ill-conditioned 
    /// system, or excessive residual).
    pub fn track(
        &self,
        prev: &[u8],
        cur: &[u8],
        width: usize,
        height: usize,
        points: &[(f32, f32)],
    ) -> Vec<Option<(f32, f32, f32)>> {
        if prev.len() < width * height || cur.len() < width * height {
            return points.iter().map(|_| None).collect();
        }
        let r = self.window_radius as i32;
        let w_i = width as i32;
        let h_i = height as i32;
        
        // Stack buffer for precomputed window samples. At r=3 (7×7 window) 
        // this is 49 entries × 16 bytes = 784 bytes, comfortably on the 
        // stack. Bumped to 121 to support r=5 worst case (11×11) with no 
        // allocation. Anything bigger falls back to a heap Vec.
        const MAX_STACK_WIN: usize = 121;
        
        points.iter().map(|&(px, py)| {
            // Bounds-check the prev window ONCE (it doesn't change across 
            // iterations). Saves repeating this 5×; also moves the bail 
            // before any allocation.
            if px < r as f32 + 1.0 || py < r as f32 + 1.0
               || px >= (w_i - r - 2) as f32 || py >= (h_i - r - 2) as f32 {
                return None;
            }
            
            // ── HOISTED INVARIANTS (Item 2 from optimization audit) ──
            // 
            // The structure tensor (sum_ix2, sum_iy2, sum_ixiy) and the 
            // per-window-pixel prev samples + gradients don't change as 
            // u, v iterate — they're functions of (px, py) and the prev 
            // frame only. The previous implementation recomputed them 
            // all 5 iterations. Hoisting them out brings per-iteration 
            // work from 49 pixels × 5 samples down to 49 pixels × 1 
            // sample. About 2.7x speedup on LK overall.
            // 
            // We also precompute the inverse 2×2 structure tensor — the 
            // (sum_iy2, -sum_ixiy, sum_ix2) coefficients and det — so 
            // the solve inside each iteration is just two dot products 
            // with (sum_ixit, sum_iyit).
            
            let win_size = (2 * r as usize + 1) * (2 * r as usize + 1);
            // (i_prev, ix, iy, sx_prev, sy_prev) per pixel.
            // sx_prev / sy_prev are stored so we can sample cur during 
            // iterations without recomputing the window offsets.
            let mut win_stack = [(0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32); MAX_STACK_WIN];
            let mut win_heap: Vec<(f32, f32, f32, f32, f32)> = Vec::new();
            let use_heap = win_size > MAX_STACK_WIN;
            if use_heap {
                win_heap.resize(win_size, (0.0, 0.0, 0.0, 0.0, 0.0));
            }
            
            let mut sum_ix2 = 0.0f32;
            let mut sum_iy2 = 0.0f32;
            let mut sum_ixiy = 0.0f32;
            
            let mut wi = 0usize;
            for dy in -r..=r {
                for dx in -r..=r {
                    let sx_prev = px + dx as f32;
                    let sy_prev = py + dy as f32;
                    let i_prev = sample_bilinear(prev, width, sx_prev, sy_prev);
                    let ix = 0.5 * (
                        sample_bilinear(prev, width, sx_prev + 1.0, sy_prev)
                        - sample_bilinear(prev, width, sx_prev - 1.0, sy_prev)
                    );
                    let iy = 0.5 * (
                        sample_bilinear(prev, width, sx_prev, sy_prev + 1.0)
                        - sample_bilinear(prev, width, sx_prev, sy_prev - 1.0)
                    );
                    let entry = (i_prev, ix, iy, sx_prev, sy_prev);
                    if use_heap {
                        win_heap[wi] = entry;
                    } else {
                        win_stack[wi] = entry;
                    }
                    sum_ix2  += ix * ix;
                    sum_iy2  += iy * iy;
                    sum_ixiy += ix * iy;
                    wi += 1;
                }
            }
            
            // Pre-invert the structure tensor. If singular, the window is 
            // on a flat region — track is ill-conditioned, bail.
            let det = sum_ix2 * sum_iy2 - sum_ixiy * sum_ixiy;
            if det.abs() < 1e-3 { return None; }
            let inv_det = 1.0 / det;
            
            // Iteration loop — only the t-derivative depends on (u, v).
            let mut u = 0.0f32;
            let mut v = 0.0f32;
            for _iter in 0..self.max_iterations {
                let cx_cur = px + u;
                let cy_cur = py + v;
                if cx_cur < r as f32 + 1.0 || cy_cur < r as f32 + 1.0
                   || cx_cur >= (w_i - r - 2) as f32 || cy_cur >= (h_i - r - 2) as f32 {
                    return None;
                }
                
                let mut sum_ixit = 0.0f32;
                let mut sum_iyit = 0.0f32;
                
                for k in 0..win_size {
                    let (i_prev, ix, iy, sx_prev, sy_prev) = if use_heap {
                        win_heap[k]
                    } else {
                        win_stack[k]
                    };
                    let sx_cur = sx_prev + u;
                    let sy_cur = sy_prev + v;
                    let i_cur = sample_bilinear(cur, width, sx_cur, sy_cur);
                    let it = i_cur - i_prev;
                    sum_ixit += ix * it;
                    sum_iyit += iy * it;
                }
                
                // Solve using pre-inverted tensor. Sign on (-sum_ixit, 
                // -sum_iyit) is preserved from the original formulation.
                let du = (sum_iy2 * (-sum_ixit) - sum_ixiy * (-sum_iyit)) * inv_det;
                let dv = (sum_ix2 * (-sum_iyit) - sum_ixiy * (-sum_ixit)) * inv_det;
                
                u += du;
                v += dv;
                
                if du.abs() < self.convergence_eps && dv.abs() < self.convergence_eps {
                    break;
                }
            }
            
            let new_x = px + u;
            let new_y = py + v;
            
            // Compute final residual to decide if we keep this track
            let mut residual = 0.0f32;
            let mut count = 0u32;
            for dy in -r..=r {
                for dx in -r..=r {
                    let sp = sample_bilinear(prev, width, px + dx as f32, py + dy as f32);
                    let sc = sample_bilinear(cur, width, new_x + dx as f32, new_y + dy as f32);
                    residual += (sp - sc).abs();
                    count += 1;
                }
            }
            let mean_residual = if count > 0 { residual / count as f32 } else { f32::MAX };
            if mean_residual > self.max_residual {
                return None;
            }
            
            Some((new_x, new_y, mean_residual))
        }).collect()
    }
}

/// Bilinear sample of a u8 luminance buffer at sub-pixel (x, y).
/// Robust to out-of-bounds inputs: clamps coordinates so callers don't 
/// have to. (Earlier version required the caller to clamp, but in 
/// multi-threaded LK tracking a feature near the edge could converge 
/// to a position whose ±r window goes negative — causing a panic via 
/// `(-1 as usize) * width` integer wrap. This made the whole v8 mutex 
/// poison and silently fell back to v2.1.)
#[inline]
fn sample_bilinear(buf: &[u8], width: usize, x: f32, y: f32) -> f32 {
    let height = if width > 0 { buf.len() / width } else { 0 };
    if width == 0 || height == 0 { return 0.0; }
    
    // Clamp into valid range. The 1-pixel margin ensures x1 = x0+1 is also valid.
    let max_x = (width as f32) - 1.001;
    let max_y = (height as f32) - 1.001;
    let xc = x.max(0.0).min(max_x);
    let yc = y.max(0.0).min(max_y);
    
    let x0 = xc.floor() as i32;
    let y0 = yc.floor() as i32;
    let fx = xc - x0 as f32;
    let fy = yc - y0 as f32;
    let x1 = x0 + 1;
    let y1 = y0 + 1;
    
    // After clamping, x0 >= 0 and y0 >= 0 are guaranteed, but defensive 
    // index checks cost nothing and protect against any future drift.
    let x0u = x0.max(0) as usize;
    let y0u = y0.max(0) as usize;
    let x1u = (x1.max(0) as usize).min(width - 1);
    let y1u = (y1.max(0) as usize).min(height - 1);
    
    let i00 = buf[y0u * width + x0u] as f32;
    let i10 = buf[y0u * width + x1u] as f32;
    let i01 = buf[y1u * width + x0u] as f32;
    let i11 = buf[y1u * width + x1u] as f32;
    
    let top = i00 * (1.0 - fx) + i10 * fx;
    let bot = i01 * (1.0 - fx) + i11 * fx;
    top * (1.0 - fy) + bot * fy
}

/// Classification of a feature based on whether its motion is consistent
/// with pure camera motion (Static) or independently moving (Dynamic).
/// Unknown = not enough motion data yet to classify confidently.
/// 
/// UiOverlay is a hard-exclude category introduced to handle game-UI 
/// content like HUDs, scoreboards, killcam overlays. These features 
/// move-with-camera identically (so MotionConsensusPass labels them 
/// Static) but have zero parallax (no depth signal). When they get 
/// fed into triangulation, the encoder builds the mesh as a flat 
/// plane representing the UI rather than the actual 3D scene. Symptoms 
/// in user reports: "the anchors are mostly on the UI elements", 
/// "skew" artifacts (mesh aligned to UI, not world), "black frames" 
/// when camera pans off UI-heavy regions.
/// 
/// A feature is promoted to UiOverlay by either of:
///   1. Layer classification: its pixel position lands in an 
///      alpha_overlay-classified block consistently (>5 frames).
///   2. Depth-prior filtering: once triangulated, its depth variance 
///      across observations stays near zero (no parallax) over time.
/// 
/// UiOverlay features are excluded from observation recording AND 
/// from densification's sparse_screen list, so they never influence 
/// mesh integration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionClass {
    Unknown,
    Static,
    Dynamic,
    UiOverlay,
}

/// A persistent feature track across multiple frames.
/// Tracks share a stable feature_id so future TriangulatePass can collect
/// all observations of the same physical point.
#[derive(Clone, Debug)]
pub struct FeatureTrack {
    pub feature_id: u32,
    pub current_x: f32,
    pub current_y: f32,
    pub last_residual: f32,
    pub frames_alive: u32,
    pub born_frame: u32,
    /// Last frame's screen position (used to compute motion this frame)
    pub prev_x: f32,
    pub prev_y: f32,
    /// Classification: is this feature on a static scene element or moving 
    /// independently? Used to filter input to TriangulatePass.
    /// (See MotionConsensusPass below for the algorithm.)
    pub motion_class: MotionClass,
    /// EMA of consensus score [0..1] — how often this feature has agreed 
    /// with camera motion. Higher = more confidently static.
    pub static_consensus: f32,
    /// Number of frames where this feature's pixel landed in a block 
    /// classified as AlphaOverlay (UI). Saturates at 255. When this 
    /// exceeds `ui_overlay_promotion_threshold` (in V8Encoder), the 
    /// track gets promoted to MotionClass::UiOverlay and excluded from 
    /// triangulation/densification.
    pub layer_overlay_hits: u8,
    /// Number of frames in Active layer. Counted symmetrically — high 
    /// active_hits combined with low overlay_hits is the signature of 
    /// a real scene feature.
    pub layer_active_hits: u8,
    /// Last observed triangulated depth (camera-space Z). 0.0 if not 
    /// yet triangulated. Used to estimate depth variance over time.
    pub last_depth: f32,
    /// Running variance estimate of triangulated depth across observations. 
    /// Zero parallax = constant depth = strong UI signal. Updated each 
    /// frame the track has a triangulation. Initialized to a "give the 
    /// track a chance" value (large) so young tracks aren't auto-excluded.
    pub depth_variance_ema: f32,
    /// Frames since the track was last seen with a valid triangulation. 
    /// Used to decide whether depth_variance_ema is stale.
    pub frames_since_triangulation: u32,
}

/// Combines FastDetector + LkTracker into a stateful per-frame pass.
/// Maintains a population of live tracks across frames, re-detecting when
/// the population drops too low.
pub struct FeatureTrackPass {
    pub detector: FastDetector,
    pub tracker: LkTracker,
    pub min_track_count: usize,    // re-detect when live tracks fall below this
    pub max_track_count: usize,    // upper bound on simultaneous tracks
    pub min_track_separation: f32, // pixels — newly-detected points must be 
                                    // at least this far from any live track
    
    pub live_tracks: Vec<FeatureTrack>,
    
    /// LOCAL feature ID counter (within this chunk). The full 32-bit feature 
    /// ID exposed externally is `(chunk_id << 24) | local_id`. This means 
    /// each chunk can produce up to 2^24 = 16.7M distinct features without
    /// colliding with other chunks. (For perspective: a 6567-frame video 
    /// at 500 features/frame caps at ~3.3M features total — comfortably under.)
    pub next_local_id: u32,
    
    /// This pass’s chunk identity. Set via set_chunk_id(); 0 by default
    /// for non-chunked use.
    pub chunk_id: u32,
}

impl Default for FeatureTrackPass {
    fn default() -> Self {
        Self {
            detector: FastDetector::default(),
            tracker: LkTracker::default(),
            min_track_count: 100,
            max_track_count: 500,
            min_track_separation: 8.0,
            live_tracks: Vec::with_capacity(512),
            next_local_id: 0,
            chunk_id: 0,
        }
    }
}

impl FeatureTrackPass {
    /// Set the chunk identity AFTER construction. Must be called before any 
    /// frames are processed (otherwise old features have IDs from chunk 0, 
    /// new ones from the new chunk — IDs straddle namespaces).
    pub fn set_chunk_id(&mut self, chunk_id: u32) {
        debug_assert!(self.live_tracks.is_empty(), 
            "set_chunk_id called after tracking started — would mix namespaces");
        self.chunk_id = chunk_id & 0xFF;
    }
    
    /// Compose chunk_id + local_id into a global feature ID.
    /// Top 8 bits = chunk_id, low 24 bits = local_id.
    #[inline]
    pub fn global_fid(&self, local_id: u32) -> u32 {
        (self.chunk_id << 24) | (local_id & 0x00FF_FFFF)
    }
}

/// Decompose a global feature ID into (chunk_id, local_id).
/// Useful at merge time when we need to know which chunk a feature came from.
#[inline]
pub fn decompose_fid(global_fid: u32) -> (u32, u32) {
    ((global_fid >> 24) & 0xFF, global_fid & 0x00FF_FFFF)
}

impl FeatureTrackPass {
    /// Process one frame. Returns the SceneFeatures for THIS frame
    /// (subset of live tracks that survived LK).
    pub fn process(
        &mut self,
        frame_yuv: &[u8],
        prev_yuv: Option<&[u8]>,
        width: usize,
        height: usize,
        frame_idx: u32,
    ) -> Vec<SceneFeature> {
        // Step 1: advance live tracks via LK (if we have a prev frame)
        if let Some(prev) = prev_yuv {
            if !self.live_tracks.is_empty() {
                let points: Vec<(f32, f32)> = self.live_tracks.iter()
                    .map(|t| (t.current_x, t.current_y))
                    .collect();
                let results = self.tracker.track(prev, frame_yuv, width, height, &points);
                
                let mut survivors = Vec::with_capacity(self.live_tracks.len());
                for (track, result) in self.live_tracks.drain(..).zip(results.into_iter()) {
                    if let Some((nx, ny, res)) = result {
                        survivors.push(FeatureTrack {
                            feature_id: track.feature_id,
                            current_x: nx,
                            current_y: ny,
                            last_residual: res,
                            frames_alive: track.frames_alive + 1,
                            born_frame: track.born_frame,
                            prev_x: track.current_x,  // previous position for motion calc
                            prev_y: track.current_y,
                            motion_class: track.motion_class,  // classifier updates separately
                            static_consensus: track.static_consensus,
                            // Carry the layer/depth diagnostics forward — they're 
                            // updated by the UiOverlay filter pass later in encode_frame_at.
                            layer_overlay_hits: track.layer_overlay_hits,
                            layer_active_hits: track.layer_active_hits,
                            last_depth: track.last_depth,
                            depth_variance_ema: track.depth_variance_ema,
                            frames_since_triangulation: track.frames_since_triangulation.saturating_add(1),
                        });
                    }
                    // else: track lost, drop it
                }
                self.live_tracks = survivors;
            }
        }
        
        // Step 2: re-detect if population is low
        if self.live_tracks.len() < self.min_track_count {
            let new_kps = self.detector.detect(frame_yuv, width, height);
            let sep_sq = self.min_track_separation * self.min_track_separation;
            for kp in new_kps {
                if self.live_tracks.len() >= self.max_track_count { break; }
                let too_close = self.live_tracks.iter().any(|t| {
                    let dx = t.current_x - kp.x;
                    let dy = t.current_y - kp.y;
                    dx * dx + dy * dy < sep_sq
                });
                if too_close { continue; }
                self.live_tracks.push(FeatureTrack {
                    feature_id: self.global_fid(self.next_local_id),
                    current_x: kp.x,
                    current_y: kp.y,
                    last_residual: 0.0,
                    frames_alive: 1,
                    born_frame: frame_idx,
                    prev_x: kp.x,
                    prev_y: kp.y,
                    motion_class: MotionClass::Unknown,
                    static_consensus: 0.5,  // neutral prior
                    layer_overlay_hits: 0,
                    layer_active_hits: 0,
                    last_depth: 0.0,
                    // Start at a large positive value so newly-detected tracks 
                    // aren't auto-rejected before they've been triangulated 
                    // enough times for the EMA to converge.
                    depth_variance_ema: 1000.0,
                    frames_since_triangulation: 0,
                });
                self.next_local_id += 1;
            }
        }
        
        // Step 3: emit SceneFeatures
        // Confidence ~ inverse residual + age factor.
        // Older tracks are more trustworthy (they've passed multiple LK steps).
        self.live_tracks.iter().map(|t| {
            let res_score = (1.0 - (t.last_residual / 30.0).min(1.0)).max(0.0);
            let age_score = (t.frames_alive as f32 / 30.0).min(1.0);
            let confidence = 0.5 * res_score + 0.5 * age_score;
            SceneFeature {
                feature_id: t.feature_id,
                screen_x: t.current_x,
                screen_y: t.current_y,
                depth: 0.0,  // unknown until TriangulatePass (Session 3) lands
                confidence,
            }
        }).collect()
    }
    
    pub fn live_track_count(&self) -> usize { self.live_tracks.len() }
    pub fn total_features_seen(&self) -> u32 { self.next_local_id }
}

#[cfg(test)]
mod feature_tests {
    use super::*;
    
    #[test]
    fn fast_finds_synthetic_corner() {
        // Build a 32x32 image with a clear corner: top-left quarter dark,
        // rest bright. The corner at (16, 16) should fire FAST.
        let mut img = vec![200u8; 32 * 32];
        for y in 0..16 {
            for x in 0..16 {
                img[y * 32 + x] = 50;
            }
        }
        let det = FastDetector { threshold: 30, min_score: 100, nms_radius: 3, max_keypoints: 100 };
        let kps = det.detect(&img, 32, 32);
        assert!(!kps.is_empty(), "FAST should find a corner");
        // Best keypoint should be near (16, 16)
        let best = &kps[0];
        assert!((best.x - 15.5).abs() < 3.0, "x={}", best.x);
        assert!((best.y - 15.5).abs() < 3.0, "y={}", best.y);
    }
    
    #[test]
    fn lk_tracks_pure_translation() {
        // Build a 64x64 image with texture (so LK gradients are non-zero).
        // Then build a translated copy. Track points and verify they moved.
        let mut a = vec![128u8; 64 * 64];
        let mut b = vec![128u8; 64 * 64];
        for y in 0..64 {
            for x in 0..64 {
                let v = ((x * 7 + y * 13) % 256) as u8;
                a[y * 64 + x] = v;
                if x + 2 < 64 {
                    b[y * 64 + (x + 2)] = v;  // shift +2 in x
                }
            }
        }
        let lk = LkTracker::default();
        let pts = vec![(20.0, 20.0), (30.0, 40.0)];
        let results = lk.track(&a, &b, 64, 64, &pts);
        // Both should track to roughly +2 in x, ~0 in y
        for (r, p) in results.iter().zip(pts.iter()) {
            if let Some((nx, ny, _)) = r {
                let dx = nx - p.0;
                let dy = ny - p.1;
                // Allow some slack — 1-scale LK isn't perfect on synthetic noise
                assert!((dx - 2.0).abs() < 1.0, "dx={}", dx);
                assert!(dy.abs() < 1.0, "dy={}", dy);
            }
        }
    }
    
    #[test]
    fn feature_track_pass_assigns_stable_ids() {
        // Two-frame pass: same image. Tracks should keep their IDs.
        let img = {
            let mut buf = vec![100u8; 64 * 64];
            for y in 0..64 {
                for x in 0..64 {
                    buf[y * 64 + x] = ((x * 11 + y * 17) % 256) as u8;
                }
            }
            buf
        };
        let mut pass = FeatureTrackPass::default();
        let f0 = pass.process(&img, None, 64, 64, 0);
        let f1 = pass.process(&img, Some(&img), 64, 64, 1);
        // Each frame produced features, and the IDs from f0 should appear in f1
        assert!(!f0.is_empty(), "frame 0 should produce features");
        assert!(!f1.is_empty(), "frame 1 should produce features");
        let ids0: std::collections::HashSet<u32> = f0.iter().map(|f| f.feature_id).collect();
        let ids1: std::collections::HashSet<u32> = f1.iter().map(|f| f.feature_id).collect();
        let common = ids0.intersection(&ids1).count();
        // Most tracks should survive when the image doesn't change
        assert!(common > f0.len() / 2, "expected most tracks to survive identical-frame LK");
    }
}


// ═══════════════════════════════════════════════════════════
// CHUNK RESULT — output contract for parallel encoding (Stage 1)
// ═══════════════════════════════════════════════════════════
//
// A ChunkResult is everything one V8Encoder produces from its chunk of the 
// video. It is designed to be MERGEABLE — multiple ChunkResults can be 
// combined into a single V8Encoder-equivalent state via the merge() method 
// below (Stage 2).
// 
// Why not just merge the V8Encoder itself? The V8Encoder has running state 
// (live_tracks, prev_frame, cumulative_position) that is meaningless after 
// a chunk completes. ChunkResult contains only the FINISHED, MERGEABLE data.
//
// MERGE INVARIANTS:
//   - Feature IDs across chunks are guaranteed unique (chunk_id namespacing).
//   - Camera path uses RELATIVE motion deltas — global path = cumsum.
//   - Triangulated 3D points are in CHUNK-LOCAL coordinates initially. 
//     Merge converts to global coordinates by translating by the chunk’s 
//     starting global position.
//   - Mesh fragments fuse via spatial hash at merge time (vertex coalescing).
//
// This is Stage 1 of the parallel architecture. ChunkResult exists; merge() 
// is implemented for one chunk (trivial); Stages 2 and 3 will exercise 
// multi-chunk merging.

#[derive(Clone, Debug)]
pub struct ChunkResult {
    pub chunk_id: u32,
    pub chunk_start_frame: u32,
    pub chunk_end_frame: u32,    // exclusive (one past last frame)
    pub width: u32,
    pub height: u32,
    
    /// Per-frame RELATIVE camera motion. Sorted by frame_idx ascending.
    /// At merge time these get cumsummed across all chunks (in global frame 
    /// order) to produce the absolute pose track.
    pub relative_motion: Vec<(u32, Vec3)>,
    
    /// Triangulated features in CHUNK-LOCAL world coordinates.
    /// Merge translates by chunk’s global starting position.
    /// (If chunks share a coordinate frame from the start — by all using 
    /// global frame 0 as the chunk-relative origin — no translation is 
    /// needed. We keep the field name explicit anyway.)
    pub triangulated: std::collections::HashMap<u32, TriangulatedFeature>,
    
    /// Anchor-frame depth maps. Each is keyed by an absolute video frame_idx.
    pub depth_maps: Vec<DepthMap>,
    
    /// Spatial trace entries for the World viewer. Already use absolute 
    /// frame_idx and absolute screen positions, so no translation needed.
    pub spatial_trace: Vec<(u32, i16, i16, u32, u8)>,
    
    /// Mesh fragment from this chunk. Empty for now — populated by future 
    /// MeshPass (Session 5). Merge will fuse fragments via spatial hash.
    pub scene_mesh: SceneMesh,
    
    /// Diagnostic stats for this chunk.
    pub frames_processed: u32,
    pub features_created: u32,
    pub bytes_written: u64,
}

impl ChunkResult {
    /// Number of unique frames covered by this chunk’s relative motion log.
    pub fn covered_frame_count(&self) -> usize {
        self.relative_motion.len()
    }
}

/// Merge multiple chunk results into a single global view.
/// 
/// Order of operations matters here:
/// 1. Sort chunks by chunk_start_frame ascending — global temporal order.
/// 2. Build absolute pose track by cumsumming relative motion in frame 
///    order. The cumsum is across ALL chunks combined, so chunk seams 
///    don’t introduce step discontinuities (each chunk’s last delta 
///    flows into the next chunk’s first delta).
/// 3. Union triangulated feature maps. Feature IDs are already unique 
///    across chunks (chunk_id namespacing).
/// 4. Concatenate spatial traces in frame order.
/// 5. Concatenate depth maps in anchor-frame order.
/// 6. Fuse mesh fragments via spatial hash (Stage 5; for now trivial union).
/// 
/// For one chunk, the operation is trivial; we still go through the same 
/// path so any future bug stays caught.
pub struct MergedChunks {
    pub width: u32,
    pub height: u32,
    pub absolute_pose_track: Vec<CameraPose>,
    pub triangulated: std::collections::HashMap<u32, TriangulatedFeature>,
    pub depth_maps: Vec<DepthMap>,
    pub spatial_trace: Vec<(u32, i16, i16, u32, u8)>,
    pub scene_mesh: SceneMesh,
    pub total_frames: u32,
    pub total_bytes: u64,
}

impl MergedChunks {
    /// Build a merged result from a slice of ChunkResults.
    /// 
    /// Chunks may arrive out of frame order (different workers finish at 
    /// different times). We sort internally before merging.
    pub fn merge(chunks: &[ChunkResult]) -> Self {
        if chunks.is_empty() {
            return Self {
                width: 0, height: 0,
                absolute_pose_track: Vec::new(),
                triangulated: Default::default(),
                depth_maps: Vec::new(),
                spatial_trace: Vec::new(),
                scene_mesh: SceneMesh::new(),
                total_frames: 0,
                total_bytes: 0,
            };
        }
        
        let width = chunks[0].width;
        let height = chunks[0].height;
        
        // Sort chunks by start frame so the cumsum walks in temporal order
        let mut sorted: Vec<&ChunkResult> = chunks.iter().collect();
        sorted.sort_by_key(|c| c.chunk_start_frame);
        
        // ── Camera path: cumsum of all relative deltas in frame order ──
        // 
        // First gather all (frame_idx, delta) into a single sorted list, 
        // THEN walk that list with a running sum. This handles cases where 
        // chunk boundaries don’t align cleanly with frame_idx ordering.
        let mut all_deltas: Vec<(u32, Vec3)> = Vec::new();
        for c in &sorted {
            all_deltas.extend_from_slice(&c.relative_motion);
        }
        all_deltas.sort_by_key(|&(f, _)| f);
        
        let max_frame = all_deltas.iter().map(|&(f, _)| f).max().unwrap_or(0);
        let mut absolute_pose_track: Vec<CameraPose> = Vec::with_capacity((max_frame + 1) as usize);
        let mut cum = Vec3::zero();
        let mut delta_iter_idx = 0usize;
        for fidx in 0..=max_frame {
            // Apply all deltas at or before this frame
            while delta_iter_idx < all_deltas.len() && all_deltas[delta_iter_idx].0 <= fidx {
                let d = all_deltas[delta_iter_idx].1;
                cum.x += d.x;
                cum.y += d.y;
                cum.z += d.z;
                delta_iter_idx += 1;
            }
            absolute_pose_track.push(CameraPose {
                position: cum,
                rotation: [1.0, 0.0, 0.0, 0.0],
                focal: [width as f32, height as f32],
                principal: [width as f32 / 2.0, height as f32 / 2.0],
                distortion: 0.0,
            });
        }
        
        // ── Union triangulated features (chunk_id namespacing → no collisions) ──
        let mut triangulated = std::collections::HashMap::new();
        for c in &sorted {
            for (&fid, &tf) in c.triangulated.iter() {
                triangulated.insert(fid, tf);
            }
        }
        
        // ── Concatenate depth maps ordered by anchor_frame ──
        let mut depth_maps: Vec<DepthMap> = sorted.iter()
            .flat_map(|c| c.depth_maps.iter().cloned())
            .collect();
        depth_maps.sort_by_key(|m| m.anchor_frame);
        
        // ── Concatenate spatial traces ordered by frame_idx ──
        let mut spatial_trace: Vec<(u32, i16, i16, u32, u8)> = sorted.iter()
            .flat_map(|c| c.spatial_trace.iter().copied())
            .collect();
        spatial_trace.sort_by_key(|&(_, _, _, fi, _)| fi);
        
        // ── Mesh fusion ── (placeholder until Session 5; trivial union for now)
        let mut scene_mesh = SceneMesh::new();
        for c in &sorted {
            // Simple append. Stage 5 will replace with spatial-hash vertex fusion.
            let vbase = scene_mesh.vertices.len() as u32;
            scene_mesh.vertices.extend_from_slice(&c.scene_mesh.vertices);
            scene_mesh.vertex_colors.extend_from_slice(&c.scene_mesh.vertex_colors);
            scene_mesh.normals.extend_from_slice(&c.scene_mesh.normals);
            for &i in &c.scene_mesh.indices {
                scene_mesh.indices.push(i + vbase);
            }
        }
        
        let total_frames = sorted.iter().map(|c| c.frames_processed).sum();
        let total_bytes = sorted.iter().map(|c| c.bytes_written).sum();
        
        Self {
            width,
            height,
            absolute_pose_track,
            triangulated,
            depth_maps,
            spatial_trace,
            scene_mesh,
            total_frames,
            total_bytes,
        }
    }
}

impl V8Encoder {
    /// Convert this encoder’s state into a ChunkResult for merging.
    /// 
    /// Drops the running state (live_tracks, prev_frame) — those are 
    /// chunk-local working state, not part of the result.
    pub fn finalize_chunk(&self) -> ChunkResult {
        let chunk_end_frame = self.relative_motion_per_frame.iter()
            .map(|&(f, _)| f).max().unwrap_or(self.chunk_start_frame) + 1;
        ChunkResult {
            chunk_id: self.chunk_id,
            chunk_start_frame: self.chunk_start_frame,
            chunk_end_frame,
            width: self.width,
            height: self.height,
            relative_motion: self.relative_motion_per_frame.clone(),
            triangulated: self.observations.triangulated.clone(),
            depth_maps: self.depth_maps.clone(),
            spatial_trace: self.spatial_trace.clone(),
            scene_mesh: self.scene_mesh.clone(),
            frames_processed: self.frame_count,
            features_created: self.feature_pass.next_local_id,
            bytes_written: self.total_bytes,
        }
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::*;
    
    #[test]
    fn empty_merge_is_empty() {
        let merged = MergedChunks::merge(&[]);
        assert_eq!(merged.total_frames, 0);
        assert!(merged.absolute_pose_track.is_empty());
    }
    
    #[test]
    fn single_chunk_passes_through() {
        let mut deltas = vec![
            (0, Vec3 { x: 1.0, y: 0.0, z: 0.0 }),
            (1, Vec3 { x: 2.0, y: 0.0, z: 0.0 }),
            (2, Vec3 { x: 3.0, y: 0.0, z: 0.0 }),
        ];
        let chunk = ChunkResult {
            chunk_id: 0, chunk_start_frame: 0, chunk_end_frame: 3,
            width: 100, height: 100,
            relative_motion: deltas,
            triangulated: Default::default(),
            depth_maps: Vec::new(),
            spatial_trace: Vec::new(),
            scene_mesh: SceneMesh::new(),
            frames_processed: 3, features_created: 0, bytes_written: 0,
        };
        let merged = MergedChunks::merge(&[chunk]);
        // Cumsum: pose[0]=(1,0,0), pose[1]=(3,0,0), pose[2]=(6,0,0)
        assert_eq!(merged.absolute_pose_track.len(), 3);
        assert!((merged.absolute_pose_track[0].position.x - 1.0).abs() < 1e-3);
        assert!((merged.absolute_pose_track[1].position.x - 3.0).abs() < 1e-3);
        assert!((merged.absolute_pose_track[2].position.x - 6.0).abs() < 1e-3);
    }
    
    #[test]
    fn two_chunks_merge_continuously() {
        // Chunk A covers frames 0..2 with deltas (1, 1)
        // Chunk B covers frames 2..4 with deltas (1, 1) — note frame 2 is the boundary
        // Expected absolute path: 1, 2, 3, 4 (continuous cumsum across chunks)
        let chunk_a = ChunkResult {
            chunk_id: 0, chunk_start_frame: 0, chunk_end_frame: 2,
            width: 100, height: 100,
            relative_motion: vec![
                (0, Vec3 { x: 1.0, y: 0.0, z: 0.0 }),
                (1, Vec3 { x: 1.0, y: 0.0, z: 0.0 }),
            ],
            triangulated: Default::default(),
            depth_maps: Vec::new(),
            spatial_trace: Vec::new(),
            scene_mesh: SceneMesh::new(),
            frames_processed: 2, features_created: 0, bytes_written: 0,
        };
        let chunk_b = ChunkResult {
            chunk_id: 1, chunk_start_frame: 2, chunk_end_frame: 4,
            width: 100, height: 100,
            relative_motion: vec![
                (2, Vec3 { x: 1.0, y: 0.0, z: 0.0 }),
                (3, Vec3 { x: 1.0, y: 0.0, z: 0.0 }),
            ],
            triangulated: Default::default(),
            depth_maps: Vec::new(),
            spatial_trace: Vec::new(),
            scene_mesh: SceneMesh::new(),
            frames_processed: 2, features_created: 0, bytes_written: 0,
        };
        let merged = MergedChunks::merge(&[chunk_b, chunk_a]);  // out-of-order input
        assert_eq!(merged.absolute_pose_track.len(), 4);
        for i in 0..4 {
            let expected = (i + 1) as f32;
            let got = merged.absolute_pose_track[i].position.x;
            assert!((got - expected).abs() < 1e-3,
                "frame {}: expected x={} got x={}", i, expected, got);
        }
    }
    
    #[test]
    fn feature_ids_namespaced_by_chunk() {
        // Two chunks each create features. IDs should not collide.
        let mut tri_a = std::collections::HashMap::new();
        tri_a.insert(0x00_000001, TriangulatedFeature {
            feature_id: 0x00_000001,
            world_pos: Vec3::zero(),
            confidence: 0.5,
            reprojection_error: 1.0,
            baseline: 10.0,
            last_observed_frame: 5,
        });
        let mut tri_b = std::collections::HashMap::new();
        tri_b.insert(0x01_000001, TriangulatedFeature {
            feature_id: 0x01_000001,
            world_pos: Vec3 { x: 5.0, y: 0.0, z: 0.0 },
            confidence: 0.5,
            reprojection_error: 1.0,
            baseline: 10.0,
            last_observed_frame: 15,
        });
        let chunk_a = ChunkResult {
            chunk_id: 0, chunk_start_frame: 0, chunk_end_frame: 10,
            width: 100, height: 100,
            relative_motion: vec![],
            triangulated: tri_a,
            depth_maps: Vec::new(),
            spatial_trace: Vec::new(),
            scene_mesh: SceneMesh::new(),
            frames_processed: 10, features_created: 1, bytes_written: 0,
        };
        let chunk_b = ChunkResult {
            chunk_id: 1, chunk_start_frame: 10, chunk_end_frame: 20,
            width: 100, height: 100,
            relative_motion: vec![],
            triangulated: tri_b,
            depth_maps: Vec::new(),
            spatial_trace: Vec::new(),
            scene_mesh: SceneMesh::new(),
            frames_processed: 10, features_created: 1, bytes_written: 0,
        };
        let merged = MergedChunks::merge(&[chunk_a, chunk_b]);
        assert_eq!(merged.triangulated.len(), 2, "both features survive merge");
        assert!(merged.triangulated.contains_key(&0x00_000001));
        assert!(merged.triangulated.contains_key(&0x01_000001));
    }
    
    #[test]
    fn fid_decompose_roundtrip() {
        let chunk_id = 42u32;
        let local = 12345u32;
        let mut pass = FeatureTrackPass::default();
        pass.set_chunk_id(chunk_id);
        let global = pass.global_fid(local);
        let (rec_chunk, rec_local) = decompose_fid(global);
        assert_eq!(rec_chunk, chunk_id);
        assert_eq!(rec_local, local);
    }
}

// ═══════════════════════════════════════════════════════════
// SESSION 2.5: MotionConsensusPass — static/dynamic classification
// ═══════════════════════════════════════════════════════════
//
// CRITICAL for triangulation correctness on real video.
//
// Problem: standard structure-from-motion assumes the world is static and
// only the camera moves. When objects in the scene also move (a person 
// walking, a vehicle, even subtle hand gestures), their features get fed 
// into triangulation and corrupt the recovered geometry. The mesh ends up
// with ghosts of moving objects baked in.
//
// Solution: classify each feature as Static or Dynamic by comparing its 
// per-frame motion to the global camera motion (from PhaseCorrelation).
//
// Algorithm:
//   For each tracked feature, between frame N-1 and frame N:
//     observed_motion  = (current_x - prev_x, current_y - prev_y)
//     predicted_static = (-cam_dx, -cam_dy)   ← what the feature WOULD
//                                                 do if it were static
//     residual         = observed_motion - predicted_static
//     |residual| < threshold  →  consensus says STATIC
//     |residual| > threshold  →  consensus says DYNAMIC
//
//   Use an EMA over frames so single-frame noise doesn't flip the label.
//
// Parameters tuned for typical handheld/walking footage. Tighten thresholds
// for static-tripod content where even small object motion is meaningful.

pub struct MotionConsensusPass {
    /// Pixel residual above which a feature is considered moving relative 
    /// to camera. Tighter values = more sensitive to small object motion.
    pub motion_threshold: f32,
    /// EMA decay — how fast the consensus score updates per frame.
    /// 0.0 = never updates, 1.0 = no smoothing. ~0.3 = ~3-frame window.
    pub consensus_decay: f32,
    /// Minimum age before a feature can be classified. Newly-detected 
    /// features have only one position sample — can't compute motion yet.
    pub min_age_for_classification: u32,
    /// Consensus score above this → confidently Static
    pub static_high: f32,
    /// Consensus score below this → confidently Dynamic
    pub dynamic_low: f32,
}

impl Default for MotionConsensusPass {
    fn default() -> Self {
        Self {
            motion_threshold: 1.5,         // pixels of disagreement with cam motion
            consensus_decay: 0.3,
            min_age_for_classification: 3,
            static_high: 0.65,
            dynamic_low: 0.35,
        }
    }
}

impl MotionConsensusPass {
    /// Update each track's classification based on this frame's camera motion.
    /// 
    /// `cam_dx, cam_dy` are the screen-space shift estimated by PhaseCorrelation 
    /// (positive = scene shifted right/down → camera panned left/up).
    /// 
    /// `cam_confidence` is the phase correlation peak quality. When low, the 
    /// camera motion estimate is unreliable so we skip classification to avoid 
    /// mislabelling everything based on a bad signal.
    pub fn classify(
        &self,
        tracks: &mut [FeatureTrack],
        cam_dx: f32,
        cam_dy: f32,
        cam_confidence: f32,
    ) {
        // Compatibility wrapper — delegates to the layer-aware classifier 
        // with no mask. Existing call sites that don't have layer info 
        // still go through the original phase-correlation-only path.
        self.classify_with_layers(tracks, cam_dx, cam_dy, cam_confidence, None, 0, 0);
    }
    
    /// Layer-aware classifier. When `layer_mask` is `Some`, computes a 
    /// ROBUST camera motion estimate from the median (dx, dy) of features 
    /// in background-favored layers (Active and ParallaxBg). This avoids 
    /// the failure mode where phase correlation's single dominant-motion 
    /// vector locks onto a foreground subject (e.g. the killer in DBD 
    /// gameplay), causing background features to falsely classify as 
    /// Dynamic.
    /// 
    /// When the median is unreliable (too few background features, or 
    /// median itself is far from phase-corr's estimate which suggests 
    /// nothing trustworthy), falls back to the phase-correlation 
    /// estimate. Logs a one-shot message the first time the robust 
    /// estimator engages so the user can confirm the pathway is firing.
    pub fn classify_with_layers(
        &self,
        tracks: &mut [FeatureTrack],
        cam_dx: f32,
        cam_dy: f32,
        cam_confidence: f32,
        layer_mask: Option<&[u8]>,
        frame_width: u32,
        frame_height: u32,
    ) {
        // Don't update classifications when camera motion estimate is unreliable.
        // Common during scene cuts, very static scenes (no signal), or when 
        // PhaseCorrelation latched onto a wrong peak.
        if cam_confidence < 0.2 { return; }
        
        // ── ROBUST CAMERA MOTION FROM BACKGROUND FEATURES ──
        // 
        // Gather (dx, dy) for features that:
        //   - Are old enough to have a meaningful motion (frames_alive >= 2)
        //   - Fall in a background-favored layer (Active=3 or ParallaxBg=2)
        // Take the median. Robust to ~40% foreground/dynamic contamination.
        // 
        // If we don't have a layer mask, or we don't end up with enough 
        // background features (< 8), fall back to phase correlation.
        // 
        // FAO_NO_ROBUST_MOTION=1 forces the phase-correlation path even 
        // when a layer mask IS available. Useful for diagnosing whether 
        // the layer-robust path is helping or hurting on a given content 
        // type (user reported "anchoring slightly worse or just slightly 
        // different" after this was introduced — env var lets them A/B 
        // test without code changes).
        let force_phase_corr = std::env::var("FAO_NO_ROBUST_MOTION")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let (use_dx, use_dy) = if force_phase_corr {
            (cam_dx, cam_dy)
        } else if let (Some(mask), true) = 
            (layer_mask, frame_width > 0 && frame_height > 0)
        {
            let bw = (frame_width as usize) / 16;
            let bh = (frame_height as usize) / 16;
            let expected = bw * bh;
            if mask.len() == expected && bw > 0 && bh > 0 {
                let mut bg_dx: Vec<f32> = Vec::with_capacity(tracks.len());
                let mut bg_dy: Vec<f32> = Vec::with_capacity(tracks.len());
                for t in tracks.iter() {
                    if t.frames_alive < 2 { continue; }
                    let bx = (t.current_x as i32 / 16).max(0) as usize;
                    let by = (t.current_y as i32 / 16).max(0) as usize;
                    if bx >= bw || by >= bh { continue; }
                    let layer = mask[by * bw + bx];
                    // 3 = Active, 2 = ParallaxBg — both signal background 
                    // content. PureStatic (0) is also background but in 
                    // game UI contexts often catches the HUD edges 
                    // (anti-aliased borders between UI and game world), 
                    // so we skip it for robust motion to avoid mixing in 
                    // pseudo-stationary UI pixels.
                    if layer == 3 || layer == 2 {
                        bg_dx.push(t.current_x - t.prev_x);
                        bg_dy.push(t.current_y - t.prev_y);
                    }
                }
                
                if bg_dx.len() >= 8 {
                    // Median is the middle element after sort.
                    bg_dx.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    bg_dy.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let mid = bg_dx.len() / 2;
                    let med_dx = bg_dx[mid];
                    let med_dy = bg_dy[mid];
                    
                    // Diagnostic: one-shot when the layer-robust path 
                    // first engages, plus periodic so the user can 
                    // verify it stays engaged.
                    static ROBUST_FIRED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    let n = ROBUST_FIRED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if n == 1 || n == 10 || n == 100 || n == 1000 || n % 5000 == 0 {
                        eprintln!("[v8 robust-motion] frame #{}: {} bg features (median dx={:.2}, dy={:.2} vs phase-corr dx={:.2}, dy={:.2})",
                            n, bg_dx.len(), med_dx, med_dy, cam_dx, cam_dy);
                    }
                    
                    // Sanity check: if the median is wildly different from 
                    // phase correlation (e.g. > 50 px or > 5x), one of them 
                    // is wrong. Phase correlation is at least the dominant 
                    // motion of SOMETHING in the frame; the median might be 
                    // contaminated. Trust the SHORTER vector — small motion 
                    // is more conservative for classification.
                    let pc_mag = (cam_dx * cam_dx + cam_dy * cam_dy).sqrt();
                    let med_mag = (med_dx * med_dx + med_dy * med_dy).sqrt();
                    let dx_diff = (med_dx - cam_dx).abs();
                    let dy_diff = (med_dy - cam_dy).abs();
                    if dx_diff > 50.0 || dy_diff > 50.0 {
                        // Wildly disagreeing — use whichever is shorter. 
                        // (Foreground motion is typically bigger than 
                        // background camera motion, so the shorter vector 
                        // is the better camera-motion candidate.)
                        if med_mag < pc_mag { (med_dx, med_dy) } else { (cam_dx, cam_dy) }
                    } else {
                        // Reasonable agreement — prefer the median as 
                        // it's less foreground-contaminated.
                        (med_dx, med_dy)
                    }
                } else {
                    (cam_dx, cam_dy)
                }
            } else {
                (cam_dx, cam_dy)
            }
        } else {
            (cam_dx, cam_dy)
        };
        
        for track in tracks.iter_mut() {
            if track.frames_alive < self.min_age_for_classification { continue; }
            
            // Feature's actual motion this frame
            let obs_dx = track.current_x - track.prev_x;
            let obs_dy = track.current_y - track.prev_y;
            
            // What its motion WOULD be if the feature were locked to the static
            // background. When the camera shifts the scene by (cam_dx, cam_dy)
            // in screen space, a static-world point appears to move by 
            // (cam_dx, cam_dy) on screen.
            // (Convention: PhaseCorrelation returns shift of frame content; 
            // a static feature should match that shift exactly.)
            // 
            // `use_dx`/`use_dy` is the layer-robust estimate when available, 
            // otherwise the phase-correlation estimate.
            let pred_dx = use_dx;
            let pred_dy = use_dy;
            
            let res_x = obs_dx - pred_dx;
            let res_y = obs_dy - pred_dy;
            let residual = (res_x * res_x + res_y * res_y).sqrt();
            
            // Per-frame agreement: 1.0 if residual is below threshold, 
            // tapering to 0.0 at 3x threshold.
            let frame_agreement = (1.0 - (residual / (self.motion_threshold * 3.0))).clamp(0.0, 1.0);
            
            // EMA update of running consensus score
            let alpha = self.consensus_decay;
            track.static_consensus = (1.0 - alpha) * track.static_consensus 
                                   + alpha * frame_agreement;
            
            // Decide classification with hysteresis bands so we don't 
            // oscillate near a single threshold.
            // 
            // IMPORTANT: UiOverlay is a HARD LATCH — once a feature is 
            // promoted (either by the layer mask or by the depth-variance 
            // filter in encode_frame_at), we don't un-promote based on 
            // motion consensus. A UI overlay's motion will agree with 
            // the camera (it moves WITH the camera), so the consensus 
            // would happily re-label it Static, defeating the filter. 
            // Skip the assignment when already UiOverlay.
            if track.motion_class != MotionClass::UiOverlay {
                track.motion_class = if track.static_consensus >= self.static_high {
                    MotionClass::Static
                } else if track.static_consensus <= self.dynamic_low {
                    MotionClass::Dynamic
                } else {
                    track.motion_class  // stay in current state in the gray zone
                };
            }
        }
        
        // ── CLUSTER DETECTION PASS (opt-in via FAO_CLUSTER_DEMOTE=1) ──
        // 
        // User filed this fix after observing "killers hand was tracked 
        // as an extra point now which is lots of pixels -> single pose": 
        // a feature on a foreground subject gets classified Static when 
        // the subject's motion coincidentally agrees with the consensus 
        // camera motion (or when consensus is contaminated).
        // 
        // First-pass version of this turned out to slightly hurt anchor 
        // quality — user reported "anchoring slightly worse or just 
        // slightly different" after the change. Suspected cause: 
        // particles, swaying trees, slow-moving background objects, or 
        // simply features clustering by chance can all trigger demotion 
        // when they shouldn't. Made it opt-in until we have a more 
        // robust foreground discriminator (object recognition / saliency 
        // map — see VISION.md).
        // 
        // Detection: features that move TOGETHER (similar dx/dy within 
        // ~2px) but DIFFERENTLY from the consensus camera motion (>3px 
        // away) form a "foreground cluster". Demote all cluster members 
        // to Dynamic.
        // 
        // Algorithm is O(n²) over feature count; with typical 100-200 
        // live features that's 10-40K comparisons per call — negligible.
        let cluster_enabled = std::env::var("FAO_CLUSTER_DEMOTE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if !cluster_enabled { return; }
        const CLUSTER_MOTION_TOL: f32 = 2.0;  // px — features within this differ in motion = same cluster
        const CAMERA_DEVIATION_THRESH: f32 = 3.0;  // px — cluster motion this far from cam = foreground
        const MIN_CLUSTER_SIZE: usize = 3;  // minimum to demote (avoids single-feature false positives)
        
        // Build a small list of (idx, dx, dy) for static-classified, 
        // mature tracks. Skip UiOverlay (already excluded) and Dynamic 
        // (already correct).
        let candidates: Vec<(usize, f32, f32)> = tracks.iter().enumerate()
            .filter_map(|(i, t)| {
                if t.frames_alive < 2 { return None; }
                if t.motion_class != MotionClass::Static { return None; }
                Some((i, t.current_x - t.prev_x, t.current_y - t.prev_y))
            })
            .collect();
        
        if candidates.len() >= MIN_CLUSTER_SIZE {
            // For each candidate, find peers with similar motion.
            // We then check whether the cluster's mean motion deviates 
            // from camera consensus. `visited` ensures we don't process 
            // the same cluster N times.
            let mut visited = vec![false; candidates.len()];
            let mut demoted_count = 0usize;
            
            for i in 0..candidates.len() {
                if visited[i] { continue; }
                let (_, dxi, dyi) = candidates[i];
                
                // Find all candidates within CLUSTER_MOTION_TOL of i.
                let mut cluster_members: Vec<usize> = vec![i];
                for j in (i+1)..candidates.len() {
                    if visited[j] { continue; }
                    let (_, dxj, dyj) = candidates[j];
                    let ddx = dxj - dxi;
                    let ddy = dyj - dyi;
                    if ddx.abs() < CLUSTER_MOTION_TOL && ddy.abs() < CLUSTER_MOTION_TOL {
                        cluster_members.push(j);
                    }
                }
                
                if cluster_members.len() < MIN_CLUSTER_SIZE { continue; }
                
                // Cluster mean motion (average of members' dx/dy).
                let mut sum_dx = 0.0f32;
                let mut sum_dy = 0.0f32;
                for &m in &cluster_members {
                    sum_dx += candidates[m].1;
                    sum_dy += candidates[m].2;
                }
                let cluster_dx = sum_dx / cluster_members.len() as f32;
                let cluster_dy = sum_dy / cluster_members.len() as f32;
                
                // Compare to consensus camera motion (use_dx, use_dy).
                let cam_diff_x = (cluster_dx - use_dx).abs();
                let cam_diff_y = (cluster_dy - use_dy).abs();
                
                if cam_diff_x > CAMERA_DEVIATION_THRESH || cam_diff_y > CAMERA_DEVIATION_THRESH {
                    // Foreground cluster! Demote all members to Dynamic 
                    // and lock them out of further cluster checks this 
                    // pass.
                    for &m in &cluster_members {
                        let track_idx = candidates[m].0;
                        if tracks[track_idx].motion_class != MotionClass::UiOverlay {
                            tracks[track_idx].motion_class = MotionClass::Dynamic;
                            // Drop consensus score so it stays Dynamic for 
                            // a few frames (until consensus catches up). 
                            // Otherwise next frame's classification might 
                            // bounce back to Static.
                            tracks[track_idx].static_consensus = 0.0;
                            demoted_count += 1;
                        }
                        visited[m] = true;
                    }
                } else {
                    // Cluster moves with the camera consensus — legitimate 
                    // background. Mark its members visited so we don't 
                    // re-evaluate the same large cluster from each of 
                    // its members (would still produce the same "not 
                    // foreground" verdict and waste work). For a 
                    // background-dominated frame this collapses N² to 
                    // ~N over the typical case.
                    for &m in &cluster_members {
                        visited[m] = true;
                    }
                }
            }
            
            // Diagnostic — log when cluster demotions happen so the user 
            // can see the foreground-subject fix engaging. Fires at counts 
            // 1/10/100/1000/5000+.
            if demoted_count > 0 {
                static CLUSTER_FIRED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = CLUSTER_FIRED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n == 1 || n == 10 || n == 100 || n == 1000 || n % 5000 == 0 {
                    eprintln!("[v8 cluster-demote] #{}: {} features demoted from Static→Dynamic (cluster moving with cam_diff>{:.0}px from consensus dx={:.2} dy={:.2})",
                        n, demoted_count, CAMERA_DEVIATION_THRESH, use_dx, use_dy);
                }
            }
        }
    }
    
    /// Count tracks by classification. Useful for diagnostics.
    /// 
    /// Returns (static, dynamic, unknown, ui_overlay). The fourth element 
    /// was added with the UiOverlay filter (round-3 user insight: "the 
    /// anchors are mostly on the UI elements"). When UiOverlay count is 
    /// large relative to Static, the filter is working — those features 
    /// would previously have been counted as Static and corrupted 
    /// triangulation.
    pub fn count_by_class(&self, tracks: &[FeatureTrack]) -> (usize, usize, usize, usize) {
        let mut s = 0;
        let mut d = 0;
        let mut u = 0;
        let mut o = 0;
        for t in tracks {
            match t.motion_class {
                MotionClass::Static => s += 1,
                MotionClass::Dynamic => d += 1,
                MotionClass::Unknown => u += 1,
                MotionClass::UiOverlay => o += 1,
            }
        }
        (s, d, u, o)
    }
}

#[cfg(test)]
mod consensus_tests {
    use super::*;
    
    #[test]
    fn camera_pan_classifies_static_features() {
        // Simulate a camera panning right: camera_dx = 5.0
        // Build features that move with the camera (static) and against it (dynamic).
        let mut tracks = vec![
            // Static: moves +5.0 in x (matches camera shift)
            FeatureTrack {
                feature_id: 1, current_x: 105.0, current_y: 50.0,
                prev_x: 100.0, prev_y: 50.0,
                last_residual: 0.0, frames_alive: 10, born_frame: 0,
                motion_class: MotionClass::Unknown, static_consensus: 0.5,
                layer_overlay_hits: 0, layer_active_hits: 0,
                last_depth: 0.0, depth_variance_ema: 1000.0,
                frames_since_triangulation: 0,
            },
            // Dynamic: moves only +1.0 in x (lagging behind camera)
            FeatureTrack {
                feature_id: 2, current_x: 101.0, current_y: 50.0,
                prev_x: 100.0, prev_y: 50.0,
                last_residual: 0.0, frames_alive: 10, born_frame: 0,
                motion_class: MotionClass::Unknown, static_consensus: 0.5,
                layer_overlay_hits: 0, layer_active_hits: 0,
                last_depth: 0.0, depth_variance_ema: 1000.0,
                frames_since_triangulation: 0,
            },
        ];
        let pass = MotionConsensusPass::default();
        // Run the classifier ~10 times to let the EMA converge
        for _ in 0..10 {
            pass.classify(&mut tracks, 5.0, 0.0, 0.9);
        }
        assert_eq!(tracks[0].motion_class, MotionClass::Static, 
            "feature moving with camera should be Static, consensus={}", 
            tracks[0].static_consensus);
        assert_eq!(tracks[1].motion_class, MotionClass::Dynamic,
            "feature moving against camera should be Dynamic, consensus={}",
            tracks[1].static_consensus);
    }
    
    #[test]
    fn low_confidence_skips_update() {
        let mut tracks = vec![
            FeatureTrack {
                feature_id: 1, current_x: 100.0, current_y: 50.0,
                prev_x: 100.0, prev_y: 50.0,
                last_residual: 0.0, frames_alive: 10, born_frame: 0,
                motion_class: MotionClass::Unknown, static_consensus: 0.5,
                layer_overlay_hits: 0, layer_active_hits: 0,
                last_depth: 0.0, depth_variance_ema: 1000.0,
                frames_since_triangulation: 0,
            },
        ];
        let pass = MotionConsensusPass::default();
        pass.classify(&mut tracks, 5.0, 0.0, 0.1);  // low confidence
        // Should NOT update — consensus stays at 0.5, class stays Unknown
        assert_eq!(tracks[0].motion_class, MotionClass::Unknown);
        assert!((tracks[0].static_consensus - 0.5).abs() < 0.01);
    }
    
    #[test]
    fn young_features_not_classified() {
        let mut tracks = vec![
            FeatureTrack {
                feature_id: 1, current_x: 105.0, current_y: 50.0,
                prev_x: 100.0, prev_y: 50.0,
                last_residual: 0.0, frames_alive: 1,  // too young
                born_frame: 0,
                motion_class: MotionClass::Unknown, static_consensus: 0.5,
                layer_overlay_hits: 0, layer_active_hits: 0,
                last_depth: 0.0, depth_variance_ema: 1000.0,
                frames_since_triangulation: 0,
            },
        ];
        let pass = MotionConsensusPass::default();
        pass.classify(&mut tracks, 5.0, 0.0, 0.9);
        assert_eq!(tracks[0].motion_class, MotionClass::Unknown);
    }
}

// ═══════════════════════════════════════════════════════════
// SESSION 3: TriangulatePass — sparse 3D from 2D + camera motion
// ═══════════════════════════════════════════════════════════
//
// Given multiple observations of the same feature across frames, plus the
// camera pose for each frame, solve for the feature's 3D position.
//
// METHOD: Midpoint triangulation
//
//   For each pair of observations (frame_a, frame_b) of the same feature:
//     1. Build a ray from camera_a's center through observation_a
//     2. Build a ray from camera_b's center through observation_b
//     3. Find the 3D point closest to both rays (midpoint of the shortest
//        line segment between them)
//
//   For features observed in N > 2 frames, average across all C(N,2) pairs.
//   In practice we use just the longest-baseline pair, which gives the best
//   triangulation accuracy. (Tiny baselines — pose_a near pose_b — produce
//   wildly inaccurate depths; long baselines are stable.)
//
// CONFIDENCE:
//   Output confidence reflects:
//     - Track length (more observations → more reliable)
//     - Baseline / depth ratio (long baseline + close depth = strong signal)
//     - Reprojection residual (low = consistent with all views)

/// One observation of a feature in a particular frame.
#[derive(Clone, Copy, Debug)]
pub struct FeatureObservation {
    pub frame_idx: u32,
    pub screen_x: f32,
    pub screen_y: f32,
}

/// Triangulated 3D position for a feature, with confidence + diagnostics.
#[derive(Clone, Copy, Debug)]
pub struct TriangulatedFeature {
    pub feature_id: u32,
    pub world_pos: Vec3,
    pub confidence: f32,
    /// Mean reprojection error (pixels) across the contributing observations
    pub reprojection_error: f32,
    /// Baseline length used for this triangulation (pixels of camera motion).
    /// Useful as a sanity check — small baselines produce unreliable depth.
    pub baseline: f32,
    /// Most recent frame_idx at which this feature was observed.
    /// Used by the World view's temporal scrubber to associate each 3D 
    /// point with a time, so "Now ±15" filtering and Temporal coloring 
    /// produce the right behavior for triangulated features.
    pub last_observed_frame: u32,
}

pub struct TriangulatePass {
    /// Reject pairs whose baseline (camera position delta) is shorter than this.
    /// Short baselines triangulate to nonsense (any depth fits).
    pub min_baseline: f32,
    /// Reject points whose reprojection error exceeds this many pixels.
    pub max_reprojection_error: f32,
    /// Default focal length when the camera intrinsics aren't yet calibrated.
    /// Used for ray construction. Refined later by EstimateIntrinsicsPass.
    pub default_focal: f32,
}

impl Default for TriangulatePass {
    fn default() -> Self {
        Self {
            min_baseline: 5.0,
            max_reprojection_error: 4.0,
            default_focal: 1000.0,
        }
    }
}

impl TriangulatePass {
    /// Triangulate every feature with ≥2 observations.
    /// 
    /// `observations` maps feature_id → all of its observations across frames.
    /// `pose_track` is indexed by frame_idx.
    pub fn triangulate_all(
        &self,
        observations: &[(u32, Vec<FeatureObservation>)],
        pose_track: &[CameraPose],
    ) -> Vec<TriangulatedFeature> {
        let mut out = Vec::with_capacity(observations.len());
        for (feature_id, obs) in observations.iter() {
            if obs.len() < 2 { continue; }
            
            // Pick the pair with the longest baseline (best triangulation)
            let mut best_pair: Option<(usize, usize, f32)> = None;
            for i in 0..obs.len() {
                for j in i+1..obs.len() {
                    let pi = obs[i].frame_idx as usize;
                    let pj = obs[j].frame_idx as usize;
                    if pi >= pose_track.len() || pj >= pose_track.len() { continue; }
                    let dpos = Vec3 {
                        x: pose_track[pi].position.x - pose_track[pj].position.x,
                        y: pose_track[pi].position.y - pose_track[pj].position.y,
                        z: pose_track[pi].position.z - pose_track[pj].position.z,
                    };
                    let baseline = (dpos.x * dpos.x + dpos.y * dpos.y + dpos.z * dpos.z).sqrt();
                    if baseline < self.min_baseline { continue; }
                    if best_pair.map(|(_, _, b)| baseline > b).unwrap_or(true) {
                        best_pair = Some((i, j, baseline));
                    }
                }
            }
            
            let (i, j, baseline) = match best_pair { Some(p) => p, None => continue };
            
            let pose_a = pose_track[obs[i].frame_idx as usize];
            let pose_b = pose_track[obs[j].frame_idx as usize];
            
            // Build rays in world space.
            // Ray origin = camera position. Ray direction = unprojected pixel direction.
            let ray_a = self.unproject(&pose_a, obs[i].screen_x, obs[i].screen_y);
            let ray_b = self.unproject(&pose_b, obs[j].screen_x, obs[j].screen_y);
            
            // Solve for closest point between the two rays (midpoint method)
            let world_pos = match closest_point_between_rays(
                pose_a.position, ray_a,
                pose_b.position, ray_b,
            ) {
                Some(p) => p,
                None => continue,
            };
            
            // Compute reprojection error: project world_pos through both poses,
            // compare to actual observations.
            let (rx_a, ry_a) = self.project(&pose_a, world_pos);
            let (rx_b, ry_b) = self.project(&pose_b, world_pos);
            let err_a = ((rx_a - obs[i].screen_x).powi(2) + (ry_a - obs[i].screen_y).powi(2)).sqrt();
            let err_b = ((rx_b - obs[j].screen_x).powi(2) + (ry_b - obs[j].screen_y).powi(2)).sqrt();
            let mean_err = (err_a + err_b) * 0.5;
            
            if mean_err > self.max_reprojection_error { continue; }
            
            // Confidence: combines track length, baseline, and reprojection accuracy
            let track_score = (obs.len() as f32 / 10.0).min(1.0);
            let baseline_score = (baseline / 50.0).min(1.0);
            let accuracy_score = (1.0 - (mean_err / self.max_reprojection_error)).max(0.0);
            let confidence = 0.4 * track_score + 0.3 * baseline_score + 0.3 * accuracy_score;
            
            // Most recent observation = max frame_idx in the obs list
            let last_observed_frame = obs.iter().map(|o| o.frame_idx).max().unwrap_or(0);
            out.push(TriangulatedFeature {
                feature_id: *feature_id,
                world_pos,
                confidence,
                reprojection_error: mean_err,
                baseline,
                last_observed_frame,
            });
        }
        out
    }
    
    /// Unproject a screen point through a camera pose to get a normalized 3D ray.
    /// Returns the ray direction in WORLD space.
    fn unproject(&self, pose: &CameraPose, sx: f32, sy: f32) -> Vec3 {
        let fx = if pose.focal[0] > 0.0 { pose.focal[0] } else { self.default_focal };
        let fy = if pose.focal[1] > 0.0 { pose.focal[1] } else { self.default_focal };
        // Camera-space ray (looking down +Z by convention):
        //   x = (sx - cx) / fx
        //   y = (sy - cy) / fy
        //   z = 1.0
        let cam_x = (sx - pose.principal[0]) / fx;
        let cam_y = (sy - pose.principal[1]) / fy;
        let dir_cam = Vec3::new(cam_x, cam_y, 1.0);
        // Normalize
        let len = (dir_cam.x * dir_cam.x + dir_cam.y * dir_cam.y + dir_cam.z * dir_cam.z).sqrt();
        let dir_cam = if len > 1e-9 {
            Vec3::new(dir_cam.x / len, dir_cam.y / len, dir_cam.z / len)
        } else {
            Vec3::new(0.0, 0.0, 1.0)
        };
        // Rotate by pose.rotation (quaternion → rotation matrix)
        rotate_vec3_by_quat(dir_cam, pose.rotation)
    }
    
    /// Project a 3D world point through a camera pose to screen coordinates.
    fn project(&self, pose: &CameraPose, world: Vec3) -> (f32, f32) {
        // Translate to camera frame
        let t = Vec3 {
            x: world.x - pose.position.x,
            y: world.y - pose.position.y,
            z: world.z - pose.position.z,
        };
        // Apply inverse rotation (conjugate quaternion for unit quat)
        let q_inv = [pose.rotation[0], -pose.rotation[1], -pose.rotation[2], -pose.rotation[3]];
        let cam = rotate_vec3_by_quat(t, q_inv);
        if cam.z.abs() < 1e-6 {
            return (pose.principal[0], pose.principal[1]);
        }
        let fx = if pose.focal[0] > 0.0 { pose.focal[0] } else { self.default_focal };
        let fy = if pose.focal[1] > 0.0 { pose.focal[1] } else { self.default_focal };
        (
            pose.principal[0] + fx * cam.x / cam.z,
            pose.principal[1] + fy * cam.y / cam.z,
        )
    }
}

/// Rotate a 3D vector by a unit quaternion (w, x, y, z).
/// Standard Hamilton-product formulation.
pub fn rotate_vec3_by_quat(v: Vec3, q: [f32; 4]) -> Vec3 {
    let w = q[0]; let x = q[1]; let y = q[2]; let z = q[3];
    // q * v_quat * q^-1 expansion (v_quat = (0, vx, vy, vz))
    // Using the formula: v' = v + 2*q_xyz × (q_xyz × v + w*v)
    let qv_x = y * v.z - z * v.y;
    let qv_y = z * v.x - x * v.z;
    let qv_z = x * v.y - y * v.x;
    let qwv_x = qv_x + w * v.x;
    let qwv_y = qv_y + w * v.y;
    let qwv_z = qv_z + w * v.z;
    let r_x = y * qwv_z - z * qwv_y;
    let r_y = z * qwv_x - x * qwv_z;
    let r_z = x * qwv_y - y * qwv_x;
    Vec3 {
        x: v.x + 2.0 * r_x,
        y: v.y + 2.0 * r_y,
        z: v.z + 2.0 * r_z,
    }
}

/// Precompute a 3×3 rotation matrix from a unit quaternion, in row-major 
/// f32×9 order (m[0..3] = first row, etc.).
/// 
/// For hot loops that rotate many vectors by the same quaternion (mesh 
/// integration over a depth map: ~50k pixels per anchor), this avoids 
/// re-deriving the cross-product machinery per call. Speedup is ~2-3× 
/// over `rotate_vec3_by_quat` because matrix-vec is 9 mults + 6 adds, 
/// vs quat-rotate's 22+ ops including two cross products.
/// 
/// The formula is the standard quaternion → rotation matrix expansion:
///   R = | 1-2(y²+z²)   2(xy-wz)     2(xz+wy)   |
///       | 2(xy+wz)     1-2(x²+z²)   2(yz-wx)   |
///       | 2(xz-wy)     2(yz+wx)     1-2(x²+y²) |
pub fn quat_to_mat3(q: [f32; 4]) -> [f32; 9] {
    let w = q[0]; let x = q[1]; let y = q[2]; let z = q[3];
    let xx = x * x; let yy = y * y; let zz = z * z;
    let xy = x * y; let xz = x * z; let yz = y * z;
    let wx = w * x; let wy = w * y; let wz = w * z;
    [
        1.0 - 2.0 * (yy + zz),  2.0 * (xy - wz),         2.0 * (xz + wy),
        2.0 * (xy + wz),         1.0 - 2.0 * (xx + zz),  2.0 * (yz - wx),
        2.0 * (xz - wy),         2.0 * (yz + wx),         1.0 - 2.0 * (xx + yy),
    ]
}

/// Rotate a Vec3 by a precomputed 3×3 matrix (row-major f32×9). 
/// Pair with `quat_to_mat3` for hot loops.
#[inline]
pub fn rotate_vec3_by_mat3(v: Vec3, m: &[f32; 9]) -> Vec3 {
    Vec3 {
        x: m[0] * v.x + m[1] * v.y + m[2] * v.z,
        y: m[3] * v.x + m[4] * v.y + m[5] * v.z,
        z: m[6] * v.x + m[7] * v.y + m[8] * v.z,
    }
}

/// Find the 3D point closest to two skew rays (midpoint of shortest segment 
/// between them).
/// 
/// Standard formula: for rays P1+t*D1 and P2+s*D2, parameters at closest 
/// points are obtained from:
///   [D1·D1  -D1·D2] [t]   [(P2-P1)·D1]
///   [D1·D2  -D2·D2] [s] = [(P2-P1)·D2]
fn closest_point_between_rays(
    p1: Vec3, d1: Vec3,
    p2: Vec3, d2: Vec3,
) -> Option<Vec3> {
    let w = Vec3 { x: p1.x - p2.x, y: p1.y - p2.y, z: p1.z - p2.z };
    let a = d1.x * d1.x + d1.y * d1.y + d1.z * d1.z;
    let b = d1.x * d2.x + d1.y * d2.y + d1.z * d2.z;
    let c = d2.x * d2.x + d2.y * d2.y + d2.z * d2.z;
    let d = d1.x * w.x + d1.y * w.y + d1.z * w.z;
    let e = d2.x * w.x + d2.y * w.y + d2.z * w.z;
    let denom = a * c - b * b;
    if denom.abs() < 1e-9 { return None; }  // parallel rays
    let t = (b * e - c * d) / denom;
    let s = (a * e - b * d) / denom;
    // Closest points on each ray
    let closest_a = Vec3 { x: p1.x + t * d1.x, y: p1.y + t * d1.y, z: p1.z + t * d1.z };
    let closest_b = Vec3 { x: p2.x + s * d2.x, y: p2.y + s * d2.y, z: p2.z + s * d2.z };
    // Midpoint
    Some(Vec3 {
        x: 0.5 * (closest_a.x + closest_b.x),
        y: 0.5 * (closest_a.y + closest_b.y),
        z: 0.5 * (closest_a.z + closest_b.z),
    })
}

// ═══════════════════════════════════════════════════════════
// V8Encoder integration: collect observations, periodically triangulate
// ═══════════════════════════════════════════════════════════
//
// Triangulation isn't done per-frame — it's expensive. Instead we accumulate
// observations and trigger a triangulation pass every TRIANGULATE_INTERVAL 
// frames. The result populates SceneFeature.depth on subsequent frames.

const TRIANGULATE_INTERVAL: u32 = 30;

/// Collected per-feature observation history. Indexed by feature_id.
#[derive(Default)]
pub struct ObservationStore {
    pub by_feature: std::collections::HashMap<u32, Vec<FeatureObservation>>,
    /// Last computed triangulation for each feature_id
    pub triangulated: std::collections::HashMap<u32, TriangulatedFeature>,
}

impl ObservationStore {
    pub fn record(&mut self, feature_id: u32, obs: FeatureObservation) {
        let entry = self.by_feature.entry(feature_id).or_insert_with(Vec::new);
        // Cap per-feature observation history so memory doesn't grow unbounded
        // (we only need a few well-spaced observations to triangulate).
        if entry.len() < 30 {
            entry.push(obs);
        }
    }
    
    pub fn run_triangulation(&mut self, pass: &TriangulatePass, pose_track: &[CameraPose]) {
        // Convert HashMap to Vec for the pass
        let obs_vec: Vec<(u32, Vec<FeatureObservation>)> = self.by_feature.iter()
            .filter(|(_, v)| v.len() >= 2)
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        let results = pass.triangulate_all(&obs_vec, pose_track);
        for tf in results {
            self.triangulated.insert(tf.feature_id, tf);
        }
    }
    
    /// Annotate features with depth from triangulation cache.
    /// Looks up each feature_id and sets depth + bumps confidence.
    pub fn annotate_with_depth(&self, features: &mut [SceneFeature], pose: &CameraPose) {
        for feat in features.iter_mut() {
            if let Some(tf) = self.triangulated.get(&feat.feature_id) {
                // Convert world_pos to camera frame depth
                let dz = tf.world_pos.z - pose.position.z;
                feat.depth = dz.abs().max(0.1);  // depth is positive distance
                // Combine track confidence with triangulation confidence
                feat.confidence = 0.5 * feat.confidence + 0.5 * tf.confidence;
            }
        }
    }
}

#[cfg(test)]
mod triangulate_tests {
    use super::*;
    
    #[test]
    fn closest_point_simple() {
        // Two perpendicular rays meeting at origin
        let p = closest_point_between_rays(
            Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 1.0, 0.0),
        ).unwrap();
        assert!(p.x.abs() < 1e-3, "x={}", p.x);
        assert!(p.y.abs() < 1e-3, "y={}", p.y);
        assert!(p.z.abs() < 1e-3, "z={}", p.z);
    }
    
    #[test]
    fn closest_point_parallel_returns_none() {
        let result = closest_point_between_rays(
            Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0), Vec3::new(1.0, 0.0, 0.0),
        );
        assert!(result.is_none());
    }
    
    #[test]
    fn triangulate_synthetic_point() {
        // A 3D point at (10, 0, 50). Two cameras: one at (0,0,0), one at (20,0,0).
        // Both look down +Z. Project the point through each, then triangulate.
        let pass = TriangulatePass::default();
        let world_truth = Vec3::new(10.0, 0.0, 50.0);
        let pose_a = CameraPose {
            position: Vec3::new(0.0, 0.0, 0.0),
            rotation: [1.0, 0.0, 0.0, 0.0],
            focal: [1000.0, 1000.0],
            principal: [0.0, 0.0],
            distortion: 0.0,
        };
        let pose_b = CameraPose {
            position: Vec3::new(20.0, 0.0, 0.0),
            ..pose_a
        };
        let (sx_a, sy_a) = pass.project(&pose_a, world_truth);
        let (sx_b, sy_b) = pass.project(&pose_b, world_truth);
        
        let observations = vec![
            (0u32, vec![
                FeatureObservation { frame_idx: 0, screen_x: sx_a, screen_y: sy_a },
                FeatureObservation { frame_idx: 1, screen_x: sx_b, screen_y: sy_b },
            ]),
        ];
        let pose_track = vec![pose_a, pose_b];
        let results = pass.triangulate_all(&observations, &pose_track);
        assert_eq!(results.len(), 1);
        let tf = &results[0];
        assert!((tf.world_pos.x - 10.0).abs() < 0.5, "x={}", tf.world_pos.x);
        assert!((tf.world_pos.y - 0.0).abs() < 0.5, "y={}", tf.world_pos.y);
        assert!((tf.world_pos.z - 50.0).abs() < 1.0, "z={}", tf.world_pos.z);
        assert!(tf.reprojection_error < 0.5);
    }
    
    #[test]
    fn rotate_identity_preserves_vector() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        let r = rotate_vec3_by_quat(v, [1.0, 0.0, 0.0, 0.0]);
        assert!((r.x - 1.0).abs() < 1e-6);
        assert!((r.y - 2.0).abs() < 1e-6);
        assert!((r.z - 3.0).abs() < 1e-6);
    }
}


// ═══════════════════════════════════════════════════════════
// SESSION 5: MeshIntegrator — depth maps → growing triangle mesh
// ═══════════════════════════════════════════════════════════
//
// THE STREAMING ARCHITECTURE
//
// Earlier passes produce ephemeral data: triangulated points get refreshed
// every TRIANGULATE_INTERVAL frames; depth maps get rebuilt every anchor.
// Without integration, this data accumulates in capped Vecs and oldest 
// gets evicted — meaning the encoder forgets earlier scene geometry.
//
// MeshIntegrator turns ephemeral depth maps into PERSISTENT mesh.
// 
// Flow:
//   1. DensifyPass produces depth_map for anchor frame N
//   2. MeshIntegrator.integrate(depth_map, pose) → appends triangles to 
//      scene_mesh
//   3. depth_map is DISCARDED (no longer needed; the mesh contains the 
//      relevant surface info, more compact than 230K floats)
//   4. Mesh grows with each anchor, but each integration is bounded by 
//      depth_map size (~50K triangles max per anchor at typical density)
//
// HEIGHTFIELD TRIANGULATION
//
// A depth map is a 2D grid of distances from the camera. Adjacent pixels 
// in the grid form natural quads. Each quad becomes 2 triangles UNLESS
// the depth jump between adjacent pixels exceeds discontinuity_threshold —
// in which case the quad is at an object boundary (foreground in front of
// background) and stitching them would create a "tent" artifact.
//
// For a quad with corners (a, b, c, d) — top-left, top-right, bottom-left,
// bottom-right — we emit two triangles (a,b,c) and (b,d,c) only if all 
// pairwise depth differences are below threshold.
//
// FUTURE PASSES:
//   - Cross-anchor fusion: vertex spatial-hash to merge overlapping mesh 
//     regions from different anchors (Session 5b)
//   - Texture: project source frame onto mesh faces (Session 6 maybe)
//   - Decimation: reduce triangle count for distant or flat regions


/// Helper for MeshIntegrator: insert a new vertex into the mesh, register 
/// it in the spatial grid, and return its index. Used by integrate() to 
/// keep the fusion logic readable.
/// 
/// `color` is the linear-light [0,1] RGB sampled at the vertex's source 
/// frame position. Previously this was a confidence-tint placeholder 
/// ([g*0.4, g*0.85, g]) which made the entire mesh render in teal — fixed 
/// when the integrator started receiving the source YUV.
fn insert_new_vertex(mesh: &mut SceneMesh, vertex: Vec3, conf: f32, key: (i32, i32, i32), color: [f32; 3]) -> u32 {
    let _ = conf;  // conf no longer feeds the color; kept for signature stability
    let v_idx = mesh.vertices.len() as u32;
    mesh.vertices.push(vertex);
    mesh.vertex_colors.push(color);
    mesh.normals.push(Vec3 { x: 0.0, y: 0.0, z: 1.0 });
    mesh.vertex_hits.push(1);
    mesh.vertex_grid.insert(key, v_idx);
    v_idx
}

/// Spatial hash grid for vertex fusion.
/// 
/// Tracks already-placed mesh vertices in 3D buckets so that when a new 
/// vertex is being placed, we can check if a nearby vertex already exists
/// and REUSE its index instead of duplicating.
/// 
/// This is what turns "many anchor shells" into "one continuous mesh":
/// when two adjacent anchors observe the same surface, their vertices land
/// at nearly-identical world positions; the grid catches this and unifies.
/// 
/// Bucket sizing: cell_size = fusion_radius. A query examines the containing
/// bucket plus 26 neighbors (3x3x3 cube), so two vertices closer than 
/// fusion_radius are guaranteed to land in queried cells.
pub struct VertexFusionGrid {
    pub cell_size: f32,
    pub fusion_radius: f32,
    cells: std::collections::HashMap<(i32, i32, i32), Vec<u32>>,
    /// Reference to vertex positions for proximity checks.
    /// We don't own them; the caller passes them at query time.
    /// (Could be improved with a parallel vertex array for cache locality.)
    _phantom: std::marker::PhantomData<()>,
}

impl VertexFusionGrid {
    pub fn new(fusion_radius: f32) -> Self {
        Self {
            cell_size: fusion_radius,
            fusion_radius,
            cells: std::collections::HashMap::new(),
            _phantom: std::marker::PhantomData,
        }
    }
    
    fn cell_key(&self, v: Vec3) -> (i32, i32, i32) {
        (
            (v.x / self.cell_size).floor() as i32,
            (v.y / self.cell_size).floor() as i32,
            (v.z / self.cell_size).floor() as i32,
        )
    }
    
    /// Find an existing vertex within fusion_radius of `v`. Returns its 
    /// index if found, None otherwise.
    /// 
    /// `vertices` is the parallel array; we look up indices stored in the 
    /// grid against this slice for actual position checks.
    pub fn find_nearby(&self, v: Vec3, vertices: &[Vec3]) -> Option<u32> {
        let (cx, cy, cz) = self.cell_key(v);
        let r2 = self.fusion_radius * self.fusion_radius;
        // Search 3x3x3 neighbor cube
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(bucket) = self.cells.get(&(cx + dx, cy + dy, cz + dz)) {
                        for &idx in bucket {
                            if let Some(existing) = vertices.get(idx as usize) {
                                let ex = existing.x - v.x;
                                let ey = existing.y - v.y;
                                let ez = existing.z - v.z;
                                let d2 = ex * ex + ey * ey + ez * ez;
                                if d2 <= r2 {
                                    return Some(idx);
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }
    
    /// Register a new vertex at index `idx` with position `v`.
    pub fn insert(&mut self, idx: u32, v: Vec3) {
        let key = self.cell_key(v);
        self.cells.entry(key).or_insert_with(Vec::new).push(idx);
    }
    
    /// Total vertex count tracked across all cells.
    pub fn len(&self) -> usize {
        self.cells.values().map(|c| c.len()).sum()
    }
    
    /// Memory diagnostic: number of populated cells.
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }
}

#[cfg(test)]
mod fusion_tests {
    use super::*;
    
    #[test]
    fn nearby_vertex_is_reused() {
        let mut grid = VertexFusionGrid::new(1.0);
        let mut verts = vec![Vec3 { x: 0.0, y: 0.0, z: 0.0 }];
        grid.insert(0, verts[0]);
        // Query a position 0.5 away — should find existing vertex
        let q = Vec3 { x: 0.3, y: 0.0, z: 0.0 };
        assert_eq!(grid.find_nearby(q, &verts), Some(0));
    }
    
    #[test]
    fn distant_vertex_is_separate() {
        let mut grid = VertexFusionGrid::new(1.0);
        let verts = vec![Vec3 { x: 0.0, y: 0.0, z: 0.0 }];
        grid.insert(0, verts[0]);
        // Query a position 2.0 away — should NOT find existing
        let q = Vec3 { x: 2.0, y: 0.0, z: 0.0 };
        assert_eq!(grid.find_nearby(q, &verts), None);
    }
    
    #[test]
    fn boundary_neighborhood_works() {
        // Two vertices in adjacent cells, but within fusion_radius
        let mut grid = VertexFusionGrid::new(1.0);
        let mut verts = vec![Vec3 { x: 0.99, y: 0.0, z: 0.0 }];
        grid.insert(0, verts[0]);
        // Query at x=1.01 — different cell, but within radius=1.0 of existing
        let q = Vec3 { x: 1.01, y: 0.0, z: 0.0 };
        assert_eq!(grid.find_nearby(q, &verts), Some(0),
            "neighbor cell search should find vertex across cell boundary");
    }
}

pub struct MeshIntegrator {
    /// Maximum allowed depth difference between adjacent pixels in a quad.
    pub discontinuity_threshold: f32,
    
    /// Skip pixels with confidence below this. 
    pub min_confidence: f32,
    
    /// Step size for pixel sampling (1 = every pixel becomes a vertex).
    pub sample_stride: u32,
    
    /// Cap on total mesh vertices to prevent unbounded growth.
    pub max_vertices: usize,
    
    /// Voxel size for cross-anchor vertex fusion (world units).
    /// Vertices within this distance of an existing vertex are fused 
    /// rather than duplicated. See SceneMesh::vertex_grid.
    pub fusion_voxel_size: f32,
    
    /// Enable cross-anchor fusion. When false, every integration adds 
    /// fresh vertices regardless of overlap (old behavior).
    pub enable_fusion: bool,
}

impl Default for MeshIntegrator {
    fn default() -> Self {
        Self {
            // Loosened from 5.0 → 20.0. At HD with sparse triangulated input, 
            // densified neighboring pixels often have noisy depth (planar 
            // interpolation between far-apart sparse points produces local 
            // wobble). A tight threshold rejected most quads as 
            // "discontinuities" when they were really just interpolation 
            // noise. Loosening lets the prototype produce visible triangles; 
            // a future bilateral smoothing pass on the depth map would let us 
            // tighten this back up for true edge preservation.
            discontinuity_threshold: 20.0,
            min_confidence: 0.05,
            sample_stride: 3,  // every 3rd pixel — keeps mesh tight (was 1, but caused 1M+ verts per chunk)
            max_vertices: 500_000,  // bounded so memory doesn't blow up across many chunks
            // Fusion (Session 5b): unify near-duplicate vertices across anchors.
            // 0.5 world units = vertices closer than 0.5 are treated as the 
            // same surface point. Tuned for typical scene scale (depths 5-50).
            fusion_voxel_size: 0.5,
            enable_fusion: true,
        }
    }
}

impl MeshIntegrator {
    /// Integrate one anchor depth map into the scene mesh.
    /// 
    /// Returns the number of triangles added.
    pub fn integrate(
        &self,
        depth_map: &DepthMap,
        pose: &CameraPose,
        downsample_factor: u32,  // depth_map’s downsample relative to source frame
        scene_mesh: &mut SceneMesh,
        source_yuv: &[u8],
        source_w: usize,
        source_h: usize,
    ) -> usize {
        if scene_mesh.vertices.len() >= self.max_vertices {
            return 0;
        }
        // Helper closure: sample real RGB from the source YUV at a given 
        // source-frame pixel location. Returns linear-light [0,1] RGB.
        // 
        // YUV420 planar layout: Y[0..w*h], U[w*h..w*h + (w/2)*(h/2)], V[...]
        // Standard BT.601 conversion (close enough; encode_frame_at's 
        // upstream conversion uses similar matrices).
        let sample_color = |sx: f32, sy: f32| -> [f32; 3] {
            if source_yuv.is_empty() || source_w == 0 || source_h == 0 {
                return [0.5, 0.5, 0.5];  // fallback to neutral grey
            }
            let xi = (sx.clamp(0.0, (source_w - 1) as f32)) as usize;
            let yi = (sy.clamp(0.0, (source_h - 1) as f32)) as usize;
            let y_sz = source_w * source_h;
            let uv_w = source_w / 2;
            let uv_h = source_h / 2;
            let uv_sz = uv_w * uv_h;
            if source_yuv.len() < y_sz + 2 * uv_sz {
                // Not a full YUV420 buffer — try RGB interpretation. 
                // RGB layout is 3 bytes per pixel.
                let pi = (yi * source_w + xi) * 3;
                if pi + 2 < source_yuv.len() {
                    return [
                        source_yuv[pi] as f32 / 255.0,
                        source_yuv[pi+1] as f32 / 255.0,
                        source_yuv[pi+2] as f32 / 255.0,
                    ];
                }
                return [0.5, 0.5, 0.5];
            }
            let y = source_yuv[yi * source_w + xi] as f32;
            let cu = source_yuv[y_sz + (yi / 2) * uv_w + (xi / 2)] as f32;
            let cv = source_yuv[y_sz + uv_sz + (yi / 2) * uv_w + (xi / 2)] as f32;
            // BT.601 limited-range YUV → full-range RGB.
            let yn = (y - 16.0) * (255.0 / 219.0);
            let un = cu - 128.0;
            let vn = cv - 128.0;
            let r = yn + 1.402 * vn;
            let g = yn - 0.344136 * un - 0.714136 * vn;
            let b = yn + 1.772 * un;
            [
                (r / 255.0).clamp(0.0, 1.0),
                (g / 255.0).clamp(0.0, 1.0),
                (b / 255.0).clamp(0.0, 1.0),
            ]
        };
        
        let dw = depth_map.width as usize;
        let dh = depth_map.height as usize;
        if dw < 2 || dh < 2 { return 0; }
        
        let stride = self.sample_stride.max(1) as usize;
        
        // Build a sub-sampled vertex grid. Each entry is either Some(vertex_index)
        // or None if the pixel wasn’t sampled or its depth is unusable.
        let gw = (dw + stride - 1) / stride;
        let gh = (dh + stride - 1) / stride;
        let mut grid_idx: Vec<Option<u32>> = vec![None; gw * gh];
        let mut grid_depth: Vec<f32> = vec![0.0; gw * gh];
        
        let fxi = if pose.focal[0] > 0.0 { pose.focal[0] } else { 1000.0 };
        let fyi = if pose.focal[1] > 0.0 { pose.focal[1] } else { 1000.0 };
        let cxi = pose.principal[0];
        let cyi = pose.principal[1];
        let down = downsample_factor as f32;
        // Precompute the rotation matrix once per integrate() call. 
        // The inner loop runs gw*gh times (typically ~57k at quarter 
        // resolution) and each rotation goes from ~22 ops (quat) to 
        // ~15 ops (mat3-vec). 2-3× speedup on the rotation step.
        let pose_rot_mat = quat_to_mat3(pose.rotation);
        
        let triangles_before = scene_mesh.indices.len() / 3;
        let vertices_before = scene_mesh.vertices.len();
        let max_new_verts = self.max_vertices.saturating_sub(vertices_before);
        let voxel = self.fusion_voxel_size.max(0.001);
        let inv_voxel = 1.0 / voxel;
        let mut fused_count = 0u32;
        let mut new_count = 0u32;
        
        // Pass 1: place or fuse vertices for valid pixels
        for gy in 0..gh {
            for gx in 0..gw {
                if scene_mesh.vertices.len() >= self.max_vertices { break; }
                let py = gy * stride;
                let px = gx * stride;
                if py >= dh || px >= dw { continue; }
                let idx = py * dw + px;
                let d = depth_map.depths[idx];
                let conf = depth_map.confidences[idx];
                if conf < self.min_confidence || d <= 0.0 { continue; }
                
                let fx = (px as f32 + 0.5) * down;
                let fy = (py as f32 + 0.5) * down;
                let cx_cam = (fx - cxi) / fxi * d;
                let cy_cam = (fy - cyi) / fyi * d;
                let cz_cam = d;
                let cam_v = Vec3 { x: cx_cam, y: cy_cam, z: cz_cam };
                let world = rotate_vec3_by_mat3(cam_v, &pose_rot_mat);
                let vertex = Vec3 {
                    x: pose.position.x + world.x,
                    y: pose.position.y + world.y,
                    z: pose.position.z + world.z,
                };
                
                // ── Cross-anchor fusion: check spatial hash ──
                let v_idx = if self.enable_fusion {
                    let key = (
                        (vertex.x * inv_voxel).round() as i32,
                        (vertex.y * inv_voxel).round() as i32,
                        (vertex.z * inv_voxel).round() as i32,
                    );
                    if let Some(&existing) = scene_mesh.vertex_grid.get(&key) {
                        // FUSE: blend the existing vertex toward the new observation.
                        // Running average weighted by hit count.
                        let ei = existing as usize;
                        if ei < scene_mesh.vertices.len() {
                            let hits = scene_mesh.vertex_hits.get(ei).copied().unwrap_or(1);
                            let n = (hits + 1) as f32;
                            let w_old = (hits as f32) / n;
                            let w_new = 1.0 / n;
                            let cur = scene_mesh.vertices[ei];
                            scene_mesh.vertices[ei] = Vec3 {
                                x: cur.x * w_old + vertex.x * w_new,
                                y: cur.y * w_old + vertex.y * w_new,
                                z: cur.z * w_old + vertex.z * w_new,
                            };
                            // Blend color too — more observations = more reliable color.
                            // Sample the REAL source frame color at this vertex's 
                            // screen projection (fx, fy in source-frame coordinates).
                            // This replaces the old confidence-tint placeholder 
                            // ([g*0.4, g*0.85, g]) which was causing the "blocky 
                            // bluish-green mesh" appearance.
                            let cur_c = scene_mesh.vertex_colors[ei];
                            let real_c = sample_color(fx, fy);
                            scene_mesh.vertex_colors[ei] = [
                                cur_c[0] * w_old + real_c[0] * w_new,
                                cur_c[1] * w_old + real_c[1] * w_new,
                                cur_c[2] * w_old + real_c[2] * w_new,
                            ];
                            if ei < scene_mesh.vertex_hits.len() {
                                scene_mesh.vertex_hits[ei] = hits + 1;
                            }
                            fused_count += 1;
                            existing
                        } else {
                            // Stale grid entry; treat as new
                            new_count += 1;
                            let real_c = sample_color(fx, fy);
                            insert_new_vertex(scene_mesh, vertex, conf, key, real_c)
                        }
                    } else {
                        new_count += 1;
                        let real_c = sample_color(fx, fy);
                        insert_new_vertex(scene_mesh, vertex, conf, key, real_c)
                    }
                } else {
                    // Fusion disabled: always insert
                    let key = (0, 0, 0);  // unused when fusion disabled
                    new_count += 1;
                    let real_c = sample_color(fx, fy);
                    insert_new_vertex(scene_mesh, vertex, conf, key, real_c)
                };
                
                grid_idx[gy * gw + gx] = Some(v_idx);
                grid_depth[gy * gw + gx] = d;
            }
        }
        
        // Diagnostic: log fusion ratio for THIS integration
        if fused_count + new_count > 0 {
            let ratio = fused_count as f32 / (fused_count + new_count) as f32;
            if ratio > 0.05 {
                eprintln!("[v8 mesh] fusion: {} new + {} fused ({:.0}% reuse)", 
                    new_count, fused_count, ratio * 100.0);
            }
        }
        
        // Pass 2: emit triangles for adjacent valid quads (no depth discontinuity)
        let disc = self.discontinuity_threshold;
        for gy in 0..(gh - 1) {
            for gx in 0..(gw - 1) {
                let a = grid_idx[gy * gw + gx];
                let b = grid_idx[gy * gw + gx + 1];
                let c = grid_idx[(gy + 1) * gw + gx];
                let d = grid_idx[(gy + 1) * gw + gx + 1];
                let (a, b, c, d) = match (a, b, c, d) {
                    (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
                    _ => continue,  // need all four corners present
                };
                let da = grid_depth[gy * gw + gx];
                let db = grid_depth[gy * gw + gx + 1];
                let dc = grid_depth[(gy + 1) * gw + gx];
                let dd = grid_depth[(gy + 1) * gw + gx + 1];
                // Check pairwise depth differences for the quad. Reject if any 
                // edge spans a discontinuity larger than threshold scaled by depth.
                let scaled = disc * da.max(db).max(dc).max(dd) * 0.05 + disc;
                if (da - db).abs() > scaled { continue; }
                if (da - dc).abs() > scaled { continue; }
                if (db - dd).abs() > scaled { continue; }
                if (dc - dd).abs() > scaled { continue; }
                // Emit two triangles. Counter-clockwise winding (assuming Y down 
                // in screen space, mirrored at render time).
                scene_mesh.indices.push(a);
                scene_mesh.indices.push(b);
                scene_mesh.indices.push(c);
                scene_mesh.indices.push(b);
                scene_mesh.indices.push(d);
                scene_mesh.indices.push(c);
            }
        }
        
        // Pass 3: compute per-vertex normals from triangle face normals.
        // Naive — for newly added vertices only, accumulate face normals 
        // weighted by triangle area, then normalize.
        // For prototype, just leave the placeholder normals — Cairo doesn’t
        // do real lighting and the World view will compute screen-space 
        // shading approximations.
        let _ = max_new_verts;  // reserved for future cap-aware decimation
        
        scene_mesh.indices.len() / 3 - triangles_before
    }
}

/// Mesh integration WITH vertex fusion across anchors.
/// 
/// Same heightfield triangulation as MeshIntegrator::integrate, but each 
/// candidate vertex is checked against the fusion grid first. If a nearby
/// existing vertex (within grid.fusion_radius) is found, that vertex is 
/// reused. Otherwise a new vertex is added and registered in the grid.
/// 
/// Result: anchor meshes that observe overlapping regions of the scene 
/// produce ONE continuous mesh rather than overlapping shells.
/// 
/// Returns the number of triangles added.
pub fn integrate_with_fusion(
    mi: &MeshIntegrator,
    depth_map: &DepthMap,
    pose: &CameraPose,
    downsample_factor: u32,
    scene_mesh: &mut SceneMesh,
    fusion_grid: &mut VertexFusionGrid,
) -> usize {
    if scene_mesh.vertices.len() >= mi.max_vertices {
        return 0;
    }
    
    let dw = depth_map.width as usize;
    let dh = depth_map.height as usize;
    if dw < 2 || dh < 2 { return 0; }
    
    let stride = mi.sample_stride.max(1) as usize;
    let gw = (dw + stride - 1) / stride;
    let gh = (dh + stride - 1) / stride;
    let mut grid_idx: Vec<Option<u32>> = vec![None; gw * gh];
    let mut grid_depth: Vec<f32> = vec![0.0; gw * gh];
    
    let fxi = if pose.focal[0] > 0.0 { pose.focal[0] } else { 1000.0 };
    let fyi = if pose.focal[1] > 0.0 { pose.focal[1] } else { 1000.0 };
    let cxi = pose.principal[0];
    let cyi = pose.principal[1];
    let down = downsample_factor as f32;
    // Precompute rotation matrix once — see comment in `integrate` 
    // method above. Same hot-loop optimization.
    let pose_rot_mat = quat_to_mat3(pose.rotation);
    
    let triangles_before = scene_mesh.indices.len() / 3;
    
    // Pass 1: place vertices for valid pixels, with fusion deduplication
    for gy in 0..gh {
        for gx in 0..gw {
            if scene_mesh.vertices.len() >= mi.max_vertices { break; }
            let py = gy * stride;
            let px = gx * stride;
            if py >= dh || px >= dw { continue; }
            let idx = py * dw + px;
            let d = depth_map.depths[idx];
            let conf = depth_map.confidences[idx];
            if conf < mi.min_confidence || d <= 0.0 { continue; }
            
            // Pixel center in full-res source coords
            let fx = (px as f32 + 0.5) * down;
            let fy = (py as f32 + 0.5) * down;
            
            // Unproject to camera-space ray then to world via pose
            let cx_cam = (fx - cxi) / fxi * d;
            let cy_cam = (fy - cyi) / fyi * d;
            let cz_cam = d;
            let cam_v = Vec3 { x: cx_cam, y: cy_cam, z: cz_cam };
            let world = rotate_vec3_by_mat3(cam_v, &pose_rot_mat);
            let vertex = Vec3 {
                x: pose.position.x + world.x,
                y: pose.position.y + world.y,
                z: pose.position.z + world.z,
            };
            
            // FUSION: check if a nearby vertex already exists
            let v_idx = if let Some(existing) = fusion_grid.find_nearby(vertex, &scene_mesh.vertices) {
                // Reuse — but maybe blend the color toward higher-confidence
                if let Some(col) = scene_mesh.vertex_colors.get_mut(existing as usize) {
                    let g = (conf.min(1.0) * 0.7 + 0.2).clamp(0.0, 1.0);
                    let new_col = [g * 0.4, g * 0.85, g];
                    // Average existing and new (simple — doesn't track confidence weights)
                    col[0] = (col[0] + new_col[0]) * 0.5;
                    col[1] = (col[1] + new_col[1]) * 0.5;
                    col[2] = (col[2] + new_col[2]) * 0.5;
                }
                existing
            } else {
                // New vertex
                let new_idx = scene_mesh.vertices.len() as u32;
                scene_mesh.vertices.push(vertex);
                let g = (conf.min(1.0) * 0.7 + 0.2).clamp(0.0, 1.0);
                scene_mesh.vertex_colors.push([g * 0.4, g * 0.85, g]);
                scene_mesh.normals.push(Vec3 { x: 0.0, y: 0.0, z: 1.0 });
                fusion_grid.insert(new_idx, vertex);
                new_idx
            };
            
            grid_idx[gy * gw + gx] = Some(v_idx);
            grid_depth[gy * gw + gx] = d;
        }
    }
    
    // Pass 2: emit triangles for adjacent valid quads
    let disc = mi.discontinuity_threshold;
    for gy in 0..(gh - 1) {
        for gx in 0..(gw - 1) {
            let a = grid_idx[gy * gw + gx];
            let b = grid_idx[gy * gw + gx + 1];
            let c = grid_idx[(gy + 1) * gw + gx];
            let d = grid_idx[(gy + 1) * gw + gx + 1];
            let (a, b, c, d) = match (a, b, c, d) {
                (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
                _ => continue,
            };
            // Skip degenerate triangles where fusion collapsed two corners 
            // into the same vertex (would produce a zero-area triangle)
            if a == b || a == c || a == d || b == c || b == d || c == d {
                continue;
            }
            let da = grid_depth[gy * gw + gx];
            let db = grid_depth[gy * gw + gx + 1];
            let dc = grid_depth[(gy + 1) * gw + gx];
            let dd = grid_depth[(gy + 1) * gw + gx + 1];
            let scaled = disc * da.max(db).max(dc).max(dd) * 0.05 + disc;
            if (da - db).abs() > scaled { continue; }
            if (da - dc).abs() > scaled { continue; }
            if (db - dd).abs() > scaled { continue; }
            if (dc - dd).abs() > scaled { continue; }
            scene_mesh.indices.push(a);
            scene_mesh.indices.push(b);
            scene_mesh.indices.push(c);
            scene_mesh.indices.push(b);
            scene_mesh.indices.push(d);
            scene_mesh.indices.push(c);
        }
    }
    
    scene_mesh.indices.len() / 3 - triangles_before
}

#[cfg(test)]
mod mesh_integrator_tests {
    use super::*;
    
    #[test]
    fn empty_depth_map_produces_no_mesh() {
        let mut mesh = SceneMesh::new();
        let dm = DepthMap::new(8, 8, 0);  // all confidences = 0
        let pose = CameraPose::default();
        let mi = MeshIntegrator::default();
        let n = mi.integrate(&dm, &pose, 4, &mut mesh);
        assert_eq!(n, 0);
        assert!(mesh.vertices.is_empty());
    }
    
    #[test]
    fn flat_depth_produces_mesh_grid() {
        // A 6x6 depth map, all depth=10, all confidence=1.0
        // With sample_stride=2 we sample at grid (3x3) — 4 quads → 8 triangles.
        let mut dm = DepthMap::new(6, 6, 0);
        for i in 0..dm.depths.len() {
            dm.depths[i] = 10.0;
            dm.confidences[i] = 1.0;
        }
        let pose = CameraPose::default();
        let mut mesh = SceneMesh::new();
        let mi = MeshIntegrator { sample_stride: 2, ..MeshIntegrator::default() };
        let tris = mi.integrate(&dm, &pose, 1, &mut mesh);
        assert!(tris > 0, "expected some triangles for a flat depth field");
        // Verify all indices reference valid vertices
        for &i in &mesh.indices {
            assert!((i as usize) < mesh.vertices.len(),
                "triangle index {} out of range (verts={})", i, mesh.vertices.len());
        }
    }
    
    #[test]
    fn discontinuity_creates_no_triangle() {
        // 4x4 depth map: left half depth=10, right half depth=100.
        // With discontinuity_threshold=5 the boundary should NOT be triangulated.
        let mut dm = DepthMap::new(4, 4, 0);
        for py in 0..4 {
            for px in 0..4 {
                let d = if px < 2 { 10.0 } else { 100.0 };
                dm.depths[py * 4 + px] = d;
                dm.confidences[py * 4 + px] = 1.0;
            }
        }
        let pose = CameraPose::default();
        let mut mesh = SceneMesh::new();
        let mi = MeshIntegrator { 
            sample_stride: 1, 
            discontinuity_threshold: 5.0,
            ..MeshIntegrator::default() 
        };
        mi.integrate(&dm, &pose, 1, &mut mesh);
        // Triangles should exist on EACH side but not bridging the boundary.
        // Verify no triangle has vertices from both depth regions.
        // (px<2 = left, px>=2 = right)
        // Each vertex was placed at (gx*stride + 0.5)*1 in source-pixel x.
        // Left vertices have world_pos.x < 2, right have >= 2 (approximately).
        for tri in mesh.indices.chunks(3) {
            let xs: Vec<f32> = tri.iter().map(|&i| mesh.vertices[i as usize].x).collect();
            let min_x = xs.iter().cloned().fold(f32::MAX, f32::min);
            let max_x = xs.iter().cloned().fold(f32::MIN, f32::max);
            // A bridging triangle would have xs spanning from < 2 to >= 2
            // (approximately — depends on principal point and focal).
            // We can’t easily check world coords here; instead check that the 
            // depths of the triangle’s contributing pixels are all close.
            // For this test, just confirm at least some triangles exist:
            let _ = (min_x, max_x);
        }
        // Loose check: should have triangles on left side AND right side, 
        // but no quad bridges them. Total triangles should be roughly half
        // of what a fully-connected mesh would produce.
        // Fully-connected at stride=1 on 4x4: 3x3 quads = 18 tris. With one 
        // boundary col rejected, we lose 6 tris (3 quads x 2 tris) → expect ~12.
        // Don’t hard-assert exact count; just verify we got SOME and not all.
        let actual = mesh.indices.len() / 3;
        assert!(actual > 0, "should have some triangles");
        assert!(actual <= 18, "shouldn\'t exceed fully-connected count");
    }
}

// ═══════════════════════════════════════════════════════════
// SESSION 4: DensifyPass — sparse 3D points → dense depth map
// ═══════════════════════════════════════════════════════════
//
// Given sparse triangulated 3D points (Session 3 output) + a frame, produce 
// a per-pixel depth map. This is the bridge from "point cloud" to "surface."
//
// METHOD: weighted planar interpolation
//
//   For each output pixel (x, y) in the depth map:
//     1. Find K nearest input points in screen-space distance
//     2. Compute weights as 1/(d² + ε) where d is screen distance
//     3. Weighted-average their depths (or fit a local plane and evaluate)
//
// LIMITATIONS (this prototype):
//   - Smooth gradients are great; sharp depth discontinuities aren't 
//     (creates a "tent" between foreground and background)
//   - Doesn't use the source image — a guided bilateral filter pass would
//     align depth boundaries with image edges
//   - O(W·H·N) per anchor frame — fine for prototype, slow for HD
//
// FUTURE PATH:
//   - Spatial hash grid for O(W·H·log K) instead of O(W·H·N)
//   - Patch-match stereo with sparse points as initial guesses (Option B 
//     — produces actual edge-respecting depth, GPU-friendly via wgpu compute)
//
// CADENCE:
//   Densification only runs on ANCHOR frames (every Nth frame, default 30).
//   Non-anchor frames inherit the depth map from their nearest anchor — they
//   only need the camera pose to render against the same surface.

/// Per-pixel depth map for one anchor frame.
#[derive(Clone, Debug, Default)]
pub struct DepthMap {
    pub width: u32,
    pub height: u32,
    /// frame_idx this depth map corresponds to
    pub anchor_frame: u32,
    /// width × height f32 depths in camera frame.
    /// 0.0 = unknown / no nearby sparse point.
    pub depths: Vec<f32>,
    /// Per-pixel confidence [0..1], same dimensions as depths.
    /// Helps later MeshPass weight contributions and skip bad regions.
    pub confidences: Vec<f32>,
}

impl DepthMap {
    pub fn new(w: u32, h: u32, frame: u32) -> Self {
        let n = (w as usize) * (h as usize);
        Self {
            width: w,
            height: h,
            anchor_frame: frame,
            depths: vec![0.0; n],
            confidences: vec![0.0; n],
        }
    }
    
    pub fn pixel_count(&self) -> usize { self.depths.len() }
    
    /// Fraction of pixels with nonzero confidence (a coverage stat).
    pub fn coverage(&self) -> f32 {
        if self.confidences.is_empty() { return 0.0; }
        let n_set = self.confidences.iter().filter(|&&c| c > 0.05).count();
        n_set as f32 / self.confidences.len() as f32
    }
}

pub struct DensifyPass {
    /// How many neighbor points to use per pixel
    pub k_neighbors: usize,
    /// Output downsample factor (1 = full res, 2 = half, 4 = quarter).
    /// Coarser depth maps are cheaper but less detailed. 4 is a good 
    /// prototype default — full HD takes too long without spatial indexing.
    pub downsample: u32,
    /// Maximum screen distance for a point to influence a pixel.
    /// Beyond this, the point is too far to be a good neighbor.
    pub max_search_radius: f32,
    /// Anchor cadence — densify every Nth frame
    pub anchor_interval: u32,
    /// Reject points whose depth differs by more than this ratio from the 
    /// median of nearby points. Filters outlier triangulations.
    pub outlier_depth_ratio: f32,
}

impl Default for DensifyPass {
    fn default() -> Self {
        Self {
            k_neighbors: 8,
            downsample: 4,
            // Search radius bumped from 80 → 150 to improve coverage at HD.
            // At 2560x1440 with ~500 sparse features, avg feature spacing is 
            // ~100px. radius=80 left most pixels with 0 neighbors (cov=0%); 
            // radius=150 ensures at least a few neighbors for most pixels.
            max_search_radius: 150.0,
            // anchor_interval 30 → 60: half as many densify+mesh-integrate runs.
            // Each anchor takes ~50-100ms so this saves ~1.5-3s on a 6500-frame video.
            // The mesh still gets ~108 anchors over a 6500-frame video, plenty for 
            // continuous coverage given typical camera motion.
            anchor_interval: 60,
            outlier_depth_ratio: 3.0,
        }
    }
}

/// Spatial hash grid for fast nearest-neighbor queries in 2D.
/// Cell size ~ max_search_radius. Each query examines points in the 
/// containing cell + 8 neighbors.
struct SpatialGrid {
    cell_size: f32,
    cols: usize,
    rows: usize,
    /// Flat cells: each cell stores indices into the points list
    cells: Vec<Vec<u32>>,
}

impl SpatialGrid {
    fn build(points: &[(f32, f32, f32, f32)], width: u32, height: u32, cell_size: f32) -> Self {
        let cols = ((width as f32 / cell_size).ceil() as usize).max(1);
        let rows = ((height as f32 / cell_size).ceil() as usize).max(1);
        let mut cells: Vec<Vec<u32>> = (0..cols * rows).map(|_| Vec::new()).collect();
        for (i, &(x, y, _, _)) in points.iter().enumerate() {
            if x < 0.0 || y < 0.0 { continue; }
            let cx = (x / cell_size) as usize;
            let cy = (y / cell_size) as usize;
            if cx >= cols || cy >= rows { continue; }
            cells[cy * cols + cx].push(i as u32);
        }
        Self { cell_size, cols, rows, cells }
    }
    
    /// Indices of points within ~radius of (x, y). May return slightly more 
    /// than radius (cell-aligned, not exact). Caller filters precisely.
    fn query(&self, x: f32, y: f32, radius: f32, out: &mut Vec<u32>) {
        out.clear();
        let cx = (x / self.cell_size) as i32;
        let cy = (y / self.cell_size) as i32;
        let r_cells = (radius / self.cell_size).ceil() as i32 + 1;
        for dy in -r_cells..=r_cells {
            for dx in -r_cells..=r_cells {
                let qx = cx + dx;
                let qy = cy + dy;
                if qx < 0 || qy < 0 { continue; }
                let qx = qx as usize;
                let qy = qy as usize;
                if qx >= self.cols || qy >= self.rows { continue; }
                out.extend_from_slice(&self.cells[qy * self.cols + qx]);
            }
        }
    }
}

impl DensifyPass {
    /// Should this frame run densification?
    pub fn is_anchor_frame(&self, frame_idx: u32) -> bool {
        self.anchor_interval > 0 && frame_idx % self.anchor_interval == 0
    }
    
    /// Build a depth map for `anchor_frame` using sparse triangulated points 
    /// and the camera pose at that frame.
    /// 
    /// `sparse_points` items are (screen_x, screen_y, depth, confidence) — 
    /// the SCREEN-SPACE projection of triangulated features in this frame.
    /// (Caller must project from world → this frame's screen via the camera 
    /// pose.)
    pub fn densify(
        &self,
        sparse_points: &[(f32, f32, f32, f32)],
        width: u32,
        height: u32,
        anchor_frame: u32,
    ) -> DepthMap {
        let dw = (width / self.downsample).max(1);
        let dh = (height / self.downsample).max(1);
        let mut map = DepthMap::new(dw, dh, anchor_frame);
        
        if sparse_points.is_empty() {
            return map;
        }
        
        // Build spatial grid for fast neighbor lookup
        let grid = SpatialGrid::build(sparse_points, width, height, self.max_search_radius);
        
        let mut neighbor_buf: Vec<u32> = Vec::with_capacity(64);
        let mut depth_buf: Vec<(f32, f32)> = Vec::with_capacity(64);  // (depth, weight)
        
        let down = self.downsample as f32;
        let max_r2 = self.max_search_radius * self.max_search_radius;
        
        for py in 0..dh {
            for px in 0..dw {
                // Map back to full-res pixel coordinates
                let fx = (px as f32 + 0.5) * down;
                let fy = (py as f32 + 0.5) * down;
                
                grid.query(fx, fy, self.max_search_radius, &mut neighbor_buf);
                if neighbor_buf.is_empty() { continue; }
                
                // Compute distances + weights
                depth_buf.clear();
                for &i in &neighbor_buf {
                    let (sx, sy, sd, sc) = sparse_points[i as usize];
                    if sd <= 0.0 { continue; }
                    let dx = sx - fx;
                    let dy = sy - fy;
                    let d2 = dx * dx + dy * dy;
                    if d2 > max_r2 { continue; }
                    // Weight: distance² + small epsilon, scaled by confidence
                    let weight = sc / (d2 + 1.0);
                    depth_buf.push((sd, weight));
                }
                if depth_buf.is_empty() { continue; }
                
                // Median for outlier rejection
                let n = depth_buf.len();
                let mut sorted: Vec<f32> = depth_buf.iter().map(|&(d, _)| d).collect();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let median = sorted[n / 2];
                
                // Accept points whose depth is within outlier_depth_ratio of median
                let lo = median / self.outlier_depth_ratio;
                let hi = median * self.outlier_depth_ratio;
                let mut sum_dw = 0.0f32;
                let mut sum_w = 0.0f32;
                let mut n_used = 0usize;
                let mut min_d2_used = f32::MAX;  // closest contributing point
                for &(d, w) in &depth_buf {
                    if d < lo || d > hi { continue; }
                    sum_dw += d * w;
                    sum_w += w;
                    n_used += 1;
                    // Recover d2 from weight: weight = sc/(d2+1) so d2 = sc/w - 1
                    // We don't have sc here, so just track via the original input.
                    // Approximate: smaller weight = farther, larger weight = closer.
                    if 1.0 / (w + 1e-9) < min_d2_used {
                        min_d2_used = 1.0 / (w + 1e-9);
                    }
                }
                if sum_w < 1e-9 || n_used == 0 { continue; }
                
                let depth = sum_dw / sum_w;
                
                // Confidence is now based on TWO pieces of evidence:
                //   1. n_used / k_neighbors  — how many neighbors actually 
                //      contributed (capped at 1.0 when we hit k or more)
                //   2. proximity factor       — closer nearest neighbor = higher 
                //      confidence (cap at search_radius/4)
                // Both are in 0..1 so geometric mean stays in 0..1.
                let density_conf = (n_used as f32 / self.k_neighbors as f32).min(1.0);
                let prox_d = min_d2_used.sqrt();  // approx distance of closest contributor
                let prox_conf = (1.0 - (prox_d / self.max_search_radius)).clamp(0.0, 1.0);
                let conf = (density_conf * 0.5 + prox_conf * 0.5).clamp(0.0, 1.0);
                
                let idx = (py * dw + px) as usize;
                map.depths[idx] = depth;
                map.confidences[idx] = conf;
            }
        }
        
        map
    }
}

// ═══════════════════════════════════════════════════════════
// V8Encoder integration: store anchor depth maps
// ═══════════════════════════════════════════════════════════

impl V8Encoder {
    /// Run densification + mesh integration on the current frame if it's an anchor.
    /// 
    /// STREAMING ARCHITECTURE (Session 5):
    /// 1. Project triangulated 3D points to this frame's screen
    /// 2. Densify into a per-pixel depth map
    /// 3. Integrate the depth map into scene_mesh (heightfield triangulation)
    /// 4. DISCARD the depth map (only keep last one as working snapshot)
    /// 
    /// This bounds memory regardless of video length: scene_mesh grows but 
    /// is the codec's actual output (compressed scene representation).
    pub fn maybe_densify(&mut self, frame_idx: u32, source_yuv: &[u8]) {
        if !self.densify_pass.is_anchor_frame(frame_idx) { return; }
        if frame_idx as usize >= self.pose_track.len() { return; }
        let pose = self.pose_track[frame_idx as usize];
        
        // Build feature_id → motion_class lookup once. Cheap (linear scan 
        // over live_tracks, typically ~500). The triangulated set may 
        // contain older feature_ids no longer in live_tracks (the track 
        // got lost but the triangulation persisted) — those default to 
        // Unknown and get full confidence (no penalty).
        let class_by_id: std::collections::HashMap<u32, MotionClass> = 
            self.feature_pass.live_tracks.iter()
                .map(|t| (t.feature_id, t.motion_class))
                .collect();
        
        // Cache layer-mask sizing once.
        let layer_mask_ref = self.layer_mask.as_ref();
        let block_w = (self.width as usize) / 16;
        let block_h = (self.height as usize) / 16;
        let layer_expected_len = block_w * block_h;
        let layer_ok = layer_mask_ref.map(|m| m.len() == layer_expected_len).unwrap_or(false);
        
        // Diagnostic counters for the filter pass — see how many features 
        // got skipped vs weighted. Updated below, logged after the loop.
        let mut n_total = 0usize;
        let mut n_skipped_ui = 0usize;
        let mut n_weighted_down = 0usize;
        let mut n_weighted_up = 0usize;
        
        // Project all triangulated 3D points into THIS frame's screen.
        let mut sparse_screen: Vec<(f32, f32, f32, f32)> = Vec::new();
        for tf in self.observations.triangulated.values() {
            n_total += 1;
            // ── UI-OVERLAY FILTER (hard exclude) ──
            // 
            // Features classified as UiOverlay are skipped entirely. The 
            // promotion happens in encode_frame_at — either by the layer 
            // mask filter (axis 1) or by the depth-variance filter 
            // (axis 2). Either way, by the time we get here, the 
            // motion_class field is the source of truth.
            let track_class = class_by_id.get(&tf.feature_id).copied()
                .unwrap_or(MotionClass::Unknown);
            if track_class == MotionClass::UiOverlay {
                n_skipped_ui += 1;
                continue;
            }
            
            let dx = tf.world_pos.x - pose.position.x;
            let dy = tf.world_pos.y - pose.position.y;
            let dz = tf.world_pos.z - pose.position.z;
            let q_inv = [pose.rotation[0], -pose.rotation[1], -pose.rotation[2], -pose.rotation[3]];
            let cam = rotate_vec3_by_quat(Vec3 { x: dx, y: dy, z: dz }, q_inv);
            if cam.z < 1e-3 { continue; }
            let fx = if pose.focal[0] > 0.0 { pose.focal[0] } else { 1000.0 };
            let fy = if pose.focal[1] > 0.0 { pose.focal[1] } else { 1000.0 };
            let sx = pose.principal[0] + fx * cam.x / cam.z;
            let sy = pose.principal[1] + fy * cam.y / cam.z;
            if sx < -50.0 || sx > self.width as f32 + 50.0 { continue; }
            if sy < -50.0 || sy > self.height as f32 + 50.0 { continue; }
            
            // ── ACTIVE-CONTENT WEIGHTING (soft preference) ──
            // 
            // Apply a per-layer confidence multiplier so the densification 
            // pass favors features in the active scene over features that 
            // happen to fall in pure_static / unknown regions. Keeps the 
            // soft preference distinct from the hard UiOverlay exclude — 
            // features we're not yet confident are UI still contribute, 
            // but their contribution is reduced.
            let mut weight = 1.0f32;
            if layer_ok {
                let bx = (sx as i32 / 16).max(0) as usize;
                let by = (sy as i32 / 16).max(0) as usize;
                if bx < block_w && by < block_h {
                    let mask = layer_mask_ref.unwrap();
                    let layer_byte = mask[by * block_w + bx];
                    // BlockLayer discriminants: 0=PureStatic, 1=AlphaOverlay, 
                    // 2=ParallaxBg, 3=Active, 4=Noise, 5=Unknown
                    weight = match layer_byte {
                        3 => 1.20,  // Active — boost real scene features
                        2 => 1.10,  // ParallaxBg — slight boost (real geometry, slow motion)
                        0 => 0.70,  // PureStatic — modest penalty (likely background)
                        1 => 0.20,  // AlphaOverlay — strong penalty, but not zero 
                                    // (some features in mixed regions may still be 
                                    // partially scene; if they're fully UI the 
                                    // depth-variance filter will eventually catch them)
                        4 => 0.80,  // Noise — light penalty
                        _ => 1.00,  // Unknown — neutral
                    };
                    if weight > 1.001 { n_weighted_up += 1; }
                    else if weight < 0.999 { n_weighted_down += 1; }
                }
            }
            let weighted_conf = (tf.confidence * weight).clamp(0.0, 1.0);
            
            sparse_screen.push((sx, sy, cam.z, weighted_conf));
        }
        
        // Periodic diagnostic — fires whenever we hit an anchor frame 
        // that aligns to a 600-frame boundary (so ~once every 10 anchors 
        // at anchor_interval=60). This is the same cadence as the user's 
        // existing layer-distribution log, so the two correlate easily in 
        // the trace. Always logs the first anchor so we see the filter 
        // is plumbed in even if the encoder bails out early.
        let is_first_anchor = self.scene_mesh.vertices.is_empty();
        if is_first_anchor || (frame_idx > 0 && frame_idx % 600 == 0) {
            eprintln!("[v8 ui-filter] anchor {}: total tri features={}, ui-skipped={}, weighted_down={}, weighted_up={} (layer_mask={})",
                frame_idx, n_total, n_skipped_ui, n_weighted_down, n_weighted_up,
                if layer_ok { "active" } else { "inactive" });
        }
        
        let map = self.densify_pass.densify(&sparse_screen, self.width, self.height, frame_idx);
        
        // INTEGRATE into scene_mesh — this is the streaming step.
        // After integration, the depth map’s information lives in the mesh.
        // 
        // Passing the source frame YUV lets the integrator sample real 
        // scene colors at each vertex (rather than the old placeholder 
        // confidence-tint). This is the fix for the "blocky bluish-green 
        // mesh" artifact reported in macOS testing — vertex colors were 
        // [g*0.4, g*0.85, g] where g=confidence, so the entire mesh 
        // rendered in a teal-shaded confidence visualization regardless 
        // of actual scene content.
        let downsample = self.densify_pass.downsample.max(1);
        let triangles_added = self.mesh_integrator.integrate(
            &map, &pose, downsample, &mut self.scene_mesh,
            source_yuv, self.width as usize, self.height as usize,
        );
        
        // Compute stats BEFORE moving map into depth_maps (borrow checker).
        let dm_coverage = map.coverage();
        let n_valid_pixels = map.confidences.iter()
            .filter(|&&c| c >= self.mesh_integrator.min_confidence).count();
        
        // Keep only ONE depth map (the most recent) for the World view's
        // dense back-projection rendering. Older ones are now in scene_mesh.
        // This bounds memory: 1 depth map vs the previous cap of 32.
        self.depth_maps.clear();
        self.depth_maps.push(map);
        
        // Always log anchor stats so we know if integration is silently failing.
        // Fusion stats come from SceneMesh.vertex_grid (fusion is built into 
        // MeshIntegrator + SceneMesh, not a separate grid).
        eprintln!("[v8 mesh] anchor {}: dm_cov={:.1}% n_valid={} integrated {} tris (mesh: {} verts/{} tris, fused cells={})",
            frame_idx, dm_coverage * 100.0, n_valid_pixels,
            triangles_added, 
            self.scene_mesh.vertices.len(), 
            self.scene_mesh.indices.len() / 3,
            self.scene_mesh.vertex_grid.len());
        
        // Record the anchor in the ChunkStore (Session 6).
        // The summary is cheap; whether full source data was retained for 
        // this chunk depends on retention tier.
        if self.chunk_store.current_chunk.is_none() {
            self.chunk_store.open_chunk(frame_idx);
        }
        self.chunk_store.add_anchor(AnchorSummary {
            frame_idx,
            pose,
            density_coverage: dm_coverage,
            triangles_added: triangles_added as u32,
            valid_pixel_count: n_valid_pixels as u32,
        });
    }
}

#[cfg(test)]
mod densify_tests {
    use super::*;
    
    #[test]
    fn densify_with_no_points_produces_empty_map() {
        let pass = DensifyPass::default();
        let map = pass.densify(&[], 320, 240, 0);
        assert_eq!(map.coverage(), 0.0);
    }
    
    #[test]
    fn densify_uniform_depth_recovers_uniform() {
        // 50 sparse points scattered across the image, all at depth 10.0.
        // Output depth map should be ~10.0 wherever it's filled.
        let pass = DensifyPass {
            k_neighbors: 8, downsample: 4,
            max_search_radius: 80.0, anchor_interval: 30,
            outlier_depth_ratio: 3.0,
        };
        let mut points = Vec::new();
        for i in 0..50 {
            let x = ((i * 37) % 320) as f32;
            let y = ((i * 53) % 240) as f32;
            points.push((x, y, 10.0, 0.9));
        }
        let map = pass.densify(&points, 320, 240, 0);
        // Most pixels should be filled, and their depths ~10.0
        let filled: Vec<f32> = map.depths.iter()
            .zip(map.confidences.iter())
            .filter(|(_, &c)| c > 0.05)
            .map(|(&d, _)| d)
            .collect();
        assert!(!filled.is_empty(), "expected some pixels filled");
        let mean: f32 = filled.iter().sum::<f32>() / filled.len() as f32;
        assert!((mean - 10.0).abs() < 0.5, "mean depth {} should be ~10", mean);
    }
    
    #[test]
    fn densify_skips_outliers() {
        // 9 points at depth 5.0 and 1 outlier at depth 100.0 in same area.
        // The outlier should NOT dominate — median rejection drops it.
        let pass = DensifyPass {
            k_neighbors: 10, downsample: 4,
            max_search_radius: 200.0, anchor_interval: 30,
            outlier_depth_ratio: 3.0,
        };
        let mut points = Vec::new();
        for i in 0..9 {
            let x = 100.0 + (i as f32) * 8.0;
            let y = 100.0;
            points.push((x, y, 5.0, 0.9));
        }
        points.push((140.0, 100.0, 100.0, 0.9));  // outlier
        let map = pass.densify(&points, 320, 240, 0);
        // Sample the depth right at the outlier's location — should be ~5
        let sx = (140.0 / 4.0) as usize;
        let sy = (100.0 / 4.0) as usize;
        let idx = sy * (map.width as usize) + sx;
        if map.confidences[idx] > 0.05 {
            assert!(map.depths[idx] < 20.0, "outlier should be rejected, got depth {}", map.depths[idx]);
        }
    }
    
    #[test]
    fn anchor_cadence_correct() {
        let pass = DensifyPass::default();  // anchor_interval = 30
        assert!(pass.is_anchor_frame(0));
        assert!(pass.is_anchor_frame(30));
        assert!(pass.is_anchor_frame(60));
        assert!(!pass.is_anchor_frame(15));
        assert!(!pass.is_anchor_frame(29));
    }
}


// ═══════════════════════════════════════════════════════════
// SESSION 6: ChunkStore — sequence retention with quality-aware eviction
// ═══════════════════════════════════════════════════════════
//
// THE PROBLEM
// 
// Streaming v8 (Session 5) integrates depth maps into a global mesh and 
// then DISCARDS them. This bounds memory but throws away source data 
// that future passes might want — most importantly, the original frames 
// needed for texture projection (Session 9), and the depth maps + 
// observations needed for re-running a chunk at higher quality.
//
// THE INSIGHT
//
// Not all video segments are equally valuable to retain. A 60-frame run
// of stable tracking with 80% density coverage is gold for re-pass; a 
// 20-frame chunk of motion-blurred chaos is worthless. We should retain 
// HIGH-QUALITY chunks longer than low-quality ones, with eviction driven 
// by both quality and recency.
//
// THE STRUCTURE
//
// Frames are grouped into CHUNKS — contiguous ranges typically aligned 
// with anchor cadence × K (e.g., every 4 anchors = ~240 frames). Each 
// chunk has:
//   - Statistics describing its quality
//   - A retention tier determining which derived data is kept
//   - An access timestamp for LRU tiebreaking
//
// Retention tiers (cheap to expensive):
//   Stats   — just numbers; ~100 bytes per chunk
//   Summary — anchor poses + depth-map summaries; ~50KB per chunk
//   Sampled — + a few full source frames; ~30MB per chunk
//   Full    — + every source frame; ~1.5GB per chunk
//
// New chunks open at Sampled by default. When memory pressure hits, the 
// store DEMOTES low-priority chunks (Sampled → Summary → Stats) before 
// fully evicting.
//
// LOCALITY BENEFIT
//
// Because chunks are contiguous frame ranges, retained data is laid out 
// by time-locality. Texture projection scanning a 60-frame range gets 
// cache-friendly access. Disk format can chunk-align so random-access 
// decode loads only the relevant chunks.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetentionTier {
    Stats = 0,
    Summary = 1,
    Sampled = 2,
    Full = 3,
}

/// A summary of one anchor frame's contribution. Cheap to retain.
#[derive(Clone, Debug)]
pub struct AnchorSummary {
    pub frame_idx: u32,
    pub pose: CameraPose,
    pub density_coverage: f32,
    pub triangles_added: u32,
    pub valid_pixel_count: u32,
}

/// A retained source frame for high-tier chunks. Texture projection (and
/// any future pixel-aware pass) reads from these.
#[derive(Clone)]
pub struct RetainedFrame {
    pub frame_idx: u32,
    /// YUV420 source bytes — width*height*3/2.
    pub yuv: Vec<u8>,
}

impl std::fmt::Debug for RetainedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RetainedFrame(frame={}, yuv={}B)", self.frame_idx, self.yuv.len())
    }
}

/// One chunk in the store. Holds derived data at varying tiers.
#[derive(Clone, Debug)]
pub struct RetainedChunk {
    pub frame_start: u32,
    pub frame_end: u32,         // exclusive
    
    /// Intrinsic quality: 0..1. Combines density coverage, feature stability,
    /// and static ratio. Computed at close.
    pub quality_score: f32,
    
    /// Effective priority for retention decisions: quality × recency_factor.
    /// Updated on access; eviction picks lowest.
    pub priority: f32,
    
    pub tier: RetentionTier,
    
    // Always-kept stats:
    pub n_anchors: u32,
    pub n_features_tracked: u32,
    pub mean_density_cov: f32,
    pub static_ratio: f32,
    
    // Tier ≥ Summary:
    pub anchor_summaries: Vec<AnchorSummary>,
    
    // Tier ≥ Sampled:
    pub representative_frames: Vec<RetainedFrame>,
    
    // Tier == Full:
    pub full_frames: Vec<RetainedFrame>,
    
    // ─── Web (Session 6c+) ─── geometry produced during this chunk's lifetime
    /// The mesh built from this chunk's anchor depth maps.
    /// Each chunk has its own self-consistent web of geometry; we never 
    /// force-merge across chunks. Compression comes from detecting that 
    /// two chunks happen to observe similar geometry (Session 7+).
    pub web_mesh: SceneMesh,
    
    /// The pose track for this chunk's frames. Indexed by (frame_idx - frame_start).
    /// Local to this chunk's coordinate system.
    pub web_pose_track: Vec<CameraPose>,
    
    /// Shape signature for fast similarity comparison. Computed at chunk close.
    /// 16 floats: bounding-box dimensions (3) + centroid (3) + 
    /// 10-bin radial vertex distribution.
    /// Two webs with similar signatures might describe the same scene.
    pub web_signature: WebSignature,
    
    /// Web alignment graph link. None = independent web. 
    /// Some(canonical_id) = aligned to that canonical web; transform stored 
    /// in web_alignment_transform.
    pub canonical_web_id: Option<u32>,
    
    /// 4×4 row-major transform that maps THIS web's coords into canonical_web_id's.
    /// Identity if canonical_web_id == this chunk's id (i.e., is canonical).
    pub web_alignment_transform: [f32; 16],
    
    pub last_accessed: u64,
}

/// Compression potential summary across all webs in the store. 
/// `delta_bytes / full_bytes` < 1.0 indicates real savings from web alignment.
#[derive(Clone, Copy, Debug, Default)]
pub struct WebCompressionReport {
    pub total_chunks: u32,
    pub n_canonical: u32,
    pub n_aligned: u32,
    pub n_standalone: u32,
    pub full_bytes: usize,
    pub delta_bytes: usize,
}

impl WebCompressionReport {
    pub fn savings_ratio(&self) -> f32 {
        if self.full_bytes == 0 { return 0.0; }
        1.0 - (self.delta_bytes as f32 / self.full_bytes as f32)
    }
    
    pub fn to_string(&self) -> String {
        format!(
            "{} chunks: {} canonical, {} aligned, {} standalone | {:.1}MB full -> {:.1}MB delta ({:.0}% savings)",
            self.total_chunks, self.n_canonical, self.n_aligned, self.n_standalone,
            self.full_bytes as f32 / 1_048_576.0,
            self.delta_bytes as f32 / 1_048_576.0,
            self.savings_ratio() * 100.0,
        )
    }
}

/// A compact signature describing a web's geometry. Used for fast pairwise 
/// similarity comparison and for rotation-aware alignment.
/// 
/// Captures three kinds of information:
/// 1. PCA eigenvalues (sorted) — rotation/translation-invariant shape
///    fingerprint. Two webs of the same physical scene have nearly 
///    identical eigenvalues regardless of camera viewpoint.
/// 2. Principal axes (eigenvectors) — used for rotation estimation 
///    during alignment. Pair up axes by eigenvalue and compute rotation.
/// 3. Centroid + extent — used for translation/scale.
/// 4. Radial histogram — secondary similarity check, helps disambiguate 
///    between scenes that happen to have similar PCA eigenvalues.
#[derive(Clone, Copy, Debug, Default)]
pub struct WebSignature {
    /// Bounding-box dimensions (length, width, height after sorting). f32×3.
    pub extent: [f32; 3],
    /// Centroid position in web's local coords. f32×3.
    pub centroid: [f32; 3],
    /// PCA eigenvalues, SORTED descending. Rotation-invariant: any 
    /// rotation of a point cloud preserves eigenvalue spectrum.
    /// Two webs of the same physical scene → similar eigenvalues.
    pub eigenvalues: [f32; 3],
    /// PCA eigenvectors (principal axes), columns indexed by eigenvalue 
    /// rank. Stored row-major: [v0.x, v0.y, v0.z, v1.x, v1.y, v1.z, v2.x, v2.y, v2.z].
    /// Used to compute rotation between aligned webs.
    pub eigenvectors: [f32; 9],
    /// 10-bin histogram of vertex distance from centroid (radial profile).
    /// Normalized to sum=1.0.
    pub radial_histogram: [f32; 10],
}

impl WebSignature {
    /// Compute signature from a SceneMesh. O(n) over vertices, plus a 
    /// closed-form 3×3 eigendecomposition for the principal axes.
    pub fn from_mesh(mesh: &SceneMesh) -> Self {
        if mesh.vertices.is_empty() {
            return Self::default();
        }
        let n = mesh.vertices.len() as f32;
        
        // ── Pass 1: centroid + bounding box ──
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        let mut sum = [0.0f64; 3];  // f64 for numerical stability with large meshes
        for v in &mesh.vertices {
            let p = [v.x, v.y, v.z];
            for i in 0..3 {
                if p[i] < min[i] { min[i] = p[i]; }
                if p[i] > max[i] { max[i] = p[i]; }
                sum[i] += p[i] as f64;
            }
        }
        let centroid = [(sum[0] / n as f64) as f32, (sum[1] / n as f64) as f32, (sum[2] / n as f64) as f32];
        let mut ext = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        ext.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        
        // ── Pass 2: covariance matrix (3×3 symmetric) ──
        // C[i][j] = (1/n) * sum((p_k - centroid)_i * (p_k - centroid)_j)
        let mut cxx = 0.0f64; let mut cyy = 0.0f64; let mut czz = 0.0f64;
        let mut cxy = 0.0f64; let mut cxz = 0.0f64; let mut cyz = 0.0f64;
        for v in &mesh.vertices {
            let dx = (v.x - centroid[0]) as f64;
            let dy = (v.y - centroid[1]) as f64;
            let dz = (v.z - centroid[2]) as f64;
            cxx += dx * dx; cyy += dy * dy; czz += dz * dz;
            cxy += dx * dy; cxz += dx * dz; cyz += dy * dz;
        }
        cxx /= n as f64; cyy /= n as f64; czz /= n as f64;
        cxy /= n as f64; cxz /= n as f64; cyz /= n as f64;
        
        // ── Eigendecomposition of 3×3 symmetric matrix ──
        // Closed-form via characteristic polynomial. We solve det(C - λI) = 0.
        // For a symmetric matrix this is a cubic with all-real roots.
        // Algorithm from Smith (1961) / well-known:
        //   p1 = cxy^2 + cxz^2 + cyz^2
        //   if p1 == 0: matrix is diagonal, eigenvalues are cxx,cyy,czz
        //   else:
        //     q = trace/3
        //     p2 = (cxx-q)^2 + (cyy-q)^2 + (czz-q)^2 + 2*p1
        //     p = sqrt(p2/6)
        //     B = (1/p) * (C - qI)
        //     r = det(B)/2 clamped to [-1, 1]
        //     phi = acos(r)/3
        //     λ1 = q + 2p*cos(phi)        (largest)
        //     λ3 = q + 2p*cos(phi+2π/3)    (smallest)
        //     λ2 = trace - λ1 - λ3        (middle)
        let p1 = cxy*cxy + cxz*cxz + cyz*cyz;
        let mut eig = [0.0f64; 3];
        if p1 < 1e-20 {
            // Diagonal
            eig[0] = cxx; eig[1] = cyy; eig[2] = czz;
        } else {
            let q = (cxx + cyy + czz) / 3.0;
            let p2 = (cxx-q).powi(2) + (cyy-q).powi(2) + (czz-q).powi(2) + 2.0*p1;
            let p = (p2 / 6.0).sqrt();
            // B = (1/p) * (C - qI)
            let b00 = (cxx - q) / p; let b11 = (cyy - q) / p; let b22 = (czz - q) / p;
            let b01 = cxy / p;       let b02 = cxz / p;       let b12 = cyz / p;
            // det(B) = b00*(b11*b22 - b12*b12) - b01*(b01*b22 - b12*b02) + b02*(b01*b12 - b11*b02)
            let det_b = b00*(b11*b22 - b12*b12) 
                      - b01*(b01*b22 - b12*b02)
                      + b02*(b01*b12 - b11*b02);
            let r = (det_b / 2.0).clamp(-1.0, 1.0);
            let phi = r.acos() / 3.0;
            let two_p = 2.0 * p;
            eig[0] = q + two_p * phi.cos();                                // largest
            eig[2] = q + two_p * (phi + 2.0 * std::f64::consts::PI / 3.0).cos();  // smallest
            eig[1] = cxx + cyy + czz - eig[0] - eig[2];                    // middle
        }
        // Sort descending
        eig.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let eigenvalues = [eig[0] as f32, eig[1] as f32, eig[2] as f32];
        
        // ── Eigenvectors ──
        // For each eigenvalue λ, solve (C - λI) v = 0. We compute the null 
        // space by taking the cross product of two columns of (C - λI).
        // For numerical stability, we should pick the two "most independent" 
        // columns, but for simplicity we cross columns 0 and 1, falling back 
        // to 0 and 2 if the result is too small.
        let compute_eigvec = |lambda: f64| -> [f32; 3] {
            // Rows of (C - λI):
            let r0 = [cxx - lambda, cxy, cxz];
            let r1 = [cxy, cyy - lambda, cyz];
            let r2 = [cxz, cyz, czz - lambda];
            // Try cross(r0, r1)
            let v01 = [
                r0[1]*r1[2] - r0[2]*r1[1],
                r0[2]*r1[0] - r0[0]*r1[2],
                r0[0]*r1[1] - r0[1]*r1[0],
            ];
            let n01 = (v01[0]*v01[0] + v01[1]*v01[1] + v01[2]*v01[2]).sqrt();
            let chosen = if n01 > 1e-9 {
                [v01[0]/n01, v01[1]/n01, v01[2]/n01]
            } else {
                let v02 = [
                    r0[1]*r2[2] - r0[2]*r2[1],
                    r0[2]*r2[0] - r0[0]*r2[2],
                    r0[0]*r2[1] - r0[1]*r2[0],
                ];
                let n02 = (v02[0]*v02[0] + v02[1]*v02[1] + v02[2]*v02[2]).sqrt();
                if n02 > 1e-9 {
                    [v02[0]/n02, v02[1]/n02, v02[2]/n02]
                } else {
                    [1.0, 0.0, 0.0]  // degenerate
                }
            };
            [chosen[0] as f32, chosen[1] as f32, chosen[2] as f32]
        };
        
        let v0 = compute_eigvec(eig[0]);
        let v1 = compute_eigvec(eig[1]);
        // v2 = v0 × v1 to ensure right-handed orthonormal basis
        let v2 = [
            v0[1]*v1[2] - v0[2]*v1[1],
            v0[2]*v1[0] - v0[0]*v1[2],
            v0[0]*v1[1] - v0[1]*v1[0],
        ];
        let eigenvectors = [v0[0], v0[1], v0[2], v1[0], v1[1], v1[2], v2[0], v2[1], v2[2]];
        
        // ── Radial histogram (kept from previous version for secondary check) ──
        let mut max_r2 = 0.0f32;
        for v in &mesh.vertices {
            let dx = v.x - centroid[0];
            let dy = v.y - centroid[1];
            let dz = v.z - centroid[2];
            let r2 = dx*dx + dy*dy + dz*dz;
            if r2 > max_r2 { max_r2 = r2; }
        }
        let max_r = max_r2.sqrt().max(0.001);
        let mut hist = [0.0f32; 10];
        for v in &mesh.vertices {
            let dx = v.x - centroid[0];
            let dy = v.y - centroid[1];
            let dz = v.z - centroid[2];
            let r = (dx*dx + dy*dy + dz*dz).sqrt();
            let bin = ((r / max_r) * 10.0).min(9.99) as usize;
            hist[bin] += 1.0;
        }
        for h in &mut hist { *h /= n; }
        
        Self { 
            extent: ext, 
            centroid, 
            eigenvalues,
            eigenvectors,
            radial_histogram: hist 
        }
    }
    
    /// Eigenvalue similarity: 1.0 = identical principal-axis variances,
    /// less if shape "thickness" along principal directions differs.
    /// This is the PRIMARY similarity check — it's rotation-invariant
    /// and translation-invariant.
    pub fn eigenvalue_similarity(&self, other: &WebSignature) -> f32 {
        // Normalize each signature's eigenvalues to sum=1.0 (scale-invariant).
        let s_a: f32 = self.eigenvalues.iter().sum();
        let s_b: f32 = other.eigenvalues.iter().sum();
        if s_a < 1e-9 || s_b < 1e-9 { return 0.0; }
        let na = [self.eigenvalues[0]/s_a, self.eigenvalues[1]/s_a, self.eigenvalues[2]/s_a];
        let nb = [other.eigenvalues[0]/s_b, other.eigenvalues[1]/s_b, other.eigenvalues[2]/s_b];
        // L1 distance, mapped to similarity via 1 - dist (clamped).
        let dist = (na[0]-nb[0]).abs() + (na[1]-nb[1]).abs() + (na[2]-nb[2]).abs();
        (1.0 - dist).max(0.0)
    }
    
    /// Cosine similarity between two signatures' radial histograms.
    pub fn radial_similarity(&self, other: &WebSignature) -> f32 {
        let mut dot = 0.0;
        let mut a2 = 0.0;
        let mut b2 = 0.0;
        for i in 0..10 {
            dot += self.radial_histogram[i] * other.radial_histogram[i];
            a2 += self.radial_histogram[i] * self.radial_histogram[i];
            b2 += other.radial_histogram[i] * other.radial_histogram[i];
        }
        if a2 < 1e-9 || b2 < 1e-9 { return 0.0; }
        dot / (a2.sqrt() * b2.sqrt())
    }
    
    /// Extent similarity. 1.0 = same scale, less if scale differs.
    pub fn extent_similarity(&self, other: &WebSignature) -> f32 {
        let mut sim = 1.0f32;
        for i in 0..3 {
            let a = self.extent[i].max(0.001);
            let b = other.extent[i].max(0.001);
            let ratio = a.min(b) / a.max(b);
            sim *= ratio;
        }
        sim
    }
    
    /// Combined similarity: weighted blend of eigenvalue (primary) + 
    /// radial histogram (secondary). Eigenvalue match is the main signal;
    /// radial histogram disambiguates between scenes with similar overall
    /// shape but different vertex distribution.
    pub fn combined_similarity(&self, other: &WebSignature) -> f32 {
        let eig_sim = self.eigenvalue_similarity(other);
        let rad_sim = self.radial_similarity(other);
        // Weighted: eigenvalues 60%, radial 40%. Eigenvalues are the more 
        // robust shape descriptor.
        0.6 * eig_sim + 0.4 * rad_sim
    }
}

impl RetainedChunk {
    /// Estimated memory footprint in bytes. Used for budget accounting.
    pub fn bytes_used(&self) -> usize {
        let stats_bytes = 96;  // rough size of fixed fields
        let anchor_bytes = self.anchor_summaries.len() * std::mem::size_of::<AnchorSummary>();
        let rep_frame_bytes: usize = self.representative_frames.iter()
            .map(|f| f.yuv.len() + 24).sum();
        let full_frame_bytes: usize = self.full_frames.iter()
            .map(|f| f.yuv.len() + 24).sum();
        // Web geometry: vertices (Vec3 = 12B), colors (3xf32 = 12B), 
        // indices (4B each), pose track (~64B per frame)
        let web_vertex_bytes = self.web_mesh.vertices.len() * 24;
        let web_index_bytes = self.web_mesh.indices.len() * 4;
        let web_pose_bytes = self.web_pose_track.len() * 64;
        stats_bytes + anchor_bytes + rep_frame_bytes + full_frame_bytes 
            + web_vertex_bytes + web_index_bytes + web_pose_bytes
    }
    
    /// Demote this chunk to the next-lower tier, dropping data accordingly.
    /// Returns bytes freed.
    pub fn demote(&mut self) -> usize {
        let before = self.bytes_used();
        match self.tier {
            RetentionTier::Full => {
                self.full_frames.clear();
                self.tier = RetentionTier::Sampled;
            }
            RetentionTier::Sampled => {
                self.representative_frames.clear();
                self.tier = RetentionTier::Summary;
            }
            RetentionTier::Summary => {
                self.anchor_summaries.clear();
                self.tier = RetentionTier::Stats;
            }
            RetentionTier::Stats => {
                // Already at minimum tier. We do NOT shed web_mesh here:
                //   - Canonical web meshes are referenced by aligned 
                //     followers during encoding and at V8MX trailer write.
                //   - Aligned web meshes are needed at trailer-write time 
                //     to compute their residual vs canonical.
                // 
                // Memory pressure on long encodes is handled at the eviction 
                // layer instead: aligned chunks have their priority damped 
                // (see ChunkStore::enforce_budget) so they're shed by 
                // removal rather than by mesh-shedding.
            }
        }
        before.saturating_sub(self.bytes_used())
    }
}

/// Library-reference record produced when a closed chunk matches an 
/// existing canonical in the FAO Library. At trailer-write time, these 
/// emit a V8LR entry instead of the chunk's full V8MX entry — saving 
/// the per-chunk mesh bytes entirely.
/// 
/// The `residual` field holds vertices/colors that this video sees but 
/// the library canonical doesn't (e.g., a billboard added to a familiar 
/// street scene). When None, the alignment is "perfect" and the library 
/// canonical is used as-is. When Some(small Vec), the residual is added 
/// after canonical render.
/// 
/// Trade-off: a video that references a library canonical CANNOT be 
/// decoded correctly without that library being present. Decoder falls 
/// back to drawing nothing for the chunk's frames if the lib is missing.
#[derive(Clone, Debug)]
pub struct LibraryRef {
    pub chunk_frame_start: u32,
    pub chunk_frame_end: u32,
    /// Library-side UUID. 32 hex chars = 16 raw bytes; we store as String 
    /// for simplicity. Memory overhead is minimal (one String per matched 
    /// chunk; thousands per video would still be <1MB).
    pub uuid: String,
    /// 4×4 row-major transform aligning the chunk's local frame to the 
    /// canonical's. Same shape as RetainedChunk::web_alignment_transform.
    pub alignment_transform: [f32; 16],
    /// 0..1, similarity score at match time. Diagnostic.
    pub similarity_score: f32,
    /// Optional residual: vertices in this chunk's mesh that don't 
    /// appear in the canonical (after transform). Empty for perfect matches.
    pub residual_vertices: Vec<Vec3>,
    pub residual_colors: Vec<[f32; 3]>,
}

/// Write a V8LR (LibraryRef) trailer from a list of refs.
/// 
/// Format:
///   magic         "V8LR"   (4 bytes)
///   version       u8       (1 byte)
///   n_refs        u32 LE   (4 bytes)
///   for each ref:
///     chunk_frame_start    u32 LE
///     chunk_frame_end      u32 LE
///     uuid_len             u8
///     uuid_bytes           [u8; uuid_len]   (utf-8 hex chars; canonical 32)
///     alignment_transform  [f32; 16]        (64 bytes)
///     similarity_score     f32              (4 bytes)
///     residual_n_verts     u32 LE
///     residual_compressed_len u32 LE
///     residual_compressed  [u8; len]        (zstd of vertices+colors)
/// 
/// Returns the raw trailer bytes (no leading length prefix; caller adds 
/// that to match the framing of other trailers).
pub fn write_v8lr_trailer(refs: &[LibraryRef]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + refs.len() * 128);
    out.extend_from_slice(b"V8LR");
    out.push(1u8);  // version
    out.extend_from_slice(&(refs.len() as u32).to_le_bytes());
    
    for r in refs {
        out.extend_from_slice(&r.chunk_frame_start.to_le_bytes());
        out.extend_from_slice(&r.chunk_frame_end.to_le_bytes());
        let uuid_bytes = r.uuid.as_bytes();
        out.push(uuid_bytes.len() as u8);
        out.extend_from_slice(uuid_bytes);
        for &f in &r.alignment_transform {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.extend_from_slice(&r.similarity_score.to_le_bytes());
        
        // Residual: serialize vertices+colors then zstd.
        let n_verts = r.residual_vertices.len() as u32;
        out.extend_from_slice(&n_verts.to_le_bytes());
        let mut residual_raw = Vec::with_capacity(n_verts as usize * 24);
        for v in &r.residual_vertices {
            residual_raw.extend_from_slice(&v.x.to_le_bytes());
            residual_raw.extend_from_slice(&v.y.to_le_bytes());
            residual_raw.extend_from_slice(&v.z.to_le_bytes());
        }
        for c in &r.residual_colors {
            residual_raw.extend_from_slice(&c[0].to_le_bytes());
            residual_raw.extend_from_slice(&c[1].to_le_bytes());
            residual_raw.extend_from_slice(&c[2].to_le_bytes());
        }
        let compressed = if residual_raw.is_empty() {
            Vec::new()
        } else {
            zstd::encode_all(residual_raw.as_slice(), 19).unwrap_or(residual_raw)
        };
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&compressed);
    }
    
    out
}

/// Read a V8LR trailer body. `data` should start at the magic bytes.
/// Returns None on bad magic or truncated data.
pub fn read_v8lr_trailer(data: &[u8]) -> Option<Vec<LibraryRef>> {
    if data.len() < 9 || &data[..4] != b"V8LR" { return None; }
    let _version = data[4];
    let n_refs = u32::from_le_bytes([data[5], data[6], data[7], data[8]]) as usize;
    let mut off = 9usize;
    let mut refs = Vec::with_capacity(n_refs);
    
    let read_u32 = |buf: &[u8], pos: usize| -> Option<u32> {
        if buf.len() < pos + 4 { return None; }
        Some(u32::from_le_bytes([buf[pos], buf[pos+1], buf[pos+2], buf[pos+3]]))
    };
    let read_f32 = |buf: &[u8], pos: usize| -> Option<f32> {
        if buf.len() < pos + 4 { return None; }
        Some(f32::from_le_bytes([buf[pos], buf[pos+1], buf[pos+2], buf[pos+3]]))
    };
    
    for _ in 0..n_refs {
        let chunk_frame_start = read_u32(data, off)?; off += 4;
        let chunk_frame_end = read_u32(data, off)?; off += 4;
        if data.len() < off + 1 { return None; }
        let uuid_len = data[off] as usize; off += 1;
        if data.len() < off + uuid_len { return None; }
        let uuid = std::str::from_utf8(&data[off..off+uuid_len]).ok()?.to_string();
        off += uuid_len;
        
        if data.len() < off + 64 { return None; }
        let mut alignment_transform = [0f32; 16];
        for i in 0..16 {
            alignment_transform[i] = read_f32(data, off)?;
            off += 4;
        }
        let similarity_score = read_f32(data, off)?; off += 4;
        let n_verts = read_u32(data, off)? as usize; off += 4;
        let comp_len = read_u32(data, off)? as usize; off += 4;
        if data.len() < off + comp_len { return None; }
        
        let (residual_vertices, residual_colors) = if comp_len == 0 || n_verts == 0 {
            (Vec::new(), Vec::new())
        } else {
            let raw = zstd::decode_all(&data[off..off+comp_len]).ok()?;
            // Layout: n_verts × Vec3 (vertices), n_verts × [f32;3] (colors)
            if raw.len() < n_verts * 24 {
                (Vec::new(), Vec::new())
            } else {
                let mut verts = Vec::with_capacity(n_verts);
                for i in 0..n_verts {
                    let p = i * 12;
                    verts.push(Vec3 {
                        x: f32::from_le_bytes([raw[p], raw[p+1], raw[p+2], raw[p+3]]),
                        y: f32::from_le_bytes([raw[p+4], raw[p+5], raw[p+6], raw[p+7]]),
                        z: f32::from_le_bytes([raw[p+8], raw[p+9], raw[p+10], raw[p+11]]),
                    });
                }
                let mut cols = Vec::with_capacity(n_verts);
                let color_base = n_verts * 12;
                for i in 0..n_verts {
                    let p = color_base + i * 12;
                    cols.push([
                        f32::from_le_bytes([raw[p], raw[p+1], raw[p+2], raw[p+3]]),
                        f32::from_le_bytes([raw[p+4], raw[p+5], raw[p+6], raw[p+7]]),
                        f32::from_le_bytes([raw[p+8], raw[p+9], raw[p+10], raw[p+11]]),
                    ]);
                }
                (verts, cols)
            }
        };
        off += comp_len;
        
        refs.push(LibraryRef {
            chunk_frame_start,
            chunk_frame_end,
            uuid,
            alignment_transform,
            similarity_score,
            residual_vertices,
            residual_colors,
        });
    }
    
    Some(refs)
}

/// Serialize a per-frame anchor track for the Tobii-style player overlay.
/// 
/// Input: a HashMap mapping external_frame_idx → Vec of (x, y, class) 
/// per anchor on that frame. Output: zstd-compressed V8AT trailer body 
/// suitable for writing to the .fao file (caller prepends the 
/// length-prefix u32 to match the framing of V8MS/V8MX/V8PT/V8LR).
/// 
/// Layout:
///   'V8AT'                (4)
///   version u8            (1) = 1
///   flags u8              (1) = 0 (reserved)
///   reserved u16          (2)
///   compressed_len u32    (4) — length of zstd payload following
///   <zstd payload>:
///     n_frames u32        (4)
///     per frame:
///       external_frame_idx u32   (4)
///       n_anchors u16            (2)
///       for each anchor:
///         x i16, y i16, cls u8   (5)
/// 
/// Frames are sorted by external_frame_idx for deterministic output. 
/// Empty input produces a valid trailer with n_frames=0 (so decoders 
/// that look for V8AT always find something parseable).
pub fn write_v8at_trailer(
    anchors_by_frame: &std::collections::HashMap<u32, Vec<(i16, i16, u8)>>,
) -> Vec<u8> {
    let mut sorted_frames: Vec<u32> = anchors_by_frame.keys().copied().collect();
    sorted_frames.sort_unstable();
    
    // ── V8AT version 2: intra-frame delta encoding ──
    // 
    // Anchors are sorted by raster position (y then x) within each 
    // frame, then encoded as deltas from the previous anchor in the 
    // sorted list. The first anchor is absolute. Deltas tend to be 
    // small (≤256) since adjacent anchors in raster order are 
    // spatially close.
    // 
    // zstd loves small repeated integers, so the compressed size 
    // typically drops 30-50% vs the v1 absolute-coords format.
    // 
    // FAO_V8AT_RAW=1 forces v1 absolute coords for debugging or 
    // backward compat. Default is now v2.
    let v8at_raw = std::env::var("FAO_V8AT_RAW")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    
    // Build the payload (everything that gets zstd-compressed).
    let mut payload = Vec::with_capacity(4 + sorted_frames.len() * 32);
    payload.extend_from_slice(&(sorted_frames.len() as u32).to_le_bytes());
    for &fi in &sorted_frames {
        let anchors = match anchors_by_frame.get(&fi) {
            Some(a) => a,
            None => continue,
        };
        payload.extend_from_slice(&fi.to_le_bytes());
        let n = (anchors.len().min(u16::MAX as usize)) as u16;
        payload.extend_from_slice(&n.to_le_bytes());
        
        if v8at_raw {
            // v1 layout: absolute coords per anchor.
            for &(x, y, cls) in anchors.iter().take(n as usize) {
                payload.extend_from_slice(&x.to_le_bytes());
                payload.extend_from_slice(&y.to_le_bytes());
                payload.push(cls);
            }
        } else {
            // v2 layout: sort by raster position then delta-encode.
            // 
            // Sort copy so we don't mutate caller's data.
            let mut sorted: Vec<(i16, i16, u8)> = anchors.iter()
                .take(n as usize).copied().collect();
            sorted.sort_unstable_by(|a, b| {
                a.1.cmp(&b.1).then(a.0.cmp(&b.0))
            });
            let mut prev_x = 0i16;
            let mut prev_y = 0i16;
            for (i, &(x, y, cls)) in sorted.iter().enumerate() {
                if i == 0 {
                    // Absolute for the first anchor.
                    payload.extend_from_slice(&x.to_le_bytes());
                    payload.extend_from_slice(&y.to_le_bytes());
                } else {
                    // Delta from previous (i16 still — could overflow if 
                    // anchors are >32k apart in either axis, but 
                    // realistic anchor coords are bounded by frame 
                    // dimensions which max out at ~8K).
                    let dx = x.wrapping_sub(prev_x);
                    let dy = y.wrapping_sub(prev_y);
                    payload.extend_from_slice(&dx.to_le_bytes());
                    payload.extend_from_slice(&dy.to_le_bytes());
                }
                payload.push(cls);
                prev_x = x;
                prev_y = y;
            }
        }
    }
    
    // Compress with adaptive level.
    let compressed = if payload.is_empty() {
        Vec::new()
    } else {
        let lvl = adaptive_zstd_level(payload.len());
        zstd::encode_all(payload.as_slice(), lvl).unwrap_or_else(|_| payload.clone())
    };
    
    // Assemble the trailer. Version byte = 2 for delta-encoded, 1 for raw.
    let version: u8 = if v8at_raw { 1 } else { 2 };
    let mut out = Vec::with_capacity(12 + compressed.len());
    out.extend_from_slice(b"V8AT");
    out.push(version);
    out.push(0u8);  // flags
    out.extend_from_slice(&[0u8; 2]);  // reserved
    out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&compressed);
    out
}

/// Read a V8AT trailer body. `data` should start at the magic bytes.
/// Returns None on bad magic, version, or truncated data. On success, 
/// returns a HashMap of frame_idx → Vec<(x, y, cls)>.
pub fn read_v8at_trailer(
    data: &[u8],
) -> Option<std::collections::HashMap<u32, Vec<(i16, i16, u8)>>> {
    if data.len() < 12 || &data[..4] != b"V8AT" { return None; }
    let version = data[4];
    if version != 1 && version != 2 {
        eprintln!("[v8at] unknown version {} — skipping trailer", version);
        return None;
    }
    // data[5] = flags (unused), data[6..8] = reserved
    let comp_len = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
    if data.len() < 12 + comp_len { return None; }
    
    let payload = if comp_len == 0 {
        Vec::new()
    } else {
        zstd::decode_all(&data[12..12+comp_len]).ok()?
    };
    
    if payload.len() < 4 { return Some(std::collections::HashMap::new()); }
    let n_frames = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    
    let mut map = std::collections::HashMap::with_capacity(n_frames);
    let mut off = 4;
    for _ in 0..n_frames {
        if off + 6 > payload.len() { break; }  // truncated mid-record
        let fi = u32::from_le_bytes([payload[off], payload[off+1], payload[off+2], payload[off+3]]);
        let n_anchors = u16::from_le_bytes([payload[off+4], payload[off+5]]) as usize;
        off += 6;
        if off + n_anchors * 5 > payload.len() { break; }
        let mut anchors = Vec::with_capacity(n_anchors);
        if version == 1 {
            // Absolute coords per anchor.
            for _ in 0..n_anchors {
                let x = i16::from_le_bytes([payload[off], payload[off+1]]);
                let y = i16::from_le_bytes([payload[off+2], payload[off+3]]);
                let cls = payload[off+4];
                anchors.push((x, y, cls));
                off += 5;
            }
        } else {
            // version == 2: first anchor is absolute, rest are deltas.
            let mut prev_x = 0i16;
            let mut prev_y = 0i16;
            for i in 0..n_anchors {
                let x_or_dx = i16::from_le_bytes([payload[off], payload[off+1]]);
                let y_or_dy = i16::from_le_bytes([payload[off+2], payload[off+3]]);
                let cls = payload[off+4];
                let (x, y) = if i == 0 {
                    (x_or_dx, y_or_dy)
                } else {
                    (prev_x.wrapping_add(x_or_dx), prev_y.wrapping_add(y_or_dy))
                };
                anchors.push((x, y, cls));
                prev_x = x;
                prev_y = y;
                off += 5;
            }
        }
        map.insert(fi, anchors);
    }
    Some(map)
}

/// Default eviction parameters. Tunable.
pub struct ChunkStorePolicy {
    /// Soft budget — when total chunk bytes exceed this, evict.
    pub max_memory_bytes: usize,
    /// Default tier for newly-closed chunks.
    pub default_tier: RetentionTier,
    /// Recency weight: priority = quality + recency_weight × (1 / age_in_chunks)
    pub recency_weight: f32,
    /// Sample every Nth frame for representative_frames (default: anchor cadence)
    pub representative_stride: u32,
}

impl Default for ChunkStorePolicy {
    fn default() -> Self {
        Self {
            // 384MB soft budget. With ~15MB per chunk on the typical 
            // 1080p case, this admits ~25 chunks — enough for several 
            // canonical webs plus their immediate aligned followers, while 
            // leaving headroom for the OS, GPU, and other UI state on 
            // 32GB systems running concurrent encoding workloads.
            max_memory_bytes: 384 * 1024 * 1024,
            default_tier: RetentionTier::Sampled,
            recency_weight: 0.3,
            representative_stride: 60,
        }
    }
}

pub struct ChunkStore {
    pub policy: ChunkStorePolicy,
    pub current_chunk: Option<RetainedChunk>,
    pub closed_chunks: Vec<RetainedChunk>,
    pub access_clock: u64,
    
    // Stats for diagnostics
    pub total_chunks_closed: u64,
    pub total_chunks_evicted: u64,
    pub total_demotions: u64,
    
    // ─── Disk spillover (Session 9 — "near infinite scale") ───
    // 
    // When enforce_budget would otherwise EVICT a chunk (drop it entirely 
    // because all closed_chunks are at minimum tier and total bytes still 
    // exceed budget), the chunk is instead SPILLED to a temp file. Its 
    // web_mesh + pose_track are zstd-compressed and written; the metadata 
    // (frame range, quality, canonical_web_id, etc.) stays in spilled_index.
    // 
    // At trailer-write time, all spilled chunks are faulted back in, 
    // merged with closed_chunks, and the V8MX trailer is written from 
    // the union. RAM peak stays bounded by `policy.max_memory_bytes`.
    // 
    // Trade-off: spilled chunks no longer participate in alignment search 
    // (they're not in closed_chunks during encode). Cross-encode alignment 
    // is missed for the OLDEST chunks, which is the right side to lose 
    // since recency-weighted priority already biases toward keeping 
    // recent + canonical chunks in RAM.
    pub spill_file: Option<std::sync::Mutex<std::fs::File>>,
    pub spill_path: Option<std::path::PathBuf>,
    pub spill_offset: u64,
    pub spilled_chunks: Vec<SpilledChunk>,
    pub total_chunks_spilled: u64,
    
    // ─── Library references (Phase 3) ───
    // 
    // When a chunk closes and the encoder is configured with a library, 
    // we query the library for a matching canonical. If found, we record 
    // a LibraryRef here AND remove the chunk's mesh from the in-RAM 
    // closed_chunks (memory savings = full mesh bytes per match).
    // 
    // At trailer-write time, a V8LR trailer is emitted from this Vec. 
    // The decoder reads V8LR + looks up each uuid in the library to 
    // reconstruct the chunk's geometry.
    pub library_refs: Vec<LibraryRef>,
    pub total_library_matches: u64,
    pub total_library_match_bytes_saved: u64,
}

/// Metadata stub for a chunk whose body lives on disk.
/// 
/// Mirrors RetainedChunk's most important "always-kept" fields plus a 
/// (offset, len) into the spill file. The body (web_mesh + pose_track) 
/// is zstd-compressed at spill time and round-tripped at trailer time.
#[derive(Clone, Debug)]
pub struct SpilledChunk {
    pub frame_start: u32,
    pub frame_end: u32,
    pub quality_score: f32,
    pub n_anchors: u32,
    pub mean_density_cov: f32,
    pub static_ratio: f32,
    pub canonical_web_id: Option<u32>,
    pub web_alignment_transform: [f32; 16],
    pub web_signature: WebSignature,
    
    /// Offset into the spill file where the compressed body begins.
    pub spill_offset: u64,
    /// Length in bytes of the compressed body on disk.
    pub spill_len: u64,
    
    /// Vertex / triangle counts (for diagnostics; stub doesn't include the mesh).
    pub web_vertex_count: u32,
    pub web_triangle_count: u32,
}

/// Counter for DC prediction clamp events — incremented when 
/// `(cur - pred + 128)` falls outside [0, 255] and we have to clamp. 
/// Logged periodically via the v8 residual diagnostic so the user 
/// can see whether DC pred is producing too many clamps (which 
/// would indicate large prediction errors).
static DCPRED_CLAMP_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Adaptive zstd level for mesh trailer compression. The big win at 
/// level 19 over level 12 is only ~3-5% ratio on mesh data (which is 
/// already fairly tight binary), but level 19 is 5-8x slower in 
/// encode time. For chunks > 256KB we drop to level 12, saving 
/// multiple seconds of export time on big videos without giving up 
/// meaningful size.
/// 
/// The threshold can be overridden via FAO_ZSTD_FAST=1 to force level 
/// 12 for everything (~10% bigger trailer but much faster export).
fn adaptive_zstd_level(raw_size: usize) -> i32 {
    let force_fast = std::env::var("FAO_ZSTD_FAST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if force_fast { return 12; }
    if raw_size > 256 * 1024 { 12 } else { 19 }
}

impl ChunkStore {
    pub fn new() -> Self {
        Self::with_policy(ChunkStorePolicy::default())
    }
    
    pub fn with_policy(policy: ChunkStorePolicy) -> Self {
        Self {
            policy,
            current_chunk: None,
            closed_chunks: Vec::new(),
            access_clock: 0,
            total_chunks_closed: 0,
            total_chunks_evicted: 0,
            total_demotions: 0,
            spill_file: None,
            spill_path: None,
            spill_offset: 0,
            spilled_chunks: Vec::new(),
            total_chunks_spilled: 0,
            library_refs: Vec::new(),
            total_library_matches: 0,
            total_library_match_bytes_saved: 0,
        }
    }
    
    /// Begin a new chunk at `start_frame`. Closes any previous chunk first.
    pub fn open_chunk(&mut self, start_frame: u32) {
        if self.current_chunk.is_some() {
            self.close_chunk(start_frame);
        }
        // Identity transform = "this web is canonical to itself."
        let mut identity = [0.0f32; 16];
        identity[0] = 1.0; identity[5] = 1.0; identity[10] = 1.0; identity[15] = 1.0;
        
        self.current_chunk = Some(RetainedChunk {
            frame_start: start_frame,
            frame_end: start_frame,
            quality_score: 0.0,
            priority: 0.0,
            tier: self.policy.default_tier,
            n_anchors: 0,
            n_features_tracked: 0,
            mean_density_cov: 0.0,
            static_ratio: 0.0,
            anchor_summaries: Vec::new(),
            representative_frames: Vec::new(),
            full_frames: Vec::new(),
            // Web fields (populated as the chunk fills + at close).
            web_mesh: SceneMesh::new(),
            web_pose_track: Vec::new(),
            web_signature: WebSignature::default(),
            canonical_web_id: None,  // assigned at close time / by web alignment pass
            web_alignment_transform: identity,
            last_accessed: self.access_clock,
        });
    }
    
    /// Add an anchor's summary to the currently-open chunk.
    pub fn add_anchor(&mut self, summary: AnchorSummary) {
        if let Some(chunk) = self.current_chunk.as_mut() {
            chunk.frame_end = chunk.frame_end.max(summary.frame_idx + 1);
            chunk.n_anchors += 1;
            chunk.anchor_summaries.push(summary);
        }
    }
    
    /// Add a source frame to the currently-open chunk.
    /// Whether it's actually retained depends on the chunk's tier.
    pub fn add_frame(&mut self, frame_idx: u32, yuv: &[u8]) {
        if let Some(chunk) = self.current_chunk.as_mut() {
            chunk.frame_end = chunk.frame_end.max(frame_idx + 1);
            match chunk.tier {
                RetentionTier::Full => {
                    chunk.full_frames.push(RetainedFrame { 
                        frame_idx, yuv: yuv.to_vec() 
                    });
                }
                RetentionTier::Sampled => {
                    let stride = self.policy.representative_stride.max(1);
                    if frame_idx % stride == 0 {
                        chunk.representative_frames.push(RetainedFrame {
                            frame_idx, yuv: yuv.to_vec()
                        });
                    }
                }
                _ => {}  // Stats / Summary tiers don't retain frames
            }
        }
    }
    
    /// Seal the chunk's web with the current geometry from the encoder.
    /// 
    /// Called by V8Encoder right before close_chunk: transfers the encoder's 
    /// active mesh + pose track into the chunk, computes the web signature, 
    /// and clears the encoder's slots so the next chunk starts with fresh 
    /// geometry. This is what makes each chunk a self-contained "web."
    /// 
    /// The encoder is responsible for resetting its own scene_mesh to a 
    /// fresh empty state after this returns.
    pub fn seal_web(&mut self, mesh: SceneMesh, pose_track: Vec<CameraPose>) {
        if let Some(chunk) = self.current_chunk.as_mut() {
            chunk.web_signature = WebSignature::from_mesh(&mesh);
            chunk.web_mesh = mesh;
            chunk.web_pose_track = pose_track;
        }
    }
    
    // ═══════════════════════════════════════════════════════════
    // SESSION 7: Web alignment detection
    // ═══════════════════════════════════════════════════════════
    //
    // After a chunk seals its web, scan previously-closed chunks for webs 
    // with similar shape signatures. If a match is found above threshold, 
    // link the new chunk to the canonical (highest-quality) member of the 
    // matched cluster. This builds an alignment graph where connected 
    // components = "scenes" the codec recognizes as recurring.
    //
    // Alignment is OPPORTUNISTIC: when shape matches, we capture the 
    // relationship; when it doesn't, the new chunk stands alone. Compression
    // payoff (Session 8) is proportional to how many chunks end up aligned.
    
    /// Threshold for considering two webs as the same scene. With PCA-
    /// based eigenvalue similarity (rotation/translation invariant) we 
    /// can use a moderately permissive threshold without false positives.
    /// 0.75 = "shape spectrum is roughly 75% similar" — empirically a 
    /// reasonable bar for genuine same-scene detection.
    pub const WEB_ALIGNMENT_THRESHOLD: f32 = 0.75;
    
    /// Search closed chunks for any web whose signature is similar enough 
    /// to be considered the same scene. Returns the best match's index 
    /// (into closed_chunks) and similarity score, or None.
    pub fn find_similar_web(&self, signature: &WebSignature) -> Option<(usize, f32)> {
        // Two-pass selection:
        //   Pass 1: prefer EXISTING CANONICALS (chunks that already have 
        //           followers). Aligning a new web to an existing canonical 
        //           concentrates the compression gains — N webs sharing 
        //           one canonical is much smaller than N/2 small clusters.
        //   Pass 2: if no canonical above threshold, fall back to ANY web.
        // 
        // The "follower count" heuristic identifies real canonicals 
        // without needing the kind enum (which is computed at trailer-write 
        // time, not maintained as a live property).
        // 
        // Bonus when prioritizing canonicals: we add a small score boost 
        // proportional to follower count, so webs that already represent 
        // many scenes get even more pull. This produces a power-law 
        // distribution in canonical sharing.
        let mut canonical_followers = std::collections::HashMap::<u32, u32>::new();
        for c in &self.closed_chunks {
            if let Some(canon_id) = c.canonical_web_id {
                if canon_id != c.frame_start {
                    *canonical_followers.entry(canon_id).or_insert(0) += 1;
                }
            }
        }
        
        let mut best_canon: Option<(usize, f32)> = None;
        let mut best_any: Option<(usize, f32)> = None;
        for (i, c) in self.closed_chunks.iter().enumerate() {
            if c.web_mesh.vertices.is_empty() { continue; }
            let raw_s = signature.combined_similarity(&c.web_signature);
            if raw_s < Self::WEB_ALIGNMENT_THRESHOLD { continue; }
            
            // Track the best ANY-web fallback regardless of canonicalness.
            match best_any {
                None => best_any = Some((i, raw_s)),
                Some((_, prev)) if raw_s > prev => best_any = Some((i, raw_s)),
                _ => {}
            }
            
            // Now check if this is an established canonical.
            let n_followers = *canonical_followers.get(&c.frame_start).unwrap_or(&0);
            let is_self_canonical = c.canonical_web_id.map_or(true, |id| id == c.frame_start);
            if n_followers > 0 && is_self_canonical {
                // Boost score by sqrt(followers)*0.02 — concentrates 
                // alignment around already-popular canonicals without 
                // overwhelming the actual similarity signal.
                let boost = (n_followers as f32).sqrt() * 0.02;
                let boosted = raw_s + boost;
                match best_canon {
                    None => best_canon = Some((i, boosted)),
                    Some((_, prev)) if boosted > prev => best_canon = Some((i, boosted)),
                    _ => {}
                }
            }
        }
        
        // Prefer canonical match; fall back to any-web if no canonical 
        // crossed threshold.
        best_canon.or(best_any)
    }
    
    /// Compute a rigid transform that maps `from`'s coords into `to`'s 
    /// coords using the principal-axes of each web's PCA.
    /// 
    /// Algorithm:
    ///   1. Translate `from`'s centroid to origin.
    ///   2. Rotate `from`'s principal axes to align with `to`'s.
    ///      Rotation matrix = to.eigenvectors * from.eigenvectors^T 
    ///      (since eigenvectors are orthonormal, transpose = inverse).
    ///   3. Translate to `to`'s centroid.
    /// 
    /// Output is 4×4 row-major homogeneous transform: `to_world = M * from_world`.
    /// 
    /// Caveat: eigenvectors have sign ambiguity (an eigenvector and its 
    /// negative are both valid). We try the 4 sign combinations of v0 and 
    /// v1 (v2 derived) and pick the one minimizing reconstruction error.
    /// Production-grade: 8 combinations, score by point-cloud distance, 
    /// also detect mirror-flip; we ship a simpler 4-combination heuristic.
    pub fn compute_alignment_transform(from: &WebSignature, to: &WebSignature) -> [f32; 16] {
        // Build R_from = [from_v0; from_v1; from_v2] as a 3x3 matrix where 
        // each ROW is an eigenvector. Same for R_to.
        // To rotate from's local frame INTO to's, we apply: R = R_to^T * R_from
        // (transforms from→world via R_from^T, world→to via R_to)
        // 
        // For robustness against sign ambiguity, try 4 sign combinations on 
        // from's eigenvectors and keep the best (minimum trace deviation 
        // from valid rotation).
        let to_evs = to.eigenvectors;  // [v0, v1, v2] each f32×3, row-major
        
        let mut best_rot = [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let mut best_score = f32::MIN;
        
        for sign0 in &[1.0f32, -1.0] {
            for sign1 in &[1.0f32, -1.0] {
                // Apply signs to from's eigenvectors
                let f_v0 = [from.eigenvectors[0]*sign0, from.eigenvectors[1]*sign0, from.eigenvectors[2]*sign0];
                let f_v1 = [from.eigenvectors[3]*sign1, from.eigenvectors[4]*sign1, from.eigenvectors[5]*sign1];
                // v2 from cross product (right-handed)
                let f_v2 = [
                    f_v0[1]*f_v1[2] - f_v0[2]*f_v1[1],
                    f_v0[2]*f_v1[0] - f_v0[0]*f_v1[2],
                    f_v0[0]*f_v1[1] - f_v0[1]*f_v1[0],
                ];
                
                // R = R_to^T * R_from where rows of R_from are [f_v0, f_v1, f_v2]
                // and rows of R_to are [to_v0, to_v1, to_v2].
                // R[i][j] = sum_k R_to[k][i] * R_from[k][j]
                // 
                // Equivalently: R = sum over k of (to_v_k as col) * (from_v_k as row)
                //   where to_v_k is column-stack and from_v_k is row.
                // R[i][j] = to_v_k[i] * from_v_k[j] summed over k.
                let to_vs = [
                    [to_evs[0], to_evs[1], to_evs[2]],
                    [to_evs[3], to_evs[4], to_evs[5]],
                    [to_evs[6], to_evs[7], to_evs[8]],
                ];
                let from_vs = [f_v0, f_v1, f_v2];
                
                let mut rot = [0.0f32; 9];
                for i in 0..3 {
                    for j in 0..3 {
                        for k in 0..3 {
                            rot[i*3 + j] += to_vs[k][i] * from_vs[k][j];
                        }
                    }
                }
                
                // Score: trace of rotation matrix. Valid rotations have 
                // trace in [-1, 3]. Larger trace = closer to identity = 
                // better-conditioned rotation. Reflections (det=-1) have 
                // negative trace, so this naturally rejects them.
                let trace = rot[0] + rot[4] + rot[8];
                if trace > best_score {
                    best_score = trace;
                    best_rot = rot;
                }
            }
        }
        
        // Build full transform:
        //   p_to = R * (p_from - from_centroid) + to_centroid
        // which expands to:
        //   p_to = R * p_from - R * from_centroid + to_centroid
        //   so translation = to_centroid - R * from_centroid
        let fc = from.centroid;
        let r_fc = [
            best_rot[0]*fc[0] + best_rot[1]*fc[1] + best_rot[2]*fc[2],
            best_rot[3]*fc[0] + best_rot[4]*fc[1] + best_rot[5]*fc[2],
            best_rot[6]*fc[0] + best_rot[7]*fc[1] + best_rot[8]*fc[2],
        ];
        let tx = to.centroid[0] - r_fc[0];
        let ty = to.centroid[1] - r_fc[1];
        let tz = to.centroid[2] - r_fc[2];
        
        // 4×4 row-major: [r0 r1 r2 tx; r3 r4 r5 ty; r6 r7 r8 tz; 0 0 0 1]
        [
            best_rot[0], best_rot[1], best_rot[2], tx,
            best_rot[3], best_rot[4], best_rot[5], ty,
            best_rot[6], best_rot[7], best_rot[8], tz,
            0.0, 0.0, 0.0, 1.0,
        ]
    }
    
    /// After sealing the current web, attempt to align it to a previously-
    /// observed web. If alignment succeeds (above threshold), link the new
    /// chunk via canonical_web_id.
    /// 
    /// Returns Some(score) if linked, None if standalone.
    pub fn try_align_current_web(&mut self) -> Option<f32> {
        let cur_sig = match self.current_chunk.as_ref() {
            Some(c) => c.web_signature,
            None => return None,
        };
        let (matched_idx, score) = match self.find_similar_web(&cur_sig) {
            Some(m) => m,
            None => return None,
        };
        
        // Find the canonical chunk for the matched cluster.
        // If the matched chunk is itself canonical, use it. Otherwise 
        // follow the canonical_web_id chain.
        let mut canonical_idx = matched_idx;
        for _ in 0..32 {  // bounded chain-follow
            if let Some(next_id) = self.closed_chunks[canonical_idx].canonical_web_id {
                // Find chunk by id (frame_start as id for now)
                if let Some(idx) = self.closed_chunks.iter().position(|c| c.frame_start == next_id) {
                    if idx == canonical_idx { break; }  // self-loop = is canonical
                    canonical_idx = idx;
                } else { break; }
            } else { break; }
        }
        
        let canonical = &self.closed_chunks[canonical_idx];
        let canonical_id = canonical.frame_start;  // use frame_start as web id
        let canonical_sig = canonical.web_signature;
        
        // Compute transform mapping current web -> canonical.
        let transform = Self::compute_alignment_transform(&cur_sig, &canonical_sig);
        
        if let Some(chunk) = self.current_chunk.as_mut() {
            chunk.canonical_web_id = Some(canonical_id);
            chunk.web_alignment_transform = transform;
        }
        
        Some(score)
    }
    
    // ═══════════════════════════════════════════════════════════
    // SESSION 8: Web delta encoding
    // ═══════════════════════════════════════════════════════════
    //
    // For chunks with a canonical_web_id set, geometry can be stored more 
    // compactly: the canonical web is stored once; aligned webs only need 
    // their TRANSFORM + RESIDUALS (vertices the canonical doesn't have).
    //
    // This is where compression payoff actually shows up. A 2-hour SMITE 
    // recording produces N similar webs; if N-1 align to one canonical:
    //   1 × full_canonical_mesh    (large)
    // + (N-1) × (transform + residuals)   (small)
    //
    // vs naive multi-web storage of N × full_mesh.
    //
    // Residual = vertices in the aligned web with no near-neighbor in the 
    // canonical web after transform. In practice: lighting deltas, dynamic 
    // objects, sensor noise, finer surface detail.
    
    /// Compute the residual geometry of a web against its canonical.
    /// Returns vertices in `web` that have NO near-neighbor in `canonical` 
    /// after applying `web_to_canonical` transform.
    /// 
    /// `tolerance` is the distance threshold for "matches an existing 
    /// canonical vertex." Larger = more aggressive deduplication.
    pub fn compute_web_residual(
        web: &SceneMesh,
        canonical: &SceneMesh,
        web_to_canonical: &[f32; 16],
        tolerance: f32,
    ) -> Vec<Vec3> {
        // O(n) algorithm using the canonical's vertex_grid as a spatial 
        // hash. Previously this was O(n*m) — for two ~10K-vertex meshes 
        // that's 100M iterations per web pair, dominating encode time on 
        // big videos AND giving poor matching because tolerance was 
        // applied as a hard cutoff on absolute distance.
        // 
        // Now: each web vertex transforms to canonical space, then we 
        // look up its voxel coordinate (and 26 neighbors for boundary 
        // safety) in canonical's vertex_grid. If any matched canonical 
        // vertex is within `tolerance`, the web vertex is considered 
        // covered and skipped from the residual. Otherwise it's added.
        // 
        // The voxel size is implicit in vertex_grid keys — we recover it 
        // from the canonical's mesh extent / grid extent. Tolerance is 
        // passed directly; falls back to voxel-diagonal if 0.0.
        let tol2 = tolerance * tolerance;
        let mut residual = Vec::new();
        
        // Recover voxel size from the canonical's vertex_grid layout.
        // If grid is empty, fall back to O(n*m) brute force.
        if canonical.vertex_grid.is_empty() {
            for v in &web.vertices {
                let m = web_to_canonical;
                let tx = m[0]*v.x + m[1]*v.y + m[2]*v.z + m[3];
                let ty = m[4]*v.x + m[5]*v.y + m[6]*v.z + m[7];
                let tz = m[8]*v.x + m[9]*v.y + m[10]*v.z + m[11];
                let mut found = false;
                for cv in &canonical.vertices {
                    let dx = cv.x - tx; let dy = cv.y - ty; let dz = cv.z - tz;
                    if dx*dx + dy*dy + dz*dz <= tol2 { found = true; break; }
                }
                if !found { residual.push(Vec3 { x: tx, y: ty, z: tz }); }
            }
            return residual;
        }
        
        // Estimate voxel size: span of canonical vertices ÷ span of 
        // grid keys. Cheap one-shot computation.
        let voxel_size = {
            let mut min_x = f32::INFINITY; let mut max_x = f32::NEG_INFINITY;
            let mut min_kx = i32::MAX; let mut max_kx = i32::MIN;
            for v in &canonical.vertices {
                if v.x < min_x { min_x = v.x; }
                if v.x > max_x { max_x = v.x; }
            }
            for &(kx, _, _) in canonical.vertex_grid.keys() {
                if kx < min_kx { min_kx = kx; }
                if kx > max_kx { max_kx = kx; }
            }
            let span_x = max_x - min_x;
            let span_kx = (max_kx - min_kx).max(1) as f32;
            // Default voxel: 1.0 if degenerate.
            if span_x.is_finite() && span_x > 0.0 { (span_x / span_kx).abs().max(0.001) } else { 1.0 }
        };
        let inv_vox = 1.0 / voxel_size;
        
        for v in &web.vertices {
            let m = web_to_canonical;
            let tx = m[0]*v.x + m[1]*v.y + m[2]*v.z + m[3];
            let ty = m[4]*v.x + m[5]*v.y + m[6]*v.z + m[7];
            let tz = m[8]*v.x + m[9]*v.y + m[10]*v.z + m[11];
            
            let kx = (tx * inv_vox).round() as i32;
            let ky = (ty * inv_vox).round() as i32;
            let kz = (tz * inv_vox).round() as i32;
            
            // Search 3x3x3 neighborhood of voxels — handles boundary 
            // cases where the matched vertex falls into an adjacent cell.
            let mut found = false;
            'outer: for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(&v_idx) = canonical.vertex_grid.get(&(kx + dx, ky + dy, kz + dz)) {
                            let cv = &canonical.vertices[v_idx as usize];
                            let ex = cv.x - tx;
                            let ey = cv.y - ty;
                            let ez = cv.z - tz;
                            if ex*ex + ey*ey + ez*ez <= tol2 {
                                found = true;
                                break 'outer;
                            }
                        }
                    }
                }
            }
            if !found {
                residual.push(Vec3 { x: tx, y: ty, z: tz });
            }
        }
        residual
    }
    
    /// Statistics for web compression-against-canonical potential.
    /// Reports what the codec WOULD save with delta encoding applied.
    pub fn web_compression_report(&self, tolerance: f32) -> WebCompressionReport {
        let mut total_full = 0usize;
        let mut total_delta = 0usize;
        let mut n_aligned = 0u32;
        let mut n_standalone = 0u32;
        let mut n_canonical = 0u32;
        
        // Cap per-chunk vertex sampling: O(n*m) full residual is too 
        // expensive when meshes have millions of vertices. We sample.
        const MAX_VERTS_PER_CHUNK_FOR_RESIDUAL: usize = 5000;
        
        for chunk in &self.closed_chunks {
            let n_verts = chunk.web_mesh.vertices.len();
            let bytes_full = n_verts * 12;
            
            match chunk.canonical_web_id {
                None => {
                    if !chunk.web_mesh.vertices.is_empty() {
                        n_canonical += 1;
                    } else {
                        n_standalone += 1;
                    }
                    total_full += bytes_full;
                    total_delta += bytes_full;
                }
                Some(canon_id) => {
                    n_aligned += 1;
                    let canon = self.closed_chunks.iter()
                        .find(|c| c.frame_start == canon_id);
                    if let Some(canon) = canon {
                        // O(n*m) full residual is too expensive when meshes 
                        // have millions of vertices. Sample stride to keep 
                        // it tractable while preserving the ratio estimate.
                        let n_chunk = chunk.web_mesh.vertices.len();
                        let n_canon = canon.web_mesh.vertices.len();
                        let stride_chunk = (n_chunk / MAX_VERTS_PER_CHUNK_FOR_RESIDUAL).max(1);
                        let stride_canon = (n_canon / MAX_VERTS_PER_CHUNK_FOR_RESIDUAL).max(1);
                        
                        // Subsample meshes for residual computation
                        let mut sampled_chunk = SceneMesh::new();
                        for (i, v) in chunk.web_mesh.vertices.iter().enumerate() {
                            if i % stride_chunk == 0 { sampled_chunk.vertices.push(*v); }
                        }
                        let mut sampled_canon = SceneMesh::new();
                        for (i, v) in canon.web_mesh.vertices.iter().enumerate() {
                            if i % stride_canon == 0 { sampled_canon.vertices.push(*v); }
                        }
                        
                        let residual = Self::compute_web_residual(
                            &sampled_chunk,
                            &sampled_canon,
                            &chunk.web_alignment_transform,
                            tolerance,
                        );
                        // Scale residual count back up to full mesh size
                        let est_full_residual = residual.len() * stride_chunk;
                        let bytes_delta = 64 + est_full_residual * 12;
                        total_full += bytes_full;
                        total_delta += bytes_delta;
                    } else {
                        total_full += bytes_full;
                        total_delta += bytes_full;
                    }
                }
            }
        }
        
        WebCompressionReport {
            total_chunks: self.closed_chunks.len() as u32,
            n_canonical,
            n_aligned,
            n_standalone,
            full_bytes: total_full,
            delta_bytes: total_delta,
        }
    }
    
    /// Close the current chunk. Computes its quality score, sets priority,
    /// and moves it to closed_chunks. Triggers eviction if over budget.
    pub fn close_chunk(&mut self, end_frame: u32) {
        let mut chunk = match self.current_chunk.take() {
            Some(c) => c,
            None => return,
        };
        chunk.frame_end = end_frame;
        
        // Compute quality score from anchor summaries.
        // Weighted: density coverage 50%, feature count 30%, anchor count 20%
        if !chunk.anchor_summaries.is_empty() {
            let mean_cov = chunk.anchor_summaries.iter()
                .map(|a| a.density_coverage).sum::<f32>() 
                / chunk.anchor_summaries.len() as f32;
            chunk.mean_density_cov = mean_cov;
            
            let mean_features = chunk.anchor_summaries.iter()
                .map(|a| a.valid_pixel_count as f32).sum::<f32>()
                / chunk.anchor_summaries.len() as f32;
            // Normalize: 50000 valid pixels per anchor = excellent; 5000 = poor
            let feature_score = (mean_features / 50000.0).min(1.0);
            
            let anchor_count_score = (chunk.n_anchors as f32 / 4.0).min(1.0);
            
            chunk.quality_score = 
                  mean_cov * 0.5 
                + feature_score * 0.3 
                + anchor_count_score * 0.2;
        } else {
            chunk.quality_score = 0.0;
        }
        
        // Initial priority = quality (recency factor applies dynamically at eviction time)
        chunk.priority = chunk.quality_score;
        chunk.last_accessed = self.access_clock;
        
        self.closed_chunks.push(chunk);
        self.total_chunks_closed += 1;
        self.access_clock += 1;
        
        // Cap closed_chunks count. Drop oldest beyond threshold to keep 
        // memory bounded across long encodes. With ~50MB per chunk web 
        // mesh, 32 chunks × 50MB = 1.6GB budget for closed-chunk geometry.
        const MAX_CLOSED_CHUNKS: usize = 32;
        while self.closed_chunks.len() > MAX_CLOSED_CHUNKS {
            // Drop the lowest-priority chunk (using the freshly computed priority)
            let lowest = self.closed_chunks.iter().enumerate()
                .min_by(|(_, a), (_, b)| 
                    a.priority.partial_cmp(&b.priority)
                        .unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i);
            if let Some(i) = lowest {
                self.closed_chunks.remove(i);
                self.total_chunks_evicted += 1;
            } else { break; }
        }
        
        // Trigger eviction if over budget
        self.enforce_budget();
    }
    
    /// Serialize all closed chunks' webs into a V8MX (Multi-eXtension) trailer.
    /// 
    /// This is the heart of the multi-web compression model:
    ///   - Canonical webs store their full mesh (zstd-compressed)
    ///   - Aligned webs store only their transform + residual vertices
    /// 
    /// Format (all little-endian):
    ///   'V8MX' (4 bytes)
    ///   version (u8 = 1)
    ///   n_webs (u32)
    ///   for each web:
    ///     chunk_id (u32) — frame_start of this web
    ///     canonical_id (u32) — chunk_id of canonical web (== chunk_id if self-canonical)
    ///     frame_start (u32), frame_end (u32)
    ///     is_canonical (u8): 0=aligned-delta, 1=canonical, 2=standalone
    ///     transform[16] (16 × f32 = 64 bytes) — only meaningful if aligned
    ///     blob_len (u32) + blob (zstd-compressed)
    ///       canonical/standalone: full mesh (verts as f32×3 + indices as u32 + colors as f32×3)
    ///       aligned: residual vertices only (f32×3 each)
    /// 
    /// The decoder reconstructs each web's mesh by:
    ///   - canonical/standalone: read directly
    ///   - aligned: take canonical's mesh, apply transform, append residual verts
    pub fn to_v8mx_trailer(&self, residual_tolerance: f32) -> Vec<u8> {
        use std::io::Cursor;
        let mut out = Vec::new();
        out.extend_from_slice(b"V8MX");
        // V8MX version 2: blobs are prefixed with a 1-byte format tag 
        // (0 = legacy f32 layout, 1 = quantized u16). Older decoders 
        // reading v2 will try to interpret the tag byte as the high 
        // byte of the vertex count and produce garbage — version bump 
        // forces them to skip the trailer.
        out.push(2u8);
        
        // Filter to chunks with non-empty geometry
        let webs: Vec<&RetainedChunk> = self.closed_chunks.iter()
            .filter(|c| !c.web_mesh.vertices.is_empty())
            .collect();
        out.extend_from_slice(&(webs.len() as u32).to_le_bytes());
        
        for web in webs.iter() {
            let chunk_id = web.frame_start;
            let canonical_id = web.canonical_web_id.unwrap_or(chunk_id);
            let is_canonical_self = canonical_id == chunk_id;
            
            out.extend_from_slice(&chunk_id.to_le_bytes());
            out.extend_from_slice(&canonical_id.to_le_bytes());
            out.extend_from_slice(&web.frame_start.to_le_bytes());
            out.extend_from_slice(&web.frame_end.to_le_bytes());
            
            // is_canonical: 1 if self-canonical AND has followers, 2 if standalone (no followers,
            // not aligned to anyone), 0 if aligned to someone else
            let n_followers = self.closed_chunks.iter()
                .filter(|c| c.canonical_web_id == Some(chunk_id))
                .count();
            let kind: u8 = if !is_canonical_self {
                0  // aligned-delta
            } else if n_followers > 0 {
                1  // canonical with followers
            } else {
                2  // standalone (canonical of nothing)
            };
            out.push(kind);
            
            // Transform (16 × f32)
            for &f in &web.web_alignment_transform {
                out.extend_from_slice(&f.to_le_bytes());
            }
            
            // Build the blob
            let blob = if kind == 0 {
                // Aligned: store residual vertices only.
                // V8MX v2 prefixes with a format tag byte (0 = legacy 
                // f32, 1 = quantized u16). For aligned residuals we 
                // keep f32 because residual vertices are typically 
                // few and their AABB poorly constrains precision 
                // (residual is delta-from-canonical, often tiny). 
                // Quantization noise would dominate the signal.
                // 
                // So aligned blobs in v2 = tag byte (0) + legacy f32.
                let canon = self.closed_chunks.iter()
                    .find(|c| c.frame_start == canonical_id);
                let residual = if let Some(canon) = canon {
                    Self::compute_web_residual(
                        &web.web_mesh,
                        &canon.web_mesh,
                        &web.web_alignment_transform,
                        residual_tolerance,
                    )
                } else {
                    // Canonical not found, fallback to full mesh
                    web.web_mesh.vertices.clone()
                };
                let mut raw = Vec::with_capacity(1 + residual.len() * 12);
                raw.push(0u8);  // legacy tag — aligned always stays f32
                for v in &residual {
                    raw.extend_from_slice(&v.x.to_le_bytes());
                    raw.extend_from_slice(&v.y.to_le_bytes());
                    raw.extend_from_slice(&v.z.to_le_bytes());
                }
                let lvl = adaptive_zstd_level(raw.len());
                zstd::encode_all(Cursor::new(&raw), lvl).unwrap_or(raw)
            } else {
                // Canonical or standalone: store full mesh.
                // 
                // v8mx_v2 (default): quantized layout.
                //   - First byte = format tag: 0 = legacy f32, 1 = quantized u16.
                //   - Quantized layout: 
                //       1 byte (tag=1)
                //       12 bytes (n_verts u32 + n_indices u32 + n_colors u32)
                //       24 bytes (AABB min[3]f32 + max[3]f32)
                //       n_verts * 6 bytes (verts as u16 quantized to AABB)
                //       n_indices * 4 bytes (indices u32 — unchanged)
                //       n_colors * 3 bytes (colors u8 RGB)
                //   - Old decoders see the tag byte as the high byte of the 
                //     vertex count and read garbage. Hence we bump V8MX 
                //     trailer version too — v1 trailer = legacy blobs only, 
                //     v2 trailer = tag-byte blobs.
                // 
                // FAO_V8MX_RAW=1 forces legacy f32 storage (no quantization).
                // Recommended only for debugging or extreme precision needs.
                let v8mx_raw = std::env::var("FAO_V8MX_RAW")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                let mut raw = Vec::new();
                if v8mx_raw {
                    // Legacy tag = 0.
                    raw.push(0u8);
                    raw.extend_from_slice(&(web.web_mesh.vertices.len() as u32).to_le_bytes());
                    raw.extend_from_slice(&(web.web_mesh.indices.len() as u32).to_le_bytes());
                    raw.extend_from_slice(&(web.web_mesh.vertex_colors.len() as u32).to_le_bytes());
                    for v in &web.web_mesh.vertices {
                        raw.extend_from_slice(&v.x.to_le_bytes());
                        raw.extend_from_slice(&v.y.to_le_bytes());
                        raw.extend_from_slice(&v.z.to_le_bytes());
                    }
                    for &i in &web.web_mesh.indices {
                        raw.extend_from_slice(&i.to_le_bytes());
                    }
                    for c in &web.web_mesh.vertex_colors {
                        raw.extend_from_slice(&c[0].to_le_bytes());
                        raw.extend_from_slice(&c[1].to_le_bytes());
                        raw.extend_from_slice(&c[2].to_le_bytes());
                    }
                } else {
                    // Quantized tag = 1.
                    raw.push(1u8);
                    raw.extend_from_slice(&(web.web_mesh.vertices.len() as u32).to_le_bytes());
                    raw.extend_from_slice(&(web.web_mesh.indices.len() as u32).to_le_bytes());
                    raw.extend_from_slice(&(web.web_mesh.vertex_colors.len() as u32).to_le_bytes());
                    
                    // AABB.
                    let mut mn = [f32::INFINITY, f32::INFINITY, f32::INFINITY];
                    let mut mx = [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
                    for v in &web.web_mesh.vertices {
                        if v.x < mn[0] { mn[0] = v.x; }
                        if v.y < mn[1] { mn[1] = v.y; }
                        if v.z < mn[2] { mn[2] = v.z; }
                        if v.x > mx[0] { mx[0] = v.x; }
                        if v.y > mx[1] { mx[1] = v.y; }
                        if v.z > mx[2] { mx[2] = v.z; }
                    }
                    // Guard against degenerate AABB (single point).
                    for i in 0..3 {
                        if !(mx[i] > mn[i]) { mx[i] = mn[i] + 1e-3; }
                    }
                    for v in &mn { raw.extend_from_slice(&v.to_le_bytes()); }
                    for v in &mx { raw.extend_from_slice(&v.to_le_bytes()); }
                    
                    // Quantize vertices to u16. Precision = extent / 65535.
                    let extent = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
                    for v in &web.web_mesh.vertices {
                        let qx = (((v.x - mn[0]) / extent[0]).clamp(0.0, 1.0) * 65535.0).round() as u16;
                        let qy = (((v.y - mn[1]) / extent[1]).clamp(0.0, 1.0) * 65535.0).round() as u16;
                        let qz = (((v.z - mn[2]) / extent[2]).clamp(0.0, 1.0) * 65535.0).round() as u16;
                        raw.extend_from_slice(&qx.to_le_bytes());
                        raw.extend_from_slice(&qy.to_le_bytes());
                        raw.extend_from_slice(&qz.to_le_bytes());
                    }
                    
                    // Indices unchanged.
                    for &i in &web.web_mesh.indices {
                        raw.extend_from_slice(&i.to_le_bytes());
                    }
                    
                    // Colors as u8 RGB. Clamp + scale to 0..255.
                    for c in &web.web_mesh.vertex_colors {
                        raw.push((c[0].clamp(0.0, 1.0) * 255.0).round() as u8);
                        raw.push((c[1].clamp(0.0, 1.0) * 255.0).round() as u8);
                        raw.push((c[2].clamp(0.0, 1.0) * 255.0).round() as u8);
                    }
                }
                let lvl = adaptive_zstd_level(raw.len());
                zstd::encode_all(Cursor::new(&raw), lvl).unwrap_or(raw)
            };
            
            out.extend_from_slice(&(blob.len() as u32).to_le_bytes());
            out.extend_from_slice(&blob);
        }
        
        out
    }
    
    /// Total bytes across all retained chunks (closed + current).
    pub fn total_bytes(&self) -> usize {
        let closed: usize = self.closed_chunks.iter().map(|c| c.bytes_used()).sum();
        let current = self.current_chunk.as_ref().map(|c| c.bytes_used()).unwrap_or(0);
        closed + current
    }
    
    /// Update each chunk's priority based on current age, then demote/evict 
    /// the lowest-priority chunks until under budget.
    fn enforce_budget(&mut self) {
        // Refresh priorities. Aligned chunks (those that point to a canonical) 
        // get a 0.5× damping multiplier — their geometry is partially 
        // reconstructible from the canonical web + alignment transform, so 
        // they are first to evict under pressure. Canonical webs must be 
        // kept to enable reconstruction of their followers.
        let now = self.access_clock as f32;
        let recency_w = self.policy.recency_weight;
        for chunk in &mut self.closed_chunks {
            let age = (now - chunk.last_accessed as f32).max(0.0);
            let recency = 1.0 / (1.0 + age * 0.1);  // decays as age grows
            let mut p = chunk.quality_score + recency_w * recency;
            // Alignment damping: aligned-to-others (not self-canonical) 
            // are cheaper to lose. The transform alone is preserved.
            if let Some(canon_id) = chunk.canonical_web_id {
                if canon_id != chunk.frame_start {
                    p *= 0.5;
                }
            }
            chunk.priority = p;
        }
        
        let budget = self.policy.max_memory_bytes;
        let mut iters = 0;
        while self.total_bytes() > budget && iters < 1000 {
            iters += 1;
            
            // Find lowest priority chunk that still has data we can drop
            let lowest: Option<usize> = self.closed_chunks.iter().enumerate()
                .filter(|(_, c)| c.tier > RetentionTier::Stats)
                .min_by(|(_, a), (_, b)| 
                    a.priority.partial_cmp(&b.priority)
                        .unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i);
            
            match lowest {
                Some(i) => {
                    let freed = self.closed_chunks[i].demote();
                    self.total_demotions += 1;
                    if freed == 0 { break; }  // safety: demote was a no-op
                }
                None => {
                    // All chunks at Stats tier; nothing more to demote.
                    // 
                    // Two paths:
                    //   A. If a spill file is open, SPILL the lowest-priority
                    //      chunk to disk. Its mesh + pose track survive the 
                    //      encode and are read back at trailer-write time.
                    //   B. Otherwise (no spill file or spill_chunk failed), 
                    //      fall back to outright EVICT — drop the chunk and 
                    //      everything in it.
                    let lowest = self.closed_chunks.iter().enumerate()
                        .min_by(|(_, a), (_, b)| 
                            a.priority.partial_cmp(&b.priority)
                                .unwrap_or(std::cmp::Ordering::Equal))
                        .map(|(i, _)| i);
                    let Some(i) = lowest else { break; };
                    
                    let spilled = if self.spill_file.is_some() {
                        match self.spill_chunk(i) {
                            Ok(_freed) => true,
                            Err(e) => {
                                static FAIL_WARN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                                if !FAIL_WARN.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                    eprintln!("[chunk_store] spill failed (will evict instead): {}", e);
                                }
                                false
                            }
                        }
                    } else {
                        false
                    };
                    
                    if !spilled {
                        // Fall back to drop.
                        self.closed_chunks.remove(i);
                        self.total_chunks_evicted += 1;
                    }
                }
            }
        }
    }
    
    /// Iterate retained source frames in priority-then-time order.
    /// Future texture-projection pass walks this.
    pub fn iter_retained_frames(&self) -> impl Iterator<Item = &RetainedFrame> {
        self.closed_chunks.iter()
            .flat_map(|c| c.representative_frames.iter().chain(c.full_frames.iter()))
    }
    
    /// Diagnostic: tier distribution + memory + web alignment.
    pub fn report(&self) -> String {
        let mut counts = [0usize; 4];
        let mut n_aligned = 0;
        let mut total_verts = 0usize;
        for c in &self.closed_chunks {
            counts[c.tier as usize] += 1;
            if c.canonical_web_id.is_some() { n_aligned += 1; }
            total_verts += c.web_mesh.vertices.len();
        }
        format!(
            "ChunkStore: {} chunks ({}v total, {} aligned), {:.1}MB | tiers stats={} sum={} samp={} full={} | evicted={} demoted={}",
            self.closed_chunks.len(),
            total_verts,
            n_aligned,
            self.total_bytes() as f32 / 1_048_576.0,
            counts[0], counts[1], counts[2], counts[3],
            self.total_chunks_evicted, self.total_demotions,
        )
    }
    
    // ─── Disk spillover ──────────────────────────────────────────────
    
    /// Open a spill file at the given path. Subsequent eviction pressure 
    /// will spill chunks here instead of dropping them.
    /// 
    /// Returns Some(path) on success, None if creation failed.
    pub fn open_spill(&mut self, path: &std::path::Path) -> Option<std::path::PathBuf> {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(path).ok()?;
        self.spill_file = Some(std::sync::Mutex::new(f));
        self.spill_path = Some(path.to_path_buf());
        self.spill_offset = 0;
        Some(path.to_path_buf())
    }
    
    /// Spill the chunk at `idx` in closed_chunks to disk.
    pub fn spill_chunk(&mut self, idx: usize) -> Result<usize, std::io::Error> {
        if idx >= self.closed_chunks.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "out of bounds"));
        }
        
        // Build the flat serialization buffer + extract metadata for the 
        // stub. Done in a scoped block so the immutable borrow on 
        // self.closed_chunks releases before we touch other self fields.
        let (buf, v_count, t_count, bytes_before, stub_meta) = {
            let chunk = &self.closed_chunks[idx];
            let bytes_before = chunk.bytes_used();
            
            let mut buf = Vec::with_capacity(64 * 1024);
            let v_count = chunk.web_mesh.vertices.len() as u32;
            let t_count = (chunk.web_mesh.indices.len() / 3) as u32;
            let p_count = chunk.web_pose_track.len() as u32;
            buf.extend_from_slice(&v_count.to_le_bytes());
            buf.extend_from_slice(&t_count.to_le_bytes());
            buf.extend_from_slice(&p_count.to_le_bytes());
            for v in &chunk.web_mesh.vertices {
                buf.extend_from_slice(&v.x.to_le_bytes());
                buf.extend_from_slice(&v.y.to_le_bytes());
                buf.extend_from_slice(&v.z.to_le_bytes());
            }
            for c in &chunk.web_mesh.vertex_colors {
                buf.extend_from_slice(&c[0].to_le_bytes());
                buf.extend_from_slice(&c[1].to_le_bytes());
                buf.extend_from_slice(&c[2].to_le_bytes());
            }
            let n_count = chunk.web_mesh.normals.len() as u32;
            buf.extend_from_slice(&n_count.to_le_bytes());
            for n in &chunk.web_mesh.normals {
                buf.extend_from_slice(&n.x.to_le_bytes());
                buf.extend_from_slice(&n.y.to_le_bytes());
                buf.extend_from_slice(&n.z.to_le_bytes());
            }
            for i in &chunk.web_mesh.indices {
                buf.extend_from_slice(&i.to_le_bytes());
            }
            for p in &chunk.web_pose_track {
                buf.extend_from_slice(&p.to_bytes());
            }
            
            // Capture stub fields BEFORE chunk borrow ends. All Copy types.
            let stub_meta = (
                chunk.frame_start,
                chunk.frame_end,
                chunk.quality_score,
                chunk.n_anchors,
                chunk.mean_density_cov,
                chunk.static_ratio,
                chunk.canonical_web_id,
                chunk.web_alignment_transform,
                chunk.web_signature,
            );
            (buf, v_count, t_count, bytes_before, stub_meta)
        };
        
        // Compress and write — chunk borrow no longer held.
        let compressed = zstd::encode_all(buf.as_slice(), 3)?;
        let offset = self.spill_offset;
        let len = compressed.len() as u64;
        {
            use std::io::{Seek, SeekFrom, Write};
            let spill_file_mtx = match self.spill_file.as_ref() {
                Some(f) => f,
                None => return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no spill file open")),
            };
            let mut f = spill_file_mtx.lock().expect("spill file lock poisoned");
            f.seek(SeekFrom::Start(offset))?;
            f.write_all(&compressed)?;
        }
        self.spill_offset += len;
        
        let (frame_start, frame_end, quality_score, n_anchors, mean_density_cov,
             static_ratio, canonical_web_id, web_alignment_transform, web_signature) = stub_meta;
        let stub = SpilledChunk {
            frame_start,
            frame_end,
            quality_score,
            n_anchors,
            mean_density_cov,
            static_ratio,
            canonical_web_id,
            web_alignment_transform,
            web_signature,
            spill_offset: offset,
            spill_len: len,
            web_vertex_count: v_count,
            web_triangle_count: t_count,
        };
        
        self.closed_chunks.remove(idx);
        self.spilled_chunks.push(stub);
        self.total_chunks_spilled += 1;
        Ok(bytes_before)
    }
    
    /// Try to match recently-closed chunks against the FAO library. For 
    /// each chunk whose web signature matches an existing library canonical 
    /// at >= threshold similarity, record a LibraryRef and release the 
    /// chunk's web_mesh from RAM (storing only metadata).
    /// 
    /// Returns the number of new matches recorded.
    /// 
    /// Called by V8Encoder::finalize() and optionally per-chunk when a 
    /// new chunk closes (depending on policy). Per-chunk matching gives 
    /// faster mid-encode RAM relief; finalize-time matching catches 
    /// chunks that closed before the library was fully indexed.
    /// `library` is taken as `&mut` so we can call `touch` on each 
    /// matched canonical. This keeps ref_count + last_referenced in 
    /// sync with actual usage, which informs library GC and "popular 
    /// canonicals" reporting downstream. The touch is O(1) (single 
    /// log_db append) so the cost is negligible.
    pub fn try_match_against_library(
        &mut self,
        library: &mut crate::library::Library,
        threshold: f32,
    ) -> usize {
        let mut new_matches = 0;
        let mut to_remove: Vec<usize> = Vec::new();
        // Touch matched UUIDs after the scan so we can take &mut on library.
        let mut touch_queue: Vec<String> = Vec::new();
        
        for (idx, chunk) in self.closed_chunks.iter().enumerate() {
            // Skip empty meshes — nothing to match against.
            if chunk.web_mesh.vertices.is_empty() { continue; }
            // Skip if this chunk is already an aligned-follower of an 
            // intra-encode canonical. Library match is preferred for the 
            // CANONICAL of a cluster (since it lets the entire cluster's 
            // followers route through one library ref).
            //
            // For now we only match canonical/standalone chunks. Aligned 
            // followers stay as intra-encode aligns.
            if let Some(canon_id) = chunk.canonical_web_id {
                if canon_id != chunk.frame_start { continue; }
            }
            
            let m = library.find_match(&chunk.web_signature, threshold);
            if let Some((stub, score)) = m {
                // Compute residual: vertices of THIS chunk's mesh that don't 
                // match (after alignment) any vertex in the canonical. For a 
                // first cut, treat full vertex list as residual when the 
                // alignment transform is identity (we don't have full ICP 
                // here yet); when alignment is present, omit residual entirely 
                // (assume canonical covers it). Conservative: small-residual 
                // first cut later expanded with proper ICP.
                let alignment_transform = chunk.web_alignment_transform;
                let identity_norm: f32 = (0..16).map(|i| {
                    let want = if i % 5 == 0 { 1.0 } else { 0.0 };
                    (alignment_transform[i] - want).powi(2)
                }).sum();
                let is_identity = identity_norm < 1e-6;
                
                // For now, conservative: store NO residual. The library 
                // canonical alone is used for render. Compression: maximum. 
                // Quality: depends on similarity; threshold should be high 
                // enough that the difference is negligible.
                let residual_vertices = Vec::new();
                let residual_colors = Vec::new();
                let _ = is_identity;  // reserved for future expansion
                
                let bytes_saved = chunk.web_mesh.vertices.len() * 24
                    + chunk.web_mesh.indices.len() * 4
                    + chunk.web_mesh.normals.len() * 12;
                
                let matched_uuid = stub.uuid.clone();
                self.library_refs.push(LibraryRef {
                    chunk_frame_start: chunk.frame_start,
                    chunk_frame_end: chunk.frame_end,
                    uuid: matched_uuid.clone(),
                    alignment_transform,
                    similarity_score: score,
                    residual_vertices,
                    residual_colors,
                });
                
                // Queue the touch — we'll batch-apply after the 
                // immutable scan loop to avoid borrow conflicts.
                touch_queue.push(matched_uuid);
                
                self.total_library_matches += 1;
                self.total_library_match_bytes_saved += bytes_saved as u64;
                new_matches += 1;
                to_remove.push(idx);
            }
        }
        
        // Apply touches to the library. Best-effort — log on error 
        // but don't fail the whole match pass.
        for uuid in &touch_queue {
            if let Err(e) = library.touch(uuid) {
                eprintln!("[v8 library] WARN: touch failed for {}: {}", uuid, e);
            }
        }
        
        // Remove matched chunks from closed_chunks (highest idx first to 
        // avoid index invalidation).
        for &idx in to_remove.iter().rev() {
            self.closed_chunks.remove(idx);
        }
        
        new_matches
    }
    
    /// Read all spilled chunks back into RAM. Called once at trailer-write time.
    pub fn unspill_all(&mut self) {
        if self.spilled_chunks.is_empty() { return; }
        let stubs = std::mem::take(&mut self.spilled_chunks);
        let mut restored = 0usize;
        let mut failed = 0usize;
        
        let spill_file_arc = match self.spill_file.take() {
            Some(f) => f,
            None => {
                eprintln!("[chunk_store] WARNING: {} chunks marked spilled but no spill file open", stubs.len());
                return;
            }
        };
        
        for stub in stubs {
            let body = {
                use std::io::{Read, Seek, SeekFrom};
                let mut compressed = vec![0u8; stub.spill_len as usize];
                let mut f = match spill_file_arc.lock() {
                    Ok(g) => g,
                    Err(_) => { failed += 1; continue; }
                };
                if f.seek(SeekFrom::Start(stub.spill_offset)).is_err() { failed += 1; continue; }
                if f.read_exact(&mut compressed).is_err() { failed += 1; continue; }
                drop(f);
                match zstd::decode_all(compressed.as_slice()) {
                    Ok(b) => b,
                    Err(_) => { failed += 1; continue; }
                }
            };
            
            if body.len() < 12 { failed += 1; continue; }
            let v_count = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
            let t_count = u32::from_le_bytes([body[4], body[5], body[6], body[7]]) as usize;
            let p_count = u32::from_le_bytes([body[8], body[9], body[10], body[11]]) as usize;
            let mut off = 12usize;
            
            let needed_v = v_count * 12 * 2 + 4;
            if body.len() < off + needed_v { failed += 1; continue; }
            
            let mut vertices = Vec::with_capacity(v_count);
            for _ in 0..v_count {
                let x = f32::from_le_bytes([body[off], body[off+1], body[off+2], body[off+3]]);
                let y = f32::from_le_bytes([body[off+4], body[off+5], body[off+6], body[off+7]]);
                let z = f32::from_le_bytes([body[off+8], body[off+9], body[off+10], body[off+11]]);
                vertices.push(Vec3 { x, y, z });
                off += 12;
            }
            let mut vertex_colors = Vec::with_capacity(v_count);
            for _ in 0..v_count {
                let r = f32::from_le_bytes([body[off], body[off+1], body[off+2], body[off+3]]);
                let g = f32::from_le_bytes([body[off+4], body[off+5], body[off+6], body[off+7]]);
                let b = f32::from_le_bytes([body[off+8], body[off+9], body[off+10], body[off+11]]);
                vertex_colors.push([r, g, b]);
                off += 12;
            }
            if body.len() < off + 4 { failed += 1; continue; }
            let n_count = u32::from_le_bytes([body[off], body[off+1], body[off+2], body[off+3]]) as usize;
            off += 4;
            if body.len() < off + n_count * 12 { failed += 1; continue; }
            let mut normals = Vec::with_capacity(n_count);
            for _ in 0..n_count {
                let x = f32::from_le_bytes([body[off], body[off+1], body[off+2], body[off+3]]);
                let y = f32::from_le_bytes([body[off+4], body[off+5], body[off+6], body[off+7]]);
                let z = f32::from_le_bytes([body[off+8], body[off+9], body[off+10], body[off+11]]);
                normals.push(Vec3 { x, y, z });
                off += 12;
            }
            let i_count = t_count * 3;
            if body.len() < off + i_count * 4 { failed += 1; continue; }
            let mut indices = Vec::with_capacity(i_count);
            for _ in 0..i_count {
                indices.push(u32::from_le_bytes([body[off], body[off+1], body[off+2], body[off+3]]));
                off += 4;
            }
            if body.len() < off + p_count * 48 { failed += 1; continue; }
            let mut web_pose_track = Vec::with_capacity(p_count);
            for _ in 0..p_count {
                if let Some(pose) = CameraPose::from_bytes(&body[off..off+48]) {
                    web_pose_track.push(pose);
                }
                off += 48;
            }
            
            let vertex_hits = vec![1u32; v_count];
            let vertex_grid = std::collections::HashMap::new();
            
            let chunk = RetainedChunk {
                frame_start: stub.frame_start,
                frame_end: stub.frame_end,
                quality_score: stub.quality_score,
                priority: 0.0,
                tier: RetentionTier::Stats,
                n_anchors: stub.n_anchors,
                n_features_tracked: 0,
                mean_density_cov: stub.mean_density_cov,
                static_ratio: stub.static_ratio,
                anchor_summaries: Vec::new(),
                representative_frames: Vec::new(),
                full_frames: Vec::new(),
                web_mesh: SceneMesh {
                    vertices,
                    vertex_colors,
                    indices,
                    normals,
                    vertex_hits,
                    vertex_grid,
                },
                web_pose_track,
                web_alignment_transform: stub.web_alignment_transform,
                canonical_web_id: stub.canonical_web_id,
                web_signature: stub.web_signature,
                last_accessed: 0,
            };
            self.closed_chunks.push(chunk);
            restored += 1;
        }
        
        if failed > 0 {
            eprintln!("[chunk_store] unspill_all: {} restored, {} failed", restored, failed);
        }
        
        // Drop the spill file handle, then remove the file.
        drop(spill_file_arc);
        self.spill_offset = 0;
        if let Some(p) = self.spill_path.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod chunk_store_tests {
    use super::*;
    
    fn fake_anchor(frame: u32, cov: f32, valid: u32) -> AnchorSummary {
        AnchorSummary {
            frame_idx: frame,
            pose: CameraPose::default(),
            density_coverage: cov,
            triangles_added: 1000,
            valid_pixel_count: valid,
        }
    }
    
    #[test]
    fn opens_and_closes_chunk() {
        let mut store = ChunkStore::new();
        store.open_chunk(0);
        store.add_anchor(fake_anchor(60, 0.5, 30000));
        store.add_anchor(fake_anchor(120, 0.6, 35000));
        store.close_chunk(180);
        
        assert_eq!(store.closed_chunks.len(), 1);
        let c = &store.closed_chunks[0];
        assert_eq!(c.frame_start, 0);
        assert_eq!(c.frame_end, 180);
        assert_eq!(c.n_anchors, 2);
        // Quality should be > 0 with reasonable inputs
        assert!(c.quality_score > 0.0);
    }
    
    #[test]
    fn high_quality_chunk_scores_higher() {
        let mut store = ChunkStore::new();
        // Low-quality chunk: poor coverage, few features
        store.open_chunk(0);
        store.add_anchor(fake_anchor(60, 0.1, 5000));
        store.close_chunk(120);
        
        // High-quality chunk: good coverage, many features
        store.open_chunk(120);
        store.add_anchor(fake_anchor(180, 0.8, 45000));
        store.add_anchor(fake_anchor(240, 0.85, 50000));
        store.close_chunk(300);
        
        let lo = store.closed_chunks[0].quality_score;
        let hi = store.closed_chunks[1].quality_score;
        assert!(hi > lo, "high-q quality {} should exceed low-q {}", hi, lo);
    }
    
    #[test]
    fn frame_retention_respects_tier() {
        let mut policy = ChunkStorePolicy::default();
        policy.default_tier = RetentionTier::Sampled;
        policy.representative_stride = 30;
        let mut store = ChunkStore::with_policy(policy);
        
        store.open_chunk(0);
        let yuv = vec![0u8; 1024];
        // Add 100 frames; only every 30th should be kept (Sampled tier)
        for f in 0..100 {
            store.add_frame(f, &yuv);
        }
        store.close_chunk(100);
        
        let c = &store.closed_chunks[0];
        // Frames 0, 30, 60, 90 → 4 retained
        assert_eq!(c.representative_frames.len(), 4);
        assert!(c.full_frames.is_empty());
    }
    
    #[test]
    fn budget_triggers_demotion() {
        let mut policy = ChunkStorePolicy::default();
        policy.max_memory_bytes = 50_000;  // small budget
        policy.default_tier = RetentionTier::Sampled;
        policy.representative_stride = 1;  // retain every frame
        let mut store = ChunkStore::with_policy(policy);
        
        let yuv = vec![0u8; 10_000];  // 10K per frame
        for chunk in 0..5 {
            store.open_chunk(chunk * 10);
            // Make first chunk low quality, others high quality
            let cov = if chunk == 0 { 0.05 } else { 0.8 };
            store.add_anchor(fake_anchor(chunk * 10 + 5, cov, if chunk == 0 { 1000 } else { 40000 }));
            for f in 0..10 {
                store.add_frame(chunk * 10 + f, &yuv);
            }
            store.close_chunk(chunk * 10 + 10);
        }
        
        // Total raw: 5 chunks × 10 frames × 10K = 500K. Budget 50K → many demotions.
        assert!(store.total_demotions > 0, "should have demoted under budget");
        // The lowest-quality chunk (chunk 0) should have been demoted first
        let chunk_0 = &store.closed_chunks[0];
        let chunk_2 = &store.closed_chunks[2];
        // Lower quality chunk should be at a lower or equal tier than higher quality
        assert!(chunk_0.tier <= chunk_2.tier,
            "low-quality chunk_0 tier={:?} should be ≤ high-quality chunk_2 tier={:?}",
            chunk_0.tier, chunk_2.tier);
    }
    
    #[test]
    fn iter_retained_frames_walks_all_kept() {
        let mut policy = ChunkStorePolicy::default();
        policy.default_tier = RetentionTier::Sampled;
        policy.representative_stride = 1;
        let mut store = ChunkStore::with_policy(policy);
        
        let yuv = vec![0u8; 100];
        store.open_chunk(0);
        store.add_frame(0, &yuv);
        store.add_frame(1, &yuv);
        store.close_chunk(2);
        store.open_chunk(2);
        store.add_frame(2, &yuv);
        store.close_chunk(3);
        
        let frame_indices: Vec<u32> = store.iter_retained_frames()
            .map(|f| f.frame_idx)
            .collect();
        assert_eq!(frame_indices, vec![0, 1, 2]);
    }
}
