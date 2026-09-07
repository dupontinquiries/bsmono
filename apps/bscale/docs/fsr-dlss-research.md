# FSR / DLSS → bscale: what transfers

Researched against the AMD **FSR SDK 2.3.0 "Redstone"** source drop (FSR Upscaler 3.1.5,
FSR ML-Upscaler 4.1.1, FSR Frame Generation 3.1.6 / ML 4.0.1, Ray Regeneration 1.2.0,
Radiance Caching preview) and NVIDIA's DLSS 4.5 / DLSS 5 announcements.

## The short version

bscale is an **offline video** upscaler. FSR and DLSS are **real-time renderer** upscalers.
That difference decides almost everything, and it is worth stating plainly before listing
ideas, because most of the FSR chain is unusable here for a structural reason rather than an
effort reason.

A game engine hands its upscaler three things a decoded video file does not have:

1. **Per-pixel motion vectors**, exact and free, because the engine computed the motion.
2. **A depth buffer**, which is what disocclusion detection is built on.
3. **A sub-pixel jitter sequence**, deliberately applied to the camera so that successive
   frames sample *different* points inside each output pixel — which is the actual source of
   the extra information temporal upscaling reconstructs.

Video has none of these. Point 3 is the one people miss: FSR2/3 is not "smart blur", it is
accumulating genuinely new samples the renderer was told to produce. A video decoder
produces frames sampled wherever the camera happened to be, with no guarantee of sub-pixel
diversity and no way to request it.

So the temporal core does not port. What ports is the parts that operate on a **finished
frame**.

## Applied: RCAS (`d2x-core/src/rcas.rs`) — done

AMD's Robust Contrast-Adaptive Sharpening, from `ffx_fsr1.h`, ported to CPU f32 over 8-bit
RGB. It is the last pass of FSR and the one piece that needs no motion vectors, no jitter and
no history.

It earns its place here for a specific reason. Every path in this pipeline produces output
slightly softer than a native frame — a CNN trained on an L1/perceptual mix regresses toward
the mean, bicubic and flat-bypass are soft by construction, and a **reused block is a copy of
an earlier upscale** and inherits that frame's softness. A generic unsharp mask would make
that worse, because it also amplifies ringing and the block-boundary discontinuities the
reuse gate can leave. RCAS is specifically built not to:

- **It cannot overshoot.** The lobe comes from the min/max of the 4-tap ring, so the result
  stays inside the range the neighbours already occupy. No new halo along a block seam —
  which is exactly the artifact that would make reuse visible.
- **It backs off on noise.** A luma detector measures the centre's departure from its ring
  relative to the ring's own spread, and scales the lobe from 1.0 down to 0.5. On AV1 sources
  — where `analyze` already reports *more* pixels going through the network because
  quantisation noise changes frame to frame — sharpening without this makes that noise
  permanent.

Kept faithful: the same 5-tap cross, the same `0.5·R + G + 0.5·B` luma, the same per-channel
hit-min/hit-max lobe, the same `FSR_RCAS_LIMIT = 0.25 − 1/16`, and sharpness in **stops**
(AMD's units — 0 is maximum, each whole stop halves it), so a value tuned against any FSR
implementation means the same thing here.

Two deliberate differences, both documented in the module: f32 rather than FSR's optional f16
path, and it runs on gamma-encoded 8-bit rather than FSR's post-resolve linear-ish values —
which makes a given lobe bite harder in shadows, hence the conservative 0.8-stop default.

Six tests, and two of them were wrong first and are worth recording:

- The "an edge gains contrast" test initially picked a centre pixel sitting exactly on its
  ring's mean. That is a **fixed point** of the kernel — the cross is symmetric, so the
  output is the input whatever the lobe is. It passed while measuring nothing.
- The noise test first compared a grain-like pixel's movement against an edge-like one's and
  failed. Correctly: the lobe's contrast limiter is more generous on a mid-grey ring than on
  a high-contrast one, so the grain pixel moved *further* despite a smaller noise factor.
  Both are true and only one of them is the gate, so the test now asserts the factor
  directly.

**Not yet wired into the pipeline.** It is a `d2x-core` primitive with tests; the merge stage
does not call it and there is no `--sharpness` flag. That is the next step, and it wants a
`compare` run (PSNR/SSIM against an unsharpened reference) before it becomes a default,
because sharpening always trades a metric for an appearance.

## Worth doing next, in order

**1. RCAS in the merge stage, per-block-origin.** The interesting version is not "sharpen the
frame". It is: sharpen *reused* blocks slightly harder than *CNN* blocks, because they are the
ones carrying an older frame's softness. The pipeline already knows which is which — that is
what `FramePlan` is. This is the one idea in this document that FSR does not have and could
not have, because FSR's output does not come from two different processes per region.

**2. FSR's disocclusion logic as a reuse-gate cross-check.** FSR2/3 rejects history where the
reprojected sample cannot be reconciled with the current frame's neighbourhood. bscale's gate
is a JPEG-quality threshold on the block match, which is a *fidelity* measure and does not
know about temporal plausibility. A neighbourhood-clamp check would catch the specific case
the quality gate is weakest on: a block that matches well by SSD but sits in a region whose
surroundings have all changed. Cheap, CPU-side, testable.

**3. EASU (FSR1's spatial upscaler) for the flat-bypass path.** Flat-bypass currently means
"don't run the network here". Those regions still have to get to output resolution somehow,
and EASU is a better answer than bicubic for a handful of ALU ops. It is in the same
`ffx_fsr1.h` and has no temporal dependency.

## Not applicable, and why

**FSR 4's ML upscaler (`super-resolution-ml`).** A trained model with the same motion-vector
and jitter requirements as FSR3, shipped as compiled shader blobs for RDNA. Neither the inputs
nor the weights are available to a video pipeline, and the architecture is tuned for a
3.5 ms budget at 4K — a constraint bscale explicitly does not have. bscale's own trade is the
opposite: it can afford 9 s/frame for RRDBNet if the picture is better.

**FSR Frame Generation / DLSS Multi Frame Generation.** Interpolating frames between rendered
ones, using optical flow plus the engine's motion vectors. For video this is frame-rate
interpolation, which is a different product from upscaling, and the good open implementations
(RIFE et al.) are not what is in this SDK.

**DLSS 5.** Announced at GTC in March 2026, launched September 3 2026 with NBA 2K27 on RTX 50.
It is worth being clear that despite the name it is **not an upscaler generation** — it is
real-time neural *rendering*: a model that infuses pixels with photoreal lighting and
materials, driven by the game's own G-buffer and 3D scene guidance, costing 50–60% of GPU
frame time. There is no scene to guide it with here. The super-resolution lineage continues
in **DLSS 4.5** (second-generation transformer SR model, announced at CES 2026), and the only
thing that transfers from that is a direction — transformer-based SR over CNN — which is a
model-training project, not a port. The README's existing note is the relevant one: truncating
RRDB without retraining is already worse than bicubic, so architecture changes here mean
training, not surgery.

**FSR Ray Regeneration / Radiance Caching.** Path-tracing denoise and irradiance reuse. No
analogue in video.

## Licensing

The FSR SDK is MIT. A port of the RCAS kernel is a derivative of MIT-licensed source and
carries AMD's copyright notice; `rcas.rs` names the origin in its module docs. If more of
`ffx_fsr1.h` is ported (EASU), the same applies and the notice should be made explicit in the
crate's licence file rather than only in a doc comment.
