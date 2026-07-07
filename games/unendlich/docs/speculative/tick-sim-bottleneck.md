# Speculative: cpu.tick_sim bottleneck (60-180ms/sec in plant_lifecycle)

## 2026-05-17 update: read-only code investigation + RC34 shipped

Without hardware data, I read the `plant_lifecycle` handler
carefully. Three concrete findings:

### Finding 1: `pressure_samples` was allocating per call → FIXED in RC34

`pressure_samples()` in `pressure_sampler.rs:138` returns a
`Vec<PressureSample>` even though every call site iterates the
result linearly with `for s in pressure_samples(...)`. Four call
sites in `plant_lifecycle.rs` × 2-4 ticks per chunk per tick ×
~925 chunks at RD=6 × ~2 ticks/sec = ~3700 small heap allocations
per second.

**Shipped as RC34**: added a non-allocating `pressure_samples_iter`
variant that returns `impl Iterator<Item = PressureSample>`. The
Vec version is preserved (delegates to `_iter().collect()`) so
tests that do `assert_eq!(a, b)` on full sample sequences keep
working. Plant-lifecycle's four hot loops use the iterator
version.

Estimated win: ~3 ms/sec out of ~60-180 ms/sec. Small but free.

### Finding 2: Pass 2 still pays log(N) per stable plant

The fast-path skip for stable Mature plants
(`plant_lifecycle.rs:1458`) requires:
- BTreeMap lookup: `chunk.instance_state.get(&plant_idx)` (~30ns)
- Two integer comparisons

Per plant ~50ns × 100 plants/chunk × 300 plant-chunks at RD=6 ×
2 ticks/sec = ~3 ms/sec. Already small.

The bigger issue: every plant in `chunk.plant_indices` is visited
even when nothing's happening. To skip plants entirely, we'd
need a "dirty plants" set tracked alongside `plant_indices`.
Maintained by:
- `advance_n_ticks` adds the plant when it starts a transition
- The committed/finished transition removes it

Estimated effort: ~100 LoC across chunk.rs + plant_lifecycle.rs.
Estimated win: drops Pass 2 from O(plants-in-chunk) to
O(plants-transitioning), which is usually 1-5% of total plants.
Could reduce Pass 2 cost by 95% on steady-state forests.

Worth pursuing if RC34 doesn't move tick_sim enough.

### Finding 3: POI_REFRESH_INTERVAL_TICKS=10 is a known placeholder → FIXED in RC35

`chunk.rs:302` defined `POI_REFRESH_INTERVAL_TICKS = 10`. The
comment at `plant_lifecycle.rs:748` called this a placeholder for
"structural dirty marking" — when that lands, the check becomes
a single boolean lookup and idle chunks NEVER rebuild.

**Shipped as RC35**: added `structural_dirty: bool` field to
Chunk. Set in `Chunk::set()` on wood transitions (add/remove/
species swap). Cleared in InstallPoi op (both real path and
test apply path). `should_refresh_poi` now reads the flag
instead of the cadence. ~2 ms/sec saved + architectural
groundwork (any future "is this chunk unchanged?" question
now has a clean answer).

Steady-state cost of refresh: dropped from ~2 ms/sec to ~0
(only rebuilds on actual edits).

### Updated cost model

For RD=6 (~925 chunks, ~300 plant-bearing):

| Source | Cost/sec (estimated) | Status |
|---|---|---|
| pressure_samples alloc | ~3 ms | **FIXED in RC34** |
| Pass 2 plant iteration | ~30-60 ms | Could drop to ~3-6 ms with dirty-set |
| POI cache refresh | ~2 ms | **FIXED in RC35** |
| Distance band cull | savings | Already done; working |
| Empty-chunk fast-path | savings | Already done; works after first tick |
| **Total (post-RC34+35)** | ~30-60 ms | Down from ~35-65 ms |

The 180 ms/sec upper bound is presumably at higher RD (RD=10+,
~4000+ chunks). Pass 2 scaling dominates there — the dirty-set
optimization becomes proportionally bigger.

### Recommended next steps

1. **Land RC34** (`pressure_samples_iter`) — done in this session.
2. **Hardware measurement** of post-RC34 tick_sim cost. If still
   >50 ms/sec at RD=6, pursue the dirty-set work (Finding 2).
3. **Structural-dirty marking** (Finding 3) is independently
   valuable — eliminates the periodic POI rebuild cost AND
   enables future optimizations that depend on knowing "is this
   chunk really unchanged."
4. **Profiling instrumentation**: add cumulative-timer events
   for Pass 1 vs Pass 2 vs POI refresh so hardware data can
   distinguish.

---

## Original findings (pre-update)

## Status

INVESTIGATION ONLY. No fix proposed yet. Documenting findings from
code reading so the next session can build on this rather than
re-discovering it.

## The observation

From RC29 hardware validation, with all 925 chunks loaded at RD=6
and the player stationary:

```
cpu: ... tick_sim=60-180ms ...   (per second)
```

Variance across 1-second windows. Not the dominant cost (that's
`submit=800ms/sec`, addressed by RC30), but big enough to keep
some long-frame events alive even after RC30 lands.

## Code shape

The whole `tick_sim` budget goes through one path:

```
ChunkManager::tick_simulation(real_dt)
    → registry.run_tick(...)
        → for h in handlers: h.handler(&mut ctx)
            → only handler registered: "plant_lifecycle"
```

Tick period defaults to `0.5s` (2 ticks/sec; see
`world::ticks::DEFAULT_TICK_PERIOD`). With 60-180ms/sec total,
that's 30-90ms per tick on average — entirely concentrated in
the `plant_lifecycle` handler.

Wiring: `crates/unendlich-engine/src/bin/unendlich_world.rs` lines
304-308. Leaf-decay is intentionally NOT registered (line 309-318);
fire/water/spread/mob-AI handlers don't exist yet.

## What plant_lifecycle already does well

`crates/unendlich-engine/src/world/plant_lifecycle.rs`, function
`lifecycle_handler()` returning a closure (line 633).

Per-tick the closure:

1. Iterates `ctx.sorted_chunk_coords()` (~925 entries, sorted for
   deterministic replay).
2. Per coord: distance-band classification (`band_for_chunk`).
   - **CatchUp** band (far): skip with `continue`, costs ~6ns.
   - **Render** band (near): fire every tick.
   - **Simulation** band (mid): fire every Nth tick (catch-up
     applied via `fire_tracker`).
3. Per firing coord: fast-path early-exit (lines 708-719):

   ```rust
   let has_plants = !chunk.plant_indices.is_empty();
   let has_known_trees = !chunk.tree_root_indices.is_empty();
   let cache_says_no_trees = chunk.poi_cache
       .as_ref().map(|c| c.tree_pois.is_empty()).unwrap_or(false);
   if !has_plants && !has_known_trees && cache_says_no_trees {
       /* mark fired + continue */
   }
   ```

   So a sky chunk or pure-rock chunk costs ~10 atomics + a DashMap
   read + this check. Cheap.

4. For chunks past the fast-path: do the heavy work. ~2700 LoC of
   logic — aging, transitions, spreading, fire propagation, root
   tracking, biome interactions.

## What's likely dominating the cost

Without on-machine profiling data, the suspects in priority order:

1. **Chunks with plants.** At RD=6, surface chunks at the player's
   y level. Maybe 100-200 of the 925 chunks fall in this category.
   Each one runs the heavy path.
2. **The heavy-path inner loop** over `chunk.plant_indices`. Each
   plant: field samples (temperature, humidity), neighbor lookups
   (which can cross chunk boundaries — see `read_block_world` at
   plant_lifecycle.rs:842 fetching cross-chunk via `ctx.chunks`),
   age/transition state machine.
3. **Cross-chunk neighbor reads.** For plants near a chunk boundary,
   the handler does `ctx.chunks.get(&neighbor_coord)`, which is a
   DashMap read per neighbor query per plant. Cheap individually,
   but multiplied across all plants of all surface chunks…
4. **The POI cache build.** A chunk without `poi_cache` runs a
   first-pass to find tree roots, then sets the cache. After settle,
   all chunks should have `poi_cache` so this is one-time. Confirm
   by looking at whether `poi_cache.is_some()` for all chunks after
   ~10s — if not, that's a leak.

## Candidate optimizations (ranked by safety × estimated win)

### A. Same-pattern RC30-style skip (LOW WIN; UNLIKELY USEFUL)

"Skip the whole handler when nothing has changed since last tick."
This is fundamentally what RC30 does for the submit phase.

**Why it probably doesn't apply here**: the handler's whole job IS
to advance plant state every tick. There's no "nothing changed"
state — every plant ages with each tick. Even if no edits occurred,
plants are still ticking forward toward their next transition.

Caveat: for a forest in steady state (every plant is already
mature, no births or deaths happening, no fire), the handler IS
doing useless work — just bumping age counters that nothing reads.
If we can characterize "this chunk has no actively-transitioning
plants" cheaply, we could early-exit. Needs a per-chunk
`next_event_tick: u64` cache to know when something interesting
will happen.

**Recommendation**: defer. Costly to design correctly; the savings
depend on whether the heavy path is the cost (it probably is) or
the iteration is (it probably isn't, given the existing fast path).

### B. Per-chunk earliest-next-event cache (MEDIUM WIN; CAREFUL DESIGN)

After each tick, the handler computes for each chunk: when is the
next event (transition, birth, death, spread) due? Store as
`chunk.next_lifecycle_event_tick: u64`. Skip the chunk on ticks
where `current_tick < next_lifecycle_event_tick`.

**Wins**: a fully-mature forest chunk where the next event is
"berry ripens in 200 ticks" gets skipped 199 ticks in a row.

**Risks**:
- Events triggered by neighbor state (fire spread, sun-block changes)
  break the cache. Either invalidate aggressively on neighbor edits
  (probably too aggressive) or accept that the cache is a heuristic
  with a backstop "always fire at least every N ticks regardless".
- Adding state to `Chunk` requires versioning the serialization
  format. Probably OK to use `#[serde(skip)]` and rebuild on load.

**Recommendation**: viable. Probably the right next perf RC if
plant_lifecycle stays expensive after RC30 lands.

### C. SIMD or batched field sampling (MEDIUM WIN; CAREFUL DESIGN)

Field reads via `chunk_field_reader_owned` already take one snapshot
per chunk per tick (line 557 in chunk_manager.rs). Within a chunk
processing all plants, we could vectorize the per-plant sampling.

**Recommendation**: defer until profiling data shows field sampling
is a top cost. Likely modest gains.

### D. Move plant_lifecycle to a worker thread (HIGH WIN; HIGH RISK)

Currently runs synchronously on the main thread. If it moved to
its own thread, main thread would only see `tick_sim` cost equal
to the apply phase (much smaller) plus thread-sync cost. Heavy
ticks would no longer hitch the render frame.

**Risks**:
- The handler reads `ctx.chunks` (DashMap of Arc<Chunk>) — fine
  to share across threads.
- Edits are deferred into Vecs (`block_edits`, `state_changes`)
  and applied in Phase 3 (chunk_manager.rs:600 onward). Application
  still has to happen on the main thread (single-writer to chunks).
- But the handler ITSELF could run on a worker as long as we hand
  the edits Vec back on completion. The synchronization model is
  similar to MeshingPool.
- Adds complexity. Probably not worth it before per-chunk caching
  (option B) is tried.

**Recommendation**: design memo for the long-term. Not a near-term
RC.

## What I want next session

Per-handler timing data from the actual run. The instrumentation
already exists:

```rust
// chunk_manager.rs:570-571
let mut handler_us_accum: HashMap<&'static str, u64> = HashMap::new();
```

And the bin reads it via `chunk_manager.last_handler_us()` (line
1232 of chunk_manager.rs). It might already be logged somewhere
in the FPS log line — check `crates/unendlich-engine/src/bin/unendlich_world.rs`
for `last_handler_us` usage.

If we know:
- Per-tick plant_lifecycle cost (likely the whole 30-90ms)
- How many chunks fired vs were skipped via fast-path
- How many chunks have plants vs no plants

…we can pick between B (most likely), C (if field sampling dominates),
or D (if the synchronous wait is the killer).

## Cross-references

- `crates/unendlich-engine/src/world/plant_lifecycle.rs` — 3300 LoC handler
- `crates/unendlich-engine/src/world/ticks.rs` — registry + scheduler
- `crates/unendlich-engine/src/world/chunk_manager.rs:504` — tick_simulation
- `crates/unendlich-engine/src/world/scheduler.rs` — band/divisor logic
