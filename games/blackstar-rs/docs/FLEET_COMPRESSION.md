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
