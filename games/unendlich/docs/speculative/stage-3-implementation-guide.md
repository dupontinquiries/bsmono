# Stage 3 implementation guide: DEVICE_LOCAL mesh pool + staging buffer ring

## Why this is in /speculative/ not /claude-notes/

The previous sessions established this is the right architectural
move; this session's verification reads confirmed it; the design
memo lays out the high-level path. But the Stage 3 code itself
(estimated 600-800 LoC of Vulkan with real synchronization risk)
should NOT be written and shipped in a hardware-less session.

A Vulkan sync mistake is silent failure mode:
- Wrong vertices drawn (race between write and read)
- Validation layer errors only surface at runtime
- GPU hangs that require driver restart
- Worst case: corrupt mesh buffers that look intermittent

This guide is the handoff to a session with `cargo build` and
`cargo run` available, so each sub-stage can be verified
incrementally.

## What was confirmed by reads this session

- `crates/unendlich-engine/src/render/mesh_buffer_pool.rs:367` —
  the mesh pool allocates `HOST_VISIBLE | HOST_COHERENT` memory
  and maps it persistently. CPU writes go straight to that memory.
  No HOST_CACHED variant. No DEVICE_LOCAL variant.
- `crates/unendlich-engine/src/render/context.rs:170-185` — the
  context already has a `compute_queue_is_distinct` flag that's
  `false` on AGX/Honeykrisp (single queue family, single queue
  per family). Stage 3 cannot get queue-level parallelism from
  hardware on this device.

What Stage 3 buys on this hardware:
1. **Move CPU memcpy off the render thread** (big win — render
   thread no longer pays the memcpy cost). This requires off-thread
   work but no queue parallelism.
2. **Switch to DEVICE_LOCAL device-side memory** (big win — the
   cache-coherency cost on GPU read goes away because the GPU is
   reading from its own memory, not host-mapped memory).
3. **Use vkCmdCopyBuffer for the host→device transfer** (modest
   win — the driver may schedule the copy more efficiently than
   a CPU-mapped write, even on single queue).

## Recommended sub-staging

To make this verifiable on hardware in small steps:

### Sub-stage A: Add the DEVICE_LOCAL+staging path alongside the existing

Goal: have BOTH paths available, flagged by env var. Default to the
existing HOST_COHERENT path. Verify the new path works at all before
trusting it.

Estimated effort: ~250 LoC.

Files to touch:

1. **`crates/unendlich-engine/src/render/mesh_buffer_pool.rs`**

   Add a parallel struct `DeviceLocalMeshPool` (or extend the
   existing `CpuMeshPool` with a `backing: MeshPoolBacking { ... }`
   variant). The new struct holds:
   - DEVICE_LOCAL vertex buffer (`vk::Buffer` + `vk::DeviceMemory`),
     not mapped
   - DEVICE_LOCAL index buffer, not mapped
   - One or more HOST_VISIBLE staging buffers (sized to a fraction
     of the device buffers — start with 4 MB each, two in a ring,
     for double-buffering)
   - The same allocator state (free spans, slots) — the allocator
     decisions don't change between memory types

   Reference for memory type selection: replicate the
   `create_persistent_buffer` pattern at line 346 but change:
   ```rust
   // Was:
   vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
   // For device buffer:
   vk::MemoryPropertyFlags::DEVICE_LOCAL
   // For staging buffer:
   vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_CACHED
   ```

   HOST_CACHED (not HOST_COHERENT) for staging — CPU writes are
   fast because the cache absorbs them. Cache is flushed before
   the GPU reads via `vkFlushMappedMemoryRanges` OR via an
   explicit memory barrier (preferred).

   Buffer usage flags:
   ```rust
   // Device buffer:
   vk::BufferUsageFlags::VERTEX_BUFFER
       | vk::BufferUsageFlags::TRANSFER_DST
   // Staging buffer:
   vk::BufferUsageFlags::TRANSFER_SRC
   ```

2. **`write_at` becomes a two-step operation**:

   Old:
   ```rust
   pub unsafe fn write_at(&mut self, slot: Slot, verts: &[u8], indices: &[u8]) {
       std::ptr::copy_nonoverlapping(verts.as_ptr(),
           self.vertex_ptr.add(slot.vert_offset as usize), verts.len());
       // ...
   }
   ```

   New: memcpy into a staging buffer's free region, then queue a
   pending `CopyRegion { src_offset, dst_offset, size }` for both
   vertex and index. The caller (renderer) is responsible for
   flushing pending copies into a cmd buffer before the GPU reads.

3. **`crates/unendlich-engine/src/render/world_renderer.rs:upload_mesh`**

   Currently calls `mesh_pool.write_at(...)`. In sub-stage A, the
   write_at now ONLY records pending copies (no actual GPU work
   yet). Then before the draw, the renderer must call
   `mesh_pool.flush_pending_copies(cmd_buffer)` which:
   - Records `vkCmdCopyBuffer` for each pending copy
   - Inserts a memory barrier:
     ```rust
     // Transition vertex/index buffer from TRANSFER_WRITE to VERTEX_INPUT_READ
     let barrier = vk::BufferMemoryBarrier::builder()
         .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
         .dst_access_mask(vk::AccessFlags::VERTEX_ATTRIBUTE_READ
                        | vk::AccessFlags::INDEX_READ)
         .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
         .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
         .buffer(vert_buffer)
         .offset(0).size(vk::WHOLE_SIZE);
     device.cmd_pipeline_barrier(cmd,
         vk::PipelineStageFlags::TRANSFER,
         vk::PipelineStageFlags::VERTEX_INPUT,
         vk::DependencyFlags::empty(),
         &[], &[*barrier], &[]);
     ```

4. **Env flag wiring**: `UNENDLICH_DEVICE_LOCAL_MESH=1` to select
   the new backing. Default OFF for safety. Log on init:
   ```
   mesh_pool: backing=device_local (staging_size=4MB ring=2)
   ```

#### Verification for Sub-stage A

```bash
# 1. Build clean
cargo build --release -p unendlich-engine --features render

# 2. Run with validation layers enabled (catches sync errors)
VK_INSTANCE_LAYERS=VK_LAYER_KHRONOS_validation \
UNENDLICH_DEVICE_LOCAL_MESH=1 \
  cargo run --release --bin unendlich-world --features="render,gpu,window"

# 3. Walk around the world. If chunks render correctly AND no
#    validation layer warnings spam, the sync model is right.
#    If chunks render WRONG (wrong colors, gaps, garbled), the
#    transfer-to-vertex barrier is incorrect.
#    If validation spams about VUID-vkCmdDraw-* — buffer state is
#    being read while in transfer state. Add barriers or reorder.

# 4. A/B perf comparison (with submit-skip, slack-scheduler, etc):
# Baseline:
$COMMON UNENDLICH_ADAPTIVE_UPLOADS=1 UNENDLICH_SUBMIT_SKIP=1 \
        UNENDLICH_SLACK_SCHEDULER=1 \
  cargo run --release ...

# With device-local backing:
$COMMON UNENDLICH_ADAPTIVE_UPLOADS=1 UNENDLICH_SUBMIT_SKIP=1 \
        UNENDLICH_SLACK_SCHEDULER=1 \
        UNENDLICH_DEVICE_LOCAL_MESH=1 \
  cargo run --release ...
```

Expected: GPU spikes should drop substantially. `wait_fences` EMA
should drop. The cache-coherency-cost-on-GPU-read hypothesis is
tested directly.

### Sub-stage B: Move the memcpy off the render thread

Goal: a separate thread owns the CPU memcpy into staging. The
render thread only records the vkCmdCopyBuffer.

Estimated effort: ~200 LoC.

Architecture:

```
   meshing_pool.drain ─────► upload_thread (memcpy to staging)
                                       │
                                       │ writes to ring + records
                                       │ "ready slot" via channel
                                       ▼
                              ┌─────────────────┐
                              │ pending_copies  │ (Mutex<Vec<CopyRegion>>)
                              └─────────────────┘
                                       │
                                       │ drained by
                                       ▼
                              render_thread:
                                  recordsvkCmdCopyBuffer
                                  + barrier
                                  + draw
```

Files to touch:

1. New file `crates/unendlich-engine/src/render/upload_thread.rs`:
   - Owns the staging ring
   - Pulls mesh results from a channel fed by the bin's upload phase
   - Memcpys into the next free staging slot
   - Pushes a CopyRegion entry to `pending_copies`
   - Notifies the render thread that a copy is ready

2. `world_renderer.rs`: at the top of `draw_frame_with_view`,
   drain `pending_copies` and record copy commands + barrier
   into the frame's cmd buffer.

3. `unendlich_world.rs`: spawn the upload thread at startup;
   send drained MeshResults to it instead of calling
   `upload_prebuilt_mesh_at_lod` directly. The upload phase
   becomes a `mesh_pool.drain → upload_thread.send` loop.

#### Verification for Sub-stage B

- Validation layer must not complain about cross-thread cmd buffer
  recording (you should NOT be recording from the upload thread —
  the upload thread only memcpys + queues, the render thread
  records).
- Fence-wait should drop further as the memcpy cost is off the
  critical path.

### Sub-stage C: Optimize staging ring sizing

Goal: avoid stalls when the staging ring is full.

If the upload thread tries to memcpy a new mesh but all staging
buffers are in flight, it must wait for the GPU to consume one.
This wait IS latency — a slow GPU frame stalls upload throughput
which stalls subsequent uploads.

Mitigation strategies (pick one based on profiling):
- Increase ring size (e.g., 4 buffers × 4 MB = 16 MB staging)
- Track per-buffer fences so the upload thread can check
  individual buffer freedom without waiting on all
- Per-frame staging budget: cap uploads per frame so the ring
  always has slack

## Synchronization model summary

For sub-stage A (single thread, in-frame copy):

```
Frame N:
    upload_mesh(coord, mesh, ver)
        memcpy → staging[slot]            (CPU work, write-combined)
        queue CopyRegion(slot, dst)       (cheap)
    [more uploads...]

    draw_frame:
        cmd.begin
        for region in pending_copies:
            cmd.copy_buffer(staging, vertex_buf, &[region])
        cmd.pipeline_barrier(TRANSFER → VERTEX_INPUT)
        cmd.draw...
        cmd.end
        queue.submit(cmd)
    [draw completes; staging slots are reusable next frame]
```

For sub-stage B (upload thread, async copy):

```
Upload thread (continuously):
    result = recv from channel
    free_slot = wait_for_staging_slot()  // blocks if ring full
    memcpy result → staging[free_slot]
    pending_copies.lock().push(CopyRegion {...})
    signal render-thread

Render thread (per frame):
    let regions = pending_copies.lock().drain(..)
    cmd.begin
    for region in regions:
        cmd.copy_buffer(staging, vertex_buf, &[region])
    cmd.pipeline_barrier(TRANSFER → VERTEX_INPUT)
    cmd.draw...
    cmd.end
    queue.submit(cmd, signal_fence)
    [Frame N+1 acquires the slot back when fence signals]
```

## Open design questions

1. **Memory size**: how big should the DEVICE_LOCAL buffer be?
   Current HOST_COHERENT buffer is sized via `MESH_POOL_VERT_MB`
   and `MESH_POOL_IDX_MB` (check Cargo.toml or bin for current
   defaults). Same sizes should work for DEVICE_LOCAL since the
   buffer purpose is identical.

2. **Staging buffer count**: trade-off between memory use and
   stall freedom. Default 2 (double-buffer) is safe but might
   stall under burst loads. Default 4 (quad-buffer) costs 16 MB
   total but absorbs more bursts.

3. **HOST_CACHED vs HOST_COHERENT for staging**: HOST_CACHED is
   faster for CPU writes (cache absorbs) but requires explicit
   flush before GPU reads. HOST_COHERENT is slower for writes
   (write-combining only) but no flush needed. Default HOST_CACHED
   for sub-stage A; revisit if flush cost dominates.

4. **What if DEVICE_LOCAL allocation fails?** Some GPUs have very
   limited DEVICE_LOCAL pools. Fallback path: stay on the
   HOST_COHERENT memory; log a warning. Should not crash.

## Rollback plan

If sub-stage A introduces bugs:

```bash
# Unset the env to fall back to HOST_COHERENT path:
unset UNENDLICH_DEVICE_LOCAL_MESH

# Or revert the code:
git checkout HEAD -- crates/unendlich-engine/src/render/mesh_buffer_pool.rs \
                     crates/unendlich-engine/src/render/world_renderer.rs
```

The env-gated path means a code-incomplete sub-stage A can be
"merged" without affecting normal operation. Make sure the flag
default is OFF and the code path with flag=OFF is identical to
pre-RC34.

## What to ship vs. defer

- **Ship sub-stage A in RC34**: ~250 LoC, single-threaded, fully
  testable on hardware.
- **Defer sub-stages B + C**: only pursue if sub-stage A shows
  the predicted improvement. If sub-stage A doesn't help, the
  cache-coherency hypothesis was wrong and a different
  architectural direction is needed.

## Cross-references

- `docs/speculative/upload-scheduler-design.md` — overall design
  with 5 options (Stage 3 = Option C of that memo).
- `docs/architecture-v1.md` — strategic destination (GPU-resident
  world). Stage 3 is the architectural stepping-stone.
- `docs/v0.24.3-chug-investigation-plan.md` — predecessor session's
  "Path 1: Eliminate chugs entirely" which Stage 3 implements.
- `crates/unendlich-engine/src/render/mesh_buffer_pool.rs:346` —
  the function to fork into a DEVICE_LOCAL variant.
- `crates/unendlich-engine/src/render/world_renderer.rs:1266` —
  `upload_mesh`, where the new flush-pending-copies call goes.
