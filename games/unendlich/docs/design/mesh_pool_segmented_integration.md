# mesh_buffer_pool → segmented allocator: the GPU-binding integration (NOT APPLIED)

`render::segmented_alloc::SegmentedSpanAllocator` (landed this turn, compiled + tested against
the REAL unmodified `mesh_buffer_alloc.rs`, 8/8 new tests + the 15 pre-existing `SpanAllocator`
tests all green — 23/23 total) is the CPU-side logic for VulkanMod-style growable memory: instead
of one fixed-size buffer that silently drops meshes on exhaustion, it's a `Vec<SpanAllocator>`
that adds a new segment when every existing one is full, up to a real `max_segments` ceiling.

This doc is the plan for the other half — actually making `mesh_buffer_pool.rs` use it — written
out but **not applied**, for the same reason `mesh_dirty_integration_sketch.md` wasn't applied to
the creature-mesh block: this touches the hot Vulkan draw path with no GPU/display here to verify
it, and getting it wrong doesn't fail loudly, it silently corrupts geometry or barriers.

## Why this is bigger than the env-var stopgap

I checked every place `mesh_pool.vertex_buffer` / `mesh_pool.index_buffer` get used, and it's not
just allocation — the whole draw path assumes exactly ONE `vk::Buffer` per axis:

- `world_renderer.rs:6106-6142`, `:6516-6518`, `:6584-6614` — six separate call sites (shadow
  pass, main pass, at least one more LOD/pass variant each), every one of them:
  ```rust
  ctx.device.cmd_bind_vertex_buffers(cmd, 0, &[self.mesh_pool.vertex_buffer], &[0]);
  ctx.device.cmd_bind_index_buffer(cmd, self.mesh_pool.index_buffer, 0, vk::IndexType::UINT32);
  ctx.device.cmd_draw_indexed_indirect_count(...);
  ```
  One bind, one indirect-count draw call, covering every visible chunk in the whole pool. This is
  a genuinely good pattern (matches VulkanMod's own "one `IndirectBuffer` per frame-in-flight, one
  indirect call" design) — the cost of segmenting is that "one call for everything" becomes "one
  call per segment that has visible chunks in it."
- `chunk_indirect.rs`'s `IndirectDrawSet` writes per-chunk draw commands into ONE indirect buffer
  via a GPU cull compute pass (`record_cull_dispatch`), keyed by a flat per-chunk metadata index.
  It has no concept of "which buffer does this chunk's geometry live in" — that's implicit today
  because there's only one.

So segmenting for real means:

1. **`VulkanMeshPool`**: replace the single `SpanAllocator` per axis with
   `SegmentedSpanAllocator`. Each segment needs its OWN `vk::Buffer` + persistent host map (or
   device-local + staging pair, matching the existing `UNENDLICH_DEVICE_LOCAL_MESH` path) —
   segments are created lazily as `alloc` grows, so buffer creation moves from "once at pool
   construction" to "on first use of each segment."
2. **`MeshSlot`**: needs a segment index for the vertex span AND (separately, since vertex and
   index are independent `SegmentedSpanAllocator`s) the index span. They won't always be the same
   segment number.
3. **Indirect draw commands** (`chunk_indirect.rs`): per-chunk metadata needs to carry which
   segment it belongs to, and the cull compute pass needs to write into a PER-SEGMENT indirect
   command buffer (or a single buffer with per-segment sub-ranges + per-segment counts) so the
   draw-side loop knows how many commands belong to each segment before issuing that segment's
   `cmd_draw_indexed_indirect_count`.
4. **The six draw call sites**: each becomes a loop over `0..segment_count`, binding that
   segment's buffers and firing its own indirect-count draw — skipping segments with zero visible
   chunks this frame (cheap check against the per-segment count written by the cull pass).
5. **Descriptor sets / push constants**: if any shader reads global vertex/index data by a byte
   offset assumed relative to a single buffer (rather than purely through `vertexOffset` in the
   indirect command, which is already per-draw and segment-relative), that assumption needs an
   audit. The existing `vertexOffset`-based design (v0.24.20-vertex-pack, already per-chunk) is
   actually the right shape for this — it should carry over unchanged, since it's already
   expressing "base index within THIS chunk's buffer region," which generalizes to "within this
   segment" for free. This is the one place I'd guess the blast radius is smaller than it looks.

## What I'd verify, in order, if I had a GPU

1. `cargo check` the whole tree once actually wired (need rustc ≥1.78, which this sandbox
   doesn't have — same limitation noted throughout this session).
2. Force a tiny `segment_capacity` (e.g. 4MB) via a debug override so a normal play session
   triggers growth within seconds, and confirm: no dropped meshes, no z-fighting/garbage geometry
   from a wrong segment index, shadow pass and main pass agree (they must both loop the same way).
3. `UNENDLICH_OCTREE_VERIFY=1`-style GPU↔CPU parity check adapted for this, if one doesn't already
   exist for the mesh pool — confirm segment N's buffer contents match what was written.
4. Profile: does looping per-segment measurably regress frame time when there's only 1-2 segments
   (the common case)? Should be near-zero — same total draw calls, just grouped — but only a real
   run confirms it.
5. Confirm `evict_distant`/LRU eviction still frees segments' worth of space correctly, and that a
   segment that empties out completely could (later optimization, not this pass) be freed back to
   the driver rather than kept allocated forever — VulkanMod's AreaBuffer supports this; this
   implementation doesn't shrink today (`SegmentedSpanAllocator` has no segment-removal path,
   deliberately, since shrinking safely needs to know no in-flight frame still references that
   segment's `vk::Buffer` — a `frames_in_flight`-gated deferred-destroy, same pattern this file
   already uses for `pending_destroy`).

## Honest scope note

Steps 1-2 above (`VulkanMeshPool` + `MeshSlot` changes) are moderate, mechanical, and I'd be
willing to write them blind-but-careful if asked — they're data-structure surgery, not draw-order
surgery. Steps 3-4 (indirect commands + six draw-call-site loops) are where a wrong assumption
produces silent visual corruption instead of a compile error, which is exactly the class of bug
this project's own log has hit repeatedly on render code (r175b, r182b, r234, r245's parallax
regression) — that's the part I'd genuinely want a build-and-look loop for before landing.
