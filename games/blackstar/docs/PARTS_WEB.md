# The parts web — 1,000 catalog parts, annotated, 2.77MB

Asked whether adding the 1,000 catalog parts to the web with annotations would
be quick and low-storage. **Yes, on both counts**, and the numbers are better
than the question assumed.

## What was built

`tools/build_web.py` packs the catalog meshes and the harvester's annotations
into one container, `catalog/parts_web.bin.gz`:

```
parts        : 1000
unique geom  : 893   (107 exact duplicates folded)
annotated    : 1000 / 1000
index        : 352,178 bytes    payload: 3,826,968 bytes
web file     : 2,771,035 bytes
```

**2.77MB against 17MB of .obj on disk**, and against 4.67MB for the same .obj
gzipped. Three things get it there:

1. **Quantised positions** — 16-bit per axis in each part's own bounding box.
   Round-tripped and measured: max vertex error **0.0076% of the part's
   extent**. A part is a wheel, not a landscape; float32 is three times the
   size for precision nothing can see.
2. **Content dedup** — 107 parts turned out to be byte-identical geometry under
   different names. Names all survive, because names are how placements refer.
3. **One container, one compression pass** — concatenated parts share a
   dictionary. These are all vehicles; vehicle parts repeat.

Index and payload are separate inside the container so a tool can read the
annotations — usually all anyone wants — without touching the geometry.

## The annotations are the point

Each part carries what it is (`name`: barrel, roadwheel, tread_link), where it
came from (`from`: car-1968_alfa_romeo_33_stradale), how often it recurred
(`instances`: 11), and how well its cluster fit (`residual_mean`,
`residual_max`). That last pair decides whether a part can be replaced by its
cluster representative or needs its own copy — the difference between "looks
right" and "looks like the wrong wheel".

## What this says about the 1,012 models

Measured from the two existing catalogs:

| catalog | source models | stored meshes | placements | instances | reuse |
|---|---:|---:|---:|---:|---:|
| vehiclecat | 169 | 500 | 178 | 26,680 | **53×** |
| greeblecat | 179 | 500 | 188 | 88,618 | **177×** |

**169 models resolved to 500 stored meshes.** That is the whole thesis
confirmed on real data: geometry is shared across vehicles, and what differs
between two cars is mostly which parts, placed where — metadata, as you said.

Extrapolating at the same rate, 1,012 models want roughly **3,000 stored
meshes**, which at this web's density is around **8MB** — against **512MB** of
baked `.iqm`. Even allowing that later models add proportionally more novel
geometry than the first 169, the order of magnitude is not in doubt.

## Will it look as good?

The evidence says yes for the parts themselves — 0.0076% quantisation error is
far below the voxel probe's 0.68% at its finest grid, and far below anything a
renderer shows. **The risk is not the parts, it is the residuals**: a part
placed by a rigid transform is exact only if the cluster genuinely fit it, and
`residual_max` is already recorded per part precisely so the ones that do not
fit can keep their own geometry rather than being silently approximated.

## Order of work

1. Convert the 748 undecomposed models with `iqm_to_obj.py` (exists, verified).
2. Harvest them into the part library — the long compute step.
3. Rebuild the web across all catalogs.
4. **Then** the baked `.iqm` become redundant and can go, which is what makes
   dropping 512MB safe rather than lossy.

Voxels come after that, per part, not instead: the probe's own finding was that
parts-first is the right order, because voxelising before deduplication
voxelises the same wheel six times.
