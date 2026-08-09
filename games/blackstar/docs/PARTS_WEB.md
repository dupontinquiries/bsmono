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

---

# The 748 run — done, and the honest result

Converted all 748 undecomposed models with `iqm_to_obj.py` (747 in the first
pass; the 748th is an 11MB outlier that needed its own run — it converts fine,
308,037 verts, the batch had simply run out of time). Harvested them, emitted
exemplars per category, and rebuilt the web.

## What came out

```
harvest      : 4,950 part types, 9,320 placements, from 748 models
repeated     : 4,830,354 triangles across placements
               3,400,272 if stored once each   -> 30% is repeated geometry
web (all)    : 1,790 parts, 1,683 unique geometries, 5,350,018 bytes
placements   : 571,979 bytes gzipped
```

**Web form 5.9MB against 314MB baked — 53× smaller.**

## Where this differs from the first catalogs, and why that matters

vehiclecat showed 53× *instance* reuse; this batch shows **30% repeated
geometry**, which is much less. That is not a failure of the method — it is
selection. These 748 are precisely the models nobody had decomposed, and the
169 that were done first were the ones where reuse was obvious. The tail is
more heterogeneous, and any estimate extrapolated from the head (including the
~8MB figure earlier in this document) was optimistic. **5.9MB is the measured
answer; 8MB was the guess.**

## What is NOT done

* **Exemplars are capped at 80 per category** (`--emit-per-category 80`), so the
  web carries 790 of the 4,950 harvested part types. The tool's own help
  explains why the cap is per-category rather than global: ranking by placement
  count alone "yields almost only wheels and tread_links". Raising the cap
  trades size for coverage, and the right value is a rendering question rather
  than a compression one.
* **Nothing has been rendered yet.** Every fidelity number here is geometric —
  quantisation error, IoU, cluster residual. The claim "at least as good as the
  current models" is not yet tested where it counts, which is on screen.
* **The baked `.iqm` are still in place.** They should not be removed until a
  render comparison passes, and `residual_max` is the field that decides which
  parts need their own geometry rather than a cluster representative.

---

# Render parity — measured, and it does not pass yet

`tools/render_parity.py` rasterises the original and the web reconstruction from
three viewpoints and compares silhouette IoU and depth RMSE. Software
rasteriser, no GPU needed: a depth buffer answers both questions. Reported per
view as well as averaged, because a reconstruction can be convincing head-on and
wrong from above.

## First run: 0.183 mean IoU

```
model                                    place  miss   front   side    top
imported_soldierrib_part0                    6     4   0.019  0.013  0.046
imported_donors_bot_yukon_brawler           62    59   0.192  0.139  0.061
imported_raidboss_tremor                    61    50   0.233  0.166  0.228
mean silhouette IoU: 0.183
```

The `miss` column is the finding: **59 of 62 placements had no part in the web**.
Measured directly — placements reference 4,950 distinct signatures and the web
carries 790. **16% coverage.** The `--emit-per-category 80` cap, not
quantisation, was doing the damage.

## Second run: coverage fixed, 0.509 mean IoU

Re-harvested the same five models emitting every part type (112 of 112), rebuilt
the web, re-measured:

```
imported_raidboss_tremor                    61     0   0.720  0.700  0.708
imported_donors_bot_yukon_brawler           62     0   0.448  0.602  0.532
mean silhouette IoU: 0.509
```

Zero misses, and IoU nearly tripled. **So the cap was the dominant term** — but
0.509 is still not "indistinguishable", and the remaining gap has a known cause.

## The remaining gap is orientation

A placement records `sig`, `centroid` and `size`. **It records no rotation.** So
the reconstruction can put the right part in the right place at the right scale
and still have it turned the wrong way, which is exactly what a half-matching
silhouette looks like.

That is not a defect in the harness or the web format — it is what
`partlib.json` currently carries. And the fix already exists in the repo:
`derive_web.py` recovers rigid transforms with Kabsch, described in its own
header as *"the motion it undergoes is not something to invent — it is the rigid
transform that carries one onto the other"*. The same solve applies here; it
just has not been run for placements.

## Where that leaves the 53×

The 5.9MB figure is real but it is 5.9MB of a representation that **cannot yet
rebuild its models faithfully**. Two things must land before the baked `.iqm`
can go:

1. **Per-placement rotation**, via the existing Kabsch solve.
2. **Full part coverage** in the web, or a measured cap that keeps IoU high —
   the 80-per-category cap costs far more than it saves.

Both are known work with existing tools rather than open questions, and the
harness now measures whether they worked.

---

# Solving the rotation — two attempts, one negative result

`tools/solve_placements.py` fills in the orientation a placement never carried.
Measured on the same five models with full part coverage:

| placement rotation | mean silhouette IoU |
|---|---:|
| none (baseline) | 0.509 |
| **Kabsch on radius-ordered points** | **0.495 — worse** |
| principal-axis alignment | **0.535** |

## Why the first attempt made it worse

Kabsch needs point *pairs*, and two meshes of the same part share no vertex
order. I paired them by sorting each set by distance from its own centroid —
rotation-invariant, and not a correspondence. Two copies of a part interleave
differently under that sort, so Kabsch fitted noise and returned a confident
wrong rotation. The residuals said so before the render did: median 0.508 of
part size, which is not a fit.

This is worth keeping in the record because the fix looked obvious. `derive_web.py`
already solves rigid transforms with Kabsch, so "run the same solve for
placements" seemed like plumbing. It is not: derive_web has a correspondence
(the same component identified in both of a character's forms) and placements do
not.

**And the harvester cannot supply one.** Its `residual()` uses "the same
rotation/mirror-invariant radial profile the clustering uses" — it matches
shapes *without* ever recovering pose, by design. So no transform exists to be
copied; it has to be derived.

## Why the second worked

Principal axes avoid correspondence rather than solving it. The covariance's
eigenvectors are a property of the shape, not of any vertex pairing, so two
copies of a part yield the same frame however they are tessellated. Four
sign combinations are tried and scored by extent agreement, keeping `det = +1`
so a mirror never passes as a rotation.

## Where this leaves it

0.535 is progress and is not parity. What the numbers now say about the
remaining gap:

* **Symmetric parts are unfixable this way.** A wheel's principal axes are
  degenerate about its spin axis, so its orientation is genuinely
  underdetermined by shape alone — and for a wheel it also does not matter.
  Which parts those are is knowable from the eigenvalue spread, and worth
  recording so effort goes where it counts.
* **Component extraction is crude.** A placement's component is currently "the
  model's vertices within 0.75 × size of the centroid", which mixes in
  neighbouring parts. A real component split — the harvester already does one —
  would sharpen both the alignment and the score.
* **Per-instance residuals are the escape hatch.** `rot_residual` is written per
  placement, so a part the solve could not pose can keep its own geometry
  instead of being rebuilt wrong. That is the mechanism that makes an imperfect
  solve safe to ship.

---

# Third round — the residual has to be occupancy, not displacement

Two changes measured, and the second one is the finding.

## Real components instead of a sphere

Extraction now uses the same connected-component split the harvester uses
(`transform_collage.weld` + `components`) rather than "vertices within 0.75 ×
size of the centroid". The alignment residual improved sharply — **median 1.528
→ 0.344 of part size** — so the pieces being posed really are cleaner.

Silhouette IoU: **0.535 → 0.501.** Slightly worse. The residual and the render
disagree, which means the remaining error is not in the *fit*; matching a
placement to a component by nearest centroid is now the weak link, since several
placements can claim the same piece.

## The shape modifier, and why it did nothing

`--modifiers` stores a volumetric residual: the part's bounding box divided
8×8×8, each occupied cell carrying the mean displacement from the posed exemplar
to the component. Volumetric rather than per-vertex on purpose — a per-vertex
delta is tied to one tessellation and cannot be applied to an exemplar with a
different vertex count, which is exactly the case here. A field over space
transfers to any mesh occupying that space.

It cost 1,180 cells (~3.5KB) and moved IoU by **0.000**.

Measured why, on one model's first twenty placements:

```
cells occupied by exemplar only : 1,092
cells occupied by component only:   755
cells occupied by both          :   236
```

**A displacement field only acts where both shapes are present** — 236 cells out
of 2,083. The other 1,847 are pure occupancy disagreement: volume the exemplar
has and the component does not, or the reverse. No displacement can fix that,
because there is nothing in the cell to displace. Silhouette error *is*
occupancy error, so a displacement modifier is structurally the wrong tool for
the thing being measured.

## What the residual should be

An **occupancy delta**, not a displacement — per cell, add volume / remove
volume / leave alone. Two bits per cell against three signed bytes, so it is
*cheaper* than what was just tried, and it acts on exactly the 1,847 cells that
matter rather than the 236 that do not.

That lands on the voxel probe from the other direction. The probe found run-length
voxels at 1.1% of OBJ with IoU flat at 0.953 across resolutions, and concluded
the error was surface thickness rather than lost detail. Combine them:

* the **exemplar** carries the shape, cheaply, shared across every instance
* the **occupancy delta** carries what this instance does differently
* both are volumetric, which is what a part is — a shape that fills space, not a
  surface that approximates one

The parts library and the voxel work were being treated as alternatives. The
measurement says they are the two halves of the same representation.
