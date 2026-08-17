# Compressing the fleet, and what the new fold needs from it

Measured, not estimated. `assets/bots` is 978 MB, of which **895 MB is
`scene.bin`** — raw vertex buffers across 1,289 poses. The `.gltf` files are
44 MB and not worth touching.

## What PARTS_WEB's three techniques give here

The parts web got 1,000 catalog parts to 2.77 MB with quantisation, content
dedup and one shared compression pass. Applied to fold strips, they do NOT pay
equally:

| technique | on the parts catalog | on fold strips |
|---|---|---|
| content dedup | 107 of 1000 folded | **0 of 1289** |
| shared dictionary | helped | **no gain over per-file** |
| 16-bit quantisation | the bulk of it | **936 MB -> ~205 MB** |

Dedup finds nothing because every pose of every chassis is geometrically
distinct — that is what a fold strip IS. The shared dictionary adds nothing
because float32 vertex data is already dense; per-file gzip and one concatenated
stream both land at 49%.

Quantisation is the whole win, and it is the same win the parts web measured:
positions do not need float32 precision. Sampled 12 files: **18% of raw at
0.0008% round-trip error**.

## The constraint the new fold imposes

`xform::Morph::build` refuses poses whose prim counts or vertex counts differ,
because the fold LERPS between corresponding vertices. Vertex counts do match
across poses — checked on `quickfire`, 938,898 floats in both the first and last
pose — so the morph works.

But **the bounding boxes differ between poses of one chassis**: quickfire's
pose 0 spans [-1.00, 1.08] while the strip as a whole spans [-1.30, 1.08]. The
vehicle and the robot are not the same size, which is the point of a transform.

So quantising each pose in **its own** box would map identical world positions to
different integers in different poses, and the lerp would interpolate between
them. The fold would warp — subtly, and worst at the extremes where the boxes
differ most.

**Quantise per chassis, not per pose.** One box covering every pose in the strip
keeps the mapping identical across the fold. Measured on quickfire's five poses:
**18.8 MB -> 4.0 MB (22%)**, max error 0.000021 model units against a 2.75-unit
span. Slightly worse than per-pose (22% vs 18%) because the shared box is larger
— that is the cost of correctness and it is worth paying.

## So: does the new transform break the web?

No, provided the packer uses a per-chassis box. The requirement runs the other
way round from how the question is usually asked — it is not that the fold
breaks compression, it is that **naive compression breaks the fold**, and only
at the fold, which is exactly the case a static-model test would miss.

## Expected result

936 MB of scene.bin at 22% is ~205 MB, plus 44 MB of gltf and the rest, against
978 MB now. The bundle should come back under 300 MB — the 500 MB figure is
about right if the gltf is packed too.

## Can the engine rebuild the intermediates from the vehicle pose?

Asked directly, and the answer is **no, not as the engine stands** — but the
prize is big enough to say exactly why, because the shape of the "no" is what a
future fix has to attack.

**What the engine actually does.** `Morph::sample` reads `self.poses[k]` and
`self.poses[k+1]` and lerps between them. It interpolates BETWEEN STORED POSES;
it does not synthesise them. And `compact.rs` — the new displacement field — is
explicit that it is "layered over the existing lerp, not a replacement for it",
with a curve that is "0 at both ends of the fold". It reshapes the middle. It
does not create the middle.

**Measured, not inferred.** If the intermediates were derivable by lerping the
two endpoints, they would sit close to that line. They do not:

```
quickfire  f01 t=0.25   deviation from endpoint-lerp: 53.9% of span
           f02 t=0.50                                 61.1%
           f03 t=0.75                                 45.1%
vanquish   f01 t=0.25                                 49.0%
           f02 t=0.50                                 63.3%
           f03 t=0.75                                 46.7%
```

A vertex halfway through the fold is over half the model's own extent away from
where a straight line between vehicle and robot would put it. That is exactly
what a fold IS — panels rotate about hinges, and a rotation is not a straight
line. Discarding the intermediates would replace every fold with the slide that
`compact.rs`'s header calls "exactly the trick this project set out not to do".

**What it would save**: 785 MB of poses down to 314 MB, so 471 MB. Worth
wanting.

**What would have to exist first.** The intermediates encode per-part rotations.
Reconstructing them needs the hinge structure — which axis each prim turns
about, and through what angle — and that is what `joints.json` in each chassis
directory is for. An engine that read joints and rotated prims could store two
poses and generate the rest. That is a real feature with a clear input, and it
is not what `Morph` does today: `Morph` knows vertices, not joints.

So the honest sequence is: quantise now for ~205 MB with no behaviour change,
and treat joint-driven synthesis as its own piece of work with `joints.json` as
its input and these deviation numbers as its acceptance test.

## Not done here

The packer itself. This document is the measurement and the constraint it found;
writing `build_bots_web.py` and the loader side is the next step, and it should
carry the per-chassis-box rule as a test rather than a comment.

## The history rewrite

Quantising shrank the working tree but GREW the bundle: 904 MB, because git
still held both the old float32 blobs and the new packs. A deletion does not
reclaim anything until the old objects go.

`git filter-repo --invert-paths` stripped every `scene.bin` under a chassis that
now has a `bots.pack` — 1,289 blobs, 892 MB of history.

Two things made this safe to do rather than a gamble:

* **The strip list was generated from the packs, not from a glob.** One rule per
  pose directory of each of the 217 packed chassis, so the four chassis that
  could NOT be packed keep their `scene.bin` in history and on disk. A
  `glob:**/scene.bin` would have deleted the only copy of those four.
* **Packing was verified before deleting.** `pack_bots.py --verify` decoded all
  217 and compared against the originals while the originals still existed.

Result: `.git` 1.1 GB -> 529 MB, bundle 904 MB -> 521 MB, working tree
unchanged (`git status` clean, 217 packs and 204 exception files present),
`git fsck` silent across 3,259 commits.

**This rewrites history**, so every clone must re-fetch rather than pull. The
repo had been through this once before — filter-repo's own `already_ran` marker
was dated a week earlier — which is why it asked to confirm continuation.

## Where the remaining 521 MB is — measured by PACKED size

The first attempt at this measured RAW blob sizes and got the wrong answer:
`apps/logscope` looked like 414 MB, the largest thing in the repo, because
`gpu-gemm/src/lib.rs` is ~0.9 MB and has 388 revisions. Git deltas those to
almost nothing — **logscope is 11 MB packed**. Raw size is not a bill anyone
pays.

By packed size:

```
 209.6 MB  games/blackstar-rs/assets/bots
  91.0 MB  games/blackstar/engine_src/data
  34.8 MB  games/blackstar/catalog/parts_web_full.bin.gz
  10.5 MB  games/blackstar/catalog/importedcat
   8.4 MB  games/blackstar-rs/assets/cars
```

and inside the bots:

```
 122.4 MB   217 blobs  bots.pack     the quantised fleet, on disk now
  78.0 MB   204 blobs  scene.bin     the 4 unpackable chassis — but only 15 MB
                                     of that is current; the rest is their
                                     pre-rewrite revisions
   6.6 MB  1516 blobs  scene.gltf
```

## So what is worth doing next

**Not runtime pose synthesis.** It is the largest single idea but the packs are
already only 122 MB, and dropping to two poses per chassis saves maybe 70 MB of
that — for a feature that needs joint-driven reconstruction, has a real risk of
looking wrong mid-fold, and cannot be verified without a display. It is the
right long-term direction and the wrong next step.

**Four cheaper wins, in order of MB per unit of risk:**

1. **`parts_web_full.bin.gz`, 34.8 MB.** There is also a `parts_web.bin.gz` at
   7.7 MB. If the full one is a superset kept for provenance rather than a
   runtime input, it is 35 MB for a file nothing loads.
2. **The 63 MB of superseded `scene.bin` revisions** for the four unpackable
   chassis. Their current 15 MB must stay; their history need not. A targeted
   filter-repo on those four paths, keeping only the tip.
3. **`engine_src/data`, 91 MB** — the C engine's textures and models. The Rust
   port has its own assets; if the C engine is reference rather than something
   that still gets built, this is the single biggest item and it is not code.
4. **`assets/cars` and `assets/fennec`, 14 MB combined** — these are float32
   glTF like the bots were, and `pack_bots.py` would work on them unchanged if
   they are fold strips.

1, 2 and 4 together are ~110 MB with no behaviour change, which lands the bundle
around 410 MB. Item 3 alone would take it under 350 MB but is a judgement about
whether the C engine is still live, which is not mine to make.

## The C engine already folds — and what that tells us about joints

`game.c:545` has a SINGLE-MODEL FOLD that does exactly what runtime synthesis
would need: it rotates each `_partN` about a hinge, with the pivot from the
part's offset from its group centre and the axis from `model_analysis.h`. Its
own comment is worth keeping:

> That needs no vertex access and no shader change: separate ex_model_t's each
> take their own rotation, and the parts are already on disk.

It also eases (`t*t*(3-2t)`, because "a linear blend reads as a slideshow rather
than a mechanism") and staggers parts by slot, "which is what makes a fold look
like an explosion" if you don't.

So the C side is AHEAD of the Rust side conceptually: it folds by rotation,
where `xform::Morph` lerps vertices.

### But the fleet's joints are not yet the input it needs

Every fleet chassis ships `joints.json` with pivot, axis, angle and parent — 220
files, 88,641 joints. Measured across all of them:

```
joints with a non-zero angle:  4,732 of 88,641   (5.3%)
pyrachnid specifically:           13 of 474
```

The hinge STRUCTURE was detected — 402 distinct axes on one chassis — but the
rotation ANGLES are almost all zero. The generator found where the joints are
and not how far each one turns.

That is why the poses deviate 63% from an endpoint lerp and cannot be rebuilt:
the information that would rebuild them is exactly the 95% of angles that are
zero. `joints.json` is a skeleton without a pose.

### Consequence for storage

Runtime synthesis is blocked on the GENERATOR, not on either engine. Both
engines could consume angles today — the C one already does, for its 38
hand-split models. What is missing is a foldview pass that solves for the angle
of each joint by fitting f00 to f04, and that is upstream of this repo.

Until then the intermediate poses ARE the angle data, in the least compact
possible form, and they have to be stored.

## Can the angles be solved for locally? No — the parts are not rigid

Unpermuting the strips removed the obvious objection: part *i* is now the same
part in both poses for 215 chassis, so a per-part fit is finally meaningful.
Ran it.

A joint-driven fold assumes each part ROTATES: its shape is preserved and only
its placement changes. That is testable without solving anything — under a rigid
motion, every vertex keeps its distance from the part's centroid. Measured that
drift from f00 to f04, per part, across 12 chassis and 451 parts:

```
rigid      (<2% radius drift):   37 of 451     8%
near-rigid (<10%):               82 of 451    18%
median drift:                    38%
worst (analog):                 108%
```

**The parts deform, they do not rotate.** A door does not swing on a hinge here
— it changes shape on the way. So there is no set of joint angles that
reproduces these poses, however well the hinge tree is known, and
`joints.json`'s 95% zero angles are not an oversight in the generator so much as
an admission: there was no angle to write, because the motion is not a rotation.

This settles three things at once:

* **Runtime synthesis cannot recover the intermediates**, and not for want of
  angle data. The earlier finding that they deviate 63% from an endpoint lerp is
  the same fact seen from outside.
* **The C engine's fold is a different technique, not an earlier version of this
  one.** `game.c:545` rotates hand-split `_partN` models about derived hinges and
  says so — "38 of them, covering 442 model entries". It works because those
  models were AUTHORED as rigid parts. The fleet's 173 were harvested, and
  harvested parts deform.
* **The storage floor is the poses.** 196 MB of packs is not a step on the way
  to something smaller; it is the size of the information.

What would change this is a foldview that emits rigid parts — segmenting the
mesh so each piece moves rigidly, rather than fitting hinges to pieces that do
not. That is a different generator, not a missing field in this one.

## Cost of the history rewrite: incremental bundles from siblings stop applying

Seven new dialer bundles arrived and none of them can be fetched. Every one
names the same prerequisite:

```
error: Repository lacks these prerequisite commits:
error: 76e2e7021168959964851b9edfbb51704432fdc7
```

That commit is **ours** — `snapclone-egui: the incoming-call sheet, and a test
that found four more like it`. It is present, with identical content, at
`874f8965`. The `filter-repo` run renumbered every commit in the repo, and an
incremental bundle records its base by HASH.

Git will not accept the substitution, and is right not to: `fetch`, `unbundle`
and `verify` all refuse, because a bundle's prerequisite is a cryptographic
claim about history, not a hint. There is no flag for "I have that content under
a different name".

**This is the real, recurring cost of rewriting shared history** — larger than
the 42 chassis it cost the first time, because that was recoverable and this
affects every sibling session that branched before the rewrite and has not
re-cloned.

The fix is on the producing side: re-cut those seven bundles against the current
tip, or send them as `git format-patch` output, which records content rather
than ancestry and applies regardless of renumbering.

## Voxels and round objects: four things measured, two dead ends

Asked whether voxelising the vehicles — with special handling for tires and
round parts — would compress better. Measured each idea rather than reasoning
about it, and the two that sound best are the two that fail.

### 1. Surfaces of revolution: no (dead end)

If a wheel were a surface of revolution it could be stored as a profile plus a
sweep — a handful of numbers instead of a mesh. Tested every part of `analog` on
all three axes, bucketing vertices along the axis and measuring how much the
radius varies within each bucket:

```
best part:  15.4% radius spread   (288 verts)
median:     ~40%
worst:      69.7%
```

Nothing is round enough. **15% spread is not a cylinder**, it is a wheel with a
tread pattern, a hub, and bolt detail — which is exactly why it was modelled
rather than generated. And the roundest parts are 36 verts, so even perfect
fitting would save nothing worth the decoder.

### 2. Cross-vehicle part instancing: barely (12%)

A wheel is a wheel on forty vehicles, so shared parts should dedup. Normalised
each part to its own centroid, quantised to 12 bits and hashed: **460 parts
across 40 vehicles, 405 unique shapes, 12% repeats, most-repeated shape appears
4 times.**

These are harvested from 1,012 different source vehicles, so the wheels genuinely
differ. The parts-web found 107 duplicates in 1,000 CATALOG parts because those
came from a parts library; the fleet did not.

### 3. Per-part quantisation boxes: no (dead end)

Smaller boxes should mean fewer bits for the same precision. But the median part
box is **51% of the chassis box** — parts here are body panels, not greebles —
so at equal precision it needs 100% of the bits. No gain.

### 4. Axis deinterleaving + delta + byte planes: YES, 15%, lossless

The one that works, and it works for the reason the round-object question was
pointing at: **a ring of vertices varies smoothly, and interleaved xyz destroys
that.** Splitting the stream by axis lets the compressor see the smooth run:

```
interleaved q16       115.9 KB   100%
deinterleaved by axis 110.4 KB    95%
+ byte planes         101.0 KB    87%
+ per-axis delta       97.4 KB    84%
```

Across 20 chassis: **85%**. On 196 MB of packs that is **~167 MB, saving 29 MB**
with no precision change at all — the same u16 values, reordered before the
zlib pass and reassembled after.

This is a `bots.pack` v2: same quantisation, different byte layout. The decoder
change is small and mechanical, and `botpack.rs` already refuses an unknown
version rather than misreading it, so v1 and v2 can coexist.

## Mesh and index compression — 70 MB left, and it is not in the vertices

Asked about mesh/triangle compression, an internal volume tracker, and a neural
upscaler. Measured all three framings.

### The vertices are not where the bytes are

**54% of every pack is INDICES**, not positions. Measured across 25 packs: 14.1
MB of float stream against 16.5 MB of index stream. The quantiser spent all its
effort on the smaller half.

And the indices are stored the worst possible way — raw u32 through plain zlib.
They are the easiest thing in the file to compress, because a triangle list
references nearby vertices repeatedly:

```
one chassis, 40,557 indices:
  raw u32                158 KB
  zlib as-is              36 KB
  u16 where it fits       33 KB   (all 39 prims fit)
  + delta vs prev index    8 KB   20x, LOSSLESS
```

Across the packs' index streams: **48%, or ~70 MB off the 252 MB of packs.**
This is the single largest remaining win and it changes nothing about the
geometry — same indices, different encoding.

### Vertex welding: 14%, and legal here

14% of vertices are exact positional duplicates, and these meshes carry
**POSITION only** — no normals, no UVs. That matters: welding is normally
illegal because two vertices at the same point can carry different normals or
texture coordinates, and merging them creases the shading. With position-only
geometry there is nothing to disagree about. Worth doing after the index work,
since welding renumbers indices.

### An internal volume tracker: no

The idea is to store a coarse volume and rebuild detail at runtime. Two
measurements already rule it out. The parts DEFORM rather than rotate (median
38% radius drift f00->f04), so a volume that tracks a part cannot place it. And
the surface is not a surface of revolution or anything else parametric — the
roundest part measured 15% radius spread. **The detail is not derivable from a
volume, because the detail is the thing that was modelled.**

### A neural upscaler: no, and the reason is the storage budget

A net that predicts fine geometry from coarse would have to be either per-model
or general.

**Per-model** is a compression scheme where the weights ARE the data. A net that
reproduces one 250 KB chassis to visual fidelity will not be much smaller than
250 KB, and it would need training per chassis — 259 training runs to save less
than the index fix saves for free.

**General** — one net over the whole fleet — is the more interesting version and
still fails on this data. It would have to hallucinate plausible detail rather
than reproduce actual detail, and these are recognisable vehicles: a Charger
with invented panel lines is a wrong Charger. That is acceptable for terrain and
unacceptable for a licensed silhouette, which is what the whole harvest exists
to preserve.

There is also a plain engineering objection: it puts a neural inference in the
model load path of a game that currently starts in 0.2s, to save less than
delta-coding an integer array.

**The honest ranking**: indices (~70 MB, lossless, mechanical), then welding
(~14% of the float half, lossless), then nothing. Both are arithmetic, both are
verifiable byte-for-byte, and neither risks the silhouette.

## Vertex welding: 1.9%, not 14% — and not worth doing

The earlier note said 14% of vertices are exact positional duplicates and
welding is legal because these meshes are POSITION-only. Both halves are true
and the conclusion was still wrong, because I measured duplicates **in one
pose**.

A weld has to hold in EVERY pose. `xform::Morph` lerps vertex *i* of pose *k*
toward vertex *i* of pose *k+1*, so the poses must agree on what vertex *i* is.
Merge two vertices that are coincident in the vehicle pose but separate during
the fold, and the robot pose loses a vertex it needed — the surface tears
exactly where the fold does the most work.

Measured on `constructor_heavy_spider_animated`, prim 1: **776 duplicate groups
in f00, and 716 of them split apart by f04.** Across four chassis and 93,935
vertices:

```
duplicates in pose 0 alone : 12,989   13.8%   <- the misleading number
weldable across ALL poses  :  1,774    1.9%
```

92% of the apparent duplicates are vertices that only *happen* to coincide when
the vehicle is folded up — panels touching, wheels tucked into arches. They are
distinct vertices doing distinct things and the fold proves it.

**Worth ~1.6 MB** of the 86 MB float half, and it would renumber every index —
disturbing the delta stream that v3 just used to save 78 MB, since delta
compression depends on indices staying in a locality-preserving order. A
renumber optimised for vertex reuse is not the same order.

So: not done, and the reason generalises. **A property measured on one pose of a
morph target is not a property of the morph target.** The same mistake would
have been made by any tool that inspects a single glTF, which is most of them.

## The SDF budget, and whether a raid fits

Two questions: can the 126-model cap be raised, and would a 6-player raid with
enemies and a world still fit?

### The cap was WebGPU's baseline, not the device's

`render.rs` requested `Limits::default().using_resolution(adapter.limits())`.
`using_resolution` raises the 2D dimensions and leaves
`max_texture_dimension_3d` at WebGPU's guaranteed baseline of **2048** — so the
SDF volume was capped at 2048 depth regardless of what the GPU could do, and
`CSDF_SLAB * CSDF_MAXM` = 16 x 126 = 2016 sits just under it.

Now the device is asked for its real 3D limit and `sdf_capacity()` reads back
what was granted:

```
device max_texture_dimension_3d   models   chassis (5 poses each)
  2048  (WebGPU baseline)           126        25
  4096  (common desktop)            256        51
  8192                              512       102
```

Read back rather than assumed, because a device may grant less than requested,
and sizing a texture from an unhonoured limit is a validation error at creation
rather than a smaller roster. It never goes below `CSDF_MAXM`, because the
shader constants and staging buffer are sized against that — a roster the shader
cannot index is a worse failure than a full one.

The SLAB stays at 16. It is the depth resolution of each model's field and was
already cut from 44 to reach 126; cutting further trades silhouette accuracy for
count, and the silhouette is the thing this whole asset pipeline exists to
preserve.

### A raid fits, and it was never the problem

Every model on screen needs a slab; the CATALOGUE does not:

```
6 players, all different chassis     6 chassis    30 models
+ 12 enemy types                    18 chassis    90 models
+ 6 bosses and variants             24 chassis   120 models   <- fits in 2048
whole fleet                        221 chassis  1105 models
```

**A full raid is 24 distinct chassis and the baseline holds 25.** The budget was
never sized for a fight; it was being spent on loading every chassis in the
catalogue at startup, which no session can display at once.

So the durable fix is the one already in `roster.rs` — load a SUBSET — and the
right subset for a raid is "what this match uses", not "the first 126
alphabetically". `--bots` picks by tier or page today; picking by match roster is
the same mechanism with a different input.

The world and the players are unaffected either way: terrain, ball and effects
do not consume model slabs.

## Capital ships, 12+ players, procgen patrol space

### A big ship on screen costs pixels, not slabs

The SDF budget is indexed by MODEL, not by screen area or vertex count. A
capital ship filling the viewport is one chassis and consumes exactly as much of
the budget as a compact car. What it costs instead is fill rate and draw
distance, which are different resources with a different fix.

### Counting a patrol scenario

```
12 players, all distinct       12 chassis    60 models   depth   960   fits
+ 1 capital ship               13 chassis    65 models   depth  1040   fits
+ 8 fighter types              21 chassis   105 models   depth  1680   fits
+ 6 world props                27 chassis   135 models   depth  2160   over 2048
+ 4 boss variants              31 chassis   155 models   depth  2480   over 2048
```

Twelve players with a capital ship and eight enemy types fits the WebGPU
baseline. It is the world props and boss variants that push past it — and on a
4096 device all of it fits with room to spare.

### The lever nobody has pulled: static things need one pose

A chassis costs a slab PER POSE, and a fold strip is five. But a capital ship
that never transforms into a robot does not need a strip:

```
31 chassis as fold strips        155 models   depth 2480   over
31 chassis as static props        31 models   depth  496   fits easily
20 folders + 11 static           111 models   depth 1776   fits
```

**5x, for anything that does not fold.** Most of a patrol space is scenery,
capital ships and structures — things with no robot mode — and the loader
currently gives each of them five slabs because the fleet's format has five
poses. A single-pose asset already loads (the loader skips strips with fewer
than two poses only for MORPH purposes); what is missing is the roster treating
those as one model rather than five.

That is the cheapest capacity win available and it does not touch the renderer.

### What raising the cap actually requires

`sdf_capacity()` reports what the device could hold, and the roster still uses
`CSDF_MAXM`. That gap is deliberate: `shader.wgsl` hardcodes `CSDF_D = 2016.0`
and divides by it to locate a model's slab. Sizing the texture from the device
without changing that constant makes every sample read the wrong depth — a
silhouette assembled from other models' fields, which renders and is wrong.

The shader's own comment records that "three numbers had to agree ... and two of
the three were already wrong" before. Raising the cap means pushing CSDF_D in as
a uniform so there is ONE number, rather than adding a fourth place to disagree.

## Better utilisation: the atlas stacks on one axis of three

You were right that LOD does not solve this and utilisation is the key. Here is
where the waste actually is.

### The budget is 1.7 MB, not a memory problem

One slab is 36 x 24 x 16 = 13,824 voxels at one byte each. The whole 126-model
atlas is **1.7 MB of GPU texture.** Nothing here is constrained by memory. It is
constrained by a single number: `max_texture_dimension_3d`, 2048.

### And that limit applies to all three axes, while only one is being used

```
current   1 model per 36 x 24 x 16 column, stacked on DEPTH only
          depth per model 16, ceiling 2048  ->  128 models
```

Width is 36 and height is 24. Both have the same 2048 ceiling and both are
sitting nearly empty. Tiling models across W and H instead of only stacking them
on D:

```
tile 2x2   texture  72 x  48 x D    4 models per depth-slab   ->   512 models
tile 4x4   texture 144 x  96 x D   16 models per depth-slab   ->  2,048 models
tile 8x8   texture 288 x 192 x D   64 models per depth-slab   ->  8,192 models
```

All three fit every axis limit. At 4x4 with 512 depth that is **512 models for
7.1 MB** — four times the current roster, at a quarter of the depth ceiling,
with **no change to SLAB and therefore no quality loss**.

This is the answer to "better utilisation": the atlas is a 3D texture being used
as a 1D stack.

### What it costs to do

The shader's slab lookup becomes a 3D tile index rather than a depth offset:
`w = base + uvw.z` becomes a (u, v, w) offset from the model's tile origin. That
is a handful of lines, and it is the same place `CSDF_D` needs to become a
uniform — which the previous entry already flagged as the prerequisite for
raising capacity at all. Both changes land in the same function.

### Why this matters for the cases you raised

* **Transforming capital ships with interiors** are fold strips like anything
  else — five slabs — and an interior is more surface in the same box, so it
  wants MORE slab depth, not less. Tiling frees the count so SLAB can go back up
  toward its original 44 for the models that need it.
* **Combiner bosses** are the sharpest case: Devastator is six bots that are also
  one bot, so a single encounter is 7 chassis and 35 slabs under the current
  scheme. Four combiner bosses would exhaust a third of the whole budget.
* **Procgen raids** cannot be planned around a fixed roster at all, which is
  exactly why a 4x-to-16x headroom matters more than a smarter subset.

LOD does not help any of these, and it is worth saying why: LOD reduces
TRIANGLES for distant things, and the SDF budget is indexed by MODEL COUNT. A
prop at 5% screen size consumes exactly the slab a hero asset does. Utilisation
is the right lever and the atlas layout is where it lives.

## Classifying walkers vs ships: the geometry cannot answer it yet

The case: a ship with four-fold wing symmetry reads like a quadruped, so a
classifier needs to check for thrusters, and to tell a load-bearing leg from an
aerodynamic fin. Tried to build that from geometry. It does not work on this
data, and the reason is worth recording before anyone tries again.

### What was measured

**Ground contact and slenderness look promising and are not enough.** Taking
prims that reach the lowest 15% of the body and scoring vertical/horizontal
extent, 45 of 221 chassis have "leg-like" ground prims — but the top of that
list is `jet_vertigo` and `flyer_resonator`. Slenderness alone cannot separate a
landing strut from a leg, which is exactly the objection.

**The thruster test found nothing, correctly.** Rear prims that do not reach the
ground: median 0 across every category, including spaceship. My first attempt
also measured the wrong axis — `analog` declares forward as +Y in ASSET space,
so a raw-coordinate rear test is meaningless; redoing it in engine space via the
strip's orientation gave the same zero.

### Why: prims are materials, not parts

```
analog f04:  39 prims,  39 materials,  39 distinct materials referenced
             57% of prim pairs overlap spatially
```

A prim is a MATERIAL GROUP that spans the whole body — every red-painted surface
in one prim, every dark metal in another. They interpenetrate. There is no prim
that is "the left leg" or "the port thruster", so no per-prim test can find one,
however clever the metric.

This is the same wall three earlier findings hit from different sides: the parts
deform rather than rotate (median 38% radius drift), `joints.json` has 95% zero
angles, and the fleet's 46 part names are robot anatomy with exactly one
locomotion word. All four are the same fact — **the harvest preserved surfaces,
not structure.**

### What would actually work

Segmentation, not classification. A connected-component pass over the triangle
graph would give real parts — a leg is a component that touches the ground and
carries load; a thruster is one that does not and sits aft. Every metric
proposed here becomes computable the moment parts exist, and none of them are
computable before.

That is a mesh-processing pass over 221 chassis, and it belongs upstream in
foldview beside the rigidity segmentation the runtime-synthesis finding also
asked for. The two want the same thing and should be built once.

Until then `chassis_traits` seeds from the harvester's own category, which is
imperfect but is real evidence, and the vector can be corrected per chassis by
hand where it is wrong.

## v4: the same streams under one window (measured 2026-08)

v3 left one redundancy on the table, and it is CROSS-POSE: each pose is
zlib-compressed separately, and zlib's 32 KB window cannot see from one pose to
the next. But poses of a strip are the same model slightly moved — on strips
whose poses share a buffer layout, the delta-coded index stream repeats
VERBATIM between poses (measured: 7 of a 14-strip sample have exactly one
distinct index stream across all poses).

Two candidates were measured against the fleet before either was built:

| candidate | of v3 size | verdict |
|---|---:|---|
| temporal delta of quantised codes, per-pose zlib | 90-92%, sometimes >100% | rejected |
| same raw streams, ONE zstd-19 frame per strip | **49%** | v4 |

Explicit temporal modelling loses to the solid frame because pose layouts are
not always aligned (the same per-pose range tables that v3 exists to carry),
and a mispredicted delta is noise. zstd's strip-wide window finds the real
matches without being told where they are — so the format stays dumb on
purpose, and v4 changes ONLY the compression envelope. Same codes, same box,
same index deltas: v3 <-> v4 is a lossless transcode, and the fold-safety
argument (one box per chassis) transfers untouched.

Fleet result: **296 packs, 226.2 MB -> 118.5 MB (52%)**, every pose of every
pack byte-compared in `tools/pack_v4.py --verify`, cross-checked through the
Rust decoder (13 strips decoded byte-identically via both versions), and
`--revert` reproduces the original v3 files byte-for-byte (zlib level 9
matches pack_bots.py exactly).

The decode cost moved: v4 inflates the WHOLE strip per pose read, so
`pose_buffer` keeps a one-entry cache of the last decompressed frame — the
loader walks a chassis's poses consecutively, so one entry is the whole win
(measured 2.6x) without pinning ~300 strips' frames in memory. Decoding is
ruzstd (pure Rust, decode-only, no C toolchain in the cross builds), pinned to
0.7: 0.8 moves StreamingDecoder into `ruzstd::decoding` and needs cargo
resolver 3.

Still unpacked in the same tree, in size order: 83 MB of text gltf+json
(same sidecar trick would apply, but the seam is `gltf::Gltf::open` in
model.rs — a loader change), and 20 MB of scene.bin on the non-bot models
(fennec/octane/dominus/ball) that were never packed at all.
