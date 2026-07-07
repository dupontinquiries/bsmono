# Testing — where the sim tests live and how to run them

## What they are

The simulation tests are **inline unit tests**, not separate binaries. Each lives
in a `#[cfg(test)] mod tests { … }` block at the bottom of the same source file as
the code it covers (e.g. `crates/unendlich-engine/src/sim/fall.rs`). Cargo
compiles them into one ephemeral test binary on `cargo test`; being `#[cfg(test)]`
they never ship in the game.

They run against the **pure library** with `--no-default-features` — no GPU,
shaders, render, or window — so they are bit-deterministic (identical on M2 and
x86) and fast. They are the part of the project that can be fully verified
without the machine.

## Running them

```sh
# All simulation tests (~2.5s):
cargo test -p unendlich-engine --no-default-features --lib sim::

# One module:
cargo test -p unendlich-engine --no-default-features --lib sim::fall::

# The whole library suite (~16-18s, ~1680 tests) — run this when in doubt:
cargo test -p unendlich-engine --no-default-features --lib

# Render-side mesher tests (no shader compiler needed):
cargo test -p unendlich-engine --no-default-features --features mesh-ref --lib render::mesher::
```

`toolkit/apply_and_test.sh` applies a delivered patch/zip and runs the suite.

## The mob-behaviour tests

When touching creature/mob behaviour, these are the modules to rerun (counts are
indicative and grow over time):

- `sim::fall` — step-up limits, fall damage, the cliff-march (can't scale a wall
  over time), and **wall containment** (a creature can't ghost/tunnel through a
  one-block barrier even with a large step). `sighted_step`'s path is sub-sampled.
- `sim::feeding` — energy drain/refill, hunger threshold, food-gated breeding,
  mate-picking (opposite sex + fed), and `graze_rate(biome)` (habitat feeding).
- `sim::forage_memory` — remembered food sites, fading, eviction, eaten-forgotten.
- `sim::forage_sim` — the whole look→remember→seek→eat→survive loop, including the
  proof that a creature starves beside grass it never saw (no omniscience).
- `sim::popdyn` — colony dynamics: grows when fed, starves with no grass, eats the
  grass pool down, can't outbreed scarcity.
- `sim::awareness` — information-seeking facing + `turn_toward`.
- `sim::cognition::steering` — the detent + nonlinear turn map (no vacillation).
- `sim::senses`, `sim::eyes`, `sim::localize`, `sim::sound`, `sim::landscape_memory`
  — perception (vision profiles, sound localization, biome masking, memory).
- `genetics`, `sim::popgen` — the diploid genome, sex, and selection.

## Known failures (expected, not behaviour)

Two tests fail by design in this environment and are unrelated to mobs:

- `sim::smoke_gpu::tests::pure_decay_monotonic_decrease`
- `shader_artifacts::committed_spirv_is_fresh_against_transitive_source`

A green run is therefore "N passed, 2 failed" — anything else in the behaviour
modules is a real regression.

## Convention

The behaviour models are pure and tested first; the bin/world wiring sits on top
and is verified by compiling/running on the machine. Whenever mob behaviour
changes, run at least `--lib sim::` (fast) and ideally the full `--lib` suite.
