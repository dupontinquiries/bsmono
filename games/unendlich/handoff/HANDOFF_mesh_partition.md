# HANDOFF — mesh_partition bin wiring (hh-r332-t3)

**Status:** PROPOSAL ONLY. No bin/glfw/render path edited (SHARED_RULES §2). The pure
core `sim::mesh_partition` is landed + gated; this is how the coordinator wires it
on-device. Anchors are file:line-verified against the base tree.

## Goal recap
A new neighbour chunk currently bumps this chunk's version → **full** re-mesh (all 6 face
planes + interior). Only the one boundary plane facing the new neighbour can change.
Partition the boundary mesh into 6 directional sub-meshes and re-mesh only the dirty one.
Expected: the documented "~35 re-meshes per chunk/session" storm drops by ~6× on the
boundary work; interior decoration (grass/flowers, always interior per FINDINGS-r62-05) is
never re-meshed on a neighbour event.

## (a) `mesh_buffer_pool`: `[Option<MeshSlot>; 6]` per chunk
- Today a chunk owns one mesh span; `MeshSlot` + `alloc()`/`free()` already exist
  (`render/mesh_buffer_pool.rs:80, 129, 168`). The pool is a span allocator, so it already
  supports many spans — a chunk just needs to hold up to 6.
- Change the per-chunk record from one `MeshSlot` to `[Option<MeshSlot>; 6]`, indexed by
  `SubMeshDir::index()` (0..6, stable). `alloc`/`free` a sub-mesh's slot independently when
  its `SubMeshDir` is (re)built or emptied. No pool-internal API change required.

## (b) Mesher emits into 6 buckets
- The greedy mesher already walks the 6 face planes via `AXIS_CONFIGS` and has `cfg.face`
  in hand at emit time (`render/mesher.rs:1502`, `mesh_chunk_greedy*` 6-plane loop).
- At each quad emit, label it: `let dir = SubMeshDir::from_face_index(cfg.face);` (exact,
  integer — preferred over `from_face_normal` since the index is already there). Push the
  quad's verts/indices into the bucket for `dir` instead of one flat `ChunkMesh`.
- Return `[ChunkMesh; 6]` (or a `SubMeshDir`-keyed map). **Interior decoration**
  (sub-voxel grass/flowers from the decoration pass) is NOT a boundary face → keep it in a
  7th "interior" bucket that is rebuilt only on a real block edit, never on a neighbour
  event. (`assign_quad(face_normal)` is the fallback if a path only has the `[f32;3]`
  normal; it returns `None` for non-axis/degenerate normals → route those to interior.)

## (c) `chunk_manager`: scope the neighbour bump to one sub-mesh
- The boundary-invalidation path is `bump_neighbor_if_edge(coord, axis, local_coord)`
  (`world/chunk_manager.rs:2211`), called per axis from `set_block` with `delta = -1`
  (local==0) or `+1` (local==CHUNK_SIZE-1). It bumps neighbour `nbr = coord + delta·axis`.
- **Which sub-mesh of `nbr` is dirty:** `nbr`'s face that borders this chunk points *back*
  toward us — direction `(-delta)` on `axis`. So:
  ```rust
  if let Some(my_side) = sim::mesh_partition::SubMeshDir::from_axis_delta(axis, delta) {
      let nbr_side = my_side.opposite();              // == from_axis_delta(axis, -delta)
      // mark only nbr_side dirty on the neighbour instead of a full version bump:
      nbr_submesh_dirty.mark(nbr_side);               // SubMeshSet per chunk
  }
  ```
  Give each chunk a `SubMeshSet` (next to its version). The streaming/remesh loop calls
  `take_dirty()` and re-meshes/re-uploads only those `SubMeshDir`s (re-`alloc` just those
  pool slots), leaving the other 5 + interior intact.
- The same `dirtied_by_neighbor(side)` rule applies to a neighbour *appearing/leaving*
  during streaming (not just edits): mark the one facing sub-mesh, not the whole chunk.
- Keep a `mark all 6 + interior` path for a genuine in-chunk block edit (that can change any
  plane) and for first mesh — use `SubMeshSet::all()`.

## (d) Draw stays one indirect set
- The 6 sub-mesh spans **concatenate** in the buffer; the draw path emits the same single
  indirect set over the chunk's combined span range. No change to draw order or the
  off-limits shaders (`chunk.frag`/`froxel_fill.comp`). Purely which spans get rebuilt.

## Interactions to keep in mind
- **`transition_remesh_stride`** (`chunk_manager.rs:292`, env `UNENDLICH_TRANSITION_REMESH_STRIDE`):
  each remesh is now ~6× cheaper on the boundary, so the time-throttle can be relaxed
  (more responsive transitions at the same cost). Tuning call — leave to the M2 run.
- **Seam / light-leak blend (`handoff/WORKER_E_mesh_seam.md`)**: the blend must still see
  **both** sides of a boundary plane. The partition doesn't change geometry — both the
  `S` sub-mesh here and the `opposite(S)` sub-mesh on the neighbour still exist; just make
  sure the seam pass reads the neighbour's `opposite(S)` span, not a stale combined mesh.
- **No visible-geometry change** (brief "Watch"): same triangles, same winding, just
  partitioned by direction + selectively rebuilt. A full rebuild of all 6 + interior must
  be byte-identical to today's single-mesh output — worth a one-off equality assert
  on-device (mesh all-6 vs. legacy `mesh_chunk_greedy`).

## Validation already done (pure core)
`sim::mesh_partition` — 8 tests, standalone `rustc --test` AND
`cargo test --features "render-core egui" --lib mesh_partition` under 1.96: both 8/8.
`from_face_index`/`from_face_normal` proven to agree and to match the mesher's documented
`FACE_NORMALS` + `face::*` convention.
