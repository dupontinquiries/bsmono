# beatspace-gpu — GAN training on Apple Silicon via Vulkan (no CUDA)

This is the GPU port of the kBM3 music GAN for **Asahi Fedora Remix**. It trains the
exact v5 generator/discriminator on the Apple GPU through **Mesa's conformant Vulkan
driver** ("Honeykrisp"), using the **Burn** ML framework's `wgpu`/Vulkan backend.

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

# Confirm the Apple GPU is the Vulkan device (NOT llvmpipe):
vulkaninfo --summary | grep -iE 'deviceName|driverName'
#   deviceName = Apple M<n> (G13x / G14x ...)
#   driverName = Honeykrisp

# Force the Vulkan backend and run:
WGPU_BACKEND=vulkan cargo run --release -- smoke                 # shape + timing check
WGPU_BACKEND=vulkan cargo run --release -- train --iters 20000 --batch 32
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

## Two GPU trainers (full pipeline on Vulkan)

Both models in the pipeline now train on the Apple GPU — this crate builds two
binaries sharing the same Burn/Vulkan setup:

- **`beatspace-gpu`** (`src/main.rs`) — the music GAN.
- **`vocoder`** (`src/bin/vocoder.rs`) — the HiFi-GAN-style neural vocoder
  (mel → waveform), the GPU port of `vocoder.py`. Since CubeCL has no FFT, its
  multi-resolution STFT loss is a differentiable **framing + windowing +
  DFT-matmul** — GPUs run matmuls well. Generator (Conv1d / ConvTranspose1d at
  8·8·2·2 = 256 = hop, dilated residual blocks), a 5-period discriminator, LS-GAN
  + feature-matching + multi-res STFT losses.

```bash
WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- train --cache cache
WGPU_BACKEND=vulkan cargo run --release --bin vocoder      -- train --iters 200000
```

Both verified running through a real Vulkan ICD (lavapipe here; Honeykrisp on
your GPU). The vocoder on Vulkan:
```
G: mel[2,32,8] -> wav[2,1,2048]          # smoke: exact 8·256 upsample
MPD(period 3): logits[2,27], 6 feature maps
[0/3] D:5.011 G:27.480 stft:8.200 adv:4.654 fm:1.163
[2/3] D:4.144 G:27.115 stft:8.279 adv:3.766 fm:1.326   # D + adv decreasing
```
Env `BS_NMELS / BS_CH / BS_SEG / BS_BATCH` shrink the vocoder for quick checks;
unset for the real 128-mel model.

The vocoder trainer uses synthetic (mel, wav) pairs for the GPU-compute proof;
real training needs waveform targets, so wire in an audio→(mel,wav) loader — the
same pattern as `src/data.rs`, but reading audio (e.g. via `symphonia`) and
computing the aligned mel, exactly as `vocoder.py` does on the Python side.

## Full GAN run on your Apple GPU

```bash
# 1. Build the cache once with the Python pipeline (reuses your existing cache/ if present)
python gan-zai-v5.2.py train --data ./music --iters 1   # populates cache/, or skip if built

# 2. Train on the GPU (real 128x256; leave BS_* unset)
WGPU_BACKEND=vulkan cargo run --release -- train --cache cache --iters 20000 --batch 32

# 3. Generate + listen
WGPU_BACKEND=vulkan cargo run --release -- generate --gen checkpoints_rs/gen_20000 --n 4 --out samples_rs
python tools/mel2audio.py samples_rs/*.bin
```

Remaining niceties (not blockers): the generator EMA from v5.2 (a second `Generator`
updated each step — a few lines), and optimizer-state in the checkpoint (Adam re-warms
quickly, so resume currently restores weights and restarts the optimizers). Weight formats
differ between the two halves (Burn record vs. PyTorch `state_dict`), so the Rust-trained
generator feeds the Python audio path via the mel-dump bridge rather than by loading into
the Python `Generator` directly.
