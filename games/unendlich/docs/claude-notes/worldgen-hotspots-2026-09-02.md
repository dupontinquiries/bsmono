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

## What is left, in order

A surface chunk is now ~44 ms in the breakdown test:

| part | cost | note |
|---|---|---|
| column mask pass | 12 ms | surface_target 5.0, is_cave 4.7, density 2.6 |
| surface/biome pass | 32 ms | of which `biome_at_3d` is 9.6 ms |

`biome_at_3d` (once per column, 9.4 µs): sea_level 1.67, humidity 0.86,
continental+hills 0.46, temperature 0.42, fbm_2d erosion 0.03, and
`surface_y_at` 7.15 µs on the columns that reach the shelf branch.

**The next target is the ~22 ms that is NOT `biome_at_3d`** — the
per-cell surface/subsurface loop. The specific finding:

> `biome_def_at(x, y, z)` recomputes `model.temperature` and
> `model.humidity` and re-runs `registry.best_fit`, and it is called
> from `pick_surface_block` (once per column) and `pick_subsurface_block`
> (three times per column, the `for d in 1..=3` loop). That is 4×
> (temp + humidity + best_fit) per column ≈ 64 `at_block_blended`
> DashMap lookups, recomputing what `biome_at_3d` just computed for the
> same column.

It is **not** a free hoist, which is why it is written down instead of
done: `temperature(x, y, z)` depends on `y` (altitude cooling and the
3D blend), and `biome_at_3d` samples at `origin_y` while
`pick_*_block` samples at the cell's `wy`. Reusing the column value
would change output. The options, cheapest first:

1. Cache `(temp, humid)` per `(column, y)` inside the surface pass — the
   subsurface loop asks for 3 adjacent y values and the surface block
   for a 4th, so a 4-entry stack cache per column is exact and removes
   nothing but repeats. Check whether the 4 y values are actually
   distinct first; if `best_fit` returns the same def across them, the
   whole call collapses.
2. Give `biome_def_at` a variant taking `(temp, humid)` and hoist the
   *per-y* computation to one place in the loop.
3. Longer term: `at_block_blended` does 8 DashMap gets per call and all
   1 024 columns of a chunk fall in one or two region cells — the 8
   corner values are nearly constant for the whole chunk. A per-chunk
   corner cache would make temperature/humidity/sea_level/continental/
   hills all close to free. This is the biggest structural win left in
   the generator and it is measurable before it is written.

## Method note

Every number here comes from an in-process, interleaved A/B or a
component timing in one process. Cross-run timing on this hardware has
a ±25 % band, wider than most of these effects; a before/after of two
separate runs proves nothing and let a regression through earlier in
the session (see `mesher_binary::tests::cache_shape_ab`). Every
optimisation above also ships with an equality test against the code
path it replaced, and the two that ship report identical solid-cell
counts (28811 / 32691 / 24637).
