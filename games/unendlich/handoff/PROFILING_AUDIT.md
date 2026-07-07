# Single-core profiling audit (r630–r631)
Motto: "multithreaded is good — but what if one core is all we need." Only
behaviour-preserving wins (verified by each module's tests staying green).

## What was checked
- transcendental-in-comparison (sqrt/pow used only to compare)
- .clone() density + clones inside loops
- pointer chasing (map .get in loops, Box<dyn>/Rc/Arc indirection in hot structs)
- per-frame heap allocations in pure-map functions

## Findings — the tree is already disciplined
- **sqrt-in-comparison (hot paths): DONE.** `eyes.rs` (FOV cone as dot²≥cos²·|d|²)
  and `body_collision.rs` (early-out on dist²≥r²) already did this. r630 brought
  the last two hot sites in line: `prey_scan::gather` (squared pre-filter, sqrt
  only for in-range candidates that get stored) and `dragon_territory::zone` +
  `territories_conflict` (squared compare, sqrt dropped). No hot sqrt-compare
  sites remain; the rest are in tests or cold image/edge code.
- **Clones are load-bearing, not waste.** The loop-clones are intentional:
  COW snapshots (env_batch / water_batch / sync_check / timeline — the Class-C
  safe-parallelism pattern), Jacobi constraint snapshots (`verlet_strand`), and
  pure-map owned outputs (`dragon_forge::skin`). None can be removed without
  changing behaviour.
- **No pointer-chasing waste.** Hot loops index flat `Vec`/grid (`grid.get(x,y,z)`,
  `out.get(x,y,z)`) — cache-friendly. The only `.get(&key)` map lookup in a loop
  is a batch-dedup. The only `Box<dyn>` is a cold plugin registry.

## The one real seam — per-frame pure-map allocations
Pure-map functions that return a fresh `Vec` each call reallocate every frame.
The fix is the scratch-buffer/out-param pattern `prey_scan::gather`/`select`
already use: an `_into(&mut out)` variant, with the allocating version as a thin
wrapper. **Done this round:** `dragon_forge::skin_into(model, anim, &mut out)`
(r631) — the dragon skinning path rebuilt a posed `Vec<ForgeBox>` per dragon per
frame; callers can now reuse a buffer. `skin()` stays as a wrapper (identical
output; verified by the skin tests).

## Recommended next steps (each verified by tests, no behaviour change)
1. Adopt `skin_into` on the render path: hold one `Vec<ForgeBox>` per visible
   dragon (or one shared scratch) and pass it in each frame.
2. Batch the same `_into` treatment for the other *hot* pure-map Vec returners
   (only the per-frame ones — most of the ~129 Vec-returning pure fns are cold).
3. There are NO parity-critical or threading changes here — this is pure
   per-core work reduction, orthogonal to the P/C/S parallelism lanes.

---

# Round 2 (r632): more _into + locking audit

## More scratch-buffer wins (verified in dragon_forge via harness)
- `skin_with_flex_into(model, anim, offs_l, offs_r, &mut out)` added; `skin_with_flex`
  is now a thin wrapper. It internally uses `skin_into`, so the per-frame
  wing-flex skinning path reallocates nothing when a buffer is reused.

## Adopting skin_into on the render path (bin/engine — do at the call site)
`world/entity.rs:~1189` skins every dragon every frame:
```rust
let boxes = crate::sim::dragon_forge::skin(model, &anim);
for b in &boxes { /* build render boxes */ }
```
Hold ONE scratch `Vec<ForgeBox>` in the per-frame render state (or a thread-local)
and pass it in:
```rust
// once, in the renderer's frame state:
//   skin_scratch: Vec<ForgeBox>
crate::sim::dragon_forge::skin_into(model, &anim, &mut self.skin_scratch);
for b in &self.skin_scratch { /* unchanged */ }
```
Same for any `skin_with_flex` call → `skin_with_flex_into(.., &mut scratch)`.
Output is byte-identical (verified by the skin tests); this only removes the
per-dragon-per-frame heap allocation. `mob_limb_girth`/`mob_pixel_peeper` call
`skin` too but are analysis tools (cold) — leave them.

## Locking audit — the tree is already disciplined
Checked every `Mutex`/`RwLock` site. What's already right:
- **`parking_lot::Mutex`** in the hot scheduler (faster than std; no poisoning).
- **`RwLock`** for read-heavy shared state (`heat_source` sources list).
- **`chunk_loader`** drops the queue lock (`drop(q)`) BEFORE waking workers /
  doing work — no lock held across gen/mesh. Duplicate detection is O(1) via a
  `queued` HashSet mirror (was O(N)). Loader threads are deprioritized to avoid
  the gen/mesh/render "chug" contention on shared P-cores.
- **No `Mutex<primitive>`** (atomics used where a lock-free counter suffices).
- **No lock re-acquired inside a loop**; batch operations take the lock once.

The ONE contention candidate (optional, telemetry only, NOT behaviour-preserving
so left undone):
- **`render/mesher_timer.rs::record`** takes a single global `Mutex` on EVERY
  chunk-mesh completion (all mesh workers contend) and does
  `lifetime[i].push(elapsed_us)` into an **unbounded** Vec (grows for the whole
  session; reallocations). Recommendation: accumulate per-worker (thread-local)
  and merge periodically, OR replace the growing Vec with an atomic
  count+sum (running mean) plus a fixed-size ring for percentiles. This changes
  the stat from "all-time samples" to "running aggregate", so it needs a
  deliberate call, not a silent optimization.

Net: no further blind lock edits are warranted — the locking is well-built.
