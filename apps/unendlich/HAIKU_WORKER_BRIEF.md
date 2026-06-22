# HAIKU WORKER BRIEF — unendlich parallel wiring (paste this to the worker)

You are a worker on the unendlich Rust voxel engine. You work FAST but you are
graded ONLY on changes that compile and demonstrably run. Follow these rules
exactly; they exist because previous fast runs produced unverifiable claims.

## Hard rules

1. NO new documentation files. Zero. No START_HERE, no PHASE guides, no
   summaries. Your entire report is: files touched, exact commands run, exact
   output. If you didn't run a command, you may not claim its result.
2. NO new architecture. The repo already has THE wiring mechanism:
   - stateful services -> `#[distributed_slice(SERVICES)]` in
     `sim/service_registry.rs` (host already ticks in the bin)
   - pure fns -> `#[distributed_slice(KERNELS)]` in `sim/kernel_registry.rs`
     (host already sweeps in the bin)
   Do NOT create parallel layers (no new `integration/` modules, no service
   fields bolted onto `App` unless the task says so). Two architectures for
   the same job is how merges die.
3. NO phase plans. Every task below is a leaf: one sitting, one verification.
4. Claims discipline: "wired" means a log line proves it fired at runtime, or
   a test exercises it. "Accessible" is not a number anyone cares about.
5. Verification per change (pick what the task says):
   - `cargo check --release -p unendlich-engine --features render`
   - `cargo test -p unendlich-engine --lib <module>`
   - `python3 tools/bootstrap_registry.py` (regenerates + self-heals the
     generated registries; run after adding any module)
6. Every feature you wire gets a `log::info!` line proving it fired, with its
   kill switch named. Pattern: see the `[features]` banner in
   `unendlich_fullgame.rs`.

## Leaf tasks (each independent; do in order; stop when a verification fails
## and report rather than piling on)

T1. Run `python3 tools/bootstrap_registry.py` to completion. Report survivor
    counts and anything it pruned. (This alone may take a while; it is the
    highest-value single command in the repo.)

T2. dragon_wing_flex -> mesh. `sim/dragon_wing_flex.rs` computes wing flex but
    never reaches `world/entity/forge_to_vertices`. Find where the dragon's
    forge body boxes are emitted at mesh time; thread an anim parameter
    (anim_time already ticks in the bin) so wings rotate. Verify: lib test or
    a headless PNG example per `examples/sim_collage.rs` pattern.

T3. Hand adapters for the top 10 modules `tools/audit_wiring.py` (if present)
    or `tools/bootstrap_registry.py --no-verify` skipped for non-synthesizable
    ctor args. Pattern: `sim/service_registry_batch.rs` (FluidService /
    MoistureService show the shape, including neutral-but-valid inputs).
    Verify: `cargo test -p unendlich-engine --lib registry -- --skip report_all_panickers`.

T4. Items-on-ground render path. `items/drops.rs` tracks ground items; nothing
    renders them. Smallest visible step: represent a drop as a half-scale
    block at its position via the existing entity/instance path the test mobs
    use. Verify: compile + a log line per spawned drop.

T5. Bloom pass. `render/bloom.rs` exists; check whether it is invoked in the
    frame graph in `render/world_renderer.rs`. If dormant, wire it behind
    `UNENDLICH_BLOOM=1` with a startup log line. Verify: compile with
    `--features render`; do not claim visuals you cannot see.

## Report format (and nothing else)

FILES: <list>
COMMANDS: <each command + last 3 lines of its output, verbatim>
WIRED: <feature -> the log line that proves it>
FAILED/BLOCKED: <what + the actual error text>
