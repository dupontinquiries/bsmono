# MESH TIER PLAN — the missing high-detail GPU tier "right after near"
Confirmed from the RD=24 log: the meshing bottleneck is the **CPU near tier**, and there is no
high-detail GPU tier between it and `mid`.

## Evidence
- mesher near (CPU): n=11726, p50=0.27ms, **p90=54ms, p99=242ms, max=519ms**; ~630 resident near
  chunks ⇒ ~18 remeshes/chunk; empty_meshes near=299 (wasted).
- mesher mid (GPU): n=3290, **~0.00ms** — the first GPU tier sits nearly idle.
- far/vfar: no chunks (RD=24). GPU_MESHER=2 routes {mid,far,vfar}→GPU, **near→CPU**.
So `near` (LOD factor 1, CPU) is the ONLY high-detail tier, it carries everything close, and its
half-second tail spikes stall frames. The tier "right after near" — GPU at high detail — is missing.

## Fix (M2 / render thread)
1. **Add a GPU near-detail tier.** Route the near range (all but a thin innermost ring) to the GPU
   mesher at factor 1, so "high detail" no longer means "CPU". A `GPU_MESHER=3` / new tier. The CPU
   near mesher then only meshes the few chunks being actively *edited* (synchronous remesh for
   instant block-place/break feedback — ties to `block_edit`, R13).
2. **Stopgap now (flags):** narrow the CPU near band — `LOD_BANDS 12,24,48 → ~6,16,32` — so fewer
   chunks are CPU-meshed and GPU `mid` picks up sooner. Trades a little near detail for big CPU relief.
3. **Churn is load/transition-driven, not edit-driven.** `remesh_gate skipped=0` is EXPECTED — that
   gate only suppresses far micro-detail *edit* remeshes, which didn't occur this run; it was never
   going to touch the near churn. Thin the count with `UNENDLICH_TRANSITION_REMESH_STRIDE` (default 1);
   kill the per-mesh CPU *cost* with the GPU near tier above. (See docs/FOLLOWUPS 2026-06-04.)

## Pure core (queued, testable) — `mesh_tier`
    enum MeshTier { CpuNear, GpuNear, GpuMid, GpuFar, GpuVFar, Skip }
    fn mesh_tier(dist: f32, edited_recently: bool, gpu_ok: bool, bands: &[f32]) -> MeshTier
A thin CPU ring (edited/closest) → GPU at descending detail. Pairs with `lod_band` (r9),
`mesh_decimate` (r11), `chunk_priority` (r11). The mesher consumes it to pick CPU↔GPU + LOD factor.
Determinism wall unchanged (sim authoritative).
