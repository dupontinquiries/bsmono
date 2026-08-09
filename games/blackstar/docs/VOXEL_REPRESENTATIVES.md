# Representative lossy models — what SVOX contributed

## The problem it solves

The new-model test made the gap concrete. An Optimus Prime from a different
asset lineage shared **19 of 157 part types** with the 40,742-part library, and
the best geometric match for its unmatched parts was **0.32 IoU** — checked
directly, so this is genuine novelty rather than a signature being too strict.

**Parts do not transfer across lineages.** A library that only stores parts
cannot keep a model it has never seen anything like, which is exactly the model
someone will add next.

## What SVOX contributed

One idea, and it is the one we were missing. `.vox` always writes the full
256-colour palette even when a model uses eight, so **a trimmed per-model
palette is most of the saving on small assets**. Our own voxel probe measured
geometry only — and a grey voxel blob is not a "still looks good" model.

Its layout is not reusable: `voxelAmount` is a single byte, capping a file at
255 voxels, where these models want tens of thousands. So `tools/voxel_rep.py`
takes the idea and widens the fields — u16 coordinates, u32 counts.

## Measured on the new model

```
              size (gz)   % of 10.4 MB .obj   shape IoU vs 96³   colour err (median/p95)
grid 32          9,196          0.09%              0.955             2.0 / 15.4
grid 48         25,032          0.24%              0.955             3.6 / 11.7
```

**Grid 32 keeps 0.955 shape agreement at 0.09% of the source**, with colour
error of 2 units in a space where the maximum is 441 — invisible.

Two findings worth keeping:

* **Palette size is nearly free.** 8 entries costs 23,016 bytes and 64 costs
  26,085. Geometry dominates, so trim the grid and be generous with colour —
  the opposite of where I would have spent the budget by intuition.
* A grid-64 measurement showed IoU 0.099 and is **an artefact, not a result**:
  64 does not divide the 96³ reference, so the downsample compared mismatched
  indices. Only grids dividing the reference are comparable.

## Where this fits

Three tiers, and the corpus needs all three:

| tier | cost | when |
|---|---|---|
| exact part reconstruction | ~36 KB/model | the model's parts are in the library |
| **coloured voxel representative** | **~9–25 KB/model** | a new lineage the library cannot express |
| original mesh | ~500 KB–11 MB | validators and the few keepers |

At 9KB, keeping a representative of **every** model costs about 9MB for 1,012 —
which makes "we cannot rebuild this one exactly" stop being a reason to keep
half a gigabyte.
