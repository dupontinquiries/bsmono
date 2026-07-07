# Reference-map extraction toolchain

Turns a Minecraft (Java 1.18+ Anvil) reference build into a categorical map blueprint the
engine can stamp. This is the "reverse-engineer good maps" pipeline: real human-designed
spatial layouts become playable arena bones.

Pipeline:
1. `mcblocks.py`  — Anvil region reader: chunk decompress, NBT parse (nbtlib), heightmap +
   per-column surface block decode, material classification (building/street/water/nature/sand).
2. `extract2.py`  — two-pass extract over a world window: heights + surface blocks, then
   classify each column against a median-filtered local ground level. Renders a clean class PNG.
3. downsample    — crop to the dense built core (building bbox), majority-vote each game tile
   (weighting building/street so thin walls and roads survive), export a char grid.

Char grid legend (src/blueprints/*.txt):  `.` open  `#` building  `+` street  `~` water
`^` nature  `:` sand

Deps: `pip install nbtlib numpy pillow scipy`
