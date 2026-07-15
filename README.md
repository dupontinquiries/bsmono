# kBM3 — AI music generation, end to end

A GAN that generates trap-flavoured music as mel spectrograms, a neural vocoder
that turns those spectrograms into audio, a browser-based latent-space explorer
to steer it, and a Rust/Vulkan port so the whole thing trains on Apple Silicon
GPUs under Asahi Linux — where PyTorch has no GPU backend at all.

```
  latent z ──▶ [ GAN generator ] ──▶ mel spectrogram ──▶ [ vocoder ] ──▶ audio
                                            │
                              audio ──▶ mel cache (training data)
                                            ▲
                              explorer  ◀───┘  (steer z, watch the mel, hear it)
```

## Components

| Path         | What it is | Language |
|--------------|------------|----------|
| `pipeline/`  | The music GAN: sharded-SQLite mel cache (stable hash-based sharding), DCGAN generator/discriminator, resumable iteration-based training with health metrics, generator EMA, progress plots, a BPM/key-aware augmentation module, and a cache diagnostic. | Python / PyTorch |
| `vocoder/`   | HiFi-GAN-style neural vocoder that inverts the pipeline's exact mel representation (mel → waveform), replacing Griffin-Lim. | Python / PyTorch |
| `gpu/`       | Rust port of both trainers (GAN + vocoder) on the Burn framework's **Vulkan** backend, so training runs on the Apple GPU with no CUDA. Reads the same SQLite cache. | Rust / Burn |
| `explorer/`  | Web-based latent-space explorer — live mel spectrogram, 2D navigator, latent sliders, A/B morph, presets. Runs standalone (demo synth) or against a model server. | HTML/JS + FastAPI |

Each directory has its own README with details and exact commands.

## Quickstart

**Train the GAN (CPU or GPU).**
```bash
cd pipeline
python gan-zai-v5.3.py train --data ./music --batch 32 --workers 4
python gan-zai-v5.3.py plot  --log checkpoints/train_log.csv --out progress.png
```
> v5.3 fixed two real bugs found from actual training runs:
> 1. **Cache sharding** — files could silently get orphaned from their shard
>    whenever the library size changed (5535 mels built, only 740 "valid").
>    Delete `cache/` and rebuild once; `diagnose_cache.py` confirms if you need to.
> 2. **Discriminator gradient death** — a hard activation-sparsity threshold in
>    `SparseBlock` had exactly zero gradient below threshold, and could (and
>    did, in a real run) collapse the whole discriminator into a permanently
>    frozen, input-independent state after ~150 iterations: identical
>    D/G/D(real)/D(fake) logged for 40,000+ iterations straight. Fixed with a
>    smooth sigmoid gate that's never exactly zero-gradient.
>
> Both mean **old checkpoints aren't worth resuming from** — start a fresh run
> rather than `--resume`.

**Explore what it learned.** Open `explorer/index.html` for the standalone demo,
or serve a checkpoint for real output:
```bash
cd explorer
pip install fastapi uvicorn torch librosa soundfile numpy
python server.py --weights ../pipeline/checkpoints/gen_8000.pt \
                 --vocoder ../vocoder/vocoder_ckpt/voc_100000.pt   # optional
# open http://localhost:8000
```

**Diagnose a cache/valid-tracks mismatch** (read-only, no rebuild needed):
```bash
cd pipeline
python diagnose_cache.py --cache cache
```

**Multiply the effective dataset** by blending tracks with matching tempo/key
(only blends musically-compatible pairs, not random tracks):
```bash
cd pipeline
python augment.py features --cache cache   # one-time BPM/key pass, resumable
python augment.py groups   --cache cache   # see how many tracks got a blend partner
```
Then wrap your dataset in training: `AugmentedDataset(OpusMelDataset(cfg), cache_dir=cfg.cache_dir, blend_prob=0.35)`.


```bash
cd vocoder
python vocoder.py train --data ./music --iters 200000 --batch 16
```

**Train on the Apple GPU (Asahi Fedora Remix, Mesa Vulkan).**
```bash
cd gpu
WGPU_BACKEND=vulkan cargo run --release --bin beatspace-gpu -- train --cache ../pipeline/cache
WGPU_BACKEND=vulkan cargo run --release --bin vocoder       -- train
```

## The GPU story

The training log that started this all said `No GPU backend found` because
PyTorch has no path to the Apple GPU on Linux — no CUDA, no ROCm, and MPS is
macOS-only. But Asahi ships conformant **Vulkan 1.3/1.4** (Mesa "Honeykrisp")
and **OpenCL 3.0** (rusticl). The `gpu/` crate uses Burn's Vulkan backend to run
both models on that GPU. Both trainers are verified running through a real Vulkan
ICD (lavapipe in CI; Honeykrisp on the target machine). See `gpu/README.md`.

## Status

Everything here compiles and has been exercised end-to-end on synthetic or small
inputs (shapes, a training step, checkpoint save/resume, inference). What each
piece still needs from you: a music library to build the cache, real training
time (ideally on the GPU), and — for the Rust vocoder — an audio→(mel, wav)
loader to feed it real waveforms (the pattern is in `gpu/src/data.rs`).

## Repo layout

```
kbm3/
├── pipeline/   gan-zai-v5{,.1,.2,.3}.py   (v5.3 is current)   + docs/training-curves.png
├── vocoder/    vocoder.py, README.md
├── gpu/        Cargo.toml, src/{main,model,data}.rs, src/bin/vocoder.rs, tools/mel2audio.py
└── explorer/   index.html, server.py, README.md, docs/spectrogram-preview.png
```

## License

No license is set. Add one (e.g. `LICENSE`) before sharing publicly if you want
to define how others may use it.
