# Headless Coverage, Divergence & Butterfly Testing — wiring-faithful plan
(ingests docs/design/headless_coverage_testing v2 + 100days notes; maps
every concept onto machinery THIS tree already has)

## Premise (operator doc, verified against the tree)
"The world is bit-exact given a seed and an input log. The only
butterflies are the player." Verified foundations: Fixed-point physics
(physics::Fixed), mob RNG seeded from entity id, deterministic tick
scheduler (world::ticks), seeded worldgen with CPU<->GPU parity tests,
InputBuffer/InputFrame/ActionBits (input_capture.rs) = the input log
format ALREADY DEFINED.

## The harness (v1 SHIPPED as examples/headless_run.rs, r190)
One process, no window, fixed dt:
  seed -> WorldGenerator -> N ticks of {fuzzed InputFrame -> sim step ->
  per-tick DIGEST} -> JSON report + PPM snapshot(s).
- FUZZED INPUT, deterministic: an LCG over (seed, tick) emits ActionBits
  + look deltas. Same seed = same run, byte-for-byte. "Butterfly pair"
  mode: run twice with ONE flipped input bit at tick T; the report's
  divergence_tick is where digests first split, and the divergence
  CURVE (hamming distance of digests over time) is the butterfly trace.
- DIGEST per tick: FNV over [player pos quantized, body vitals, chunk
  edit count, drop count, mob count, registry fire counters]. Cheap,
  stable, diffable.
- COVERAGE SCORE, multi-dimensional (the doc's §10-§14 mapped):
  * KERNEL/SERVICE coverage: which generated registry entries fired
    (kernel_registry_gen* + service_registry* enumerate the universe —
    the AI-code-coverage instrument the doc asks for, already enumerable).
  * TICK-HANDLER coverage: which TickRegistry handlers ran (names known).
  * BLOCK-INTERACTION coverage: which BlockIds were placed/broken/stood-on.
  * BIOME coverage: which Biomes were visited (cheap from digests).
  * FSM TRANSITION coverage (§12): game_flow transition PAIRS exercised.
  Score = weighted fraction per dimension; report prints the matrix.
- IMAGE OUTPUTS for operator review: the 27 existing examples
  (flythrough_snapshot, sim_collage, heightmap_collage, menu_snapshot...)
  are the visual tier; headless_run emits a top-down world PPM per run so
  subtle worldgen/sim bugs are VISIBLE (operator: "so I can check too").
- SPEED: target <2s for 1k ticks at 9-chunk residency (measured below).

## 100-days benchmark (the txt's idea, staged)
A scripted long-run "playthrough" = phases (spawn/gather/craft/build/
mine/fight/farm) each a fuzz profile; coverage matrix per phase; ranking
= the doc's numerical multi-rank. v1 ships phase profiles as fuzz seeds;
the full 100-day script composes them once mobs/combat merge.

## Divergence analysis for CI
Golden digests per (seed, input-log) commit to docs/golden/. A run that
diverges names the FIRST tick + the dimension that moved — regression
bisection without graphics. render_parity::image_diff covers the visual
tier for snapshot drift.

## Open design challenges (for the operator)
1. Registry fire-counters need a cheap global (atomic per entry id) —
   v1 instruments the TickRegistry only; kernel/service counters next.
2. Determinism audit: any HashMap iteration affecting sim order is a
   butterfly NOT from the player (found one candidate: spawn director
   uses world-pos hash — fine; audit list in report).
3. Chunk digests: full-content FNV is too slow per tick; v1 hashes edit
   COUNTS; per-chunk rolling digests on write is the v2 design.
