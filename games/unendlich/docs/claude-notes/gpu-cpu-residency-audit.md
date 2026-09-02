# GPU/CPU residency audit — what crosses the bus, and what shouldn't (2026-09-02)

Question asked: "should some things live on the other side, or are they
copied needlessly or in the wrong shape?" Answer from reading the code, not
the docs, per CLAUDE.md §5. Numbers are per 32³ chunk unless stated. "Copy"
means bytes the CPU touches; the GPU's own reads are free here.

## Per chunk, GPU-mesher path (`UNENDLICH_GPU_MESHER=2`)

| step | where | bytes the CPU touches | needed? |
|---|---|---|---|
| 1. worldgen output | `gpu_generator::read_back_from` | 32 KB packed → 32 KB `Vec<BlockId>` → `Chunk::from_blocks` | the CPU sim needs the chunk; the **shape** is wrong: a 32 768-iteration `try_from` loop per chunk where a LUT/validate + memcpy would do |
| 2. octree build | `octree_scheduler` → `OctreeArray::from_chunk` | walks 32³, emits 8–11 K nodes typical (320–440 KB), 40 960 worst (1.6 MB) | built on the CPU from data the GPU generated. Wrong side for GPU-generated chunks; right side for edits |
| 3. octree upload | `OctreeBufferPool::upload_if_changed` | memcpy of the nodes into a HOST_VISIBLE pool | needed while the octree is built on the CPU |
| 4. mesh dispatch | `gpu_meshing_octree` | — | the GPU writes 24 B/vertex + 4 B/index into HOST_VISIBLE output |
| 5. mesh readback | `readback_slot` | **copy 1**: `to_vec` of 15–30 K vertices × 24 B + 45–90 K indices × 4 B ≈ 0.5–1 MB | **needless** — see below |
| 6. pack on main thread | `world_renderer::upload_mesh` | **copy 2**: `PackedVertex::from_world` per vertex, 24 B → 8 B, plus a min/max-Y pass; "the pack call is the bottleneck" (its own comment) | needless: the GPU had the data in the shape it wanted one step ago |
| 7. pool write | `mesh_buffer_pool::write_at` | **copy 3**: memcpy 8 B/vertex + indices into the HOST_COHERENT pool ("the M2 upload-pressure stall", `UNENDLICH_UPLOAD_BUDGET_MS` exists to ration it) or into staging + `vkCmdCopyBuffer` | needless for a GPU-meshed chunk |
| 8. draw | indirect / per-chunk | 0 | — |

So a mesh the GPU computed is read down (copy 1), transcoded on the main
thread (copy 2), and written back up (copy 3): ~1 MB down, ~0.35 MB up,
plus a per-vertex loop, per chunk, on the thread that also has to present.
At the measured 220 chunks/s that is ~300 MB/s of memcpy that should not
exist, and it is the single reason the upload budget knobs exist.

**And the readback was wrong.** `mesher::Vertex` is
`repr(C) { [f32;3], [f32;3], u8 }` — 28 bytes since r594 added `sway`. The
v0.24.44 "readback-bulk" cast of the shader's 6-float (24 B) output to
`*const Vertex` read every vertex after the first at the wrong stride and,
for the last slot, ~390 KB past the end of the mapped buffer. Fixed in the
ring commit (`GpuWireVertex`, 24 B, static-asserted; a test pins that
`Vertex` is *not* 24 B so the distinction cannot be lost again). Anyone who
ran `GPU_MESHER=2` after r594 and saw garbage was seeing this, not the
shader. The sparse mesher (`=1`) constructs `Vertex::new` per element and
was never affected.

### The fix, in two sizes

**A. Shader emits `PackedVertex` (8 B) directly.** Deletes copy 2 and
shrinks copy 1 by 3×. The shader has `chunk_origin` in its meta, so the
chunk-local fixed-point quantize (`POS_FRAC_BITS`) and the sway bits are a
few integer ops at emit. Keeps the CPU in the loop, so the upload budget
stays. Half a day; needs the box to validate the quantize matches
`PackedVertex::from_world` bit-for-bit (a parity test against the CPU
mesher's packed output of the same chunk is the check).

**B. The mesh never leaves the GPU.** The mesher writes into a
DEVICE_LOCAL scratch per ring slot; the host reads back **only the
counters** (16 B/chunk) from a small HOST_VISIBLE buffer; the CPU allocates
exact spans in the mesh pool from those counts; a `vkCmdCopyBuffer`
scratch → pool, recorded into the slot's *next* command buffer (or a
short follow-up submission on the same queue), moves the data on the GPU
timeline. CPU bytes per GPU-meshed chunk: 16. Deletes copies 1, 2, 3, the
per-vertex pack, and the upload budget for GPU-meshed chunks entirely.
`UNENDLICH_GPU_DECORATE`'s CPU post-passes (grass/water/lava) become a
small CPU-uploaded *append* span — they are off by default and add little
geometry. Prerequisites already in place after the ring: per-slot output
regions, pinning, the compute queue, and the pool buffer being
CONCURRENT across families. Two to three days; the risk is the
`mesh_buffer_pool` span allocator meeting a GPU-side copy for the first
time (it has a DEVICE_LOCAL+staging mode already, so the copy machinery
exists). **This is the change that makes "GPU-powered" true for terrain**;
A is the stepping stone if B has to wait for hardware.

## Per chunk, CPU-mesher path (Honeykrisp today, the phone probably)

Worker emits `Vertex` (28 B) → `MeshResult` over a channel (a move, no
copy) → **main thread** packs per vertex (24 → 8 B) and memcpys into the
pool. The pack loop runs on the render thread for every arriving chunk;
during a streaming burst that is 64 chunks × 20–40 K vertices of per-vertex
work on the frame's critical path.

**Fix: pack on the worker.** `MeshResult` carries `Vec<PackedVertex>` and
the y-bounds; `upload_mesh` becomes one memcpy. No format change, no
hardware needed, and it is the path both of your devices run right now.
Cheapest real win in this list. (Started this session — see the commit that
adds `ChunkMesh::packed`.)

## Per frame

- Indirect chunk metadata: dirty-tracked per frame-in-flight
  (`chunk_indirect::frame_dirty`). Right shape.
- GPU timers: `get_query_pool_results` with `WAIT` after the frame fence —
  non-blocking in practice. Fine.
- View params, dynamic lights, froxel fill: small uniforms. Fine.
- `acquire` at 1570 ms in the report is the swapchain waiting behind a mesh
  dispatch on the graphics queue — the queue serialization, not a copy.
  The ring + batch sizer + compute queue are its fix.
- Culling with `UNENDLICH_INDIRECT=0` (the honeykrisp preset): CPU frustum
  test + one draw call per chunk. Fine at ~900 chunks (rd 6); not at
  33 000 (rd 20). Indirect must win at scale, so the 1.4 ms fixed cost
  needs its cause found rather than avoided: the suspect is the
  `*_indirect_count` variant (Apple has no native multi-draw-indirect-
  count). Cheap A/B to add: `UNENDLICH_INDIRECT_COUNT=0` → plain
  `cmd_draw_indexed_indirect` over `high_water` commands with culled slots
  written as `index_count = 0` in place. Needs `chunk_cull.comp` to write
  in place instead of compacting.

## Wrong side, today, on specific hardware

- **Octree mesher on Honeykrisp**: 8–15 ms/chunk on the GPU against
  18 ms/chunk/worker × 24 CPU workers. The CPU wins by ~10× on that box.
  Driver/shader-shape specific (one thread per cell, 512 workgroups per
  chunk, divergent octree walk); the preset already routes around it. The
  phone report decides whether it is Honeykrisp-only.
- **GPU worldgen**: right side, wrong shape — `cmd_dispatch(1,1,1)` per
  chunk, one 8×8×8 workgroup, 16 workgroups per batch. That is why it
  measures 0.63–0.98× a 24-core CPU. The shader wants (4,4,4) workgroups
  per chunk with one block per invocation; the ring is already in place
  to pipeline it. Validate on the box (GLSL change, `glslangValidator`
  is installed here now).
- **Octree build for GPU-generated chunks**: the blocks were on the GPU;
  the octree is built on the CPU and uploaded. A compute-side builder
  (bottom-up merge over the generated block buffer, writing nodes straight
  into the pool) removes step 2+3 for streamed chunks. Edits keep the CPU
  builder. Bigger; after B.

## Duplicates that will bite

- Two `PackedVertex` types: `render/mesher.rs` (8 B, pos+color, the one
  the pipelines' vertex stride uses) and `src/vertex_pack.rs` (10 B,
  pos+normal+block_id+ao, wired to nothing in the render path). The second
  is the better format for a deferred/g-buffer path but it is not the one
  on the wire; `orphan_check`'s duplicate-public-function pass should
  flag it. Decide before either grows.

## Order

1. Pack on the worker (CPU path) — today, both devices.
2. Mesher ring + batch sizer + compute queue (landed) — the hitch.
3. B, mesh stays on the GPU — the "GPU-powered" claim for terrain.
4. Generator occupancy — GPU worldgen faster than the CPU on every device.
5. `INDIRECT_COUNT=0` A/B — scale.
6. GPU octree builder — removes the last per-chunk upload for streamed
   chunks.
