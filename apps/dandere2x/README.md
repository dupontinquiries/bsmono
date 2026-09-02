# dandere2x (Rust / wgpu / AV1)

Block-matched residual super-resolution for video: only the parts of a frame
that changed go through the CNN; everything else is copied from the previous
output through a constant-quality gate. This is a port of
[aka-katto/dandere2x](https://github.com/aka-katto/dandere2x) with the
upscaler brought in-process (Real-ESRGAN / waifu2x on wgpu compute, Vulkan on
Asahi) and an AV1 pipeline around it.

```
ffmpeg (libdav1d) ─▶ analysis (CPU) ─▶ residual ─▶ wgpu CNN ─▶ merge ─▶ ffmpeg (libsvtav1 10-bit)
```

## Layout

| crate | what | deps |
|---|---|---|
| `crates/d2x-core` | the engine, headless: gate, matchers, plan, residual/merge, the `Network` op-list model layer with CPU reference, the two-stage pipeline | image, rayon, serde |
| `crates/d2x-wgpu` | `Network` on wgpu 29: conv3x3 / deconv / add / pack kernels, tile loop, selftest | wgpu, pollster |
| `crates/d2x-job` | the job as a library: ffmpeg I/O, AV1 sink, backends, keyframe-aligned segment parallelism, clip ranges, cancel, stage events | image, serde_json |
| `crates/dandere2x` | CLI + egui shell (`app` feature) over `d2x-job` | clap, eframe, uikit-egui |

Studio (`apps/studio`) uses `d2x-job` directly: "upscale clip" bounces the
selected clip's range in the background and lands a new source + clip beside
the original (autotune's print-side pattern; dandere2x is sequential, so it
cannot be a chain effect).

Every layer above the engine only sees the traits `FrameSource`, `FrameSink`,
`Upscaler`. The GPU is one `Upscaler`; the CPU reference is another; tests
use nearest-neighbour, which makes the residual/merge round trip an exact
identity.

## Models

The model is data: `Network::load` builds an op list from

- `.safetensors` — Real-ESRGAN **RRDBNet** (x2/x4/x8 photo models: `conv_first` … 23 RRDB … `conv_up*`), or **SRVGGNetCompact** (`realesr-animevideov3`). Convert PyTorch checkpoints with `tools/pth2st.py` (numpy only, no torch).
- `.json` — waifu2x `upconv_7` (nagadomi's format).

`models/` vendors `realesr-animevideov3.safetensors` (2.5 MB, x4, ~0.62 MMAC
per input pixel). The RRDB photo models are 67 MB each and are not vendored;
`RealESRGAN_x2` costs ~4.5 MMAC per input pixel, x4 ~17. `dandere2x model
<file>` prints a model's architecture and per-frame cost.

Measured while deciding this (tools/depth_probe.py, RealESRGAN_x2, one
rendered frame): truncating the RRDB body without retraining is not viable —
20 of 23 blocks is already as far from the full model as plain bicubic. Use
the models that were trained small, or distil from the full model on your own
clips.

## Usage

```
dandere2x probe                                    # adapters wgpu can see
dandere2x selftest --model RealESRGAN_x2.safetensors   # GPU vs CPU, byte-exact
dandere2x analyze  -i in.mkv                       # what block reuse buys, per frame, no CNN
dandere2x upscale  -i in.mkv -o out.mkv --model RealESRGAN_x2.safetensors
dandere2x upscale  -i in.mkv -o out.mkv -j 4      # keyframe-aligned segments, one shared GPU, 4 encoders
dandere2x upscale  -i in.mkv -o clip.mkv --start 61.5 --end 90   # a range (audio trimmed + re-encoded, opus)
dandere2x app                                      # the shell
```

Reuse gate: `--quality` (JPEG quality the reuse must beat; higher = stricter
= less reuse), `--block-size`, `--bleed`, `--matcher stationary|diamond|
exhaustive:r`. `analyze` prints the fraction of pixels that would go through
the network; on compressed AV1 sources it is lower than on clean sources
because the artifacts change frame to frame.

Encode: libsvtav1, 10-bit, `tune=0`, `scd=1`, `keyint` set from the source's
median keyframe interval (ffmpeg < 7 ignores `-force_key_frames` for
libsvtav1; ≥ 7 gets the exact list). Colour tags are carried from the source;
untagged sources are converted and tagged BT.709/tv. Audio, subtitles and
metadata are remuxed from the source.

Before frame 0 a long input costs a probe, a keyframe scan (packet headers
only, cached in `<input>.<ext>.d2xkf`), the model load, GPU init and one
full-frame upscale; each is reported as a stage so the shell is never silent.

## Numbers and where the time goes

- The engine is 8-bit RGB. 10-bit sources are dithered to 8 on decode; the
  output is 10-bit AV1 carrying 8-bit-precision content. Real-ESRGAN is
  8-bit-trained, so only reused blocks would gain from a 16-bit path.
- RRDB x2 at 1080p is ~9 TMAC per full frame. On an M2-class GPU that is an
  overnight batch for an episode even with reuse; the reuse fraction and
  the per-frame reports are the levers to watch before committing hours.
- Output is 2× the input, so a 1080p source encodes at 4K; with the fast
  model the encoder is the bottleneck, which is what `-j` is for.
- Tile overlap for same-pad nets is counted in *feature* pixels: x2 RRDB
  pixel-unshuffles, so 16 input px of overlap is 8. Big tiles (default 400)
  keep seams around 1 LSB.

## Asahi / Honeykrisp

Rules from `maintenance/ASAHI_GPU.md` and `ASAHI_VULKAN_LESSONS.md` are
followed: the adapter's own limits are requested, no subgroup ops, no
workgroup memory, no dynamic shifts in WGSL, dispatch dimensions checked
against the 65535 limit, software adapters named out loud (`--require-hw`
refuses them). `mesa-vulkan-drivers` from the asahi COPR; `vulkaninfo
--summary` should list Honeykrisp; `dandere2x probe` shows what wgpu sees.

## Verified

- `d2x-core`: 38 tests. Residual/merge exact round trips at bleed 0/1/3 and
  scale 1/2; RRDB x2 CPU output exact against a numpy reference of the
  PyTorch semantics (`models/depth_full.png`); SRVGG within float noise.
- `d2x-wgpu`: GPU bit-exact (100 %, worst diff 0) against the CPU reference
  on lavapipe for synthetic upconv_7, animevideov3 and RealESRGAN_x2.
- End to end on a synthetic 10-bit AV1 clip with audio, single pipeline and
  `-j 2/3`, output frame counts and keyframe cadence checked.
- Not verified: anything on Honeykrisp / M-series; the egui shell has only
  been compiled and linked, not run (no display here).

## Next

- Flat-block bypass (bicubic for near-zero-variance new content), neighbour
  motion-vector predictors, gate-margin temporal blend on residual blocks.
- Multi-frame reference ring (cut-backs reuse an earlier shot's output).
- Async tile pipelining in the runner (several tiles in flight, double-buffered
  readback); Winograd F(2,3) for the conv kernel.
- Distillation of a shallow net from the full RRDB on your own clips.
