# Speculative: async (pipelined) octree-mesher dispatch

Status: DESIGN + headless core only. The Vulkan wiring is deliberately NOT
written — see "Why not written blind" below. Companion code:
`render/mesh_dispatch_ring.rs` (pure in-flight ring state machine, unit-tested
headlessly).

## Problem this would solve — and the gate before building it

Post r-t1-pool-pin, the octree mesher's fence wait no longer blocks the
render/main threads. What remains is dispatcher-side LATENCY: each batch is a
synchronous submit → wait round-trip measured at 38–171 ms/chunk wall time on
Asahi (log-r462), which caps chunk pop-in rate, not frame time.

**GATE: do not build this until the pin A/B has run on hardware.** Two
outcomes cancel the whole project:
1. `UNENDLICH_GPU_POOL_PIN=1` fixes frames AND pop-in at the target RD feels
   fine → async dispatch is complexity with no user-visible payoff.
2. The `[mesh-stall]` warns show the wall time is dominated by the GPU
   actually executing (per-chunk shader cost), not submit-to-signal latency →
   pipelining submits won't help; the shader (or batch composition) is the
   target instead.
Async dispatch only pays off in the third outcome: frames fixed, pop-in still
too slow, and the stall warns show latency (small execution time, large
signal delay) — i.e. throughput is round-trip-bound.

## Ways this can make things WORSE (why it is not free)

- **Frame pacing regression.** The synchronous round-trip is an accidental
  throttle: between batches the single Asahi queue has idle gaps where render
  frames slot in. Overlapped batches fill those gaps — higher mesh throughput,
  deeper queue occupancy, and present-latency spikes. This can resurrect the
  exact hitch class the r708 per-frame budget was built to prevent.
- **Control-loop invalidation.** Adaptive batch sizing (r476), `yield_pct`,
  and the r708 budget all key off `dispatch_us` of a SYNCHRONOUS wait. With
  overlap that number stops meaning "GPU time consumed": the budget
  double-counts (sum of overlapping intervals > wall time) or under-counts,
  and the shrink/grow logic can oscillate. The budget must move to a
  completion-driven accounting (charge GPU time at fence-signal, estimated as
  signal_time − max(submit_time, previous_signal_time)).
- **Output-buffer aliasing.** The mesher owns ONE command buffer, ONE fence,
  and ONE counter/vertex/index buffer set. In-flight batches must each own a
  ring slot (own cmd buffer, own fence, own output region) or they race.
  Failure mode is garbled meshes / VUID spam / GPU hang — silent until
  hardware runs it.
- **Pin dwell time.** Pins now span "submit → this batch's fence", but with N
  batches in flight, total pinned footprint grows ~N×, and deferred evictions
  dwell longer. Pool-full upload failures become more likely under streaming
  bursts. Mitigation: cap in-flight batches (start N=2) and surface
  `pin_stats()` in the 1s log.
- **Unified-memory pressure.** N× output regions on the M2 compete with the
  mesh pool + octree pool + framebuffer. Ring slots should be sized to the
  ADAPTIVE batch target, not MAX_BATCH_CHUNKS.

## Architecture (when the gate passes)

Ring of `N` dispatch slots (start N=2, env-tunable):

```
slot k: { cmd_buffer_k, fence_k, counter/vertex/index region k,
          pinned_offsets_k, submitted_at_k }

dispatcher loop:
  1. drain completed slots: for each slot whose fence is SIGNALED (poll,
     timeout 0): readback region k → send results → unpin(pinned_offsets_k)
     → release slot.
  2. if a batch is ready AND a slot is free AND budget allows: upload+pin
     under pool lock → record cmd_buffer_k → submit(fence_k) → continue
     (NO wait).
  3. if no slot free: wait on the OLDEST in-flight fence (bounded wait —
     this is the natural backpressure), then drain.
```

The pure slot lifecycle (acquire / submit / poll-complete / release, budget
accounting, backpressure decision) is `mesh_dispatch_ring.rs` — written and
tested now so the Vulkan session only wires `vk::Fence` polling and per-slot
resources into an already-verified state machine.

## Sub-staging for the hardware session

- **A (ring of 1):** mechanical refactor onto the ring with N=1 — behavior
  identical to today (submit → wait → readback), validating the plumbing with
  zero overlap risk. Validation layers must be silent.
- **B (N=2, poll-driven):** real overlap. A/B against sub-stage A with the
  stall warns + LONG FRAME counts + pop-in rate (chunks/sec from the
  octree-mesher 10s diag line). REVERT if frame p99 regresses even when
  pop-in improves — frames win.
- **C (budget rework):** completion-driven GPU-time accounting replacing the
  r708 window math, only if B ships.

Env: `UNENDLICH_GPU_MESH_INFLIGHT=1` (default 1 = today's behavior; 2 enables
overlap). Rollback = unset.

## Why not written blind

The Stage-3 guide's rule applies with more force here than it did to Stage 3
itself: a wrong sync model in this change produces intermittent garbled
meshes or hangs, is invisible to reading, and is debugged iteratively against
validation layers. Blind-written async Vulkan is likely rewritten on hardware
— negative work. The headless ring core is the part that survives.
