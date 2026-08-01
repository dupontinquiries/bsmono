# The physical UI: why textures slide, and why it became floating orbs

Both reported problems have one cause, and it is not a texturing bug — **there are no textures.**

## The vertex format has no UVs

`gpuscene.rs` packs **12 floats per vertex**: `pos×3, normal×3, color×4, mat, emissive`. There is no
texture coordinate anywhere in the pipeline, and no `sampler2D` in the geometry pass — the only
samplers are in the fullscreen post-process, where screen-space UV is correct.

So every surface "texture" is a **procedural pattern sampled from world position**. That works, and it
is cheap, but it means the projection plane is chosen in the shader rather than baked into the mesh.

## Why they slide

The water warp sampled `v_world.xz` — a **planar XZ projection**. On a floor that is exactly right. On
a vertical face the pattern is *constant along Y*, so it stretches down the face and appears to slide
across it as the camera moves.

Fixed with **triplanar** mapping: choose the projection plane per-fragment from the dominant axis of
the normal, blended at the seams so a 45° face does not pop. `a = pow(abs(n), 4)` sharpens toward the
dominant axis; too low and every face smears, too high and the seam is a visible line.

**Two projections were deliberately left as XZ.** The caustics and the golden-rain marble stain both
*fall from above*, so a top-down projection is what they physically are — converting them to triplanar
would make light appear to come from the sides.

## Why the UI became floating orbs

This is the same finding wearing different clothes. **Without UVs you cannot inscribe anything into a
surface** — no runes, no lettering, no panel text bound to a face. The renderer can express untextured
lit geometry with per-vertex colour and an emissive term, and nothing else.

So the UI drifted to what the renderer *can* do: large, self-contained, glowing objects. Orbs and
oversized buttons are not a style choice that went wrong, they are the only vocabulary available.

## What would unlock the intended direction

The direction described — inscriptions on surfaces, plants that respond to touch, runes that light on
proximity — needs two things, in this order:

1. **A UV attribute.** 12 → 14 floats per vertex, `layout(location=5) in vec2 a_uv`. Locations 0–4 are
   taken (pos, normal, color, mat, emissive). This is the enabling change: without it, nothing can be
   written *on* a face.

2. **A per-vertex or per-material "response" channel.** Breathing plants and lighting runes are the
   same mechanism — a scalar that a fragment shader can drive from time *and* from proximity to a
   touch point. The scene already has `u_time` and per-vertex `a_emissive`; what is missing is a
   uniform carrying the last touch position in world space, so emissive can rise with nearness.

Neither needs a new render pass. Both are additive to the existing pipeline, and (2) is small once (1)
exists — which is the argument for doing them in that order rather than reaching for a mesh format with
tangents and normal maps.

## On button size

Worth separating from the above: buttons reading as *independent floating objects* is partly the
missing-inscription problem and partly scale. A control sized for a finger in **screen** space becomes
a boulder in **world** space when the camera is close. The `mobilelayout` touch minimum (44pt) is a
2-D rule and does not transfer — a world-space control needs its size derived from its distance to the
camera, so it subtends a constant angle rather than occupying constant world volume.
