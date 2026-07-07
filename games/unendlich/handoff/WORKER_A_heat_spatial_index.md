# Worker thread A — spatial-index the heat-source map

You are one of three parallel workers on **unendlich**, a Rust voxel/dragon game
engine (Vulkan, Linux/Asahi, FP32). A coordinator portioned you this task from
the project docs. Work only from this brief and the docs it cites.

## Your environment (this OVERRIDES the stale "you cannot compile" note in `handoff/SESSION_HANDOFF.md §0`)
- You **can compile and run.** `cargo` + `rustc` 1.75 are installed.
- You have **llvm-pipe / lavapipe** (Mesa software Vulkan), the **test harnesses**
  (`harness/run.sh`), and the **docs** — read them.
- Repo root is `unendlich/` (the tree you were given); it is the ground truth.
- **Cargo.lock is v4** (needs cargo ≥1.78; you have 1.75). Build/test like this,
  reversibly: `cp Cargo.lock /tmp/lock.v4; rm -f Cargo.lock; cargo <cmd>; cp /tmp/lock.v4 Cargo.lock`.
  Restore the v4 lock before handing back.
- Run `harness/run.sh` from the **repo root**.
- Stay in scope. If blocked or a doc is ambiguous, stop and report back — don't guess.

## Read first
- `docs/FOLLOWUPS.md` — the 2026-06-04 heat-source entry. It says: `total_heat_at`
  does an O(n) linear scan over every source, called from convection/plant/ice
  each tick; **Fix B (recommended): spatial-index `HeatSourceMap` so queries are
  O(nearby).** That is your task.
- `handoff/HEAT_SOURCES_DIAGNOSIS.md` — background.
- `crates/unendlich-engine/src/heat_source.rs` — `struct HeatSourceMap` (~line 90),
  `fn total_heat_at` (~line 184): `self.sources.iter().map(|s| s.contribution_at(..)).sum()`.
  Read `HeatSource::contribution_at` to find the **falloff cutoff radius** (the
  distance past which a source contributes ~0). That radius sizes your grid.

## Task
Add a **spatial index** inside `HeatSourceMap` (a uniform grid: bucket each source
by its `floor(pos / CELL)` cell, `CELL ≈ the max influence radius`). Rewrite
`total_heat_at(wx,wy,wz)` to sum only sources in the cells overlapping the query's
influence neighbourhood (the 3×3×3 — or radius-derived — block of cells around the
query), instead of all sources. Keep the **public API identical** (`total_heat_at`,
`len`, `is_empty`, `snapshot`, registration). Rebuild the index incrementally on
insert/clear, or lazily; your call, but keep it correct under the existing
`RwLock` usage.

**Determinism:** the engine relies on deterministic sims. Sum contributions in a
**stable order** (e.g., iterate cells in a fixed order and sources in
insertion order within a cell) so the f32 result is bit-identical run to run.

## Validate (you can actually run this)
`heat_source.rs` is a near-pure module. Add a `#[cfg(test)] mod tests` (or extend
the existing one) with:
- A **parity property test**: build N random sources at random positions, query
  M random points, assert the indexed `total_heat_at` equals a brute-force
  `O(n)` sum **bit-for-bit** (compare `.to_bits()`) — they must match exactly
  because you're summing the same terms in the same order.
- Edge cases: empty map → 0.0; single source; a query exactly at the cutoff
  radius; sources sharing a cell; far query (returns 0, touches no sources).
- A coarse perf sanity check (optional): assert that for a clustered set the
  indexed query visits ≪ N sources (expose a debug counter if useful).

Run it standalone (fast loop):
```
rustc --test --edition 2021 crates/unendlich-engine/src/heat_source.rs -o /tmp/ht && /tmp/ht
```
Then confirm the crate still builds and the harness is green:
```
cp Cargo.lock /tmp/lock.v4; rm -f Cargo.lock
cargo build -p unendlich-engine
cp /tmp/lock.v4 Cargo.lock
bash harness/run.sh            # from repo root
```

## Definition of done
- Indexed `total_heat_at` is **O(nearby)** and **bit-identical** to brute force
  (proven by your parity test).
- Public API unchanged; crate builds; your tests + `harness/run.sh` green.
- Hand back an **overlay** (just `crates/unendlich-engine/src/heat_source.rs`,
  repo-relative path) + a short report (approach, the skip-ratio you measured,
  any blockers). Exclude `target/`.
