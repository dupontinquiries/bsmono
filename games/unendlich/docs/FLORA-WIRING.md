# Wiring cave flora into worldgen — build-loop-ready plan

Status: **designed, not wired.** The placement touches the hot chunk-gen path, so
it is intentionally left for a session with the build-and-screenshot loop. This
doc captures every integration point + decision so execution is 1–2 iterations.

## What exists (compiles, unit-tested, unwired)
- `world/flora_sites.rs::find_sites(min, dims, world_seed, max_y, occupied) -> Vec<FloraSite>`
  — picks rooted sites in a world box, kept under `max_y` (underground).
- `world/cave_flora.rs::CaveFlora` — a grown plant: `stem: StemGraph` + `lumen`.
  `CaveFlora::grow(root, dir, seed, steps, moisture, temperature, occupied, exposure)`
  and `CaveFlora::cells() -> Vec<Cell>` ("the stem blocks to place").
- `world/chunk_flora.rs::ChunkFlora::populate(coord, world_seed, max_y, occupied,
  moisture, temperature) -> ChunkFlora { coord, plants: Vec<CaveFlora> }`, plus
  `tick(daylight)`, `pearl_count()`, `peak_emission()`.
- `world/plant_lifecycle::lifecycle_handler()` — already registered in the bin's
  `TickRegistry` (`bin/unendlich_world.rs:306`). Has a `Stage` enum.

All of this is **BlockId-agnostic** by design: the modules produce `Cell` sets;
the caller binds cells → block IDs.

## Decisions to lock before wiring
1. **Block binding.** No dedicated stem/glow block exists. v1: reuse `BlockId::Vine`
   for the stem cells (it is already a non-solid plant block — see `blocks.rs`
   plant predicates). A dedicated `GlowStem`/`Pearl` emissive block is a follow-up
   (touches the block enum + face/light predicates + the shader's emissive path;
   do it deliberately, appending variants so discriminants don't shift).
2. **Where it appears.** Cave flora is UNDERGROUND + bioluminescent — it will NOT
   show on the surface (that's the GPU tree code in `chunk_generation.comp`). Test
   by flying into a cave, not by looking at the surface.
3. **Scope of the first cut.** CPU generator path only (the GPU-worldgen path would
   need the same logic ported into a compute shader — separate brick).

## Integration points
1. **Closures** (built from the just-filled chunk + the `WorldModel`):
   - `occupied: Fn(Cell)->bool` — read the chunk's filled voxel array for in-box
     cells; for out-of-box cells fall back to `model.density(...) > 0 || !is_air`.
     Simplest v1: in-box lookup, treat out-of-box as empty (see cross-chunk below).
   - `moisture: Fn(Cell)->f32` → `model.humidity(x, y, z, 0.0)`.
   - `temperature: Fn(Cell)->f32` → `model.temperature(x, y, z, 0.0)`.
   - `max_y` → `sea_level_at(x,z) as i32 - a few` (keep it underground).
2. **Call site.** In the chunk voxel-fill routine (the loop that writes `BlockId`
   per cell using `pick_surface_block`/`pick_subsurface_block`/`is_cave`), AFTER
   the terrain+cave pass, BEFORE the chunk is handed to the mesher:
   ```
   if std::env::var("UNENDLICH_CAVE_FLORA").is_ok() {
       let flora = ChunkFlora::populate(coord, seed as u64, max_y, &occ, &moist, &temp);
       for plant in &flora.plants {
           for cell in plant.cells() {
               if in_chunk_box(cell) { set_block(cell, BlockId::Vine); }
           }
       }
       flora_store.insert(coord, flora); // for the lifecycle tick
   }
   ```
3. **Remesh.** Writing happens before the mesh is built, so no explicit remesh is
   needed at gen time. Lifecycle-driven changes (growth/relight) DO need a remesh:
   mark the chunk dirty when `tick` mutates visible cells (defer to v2 — v1 places
   static plants and only animates light, which is emissive-only, no remesh).
4. **Lifecycle storage.** Keep a `HashMap<ChunkCoord, ChunkFlora>` next to the
   chunk store; the registered `plant_lifecycle` handler ticks it with daylight.
   v1 may skip storage (placement only) and add storage when lighting lands.

## Cross-chunk growth (the one real hazard)
A plant rooted near a chunk edge can grow cells OUTSIDE `[origin, origin+32)`.
v1: **clamp** — only write in-box cells (above). This truncates edge plants but is
safe and deterministic. v2: defer out-of-box cells to a per-neighbor "pending
writes" queue applied when the neighbor generates (needs the chunk store to accept
deferred edits — non-trivial, its own brick).

## Build-loop step plan
1. Add the gated placement at the fill site with `BlockId::Vine`, clamp to box,
   no storage. Build with `UNENDLICH_CAVE_FLORA=1`, fly into a cave, confirm vines.
2. Add `flora_store` + confirm the `plant_lifecycle` tick runs (log pearl_count).
3. Emissive lighting (pearls) → the froxel/light path; dedicated GlowStem block.
4. Cross-chunk deferred writes. Then port to the GPU-worldgen compute path.

## Risks
- Closure type mismatches (`Cell` field access, `Fn` bounds) — blind-compile risk,
  hence deferred to the build loop.
- `max_y`/site density tuning (too many plants = noise; too few = invisible).
- Reusing `Vine` means flora looks like vines until the emissive block exists.
