# World Structure Generation — Design

**Worker 03 (WORLD) · Round 15.** How `unendlich` places hand-feeling structures
into procedurally generated terrain, built on the pure cores already shipped.
Everything here is deterministic from `(world_seed, region)`; no global state.

## Taxonomy

Structures span a size/rarity spectrum, each a different mix of the same
pipeline stages:

- **Ruins** — small scattered footprints (broken walls, partial floors). Common,
  surface, biome-flavored. Built from a `structure_template` footprint with a
  rubble pass.
- **Towers** — tall vertical footprints; rarer; prefer flat, elevated sites.
- **Villages** — clusters of room-footprints joined by paths; a placement graph
  rather than a single footprint.
- **Dungeons** — underground room+corridor complexes from `dungeon_layout`,
  carved into stone, often hung off a cave node.
- **Monoliths** — single rare landmarks (one per large region), deliberately
  visible from distance; the rarest gate.

## Pipeline: grammar → template → placement → carve

1. **Grammar** (`structure_grammar`, R8). An L-system expands a per-type axiom
   into a command string, and the turtle walk yields a set of `Placement`s
   (lanes, wings, corridor spines). Villages and dungeon spines use this; a
   single ruin may skip straight to a template.
2. **Template** (`structure_template`, R4). Each grammar cell resolves to a
   footprint of block codes (walls/floor/door). `make_ruin`-style generators
   produce the actual cell grid, seeded so a given site is reproducible.
3. **Placement** (`structure_place`, R2 + `dungeon_layout`, R10). Scores
   candidate sites by terrain suitability (flatness under the *whole* footprint,
   not one cell) and keeps the best that fit in bounds. `dungeon_layout`
   supplies the room set + corridor MST for the underground types.
4. **Carve** (`cave_carve`, R11). Dungeons and monolith vaults carve negative
   space (capsule tunnels between rooms, chambers) before the templates stamp
   solid geometry, so corridors actually connect.

## Biome & region gating

Spawn eligibility is a per-region permille gate hashed from `(region, seed)`
(the `giant_cave` R6 pattern): each structure type has a rarity and an allowed
biome set. Biome membership uses `biome_transition` (R12) weights so a type that
belongs to "forest" can still appear in a forest↔plains blend zone with reduced
probability. Region gating keeps density bounded (e.g. ≤ 1 monolith per N×N
region) without a global pass.

## Collision with terrain & caves

- **Terrain:** placement scoring rejects sites whose footprint flatness is below
  threshold; the stamp clamps to the surface so a building doesn't float or bury.
- **Caves:** before stamping, the footprint volume is tested against the cave
  fields (`giant_cave` / `cave_network` + `cave_carve`). Surface structures
  reject sites undermined by a cavern; dungeons instead *attach* to a cave node.
- **Structures vs structures:** village rooms use the `dungeon_layout`
  non-overlap rule; cross-type overlap is resolved by the rarer type winning
  (deterministic tiebreak by type id, then hash).

## Determinism

Every stage is a pure function of `(world_seed, region, type)`. No structure
reads mutable world state, so generation is order-independent and identical on
every client — the same wall on the Fairphone and the M2 Ultra. Carving and
stamping are authoritative voxel writes (sim side); the cosmetic FX (R14) layer
on top without affecting placement.

## Open questions

- **Cross-region structures.** Villages/dungeons that straddle a region boundary
  need a deterministic owner region; current gate is per-region.
- **Loot/spawner placement.** Where do interactive contents hook in — a fourth
  pipeline stage, or a template annotation?
- **Edit interaction.** If a player carves into a generated dungeon, does the
  octree re-merge (R7) cleanly reclaim the freed nodes? Needs an audit pass.
- **Tuning surface.** Rarities, biome sets, and footprint sizes are placeholders;
  they want a data-driven table rather than constants in each core.
