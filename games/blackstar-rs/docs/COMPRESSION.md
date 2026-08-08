# COMPRESSION — measured before designed

The atlas idea is right, but the measurements reorder it, and one result
reverses the obvious conclusion. Numbers first.

## What is actually redundant

Sampling 14 chassis, 6754 parts, 263k vertices, signature = 8³ occupancy of
the part normalised for position, scale and axis order:

```
unique shape signatures          2681  (39.7% of parts)
so ~60% of PARTS are shape-duplicates of another part
vertices if one exemplar per signature   83.1% of original  = 1.2x
vertices living in parts of <=16 verts   14%
```

**Sixty per cent of parts are duplicates and they hold seventeen per cent of
the vertices.** The redundancy is in part COUNT, not in vertex data — the
repeated things are boxes and greebles, eight vertices each. A coarser 6³
signature said 64% duplicates and looked like a huge win; at 8³ with vertex
count it collapses to 1.2×. The finer measurement is the honest one.

So: **a shape atlas is a performance win, not a size win.** 2.5× fewer
unique meshes means 2.5× fewer buffers and real instancing across a crowd of
bots. Worth doing for that reason. It is not where the megabytes are.

## Where the megabytes actually are

```
28 chassis x 5 poses           125 MB
pose f00 alone, all chassis     25 MB
```

**Four of every five bytes are the same body in a different position.** A
pose strip stores five complete copies of a chassis. A chassis has ~80
joints that own parts; five poses expressed as joint angles is 80 floats x 5
= 1.6 kB. Against 4 MB of duplicated vertices per chassis.

Projected to the 216 foldable chassis the survey found:

```
current scheme          0.96 GB
rest pose + angles      0.19 GB      5.0x
+ 16-bit positions      0.10 GB      9.6x
+ shape atlas           0.08 GB     12.0x
```

The first step is worth more than everything after it, and it is the one
that also fixes the flipbook: a pose reconstructed from joint angles is a
pose you can sample at ANY t, not just the five that were exported. It
deletes `Morph`'s vertex blending, deletes the strip-parity problem, and
gives gait for free — a walk cycle is just another set of angles.

## Order, and the dependency that decides it

1. **Rest pose + joint angles.** 5x, and it is the enabler for continuous
   folds, gait, and per-frame poses. **Blocked on the joints being right** —
   which is the same blocker as animation, and now has a second reason to be
   fixed first.
2. **16-bit quantised positions**, relative to the part's own bounding box.
   2x, mechanical, no dependencies. Could be done today.
3. **Shape atlas + instancing.** 1.2x on size but 2.5x on unique meshes,
   which is the draw-call win. Needs the descriptor above plus a tolerance
   for near-matches.
4. **Residuals.** Only pays after 1-3, because a residual against an
   exemplar is only small if the exemplar is close, and 60% of parts already
   match exactly at 8³ — the remaining 40% are the ones that differ, which
   is exactly where residuals are largest.

## The part that is more than compression

Treating IQMs as part libraries rather than whole bots follows from step 3
rather than preceding it. Once parts are atlas entries with metadata — which
limb, what size class, what silhouette — the same index that saves space is
a catalogue you can build from: pick a torso from one region, arms from
another. That is procedural bots out of the compression scheme rather than
as a separate feature.

But it is fourth in line, not first, because every one of those parts still
has to attach to a joint, and the joints are wrong on most of the library.
An atlas of parts with nowhere correct to hang them is a tidier version of
the current problem.

## What I would NOT do yet

Voxel rebuilds and curve-profile refitting. Both discard the original
geometry for a reconstruction, which is a one-way door, and the measurements
say the win is 1.2x before residuals. Quantisation gets 2x for none of that
risk. Revisit if step 4 stalls.

---

# OCTREES, and where they would actually fit

Measured, since the question was whether they fit anywhere rather than
whether they are nice.

## The engine already has a voxel collider, and it is capped

`render.rs`:

```
CSDF_W 36  CSDF_H 24  CSDF_SLAB 44   = 38,016 voxels per model
CSDF_MAXM 12                          <- twelve models. Total.
```

That cap is not a tuning choice, it is what a DENSE grid costs: every model
pays for its whole bounding box whether or not there is anything in it. With
28 chassis, sixteen of them already have no exact collider. With the 216 the
survey found, the number is 204.

## How empty a bot actually is

Occupancy of the robot pose in that same 36x24x44 grid:

```
analog      10.1%    boartail     8.0%    brutis      11.9%
beetlejuice 12.6%    blackstar   13.6%    chokepoint  16.0%
cryofin      7.9%    ebr_105     14.2%
mean 11.8%
```

A bot is a shell. Seven-eighths of its bounding box is air, and the dense
grid stores that air at full price.

## What an octree gives

Building the occupancy pyramid at depth 6 — a 64³ effective resolution,
which is EIGHT TIMES the linear resolution of the current slab — and
collapsing any node whose eight children are all present:

```
analog       4.7% of dense      boartail     3.5%
beetlejuice  5.7%               brutis       5.7%
blackstar    7.0%               chokepoint   8.4%
mean 5.8%  ->  17x smaller, at 8x the resolution
```

So the trade is not "octree or grid". It is that an octree buys BOTH: finer
collision and enough headroom to give every chassis an exact collider rather
than twelve of them.

## Why this is the same structure as the atlas

An octree of a part is a shape descriptor. The 8³ occupancy signature used
for atlas dedup IS an octree truncated at depth 3. One structure serves
both: dedupe by comparing subtrees, collide by descending them, and a part
that matches an exemplar shares its tree rather than storing a second copy.

That is also why poses fit the graph. A pose does not change a part's tree,
only its transform — so poses are edges, parts are nodes, and the collider
is the node's own contents. Same graph, three uses.

## Order

Octrees go AFTER the joints and the pose-angle representation, for the same
reason as everything else: a collider tree is per-part, and per-part
transforms come from joints. But unlike the residual work, this one has a
number that stands on its own — 17x at 8x resolution — and it lifts a
current hard cap of twelve.

---

# SPHERE / CUBE MODS AND A 3D DCT — measured

Asked whether primitive-plus-residual or a trig basis would pay. Measured on
real parts rather than reasoned about, and the answer is a clear no for
geometry and a clear yes for something else.

## Are these parts even star-shaped?

A sphere parameterisation needs `r(θ,φ)` to be single-valued — one surface
crossing per direction. Tested across grit, blackstar and patton:

```
82% of parts are star-shaped within 18% radial spread
median spread per direction bin: 0.11
```

So the parameterisation is VALID for four parts in five. The remaining
fifth — L-brackets, rings, anything with a concavity — cannot be expressed
this way at all, which already caps the method below "use it for everything".

## What it costs and what it gets

```
method                     bytes/part    median error (% of part size)
raw f32                        99,082        0.00
16-bit quantised               49,541        0.03
sphere r(θ,φ) 32x32             2,054        4.72
sphere r(θ,φ) 16x16               518        5.79
sphere r(θ,φ) 8x8                 134        6.84
sphere r(θ,φ) 4x4                  38        8.69
sphere (single radius)              8       11.29
```

**48x smaller than quantised at 32x32, and 4.7% error.** That sounds good
until you look at the shape of the curve: quadrupling the coefficients from
4x4 to 16x16 buys 8.7% -> 5.8%. The error is not converging, because most of
it is not high-frequency detail — it is the 18% of parts that are not
star-shaped, plus the hard edges of the ones that are. A box has a
discontinuous `r(θ,φ)`, and no smooth basis represents a discontinuity
cheaply. That is Gibbs, and a DCT has exactly the same problem.

**4.7% of part size is not acceptable for geometry.** On a 2 m torso that is
9 cm of wobble on every panel edge. It would look like the model was carved
from soap.

## So: no for meshes, and here is what it IS for

The measurement kills the residual idea for POSITIONS and hands it a much
better job. Look again at where the megabytes actually are: five poses of
the same body, and the fix is rest-pose-plus-joint-angles (5x). Nothing
about that wants a spherical basis.

But `r(θ,φ)` at 4x4 — THIRTY-EIGHT BYTES — carries a part's shape to 8.7%,
which is uselessly coarse for rendering and excellent as a DESCRIPTOR. It is
the same role the 8³ occupancy signature plays for the atlas and the octree,
in a fifth of the space and with rotation handled by shifting the θ axis
rather than re-voxelising.

So the honest recommendation is to stop treating this as compression and use
it as the shape key: cheap, rotation-aware, and already validated on 82% of
the library. The 18% that are not star-shaped keep the voxel signature,
which handles concavity fine.

## The 3D DCT specifically

Worse than the above, for a reason that is structural rather than tuneable.
A 3D DCT wants a dense volume, and these meshes are SHELLS — 11.8% occupancy
in the collision grid. You would be transforming seven-eighths air, and the
coefficients would spend their energy describing where the emptiness is.

The 2D DCT over `r(θ,φ)` is the sensible version of the idea, and it is
what the table above approximates: a grid IS the DCT with every coefficient
kept, so the numbers there are the BEST a DCT of that resolution could do.
Truncating coefficients can only move down that column.
