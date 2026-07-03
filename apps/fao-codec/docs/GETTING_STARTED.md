# Getting Started

A practical walk-through. Assumes you've cloned the workspace zip and 
have a working Rust toolchain.

## Prerequisites

- **Rust 1.75+** (older may work; recent is safer)
- **ffmpeg + ffprobe** on `$PATH` (FAO shells out to these for input 
  decoding and demuxing)
- **GTK4 + libadwaita** development packages
  - macOS: `brew install gtk4 libadwaita`
  - Linux (Debian/Ubuntu): `apt install libgtk-4-dev libadwaita-1-dev`
  - Linux (Fedora): `dnf install gtk4-devel libadwaita-devel`
- **GPU acceleration (optional but recommended):** Metal on macOS, 
  Vulkan on Linux. The `gpu` cargo feature enables wgpu-backed paths.

Quick check:
```bash
rustc --version       # ≥ 1.75
ffmpeg -version       # any recent
ffprobe -version      # ships with ffmpeg
pkg-config --modversion gtk4   # any 4.x
```

## Build

```bash
cd workspace
cargo build --release --bin fao-app --features=gpu
cargo build --release --bin fao-cli
```

First build takes 5-15 minutes depending on machine. Subsequent builds 
are incremental and quick.

If the GPU feature fails to build, drop `--features=gpu` — the codec 
runs CPU-only at reduced encoder speed but produces identical output.

## Your first encode

### Through the GUI

```bash
./target/release/fao-app
```

1. File menu → Open → pick a video file. ffmpeg-supported containers 
   work: `.mp4`, `.mkv`, `.mov`, `.webm`, `.avi`...
2. The Player tab shows the source. Scrub to confirm playback works.
3. Click the Codec selector at the top right. Pick `v8.0`.
4. Set pin start / pin end with the In / Out buttons (or leave at 0 
   .. duration to encode everything).
5. Click **Transcode**. Watch the stats panel:
   - `FAO v8.0: frame X → Y bytes (live N = 0S/0D/NU, tri T, mesh Vv/Tt...)`
   - `live N` = active feature tracks; should stabilize around 200-400
   - `tri T` = triangles emitted this frame
   - `mesh Vv/Tt` = total verts/tris accumulated in the current chunk
6. When the disk writer reports `FAO: disk writer finished, X frames, 
   Y MB`, the encode is done.
7. The output `.fao` file sits next to the input.

### Through the CLI

The CLI handles library management. For encoding, the GUI is currently 
the only path. (CLI encode is planned; see TODO.md.)

```bash
./target/release/fao-cli library  # show subcommands
```

## Playback

```bash
./target/release/fao-app
```

File menu → Open → pick a `.fao` file.

## Export to AV1 / H.264 (.mkv)

The `.fao` format is FAO-specific. To share footage outside the FAO 
ecosystem, re-encode to a portable codec.

### From the GUI

1. Open or finish encoding a `.fao` file (the player tab shows it).
2. Click the **→ AV1** button next to Export in the encode controls.
3. A progress dialog opens showing frame-by-frame decode progress. 
   Cancel works — partial output is preserved.
4. The result lands in `<source-folder>/FAO_exports/<name>_av1.mkv`.

Uses libsvtav1 at CRF 28, preset 6 by default — a reasonable balance 
of size and quality for AV1.

### From the CLI

```bash
./target/release/fao-cli export <in.fao> <out.mkv> [--codec av1|h264] [--crf N]
```

CLI options:
- `--codec av1` (default) or `--codec h264`
- `--crf N` to tune quality (default 28 av1 / 23 h264; 18=high quality, 40=small)
- `--library PATH` if the `.fao` has V8LR library references

The CLI prints per-frame stats and a final summary of decoded frames 
by codec tier. The Player tab switches to 
FAO mode automatically. Press play.

You can also drag-drop the `.fao` onto the window.

## Library mode: cross-encode mesh sharing

Library mode lets multiple videos share a pool of canonical 3D meshes. 
When you encode a video against a writable library, chunks that match 
existing canonicals reference them by UUID instead of storing their 
own mesh. Subsequent encodes benefit from the accumulated pool.

### Initialize a library

```bash
./target/release/fao-cli library init ~/fao-lib
# Library initialized at "/Users/you/fao-lib"
```

This creates `~/fao-lib/manifest.json` and `~/fao-lib/webs/`.

### Encode against the library (read-write)

```bash
FAO_LIBRARY=~/fao-lib FAO_LIBRARY_RW=1 ./target/release/fao-app
```

In the GUI:
1. Open your first video, transcode as usual. The library will be 
   populated with this video's webs.
2. Open a second, related video. Transcode. Chunks that match existing 
   library webs will be stored as references (much smaller).

The Library tab shows status (web count, disk usage, writable state).

### Encode against the library (read-only)

```bash
FAO_LIBRARY=~/fao-lib ./target/release/fao-app
```

Without `FAO_LIBRARY_RW=1`, the library is read-only. Matching chunks 
still get referenced, but new chunks fall back to in-file storage 
(they aren't added to the library). Useful for distributing a 
pre-built library to playback machines.

### Playback with library

The decoder needs the library attached to render V8LR-referenced 
chunks. With `FAO_LIBRARY` set, the player attaches automatically. 
Without it, V8LR chunks render as gray frames (gracefully degraded).

### Inspect the library

```bash
./target/release/fao-cli library list  ~/fao-lib    # one line per web
./target/release/fao-cli library info  ~/fao-lib    # summary + disk usage
./target/release/fao-cli library verify ~/fao-lib   # check blob integrity
```

## Worked example: encoding a horror movie

Realistic case where library mode helps less (frequent scene cuts mean 
canonical reuse is low). But still useful for keeping geometry from 
each scene available for future re-encodes.

```bash
# Set up a library specifically for this movie's series
mkdir -p ~/horror-library
./target/release/fao-cli library init ~/horror-library

# Encode the movie
FAO_LIBRARY=~/horror-library FAO_LIBRARY_RW=1 ./target/release/fao-app
# In the GUI:
#   1. Open the movie file
#   2. Codec selector → v8.0
#   3. Transcode
#   4. Wait. (At ~30 frames/sec encode, a 90-minute 30fps movie is 
#      roughly 90 minutes of encode time on an M4 Max — currently 
#      ~real-time, not faster.)
#   5. Inspect the result: open the .fao file, scrub through it

# Check what landed in the library
./target/release/fao-cli library info ~/horror-library
# Output: web count, total disk bytes, recent matches
```

Expected file size today: about 8x AV1 for typical movie footage. The 
v8.1 photometric residual planned in V8_PLUS_ROADMAP.md will cut that 
substantially.

## Worked example: a security-camera scenario

This is where library mode shines today. The same physical scene 
appears in every recording.

```bash
mkdir -p ~/cam-library
./target/release/fao-cli library init ~/cam-library

# Day 1 encode (populates the library with the scene's canonical webs)
FAO_LIBRARY=~/cam-library FAO_LIBRARY_RW=1 ./target/release/fao-app
# Open day1.mp4 → v8.0 → Transcode

# Day 2 encode (most chunks reference day 1's canonicals)
FAO_LIBRARY=~/cam-library FAO_LIBRARY_RW=1 ./target/release/fao-app
# Open day2.mp4 → v8.0 → Transcode

# Diff the sizes
ls -lh day1.fao day2.fao
# day1.fao will be larger (more new geometry)
# day2.fao is smaller (most geometry references day1)

# Verify the library is doing work
./target/release/fao-cli library info ~/cam-library
```

## Environment variables reference

| Variable | Meaning |
|----------|---------|
| `FAO_LIBRARY` | Path to a library directory. If set, the app attempts to attach the library at startup. |
| `FAO_LIBRARY_RW` | When set to `1`, opens the library writable. Required for new chunks to be inserted. |
| `FAO_V81` | When set to `1`, enables v8.1 photometric residual encoding. Adds quarter-res RGB delta to each v8 frame (~10-40KB extra per frame) for near-source quality. Decoder is back-compat in either direction. Off by default. |
| `FAO_NO_GPU` | When set to `1`, forces the CPU mesh rasterizer even when the binary was built with `--features=gpu`. Useful for debugging GPU vs CPU output divergence. |
| `FAO_LESS_JITTER` | When set to `1`, the decoder snaps consecutive-frame poses to a stable value when the pose-position delta is below 0.01 scene-units per axis. Suppresses encoder-side pose noise that appears as per-frame jitter. Decode-only — same `.fao` file plays differently with/without the flag. Only applies within a 4-frame sliding window so seeks don't snap to stale poses. |
| `FAO_NO_CULL` | When set to `1`, disables the degenerate-triangle culling in both CPU and GPU rasterizers. Useful for diagnosing whether streak artifacts ("long lines drawing across the screen") are caused by genuine bad geometry or by the culling itself being too aggressive. Default: cull enabled (skips triangles with extreme edge-length ratios or NaN positions). |
| `FAO_PURE_MESH` | When set to `1`, disables the composite-over-source-video fallback for low-coverage v8 frames. Useful for diagnosing pure mesh quality without the bandage of source compositing. Default: composite enabled. |
| `FAO_COVERAGE_THRESHOLD` | Float in 0..1, default 0.25. When v8 mesh coverage of a frame is below this fraction, the player falls back to composite-over-source instead of rendering the (mostly-black) mesh alone. Lower values keep more pure-v8 frames; higher values trigger composite more aggressively. |
| `FAO_COMPOSITE_FEATHER` | Integer 0..3, default 0. When non-zero, the composite path edge-feathers the v8/source blend over an `(2N+1)×(2N+1)` neighborhood. Reduces hard seams at mesh edges where mesh meets source ("glue" artifact). Cost: ~5ms/frame at N=1, ~14ms at N=2, ~30ms at N=3. Default 0 keeps the original hard-cut behavior. |
| `FAO_POSE_EMA` | Float in 0..0.95, default 0. When non-zero, the decoder applies an exponential moving average to consecutive-frame poses (alpha × prev + (1-alpha) × current). Alternative to `FAO_LESS_JITTER`'s binary threshold — gives tunable smoothing for pose noise. Recommended range 0.3-0.7. Quaternion rotation uses lerp + renormalize. Like `FAO_LESS_JITTER`, only applies within a 4-frame window so seeks don't blend across cuts. |
| `FAO_NO_ANCHOR_TRACK` | When set to `1`, skips writing the V8AT anchor-track trailer at export time. Saves ~3-4MB per typical encode. Without the trailer, the Tobii-style anchor overlay (keyboard `A` in the player) has no data to display. Default: V8AT is always written. |
| `FAO_V21_TINY` | When set to `1`, the v2.1 T-frame encoder uses quarter-resolution residuals (1/16 the raw samples) plus a 2× more aggressive SAD threshold for block-skip decisions. Result: T-frame size drops from ~68KB → ~10-15KB at 1080p. PSNR drops 2-4dB but is still better than no-residual motion-only frames. Useful when v8 mesh coverage is too sparse to dominate filesize on its own. Decoder is back-compat — files encoded with and without `FAO_V21_TINY` play correctly with any decoder (the format flag bit announces the downsample factor). |
| `FAO_COMPOSITE_COLORMATCH` | Integer 0..2, default 0. When non-zero, the composite path measures the mean RGB offset between mesh and source at coverage-boundary pixels, then shifts the mesh's colors by that offset before compositing. Addresses the "brightness/color differences" seen during scrub: mesh vertex colors are baked at integration time, so they don't match the current frame's lighting. `1` = mild (50% of measured delta applied), `2` = full delta. Cap of ±30 per channel prevents over-correction on noisy boundary samples. Combine with `FAO_COMPOSITE_FEATHER` for the best results. |
| `FAO_CLUSTER_DEMOTE` | When set to `1`, enables a secondary classification pass that demotes Static-classified features in clusters of ≥3 to Dynamic if the cluster's mean motion deviates from the consensus camera motion by >3 pixels. Targets cases where foreground subjects (e.g. DBD killer's hand) get classified Static and pollute pose estimation. Disabled by default because it can over-demote legitimate background features (particles, swaying foliage). User-controlled until a more robust foreground discriminator is in place. |
| `FAO_NO_ROBUST_MOTION` | When set to `1`, the v8 motion classifier ignores the layer mask and uses phase-correlation's raw camera motion estimate. Useful for diagnosing whether the layer-robust median estimator is helping or hurting on a given content type. Default: layer-robust enabled. |
| `FAO_V9` | **EXPERIMENTAL.** When set to `1`, the encoder builds a v9 volumetric trailer (V9DC) alongside the existing v8 mesh trailer (V8MX). Same .fao file ends up with both. v9 converts the v8 triangle mesh into a sparse 3D voxel grid, runs 3D DCT on 4×4×4 blocks, quantizes with saliency-driven bit allocation, and stores. See `docs/V9_ARCHITECTURE.md`. No tuning yet — this is a "signs of life" prototype. |
| `FAO_V9_NO_SIGNS` | When set to `1`, suppresses the signs-of-life PPM dump (`/tmp/v9_signs_of_life.ppm`) on load of a .fao with a V9DC trailer. By default the load decodes the first non-empty V9 chunk, ray-marches it from a synthetic angle, and writes a PPM you can open to see whether v9 produced anything coherent. |
| `FAO_V9_RES` | Integer 8..256, default 64. Voxels per axis along the longest dimension of each v9 chunk. 32 = coarse/fast, 64 = balanced, 128 = fine/slow. Higher res produces sharper output but increases encode time linearly and trailer size up to cubically (within the sparse-cell occupancy limits). |
| `FAO_V9_MAX_CHUNKS` | Integer, default 0 (= all chunks). When non-zero, the v9 encode caps the number of v8 chunks it converts to volumetric. Useful while iterating on the prototype because the CPU-only splat is slow and capping to e.g. 5 chunks gets you a v9-trailer-containing .fao in seconds instead of minutes. |
| `FAO_V9_RENDER` | When set to `1`, the decoder uses the v9 ray-march path for frames that have V9DC voxel coverage. Pose comes from the v8 pose track; falls back to a synthetic looking-at-the-chunk pose if no pose is recorded. On any ray-march miss (no chunk for that frame, all-black output, no v9 chunks loaded), falls through to the v8 mesh renderer. Default: off (v8 mesh rendering). |

Standard rust / gtk vars (`RUST_LOG`, `GDK_BACKEND`, etc) work as usual.

## Keyboard shortcuts

Player tab:

| Key | Action |
|-----|--------|
| `A` | Toggle the Tobii-style anchor overlay. Shows translucent rings at each tracked feature's position on the current frame, colored by motion-class: green for Static, red for Dynamic, magenta for UiOverlay, grey for Unknown. Works on both A and B player panels simultaneously. The `◉ Anchors` toggle button in the player control bar does the same thing — keyboard and button stay in sync. Requires a `.fao` file with a V8AT trailer (encode without `FAO_NO_ANCHOR_TRACK=1`). |

World tab (3D viewer):

| Key | Action |
|-----|--------|
| `WASD` / arrow keys | Move freecam |
| Space | Move freecam up |
| `Shift` | Move freecam down |
| `L` | Toggle web layout (side-by-side vs overlapping) |
| `F` | Toggle camera frustum visualization |
| `M` | Toggle mesh visibility |
| `R` | Cycle mesh rendering (filled / wireframe) |
| `B` | Cycle mesh brightness multiplier (0.5x, 1x, 2x, 4x) |

### v8.1 photometric residual: trade-offs

`FAO_V81=1` activates an experimental high-quality encoding path. 
What you gain: near-source PSNR (typically 35-45dB) instead of the 
20-28dB you get from pure mesh rasterization.

What it costs: roughly 10-40KB extra per frame compressed, and a 
5-20ms encode overhead per frame for the rasterize-and-diff work. 
Encoded `.fao` files are 5-30x larger than pure v8.0 output for the 
same content, depending on how well the mesh predicts the source.

When to flip it on:
- Visual quality matters (archival, presentation)
- File size budget is comfortable (still substantially smaller than 
  raw video; competitive with AV1 at high CRF values)

When to leave it off:
- Storage is the dominant constraint
- You're willing to trust the mesh-only reconstruction for the 
  content type
- You're benchmarking the v8.0 codec specifically

Files encoded with `FAO_V81=1` play back identically whether the 
decoder has the env var set or not — the decoder always understands 
both wrapper formats.

## Diagnostic logging

Logs print to stderr by default. The most useful ones during encode:

```
FAO transcode: pin_s=X pin_e=Y range=Z duration=D
  → Selected encode range
FAO: ffprobe N frames, median XB, threshold=YB (Z% skippable...)
  → ffprobe stats; skippable = duplicate-frame ratio
FAO v8.0: frame X → Y bytes (live N ...)
  → Per-frame v8 output
FAO v2.1: frame X → T-WITH-RESIDUAL (Y bytes ...) psnr ZdB
  → v2.1 fallback when v8 lost the size battle
FAO disk: write frame X orig_c=A recon_c=B tier=L
  → Per-frame disk write with tier label
[v8 trace] merge: repartitioned N non-empty webs across [0..M] (span ~K frames each)
  → Chunk-merge repartition step (Phase 3 critical fix)
FAO: disk writer finished, X frames, Y MB
  → Encode complete
```

If something looks wrong, copy the full log output. Many bugs in 
recent sessions were diagnosed entirely from these lines.

## Troubleshooting

**"Codec dropped to v2.1 immediately"**
v8 was disabled by the auto-fallback policy. Common on slideshow / 
talking-head content where the mesh stays empty. Look for: `mesh 0v/0t` 
in the frame stats. v2.1 is the appropriate codec for that content.

**"Encode finished at 14 frames but I selected the whole video"**
You triggered a second Transcode while the first was running. The new 
guard (start_transcode IGNORED) prevents this; older builds did not. 
If you see "start_transcode IGNORED" in the log, you double-clicked 
Encode — wait for the in-flight one to finish.

**"Playback stutters / freezes"**
- If the .fao has 0 webs in its V8MX trailer (old broken-export bug), 
  decoder holds previous frame. Look for "FAO: v8 trailer empty" in 
  log. Solution: re-encode with current build (fixes shipped in 
  earlier sessions).
- If valid webs exist but some frames fall through to source-fetch, 
  ffmpeg is being spawned per fallback frame. Filed (source-fetch 
  worker thread) but not yet fixed.

**"Library tab says 'no library attached'"**
Set `FAO_LIBRARY=/path/to/lib` before launching, OR use the Library 
tab → Attach Existing button.

**"GTK4 not found"**
Install gtk4 + libadwaita dev packages (see Prerequisites).

**"wgpu init failed (Metal/Vulkan)"**
Try without `--features=gpu`. The GPU path is optional. If you need 
GPU, check that your driver is current and that wgpu's adapter list 
finds your device.

## Where to go next

- Encoded a file? Open it in the player; scrub through to see what 
  v8 looks like on your content.
- Curious about compression? Read `V8_PLUS_ROADMAP.md` for the plan.
- Want to contribute? `CONTRIBUTING.md` has conventions.
- Hit a bug? Check `TODO.md` for known issues, then file a new one.


## v9 GUI controls

Three buttons added to the toolbar alongside Transcode/Cancel/Export:

- **V9** — Toggle button. When pressed, sets `FAO_V9=1` and `FAO_V81=1` for the next encode. v9 builds from v8 chunks; without v8 active there's no source data. Also forces the codec selector to v8 if the user picks an older codec while V9 is on. Button turns green when active.
- **V9R** — Toggle button. When pressed, sets `FAO_V9_RENDER=1`. The playback path then ray-marches v9 voxel grids instead of rasterizing v8 meshes. Falls through to v8 mesh if no v9 chunk covers the current frame or the ray-march produces all-black output.
- **V9 PPM** — Click button. Opens the signs-of-life PPM dump in the system default viewer (xdg-open / open). The dump is written automatically on load of any .fao with a V9DC trailer.

The status label next to the buttons updates ~10 Hz, showing:
- `v9: idle` — no v9 activity
- `v9: enc ON` — V9 toggle on, awaiting next encode
- `v9: render` — V9R toggle on, awaiting next .fao load
- `v9: 5c/142b` — 5 v9 chunks loaded, 142 blocks total
- `v9: 5c/142b rndr` — chunks loaded AND V9R enabled (live ray-march on each frame)

If the v9 toggle is on, the encode produces both V8MX and V9DC trailers in the same .fao file. Older decoders see V9DC as an unknown trailer and fall back to V8MX.


## Library safety + rollback (Phase 6 starter)

The FAO library now keeps a rotating set of 5 manifest backups. Each time the library writes its manifest.json (e.g. after an encode adds new canonical webs), the previous manifest is copied to `manifest.json.bak.0`, the old `.bak.0` shifts to `.bak.1`, and so on through `.bak.4`. The oldest backup falls off.

This means at any moment you have 5 prior states of the library to roll back to. Backups are local files in the library directory — no separate service required.

### Library Mode tab → Rollback…

A new button in the Library Mode tab opens a slot picker showing the 5 backup slots with their timestamps. Pick a slot and click **Restore**. The current manifest is pushed to `.bak.0` before restore, so rollback is itself reversible — if you regret it, click **Rollback…** again and pick slot 0.

### How safe is "safe"?

- **Atomic writes** — manifest writes go to `manifest.json.tmp` first, then rename. POSIX rename(2) is atomic on the same filesystem.
- **Directory fsync** — after rename, the parent directory is fsynced so the rename survives a power failure on filesystems that allow loose ordering (ext4 with default settings, XFS, etc.).
- **Forward-compat refusal** — if a future FAO build writes manifest version > 1, this build refuses to read it (with an error message pointing to rollback). Better than silently dropping fields.
- **Rotating backups** — 5 slots deep, automatic, no configuration.

### How to test the rollback

```bash
# Attach a library that already has webs
# In GUI: Library Mode → Attach Existing… → pick folder

# Encode a video against it (writes new manifest, creates .bak.0)
# In GUI: Transcode → pick v8 → encode

# Open the library folder in a file manager — you should see:
#   manifest.json
#   manifest.json.bak.0   (the state BEFORE this encode)

# Click Rollback… → pick slot 0 → Restore
# manifest.json is now back to its pre-encode state
# manifest.json.bak.0 is now the post-encode state (rolled forward via rollback if needed)
```

### Performance

Backup rotation costs a single `std::fs::copy(manifest.json, .bak.0)` per write. For a library with ~1000 webs (manifest ~200KB), this is sub-millisecond. The rotation itself is just renames. Total overhead: ~1ms per encode.
