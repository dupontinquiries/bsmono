# Motion-aware sampling: find the cinematic sections first

Your observation — the building detail lives in slow camera moves,
while the narration is spread across the whole runtime — means a
uniform frame interval spends most of its budget where there is
nothing to read. `ocrscan plan` fixes that, and it borrows its
taxonomy from `fao_codec_core::region_activity`.

## Why it borrows from fao-codec

`region_activity.rs` opens with the right insight: *frames are rarely
globally static or moving — they are mixed*, and the useful
distinction is between smooth parallax motion and localised activity.
Its `BlockLayer::ParallaxBg` is exactly your cinematic pan, separated
from `Active` and `Noise`.

`plan` reimplements that distinction on a coarse luma grid rather than
linking the crate, so ocrscan stays a standalone binary. If you later
want the real per-block layer assignment, `RegionActivityTracker`
already produces it — the swap is one function.

## The proxy step is not optional

4K AV1 at 60 fps must be fully decoded even to emit 2 fps of probe
frames. On your `part1.mkv` (8 min, 3840x2160) that ran past a
five-minute limit. Make a proxy once:

```bash
ffmpeg -i part1.mkv -vf "fps=4,scale=-2:180" -c:v libx264 -crf 28 -an proxy.mkv
```

Planning on the proxy then takes **~2 seconds per two minutes** of
source. Everything downstream still refers to original timestamps, so
the proxy is throwaway.

## Running it

```bash
ocrscan plan --video proxy.mkv --probe-hz 4 --out plan.tsv
```

```
 380.0-395.0  ACTIVE  every 4   s  mag=8.1  unif=0.62
 395.0-396.8  PAN     every 0.5 s  mag=6.6  unif=0.80
 396.8-423.2  ACTIVE  every 4   s  mag=3.8  unif=0.38
```

Four classes, each with a suggested interval: **PAN** 0.5 s (the
cinematic move — sample densely), **ACTIVE** 4 s (localised editing),
**STATIC** 20 s (one frame is enough), **CUT** skipped. On the sample
clip this planned 31 frames where uniform 1 Hz would have taken 150,
while sampling the pan 8x more densely than the rest.

## How a pan is actually detected — and a mistake worth avoiding

The obvious signal is *uniformity*: motion spread across the whole
frame rather than concentrated. That is necessary but **not
sufficient**, and trusting it alone produced a false positive on your
footage: a section at unif=0.57 looked like a pan and was not.

Checking it properly — does shifting one frame by a few pixels reduce
the difference? — showed a 10 px shift barely helped, so the motion
was distributed activity, not a camera move.

So the classifier now requires that a **single global shift explains
the change**: it searches shifts of ±6 x ±4 cells on a 64x36 luma grid
and computes how much the best one reduces the residual
(`--pan-explained`, default 0.25). Uniformity plus an explanatory
shift is a pan; uniformity alone is not.

Two other corrections from running it on real footage, both worth
knowing if you tune further:

- **Smooth the measurements, not the labels.** Earlier versions
  median-filtered the class sequence, which relabelled runs without
  relabelling their statistics — producing STATIC sections with higher
  magnitude than neighbouring ACTIVE ones, which is visibly wrong.
  Frame-to-frame magnitude swings 0 to 200 on real video; the camera
  does not. A rolling mean over ~1 second is the honest signal.
- **Calibrate against the distribution.** `--dump-raw` writes per-
  instant `mag` and `unif`. On this footage: mag median 2.9, p90 27.5;
  unif median 0.24, p75 0.51. The defaults come from those numbers,
  not from taste. Re-dump on your own material before trusting them.

## What this found on part1.mkv

- **First two minutes: no camera pans at all**, at any threshold down
  to `--pan-explained 0.05`. Not a tuning artefact — genuine absence.
- **Around 395 s: a real pan** (unif 0.80), with the 15 seconds before
  it at unif 0.62 — a near-miss worth inspecting by eye to decide
  whether your definition of "cinematic" is looser than mine.

That matches what you said: the detail is in the later moving
sections. It also means a uniform scan of this video would have spent
most of its frames on the first half.

## Feeding it forward

```bash
# dense capture only where it matters
awk -F'\t' '$3=="PAN"' plan.tsv | while IFS=$'\t' read -r s e cls iv rest; do
  ocrscan blocks --video part1.mkv --interval "$iv" \
      --palette palette.tsv --out "blocks_${s}.tsv"
done
```

`blocks` and `scan` take `--interval` per invocation, so the plan
drives them one section at a time. Cutting the section out first
(`ffmpeg -ss $s -to $e`) avoids re-decoding the whole 4K file for each
one — worth it given the decode cost above.

## Tuning knobs

| flag | default | what it does |
|---|---|---|
| `--probe-hz` | 2 | probe sampling rate; 4 on proxies |
| `--pan-min` | 2.0 | motion floor before anything is "moving" |
| `--uniform-min` | 0.50 | measured p75 of `unif` on real footage |
| `--pan-explained` | 0.25 | how much a global shift must explain |
| `--cut-min` | 28.0 | above this, a scene change |
| `--smooth-window` | ~1 s | rolling mean width |
| `--dense/normal/sparse-interval` | 0.5/4/20 | per-class output intervals |
| `--dump-raw` | — | per-instant measurements for calibration |
