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
