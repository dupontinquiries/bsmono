# BODY_PART_MESHING.md — part-aware meshing: fewer triangles, no gaps, real carnage

## Problem
Updating dragon visuals voxel-by-voxel risks two things: (a) cracks/gaps where regions at
different detail meet, and (b) per-voxel triangle counts that cost compute we don't have at the
entity counts we want. But we also WANT destructible detail — a player hacking one leg should see
voxels come off *that leg*. So the goal is: cheap where nothing is happening, fine where it's earned.

## Idea: let the body-part segmentation drive mesh resolution
Each entity is segmented into parts (head, neck, torso, legs, wings, tail — see
`handoff/ENTITY_BODY_SEGMENTATION.md`). Mesh each part at a resolution chosen from:
- **distance** (far ⇒ coarse), and
- **the part's damage state** (intact ⇒ aggressively greedy-merged quads; damaged / recently-hit
  ⇒ keep fine voxels so it can shed shards).

Static healthy regions become a handful of merged quads; the leg under attack stays voxel-fine.
"Visual carnage where it's earned" without paying for it everywhere.

## Three sub-systems (cores delegated below)
1. **Greedy voxel merge** (the compute saving). Merge runs of coplanar, same-material, solid
   voxels into larger quads. This is the same op the chunk mesher already does
   (`render/mesher.rs`, `render/gpu_meshing_octree_cpu_ref.rs`, `shaders/chunk_mesh*.comp`) —
   scoped to an entity part. Coarse parts merge hard; fine parts barely merge. Core:
   `voxel_greedy_run` — pure, bit-exact merged spans, so the merge is provably lossless.
2. **Watertight seams** (no gaps). Where a coarse face meets a fine face (between two parts, or two
   LOD levels), the edges don't line up → a crack. Fix = a **skirt/apron** dropped at the boundary,
   deep enough to cover the worst-case step. Core `seam_skirt` computes that depth from the two LODs
   + voxel size. Core `part_mesh_lod` picks each part's LOD so neighbors never differ by more than
   one level (restricted-quadtree rule) — which *bounds* the seam so the skirt is always small.
3. **Carnage** (destructible detail). `limb_detail_flag` marks a part fine when its HP drops below a
   threshold or it was just hit, with a cooldown so it doesn't pop back to coarse mid-fight.
   `damage_shard` turns a hit's magnitude + location into shard count + spread velocity.

## Integration sketch (M2 side)
- The entity mesher (entity path in `render/mesher.rs`) takes a per-part LOD vector from the sim;
  per-limb HP + last-hit tick already live in the body-part component. For each part: pick LOD
  (`part_mesh_lod`), greedy-merge at that LOD (`voxel_greedy_run`), emit a seam skirt at part
  boundaries (`seam_skirt`). Re-mesh a part only when its LOD or voxels changed — edit-scoped, like
  the chunk `remesh_gate`, so healthy static parts never re-mesh.
- Shards are short-lived debris entities (reuse the ember/dust field pattern) seeded by
  `damage_shard`; they do NOT mutate the persistent voxel grid, which keeps the body-part mesh
  stable and cheap.
- **Gap guard belongs in the GPU harness eventually:** a `vk-harness` parity test that a part mesh
  plus its skirts is watertight — every boundary edge referenced an even number of times (no T-junction
  leaks). That's the integration-tier proof; `gpu_check.sh` keeps the shaders compiling meanwhile.

## Delegated cores
- R26 (T02): `voxel_greedy_run`, `seam_skirt`, `part_mesh_lod`.
- R27 (T01): `limb_detail_flag`, `damage_shard` (+ `dismember_check`).
- R27 (T02): `normal_smooth`, `specular_aa`, `crack_fill` (faceting + seam polish).
Each pure + ≥6 tests per the ruleset; the meshing cores carry no `crate::render` import (R1).
