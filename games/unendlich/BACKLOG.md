# BACKLOG — unified (the three thread TODOs, merged & owner-tagged)

One board instead of three. **Area** tags route the work (see `OWNERSHIP.md`):
`SIM` · `ATMO` (atmo/render) · `WORLD` (world/perf) · `MAIN` (bin/decls/gates, you).
Ordered by the real seam, not by thread: **cores you can build+test in the
sandbox first; wiring that needs the M2 second.**

Legend: ✅ done · ▢ to build · ⟶ wire (core exists) · ▦ render/GPU · ✦ design.

---

## Round log

- **R1 (2026-06-04) — DONE, merged, harness GREEN, 49 new tests.** Pure cores
  delivered by the three workers: `sim/flex_bend`, `sim/foot_ik`, `sim/strafe_face`
  (B + the moonwalk item); `MoistureField::recenter` + `climate::temp_for` (cores of
  the player-recenter and diurnal-temp items — *wiring still pending*);
  `world/octree_compact`, `world/recompaction`, `world/competition`,
  `world/lod_upload` (B + PERF #4). No conflicts, nothing pruned.
- New ideas captured + decomposed in **`BACKLOG_INBOX.md`** (adds the NET area +
  GPU-RUNTIME / DEVICE-TIERING concerns).

---

## A. Ready to WIRE now — pure core built & unit-tested, only the (M2-verifiable) hookup is left

These are the highest-leverage, lowest-risk items: the hard part is done and
tested; what remains is bin/render integration.

- ✅ `SIM` **Ragdoll on death/knockout** — DONE (c16 rig session): `sim::ragdoll_bridge::from_anatomy` + `sample_body_local`; `begin_death` in the bin flips a roamer into `RoamerRig.dying`, `creature_to_vertices_rigged` blends the pose to the nodes; corpse lingers 6 s. (`from_creature` used COLLISION limbs, not rendered parts — that was the missing bridge.)
- ✅ `SIM` **Per-state liveliness** — DONE: `anim_rig::pose` blends by `limpness(state, progress)` and breathes the torso; the roamer render site sets `MotionState` (Dying/Resting/Walking/Idle).
- ✅ `SIM` **Squash & stretch deform** — DONE: torso squash from `RoamerRig.impact` (touchdown), motion stretch from the roamer's real speed, in `anim_rig`.
- ◐ `SIM` **Circadian rest cycle** — `rest_urge` now feeds the arbiter's `fatigue` (was 0.0 since r269); `wants_rest` sets `RoamerRig.resting` → `settle` eases → the rig beds the creature down. STILL OPEN: `chill` from local temperature (rain only today). *(needs ATMO temp — see deps)*
- ⟶ `SIM` **Dragon/large-body sensing & pathing** — wire `body_scale::BodyScale` + `collision_probe::coarsened` into the large-creature tick (size-aware *lean* already wired).
- ⟶ `SIM` **Stamina + sprint** — wire the speed multiplier into the roamer movement tick.
- ✅ `SIM` **Creature-vs-creature melee** — DONE (c16): `sim::melee` strike loop, wired for the Dinosaur hunt (bite lands on the lunge peak; prey takes damage/knockback/flinch; lethal → ragdoll).
- ✅ `SIM` **Legendary weapon abilities** — DONE (c16): `sim::weapon_ability` — every `ProcKind` fires from the equipped weapon; statuses on `RoamerRig.status`. OPEN: player-facing feedback (a burn/chill icon on the target, a proc log line in the HUD); a serpent spine wave in `anim_rig` so `Arche::Serpent` can join `FAUNA_ARCHES`.
- ◐ `UI` **Mobile Linux** — `mobile_ui` detection + touch controls wired (c16). OPEN: verify on a device (Phosh drag ordering, button radius), a settings toggle for touch mode, pinch-zoom for the inventory.
- ⟶ `SIM` **Eased turning** — wire `turning::ease_yaw` into the roamer heading update. NOTE (c16): `turn_toward`/`wander_turn` already rate-cap yaw, so this is a second-order refinement, not a snap fix.
- ⟶ `SIM` **Surface cache + cadence** — wire to cut redundant surface probing (perf lever).
- ⟶ `WORLD` **Plant genome storage + growth-tick** — wild = `from_seed` (free); bred/tended = sparse sidecar by block-index; growth tick uses `rate_mul`, caps at `target_height`. *Open Q: per-block / per-cluster / per-mother-tree.*
- ⟶ `WORLD` **Pollination → breeding loop** — carrier entities, apply offspring genomes to the world, cross-pollination/dispersal radius. *(needs SIM bee/songbird — see deps)*
- ⟶ `WORLD` **Exotics wiring** — auras as `emission` Discharge emitters; FireResist→heat, OreBloom→Earth edit, FruitBearing→drops, `fog` flag→render particles (▦).
- ⟶ `WORLD` **`express_part` → unique boughs** — feed per-part expression into the blueprint/mesh path so heterozygous trees branch distinctly.
- ⟶ `ATMO` **Demo-binary binding-4 placeholder** — `ChunkRenderer` demos leave descriptor binding 4 unwritten; add a zero-inited 2D `MoistureTexture` placeholder if a demo runs approach 4. *(main `unendlich_world` is wired.)*
- ⟶ `MAIN` **Land the fog→renderer wiring** — apply `handoff/fog_port/` (see RECONCILE_REPORT for the 6 conflicts + R2 .spv regen).

## B. Pure cores to BUILD next — fully sandbox-verifiable, no M2 needed (good autonomous work)

- ▢ `WORLD` **Octree recompaction policy** — periodic `from_chunk` trigger when `node_count()` > ~1.5× baseline (cold path). Reclaims pool bytes; correctness unaffected.
- ▢ `WORLD` **`OctreeArray` re-merge** — collapse 8 uniform sibling leaves on ascent (split-only skips this). Additive + parity-testable.
- ▢ `WORLD` **Rebuild coalescer** — one octree rebuild/stamp per chunk per cadence instead of per growth event.
- ▢ `WORLD` **Cull / LOD upload policy** (PERF_QUEUE #4) — pure decision core for which chunks to upload/cull at a given RD.
- ▢ `WORLD` **`competition_factor(height_rank, shade)`** — forest competition via `neighbor_grid` broadphase (noted "not built" in PLANT_GENETICS §3c).
- ✅ `SIM` **Neck/tail flex-bend** — DONE: `anim_rig` turns a Tail/Neck part into a `CHAIN_SEGMENTS` chain bent by `flex_bend` (tail out of the turn, neck into it, swish at the gait phase).
- ◐ `SIM` **Per-leg catch-force / foot-lock IK** — the residual float is gone on the rigged path: legs pivot at the hip and the STANCE leg extends so its lowest corner sits on the rest sole (`anim_rig`). `foot_ik` (terrain-height planting) is still unwired.
- ▢ `ATMO` **`MoistureField::recenter(new_origin)`** — integer-cell grid shift, zero-fill new cells (pure & testable); prerequisite for player-following fog.

## C. Render / GPU / integration — ATMO + MAIN, needs the M2

- ▦ `WORLD`→▦ **SurfaceRun far-render path** (PERF #2) — wire `upload_repr::classify` into the upload path (Octree→`chunk_mesh_octree.comp`, SurfaceRun→`chunk_mesh_runs.comp`, Procedural→skip). **Sub-step 1:** shadow-telemetry `project_residency` at RD 16/32/64. Fixes the RD-10 pool OOM. Contract: `UPLOAD_REPR_WIRING.md`.
- ▦ `ATMO` **Fragment-shader tint animation** (PERF #3) — per-cell `(from,to,started_tick)` buffer + `current_tick` uniform + fragment lerp; **then delete the transition-remesh sweep** in `chunk_manager` (`WORLD`).
- ▦ `ATMO` **exec_tier GPU passes** (PERF #6) — GPU compute for cosmetic flora/fauna (grass/leaf sway, ambient flocks). The CPU/GPU *boundary* (`exec_tier`) is done.
- ▦ `ATMO` **Real world data for fog closures** — replace `world_renderer.rs` tick stand-ins: hash "lakes"→real water mask; fixed `temp=0.30`→real climate temp. *(WORLD can supply both.)*
- ▦ `ATMO` **Diurnal temperature cycle** — drive fog `temp` from a day/night curve (`atmos.time_of_day`).
- ▦ `ATMO` **Wind deflection by terrain** — feed `wind_sampler` a real heightmap closure (cf. the cpu_generator binary-search helper, RECONCILE_REPORT note 4).
- ▦ `ATMO` **Tune approach 4** — `chunk.frag` thickness `0.05` + ground-hug `0.015·p.y` + `ClimateParams`; pending an in-game read.
- ▦ `ATMO` **Froxel volume fill pass** *(verify)* — wire `froxel_fill_pipeline` so approach 1 is a true froxel volumetric (currently sentinel→per-fragment fallback).
- ▦ `ATMO` **SSR verify + tune** *(verify)*; **Shutdown segfault** *(verify, teardown)*; **macOS/MoltenVK** *(verify, deprioritized)*.
- ▦ `ATMO` **(stretch) Shadowed in-scatter for approach 4** — borrow approach 1's shadow sampling/god-rays.
- ⚙ `WORLD` **Operational levers** — `UNENDLICH_OCTREE_POOL_MB` 256→384–512 (laptop stopgap; dies on Fairphone); A/B `APPLY_BUDGET`/`EVICT_BUDGET`; confirm `tick_apply` p50 drop now #1 is wired.
- ⚙ `WORLD` **Slower `STAGE_*_DURATION_TICKS`** — cheapest mobile win (less plant-edit churn); MAIN owns `plant_lifecycle`.

## D. Bigger design / specs — break into core+wiring before scheduling

- ✦ `SIM` Block-level grass foraging · Predator/prey loop · Fall-damage application · Swim/fly 3D pathing · Voice genome + Mimic horror.
- ✦ `SIM`+`WORLD` **Bee** (pollinator + first builder-creature) · **Songbird** (pollinator + raises `biome_ambient_loudness`) · **Monkeys** (want forests first).
- ✦ `WORLD` **Builder-creatures (§6A)** — architect gene: aesthetic-vector + capability budget + deterministic terraform. *Open Q: griefing bounds; deterministic aesthetic update; gene rarity.*
- ✦ `WORLD` **Grower-resonance field** — `growth_response` scaled by a dragon/grower field. *Open Q: reuse `emission` Wood payload vs dedicated field; accelerate-vs-lift-cap.*
- ✦ `WORLD`+`ATMO` **Dragons & emission** — world/render adapter (`emission`→`HeatField`+impulse) · dragon genome→`Signature` · anatomy-seeded origins · eye-occlusion table · keystone grove · egg=genome item + bond-quest · render consumer (ribbon/cone/aura→bloom/in-scatter) · LOD silhouette/breath · Lauscher translucency gene · wood-dragon proportions · hybrid rules.
- ✦ `ATMO` Sandstorm/weather · underwater fog · other-dimension atmospherics (all studied, none built).
- ✦ `MAIN` Inventory/settings/launcher UI · Multiplayer.

## E. Workflow / tooling / housekeeping

- ▢ **Adopt the delta-patch dev workflow** — switch from full zips to `CHANGES.patch` + `harness/snapshot_diff.sh`. *(This is the fix for the reconcile pain — see the plan.)*
- ▢ **Launcher delta-update path** (`RELEASE_TOOLING.md`) — version-stamped N→N+1 patches, full-download fallback.
- ▢ **Re-baseline perf** — `harness/perf_check.py` baseline predates approach 4's 8-step march.
- ✅ **Reconcile snapshot → `unend-mono`** — *this tree* (was TODO_02's open item).

---

## Cross-cutting dependencies (why single items span areas)

- **Bee** → SIM (kind) + WORLD (pollination) + ATMO (climate temp for `chill`) + MAIN (`creature_design` enum).
- **Rest cycle `chill`** → needs ATMO real temperature.
- **Pollination loop** → needs SIM bee/songbird carriers.
- **Fog real-temp/water closures** → WORLD can supply; ATMO consumes.
- **Stage-tint** → ATMO shader, *then* WORLD deletes the `chunk_manager` remesh sweep.
- **Exotics `fog` flag / dragon emission** → WORLD core → ATMO render consumer.

Build the pure core (sandbox) first; queue the wiring for the next integration pass.
