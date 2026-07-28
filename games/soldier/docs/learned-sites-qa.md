# Learned sites — QA report (0726 scan set)

The 13 learned maps come from IQM photogrammetry/model scans (`mapstudy` → height-tiered
blueprints in `src/blueprints/learned/`). Beyond the original massing/courtyard/gate learning,
the blueprints now carry two DEEPER structural learnings, each with its own detector and its own
stamp, locked by `learned_sites_stamp_their_learning`:

**Columns ('c')** — the hypostyle rhythm. Two detectors: small isolated massing piers whose whole
boundary is open floor (capped at 40/map so the Haunted Parish debris field doesn't become a
bamboo forest), and — for solid scans — the *hollow-hall* pass: cells whose height histogram has
a populated floor band, a populated roof band, and an empty WALK band are roofed-over-open; a
large connected hollow region opens as interior floor and the mid-full shafts inside it are its
columns. Columns stamp as slender full-height cover and register in `cover_obstacles`, so the
evaluators see what the architecture is doing: holding space open instead of closing it.

**Ramparts ('r')** — the castles' defensive circulation. The courtyard-facing edge of massing
(any tier — ruins scan low, temples tall; the *courtyard adjacency* is the selective part)
becomes a wall-walk: a standable platform at wall-top, a parapet lip on its outward sides, and
stepped stairs (8 risers ≤0.6 over two tiles, under the engine's STEP=0.65) down into the bailey
at up to two floor-adjacent ends per run.

Two support passes keep regenerated scans playable: a **restoration pass** carves a ≤4-cell
doorway (with lintel) for any roomed space the rasterization sealed — in the building it learns
from, that hall HAS a door — and near-open scans (Morton's razed footprint) get quadrant room
anchors over the floor bbox so waves populate.

## Current table (`cargo run --example learned_report`)

```
site                     grade  conn struct  choke  plats  cover  cols ramps
Naranco Sanctuary           76  true   0.45     90     40    357    40    43
Haunted Parish              74  true   0.46    214     12    999    40    26
Colossus Fountain           68  true   0.57     51     19    389    29    43
San Pellegrino Piazza       68  true   0.61     49     46    679    12    97
Castillo de Pioz            81  true   0.37      5     32    120     0    27
Barcience Keep              71  true   0.56     11      0     13     5     0
Amman Cistern               81  true   0.53     30    101    126     3   125
Morton Castle               71  true   0.50      7      0     27     8     0
Pambre Bastion              81  true   0.32     14      4    163     3    12
Perarrua Crag               68  true   0.37      3      0      5     3     0
Lost Palace                 68  true   0.50    124      0    536    40     0
Thian Hock Keng             75  true   0.39      9      0    135     0     0
Sky Palace Corridor         74  true   0.45     42    322    847     5   263
```

Notes, honest ones: **Thian Hock Keng** yields 0/0 on the new learnings — its scan reads as a
walled temple court without a detected hypostyle band; its courts still carry the map.
**Barcience / Morton / Perarrua / Lost Palace** have no ramparts because their courtyard rings
didn't survive scanning (ruins and shells); their columns and courts do the work instead. The
three lowest structure scores (Pambre 0.32, Pioz/Perarrua 0.37) are dense keeps — the
archetype-fair scorer expects that band for interiors.

## Appearance learning (textures + normals)

The scans' TEXCOORDs were zeroed by conversion (verified exhaustively), so exact UV sampling is
impossible — but the uploaded texture archive still carries what each building LOOKS like. Two
new sidecar layer families, both invisible to older parsers:

* `;;H` / `;;S` / `;;R` (mapstudy, from vertex **normals**): fine per-cell height 0-F (real
  silhouettes — Pioz's keep reads `d`/`e` against its curtain), roof-pitch class 0-3 from
  top-band normal tilt, and weathering 0-F as 1 − mean-resultant-length of face normals
  (dressed stone calm, rubble scattered).
* `;;P` / `;;C` (tools/texlearn.py, from the model's **own texture set**): a median-cut palette
  (binding by archive naming, confidence-tagged in each header — MORTON/PAMBRE/PERARRUA/
  BARCIENCE/pellegrino/parish high; the 26-tile sandstone UDIM scan → Amman Cistern; the warm
  ornate cluster → Thian; the grayscale metals → SciFi; Pioz gets a sibling-castle union;
  church/lost-palace/colossus honestly keep the skin palette) and a per-cell tint index driven
  by height band + roughness jitter, roofs taking the palette's most hue-distinct entry.

`Level.learned_facade` consumes both in the wall pass: per-cell wall TOP from `;;H`
(silhouettes instead of one slab — dado, pilasters, trim and sconces all scale with it), wall
color from the learned palette with dither strength scaled by `;;R`. The lock-in test asserts
the facade parses on every sidecar map, heights are non-uniform, and bound palettes tint cells.

Shipped since: **real photograph tiles** — `Material::LearnedWall(site)` slots in a 7x7 atlas,
painted from 160px crops of each model's own texture set (`src/assets/learned/*.rgb`, ~1 MB,
attribution commented); **`build_learned_walls`**, the interior open-air wall renderer (interior
Wall cells previously emitted NO geometry — the curtains were invisible collision; now every
face gets two photo bands at the scan's `;;H` height, palette-tinted with `;;R`-scaled dither, a
cap, and `;;S` ridge boxes). Ownership is single: the indoor wall loop stands down on learned
maps, and the pass runs from `build_cliffs` as a choke point — the theme-method guard that
silently skipped the Military-skinned castles is gone. **Learned fog** pulls the fog colour
toward the palette's light tone and sits closer (aerial perspective at scale). The downhill
GROUND_SNAP fix (46 airborne flickers -> 0 on the ramp test) makes walking the sites smooth.

**Model-first pass** (field report: "brick stairs everywhere… the terrains and buildings and
textures were way better"): the scans' `;;G` GROUND layer now shapes `Level.heights` — Perarrua
sits on its crag, Morton on its mound, the ring relaxing outward — and grades jumped for it
(Pioz 73→81, Naranco 66→76, Piazza 59→68: the evaluator agrees the model's terrain beats the
flat slab). The photograph carries wall colour now (near-white verts with a 22% palette wash —
full tints muddied the texel), wall-walks ride the per-cell scan height instead of one slab,
stairs went from two-per-run eight-riser brick piles to ONE per real run (≥6 cells) of five
photo-stone slabs, and merlons are every-other-cell, lower, in the model's own stone.

**Crop-set + set-piece pass** (field report: one tiny crop tiled everywhere is not the scan;
the ground had no texture; the real value is DbD-style palette SETS): the atlas is 8x8 with
THREE crops per site — two high-detail wall regions from different quadrants/files, picked
between in coarse 3-cell patches so a curtain drifts across real surface regions, plus a
low-variance GROUND crop the floor mesh now wears (`learned_slot(site, which)`). And the
scans feed EVERY map now: `tools/extract_pieces.py` cuts 25 connected structures (towers,
gatehouses, halls) from the final blueprints with their full appearance;
`Level::stamp_learned_piece` drops one into any level (facade created/merged, palette and
tints travelling along); the landmark pass places a piece on ~1/3 of Military/Urban/Canopy
seeds — a photographed tower where a procedural monument used to be. Stamping can seal an
existing corridor, so the doorway-restoration pass was extracted to `restore_room_access`
and runs after every stamp (the same cure the site maps use).

Still open, deliberately: '^' expansion-ring scatter, destructible columns/merlons.
