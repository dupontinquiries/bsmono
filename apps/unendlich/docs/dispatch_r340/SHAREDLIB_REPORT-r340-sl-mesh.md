# SHAREDLIB_REPORT — r340-sl-mesh

**WID:** r340-sl-mesh · **Cluster:** `render::{mesher, gpu_meshing_sparse}` +
`sim::{mesh_partition, mesh_budget}` (+ adjacent: `world/brick_mesher`, `world/voxel_raycast`,
`world/flora_sites`, `vertex_pack`).
**Base scanned:** `unendlich-r330b` (the r339 base was not provided — see "Base mismatch").

Summary: **1 candidate extracted** (canonical face/quad primitives), **3 proposed** (greedy
face-merge, AABB struct — cross-cluster, GPU-buffer pool — cross-cluster). Perf bonus (a) shipped
with bit-parity.

---

## CANDIDATE 1 — canonical cube-face table (normals / offsets / verts / indices) — **EXTRACTED**
**Shared home:** `render::mesh_prims` (new, shipped).
**The duplication (same six axis-aligned faces, 3+ orderings & types):**
| site | form | order |
|---|---|---|
| `render/mesher.rs:736` `FACE_NORMALS:[[f32;3];6]`, `:784` `FACE_OFFSETS`, `:762` `FACE_VERTS`, `:726` `FACE_LIGHT`, `:747` `mod face` | f32 normals + i32 offsets + quad verts + indices | TOP,BOTTOM,EAST,WEST,SOUTH,NORTH (= +Y,−Y,+X,−X,+Z,−Z) — **36 use-sites in mesher** |
| `world/voxel_raycast.rs:41` `face_normal(Face)->(i32,i32,i32)` | i32, via `enum Face` | None,PosX,NegX,PosY,NegY,PosZ,NegZ — **different order + a `None`** |
| `world/flora_sites.rs:21` `FACES:[(i8,i8,i8);6]` | i8 | +X,−X,+Y,−Y,+Z,−Z — **third order** |
| `vertex_pack.rs` `oct_encode_normal`/`oct_decode_normal` (test @151) | packed octahedral (lossy) | references the same 6 normals as its golden set |

**⚠ Index-order hazard (read before merging):** the three tables encode the *same directions*
in **different index orders**. `mesher`'s order is **load-bearing** — `FACE_VERTS`, the greedy
`AxisConfig` table (`mesher.rs:1504`), vertex packing and the GPU buffers all index by it.
The extraction therefore keeps **mesher's order canonical** and adds a **direction-keyed bridge**
`face_from_offset(off)->Option<usize>` so `voxel_raycast`/`flora_sites` consolidate by *direction*,
never by a renumbered index. Do **not** renumber `face::*`.

**Extracted (`render::mesh_prims`, 7 tests):** `face::{TOP..NORTH}`, `FACE_NORMALS`,
`FACE_OFFSETS`, `FACE_VERTS`, `FACE_LIGHT`, `normal_f32`/`offset_i32`/`offset_i8`,
`face_from_offset`, `quad_corners`/`quad_aabb` (+ the perf-bonus pass). Tests prove: normals
unit+axis-aligned; offsets == sign(normals); `FACE_VERTS` lie on the right plane, span the unit
square, and are perpendicular to their normal (winding/copy correctness); `face_from_offset`
round-trips canonical **and** maps the voxel_raycast & flora orderings on; rejects diagonal/zero.

**Call sites to re-point (coordinator sweep, don't rewrite all here):** the 36 `FACE_*` uses in
`mesher.rs` → `mesh_prims::FACE_*`; `voxel_raycast::face_normal` body → `mesh_prims::offset_i32`
keyed by its `Face`→dir; `flora_sites::FACES` → `mesh_prims::FACE_OFFSETS` via `offset_i8`.
**Cross-cluster overlap:** the dot/cross/normalize used to *validate* the table are
`util::vecmath`/`simvec` ops → likely also proposed by **sl-math**; the table itself is mesh-only.

---

## CANDIDATE 2 — greedy face-merge — **PROPOSE** (not extracted)
**Duplication:** `render/mesher.rs:1568 mesh_chunk_greedy` / `:1950 mesh_chunk_greedy_undecorated`
(greedy over a per-plane slice mask, `AxisConfig` @1504) **and** `world/brick_mesher.rs:88
fn greedy_merge(mask, emit)` (a second, independent greedy-merge over a brick mask).
**Proposed home:** `render::mesh_prims::greedy_merge_mask(mask, dims, emit)` — a generic
mask→rectangles merger both call, emitting `(u0,v0,du,dv,id)` runs.
**Why proposed, not extracted:** the two have different mask types and the mesher's path is
sky-/decoration-aware; unifying must stay **bit-identical in emitted quads**. That's a careful
merge the coordinator should own (and confirm against `mesher`'s greedy tests @3154–3210 and
`brick_mesher`'s `greedy_covers_exactly_the_same_faces_as_naive` @208). **RESPECT:** keep it
per-face-plane greedy (sub-voxel + the r332 directional sub-mesh scheme stay intact) — do not
collapse to a cube-grid assumption.

---

## CANDIDATE 3 — the `Aabb` struct + min/max/union — **PROPOSE (CROSS-CLUSTER — flag)**
**Duplication (6+ independent structs):** `world/octree_region.rs:42 Aabb3`,
`world/entity.rs:150 Aabb`, `sim/aabb_resolve.rs:11 Aabb`, `sim/chunk_frustum_cull.rs:34 Aabb`,
`view_cull.rs:12 Aabb`, `collision.rs:26 Aabb` (+`:82 union`), `world/edit_coalesce.rs:55 union`.
**Proposed home:** a single `util::aabb` (`Aabb { min,max }`, `union`, `contains`, `from_points`).
**⚠ Cross-cluster:** this spans **mesh** (frustum/view cull), **collision**, **octree/gpu**,
**entity**, **worldgen** — it will almost certainly be proposed by **sl-math**, **sl-gpu**
(octree bounds), and **sl-worldgen** too. The coordinator should pick ONE canonical `Aabb` so the
seven threads don't merge seven overlapping versions. I did **not** extract it (cross-cluster
ownership); `mesh_prims::quad_aabb` is only the mesh-local min/max consumer.

---

## PERF BONUS (option a) — data-parallel per-quad AABB pass — **SHIPPED with parity**
`mesh_prims::quad_aabbs_parallel(quads, min_parallel)` runs the per-quad
`quad_corners`→`quad_aabb` kernel data-parallel. The kernel `quad_aabb_of(&QuadSpec)` is
`Send+Sync`, reads only its item, holds no shared mutable state, and writes an indexed slot — so
the index-preserving parallel map yields **bit-identical** output to `quad_aabbs_serial`. Proven
by `serial_equals_parallel` (f32 `to_bits` equality over n = 0,1,7,64,1000).

**`task_lanes` wiring:** on the real base this routes through
`util::task_lanes::map_pure_threshold(quads, MIN_PARALLEL, quad_aabb_of)`. `task_lanes` pulls
`rayon`; to keep the sandbox build offline (SHARED_RULES §5) I proved the same parity with a
`std::thread::scope` chunked map instead. `map_pure_threshold`'s contract (pure `Fn`, indexed map)
makes the rayon result identical — swap the std-thread body for the `task_lanes` call on merge.

---

## RESPECT compliance (the two hard constraints)
- **Fields preserved:** nothing here touches a physics field, its precision, or its stencils.
  The face table is the cube-face primitive the *voxel mesher* uses to emit quads for solid voxels
  — it does not flatten any `*_field` to booleans. The perf bonus is bit-identical (`to_bits`).
- **No Minecraft assumptions:** this does **not** assume a 6-face cube *world* — it provides the
  6 cube-*face* directions the existing mesher already uses, and the `mesh_partition` directional
  sub-mesh scheme (r332) and sub-voxel geometry keep their own logic and would *consume* these
  primitives via `face_from_offset`. No 16³-chunk gameplay unit, no block-grid pathing, no
  static water blocks introduced.

## Base mismatch (honesty)
Built against `unendlich-r330b`, not the brief's `unendlich-r339-…-CHECKED`. Present and grounded:
`mesher`, `gpu_meshing_sparse`, `brick_mesher`, `voxel_raycast`, `flora_sites`, `vertex_pack`,
`task_lanes`. **Absent (r332 additions):** `sim::mesh_partition`, `sim::mesh_budget` — so their
duplication is inferred from the r332 brief (mesh_partition buckets greedy quads by face normal →
overlaps CANDIDATE 1's `FACE_NORMALS`/`face_from_offset`; mesh_budget is cost/admission, no face
dup). The coordinator should confirm those two on r339 and add them to the CANDIDATE 1 sweep.
