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
dandere2x upscale  -i in.mkv -o out.mkv --model RealESRGAN_x2.safetensors --cascade default --flat-bypass
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

## CPU hot paths

Checked in the post-LTO binary (`objdump`), not the pre-LTO `.s` — thin LTO
inlines `block_sse` into its callers, and there it vectorises (`pmaddwd` on
SSE2, `smull`/`sabd` on NEON). What did not vectorise were the per-pixel
index-arithmetic loops around the GPU; those are now row-wise with narrow
integer reductions. `cargo test --release -p d2x-core --test bench_hot --
--ignored --nocapture` prints per-call timings; on one SSE2 core:

| path | before | after |
|---|---|---|
| `padded_to` replicate, 1080p +16 | 4.4 ms | 1.6 ms |
| `add_uniform_noise`, 1080p | 13.7 ms | 3.2 ms |
| `extract_input`, 400-tile (u=1 / u=2) | 1.1 / 1.3 ms | 0.25 / 0.63 ms |
| `write_output_rgba8`, 400-tile ×4 | 2.0 ms | 1.6 ms |
| `block_sse` ×2304, `hash_cell` ×2304 | ~2 ms | unchanged (already packed) |

The JPEG gate now encodes the frame in 16-row-aligned strips across the
rayon pool; JPEG blocks reconstruct independently, so the threshold map is
bit-identical to the single-strip result (tested at 2/3/5/8 threads) and
the largest single stage-A cost scales with cores. No `target-cpu` flags:
NEON is baseline on aarch64, and fleet-wide `fullfp16` is the SIGILL the
lessons doc warns about; on x86 hosts `RUSTFLAGS=-C target-cpu=native` is
a per-machine choice that roughly doubles the SSE paths.

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

## Throughput notes (multi-hour footage)

What is built:

- **Reference ring.** Missing blocks are checked at the same position in the
  last `--refs` outputs and the last `--keyframe-refs` shot starts before
  going to the network; a hit is a copy (`rec` in the per-frame line).
  Occlusions, flicker, and cut-backs stop costing CNN time. One bounded SSE
  per reference per missing block, so a deep ring is nearly free in CPU and
  ~25 MB per reference at 4K in memory.
- **Rectangular residual packing** (`cols = ceil(sqrt(n))`): wastes at most
  `cols − 1` cells instead of up to `2·sqrt(n)`.
- **Cross-frame residual batching** (`--batch`, and across `-j` segments via
  the GPU server): small residuals are packed into one canvas per call. A
  residual of a few cells is a handful of workgroups; below ~150 px on a side
  the device is underoccupied and per-dispatch overhead (RRDB is ~400
  dispatches per tile) dominates. Above that the network already fills the
  GPU, so only clearly small items (< canvas²/8) are packed — the replicate
  gutter costs 17–46 % extra pixels and only pays where the GPU is idle
  anyway. On lavapipe this is a net loss (CPU rasterizer, pixel-
  proportional); the gain exists only on a real GPU and is bounded by how
  much time the job spends on tiny residuals.
- **Keyframe-aligned segments** (`-j`): N encoders, one shared GPU.
- **Resume** (`--resume`, with `--keep-parts` on the first run): each finished
  segment writes a sidecar stamped with every setting that affects pixels or
  timing — model, kernel, scale, gate, cascade, matcher, codec, crf, segment
  count. A rerun reuses only parts whose stamp matches, so an interrupted
  multi-hour job costs one segment rather than the whole night, and changing
  `--quality` (or anything else) correctly invalidates everything. A part
  without its sidecar is treated as a truncated write and recomputed.
- **Difficulty cascade** (`--cascade default|model.safetensors`,
  `--flat-bypass`): missing blocks are classed by luma standard deviation.
  Flat blocks (< `--flat-std`, default 2) get bicubic; smooth blocks
  (< `--smooth-std`, default 8) get the compact network (~7× cheaper than
  RRDB x2); only textured blocks pay for the main model. Each tier is its own
  packed residual image and its own (batched) upscaler; `analyze` reports
  the main-network share separately, which is what RRDB time follows. On the
  synthetic partial-motion clip in `tools/` this cut the main-network share
  from 50 % to 36 % of pixels; photo footage has a far larger smooth share.
  Seams between tiers are possible on blocks near a threshold — raise
  `--smooth-std` cautiously and look.
- **Memory budgets** (`--ram-budget`, `--vram-budget`; default half of
  `MemAvailable`, a quarter of that for the GPU — gpu-gemm's memplan
  convention). The RAM budget sizes the reference rings (40 %, split across
  `-j` workers: up to 32 recent, the rest shot starts up to 4096) and the
  block cache (20 %), leaving headroom for decoder, encoder and queues.
  `--refs/--keyframe-refs/--block-cache` override. With 192 GB that is a
  ring spanning the whole film's shot starts, so any cut-back is a copy.
- **Block cache** (`cache` column): a cell whose exact bytes (block + bleed
  ring) were upscaled before — anywhere in the frame, any time in the job —
  is served from memory: static overlays, letterbox, black, repeated UI,
  animation cycles, a moving logo. Exact match only; FIFO under its byte
  budget; shared across `-j` workers. Hits lag the pipeline depth (stage A
  analyses ahead of stage B's inserts), so a repeat two frames later misses
  and one twenty frames later hits. On photo footage the hit rate is
  content-dependent; the column tells you what it is on yours.
- **In-flight activation sets** (`--inflight N`, default 2): every GPU tier
  holds N sets of buffers and rotates tiles through them, waiting on each
  tile's own submission, so upload of tile k+1 and readback of tile k−1
  overlap compute of tile k. The GPU budget is split across tiers by
  per-pixel cost and across sets; tiles shrink to fit. All tiers share one
  device. Bit-exact against the CPU reference with three sets over
  five-tile frames on lavapipe; the overlap itself only shows on a real
  GPU.
- **Gate hysteresis** (`--streak-bonus b`, default 0 = off): a block reused
  for `n` consecutive frames gets its threshold scaled by
  `1 + b·min(n,8)/8`, so blocks that keep matching stop flickering out at
  the threshold. Drift is bounded at that multiple of the JPEG error.

**Kernel results on an M2 Ultra (Honeykrisp, RealESRGAN x2, tile 688, f32
peak ~13.6 TFLOPS).** Every variant is bit-exact against the CPU reference;
they differ only in how they spend registers and address arithmetic:

| kernel | GMAC/s | shape |
|---|---|---|
| rt1 | 661 | 1 px × 4 ch |
| rt4 | 683 | 4×4 px × 4 ch — pixel axis, ~130 registers |
| wino | 677 | Winograd F(2,3), 2.25× fewer multiplies |
| rt2 | 896 | 2×2 px × 4 ch |
| rt2c8 | 977–988 | 2×2 px × 8 ch — channel axis |
| **rt2f** | **1022** | rt2 + branch-duplicated interior fast path |
| rt2f (SRVGG, tile 1024) | 1403 | the compact net reaches 2.8 TFLOPS, ~21% of peak |
| rt2c8f | 803 | both of the above — the duplication spills |
| rt2c16f | 495 | 2×2 px × 16 ch — eight copies of the block |

The last two are the instructive failures: each win is real alone, but the
fast path was implemented as a duplicated branch, so composing it with
channel blocking put four (then eight) copies of the accumulator block in
one shader and the register allocator spilled. `rt2h` / `rt2c8h` redo it
structurally — the bounds test and in_scale divide are hoisted out of the
channel loop into per-thread row bases, column offsets and 0/1 masks, with a
single copy of the block.

Kernel findings that generalise past this app — the register/multiply
tradeoff on AGX, why Winograd loses, why in-flight overlap is a no-op on
unified memory — are written up for the gpu-gemm side in
[`docs/FOR_LOGSCOPE.md`](docs/FOR_LOGSCOPE.md), along with the two gpu-gemm
lessons dandere2x has not yet applied.

Dispatch-level lessons come from `apps/logscope/reference-docs/ENGINE_PERF_STATUS.md`,
which settled the same question for gpu-gemm: dispatch granularity, not
arithmetic. Params uniforms are created once and (with balanced tiling) have
identical dims across tiles; per-tile rewriting is the mistake that doc calls
the dims-uniform cache.

Winograd cutting multiplies by 2.25× and *losing* is the useful result: the
kernel is not multiply-bound. The inverted U peaking at 2×2 says registers
are the binding resource, and the channel axis is the one way to raise
arithmetic per load without spending input registers. `rt2c8f` (channel
blocking + a branchless interior path that drops the per-scalar bounds test
and in_scale divide) is the current default; `rt2c16f` pushes the channel
axis further. Tile size and in-flight sets are settled: flat from 400 to 688,
and extra sets only steal VRAM from tile size.

**Measuring the kernel on your GPU** — `dandere2x bench --model X --vram-budget 8G`
runs both conv kernels on full-size tiles and prints ms/tile, GMAC/s, TFLOPS,
and the implied seconds per full 1080p frame. That number against the
device's peak (M2 Max ≈ 13.6, M2 Ultra ≈ 27 TFLOPS f32) is the kernel's
efficiency; `analyze` gives the reuse fraction; the product of the two is the
job. Kernels: `rt4` (default: 4×4 px × 4 ch per thread, ~2.9× the FMAs per
load of `rt2`) and `rt2`; both bit-exact against the CPU reference, pick by
`bench`. With a VRAM budget the tile auto-sizes up to 1024 (a body conv on a
400-px tile is ~1.3 waves on 76 cores) and tiles are balanced (1920 →
5 × 384, never 4 × 400 + 320).

What would move the big number further (RRDB is ~9 TMAC per 1080p frame; the
conv kernel is all of it):

- **Winograd F(2×2, 3×3)**: 2.25× fewer multiplies for every 3×3 same-pad
  conv, exact in f32. Transform-domain batched GEMM is a kernel the tree
  already has form for (gpu-gemm).
- **f16 storage** (f32 math) halves activation bandwidth where the adapter
  offers `SHADER_F16`.
- **Gate**: the JPEG round trip could be replaced by a DCT-domain
  quantisation-error estimate (same number by Parseval, ~10× cheaper,
  library-independent) if analysis ever becomes the bottleneck under `-j`.
- **Distillation** of a shallow net from the full RRDB on your own clips.

## Next

- Flat-block bypass (bicubic for near-zero-variance new content), neighbour
  motion-vector predictors, gate-margin temporal blend on residual blocks.
- Multi-frame reference ring (cut-backs reuse an earlier shot's output).
- Async tile pipelining in the runner (several tiles in flight, double-buffered
  readback); Winograd F(2,3) for the conv kernel.
- Distillation of a shallow net from the full RRDB on your own clips.
