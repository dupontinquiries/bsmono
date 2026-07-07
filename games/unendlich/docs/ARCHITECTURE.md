# unendlich — Architecture (rc29 feature set)

This documents the work layered on top of the checkpoint `88b5542`
(`rc29-adaptive-uploads`, "wip: checkpoint before applying sim+items") as 17
tagged bricks. It's the map for reading the code, understanding how the pieces
compose, and wiring them in. Companion: `TESTING.md` (how to get building/testing
again).

## How the work is packaged
Every feature is a `brick/<name>` git commit on top of `88b5542`. They apply via
`toolkit/apply_from_zip_v2.sh <patch-zip>` (preflight → `git am --3way` → tag).
Applying the full series onto a clean `88b5542` reproduces a byte-identical tree
(self-tested) with zero module-resolution gaps. Each brick is purely additive:
new module files + the `pub mod` line wired into `lib.rs` / `world/mod.rs`, never
rewriting base modules.

**Validation status (read this):** every kernel is logic-validated (the subtle
math cross-checked in Python against an independent reference before any Rust was
written), brace-balanced, module-gap-clean, and carries unit tests (186 `#[test]`
fns across `sim/`). **None has been `cargo build`'d** — there is no Rust toolchain
in the environment they were authored in. So they are correct *in design and in
isolation*; the open risk is API drift against the real base (signatures that
differ from what a kernel assumed). That's what `TESTING.md` is for.

## The one load-bearing idea: the determinism boundary
Everything else hangs off this. State splits into two tiers that must never leak
into each other:

| | Render / speculative | Simulation / confirmed |
|---|---|---|
| Math | `f32` (not bit-reproducible across machines) | fixed-point `Fixed` i64 / integer |
| Scope | per-player, local | shared, identical on every peer |
| Examples | volumetric scatter, foveation, the f32 `perceive`, animation, prediction | mob decisions that affect outcomes, world edits, anything hashed |
| Consistency | best-effort, corrected by the authority | deterministic; agreed via digest/quorum |

Rules the confirmed tier obeys so peers agree: fixed-point only, canonical
iteration (sorted / `BTreeMap`, never `HashMap`), seeded RNG only, no wall-clock.
This is why rendering can be foveated per-player but mob *AI* cannot, and why mob
perception has both an `f32` form (render) and a quantized form (sim).

## Three clusters

### 1 — Simulation core & content (the base feature work)
`brick/sim`, `brick/items`, `brick/telemetry`, `brick/world-flora`. The
deterministic sim (Fixed/Vec3/SimState/step, rollback+physics, net
session/transport/wire, mobs, cognition, the `gpu_bench` harness), the items
subsystem, run telemetry (achievements/report/window → `run_summaries.txt` /
`events.jsonl`), and the world flora/lumen modules (elemental, field_brush,
effects, erosion, collapse, lumen, stem(+gpu), cave_flora, flora_sites,
chunk_flora). Everything below builds on the `sim::gpu_bench` harness this
introduced.

### 2 — GPU-bench kernels & render fidelity
All in `sim/`, all following the harness pattern (a GPU-ready CPU reference + a
`bench_*` wrapper + a documented `GPU DISPATCH CONTRACT` + cross-validation
tests), and all emitting a `metric_line` so their cost lands in the dashboard.

- `navfield_gpu` (`brick/navfield-gpu`, `brick/navfield-flow`) — distance field by
  Jacobi relaxation + a flow-field companion (per-cell descent toward the nearest
  seed). GPU-friendly nav without per-agent A*.
- `ao_gpu` (`brick/voxel-ao`) — canonical Minecraft-style corner ambient occlusion,
  one thread per voxel face.
- `volumetric_gpu` (`brick/volumetric-integrate`) — the froxel front-to-back
  scattering integration your `render::froxel` docs flagged as missing. **This is
  the godray / volumetric-fog keystone.** Transmittance is Beer-Lambert exact.
- `smoke_gpu` (`brick/smoke-fire`) — buoyant smoke-from-fire field (emit from
  `lumen` ignite → semi-Lagrangian advect → decay) that injects extinction +
  ember emission into the froxel volume, so it renders through `volumetric_gpu`.
- `heat_distort` (`brick/heat-distort`) — screen-space heat-shimmer refraction post
  driven by the temperature field.
- `foveation` (`brick/foveation`) — the keystone of "foveate the froxels/shadows/
  lighting": a gaze-proxy (aim-point) LOD field every expensive pass samples to
  scale work, plus a foveated `volumetric_gpu` integration. Merging Z-slices
  preserves transmittance bit-exactly, so the periphery never looks *wrong*, only
  less detailed. Foveation *subtracts* cost, so the win shows as lower
  `scatter`/`shadow`/`main` ms in the dashboard.

These share the existing froxel volume (`render/froxel.rs`, `froxel_fill.comp`)
and the cascaded shadow + `gpu_timer` zone machinery. The "fog" math here is the
same one the perception cluster reuses.

### 3 — Perception, netcode & determinism
The multiplayer-consistency half. The unifying motif: **one fog model, sampled at
two fidelities** — the camera reads it foveated-fine, mobs read it coarse.

- `perception_fog` (`brick/perception-fog`) — mob eyesight as transmittance
  through a coarse occlusion field. `perceive()` is the graded advanced API;
  `can_see()` is the simple API = the gradient thresholded against stable noise,
  calibrated so `P(can_see) == perceive` and collapsing to the old binary at the
  extremes.
- `perception_field` (`brick/perception-field`) — mob sight as a **CRDT
  aggregation over players** (sum = alertness, argmax = target), not contested
  state. Commutative/associative/idempotent merge ⇒ partial area-of-interest
  views converge with no consensus round. Each term is `quantize(perceive(...))`
  (the determinism bridge).
- `sync_check` (`brick/rollback-sync-check`) — the rollback "proof": a
  deterministic confirmed-state digest (splitmix mix, canonical id order) +
  `first_desync` localization + the speculative/confirmed entity split with
  `reconcile()` for re-entering deterministic behaviour.
- `consensus` (`brick/region-consensus`) — quorum tally over region digests:
  `Confirmed` / `Unverified` (lone region → validate lazily or recruit) /
  `Contested`, plus `suspects()` and adaptive `check_interval`. Honestly bounded:
  beats lone cheaters + accidental desync, not collusion.
- `desync_monitor` (`brick/desync-monitor`) — adaptive per-entity scrutiny (back
  off synced peers, focus troublemakers; cheap compare hard-capped at the rollback
  window) + `DesyncEvent` / `annotate_frametime` folding resim cost + markers into
  the frametime series.
- `world_authority` (`brick/world-authority`) — command-sourced canonical world
  (clients send commands, not state; validate-and-apply; `BTreeMap` canonical
  order) + deterministic `replay` + `Snapshot`. Persistence = snapshot +
  replayable command log; a flagged player's commands re-validate against truth so
  cheats are rejected on replay while legit builds stand.

## How analytics ties it together
The dashboard (in `toolkit/`: `unendlich-dash/` egui app + `unendlich-telemetry.html`)
parses `run_summaries.txt` metric lines and `events.jsonl`. Each render kernel is
a `gpu_timer` zone (`scatter`, `smoke`, `heat_post`, …) joining the existing
`shadow`/`froxel`/`cull`/`main`/`total` under the 8.3 ms stage budget (feeding the
🚨 stage-p99 achievement). Netcode health rides the same rails: `desync_monitor`
emits `desync` events to `events.jsonl` and folds resim cost into the frametime
chart. So turning a feature on automatically makes its cost (or its desyncs)
visible where you watch frame pacing.

## Brick catalog (apply order)
| # | tag | module | cluster |
|---|-----|--------|---------|
| 1 | `brick/sim` | `sim/*` | core |
| 2 | `brick/items` | `items/*` | core |
| 3 | `brick/telemetry` | `telemetry/*` | core |
| 4 | `brick/world-flora` | `world/{elemental,…,chunk_flora}` | core |
| 5 | `brick/navfield-gpu` | `sim/navfield_gpu.rs` | fidelity |
| 6 | `brick/navfield-flow` | `sim/navfield_gpu.rs` | fidelity |
| 7 | `brick/voxel-ao` | `sim/ao_gpu.rs` | fidelity |
| 8 | `brick/volumetric-integrate` | `sim/volumetric_gpu.rs` | fidelity |
| 9 | `brick/smoke-fire` | `sim/smoke_gpu.rs` | fidelity |
| 10 | `brick/heat-distort` | `sim/heat_distort.rs` | fidelity |
| 11 | `brick/foveation` | `sim/foveation.rs` | fidelity |
| 12 | `brick/perception-fog` | `sim/perception_fog.rs` | perception/net |
| 13 | `brick/rollback-sync-check` | `sim/sync_check.rs` | perception/net |
| 14 | `brick/region-consensus` | `sim/consensus.rs` | perception/net |
| 15 | `brick/desync-monitor` | `sim/desync_monitor.rs` | perception/net |
| 16 | `brick/world-authority` | `sim/world_authority.rs` | perception/net |
| 17 | `brick/perception-field` | `sim/perception_field.rs` | perception/net |

(Logical grouping differs from this chronological order; clusters above are the
better mental model.)
