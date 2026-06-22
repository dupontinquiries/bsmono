# AV1 Batch Tuner — design

A reliable, friendly GUI for tuning and batch-encoding AV1, built on 
SVT-AV1 and rav1e. Successor in spirit to av1ator (which produced good 
results but crashed) and av1an (excellent CLI, no GUI).

## Goals

- Reliable on multi-hour encodes (no losing work to crashes)
- Interactive tuning feedback (see results in seconds, not minutes)
- Batch queue with share/reuse across jobs
- Comfortable to hack on internals (rate control, motion search, GPU)
- Native Linux, ARM-friendly (Asahi target)

## Non-goals

- Beating AV1 at compression (Version A acceptance: AV1 wins; we tune)
- Custom bitstream extensions (stay compatible with all AV1 decoders)
- Cross-platform polish at launch (Linux first; mac/Windows later)
- Hardware encode (Linux has no hw AV1 encode access on Apple Silicon)

## Stack

| Layer        | Choice                                                |
|--------------|-------------------------------------------------------|
| Language     | Rust                                                  |
| Frontend     | egui (immediate-mode, debuggable, no GTK fragility)   |
| Encoder (prod) | SVT-AV1 (C, mature NEON, fast)                      |
| Encoder (exp) | rav1e (Rust, hackable, future GPU motion-search work) |
| Decoder      | dav1d (preview pipeline, CPU NEON)                    |
| Demux/mux    | ffmpeg subprocess at first; native crates later       |

Both encoders behind one Rust trait. The GUI doesn't know or care 
which is running unless the user picks.

## Process model — per-segment isolation

Foundation borrowed from av1an. Source video splits at scene cuts into 
N chunks. Each chunk encodes in its own subprocess. Failed chunks 
retry individually. Final concat from successful chunks.

```
input.mkv
  ↓
[scene-detect: pyscenedetect or AV1's --scd]
  ↓
chunks[0..N]  (chunk_i = frames a..b, written as .y4m to a tmp dir)
  ↓
[per-chunk encoder workers, parallel up to job concurrency cap]
  ↓
chunk_outputs[0..N]  (.ivf files in chunk_state dir)
  ↓
[concat via ffmpeg -f concat -c copy]
  ↓
output.mkv
```

This single mechanism gives us three properties:
- **Resumable**: chunks have on-disk state. Crash → restart picks up 
  unfinished chunks only. No lost work on a 4-hour encode that died 
  at chunk 47/120.
- **Isolated**: one chunk segfaulting doesn't kill the job. Retry the 
  chunk; if it fails twice, log and skip with a warning.
- **Parallel**: N chunks encode at once up to a concurrency cap. 
  Doubles or triples throughput on multi-core machines.

## Reliability — all three properties

Per the conversation:

1. **Resumable encodes** → chunk state is a sidecar dir 
   `<output>.fao_chunks/` with one file per chunk: 
   `chunk_N_state.json` (status, attempts, last error) plus 
   `chunk_N.ivf` when done. Restart looks at this dir, reuses 
   completed chunks, retries failed/incomplete.

2. **Per-segment isolation** → each chunk is its own subprocess. 
   Encoder linked as library OR invoked as separate binary; both 
   work. Subprocess crash bubbles up as exit code → mark chunk 
   failed → retry once → if still failed, mark error and skip.

3. **Pre-flight validation** → before any encode starts:
   - Verify encoder binary exists and runs (`SvtAv1EncApp --help`)
   - Sanity-check settings (CRF 0-63, presets 0-13, etc.)
   - Verify input file decodes (run ffprobe, confirm frame count)
   - Verify output directory writable, has space (rough heuristic: 
     source_size * 0.5 for typical CRF range)
   - Check ffmpeg version (some flags differ between 4.x and 6.x)
   - Show all problems in one dialog before user hits "go"

## Interactive feedback — all three modes

1. **Live preview of a sample chunk.** User picks a CRF/preset 
   combination, hits Preview. Tool:
   - Selects a representative chunk (high-motion or user-chosen)
   - Encodes that one chunk with current settings (10-30 seconds)
   - Decodes with dav1d
   - Displays alongside the source
   
   This is the killer feature. Most tuning sessions are "what does 
   CRF 28 look like on the hard part of MY content?" Answered in 
   30 seconds. egui can display the decoded frames in a viewport 
   directly via a wgpu surface.

2. **Real-time bitrate/size/quality projections.** As user moves 
   sliders, tool extrapolates from current preview chunk to full 
   video. "At CRF 28 preset 6 this chunk produced X bytes; if the 
   whole video has similar complexity, total ≈ Y MB." With a 
   warning that complexity may vary — it's an estimate.

3. **A/B comparison view.** Encode the same chunk at multiple 
   settings, show them side-by-side or in a flip-toggle viewer. 
   PSNR/SSIM/VMAF numbers if user wants them. Default to visual.

## Queue / batch / share / reuse

The batch picture from the conversation:

```
[Queue manager]
  ├─ Job 1: video_A.mkv → output_A.mkv  (preset 6, CRF 28)
  ├─ Job 2: video_B.mkv → output_B.mkv  (preset 6, CRF 28)
  └─ Job 3: video_C.mkv → output_C.mkv  (preset 4, CRF 32)
  
[Persistent state]
  ~/.config/av1tuner/queue.json
    - jobs[], including their chunk state dirs
    - resume on restart
    - share state via export/import for moving between machines

[Reuse]
  - Setting profiles: "1080p anime", "Game footage", "Talking head"
  - Each profile = SVT-AV1 flag set + scene-detect config
  - Applied to job at queue-add time, can be tweaked per job
  - Profiles export/import as small JSON files
```

GPU offload — Phase 2:

```
[rav1e with GPU motion search via Vulkan]
  - honeykrisp Vulkan 1.3 compute
  - GPU does block-match SAD/SATD across reference frames
  - Result: motion vectors handed to CPU rate-distortion
  - CPU does the bitstream emit unchanged
  - Compatibility preserved (see compat section)
```

## Compatibility

The safe-modification surface (from conversation):

**Modifiable without breaking decoder compatibility:**
- Rate control (CRF, ABR, bit allocation, scene-aware budget)
- Mode decision (which prediction modes to try, search order, early-exit)
- Motion estimation (algorithm, GPU offload, learned priors)
- Tile/threading (work distribution across CPU/GPU/cores)
- Speed presets (heuristics, thresholds, what to skip at fast presets)
- Pre-analysis (scene detect, frame classification, complexity)
- GPU acceleration (anything producing results CPU then encodes)
- SIMD/NEON paths, scheduling, caching

**Off-limits (would break compatibility):**
- OBU writers, syntax-element packers, range coder
- Bitstream emit (`EbBitstreamUnit.c` in SVT, write_* in rav1e)
- CDF table layout, entropy coding state machine
- Frame header structure, sequence header, metadata OBUs

Validation: AOM official conformance suite as release gate. Decode 
each output with dav1d AND libgav1; both must succeed. Run once per 
release, not per encode.

## Architecture summary

```
┌──────────────────────────────────────────────────────────┐
│ egui Frontend                                            │
│  ├─ Queue view (jobs + status)                           │
│  ├─ Tuning panel (sliders, preset picker, preview pane)  │
│  ├─ Settings (paths, concurrency, scene detection)       │
│  └─ Log viewer                                           │
└─────────────────────┬────────────────────────────────────┘
                      │ msg passing
┌─────────────────────┴────────────────────────────────────┐
│ Core (Rust library)                                      │
│  ├─ Queue manager (persistent state, job lifecycle)      │
│  ├─ Encoder trait (Svt | Rav1e impls)                    │
│  ├─ Chunk scheduler (per-segment isolation, retry)       │
│  ├─ Preview pipeline (small-chunk encode + decode)       │
│  ├─ Profile manager (named setting bundles)              │
│  └─ Validation (preflight, conformance gate)             │
└─────────────────────┬────────────────────────────────────┘
                      │ subprocess
┌─────────────────────┴────────────────────────────────────┐
│ External binaries                                        │
│  ├─ SvtAv1EncApp (encode chunks)                         │
│  ├─ rav1e (alt encoder)                                  │
│  ├─ ffmpeg (demux, concat, format conversion)            │
│  └─ dav1d (preview decode)                               │
└──────────────────────────────────────────────────────────┘
```

## Milestones

**M1 — minimal working encoder (single video)**
- Pick a video, set CRF + preset, hit go
- Per-segment isolation with retry, resume
- Watchable output

**M2 — interactive tuning**
- Live preview of representative chunk
- Settings panel with live re-encode of preview
- Save/load profiles

**M3 — batch queue**
- Add multiple jobs to queue
- Persistent queue across restarts
- Per-job profile selection
- Concurrent encoding (N chunks of M jobs)

**M4 — sharing and reuse**
- Profile export/import
- Queue export/import
- Optional: shared library of chunk-results for resume across 
  same-source re-encodes

**M5 — GPU experimentation (rav1e branch)**
- Fork rav1e
- Replace motion search with Vulkan compute path
- Honeykrisp target
- Conformance-test outputs before merging back

## Risks

- **ffmpeg regressions on AV1** — the conversation noted this. 
  Mitigate by pinning a known-good ffmpeg version per release and 
  considering a native demux/mux path later (mp4parse + matroska 
  crates exist).
- **SVT-AV1 API drift** — major version bumps occasionally change 
  encoder API. Wrap behind the encoder trait so updates are localized.
- **egui look-and-feel** — accepted tradeoff. Re-evaluate at M3 
  whether to migrate to iced.
- **Asahi-only GPU paths** — keep Vulkan optional; fallback to CPU 
  always works.

## What gets filed, not built

- Speculative/Monte Carlo entropy decoding research (see 
  NOTES_speculative_entropy_decode.md)
- Learned mode-decision (defer until base tool stable)
- FAO neural codec direction (see FAO_FUTURE.md)
