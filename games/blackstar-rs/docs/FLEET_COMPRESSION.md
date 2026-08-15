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

## Not done here

The packer itself. This document is the measurement and the constraint it found;
writing `build_bots_web.py` and the loader side is the next step, and it should
carry the per-chassis-box rule as a test rather than a comment.
