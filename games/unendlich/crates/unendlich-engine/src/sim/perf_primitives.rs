//! `sim::perf_primitives` — tiny hot-path helpers shared by the perf cores (R39).
//!
//! `chunk_frustum_cull`, `lod_budget`, `draw_order_sort`, `visible_delta`, and
//! `draw_metadata_pack` each reach for the same small primitives. This is the
//! consolidated, allocation-free toolkit so they stop re-deriving them. Every
//! helper matches a naive reference bit-for-bit and allocates nothing.
//!
//! Added here (no prior definition — grep-verified): `lod_band`, `chunk_dist2`,
//! `aabb_center_extent`, `clamp_u32`, `select_f32`.
//! Skipped as already-owned: morton / Z-order interleaving lives in
//! `octree.rs` / `cluster.rs` — not duplicated. (`world::lod::lod_bands_for_rd`
//! *produces* band edges; `lod_band` here *classifies* a distance against them,
//! so they complement rather than duplicate.)
//!
//! Pure, deterministic, panic-free — no `crate::render`, no GPU, no I/O, no
//! clock, no RNG, no `.unwrap()`/`.expect()`. Release-safe: every helper is
//! unconditional (no `#[cfg]` gating) and compiles clean under `cargo check --release`.

/// LOD tier from squared distance against **ascending** `band_edges`: the count
/// of edges that `dist2` has reached. `0` below the first edge, `N` at/above the
/// last (`N = band_edges.len()`). Feeds `lod_budget` / `draw_order_sort`.
pub fn lod_band(dist2: u64, band_edges: &[u64]) -> u8 {
    let mut tier = 0u8;
    let mut i = 0;
    while i < band_edges.len() {
        if dist2 >= band_edges[i] {
            tier = tier.saturating_add(1);
        }
        i += 1;
    }
    tier
}

/// Squared distance between two `i32` chunk coords, overflow-free. Computed in
/// `i128` (a single `i32` difference squared can exceed `u64`), saturating at
/// `u64::MAX`. Matches the `i64` reference for all in-range inputs. Feeds
/// cull / priority.
pub fn chunk_dist2(a: [i32; 3], b: [i32; 3]) -> u64 {
    let dx = a[0] as i128 - b[0] as i128;
    let dy = a[1] as i128 - b[1] as i128;
    let dz = a[2] as i128 - b[2] as i128;
    let s = dx * dx + dy * dy + dz * dz; // ≥ 0, fits i128
    if s > u64::MAX as i128 {
        u64::MAX
    } else {
        s as u64
    }
}

/// AABB centre + half-extent from corner points. Handles inverted `min`/`max`
/// sanely: the centre is the midpoint and the half-extent is always
/// non-negative (`|max-min|/2`). Feeds `chunk_frustum_cull`'s `Aabb`.
pub fn aabb_center_extent(min: [f32; 3], max: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let mut center = [0.0f32; 3];
    let mut half = [0.0f32; 3];
    let mut i = 0;
    while i < 3 {
        center[i] = (min[i] + max[i]) * 0.5;
        half[i] = (max[i] - min[i]).abs() * 0.5;
        i += 1;
    }
    (center, half)
}

/// Branchless-style clamp of `v` into `[lo, hi]` (assumes `lo <= hi`).
pub fn clamp_u32(v: u32, lo: u32, hi: u32) -> u32 {
    v.max(lo).min(hi)
}

/// Select `a` when `cond`, else `b`. Returns the chosen operand verbatim — NaN
/// bits are preserved — so it's deterministic for any inputs.
pub fn select_f32(cond: bool, a: f32, b: f32) -> f32 {
    if cond {
        a
    } else {
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- lod_band ----
    fn lod_band_ref(dist2: u64, edges: &[u64]) -> u8 {
        edges.iter().filter(|&&e| dist2 >= e).count() as u8
    }

    #[test]
    fn lod_band_at_and_between_edges() {
        let edges = [100u64, 400, 900, 1600]; // N = 4
        assert_eq!(lod_band(0, &edges), 0, "below first edge");
        assert_eq!(lod_band(99, &edges), 0);
        assert_eq!(lod_band(100, &edges), 1, "at first edge");
        assert_eq!(lod_band(250, &edges), 1, "between edge0 and edge1");
        assert_eq!(lod_band(400, &edges), 2, "at second edge");
        assert_eq!(lod_band(1600, &edges), 4, "at last edge");
        assert_eq!(lod_band(u64::MAX, &edges), 4, "above last edge ⇒ N");
        // no edges ⇒ always tier 0
        assert_eq!(lod_band(123, &[]), 0);
        // matches the naive reference across a sweep
        for d in (0u64..2000).step_by(7) {
            assert_eq!(lod_band(d, &edges), lod_band_ref(d, &edges), "mismatch at {d}");
        }
    }

    // ---- chunk_dist2 ----
    #[test]
    fn chunk_dist2_matches_i64_reference_incl_large() {
        let cases = [
            ([0, 0, 0], [0, 0, 0], 0u64),
            ([0, 0, 0], [1, 2, 2], 9),
            ([3, 0, 0], [0, 4, 0], 25),
            ([-5, 0, 0], [5, 0, 0], 100),
            ([1_000_000, 0, 0], [0, 2_000_000, 0], 1_000_000u64 * 1_000_000 + 2_000_000u64 * 2_000_000),
        ];
        for (a, b, want) in cases {
            assert_eq!(chunk_dist2(a, b), want);
            // i64 reference (inputs chosen within i64-safe range)
            let dx = a[0] as i64 - b[0] as i64;
            let dy = a[1] as i64 - b[1] as i64;
            let dz = a[2] as i64 - b[2] as i64;
            assert_eq!(chunk_dist2(a, b), (dx * dx + dy * dy + dz * dz) as u64);
        }
        // symmetric and zero on equal
        assert_eq!(chunk_dist2([7, -3, 9], [7, -3, 9]), 0);
        assert_eq!(chunk_dist2([1, 2, 3], [-4, 5, -6]), chunk_dist2([-4, 5, -6], [1, 2, 3]));
    }

    #[test]
    fn chunk_dist2_extreme_coords_no_panic() {
        // full i32 extremes: a single squared diff exceeds u64 ⇒ saturates, no panic.
        let d = chunk_dist2([i32::MIN, i32::MIN, i32::MIN], [i32::MAX, i32::MAX, i32::MAX]);
        assert_eq!(d, u64::MAX, "saturates rather than overflowing");
    }

    // ---- aabb_center_extent ----
    #[test]
    fn aabb_center_extent_basic_and_inverted() {
        let (c, h) = aabb_center_extent([0.0, -2.0, 1.0], [4.0, 2.0, 3.0]);
        assert_eq!(c, [2.0, 0.0, 2.0]);
        assert_eq!(h, [2.0, 2.0, 1.0]);
        // inverted min/max ⇒ same centre, non-negative half
        let (c2, h2) = aabb_center_extent([4.0, 2.0, 3.0], [0.0, -2.0, 1.0]);
        assert_eq!(c2, [2.0, 0.0, 2.0]);
        assert_eq!(h2, [2.0, 2.0, 1.0]);
        assert!(h2.iter().all(|&x| x >= 0.0));
    }

    // ---- clamp_u32 ----
    #[test]
    fn clamp_u32_both_ends_and_passthrough() {
        assert_eq!(clamp_u32(5, 10, 20), 10, "below ⇒ lo");
        assert_eq!(clamp_u32(25, 10, 20), 20, "above ⇒ hi");
        assert_eq!(clamp_u32(15, 10, 20), 15, "mid ⇒ passthrough");
        assert_eq!(clamp_u32(10, 10, 20), 10, "at lo");
        assert_eq!(clamp_u32(20, 10, 20), 20, "at hi");
    }

    // ---- select_f32 ----
    #[test]
    fn select_f32_picks_and_is_nan_deterministic() {
        assert_eq!(select_f32(true, 1.0, 2.0), 1.0);
        assert_eq!(select_f32(false, 1.0, 2.0), 2.0);
        // NaN preserved per branch, deterministic by bits
        assert!(select_f32(true, f32::NAN, 2.0).is_nan());
        assert_eq!(select_f32(false, f32::NAN, 2.0), 2.0);
        assert_eq!(select_f32(true, 3.0, f32::NAN), 3.0);
        assert!(select_f32(false, 3.0, f32::NAN).is_nan());
        // returns the operand verbatim (bit-for-bit)
        let a = f32::from_bits(0x7FC0_0001);
        assert_eq!(select_f32(true, a, 0.0).to_bits(), a.to_bits());
    }

    #[test]
    fn deterministic() {
        assert_eq!(chunk_dist2([1, 2, 3], [4, 5, 6]), chunk_dist2([1, 2, 3], [4, 5, 6]));
        assert_eq!(lod_band(500, &[100, 1000]), lod_band(500, &[100, 1000]));
    }
}
