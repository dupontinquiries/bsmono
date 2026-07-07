# RECONCILE_REPORT — three thread snapshots → one `unend-mono`

This tree is the reconciliation of the three divergent worktree snapshots
(`unendlich-monorepo-01/02/03`), one per parallel thread. It follows
**RULESET R7**: layout-independent files (pure modules, `handoff/`, `harness/`)
were taken wholesale; layout-specific files (the bin, `world_renderer.rs`,
`chunk.frag`, `lib.rs`, pipeline/descriptor setup) were hand-merged, never
blind-overwritten.

## Lineage (established by file-set + 3-way diff, no git history was present)

- **t01** — sim / locomotion / body-physics / creature-AI thread. The most
  complete base: it carries the 23 shared-ahead files that t02 lacks, the full
  locomotion stack, and the compiled `.spv` shaders.
- **t02** — fog / GPU-bridge thread. The **stale fork**: it predates 23 files
  that *both* t01 and t03 have (a batch of `sim/` primitives, `plant_genetics`,
  `upload_repr`, the `.spv`s). It uniquely carries the **fog/moisture work** and
  the **entire `harness/` toolchain**.
- **t03** — plant / dragon / performance thread. Shares t01's recent ancestor;
  uniquely carries `exec_tier`, `plant_exotics`, `pollination`, `stage_tint`,
  and is **ahead on `octree.rs` + `chunk_manager.rs`** (incremental-edit / perf).

649 files are common to all three; only **18 differed**. The merge is almost
entirely additive — which is itself evidence of the pure-module discipline.

## What was taken, and why

| Area | Decision | Basis |
|---|---|---|
| Base tree | **t01** wholesale | most complete |
| `sim/exec_tier.rs`, `world/{plant_exotics,pollination,stage_tint}.rs` | added from **t03** | t03-unique pure cores (R7-safe) |
| `world/octree.rs`, `world/chunk_manager.rs` | took **t03** | t03 is ahead (perf/incremental edit); t01==t02 had the old version |
| `moisture_field.rs`, `moisture_texture.rs` | added from **t02** | t02-unique fog modules |
| `harness/` (whole dir) + `RULESET/GPU_BRIDGE/RELEASE_TOOLING` | added from **t02** | t02-unique; layout-independent (R7-safe) |
| `lib.rs`, `sim/mod.rs`, `world/mod.rs` | **declaration union** | added the new modules' `pub mod` lines onto t01 |
| `debug_settings.rs` | **merged → t02's decoupled `LookParams` design** | t02 is *ahead* here: t01 still imported `crate::render::world_renderer::AtmosphereParams` (fails R1). Verified PURE + CLEAN + tests pass. |
| `bin/unendlich_world.rs` | t01 base + **one call-site change** | `apply_look` now takes `&mut LookParams`; the bin copies the 5 fields in/out. All five fields confirmed present on t01's `AtmosphereParams`. |
| `creature.rs`, `sim/item_entity.rs`, `sim/lightning.rs`, `world/cpu_generator.rs` | kept **t01** | t02 differs only by being behind (e.g. item_entity −108, lightning −124 lines) |
| `spawn.rs`, `docs/foraging_and_ecology.md` | kept **t01** | t01 is the one that advanced them |

## Verified here (sandbox tier — `harness/run.sh` is GREEN)

- R8 delimiter balance on every edited file.
- R1 purity, R11/R21 hygiene, R6/R14/R15 unit+property tests for the pure set.
- R4 octree winding guard; R16 `CameraUbo` Rust↔GLSL parity (10 fields, order matches).
- **Pure-module regression sweep, 16 modules / 104 tests, all PASS** across all
  three threads' standalone cores (exec_tier, plant_exotics, ragdoll, deform,
  body_scale, stamina, turning, liveliness, ai_budget, neighbor_grid,
  buddy_pool, column_memo, surface_cache, cadence, moisture_field, debug_settings).
- Every `pub mod` in `lib.rs`/`sim/mod.rs`/`world/mod.rs` resolves to a real file.

## NOT verifiable here — run the integration tier on the M2

Per APPLY.md: `cargo build --release --bin unendlich-world --features render,gpu`
and `cargo test --release --lib -p unendlich-engine --features render,gpu`.

1. **The fog → renderer wiring is delivered as a port kit, NOT applied.**
   `handoff/fog_port/*.fog.patch` holds the clean-t01 → t02 diff for
   `world_renderer.rs`, `chunk_renderer.rs`, `chunk_pipeline.rs`, `chunk.frag`.
   The fog *modules* (`moisture_field` tested, `moisture_texture`) are already in
   the tree; this kit lands the *wiring*. **Do not apply the whole-file diffs
   blind** — t02's render files predate t01's ahead render work (SSR, etc.), so a
   wholesale apply would revert it. Apply only the fog hunks. The genuine
   conflicts to resolve by hand (the rest is additive):
   - **Debug panel (`world_renderer.rs`, ~L2659–2671):** t01 has the *winding-sweep*
     UI, t02 has the *fog A/B* UI in the same egui block — **keep BOTH.**
   - `descriptor_count` pool size: take t02's higher count (binding 4 added).
   - `sky_color.a`: take t02's `fog_approach` selector (documented-free slot, R9).
   - `use ...{DEBUG, Look}` → add `FogApproach`.
   - Pipeline bind: **keep t01's `self.pipeline.current()`** (ahead) over t02's
     `.pipeline`.
   - **R2:** `chunk.frag` changes → regenerate `chunk.frag.spv`
     (`handoff/force_recompile_shader.sh`) or the fix silently no-ops.
   - **R16/R13:** keep `MOIST_*` consts in sync between `world_renderer.rs` and
     `chunk.frag`.

2. **`moisture_texture.rs` is declared plain `pub mod`** (mirroring t02). It uses
   `ash` unconditionally, so it only builds under the render feature set you use
   (`render,gpu`). If you ever do a no-render build, gate it
   `#[cfg(feature = "render")]` (R19).

3. **`chunk_manager.rs` (taken from t03) still has the transition-remesh sweep.**
   t03 BACKLOG #3 plans to *delete* that sweep once the fragment-shader tint
   animation (`stage_tint`) lands on the GPU. Tracked in BACKLOG.

4. **`cpu_generator.rs` (t01) note:** t02 had a 9-line binary-search surface-height
   helper t01 lacks — a candidate for the fog "real heightmap closure for wind
   deflection" (TODO_02). Not ported; flagged for if/when that fog item is built.

See `OWNERSHIP.md` (who owns what + the routing rule) and `BACKLOG.md` (the three
TODOs merged into one owner-tagged board).
