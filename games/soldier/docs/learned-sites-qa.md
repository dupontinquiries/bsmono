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
Naranco Sanctuary           66  true   0.45     90     73    371    40    43
Haunted Parish              66  true   0.46    214     48    999    40    26
Colossus Fountain           59  true   0.57     50     75    432    29    43
San Pellegrino Piazza       59  true   0.60     48    181    746    12    97
Castillo de Pioz            73  true   0.37      5     44    149     0    27
Barcience Keep              59  true   0.56     11      0     13     5     0
Amman Cistern               64  true   0.53     30    217    204     3   125
Morton Castle               59  true   0.50      7      0     27     8     0
Pambre Bastion              71  true   0.32     14     23    169     3    12
Perarrua Crag               56  true   0.37      3      0      5     3     0
Lost Palace                 55  true   0.50    124      0    536    40     0
Thian Hock Keng             66  true   0.39      9      0    135     0     0
Sky Palace Corridor         65  true   0.45     42    386    980     5   263
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

Still open from this pass, deliberately: '^' expansion-ring scatter (outside cells are bare
floor), destructible columns/merlons, and slope-driven roof cap meshes.
