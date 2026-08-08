# Would unendlich's neural upscaler apply to FAO?

Asked directly. Answered by reading both sides rather than by analogy.

## What unendlich actually has

Not a network. `unendlich-engine/src/sim/settings_model.rs` has two functions —
`upscale_factor` (1/render_scale, clamped) and `upscale_mode` (4 / 2 / 1) — plus
a render-scale slider in the world renderer labelled "neural upscale". It is the
**policy** that selects a reduced render target and a magnification mode. The
magnifier itself is downstream and not in that crate.

So there is no model to lift. What is genuinely shared is the policy, and it has
been promoted to `uikit_egui::upscale` for both consumers, with the clamp that
its own comment justifies: *"so a stray scale can't request an unbounded
factor"*. A scale arriving as zero turns `1/s` into infinity and something
downstream allocates a buffer from it.

## Why the deeper idea is more applicable to FAO than to a game, not less

Temporal upscalers of the DLSS/FSR2 class are not magnifiers. They accumulate
detail across frames, and they work because the renderer hands them three things
a naive upscaler does not have:

1. **Per-pixel motion vectors**, so last frame's samples can be reprojected onto
   this frame.
2. **Sub-pixel jitter**, so consecutive frames sample *different* points inside
   each output pixel — that is where recovered detail actually comes from.
3. **Depth**, to reject reprojections across occlusion boundaries.

A video decoder normally has none of these, which is why single-image video
super-resolution is mostly hallucination. **FAO has the first one already.**
`fao_codec_core`'s `bitpack` exists to pack "block IDs and motion vectors", and
v2.1 codes K-frames and T-frames — it is a temporal codec, so the decoder parses
motion vectors as a matter of course. The expensive ingredient is free.

The second is the interesting one, and it is an **encoder** change rather than a
decoder one. If FAO is going to encode below output resolution, the encoder
chooses the downsampling phase — so it can alternate sub-pixel phase between
frames. That hands the decoder genuinely different samples of the same scene to
accumulate, which is the difference between recovering resolution and inventing
it. A game jitters its projection matrix; an encoder jitters its downsample.
Nothing in the format prevents it, and the phase costs a couple of bits a frame
to signal.

~~The third has no equivalent. There is no depth buffer~~ — **WRONG, and
corrected below.**

## Correction: FAO has all three, and that was the design all along

I wrote the above after reading `fao_codec_core` and stopping. v8's own header
states the opposite:

> Per-frame depth map (each pixel gets a Z value) … Triangle mesh of major
> surfaces … **CompressionPass — use mesh+texture to encode each frame as
> "render this mesh from this camera pose". Frame becomes a 12-byte camera
> matrix instead of pixel data.**

`CameraPose` carries position, a rotation quaternion, focal lengths and a
principal point. Depth, rotation and zoom are first-class bitstream content.
This is not a codec missing the third ingredient of a game upscaler; it is a
codec whose entire thesis is to have all three, and surpassing AV1 that way was
the point.

**It is better placed than a game engine, not worse.** A renderer derives motion
vectors from geometry as a side effect; here the geometry IS the bitstream. And
unlike a renderer, a v8 decoder knows the PREVIOUS frame's depth too — which is
what makes disocclusion detectable rather than guessable.

`egui-frontend/src/reproject.rs` implements the piece that was missing:
per-pixel motion from depth and pose, and a three-way visibility test
(consistent / occluded / disoccluded) against the current depth map. 16 tests,
including parallax — nearer geometry moving more under the same camera motion,
which is precisely the information one block vector cannot carry because one
block can contain both depths.

## And it changes what "residual" should mean

The stated goal is to store less residual. Reprojection is the mechanism: if the
decoder can predict frame N from N-1's pixels plus this frame's depth and pose,
the encoder sends only what reprojection gets WRONG — disocclusions, speculars,
moving objects. Those are small and spatially clustered, which is the shape
entropy coding likes. `residual_fraction` is that number.

**PSNR is the wrong yardstick for judging it.** A reprojected frame that is
geometrically right but shifted half a pixel scores terribly on whole-frame PSNR
while looking correct; a blurred frame that is slightly wrong everywhere scores
well and looks soft. Optimising a reprojecting codec against PSNR steers it
toward blur — the one artefact this design must not have. `masked_error`
measures only where the encoder was responsible, which separates "the prediction
was good" from "the residual coding was good": two things a rate-distortion
decision must weigh separately and which one PSNR number fuses.

## What this would be, concretely

A **coding tool**, not a post-process. Encode at half resolution with alternating
phase, signal the scale and phase in the frame header, decode at half and
reconstruct at full using the motion vectors already in the bitstream. That is
the shape AV1's `superres` takes, and the reason it lives in the codec rather
than in the player is that the encoder's cooperation is what makes it work.

## What would make it a bad idea

* **The quality numbers must not lie.** PSNR computed against the upscaled output
  as though it were a lossless decode of the original would make this look like
  a free win. It is a lossy tool and belongs in the rate-distortion curve.
* **Block motion vectors are coarse.** Sub-block motion, rotation and anything
  non-translational will smear. Worth measuring on real content before believing
  a demo.
* **A neural magnifier on a phone is not free.** The fleet already declines to
  run a gaze pipeline at 30fps for battery reasons; a per-frame network at video
  rates is the same argument, larger.

## Status

Not built. The shared policy is (`uikit_egui::upscale`, 9 tests). The temporal
part is a codec change with an encoder half, and it should be measured against
FAO's existing rate-distortion numbers before anyone writes the magnifier.

---

# The part grammar: modelling the things in the video

blackstar's `partdata` turns out to be directly relevant, and not for the reason
a model library would be. Its own header:

> The library ships no geometry: it is a catalogue of 5,873 part signatures and,
> for 178 vehicles, WHERE each part sits. **That is a compositional grammar
> rather than a mesh.**

Eight classes — block, panel, strut, disc, wheel, barrel, greeble, tread — with
a position and a size. That is a **prior over structure**, which is exactly what
a reconstruction codec lacks. v8 densifies depth into points and then has to
code the geometry; if a cluster can be described as *"wheel, here, this big"*,
the bitstream carries nine bytes instead of a mesh patch.

**And it carries meaning, which is worth more than the bytes.** `PartClass::spins`
marks wheel, disc and tread. A wheel's motion between frames is a rotation about
a known axis — so the decoder can PREDICT it rather than the encoder measuring
it. No block motion vector can express "this rotated"; that is the gap the
reprojection work above opens and the part grammar fills.

## The failure mode is guarded before the win is claimed

A part fitter that is confident and wrong does not produce noise. It produces
**plausible structure that was never filmed** — the same trap as optimising
against PSNR, wearing different clothes: a hallucinated wheel looks like content
and scores well on any whole-frame metric.

So every fit carries a residual, `accept` refuses one that does not explain its
own points, and the fallback is ordinary residual coding rather than a worse
fit. **Refusing is the expected case**: most video is not vehicles, and a grammar
mined from 178 vehicles has no business describing a face or a tree.

`worth_coding` adds the second condition, which is easy to forget: a fit must be
accepted AND cheaper than the geometry it replaces. A perfect fit for four
points costs more than the four points.

## Two bugs the tests caught, both worth recording

**Flatness must be tested before aspect.** A flat square — 3 × 3 × 0.2 — has an
aspect of 15, so an aspect-first classifier calls a disc a strut. "Long and
thin" has to mean thin in BOTH minor axes, and only the flatness test
distinguishes thin-one-way from thin-two-ways.

**A bounding box cannot see an outlier.** Measuring residual against the
cluster's own min-to-max box means a stray point simply becomes the box, and a
cloud with a point fifty units away fits perfectly. The extents are now the 90th
percentile of per-axis deviation, so the bulk of the cluster defines the shape
and a stray point lands outside it. That is the whole job of a residual, and the
first version did not do it.

## Status

`part_fit.rs`, 13 tests. Not wired into the codec: this takes points something
else has already grouped, and v8's clustering is where it would attach. The
honest sequence is reprojection first (it needs no grammar and helps all
content), then part fitting for the content where the grammar applies.
