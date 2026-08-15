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
