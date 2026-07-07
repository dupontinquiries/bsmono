# SHAREDLIB_REPORT — `r340-sl-gpu` (GPU buffer / compression consolidation)

**WID:** `r340-sl-gpu` · **Cluster:** `render::{mesh_buffer_pool, octree_buffer, chunk_indirect, q8, neural_gemm_pipeline, gpu_meshing_sparse}`

> ⚠️ **BASE CAVEAT — read first.** The packet names base
> `unendlich-r339-missing-pieces-CHECKED.zip`, but only `DISPATCH_PACKET_r340.md`
> was provided this turn. This survey + extraction were done against the
> **r336** tree I still had (`unendlich-r336-merged-r335-CHECKED.zip`). Therefore:
> - **All `file:line` call sites below are r336 line numbers — RE-ANCHOR to r339 before the sweep.**
> - The pure extracted module (`render::gpu_pack`) is **base-robust** (no Vulkan, pure integer
>   layout math) and verified in-sandbox; it stands regardless of r336↔r339 drift.
> - The byte-parity compression *against the live octree/sparse-mesh node formats* is **deferred**
>   until the r339 base is available (those formats are base-specific; doing it against r336 risks
>   targeting a layout that changed). The perf bonus shipped here is a **provably lossless** packing
>   primitive that is format-independent.

---

## Candidates

### C1 — GPU host-buffer staging + memory-type selection  → `render::gpu_buffer`  *(PROPOSE only)*
The "create a `HOST_VISIBLE|HOST_COHERENT` buffer, select the memory type, bind, persistently map,
memcpy upload" sequence is re-implemented all over the cluster (and the wider renderer). It is the
single largest GPU-buffer-management duplication.
- Near-identical helper definitions: `neural_gemm_pipeline.rs:475 host_buf`,
  `neural_shade_pipeline.rs:215 host_buf`, `neural_feat_pipeline.rs:326 host_buf`,
  `field_diffuse_gpu.rs:32 create_host_buffer`, `egui_overlay.rs:107 host_buffer`,
  `chunk_indirect.rs:467 create_buffer`.
- The **memory-type find-loop** (`memory_type_count` → first type with `HOST_VISIBLE|HOST_COHERENT`)
  is duplicated in **19 files**: chunk_indirect, chunk_renderer, depth, egui_overlay, entity_pipeline,
  field_diffuse_gpu, froxel, gpu_meshing_octree, gpu_meshing_sparse, mesh_buffer_pool,
  neural_feat_pipeline, neural_gemm_pipeline, neural_shade_pipeline, octree_buffer,
  octree_probe_pipeline, offscreen, shadow_map, view_params, world_renderer.
- **Proposed:** `render::gpu_buffer` — `HostBuffer::new(device, instance, pd, size)` (create+select+
  bind+map) + `upload(bytes)` + `Drop`. **PROPOSE, not extracted:** it is `unsafe`/`ash` Vulkan glue
  with no in-sandbox compile path (glfw/Vulkan not allow-listed), so per SHARED_RULES it cannot be
  gated by `rustc --test`. The coordinator does this one on-device.
- RESPECT: pure plumbing; touches no field numerics. `field_diffuse_gpu`'s field *data* is untouched —
  only its buffer-*creation* helper is flagged.

### C2 — Fixed-slot recycler  → `render::gpu_pack::SlotPool`  *(EXTRACTED ✓, tested)*
The `free_list: VecDeque<u32>` + `high_water` + `capacity` slot recycler.
- Call site: `chunk_indirect.rs:229–234, 330–340, 366–394` (`IndirectDrawSet`).
- **Extracted** as `gpu_pack::SlotPool` (pure index bookkeeping; preserves the existing semantics —
  ascending deterministic handout, high-water never shrinks on free, double-free/out-of-range guard).
- **Cross-cluster flag:** likely also surfaces as entity/connection slot pools in **sl-batch (T2)**
  and **sl-net (T4)**; if so, this is a ≥2-thread overlap — pick one `SlotPool`.

### C3 — Byte-span free-list allocator  → `render::mesh_buffer_alloc::SpanAllocator`  *(ALREADY CONSOLIDATED — no action)*
Reporting so the coordinator does **not** re-extract it: the aligned, coalescing best-fit span
allocator is **already a shared lib** (`mesh_buffer_alloc::{Span, SpanAllocator, AllocError}`) and is
consumed by `mesh_buffer_pool.rs:20,92–103`, `octree_buffer.rs:89,187–279`, and `world_renderer.rs`.
No duplication remains here.

### C4 — Lossless word / tile re-packing  → `render::gpu_pack::{pack_tiles, unpack_tiles}`  *(EXTRACTED ✓, tested)*
Re-lays a row-major packed `[n,k]` word grid into tile-major order for GEMM / mesh-tile locality;
exact inverse; pure permutation when evenly tiled.
- Current home: `q8.rs:147–205` (`pack_tiles`/`unpack_tiles`/`packed_tiles_len`, landed in r336-neural).
- **Proposed move:** make `gpu_pack` the canonical home; `q8::pack_tiles` becomes a thin re-export or
  is dropped (q8 keeps *quantization*, gpu_pack owns *layout*). Coordinator sweeps the one call path.
- **Cross-cluster flag:** overlaps **sl-mesh (T5)** "tighter GPU mesh packing in `gpu_meshing_sparse`."
  T5 and T7 will both touch word packing — dedup to the `gpu_pack` version.

### C5 — Fixed-width bit packing  → `render::gpu_pack::{pack_bits, unpack_bits}`  *(EXTRACTED ✓, tested — PERF BONUS (b))*
Packs integer fields of `bits∈1..=32` into a dense LE `u32` bitstream; exact inverse.
- The shared primitive the GPU-compression sites need: octree-node child masks + small child indices
  (`octree_buffer.rs` node layout) and sparse-mesh word packing (`gpu_meshing_sparse.rs`) both want
  sub-word fields but currently spend a full word per field.
- **Extracted** here as the lossless building block. **Wiring** the octree/sparse-mesh layouts onto it
  is **deferred to the r339 base** (the live node formats are base-specific; see caveat).

### C6 — q8 / q4 quantization  → `render::q8`  *(already self-contained; untouched this round)*
`q8` is already a clean self-contained module (r336-neural). Byte-parity with `oracle.quantize_q8` is
intact; `quantize_q4_block_experimental` exists for density experiments. No change needed for the
shared-lib hunt beyond C4's note that the *packing* helpers move to `gpu_pack`.

---

## PERF BONUS (b) — tighter GPU packing, **lossless** (parity proof)
`gpu_pack::{pack_bits, unpack_bits}`. "Lossless-or-prove-the-bound": it is **lossless** — the proof is
that `unpack_bits(pack_bits(v, bits), bits, v.len()) == v` for in-range `v`, verified for **every**
width `bits = 1..=32` over fuzzed inputs, plus word-boundary-straddling cases, all-zero, masking, and
the 32-bit identity case (test `bit_pack_round_trips_all_widths` + `bit_pack_is_dense_and_handles_boundaries`).
Density example: eight 6-bit fields → **2 words instead of 8** (`packed_bits_len(8,6)==2`). This is the
reusable lever for octree-node / sparse-mesh compression once re-anchored to r339.

## RESPECT-constraint compliance
- **No field flattened / no precision change.** `gpu_pack` operates only on already-encoded `u32`
  words and unsigned integer fields — never on a physics field's float grid. No `*_field` numerics
  were touched. The diffusion/advection fields are out of this cluster's scope and untouched.
- **No Minecraft assumptions.** `SlotPool` is generic index bookkeeping (not a 16³-chunk unit);
  packing is layout-only; nothing assumes a 6-face cube grid, instant block edits, or biome==palette.
- **q8 byte-parity preserved** (untouched; r336 oracle parity stands).

## Verify (in-sandbox)
Toolchain Rust **1.96.0** (offline bundle). `gpu_pack.rs` is fully self-contained (0 `crate::` refs):
```
$ rustc --edition 2021 --test gpu_pack.rs -o t && ./t
test result: ok. 8 passed; 0 failed; 0 ignored
```
Tests: `slotpool_alloc_free_reuse_and_exhaustion`, `slotpool_high_water_is_monotone_and_bounded`,
`slotpool_deterministic`, `tile_pack_unpack_round_trips`, `tile_pack_is_permutation_when_evenly_tiled`,
`bit_pack_round_trips_all_widths`, `bit_pack_is_dense_and_handles_boundaries`,
`bit_pack_masks_out_of_range_and_is_deterministic`. Full log: `PROOF-r340-sl-gpu.txt`.

## To finalize (needs r339 base)
1. Re-anchor every `file:line` above to r339.
2. Wire C5 `pack_bits` onto the live octree-node + sparse-mesh formats and prove byte-parity vs the
   current packed bytes.
3. Confirm `q8` oracle bytes unchanged on the r339 quantizer.
4. Coordinator: dedup C2/C4/C5 against sl-batch/sl-net/sl-mesh per the overlap matrix.

## Return contents
- `SHAREDLIB_REPORT-r340-sl-gpu.md` (this file)
- `crates/unendlich-engine/src/render/gpu_pack.rs` (extracted module + ≥6 tests; +1 `pub mod` line for `render/mod.rs`, coordinator reconciles)
- `PROOF-r340-sl-gpu.txt` (verification log)
