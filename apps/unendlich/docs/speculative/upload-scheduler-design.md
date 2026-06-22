# Speculative: decoupling chunk uploads from the render pipeline

## What problem this is solving

User report (paraphrased): "I want to decouple the jarring uploads
from the immediate rendering pipeline but NOT with a frame-time queue
because that adds latency. There has to be a more reasonable way. I
was thinking of a scheduler."

The "jarring" is the chug pattern surveyed exhaustively in the v0.24.3
"chug arc" docs (`docs/v0.24.3-chug-debug-1-readme.md`,
`-chug-debug-2-readme.md`, `-chug-investigation-plan.md`,
`-upload-pressure-readme.md`). Past sessions established:

- Apple AGX (M2 Max via Asahi / Honeykrisp Mesa) exposes ONE queue
  family. No dedicated transfer queue available. ("multi-queue-a"
  confirmed.)
- The current mesh upload is **CPU memcpy into HOST_VISIBLE |
  HOST_COHERENT persistently-mapped GPU memory** — no staging
  buffer, no `vkCmdCopyBuffer`. (See `world_renderer::upload_mesh`
  + `mesh_pool::write_at`.)
- The chug hypothesis: a CPU write to HOST_COHERENT memory forces
  cache-coherency work the GPU pays on its next read of the same
  buffer. Both shadow + main draws read the same pool. The chug
  shows as correlated shadow+main spikes.
- `MAX_FRAMES_IN_FLIGHT = 1` — single-frame-in-flight, no pipeline
  buffer absorbing GPU spikes.
- Strategic direction (architecture-v1): "GPU-resident world plus
  procedural reconstitution upload." World data lives on GPU,
  client only uploads deltas. Big refactor; not blocking on this.

RC29's `AdaptiveUploadController` already paces the cap dynamically
(EMA of `wait_fences_ms` drives the cap up or down). That's a
**rate-limiter scheduler**, but uploads still happen on the render
thread, still write into HOST_COHERENT memory, and still serialize
with the next GPU read.

## What "not a frame-time queue" rules out

A naive fix is "buffer N frames of uploads, present them all together."
This adds N frames of latency: a freshly meshed chunk appears N frames
after its mesh is ready. At 60 FPS, N=2 is ~33ms of added eye-to-photon
delay. Past chug-investigation notes explicitly flagged this:

> Going to FIF=2 would hide upload pressure behind GPU work at the
> cost of +1 frame latency. Not advocating; just noting.

The user wants the decoupling without paying that latency.

## The scheduler space — five options

I'll lay them out from least-architectural-change to most.

### A. Slack scheduler (smallest delta, immediate win)

Idea: don't cap by a fixed `max_uploads_per_frame`. Instead, **upload
until you'd miss the frame budget**, then stop.

```rust
let frame_start = Instant::now();
let frame_budget = Duration::from_millis(8); // 120 FPS target
// ... do the rest of the frame work ...
let elapsed_so_far = frame_start.elapsed();
let upload_slack = frame_budget.checked_sub(elapsed_so_far)
    .unwrap_or(Duration::ZERO);

let upload_deadline = Instant::now() + upload_slack;
while Instant::now() < upload_deadline {
    if let Some(result) = mesh_pool.try_pop() {
        upload(result);
    } else {
        break;
    }
}
```

**Latency**: zero added — uploads happen this frame OR next frame, no
fixed queue depth.

**Decoupling**: weak. Uploads are still on the render thread, just
budgeted dynamically.

**Effort**: ~50 LoC in the bin's upload loop. Mostly a rewrite of the
existing fixed-cap drain.

**Risk**: low. The `upload_deadline` is a soft cap. Worst case the
deadline is missed and one frame stutters anyway (same as today).

**Value**: complements RC29 but addresses a different signal. RC29
reacts to past frames' GPU pressure; the slack scheduler reacts to
THIS frame's progress. Both could run together.

### B. Priority queue over the upload pool (medium delta)

Idea: the current upload loop drains `mesh_pool.drain_up_to(N)`. That's
FIFO order from the meshing pool. Replace with a priority-ordered
drain: **chunks the player can see > chunks they MIGHT see > distant
chunks**.

Priority signals (rank cheaply, no DashMap reads):
1. Within near LOD AND in the camera frustum → priority 0 (immediate)
2. Within near LOD, just outside frustum → priority 1
3. Within mid LOD → priority 2
4. Far/VeryFar → priority 3

Drain order: 0 first, then 1, etc. Within a priority tier, FIFO
preserves nearest-first because the meshing pool already submits in
shell order.

**Latency**: zero added — every chunk still uploads, just in a smarter
order.

**Decoupling**: zero. Same render-thread upload loop.

**Effort**: ~150 LoC. The mesh pool's result channel needs to either
emit priority along with results, or the bin needs to peek at the
chunk's LOD and frustum-test it before deciding to upload.

**Risk**: low-medium. Could starve low-priority chunks if high-priority
keeps arriving. Mitigation: ensure FIFO within a tier and add a
"max frames a chunk can wait" backstop (after which it bumps up a tier).

**Value**: real. Chunks the player is staring at land first. The
"jarring" feeling is most pronounced when the chunk you're walking
toward arrives late; this fixes that specifically.

### C. Off-thread upload scheduler (large delta, architectural)

Idea: a dedicated **upload thread** owns the upload loop. The render
thread does NOT call `upload_mesh`. Instead, the upload thread:

1. Pulls completed meshes from `mesh_pool`.
2. Maintains a priority queue (option B).
3. Has its own **ring of staging buffers** (HOST_VISIBLE | HOST_CACHED;
   not COHERENT — CPU-cached writes are fast).
4. CPU-memcpy's into a free staging buffer.
5. Records a `vkCmdCopyBuffer` from staging → DEVICE_LOCAL mesh pool
   into a **transfer command buffer**.
6. Submits the transfer cmd buffer to the (single) graphics queue
   independently of the render frame, OR feeds the recorded cmd
   buffer to the render thread which `vkQueueSubmit`s it before
   the render commands of frame N+1.

**Latency**: 0–1 frame added. Best case the transfer finishes in time
for the same frame's draws (semaphore-gated). Worst case it lands one
frame later. NOT a fixed N-frame queue — it's "as soon as the GPU is
free."

**Decoupling**: strong. The CPU memcpy is off the render thread. The
GPU-side vkCmdCopyBuffer is async (driver schedules it; on AGX single
queue it still serializes BUT the driver has more freedom to
interleave smaller copies between tile-bin batches).

**Effort**: large. Roughly:
- Spawn an upload thread (~50 LoC).
- Staging buffer ring + free/in-use tracking (~200 LoC).
- Mesh pool memory: HOST_COHERENT → DEVICE_LOCAL (architectural —
  need to verify the pool's allocator can do this; check
  `mesh_pool.rs`).
- Submit-side semaphore wiring so the render thread waits on
  upload completion (~100 LoC + careful Vulkan sync review).
- Probably another 200 LoC of bookkeeping and edge-case handling
  (cancellation when a chunk's version bumps mid-upload, etc).

Estimated 600-800 LoC + careful testing on real hardware.

**Risk**: medium-high. Vulkan synchronization is famously footgun-y.
On AGX single queue, the win may be smaller than on discrete GPUs.

**Value**: high if it works. This is the architectural step toward
the GPU-resident-world vision. Once mesh memory is DEVICE_LOCAL,
the cache-coherency-cost-on-GPU-read hypothesis goes away entirely.

### D. Predict-ahead visibility window (complement, not replacement)

Idea: don't wait for a chunk to be in the camera frustum to mesh it;
mesh chunks the player is *about to* see, based on velocity/heading.

The meshing pool already does this approximately via shell walks
around the player chunk. The improvement is **biasing the shell walk
toward the player's forward direction**.

```rust
// Bias factor: dot product of (chunk - player) with player.forward()
// Add a small ε to break ties favorably toward forward.
let forward_bias = (chunk_pos - player_pos).dot(player_forward).max(0.0);
let walk_order_key = chebyshev_distance + forward_bias * BIAS_WEIGHT;
```

**Latency**: negative (chunks land before they're needed).

**Decoupling**: zero — still same upload path.

**Effort**: ~100 LoC in the shell walk.

**Risk**: low.

**Value**: synergistic with A/B/C — gives them more time to settle
chunks before the user looks at them. Probably worth doing
regardless of upload strategy.

### E. Move uploads to a separate descriptor / draw path (largest delta)

Idea: a chunk going through "upload pending" state is drawn from
a different mesh buffer than "fully resident." The render pass
branches: if the chunk's upload is still pending, draw the previous
version's buffer (or skip and let the LOD coarser fill in). Once
upload completes, switch to the new buffer.

**Effort**: very large. Touches descriptor sets, indirect draw,
mesh pool's slot ownership.

**Value**: clean architectural model, but probably over-engineering
for the current pain point.

## Recommendation

**Stage 1 (next 1-2 sessions, low risk):**

1. Ship A (slack scheduler) as an env-gated RC.
2. Ship D (forward-biased shell walk) as an env-gated RC.

These are small, independent, and complement RC29's adaptive cap.
Each can be A/B-tested in isolation. Their combined effect: the
upload phase gets the time it needs without overrunning the frame
budget, AND the chunks that get uploaded are the ones the player is
about to see.

**Stage 2 (medium-term, after Stage 1 plateaus):**

3. Ship B (priority queue). Builds on D's frustum classification.
4. Profile what's left. If `upload_pressure` events are gone and
   chugs persist, the cache-coherency hypothesis is reinforced.

**Stage 3 (architectural):**

5. Ship C (off-thread upload + DEVICE_LOCAL backing). This is the
   real fix to the cache-coherency hypothesis. Should be done with
   careful Vulkan-sync review and hardware A/B verification.

**Stage 4 (strategic destination, separate arc):**

6. GPU-resident world (architecture-v1's vision). Uploads become
   sparse delta-only. The upload scheduler becomes an edit
   scheduler — fundamentally different problem shape.

## What I'd want before designing Stage 3

- Per-axis profiling data from a hardware run with diagnostic events:
  - `upload_pressure` event count vs `gpu_spike` event count
  - The ratio post-RC30 (RC29 + adaptive uploads + submit-skip)
- A test of mesh-pool memory type: confirm
  `HOST_VISIBLE | HOST_COHERENT` (not `HOST_CACHED`) via
  `vkGetPhysicalDeviceMemoryProperties`. If we're actually on
  COHERENT, the cache-coherency hypothesis is strongest. If
  HOST_CACHED with manual flush, the hypothesis weakens.
- Confirmation of AGX queue topology in current Mesa version
  (multi-queue-a was several months ago; Honeykrisp evolves).

## Cross-references

- `docs/v0.24.3-upload-pressure-readme.md` — diagnostic infrastructure
  + bytes budget knob. Still useful for measuring Stage 1/2 effect.
- `docs/v0.24.3-chug-investigation-plan.md` — strategic plan that
  predates this memo; this memo is the "concrete scheduler design"
  follow-up the plan called for.
- `docs/v0.24.3-chug-debug-1-readme.md`, `-debug-2-readme.md` —
  original hardware A/B that surfaced the chug pattern.
- `docs/architecture-v1.md` — long-term GPU-resident-world arc.
- `docs/claude-notes/v0.24.64-submit-skip-readme.md` — RC30's
  adaptive cap (RC29) is the closest prior art to a "scheduler."
- `crates/unendlich-engine/src/render/world_renderer.rs:1266` —
  current `upload_mesh` implementation.
- `crates/unendlich-engine/src/render/mesh_pool.rs` — `write_at`
  is where the actual memcpy happens.

## Open questions

- Does the mesh pool's `write_at` issue a memory barrier after the
  memcpy? If yes, that's already serializing. If no, the GPU read
  is racing the CPU write and the cache cost is happening on first
  read regardless. Worth checking.
- Are there driver-level hints we can give (Vulkan
  `VK_AMD_buffer_marker` equivalent, Mesa-specific extension) for
  "this region was just CPU-written, please prefetch"? Unlikely
  to exist for AGX but worth asking the Honeykrisp folks.
- Does Honeykrisp expose a TRANSFER-bit-only queue, even if it's
  in the same family as graphics? If yes, even on the same queue
  family we might get scheduling benefits by submitting transfer
  cmd buffers to a separate vkQueue. Worth re-checking.
