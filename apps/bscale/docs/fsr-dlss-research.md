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


---

# Cross-worker reuse (`d2x-core/src/xref.rs`)

Separate from the FSR work above, and prompted by the same question: where else is this
pipeline doing work it has already done?

## What already worked

A `-j N` run splits at keyframes, one worker per segment. Two kinds of reuse existed:

* the **reference ring**, per worker — near-matches, accepted by the quality gate on a
  bounded SSE, confined to that worker's own frames;
* the **`BlockCache`**, already shared across workers — but **exact bytes only**.

So cross-worker reuse existed and could only fire on an exact repeat.

## The gap

Video repeats *approximately* far more often than exactly: a cut back to the same shot, a
character against the same wall two minutes later, a slow pan revisiting scenery. Each is a
near-match the ring would happily take and cannot, because it lives in another segment. On a
`-j 8` dialogue scene the same two shoulders get upscaled eight times.

## The design, and the one safety argument that matters

`Xref` is keyed by a coarse fingerprint of the low-res cell — 4×4 block luma means quantised
to 5 bits. **The fingerprint's only job is to make the candidate list short.** The existing
SSE budget, the same number the reference ring is tested against, decides whether a candidate
may be used.

That ordering is the safety argument. The gate remains the sole authority, so a cross-worker
hit is exactly as good as an in-worker one by the measure the user set with `--quality`. This
cannot make output worse than the gate permits; it can only find more places the gate says
yes. A fingerprint collision costs one bounded comparison and is rejected.

Deliberately NOT a perceptual hash with its own similarity threshold: a second similarity
measure that disagreed with the gate would be a second policy, and the failure mode would be
a block the gate would have rejected being served because something else called it close
enough.

Entries store the low-res cell as well as the upscaled block, because the gate has to compare
against something — an upscaled-only store could not verify anything and would be back to
trusting the hash. At `block=32, bleed=4, scale=2` that is ~4.8 kB in and ~12 kB out per
entry, so 256 MB holds roughly fifteen thousand distinct cells.

Two bounds, each with a test: FIFO eviction under the byte budget (as `BlockCache`), and a
per-fingerprint cap, because a flat fingerprint — sky, letterbox, a fade — would otherwise
collect thousands of near-identical cells and make every lookup against it walk the lot. The
gate would still be correct; the run would just be slower than not caching at all.

`Stats` counts hits, gate rejections, empty buckets, evictions and **comparisons**, because
without the cost side the feature is unfalsifiable: more memory, and no way to say what it
bought. A high compared-to-hits ratio means the fingerprint is too coarse for that content.

## Status: measured, and it does not pay yet

`cfg.xref_bytes` defaults to 0 and the pipeline does not consult the store. That is now a
decision rather than an unfinished step.

A separate measurement of the closely-related subtile/block cache — with harvesting and
gated probing, on Lone Ranger AV1, 120 frames, 512 MB — found harvesting raised supply 3.7×
(14,567 inserts to 54,390) **and the hit rate did not move**: 0.2% to 0.3%. Gated probing by
a quantised signature was flat at every quantiser and at both 4×4 and 2×2 signatures. On a
cut-heavy clip, 60 hits in 88,679 probes; on SMITE, zero.

`Xref` makes the same two mistakes, and those numbers are the evidence:

1. A quantised fingerprint is **still an exact match, in a coarser space** — 16 numbers that
   must all land in the same bin. LSH or a small ANN index is the right tool, not a different
   quantiser.
2. **The probe set is the bottleneck.** A cell only reaches this store after Stationary, the
   motion search, the reference ring, the plate and the anchor have all failed it. What
   survives is content genuinely new to the shot, which by definition has no twin in a few
   seconds of cache. The cases the cache was built for — letterbox, static HUD, unchanged
   background — never arrive, because they are Stationary, which is cheaper. `RecalledMoved`
   serves ~1238 blocks/frame on SMITE; the cache serves 0.3.

Cross-worker sharing widens **supply**, and supply was already shown not to be the binding
constraint. So the honest read is that the sharing is not the thing that was missing.

What would change the answer, in order: outlive the shot (an on-disk index keyed by a shot
descriptor — the one change that makes cross-worker sharing matter, since segments are cut at
keyframes and a returning shot is very likely in another segment); probe with motion rather
than identity; and keep it for animation, where exact repeats are the norm.

---

# Seams: the thin bright/dark grid lines (`d2x-core/src/seams.rs`)

## Three faults look the same and are not the same bug

**1. Residual-packing contamination — a hard 1px rim.** `make_residual` packs cells into a
small square image with `bleed` pixels of real context each. Beyond that ring sits *the next
cell*: unrelated content, with a step change at the join. The network reads that step as an
edge and rings against it, inward by its receptive radius.

`receptive_radius` computes that radius off the op list — every conv here is 3×3, so each adds
one pixel at its buffer's level. Eight convs reach eight pixels; SRVGGNetCompact's sixteen
reach sixteen; RRDBNet's twenty-three blocks reach many times that. **The default `bleed` in
this pipeline is 1.** So the outer pixels of every block are computed partly from unrelated
content, which is a thin rim at the block pitch — exactly the reported symptom.

The feathered merge cannot fix this. It blends contaminated pixels with their neighbours; the
contamination happened before any blending.

**Found while writing this:** the codebase already has the right number and does not use it
here. `Network::pre_pad` is documented as "the overlap that hides tile seams" — Real-ESRGAN
ships 10, upconv_7 an exact 7 — and the *tiler* uses it while the residual packing uses
`bleed`. Two numbers for one physical requirement, and the smaller one is the default.
`BleedCheck::disagrees_with_tiler` flags that.

**2. Level mismatch — a soft step, no rim.** A reused block is a copy of an earlier upscale;
a fresh one came out of the network. Both pass the gate, because a uniform half-level offset
over a 32×32 block is a tiny SSE and a visible edge.

**3. Merge rounding — a true 1px line.** The feather ramp is float and rounds per pixel; where
two blocks' ramps both round just under 1.0 the destination keeps a fraction of what was
underneath. Invisible on a static background, not over a cut.

## Measuring, and two wrong guesses on the way

`seam_profile` compares the mean luma step across block boundaries with the step at interior
lines of the same frame. Block boundaries are not special places in a picture, so any excess
is the pipeline's.

Two things had to be corrected against the tests:

* **One measure is not enough.** On noisy content a one-level block offset is invisible in
  mean |step| — twenty levels of noise against one of offset — and obvious in the SIGNED
  mean, where noise cancels and DC does not. A rim is the reverse. Hence `excess_db` and
  `level_step`, separately.
* **A rim does not cancel in the signed mean**, which was the first discriminator and was
  wrong: ringing puts a dark pixel one side of the boundary and a bright one the other, so
  the step is large and signed exactly like a level offset. What separates them is the
  NEXT step in — a rim is a local extremum and reverses, a level offset is monotone.
  `flank_ratio` is that, ~-1 for a rim and ~0 for a level step.

The control fixture had to be corrected too: a `(x + y) % 200` ramp puts its wrap
discontinuity on some sampled lines and not others, and reported −2.5 dB on a frame with no
seams. It is deterministic narrow-band noise now — wide noise makes a clean frame's signed
means wander by the same half-level the fault test looks for.

## What to do about them, in order

1. **Raise `bleed` to the model's `pre_pad`** and measure. One-line change, and the number to
   watch is `seam_profile` before and after on a frame with a known grid. Costs residual area
   — a bigger cell per block — so it trades throughput for the rim, which is exactly the kind
   of trade `compare` exists to price.
2. **Mirror the cell's own edge into the bleed ring** where real context is unavailable
   (frame edge), rather than whatever the packing leaves there.
3. **A gutter between cells in the residual image**, filled by extending each cell's edge, so
   there is no hard content step for the network to ring against at all. Strictly better than
   raising bleed for the contamination fault, and cheaper: a gutter is shared between two
   neighbours, where bleed is paid per cell per side.
4. **DC-match a reused block to its surroundings** before writing it, for fault 2. This is the
   one to be careful with: it changes pixels the gate approved, so it needs its own bound.

Nothing here is wired in — `seams` is a measurement module and a diagnostic. The next run
should print `BleedCheck::describe` at startup and `seam_profile` on the first output frame,
which turns "I see faint lines" into two numbers before anything is changed.
