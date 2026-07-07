# HANDOFF — more plants (`r336-plants`)

**Worker:** `r336-plants`  **Status:** pure cores shipped + gated; scatter wiring is
*proposal-only* (SHARED_RULES §2).

## 0. What shipped (additive, allow-listed `world::plant_blueprint`)
Six new species geometry builders, mirroring the existing `grass_tuft_blueprint`/
`tree_leaf_blueprint` contract exactly (each returns a `Blueprint` the sub-voxel mesher
consumes unchanged via `to_subgrid()`):
`reed_blueprint`, `fern_blueprint`, `vine_blueprint`, `shrub_blueprint`,
`wildflower_blueprint(stem,bloom,..)`, `mushroom_blueprint(stem,cap,..)`.
Plus the catalog the scatter consumes:
- `species_colors(FloraSpecies) -> ([f32;3],[f32;3])` — default (primary, accent) colors.
- `species_blueprint(species: FloraSpecies, seed_hash) -> Blueprint` — **total over
  `flora_placement::FloraSpecies::ALL`**: every species the scatter can place now has geometry.

## 1. The gap this closes
`world::flora_placement` already decides *where + which species* (`species_weights(biome)` is the
biome affinity, `FloraSpecies` the taxonomy) and its own doc says *"Geometry/blueprints live in
`plant_blueprint`"* — but only Grass and tree foliage had builders. The other species
(Fern, Reed, Shrub, Flower, Mushroom, Cactus, Conifer, Broadleaf, Palm) had **no geometry**.
`species_blueprint` is the missing link from the existing taxonomy to a `Blueprint`.

## 2. Scatter wiring (propose — worldgen/bin)
Where the flora scatter places a plant, after it has picked the species:
```rust
use crate::world::{flora_placement as fp, plant_blueprint as pb};

// fp::place(...) already rolls "does a plant land here?"; species is chosen from
// fp::species_weights(ctx.biome) weighted by the per-cell hash (as the scatter does today).
let species: fp::FloraSpecies = pick_species(fp::species_weights(ctx.biome), cell_hash);
let grid = pb::species_blueprint(species, cell_hash).to_subgrid();   // <-- new geometry link
stamp_subgrid(block_pos, grid);                                       // existing mesher path
```
- `seed_hash` must be the deterministic per-cell hash the scatter already derives (no clock/RNG)
  so the same world re-generates identically.
- Colors: `species_blueprint` uses `species_colors` defaults; if the scatter wants biome-tinted
  plants, pass biome/moisture into a future `species_blueprint_tinted` (left as a follow-up — the
  current fn keeps the existing builder signatures).

## 3. Species → geometry mapping (documented)
Grass→`grass_tuft_blueprint` · Shrub→`shrub_blueprint` · Flower→`wildflower_blueprint` ·
Fern→`fern_blueprint` · Reed→`reed_blueprint` · Mushroom→`mushroom_blueprint` ·
Cactus→`shrub_blueprint` (stout cluster stand-in) · Conifer/Broadleaf→`tree_leaf_blueprint` ·
Palm→`fern_blueprint` (frond crown). Cactus/Palm reuse the closest existing/new builder; if the
operator wants dedicated cactus/palm silhouettes later, add two more builders and re-point those
two arms — no contract change.

## 4. Verify commands I ran
```
rustc --edition 2021 --test flat_plants.rs && ./t   # 6 passed / 0 failed (real plant_growth + real plant_blueprint body; rustc 1.96)
cargo check -p unendlich-engine --lib --tests --features "render-core egui"   # 0 errors (in-tree; compiles the 7th test too)
git apply --check -p1 CHANGES-r336-plants.patch     # CLEAN (additive: 310 +, 0 −)
```

## 5. Base / contract note (important)
Validated against the `r330b` tree I had on hand (the `r336-merged` base wasn't provided). In
that tree, `world::plant_blueprint` is the **sub-voxel mesh** system (`Blueprint`/`Segment`), and
the species taxonomy + biome affinity live in `world::flora_placement` (`FloraSpecies` /
`species_weights`) — **not** a `PlantBlueprint{morphology,biome_affinity,growth_params}` +
`blueprints()` type as the brief's text describes (that is the r336-evolved shape). I therefore
grounded in the **real** contract that exists: new `Blueprint` builders + a `FloraSpecies`
dispatcher, reusing `flora_placement::species_weights` for affinity (not duplicating it) and
`plant_growth::advance` for the growth check. If the r336 base genuinely has a different
`PlantBlueprint` species type, the coordinator should re-point `species_blueprint`'s return type
to it — the six builders' geometry logic is unchanged either way. Header is `// r336-plants`.
This lands third in the r336 merge order (`simvectors → dragon-size → plants → …`).
