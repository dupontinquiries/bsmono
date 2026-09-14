# Global motion in rav1e — state of the work

**Status: estimator written and tested standalone. Not integrated into
rav1e. Not verified against a decoder.**

Everything described here lives in `apps/av1tuner/av1tuner_core/src/gm/`
as a standalone library with 32 tests. It has no rav1e dependency, so it
can be reasoned about and tested without building rav1e — which is
deliberate, and which is also why it is not yet wired in.

---

## Why this is the tractable encoder project

rav1e **already writes global motion**. `src/header.rs` codes all four
model types — identity, translation, rotzoom, affine — with the subexp
encoding the spec wants. What it writes is:

```rust
GlobalMVMode::TRANSLATION => {
  let mv_x = 0;
  let mv_x_ref = 0;
  let mv_y = 0;
  let mv_y_ref = 0;
```

Zero. The syntax layer is finished; nothing computes the parameters, so
`globalmv_transformation_type` stays `IDENTITY` and the feature is inert.
SVT-AV1 has 26 files of global motion. rav1e has a writer and no
estimator.

For comparison, the *other* big gap — temporal filtering — is 4,104
lines of C plus AVX2, SSE4.1 and NEON-DOTPROD hand-vectorised paths.
This one is an estimation problem with the plumbing already done.

**What it buys:** on a pan, every block carries nearly the same motion
vector and coding it per block spends bits describing the same fact
thousands of times. Global motion states it once in the frame header.
Handheld and slow pans — most documentary, and most of this production's
footage — are where it pays.

---

## What exists

| module | what it does | tests |
|---|---|---|
| `gm/lib.rs` | the estimator: translation search, local motion field, affine fit | 8 |
| `gm/wire.rs` | converts an estimate into the integers rav1e's writer wants | 7 |
| `gm/segment.rs` | drives AV1 segmentation from the same motion field | 7 |
| `gm/buckets.rs` | segments a timeline by motion character, for studio-live | 8 |

The last two are not encoder work — they reuse the same measurement for
other purposes. See `docs/preview-buckets.md`.

### The estimator

Coarse-to-fine translation search, then a local motion field on a grid,
then a least-squares affine fit with outlier rejection. It returns the
*simplest* model that earns its place, because an affine header costs
more than a translation and there are seven reference slots per frame.

`Estimate::worth_sending()` is the important method: it refuses to
signal anything that does not remove at least 8% of the prediction error
with better than 50% inliers. **A model that does not fit is worse than
no model** — blocks reference a prediction that is off, spend bits on the
residual, *and* paid for the header.

---

## What is verified, and what is not

**Verified by test:** pans are found and reported as translations; still
frames stay identity; noise produces nothing worth sending; a moving
subject does not drag the model off the background; a pan is not
promoted to affine just because six parameters can express it slightly
better.

**NOT verified:** anything about the bitstream. No stream has been
produced, no decoder has read one. Three specific worries, in order:

### 1. The precision constant is derived, not confirmed

`wire.rs` uses `TRANS_ONLY_PREC_BITS = 13`, taken as
`WARPEDMODEL_PREC_BITS - GM_TRANS_ONLY_PREC_BITS` and cross-checked by
asking what range it produces against rav1e's own `bits` and
`bits_diff`:

| frac | high precision | low precision |
|---|---|---|
| 16 | 8.0 px | 2.0 px |
| **13** | **64.0 px** | **16.0 px** |
| 12 | 128.0 px | 32.0 px |

16 is clearly wrong — a 3.3-pixel pan, an ordinary camera move,
overflows the coded range. 13 gives a range that matches what global
motion is for. **But that is reasoning, not confirmation.**

**First thing to do:** encode a known translation, decode with a
reference decoder, check the reconstructed shift matches. Getting this
wrong produces a stream that decodes to the *wrong motion* rather than
one that fails loudly, which is the worst kind of wrong.

### 2. `allow_high_precision_mv` behaves backwards

In rav1e's arithmetic the flag *increases* `bits_diff` (9 → 10), so the
step gets **coarser** — 0.0625 px to 0.125 px — while the range doubles.
That is the opposite of what the flag's name suggests.

It may be correct: the flag governs motion vector precision, and global
motion translation may deliberately trade precision for reach when MVs
are finer. Or the constants may be transposed. **The branch has never
executed**, so a sign or order error there would never have been caught
by anything.

The test asserts the *observed* behaviour and names the question rather
than encoding an expectation.

### 3. Only translation is writable

```rust
GlobalMVMode::ROTZOOM => unimplemented!(),
GlobalMVMode::AFFINE => unimplemented!(),
```

Those branches **panic**. `wire::for_rav1e` refuses those models and says
why, rather than silently downgrading — a silent downgrade would hide
that the branch still needs writing.

This also sets the build order: translation is the common case on real
footage (a pan *is* a translation), it is the branch that exists, and it
can be validated end to end before anyone writes the rotzoom coder.

---

## Integration, when someone does it

Three changes to rav1e, in order:

**1. Somewhere to put the parameters.** `FrameInvariants` has
`globalmv_transformation_type: [GlobalMVMode; INTER_REFS_PER_FRAME]` and
**no field for the values** — which is itself evidence the feature was
scaffolded and never finished. Add the per-reference parameters
alongside it.

**2. Compute before the header is written.** `encode_frame` calls
`write_frame_header_obu` at `src/encoder.rs:3798`. The reference frames
are known by then. Run the estimator per reference slot and fill in both
the type and the values.

**3. Use them in the writer.** Replace the hardcoded zeros in
`header.rs` with the computed values.

**Then, and only then, the thing that actually matters:** blocks must
predict from the *quantised* warp, not the estimate. `wire::as_decoded`
exists for this. Using the unquantised estimate while the decoder has
the quantised one is a drift bug — every block predicts from a slightly
different place than the decoder will, and the residual silently pays
for it. That will not show up as a crash or a corrupt image, only as
worse compression, which is the hardest kind of bug to notice.

---

## Traps already hit, recorded so they are not hit twice

**Point-sampling aliases.** Coarse-to-fine search must low-pass before
subsampling. Taking every 8th pixel of a textured frame makes the
subsampled images correlate at offsets the full-resolution images do
not, and the coarse stage hands the refiner a false minimum it cannot
escape. Measured: a true (12, −9) came back as (−60, 39). The fix is
averaging over the step window, which is the low-pass a pyramid would
normally provide.

**Do not rescale between search stages.** `search_at_scale` returns a
full-pixel offset; `step` controls how coarsely it *samples*, not what
units it answers in. Multiplying the coarse result before refining
quadrupled it at every stage: (12, −9) became (−512, 320).

**Reject outliers by disagreement, not by match quality.** A coherently
moving subject matches *beautifully* — it is a clean translation, just a
different one from the camera's. Measured on a frame with a quarter
covered by a subject moving the other way: SAD-based rejection gave
tx 8.01 against a true 6.0, and keeping everything gave 5.31. Rejection
made it **worse**, because it threw away well-matched background and
kept the well-matched subject. Fit, measure each block's distance from
the fit, drop the ones that disagree, refit.

**The warp is the inverse.** It maps a point in the current frame back
to where it came from in the reference, because that is what a predictor
needs. If a test asserts the forward sign it will fail and the code will
look wrong.

**Suspect the test scene.** Three rounds were spent debugging a correct
estimator against `sin(x) + cos(y) + (x*y)%97` — a diagonal moiré with
strong horizontal periodicity, in which a 12-pixel shift genuinely *is*
ambiguous. Same code on broadband texture: exact, every time. If a
motion estimator fails on synthetic input, check the input has broadband
non-periodic texture before touching the algorithm.
