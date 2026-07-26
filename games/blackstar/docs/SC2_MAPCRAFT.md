# StarCraft 2 map-craft principles -> applied to this game
## Research pass (Blizzard "Best Practices in Art and Performance", Envato SC2
## Level Design, Liquipedia, TerranCraft) mapped to our arena maps.

Sourced from Blizzard's official best-practices tutorial, the Envato Tuts+
SC2 aesthetic-design series, Liquipedia's map/editor pages, and TerranCraft's
design-philosophy essay. The transferable principles (engine-agnostic):

## The principles (and how they translate here)

1. **Theme first; every choice flows from it.** (Envato: "determine your
   theme early... it acts as a guide.") We already do this per-map via
   docs/ART_BIBLE.md + the map fiction. Keep it.

2. **Doodad spacing over density.** (Blizzard: half as many trees, more space,
   same look, better perf; avoid overlap + overdraw.) -> ground_cluster()
   enforces spacing (rejects props within 2u of each other). Never carpet a
   margin with overlapping props.

3. **Keep gameplay areas UNOBSTRUCTED; detail lives in the margins.**
   (Blizzard: "keep areas of gameplay unobstructed, leave doodads to
   non-playable areas.") -> our detail helpers are all collide=False and the
   SC2 pass places clusters in far CORNERS, never the fighting lanes. Verified
   by the collider score staying byte-identical before/after.

4. **Lead the eye.** (Blizzard Painting&Effects: trails + doodads tell players
   where to go; a map without this is "a confusing mess.") -> eye_trail():
   a line of lit floor markers from a hub toward a key area (pluto: dais->each
   gate; beach: pier->sea).

5. **Ground your props.** (Blizzard: paint texture UNDER doodads so they don't
   float on mismatched terrain.) -> ground_cluster() lays a painted ground
   patch under every cluster so props read as planted.

6. **Limited, harmonized palette; readability first.** (Blizzard: too many
   colors distract; lighting's top priority is unit readability.) Our maps
   already theme-tint per-map (apply_theme_lighting). Caution against adding
   clashing emit colors in the play space.

7. **Value-contrast elevation tiers.** (Blizzard: cliff textures with varied
   color VALUES so players read high vs low ground.) Opportunity: our vertical
   maps could tint higher tiers lighter/darker for legibility. Not yet done.

## Applied this pass (Update 223)
- pluto: eye-trails dais->4 gates; grounded tholin/abyssrock debris clusters
  in the 4 corners.
- beach: eye-trail down the pier to the drained sea; grounded sand/rust
  beach-wrack clusters inland.
- Both verified collision-neutral (pluto 85.1 A / 47 boxes, beach 80.2 A / 99
  boxes -- identical before/after). map_score --check: all 8 maps pass floor.

## Applied Update 224 (the logged follow-ons)
- **height_bands()** helper: the value-contrast-elevation principle #7, done
  via MATERIAL (mesh has no per-vertex colour) -- rings a structure with lit
  bands stepping dim->bright with height so elevation reads as brightness.
- vault: eye-trails from all 4 edges -> the central relic plinth; value-bands
  up the archive towers (spec_rock->riftcrystal->white); grounded shard
  clusters in bare margins.
- core: value-bands up the flow towers (obsidian->rust->molten, "heat rises");
  grounded slag clusters in 3 corners. (rivers already lead the eye.)
- chicago: eye-trails to both river-bridge crossings; grounded reconstruction
  rubble in the far-south margins.
- All collision-neutral (vault 85.2/86, core 84.0/76, chicago 82.4/64 --
  identical before/after); all 8 maps still pass floor; 56/56 ctest green.

## Next opportunities (from the research, not yet done)
- A "readability audit": ensure no decorative emit color competes with the
  weapon/enemy palette in the fighting space (principle 6). NOT yet done.
- Extend eye_trail/ground_cluster/height_bands to the remaining space + ice
  maps (kuiper, glaciem, europa, io, titan, etc.) where routes are ambiguous
  or margins are bare. Partially done (7 maps dressed: pluto, beach, vault,
  core, chicago + earlier trim passes); the rest remain.
