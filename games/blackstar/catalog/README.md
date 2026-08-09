# Harvested part catalog

Produced by `tools/part_harvest.py` over the model library.

    greeblecat/   min-tris 50   10,375 part types, 88,618 placements, 500 exemplar meshes
    vehiclecat/   min-tris 200   5,873 part types, 26,680 placements, 500 exemplar meshes
    tanklib20/    tanks only     736 types  (index only)
    vehiclelib20/ 178 vehicles  5,873 types (index only)
    botslib20/    81 bots       1,108 types (index only)

`partlib.json` holds, per part type: `name` (wheel/tread_link/strut/panel/barrel/
disc/block/greeble), `tris`, `instances`, `residual_mean` (how much family members
vary), and `mesh` (the exemplar filename, when emitted). `placements` lists every
occurrence per model. `rules` gives the 25-75% relative position each category was
observed at -- learned placement logic, not hand-authored.

Exemplar meshes carry box-projection UVs, so they can take a texture.

Install any subset as browsable models:

    python3 tools/install_models.py --iqm-dir catalog/greeblecat/meshes \
        --engine engine_src --limit 60
    cmake --build build -j4

24 of these are already installed in this build (indices 48-71), named by
category and signature, e.g. "WHEEL FFCEA5".

## The meshes are .obj; .iqm is built, not stored

`catalog/*/meshes` holds the part geometry as **.obj only**. The matching .iqm
used to sit beside each one and no longer does, for three reasons that were each
checked rather than assumed:

* **The engine never reads the catalog.** `game.c` loads `data/*.iqm`; nothing
  in `engine_src` mentions `catalog` at all. These were never runtime assets.
* **The tools that touch the catalog read .obj.** `bot_generate.py` and
  `detail_apply.py` only ever *write* .iqm, into `data/`, never read the
  catalog's.
* **They regenerate exactly.** `tools/obj_to_iqm.py` rebuilt all 1,000 of them
  earlier in this work, verified byte-count-identical on a sample and carrying
  the real `INTERQUAKEMODEL\0` header.

To rebuild them:

```
for f in catalog/*/meshes/*.obj; do
  python3 tools/obj_to_iqm.py "$f" --out "${f%.obj}.iqm"
done
```

Note the contrast with `engine_src/data/*.iqm`, which are NOT removable: the
engine loads those directly, and their .obj sources are not in the tree.
