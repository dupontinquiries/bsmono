# Perf Finding: per-frame creature mesh rebuild (r247 analysis)

> Follows `PERF_FINDING_r245.md`. Method: since the r245 profile localized the
> stutter to `game_update` (the untimed pre-draw loop work, NOT draw/streaming),
> and the operator can't test, I reproduced the candidate costs with standalone
> `rustc -O` microbenchmarks at the operator's stated scale: **~100 dragons +
> ~100 fauna, render distance 4.** Host is llvmpipe CPU (slower than the M2);
> the RELATIVE magnitudes are what transfer.

## What the microbenchmarks RULED OUT (with numbers)
At 100 dragons × 100 fauna:
- **Dragon-hunt target scan** (the O(dragons×fauna) nested loop at
  fullgame ~5126): **0.024 ms/frame** even in the worst case (all 100 dragons
  hunting at once). Distance checks are multiply-compare; 10k of them is free.
- **Per-roamer `any()` proximity scans** (too_close/predator_near): 0.0075
  ms/frame at 200×200 = 40k checks — and they're conditionally rare anyway
  (spawn-gated / F7-gated).
- **Steady per-roamer AI tick** (steering/wander math, already distance-banded
  by `sim::ai_budget`): 0.006 ms/frame.
- **Per-dragon `dragon_observe`**: takes a capacity-1 stimulus list (just the
  player) — cheap, not O(n²).

So the creature **AI** is NOT the spike. Three orders of magnitude too small.

## What the microbenchmark IMPLICATES
**The per-frame creature MESH REBUILD** at `unendlich_fullgame.rs` ~7468–7490:
```rust
{   // runs EVERY frame, unconditionally
    let mut everts = Vec::new(); let mut eindices = Vec::new();
    for c in &test_creatures { let (cv,ci)=creature_to_vertices(c); everts.extend(cv); … }
    for r in &roamers        { let (cv,ci)=creature_to_vertices(&r.creature); everts.extend(cv); … }
    // … then uploaded to the GPU
}
```
This regenerates the FULL multi-box geometry for all ~200 creatures from scratch
every frame, into fresh `Vec`s, then uploads it — **even when nothing moved.**

Modeled (100 dragons × ~22 boxes + 100 fauna × ~8 boxes → ~72k verts / ~108k
indices per frame): **0.53 ms/frame just for the vertex generation**, plus:
- ~2.6 MB of `Vec` allocated and freed PER FRAME (allocator churn), and
- a ~2.6 MB GPU upload PER FRAME (and if the buffer must grow/reallocate on a
  given frame, a pipeline stall).

0.53ms compute alone isn't 55ms, but the COMPOUND — regenerate + realloc +
re-upload every frame, on the real (more complex) forge-body geometry, with the
upload possibly stalling on buffer growth — is the kind of thing that produces
intermittent multi-ms→tens-of-ms spikes in `game_update`, matching the p95/p99
tail (fast median, periodic spikes when the upload/realloc path hits).

## The fix (cheap, safe, high-value) — DIRTY-FLAG the rebuild
The geometry only changes when a creature moves / animates / spawns / despawns.
Cache the built `(everts, eindices)` and only rebuild when something changed:
- Keep the last-built buffers + a `creatures_dirty` flag.
- Set dirty on: any roamer position/pose/anim change, spawn, despawn, or every
  N frames as a backstop for continuous animation (wings). For animation-only
  changes, a coarser cadence (rebuild the animated parts every 2–3 frames) is
  invisible at distance and cuts the cost by 2–3×.
- Reuse the `Vec` allocations across frames (`.clear()` + reuse, not fresh
  `Vec::new()`) to kill the per-frame alloc churn, and only re-upload when
  dirty.
This is the same "don't redo work nothing asked for" principle as the chunk
remesh gate. It needs the bin's render state to hold the cached buffers — a
contained change, but one that wants a running build to verify (it changes
what's uploaded), so it should be done + tested on the M2.

## Honesty / limits
- The 0.53ms is a MODEL of `creature_to_vertices`, not the real function (the
  full-engine lib build kept timing out in the sandbox). The real forge body may
  be heavier (more boxes, normals, AO) — so treat 0.53ms as a floor.
- I have NOT proven this is the entire 55ms; I've proven the AI scans are NOT it
  and that the unconditional per-frame mesh rebuild is the largest per-frame
  game-logic cost I can find by inspection + benchmark. The r246 `game_update`
  zone + a sub-timer around this block on a real run would confirm it directly.

## Next measurement to close it
On the M2: wrap JUST the creature-mesh-rebuild block in its own profiler zone
(or temporarily skip it and watch game_update). If game_update p95 collapses,
the dirty-flag fix is the win.
