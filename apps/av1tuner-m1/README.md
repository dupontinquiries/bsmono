# av1tuner

Reliable, friendly GUI for tuning and batch-encoding AV1 video. M1 
ships single-video encoding with per-segment isolation, resume, 
preflight validation, ETA, cancellable encodes, settings persistence, 
and concurrency control.

## Build

```
cargo build --release
```

## Run

```
cargo run --release --bin av1tuner
```

### Renderer choice

Defaults to wgpu (Vulkan on Linux/Asahi, Metal on macOS, DX12 on Windows).
To force OpenGL via glow (rare workaround for driver issues):

```
AV1TUNER_RENDERER=glow cargo run --release --bin av1tuner
```

## Requirements

- Rust 1.75+
- ffmpeg + ffprobe on PATH (any version 4.4+)
- SvtAv1EncApp on PATH (for AV1, the default)
- Optionally: vvencapp on PATH (for VVC/H.266, experimental)

On Asahi Fedora:
```
sudo dnf install ffmpeg svt-av1
# Optional, for VVC support — VVenC isn't in dnf, build via the app:
sudo dnf install git cmake gcc-c++   # build tools for VVenC install
# then use the "Install VVenC from source" button in the UI
```

On Arch:
```
sudo pacman -S ffmpeg svt-av1
```

On macOS (Homebrew):
```
brew install ffmpeg svt-av1
```

## How M1 works

1. Pick an input video. Native file picker filters to common video 
   formats; you can also paste a path.
2. Output path auto-derives as `<input_basename>.av1.mkv` in the same 
   directory. Override with the Output Browse button.
3. Adjust CRF, preset, pixel format, chunk size, parallel workers.
4. Click **Preflight** — verifies all tools available, settings sane, 
   paths writable, input ≠ output (catches the data-loss case before 
   encoding can overwrite your source). Shows every problem at once.
5. Click **Start** — runs a fast probe (instant for most files; up to 
   ~10s for sources without container frame counts). UI stays 
   responsive — probe runs in a background thread with a spinner.
6. Scheduler runs N chunks in parallel (default: half your CPU cores; 
   slider lets you dial up or down). Each chunk encodes in its own 
   `ffmpeg | SvtAv1EncApp` pipe.
7. Progress bar shows chunk completion. Chunk strip below visualizes 
   each chunk's state (gray = pending, yellow = encoding, green = done,
   red = failed/skipped). Hover any chunk for index, frame range, 
   status, attempts, encode time, output bytes, and error if any.
8. Throughput (wall-clock fps, parallelism-aware) and ETA update live. 
   "Encoded so far" + projected total size shown alongside.
9. A chunk failure retries once. Two failures → marked Skipped, 
   scheduler keeps going. The job doesn't die from one bad chunk.
10. When all chunks finish, ffmpeg concats the .ivf files into the 
    final output, muxing audio per your audio settings (passthrough 
    by default, or re-encode to Opus, or drop). Completion panel 
    shows source size, output size, percent change, total time, with 
    a "Show in file manager" button.

## Codec selection

The Codec picker switches between SVT-AV1 (default, royalty-free, 
broad support) and VVenC (VVC/H.266, ~5-15% smaller files, patents 
murky for commercial use, decode support nearly nonexistent outside 
mpv/VLC).

### Compare codecs on a sample

Below the action buttons, the **Compare codecs** row lets you encode 
a short sample slice (default 8 seconds, slider adjustable) with both 
SVT-AV1 and VVenC at your current per-codec settings, then shows a 
side-by-side table: sample size, encode time + fps per codec, and a 
linear projection of full-source output size for each. A one-line 
summary calls out the size and speed delta.

Workflow:
1. Pick input + output paths
2. Click **Probe source** to read frame count + audio info
3. Click **Encode samples** (button appears once probe completes)
4. After both encodes finish, the result panel shows the comparison

The projection is linear scaling from sample to full length — real 
encodes can vary by ±20% depending on content complexity, especially 
for high-motion scenes that aren't well represented in the middle of 
the source. Treat it as a first-look estimate, not a promise.

Both codecs are encoded with whatever per-codec settings are currently 
visible in the UI (CRF + preset for SVT-AV1, QP + preset + QPA for 
VVenC). Adjust either side's settings and re-run to see how it shifts 
the comparison.

When you switch codecs, the settings panel swaps to show the relevant 
controls — numeric preset slider + tune toggle for SVT-AV1, named 
preset dropdown + QPA toggle for VVenC. The QP/CRF slider stays in 
both but the typical range differs.

Stick with SVT-AV1 unless you have a specific reason to test VVC. 
The Output path auto-derives differently per codec (`name.av1.mkv` 
vs `name.vvc.mkv`) so you can keep parallel encodes for comparison.


## Groups (merge tabs)

Switch the top mode toggle to "Groups (merge)" to work with multiple
videos that combine into one output.

Each tab is a merge group: an ordered list of clips that concatenate
into a single MKV with chapter markers (one chapter per source clip,
named after it). Workflow:

1. Add a tab (or use the default Group 1)
2. Add clips: "Add folder…" (recursive, finds videos in subfolders),
   "Add folder (top-level only)", "Add files…", or "Add from zip…"
3. Reorder clips with ↑/↓, uncheck any you want to skip, ✕ to remove
4. If clips have mixed resolutions, a flag appears; the resolution
   policy (scale to smallest / largest / keep native) normalizes them
5. Set an output template (or leave blank to use the global one)
6. "Run this group" — encodes each clip, then stitches with chapters

Clips encode one at a time (each already parallelizes across chunks).
A failed clip is skipped and reported; the rest still merge.

### Output templates

Tokens: `{groupname}`, `{sourcename}`, `{codec}`, `{crf}`, `{date}`,
`{time}`, `{localdir}`, `{savings}`. `{savings}` is only known after
encoding, so a template using it triggers a post-encode rename.

Zip note: importing from a zip warns if the zip barely compressed its
videos (typical — already-compressed video doesn't shrink in a zip),
then extracts anyway.

## Prediction analysis

Below the Compare codecs row sits a **Prediction analysis** row. It runs SVT-AV1 with stat reporting enabled on a sample of the source and produces a Green/Yellow/Red verdict about whether building a learned predictor (Phase 1 work, outside this tool) is likely to pay off on your content.

The verdict is based on AV1's P/I size ratio: how much smaller inter-predicted frames are than keyframes. Small ratio = AV1's inter-prediction is finding lots of structure; learned predictor has little headroom. Large ratio = AV1 isn't capturing much; learned predictor on long-range context could meaningfully help.

This is a Phase 0 proxy. It does NOT train a real predictor. It only tells you whether the cheap-to-measure structural signal exists that would justify the Phase 1 experiment (train a small CNN/transformer predictor on your content and measure actual residual reduction). Phase 1 lives outside av1tuner.

Only SVT-AV1 supports this currently — VVenC has a different stat output format.

## Audio handling

The Audio panel above the action buttons lets you choose:

- **Passthrough** (default) — copy audio + subtitles unchanged. Fastest, 
  no quality loss, but keeps the original audio bitrate.
- **Encode → Opus** — re-encode the first audio track to Opus. Bitrate 
  slider (32-320 kbps), application hint (Audio / VoIP / Low Delay), 
  VBR toggle, compression level (0-10). Sane defaults: 128 kbps VBR, 
  application=Audio, compression=10.
- **Drop** — strip audio entirely. Video-only output. Subtitles still 
  pass through if the source has them.

Multi-track audio sources: passthrough keeps all tracks; Opus encodes 
only the first track (subtitles + chapters still pass). Multi-track 
selection is M2 work.

Preflight checks `ffmpeg -encoders` for libopus before letting you 
start an Opus encode — if your ffmpeg build doesn't have libopus, you 
get a clear error before encoding.

## Cancel

Click **Cancel** mid-encode to kill running subprocesses within ~100ms. 
The chunk being encoded reverts to Pending so a future restart picks 
it up cleanly.

## Resume after crash

State lives in `<output>.av1tuner/` next to your output path:
- `job.json` — overall job state
- `chunk_NNNN.json` — per-chunk state (status, attempts, last error, 
  encode time, output size)
- `chunk_NNNN.ivf` — encoded chunks (when Done)

If the app crashes, you kill the process, or you Ctrl-C the terminal, 
relaunch and point at the same input/output. A "Resume" banner shows 
how many chunks are already done. Click Start (now labeled "Resume") 
to pick up. Click **Discard previous** to wipe state and restart.

## Settings persistence

CRF, preset, pixel format, chunk size, parallel workers, and last-used 
directories persist across app restarts. Saved to:
- Linux: `$XDG_CONFIG_HOME/av1tuner/settings.json` (or `~/.config/...`)
- macOS: `~/Library/Application Support/av1tuner/settings.json`  
- Windows: `%APPDATA%\av1tuner\settings.json`

Saved when settings actually change, debounced to at most once per 
second to avoid disk thrashing as you type. Old saved configs missing 
newer fields load cleanly (serde forward-compat via `#[serde(default)]`).

## Roadmap

- **M1 (this)** — single video, per-segment isolation, resume, 
  preflight, ETA, cancellable, settings persistence
- **M2** — side-by-side experimental encodes for comparing tunings
- **M3** — batch queue (drop in 50 videos, walk away)
- **M4** — settings profiles, queue persistence, sharing
- **M5** — GPU motion search via Vulkan (rav1e fork)

See `docs/DESIGN.md` for the full architecture and 
`docs/FAO_FUTURE.md` for the predecessor project's status.
