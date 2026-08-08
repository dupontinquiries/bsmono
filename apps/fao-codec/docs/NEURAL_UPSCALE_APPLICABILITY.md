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
