# beatspace-gpu — GAN training on Apple Silicon via Vulkan (no CUDA)

This is the GPU port of the kBM3 music GAN for **Asahi Fedora Remix**. It trains the
exact v5 generator/discriminator on the Apple GPU through **Mesa's conformant Vulkan
driver** ("Honeykrisp"), using the **Burn** ML framework's `wgpu`/Vulkan backend.

See **[ASAHI_NOTES.md](ASAHI_NOTES.md)** for AGX/Honeykrisp-specific findings
(single queue family, kernel-cache vs. fusion, coalesced memory access) pulled
from real hardware testing on this monorepo's other Vulkan/Asahi projects.

## Why this exists (the GPU situation on Asahi)

Your training log said `No GPU backend found` because **PyTorch has no backend for the
Apple GPU on Linux** — there's no CUDA (that's NVIDIA), no ROCm (that's AMD), and
PyTorch's Apple path (MPS) is macOS-Metal only. So on Asahi, PyTorch is CPU-only, full stop.

What your hardware *does* have, as of 2026, are two conformant GPGPU stacks from Mesa:

- **Vulkan 1.3/1.4** (the "Honeykrisp" driver) — conformant, upstream in Mesa.
- **OpenCL 3.0** (via rusticl) — conformant.

Compute shaders are first-class on both. The clean way to reach them from a training
framework is Vulkan compute, and the most mature Rust route is **Burn on wgpu/Vulkan**,
which composes with autodiff for training. (candle — used in the earlier `beatspace`
port — only has CUDA + macOS-Metal backends, so on Asahi it's CPU-only. That's why this
port switches to Burn.)

## Status — what's verified

Built with **Rust 1.96 + Burn 0.21**. Compiled clean (0 warnings) and **run end-to-end
through a real Vulkan ICD** — both the forward path and a full autodiff training step.

Forward (`smoke`):
```
Backend: Wgpu (Vulkan). Device: DefaultDevice
G(z)    -> [4, 1, 128, 256]     # matches the mel target exactly
D(G(z)) -> [4]
```

Full training step (`train`, forward G/D + BCE-with-logits + autodiff backward + Adam for
both nets, at reduced 32x32 resolution so it fits on a CPU box):
```
[0/4] lossD: 1.4272 lossG: 0.6414 | D(real): 0.57 D(fake): 0.57  (185665 ms/it)
[3/4] lossD: 1.4281 lossG: 0.6330 | D(real): 0.56 D(fake): 0.56  ( 53428 ms/it avg)
Done in 213.7s.
```
Note the timing: iteration 0 spends ~185 s compiling every SPIR-V kernel; iterations 1–3
run in ~9 s each from cache — a ~20x speedup once compiled.

Verification here used Mesa **lavapipe** (software Vulkan on CPU) because this build box
has no Apple GPU — but lavapipe exercises the *identical* Vulkan code path (naga → SPIR-V
→ Vulkan) that Honeykrisp runs on your GPU. On real hardware those kernels run on the GPU
(hundreds of times faster than llvmpipe), and the one-time compile is quicker too. Set
`BS_NMELS`/`BS_TF`/`BS_GF`/`BS_DF` to shrink the model for quick checks; unset for the real
128x256.

## Build & run on Asahi Fedora Remix

```bash
# Toolchain + Vulkan runtime (Honeykrisp ships with the distro's Mesa)
sudo dnf install rust cargo vulkan-loader vulkan-tools

# Opus decode (vocoder's real-data path) links against libopus via audiopus_sys
sudo dnf install libopus-devel   # Debian/Ubuntu: apt install libopus-dev

# Confirm the Apple GPU is the Vulkan device (NOT llvmpipe):
vulkaninfo --summary | grep -iE 'deviceName|driverName'
#   deviceName = Apple M<n> (G13x / G14x ...)
#   driverName = Honeykrisp

# Force the Vulkan backend and run (beatspace-gpu only ever READS a pre-built
# cache -- the Python pipeline is what builds cache/ from ./music):
WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- smoke   # shape + timing check
WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- train --cache ./cache --iters 20000 --batch 32
```

If `smoke` prints `driverName = llvmpipe`, Vulkan fell back to software — check that
`linux-asahi-edge` + Mesa are current and you rebooted after the last GPU update.

## What's here vs. what's next

**Everything for a real GPU training run is now in place and verified end-to-end on a
real Vulkan ICD** (lavapipe here; identical code path to Honeykrisp on your GPU):

- `src/model.rs` — Generator + Discriminator, architecturally identical to the PyTorch
  v5 model.
- `src/data.rs` — reads your **existing sharded SQLite mel cache** directly
  (`manifest.json` + `db-*.sqlite`, zlib-compressed row-major f32 in [-1,1]). No format
  change, so the 5,630-track cache is reused as-is. Verified: `Loaded cache: 20 valid
  tracks` from a cache built by the Python format.
- `src/main.rs` — Vulkan backend wiring; full DCGAN training step (forward, BCE-with-logits,
  autodiff `backward()`, Adam) reading real batches from the cache; **Burn checkpoints**
  (`gen_{step}.mpk`, `disc_{step}.mpk`) with `--resume`; and a `generate` command that
  writes generated mels as raw f32.
- `tools/mel2audio.py` — turns generated mels into wav via Griffin-Lim, reusing the exact
  inverse mapping from `gan-zai-v5.2.py`, so audio stays consistent across the Rust and
  Python halves.

Verified runs (32x32 test config so it fits on the CPU build box):
```
train  -> Loaded cache 'testcache': 20 valid tracks.
          [0/4] lossD: 1.4072 lossG: 0.7140 | D(real): 0.52 D(fake): 0.52
          checkpoint @ 2: gen_2.mpk, disc_2.mpk, sample_2_0.bin
          Done. Final: ck_test/gen_4.mpk
resume -> Resumed generator+discriminator from step 2.   [3/4] ...  Done.
generate -> Wrote mel tensors to samp_test/  (mel_0_32x32.bin, mel_1_32x32.bin)
```
The mel dumps read back correctly as f32 [-1,1] in numpy, and `tools/mel2audio.py` converts
them to wav.

## Three GPU trainers (full pipeline on Vulkan), all on real data

The pipeline's models train on the Apple GPU, plus the coevolutionary
population trainer — this crate builds three binaries sharing the same
Burn/Vulkan setup, and **all three now train on real data**, not synthetic
placeholders:

- **`beatspace-gpu`** (`src/main.rs`) — the music GAN. Reads the real
  SQLite mel cache via `src/data.rs`.
- **`vocoder`** (`src/bin/vocoder.rs`) — the HiFi-GAN-style neural vocoder
  (mel → waveform), the GPU port of `vocoder.py`. Since CubeCL has no FFT, its
  multi-resolution STFT loss is a differentiable **framing + windowing +
  DFT-matmul** — GPUs run matmuls well. Generator (Conv1d / ConvTranspose1d at
  8·8·2·2 = 256 = hop, dilated residual blocks), a 5-period discriminator, LS-GAN
  + feature-matching + multi-res STFT losses. Reads real audio directly via
  `src/audio.rs` (below).
- **`population`** (`src/bin/population.rs`) — the GPU port of
  `pipeline/population_trainer.py`. Maintains a population of generators and
  discriminators, cross-plays every pair each round (a full score matrix),
  and replaces weak members with a Gaussian-perturbed (mutated) copy of a
  random survivor, using a hand-written `ModuleMapper` (`AddNoise`) to perturb
  every parameter tensor in a cloned model — Burn's public mechanism for
  "apply this transform to every tensor in the module tree" (the same one
  the framework itself uses internally for things like `no_grad()`). Reuses
  `model.rs`'s `Generator`/`Discriminator` and `data.rs`'s `MelCache` directly,
  shared via `#[path = "..."] mod ...;` (binary targets in a lib-less crate
  don't share modules by default).

```bash
WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- train --cache ./cache --iters 20000 --batch 32
WGPU_BACKEND=vulkan cargo run --release --bin vocoder       -- train --data ./music --iters 200000
WGPU_BACKEND=vulkan cargo run --release --bin population    -- train --cache ./cache --pop 4 --rounds 200 --inner-iters 20
```

All three verified running through a real Vulkan ICD (lavapipe here; Honeykrisp
on your GPU) **against real files**, not synthetic tensors:
```
# vocoder, real .opus files, real STFT/mel losses:
RealVocoderData: 2 audio files under ./music
Training 3 iters, batch 2, on GPU (Vulkan). Real audio data.
[2/3] D:4.187 G:16.964 stft:4.519 adv:3.730 fm:0.968 (76976ms/it)

# population, real SQLite cache:
Loaded cache './cache': 30 valid tracks.
[round 1/2] G fitness best=0.470 mean=0.467 | D fitness best=0.003 mean=0.001 | replaced G[1,2] D[2,0]
```
`BS_MUTATE_FRAC`/`BS_MUTATION_STD` tune the population; `BS_NMELS/BS_TF/BS_GF/BS_DF`
(and the vocoder's `BS_CH/BS_SEG/BS_BATCH`) shrink models for quick checks —
unset for the real 128-mel config.

### `src/audio.rs` — real audio decode + mel computation, verified against librosa

Two things, both numerically verified against the actual Python pipeline
(element-wise diff against real librosa/vocoder.py output, not eyeballed):

- **Opus decode**: `audiopus` (libopus bindings) + `ogg` (container demux),
  the exact recipe already proven elsewhere in this monorepo at
  `games/unendlich/crates/unendlich-engine/src/audio.rs`. Opus decodes at a
  fixed 48kHz internally; resampled down to the pipeline's 22050Hz via linear
  interpolation. WAV via `hound`. **Requires `libopus-dev`/`libopus-devel`
  installed** (see build instructions above) since `audiopus_sys` links
  against system libopus.
- **Mel computation**: STFT (centered, zero-padded — librosa's *current*
  default `pad_mode='constant'`; an initial reflect-padding assumption was
  wrong and caught by comparing edge frames against real librosa output,
  where the error was concentrated) → Slaney-normalized mel filterbank
  (matches `librosa.filters.mel` to ~1e-9) → `power_to_db` with librosa's
  `top_db=80` relative-to-max clamp → the pipeline's fixed `[-80,40]`
  normalization to `[-1,1]`. Full pipeline matches Python's
  `wav_to_unit_mel` to ~1.3e-6 (float32 precision noise) across every frame,
  including the ones at the start/end of a clip.

### The sharding fix this also caught

While wiring `population.rs` to a real cache, `src/data.rs` turned out to
still be reading the **old, pre-v5.3 index-range shard naming**
(`db-SSS-EEE.sqlite`) — it was written before the Python pipeline's v5.3
sharding fix (crc32-hash-based shards, see `pipeline/gan-zai-v5.3.py`) and
was never updated to match. That means the Rust GAN trainer would have
silently failed to find anything (or read a stale old-format cache) against
your actual current cache. Fixed: `data.rs` now computes
`shard_id = crc32(path) % num_shards` (verified `crc32fast` matches Python's
`zlib.crc32` bit-for-bit) and reads `shard-NNN-of-032.sqlite`, matching the
real pipeline exactly. Both `beatspace-gpu` and `population` re-verified
against a real hash-sharded cache after the fix.

### One known remaining gap

Checkpoints save/load G, D, and the EMA generator, but **not** Adam's
optimizer state (momentum/variance) — matches the Python pipeline's own
documented tradeoff (`--resume` restores weights and restarts the
optimizers; Adam re-warms quickly in practice). Python's checkpoint format
does now include `optG`/`optD` state, so this is a real, if minor, parity
gap if exact bit-for-bit resume semantics matter to you.

## Full GAN run on your Apple GPU

```bash
# 1. Build the mel cache from a folder of audio. Either side can build it --
#    the on-disk format is identical (hash-sharded SQLite + manifest.json).

#    (a) In Rust, no Python needed -- decodes .opus/.wav, computes mels,
#        writes the cache. Fast now that the STFT uses a real FFT (rustfft):
#        seconds for a handful of tracks. Still use --release.
WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- \
    build-cache --data ./music-kitchen --cache ./cache-kitchen

#    (b) ...or with the Python pipeline, if you prefer:
# python gan-zai-v5.3.py train --data ./music --iters 1   # populates cache/

# 2. Train on the GPU (real 256x560 config; leave BS_* unset)
WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- train --cache ./cache-kitchen --iters 20000 --batch 32

# 3. Generate + listen
WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- generate --gen checkpoints_rs/gen_20000 --n 4 --out samples_rs
python tools/mel2audio.py samples_rs/*.bin
```

`build-cache` flags: `--data` (required, folder searched recursively for
`.opus`/`.wav`), `--cache` (output dir, default `cache`), `--num-shards`
(default 32, must match what training uses), and `--sr`/`--n-fft`/`--hop`
(default 48000/4096/512 — must match the audio config the models expect).
`n_mels`/`time_frames` come from the model config (the `BS_*` env vars) so the
cached mels are the right shape for training. Tracks too short to yield even
one `time_frames`-wide crop are skipped and reported.

**Cache size.** By default `build-cache` writes **int8** mels (~4.25× smaller
than f32 — measured, e.g. 278KB→65KB — at max error ½·(1/127)≈0.004, which is
negligible for GAN training since the random-crop augmentation varies more
than that). Pass `--f32-cache` to write raw f32 instead, byte-compatible with
the Python pipeline's caches. The format is self-describing: int8 blobs carry
a 2-byte magic header and the reader auto-detects it, so f32 caches (including
every Python-built one) still read unchanged.

**Troubleshooting "N valid tracks" much smaller than expected.** If the shard
`.sqlite` files are large (lots of MB) but training reports only a handful of
valid tracks, the cache's rows are *orphaned*: they were written under one
path form (e.g. absolute `/home/you/music/x.opus`) but `manifest.json` lists
another (e.g. relative `./music/x.opus`), so the validity lookup misses them.
This happens when the data dir moved or the manifest was rebuilt after a path
change. `build-cache` and training now print a WARNING when they detect this.
The fix is to rebuild the cache from the current data dir:
`build-cache --data <dir> --cache <dir>` — the manifest and row keys will then
agree. (Note: a cache with genuinely few tracks is fine to train on — the
loader samples with replacement — it just warns rather than refusing.)

**Always use `--release`** (`cargo run --release`, or `cargo build --release` +
run the `target/release/` binary directly). All commands elsewhere in this
README that omit `--release` are for quick shape/smoke checks only — see the
Performance section below for why this matters and by how much.

### Full CLI reference (`beatspace-gpu train`)

| flag | default | matches Python's |
|---|---|---|
| `--iters` | 20000 | `max_iters` |
| `--batch` | 32 | batch size |
| `--lr` | 2e-4 | `cfg.lr` |
| `--beta1` | 0.5 | `cfg.beta_1` |
| `--label-smooth` | 0.9 | `cfg.label_smooth` |
| `--log-every` | 50 | `cfg.log_every` |
| `--ckpt-every` | 2000 | `cfg.ckpt_every` |
| `--sample-every` | = `--ckpt-every` | `cfg.sample_every` (independently configurable) |
| `--ckpt-dir` | `checkpoints_rs` | `cfg.ckpt_dir` |
| `--cache` | `cache` | `cfg.cache_dir` |
| `--num-shards` | 32 | `cfg.num_shards` |
| `--no-ema` | (EMA on) | `cfg.use_ema` |
| `--ema-decay` | 0.999 | `cfg.ema_decay` |
| `--noise-sigma` | 0.1 | `cfg.instance_noise_sigma` |
| `--noise-iters` | `iters/5` | `cfg.instance_noise_iters` |
| `--resume <step>` | — | `--resume` |
| `--synthetic` | off | (Rust-only: skip the cache, train on random data) |

`checkpoints_rs/train_log.csv` uses the **exact same columns** as the Python
pipeline's log (`step,lossD,lossG,D_real,D_fake,noise_sigma,ms_per_it`), so
`python gan-zai-v5.3.py plot --log checkpoints_rs/train_log.csv` works
directly on Rust training runs.

### Feature parity with gan-zai-v5.3.py

Ported and verified (each with a real Vulkan run, not just a read-through):

- **SparseBlock discriminator** — depthwise-separable convs with the
  sigmoid-activation-sparsity gate (the *fixed* version; this port never had,
  and never will have, the original hard-threshold gate that caused v5.3's
  discriminator-gradient-death bug). ~8-10x fewer multiply-adds than a dense
  conv of the same channel count. Verified: forward shapes correct, and a
  real training step (forward + backward + optimizer step) runs cleanly —
  the exact place the original Python bug hid.
- **Generator EMA** — a shadow copy blended `decay*ema + (1-decay)*live` each
  step (with the same early-training warmup as Python), sampled and
  checkpointed instead of the raw generator. BatchNorm running stats are
  copied from the live model rather than blended, matching Python exactly.
  Implemented as explicit per-field blending in `model.rs` (Burn's
  `ModuleMapper` only walks one model's own tensors at a time, so it can't
  naturally pair two different models' aligned parameters — built-in Burn
  layers expose their params as plain `pub` fields, so direct field-level
  blending is the robust option). Verified: `gen_N.mpk` and `gen_ema_N.mpk`
  are provably different files (different weights) after training, and the
  EMA checkpoint reloads and produces a correct forward pass.
- **Instance noise** — Gaussian noise on the discriminator's inputs, linearly
  decaying to 0 over `--noise-iters`. Verified decaying correctly step by
  step in a real run.
- **Honest windowed timing** — `ms/it` is now the average since the *last*
  log line, not a cumulative average since training start (the same
  misleading-metric bug found and fixed in the Python pipeline).
- **CSV logging** — matches Python's `train_log.csv` format exactly (see
  above), so the existing `plot` tooling works unmodified on Rust runs.

## Backend feature flags (the biggest speed lever found so far)

Until now this crate built Burn with only `["std", "wgpu", "autodiff"]` — which
means **kernel fusion and autotuning were both OFF**. On a stack where the
measured bottleneck is per-iteration *dispatch overhead* (≈1241ms fixed per
iteration on M2 Ultra/Asahi, independent of batch size), that was leaving the
single most relevant optimization on the table.

Now enabled by default:

| feature | what it does | why it matters here |
|---|---|---|
| `fusion` | merges elementwise/reduction chains into single kernels | fewer dispatches = less driver overhead, which *is* the Asahi bottleneck |
| `autotune` | benchmarks kernel variants per-device, caches the winner | default kernel choices are tuned for NVIDIA; Honeykrisp/Mesa needs its own |

With `fusion` on, Burn's `Wgpu` alias automatically becomes
`Fusion<CubeBackend<..>>` — no code change was needed.

**Autotune has a large one-time cost.** Measured here: the first training
iteration took **179s** (vs ~2s steady-state) while it benchmarked variants.
Results cache in `target/autotune/`, so it's paid once per device/shape combo.
To shrink it:

```bash
export CUBECL_AUTOTUNE_LEVEL=minimal   # fewer variants benchmarked
```

### Opt-in: the SPIR-V backend (`--features spirv`)

Burn's own docs say the SPIR-V compiler backend *"performs significantly better
than WGSL"*, especially for matmul, and it's the only path that can reach
f16/TensorCore-class matmul. It is **opt-in**, not default:

```bash
cargo build --release --features spirv --bin beatspace-gpu
```

It's opt-in because it requires a real Vulkan adapter advertising SPIR-V
passthrough. On a software rasterizer (lavapipe) adapter selection fails hard
with `No possible adapter available for backend`, so defaulting it on would
break CPU-only and CI environments. **Honeykrisp is a real Vulkan 1.3 driver,
so this should work on the M2 Ultra — and it is the most promising untested
speed lever remaining.** Worth benchmarking first thing:

```bash
# baseline (fusion+autotune, WGSL)
cargo run --release --bin beatspace-gpu -- bench --batch 8 --warmup 10 --measure 50
# SPIR-V
cargo run --release --features spirv --bin beatspace-gpu -- bench --batch 8 --warmup 10 --measure 50
```

For reference, an unrelated Burn project (voxcpm-rs) measured **~2.6× from
native-Vulkan + bf16 over plain wgpu** on AMD RDNA4 through Mesa, and ~5–7%
from fusion+autotune alone. Different workload and hardware, so treat it as
"worth testing", not a promise. That project also needed small patches for
bf16 SPIR-V ops that Mesa's NIR translator mis-lowers — if SPIR-V misbehaves on
Asahi, that's the class of bug to suspect.

### Dispatch batching

```bash
export BURN_WGPU_MAX_TASKS=64   # how many compute tasks are batched per submit
```
Higher values mean fewer, larger submissions to the graphics API. Since the
diagnosed problem is per-submit overhead, this is worth sweeping (try 16 / 32 /
64 / 128) alongside the bench command.

## SOTA training objective: RpGAN (R3GAN)

`--loss rpgan` switches the adversarial objective from the classic BCE to the
**relativistic pairing loss** from *"The GAN is dead; long live the GAN! A
Modern Baseline GAN"* (Huang et al., NeurIPS 2024 — R3GAN).

The classic loss asks D "is this sample real?" — an absolute decision boundary.
RpGAN asks "is this real sample better than that fake one?", scoring a *pair*:

```
L_D = mean(softplus(-(D(real) - D(fake))))
L_G = mean(softplus( (D(real) - D(fake))))
```

That one change is what fixes **mode dropping**. On the StackedMNIST
1000-mode benchmark, the classic loss drops modes while RpGAN reaches full
coverage, cutting reverse-KL from 0.9270 to 0.0781. For a trap-music generator
— where the failure you'd actually hear is "every sample sounds like the same
loop" — mode coverage is the metric that matters.

Verification that the implementation is correct: at initialization D is
untrained, so `D(real) ≈ D(fake)`, the difference is ~0, and **both losses must
equal `softplus(0) = ln 2 = 0.6931`**. Observed: `lossD 0.6913, lossG 0.7068`.
The classic BCE path, which has two terms, must equal `2·ln2 = 1.3863` —
observed `1.4064`. Both match. The softplus uses the
`max(x,0) + log(1+exp(-|x|))` identity, verified stable to x=700 where the
naive form overflows.

Default stays `bce`, so existing runs are unchanged.

### Why R1/R2 are not in the Rust trainer

R3GAN's full recipe is RpGAN **+ R1 + R2** (zero-centered gradient penalties on
real *and* fake). The paper's central theoretical result is that RpGAN alone
still diverges, and R1 alone is *not* sufficient — you need both for proven
local convergence.

Those penalties need **second-order gradients**, and Burn's autodiff does not
provide them: `AutodiffBackend::grad()` returns an `InnerBackend` tensor, which
carries no autodiff tracking, so there is nothing to backpropagate through. The
type signature makes this unambiguous.

The tractable stand-in is **instance noise**, which this trainer already applies
(decaying sigma on D's inputs) — and that is not a hand-wave: Roth et al. (2017)
showed instance noise is theoretically equivalent in effect to the R1/R2
penalties, and R3GAN cites exactly that link. So the Rust path gets
RpGAN + instance-noise-as-regularizer; the Python pipeline, which has PyTorch's
double backward, gets the exact R1/R2 (see `--r1-gamma`, `--reg-every`).

## Performance: getting toward sub-1000ms/it

**Use `--release`.** Directly measured on this dev box (lavapipe, so absolute
numbers don't transfer, but the *comparison* is real): at a moderate config
(64 mels, 64 frames, 64/32 filters, batch 8), debug ran ~4100-4360ms/it
steady-state; release ran ~2700-2770ms/it — about **1.5x**. That's smaller
than the 5-30x release commonly gives CPU-bound Rust code, and the reason is
informative: most wall-clock time here is spent executing GPU kernels
(software-rasterized on lavapipe), not in CPU-side Rust orchestration code,
which is exactly what `--release` optimizes. In other words, the code itself
isn't the bottleneck -- which means the same should hold on real Honeykrisp
hardware: `--release` is free and worth taking, but the dominant lever for
sub-1000ms is real GPU throughput, not further micro-optimizing the Rust.

**What actually reduces GPU-side compute (already done):**
- SparseBlock's depthwise-separable structure (~8-10x fewer discriminator
  MACs vs. a dense conv stack) -- ported and verified above.
- Per Asahi/AGX findings in `ASAHI_NOTES.md`: don't try to parallelize across
  multiple command streams (single Vulkan queue family on this hardware,
  finding #1) -- bigger batches, not more concurrent dispatches, is the right
  lever if you want more GPU utilization per wall-clock second.

**How to actually profile on your hardware (`bench` command):**
```bash
# confirm you're really on the GPU, not a software fallback:
WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- smoke
#   -> Device should show your Apple GPU via Honeykrisp, not llvmpipe

# clean throughput measurement: runs the REAL training step (D+G fwd/bwd/step)
# on synthetic data, absorbs kernel-compile warmup, then reports percentiles.
# With BS_* unset it uses the full 256x560 production config -- i.e. it
# measures exactly what `train` runs.
WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- \
  bench --batch 8 --warmup 10 --measure 50
#   -> ms/it  min .. median .. p95 .. mean ..
#      throughput (median): N it/s  (M samples/s at batch 8)
```
`bench` reports min/median/p95/mean rather than a single running average, so
you get an honest picture including the tail, and directly comparable numbers
between machines (e.g. M4 Max vs M2 Ultra).

**Finding the memory-bandwidth knee (relevant for M4 Max vs M2 Ultra).** This
workload is largely memory-bandwidth-bound on Apple GPUs (see `ASAHI_NOTES.md`,
finding on coalesced access + single queue family). The M2 Ultra has roughly
2x the memory bandwidth of an M4 Max (~800 vs ~400 GB/s) despite being an older
architecture, so it may actually train this faster -- and the batch size where
throughput saturates will differ. Sweep it:
```bash
for b in 1 2 4 8 16 32; do
  echo "=== batch $b ==="
  WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- bench --batch $b --warmup 8 --measure 40
done
```
Watch `samples/s` (throughput): it climbs with batch size until the GPU
saturates, then flattens (or ms/it starts rising faster than batch). The knee
is your most efficient batch size. A higher-bandwidth GPU (M2 Ultra) should
push that knee to a larger batch and a higher peak samples/s.

If you're above 1000ms/it and need to get under it, the levers in order of
impact: (1) reduce `--batch` (near-linear compute reduction); (2) reduce
`g_filters`/`d_filters` (`BS_GF`/`BS_DF`) -- these scale compute roughly
quadratically per layer; (3) reconsider whether `256x560` is necessary vs. the
original `128x256` -- the audio-quality config bump traded speed for frequency
resolution, and that's a dial you can turn back if latency matters more than
resolution right now.

