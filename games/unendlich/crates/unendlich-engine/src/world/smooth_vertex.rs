//! SMOOTH VERTEX — the GPU format a surface-nets chunk needs, and why the
//! existing one cannot be it.
//!
//! `render::mesher`'s `PackedVertex` is eight bytes: a 10:10:10 position and an RGBA8 colour whose alpha carries a
//! material id. It is a good format for greedy meshing and it is unusable for
//! this path, for two independent reasons found by reading it rather than by
//! rendering:
//!
//! **1. It has no normal.** Greedy quads are axis-aligned, so the shader can
//! derive a face normal from the quad itself. A surface-nets vertex has an
//! arbitrary direction that is not recoverable from position, and the whole
//! reason `isosurface` computes an analytic gradient is that this direction is
//! better than anything the mesh can reconstruct. Dropping it here would throw
//! away the work and reintroduce the creasing at LOD boundaries that the
//! gradient exists to avoid.
//!
//! **2. Its range stops at the chunk.** `pack_pos` maps `[0, 32]` to
//! `[0, 1023]` and CLAMPS outside it. But a smooth chunk deliberately meshes
//! past its own bounds — `smooth_plan` gives the finer side of an LOD boundary
//! one coarse footprint of overlap, which moves `origin` negative on a low face
//! and extends `dims` past 32 on a high one. Packed with the existing
//! function, every overlap vertex silently collapses onto the chunk boundary:
//! the seam work would be undone at upload time, by a clamp, with no error
//! anywhere. `the_existing_packer_would_flatten_the_overlap` pins that down.
//!
//! ## What this uses instead
//!
//! Twelve bytes rather than eight. That is a 50% increase on a format whose
//! size was clearly deliberate, so it is worth saying what each byte buys:
//!
//! * **position, 10:10:10 over `[-8, 40]`** — the overlap range. The widest
//!   overlap is one cell of the coarsest neighbour, and `lod_cell_stride` tops
//!   out at 8, so 8 blocks either side covers every case the planner can
//!   produce. Step is 48/1023 ≈ 0.047 blocks, still finer than the
//!   extractor's own ~0.2-cell accuracy, so quantisation is not the limit.
//! * **normal, octahedral 16:16** — a unit vector in 32 bits with worst-case
//!   error well under a degree. Cheaper than three floats and decoded in a few
//!   shader instructions.
//! * **colour + material id, RGBA8** — unchanged from `PackedVertex`, so the
//!   existing texture-mapping seam applies without translation.

/// Blocks of overlap allowed either side of a chunk. `lod::lod_cell_stride`
/// tops out at 8 and the planner grants one neighbour cell, so 8 is exact
/// rather than generous.
pub const OVERLAP_BLOCKS: f32 = 8.0;
/// Chunk edge in blocks.
pub const CHUNK_EXTENT: f32 = 32.0;
/// Low end of the encodable range.
pub const POS_MIN: f32 = -OVERLAP_BLOCKS;
/// Span of the encodable range.
pub const POS_SPAN: f32 = CHUNK_EXTENT + 2.0 * OVERLAP_BLOCKS;

/// A vertex of a smooth chunk, as uploaded.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SmoothVertex {
    pub pos_packed: u32,
    pub norm_packed: u32,
    pub color_packed: u32,
}

impl SmoothVertex {
    pub const STRIDE: u32 = std::mem::size_of::<SmoothVertex>() as u32; // 12

    pub fn new(local: [f32; 3], normal: [f32; 3], color: [f32; 3], material: u8) -> SmoothVertex {
        SmoothVertex {
            pos_packed: pack_pos(local),
            norm_packed: pack_normal(normal),
            color_packed: pack_color_id(color, material),
        }
    }
}

/// Chunk-local position, in blocks, over `[-8, 40]` per axis.
#[inline]
pub fn pack_pos(local: [f32; 3]) -> u32 {
    let q = |v: f32| -> u32 {
        let t = ((v - POS_MIN) / POS_SPAN).clamp(0.0, 1.0);
        (t * 1023.0).round() as u32
    };
    q(local[0]) | (q(local[1]) << 10) | (q(local[2]) << 20)
}

#[inline]
pub fn unpack_pos(p: u32) -> [f32; 3] {
    let d = |bits: u32| (bits & 0x3FF) as f32 / 1023.0 * POS_SPAN + POS_MIN;
    [d(p), d(p >> 10), d(p >> 20)]
}

/// Octahedral encoding: project the unit sphere onto an octahedron, unfold it
/// to the unit square, store two 16-bit fixed-point coordinates. Uniform-ish
/// angular resolution, unlike storing two spherical angles, and it needs no
/// branches on the GPU to decode.
#[inline]
pub fn pack_normal(n: [f32; 3]) -> u32 {
    let l = (n[0].abs() + n[1].abs() + n[2].abs()).max(1e-20);
    let (mut x, mut y) = (n[0] / l, n[1] / l);
    if n[2] < 0.0 {
        // fold the lower hemisphere out across the diagonals
        let (ax, ay) = (x.abs(), y.abs());
        let (sx, sy) = (if x >= 0.0 { 1.0 } else { -1.0 }, if y >= 0.0 { 1.0 } else { -1.0 });
        x = (1.0 - ay) * sx;
        y = (1.0 - ax) * sy;
    }
    let q = |v: f32| -> u32 {
        let t = (v.clamp(-1.0, 1.0) * 0.5 + 0.5) * 65535.0;
        t.round() as u32 & 0xFFFF
    };
    q(x) | (q(y) << 16)
}

#[inline]
pub fn unpack_normal(p: u32) -> [f32; 3] {
    let d = |bits: u32| (bits & 0xFFFF) as f32 / 65535.0 * 2.0 - 1.0;
    let (mut x, mut y) = (d(p), d(p >> 16));
    let z = 1.0 - x.abs() - y.abs();
    if z < 0.0 {
        let (ax, ay) = (x.abs(), y.abs());
        let (sx, sy) = (if x >= 0.0 { 1.0 } else { -1.0 }, if y >= 0.0 { 1.0 } else { -1.0 });
        x = (1.0 - ay) * sx;
        y = (1.0 - ax) * sy;
    }
    let len = (x * x + y * y + z * z).sqrt().max(1e-20);
    [x / len, y / len, z / len]
}

/// RGBA8 with the material id in alpha — the same layout and the same seam as
/// `PackedVertex::pack_color_id`, so `world::texture_mapping` needs no
/// translation for this path.
#[inline]
pub fn pack_color_id(color: [f32; 3], id: u8) -> u32 {
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as u32) & 0xFF;
    q(color[0]) | (q(color[1]) << 8) | (q(color[2]) << 16) | ((id as u32) << 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `PackedVertex::pack_pos`, reproduced. The real one lives in
    /// `render::mesher`, which is behind `--features render-core` and is not
    /// compiled in a headless build — this module was originally written in
    /// `render/` and therefore was never compiled at all until it moved here,
    /// which is worth knowing before putting anything else in that tree.
    ///
    /// Kept as a copy so the incompatibility is asserted in every build rather
    /// than only in the one nobody runs. `mirrors_the_real_packer` guards the
    /// copy where the real type is available.
    const OLD_EXTENT: f32 = 32.0;
    fn old_pack_pos(local: [f32; 3]) -> u32 {
        let q = |v: f32| -> u32 {
            let t = (v / OLD_EXTENT).clamp(0.0, 1.0);
            (t * 1023.0).round() as u32
        };
        q(local[0]) | (q(local[1]) << 10) | (q(local[2]) << 20)
    }

    #[cfg(feature = "render-core")]
    #[test]
    fn mirrors_the_real_packer() {
        use crate::render::mesher::PackedVertex;
        for v in [[0.0f32, 0.0, 0.0], [1.5, 17.25, 31.9], [32.0, 0.0, 8.0]] {
            assert_eq!(old_pack_pos(v), PackedVertex::pack_pos(v), "copy drifted at {v:?}");
        }
    }

    #[test]
    fn the_existing_packer_would_flatten_the_overlap() {
        // THE REASON THIS MODULE EXISTS. smooth_plan hands the finer side of an
        // LOD boundary geometry outside its own chunk; PackedVertex::pack_pos
        // clamps to [0, 32], so that geometry would arrive on the boundary
        // plane instead — the seam work undone at upload, by a clamp, with
        // nothing reporting an error.
        let outside = [-4.0f32, 16.0, 36.0];
        let old = old_pack_pos(outside);
        // clamped: x collapses to 0 and z to 32
        let step = OLD_EXTENT / 1023.0;
        let x_back = (old & 0x3FF) as f32 * step;
        let z_back = ((old >> 20) & 0x3FF) as f32 * step;
        assert!(x_back.abs() < 1e-3, "x survived at {x_back}; the clamp may have changed");
        assert!(
            (z_back - OLD_EXTENT).abs() < 1e-2,
            "z survived at {z_back}"
        );

        // and this module keeps it
        let new = unpack_pos(pack_pos(outside));
        for i in 0..3 {
            assert!(
                (new[i] - outside[i]).abs() < 0.05,
                "axis {i}: {} -> {}",
                outside[i],
                new[i]
            );
        }
    }

    #[test]
    fn positions_round_trip_inside_the_quantisation_step() {
        let step = POS_SPAN / 1023.0;
        for v in [
            [0.0f32, 0.0, 0.0],
            [32.0, 32.0, 32.0],
            [-8.0, 40.0, 15.5],
            [1.0 / 3.0, 7.77, 31.999],
        ] {
            let back = unpack_pos(pack_pos(v));
            for i in 0..3 {
                assert!(
                    (back[i] - v[i]).abs() <= step,
                    "axis {i}: {} -> {} (step {step:.4})",
                    v[i],
                    back[i]
                );
            }
        }
    }

    #[test]
    fn the_step_is_finer_than_the_extractor_is_accurate() {
        // Quantisation must not be the limiting error. isosurface's own
        // measured bound is about 0.2 cells (a_sphere_comes_out_round).
        let step = POS_SPAN / 1023.0;
        assert!(step < 0.05, "position step {step:.4} blocks");
    }

    #[test]
    fn normals_round_trip_to_within_a_degree() {
        // Sweep the sphere rather than spot-check: octahedral encoding is
        // worst at the fold diagonals and a few axis-aligned samples would
        // miss it entirely.
        let mut worst: f32 = 1.0;
        let n = 40;
        for i in 0..n {
            for j in 0..n {
                let u = (i as f32 + 0.5) / n as f32 * std::f32::consts::PI;
                let v = (j as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
                let dir = [u.sin() * v.cos(), u.cos(), u.sin() * v.sin()];
                let back = unpack_normal(pack_normal(dir));
                let dot = dir[0] * back[0] + dir[1] * back[1] + dir[2] * back[2];
                worst = worst.min(dot);
            }
        }
        let deg = worst.clamp(-1.0, 1.0).acos().to_degrees();
        assert!(deg < 1.0, "worst normal error {deg:.3} degrees");
    }

    #[test]
    fn a_decoded_normal_is_always_unit_length() {
        for p in [0u32, 0xFFFF_FFFF, 0x8000_8000, 0x1234_5678, 0x0000_FFFF] {
            let n = unpack_normal(p);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "packed {p:#x} decoded to length {len}");
            assert!(n.iter().all(|c| c.is_finite()));
        }
    }

    #[test]
    fn the_material_id_rides_alpha_untouched() {
        // The existing texture-mapping seam depends on the id surviving in the
        // alpha byte exactly; a colour round-trip must not disturb it.
        for id in [0u8, 1, 127, 255] {
            let packed = pack_color_id([0.2, 0.9, 0.45], id);
            assert_eq!((packed >> 24) as u8, id);
        }
    }

    #[test]
    fn the_stride_is_what_a_pipeline_binding_will_say_it_is() {
        assert_eq!(SmoothVertex::STRIDE, 12);
        assert_eq!(std::mem::size_of::<SmoothVertex>(), 12, "padding crept in");
    }
}
