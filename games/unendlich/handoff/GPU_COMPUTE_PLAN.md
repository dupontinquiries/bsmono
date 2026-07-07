# GPU-COMPUTE + CULLING/SCHEDULING PLAN
From the user's idea-stream + the 901s RD=24 log (13 FPS, ~150% CPU, ~15GB, octree pool 96% full).

## What the log says
- **octree pool 96% full**: octree_used_mb=1878/2048, 11929 chunks × 161KB; srun=5KB/chunk
  (ratio 31.9×); largest_free ~130MB. Near OOM at this exploration extent. ⇒ SurfaceRun far-LOD
  routing is now MEMORY-urgent (the single highest-value integration).
- **CPU-bound, not GPU-bound here**: frame p50≈79ms (13 FPS) but GPU total p50≈24ms, main p50=15ms
  when static. The deficit is CPU: draw-call recording + submit + tick_sim, all scaling with the
  11928 rendering chunks / 26048 loaded. main/shadow are bimodal (cheap static, huge on motion/load).
- **fires=1,006,633**: heat Fix A (clustering patch) + Fix B (spatial heat_source.rs) NOT applied yet.
- near mesher n=235483 over 901s — enormous load churn from outward exploration.

## Direction (decompose into: pure cores now · GPU/integration on M2)
### 1. Abstract the GPU compute into a reusable field-kernel framework (user)
The engine already runs compute pipelines: `shaders/chunk_cull.comp`, `render/view_compute_pipeline.rs`,
the GPU mesher in `render/meshing_pool`. Generalize the plumbing into ONE reusable kernel abstraction:
{compute pipeline + input/output SSBO(s) + push-constants + barriers + dispatch sizing}. New
embarrassingly-parallel workloads then plug in without bespoke Vulkan each time.
### 2. Field updates → GPU first (weather, water_field) — "GPU-perfect"
2D/3D grid stencil updates (diffusion/advection) are ideal GPU compute and frees CPU tick_sim
(currently ~plant_lifecycle 12ms + tick 46ms in the heavy frame). Pure CPU reference: `field_step`
(one stencil step); the GPU kernel mirrors it bit-for-bit.
### 3. Genetics / popdyn / plant-genetics / pollination → GPU as populations grow (user)
"Plants getting genetics ⇒ diversity↑ but compute↑ (sub-part genetics + heavy pollination)." Move the
hot, uniform-per-entity work to GPU kernels: batched expression (`plant_gene_express`, like batch_gene),
population step (`popdyn_step`), pollen transfer (`pollen_spread`). Pure CPU refs → GPU kernels.
### 4. Multi-level culling (user)
Add a COARSE region tier (e.g. 4×4×4-chunk regions): frustum-cull regions first, descend to per-chunk
cull only for visible regions. Cuts both the per-chunk cull invocations and the CPU draw-call setup at
~12k rendering chunks. Pure core: `region_cull` (region AABB test + which children to descend).
### 5. Chunk scheduling + fast fetch/gen predictors (user)
- `prefetch_predict`: from player pos+velocity, the priority-ordered chunks to fetch/pre-gen AHEAD of
  motion — hides gen/mesh latency, smooths the 235k near-mesh churn.
- `chunk_schedule`: budget-aware selection — given priorities (chunk_priority r11) + a per-frame budget,
  pick the ordered subset to process. `gen_estimate`: per-chunk gen-cost estimate for the budget.

## Pure cores queued — R22
SIM: popdyn_step, pollen_spread, plant_gene_express, ability_aura, batch_horn_charge.
RENDER: region_cull, lod_morph, light_falloff, batch_rim_light.
WORLD: prefetch_predict, chunk_schedule, field_step, gen_estimate, batch_flow_route.

## Integration (M2, not pure cores)
- **SurfaceRun far-LOD routing** (urgent, memory): route far/mid chunks to SurfaceRun via the
  upload_repr classifier (handoff/SURFACERUN_KIT.md) — relieves the 96%-full octree pool + cuts main/
  shadow geometry. Do this first.
- GPU kernel framework (#1) + wire field/genetics/pollination kernels (#2,#3).
- region-cull hierarchy in world_renderer (#4); scheduler/predictor into chunk_manager (#5).
- Apply heat Fix A (fix_lava_heat_clustering.patch) + Fix B (heat_source.rs); apply cull
  workgroup-aggregation (chunk_cull.comp) for the dense-scene cull cost.
