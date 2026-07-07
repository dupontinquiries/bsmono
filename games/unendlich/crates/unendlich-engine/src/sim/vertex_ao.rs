//! `sim::vertex_ao` — pure CPU reference for the mesher's per-vertex ambient
//! occlusion (the classic "3 neighbours per corner" voxel AO).
//!
//! Per `docs/sections/SECTION_rendering.md` roadmap item 3 and the plan in
//! `docs/shaders/AO_FINDINGS_r68.md` (Option A): at each emitted face vertex,
//! darken by how many of the 3 adjacent voxels in the air layer just outside the
//! face are solid — the two edge neighbours plus the diagonal corner. This is the
//! CPU reference the mesher + `chunk.vert/.frag` will consume; the coordinator
//! wires it into the vertex format and proves GPU parity.
//!
//! ## Convention (matches `sim::ao_gpu` for parity)
//! [`corner_ao`] returns a level in `0..=3` where **3 = unoccluded (brightest)**
//! and **0 = a concave corner (darkest)** — bit-identical to
//! `sim::ao_gpu::vertex_ao` (`ao_gpu.rs:47-53`). [`face_ao`] returns the four
//! corner levels in the same fixed order `ao_gpu`'s bake uses
//! (`ao_gpu.rs:60-66` / its private `face_ao` loop `ao_gpu.rs:128-141`):
//! `(sp1,sp2) = (-,-), (-,+), (+,-), (+,+)`, where `sp1` indexes the face's
//! first perpendicular axis `u` (→ `side1`) and `sp2` the second axis `v`
//! (→ `side2`). [`ao_factor`] maps a level to the `0.5..1.0` ambient multiplier
//! named in AO_FINDINGS (applied to the indirect term only).
//!
//! This module is **distinct** from `sim::ao_gpu` (which is the gpu-bench bake
//! over a `VoxelGrid`): here the input is a small grid-free [`FaceNeighbours`]
//! struct, and this module additionally supplies the **anisotropy flip rule**
//! ([`should_flip`]) that `ao_gpu` does not.
//!
//! Pure and deterministic: data in, data out. No render/Vulkan/winit, no IO, no
//! clock, no RNG, no `.unwrap()`/`.expect()`. Bounded outputs, no panics.

/// Per-vertex corner AO. `side1`/`side2` are the two edge-adjacent voxels and
/// `corner` the diagonal voxel, all sampled in the air layer just outside the
/// face. If **both** edge sides are solid the vertex is a fully-occluded concave
/// corner (level `0`, darkest) and the diagonal is irrelevant; otherwise each
/// solid neighbour drops the level by one from the unoccluded `3`. Range `0..=3`.
///
/// Mirrors `sim::ao_gpu::vertex_ao` exactly (the GPU-parity target).
#[inline]
pub fn corner_ao(side1: bool, side2: bool, corner: bool) -> u8 {
    if side1 && side2 {
        0
    } else {
        3 - (side1 as u8 + side2 as u8 + corner as u8)
    }
}

/// The eight occluder samples in the air-side plane of one face, in the face's
/// local `(u, v)` frame (the two axes perpendicular to the face normal). The
/// caller fills these from the voxel grid for the specific face; this keeps the
/// reference grid-free and trivially testable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FaceNeighbours {
    /// Edge occluder one step along `-u` / `+u` (feeds `side1`).
    pub u_neg: bool,
    pub u_pos: bool,
    /// Edge occluder one step along `-v` / `+v` (feeds `side2`).
    pub v_neg: bool,
    pub v_pos: bool,
    /// Diagonal occluders at the four `(±u, ±v)` corners.
    pub d_un_vn: bool, // (-u, -v)
    pub d_un_vp: bool, // (-u, +v)
    pub d_up_vn: bool, // (+u, -v)
    pub d_up_vp: bool, // (+u, +v)
}

/// AO levels for a quad's four corners, in the fixed order
/// `(-u,-v), (-u,+v), (+u,-v), (+u,+v)` (matching `ao_gpu`'s bake). Each entry
/// is in `0..=3`.
pub fn face_ao(n: &FaceNeighbours) -> [u8; 4] {
    [
        corner_ao(n.u_neg, n.v_neg, n.d_un_vn), // (-u, -v)
        corner_ao(n.u_neg, n.v_pos, n.d_un_vp), // (-u, +v)
        corner_ao(n.u_pos, n.v_neg, n.d_up_vn), // (+u, -v)
        corner_ao(n.u_pos, n.v_pos, n.d_up_vp), // (+u, +v)
    ]
}

/// Anisotropy flip rule. A quad is drawn as two triangles, so it must be split
/// along one of its two diagonals. With the [`face_ao`] corner order, the two
/// diagonals connect corners `{0,3}` (`(-,-)`–`(+,+)`) and `{1,2}`
/// (`(-,+)`–`(+,-)`). Triangulating along the **brighter** diagonal (greater AO
/// sum) confines a lone dark corner to a single triangle and removes the
/// directional light-leak ("anisotropy") artifact when opposite corners
/// disagree.
///
/// The default diagonal is `{0,3}`. Returns `true` when the renderer should flip
/// to the `{1,2}` diagonal instead — i.e. when `ao[1]+ao[2] > ao[0]+ao[3]`. On a
/// tie (the common all-equal / symmetric case) there is no artifact, so no flip.
#[inline]
pub fn should_flip(ao: [u8; 4]) -> bool {
    (ao[1] as u16 + ao[2] as u16) > (ao[0] as u16 + ao[3] as u16)
}

/// Map an AO level `0..=3` to the ambient brightness multiplier `0.5..1.0`
/// (AO_FINDINGS_r68 §Option A: `{0,1,2,3} → 0.5..1.0`, multiplied into the
/// *indirect* term only). Levels above 3 clamp to `1.0`.
#[inline]
pub fn ao_factor(level: u8) -> f32 {
    let l = if level > 3 { 3 } else { level };
    0.5 + 0.5 * (l as f32 / 3.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference formula `corner_ao` mirrors (`ao_gpu::vertex_ao`).
    fn expected(side1: bool, side2: bool, corner: bool) -> u8 {
        if side1 && side2 {
            0
        } else {
            3 - (side1 as u8 + side2 as u8 + corner as u8)
        }
    }

    #[test]
    fn corner_ao_canonical_cases() {
        // 0 sides, no diagonal → unoccluded, brightest.
        assert_eq!(corner_ao(false, false, false), 3);
        // exactly one occluder (either side, or just the diagonal) → 2.
        assert_eq!(corner_ao(true, false, false), 2);
        assert_eq!(corner_ao(false, true, false), 2);
        assert_eq!(corner_ao(false, false, true), 2);
        // one side + diagonal → 1.
        assert_eq!(corner_ao(true, false, true), 1);
        assert_eq!(corner_ao(false, true, true), 1);
        // both sides → fully-occluded concave corner, diagonal irrelevant → 0.
        assert_eq!(corner_ao(true, true, false), 0);
        assert_eq!(corner_ao(true, true, true), 0);
    }

    #[test]
    fn corner_ao_matches_parity_formula_and_range() {
        for &s1 in &[false, true] {
            for &s2 in &[false, true] {
                for &c in &[false, true] {
                    let v = corner_ao(s1, s2, c);
                    assert_eq!(v, expected(s1, s2, c), "parity with ao_gpu::vertex_ao");
                    assert!(v <= 3, "level in 0..=3");
                }
            }
        }
    }

    #[test]
    fn corner_ao_is_symmetric_in_sides() {
        for &s1 in &[false, true] {
            for &s2 in &[false, true] {
                for &c in &[false, true] {
                    assert_eq!(corner_ao(s1, s2, c), corner_ao(s2, s1, c));
                }
            }
        }
    }

    #[test]
    fn both_sides_ignore_corner() {
        assert_eq!(corner_ao(true, true, false), corner_ao(true, true, true));
    }

    #[test]
    fn face_ao_maps_each_corner_to_the_right_neighbours() {
        // Only the +u edge solid: the two +u corners ((+,-) idx2, (+,+) idx3)
        // each see one side occluder → 2; the -u corners stay 3.
        let n = FaceNeighbours { u_pos: true, ..Default::default() };
        assert_eq!(face_ao(&n), [3, 3, 2, 2]);

        // Only the -v edge solid: the two -v corners ((-,-) idx0, (+,-) idx2) → 2.
        let n = FaceNeighbours { v_neg: true, ..Default::default() };
        assert_eq!(face_ao(&n), [2, 3, 2, 3]);

        // Only the (+u,+v) diagonal solid: just corner idx3 → 2.
        let n = FaceNeighbours { d_up_vp: true, ..Default::default() };
        assert_eq!(face_ao(&n), [3, 3, 3, 2]);

        // A concave corner at (-u,-v): u_neg && v_neg solid → idx0 = 0.
        let n = FaceNeighbours { u_neg: true, v_neg: true, ..Default::default() };
        assert_eq!(face_ao(&n)[0], 0);
    }

    #[test]
    fn flip_rule_isolates_lone_dark_corner() {
        // Lone dark corner on the {0,3} diagonal → flip to {1,2}.
        assert!(should_flip([0, 3, 3, 3]), "dark idx0 (on {{0,3}}) → flip");
        assert!(should_flip([3, 3, 3, 0]), "dark idx3 (on {{0,3}}) → flip");
        // Lone dark corner on the {1,2} diagonal → keep default {0,3}.
        assert!(!should_flip([3, 0, 3, 3]), "dark idx1 (on {{1,2}}) → no flip");
        assert!(!should_flip([3, 3, 0, 3]), "dark idx2 (on {{1,2}}) → no flip");
        // Diagonals agree (equal sums) → no artifact, no flip.
        assert!(!should_flip([3, 3, 3, 3]));
        assert!(!should_flip([0, 0, 0, 0]));
        assert!(!should_flip([2, 1, 1, 2]), "{{0,3}}=4 ≥ {{1,2}}=2 → no flip");
    }

    #[test]
    fn flip_rule_equals_brighter_diagonal_comparison() {
        // Exhaustive: should_flip is exactly ao[1]+ao[2] > ao[0]+ao[3].
        for a0 in 0u8..=3 {
            for a1 in 0u8..=3 {
                for a2 in 0u8..=3 {
                    for a3 in 0u8..=3 {
                        let ao = [a0, a1, a2, a3];
                        let want = (a1 as u16 + a2 as u16) > (a0 as u16 + a3 as u16);
                        assert_eq!(should_flip(ao), want);
                    }
                }
            }
        }
    }

    #[test]
    fn ao_factor_endpoints_and_monotonic() {
        assert!((ao_factor(0) - 0.5).abs() < 1e-6);
        assert!((ao_factor(3) - 1.0).abs() < 1e-6);
        assert!((ao_factor(9) - 1.0).abs() < 1e-6, "clamps above 3");
        let f: Vec<f32> = (0..=3).map(ao_factor).collect();
        for w in f.windows(2) {
            assert!(w[1] > w[0], "brighter level → larger factor");
        }
        for &v in &f {
            assert!(v >= 0.5 && v <= 1.0);
        }
    }

    #[test]
    fn face_ao_exhaustive_in_range_and_deterministic() {
        for bits in 0u32..256 {
            let n = FaceNeighbours {
                u_neg: bits & 1 != 0,
                u_pos: bits & 2 != 0,
                v_neg: bits & 4 != 0,
                v_pos: bits & 8 != 0,
                d_un_vn: bits & 16 != 0,
                d_un_vp: bits & 32 != 0,
                d_up_vn: bits & 64 != 0,
                d_up_vp: bits & 128 != 0,
            };
            let ao = face_ao(&n);
            assert_eq!(ao, face_ao(&n), "deterministic");
            for &v in &ao {
                assert!(v <= 3);
            }
        }
    }
}
