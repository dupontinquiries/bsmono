# How maps are tested

This project treats map quality as something you *measure*, not just eyeball. Over its life it has grown a
layered set of map-testing tools — from hard "is it even playable" gates up to a Goodhart-resistant "does this
read as a designed place" metric. This doc catalogs them, why each exists, and how to run them, with code
examples you can copy.

The guiding principle, learned the hard way: **counts are gameable, arrangement is honest.** Any metric that
rewards "more cover / more rooms / more props" gets farmed by a generator into a degenerate map that scores
well and plays terribly. So the trustworthy signals measure *structure and arrangement*, and the closed-loop
optimizers are always validated against an independent signal they weren't trained on.

## 1. Hard playability gates (`src/mapeval.rs`)

The floor: a map that fails these isn't shippable regardless of how pretty it is.

**Connectivity** — every room must be reachable from the spawn over walkable cells. A flood fill from room 0:

```rust
use early_fps::level::{Level, PRESETS};
use early_fps::mapeval::score;

let lvl = Level::generate(PRESETS.iter().find(|p| p.name == "Spire Foundry").unwrap().config(42));
let s = score(&lvl);
assert!(s.connected, "every room must be reachable from spawn");
```

**Spawn validity** — spawns must resolve to a real floor cell the body fits on, not inside geometry. Every
authored map's showcase test asserts a full enemy wave populates (a guard against wave-spam / unreachable
spawn bugs):

```rust
let wave = early_fps::enemy::populate(&lvl, 13, 7);
assert!(wave.len() >= 10, "map must populate a full wave");
```

**`score()`** returns a `MapScore` combining openness, flow (sightline variety), and cover into a 0–100 total
with a letter grade. It's the quick "is this a C-grade playable arena or not" read. Note it rewards some counts,
so it is *not* the final word — see §4.

## 2. Sightline analysis (`src/sightline.rs`)

Measures how engagement ranges are distributed. It samples floor tiles and, from each, finds the longest open
cardinal run — but a run is broken only by **eye-height** cover, not low cover you can shoot over. That
distinction is what lets deliberately-placed cover actually shape the map's fights.

`analyze()` returns a `SightlineReport` with a `structure_score` and the short/mid/long range mix. There's also
a feedback helper, `suggest_cover(level, budget)`, that returns the most over-exposed floor positions — the
authored builders call it near the end to auto-articulate a too-open map:

```rust
// inside an authored map builder, after obstacles exist:
lvl.scatter_exposure_cover(6, rgb(110, 106, 98)); // drop up to 6 cover pieces on the worst lanes
```

Run `cargo run --example sightline_report` for a per-map breakdown.

## 3. Liveliness (`src/liveliness.rs`)

Static metrics can't tell whether a map *feels* alive in motion. The liveliness harness simulates activity
(bots moving, hazards, events) and folds it into a per-frame activity scalar, then scores the resulting time
series — rewarding periodic pricks of motion against a tense-quiet baseline rather than constant churn (which
the eye tunes out) or a dead space. Run `cargo run --example liveliness_report`.

## 4. Composition — the honest, Goodhart-resistant metric (`src/composition.rs`)

This is the metric that answers *"does this read as a designed place, or as noise?"* — and it's the one to
trust when a generator is optimizing against a score. `analyze()` measures **arrangement**: clustering,
void structure, landmark hierarchy, and multi-scale organization — none of which you can farm by spamming more
props. It also computes a `noise_likeness` that hard-caps the result:

```rust
use early_fps::composition::{honest_quality, honest_grade};

let q = honest_quality(&lvl);          // 0..1, noise-penalized
let grade = honest_grade(&lvl);        // 'A'..'F', tuned so few maps earn an A
println!("{grade} ({q:.2})");
```

`honest_quality` blends composition (0.6) with tactical sightline structure (0.4), then multiplies the result
down as `noise_likeness` climbs past 0.6 — so a noise-like layout is capped low no matter how its sightlines
happen to fall out. The grade thresholds are deliberately strict: most maps land B/C/D, and only genuinely
well-composed *and* well-playing maps reach A. Authored showcase maps grade B/A; raw procgen output lands ~C.

**Why two scores?** `mapeval::score` is the fast playability gate (and is somewhat gameable). `honest_quality`
is the trustworthy design read. When they disagree, believe composition.

## 5. Mesh & geometry audits (`src/... ` + `examples/mesh_audit.rs`)

Separate from layout quality: does the geometry hold up? `mesh_audit` checks triangle counts, degenerate tris,
and that nothing renders inside-out. `perf_bench` times frame assembly so a map that's beautiful but drops
frames gets caught.

## 6. Visual verification (headless renders)

Because the renderer is a CPU software rasterizer, we can render maps to PNG headlessly and inspect them (and
analyze them numerically with PIL/numpy) without a GPU. Key tools:

- `examples/scenic_shots.rs` — elevated orbit + eye-level establishing shots of every map.
- `examples/preview.rs` — headless world render to PNG.
- `examples/procgen_render.rs` / `rosetta_render.rs` — frame the densest cluster of generated maps at eye level.

A hard-won lesson lives in these: an early "blank render" scare turned out to be *accurate* — the maps really
were that sparse, viewed from too far. The fix was framing the densest local cluster at near-eye level and
verifying numerically (triangle counts, PIL pixel stats) as the primary signal, with the image as confirmation.

## 7. The one-command dashboard (`examples/mapqa.rs`)

Rolls the above into a single health dashboard for every authored map, so iterating on a roster doesn't mean
running five tools and cross-referencing:

```
cargo run --example mapqa                # score every authored preset
cargo run --example mapqa -- <substr>    # only maps whose name matches <substr>
cargo run --example mapqa -- --fail      # exit non-zero if any map fails a hard check (CI gate)
```

The `--fail` mode is the CI gate: connectivity, spawn validity, full-wave population, and minimum grade are all
hard checks. A map that regresses on any of them fails the build.

## 8. Regression tests (`src/level.rs` test module)

Every authored map has a showcase test asserting its invariants: connected, standable, full-wave population,
real verticality (platform count / distinct heights), signature features present (water bodies, fire sources,
etc.), and a minimum grade. Example — the multi-level map's test asserts genuine stacked floors:

```rust
let l = Level::generate(preset.config(42));
assert!(score(&l).connected);
let max_h = l.platforms.iter().map(|p| p.top).fold(0.0f32, f32::max);
assert!(max_h > 7.0, "the upper gantry should be genuinely high up");
// platforms at ≥2 distinct raised tiers → real multi-level play
```

These run in the normal `cargo test` suite (the `level::` module is slow — ~180s — so it's often run on its
own: `cargo test --lib level::`).

## Summary of what to run when

| Question | Tool |
|---|---|
| Is it playable at all? | `mapeval::score` (`.connected`, `.total`) |
| Do the fights have range variety? | `sightline::analyze` / `sightline_report` |
| Does it feel alive in motion? | `liveliness_report` |
| **Does it read as designed, not noise?** | **`composition::honest_quality` / `honest_grade`** |
| Is the geometry sound / fast? | `mesh_audit`, `perf_bench` |
| What does it actually look like? | `scenic_shots`, `preview` |
| All of the above, one command | `mapqa` (`-- --fail` for CI) |
