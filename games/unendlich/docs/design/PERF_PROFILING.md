# Performance Profiling & Stutter Diagnosis

How to find what's actually costing frames before changing anything. The
golden rule: **measure first.** Stutter/chug fixes applied blind usually move
the cost around or make it worse; the percentile profiler turns "it chugs"
into "zone X has a p99 of 40ms" so we fix the real bottleneck.

## Turn it on

```
UNENDLICH_FRAME_PROFILE=1 ./run-latest.sh
```

Every 5s the log prints a per-zone percentile table (worst p99 first):

```
[profile] per-zone percentiles (ms):
zone=manager_update n=300 mean=3.2 p50=2.1 p95=9.0 p99=38.0 max=120.0
zone=draw           n=300 mean=6.8 p50=6.5 p95=8.1 p99=11.0 max=24.0
zone=tick_sim       n=300 mean=0.9 p50=0.4 p95=2.0 p99=6.0  max=18.0
zone=frame_total    n=300 mean=11  p50=10  p95=18  p99=46   max=131
```

Zero runtime cost when the env var is unset (the per-frame `record` is a
cheap bucket increment that still runs, but the dump — the only expensive
part — is gated).

## Reading it — the three phases

The four zones map to the frame's real work:

- **`manager_update`** — chunk streaming + GPU upload
  (`chunk_manager.update_with_interest_points`). **The #1 stutter suspect.**
  Spikes here are the classic "freeze on movement": crossing a chunk boundary
  triggers a burst of meshing/upload that doesn't fit one frame. A high p99
  with a low p50 = occasional bursts, not steady cost.
- **`tick_sim`** — world simulation (`tick_simulation`): plant lifecycle, leaf
  decay, fluids, roamers. Spikes = a heavy tick handler firing on its cadence.
- **`draw`** — `draw_frame_with_view…`: vkQueueSubmit + acquire + present,
  *including GPU sync waits*. If draw p99 is high but submit/upload look
  healthy, the time is driver-level sync / swapchain (FIFO) wait / a GPU
  pipeline stall on a resource still being written.
- **`frame_total`** — the whole loop iteration. `1000 / p50` ≈ typical FPS;
  the gap between p50 and p99 IS the stutter you feel.

- **`apply`** (r242) — the deferred block/state edits landing in chunks during
  the tick (the chunk_manager's own `last_apply_us`). On a loading wave this is
  the dominant `tick_sim` spike; if `apply` p99 is high, lower
  `UNENDLICH_APPLY_BUDGET` (caps chunks applied per tick, carries the rest).
- **`upload`** (r242) — GPU buffer upload of meshed chunks. A sub-phase of the
  frame; if its p99 spikes, lower `UNENDLICH_UPLOAD_BUDGET_MS` /
  `UNENDLICH_MAX_UPLOADS_PER_FRAME`. Distinguishes an upload spike from a
  meshing (`manager_update`) or apply one.

The phase whose p99/max dominates `frame_total`'s tail is the one to attack.

## The usual suspects → the existing levers

Most perceived chug is `manager_update` bursts. The engine already exposes
tuning for exactly this (set in `run-latest.sh`'s env, or override via
`EXTRA_ENV`):

- `UNENDLICH_MAX_UPLOADS_PER_FRAME`, `UNENDLICH_UPLOAD_BUDGET_MS` — cap how
  much upload work a frame does (smaller = smoother but slower to fill in).
- `UNENDLICH_GPU_MESH_BATCH`, `UNENDLICH_APPLY_BUDGET`, `UNENDLICH_EVICT_BUDGET`
  — per-frame meshing/apply/evict caps (the "spread the burst over N frames"
  knobs).
- `UNENDLICH_RENDER_DISTANCE`, `UNENDLICH_LOD_BANDS`, `UNENDLICH_LOD_FIDELITY`
  — less world in flight = less to stream/draw.
- `UNENDLICH_PREFETCH*` — fetch ahead of movement so the burst is amortised
  before you arrive rather than hitching when you cross.
- `UNENDLICH_TARGET_FPS` + autotune (Settings → Performance) — lets the engine
  raise strides / lower budgets automatically when it falls below target.

Methodology: profile → identify the dominant tail zone → adjust *that zone's*
budget knob → re-profile and confirm p99 dropped without tanking p50. Change
one lever at a time.

## Also available

- GPU-side per-zone timers dump at shutdown (`flush_gpu_timer_summary`) — the
  GPU half of `draw`, with its own percentiles/box-plots.
- CPU timer summary at shutdown (`flush_cpu_timer_summary`).
- `crates/unendlich-engine/src/profiler.rs` is the implementation (histogram
  buckets + percentiles, unit-tested); reuse `prof.time("zone", || …)` to wrap
  any other suspect region.

## Wired in (r237)
`unendlich_fullgame.rs` feeds `manager_update` / `tick_sim` / `draw` /
`frame_total` into a `Profiler` each frame and dumps every 5s under
`UNENDLICH_FRAME_PROFILE=1`. This is the measurement substrate for the perf pass;
the actual optimisations follow from what it shows.
