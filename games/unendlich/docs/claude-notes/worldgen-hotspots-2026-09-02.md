# CPU worldgen: where the time is (2026-09-02)

Measured on one 2.1 GHz core, opt-level 2, in-process and interleaved.
The tests are in the tree: `cpu_generator::hotspot_tests::
column_pass_breakdown` and `::biome_pass_breakdown`, both `#[ignore]`d.
Run them before changing anything here — this file exists because I
guessed the hot spot wrong twice in a row.

## The headline

`generate_chunk` was **225–267 ms per chunk**. Worldgen was ~17× the
mesher, which makes it the streaming bottleneck, not meshing and not
uploads. It is now **38–81 ms** after two fixes, both of the same shape:
a pure function of `(x, z)` being recomputed per cell when the caller
already had the value.

| chunk | before | after |
|---|---|---|
| surface (y=0) | 231 ms | 61 ms |
| underground (y=-3) | 222 ms | 38 ms |
| far surface | 267 ms | 81 ms |
| sky (y=6) | 0.1 ms | 0.1 ms (early-out already existed) |

`surface_target(x, z)` costs ~4–5 µs and is the most expensive primitive
in the generator. Everything below is about how often it, and its
siblings, get called.

### Fixed

1. `WorldGenerator::is_cave` computed `surface_target` itself, for every
   cell → 1 024 distinct values computed 32 times each, ~134 ms/chunk.
   `is_cave_with_surface` takes the column's value.
2. `surface_y_at` scans ~130 y values for one column and every probe
   recomputed `surface_target` **twice** (inside `density`, inside
   `is_cave`) while the function already had it for its scan seed. Plus
   an `std::env::var` per call. Both hoisted.

### Measured and NOT worth it (do not retry without new evidence)

- Monotone early-outs in `WorldModel::is_cave`: −0.9 %. All 32 768 calls
  together are ~2–5 ms. Reverted.
- Skipping the density noise outside the decided band: −4 % on surface
  columns (kept anyway — it is free and exact — but it is not a lever).
  The density field is ~2.6 ms per chunk.

## Caveat on every number above: the biome registry

`TerrainGenerator::new` loads `BiomeRegistry::load_standard()` in
production; a bare `WorldGenerator::new(seed)` has none, and with no
registry `biome_def_at` returns `None` immediately and never computes
temperature or humidity. Timings taken without it therefore understate
the real generator. The tests now use `production_like_generator`,
which loads the standard registry (39 biomes) when it can and prints
which it got — read that line before trusting a number.

With the registry, per chunk: surface 48 ms, underground 33 ms, far
surface 68 ms.

## What is left, in order

A surface chunk is now ~44 ms in the breakdown test:

| part | cost | note |
|---|---|---|
| column mask pass | 12 ms | surface_target 5.0, is_cave 4.7, density 2.6 |
| surface/biome pass | 32 ms | of which `biome_at_3d` is 9.6 ms |

`biome_at_3d` (once per column, 9.4 µs): sea_level 1.67, humidity 0.86,
continental+hills 0.46, temperature 0.42, fbm_2d erosion 0.03, and
`surface_y_at` 7.15 µs on the columns that reach the shelf branch.

**The next target is the per-cell surface/subsurface loop.** The
specific finding:

> `biome_def_at(x, y, z)` recomputes `model.temperature` and
> `model.humidity` and re-runs `registry.best_fit`, and it is called
> from `pick_surface_block` (once per column) and `pick_subsurface_block`
> (three times per column, the `for d in 1..=3` loop). That is 4×
> (temp + humidity + best_fit) per column ≈ 64 `at_block_blended`
> DashMap lookups, recomputing what `biome_at_3d` just computed for the
> same column.

**Measured with the registry loaded** (`biome_def_breakdown`):
`biome_def_at` is 1.108 µs per call, 4.54 ms per chunk for the 4 calls
per column — and of that, temperature+humidity is 4.48 ms while
`best_fit` over 39 biomes is only 0.27 ms. So the cost is entirely the
climate recompute, not the biome search.

**The obvious fix — one `biome_def_at` per column instead of four — is
REJECTED on measurement.** `biome_def_y_disagreement_sweep` puts 38 400
subsurface cells across 8 far-apart regions against their column's
surface def, sampling at each column's ACTUAL surface (not a fixed
altitude): **1 261 differ, 3.28 %**. They are not near-misses either —
e.g. `unendlich:temperate_forest` at the surface against
`arenas:gilded_glade` three blocks down, which is a visibly different
block once dug. Collapsing the four calls would rewrite ~3 % of
subsurface blocks.

An earlier sample said 0 of 256 columns differed; that sample probed
fixed altitudes (y = 40..37) rather than each column's surface, so it
was mostly asking the question in air and rock far from any climate
boundary. The sweep is the honest version and it says no.

It is **not** a free hoist, which is why it is written down instead of
done: `temperature(x, y, z)` depends on `y` (altitude cooling and the
3D blend), and `biome_at_3d` samples at `origin_y` while
`pick_*_block` samples at the cell's `wy`. Reusing the column value
would change output. The options, cheapest first:

1. ~~One `biome_def_at` per column~~ — **rejected, 3.28 % of subsurface
   cells would change** (see above). Do not revisit without changing the
   climate model itself. This is why the sweep exists: the idea looked
   free, the cheap sample agreed with it, and it was wrong.
2. Give `biome_def_at` a variant taking `(temp, humid)` and hoist the
   *per-y* computation to one place in the loop.
3. **The one to do next**: `at_block_blended` does 8 `DashMap` gets per
   call, and the climate recompute (4.48 ms of the 4.54 ms) is entirely
   those gets — `best_fit` over 39 biomes is 0.27 ms, so the search is
   not the problem. All 1 024 columns of a chunk fall in one or two
   region cells, so the 8 corner values are nearly constant chunk-wide.
   A corner memo is EXACT — it returns the same values, unlike option 1
   — which makes it the right shape: temperature, humidity, sea_level,
   continental and hills all get close to free, and no equality sweep is
   needed beyond "same values out".

   The implementation problem is where to put it. `RegionField<T>` is
   generic and shared across rayon workers, so a `Mutex` memo would
   contend and a plain `thread_local!` cannot name `T`. The two ways
   that work: (a) pass a caller-owned `CornerCache<T>` down from
   `generate_chunk`, which is explicit but touches several signatures;
   (b) a small direct-mapped per-thread cache in front of `at()` keyed
   by `(field id, RegionCoord)` with the field carrying a `u32` id.
   Measure with `biome_pass_breakdown` before and after.

## Method note

Every number here comes from an in-process, interleaved A/B or a
component timing in one process. Cross-run timing on this hardware has
a ±25 % band, wider than most of these effects; a before/after of two
separate runs proves nothing and let a regression through earlier in
the session (see `mesher_binary::tests::cache_shape_ab`). Every
optimisation above also ships with an equality test against the code
path it replaced, and the two that ship report identical solid-cell
counts (28811 / 32691 / 24637).
