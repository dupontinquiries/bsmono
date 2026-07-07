# RULESET — invariants the harness enforces

The point of this file: every class of breakage we hit becomes a numbered rule
with a check, so it can't recur silently. The harness runs in two tiers —
**sandbox** (Claude runs `harness/run.sh`, instant, no full tree needed) and
**integration** (you run `cargo` on `unend-mono` + observe in-game). Add rules
freely; the ones below are seeded from this session.

Iteration loop: Claude writes/tweaks → `harness/run.sh` until **GREEN** → ship →
you run the integration tier → paste failures → fix + (if it's a new class) add
a rule. Over iterations the ruleset grows and regressions stop reappearing.

| # | Rule | Why (the break it prevents) | Tier / check |
|---|------|------------------------------|--------------|
| R1 | A module designated *pure* (logic/sim) must not import render or Vulkan types. | `debug_settings` hardcoded `crate::render::world_renderer` and broke when your tree's layout differed; pure modules also stay rustc-testable. | sandbox · `coupling_check.py` |
| R2 | After editing any `*.comp/.frag/.vert`, force its `.spv` to regenerate. | The stale-`.spv` trap silently no-op'd correct shader fixes twice. | sandbox+build · `handoff/check_shaders.py`, `handoff/force_recompile_shader.sh` |
| R3 | Never reorder a UBO or add a descriptor binding blind — prefer a documented-free spare component (e.g. an unused `.a`/`.w`); a new binding requires both Rust + GLSL + the set layout, verified together on your tree. | A Rust/GLSL layout mismatch is silent UB / a validation crash, not a compile error. The fog A/B used the free `sky_color.a` precisely to avoid this. | integration (your tree) |
| R4 | Octree mesher faces wind consistently (CW+BACK = front). | The missing-flat-tops bug. | sandbox · `handoff/check_octree_winding.py` |
| R5 | Discrete actions route through carry-aware `is_key_just_pressed` / `is_action_just_pressed`, never a top-drain-only flag. | The "eaten keys" / double-drain input bug. | integration · `input::tests` (cargo) |
| R6 | Sim invariants hold over randomized inputs: clamp keeps values in the input range (no negative density / ringing); zero wind is identity; curl-noise wind is divergence-free; capacity is monotone in temperature; burn-off conserves water. | Lets the advector/climate be retuned without silently breaking physics. | sandbox · `moisture_field` unit + `prop_*` tests |
| R7 | Layout-independent files (pure modules, `handoff/`, `harness/`) ship safely in a full/overlay zip. Layout-specific files (`world_renderer.rs`, the bin, `chunk.frag`, `lib.rs`, pipeline/descriptor setup) are **hand-merge**, never blind-overwrite. | Full-zip refresh clobbered your adapted `debug_settings.rs` and reverted `unend-mono`'s lead. | both · convention + `coupling_check.py` for the pure set |
| R8 | Every edited `.rs`/shader passes the delimiter balancer before shipping. | Catches the unbalanced-brace class a compiler would, which Claude can't run on the full tree. | sandbox · `handoff/check_delimiters.py` |
| R9 | When a "free" UBO/spare component is repurposed (e.g. `sky_color.a` for `fog_approach`), update that field's doc comment AND the slot registry below; never reuse a slot whose doc still says "unused" without checking shaders. | `wind_params.w` *looked* unused but gated POSTFX — reusing it would have wrecked tonemapping. | both · registry + R16 |
| R10 | Anything sampled per-frame that must be stable is a function of **world** position, never view/froxel/camera space. | The froxel fog swam with the camera; world-anchored analytic fog + the moisture grid don't. | design · review |
| R11 | Pure sim/logic uses only the tick `dt` + seeded deterministic noise — no wall-clock, no unseeded rng. | Keeps the sim reproducible under a fixed tick and the interp stable. | sandbox · `hygiene_check.py` |
| R12 | Per-tick / per-frame hot paths reuse preallocated scratch — no `vec!`/`collect`/grid `clone` per call. | `advect`/`step_climate` reuse `fwd/bwd/prev`; allocating per tick would stutter. | review (heuristic) |
| R13 | Constants shared across the Rust↔GLSL boundary (enum values, step counts) are documented once and kept in sync. | `fog_approach` 0..4 must mean the same in `debug_settings`, the upload, and `chunk.frag`. | both · review + R16 |
| R14 | Sim outputs stay finite (no NaN/Inf) for bounded inputs. | A div-by-zero or `exp` blowup would poison the fog texture. | sandbox · `prop_outputs_stay_finite` |
| R15 | Moisture never goes negative after a climate step. | Negative density is nonsense and breaks the fog math. | sandbox · `prop_climate_keeps_m_nonnegative` |
| R16 | A UBO's Rust struct and GLSL block agree on field **order** and count. | A layout mismatch is silent UB / a validation crash, not a compile error. | sandbox(snapshot)+integration · `ubo_parity.py` |
| R17 | Steady-state GPU `total` p50 and the over-budget tail don't regress past baseline (p50 +20%, over% +1pp). | Catches a tweak that quietly costs frames; baseline seeded from a real run. | integration · `perf_check.py` on a run log |
| R18 | `tick_apply` p50 and the over-8.3ms frame fraction (the felt-stutter metrics) don't regress. | The streaming/edit hitches in the perf log are the only blemish on an otherwise-smooth frame. | integration · `perf_check.py` |
| R19 | New code respects the build's feature gates (`render,hot-shaders,window,gpu,ui-overlay`); `ui-overlay`-only code stays behind `cfg(feature="ui-overlay")`. | A change that only builds in one feature combo breaks the others. | integration · `cargo build` with the flags |
| R20 | Procedural fields seed their noise from a documented seed, not an implicit/global one. | Reproducibility + so two systems sharing noise don't desync. | review |

## Slot registry (R9) — repurposed UBO components

| UBO.field.component | Holds | Set at | Read at |
|---|---|---|---|
| `CameraUbo.sky_color.a` | fog approach selector (`DEBUG.fog_approach`, 0..4) | bin atmosphere upload | `chunk.frag` fog tail |
| `CameraUbo.wind_params.w` | **RESERVED — POSTFX gate** (do not reuse) | — | `tonemap.glsl` |
| `CameraUbo.camera_pos.w` | unused (1.0) — free | — | — |
| `CameraUbo.sun_color.a` | unused — free | — | — |
| `CameraUbo.ambient_color.a` | unused — free | — | — |

> Spare-slot note: approach 4 (advected moisture sim) does **not** consume any
> UBO slot. Its field window is a compile-time constant duplicated in
> `world_renderer.rs` (`MOIST_*`) and `chunk.frag` (`MOIST_ORIGIN`/`MOIST_INV`),
> kept in sync by hand. When player-recentering lands, the window rect moves to
> three of the free `.w`/`.a` slots above (origin_x, origin_z, inv_size).

## Descriptor binding registry (R3) — chunk descriptor set (set 0)

| binding | type | stage | holds | written by |
|---|---|---|---|---|
| 0 | UNIFORM_BUFFER | VERTEX+FRAGMENT | `CameraUbo` | world_renderer / chunk_renderer |
| 1 | COMBINED_IMAGE_SAMPLER | FRAGMENT | shadow map (`sampler2DShadow`) | world_renderer / chunk_renderer (placeholder) |
| 2 | COMBINED_IMAGE_SAMPLER | FRAGMENT | froxel volume (`sampler3D`) | world_renderer / chunk_renderer (placeholder) |
| 3 | STORAGE_BUFFER | VERTEX | chunks SSBO (vertex-pack) | per-frame |
| 4 | COMBINED_IMAGE_SAMPLER | FRAGMENT | **moisture field (`sampler2D`, fog approach 4)** — TAKEN | world_renderer; **chunk_renderer demos leave it unwritten** (pool sized, but needs a 2D placeholder if a demo runs approach 4) |

> `chunk_indirect.rs` has its OWN separate descriptor set layout — unaffected by
> binding 4. Next free chunk-set binding: **5**.


## Tiers in detail

**Sandbox tier — `harness/run.sh`** (Claude, every iteration):
delimiter balance → purity check → compile+run the pure modules' unit *and*
property tests on rustc → winding guard. Must print `HARNESS GREEN` before
anything ships. This is the loop that caught the divergence-free bug live.

**Integration tier — you, when a change touches the tree:**
`cargo build` and `cargo test -p unendlich-engine` on `unend-mono` (runs the
same `moisture_field` / `debug_settings` / `input` tests in-crate), plus the
in-game A/B (the debug panel) for anything visual. Paste failures; each new
failure class earns a new rule here.

## Pure-module manifest (the R1/R6 set)
- `crates/unendlich-engine/src/moisture_field.rs`
- `crates/unendlich-engine/src/debug_settings.rs`

Add a module here only if `coupling_check.py` says it's PURE and it compiles
standalone under `rustc --test`.

## What I (the user) own vs what Claude owns
- **Claude:** writes/tweaks code; keeps `run.sh` GREEN; keeps pure modules pure;
  forces `.spv` regen on shader edits; tags each delivery's files as
  layout-independent vs hand-merge.
- **You:** run the integration tier and report; supply the real layout when a
  rule (esp. R3) needs your tree (chunk.frag, descriptor/UBO setup, the bin
  loop, the heightmap accessor); propose new rules from breaks Claude can't see.

## Authoritative tier: the `unend-mono` cargo suite

`unend-mono` already has a mature test suite (1800+ tests) that SUPERSEDES the
sandbox approximations where they overlap — treat it as the integration tier:
- `shader_artifacts::*` is the authoritative R2/R16 (transitive source hashing +
  build.rs SHADERS coverage). My `check_shaders.py` / `ubo_parity.py` are only
  the can't-run-cargo stand-ins; if they disagree, the cargo test wins.
- `sim::*`, `render_parity::*`, `world::cpu_generator::*` cover engine behaviour
  the sandbox can't reach. New pure-module property tests (R6/R14/R15) run here
  too via `cargo test -p unendlich-engine`.
Sandbox harness = fast pre-ship gate; cargo suite = ground truth.

## GPU work (R22–R26) — gating shaders + compute kernels
Iterating on M2 GPU work uses two tiers (see handoff/GPU_HARNESS.md). These rules keep it as safe as the
pure-module flow.

- **R22 (CPU reference):** every GPU compute kernel SHIPS a pure CPU-reference module (R1-pure,
  pure_test.sh-tested) defining its exact intended math. The shader mirrors the reference.
- **R23 (sandbox shader gate):** every shader stage MUST pass `harness/gpu_check.sh`
  (glslangValidator compile, #includes resolved, + spirv-val). Runs in the sandbox; wired into run.sh
  (best-effort: SKIP if the toolchain is absent, never a hard fail for shader-free rounds).
- **R24 (numeric parity = integration-tier):** GPU↔CPU agreement is PROVEN by a vk-harness test on
  lavapipe (ComputeHarness, cf. tests/mesh_parity.rs), diffing shader output vs the CPU reference. A
  kernel is not "done" until that parity test is green.
- **R25 (layout parity):** push-constant + UBO/SSBO structs match Rust↔GLSL (ubo_parity.py); reuse
  documented-free UBO slots (R3/R9/R16); kernels use the shared ComputeHarness binding/dispatch shape.
- **R26 (.spv regen):** any shader edit regenerates the .spv (subsumes R2 for compute kernels).

## Discovery (R0) — run relevant.sh BEFORE touching a topic
Before starting work on ANY topic — a new system, a fix, or a "does X already exist?" question — run
`bash harness/relevant.sh <keywords>`. It cheaply (no build) returns: (1) the CURATED glossary
(concept → the systems/docs/files that handle it, resolving synonyms the filenames don't), and (2) LIVE
filename + doc matches. The point is to FIND existing infra — a design doc, a CPU reference, a harness
like vk-harness — instead of reinventing or missing it (this rule exists because vk-harness was nearly
missed). When you add a new subsystem, add a one-line row to `harness/glossary.tsv` so the next lookup
finds it.
