# perfbisect — desktop findings (2026-09-01) and what scaling to 20–60 chunks needs

Source: `desktop.tsv` + `run_summaries.txt` + `events.jsonl` from
`games/shared/perfbisect run` on the M2 Ultra (Asahi, Honeykrisp, Mesa
25.3.6, Wayland, 24 cores). `unendlich-fullgame`, 18 variants × 12 s,
render distance 6 (default).

## The desktop is not slow. It hitches.

Steady GPU frame: **3.9 ms p50 in every variant.** Half render scale:
3.83. Shadows 512 → 4096: 4.36 → 3.96. Fog off, validation on, all
three implicit layers (MangoHud, gamescope-WSI, device_select) forced
off: no change. The machine is not fragment-, CPU-, or layer-bound.

The p99 is **0.8–1.6 s**. `events.jsonl` has five `gpu_spike` events at
7.0, 7.2, 7.8, 8.9, 10.5 s — after `warmup_complete` at 6.99 s — with
`main_us` 450 000–860 000, while `frame_window` shows loaded chunks
going 3 → 22. The CPU `acquire` timer maxes at 1570 ms at the same
moments. Every chunk arrival stalls the frame for up to a second.

**The knob:** `mesher.cpu` (`UNENDLICH_GPU_MESHER=0`) is the only variant
with a sane p99 — draw 4.61 ms, frame_total 12.86 ms. Every variant
keeping the default `GPU_MESHER=2` has draw p99 in the hundreds of ms.

## The mechanism, from the code

`render/gpu_meshing_octree.rs:820–867` (`record_submit_wait`) and
`world/gpu_generator.rs:260–268` (`generate_batch_inner`) have the same
shape:

    record command buffer → queue_submit(self.queue) under
    VulkanContext::queue_submit_lock → wait_for_fences(u64::MAX)

`self.queue` is the graphics queue. A batch of up to
`UNENDLICH_GPU_MESH_BATCH` (default 64; the rd24 preset says 256)
chunks is one dispatch, in order on the same queue as the frame. The
frame's main pass is queued behind it and its timestamps span it. On
Honeykrisp a 64-chunk octree mesh dispatch takes 0.5–1 s, so the frame
does too. On a driver with a fast mesher the same code costs a few ms
and nobody notices — which is why the phone is "fine": either it takes
the CPU path or its per-chunk cost is small. The wait on the worker
thread is not the problem; the serialization on the queue is.

The GPU generator: `world-bench` measured the GPU path at **0.63–0.98×
the CPU path** and took 422 s to say so. Same submit-and-wait, batch 16,
plus a host-visible readback per batch, against 24 cores of rayon.

Indirect draw: the cull compute is bounded by `high_water` (0.04 ms)
and is not it; turning `UNENDLICH_INDIRECT=0` takes the GPU frame from
3.85 to 2.44 ms at 31 chunks. The suspect is the driver's path for
`cmd_draw_indexed_indirect_count` on Apple hardware, which has no
native multi-draw-indirect-count. Not measured at high chunk counts,
where indirect should start winning — run `dist.far` with and without
it to find the crossover.

## What shipped for it

- **`UNENDLICH_PROFILE=honeykrisp`** (`runtime_flags::profile`): CPU
  mesher, CPU generator, indirect off, roamer budget 4 ms. A *driver*
  preset, not a quality one; the measured answer until the submissions
  are asynchronous.
- **`sim::ai_budget::FrameBudget`** — a wall-clock box on the whole
  roamer pass (`UNENDLICH_ROAMER_BUDGET_MS`, default 4). `roamer_tick`
  measured 12–16 ms p50 in four of eighteen runs and 2–4 in the rest:
  the same code, a different herd. Once the box is spent only creatures
  within 24 blocks tick; the rest fall through to the r265 catch-up path
  (they never record `last_tick_frame`), so they integrate the missed
  ticks next time, nothing freezes. Wired in `unendlich_fullgame.rs`
  before the curfew gate; `roamer_ran`/`roamer_skipped` are profiler
  zones. The ms→s unit bug in the first version was caught by its test.
  **The bin wiring is not compiled in the session that wrote it** (the
  sandbox cannot build `render window`); the library half is, and
  tested (11/11).

## What scaling actually needs

Numbers, so the target is a number and not a feeling:

| render distance (chunks) | chunks in the sphere | full-res voxels | at 220 chunks/s (4 CPU workers × 18 ms) |
|---|---|---|---|
| 6 (today) | ~900 | 3.0 × 10⁷ | 4 s |
| 12 | ~7 200 | 2.4 × 10⁸ | 33 s |
| 20 | ~33 500 | 1.1 × 10⁹ | 2.5 min |
| 60 | ~905 000 | 3.0 × 10¹⁰ | 69 min |

(32³ chunks; the sphere is 4/3·π·r³.) The LOD bands (`8,16,32` at
factors `1,2,4,8`) cut voxels, not chunk COUNT: at rd 60 the vfar shell
alone is ~180 000 meshes against an indirect pool clamped at 65 536
slots and a mesher that meshes chunks one at a time. So, in order:

1. **Async submission for both GPU paths.** A ring of N in-flight
   batches, each with its own fence; submit without waiting; the pool
   thread polls fences and unpacks results while the GPU runs the next
   batch. Where the device has a second queue family with COMPUTE, put
   the ring on it (then the frame never queues behind a batch at all).
   Where it has one family (Honeykrisp probably; check
   `queue_family_properties`), size each batch by a GPU-time budget
   (`UNENDLICH_GPU_MESH_BUDGET_US` exists; make it the batch sizer) so
   no single dispatch exceeds ~2 ms in front of the frame, and submit
   it after the present. The functions to change are the two named
   above; the rest of both modules stays.
2. **Per-chunk mesh cost on Honeykrisp** — 64 chunks in 0.5–1 s is
   8–15 ms a chunk on a GPU, slower than one CPU worker. Profile the
   octree shader there (workgroup memory, divergence in the octree
   walk). Until it is under ~1 ms a chunk the GPU mesher loses to the
   CPU one on this driver regardless of scheduling.
3. **Merged far LOD.** One mesh per 2×2×2 (mid), 4×4×4 (far), 8×8×8
   (vfar) chunk block — the Distant Horizons shape. It is what turns
   180 000 vfar meshes into ~350, and it is the only thing that makes
   rd 60 an indirect-slot count the pool can hold. `LOD_BANDS` and the
   per-LOD mesher timers are the seams; the merge is new.
4. **Mesher throughput.** 220 chunks/s fills rd 12 in half a minute;
   it does not fill rd 20. Both meshers need an order of magnitude:
   the CPU one by meshing the merged LOD blocks (fewer, coarser), the
   GPU one by (1) and (2).
5. **The roamer pass, per creature.** The budget bounds the pass; it
   does not make a creature cheaper. 0.6 ms per creature per frame is
   the number to cut, and `roamer_blockscan`/`roamer_herdscan` read 0
   in every run — the sub-timers are declared and not fed, so the
   cost inside the loop is unattributed. Feed them first. The same
   budget + band pattern is what blackstar's field creatures want; the
   `FrameBudget` is std-only and could move to `games/shared`.

## What the phone report will settle

If `mesher.cpu ≈ mesher.gpu` there and `gpu_spikes` is 0 in both, the
stall is Honeykrisp-specific and (1)+(2) are the desktop's work. If the
phone's `roamer_tick` p50 is in the tens of ms, (5) is everyone's.
