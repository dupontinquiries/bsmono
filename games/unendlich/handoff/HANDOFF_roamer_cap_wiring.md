# HANDOFF — wiring `sim::roamer_cap` at the spawn gate (WID r330-mobs)

**Status:** PROPOSAL ONLY. The pure core (`sim::roamer_cap`) is merged + verified
(8/8 standalone). The call-sites below touch the bin (`unendlich_fullgame.rs`)
and/or the shared director, which a worker must not wire (SHARED_RULES §1–§2) —
coordinator/operator apply + validate on device.

## What this fixes
`NEXT_SESSION_GUIDE.md §4d` / `stutter2`: wild roamers overpopulate (observed
**73 vs cap 8**) because spawning isn't gated on the live count. The new core is
the admission gate the spawn path consults so it **never admits past the cap**.

## The r300 constraint (do not violate)
`WIRING_STATUS.md r300`: the operator **declined population caps / forced
despawn** — the herd stays intact and per-creature cost was lowered instead.
So this is **spawn-side ONLY**. `roamer_cap` has no removal path; over-cap is
corrected by admitting **0** until the herd settles. Do not add eviction when
wiring this.

## API (pure, alloc-free, saturating)
- `admit(current, cap, requested) -> u32` = `min(requested, cap - current)`;
  `0` once `current >= cap`. Guarantees `current + admit(..) <= cap`.
- `overflow_count(current, cap) -> u32` = `current - cap` (saturating) —
  **diagnostic/logging only, NOT a despawn list.**
- `headroom(current, cap) -> u32` = `cap - current` (saturating).

## Proposed call-sites (pick one or both; A is the minimal hard gate)

### A. Bin spawn gate — the hard global cap (recommended, smallest change)
In the wild-spawn commit point of the roamer loop
(`crates/unendlich-engine/src/bin/unendlich_fullgame.rs`, roamer/spawn region
~4786–6010 per the r300 note), where a planned batch is about to be inserted:

```rust
// live = WILD (non-persistent) roamer count only, so player-hatched
// persistent creatures are never blocked (same persistent-guard as r300).
let live = wild_roamer_count;                  // exclude persistent==true
let cap  = runtime_config.roamer_cap;          // the §4d knob (default 8)
let admitted = sim::roamer_cap::admit(live, cap, planned.len() as u32) as usize;
if admitted < planned.len() {
    // diagnostic only — explains the stutter; never drives despawn
    log_once!("[roamer-cap] {} > cap {} (over by {}), admitting {}/{}",
        live, cap, sim::roamer_cap::overflow_count(live, cap),
        admitted, planned.len());
}
for req in &planned[..admitted] { /* existing insert path */ }
// planned[admitted..] is simply not spawned this tick; no removal.
```

`runtime_config.rs` already carries the perf knobs (`max_uploads_per_frame`,
`upload_budget_ms`); add a `roamer_cap: u32` field there (default 8) so it's a
live tunable + kill-switchable, matching the §4d "revisit the roamer cap
enforcement" ask. Suggested env override `UNENDLICH_ROAMER_CAP` for A/B.

### B. Director-level total clamp (pure, optional, composes with A)
`sim::mob_spawn_director::plan_spawns` already clamps **per-kind** headroom via
`batch_spawn_budget::allowed_one`. It does **not** bound the *global* roamer
total. If you want the director itself to respect a global cap, pass the live
total + global cap into the planner and clamp the summed plan with
`roamer_cap::admit(total_live, global_cap, planned_total)`, trimming lowest-
priority kinds first. This keeps the plan and the bin gate (A) agreeing because
`admit` is the same `min(req, cap-cur)` rule as `allowed_one` with unbounded
budget. (Pure change to a shared sim file — still a coordinator edit; flagged
here, not applied, per SHARED_RULES §4.)

## Perf note
`admit`/`overflow_count` are branch-light integer ops, alloc-free, safe to call
every spawn tick on the hot path. `wild_roamer_count` should be the count the
loop already maintains (or a cheap running tally), not a fresh O(n) scan.

## Verify after wiring (on device)
- With the herd seeded > cap, confirm the wild roamer total **converges down to
  `cap`** over time and **never climbs above it** post-merge (population settles
  naturally; nothing is despawned).
- `roamer_tick` p50 should ease further once the herd stops overshooting (the
  §4d/stutter2 cost driver), on top of the r300 graze-gate/disposition wins.
- Toggle `UNENDLICH_ROAMER_CAP=0` (or a high value) to A/B the cap's effect.
