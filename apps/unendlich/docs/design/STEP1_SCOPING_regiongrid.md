# Step 1 Implementation Scoping: The Coarse Regional Field Already Half-Exists

> Status: implementation scoping (r262). Concrete code-grounded notes for
> `dynamic_world_architecture.md` build Step 1 (the shared coarse regional terrain
> field) = the far-mesh perf fix (`PERF_FINDING_far_region_corrected.md`). NOT a
> change — scoping, so that when Step 1 is built (after the dynamic systems run)
> it starts from the real code, not the abstract design. No code touched.

## The key finding: `RegionGrid` is already the pattern we need

`WorldGenerator` (`world/world_model.rs`) **already caches coarse per-region
fields** in `RegionGrid<f64>`:
```
sea_level_base, pressure_base, humidity_base, temperature_base, continental
```
Each is built once via `make_grid(region_size, seed_offset)` and sampled cheaply.
So the "compute a coarse regional field once and share it" pattern the deep-time
arc kept proposing is **not new infrastructure** — it exists and is load-bearing
in worldgen today. Step 1 is therefore an **extension of an existing pattern**,
not a from-scratch caching layer. That lowers the risk and the scope.

## What's missing (the actual gap = the 11ms)

The expensive far-mesh cost (`render/mesher.rs::mesh_chunk_far_virtual`, measured
~11–13ms/chunk) is the **per-grid-cell** calls to:
- `generator.density(wx,wy,wz)` and
- `generator.is_cave(wx,wy,wz)`  (three 3D noise layers)

These are NOT backed by a `RegionGrid` — they're evaluated fresh per cell, per
far-chunk, every rebuild. That's the gap: the cheap coarse fields are cached, but
the expensive density/cave terms the far-mesher needs are not.

## The concrete Step-1 shape (when it's time)

1. **Add a coarse density/cave field to the `RegionGrid` family.** A
   `RegionGrid` (or small set) keyed at the far-mesh grid resolution (factor 4/8 →
   8³/4³ per chunk) holding the precomputed solid/cave verdict (or the inputs to
   it) per coarse cell. Built once per region like the existing base grids.
2. **Far-mesher reads the field instead of calling the generator per cell.**
   `mesh_chunk_far_virtual`'s inner loop changes from `density()&&!is_cave()` per
   cell to a lookup into the shared coarse field. The 11ms collapses to a sample.
3. **The SAME field is what the regional sim reads** (water/karst/erosion needing
   terrain structure) — so mesh + sim are consistent by construction, the whole
   point of `PERF_FINDING_far_region_corrected.md` (never skip; share).
4. **Determinism/storage hooks (forward-looking):** make the field a `Fixed`,
   fixed-order, stable-digest quantity (per `simulation_guided_seeding.md` /
   `sim_vectors_and_correction_deltas.md`) so it doubles as the per-region content
   identity. Not required for the perf win alone, but free to do right the first
   time.

## Why it's still gated (don't build it yet)

- It changes what far terrain renders (field-derived vs. per-cell regen) → needs
  GPU validation on the M2; can't verify here.
- The arc's ordering: the field's *shape* should be informed by the actual
  regional-sim consumers (water/wind/storm/population), which means Step 0 (the
  dynamic systems running) should exist first so the field serves all consumers,
  not just the mesh. Building it mesh-only now risks a field that the sim later
  can't use, forcing a redo.
- BUT the perf win (caching the far-mesh density/cave) is *separable* and could be
  a smaller standalone change if the far-mesh chug becomes urgent before Step 0 —
  scoped here as: "add a RegionGrid for the far-mesh solidity verdict, read it in
  mesh_chunk_far_virtual." That's the minimal version, and it's an extension of an
  existing, tested pattern.

## Cheap thing already true
`RegionGrid<T: Send + Sync + Clone>` is already generic + thread-safe (workers
share it), so the extension inherits the existing sharing/threading. No new
concurrency design needed — another reason this is lower-risk than the abstract
doc implied.

## Honest status
Scoping only. The finding that `RegionGrid` already implements the share-coarse-
field pattern is the useful part: Step 1 (and its perf-fix subset) is an extension
of working code, not new architecture. The actual change is gated on a GPU run +
ideally Step 0, and is filed, not started.
