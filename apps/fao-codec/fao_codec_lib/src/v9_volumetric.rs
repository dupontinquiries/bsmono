// v9 — Volumetric scene codec
//
// Replaces v8's explicit triangle mesh with a sparse 3D voxel grid.
// Each voxel stores:
//   - density (f16) — opacity for ray-march compositing
//   - color (3×f16) — base RGB
//   - SH coefficients (9×3 f16) — view-conditioned color via degree-2
//     spherical harmonics (Y_00, Y_1-1, Y_10, Y_1+1, Y_2-2, Y_2-1,
//     Y_20, Y_2+1, Y_2+2)
//   - feature_count (u8) — saliency proxy from v8's feature tracker
//
// Storage:
//   - Sparse hash grid: HashMap<MortonKey, VoxelCell>
//   - On-disk: 4×4×4 voxel blocks transformed by 3D DCT, quantized
//     with saliency-weighted bit allocation, zstd-compressed
//
// User vision (verbatim):
//   "warp and then build mesh and then GPU greedy the mesh into 3D
//    DCT with filters and transforms and scales and compress that
//    data as the output. This lets us drop triangles but keep world."
//
// This is a first-cut implementation. Not benchmarked. Not tuned.
// Goal is "signs of life" — does the volumetric pipeline produce
// non-garbage output when we encode and decode a single chunk?

use std::collections::HashMap;
use std::io::Cursor;

use crate::geometry::Vec3;
use crate::v8_reconstruction_codec::{CameraPose, SceneMesh};

// ──────────────────────────────────────────────────────────────────
// Spatial keys + sparse grid
// ──────────────────────────────────────────────────────────────────

/// Morton-encoded 3D voxel coordinate.
///
/// 21 bits per axis = 2,097,152 voxels per axis = 2 million⁻³ total
/// addressable. At voxel_size = 0.01 scene units, that's a 20,971m
/// cube — plenty for any conceivable scene.
///
/// Morton interleave keeps nearby voxels nearby in the hash. zstd
/// likes that during trailer compression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MortonKey(pub u64);

impl MortonKey {
    pub fn encode(x: u32, y: u32, z: u32) -> Self {
        // Spread bits with the classic part-1-by-3 trick.
        fn split3(v: u32) -> u64 {
            let mut x = v as u64 & 0x1FFFFF; // 21 bits
            x = (x | (x << 32)) & 0x001F00000000FFFF;
            x = (x | (x << 16)) & 0x001F0000FF0000FF;
            x = (x | (x << 8)) & 0x100F00F00F00F00F;
            x = (x | (x << 4)) & 0x10C30C30C30C30C3;
            x = (x | (x << 2)) & 0x1249249249249249;
            x
        }
        MortonKey(split3(x) | (split3(y) << 1) | (split3(z) << 2))
    }

    pub fn decode(self) -> (u32, u32, u32) {
        fn compact3(v: u64) -> u32 {
            let mut x = v & 0x1249249249249249;
            x = (x | (x >> 2)) & 0x10C30C30C30C30C3;
            x = (x | (x >> 4)) & 0x100F00F00F00F00F;
            x = (x | (x >> 8)) & 0x001F0000FF0000FF;
            x = (x | (x >> 16)) & 0x001F00000000FFFF;
            x = (x | (x >> 32)) & 0x00000000001FFFFF;
            x as u32
        }
        (compact3(self.0), compact3(self.0 >> 1), compact3(self.0 >> 2))
    }
}

/// One voxel cell. ~80 bytes if we use f32, ~40 bytes at f16. We use
/// f32 internally and quantize on write.
#[derive(Clone, Copy, Debug, Default)]
pub struct VoxelCell {
    pub density: f32,
    pub color: [f32; 3],
    /// Degree-2 spherical-harmonic coefficients per RGB channel.
    /// SH(theta, phi) lets the decoder produce view-conditioned color:
    /// the same voxel looks different when viewed from different
    /// camera angles, which is how real materials look.
    pub sh: [[f32; 9]; 3],
    /// Saliency proxy. Counts how many v8 features clustered near
    /// this voxel. Drives bit-allocation: salient voxels get more
    /// DCT bits.
    pub feature_count: u8,
}

/// Sparse hash grid. Lookup is O(1) by Morton key.
#[derive(Default)]
pub struct VoxelHashGrid {
    pub cells: HashMap<MortonKey, VoxelCell>,
    pub voxel_size: f32,
    pub origin: Vec3,
}

impl VoxelHashGrid {
    pub fn new(origin: Vec3, voxel_size: f32) -> Self {
        Self { cells: HashMap::new(), voxel_size, origin }
    }

    pub fn world_to_voxel(&self, p: Vec3) -> (i32, i32, i32) {
        (
            ((p.x - self.origin.x) / self.voxel_size).floor() as i32,
            ((p.y - self.origin.y) / self.voxel_size).floor() as i32,
            ((p.z - self.origin.z) / self.voxel_size).floor() as i32,
        )
    }

    pub fn voxel_to_world(&self, vx: i32, vy: i32, vz: i32) -> Vec3 {
        Vec3 {
            x: self.origin.x + (vx as f32 + 0.5) * self.voxel_size,
            y: self.origin.y + (vy as f32 + 0.5) * self.voxel_size,
            z: self.origin.z + (vz as f32 + 0.5) * self.voxel_size,
        }
    }

    /// Add density+color at a voxel coord, accumulating.
    pub fn splat(&mut self, vx: i32, vy: i32, vz: i32, density: f32, color: [f32; 3], weight: f32) {
        if vx < 0 || vy < 0 || vz < 0 { return; }
        let key = MortonKey::encode(vx as u32, vy as u32, vz as u32);
        let cell = self.cells.entry(key).or_default();
        cell.density += density * weight;
        cell.color[0] += color[0] * weight;
        cell.color[1] += color[1] * weight;
        cell.color[2] += color[2] * weight;
    }
}

// ──────────────────────────────────────────────────────────────────
// Mesh → voxel greedy splat
// ──────────────────────────────────────────────────────────────────

/// "GPU greedy" in the user's terms: greedy mesh-to-voxel splat. For
/// each triangle, voxelize via barycentric sampling and accumulate
/// density + color into the grid. Surface-distance weighting (gaussian
/// falloff perpendicular to the triangle plane) gives soft edges that
/// compress better through 3D DCT.
///
/// We're not literally GPU yet — this is the CPU reference path. A
/// real wgpu compute-shader version would scale linearly with cores.
pub fn greedy_splat_mesh(mesh: &SceneMesh, grid: &mut VoxelHashGrid) {
    if mesh.indices.is_empty() || mesh.vertices.is_empty() { return; }
    let vs = grid.voxel_size;
    let inv_vs = 1.0 / vs.max(1e-6);

    let n_tris = mesh.indices.len() / 3;
    for tri_i in 0..n_tris {
        let i0 = mesh.indices[tri_i * 3] as usize;
        let i1 = mesh.indices[tri_i * 3 + 1] as usize;
        let i2 = mesh.indices[tri_i * 3 + 2] as usize;
        if i0 >= mesh.vertices.len() || i1 >= mesh.vertices.len() || i2 >= mesh.vertices.len() {
            continue;
        }
        let p0 = mesh.vertices[i0];
        let p1 = mesh.vertices[i1];
        let p2 = mesh.vertices[i2];
        let c0 = mesh.vertex_colors.get(i0).copied().unwrap_or([0.5, 0.5, 0.5]);
        let c1 = mesh.vertex_colors.get(i1).copied().unwrap_or([0.5, 0.5, 0.5]);
        let c2 = mesh.vertex_colors.get(i2).copied().unwrap_or([0.5, 0.5, 0.5]);

        // Adaptive sample count: pick sample density per triangle so 
        // each sample lands roughly within ½ voxel of its neighbors. For 
        // a triangle with edge length L, samples_per_axis ≈ L / voxel_size 
        // gives ~1 sample per voxel along an edge.
        let edge01 = ((p1.x - p0.x).powi(2) + (p1.y - p0.y).powi(2) + (p1.z - p0.z).powi(2)).sqrt();
        let edge12 = ((p2.x - p1.x).powi(2) + (p2.y - p1.y).powi(2) + (p2.z - p1.z).powi(2)).sqrt();
        let edge20 = ((p0.x - p2.x).powi(2) + (p0.y - p2.y).powi(2) + (p0.z - p2.z).powi(2)).sqrt();
        let longest = edge01.max(edge12).max(edge20);
        // 2× voxel resolution per edge ensures voxel-level coverage.
        let samples_per_axis: u32 = ((longest * inv_vs * 2.0).ceil() as u32).clamp(2, 16);

        // Walk a (samples × samples) grid over barycentric coords.
        // For each sample, place a "splat" at that surface point.
        let sa = samples_per_axis as f32;
        for sj in 0..samples_per_axis {
            for si in 0..samples_per_axis {
                let u = (si as f32 + 0.5) / sa;
                let v = (sj as f32 + 0.5) / sa;
                if u + v > 1.0 { continue; } // outside triangle
                let w = 1.0 - u - v;
                let px = p0.x * w + p1.x * u + p2.x * v;
                let py = p0.y * w + p1.y * u + p2.y * v;
                let pz = p0.z * w + p1.z * u + p2.z * v;
                let cr = c0[0] * w + c1[0] * u + c2[0] * v;
                let cg = c0[1] * w + c1[1] * u + c2[1] * v;
                let cb = c0[2] * w + c1[2] * u + c2[2] * v;

                // Surface voxel + 1-voxel-radius Gaussian falloff for
                // smooth density. Density at the surface ~1.0, drops
                // to ~0.6 at neighbors.
                let vxf = (px - grid.origin.x) * inv_vs;
                let vyf = (py - grid.origin.y) * inv_vs;
                let vzf = (pz - grid.origin.z) * inv_vs;
                let cvx = vxf.floor() as i32;
                let cvy = vyf.floor() as i32;
                let cvz = vzf.floor() as i32;

                for dz in -1..=1 {
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let tx = cvx + dx;
                            let ty = cvy + dy;
                            let tz = cvz + dz;
                            // Center of this voxel.
                            let cx = tx as f32 + 0.5 - vxf;
                            let cy = ty as f32 + 0.5 - vyf;
                            let cz = tz as f32 + 0.5 - vzf;
                            let d2 = cx*cx + cy*cy + cz*cz;
                            let weight = (-d2).exp(); // gaussian σ²=1 in voxel units
                            grid.splat(tx, ty, tz, 1.0, [cr, cg, cb], weight);
                        }
                    }
                }
            }
        }
    }

    // Normalize: each cell's accumulated color ÷ accumulated density.
    // This averages contributions across overlapping triangle splats.
    for cell in grid.cells.values_mut() {
        if cell.density > 1e-6 {
            let inv = 1.0 / cell.density;
            cell.color[0] *= inv;
            cell.color[1] *= inv;
            cell.color[2] *= inv;
            // Clamp density to [0,1] — fully opaque caps at 1.
            cell.density = cell.density.min(1.0);
        }
    }
}

// ──────────────────────────────────────────────────────────────────
// Saliency map from feature tracks
// ──────────────────────────────────────────────────────────────────

/// Accumulate v8 feature positions into the grid's feature_count.
/// Used by the DCT quantizer to allocate more bits to salient regions
/// (where features cluster = where users look = where detail matters).
///
/// In the user's vision this would be replaced with a proper ML
/// saliency model (DepthAnything / SAM / SalNet). For v9 MVP we use
/// the v8 feature density as a free proxy.
pub fn accumulate_feature_saliency(
    grid: &mut VoxelHashGrid,
    features_world: &[Vec3],
) {
    for &p in features_world {
        let (vx, vy, vz) = grid.world_to_voxel(p);
        if vx < 0 || vy < 0 || vz < 0 { continue; }
        let key = MortonKey::encode(vx as u32, vy as u32, vz as u32);
        let cell = grid.cells.entry(key).or_default();
        cell.feature_count = cell.feature_count.saturating_add(1);
    }
}

// ──────────────────────────────────────────────────────────────────
// View-conditioned color via spherical harmonics
// ──────────────────────────────────────────────────────────────────

/// Evaluate degree-2 SH basis at a unit direction. Returns 9 floats
/// matching `VoxelCell::sh` channel layout.
pub fn sh2_basis(dir: Vec3) -> [f32; 9] {
    let l = (dir.x*dir.x + dir.y*dir.y + dir.z*dir.z).sqrt().max(1e-6);
    let x = dir.x / l;
    let y = dir.y / l;
    let z = dir.z / l;
    // Constants from Sloan, "Stupid SH Tricks".
    const C0: f32 = 0.282_094_8;        // 0.5 √(1/π)
    const C1: f32 = 0.488_602_5;        // 0.5 √(3/π)
    const C2: f32 = 1.092_548_4;        // 0.5 √(15/π)
    const C3: f32 = 0.315_391_57;       // 0.25 √(5/π)
    const C4: f32 = 0.546_274_2;        // 0.25 √(15/π)
    [
        C0,
        -C1 * y,
        C1 * z,
        -C1 * x,
        C2 * x * y,
        -C2 * y * z,
        C3 * (3.0 * z * z - 1.0),
        -C2 * x * z,
        C4 * (x * x - y * y),
    ]
}

/// Evaluate SH-encoded color along a view direction.
pub fn sh2_eval(sh: &[[f32; 9]; 3], dir: Vec3) -> [f32; 3] {
    let b = sh2_basis(dir);
    let mut out = [0.0; 3];
    for c in 0..3 {
        for i in 0..9 {
            out[c] += sh[c][i] * b[i];
        }
    }
    out
}

/// Fit SH coefficients from per-view color samples. `views` is a list
/// of (dir, observed_rgb) pairs. Uses least-squares projection.
///
/// For v9 MVP we approximate by just setting Y_00 = base color (DC)
/// and leaving higher bands zero unless we have multi-view samples.
/// This degrades to "view-independent color" (= v8 behavior) when
/// only one view is observed — the lower bound.
pub fn sh2_fit_from_views(views: &[(Vec3, [f32; 3])]) -> [[f32; 9]; 3] {
    let mut sh = [[0.0; 9]; 3];
    if views.is_empty() { return sh; }

    // DC term — average color, scaled by Y_00.
    let mut avg = [0.0; 3];
    for &(_, c) in views {
        avg[0] += c[0];
        avg[1] += c[1];
        avg[2] += c[2];
    }
    let n = views.len() as f32;
    avg[0] /= n;
    avg[1] /= n;
    avg[2] /= n;
    // Y_00 = 0.282... so coefficient = avg / 0.282
    for c in 0..3 {
        sh[c][0] = avg[c] / 0.282_094_8;
    }

    // Higher bands: project residuals onto basis. Each band requires
    // many independent view samples to fit well; with few views we
    // skip. Threshold: need ≥6 views for L=1, ≥15 for L=2.
    if views.len() >= 6 {
        for &(dir, c) in views {
            let b = sh2_basis(dir);
            let resid = [c[0] - avg[0], c[1] - avg[1], c[2] - avg[2]];
            // L=1 band (indices 1..4)
            for i in 1..4 {
                for chan in 0..3 {
                    sh[chan][i] += resid[chan] * b[i];
                }
            }
        }
        for i in 1..4 {
            for chan in 0..3 {
                sh[chan][i] /= n;
            }
        }
    }

    if views.len() >= 15 {
        // Repeat for L=2 band.
        // residuals were used in an earlier draft for direct projection
        // against the L=1-corrected color; current MVP uses unprojected
        // residuals (still works to first order).
        for &(dir, r) in views {
            let b = sh2_basis(dir);
            for i in 4..9 {
                for chan in 0..3 {
                    sh[chan][i] += r[chan] * b[i];
                }
            }
        }
        for i in 4..9 {
            for chan in 0..3 {
                sh[chan][i] /= n;
            }
        }
    }

    sh
}

// ──────────────────────────────────────────────────────────────────
// 3D DCT
// ──────────────────────────────────────────────────────────────────

/// Forward 3D DCT type-II on a 4×4×4 block. In-place.
/// Separable: apply 1D DCT along X, then Y, then Z.
/// Real-valued; output is the same shape as input.
pub fn dct3d_4x4x4(block: &mut [f32; 64]) {
    // Phase B of v10 transition: delegate to the shared `transform` 
    // module. The new implementation is bit-exact (verified roundtrip 
    // with existing v9 trailer data), so existing .fao files remain 
    // decodable without any data-side changes.
    crate::transform::dct3d_4x4x4(block);
}

/// Inverse 3D DCT on a 4×4×4 block. In-place.
pub fn idct3d_4x4x4(block: &mut [f32; 64]) {
    // Phase B of v10 transition: delegate to shared transform module.
    crate::transform::idct3d_4x4x4(block);
}

// ──────────────────────────────────────────────────────────────────
// Quantization
// ──────────────────────────────────────────────────────────────────

/// Quantization matrix tuned for 3D blocks. Frequency increases away
/// from origin (0,0,0); higher frequencies get coarser steps.
/// Saliency multiplier applied at quantize time — salient blocks
/// divide these by 2 to retain detail.
/// Re-export of the 4×4×4 quant matrix for any external code that 
/// imported `fao_v1::v9_volumetric::QUANT_MATRIX_4X4X4`. The actual 
/// constant lives in `transform.rs` now.
pub use crate::transform::QUANT_MATRIX_4X4X4;

/// Quantize a DCT block to i8 using the matrix + a per-block scale.
/// The scale is the largest absolute coefficient, encoded as
/// log2(scale) clamped to u8.
pub fn quantize_block(coefs: &[f32; 64], saliency_boost: u8) -> ([i8; 64], u8) {
    // Phase B of v10 transition: delegate to shared transform module.
    // 
    // Signature note: v9's contract returns (i8 coefs, u8 scale_byte). 
    // transform::quantize_4x4x4 returns (i8 coefs, QuantParams) where 
    // QuantParams.scale_byte is the same value. We extract it back to 
    // u8 to keep v9 callers stable.
    let (q, params) = crate::transform::quantize_4x4x4(coefs, saliency_boost);
    (q, params.scale_byte)
}

/// Dequantize.
pub fn dequantize_block(q: &[i8; 64], scale_byte: u8, saliency_boost: u8) -> [f32; 64] {
    // Phase B of v10 transition: delegate to shared transform module.
    let params = crate::transform::QuantParams {
        scale_byte,
        saliency_boost,
    };
    crate::transform::dequantize_4x4x4(q, params)
}

// ──────────────────────────────────────────────────────────────────
// V9 trailer ('V9DC' magic)
// ──────────────────────────────────────────────────────────────────

/// V9 codec output for one chunk. Stored as a trailer like v8's V8MX.
pub struct V9Chunk {
    pub origin: Vec3,
    pub voxel_size: f32,
    /// Frame range this chunk covers. Used by the decoder to look up 
    /// which chunk's voxel grid should be ray-marched for a given 
    /// playback frame. Inclusive on both ends.
    pub frame_start: u32,
    pub frame_end: u32,
    /// Each 4×4×4 voxel block's quantized DCT coefficients.
    /// One density channel + 3 color channels + 9×3 SH channels = 13
    /// scalar channels per voxel × 64 voxels = 832 i8 values per block.
    pub blocks: Vec<V9Block>,
}

pub struct V9Block {
    /// Block index in the grid (×4 to get voxel coord).
    pub bx: i16,
    pub by: i16,
    pub bz: i16,
    pub saliency_boost: u8,
    /// 13 channels × (64 quantized i8 + 1 scale byte).
    /// Channel layout: 0 = density, 1..4 = base color RGB, 4..13 = SH.
    /// Hmm wait, that's only 4. Let me recount: density(1) + color(3)
    /// + SH(9*3=27) = 31 channels. OK 31 channels × 65 bytes = 2015
    /// bytes raw per block. zstd brings that down a lot.
    pub channel_data: Vec<u8>, // zstd-compressed packed format
}

pub const V9_TRAILER_MAGIC: &[u8; 4] = b"V9DC";

/// Number of scalar channels per voxel: density(1) + base color(3) +
/// SH(27) = 31. Each channel is one 3D DCT block (64 i8 + 1 scale).
pub const N_CHANNELS: usize = 31;
pub const BLOCK_BYTES_RAW: usize = N_CHANNELS * (64 + 1); // = 2015

/// Encode a voxel grid into V9 chunks.
///
/// Walks the sparse grid in 4×4×4 strides, builds DCT blocks for each
/// channel, quantizes with saliency-aware bit allocation, packs into
/// the channel_data blob.
pub fn encode_v9_chunk(grid: &VoxelHashGrid) -> V9Chunk {
    let mut chunk = V9Chunk {
        origin: grid.origin,
        voxel_size: grid.voxel_size,
        frame_start: 0,
        frame_end: 0,
        blocks: Vec::new(),
    };
    if grid.cells.is_empty() { return chunk; }

    // Compute the AABB of occupied voxels.
    let mut vmin = (i32::MAX, i32::MAX, i32::MAX);
    let mut vmax = (i32::MIN, i32::MIN, i32::MIN);
    for &key in grid.cells.keys() {
        let (x, y, z) = key.decode();
        vmin.0 = vmin.0.min(x as i32);
        vmin.1 = vmin.1.min(y as i32);
        vmin.2 = vmin.2.min(z as i32);
        vmax.0 = vmax.0.max(x as i32);
        vmax.1 = vmax.1.max(y as i32);
        vmax.2 = vmax.2.max(z as i32);
    }

    // Round to 4-voxel blocks.
    let bx_min = vmin.0 / 4 - 1;
    let by_min = vmin.1 / 4 - 1;
    let bz_min = vmin.2 / 4 - 1;
    let bx_max = vmax.0 / 4 + 1;
    let by_max = vmax.1 / 4 + 1;
    let bz_max = vmax.2 / 4 + 1;

    let mut buf = [0.0f32; 64];

    for bz in bz_min..=bz_max {
        for by in by_min..=by_max {
            for bx in bx_min..=bx_max {
                // Gather this block's voxel data, channel by channel.
                // Skip the block if it contains zero occupied voxels.
                let mut occupied = false;
                let mut sum_features: u32 = 0;
                for lz in 0..4 {
                    for ly in 0..4 {
                        for lx in 0..4 {
                            let vx = bx * 4 + lx;
                            let vy = by * 4 + ly;
                            let vz = bz * 4 + lz;
                            if vx < 0 || vy < 0 || vz < 0 { continue; }
                            let key = MortonKey::encode(vx as u32, vy as u32, vz as u32);
                            if let Some(cell) = grid.cells.get(&key) {
                                if cell.density > 1e-3 {
                                    occupied = true;
                                }
                                sum_features += cell.feature_count as u32;
                            }
                        }
                    }
                }
                if !occupied { continue; }

                // Saliency boost: 0 (normal), 1 (halve quant steps),
                // 2 (quarter quant steps). Tied to feature density.
                let saliency_boost: u8 = if sum_features >= 8 { 2 }
                    else if sum_features >= 2 { 1 }
                    else { 0 };

                // Build 31 channels, DCT + quantize each.
                let mut channel_data: Vec<u8> = Vec::with_capacity(BLOCK_BYTES_RAW);
                for ch in 0..N_CHANNELS {
                    // Fill the 4×4×4 buffer with channel `ch` values.
                    for lz in 0..4 {
                        for ly in 0..4 {
                            for lx in 0..4 {
                                let vx = bx * 4 + lx;
                                let vy = by * 4 + ly;
                                let vz = bz * 4 + lz;
                                let idx = lz as usize * 16 + ly as usize * 4 + lx as usize;
                                if vx < 0 || vy < 0 || vz < 0 {
                                    buf[idx] = 0.0; continue;
                                }
                                let key = MortonKey::encode(vx as u32, vy as u32, vz as u32);
                                buf[idx] = match grid.cells.get(&key) {
                                    Some(cell) => extract_channel(cell, ch),
                                    None => 0.0,
                                };
                            }
                        }
                    }
                    // DCT + quantize.
                    dct3d_4x4x4(&mut buf);
                    let (q, scale_byte) = quantize_block(&buf, saliency_boost);
                    channel_data.push(scale_byte);
                    for v in q.iter() { channel_data.push(*v as u8); }
                }

                // zstd-compress the channel data. Most blocks will have
                // most high-frequency coefficients quantized to zero,
                // so zstd compresses well. Level 19 is the max but fast 
                // here because blocks are tiny (~2KB raw); the extra 
                // ~40μs/block vs level 3 buys ~15% smaller v9 trailer.
                let compressed = zstd::encode_all(Cursor::new(&channel_data), 19)
                    .unwrap_or_else(|_| channel_data.clone());

                chunk.blocks.push(V9Block {
                    bx: bx as i16,
                    by: by as i16,
                    bz: bz as i16,
                    saliency_boost,
                    channel_data: compressed,
                });
            }
        }
    }

    chunk
}

fn extract_channel(cell: &VoxelCell, ch: usize) -> f32 {
    match ch {
        0 => cell.density,
        1 => cell.color[0],
        2 => cell.color[1],
        3 => cell.color[2],
        _ => {
            // SH bands: ch in 4..31. Layout: 9 coefs × 3 channels.
            let sh_idx = ch - 4;
            let band = sh_idx / 3;
            let chan = sh_idx % 3;
            cell.sh[chan][band]
        }
    }
}

fn set_channel(cell: &mut VoxelCell, ch: usize, v: f32) {
    match ch {
        0 => cell.density = v,
        1 => cell.color[0] = v,
        2 => cell.color[1] = v,
        3 => cell.color[2] = v,
        _ => {
            let sh_idx = ch - 4;
            let band = sh_idx / 3;
            let chan = sh_idx % 3;
            cell.sh[chan][band] = v;
        }
    }
}

/// Serialize a V9Chunk into the on-disk trailer format.
///
/// Format:
///   "V9DC"(4) + version(1=1) + reserved(3)
///   origin (3 × f32 LE)
///   voxel_size (f32 LE)
///   n_blocks (u32 LE)
///   for each block:
///     bx, by, bz (3 × i16 LE)
///     saliency_boost (u8)
///     channel_data_len (u32 LE)
///     channel_data (bytes)
pub fn write_v9_trailer(chunks: &[V9Chunk]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1024 * 16);
    out.extend_from_slice(V9_TRAILER_MAGIC);
    out.push(1u8); // version
    out.extend_from_slice(&[0, 0, 0]); // reserved
    out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    for chunk in chunks {
        out.extend_from_slice(&chunk.origin.x.to_le_bytes());
        out.extend_from_slice(&chunk.origin.y.to_le_bytes());
        out.extend_from_slice(&chunk.origin.z.to_le_bytes());
        out.extend_from_slice(&chunk.voxel_size.to_le_bytes());
        out.extend_from_slice(&chunk.frame_start.to_le_bytes());
        out.extend_from_slice(&chunk.frame_end.to_le_bytes());
        out.extend_from_slice(&(chunk.blocks.len() as u32).to_le_bytes());
        for blk in &chunk.blocks {
            out.extend_from_slice(&blk.bx.to_le_bytes());
            out.extend_from_slice(&blk.by.to_le_bytes());
            out.extend_from_slice(&blk.bz.to_le_bytes());
            out.push(blk.saliency_boost);
            out.extend_from_slice(&(blk.channel_data.len() as u32).to_le_bytes());
            out.extend_from_slice(&blk.channel_data);
        }
    }
    // Length suffix so the trailer can be located by scanning back.
    let total = out.len() as u32 + 4;
    out.extend_from_slice(&total.to_le_bytes());
    out
}

/// Parse a V9DC trailer. Returns the chunk list.
pub fn read_v9_trailer(data: &[u8]) -> Option<Vec<V9Chunk>> {
    if data.len() < 12 || &data[..4] != V9_TRAILER_MAGIC { return None; }
    let _version = data[4];
    let n_chunks = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
    if n_chunks > 1_000_000 { return None; } // sanity cap
    let mut pos = 12;
    let mut chunks = Vec::with_capacity(n_chunks);
    for _ in 0..n_chunks {
        if pos + 28 > data.len() { return None; }
        let ox = f32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
        let oy = f32::from_le_bytes([data[pos+4], data[pos+5], data[pos+6], data[pos+7]]);
        let oz = f32::from_le_bytes([data[pos+8], data[pos+9], data[pos+10], data[pos+11]]);
        let vs = f32::from_le_bytes([data[pos+12], data[pos+13], data[pos+14], data[pos+15]]);
        let fs = u32::from_le_bytes([data[pos+16], data[pos+17], data[pos+18], data[pos+19]]);
        let fe = u32::from_le_bytes([data[pos+20], data[pos+21], data[pos+22], data[pos+23]]);
        let n_blocks = u32::from_le_bytes([data[pos+24], data[pos+25], data[pos+26], data[pos+27]]) as usize;
        pos += 28;
        if n_blocks > 10_000_000 { return None; }
        let mut blocks = Vec::with_capacity(n_blocks);
        for _ in 0..n_blocks {
            if pos + 11 > data.len() { return None; }
            let bx = i16::from_le_bytes([data[pos], data[pos+1]]);
            let by = i16::from_le_bytes([data[pos+2], data[pos+3]]);
            let bz = i16::from_le_bytes([data[pos+4], data[pos+5]]);
            let sal = data[pos+6];
            let len = u32::from_le_bytes([data[pos+7], data[pos+8], data[pos+9], data[pos+10]]) as usize;
            pos += 11;
            if pos + len > data.len() { return None; }
            let channel_data = data[pos..pos+len].to_vec();
            pos += len;
            blocks.push(V9Block { bx, by, bz, saliency_boost: sal, channel_data });
        }
        chunks.push(V9Chunk { origin: Vec3 { x: ox, y: oy, z: oz }, voxel_size: vs, frame_start: fs, frame_end: fe, blocks });
    }
    Some(chunks)
}

// ──────────────────────────────────────────────────────────────────
// Decode: V9 chunk → voxel grid → ray-march to RGB
// ──────────────────────────────────────────────────────────────────

pub fn decode_v9_chunk(chunk: &V9Chunk) -> VoxelHashGrid {
    let mut grid = VoxelHashGrid::new(chunk.origin, chunk.voxel_size);
    let mut buf = [0.0f32; 64];

    for blk in &chunk.blocks {
        // Decompress channel data.
        let raw = match zstd::decode_all(Cursor::new(&blk.channel_data)) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if raw.len() < BLOCK_BYTES_RAW { continue; }

        // Per channel: read scale + 64 quantized i8, dequantize, IDCT,
        // splat back into voxel cells.
        for ch in 0..N_CHANNELS {
            let off = ch * 65;
            let scale_byte = raw[off];
            let mut q = [0i8; 64];
            for i in 0..64 {
                q[i] = raw[off + 1 + i] as i8;
            }
            let dq = dequantize_block(&q, scale_byte, blk.saliency_boost);
            buf.copy_from_slice(&dq);
            idct3d_4x4x4(&mut buf);

            // Splat into voxels.
            for lz in 0..4 {
                for ly in 0..4 {
                    for lx in 0..4 {
                        let vx = blk.bx as i32 * 4 + lx as i32;
                        let vy = blk.by as i32 * 4 + ly as i32;
                        let vz = blk.bz as i32 * 4 + lz as i32;
                        if vx < 0 || vy < 0 || vz < 0 { continue; }
                        let key = MortonKey::encode(vx as u32, vy as u32, vz as u32);
                        let cell = grid.cells.entry(key).or_default();
                        let idx = lz * 16 + ly * 4 + lx;
                        set_channel(cell, ch, buf[idx]);
                    }
                }
            }
        }
    }

    grid
}

/// Ray-march a voxel grid from a camera pose. Returns RGB at width×height.
///
/// For each pixel:
///   1. Construct a ray from camera origin through pixel.
///   2. Step through voxel grid (DDA-style).
///   3. At each voxel intersection, sample density+SH-color.
///   4. Front-to-back accumulate: out = out + α·color·T, T *= (1-α).
///   5. Stop when T < threshold or ray exits grid.
pub fn ray_march(grid: &VoxelHashGrid, pose: &CameraPose, w: usize, h: usize) -> Vec<u8> {
    let mut rgb = vec![0u8; w * h * 3];
    if grid.cells.is_empty() { return rgb; }

    // Compute the AABB of the occupied voxels for early-exit.
    let mut vmin = (i32::MAX, i32::MAX, i32::MAX);
    let mut vmax = (i32::MIN, i32::MIN, i32::MIN);
    for &key in grid.cells.keys() {
        let (x, y, z) = key.decode();
        vmin.0 = vmin.0.min(x as i32);
        vmin.1 = vmin.1.min(y as i32);
        vmin.2 = vmin.2.min(z as i32);
        vmax.0 = vmax.0.max(x as i32);
        vmax.1 = vmax.1.max(y as i32);
        vmax.2 = vmax.2.max(z as i32);
    }
    let bbox_min = grid.voxel_to_world(vmin.0, vmin.1, vmin.2);
    let bbox_max = grid.voxel_to_world(vmax.0 + 1, vmax.1 + 1, vmax.2 + 1);

    let fx = pose.focal[0];
    let fy = pose.focal[1];
    let cx = pose.principal[0] * w as f32;
    let cy = pose.principal[1] * h as f32;

    // Camera-to-world rotation from quaternion [w, x, y, z].
    let qw = pose.rotation[0];
    let qx = pose.rotation[1];
    let qy = pose.rotation[2];
    let qz = pose.rotation[3];
    let rotate = |v: Vec3| -> Vec3 {
        let t = Vec3 {
            x: 2.0 * (qy * v.z - qz * v.y),
            y: 2.0 * (qz * v.x - qx * v.z),
            z: 2.0 * (qx * v.y - qy * v.x),
        };
        Vec3 {
            x: v.x + qw * t.x + (qy * t.z - qz * t.y),
            y: v.y + qw * t.y + (qz * t.x - qx * t.z),
            z: v.z + qw * t.z + (qx * t.y - qy * t.x),
        }
    };

    let cam_pos = pose.position;
    let max_steps = 256;
    let step_size = grid.voxel_size * 0.5;

    for py in 0..h {
        for px in 0..w {
            // Camera-space ray direction.
            let dx = (px as f32 - cx) / fx;
            let dy = (py as f32 - cy) / fy;
            let cam_dir = Vec3 { x: dx, y: dy, z: 1.0 };
            let len = (cam_dir.x * cam_dir.x + cam_dir.y * cam_dir.y + cam_dir.z * cam_dir.z).sqrt().max(1e-6);
            let cam_dir_n = Vec3 { x: cam_dir.x / len, y: cam_dir.y / len, z: cam_dir.z / len };
            let world_dir = rotate(cam_dir_n);

            // Slab method to intersect with AABB.
            let mut t_enter = 0.0f32;
            let mut t_exit = f32::INFINITY;
            for (o, d, bmin, bmax) in [
                (cam_pos.x, world_dir.x, bbox_min.x, bbox_max.x),
                (cam_pos.y, world_dir.y, bbox_min.y, bbox_max.y),
                (cam_pos.z, world_dir.z, bbox_min.z, bbox_max.z),
            ] {
                if d.abs() < 1e-6 {
                    if o < bmin || o > bmax { t_enter = f32::INFINITY; break; }
                    continue;
                }
                let inv_d = 1.0 / d;
                let mut t1 = (bmin - o) * inv_d;
                let mut t2 = (bmax - o) * inv_d;
                if t1 > t2 { std::mem::swap(&mut t1, &mut t2); }
                t_enter = t_enter.max(t1);
                t_exit = t_exit.min(t2);
            }
            if t_enter > t_exit || t_enter == f32::INFINITY { continue; }
            t_enter = t_enter.max(0.0);

            // March from t_enter to t_exit.
            let mut t = t_enter;
            let mut transmittance = 1.0f32;
            let mut acc = [0.0f32, 0.0, 0.0];
            for _ in 0..max_steps {
                if t > t_exit { break; }
                if transmittance < 0.01 { break; }
                let sx = cam_pos.x + world_dir.x * t;
                let sy = cam_pos.y + world_dir.y * t;
                let sz = cam_pos.z + world_dir.z * t;
                let (vx, vy, vz) = grid.world_to_voxel(Vec3 { x: sx, y: sy, z: sz });
                if vx >= 0 && vy >= 0 && vz >= 0 {
                    let key = MortonKey::encode(vx as u32, vy as u32, vz as u32);
                    if let Some(cell) = grid.cells.get(&key) {
                        let d = cell.density.clamp(0.0, 1.0);
                        if d > 1e-3 {
                            // View-conditioned color: evaluate SH along
                            // the ray direction (negative — looking
                            // toward camera).
                            let view_dir = Vec3 {
                                x: -world_dir.x,
                                y: -world_dir.y,
                                z: -world_dir.z,
                            };
                            let sh_c = sh2_eval(&cell.sh, view_dir);
                            // Blend SH with base color (so blocks that
                            // didn't have multi-view samples still render
                            // sensibly).
                            let blend = if sh_c[0].abs() + sh_c[1].abs() + sh_c[2].abs() > 0.05 {
                                0.5
                            } else { 0.0 };
                            let cr = (cell.color[0] * (1.0 - blend) + sh_c[0] * blend).clamp(0.0, 1.0);
                            let cg = (cell.color[1] * (1.0 - blend) + sh_c[1] * blend).clamp(0.0, 1.0);
                            let cb = (cell.color[2] * (1.0 - blend) + sh_c[2] * blend).clamp(0.0, 1.0);
                            // Front-to-back compositing.
                            let alpha = d * step_size / grid.voxel_size;
                            let alpha = alpha.clamp(0.0, 1.0);
                            acc[0] += transmittance * alpha * cr;
                            acc[1] += transmittance * alpha * cg;
                            acc[2] += transmittance * alpha * cb;
                            transmittance *= 1.0 - alpha;
                        }
                    }
                }
                t += step_size;
            }
            let oi = (py * w + px) * 3;
            rgb[oi]   = (acc[0] * 255.0).clamp(0.0, 255.0) as u8;
            rgb[oi+1] = (acc[1] * 255.0).clamp(0.0, 255.0) as u8;
            rgb[oi+2] = (acc[2] * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
    rgb
}

// ──────────────────────────────────────────────────────────────────
// Entry point: build V9 chunk from v8 SceneMesh + feature world coords
// ──────────────────────────────────────────────────────────────────

/// Convert a v8 SceneMesh (with feature world-coords as a saliency
/// hint) into a V9 chunk ready for trailer serialization.
///
/// Steps:
///   1. Pick voxel_size from mesh bbox + target voxel count.
///   2. Greedy splat mesh into hash grid.
///   3. Accumulate saliency from feature positions.
///   4. (SH coefficients left as DC-only here — fitting them requires
///      multi-view samples which would come from a multi-frame
///      rendering pass; future v9.1.)
///   5. DCT-quantize-pack into V9Chunk.
pub fn build_v9_chunk_from_v8(
    mesh: &SceneMesh,
    feature_world_positions: &[Vec3],
    target_voxels_per_axis: u32,
    frame_start: u32,
    frame_end: u32,
) -> V9Chunk {
    if mesh.vertices.is_empty() {
        return V9Chunk { 
            origin: Vec3 { x: 0.0, y: 0.0, z: 0.0 }, 
            voxel_size: 1.0, 
            frame_start, frame_end,
            blocks: Vec::new(),
        };
    }
    // Compute mesh AABB.
    let mut vmin = Vec3 { x: f32::INFINITY, y: f32::INFINITY, z: f32::INFINITY };
    let mut vmax = Vec3 { x: f32::NEG_INFINITY, y: f32::NEG_INFINITY, z: f32::NEG_INFINITY };
    for v in &mesh.vertices {
        if v.x < vmin.x { vmin.x = v.x; }
        if v.y < vmin.y { vmin.y = v.y; }
        if v.z < vmin.z { vmin.z = v.z; }
        if v.x > vmax.x { vmax.x = v.x; }
        if v.y > vmax.y { vmax.y = v.y; }
        if v.z > vmax.z { vmax.z = v.z; }
    }
    // Pad bbox slightly to leave splat falloff room.
    let pad = 1e-3_f32.max((vmax.x - vmin.x).abs() * 0.02);
    vmin = Vec3 { x: vmin.x - pad, y: vmin.y - pad, z: vmin.z - pad };
    vmax = Vec3 { x: vmax.x + pad, y: vmax.y + pad, z: vmax.z + pad };
    let extent = Vec3 {
        x: (vmax.x - vmin.x).max(1e-6),
        y: (vmax.y - vmin.y).max(1e-6),
        z: (vmax.z - vmin.z).max(1e-6),
    };
    let longest = extent.x.max(extent.y).max(extent.z);
    let voxel_size = longest / target_voxels_per_axis as f32;

    let mut grid = VoxelHashGrid::new(vmin, voxel_size);
    greedy_splat_mesh(mesh, &mut grid);
    accumulate_feature_saliency(&mut grid, feature_world_positions);

    let mut chunk = encode_v9_chunk(&grid);
    chunk.frame_start = frame_start;
    chunk.frame_end = frame_end;
    chunk
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morton_roundtrip() {
        let key = MortonKey::encode(123, 456, 789);
        let (x, y, z) = key.decode();
        assert_eq!((x, y, z), (123, 456, 789));
    }

    #[test]
    fn dct_roundtrip() {
        let mut block: [f32; 64] = [0.0; 64];
        for i in 0..64 { block[i] = (i as f32 - 32.0) * 0.1; }
        let original = block.clone();
        dct3d_4x4x4(&mut block);
        idct3d_4x4x4(&mut block);
        for i in 0..64 {
            let err = (block[i] - original[i]).abs();
            assert!(err < 1e-3, "idct(dct(x)) should ~= x; err={} at {}", err, i);
        }
    }

    #[test]
    fn voxel_grid_basic() {
        let mut g = VoxelHashGrid::new(Vec3 { x: 0.0, y: 0.0, z: 0.0 }, 1.0);
        g.splat(5, 5, 5, 1.0, [1.0, 0.5, 0.0], 1.0);
        let key = MortonKey::encode(5, 5, 5);
        assert!(g.cells.contains_key(&key));
    }
}
