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

---

# Round four — occupancy residual built, and a 2× scale bug found

## What was built

`occupancy_residual()` replaces the displacement field with two sparse sets —
cells the component fills that the exemplar does not (`add`), and the reverse
(`rem`) — packed one `u16` index per changed cell. Cheaper than the three signed
bytes it replaces, and acting on the cells that were actually wrong.

The renderer reconstructs **volume**: pose the exemplar, voxelise, apply the
sets, emit a box per occupied cell. That is the point — a displaced surface
cannot express "there is material here the exemplar lacks", and a box grid can.

## Two bugs found by measuring, not by reading

**The residual carried no frame.** The solver sized its grid over posed exemplar
*and* component; the renderer derived one from the exemplar alone. Different
boxes, so every cell index referred to somewhere else and the correction was
noise. `lo`/`span` now travel with the residual. Fixing it moved IoU 0.337 →
0.391.

**`size` is not an extent.** Measured on real placements: `size` 0.554 where the
component's max-axis extent is 0.346 — roughly a diagonal against a per-axis
measure, about 2× apart. Scaling by `size / extent` inflated every exemplar to
double, which is why nothing overlapped its component. Scaling exemplar extent
onto component extent instead cut the changed cells by a third, 28,385 →
19,087.

## Where it stands, honestly

```
pre-residual IoU (posed exemplar vs component) : 0.045
occupancy cells                                : 19,087  (~37 KB, 138 placements)
render IoU, occupancy path                     : 0.349
render IoU, mesh path (no residual)            : 0.535
```

**The occupancy path is still worse than just placing the exemplar meshes.** And
the number that explains it is `pre-residual IoU 0.045`: the posed exemplar
overlaps its component almost not at all, so the residual is not correcting a
near-match — it is storing the component nearly outright, and doing it through a
16³ grid that is coarser than the mesh it replaced.

**So the blocker is not the residual. It is the pose.** Until a posed exemplar
lands on its component, the parts library is not being reused at reconstruction
time at all, and every mechanism built on top of it measures the wrong thing.
The remaining suspects, in order of how much they would explain:

1. **Rotation.** PCA alignment scored by extent agreement is weak for parts
   whose principal axes are near-degenerate, which machine parts often are.
   Scoring by occupancy overlap directly — now cheap, since the voxel grid is
   already there — would pick the sign combination that actually fits.
2. **Placement-to-component matching.** Nearest-centroid lets several placements
   claim one component, and this model has 484 components against 61
   placements.
3. **Exemplar choice.** The first part with a matching signature wins; the
   cluster may contain a closer member.

Each is testable against `pre-residual IoU` without rendering anything, which
makes the next round cheap.

---

# Round five — solved. The component splitter was the bug.

```
model                          placements  front   side    top
imported_donors_bot_yukon_brawler      62  0.962  0.981  0.973
imported_raidboss_tremor               61  0.656  0.646  0.684
imported_soldierrib_part0               6  0.282  0.419  0.424
imported_warforcybertronautobotsoldlmb  2  0.105  0.096  0.117
```

**A model whose parts the harvest actually covered reconstructs at 0.96–0.98
silhouette IoU** — from a 232KB web plus 7.5KB of occupancy residual, against a
mesh it replaces. `yukon_brawler` was **0.014** two rounds ago.

## What was wrong

I wrote a second component splitter. `transform_collage`'s weld at 0.002 of the
model span; the harvester welds at an **absolute 1e-5**. Different epsilons,
different pieces — so the components being posed were not the components the
placements referred to, and every alignment was matching a part against a blob
that had merged several of them.

Measured across five models, best-of-24-rotation alignment:

| splitter | grid IoU |
|---|---|
| mine (relative eps) | 0.008 – 0.644, mean 0.237 |
| **the harvester's own** | **0.810 – 0.995, mean 0.920** |

Everything downstream had been measuring that mistake. The pose looked broken,
the residual looked useless, the occupancy path looked worse than the mesh path.
None of those were real results — and each one sent a round of work at the wrong
target.

Knock-on effects once fixed: alignment residual **median 0.344 → 0.000**, and
occupancy cells **19,087 → 3,842**, an 80% cut, because a correct pose has far
less to correct.

## What still limits the mean

The remaining low scorers have **6, 7 and 2 placements**. The harvest found only
a handful of parts in them, so most of those models are not represented at any
pose — that is harvest coverage, not reconstruction. `min_tris` and the
clustering tolerance are the knobs, and they are a different investigation from
this one.

The 24-frame rotation search (all proper axis permutations and signs, scored by
occupancy overlap rather than extent) is worth folding into the solver: it beat
the 4-sign version 0.222 vs 0.170 on the old split, and has not been re-measured
on the correct one.

## Standing lesson

This is the seventh time in this project that a second copy of an existing
function was the bug. The rule has been "grep before writing"; the sharper
version is **share the function, do not re-derive its behaviour** — two
implementations of "split a mesh into pieces" cannot disagree if there is only
one.

---

# Round six — the recipe works. min_tris was throwing away most of the model.

## The measurement that found it

Coverage tracked render IoU almost exactly, so I measured what the harvester's
default threshold discards:

| model | components | tris | ≥120 tris | covered | render IoU |
|---|---:|---:|---:|---:|---:|
| yukon_brawler | 300 | 32,000 | 62 | **73.4%** | 0.96–0.98 |
| raidboss_tremor | 563 | 32,000 | 61 | 43.0% | 0.65–0.68 |
| soldierrib | 118 | 4,527 | 6 | **29.7%** | 0.28–0.42 |

`--min-tris 120` was dropping **70% of some models' geometry** before clustering
ever saw it. Not a reconstruction failure — those parts were never in the
library.

## The result

| min_tris | part types | placements | web | residual | **mean IoU** |
|---:|---:|---:|---:|---:|---:|
| 120 | 112 | 138 | 226 KB | 8 KB | 0.500 |
| **20** | **502** | **747** | **371 KB** | **23 KB** | **0.844** |
| 6 | 629 | 1,076 | 386 KB | — | 0.852 |

**0.500 → 0.844 for 63% more bytes.** And 6 buys almost nothing over 20, so
**20 is the knee**.

Per model at min_tris 20:

```
yukon_brawler   0.973  0.988  0.987
raidboss_tremor 0.933  0.954  0.929
soldierrib      0.735  0.918  0.869
```

## Projected to the corpus

The 748-model harvest at 120 produced 4,950 part types and a 5.35MB web. Scaling
by the measured 112 → 502 factor: **~22,000 types, ~24MB**, against **512MB** of
baked `.iqm` — **4.7%**, at a fidelity that is 0.93–0.99 on models the harvest
covers well.

That is a projection, not a measurement, and the earlier 8MB projection from the
head of the corpus turned out optimistic when the tail was measured. Treat 24MB
as the estimate to test, not the answer.

## The working recipe

1. `iqm_to_obj.py` — the corpus is only readable at all through this.
2. `part_harvest.py --min-tris 20 --emit-per-category <large>` — the default
   threshold and the exemplar cap were each costing more than they saved.
3. `solve_placements.py --modifiers --grid 16` — poses by principal axes,
   scales exemplar extent onto component extent, and stores an occupancy delta
   with its own grid frame.
4. **Components from the harvester's own splitter, never a second one.**
5. `render_parity.py` — the only check that catches the mistakes the geometric
   metrics missed, three times running.

---

# The full corpus run — measured, not projected

Ran the settled recipe over every model.

```
converted     : 1,011 of 1,012 (one stem collision)
harvest       : 40,701 part types, 85,659 placements, 993 models placed
repeated      : 11,999,929 triangles across placements
                 7,927,868 stored once each   ->  34% repeated
web           : 54,660,477 bytes (40,701 parts, all annotated)
residual      : ~8 KB per 40 placements at grid 16
render parity : 0.826 mean silhouette IoU across 4 models x 3 views
```

Per model: 0.735/0.918/0.869, 0.783/0.878/0.802, 0.721/0.918/0.863,
0.716/0.854/0.860. **The recipe holds at 200× the sample it was tuned on** —
0.844 on five models, 0.826 on the full library.

## Against the projection

I projected ~22,000 types and ~24MB. Measured: **40,701 types and 54.7MB.**
Off by 1.8× and 2.3× — the corpus has more distinct geometry than the five-model
sample implied, which is the same direction the earlier 8MB projection missed in.

**Two projections, both optimistic, both from small samples.** The lesson is
specific rather than general: part-count extrapolation from a handful of models
understates, because the models sampled first are the ones that decompose
cleanly, and each new model adds more novel geometry than the average of those
before it.

## What it means for the 500MB budget

```
web + residuals   :  ~55 MB
baked .iqm        :  512 MB
ratio             :  10.7%   (9.4x smaller)
```

Not the 4.7% projected, but replacing 512MB with 55MB takes the bundle from
635MB to roughly **180MB** — comfortably inside 500MB, with room that the
projection did not depend on.

## What is still true and unfinished

* **0.826 is not 1.0.** It is good enough to browse, stream, overlay and animate
  from; it is not yet a pixel-identical replacement, and the remaining gap is
  mostly parts the harvest still misses at `min_tris 20`.
* **Only 993 of 1,011 models got placements.** The other 18 produced no
  clusterable components at all and would need their geometry kept.
* **Textures and materials are untouched.** Every number here is geometry.
* The 24-frame rotation search measured better than the 4-sign one (0.222 vs
  0.170) on the old component split and has never been re-measured on the
  correct one. It is the cheapest remaining improvement.

---

# Round seven — 0.906, and the web becomes buildable

## Accuracy: the 24-frame search, finally measured on a correct split

The rotation search had beaten the 4-sign version on the *broken* component
split (0.222 vs 0.170) and was never re-measured after that split was fixed.
Folded in, with one further change: **scored by occupancy overlap rather than
extent agreement**, which is cheap now that the grid exists and which extent
cannot do — extent cannot tell a part from its 180° flip.

```
                        pre-residual IoU   residual cells   mean silhouette IoU
4-sign, extent-scored              0.075           11,659                 0.844
24-frame, overlap-scored           0.208            8,818                 0.906
```

Per model: 0.970/0.983/0.988 · 0.929/0.954/0.924 · 0.878/**1.000**/0.888 ·
0.736/0.926/0.877. `soldlmb` was **0.10** three rounds ago and now reaches 1.000
on one view.

Why 24 and not 4: the old version fixed axis ORDER from the eigenvalue sort and
tried only sign flips. Machine parts routinely have near-degenerate eigenvalues
— a wheel's two in-plane axes, a plate's two long ones — so the sort order is
arbitrary between two copies of the same shape, and **the correct rotation was
often not among the four tried**.

## Usefulness: slots, variants and a practical latent space

`tools/web_slots.py`. The web reconstructs well, which makes it an archive; it
did not answer the question a builder asks — *what else could go here?*

A **slot is a category plus a size band**, not a category alone. `roadwheel` has
33 members spanning an order of magnitude; offering them in one list gives a
vehicle one absurd wheel. Bands are geometric because parts scale
multiplicatively.

```
slots : 22   (18 with a real choice)
block/band2       169 variants   0.202 - 0.594
block/band1        81 variants   0.071 - 0.198
disc/band2         46 variants   0.202 - 0.632
```

**Swapping works because the frame belongs to the slot.** Every placement now
carries `rot` and `scale`, so a swap keeps the placement's orientation and
refits the variant's extent — the part fills a pose it did not define.
`swap_plan` drops the occupancy residual deliberately: it was measured against
the part that used to be there, and carrying it over would apply one part's
corrections to another's geometry.

**The latent space is built from harvest evidence, not a learned embedding.**
Each part already has `sig`, `tris`, `size`, `instances` and `residual_mean`;
the feature vector is `(log size, tri density, log instances, residual)` and
neighbours are restricted to the same category — distance alone will report that
a small disc resembles a small block, which is numerically true and useless.
