//! # mesh_pack — keeping the 3D world model from eating the file
//!
//! The measurement that motivates this module: on the w1 store GOP the
//! dense canonical serialized to **230 KB against a 71.8 KB 2D plate**,
//! for *worse* prediction. A world model that costs 3× the model it
//! replaces has to earn that back in reuse, and every byte here moves
//! the break-even take count. Asset size is the 3D path's real
//! constraint, not render quality.
//!
//! ## What this is
//!
//! A headless codec over `SceneMesh`: bytes in, bytes out, no renderer
//! and no codec state. `encode` returns the wire bytes AND the mesh the
//! decoder will reconstruct, so callers render from the DECODED mesh
//! and the measured prediction quality reflects the quantisation
//! honestly (the same "adopt the decoded surface" contract the plate
//! and the sprite atlas use).
//!
//! ## Why quantisation, specifically
//!
//! `games/blackstar-rs/docs/FLEET_COMPRESSION.md` measured all three
//! parts-web techniques on dense float32 vertex buffers: content dedup
//! folded 0 of 1289, a shared dictionary gained nothing over per-file,
//! and **16-bit quantisation was the whole win — 936 MB to ~205 MB at
//! 0.0008% round-trip error**. Sprite atlasing tried to borrow the
//! solid-window half of that lesson and failed (film cutouts repeat
//! nothing exactly). This module borrows the half that actually
//! measured: canonical meshes are the same data shape those numbers
//! came from, dense float32 positions, so the technique should transfer
//! where the other did not.
//!
//! FLEET_COMPRESSION's correctness caveat transfers too, and matters
//! more here than it did there: **one box for the whole model, never
//! per-part**. Quantising sub-meshes in their own boxes maps identical
//! world positions to different integers, which warps anything that
//! interpolates between them — there it was the fold, here it would be
//! cross-anchor vertex fusion and any future lineage delta between
//! canonicals. One AABB per mesh, stated on the wire.
//!
//! ## Layout
//!
//! ```text
//! u32 n_verts, u32 n_tris, u8 pos_bits, u8 flags
//! f32 lo[3], f32 hi[3]                      one box, whole mesh
//! zstd-19 over the concatenated PLANES:
//!   qx[n] qy[n] qz[n]      positions, plane-split, LE per pos_bits
//!   r[n] g[n] b[n]         colours, plane-split
//!   i0[m] i1[m] i2[m]      indices, plane-split, zigzag varint deltas
//! ```
//!
//! Plane-splitting is the band-major lesson from the residual coder
//! applied to geometry: a stream of one component has a homogeneous
//! local distribution, an interleaved xyzxyz stream does not, and zstd
//! bites on the former. Indices delta-code because a depth-grid mesh
//! walks its lattice in order — consecutive corners differ by small
//! constants, and the deltas collapse to a few repeated symbols.

use crate::geometry::Vec3;
use crate::v8_reconstruction_codec::SceneMesh;

#[derive(Clone)]
pub struct MeshPackOpts {
    /// Bits per position component (8–16). The RD knob: fewer bits is a
    /// smaller asset and a coarser world. Sweep it, don't guess it.
    pub pos_bits: u8,
    /// Drop vertex colours (the renderer then shades flat). Colours are
    /// ~20% of the raw payload; worth having as a switch for geometry-
    /// only canonicals that a per-take texture will re-colour anyway.
    pub keep_colors: bool,
}

impl Default for MeshPackOpts {
    fn default() -> Self {
        // 16 bits is FLEET_COMPRESSION's measured operating point and a
        // safe default; the sweep lives in the caller.
        MeshPackOpts { pos_bits: 16, keep_colors: true }
    }
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(data: &[u8], p: &mut usize) -> Option<u64> {
    let (mut v, mut shift) = (0u64, 0u32);
    loop {
        let b = *data.get(*p)?;
        *p += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// Encode a canonical mesh. Returns (wire bytes, mesh as the decoder
/// will see it) — render from the second, never from the input, or the
/// measurement lies about what the quantisation cost.
pub fn encode(mesh: &SceneMesh, opts: &MeshPackOpts) -> (Vec<u8>, SceneMesh) {
    let n = mesh.vertices.len();
    let m = mesh.indices.len() / 3;
    let bits = opts.pos_bits.clamp(8, 16) as u32;
    let levels = ((1u32 << bits) - 1) as f32;
    let bytes_per = if bits > 8 { 2usize } else { 1 };

    // ONE box for the whole mesh (see module docs — per-part boxes warp
    // anything that interpolates between parts).
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in &mesh.vertices {
        for (i, c) in [v.x, v.y, v.z].iter().enumerate() {
            if c.is_finite() {
                lo[i] = lo[i].min(*c);
                hi[i] = hi[i].max(*c);
            }
        }
    }
    for i in 0..3 {
        if !lo[i].is_finite() || !hi[i].is_finite() {
            lo[i] = 0.0;
            hi[i] = 0.0;
        }
        if hi[i] - lo[i] < 1e-9 {
            hi[i] = lo[i] + 1e-9;
        }
    }

    let mut planes: Vec<u8> = Vec::with_capacity(n * 9 + m * 6);
    let mut q: Vec<[u32; 3]> = Vec::with_capacity(n);
    for v in &mesh.vertices {
        let c = [v.x, v.y, v.z];
        let mut qq = [0u32; 3];
        for i in 0..3 {
            let t = if c[i].is_finite() {
                ((c[i] - lo[i]) / (hi[i] - lo[i])).clamp(0.0, 1.0)
            } else {
                0.0
            };
            qq[i] = (t * levels).round() as u32;
        }
        q.push(qq);
    }
    for axis in 0..3 {
        for qq in &q {
            if bytes_per == 2 {
                planes.extend_from_slice(&(qq[axis] as u16).to_le_bytes());
            } else {
                planes.push(qq[axis] as u8);
            }
        }
    }
    if opts.keep_colors {
        for ch in 0..3 {
            for c in &mesh.vertex_colors {
                planes.push((c[ch] * 255.0).round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    // Indices: plane-split by corner, zigzag varint deltas within each
    // plane. A depth-grid mesh emits corners in lattice order, so these
    // deltas are a handful of repeated small numbers.
    for corner in 0..3 {
        let mut prev = 0i64;
        for t in 0..m {
            let cur = mesh.indices[t * 3 + corner] as i64;
            put_varint(&mut planes, zigzag(cur - prev));
            prev = cur;
        }
    }

    let payload = zstd::encode_all(planes.as_slice(), 19).unwrap_or(planes);
    let mut out = Vec::with_capacity(payload.len() + 34);
    out.extend_from_slice(&(n as u32).to_le_bytes());
    out.extend_from_slice(&(m as u32).to_le_bytes());
    out.push(bits as u8);
    out.push(if opts.keep_colors { 1 } else { 0 });
    for i in 0..3 {
        out.extend_from_slice(&lo[i].to_le_bytes());
    }
    for i in 0..3 {
        out.extend_from_slice(&hi[i].to_le_bytes());
    }
    out.extend_from_slice(&payload);

    // Reconstruct exactly what decode() will produce, so encoder-side
    // renders match the decoder by construction.
    let dec = decode(&out).unwrap_or_else(SceneMesh::default);
    (out, dec)
}

pub fn decode(data: &[u8]) -> Option<SceneMesh> {
    if data.len() < 34 {
        return None;
    }
    let n = u32::from_le_bytes(data[0..4].try_into().ok()?) as usize;
    let m = u32::from_le_bytes(data[4..8].try_into().ok()?) as usize;
    let bits = data[8] as u32;
    let keep_colors = data[9] == 1;
    let mut lo = [0f32; 3];
    let mut hi = [0f32; 3];
    for i in 0..3 {
        lo[i] = f32::from_le_bytes(data[10 + i * 4..14 + i * 4].try_into().ok()?);
        hi[i] = f32::from_le_bytes(data[22 + i * 4..26 + i * 4].try_into().ok()?);
    }
    let planes = zstd::decode_all(&data[34..]).ok()?;
    let bytes_per = if bits > 8 { 2usize } else { 1 };
    let levels = ((1u64 << bits) - 1) as f32;

    let mut mesh = SceneMesh::default();
    mesh.vertices = vec![Vec3 { x: 0.0, y: 0.0, z: 0.0 }; n];
    let mut p = 0usize;
    for axis in 0..3 {
        for vi in 0..n {
            let qv = if bytes_per == 2 {
                u16::from_le_bytes(planes.get(p..p + 2)?.try_into().ok()?) as u32
            } else {
                *planes.get(p)? as u32
            };
            p += bytes_per;
            let val = lo[axis] + (qv as f32 / levels) * (hi[axis] - lo[axis]);
            match axis {
                0 => mesh.vertices[vi].x = val,
                1 => mesh.vertices[vi].y = val,
                _ => mesh.vertices[vi].z = val,
            }
        }
    }
    if keep_colors {
        mesh.vertex_colors = vec![[0.0f32; 3]; n];
        for ch in 0..3 {
            for vi in 0..n {
                mesh.vertex_colors[vi][ch] = *planes.get(p)? as f32 / 255.0;
                p += 1;
            }
        }
    } else {
        mesh.vertex_colors = vec![[0.5f32; 3]; n];
    }
    let mut idx = vec![0u32; m * 3];
    for corner in 0..3 {
        let mut prev = 0i64;
        for t in 0..m {
            let d = unzigzag(get_varint(&planes, &mut p)?);
            prev += d;
            idx[t * 3 + corner] = prev.max(0) as u32;
        }
    }
    mesh.indices = idx;
    Some(mesh)
}

/// Max per-vertex position error against the original, in model units,
/// and as a fraction of the model's diagonal — the honesty metric
/// FLEET_COMPRESSION reports and the one that says whether a bit-depth
/// choice is safe.
pub fn quant_error(orig: &SceneMesh, dec: &SceneMesh) -> (f32, f32) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in &orig.vertices {
        for (i, c) in [v.x, v.y, v.z].iter().enumerate() {
            if c.is_finite() {
                lo[i] = lo[i].min(*c);
                hi[i] = hi[i].max(*c);
            }
        }
    }
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
    let mut max_e = 0.0f32;
    for (a, b) in orig.vertices.iter().zip(&dec.vertices) {
        let e = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt();
        if e.is_finite() && e > max_e {
            max_e = e;
        }
    }
    (max_e, if diag > 0.0 { max_e / diag } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_mesh(nx: usize, ny: usize) -> SceneMesh {
        let mut m = SceneMesh::default();
        for y in 0..ny {
            for x in 0..nx {
                m.vertices.push(Vec3 {
                    x: x as f32 * 1.5 - 10.0,
                    y: y as f32 * 2.25 + 3.0,
                    z: 100.0 + ((x * 7 + y * 13) % 11) as f32,
                });
                m.vertex_colors.push([
                    (x % 8) as f32 / 8.0,
                    (y % 5) as f32 / 5.0,
                    ((x + y) % 3) as f32 / 3.0,
                ]);
            }
        }
        for y in 0..ny - 1 {
            for x in 0..nx - 1 {
                let i = (y * nx + x) as u32;
                m.indices.extend_from_slice(&[i, i + 1, i + nx as u32]);
                m.indices.extend_from_slice(&[i + 1, i + nx as u32 + 1, i + nx as u32]);
            }
        }
        m
    }

    /// Round-trip must preserve topology exactly and positions to within
    /// the quantiser's own step — a mesh codec that silently drops
    /// triangles would show up as render holes, not as an error.
    #[test]
    fn roundtrip_preserves_topology_and_bounds_error() {
        let orig = grid_mesh(40, 30);
        let (wire, dec) = encode(&orig, &MeshPackOpts::default());
        assert_eq!(dec.vertices.len(), orig.vertices.len());
        assert_eq!(dec.indices, orig.indices, "topology must be exact");
        assert_eq!(dec.vertex_colors.len(), orig.vertex_colors.len());
        let (abs_e, frac) = quant_error(&orig, &dec);
        assert!(frac < 1e-4, "16-bit error {frac} of diagonal (abs {abs_e})");
        // And it must actually be smaller than the naive form the
        // render3d bridge shipped first (f32 xyz + u8 rgb + u32 idx,
        // zstd'd whole).
        let mut naive = Vec::new();
        for v in &orig.vertices {
            naive.extend_from_slice(&v.x.to_le_bytes());
            naive.extend_from_slice(&v.y.to_le_bytes());
            naive.extend_from_slice(&v.z.to_le_bytes());
        }
        for c in &orig.vertex_colors {
            for ch in c {
                naive.push((ch * 255.0).round() as u8);
            }
        }
        for i in &orig.indices {
            naive.extend_from_slice(&i.to_le_bytes());
        }
        let naive_z = zstd::encode_all(naive.as_slice(), 19).unwrap().len();
        assert!(
            wire.len() < naive_z,
            "packed {} not smaller than naive {}",
            wire.len(),
            naive_z
        );
    }

    /// Fewer bits must cost bytes monotonically and stay decodable —
    /// this is the knob the caller sweeps, so it has to behave.
    #[test]
    fn bit_depth_is_a_monotone_rate_knob() {
        let orig = grid_mesh(30, 24);
        let mut last = usize::MAX;
        for bits in [16u8, 12, 10, 8] {
            let (wire, dec) = encode(
                &orig,
                &MeshPackOpts { pos_bits: bits, keep_colors: true },
            );
            assert_eq!(dec.indices, orig.indices);
            assert!(wire.len() <= last, "bits {bits} grew: {} > {last}", wire.len());
            last = wire.len();
        }
    }
}
