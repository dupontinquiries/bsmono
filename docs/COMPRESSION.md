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
