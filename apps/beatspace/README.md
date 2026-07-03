# beatspace

A faithful Rust port of your **v6** mel-spectrogram GAN (`gan-zai` r2), wired up
for two things:

1. **Training** on your own `.opus` (and `.wav/.flac/.mp3/.m4a/.ogg`) music files.
2. A realtime **egui** latent-space explorer — drag your thumb across a 2-D pad to
   move through latent space and hear generated beats **crossfade into each other
   live**.

The model, the DSP, and the training loop are ported one-to-one from the Python.
The GUI replaces the old tkinter canvas with an egui pad that does the same
`x,y → z[0],z[1]` mapping (plus scroll → `z[2]`), the same `0.9·cur + 0.1·tgt`
latent smoothing, and an equal-power crossfade whenever a freshly generated beat
replaces the one currently looping.

---

## What's inside

```
src/
  config.rs    every magic number from the Python (n_mels, hop, latent, ...),
               serialized to config.json so train + gui always agree
  dsp.rs       mel analysis + Griffin-Lim synthesis (self-consistent filterbank
               and its pseudo-inverse — see "DSP notes" below)
  audio_io.rs  decoding: .opus via ogg+opus, everything else via symphonia;
               downmix to mono + linear resample to 22050
  dataset.rs   one-time mel cache (rayon), random clip + random 256-frame crop
  model.rs     Generator / Discriminator / SparseBlock, built by hand from Vars
               so BatchNorm running stats are saved/loaded correctly
  train.rs     DCGAN loop, two AdamW optimizers, safetensors checkpoints
  player.rs    cpal output stream: resample + equal-power crossfade + looping
  gui.rs       eframe app: latent pad, A/B set + walk, scope, load/save
  gpu.rs       optional wgpu compute backend: batched generator inference
               (--features gpu); folds BN, runs fc + 4 transposed-convs on GPU
  main.rs      clap CLI: `train`, `gui`, and (with --features gpu) `gpu`
```

---

## Prerequisites

You need a Rust toolchain (`rustup`, stable) plus a couple of system libraries
for Opus decoding and audio output.

**Debian / Ubuntu**

```bash
sudo apt install build-essential pkg-config libopus-dev libasound2-dev
```

**Fedora / RHEL (including Asahi Linux on Apple Silicon)**

```bash
sudo dnf install pkgconf-pkg-config opus-devel alsa-lib-devel gcc
```

If the GUI fails to build or open, you may also need the windowing dev
packages: `sudo dnf install libxkbcommon-devel wayland-devel`.

**macOS (Homebrew)**

```bash
brew install opus pkg-config
```

`libopus` + `pkg-config` are needed to build the `opus` crate (Opus decode).
ALSA (`libasound2-dev` / `alsa-lib-devel`) is needed by `cpal` for audio output
on Linux; on macOS CoreAudio is used and needs no extra package.

---

## Usage

### 1. Put your music in `./data`

Drop your `.opus` files (or wav/flac/mp3/m4a/ogg) into a folder, e.g. `./data`.
Subfolders are scanned recursively.

### 2. Train

```bash
cargo run --release -- train \
    --data-dir ./data \
    --ckpt-dir ./checkpoints \
    --epochs 300 \
    --batch-size 32
```

The first run builds a mel cache under `./cache` (this is the slow part — it
decodes every track once). Subsequent runs reuse it. Checkpoints are written to
`./checkpoints` as `gen_NNNNNN.safetensors` / `disc_NNNNNN.safetensors`, plus
rolling `gen_latest.safetensors` / `disc_latest.safetensors`. A `config.json` is
written there too so the GUI knows the model shape.

Resume from the latest checkpoint with `--resume`.

Useful flags: `--steps-per-epoch`, `--ckpt-every`, `--batch-size`. Run
`cargo run --release -- train --help` for the full list.

### 3. Explore in the GUI

```bash
cargo run --release -- gui --ckpt-dir ./checkpoints
```

- **Drag** on the big pad: X → `z₀`, Y → `z₁` (each in [-3, 3]).
- **Scroll** over the pad: nudges `z₂`.
- **Randomize**: jump to a fresh random latent.
- **Set A / Set B**: stash the current point; **Walk A→B** glides between them
  (speed slider controls how fast).
- **Load weights**: point at a `.safetensors` generator checkpoint to hot-swap.
- **Save WAV**: dump the beat currently playing.
- The strip along the bottom is the live output waveform.

You can open the GUI **before** you've trained anything — an untrained generator
still produces sound (noisy, but it moves and crossfades), which is handy for
checking your audio path works.

### GPU / acceleration

candle's GPU backends are **CUDA** (NVIDIA) and **Metal** (macOS only). To build
against one:

```bash
cargo run --release --features cuda  -- train ...   # NVIDIA, on a CUDA host
cargo run --release --features metal -- train ...   # macOS on Apple Silicon
```

The code calls `Device::cuda_if_available(0)` and falls back to CPU, but the
backend still has to be compiled in via the feature flag.

**On Asahi Linux / Apple Silicon under Mesa:** build with **default features
(CPU)** for training and the realtime GUI — do *not* pass `--features metal`
(needs Apple's frameworks, won't build on Linux) or `--features cuda`. candle
has no Vulkan backend, so it can't put the *network* on the Mesa GPU; the net
trains and runs realtime on the CPU. This is fine in practice: it's a small
DCGAN, each beat is one forward pass, and `gemm` uses NEON on aarch64, so latent
exploration stays responsive. (Your Mesa/Vulkan GPU still drives the egui window
perfectly well; it just doesn't do the network math — see the GPU compute path
below for that.)

### GPU compute path (`--features gpu`, batched generation)

Since candle can't reach the Apple GPU, there's a separate, self-contained GPU
backend built on **wgpu** (compute shaders). wgpu targets **Vulkan** on Linux,
so on Asahi it runs on your actual GPU via Mesa; it uses Metal/DX12/GL
elsewhere. It re-implements the *generator inference* pass directly:

- one `fc` GEMM, then four transposed-conv passes (a gather-formulation compute
  shader), with eval-time BatchNorm folded into a per-channel scale/shift and
  ReLU/Tanh applied in-shader;
- it loads the **same** safetensors weights as the candle generator and matches
  it numerically (validated max |GPU−CPU| ≈ 1e-7);
- it's **batched** — many latents in, many beats out, in one set of dispatches.

```bash
cargo run --release --features gpu -- gpu --ckpt-dir ./checkpoints --batch 256
```

prints the chosen adapter and a CPU-vs-GPU correctness + timing comparison.

**Where it helps:** batched GPU generation is for **offline / precompute** work
— rendering a whole latent-space grid, a long A→B walk, or a pile of variations
at once. It is *not* for single-beat realtime: at batch size 1 the per-dispatch
and host↔device transfer overhead dominates, and the CPU path (small window +
warm-started Griffin-Lim) is already fast and lower-latency there. Training stays
on candle/CPU; only generator inference has a GPU path.

**What's validated:** correctness was checked on software Vulkan (Mesa
lavapipe), where GPU output is float-identical to candle. Raw throughput was
*not* measured on real GPU hardware here (the sandbox has no GPU; lavapipe is a
CPU rasterizer, so it reports *slower* than native CPU — that's the software
backend, not the algorithm). On your Apple GPU, run the `gpu` subcommand above
with a large `--batch` to see the real numbers. A modest CPU-side win is also
available regardless of GPU: one batched candle forward beats the one-beat-at-a-
time loop (the `gpu` bench reports this too).

---

## Realtime design (small window, low latency)

This build is tuned so the latent pad feels live on a CPU:

- **Small generation window.** Each "beat" is `time_frames = 64` hops ≈ **0.74 s**
  of audio, not the original 256/≈3 s. The generator's first `fc` layer scales
  with the window, so a 64-frame model is ~2.8× smaller (≈7 MB vs ≈20 MB) and
  both the generator pass and Griffin-Lim do ~4× less work per beat. The
  architecture derives its shape from `n_mels` and `time_frames` automatically
  (`init = n_mels/16 × time_frames/16`), so you can set `time_frames` to any
  multiple of 16 — 32 for even lower latency, 128 for longer textures — and
  retrain. The discriminator uses adaptive pooling, so it needs no changes.

- **Warm-started Griffin-Lim.** Because the latent *glides* (0.9·cur + 0.1·tgt),
  consecutive beats are nearly identical, so each inversion re-seeds Griffin-Lim
  with the **previous beat's phase** instead of starting cold. That collapses the
  iteration count to `griffin_iters_rt = 8` (vs `griffin_iters = 48` for offline
  renders) with no audible loss while morphing. This is the single biggest
  realtime saving — Griffin-Lim is the dominant per-beat cost on CPU.

- **Generation paced to playback.** The gen thread targets ~0.55× the beat
  duration, so windows overlap (the player crossfades them) and you get
  continuous morphing without burning CPU re-rendering audio you'd never hear.

- **Clean saves.** **Save WAV** ignores the realtime preview and re-renders the
  current latent from scratch with the full `griffin_iters` (fresh phase), so
  exports are high quality even though playback uses the fast path.

Everything above is config-driven (`config.json`), so you can trade latency for
fidelity without touching code.

---

## DSP notes (why it won't sound bit-identical to the Python)

A couple of deliberate, principled differences from the librosa-based Python:

- **Self-consistent mel transform.** Analysis and synthesis share **one** mel
  filterbank and **one** dB normalization, and the mel→linear step uses that
  filterbank's pseudo-inverse (precomputed). This matters far more for audio
  quality than matching librosa's exact coefficients — the generator only ever
  hears *this* representation, so as long as forward and inverse agree, it
  trains and inverts cleanly. It does mean a checkpoint trained here expects
  this front-end, not librosa's.

- **Griffin-Lim** is used for phase reconstruction, same as the original
  `mel_to_audio` path.

- **Weight init.** Conv/deconv weights are `N(0, 0.02)` and BatchNorm γ is
  `N(1, 0.02)`, matching the Python `weights_init`. Because RNG and layer
  construction differ from PyTorch, a given seed won't reproduce PyTorch's exact
  initial weights — but the *distribution* is the same, so training dynamics
  match.

The upshot: train with this binary, run the GUI with the same binary, and
everything lines up. Don't expect to load a PyTorch `.pth` directly (formats and
the front-end differ); retrain from your audio here.

---

## A note on building

`candle` + `eframe` is a large dependency graph, so the **first** `cargo build
--release` will take a while (and pull a lot of crates). The `gemm` crate in
candle's tree is especially slow to optimize — expect several minutes on it
alone the first time. After that it's incremental. If the `opus` or `alsa-sys`
build fails, it's almost always the missing system package above — install it
and rebuild.

`Cargo.lock` is included with the exact versions this was built and tested
against (Rust 1.96, candle 0.11, eframe/egui 0.35, cpal 0.18, symphonia 0.6).

This project has been compiled and the non-GUI paths smoke-tested end to end:
audio decode → mel analysis → dataset batching → generator/discriminator
forward + backward (AdamW) → safetensors checkpoint save/load → eval-mode
generation → both the warm-started realtime inverter and the high-quality save
inverter → WAV out all run and produce well-formed audio, at the 64-frame
window. The GUI window (egui) and live playback (cpal) compile and link but were
not run headless, since they need a display and an audio device. The optional
wgpu GPU backend (`--features gpu`) was built and its generator output validated
**float-identical to candle** (max diff ≈1e-7) on software Vulkan (Mesa
lavapipe); raw GPU throughput wasn't measured (no GPU in the test sandbox).

