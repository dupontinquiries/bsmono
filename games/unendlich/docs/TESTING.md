# Getting unendlich testable again

You have 17 validated-but-unbuilt bricks. This is the shortest path from that to
a green `cargo test` and visible results, then to the bricks actually doing
something in-engine. Read top to bottom the first time.

## Where things stand
- The bricks are logic-correct and unit-tested (186 tests) but were never
  compiled (no toolchain on my end). The realistic failure mode is **API drift**:
  a kernel calls a base API whose signature differs at `88b5542`. These surface
  as type errors at `cargo build`, are local to one module, and are quick to fix.
- The new kernels are deliberately **low-coupling**: most depend only on
  `sim::gpu_bench` (and each other within `sim/`), not on render/Vulkan or world
  internals. So they should compile with little drift; the integration calls
  (step 4+) are where real wiring happens.

## Step 1 — apply the bricks
From the patch zip:
```sh
toolkit/apply_from_zip_v2.sh  unendlich-bricks-patch.zip
# defaults to ~/Code/unendlich-development/unendlich-game; pass a path as arg 2
```
It preflights (clean tree, right branch, base is an ancestor), `git am --3way`s
the 17 patches, and tags each `brick/*`. Roll back anytime with the `git reset
--hard 88b5542` line it prints.

## Step 2 — build & test, bisecting by brick if needed
```sh
cargo build
cargo test -p unendlich-engine sim::          # the new kernels' unit tests
```
If a brick fails to compile, you don't have to fix everything at once — the tags
are a bisection ladder:
```sh
git reset --hard brick/<previous-good>        # drop back to the last green brick
```
Each brick is independent and additive, so a later brick failing never blocks an
earlier one. **Paste me the compiler errors** and I'll fix that module against the
real signatures — that's the one thing I can't see from here, and it's fast to
resolve per-module.

## Step 3 — see a result (closing the loop you've been missing)
Each kernel has a `bench_*` that emits a dashboard-format metric line. Minimal
harness in a scratch test or `examples/`:
```rust
use unendlich_engine::sim::{ao_gpu, volumetric_gpu};
let g = ao_gpu::VoxelGrid::empty(64, 64, 64);
println!("{}", ao_gpu::bench_ao(&g, 64).metric_line("voxel_ao", Some(8.3)));
```
Append those lines to a `run_summaries.txt` and open it in the dashboard
(`toolkit/unendlich-dash/` or `toolkit/unendlich-telemetry.html`) to watch the
GPU-timer panel / compare view. That gets you a tight edit→test→see loop again
without needing the full renderer running.

## Step 4 — wire ONE vertical slice (my recommended sequence)
You gave me the impl call, so here it is, ordered to maximize "testable + visible"
per unit of risk. Do them one at a time; each is independently shippable.

1. **Volumetric godrays into the froxel pass** *(highest visible payoff, lowest
   risk)*. Add a `froxel_integrate.comp` that runs after `froxel_fill`, mirroring
   `volumetric_gpu::froxel_integrate_cpu` against its dispatch contract; add a
   `scatter` `gpu_timer` zone. Cross-check the shader against the CPU reference on
   a few columns (the reference is the correctness gate). You get visible light
   shafts *and* a new analytics stage immediately.
2. **Foveation knob on that pass** — feed `foveation::quality_map` (from the aim
   point) as the per-column budget. Now the `scatter` ms drops in the periphery;
   verify in the dashboard that center quality is unchanged. This is your
   fidelity-vs-frametime lever, measurable.
3. **Perception slice** — give a mob an aggro signal from
   `perception_field` built out of `quantize(perception_fog::perceive(...))` over
   the players in range. Pure sim-side, deterministic, no renderer needed —
   testable headless. This exercises the determinism bridge for real.
4. **Smoke + heat** — inject `smoke_gpu` extinction into the froxel volume (rides
   slice 1) and add the `heat_distort` full-screen post. Both are now cheap
   because the substrate exists.
5. **Netcode loop — last, and only after single-machine is solid.** Wire
   `sync_check::frame_digest` per region on confirmed frames → `consensus::tally`
   at `desync_monitor` cadence → emit `DesyncEvent` + roll the minority. Defer
   this; it's the highest-coordination, lowest-immediate-feedback piece, and it
   wants the deterministic sim paths (slice 3) proven first.

Rationale for the order: slices 1–2 give you eyes-on fidelity and live analytics
(your two stated goals) with almost no coupling; slice 3 proves the determinism
boundary headlessly; 4 is nearly free once 1 lands; 5 is the big-coordination
payoff that benefits from everything before it being trustworthy.

## Step 5 — keep the determinism discipline (for slices 3 & 5)
When you wire anything into the **confirmed** tier: fixed-point only, `BTreeMap`/
sorted iteration, seeded RNG, no wall-clock. A cheap self-test: run the sim twice
on the same inputs and assert equal `sync_check::frame_digest` each frame; run it
on two builds/machines and assert the digests match. Any divergence is a stray
`f32` or `HashMap` ordering leaking into confirmed state.

## What to send me back
Compiler errors (per module), or "slice N built, here's the bench line / a
screenshot." From there I fix drift and build the next slice. The bricks stay the
source of truth; we're just clicking them into the engine one at a time.
