# Should the Rust engine gain normal maps? No — and the reasons are structural

Asked while adding two-channel normal decode to the C engine. Checked before
answering, and the evidence is one-sided.

## 1. The assets have no textures at all

Every shipped bot's glTF, sampled across three:

```
analog        39 materials   normalTexture 0   images 0
beetlejuice   39 materials   normalTexture 0   images 0
blackstar     34 materials   normalTexture 0   images 0
```

Not "no normal maps" — **no images whatsoever**. Each material is a
`baseColorFactor` plus `metallicFactor 0.4` and `roughnessFactor 0.6`, the same
two constants on all 39: exporter defaults, not measured values.

`model.rs` says so in its own header: the loader reads scene.gltf and scene.bin
directly "so the unused normal/roughness maps we didn't ship are never touched".
This was a decision, not an oversight.

## 2. Adding them would need a generator that is not in the tree

Re-exporting with textures means running `tools/export-roster.py`, which shells
out to `blackstar/rust/foldview` — and that directory holds assetkit,
contentlint and enginecore with **no foldview**. Same blocker recorded in
`maintenance/LOSSLESS_CLEANUP_AUDIT.md` when these assets looked deletable.

## 3. The renderer does not sample normal maps, by design

`shader.wgsl` is a ray-marched SDF renderer. Normals come from the geometry —
shapes arrive through a storage buffer, terrain through a baked heightmap, and
the per-shape material is a single `e.x` spec scalar. Tangent-space normal
mapping is not a feature this pipeline has, and bolting it on is a renderer
change rather than an asset change.

## 4. It pushes the wrong way on the number we are protecting

The scaling work just measured that 100× the character space lands near 730MB at
today's per-part cost, and that the lever is *reducing* bytes per part. Adding a
normal map per material to 48 bots adds hundreds of megabytes to the side of the
ledger we are trying to shrink.

## What to do instead, at zero storage cost

The material properties normal maps would have implied are already derivable:

* `metallicFactor` and `roughnessFactor` are **uniform 0.4/0.6 across every
  material** — they are already in the file, already loaded, and carry no
  information. Setting them per-material from the base colour's own statistics
  costs nothing and differentiates 39 materials that currently shade identically.
* The C game's `materials.json` (107 materials in 15KB, from `texture_web.py`)
  is the template for what those numbers should look like — reflectivity,
  variation, emissive coverage — though it cannot transfer directly, since the
  two games' assets have different lineages.

That is the version of "add normals" worth doing: add what the normals would have
told the renderer, without the pixels.
