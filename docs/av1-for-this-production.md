# AV1 for this production — what was measured

Every number here came from encoding the actual footage on the actual
machines. Where something is reasoned rather than measured it says so.
If you change a decision, please replace the number rather than the
prose, because the prose is only there to explain the number.

Source footage: Blackmagic Production Camera 4K, ProRes 422 Standard,
3840×2160, 10-bit 4:2:2, 24p, **472 Mbit/s**. 88 files, 391 GB.

---

## The short version

If you only read one section: **archive at CRF 18, SVT-AV1,
`yuv420p10le`, preset 6.** That takes 391 GB to about 5 GB. Do not
delete the ProRes until someone has watched the output on a real
monitor.

Encode whole files concurrently — one file per worker, 24 workers.
Never slice inside a file.

---

## 4:2:2 is not worth keeping

| | bytes | chroma PSNR | time |
|---|---|---|---|
| 4:2:2 10-bit | 159,520 | 53.87 / 53.11 | 47.7 s |
| 4:2:0 10-bit | 148,834 | 53.41 / 52.77 | 37.3 s |

**0.46 dB of chroma** for 7% smaller and 22% faster.

Keep the **10 bits** — those carry the grade, and the footage is a
deliberate low-contrast 1990s look that lives in a narrow range. Let
the chroma subsampling go. This also frees the encoder choice, because
SVT-AV1 below 4.x is 4:2:0 only.

---

## Which encoder

| | bytes | PSNR (luma) | time |
|---|---|---|---|
| libsvtav1 | 68,835 | 43.39 | 17 s |
| librav1e | 218,472 | 44.06 | 49 s |

rav1e is the Rust one and handles 4:2:2 and even 4:2:2 12-bit
natively, which SVT-AV1 only gained in 4.x. But **3.2× the size for
0.67 dB at 3× the time** is a trade SVT-AV1 wins by simply lowering its
CRF.

**Hardware cannot help.** AV1 is decode-only on Apple silicon, and
decode only from M3 — **M1 and M2 have none at all**. Separately, no
hardware AV1 encoder from any vendor does 4:2:2: NVENC offers 4:2:0 and
4:4:4 and skips it, QSV is 4:2:0 only. This is CPU work whatever the
machine, so core count decides: prefer the 24-core M2 Ultra over the
16-core M4 Max.

---

## CRF is not a scale you can carry between encoders

| | bytes |
|---|---|
| x264 crf 23 | 1,872,988 |
| **svtav1 crf 23** | **2,964,082** ← 58% BIGGER |
| svtav1 crf 30 | 1,881,933 ← the actual equivalent |
| svtav1 crf 35 | 1,033,508 ← same quality region, 45% smaller |

This is why re-encoding an old H.264 library to AV1 "at the same CRF"
produces bigger files and everyone concludes AV1 is worse. The offset
is roughly **+10 to +13**, but measure it rather than trusting that —
`av1tuner_core::calibrate::equivalent_crf` sweeps both on the same
footage and matches them by size.

Two smaller effects, both real and both much less than the CRF offset:

- **Re-encoding lossy costs ~6%.** AV1 faithfully reproduces H.264's
  ringing and blocking, which are high-frequency noise it must spend
  bits preserving.
- **Grain is brutal.** Same clip, same settings: clean 82 KB, grainy
  1.87 MB with x264. A 23× penalty.

That second one is why `film-grain=8:film-grain-denoise=1` is the
single biggest lever on a period look — denoise before encoding,
synthesise at playback, stop paying to store noise.

---

## Why longer encodes get better, and what breaks it

Same clip, looped:

| frames | bytes/frame |
|---|---|
| 6 | 3522 |
| 24 | 2039 |
| 98 | 1655 |

53% cheaper per frame at 16× the length. **Forcing keyframes every 6
frames** so the density is identical:

| frames | bytes/frame |
|---|---|
| 6 | 3522 |
| 98 | 2761 |

So **31 points were keyframe amortisation** and **22 points were
something else**. The something else is this, from
`Source/Lib/Codec/ec_process.c`:

```c
if (primary_ref_frame != PRIMARY_REF_NONE) {
    svt_memcpy(ec->fc, &ref->frame_context, sizeof(FRAME_CONTEXT));
} else {
    svt_aom_reset_entropy_coder(...);
}
```

AV1's arithmetic coder starts from generic default probabilities and
adapts as it codes. Each frame hands its converged `FRAME_CONTEXT` to
the next, so the probability model keeps learning the content — the
grain structure, the palette, the motion statistics — across the whole
encode. On hours of material it converges hard and every symbol gets
cheaper.

### What breaks it

Anything that forces `PRIMARY_REF_NONE`: a keyframe, a scene cut, or
**an independent chunk**. Measured:

| | bytes |
|---|---|
| one encode | 162,177 |
| 4 chunks | 194,213 (**+20%**) |
| 4 chunks + 12-frame warm-up | 186,235 (+15%) |

Warm-up frames — encoding some frames before each boundary and
discarding them — recover only a quarter of the loss, for 50% more work
on the warm frames. **Not worth it at this chunk size.**

> Caveat: this test loops *identical* content, so the encoder can
> predict a repeat that a real compilation would not offer. The 20% is
> probably an overestimate for varied footage. Re-measure on four
> different clips before sizing a batch around it.

### So how do you use 24 cores?

**You already do, and you do not need to chunk.** The important line is
in `pic_manager_process.c`:

```c
child_pcs->ppcs->frm_hdr.primary_ref_frame = ref_index;
```

The context chain follows the **reference graph**, not display order.
Sibling frames in a mini-GOP inherit from their own references and
encode concurrently — the serialisation runs along reference edges,
which is a dependency motion compensation already has. SVT-AV1 is 16
pipeline stages (`me_process`, `md_process`, `enc_dec_process`,
`cdef_process`, `ec_process`, …), each a thread with queues between
them, with many frames in flight.

Two chunks **cannot** share references, by definition: a decoder handed
chunk 3 has never seen chunk 2's frames. That is not a missing feature,
it is what decodability means.

**Therefore:** run whole files concurrently, one per worker. Full core
utilisation, zero context loss, and less code than any alternative. Your
88 files are already 88 natural boundaries.

---

## What AV1 offers that rav1e does not use

The spec constrains **decoders**, not encoders — nothing bounds how
hard an encoder searches. So the headroom is not hidden in the
standard, it is in tools encoders do not exploit. Files mentioning
each:

| | rav1e | svt-av1 |
|---|---|---|
| temporal filtering (alt-ref) | **0** | 60 |
| global motion | **0** | 26 |
| quantizer matrices | **0** | 14 |
| intra block copy | 4 | 44 |
| superres | 2 | 44 |
| segmentation | 14 | 29 |
| delta_q / delta_lf | 9 | 25 |

**Temporal filtering is the largest gap** and explains most of the 3.2×.
It builds a denoised, motion-compensated composite that everything else
predicts from. It is also 4,104 lines of C plus AVX2, SSE4.1 and
NEON-DOTPROD hand-vectorised paths — a months-long port, at the end of
which you have what SVT-AV1 already does, slower.

**Global motion is the tractable one**, and the reason is that rav1e
already writes it. `header.rs` codes all four model types with the
subexp encoding the spec wants — and writes:

```rust
let mv_x = 0;
let mv_y = 0;
```

Zero. The syntax layer is finished and nothing computes the parameters.
See `docs/global-motion.md` for the estimator and its open questions.

**Segmentation is the same shape.** `get_initial_segmentation` inherits
the previous frame's map and nothing ever analyses a frame to decide
what the segments should be. AV1 carries eight segments each with its
own QP delta, loop-filter delta and reference override — per-region
rate control, in-frame. On shallow-depth-of-field footage a third of
the frame is out of focus and currently gets the same bits as the face.

---

## Things that bit me, so they do not bite you

**`ROTZOOM => unimplemented!()` and `AFFINE => unimplemented!()`** in
rav1e's header writer. Those branches *panic*. An estimator returning a
rotzoom crashes the encoder mid-frame; it does not degrade. Only
translation is writable today.

**The translation precision constant is derived, not verified.**
Assuming `WARPEDMODEL_PREC_BITS = 16` makes a 3.3-pixel pan overflow the
coded range, which no real format would do. 13 bits
(`WARPEDMODEL_PREC_BITS - GM_TRANS_ONLY_PREC_BITS`) gives ±64 px with
high precision and ±16 without, which matches what global motion is for.
**Confirm against a reference decoder before shipping** — getting it
wrong produces a stream that decodes to the wrong motion rather than
failing loudly.

**In rav1e's arithmetic, `allow_high_precision_mv` makes the global
motion step COARSER** (0.0625 → 0.125 px) while doubling the range.
That may be deliberate or the constants may be transposed; the branch
has never executed, so a sign error there would never have been caught.
Open question.

**Point-sampling aliases.** Coarse-to-fine motion search must low-pass
before subsampling. Taking every 8th pixel of a textured frame makes the
subsampled images correlate at offsets the full-resolution images do
not, and the coarse stage hands the refiner a false minimum it cannot
escape.

**Rejecting outliers by match quality does not work.** A coherently
moving subject matches *beautifully* — it is a clean translation, just
a different one from the camera's. Measured: SAD-based rejection gave
tx 8.01 against a true 6.0, and keeping everything gave 5.31. Reject by
**disagreement with the fit**, not by match error.

**And one about testing, not code.** I spent three rounds debugging a
correct estimator against a test scene built from `sin(x) + cos(y) +
(x*y)%97` — a diagonal moiré with strong horizontal periodicity, in
which a 12-pixel shift genuinely *is* ambiguous. Same code on broadband
texture: exact. If a motion estimator fails on synthetic input, suspect
the input.

---

## Audio, while we are here

The boom recordings claim two channels and have one. Measured on
`STE-078.opus`: left peak **−50.5 dBFS** with a crest factor of 5.7
(that is noise, not speech), right at **+1.9 dBFS**. It is a mono boom
in the right channel of a stereo container. **A pipeline that
duplicated "the left channel" to make stereo would produce silence.**

`studio_audio::boom::examine` finds the live channel rather than
assuming.

**Reverb breaks envelope alignment.** `slate.rs` cross-correlates
envelopes, chosen because raw samples do not correlate across different
microphones — that reasoning is right. But applying a synthetic room to
a copy of the boom at a known 12 ms delay:

| | reports |
|---|---|
| true delay | 12.0 ms |
| envelope correlation | **65.2 ms** |
| onset correlation | 12.0 ms |

The reverb tail adds energy *after* the event and drags the envelope's
centre of mass late. A mic further from the source has more tail, so the
error grows with exactly the thing that makes alignment hard. Correlate
the envelope's **rises only** — the attack is what two mics share, the
tail belongs to the room.

**And mic distance is a real offset, not an error.** Sound covers
343 m/s, so 3.4 m is 10 ms. A boom at 1 m and a camera at 5 m genuinely
disagree and both are right. Whether to correct it is a decision, so the
result says how far apart the mics were rather than silently fixing it.
