# The part kit: four worlds from the `parts-comp` drop

Eight scanned models arrived as "map parts" — a Persian garden, a garden cloister,
the Bou Inania madrassa in Fez, an Alcazar fountain, a bamboo garden, Rodin's
*Burghers of Calais*, a Czech pylon monument, and a kit of African rural woodwork.
They became a reusable **part kit** (twelve builders) and **four new worlds** built
from it.

## What actually transferred

The scans are ~2-unit normalized shells whose textures are photogrammetry albedo
with shadow baked in. Geometry can't be lifted (it's a shell, not a set of
primitives) and the texels render as mud under this engine's lighting. What *does*
transfer is **proportion** and **mass distribution**, so every builder cites the
measurement it honours:

| model | measurement | what it dictated |
|---|---|---|
| Persian garden | 29k verts; mass spike at 20–30% height, second lobe at 60–80% | low parterre beds, all vertical interest in cypress + iwan |
| Garden cloister | aspect 0.31; 40% of mass in the 80–90% band | thin columns carrying a deep entablature |
| Madrassa | aspect 0.99; twin peaks at 20% and 70–80% | plain lower wall, dense ornate upper storey |
| Alcazar fountain | aspect 0.15; everything in bands 1–3 | wide, low, tiered basins |
| Bamboo garden | near-uniform histogram floor to crown | vertical culms, empty floor |
| Burghers (Rodin) | peak at mid-height, thin at both ends | standing figures, no plinth mass |
| Pylon monument | peaks at 40% and 70–80% | shaft, then a heavy capital ("the hat") |
| African woodwork | half the mass in the lowest band | ground-scattered props |

Palettes are median-cut from the drop's own textures and then **lifted**: the
Alcazar basin samples `4A4A4A` stone with `4F5B6A` water, the thatch
`361C0F..603D26`. Hue relationships are kept, values raised into the range the
other worlds live in. Said plainly here so nobody mistakes them for sampled colour.

## The kit

`horseshoe_arch` (two-centre Moorish arch with optional alfiz) · `muqarnas_corbel`
· `zellij_floor` · `water_rill` (the chahar bagh channel, with kerbs and jets) ·
`cypress` · `bamboo_culm` (segmented, swaying) · `cloister_bay` · `fountain_basin`
· `wellhead` · `bronze_figures` · `stone_pylon` · `log_stack` / `plank_fence` /
`thatch_shelter`.

All twelve are ordinary scene builders — any existing world can call them.

## The worlds

**Persian Garden** — a chahar bagh: fourfold walled garden, water rills meeting at
a central tank, cypress avenues, mud-brick wall with an iwan. **Garden Cloister** —
arcaded quadrangle round a garth, wellhead, herb beds, ivy, and the Burghers
standing in the grass as the cloister's memorial group. **Madrassa Court** — zellij
sahn, ablution basin, horseshoe arches under a carved cedar gallery, green
pyramidal roofs, and a ring of student cells. **Bamboo Grove** — raked gravel
clearing walled by culms, stepping stones, tsukubai basin, stone lantern.

## The two contracts every new world has to meet

**EnvParams variation.** A world must be generative, not a diorama: `seed` picks
colourways, placements and path directions; `nodes` drives bed/cell/culm counts;
`branches` subdivides (cross-rills, screened bays, figure count); `tree_size` /
`tree_height` / `tree_girth` shape the planting; `prop_density` scales scatter.
`part_kit_scenes_vary_with_the_conversation` asserts two seeds build two different
worlds and that density reaches the geometry.

**The expanding graph.** The graph grows as the conversation does, and a fixed
world becomes a diorama the data has outgrown, so
`graph_aware_scenes_expand_with_conversation_size` requires more geometry at 220
nodes than at 10. Two real bugs surfaced here:

* The Madrassa initially *shrank* (1880 → 1530 meshes) because its bay count was
  multiplied by `render_quality()`, which falls as the graph grows — LOD was
  demolishing the architecture. Quality now thins per-bay **ornament** only; bay
  count follows the court, and a node-scaled ring of student cells makes the
  growth unmistakable.
* `part_kit_scenes_keep_the_graph_footprint_clear` (new) caught the Persian
  fountain and the cloister wellhead sitting exactly where the graph floats. Both
  moved off the crossing — which is also the better composition and, for a chahar
  bagh's secondary basins and a cloister puteal, the more authentic plan.

Cost, measured by `part_kit_scene_report` (12 nodes → 220):

```
Persian Garden    1132 →  2013 meshes    55k →  105k tris
Garden Cloister    579 →   973 meshes    21k →   38k tris
Madrassa Court    2395 →  2704 meshes    54k →   64k tris
Bamboo Grove      2033 →  4941 meshes   116k →  226k tris
Mansion Garden    2841 →  3119 meshes   362k →  444k tris   (existing, for scale)
```

The grove's first cut hit 11.9k meshes / 512k tris at 220 nodes — four times the
busiest existing world — because each culm is ~20 meshes of segments, collars and
leaf sprays. Its density curve was tempered to a gentler slope with a ceiling,
which is also truer to a managed grove: it fills in, it does not become impassable.
