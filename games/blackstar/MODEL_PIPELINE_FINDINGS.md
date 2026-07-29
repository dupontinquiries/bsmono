# Findings: cybertron, instancing, and what the pipeline still needs

## 1. The cybertron import is a strict subset of blackstar — do not merge it

`cybertron-cmake-project` and `games/blackstar` are **the same engine** at
different points, and blackstar is the one that is ahead:

| | blackstar | cybertron |
|---|---|---|
| `game.c` | 12,190 lines | 12,077 |
| `drift.h` | present | absent |
| IQM files | 2,133 | 1,683 |
| unique model names | 1,938 | 1,488 |
| **names only in cybertron** | — | **0** |

Every model in the import already exists in blackstar. Merging it would add
**402 MB and no content**, so `import/cybertron` should be kept as an archived
snapshot or dropped, not integrated. The `MODEL_ATLAS.json` analysis on that
branch still applies — it describes blackstar's corpus, because it *is*
blackstar's corpus.

## 2. Instancing is real and large

`scorponoktrnasformershumanlmb_part1`: **295 connected components collapse to 70
unique shapes**. 289 of them are copies — **69% of that mesh's vertices are
redundant** under rotation and translation. The repeat counts read like the
creature: ×38 and ×18 of a 17-vertex piece, then ×6 groups at 71/72/96 verts on
a thing with paired limbs.

A rigid transform is **lossless**, so storing unique geometry plus a 4×4 per
instance rebuilds the mesh *exactly*, not approximately. This is the opposite of
the part-kit pipeline warned about earlier: that one re-expresses authored detail
procedurally and loses it; this one preserves every vertex and only removes
repetition.

**Caveat, stated because it matters:** `scorponoktrnasfolmb_part2` reports zero
components — 15,336 verts across 5,112 triangles means the mesh is *unwelded*,
every triangle its own island, so connectivity tells you nothing there. That is
**not measured**, not "no repeats". Unwelded meshes need spatial clustering
instead, and a good chunk of this corpus is likely unwelded.

## 3. The format gap for the stated goal

`game-model-tools/rust/obj-iqm` writes `joints: 0` — it emits geometry only. For
"a skeleton that generates from an upscaled vehicle representation", the
converter needs three things it does not have:

1. **Joints and poses** in the IQM output (the header slots are already written
   as zero, so the shape of the change is clear).
2. **Volume/collision components carried alongside mesh components**, which is
   the custom-format question: IQM has no volume concept, so it needs either a
   convention (a mesh named `vol_*` treated as a hull) or an extension chunk.
   A convention is far cheaper and survives every existing IQM tool.
3. **Instance transforms**, so a deduplicated component can be placed N times.

## 4. The verifier is the thing to build first

Conversion cannot be trusted by eye at this scale — 2,133 files. A mathematical
check is cheap and total:

* **Vertex-set equality** under the instance transforms: reconstruct, then match
  every vertex to the original within float epsilon. A rigid transform is
  lossless, so anything above epsilon is a bug, not a tolerance.
* **Triangle count and winding** preserved (a mirrored transform silently flips
  normals, and `det(R) = -1` catches it before a renderer does).
* **Bounding volume and centroid** unchanged, as a fast pre-check before the
  expensive per-vertex match.
* **Topology**: component count and per-component vertex counts must survive.

That gives a pass/fail per model over the whole corpus, which is what makes
bringing 2,133 models to a uniform level of detail tractable — you can tell,
per model, whether it made the trip.
