# UPSCALING — what the Grimlock reference actually says

Measured against the shipped roster rather than admired.

## The numbers

```
                    parts   verts/part (median)   median part size
reference Grimlock     42            58,721             23% of body
ours / grit            38                36             17%
ours / blackstar       34                18             12%
```

## The finding, which was not what I expected

**The part count is already right.** After the colour merge our chassis carry
34–38 parts against the reference's 42, and the size distribution is
comparable — a median part is about a fifth of the body in both. The fitter
and the merge are producing roughly the right ARTICULATED GRANULARITY.

**The density is off by three orders of magnitude.** 58,721 vertices per part
against 18–36. That is the entire visual gap, and it means upscaling is not
a segmentation problem:

* You cannot add detail by splitting parts. We already have the right number
  of them, and more parts would make the joint problem worse rather than
  better — every extra part is another thing to attach.
* Detail has to come from the SURFACES of the parts we have: subdivision and
  displacement, not decomposition.

## What that implies for the pipeline

A detail pass belongs AFTER the fold, not before. The fold operates on part
transforms, so a 36-vertex box and a 58,000-vertex sculpt fold identically —
which means detail can be added at the very end, per pose, and never has to
survive the joint fitting, the repair, the reattachment or the strip export.

That also keeps it out of the compression problem: 42 parts x 59k verts is
2.5 M vertices per pose, and five poses of that is 50 MB per chassis before
textures. Storing rest-pose-plus-angles (COMPRESSION.md, 5x) stops being an
optimisation and becomes a precondition.

## What this reference is NOT

It is robot form only, as OBJ. There are no bones, no vehicle mode and no
fold, so it is a POSE and DENSITY reference rather than a rig reference. It
cannot tell us where a Cybertronian's joints go — only what one should look
like once they are in the right place.

98 textures at 628 MB accompany it. Deliberately NOT committed: the bundle
is already 438 MB and carrying ten generations of regenerated bot strips.
The measurements above are the part worth keeping in the repo.
