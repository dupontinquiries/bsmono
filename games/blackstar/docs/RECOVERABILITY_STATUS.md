# Are the characters fully recoverable from what is committed? No — three gaps

Asked whether the legacy models can go. Checked against the **committed tree**
rather than against the best number ever measured, and that distinction is the
whole answer.

## Gap 1 — the committed placements have no poses

```
models with placements : 994
placements total       : 86,406
   with rotation       : 0
   with occupancy residual : 0
```

`partlib_full.json` is the raw harvest: `sig`, `centroid`, `size`. That is
exactly the state that measured **0.535** silhouette IoU before the pose work.
Every `solve_placements` run in this project wrote to `/tmp`; the output was
never committed for the full corpus.

## Gap 2 — the solver could not read the committed web (fixed here)

`solve_placements` read a directory of loose `.obj`. `catalog/importedcat/meshes`
holds **790 exemplars against 40,742 part types**, so a solve against the
committed tree found an exemplar for one placement in fifty:

```
--meshes catalog/importedcat/meshes    pre-residual IoU 0.012
--web    catalog/parts_web_full.bin.gz pre-residual IoU 0.700
```

**0.012 was a missing-file result, not a fidelity result** — the kind of number
that looks like the method failing when it is the plumbing. `--web` now reads
exemplars straight out of the committed BSWEB, which makes the pipeline
self-contained: it runs on what is in the repo rather than on what happened to
be in `/tmp`.

## Gap 3 — end-to-end parity from committed artefacts is 0.419

Solving and rendering using **only** committed files:

```
warforcybertronautobotrb_part0   22 placements   0.467 0.437 0.424
warforcybertronzetaprirb_part2   22 placements   0.419 0.378 0.391
mean 0.419
```

Against 0.906 measured from a fresh `--min-tris 20` harvest on a five-model
sample. The committed harvest gives these models 22 placements where a fresh one
gives 70+, so the committed library is thinner than the one the good numbers
came from.

## What "ready" would require

1. **Run the solve over all 994 models and commit it** — rotations, scales and
   occupancy residuals. Now possible from committed inputs thanks to `--web`.
2. **Re-harvest and commit at the settings the good numbers used**, since the
   committed library is not the one that measured 0.906.
3. **Re-measure parity end-to-end on tens of models** from committed artefacts
   only. That is the number that licenses deletion, and it does not exist yet.

The pieces all work. What is missing is that **the best-measured pipeline and
the committed pipeline are not the same pipeline** — and only the committed one
can rebuild anything after the originals are gone.
