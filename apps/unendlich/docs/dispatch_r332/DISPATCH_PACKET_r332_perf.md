# DISPATCH PACKET — r332 PERF (headlesshorseman batch)

Three perf threads off the r331 M2 profile. All read `docs/dispatch_r330/SHARED_RULES.md`
FIRST (same rules; pure cores, no bin/glfw, in-sandbox-validatable, return changed `.rs` +
MANIFEST). Base = `unendlich-r332-merged-CHECKED.zip` (or the latest -perf roll).

**The profile (r331, 844 s, 28 430 frames) — it's a TAIL/stutter problem, not throughput:**
median `total` p50 = 11.1 ms (~90 fps) but **p99 = 264.75 ms, max = 642.27 ms**. The three
hitch sources these threads attack:
- `near` mesher: p50 0.13 → **p99 104.5 / max 1020.8 ms** (a single near chunk builds for up to
  1 s synchronously).
- `record` (CPU): p50 4.0, p90 8.3, **100 % of frames over 1 ms** (steady per-instance tax).
- new chunks → **remeshing storm** (`chunk_manager` already notes "~35 re-meshes per chunk over a
  session"): a neighbour appearing bumps this chunk's version → it **fully** re-meshes.

Coordinator owns the `main`-pass / upload-flush spike (it's `froxel_fill.comp`/`chunk.frag`,
off-limits) and has landed `sim::frame_correlation` to attribute the tail on the next M2 run.

**Merge order on return:** `HH-T1 → HH-T2 → HH-T3` (all new-file pure cores; chunk_manager wiring
from T1+T3 is coordinator-merged on-device from their handoffs). Gate each under 1.96.

---

## HH-T1 — per-frame near-mesh build budget (kill the 1 s mesh spikes)
**WID:** `hh-r332-t1`
**Evidence:** `mesher_timer` near tier p99 104.5 / max 1020.8 ms. Uploads are budgeted
(`runtime_config.max_uploads_per_frame=64`, `upload_budget_ms=4.0`) but the CPU **mesh build**
is not — one oversized near chunk monopolises a frame (the "stutter1" streaming storm on fast
descent; confirm via `frame_correlation` `vvel` on the next run).
**Build:** new `sim::mesh_budget` (`crates/unendlich-engine/src/sim/mesh_budget.rs`) — a pure
per-frame build-admission core:
- `estimate_cost(solid_cells: u32, lod: u8) -> u32` — cheap proxy for build time from a chunk's
  solid-cell count (the mesher already knows this), so the scheduler can rank/limit without
  meshing first.
- `admit(pending: &[PendingMesh], budget: MeshBudget) -> Vec<usize>` — choose which queued near
  meshes to build THIS frame under `{ max_meshes_per_frame, build_budget_us }`, **never** letting
  one chunk blow the frame: hard count cap + cumulative-cost cap, nearest/in-view first (distance
  passed in), deferring the rest to later frames. Returns indices to build now (deterministic,
  alloc-light).
- Extend `runtime_config` with `max_meshes_per_frame` (default ~8) + `mesh_build_budget_ms`
  (default ~3.0), env-overridable like the upload knobs (ONE additive block under a `// hh-r332-t1`
  header; coordinator reconciles).
**Anchors:** `render/mesher_timer.rs` (near tier), `runtime_config.rs:25-26,93-94` (upload knobs
pattern), `world/chunk_manager.rs` (the near-mesh queue drain — propose the call-site, don't wire),
`docs/PERF-LEVERS.md`, the stutter1 notes.
**Verify:** ≥6 tests (admit never exceeds count cap nor cost budget; a single over-budget chunk is
admitted alone and the rest deferred, never starving forever; nearest-first ordering; empty queue →
empty; deterministic). Self-contained → `rustc --test`. Wiring proposed in `HANDOFF_mesh_budget.md`.

---

## HH-T2 — localize + cut the steady `record` cost (5 ms every frame)
**WID:** `hh-r332-t2`
**Evidence:** `record` p50 4.0, p90 8.3 ms, **100 % over 1 ms**. With the v0.20 indirect-draw path
(3–4 GPU calls/frame) that's far too high for the chunk draws → the cost is **per-instance CPU**
in recording (mob/foliage instances, overlay), or per-frame draw-list rebuild.
**Build — two pure pieces:**
1. **Record-zone accounting** `sim::record_zones`: a pure accumulator + a *proposed* sub-zone split
   (`record_chunks / record_entities / record_overlay / record_ui`) so the next M2 run shows which
   slice owns the 5 ms. Ship the accumulator + the handoff naming the exact `profiler.record(...)`
   insertion points in the bin record loop (propose, don't wire).
2. **Instanced draw grouping** `sim::draw_batch`: group a frame's instanced draws by
   `(pipeline_key, mesh_id)` so the record loop emits **one** instanced draw per group instead of
   per-instance. Build on the existing `pipeline_key` / `draw_key` / `batch_draw_key` cores (reuse
   their key types; do NOT restructure them). `group(draws: &[DrawItem]) -> Vec<DrawGroup>` +
   a count/savings helper. Pure, stable-sorted, deterministic.
**Anchors:** `profiler.rs` (zones), `pipeline_key.rs`/`draw_key.rs`/`batch_draw_key.rs`, the
indirect-draw path in `render/world_renderer.rs` (read-only; propose wiring).
**Verify:** ≥6 tests (grouping collapses N same-key instances to 1 group; distinct keys stay split;
order stable; empty → empty; savings count correct; deterministic). `cargo test --features
"render-core egui" --lib` if it pulls key types, else standalone with a minimal key stub.
**Watch:** purely a CPU-record reduction; **no** visual/order change to what's drawn. Don't touch
the off-limits shaders.

---

## HH-T3 — partial / multi-mesh remeshing so a new neighbour doesn't redraw the whole chunk
**WID:** `hh-r332-t3`  (the operator's explicit ask)
**Evidence:** `world/chunk_manager.rs` documents a **remeshing storm — "~35 re-meshes per chunk over
a session"**: when a new neighbour chunk appears it bumps this chunk's version → the chunk **fully**
re-meshes (all 6 face planes + interior) even though only the **one boundary plane facing the new
neighbour** can actually change. The greedy mesher already builds per-slice 2D quads on **each of the
6 face planes** (`mesh_chunk_greedy_undecorated`, mesher.rs:1911), and `mesh_buffer_pool` is a
span allocator (`alloc`/`free`) — so a chunk can hold **multiple** sub-mesh spans.
**Build:** new `sim::mesh_partition` (`crates/unendlich-engine/src/sim/mesh_partition.rs`) — the pure
scheme that lets a chunk be meshed/uploaded as **6 directional sub-meshes** + the dirty-scoping rule:
- `enum SubMeshDir { NegX, PosX, NegY, PosY, NegZ, PosZ }` (+ `ALL`, `from_face_normal`).
- `dirtied_by_neighbor(side: SubMeshDir) -> &[SubMeshDir]` — a neighbour appearing/leaving on `side`
  can only change THIS chunk's faces whose outward normal points toward `side` → returns just
  **that one** sub-mesh (interior + the other 5 planes are untouched). This is the whole win: a
  neighbour event re-meshes 1/6 of the boundary, not the whole chunk.
- `assign_quad(face_normal) -> SubMeshDir` — bucket each emitted greedy quad into its directional
  sub-mesh (the mesher already iterates the 6 planes, so this is a label, not a re-walk).
- A small `SubMeshSet` tracking which of the 6 are dirty, with `mark(side)`, `take_dirty() -> set`,
  so the manager re-meshes/re-uploads only the dirty sub-meshes.
**Anchors:** `world/chunk_manager.rs` (remesh_gate / `transition_remesh_*` / neighbour version bump
~1177), `render/mesher.rs` (`mesh_chunk_greedy_undecorated`:1911 — the 6 face-plane loop),
`render/mesh_buffer_pool.rs` (`alloc`/`free` span API — needs up to 6 spans/chunk),
`docs/FINDINGS-r62-05-greedy-mesh.md`, `handoff/WORKER_E_mesh_seam.md`.
**Verify:** ≥6 tests (`dirtied_by_neighbor` returns exactly the one matching plane; `assign_quad`
maps each of the 6 normals correctly; a neighbour event dirties 1 sub-mesh not 6; `SubMeshSet`
mark/take round-trips and clears; `ALL` covers every variant; deterministic). Self-contained →
`rustc --test`.
**Bin handoff (`HANDOFF_mesh_partition.md`):** propose (do NOT wire) — (a) `mesh_buffer_pool` holds
`[Option<MeshSlot>; 6]` per chunk; (b) the mesher emits into 6 buckets and returns a
`[ChunkMesh; 6]` (or a sub-mesh map); (c) `chunk_manager`'s neighbour-version-bump path calls
`dirtied_by_neighbor` and re-meshes/re-uploads only the dirty sub-mesh(es); (d) draw stays one
indirect set (the 6 spans just concatenate). Note the interaction with `transition_remesh_stride`
(this makes each remesh ~6× cheaper, so the time-throttle can relax) and the seam/light-leak blend
(WORKER_E) which must still see both sides of a boundary plane.
**Watch:** **no visible-geometry change** — same triangles, just partitioned + selectively rebuilt.
Decoration geometry (grass/flowers, per FINDINGS-r62-05) is interior → never in a boundary sub-mesh,
so it's never re-meshed on a neighbour event (that's part of the win).

---

### Coordinator checklist
- [ ] Cut `worker-<WID>-r332.zip` = base + `WORKER_ID` + this brief + `SHARED_RULES.md`.
- [ ] Wire `sim::frame_correlation` into the bin (HANDOFF_frame_correlation.md) so the NEXT M2 run
      attributes the tail — that data steers T1 (velocity?) and the coordinator's main-pass fix.
- [ ] On return merge `T1 → T2 → T3`, gate each @1.96, reconcile the `runtime_config` + the two
      chunk_manager wirings (T1 drain + T3 neighbour scope) on-device, roll `unendlich-r333-merged.zip`.
