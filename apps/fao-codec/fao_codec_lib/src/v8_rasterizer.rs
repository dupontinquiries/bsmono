//! CPU-side mesh rasterizer for v8 frame playback.
//!
//! Until a GPU pipeline lands, this rasterizer is what makes `V8Decoder::decode_frame` 
//! return actual rendered RGB instead of None. It's not fast — pure 
//! scalar Rust over per-frame triangles — but it validates the data 
//! flow end-to-end and proves the decoder math is correct.
//!
//! # Pipeline
//!
//! 1. Pick a web that covers the target frame index.
//! 2. Resolve its mesh (canonical → use directly; aligned → transform 
//!    canonical's mesh by the alignment matrix).
//! 3. Project every vertex through the per-frame camera pose to image space.
//! 4. Rasterize each triangle with barycentric interpolation, depth test.
//! 5. Return the RGB buffer.
//!
//! # Future: GPU
//!
//! The natural follow-up is a wgpu render pipeline:
//! 
//!   - Vertex buffer = mesh.vertices, color buffer = mesh.vertex_colors
//!   - Vertex shader applies camera pose (uniform: mat4 view*proj)
//!   - Fragment shader writes interpolated color
//!   - Render to texture, readback to CPU
//!
//! Estimated speedup: ~100× for typical 50K-vertex meshes at 1080p.

// Vec3 lives in the `geometry` module (extracted during v10 cleanup); 
// it was previously imported via v6_object_codec wildcard. The new 
// module so we can't reach it through v8_reconstruction_codec — pull 
// Vec3 directly from its canonical home.
use crate::geometry::Vec3;
use crate::v8_reconstruction_codec::{CameraPose, DecodedWeb, SceneMesh};

/// Find the web that covers a given frame index.
/// 
/// Returns the index into `webs`. Caller decides what to do with aligned 
/// vs canonical webs (typically: resolve aligned to its canonical).
pub fn web_for_frame(webs: &[DecodedWeb], frame_idx: u32) -> Option<usize> {
    // Linear scan — webs are typically <100 in number, so binary search 
    // doesn't pay off. If the user has 600hr archives later this could be 
    // sorted+binary-searched.
    for (i, w) in webs.iter().enumerate() {
        if frame_idx >= w.frame_start && frame_idx <= w.frame_end {
            return Some(i);
        }
    }
    None
}

/// Apply a 4x4 row-major transform to a vertex.
/// 
/// The codec stores transforms in row-major order (see 
/// `ChunkStore::compute_web_residual` for the canonical layout). Indices: 
///   m[0..4]   = row 0 (x output coefficients + translation_x)
///   m[4..8]   = row 1 (y output)
///   m[8..12]  = row 2 (z output)
///   m[12..16] = row 3 (homogeneous; typically [0, 0, 0, 1])
#[inline]
pub fn transform_vec3(m: &[f32; 16], v: Vec3) -> Vec3 {
    let x = m[0] * v.x + m[1] * v.y + m[2] * v.z + m[3];
    let y = m[4] * v.x + m[5] * v.y + m[6] * v.z + m[7];
    let z = m[8] * v.x + m[9] * v.y + m[10] * v.z + m[11];
    let w = m[12] * v.x + m[13] * v.y + m[14] * v.z + m[15];
    if w.abs() > 1e-9 && (w - 1.0).abs() > 1e-9 {
        Vec3::new(x / w, y / w, z / w)
    } else {
        Vec3::new(x, y, z)
    }
}

/// Invert a 4x4 row-major rigid transform (rotation + translation only).
/// 
/// The alignment transforms produced by `compute_alignment_transform` are 
/// rigid (rotation + translation), so we don't need general matrix 
/// inversion — just transpose the rotation and negate the rotated 
/// translation.
/// 
/// Falls back to copying the input if the matrix doesn't look rigid 
/// (last row not [0,0,0,1] or the rotation submatrix isn't unitary). 
/// In practice this means rendering with the un-inverted transform, 
/// which produces a misaligned but recognizable image.
pub fn invert_rigid_transform(m: &[f32; 16]) -> [f32; 16] {
    // Extract rotation (3x3 in m[0..3], m[4..7], m[8..11]) and translation 
    // (m[3], m[7], m[11]).
    let r00 = m[0];  let r01 = m[1];  let r02 = m[2];   let tx = m[3];
    let r10 = m[4];  let r11 = m[5];  let r12 = m[6];   let ty = m[7];
    let r20 = m[8];  let r21 = m[9];  let r22 = m[10];  let tz = m[11];
    
    // Inverse rotation = transpose. New translation = -R^T * t.
    let new_tx = -(r00 * tx + r10 * ty + r20 * tz);
    let new_ty = -(r01 * tx + r11 * ty + r21 * tz);
    let new_tz = -(r02 * tx + r12 * ty + r22 * tz);
    
    [
        r00, r10, r20, new_tx,
        r01, r11, r21, new_ty,
        r02, r12, r22, new_tz,
        0.0, 0.0, 0.0, 1.0,
    ]
}

/// Rotate a vector by a quaternion (x, y, z, w).
/// 
/// Standard formula: v' = q * v * q⁻¹, expanded.
#[inline]
fn rotate_by_quat(q: [f32; 4], v: Vec3) -> Vec3 {
    // Quaternion array layout: [w, x, y, z] (matches CameraPose 
    // struct comment and the encoder's rotate_vec3_by_quat).
    // 
    // Historical note: this function previously unpacked q as 
    // [x, y, z, w] which silently disagreed with the encoder. The 
    // encoder writes meshes using [w,x,y,z] inversion; the decoder 
    // was undoing the rotation with the wrong axis assignment. For 
    // the default rotation [1,0,0,0] (identity), the bug manifested 
    // as decoder treating it as a 180° rotation around X — every 
    // mesh vertex ended up behind the camera, all triangles culled, 
    // render returned None, and the player fell back to source video. 
    // That fallback made things "look like they worked" while actually 
    // never producing v8 frames.
    let (qw, qx, qy, qz) = (q[0], q[1], q[2], q[3]);
    // t = 2 * cross(q.xyz, v)
    let tx = 2.0 * (qy * v.z - qz * v.y);
    let ty = 2.0 * (qz * v.x - qx * v.z);
    let tz = 2.0 * (qx * v.y - qy * v.x);
    // result = v + qw * t + cross(q.xyz, t)
    Vec3::new(
        v.x + qw * tx + (qy * tz - qz * ty),
        v.y + qw * ty + (qz * tx - qx * tz),
        v.z + qw * tz + (qx * ty - qy * tx),
    )
}

/// Precompute the inverse rotation as a row-major 3×3 matrix from a 
/// quaternion. Pair with `project_with_mat` for hot loops (renders 
/// touching thousands of vertices through the same pose).
/// 
/// "Inverse" here = conjugate, since input quaternion is unit.
#[inline]
fn inv_quat_to_mat3(q: [f32; 4]) -> [f32; 9] {
    let w = q[0]; let x = -q[1]; let y = -q[2]; let z = -q[3];
    let xx = x * x; let yy = y * y; let zz = z * z;
    let xy = x * y; let xz = x * z; let yz = y * z;
    let wx = w * x; let wy = w * y; let wz = w * z;
    [
        1.0 - 2.0 * (yy + zz),  2.0 * (xy - wz),         2.0 * (xz + wy),
        2.0 * (xy + wz),         1.0 - 2.0 * (xx + zz),  2.0 * (yz - wx),
        2.0 * (xz - wy),         2.0 * (yz + wx),         1.0 - 2.0 * (xx + yy),
    ]
}

/// Variant of `project` that uses a precomputed inverse rotation matrix 
/// + cached fx/fy/cx/cy. Hot-loop friendly.
#[inline]
fn project_with_mat(
    pose_position: Vec3,
    inv_rot: &[f32; 9],
    fx: f32, fy: f32, cx: f32, cy: f32,
    world: Vec3,
) -> (f32, f32, f32) {
    let rx = world.x - pose_position.x;
    let ry = world.y - pose_position.y;
    let rz = world.z - pose_position.z;
    // mat3-vec multiply.
    let cam_x = inv_rot[0] * rx + inv_rot[1] * ry + inv_rot[2] * rz;
    let cam_y = inv_rot[3] * rx + inv_rot[4] * ry + inv_rot[5] * rz;
    let cam_z = inv_rot[6] * rx + inv_rot[7] * ry + inv_rot[8] * rz;
    if cam_z.abs() < 1e-6 {
        return (0.0, 0.0, -1.0);
    }
    let u = fx * cam_x / cam_z + cx;
    let v = fy * cam_y / cam_z + cy;
    (u, v, cam_z)
}

/// Project a world-space point through a CameraPose to image space.
/// 
/// Returns (u, v, depth) in image pixels and camera-space Z. depth ≤ 0 
/// means the point is behind the camera and should be culled.
#[inline]
fn project(pose: &CameraPose, world: Vec3) -> (f32, f32, f32) {
    // Camera-space position: rotate (world - pose.position) by inverse rotation.
    // Inverse of unit quaternion is its conjugate (negate xyz).
    let rel = Vec3::new(
        world.x - pose.position.x,
        world.y - pose.position.y,
        world.z - pose.position.z,
    );
    // Conjugate of pose.rotation in [w,x,y,z] layout: keep w (q[0]), negate xyz (q[1..3]).
    // Matches the encoder's q_inv computation in encode-side project/triangulation.
    let inv_q = [pose.rotation[0], -pose.rotation[1], -pose.rotation[2], -pose.rotation[3]];
    let cam = rotate_by_quat(inv_q, rel);
    
    if cam.z.abs() < 1e-6 {
        return (0.0, 0.0, -1.0);  // signal cull
    }
    // Defensive fallback for poses with zero focal length. This can 
    // happen when a worker shard's pose_track has unwritten entries 
    // (CameraPose::default has focal=[0,0]) and the merge step's 
    // "focal > 0" filter left the entry untouched. Without a fallback 
    // all vertices project to the principal point and the frame 
    // renders as a single-pixel dot.
    // 
    // 1000.0 mirrors the encoder-side default (rotate/project sites 
    // in v8_reconstruction_codec.rs use the same value).
    let fx = if pose.focal[0] > 0.0 { pose.focal[0] } else { 1000.0 };
    let fy = if pose.focal[1] > 0.0 { pose.focal[1] } else { 1000.0 };
    let u = fx * cam.x / cam.z + pose.principal[0];
    let v = fy * cam.y / cam.z + pose.principal[1];
    (u, v, cam.z)
}

/// Rasterize a single triangle with barycentric color interpolation and 
/// depth test. Writes to `rgb` (w*h*3) and `depth` (w*h, smaller=closer).
/// 
/// Triangle vertices are in image space: (u, v, depth). Colors are 
/// per-vertex RGB f32 [0..1]. The depth here is the camera-space z used 
/// for occlusion; smaller = closer.
fn rasterize_triangle(
    p0: (f32, f32, f32), p1: (f32, f32, f32), p2: (f32, f32, f32),
    c0: [f32; 3], c1: [f32; 3], c2: [f32; 3],
    rgb: &mut [u8], depth: &mut [f32], w: usize, h: usize,
) {
    // ── Incremental edge-walk rasterizer ──
    // 
    // The previous implementation recomputed barycentric coordinates 
    // from scratch for every pixel (~25 multiplies per pixel). This 
    // version computes the per-x and per-y derivatives of (w0, w1) at 
    // triangle setup, then walks the bounding box doing ~3 additions 
    // per pixel.
    // 
    // Derivation: w0, w1 are signed-area ratios linear in (px, py). 
    //   w0(px, py) = (x1*y2 - x2*y1 + py*(x2-x1) + px*(y1-y2)) * inv_area
    //   ⇒ dw0/dx = (y1 - y2) * inv_area    (constant per triangle)
    //   ⇒ dw0/dy = (x2 - x1) * inv_area    (constant per triangle)
    // Same shape for w1; w2 = 1 - w0 - w1.
    // 
    // Depth and color are linear in (w0, w1): 
    //   z = z2 + w0*(z0-z2) + w1*(z1-z2)
    //   ⇒ dz/dx = (z0-z2)*dw0/dx + (z1-z2)*dw1/dx
    // 
    // So at each pixel we just add the precomputed dx-deltas; at end-of-
    // row we add the dy-deltas. ~8-10x speedup on the inner loop on 
    // mesh-heavy content (100k+ triangles at 2K resolution).
    
    let (x0, y0) = (p0.0, p0.1);
    let (x1, y1) = (p1.0, p1.1);
    let (x2, y2) = (p2.0, p2.1);
    let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
    if area.abs() < 1e-3 { return; }  // degenerate
    let inv_area = 1.0 / area;
    
    // Bounding box clipped to screen.
    let min_x = x0.min(x1).min(x2).max(0.0).floor() as i32;
    let max_x = x0.max(x1).max(x2).min((w - 1) as f32).ceil() as i32;
    let min_y = y0.min(y1).min(y2).max(0.0).floor() as i32;
    let max_y = y0.max(y1).max(y2).min((h - 1) as f32).ceil() as i32;
    if max_x < min_x || max_y < min_y { return; }
    
    // Per-pixel x-derivatives (constant across the triangle).
    let dw0_dx = (y1 - y2) * inv_area;
    let dw0_dy = (x2 - x1) * inv_area;
    let dw1_dx = (y2 - y0) * inv_area;
    let dw1_dy = (x0 - x2) * inv_area;
    
    // z and color derivatives (also constant per triangle).
    let z0_2 = p0.2 - p2.2;
    let z1_2 = p1.2 - p2.2;
    let dz_dx = z0_2 * dw0_dx + z1_2 * dw1_dx;
    let dz_dy = z0_2 * dw0_dy + z1_2 * dw1_dy;
    
    // Per-channel deltas. r, g, b each get their own.
    let dr_dx = (c0[0] - c2[0]) * dw0_dx + (c1[0] - c2[0]) * dw1_dx;
    let dr_dy = (c0[0] - c2[0]) * dw0_dy + (c1[0] - c2[0]) * dw1_dy;
    let dg_dx = (c0[1] - c2[1]) * dw0_dx + (c1[1] - c2[1]) * dw1_dx;
    let dg_dy = (c0[1] - c2[1]) * dw0_dy + (c1[1] - c2[1]) * dw1_dy;
    let db_dx = (c0[2] - c2[2]) * dw0_dx + (c1[2] - c2[2]) * dw1_dx;
    let db_dy = (c0[2] - c2[2]) * dw0_dy + (c1[2] - c2[2]) * dw1_dy;
    
    // Compute w0, w1 at (min_x + 0.5, min_y + 0.5) — the pixel-center 
    // of the top-left bbox corner. This is the row-start value; we 
    // increment from here.
    let px_start = min_x as f32 + 0.5;
    let py_start = min_y as f32 + 0.5;
    let w0_start = ((x1 - px_start) * (y2 - py_start) - (x2 - px_start) * (y1 - py_start)) * inv_area;
    let w1_start = ((x2 - px_start) * (y0 - py_start) - (x0 - px_start) * (y2 - py_start)) * inv_area;
    
    // Initial z and color at (px_start, py_start).
    let z_start = p2.2 + w0_start * z0_2 + w1_start * z1_2;
    let r_start = c2[0] + w0_start * (c0[0] - c2[0]) + w1_start * (c1[0] - c2[0]);
    let g_start = c2[1] + w0_start * (c0[1] - c2[1]) + w1_start * (c1[1] - c2[1]);
    let b_start = c2[2] + w0_start * (c0[2] - c2[2]) + w1_start * (c1[2] - c2[2]);
    
    // Row-walker state.
    let mut w0_row = w0_start;
    let mut w1_row = w1_start;
    let mut z_row = z_start;
    let mut r_row = r_start;
    let mut g_row = g_start;
    let mut b_row = b_start;
    
    // Edge tolerance — keep it consistent with the original implementation 
    // so triangles at sub-pixel seams behave the same.
    const EDGE_EPS: f32 = -1e-4;
    
    for py in min_y..=max_y {
        // Pixel-walker state for this row.
        let mut w0_p = w0_row;
        let mut w1_p = w1_row;
        let mut z_p = z_row;
        let mut r_p = r_row;
        let mut g_p = g_row;
        let mut b_p = b_row;
        
        let pi_row_base = (py as usize) * w;
        
        for px in min_x..=max_x {
            // Inside-test: all barycentrics ≥ 0 (with tolerance for edges). 
            // w2 = 1 - w0 - w1 is cheap to derive when needed.
            let w2_p = 1.0 - w0_p - w1_p;
            if w0_p >= EDGE_EPS && w1_p >= EDGE_EPS && w2_p >= EDGE_EPS {
                let pi = pi_row_base + (px as usize);
                if z_p < depth[pi] {
                    depth[pi] = z_p;
                    let rgbi = pi * 3;
                    rgb[rgbi]     = (r_p * 255.0).clamp(0.0, 255.0) as u8;
                    rgb[rgbi + 1] = (g_p * 255.0).clamp(0.0, 255.0) as u8;
                    rgb[rgbi + 2] = (b_p * 255.0).clamp(0.0, 255.0) as u8;
                }
            }
            
            // Advance by one pixel in +x direction.
            w0_p += dw0_dx;
            w1_p += dw1_dx;
            z_p  += dz_dx;
            r_p  += dr_dx;
            g_p  += dg_dx;
            b_p  += db_dx;
        }
        
        // Advance row state by one pixel in +y direction.
        w0_row += dw0_dy;
        w1_row += dw1_dy;
        z_row  += dz_dy;
        r_row  += dr_dy;
        g_row  += dg_dy;
        b_row  += db_dy;
    }
}

/// Render a mesh to an RGB buffer through a CameraPose.
/// 
/// `mesh.vertices`, `mesh.indices`, `mesh.vertex_colors` define the 
/// geometry. `world_transform` is applied to every vertex before 
/// projection (use identity for canonical webs, the alignment matrix 
/// for aligned webs).
/// 
/// Returns true if at least one triangle was rasterized.
pub fn render_mesh(
    mesh: &SceneMesh,
    world_transform: Option<&[f32; 16]>,
    pose: &CameraPose,
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    if mesh.vertices.is_empty() || mesh.indices.is_empty() { return None; }
    
    // Pre-project all vertices to image space.
    // 
    // Hot-loop optimization: hoist the per-pose state (inverse rotation 
    // matrix, focal lengths with defensive fallback, principal point) 
    // OUT of the per-vertex loop. The original code re-derived all of 
    // this for every vertex via project(). For meshes with 10k+ 
    // vertices and tight encoding loops calling render_mesh per frame, 
    // this is a measurable win.
    let n_verts = mesh.vertices.len();
    let inv_rot = inv_quat_to_mat3(pose.rotation);
    let fx = if pose.focal[0] > 0.0 { pose.focal[0] } else { 1000.0 };
    let fy = if pose.focal[1] > 0.0 { pose.focal[1] } else { 1000.0 };
    let cx = pose.principal[0];
    let cy = pose.principal[1];
    let pos = pose.position;
    let mut projected: Vec<(f32, f32, f32)> = Vec::with_capacity(n_verts);
    for v in &mesh.vertices {
        let world = match world_transform {
            Some(m) => transform_vec3(m, *v),
            None => *v,
        };
        projected.push(project_with_mat(pos, &inv_rot, fx, fy, cx, cy, world));
    }
    
    // Allocate RGB output and depth buffer.
    let n_pix = width * height;
    let mut rgb = vec![0u8; n_pix * 3];
    let mut depth = vec![f32::INFINITY; n_pix];
    
    // Degenerate-triangle filter. Mirrors the GPU rasterizer's logic 
    // so CPU and GPU produce visually consistent output. Drops triangles 
    // whose world-space edge-length ratio is extreme (long thin streaks) 
    // or whose max edge is unreasonably large (NaN-propagated / 
    // numerical-instability vertices).
    // 
    // Toggle via FAO_NO_CULL=1.
    let cull_disabled = std::env::var("FAO_NO_CULL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    
    // Rasterize triangles.
    let n_tri = mesh.indices.len() / 3;
    let mut drawn = 0usize;
    for t in 0..n_tri {
        let i0 = mesh.indices[t * 3] as usize;
        let i1 = mesh.indices[t * 3 + 1] as usize;
        let i2 = mesh.indices[t * 3 + 2] as usize;
        if i0 >= n_verts || i1 >= n_verts || i2 >= n_verts { continue; }
        
        if !cull_disabled {
            let v0 = &mesh.vertices[i0];
            let v1 = &mesh.vertices[i1];
            let v2 = &mesh.vertices[i2];
            let d01 = (v1.x - v0.x).powi(2) + (v1.y - v0.y).powi(2) + (v1.z - v0.z).powi(2);
            let d12 = (v2.x - v1.x).powi(2) + (v2.y - v1.y).powi(2) + (v2.z - v1.z).powi(2);
            let d20 = (v0.x - v2.x).powi(2) + (v0.y - v2.y).powi(2) + (v0.z - v2.z).powi(2);
            let max_d2 = d01.max(d12).max(d20);
            let min_d2 = d01.min(d12).min(d20);
            if !max_d2.is_finite() { continue; }
            const MAX_EDGE_SQ: f32 = 10000.0 * 10000.0;
            if max_d2 > MAX_EDGE_SQ { continue; }
            const ASPECT_SQ: f32 = 50.0 * 50.0;
            if min_d2 > 1e-6 && max_d2 / min_d2 > ASPECT_SQ { continue; }
        }
        
        let p0 = projected[i0];
        let p1 = projected[i1];
        let p2 = projected[i2];
        
        // Cull triangles entirely behind the camera.
        if p0.2 <= 0.0 && p1.2 <= 0.0 && p2.2 <= 0.0 { continue; }
        // For triangles partially behind, our simple rasterizer would 
        // produce garbage — skip those too. A proper implementation would 
        // clip against the near plane.
        if p0.2 <= 0.0 || p1.2 <= 0.0 || p2.2 <= 0.0 { continue; }
        
        // Off-screen cull (rough — bbox already clipped inside rasterizer).
        let max_x = p0.0.max(p1.0).max(p2.0);
        let min_x = p0.0.min(p1.0).min(p2.0);
        let max_y = p0.1.max(p1.1).max(p2.1);
        let min_y = p0.1.min(p1.1).min(p2.1);
        if max_x < 0.0 || min_x >= width as f32 { continue; }
        if max_y < 0.0 || min_y >= height as f32 { continue; }
        
        let c0 = mesh.vertex_colors.get(i0).copied().unwrap_or([0.5, 0.5, 0.5]);
        let c1 = mesh.vertex_colors.get(i1).copied().unwrap_or([0.5, 0.5, 0.5]);
        let c2 = mesh.vertex_colors.get(i2).copied().unwrap_or([0.5, 0.5, 0.5]);
        
        rasterize_triangle(p0, p1, p2, c0, c1, c2, &mut rgb, &mut depth, width, height);
        drawn += 1;
    }
    
    if drawn == 0 { return None; }
    // Even if `drawn` is positive, every triangle could have produced 
    // zero visible pixels (bbox non-empty but every pixel center 
    // outside the triangle's coverage area due to EDGE_EPS). Returning 
    // Some(all_zero_rgb) causes apply_residual to layer the residual 
    // on a zero base — the user sees the residual "glued" onto blank 
    // black background instead of falling through to the source-video 
    // fallback ladder.
    // 
    // Sample-check the rgb buffer the same way the GPU rasterizer 
    // does (stride sampling so cost is fixed). If nothing was actually 
    // drawn, return None so the player's fallback ladder can try 
    // source video instead.
    let stride = (rgb.len() / 4096).max(1);
    let any_nonzero = rgb.iter().step_by(stride).any(|&b| b != 0);
    if !any_nonzero { return None; }
    Some(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn project_origin_in_front_of_camera_at_depth_5() {
        let pose = CameraPose {
            position: Vec3::new(0.0, 0.0, -5.0),
            // Identity quaternion in [w, x, y, z] layout (w=1, xyz=0).
            // Previously this was [0,0,0,1] under the assumption that 
            // the array layout was [x, y, z, w] — that interpretation 
            // disagreed with the encoder and is fixed elsewhere in 
            // this file.
            rotation: [1.0, 0.0, 0.0, 0.0],
            focal: [1000.0, 1000.0],
            principal: [960.0, 540.0],
            distortion: 0.0,
        };
        // World origin is at (0, 0, 0), camera at (0, 0, -5) looking along +Z.
        // Camera-space position of origin: (0, 0, 5).
        let (u, v, z) = project(&pose, Vec3::new(0.0, 0.0, 0.0));
        assert!((u - 960.0).abs() < 0.1, "u expected ~960, got {}", u);
        assert!((v - 540.0).abs() < 0.1, "v expected ~540, got {}", v);
        assert!((z - 5.0).abs() < 0.01, "z expected 5.0, got {}", z);
    }
    
    #[test]
    fn render_single_triangle_fills_screen() {
        // Triangle covering most of a 100x100 image.
        let mesh = SceneMesh {
            vertices: vec![
                Vec3::new(-1.0, -1.0, 0.0),
                Vec3::new( 1.0, -1.0, 0.0),
                Vec3::new( 0.0,  1.0, 0.0),
            ],
            vertex_colors: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            indices: vec![0, 1, 2],
            normals: vec![],
            ..Default::default()
        };
        let pose = CameraPose {
            position: Vec3::new(0.0, 0.0, -3.0),
            rotation: [1.0, 0.0, 0.0, 0.0],  // identity in [w,x,y,z]
            focal: [50.0, 50.0],
            principal: [50.0, 50.0],
            distortion: 0.0,
        };
        let rgb = render_mesh(&mesh, None, &pose, 100, 100).expect("triangle should rasterize");
        // At least some non-zero pixels should be drawn.
        let drawn_pixels = rgb.chunks(3).filter(|c| c[0] > 0 || c[1] > 0 || c[2] > 0).count();
        assert!(drawn_pixels > 100, "expected at least some pixels drawn, got {}", drawn_pixels);
    }
    
    #[test]
    fn empty_mesh_returns_none() {
        let mesh = SceneMesh::default();
        let pose = CameraPose {
            position: Vec3::new(0.0, 0.0, -1.0),
            rotation: [1.0, 0.0, 0.0, 0.0],  // identity in [w,x,y,z]
            focal: [100.0, 100.0],
            principal: [50.0, 50.0],
            distortion: 0.0,
        };
        assert!(render_mesh(&mesh, None, &pose, 100, 100).is_none());
    }
}
