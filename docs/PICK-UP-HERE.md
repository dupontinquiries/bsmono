# Pick up here

A fresh session's map of the work from this investigation. Read
`docs/av1-for-this-production.md` first for the measured numbers;
this is about what is built, what is not, and what to be careful of.

Nothing here is in production. Everything is tested standalone.
**Nothing has been verified against a decoder or against the real
engine build**, and the places where that matters are marked.

---

## The immediate practical job

**391 GB of ProRes wants to become about 5 GB of AV1.**

```
ffmpeg -i INPUT.mov -c:v libsvtav1 -crf 18 -preset 6 \
       -pix_fmt yuv420p10le OUTPUT.mkv
```

Run 24 of those concurrently, one whole file each. Do not slice inside
a file — see the chunking section of the measurements doc for why that
costs 20%.

`av1tuner_core::presets` has this as the `archive` preset along with
four others, each stating what it gives up. `presets::advice(391.0,
472.0)` prints the table.

**Do not delete the ProRes** until someone has watched AV1 output on a
real monitor. PSNR is blind to the two things that would actually
matter here — banding in the gradients and blocking in the haze — and a
few GB of AV1 beside 391 GB of ProRes costs nothing while you wait.

---

## What was built, and where

### `apps/av1tuner/av1tuner_core/`

| module | purpose | tests | state |
|---|---|---|---|
| `presets.rs` | five storage presets with measured sizes | 7 | ready |
| `calibrate.rs` | CRF sweep, cross-encoder mapping, broken-scale detection | 9 | ready |
| `remote.rs` | transcode over HTTP from a dufs listing | 8 | untested against live dufs |
| `gm/mod.rs` | global motion estimator | 8 | not integrated |
| `gm/wire.rs` | estimate → bitstream integers | 7 | **precision unverified** |
| `gm/segment.rs` | AV1 segmentation from the motion field | 7 | not integrated |
| `gm/buckets.rs` | timeline segmentation for preview render | 8 | not wired to studio |

### `apps/studio/crates/studio-audio/src/boom.rs`

Boom mic alignment by onset correlation, mono channel detection,
stereo-spread options with mono-fold-down safety. 7 tests. Not wired to
the command palette.

### `apps/studio/crates/studio-video/src/slate_ocr.rs`

Clapperboard reading. **5 of 7 tests pass — this one is unfinished.**
See the section below.

### `maintenance/tools/completions/`

zsh completions generated from what tools already reveal about
themselves. 12 tests. Scanned 149 real scripts: 71 entry points, 62
both, 4 steps, 12 orphans.

---

## Next steps, in the order I would do them

### 1. Verify the global motion precision constant

Half a day, and it blocks everything else in the encoder work. Encode a
known translation, decode with a reference decoder, check the
reconstructed shift. Details and the three specific worries are in
`docs/global-motion.md`.

Getting this wrong produces a stream that decodes to the *wrong motion*
rather than one that fails loudly.

### 2. Wire global motion into rav1e

Three small changes, listed in `docs/global-motion.md`. The one that
matters most is not in the list: blocks must predict from the
**quantised** warp, not the estimate, or every block drifts from what
the decoder will reconstruct. `wire::as_decoded` exists for that.

### 3. Finish the clapperboard reader

It is the most unfinished thing here. What works: frame selection by
Laplacian sharpness (found the sharp frame at index 7 of 15, where focus
ranged 19 to 73), field assembly, confidence gating, and clip
arrangement by scene/shot/take.

What does not: the recogniser. Two findings that should redirect it:

- **The board's printed labels are the reliable text.** `SCENE`, `TAKE`,
  `ROLL`, `DATE` are high-contrast reversed type. Use them as *anchors* —
  find the word `TAKE`, read the box below it — rather than guessing
  layout from position.
- **Your ffmpeg has `--enable-libtesseract`.** It has the `ocr` filter
  built in. My hand-rolled template matcher had to drop `O`, `I`, `S`,
  `Z` and `G` from its vocabulary because seven segments cannot tell them
  from `0`, `1`, `5`, `2`, `6`. Tesseract reads printed type properly.
  Writing a worse OCR when a good one is linked into the binary you
  ship is hard to justify.
- **The handwriting is a different problem.** `SCENE 2.1` in marker is
  not template-matchable. Show the operator the cropped cell and let
  them type three characters. A reader that gets the geometry right and
  asks about the numbers is worth far more than one that guesses, because
  a wrong take number is discovered at the end of the edit.

### 4. Wire the preview buckets into studio

`gm/buckets.rs` segments a timeline by motion character and assigns a
preview bitrate per region. Measured 29% less preview data than a flat
rate on a realistic dialogue-heavy timeline. The rate multipliers are
**judgements, written as numbers so they can be argued with** — see the
module docs.

### 5. Boom alignment in the command palette

`boom::align_many` already splits results into applied and *not*
applied. Keep that: a palette that silently nudges twelve clips, three
of them wrongly, gives a scene that is subtly out with no record of
which three.

---

## What I would not do

**Do not fork rav1e to close the general gap.** SVT-AV1 wins on
efficiency and speed today: 3.2× smaller at 3× the speed, measured.
rav1e's real advantages are 4:2:2/12-bit and memory safety. The
interesting Rust work is the *specific* gaps — global motion,
segmentation, quantizer matrices — not catching up overall.

**Do not port temporal filtering.** It is the largest efficiency gap
and also 4,104 lines of C plus three hand-vectorised assembly paths.
Months, ending with a slower version of what exists.

**Do not chase encoder speed.** SVT-AV1 at 24 cores is fast enough to
make proxies, which was the actual requirement.

**Do not build the GPU AV1 decode path I originally suggested.** I was
wrong: **M1 and M2 have no AV1 hardware decode at all** — that starts at
M3. On Asahi it is dav1d on CPU with NEON, plus Mesa Vulkan for colour
conversion and display. That is a smaller and different job than I
described, and I have not scoped it.

---

## Honest state of my own confidence

Things I measured and would defend: the 4:2:2 decision, the encoder
comparison, the CRF offset, the chunking cost, the CDF inheritance
mechanism, the reverb alignment error, the mono-in-stereo channel
finding.

Things I reasoned and would want checked: the global motion precision
constant, the preset rate multipliers, the bucket rate multipliers, the
20% chunking penalty on *varied* footage (my test looped identical
content, which flatters the single-encode case).

Things I got wrong during the session and corrected: M1/M2 having AV1
decode (they do not); that the flat washed-out look was overexposure (it
is a deliberate 1990s style); that SAD-based outlier rejection would
help (it hurt); that `WARPEDMODEL_PREC_BITS` applied to translation (it
does not).

One process note worth passing on: three separate times a test failure
was my *test* being wrong rather than the code — a moiré test scene, an
inverted sign convention, and an arithmetic slip on a byte count. When
something fails in a way that looks structurally strange rather than
slightly off, check the test first.
