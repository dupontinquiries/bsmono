# SECTION — World, Terrain & Flora

*Plan of record for the world domain. WID r74-04-section-world. Grounded in the
merged tree (paths/APIs below were read, not guessed) and the specs
`flora_plant_tree`, `flora_update_genetics`, `river_systems`, `cave_biomes`,
`textures_overlay_system`.*

## 1. What this section is

Everything that turns a 16-byte seed into a standable, living world: the CPU
worldgen pipeline (density, caves, biomes, ores), the in-memory query/LOD
structures it feeds, the flora life-system (genetics, growth, combustion,
pollination, dispersal), hydrology (rivers), cave biomes, and the
property-driven material/overlay system that feeds the PBR shaders. Player-facing
goal: a deterministic, regenerable world that *reacts* — trees that breed and
burn, rivers that carve and flow, caves with their own ecology, surfaces that
weather with moisture/heat/frost.

## 2. Already built (read the tree)

**Worldgen core & query structures**
- `world/cpu_generator.rs` — `WorldGenerator` (`new(seed)`, `density(x,y,z)->f64`,
  `is_cave->bool`, `sea_level_at`, `biome_at_3d->BiomeSample`, `pick_surface_block`
  / `pick_subsurface_block`). Owns no world logic — queries `WorldModel`. Has an
  **all-air fast-path** for high chunks (early-out, no phases).
- `world/noise.rs` — value-noise / fBm in **f64** (`noise_3d`, `fbm_3d`); the
  deterministic floor of the whole pipeline.
- `world/octree.rs` — `OctreeArray` flat sparse octree (`from_chunk`, `sample`,
  `to_chunk`, `set_block`, modification stamps, `OCTREE_DEPTH=5`). In-memory
  **query/LOD** structure, **not** storage (see §COMPRESSION_DECISION).
- `world/surface.rs` + `world/surface_run.rs` — `SurfaceRecord` (16 B per
  emitting face) and run-length `SurfaceRun` extraction (`extract_surface`,
  `extract_surface_runs`) feeding the GPU mesher.

**Worldgen perf threads** (PERF_BASELINE.md: worldgen is **~99.7%** of CPU prep)
- ~35 `world/batch_*.rs` structure-of-arrays helpers, e.g. `batch_density_noise.rs`
  (8-wide **bit-exact** fBm, r62), `batch_ridge_noise`, `batch_strata`,
  `batch_terrain_warp`, `batch_erosion_advect`, `batch_vegetation_density`,
  `batch_cave_tunnel_radius`, `batch_river_meander/width`, `batch_flow_route/velocity`.
- `docs/PERF_BASELINE.md` — `perf-probe` CSV: worldgen ~100 ms/chunk dominates;
  octree encode ~310 µs; density is **regional** (sky 0% solid, deep ~98% solid).

**Biomes** — `world/biomes.rs` (`Biome` enum, `BiomeSample`),
`world/biome_loader.rs` (`BiomeRegistry`), `biome_blend` / `biome_blend_weight` /
`biome_transition` / `biome_temp_map` / `batch_biome_temp_map`.

**Flora life-system** (sim/, pure)
- `sim/tree_pollination.rs` — `TreeGenome`, `cross(a,b,seed)`,
  `pollination_chance(dist,max_range,wind_align01,compat01)`, `viable(seed,chance)`
  (flora_update_genetics **§C**; current genome is a simplified continuous
  phenotype, codominant averaging).
- `sim/combustion_spread.rs` — `burn_duration(cat,fr_tier)`, `ignition_chance`,
  `BurnState`/`tick_burn` (flora_update_genetics **§A**; FR-tier scaled burn).
- `sim/plant_tree_genetics.rs`, `plant_gene_express.rs`, `plant_growth.rs`,
  `plant_growth_env.rs`, `plant_propagation.rs`, `plant_root_system.rs`,
  `seed_dispersal.rs`, `flora_harvest.rs`, `bee_pollination.rs`,
  `pollination_route.rs`; `world/forest_density.rs`, `vegetation_density.rs`,
  `vegetation_scatter.rs`.

**Hydrology** — `sim/river_flow.rs` (`flow_dir_d8`, `accumulate_flow`,
`channel_velocity(slope,depth,roughness)`, `is_channel`) — D8 core for
river_systems **§3**; archetype tuning not yet applied.

**Materials / textures (feeds PBR)** — `world/block_material.rs`,
`block_textures.rs`, `texture_mapping.rs`, `material_response.rs`,
`aquatic_materials.rs`.

**Storage decision** — `docs/COMPRESSION_DECISION.md` + `examples/forest_codec_bench.rs`:
octree is **not** a compressor (4–8× larger than dense); ship **seed** for
unmodified chunks (~16 B) and **lzma'd dense voxels** for modified (~0.5–0.9 KB);
neural compression closed. Octree never crosses the wire.

## 3. The big sub-sections to build

**3a. Diploid genome + dominance** — flora_update_genetics **§C.2** (diploid,
five loci `FIRE_RES`/`WATER_REQ`/`SUN_REQ`/`WIND_TOL`/`SIZE_POT`) + **§C.6**
(species-default trait table). Current `TreeGenome` is single-value continuous;
the spec wants paired alleles + dominance rules. Seam: composes
`sim::tree_pollination::cross` and feeds `combustion_spread` (FR tier),
`plant_growth_env`. *Pure.*

**3b. Flora species catalogue + environmental thresholds** — flora_plant_tree
**§2–§7** (temperate / conifer / tropical / wetland / rare trees, ground cover)
and **§7.4** (growth thresholds). A data catalogue + a `can_grow(species,
conditions)` gate. Seam: `plant_growth_env`, `biome_at_3d`. *Pure.*

**3c. Biome-driven flora placement** — scatter rules mapping biome + thresholds
to species density. Seam: `world/vegetation_scatter.rs`, `forest_density.rs`,
`WorldGenerator`. Depends 3a/3b. *Pure (placement), bin (instancing later).*

**3d. Cave biomes — classifier + volume field** — cave_biomes **§2** (biome
specs) + **§6** (macro structure → volume field → transition zones → surface
property influence). Today only a single `is_cave` carve exists; no cave
*biomes*. Seam: `cpu_generator::is_cave`/`biome_at_3d`, `noise`,
`batch_cave_tunnel_radius`. *Pure.*

**3e. Underground property + depth-to-biome** — cave_biomes **§4** (property
modification) + **§6.2** (depth→biome). Maps depth/surface-biome to the shared
H/R/W/P/L/S/B field deltas underground (ties to `affinity_access`). Depends 3d. *Pure.*

**3f. Cave survival (gas / heat / pressure / darkness)** — cave_biomes **§5**
(oxygen & gas chemistry, temperature, torch). Pure tick model over local cave
properties. Depends 3d/3e. *Pure.*

**3g. River archetypes + carve** — river_systems **§2** (7 types) + **§3**
(specifications) + **§10** (water-draw / wave-surf). An archetype classifier over
`accumulate_flow` + slope/width, producing carve depth/width. Seam: `river_flow`,
`batch_river_width/meander`, `WorldGenerator` (carve pass). *Pure (classifier +
carve params); worldgen wiring is a follow-on.*

**3h. Texture overlay material system** — textures_overlay_system **§5**
(property→overlay; order erosion→moisture→mud→heat→frost), **§7.3** (CPU
property-to-intensity), **§7.2** (GPU blend equation). Feeds PBR. Seam:
`material_response`, `block_textures`, the chunk fragment path. *CPU mapping pure;
GPU blend = render.*

**3i. Worldgen perf — homogeneous-solid fast-path + density SoA wire-in** —
PERF_BASELINE finding 3 (deep ~98% solid is detectable, mirroring the existing
all-air path) + wiring `batch_density_noise` into the hot density loop (r62
proved bit-exact but is unmerged into the path). Seam: `cpu_generator`,
`world/batch_density_noise`. *world module + bin; device-measured.*

**3j. Storage / wire chunk codec** — implement COMPRESSION_DECISION: seed for
unmodified, lzma'd dense (or delta) for modified. Seam: chunk IO / MP wire / save.
*bin + IO (serialization, not pure); compile-on-end.*

**3k. Cave acoustics** — cave_biomes **§3** (reverb params, acoustic navigation).
Pure reverb-parameter core from cave volume; audio wiring later. Depends 3d.

**3l. Mega trees** — flora_update_genetics **§B** (rare worldgen specimens).
Deterministic placement + structure. *Pure (placement); mesh/structure gen render-side.*

## 4. Prioritized roadmap (each ≈ one worker thread)

1. **Diploid genome + dominance** (3a) — *pure*. Foundation for breeding,
   placement, combustion FR. **Unblocks 2, 3, 12.**
2. **Flora species catalogue + thresholds** (3b) — *pure*. **Unblocks 3.**
3. **Biome-driven flora placement** (3c) — *pure core*. Depends 1,2.
4. **Cave biome classifier + volume field** (3d) — *pure*. **Unblocks 5,6,9,11.**
5. **Underground property + depth-to-biome** (3e) — *pure*. Depends 4.
6. **Cave survival model** (3f) — *pure*. Depends 4,5.
7. **River archetype classifier + carve params** (3g) — *pure*. Composes
   `river_flow`. **Unblocks the worldgen river carve pass.**
8. **Texture overlay CPU property-to-intensity** (3h, §7.3) — *pure*.
   **Unblocks 9 (the shader).**
9. **Texture overlay GPU blend** (3h, §7.2) — *render / compile-on-end*. Depends 8;
   mind r68 descriptor-stability (overlay channels in the material UBO).
10. **Worldgen homogeneous-solid fast-path + density SoA wire-in** (3i) —
    *world+bin, device-measured*. Independent; biggest frame-rate lever.
11. **Storage / wire chunk codec** (3j) — *bin/IO, compile-on-end*. Independent.
12. **Cave acoustics reverb core** (3k) — *pure*. Depends 4.
13. **Mega trees worldgen** (3l) — *pure placement*; structure gen render-side.
    Depends 1.

## 5. Open questions / decisions (operator)

- **Diploid migration:** replace `TreeGenome` with a diploid allele struct, or
  keep the current continuous genome as a derived *phenotype cache* over a new
  allele genome? Affects every flora module that reads `TreeGenome`.
- **Cave biome typing:** reuse the surface `Biome` enum or a separate
  `CaveBiome`? Spec §2 lists cave-specific biomes — leaning separate enum.
- **River carve timing:** carve at worldgen time (deterministic, but inside the
  ~99.7% budget) vs a post-pass over generated columns. Determinism + the
  COMPRESSION seed-regen contract require it be a pure function of (coord, seed).
- **Overlay channel count:** how many overlay layers (§5.1 lists 5) become
  per-material UBO fields vs packed — descriptor budget question, gated by the
  r68 shader descriptor baseline.
- **Octree LOD policy:** with `OCTREE_DEPTH=5` over 32³ chunks, what LOD tiers
  does sampling expose to the mesher/streamer?
- **Storage codec home:** engine crate vs bin; lzma dep (`xz2`) size/license sign-off.
