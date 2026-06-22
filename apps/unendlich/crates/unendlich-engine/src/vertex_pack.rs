//! Thin-vertex packing — the compaction that frees actual vertex *attribute
//! slots* (the scarce shader space). The current mesh vertex is 6 floats (24 B):
//! `position.xyz + color.rgb`. For the deferred/AAA path the fragment recomputes
//! albedo from `block_id` + procedural tint + the material table, so the vertex
//! no longer carries color at all — it carries geometry + identity:
//!
//! - **position**: chunk-local fixed-point, 16 bits/axis ([`POS_SCALE`] units),
//!   range 0..64 with ~1/2048-block error — visually exact and big enough for
//!   the chunk (0..32), its boundary faces (=32), and bumpy sub-vertices.
//! - **normal**: octahedral-encoded unit vector in 16 bits — handles arbitrary
//!   normals (bumpy tops, future normal-mapping), not just the 6 face axes.
//! - **block_id**: 8 bits (the material/tint key the fragment looks up).
//! - **ao**: 8 bits (baked ambient occlusion; SSAO can refine later).
//!
//! 10 bytes vs 24, and as attributes it's a `uvec3`-ish payload instead of two
//! `vec3`s. All pure and round-trip tested; the `.vert` decode mirrors this the
//! way the CPU mesher mirrors its shader. Lossy by design (quantization), so the
//! tests assert bounded error, not bit-exactness — except where it matters
//! (block_id/ao are exact).

/// Fixed-point fractional bits for packed positions (1 unit = 1/1024 block).
pub const POS_FRAC_BITS: u32 = 10;
/// Units per block. Position range is `0 .. 65536/POS_SCALE` ≈ `0..64`.
pub const POS_SCALE: f32 = (1u32 << POS_FRAC_BITS) as f32; // 1024.0

/// A packed mesh vertex: 10 bytes (vs 24 for `position+color` floats).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedVertex {
    /// Chunk-local position, fixed-point with [`POS_FRAC_BITS`] fractional bits.
    pub pos: [u16; 3],
    /// Octahedral-encoded unit normal (two 8-bit signed-ish components).
    pub normal_oct: u16,
    /// Block id — the fragment's key into the material/tint tables.
    pub block_id: u8,
    /// Baked ambient occlusion, 0 (dark) .. 255 (open).
    pub ao: u8,
}

/// Quantize one chunk-local coordinate (expected `0.0..64.0`) to fixed-point.
#[inline]
pub fn pack_coord(local: f32) -> u16 {
    let q = (local * POS_SCALE).round();
    q.clamp(0.0, u16::MAX as f32) as u16
}

#[inline]
pub fn unpack_coord(q: u16) -> f32 {
    q as f32 / POS_SCALE
}

/// Pack a chunk-local position (subtract the chunk origin before calling).
#[inline]
pub fn pack_position(local: [f32; 3]) -> [u16; 3] {
    [pack_coord(local[0]), pack_coord(local[1]), pack_coord(local[2])]
}

#[inline]
pub fn unpack_position(p: [u16; 3]) -> [f32; 3] {
    [unpack_coord(p[0]), unpack_coord(p[1]), unpack_coord(p[2])]
}

#[inline]
fn signf(x: f32) -> f32 {
    if x >= 0.0 { 1.0 } else { -1.0 }
}

/// Octahedral-encode a (near-)unit normal into a `u16` (two `u8` lanes).
/// Standard mapping: project onto the octahedron, fold the lower hemisphere,
/// quantize each component of `[-1,1]` to 8 bits.
pub fn oct_encode_normal(n: [f32; 3]) -> u16 {
    let inv = 1.0 / (n[0].abs() + n[1].abs() + n[2].abs()).max(1e-8);
    let (mut ox, mut oy) = (n[0] * inv, n[1] * inv);
    if n[2] < 0.0 {
        let tx = (1.0 - oy.abs()) * signf(ox);
        let ty = (1.0 - ox.abs()) * signf(oy);
        ox = tx;
        oy = ty;
    }
    let qx = (((ox * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0)) as u16;
    let qy = (((oy * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0)) as u16;
    qx | (qy << 8)
}

/// Decode an octahedral normal back to a unit vector.
pub fn oct_decode_normal(packed: u16) -> [f32; 3] {
    let qx = (packed & 0xFF) as f32;
    let qy = ((packed >> 8) & 0xFF) as f32;
    let ox = (qx / 255.0) * 2.0 - 1.0;
    let oy = (qy / 255.0) * 2.0 - 1.0;
    let mut n = [ox, oy, 1.0 - ox.abs() - oy.abs()];
    if n[2] < 0.0 {
        let tx = (1.0 - n[1].abs()) * signf(n[0]);
        let ty = (1.0 - n[0].abs()) * signf(n[1]);
        n[0] = tx;
        n[1] = ty;
    }
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-8);
    [n[0] / len, n[1] / len, n[2] / len]
}

/// Pack a full vertex. `local` is chunk-local position; `normal` should be
/// (near-)unit.
pub fn pack_vertex(local: [f32; 3], normal: [f32; 3], block_id: u8, ao: u8) -> PackedVertex {
    PackedVertex {
        pos: pack_position(local),
        normal_oct: oct_encode_normal(normal),
        block_id,
        ao,
    }
}

/// Unpack to `(local_position, normal, block_id, ao)`.
pub fn unpack_vertex(v: &PackedVertex) -> ([f32; 3], [f32; 3], u8, u8) {
    (unpack_position(v.pos), oct_decode_normal(v.normal_oct), v.block_id, v.ao)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    #[test]
    fn packed_vertex_is_much_smaller_than_float_vertex() {
        // 10 bytes vs the 24 of position+color floats.
        assert_eq!(std::mem::size_of::<PackedVertex>(), 10);
        assert!(std::mem::size_of::<PackedVertex>() * 2 < 24 + 1);
    }

    #[test]
    fn position_round_trips_within_quantization_error() {
        let tol = 1.0 / POS_SCALE; // one fixed-point step
        for &local in &[
            [0.0, 0.0, 0.0],
            [32.0, 32.0, 32.0],
            [15.5, 0.25, 31.0],
            [3.125, 28.75, 16.0],
            [12.3456, 7.89, 0.001],
        ] {
            let back = unpack_position(pack_position(local));
            for i in 0..3 {
                assert!((back[i] - local[i]).abs() <= tol, "axis {i}: {} vs {}", back[i], local[i]);
            }
        }
    }

    #[test]
    fn the_six_face_normals_round_trip_tightly() {
        // Face normals dominate the mesh; they must come back essentially exact
        // (oct encoding is lossy, so within a fraction of a degree, not bitwise).
        for &n in &[
            [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0], [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0], [0.0, 0.0, -1.0],
        ] {
            let d = oct_decode_normal(oct_encode_normal(n));
            assert!(dot(n, d) > 0.9995, "face normal {n:?} -> {d:?} (dot {})", dot(n, d));
            // Decoded normal stays unit-length.
            assert!((dot(d, d).sqrt() - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn arbitrary_normals_round_trip_within_a_couple_degrees() {
        let norm = |v: [f32; 3]| {
            let l = dot(v, v).sqrt();
            [v[0] / l, v[1] / l, v[2] / l]
        };
        for &raw in &[[1.0, 1.0, 1.0], [0.3, -0.8, 0.5], [-0.6, 0.2, -0.77], [0.1, 0.99, 0.05]] {
            let n = norm(raw);
            let d = oct_decode_normal(oct_encode_normal(n));
            // > 0.999 ≈ within ~2.5°, plenty for shading a bumpy top.
            assert!(dot(n, d) > 0.999, "{n:?} -> {d:?} (dot {})", dot(n, d));
        }
    }

    #[test]
    fn full_vertex_round_trips_ids_exactly_and_geometry_within_tol() {
        let v = pack_vertex([15.5, 8.25, 31.0], [0.0, 1.0, 0.0], 42, 200);
        let (pos, nrm, id, ao) = unpack_vertex(&v);
        assert_eq!(id, 42, "block_id is exact");
        assert_eq!(ao, 200, "ao is exact");
        for i in 0..3 {
            assert!((pos[i] - [15.5, 8.25, 31.0][i]).abs() <= 1.0 / POS_SCALE);
        }
        assert!(dot(nrm, [0.0, 1.0, 0.0]) > 0.9995);
    }
}
