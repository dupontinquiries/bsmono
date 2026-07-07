# Speculative: cave generation via moisture-driven erosion

## What problem this is solving

User-flagged (paraphrased):

> Could you add more cave generation to the world by creating
> some sort of physics random moisture erosion... something where
> water/moisture can seep through the ground. I think we need
> more soil erosion principles to modify world height. The
> question of how to simulate weathering on geological timescales
> but quickly compute it.

Three sub-asks rolled together:

1. **More caves** — the current generator is sparse-and-uniform;
   real worlds have karst chambers, river-cut valleys, fault
   networks, bedrock seepage tunnels.
2. **Moisture-driven erosion** — water flows through the world
   over time; that flow erodes; the erosion sculpts terrain (caves
   below, valleys above).
3. **Geological-timescale fast compute** — geological processes
   take 10⁵-10⁸ years; the player wants the result at world-load
   time, deterministically, without an infinite pre-pass.

The third constraint is the interesting one. Real-time hydraulic
erosion sim is well-studied (Sebastian Lague's video, Inigo
Quilez, ...) but assumes a global heightmap and many iterations.
Unendlich's world is procedurally infinite — there is no "global
heightmap." Whatever erosion we simulate has to be **per-chunk
deterministic** AND **agree with neighbors at boundaries**.

## What already exists

### Current cave generation

`world::world_model.rs:417`:
```rust
pub fn is_cave(&self, x: f64, y: f64, z: f64) -> bool {
    if y < 4.0 { return false; }  // no caves in bedrock
    let cave = noise_3d(x*0.05, y*0.05, z*0.05, seed.wrapping_add(seeds::CAVE));
    cave > 0.78
}
```

Single 3D noise threshold. Produces small isolated blob caves;
no chambers, no rivers, no structural narrative. Threshold 0.78
means ~5% of below-surface volume is cave. Tunable per biome via
`Biome::cave_threshold()`.

### Adjacent infrastructure that's relevant

- **`world::weathering.rs`** — already does per-block weathering
  (block-aging that produces bumpy/eroded look). Per-block
  weathering value 0-255. The pattern of "scalar value that
  accumulates per environmental driver" is exactly what
  geological erosion needs at a coarser resolution.

- **`world::fields::moisture`** — runtime-derived per-chunk
  moisture field. Sampled from biome humidity + water/lava
  proximity, 8³ samples per chunk. Used by plant lifecycle.
  **Missing pieces for geological erosion**: this is a
  short-timescale field (responds to current water blocks), not
  a long-timescale "how much water has flowed through here in
  10⁵ years."

- **`world::world_model::WorldModel`** — the deterministic
  generator. All `(x, y, z)` queries are pure functions of
  `(coord, seed)`. Anything that's a function of these can be
  computed lazily, on demand, in any order.

- **`world::cpu_generator::WorldGenerator`** — wraps WorldModel
  for per-chunk generation. The `is_cave` check is per-cell
  here.

- **`region_grid::RegionGrid<T>`** — a caching abstraction for
  "lazy values keyed by region coord, deterministically derived
  from `(region_coord, seed)`." Already used for biomes. This
  is the data structure for "per-region geological state."

## The deterministic-erosion question

The fundamental design constraint:

> For any chunk at coord C with seed S, the eroded terrain at C
> must be a deterministic function of `(C, S)` AND agree with
> the eroded terrain at every neighbor of C.

Three approaches that satisfy this constraint:

### Approach A: Noise statistically shaped like erosion

Design noise functions whose output **looks like** the result of
erosion without actually simulating it. Real eroded landscapes
have known statistical signatures:

- **Concave slopes** near valley bottoms (erosion thins downhill)
- **Convex hilltops** (no erosion accumulator at summit)
- **Steep cliff faces** where strong rock outcrops
- **Karst sinkholes** at low-elevation high-moisture points
- **Hexagonal jointing** in basalt (Worley noise!)

The standard "fBM + ridged + domain warp" terrain pipeline
already produces a passable approximation. Adding `concavity
bias = -sign(gradient) * (1 - height_normalized)^k` produces
concave valley floors and convex peaks.

**Cost**: cheap. Just more noise.
**Determinism**: trivial. Pure function of position.
**Cross-chunk**: trivial. No state.
**Fidelity**: low-medium. Doesn't simulate; just mimics.
**Cave coupling**: weak. Caves are still independent noise.

**Pros**: ship this in a weekend. Already 80% of how AAA
procedural terrain works.
**Cons**: no causal narrative ("this valley was cut by THIS
river over THIS many years"). No actual moisture flow.

### Approach B: Coarse global watershed + fine local erosion

At world generation, compute a **drainage basin map** at a coarse
scale (e.g., 256-block regions). This is deterministic from
`(region_coord, seed)`:

```rust
// At region scale (256×256 blocks per region):
fn compute_drainage(region: RegionCoord, seed: u64) -> DrainageInfo {
    let base_elevation = continental_noise(region);
    let rainfall = climate_rainfall(region);
    // Greedy descent: which neighbor region does this region drain to?
    let neighbors = region.cardinal_neighbors();
    let drains_to = neighbors.iter().min_by_key(|n| continental_noise(*n));
    DrainageInfo {
        accumulated_flow: parents.iter().map(|p| compute_drainage(p).accumulated_flow).sum() + rainfall,
        flow_direction: drains_to,
    }
}
```

This uses `RegionGrid` (already in the codebase). Per-region
drainage is cached deterministically.

At chunk generation, the chunk knows which region it's in and
samples that region's accumulated flow. High-flow regions get:
- More karst caves (chambers along the flow path)
- Wider river valleys carved into the surface
- More sedimentary deposit (sand/gravel near valley bottoms)

**Cost**: medium. Region-scale drainage computation is per-region
not per-chunk; cheap to amortize. The recursion (drainage from
parents) terminates at watershed boundaries (where flow goes
elsewhere or to the sea).
**Determinism**: yes, via RegionGrid caching.
**Cross-chunk**: yes — chunks in the same region share the
region's drainage value.
**Fidelity**: medium-high. The "rivers come from drainage" relationship is causally correct.
**Cave coupling**: strong. Caves are explicitly placed along
high-flow paths.

**Pros**: causally satisfying ("this is where the river is
because that's where water drains"). Reuses existing
`RegionGrid` infrastructure. Watershed boundaries are naturally
sharp, which matches reality.
**Cons**: harder to implement. The recursive drainage needs a
terminator (don't recurse infinitely up the chain). Sea-bound
flow is the natural one.

### Approach C: Per-chunk local erosion iterations

At chunk generation, run N iterations of hydraulic erosion
locally on the chunk's heightmap, using neighbors' (also
procedurally generated) heightmaps as boundary conditions.
Because the neighbor heightmaps are deterministic, the boundary
conditions are deterministic, so the erosion result is
deterministic.

```rust
fn erode_chunk(coord: ChunkCoord, seed: u64, iterations: usize) -> HeightMap {
    let mut h = base_heightmap(coord, seed);
    let neighbor_heights = [
        base_heightmap(coord + (1,0,0), seed),
        base_heightmap(coord - (1,0,0), seed),
        // ... 4 cardinal neighbors ...
    ];
    for _ in 0..iterations {
        // Drop water particles; let them flow; deposit/erode.
        // Boundary check: when a particle exits the chunk, look up
        // the neighbor's height to decide if it continues or turns.
        // Each step is deterministic given the starting state.
    }
    h
}
```

**Cost**: high per-chunk. Erosion sim is expensive — for
~100 particles × 50 steps × 7 chunks (own + 6 neighbors) =
35,000 height lookups per chunk. With proper noise sampling
(coarse, cached) maybe 1-5ms per chunk.
**Determinism**: yes if particles are seeded deterministically.
**Cross-chunk**: tricky. A particle that crosses chunk C → C'
boundary must produce the same erosion footprint on C' as if
C' had been generated first. This requires either:
  - Generating each chunk WITH its 6 neighbors (3x work per chunk)
  - A two-pass scheme: pass 1 deposits "incoming particles" at
    chunk boundaries; pass 2 receives and processes them
**Fidelity**: highest. Actual erosion simulation.
**Cave coupling**: emergent. If we extend the sim to underground
flow (Darcy's law for porous media), caves emerge from prolonged
seepage paths. Beautiful but expensive.

**Pros**: physically real. The most game-feel-correct.
**Cons**: most expensive. Cross-chunk boundary correctness is
the implementation pain.

### Approach D: GPU iterative erosion (deferred to GPU-resident era)

Once the world is GPU-resident (architecture-v1 destination,
post-compressed-chunk-representation), erosion can run as a
compute shader on batches of chunks. The GPU's parallelism makes
the per-chunk cost negligible. Deterministic because the input
state is deterministic and the kernel is deterministic.

**Cost**: O(1) per chunk if batched right.
**Determinism**: yes.
**Cross-chunk**: handle by uploading neighbor chunks alongside
the target chunk; pad the kernel with boundary cells.
**Fidelity**: highest, and fast.

**This is the destination.** Approach C is its CPU sibling;
Approach D is what it becomes once the GPU pipeline lands.

## Recommended path

### Step 1 (smallest, ship-now-able)

**Multi-scale cave generation** (Approach A applied to caves).

Replace the single-threshold cave function with a multi-scale
model:

```rust
pub fn cave_density(&self, x: f64, y: f64, z: f64) -> f64 {
    let s = self.seed;
    // Worm tunnels — current behavior, mid frequency.
    let worm = noise_3d(x*0.05, y*0.05, z*0.05, s + CAVE_WORM);
    // Large chambers — low frequency, sparser.
    let chamber = noise_3d(x*0.012, y*0.012, z*0.012, s + CAVE_CHAMBER);
    // Bedrock seepage — high frequency, only at depth.
    let depth_factor = ((4.0 - y) / 60.0).clamp(0.0, 1.0);
    let seepage = noise_3d(x*0.15, y*0.15, z*0.15, s + CAVE_SEEP) * depth_factor;
    // Combined.
    (worm * 0.6 + chamber * 0.3 + seepage * 0.1)
}

pub fn is_cave(&self, x: f64, y: f64, z: f64) -> bool {
    if y < 4.0 { return false; }
    self.cave_density(x, y, z) > biome.cave_threshold()
}
```

Tunable knobs: weights, thresholds, depth_factor curve. Could
be env-driven for experimentation.

**Effort**: ~50 LoC in `world_model.rs`. Zero architectural risk.
**Win**: visually richer caves (chambers + tunnels + seepage
veins) without changing the cost model.

### Step 2 (medium-ship)

**Region-scale drainage map** (Approach B's groundwork).

Add `world::drainage` module. Per-region (256-block scale)
computes:
- Mean elevation
- Annual rainfall (from biome × seed)
- Drains-to direction
- Accumulated flow (from upstream regions)

Wire into the cave generator: caves are 2-3× more likely along
high-flow paths.

**Effort**: ~300 LoC. New module, depends on `RegionGrid`.
**Win**: rivers visible above ground, cave systems aligned with
drainage geometry. Caves now have a causal narrative.

### Step 3 (large-ship, ties in with compressed-chunk-rep arc)

**Geological-moisture field** (parallels existing runtime
moisture but at long timescale).

A new `Field`: `geological_moisture`. Per-chunk derivation:
- Sample drainage flow from Step 2
- Sample biome rainfall
- Integrate over "geological time" (just a constant for now —
  say, 10⁵ years)
- Saturate at 1.0; values 0.0-1.0

Where geological_moisture is high AND bedrock is the right type
(limestone-equivalent in our block palette = `Stone`), boost the
cave density. This is karst-style cave formation.

The "fields" registry already supports adding new fields. The
field is deterministic; physics queries against it work like
querying any other field.

**Effort**: ~150 LoC. Adds field; modifies cave generator to
sample it.
**Win**: caves spatially correlated with surface water features.
Players see a river, follow it underground, find a karst cave
system. Real geological narrative.

### Step 4 (long-term, parallel to GPU-resident-world)

**Heightmap erosion** (Approaches C/D depending on infrastructure).

Modify the surface height itself based on the drainage map. River
valleys are carved lower; ridges are raised relatively. Implements
the "modifies world height" the user requested.

On CPU: per-chunk local erosion using neighbor heightmaps
(Approach C). Expensive but parallel-friendly across workers.

On GPU: batched erosion kernel (Approach D). Cheap.

**Effort**: 500-1000 LoC depending on approach. This is genuinely
big.
**Win**: terrain has actual geomorphology. Valleys, plateaus,
plains all emerge from physics rather than just noise.

## Specific block-types we'd need

The current block palette has: Stone, Dirt, Grass, Sand, Water,
Lava, plus wood/leaves variants and decorative items.

For richer erosion-driven caves, useful additions:
- **Limestone** (dissolves with moisture; becomes caves over time)
- **Sandstone** (sediment from erosion; appears in valley fills)
- **Gravel** (rivere-bed deposits)
- **Clay** (impermeable; can be modeled as moisture barrier)

These don't NEED to be new blocks if Stone/Dirt/Sand do double
duty. But block-type-aware erosion (limestone caves vs granite
ridges) is genre-classic.

## Why this matters beyond aesthetics

Caves with physics-relevant structure are good for gameplay (mob
spawning, exploration, resource distribution). But the architectural
significance is:

1. **Deterministic erosion is a TEST CASE for the procedural-
   reconstitution architecture.** If we can compute drainage,
   moisture, and erosion deterministically per region, we've
   proven the architecture handles "world-level computations
   that need cross-chunk coordination." That same machinery
   handles future global phenomena: tectonics, climate change,
   river migration, etc.

2. **Compressed-chunk-representation gets stronger.** A
   procedurally-eroded chunk is still 100% reconstitutable from
   `(coord, seed)` — the erosion is part of the deterministic
   generation. So `UploadRepr::Procedural` covers more chunks.

3. **Per-chunk fields generalize.** The "geological moisture"
   field is a recipe applicable to any long-timescale phenomenon:
   geological_temperature (cooling lava → granite), geological_pressure
   (deep rock → metamorphic), etc.

## Open questions

1. **How much is "more caves"?** Currently ~5% of below-surface
   volume is cave. Players probably want 15-25% in cave biomes,
   5-10% in solid bedrock biomes. Step 1 alone (multi-scale)
   could get us to ~15%.

2. **Where do rivers/lakes go on the surface?** Approach B's
   drainage map naturally tells us. We'd add water blocks at
   each "flow-accumulation > threshold" location. That's a
   separate ship from cave generation but shares the drainage
   infrastructure.

3. **Should we re-mesh existing worlds after this change?** A
   change to the generator produces different chunks. Existing
   save data (in `explicit_indices` form) would be applied on
   top of the new procedural baseline, which means SOME edits
   would land in mid-air or mid-rock. Probably OK to flag as
   a known migration cost.

4. **Speed of step 1**: how expensive is 3 noise samples per
   cell instead of 1? Cave check is currently per-cell (32³ =
   32768 per chunk). 3× the noise calls = ~3× the cave-check
   time. Likely still well under 1ms per chunk; profile to
   confirm.

5. **Geological time as a constant**: "10⁵ years" is a fudge
   factor. The deeper question is "does the player perceive
   geological time?" Probably not — they perceive the EFFECT
   of geological time. So time is just a knob for tuning
   feature scale.

## Cross-references

- `crates/unendlich-engine/src/world/world_model.rs:417` —
  current `is_cave()` function (Step 1's target).
- `crates/unendlich-engine/src/world/world_model.rs:RegionGrid`
  — the caching abstraction for per-region geological state.
- `crates/unendlich-engine/src/world/fields.rs:330` — the
  existing moisture field (template for `geological_moisture`).
- `crates/unendlich-engine/src/world/weathering.rs` — existing
  per-block weathering (block-level analog of geological erosion).
- `crates/unendlich-engine/src/world/biomes.rs:71` — per-biome
  cave_threshold (already exists, used by `is_cave`).
- `docs/architecture-v1.md` — "GPU-resident world plus procedural
  reconstitution" destination; erosion is a clean test-case for
  this architecture.
- `docs/speculative/compressed-chunk-representation.md` —
  compressed-chunk representation memo. Deterministic erosion
  STRENGTHENS the `UploadRepr::Procedural` story (more chunks
  are pure-procedural even with rich geology).

## Recommendation

Ship step 1 (multi-scale cave generation) as a small RC in a
session with hardware. Pure noise change, ~50 LoC in
`world_model.rs`, zero architectural impact. Visually richer
caves immediately.

Step 2 (drainage map) is a meaningful next-session ship — ~300
LoC, introduces `world::drainage` module, sets up the
infrastructure for everything else.

Steps 3-4 are dependent on step 2 and tie into the broader
compressed-chunk + GPU-resident-world arcs. Pursue when those
arcs are also moving.
