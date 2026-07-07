//! `mesh_tier` — the missing high-detail GPU tier "right after near".
//!
//! Worker-03 / WORLD pure core, round 15 (flagship; see MESH_TIER_PLAN.md). The
//! RD=24 profiling showed the CPU `near` mesher is the meshing bottleneck
//! (p90=54ms, p99=242ms, max=519ms) because it's the *only* high-detail tier:
//! everything close is CPU-meshed. The fix is a thin CPU ring — only the
//! closest and the actively-edited chunks (synchronous remesh so block edits
//! feel instant, ties to `block_edit` R13) — with everything else routed to the
//! GPU at descending detail by distance band. Beyond the last band a chunk is
//! skipped; if the GPU mesher is unavailable, every renderable tier falls back
//! to CPU.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Where/how a chunk should be meshed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshTier {
    /// CPU, full detail — the thin innermost ring + actively-edited chunks.
    CpuNear,
    /// GPU, full detail — the bulk of close chunks (the new tier).
    GpuNear,
    GpuMid,
    GpuFar,
    GpuVFar,
    /// Beyond render distance — don't mesh.
    Skip,
}

/// Pick the mesh tier for a chunk at `dist`, given whether it was edited
/// recently, whether the GPU mesher is available, and the ascending distance
/// `bands` separating tiers (`bands[0]` = CPU-ring radius, then GpuNear, Mid,
/// Far, VFar; at/after the last band ⇒ Skip).
pub fn mesh_tier(dist: f32, edited_recently: bool, gpu_ok: bool, bands: &[f32]) -> MeshTier {
    // Tier index = how many band thresholds `dist` has crossed (bands ascending).
    let mut idx = 0usize;
    for &b in bands {
        if dist >= b {
            idx += 1;
        } else {
            break;
        }
    }

    // At or beyond the last band (or no bands) → outside render distance.
    if idx >= bands.len() {
        return MeshTier::Skip;
    }

    // Edited chunks and the innermost ring stay on the CPU (sync, full detail).
    if edited_recently || idx == 0 {
        return MeshTier::CpuNear;
    }

    // Otherwise a GPU tier by band — falling back to CPU if the GPU is out.
    let gpu = match idx {
        1 => MeshTier::GpuNear,
        2 => MeshTier::GpuMid,
        3 => MeshTier::GpuFar,
        _ => MeshTier::GpuVFar,
    };
    if gpu_ok {
        gpu
    } else {
        MeshTier::CpuNear
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // CPU ring = 6, then GpuNear<16, Mid<32, Far<64, VFar<128, else Skip.
    const BANDS: [f32; 5] = [6.0, 16.0, 32.0, 64.0, 128.0];

    #[test]
    fn edited_or_closest_is_cpu_near() {
        assert_eq!(mesh_tier(3.0, false, true, &BANDS), MeshTier::CpuNear); // innermost ring
        assert_eq!(mesh_tier(50.0, true, true, &BANDS), MeshTier::CpuNear); // edited → CPU sync
    }

    #[test]
    fn just_past_ring_is_gpu_near() {
        assert_eq!(mesh_tier(6.0, false, true, &BANDS), MeshTier::GpuNear);
        assert_eq!(mesh_tier(15.9, false, true, &BANDS), MeshTier::GpuNear);
    }

    #[test]
    fn band_boundaries_map_to_mid_far_vfar() {
        assert_eq!(mesh_tier(16.0, false, true, &BANDS), MeshTier::GpuMid);
        assert_eq!(mesh_tier(32.0, false, true, &BANDS), MeshTier::GpuFar);
        assert_eq!(mesh_tier(64.0, false, true, &BANDS), MeshTier::GpuVFar);
        assert_eq!(mesh_tier(127.0, false, true, &BANDS), MeshTier::GpuVFar);
    }

    #[test]
    fn beyond_last_band_is_skip() {
        assert_eq!(mesh_tier(128.0, false, true, &BANDS), MeshTier::Skip);
        assert_eq!(mesh_tier(1000.0, false, true, &BANDS), MeshTier::Skip);
        // Edited but out of render range is still skipped.
        assert_eq!(mesh_tier(1000.0, true, true, &BANDS), MeshTier::Skip);
        // No bands → nothing renders.
        assert_eq!(mesh_tier(1.0, false, true, &[]), MeshTier::Skip);
    }

    #[test]
    fn no_gpu_falls_back_to_cpu() {
        for d in [6.0f32, 16.0, 32.0, 64.0, 100.0] {
            let t = mesh_tier(d, false, false, &BANDS);
            assert!(
                matches!(t, MeshTier::CpuNear | MeshTier::Skip),
                "expected CPU/Skip without GPU, got {t:?}"
            );
        }
        // Beyond range is still Skip even without GPU.
        assert_eq!(mesh_tier(500.0, false, false, &BANDS), MeshTier::Skip);
    }

    #[test]
    fn deterministic() {
        assert_eq!(
            mesh_tier(20.0, false, true, &BANDS),
            mesh_tier(20.0, false, true, &BANDS)
        );
    }
}
