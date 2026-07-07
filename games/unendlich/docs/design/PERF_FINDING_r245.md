# Perf Finding from the r245 Profile (and the r246 follow-up)

> Reads with `PERF_PROFILING.md`. Based on the operator's first real
> `UNENDLICH_FRAME_PROFILE=1` capture (22 dumps, ~14k–18k frames each).

## The data (steady across every dump)
```
zone=frame_total  p50=6.1ms  p95=60ms  p99=76ms  max=8128ms
zone=draw         p50=2.6ms  p95=4.8ms  p99=5.3ms max=83ms
zone=tick_sim     p50≈0      p95≈0      p99=0.01  max=74.6ms
zone=manager_update / apply / upload  ≈0 everywhere
```

## What it means
- **Median frame 6.1ms (~164 fps)** — when the engine is smooth, it's *fast*.
- **p95 frame 60ms (~17 fps)** — this is the stutter the operator feels: ~1 in
  20 frames takes 10× the median.
- **The spike is NOT in any work zone.** On the slow frames, `draw` is still
  ~5ms and `tick_sim`/`manager_update`/`apply`/`upload` are ~0. So **~55ms of
  every p95 frame is spent OUTSIDE everything measured so far.**
- `draw` includes `acquire_next_image` + `queue_present` (verified), so this is
  **not** vsync/present blocking — present would land in `draw`.
- `max=8128ms` (8s) on frame_total is the one-off world-load / window-event
  hitch, not the recurring stutter.

## Conclusion: it's the untimed game-logic remainder
The frame loop does a lot between "frame start" and the draw call that no zone
covered: input/events, and especially the per-frame **roamer / dragon / fauna /
drops / entity AI** updates (multiple loops over active roamers, dragon
head-tracking/observe, fauna flee checks, drop physics). `chunk_manager`'s
`tick_simulation` is ~0, but that's only the chunk sim — the *creature* AI is
separate and untimed.

## r246: localize it
Added a `game_update` zone = wall-time from frame top up to the draw call (all
the pre-draw loop work). Re-run `UNENDLICH_FRAME_PROFILE=1`:
- If **`game_update` p95 ≈ 55–60ms** → confirmed: the stutter is the per-frame
  creature AI / loop logic. Next step: sub-time the roamer and dragon update
  blocks to find which one spikes (likely an O(active roamers) or O(n²) scan
  firing when many creatures are loaded, or a periodic dragon/spawn pass).
- If `game_update` is also small → the time is in event processing or a stall
  between the draw call returning and the next frame top (rare; would point at
  the windowing/driver layer).

## Why this reframes the perf work
We'd both assumed the stutter was chunk streaming/meshing/upload (the usual
voxel-engine culprit). The data says otherwise: streaming is *already* cheap
(p99 ~0) — the budgets and the prior tuning work did their job. The mesh-vision
and batch-field work is still valuable for *scaling* (more chunks/grass/fire),
but it is **not** what's causing the current frame-time spikes. The next fix
should target whatever `game_update` localizes — almost certainly a creature-AI
pass — not the streaming path. Measure, then cut.
