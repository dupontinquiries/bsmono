# Solving after the purge — what is still possible, and what is not

## Representatives cannot substitute for originals in the solve

Worth trying, and it failed for a specific reason.

The solve aligns an exemplar to the component it stands in for, and read that
component from the original mesh — 512MB of which is now gone. A representative
is a point cloud, which is what the aligner consumes, so it looked like a drop-in
replacement.

```
whole cloud as one component      pre-residual IoU 0.013
6-connected voxel component split pre-residual IoU 0.002
```

The split does find pieces — 11 to 17 per model — but against **70 to 95
placements**, with a single blob holding 281 of ~350 voxels. **At grid 32 a
vehicle's parts touch and fuse.** The representative preserves silhouette at
0.955 and destroys part separation, and part separation is precisely what the
solve needs.

So the rotation gap for the 973 purged models is **permanent**, exactly as
flagged before the purge. The code stays (`representative_components`) because
the reasoning is worth keeping, not because it works here.

## What the surviving examples give

The 21 examples still have originals, so they solve normally:

```
placements solved     1,097  (all with rotation)
pre-residual IoU      0.466
end-to-end parity     0.694   across 5 models x 3 views
```

**0.694 from committed artefacts alone**, against 0.419 before this. That is the
number that was missing, and it is now reproducible from the repo with no
external inputs: `parts_web_full.bin.gz` + `solved_examples.json` + the 21
example meshes.

## The rule this establishes for intake

**Solve before purge.** A model's rotations must be computed while its original
mesh exists, because nothing downstream can recover them afterwards — not the
representative, not the parts library, not the web. The intake order is:

1. convert → harvest → build web
2. **solve placements, and commit the rotations**
3. generate the representative
4. only then drop the original

Steps 2 and 4 were done in the wrong order for this corpus. The cost is bounded
— the models are recoverable in shape and 21 are exact — but it is a cost, and
the sequence above is the thing to carry into the next import.
