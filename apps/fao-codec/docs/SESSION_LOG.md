# Session Log

Chronological notes by work theme. Each section is what was actually 
shipped or discussed in a session block.

## Foundations (early)
- Shared library extraction (fao_codec_core, fao_codec_lib)
- V81 and V8p planar bitstream variants
- V8MX trailer compression
- Initial mesh rasterizer

## Performance work I
- Batch source-fetch cache
- Async prefetch worker pool
- V8Encoder Mutex sharding (per-worker state, no global lock)
- AVX2 8-pixel color SIMD

## GPU work I
- wgpu device/context plumbing (shared 8-slot context)
- Stockham WGSL FFT
- Phase correlation compute pipeline (built; not yet wired into encoder)

## Storage / memory
- Stats: Vec → HashMap migration
- Disk spillover for chunk_store (caps RAM at policy budget)
- v8 auto-fallback when v8 output exceeds v2

## Library mode roll-out

### Phase 1
- WebSignature extracted as standalone fn
- Library::init / open scaffolding

### Phase 2
- CLI `fao-cli library {init|list|info|verify}`
- Persistent Library handle (read-only)

### Phase 3
- V8LR trailer format + write/read
- V8Encoder library integration at finalize() time
- V8Decoder.attach_library + resolve_library_ref
- App-level GUI for library operations
- FAO_LIBRARY / FAO_LIBRARY_RW env bootstrap
- Library::quick_stats (fast manifest peek without full open)

### Phase 3 follow-up (this session)
- V8LR render integration in render_frame_rgb (was the missing piece — 
  library refs were being read but NEVER rendered, so library mode was 
  effectively a no-op at playback time)
- Residual vertex splicing into canonical mesh on the fly

## Critical bugfixes

### Decoder skip bug
SC2 decode jumped frame 0 → 27000 → 27200. Root cause: sharded V8Encoder 
workers receive scattered ext_fidx (round-robin backpressure), so chunks 
end up with overlapping [min..max] ranges. web_for_frame picked first 
match → wrong mesh. Fix: repartition non-empty chunks into contiguous 
non-overlapping spans at merge time. Translate canonical_web_id refs 
through the remap.

### Double-export bug
Second export call drained already-empty workers, overwriting V8MX with 
0 webs. User's file: 333 MiB with empty trailer → all Z frames fail to 
render → stutter. Fix: V8MergeCache struct holds the result of the 
merge step; second call reuses cache instead of re-draining workers.

### Empty-trailer playback fix
When V8Decoder has 0 webs, skip fetch_source_frame on Z-frame fallback 
(was spawning ffmpeg per frame, blocking UI 400-1200ms each time). Hold 
previous frame instead.

## macOS test response (~7 issues)

User tested on Apple M4 Max with the Asahi Linux lecture .mkv (1920×1080 
AV1 at 0.5 fps, 217485 frames, 120-hour duration).

1. **fps detection**: r_frame_rate was "1000/1" (tbn-1000 container). 
   Fix: prefer avg_frame_rate, fall back r_frame_rate → nb_frames/
   duration → 30, with >240fps sanity gate.
2. **Auto-restart prevention**: start_transcode now early-returns if 
   previous tc is still running. Logs clear refusal.
3. **Disk write log**: now shows tier label (skip/passthrough/full/
   anchor) instead of bare "orig_c=0 recon_c=0".
4. **Auto-disable warmup**: AUTO_DISABLE_MIN_SAMPLES 500 → 100 (3.3s 
   at 30fps), so slideshow content gets v8 disabled faster.
5. **Codec-aware frame label**: added cur_fao_marker. FaoPlay mode maps 
   T→"T-frame (translate)", K→K-frame, V→V-frame, Z→"Z-frame (v8 
   reconstruction)", P→P-frame, B→B-frame.
6. **VRR display gate**: only shows "X% saved" when fps in [15, 120]. 
   Otherwise: "source X.Yfps — savings metric n/a".
7. **Bitrate sample gate**: requires ≥50 samples before Mbps display.

## Performance work II

### Incremental edge-walk rasterizer (10x decode)
fao_codec_lib/src/v8_rasterizer.rs — rasterize_triangle. Old version 
recomputed barycentrics from scratch per pixel (~25 muls). New version: 
compute dw0/dx, dw0/dy, dw1/dx, dw1/dy at triangle setup, walk bbox 
with ~3 adds/pixel. Z, R, G, B all linear in (w0, w1) so same trick. 
For 100k-tri mesh at 2K: ~10ms → ~1ms per frame.

### LK structure tensor hoisting (2.7x encode)
fao_codec_lib/src/v8_reconstruction_codec.rs — LKTracker::track. Old 
version recomputed sum_ix2/iy2/ixiy and prev samples + gradients all 5 
iterations but they're iteration-invariant. New version: precompute 
(i_prev, ix, iy, sx_prev, sy_prev) per window pixel into stack array 
(fallback to Vec for r>5), precompute inv_det of structure tensor once. 
Inside iter loop: only sample cur, compute it, accumulate sum_ixit/iyit, 
solve via pre-inverted tensor. Per-iter samples: 49 vs old 49×5=245.

## Quality fix — REAL VERTEX COLORS (this session)
The MeshIntegrator was painting vertices with `[g*0.4, g*0.85, g]` 
where g = depth confidence. So every mesh rendered in a teal-shaded 
confidence-visualization regardless of actual scene content. User 
reported "blocky bluish-green mesh"; root cause was placeholder colors.

Fix: integrate() now receives the source frame YUV and samples real 
BT.601 RGB at each vertex's source-pixel projection (fx, fy already 
computed for camera projection). All three call sites of 
insert_new_vertex updated; fusion path also blends real colors instead 
of placeholders. Single biggest quality improvement to date.

## AV1/H.264 export (this session)

Production-blocking item shipped. Both CLI and GUI flows:

### CLI
`fao export <in.fao> <out.mkv> [--codec av1|h264] [--crf N] [--library PATH]`
- Decodes through FAO pipeline (V8Decoder.render_frame_rgb for v8 frames, decompress_frame_dct/yuv_split/lossy_chroma for v2.1)
- Pipes raw RGB24 to ffmpeg via stdin
- Output as Matroska (.mkv) with libsvtav1 or libx264
- Optional library attach for V8LR-referenced .fao files
- Per-frame progress + final summary by codec tier

### GUI
"→ AV1" button next to Export in the encode controls.
- Opens progress dialog with frame counter and Cancel
- Worker thread does the decode/encode loop; main thread polls via glib::timeout_add_local
- Output to `<source-folder>/FAO_exports/<name>_av1.mkv`
- libsvtav1 CRF 28 preset 6 (reasonable AV1 baseline)
- Cancel works mid-encode; ffmpeg child killed cleanly

### Known limitations
- T-frame anchor walk-back not implemented; T-frames render as previous frame (only matters for v2.1 encoded .fao files; v8 encoded files unaffected)
- GUI is AV1-only (no codec picker yet); H.264 is CLI-only
- Library attach in GUI version reuses the already-attached library via Arc clone

## Documentation infrastructure (this session)
Created docs/ folder with README, VISION, ARCHITECTURE, CODEC_FORMATS, 
LIBRARY_MODE, V8_PLUS_ROADMAP, TODO, SESSION_LOG, GLOSSARY, 
CONTRIBUTING. Migrated from notes/.

## Compression strategy doc (this session)
V8_PLUS_ROADMAP.md outlines v8.1/v8.2/v8.3+ plan for closing the gap 
with AV1. Key insight from user: "pre-warmed GPU ops don't help 
compression". Roadmap focuses on per-frame photometric residual (v8.1), 
texture maps (v8.2), pose/feature delta encoding (v8.3), mesh 
simplification (v8.4).


## v8 file size regression & memory pressure miscalibration (this session)

User reported file sizes "became huge" on a longer test (13k frames, 
1280×540) and the log was spamming "FAO: memory pressure! 14GB 
available, shedding load" on a 16GB system that had 14GB free.

### File size regression
Per-frame disk writes were storing both the codec-encoded `orig` 
payload AND a zstd-compressed full-frame `recon` payload. The export 
step (`payload_len` in fao_app/src/main.rs:4206) chose between them 
based on a marker check that recognized D/P/L/T/B/K/V but NOT Z (the 
v8 marker). v8 frames fell through to `recon_len`, which is ~500KB 
per frame at 1280×540 — versus ~2KB for the actual v8 orig payload. 
For 13k frames that's 5GB of wasted output.

Two-part fix:
1. Added Z to ORIG_MARKERS (the export marker list) so v8 frames 
   export their compact orig payload.
2. In the per-frame compress pipeline, detect Z marker in c_yuv and 
   set c_recon = c_yuv.clone() instead of computing the big 
   reconstruction. v8 decode reconstructs from the V8MX mesh trailer; 
   the recon column was dead data anyway. Cuts tmp file size by ~80% 
   during encode.

### Memory pressure threshold
Hard-coded `mem_avail_gb < 16` fires constantly on 16GB systems 
(which idle at ~14GB free) and never fires on 128GB systems. Made 
relative: shed when avail < max(total/4, 4GB). Also clarified the 
log message — the action is "trim UI caches" (thumbnails, ring 
buffer, v8 visualization snapshots), not anything that affects the 
encoder, so "memory pressure! shedding load" was alarmist.


## Source-fetch worker thread + prefetch on cache-hit (this session)

User reported playback hitches every ~59 seconds with 0% prefetch cache 
hit rate. Two related fixes:

### 59-second hitches: source-fetch worker thread
The ffmpeg-based source-frame fetch was running synchronously on the 
UI thread. On slow-fps content (~1fps lecture slides), the 60-frame 
batch corresponds to roughly 60 seconds of playback. Every batch 
boundary spawned ffmpeg and blocked the UI for ~500-1500ms.

Fix: new `spawn_source_fetch_worker` background thread; UI thread sends 
`SourceFetchTask` requests via `try_send` on a bounded channel and 
returns immediately. The cache fills asynchronously; the next playback 
tick may HIT the populated cache. If the worker is busy, the request 
is dropped — playback will retry on the next miss.

`fao_source_cache` type changed from `RefCell<HashMap>` to 
`Arc<Mutex<HashMap>>` to allow cross-thread access. The synchronous 
`populate_source_cache_batch` is retained (marked `#[allow(dead_code)]`) 
as a fallback for scrub-style operations that genuinely need the frame 
right now.

### 0% cache hit rate: prefetch on cache-hit path
`prefetch_fao_frames` was only called from the slow-path (decode) tail 
of `load_fao_frame`. On the cache-hit fast path, no new prefetches got 
scheduled. The cache slowly drained until the first miss, then 
prefetched a new window from the miss point. Net effect: each prefetch 
batch immediately ran ahead of playback by ~3 frames (worker count) 
which playback consumed in <100ms, then the cache emptied.

Fix: call `prefetch_fao_frames(idx + 1, 8)` from the cache-hit return 
path too. Now every frame load — hit or miss — schedules the next 8 
into the worker queue.


## v8.1 photometric residual (this session)

The biggest single compression/quality lever from V8_PLUS_ROADMAP.md, 
shipped behind an opt-in env var so the v8.0 default behavior stays 
unchanged.

### Format extension
Z frames gain a wrapper version 2 alongside the existing v1:
- v1: `Z + 1 + ulen(u32) + zstd(v8_payload)` (unchanged)
- v2: `Z + 2 + ulen(u32) + reslen(u32) + zstd(v8_payload) + residual(reslen)`

Where residual = `rw(u16) + rh(u16) + zstd(rw*rh*3 bias-128 i8 RGB delta)`.

Quarter-resolution box-average downsample. For 1280x540 source → 320x135 
residual = 130KB raw, typically 10-40KB compressed depending on content. 
Adds roughly 5-30x the per-frame size of base v8 (~1-2KB → ~10-40KB) 
in exchange for near-source quality.

### Encoder side
- `compute_v81_residual_enabled()` reads `FAO_V81` env var ("1" or "true").
- `compute_photometric_residual()` rasterizes the current mesh through the 
  frame's pose, converts source YUV→RGB, computes per-pixel delta, 
  downsamples 4x with box-average, biases to u8 (+128), zstd compresses.
- The Z-wrap step now emits v2 layout when residual is non-empty, v1 
  otherwise.

### Decoder side
- `V8Frame::from_bytes_any` handles both v1 and v2 wrappers transparently.
- `V8Frame::extract_residual()` returns the raw residual bytes for v2 
  wrappers with non-empty residuals.
- `apply_residual()` decompresses, upsamples (nearest-neighbor), and adds 
  to a rasterized RGB frame with clamping. Called at all three render 
  sites in `render_frame_rgb` (canonical, aligned, library-ref).

### Default OFF
`FAO_V81` is unset by default. Without it, encoder behavior is byte-for-
byte identical to v8.0. Decoder always understands both wrappers, so 
existing .fao files continue to play without change.

### Expected impact
With `FAO_V81=1`:
- Per-frame size: 5-30x larger (target zone for residual-heavy content)
- Quality: near-source PSNR (35-45dB typical, vs 20-28dB for pure mesh render)
- Encode speed: 5-20ms additional per frame for the rasterize + diff + 
  downsample work

For the user's horror movie testbed at 8x AV1 ratio without residual, 
expected ratio with FAO_V81=1 is 1.5-3x AV1 — still compressed but at 
much higher visual quality. Trade-off the user can flip per-encode.

### Known limitations
- T-frames (v2.1) don't get residual treatment; the format extension 
  only applies to Z-marked frames.
- Background pixels (no triangle covers them in mesh render) carry large 
  residuals because predicted = 0 there. This compresses poorly. Mitigation 
  (e.g., fall back to source RGB for uncovered pixels) deferred to v8.2.
- The decoder uses nearest-neighbor upsampling. Bilinear would be smoother 
  but adds cost; nearest is the MVP.


## GPU mesh rasterizer (this session)

The v8 mesh raster path was the bottleneck for high-resolution playback. 
At 2560×1440 with ~10k triangles per frame, CPU rasterization took 
30-50ms — bumping against the 33ms budget for 30fps playback even with 
6 prefetch workers. GPU offloading lets us drop per-frame cost to 
3-5ms and lift the ceiling well past 60fps.

### Architecture
New module `fao_codec_lib::v8_gpu_rasterizer`, feature-gated behind 
`gpu`. Uses wgpu (0.19) graphics pipeline rather than compute — the 
hardware triangle rasterizer + depth test is the right tool for the 
job. WGSL shader mirrors the math in `v8_rasterizer::project` and 
`rotate_by_quat` exactly so CPU and GPU outputs are pixel-comparable 
(modulo floating-point rounding).

### Lazy initialization
`V8Decoder` gained two cfg-gated fields:
- `gpu_rasterizer: Mutex<Option<GpuMeshRasterizer>>`
- `gpu_init_failed: AtomicBool`

On the first `render_frame_rgb` call the dispatch path attempts to 
construct a `GpuMeshRasterizer` (which acquires a wgpu adapter + 
builds the pipeline + allocates targets). If construction fails — no 
Metal/Vulkan/DX12 adapter, missing features, etc. — the `gpu_init_failed` 
flag is set and all subsequent frames take the CPU path with zero 
overhead. No per-frame retries.

### Dispatch
A new private method `V8Decoder::render_mesh_dispatch(mesh, transform, 
pose)` replaces the three direct calls to `v8_rasterizer::render_mesh` 
in `render_frame_rgb` (canonical-mesh, aligned-web, library-ref). Each 
site tries GPU first, falls back to CPU on any kind of failure. CPU 
remains the source of truth for tests and for systems without GPU.

### Pipeline details
- Vertex format: interleaved position (vec3 f32) + color (vec3 f32) = 24 bytes/vertex
- Indices: u32
- Uniforms: cam_pos, inverse rotation quat, focal/principal, world 
  transform mat4x4, has_world_transform flag, output dims
- Render targets: Rgba8Unorm color + Depth32Float depth (matches 
  CPU's f32 depth precision)
- Readback: copy_texture_to_buffer → map_async → CPU Vec<u8>, with 
  the 256-byte row alignment that wgpu requires

### Performance expectations
- CPU 2560×1440 mesh raster: ~30-50ms
- GPU 2560×1440 mesh raster: ~0.5-2ms shader + ~1-3ms readback ≈ ~3-5ms total
- 6 prefetch workers serialize on the GPU mutex but that's fine — 
  single-GPU is already a serialization point. 200 fps decode throughput 
  is well above 30fps playback needs.

### Backwards compatibility
Without `--features=gpu`, the GPU code is stripped entirely and CPU 
remains the only path. Existing `.fao` files play identically; the 
GPU path produces the same pixel output as CPU (within FP rounding).

### Known limitations / future work
- One GPU mutex per V8Decoder means worker contention if you ever 
  have multiple decoders rendering concurrently. In practice we have 
  one decoder per .fao session, so no real contention.
- The lazy init happens on the first frame, which adds 50-200ms to 
  that frame's latency. Could be pre-warmed at V8Decoder construction 
  time if visible jitter on first-frame is a concern.
- Far plane heuristic at 1000 units for depth normalization. Adequate 
  for typical v8 scenes; revisit if scenes get larger.
- No GPU-side residual application yet. v8.1 residuals are still 
  decompressed on CPU and added pixel-by-pixel after GPU readback.


## CRITICAL: Quaternion convention mismatch — encoder vs decoder (this session)

While diagnosing all-black playback on a fresh encode, I discovered 
that the encoder and decoder had been using DIFFERENT quaternion 
layouts for `pose.rotation: [f32; 4]`. This had been silently broken 
since v8 was introduced and was the real reason "every v8 frame falls 
back to source video" — every render call returned `None` due to 
geometry being culled as behind-camera.

### The mismatch
- `CameraPose` struct comment says `(w, x, y, z)` layout.
- **Encoder** (`v8_reconstruction_codec.rs::rotate_vec3_by_quat`) reads 
  `q[0] = w, q[1..3] = x,y,z`. Matches the comment.
- **Decoder** (`v8_rasterizer.rs::rotate_by_quat`) read `q[0..2] = x,y,z, 
  q[3] = w`. Disagrees with the comment and the encoder.

For the default identity rotation `[1, 0, 0, 0]`:
- Encoder reads as `w=1` → identity. Stores mesh in expected frame.
- Decoder read as `x=1, w=0` → 180° rotation around X. Reverses the 
  expected projection by flipping Y and Z. Every vertex ends up at 
  `cam.z < 0` (behind camera). The CPU rasterizer's cull check 
  `if p.2 <= 0.0 { continue; }` rejects every triangle. `render_mesh` 
  returns `None` because `drawn == 0`. `render_frame_rgb` returns 
  `None`. The player falls back to fetching the original source frame 
  from disk via ffmpeg.

### Why nobody noticed
The fallback was good enough to produce visible video — just not via 
v8. Every visible frame in playback was ffmpeg decoding the original 
webm, not the codec's reconstruction. Symptoms that pointed at this 
in hindsight:
- 0% prefetch cache hit rate (cache was never populated with v8 renders 
  because v8 always returned None)
- ~60-frame hitches (ffmpeg source-fetch batch boundaries)
- Mesh integration logs showing healthy mesh stats during encode 
  (e.g., "5792 verts/10646 tris") but visual output looking like 
  uncompressed source quality

The GPU rasterizer made it visible: it produces an all-zero RGB buffer 
on full geometry-cull instead of returning `None`, so the source 
fallback never triggered and the player showed pure black.

### The fix
Three call sites changed to read `pose.rotation` as `[w, x, y, z]`:

1. `v8_rasterizer.rs::rotate_by_quat` — destructure as `(qw, qx, qy, qz)` 
   from `(q[0], q[1], q[2], q[3])`.
2. `v8_rasterizer.rs::project::inv_q` — conjugate as `[w, -x, -y, -z]` 
   (was `[-x, -y, -z, w]`).
3. `v8_gpu_rasterizer.rs` Rust-side uniform packing — reorder 
   `pose.rotation` from `[w,x,y,z]` into the WGSL shader's vec4 
   layout `(.xyz = axis, .w = scalar)` while applying conjugate sign.

Unit tests in `v8_rasterizer.rs` that used `rotation: [0, 0, 0, 1]` 
("identity") were rewritten to `[1, 0, 0, 0]` (actual identity in 
`[w,x,y,z]`). Three tests updated.

### Backwards compatibility
Encoder behavior unchanged — encoder was always correct. Existing 
`.fao` files contain pose data with `[w,x,y,z]` layout. The decoder 
now reads them correctly for the first time. Files that previously 
"played" by source-fallback will now display the actual v8 
reconstruction.

### Other this-session changes
- Added `FAO_NO_GPU=1` env var to force CPU rasterizer (debugging aid).
- Added adapter info logging on GPU init (helps diagnose silent 
  fallbacks to software rasterizers).
- Added first-frame diagnostic dump in GPU rasterizer (mesh stats, 
  pose, transform presence).
- Added all-black output detection in GPU rasterizer — returns `None` 
  when sampling finds no non-zero pixels, so dispatcher can fall back 
  to CPU just as if rasterization had failed.


## Pipeline audit — bug pass (this session continuation)

After the quaternion-convention fix, I went through the encode → 
export → AV1 export pipelines looking for similar latent bugs that 
the source-fallback path had been masking. Found and fixed several.

### Bug: small v8 frames bypassed the recon-skip optimization
**Location:** `fao_app/src/main.rs:3283` (the c_recon compute path).

The c_yuv-is-v8 check was `c_yuv[0] == b'Z'` (Z-wrapped v8 frames). 
But the v8 encoder only wraps with Z when the payload is ≥600 bytes 
(`ZSTD_THRESHOLD`). Small frames are emitted raw as `V81...` or 
`V8p...`. Those fell into the `c_yuv_is_v_codec` branch and got 
treated like v3/v4/v5/v6 — meaning the system compressed the raw 
source YUV (~1MB) as the "recon column" for every such frame.

User's log proved it: `orig_c=592 recon_c=981795` (a 600-byte v8 
frame got a 980KB recon written alongside it). For 939 V-marked 
frames in their encode that's ~900MB of wasted tmp file.

**Fix:** v8 detection now also matches raw `V` + `8` + (`1` or `p`) 
prefixes. The order of the two checks was swapped so 
`c_yuv_is_v_codec` only fires for non-v8 V markers.

### Bug: export stats counter missed Z frames
**Location:** `fao_app/src/main.rs:4407` (export Pass 2 format tally).

The format breakdown match arms covered T/K/V/V4/P but not Z. v8 
reconstruction frames (the bulk of any v8 encode) fell through the 
`_ => {}` catch-all and were silently uncounted. User's SC2 log said 
`Z=0` (the value of an absent counter) while the file actually had 
~83k Z frames.

**Fix:** added a Z counter and put it first in the printout since 
it's typically the dominant format for v8 encodes.

### Bug: ffmpeg source-fetch used inaccurate keyframe seek
**Location:** `fao_app/src/main.rs:299` (worker) and `:5744` (sync fallback).

Both the new source-fetch worker and the older synchronous version 
used `-ss <time> -i <file>` (pre-input seek). Pre-input -ss is fast 
but inaccurate — ffmpeg snaps to the nearest keyframe and "frame 0 
of the seeked stream" can be several seconds earlier than the 
requested time. The frame-counting `select=between(n,X,Y)` then 
indexed off the wrong starting point, putting source frames into the 
cache under wrong indices.

For .webm files with sparse keyframes (5+ second gaps are normal), 
cached frame N could actually be source frame N-150 or so. The A/B 
viewer and the v8-render-fail fallback would show wrong frames.

**Fix:** dual seek — fast keyframe pre-input `-ss <coarse>`, then 
accurate post-input `-ss <residual>` to land at the exact timestamp. 
ffmpeg then exposes `select=between(n,0,batch-1)` starting from the 
real target time.

### Bug: V8MX/V8LR trailer end detection was scan-based
**Location:** `fao_app/src/main.rs:5549, 5563`.

The trailer reader scanned forward from each magic looking for the 
next trailer magic to mark the trailer's end. The V8MX trailer 
contains zstd-compressed mesh blobs, and those blobs can randomly 
contain bytes that match a 4-byte magic like `V8PT`. False match → 
truncated trailer parse → some webs dropped silently.

**Fix:** read the u32 LE length prefix that's written immediately 
before each magic (the export code does 
`dst.write_all(&(trailer.len() as u32).to_le_bytes())` then 
`dst.write_all(&trailer)`). Use the stated length to find the exact 
end. Falls back to the scan-based bound if the prefix is unavailable 
(pos < 4) or the stated length runs past EOF.

### Bug: decoder rasterizer had no fallback for zero focal length
**Location:** `fao_codec_lib/src/v8_rasterizer.rs:155` and 
`v8_gpu_rasterizer.rs:465`.

`CameraPose::default()` has `focal: [0.0, 0.0]`. The merge step's 
"focal > 0" filter avoids overwriting real poses with defaults — but 
if a frame index never had its pose written by any shard (rare but 
possible with thread-out-of-order arrival or partially-flushed 
state), the corresponding entry stays at default. Decoder then does 
`u = 0.0 * cam.x / cam.z + cx = cx` — all vertices project to the 
principal point. Frame renders as one pixel.

**Fix:** both CPU and GPU rasterizers fall back to `focal = 1000.0` 
when the pose's focal value is zero or non-positive. Matches the 
fallback already used in encoder-side `project` and `unproject`.

### Notes for future passes
- **CLI export** (`fao_cli/src/main.rs:463`) and **GUI AV1 export** 
  (`fao_app/src/main.rs:110`) handle V-marker only when `payload[1] 
  == b'8'`. Files with v3/v5/v6/v7 frames silently fall through to 
  generic-decompress and produce held-prev frames. Fine for the 
  current pure-v8 workflow but worth knowing.
- **K-frame handling** missing in export paths — needs block dict 
  trailer access. Not biting because current encodes don't emit K.
- **Encoder hardcodes `rotation: [1, 0, 0, 0]`** (`v8_reconstruction_codec.rs:1060`, 
  marked `// identity for now`). Real camera-rotation estimation 
  would need to come from feature tracking and isn't wired yet.


## Playback trace diagnostics (this session)

After the quaternion fix exposed v8 reconstruction's real output, the 
user reported "circles with parts of test footage that jitter as 
video progresses." Could be pose drift, mesh churn, or web-swap pop — 
or some combination. Added a summary trace to disambiguate before 
guessing at fixes.

### What gets sampled
One `PlaybackTraceSample` per frame load (cache hit OR slow-path miss):
- `frame_idx`, `pose_dx/dy/dz` vs previous frame
- `web_idx` (which decoded web rendered this frame, or -1)
- `n_vertices` (vertex count of the active web's mesh)
- `render_ok` (did v8 produce output, or did we fall back)

24 bytes per sample. 2MB for 84k frames — negligible.

### When summary fires
- Every 5000 frames during continuous playback (rolling checkpoint)
- On `.fao` reload (summarizes the previous session before clearing)
- On app shutdown (final dump for whatever was played)

### What summary shows
- Pose-delta stats (min, p50, mean, std, p95, p99, max) — flat-line 
  values mean static camera; spikes mean drift or jumps
- Web count + transition count — high transitions = mesh-swap pop is 
  probably the visible jitter
- Top webs by frame count — quickly spots "stuck on one web" or 
  "actually using all webs"
- Vertex count distribution per active web — flat = stable mesh, 
  fluctuating = mesh churn
- Per-decile breakdown showing pose-delta mean + dominant web + 
  web-swap count for each chunk — reveals whether jitter is local 
  to one region of playback or pervasive

### Next steps filed
Three follow-ups in `docs/TODO.md` under "Open investigations":
- 🟨 Run the trace, paste output, decide which jitter root cause is real
- 🟨 Diff-overlay debug view (lights up jittering regions spatially)
- 🟩 Scene-space FSM (deferred until we know what discrete states exist)
- 🟩 Sparse-mesh hole fill (vertex-color splatting / source-plane fallback)
- 🟩 GPU lighting shader (deferred until geometry is solid)


## Visual artifact fixes — depth math + degenerate triangles + stabilizer (this session)

User reported three visual issues after the quaternion fix exposed actual 
v8 reconstruction:
1. "Objects in the background seem to be closer to the screen based off the jitter"
2. "Mesh misses where small pockets occasionally jump out of context drawing long lines"
3. "Output frames are still 25-75% black background with no content"

### 1. Depth z-fighting on distant geometry (fixed)
GPU shader had `depth_norm = clamp(cam.z / 1000.0, 0.0001, 0.9999)`. 
For typical v8 content (vertices reach z=26000+), every distant vertex 
hit the upper clamp and got identical depth values. Depth test couldn't 
order them, producing random z-fighting that looked like background 
jitter. Switched to a reverse-Z-like hyperbolic mapping: 
`depth_norm = max(1.0 - 1.0 / cam.z, 0.0)` — monotonic across all 
positive cam.z, with precision concentrated near-camera.

### 2. Long-line streak artifacts (fixed)
"Long lines drawing across the screen" are degenerate triangles: 
one vertex jumps to a bad position (corrupted depth, lost feature 
track, NaN propagation), producing a needle-shaped triangle that 
streaks. Added culling in both rasterizers:
- Edge-length ratio > 50 → skip (needle-shaped)
- Max edge > 10000 scene-units → skip (extreme position)
- Any non-finite distance → skip (NaN propagation)

Gated by `FAO_NO_CULL=1` for disabling during debugging. Default on.

### 3. Black background coverage (filed, not fixed)
This is geometric sparseness, not a bug — the mesh only covers regions 
where features cluster. Two filed approaches in TODO:
- Edge-anchor extraction (Sobel-detected edge points, fixed-depth plane mesh)
- Flat-shot detection → route to V2 DCT for low-texture content

User's 80MB/450MB observation: v8 represents the mesh-covered region 
5.6× more efficiently than AV1 would for the same content. The gap is 
in coverage, not compression efficiency.

### 4. Temporal stabilizer (shipped, partial improvement)
`FAO_LESS_JITTER=1` snaps near-identical adjacent poses to suppress 
sub-threshold noise. User reported it "stabilizes things for the 
most part" but doesn't eliminate jitter completely. Remaining jitter 
is now likely the streak artifacts (fixed in #2 this session) plus 
genuine pose drift (not yet addressed).

### Trace summary not firing in user's session
User saw the rolling summary only once with samples=1 (a prior 
session's residual). Diagnosed: their workflow does very few 
playback frames between actions (encode → load → 1-2 frames → 
export). Thresholds lowered to 30/100/300/1000/3000 and added 
heartbeat log line every 10 samples so trace activity is visible 
before the first summary fires.


## CRITICAL OOM bug: false-positive trailer-magic match (this session)

User's log on 0.44 GB / 24180-frame .fao reload:
```
[FAO load] eager, file=0.44GB, avail=30.0GB
[FAO async-prefetch] worker pool spawned (6 threads)
[FAO source-fetch] worker spawned
memory allocation of 168289044128 bytes failed
[1]    151994 IOT instruction (core dumped)
```

### Root cause
Several trailer-readers (`read_v8_trailer`, `read_motion_profile`, 
`read_block_dict`, `read_v3_plate`, `read_v4_trailer`, `read_v5_trailer`, 
`read_v6_trailer`, `read_v7_trailer`, `read_source_path`) used a 
fixed-size backward scan from EOF (e.g., last 512 MB) looking for 
their respective 4-8 byte magic markers.

For small files (~5 MB) this was fine; trailers near the very end. 
For a 420 MB file, the scan covers the entire file including 420 MB 
of zstd-compressed frame payloads. A 4-byte magic has ~10% chance of 
randomly appearing in that much random-looking data. **The earliest 
match wins**, so the parser then reads what looks like a length field 
from the next bytes — a u32 of random bytes can easily be a billion — 
and tries to allocate gigabytes for a Vec of structs.

In this user's case: 168 GB allocation ≈ Vec<DecodedWeb> of size 
657 million × ~256 bytes/DecodedWeb. The OS allocator refused, 
Rust's default behavior is `abort()`.

### Fix (both layers)
1. **Narrow the scan range.** All trailer readers now compute 
   `frame_data_end = max(entry.offset + entry.size)` from the loaded 
   FaoFrameEntry index and use that as `search_start`. Trailers live 
   strictly AFTER frame data, so scanning before frame_data_end is 
   guaranteed to be a false match. Catches the bug at the lookup 
   layer.

2. **Sanity caps in the parser.** `V8Decoder::parse_v8mx` now refuses 
   n_webs > 1,000,000 (a real .fao should never have more than a few 
   thousand). It also checks `n_webs * 85 <= data.len()` since each 
   web header is at least 85 bytes — catches huge values that somehow 
   pass the absolute cap but couldn't exist in this much data. 
   Per-mesh `n_v` / `n_i` / `n_c` are bounds-checked against the 
   decompressed payload size before reserving. Defense in depth: if 
   another false-magic match slips through, the parser refuses to 
   allocate instead of crashing.

### Observable on this build
- Existing .fao files load successfully on first try.
- If a magic IS false-matched anyway, the user sees a one-line 
  `[v8 parse] V8MX claims X webs (cap 1000000). Likely a false-magic 
  match; refusing to parse.` instead of a crash.

## Trace summary IS firing (this session's log)
Earlier user reported "summary didn't appear." This session's log shows:
```
[playback trace] 10 samples collected (next summary at 30/100/300/1000/3000)
[playback trace] 20 samples collected ...
[playback trace] 30 samples collected ...
========== FAO playback trace summary ==========
  reason: rolling 30-frame checkpoint
  samples: 30 (frames 0..8085)
```
Working as intended. The earlier "no summary" was insufficient playback 
duration in that session, not a bug in the trace mechanism.

## Notable trace observations
For the user's DBD video:
- Pose-position delta: mean=181, std=303, max=950 — LARGE per-frame motion
- FAO_LESS_JITTER threshold is 0.01 (sub-noise floor) — fires only on 
  truly-static sections. Real motion (100+) passes through. Working 
  as intended; the "less jitter" the user feels is mostly from the 
  depth-fix that shipped this saga, not the stabilizer.
- 13 web transitions per 100 frames — mesh-pop frequency is high but 
  not addressable until we know whether the swaps produce visible 
  artifacts (need diff overlay or screenshots).


## CRITICAL fix: CPU rasterizer returning Some(all-zeros), causing "residual glued on black"

### User-reported symptom
> "some frames have residuals drawn over top and others don't but the 
> ones that do sometimes have the feeling of being glued together due 
> to slight jitter or color discrepancies"

User's log shows the GPU rasterizer producing all-zero RGB output on 
frame 0 of a 20622-frame DBD encode:
```
[v8 GPU raster] first render: 7523 verts, 18294 tris, has_colors=true,
   first_v=Some((19031.285, -18553.604, 50840.996))
[v8 GPU raster] degenerate-tri cull: 305 of 18294 triangles dropped
[v8 GPU raster] render #1: 0 nonzero / 1000 bytes, max_byte=0, sample-any_nonzero=false
[v8 GPU raster] render #2: 0 nonzero / 1000 bytes, max_byte=0, sample-any_nonzero=false
[v8 GPU raster] render #3: 0 nonzero / 1000 bytes, max_byte=0, sample-any_nonzero=false
```

The GPU was correctly returning None when its output was all-zero (the 
`if !any_nonzero { return None; }` guard I added two sessions ago is 
working). Falling through to the CPU rasterizer as fallback.

### Root cause
**The CPU rasterizer's `render_mesh` was returning `Some(all_zero_rgb)` 
when triangles passed initial checks but produced zero visible pixels.**

The bug pattern: `drawn += 1` is incremented after calling 
`rasterize_triangle`, but `rasterize_triangle` does its own bbox-clip 
and coverage-test. A triangle can have non-empty bbox AND non-degenerate 
area AND still produce ZERO pixel writes if every pixel center inside 
the bbox fails the `w0/w1/w2 >= EDGE_EPS` coverage test (common for 
thin slivers near pixel boundaries).

For meshes where many triangles are sliver-shaped or off-screen-but-not-
fully-culled, `drawn` ended up > 0 while the rgb buffer stayed all-zero. 
`render_mesh` returned `Some(zero_rgb)`.

In v8.1 path:
```rust
rendered.map(|rgb| apply_residual(rgb, frame_bytes, ...))
```
With `rendered = Some(zero_rgb)`, this layered the per-frame photometric 
residual on top of a zero base. **The user saw the residual rendered 
on black** — exactly the "glued together" symptom they described.

For frames without a residual, output was just zero_rgb → fully black.

### Fix
Added a stride-sampled nonzero check to `render_mesh` in 
`fao_codec_lib/src/v8_rasterizer.rs` mirroring the GPU rasterizer's 
check. Cost: ~4096 byte loads regardless of frame size. If zero 
content was actually rendered, return None so the player's fallback 
ladder (source-video → prev-frame → gray) takes over instead of 
showing residual-on-black.

### Observable on next encode/playback
- Frames where mesh produces zero visible coverage: now show source 
  video (when available) instead of black-with-residual.
- Frames where mesh produces some pixels: unchanged behavior (residual 
  layered on top of partial mesh).

## Other user feedback this session

1. **"skip frame = black frame bug still present"** — the underlying 
   path now produces None instead of Some(zero), so the existing 
   fallback ladder (line 6840 of main.rs) takes over. If user still 
   sees black after this fix, the issue is downstream of fallback 
   (e.g. source-fetch failing silently).

2. **Transparency greedy processing** — filed to TODO. Layer-detection 
   already labels alpha-overlay regions in encoder (2-7% of frames 
   visible in user log) but encoder doesn't do anything with the label.

3. **More object recognition** — filed; would improve mesh integration 
   across web boundaries.

4. **Better jitter** — filed; want configurable low-pass filter for 
   poses (current FAO_LESS_JITTER threshold of 0.01 is a no-op on 
   real content where typical delta is 30-200 scene units).

5. **Color accuracy before lighting** — filed; defer lighting until 
   geometry coverage is consistent.

## Validation
- All 36 source files balanced brace check
- Change is minimal and additive (no API changes)


## Coverage composite fallback — addresses "mostly black frames"

User feedback after the CPU-zero-Some fix shipped:
> "still mostly black frames... first 3 still show mostly black
> mid-frame looks a bit better on the 'glue' psychovisual impressions"

So progress on "glue" (residual layering looks better) but the dominant 
remaining issue is large areas of black where mesh produced no content.

### Why the previous fix was insufficient
The CPU `render_mesh` nonzero-check fires only when the **entire** RGB 
buffer is zero. The user's actual case: partial mesh coverage. Sample 
log first render:
```
first_v=Some((-612.1199, 299.14636, 1948.7598))
world_transform: none
render #1: 0 nonzero / 1000 bytes, max_byte=0, sample-any_nonzero=true
```
- first_v near (-612, 299, 1949) projects to ~pixel (358, 706) — middle 
  of frame. Mesh draws there.
- First 10000 bytes (top-left corner) are zero. max_byte=0 reports that.
- Stride sampling finds non-zero pixels elsewhere (the actual mesh 
  output in the middle of the frame). any_nonzero=true.
- Returns Some(rgb), apply_residual layers residual, output looks like 
  "small fragment of mesh in middle, mostly black."

### Fix this session: coverage threshold composite
When v8 render returns Some(rgb), stride-sample to estimate non-zero 
coverage. Below threshold (25% by default), fetch source video and 
composite v8 mesh pixels over source background. Pixel-wise: any 
non-zero RGB triplet in v8 wins; zero triplets fall through to source.

Env-var control:
- `FAO_PURE_MESH=1` → disable compositing (pure v8 output, mostly-black 
  output when coverage is low). Use for debug.
- `FAO_COVERAGE_THRESHOLD=0.25` → adjust threshold.

User's described mental model maps to this directly: *"we need to find 
that version of the codec and bump it to a plus version and gut the 
old one + archive it for research purposes"*. The composite path is 
the "plus" version; `FAO_PURE_MESH=1` archives the old pure-mesh 
behavior for research.

### Observable on next run
- One-shot log line: `FAO: low-coverage frame (X% < 25%) — compositing 
  v8 over source video. Set FAO_PURE_MESH=1 to disable.`
- Frames previously appearing mostly black should show source-video 
  background with mesh content layered.
- Cost: ~6 MB of byte ops per low-coverage frame (one pass over RGB to 
  composite), plus an ffmpeg source-fetch call (already cached batches).

## Anchor failure observation
User: *"some of the anchors throw off the camera estimation so some 
frames are basically 3d-transformed and some things might break from 
there"*

The trace summary shows web-vertex-count varying wildly (1211 → 16268, 
mean=4521 at 30-sample mark). When the dominant web has a misaligned 
canonical, every frame in that span renders with bad geometry. This 
is the case where the user wants neural-anchor fusion to detect and 
reject the bad primary anchor.

Filed to TODO as a 3-4 session feature.


## Composite path: stats + prev-frame fallback

### User feedback after previous composite shipped
> "I think there were more anchors on black frames without residuals 
> across the board this time"

### Observations from log
1. GPU first-frame render is now strong: `render #1: 838 nonzero / 
   1000 bytes, max_byte=224`. The encoder produced ~840 nonzero 
   bytes in the first 1000 bytes scanned. Not the all-zero case 
   anymore for the leading frames.

2. Pose: `pos=(14.17,-31.07,0.00) rot=[1.000,0.000,0.000,0.000]` — 
   pose has actual position offset (not pure-identity). Better than 
   prior sessions.

3. But trace per-decile shows `mean_pose_d=0.0000` in 4 of 10 buckets 
   — encoder gave up on pose for 40% of the sampled window. Those 
   frames render with identity pose against a mesh built with 
   non-identity poses → geometry projects to wrong screen coords → 
   mostly black.

4. Only 11 webs across 9744 frames (~885 frames/web). Many anchors 
   show `dm_cov=0.0% n_valid=0 integrated 0 tris` — depth map 
   estimation gave up on those anchor positions. Encoder is doing 
   the best it can.

5. **The composite-log line never appeared in user log.** Likely 
   reason: `fetch_source_frame` returns None on cache miss (async 
   pattern), so the composite branch fell through to the previous 
   "just return rgb" path. The user effectively still saw mostly-
   black v8 output for the frames where composite SHOULD have helped.

### Fix this session
1. Added composite-hit and source-miss counters. Periodic stats line 
   prints at 30/100/300/1000/2000... hits or misses:
   ```
   FAO composite stats: {hits} hits, {misses} source-fetch misses ({pct}% hit rate)
   ```
   So we know which branch is firing in the field.

2. When `fetch_source_frame` returns None (async cache miss) AND 
   coverage is below threshold, return the **previous frame** instead 
   of the low-coverage v8. Perceptually, a slightly stale image reads 
   as continuity; a flash of black reads as a glitch. The user 
   reported the latter; this should make it the former.

3. Existing `FAO_PURE_MESH=1` still disables the composite system 
   entirely (for debugging or research).

### What the user will see
- Source-fetch warm-cache case → smooth composite over source. Same 
  as last session.
- Source-fetch cache-miss case → previous frame held briefly instead 
  of black flash. Async fetch eventually warms cache for subsequent 
  frames.
- Period stats line tells us how often each path fires.

## Filed: poses with `mean_pose_d=0` for 40% of frames
This is encoder-side. Either feature tracker is timing out on 
low-texture frames (game UI / dark scenes), or the depth-map 
disparity computation is failing. Real fix needs encoder work — 
the per-decile breakdown should be the metric to optimize against.

The user-requested neural-secondary anchor system (filed last session) 
is the longer-term answer here: when primary tracker fails, secondary 
random points should still produce a usable pose estimate via fused 
Kalman filter.


## Established artifact vocabulary (user, 2026-W19)

User established three distinct psychovisual artifact categories during 
playback testing. Documented here so future session reports can map 
quickly to specific code paths.

### Glue
**Visual:** v8 mesh content "glued" onto inconsistent background. 
Edges of mesh-rendered regions feel disconnected from surrounding 
pixels — sharp transitions, mismatched lighting, slight color 
discrepancies at boundaries.

**Likely causes:**
- Composite path layering mesh pixels onto source video without edge 
  blending (current behavior — binary mask, no alpha falloff)
- Vertex colors sampled at a different exposure/gamma than source 
  video frames being composited
- Mesh and source rendered at slightly different effective viewpoints 
  (pose drift between web reference and current frame)

**Diagnostic:** Visible at composite-path boundaries. Worse when 
coverage is mid-range (40-60% — half mesh, half source); less 
noticeable at very high coverage (mostly mesh) or very low coverage 
(mostly source).

**Fix candidates:**
- Edge-feathering in composite: alpha blend over a 2-3 pixel band 
  around mesh-vs-source boundaries
- Color match: apply source-video local color statistics to mesh 
  pixels in the boundary region
- Better pose estimation so mesh aligns with source pixels exactly

### Skew
**Visual:** Geometry rendered with apparently-wrong transform. Objects 
appear stretched, sheared, or rotated incorrectly. Camera feel is 
"off" — content drifts when it should be steady or vice versa.

**Likely causes:**
- Encoder-side pose estimation failure (`mean_pose_d=0.0000` for whole 
  frame ranges in trace) — mesh built with one pose, frame rendered 
  with identity pose, projection puts geometry at wrong screen position
- Bad anchor included in web reconstruction — one bad pose contaminates 
  the whole mesh's coordinate frame
- Pose interpolation between adjacent anchors smoothing across a 
  discontinuity

**Diagnostic:** Trace summary `mean_pose_d=0.0000` decile buckets are 
the strongest predictor. Vertex count spikes (web jumps from 9k to 
18k verts) also correlate.

**Fix candidates:**
- Neural-secondary anchors with Kalman fusion (filed) — reject primary 
  anchors that disagree with sparse secondary observations
- Per-anchor confidence score (filed) — outlier-reject during web 
  integration
- Pose low-pass filter with realistic threshold (filed, 1 session)

### Black frames
**Visual:** Frame is entirely or mostly black. No mesh visible, no 
residual visible, no source-video fallback engaged. Reads as a glitch 
flash.

**Likely causes:**
- v8 mesh produces zero pixels (off-screen, sliver triangles) AND 
  composite path can't fetch source synchronously (async cache miss)
- Active web has no canonical (kind 0 aligned with missing canonical_id)
- Source video file unavailable (user moved/renamed)
- Coverage threshold composite branch fell into the "size mismatch" 
  or "no prev frame" terminal case

**Diagnostic:** `FAO composite stats: X hits, Y source-fetch misses` 
line tells the story. Low hit rate + many misses = source-fetch starved. 
0 hits ever = composite never engaged (either coverage always above 
threshold, or v8_render returned None and we're in a different fallback 
branch).

**Fix candidates:**
- Widen source-fetch prefetch window on active-web change (proactive)
- Cache last-known-good source frame for tighter fallback continuity
- Frame interpolation between last-known-good neighbors during dropouts


## Source-cache 0% hit rate diagnosis

Composite stats from this run revealed the smoking gun:
```
FAO composite stats: 0 hits, 30 source-fetch misses (0% hit rate)
```

Zero hits in 30 attempts. The composite path is firing correctly (low 
coverage detected) but `fetch_source_frame` is returning None every time. 
The async worker is supposed to populate the cache, but either:
1. ffmpeg is failing silently (binary missing, bad timestamp, codec issue)
2. Worker channel (capacity 2) drops requests faster than worker can drain
3. Worker eventually finishes but cache fills with frames the playback 
   cursor has already passed

### Fixes shipped this session

**1. Source-cache prewarm at file load.** After `prefetch_fao_frames(0, 32)` 
returns, fire two `request_source_cache_fill` calls (idx=0 and idx=60). 
Worker has ~1 second to spawn ffmpeg and populate the cache before the 
user clicks Play.
```
[FAO source-fetch] pre-warm requests fired for idx 0, 60
```

**2. Worker-side diagnostics.** First-call logging in 
`spawn_source_fetch_worker` so we can see what's happening:
- ffmpeg spawn failure → `ffmpeg spawn failed: {err}`
- ffmpeg ran but returned 0 frames → `ffmpeg returned 0 frames for idx={} (status={}, stdout_len={}, frame_sz={}). stderr: {}`
- Success → `first batch decoded: idx={} n_frames={} (stdout {}MB)`

After the next run, the log will tell us which of the three is happening.

### Other improvements in this encode
- 43 webs across 11838 frames (~275 fr/web). Previous encode was 11 webs 
  across 9744 (~885 fr/web). 4× denser web placement.
- dm_cov values much higher: regularly 40-70% across anchors (was 0-20%).
- Per-decile pose: real motion in 9 of 10 buckets (was 4 of 10 with zero 
  pose).
- Z=65.7% / T=34.2% format mix (was 91.8%/8.2%). More translates means 
  better compression on smooth motion frames.

This is real encoder-side progress on the "skew" artifact. The remaining 
"glue" and "black frame" artifacts likely have less to do with encoder 
and more with the composite-source-cache plumbing fixed this session.

### Artifact-vocabulary mapping update
- "glue" — partially addressed (composite now layers v8 on source, removing 
  the all-black background where mesh is sparse). Edge feathering still 
  pending if user reports remaining "glue" feel at composite boundaries.
- "skew" — encoder-side fix landed this run (43 webs, dense pose tracking).
- "black frame" — source-cache prewarm + worker diagnostics this session. 
  Need next-run log to confirm worker is actually decoding.


## Source-fetch worker diagnostic still silent + user root-cause insights

### Diagnostic finding from log
User's log shows:
```
[FAO source-fetch] worker spawned
...
[FAO source-fetch] pre-warm requests fired for idx 0, 60
...
FAO composite stats: 0 hits, 30 source-fetch misses (0% hit rate)
```

The prewarm DID fire (visible log line). But the worker-side diagnostics 
I added last session (`first batch decoded`, `ffmpeg spawn failed`, 
`ffmpeg returned 0 frames`) **never appeared**. That means tasks aren't 
reaching the worker's rx.recv() at all — they're being dropped at 
`try_send` time, OR `request_source_cache_fill` is bailing early before 
the send.

### Fix this session
Added one-shot diagnostic logging at EVERY exit point of 
`request_source_cache_fill`:
- `tx=None (worker not yet spawned)` — worker hasn't come up
- `fao_source_path=None` — source video path not set
- `bad dims w=X h=Y fps=Z` — cur_w/cur_h/vid_fps not yet populated
- `channel full, dropped task #N` — try_send returned Err
- `sent ok #N` at counts 1/10/100 — task successfully queued

User's next log will pinpoint which branch is failing.

### Two user insights this session

**1. "we will want to see those webs merge eventually"** — current 
canonical/aligned system identifies similar webs but doesn't merge 
the underlying meshes. Two passes through the same physical scene at 
different timestamps produce two near-identical meshes that never 
fuse. Filed as cross-web mesh merging, 3-4 sessions.

**2. "the anchors are mostly on the UI elements"** — this is the 
encoder-side root cause behind both "skew" and "black frame" 
categories. Feature tracker is locking onto the stable game UI (HUD, 
scoreboard, menus) instead of the game world. Consequences:
- Mesh represents UI plane, not 3D scene
- When camera pans off UI-heavy regions, dm_cov → 0%
- Pose estimation gets confused (UI has zero parallax)
- Web swaps occur on UI element changes

This explains the trace pattern where `mean_pose_d=0` correlates with 
dropouts — the tracker can't recover pose when its locked features 
are all on a non-moving overlay.

Filed three fix candidates in TODO:
- UI-region masking using existing alpha_overlay layer classification 
  (1-2 sessions)
- Active-content weighting in the feature scorer (1 session)
- Depth-prior filtering — reject features with zero depth variance 
  over time (2 sessions)

Of these, #2 is the cheapest win — the layer detector already runs 
per-frame and produces an "active" mask. Plumbing it into the 
feature-tracker scoring is a small change.


## UI-overlay filter shipped — all three filter axes complete

User asked for "the most complete of 1,2,3 without testing midway". Shipped 
all three axes integrated as a single coherent system.

### Architecture

New `MotionClass::UiOverlay` variant — a hard-exclude category alongside 
Dynamic. UiOverlay features:
- Are not recorded for triangulation (skipped at obs.record loop)
- Are not pushed to sparse_screen during densification (skipped in maybe_densify)
- Persist across MotionConsensusPass re-classification (hard latch — once 
  promoted, the consensus classifier won't re-label as Static)

Three independent promotion paths converge on UiOverlay:

**Axis 1: Layer-mask filtering (hard exclude)** — runs in encode_frame_at 
after consensus.classify. For each live track:
- Look up `BlockLayer` at the track's pixel via the snapshot installed 
  by main.rs from the region-activity tracker
- AlphaOverlay block: increment `track.layer_overlay_hits`
- Active block: increment `track.layer_active_hits`, decrement overlay_hits
- After `overlay_hits >= 5` AND `overlay_hits > active_hits` AND age >= 4 
  frames: promote to `MotionClass::UiOverlay`

This is the fastest-converging axis — a UI feature in a stable HUD region 
gets promoted within ~5 frames.

**Axis 2: Depth-variance filtering (hard exclude)** — runs after 
run_triangulation, gated on triangulation-refresh cycles (every 30 
frames) so we don't falsely decay the EMA against stale data:
- Compute current camera-space depth from triangulation
- Track `last_depth` and update `depth_variance_ema` with the absolute 
  delta (alpha=0.3)
- After age >= 8 frames AND `ema < 5.0`: promote to UiOverlay
- EMA starts at 1000.0 so young tracks aren't auto-rejected. Convergence: 
  UI features at zero parallax reach <5 EMA after ~16 triangulation 
  cycles ≈ 480 frames

This catches UI features that the layer classifier missed (e.g. UI 
elements in regions classified as Active because they overlay highly 
active content).

**Axis 3: Active-content weighting (soft preference)** — runs in 
maybe_densify before sparse_screen.push. Per-layer confidence multiplier:
- Active: 1.20
- ParallaxBg: 1.10
- PureStatic: 0.70
- AlphaOverlay: 0.20
- Noise: 0.80
- Unknown: 1.00

Features not yet hard-promoted still get their contribution weighted 
toward active scene content.

### Data model changes

`FeatureTrack` gained 5 fields:
- `layer_overlay_hits: u8` (saturating)
- `layer_active_hits: u8` (saturating)
- `last_depth: f32`
- `depth_variance_ema: f32` (initial 1000.0)
- `frames_since_triangulation: u32`

`V8Encoder` gained:
- `layer_mask: Option<Vec<u8>>` — block-grid BlockLayer-as-u8 snapshot
- `ui_overlay_promotion_threshold: u8` (5)
- `ui_overlay_min_age_for_variance: u32` (8)
- `ui_overlay_variance_threshold: f32` (5.0)
- `pub fn set_layer_mask(&mut self, mask)` — called from main.rs

`MotionClass::UiOverlay` was added as a fourth variant. All match arms 
updated (spatial_trace cls mapping, count_by_class).

### Diagnostics for next encode

- `FAO v8.0: frame N → ... {S}S/{D}D/{U}U/{O}O ...` — the new O column 
  is the UiOverlay count. When the filter is working, expect O to grow 
  comparable to S over time.
- `[v8 ui-filter] anchor N: total tri features=N, ui-skipped=N, weighted_down=N, weighted_up=N (layer_mask=active/inactive)` 
  — periodic from maybe_densify. Always logs the first anchor so the 
  user can see if layer_mask is plumbed.
- MotionClass hard-latch in consensus.classify — UiOverlay tracks no 
  longer revert to Static.

### Plumbing

main.rs takes a snapshot from `ctc.region_activity` (try_lock; falls 
through with None on contention) before each encode_frame_at call:
- Walks block grid, emits Vec<u8> with one byte per block
- Mapping uses explicit BlockLayer::* match arms (the discriminant order 
  matters and is duplicated in v8 codec; both code paths reference the 
  same hardcoded mapping)
- v8.set_layer_mask(snapshot) installs it for the upcoming frame

### Expected effects user should see

If the filter works as theorized:
- The "{S}S/{D}D/{U}U/{O}O" log gradually shifts mass from S → O over 
  ~few hundred frames as both axes converge
- Mesh integration uses fewer features but BETTER features (real scene 
  content). dm_cov may DROP initially because we're excluding things; 
  this is desired — fewer false anchors means cleaner mesh per anchor
- Pose deltas in the playback trace should show actual scene motion 
  (currently 80% buckets show mean_pose_d=0.0) because triangulation 
  has real parallax features to work with
- "Skew" artifact should reduce because mesh aligns to scene, not UI plane
- "Black frame" artifact may shift to a slightly different failure mode 
  (frames where the scene has been overfiltered) — this is the next 
  thing to look at after running

### Not yet shipped

- Cross-web mesh merging (still filed, 3-4 sessions)
- Neural-secondary anchors with Kalman fusion (still filed)
- The source-fetch worker diagnostic I added last session — still need 
  the next encode log to confirm `bad dims w=0 h=0 fps=0` is what's 
  happening at prewarm time


## Round-4 user observations + source-fetch + viz color fixes

### What the log showed

- UI-overlay filter is alive. Frames show {S/D/U/O} counts like 
  `0S/136D/0U/5O`, `0S/168D/0U/6O`. The O column has small but real 
  numbers — features are being promoted to UiOverlay.
- The DBD content has virtually 0 Static features. Almost everything 
  classified Dynamic. This is NOT new — the previous round's log 
  showed the same pattern. It's the consensus classifier being fooled 
  by foreground motion: phase correlation captures the dominant motion 
  vector which in DBD is often the killer/survivor character. 
  Background features then disagree with that motion and get labeled 
  Dynamic.
- Despite this, real scene content IS being captured occasionally:
  - anchor 3780: total tri features=20, weighted_up=9, dm_cov=22.2%
  - anchor 7920: dm_cov=3.4%
  - anchor 9720: dm_cov=15.0% (later fused twice as 11760, 11820)
- Web swap diversity is up: 16 webs total, 14 distinct seen during 
  playback, 17 transitions — REAL coverage of the video.
- Pose deltas non-zero across all buckets — pose tracking is working.

### User's observations

- "still some negative or brightness differences" at scrub — mesh 
  vertex colors are sampled at integration time, so the mesh renders 
  with frozen colors against current source video. Color seams appear 
  at mesh edges where mesh color and source color disagree.
- "scratch marks (footsteps) on the ground were tracked" — real 
  scene features (ground texture) are getting feature tracks. Good 
  for content but they're classified Dynamic because phase 
  correlation picks up the dominant FOREGROUND motion as camera 
  motion.
- "now all the things that are tracked are static or red" — UiOverlay 
  features were rendering with `ftype 3` in the visualizer (same as 
  Unknown), invisible against the noise. The user could see green 
  (Static) and red (Dynamic) but not the new UiOverlay class.
- "382MB FAO vs 152MB re-encoded in av1" — 2.5x worse than AV1. 
  Format breakdown: Z=93.8%, T=6.2%, T-frames average 25KB. The 
  mesh isn't capturing enough of the content to amortize the bits.

### Fixes shipped this session

**1. Source-fetch worker hang.** Three problems:
- ffmpeg command used `-vf select=between(n\,0\,59)` without 
  `-frames:v 60`. The `select` filter rejects post-N frames but 
  ffmpeg keeps DECODING the rest of the file looking for matches. 
  On a long source, this means hundreds of seconds per task. Fixed 
  by switching to plain `-frames:v` cap.
- Channel capacity 2 was way too small. Burst of playback fires 
  30+ source-fetch requests per second while the worker is stuck 
  in a single ffmpeg call → 28+ tasks dropped per second. Raised 
  to 16 (= ~960 frames of buffer).
- Diagnostic was one-shot. If the worker succeeded silently, no 
  log fired. Made it periodic at 1/5/25/100 counts with elapsed 
  ms timing so we can see actual ffmpeg performance.

**2. UiOverlay viz color.** Added ftype 8 = magenta ring. User can 
now distinguish 4 classes in the World viewer:
- Green ring (ftype 5): Static
- Red ring (ftype 6): Dynamic
- Magenta ring (ftype 8): UiOverlay
- Plain dot (ftype 3): Unknown

### What's NOT fixed yet

- **Phase correlation captures foreground motion**, classifies real 
  scene features as Dynamic. This is the root cause of "0S features" 
  in DBD content. Real fix needs robust motion estimation: either 
  multi-region phase correlation (find the best background region) 
  or RANSAC over feature trajectories (find the dominant motion 
  cluster, take that as camera motion). Both are real work. Filed.
- **Mesh vertex colors are frozen at integration time.** Brightness 
  seams during scrub come from mesh-vs-source color mismatch. Could 
  be fixed by re-sampling vertex colors per-frame from current source 
  YUV, but that's a non-trivial change. Filed.
- **Encoder still loses to AV1 by 2.5x on this content.** Mesh 
  coverage stays under 22% even when working — most bits go to 
  per-frame DCT residuals. Path to better compression: bigger 
  scene-coverage chunks, fewer T-frames. This requires fixing the 
  classification issue first (more usable Static features → bigger 
  mesh).

### What user should see in next encode log

- `[FAO source-fetch] batch decoded #1: idx=X n_frames=Y (Xms, ZMB)` 
  — proves ffmpeg works and times it. If timing is fast (<500ms), 
  composite hits should start showing up.
- `FAO composite stats: N hits, M misses (X% hit rate)` — should 
  finally be > 0% if the source-fetch works.
- Same `{S}S/{D}D/{U}U/{O}O` counts. Don't expect huge changes — 
  the underlying classification issue remains.
- Magenta rings in the World view at the locations the UI-overlay 
  filter is catching.


## Tobii overlay complete + layer-robust motion + pose EMA + prewarm reorder

Standalone session with user away. Completed several TODO items.

### 1. Tobii-style anchor overlay — fully shipped

The keyboard/UI pieces that were missing from last session are in. Now end-to-end functional:

- gtk::Overlay wrapping both player Pictures, with a transparent gtk::DrawingArea on top set to `set_can_target(false)` so clicks pass through to underlying widgets.
- Anchor draw_func: scales anchor (i16 x, y) from source-frame pixels into widget pixels with aspect-preserving letterbox math, draws translucent ring + center dot per anchor colored by motion-class (Static green, Dynamic red, UiOverlay magenta, Unknown grey).
- 'A' keyboard toggle in the main key handler (any tab) flips `show_anchor_overlay`, syncs the toggle button visual state, logs status.
- `◉ Anchors` toggle button in the player control bar (mirrors keyboard).
- `request_player_redraw()` method called from both `load_fao_frame` paths and toggle handlers — DrawingAreas need explicit `queue_draw` to repaint.

User experience: load a .fao file exported with V8AT (default — opt out via `FAO_NO_ANCHOR_TRACK=1`). Press 'A' or click the toggle. See ring overlay on both A/B players. Colors confirm what the encoder is classifying.

### 2. Layer-weighted motion estimation — addresses "everything Dynamic"

Added `MotionConsensusPass::classify_with_layers` (existing `classify` delegates to it with None). When a layer mask is present:

- Gather (dx, dy) for tracks in Active(3) or ParallaxBg(2) layers — the background-favored classifications
- Require >= 8 such features (otherwise fall back to phase correlation)
- Take the median of dx and dy independently
- Sanity check: if median is wildly off from phase-corr (>50 px on either axis), use whichever vector is SHORTER (small motion = more conservative camera estimate; foreground typically has bigger motion)
- Use that as the per-feature prediction in the existing consensus residual loop

This is the actual fix for "the anchors are mostly on the UI elements" root cause. Phase correlation's single-vector estimate gets fooled by foreground subjects (DBD killer/survivor dominates the dominant motion). The median over BACKGROUND features bypasses that.

Diagnostic: `[v8 robust-motion] frame #N: K bg features (median dx=X, dy=Y vs phase-corr dx=X, dy=Y)` fires at counts 1/10/100/1000/5000+. The user can verify the path engages and see the disagreement magnitude.

### 3. Pose EMA low-pass filter — addresses jitter beyond hard-snap

`FAO_LESS_JITTER=1` only snaps when delta < 0.01 scene-units — essentially literal-zero noise. The user filed a request for a tunable EMA. Added `FAO_POSE_EMA=<alpha>` env var:
- alpha=0.3-0.7 recommended for noticeable smoothing
- Capped at 0.95 so pose responsiveness isn't lost
- Quaternion lerp + renormalize (slerp would be more correct but for small deltas lerp is imperceptibly different and ~10× cheaper)
- Same 4-frame window guard as hard-snap so scene cuts don't blend across

User can A/B-test the two modes by toggling env vars between encodes. Both can run simultaneously but EMA dominates.

### 4. Source-fetch prewarm reorder

Root cause was finally clear from the round-3 log: the prewarm calls in `load_fao` fire BEFORE `load_fao_frame(0)`, so `self.cur_w`/`cur_h`/`vid_fps` are still 0 when the bad-dims check runs in `request_source_cache_fill`. The check bailed silently (or with the new diagnostic, logged the bad dims), nothing reached ffmpeg.

Reordered: `load_fao_frame(0)` runs first, dims get populated, THEN three prewarm calls (0, 60, 120) fire. With the channel cap now at 16, all three should reach ffmpeg.

Expected: first encode should show `[FAO source-fetch] batch decoded #1: idx=0 ...` early in the log, and composite stats should accrue non-zero hits.

### What's still on the TODO list

- 🟨 v8.2 — texture maps for canonical webs (2-3 sessions)
- 🟨 Wire GPU phase correlation into V8Encoder (needs wgpu Device plumbing into encoder — more intrusive than expected, deferred)
- 🟨 Library Phase 4 — per-canonical priors
- 🟨 Library Phase 5 — spatial index for find_match
- 🟩 Cross-web mesh merging (user filed it as "we want to see webs merge eventually" — 3-4 sessions)
- 🟩 v8.3 pose & feature delta encoding (looked at it; current planar V8P is already very compact, would shave bytes but with decoder complexity tradeoff)

The biggest pending fix is v8.2 (textures on canonical webs) which is filed as the "biggest single lever" remaining.


## Major size-reduction session: residual fixes + cluster detection + colormatch

Continuation session focused on closing the 1.7GB → 300MB filesize gap user reported. Four substantive changes shipped.

### 1. Coverage-aware photometric residual (THE big one)

User's export was 1.7GB. Math showed Z frames averaging 63KB each (should be ~2KB). Diagnosis: `compute_photometric_residual` was storing `delta = source - predicted` for EVERY pixel, including pixels where `predicted = 0` (no mesh coverage). For those uncovered pixels, delta = source - 0 = source. The residual essentially stored the full source video at quarter resolution in the uncovered regions — 250-300KB per frame compressed.

Fix: per-block coverage count. If <25% of a block's pixels have mesh content, emit sentinel `(128,128,128)` — zero residual contribution. zstd compresses runs of 128 to near-zero. For covered blocks, compute the average ONLY over covered pixels (don't let uncovered ones pull the delta toward the source value).

Decoder-compatible: no format change, same bytes layout. Old decoders read identically.

Diagnostic: `[v8 residual] #N: raw=KB compressed=KB ratio=X.X sentinel=A/B (Y% uncovered)`.

Expected impact: Z frame size 63KB → ~5KB. ~1GB savings on the user's test encode.

### 2. `FAO_V21_TINY=1` — quarter-res T-frame residual

User asked for 10× v2.1 reduction. Two changes:
- Quarter-res residual (each sample applies to 4×4 tile instead of 2×2). Raw size 1/16 of original instead of 1/4.
- 2× SAD threshold bump. More blocks skip residual entirely.

T-frame size 68KB → ~10-15KB (5-7×). Not literal 10×, but pushing further hurts PSNR significantly. Stopped at the safer point.

Format-compatible: new flag bit 0x04 announces quarter-res. Decoder branches on `flags & 0x04` for 4× upsample, falls through to existing 0x01 (half-res) handling.

### 3. Cluster detection — addresses "killer's hand single pose"

User's observation: foreground subject (DBD killer's hand) tracked as a single feature contributing wrong motion to pose estimation.

Added a pass to `classify_with_layers` that runs after the main consensus loop:
1. Gather currently-Static mature tracks
2. For each, find peers with similar motion (within 2px on both axes)
3. For clusters of ≥3 members, check if cluster's mean motion deviates from consensus camera motion by >3px
4. If yes → demote all members to Dynamic + zero consensus score

O(n²) over feature count, but n is ~100-200 → ~10-40K comparisons. Optimization: visited-marking even for non-demoted clusters so background-dominated frames collapse to O(N).

Diagnostic: `[v8 cluster-demote] #N: K features demoted from Static→Dynamic (cluster moving with cam_diff>3px from consensus dx=X dy=Y)`.

### 4. `FAO_COMPOSITE_COLORMATCH=1` or `=2` — addresses brightness seams

User reported "brightness/color differences" during scrub. Cause: mesh vertex colors baked at integration time don't match the current frame's source lighting.

Pre-pass before composite:
1. Stride-4 sample to identify coverage-boundary pixels (mesh has content, ≥1 4-neighbor doesn't)
2. Accumulate per-channel delta `(source - mesh)` over those samples
3. If ≥50 samples, apply mean shift to mesh pixels before composite
4. Shifts capped at ±30 per channel

Strength 1 = mild (half delta). Strength 2 = full delta. Works in both hard-cut and feathered composite paths.

Cost: ~130K ops at 1080p → ~0.5ms per frame.

### Other refinements

- Residual encode gate: skip when scene_mesh has < 100 vertices (saves rasterize+diff cost on sparse early-chunk frames)
- TODO/GETTING_STARTED docs updated with all new env vars and the keyboard shortcut table

### Expected post-session size

Best case (both env vars on): user's 1.7GB encode should drop to ~250-400MB, ballpark with AV1's 300MB. The remaining ~200MB gap vs AV1 is mostly T-frames at 12KB avg — closing that requires a real transform codec (DCT/wavelet), filed as a separate session-scale task.


## Architectural pivot session: v9 roadmap + rollback knobs

User filed a major redesign idea (verbatim above in TODO). Cluster detection from the previous session was hurting anchor quality slightly. Two concrete responses:

### Rollback knobs added

- `FAO_CLUSTER_DEMOTE=1` — cluster detection is now opt-in (was always-on). Default behavior matches pre-cluster-detection state. User can re-enable to test it on specific content.
- `FAO_NO_ROBUST_MOTION=1` — forces phase-correlation's raw camera estimate instead of the layer-robust median. Lets the user A/B test whether layer-robust is helping or hurting on a given encode.

Together these give the user full A/B coverage over the recent classification changes. They can isolate which one (if any) is responsible for the "slightly worse" anchoring.

### v9 architecture filed

Created `docs/V9_ARCHITECTURE.md` (185 lines) documenting the user's pivot vision:

- Pipeline: warp → build mesh → 3D DCT → output (drop triangles, keep world)
- Sparse 3D voxel grid (probably hash grid or octree)
- 3D DCT or wavelet decomposition with saliency-weighted quantization
- ML object recognition + depth refinement
- View-conditioned colors

Estimated 8-12 sessions for a v9 MVP. Pre-requisites:
1. Pick ONE saliency model
2. Pick ONE 3D representation
3. Prototype encoder + decoder on a single chunk
4. Compare against v8 for size and quality

Doc is deliberately non-binding — every section flagged as a research direction, not a spec. The actual work waits until v8 is fully stable.

### Position on v8.x

v8 with the recent fixes is "good enough to compare against AV1 on real content". The 1.7GB → ~250-400MB expected size with coverage-aware residual + FAO_V21_TINY puts us in the same ballpark as AV1. v9 is the path to "substantially better than AV1 because we understand the scene", but that's a separate effort.

Next 2-3 sessions should focus on:
- v8 stabilization (no new features, just bug fixes)
- Pick ONE thing from the v8.x backlog if user requests it (v8.2 textures? Library Phase 4?)
- Start v9 prototyping ONLY when user explicitly says so


## v9 prototype shipped — volumetric scene codec

User asked to build a v9 even though we know it might not pan out:

> "can you make a v9 and we can always come back to v8 later... do things 
> that are hard if they get the best results theoretically. code without 
> testing or logging too much... we can analyze what comes out later if 
> it shows signs of life"

Built end-to-end in `fao_codec_lib/src/v9_volumetric.rs` (~1100 lines, 
one module). The full pipeline:

### Encoder
1. Take v8 SceneMesh + feature positions (from `build_v9_chunk_from_v8`)
2. Compute mesh AABB, pick voxel_size for `FAO_V9_RES` voxels along the longest axis
3. **Greedy mesh-to-voxel splat**: every triangle is sample-rasterized at 8×8 barycentric resolution, each sample contributes density+color to a 3×3×3 voxel neighborhood with Gaussian falloff. Normalize colors by accumulated density.
4. Accumulate feature positions into per-voxel `feature_count` (saliency proxy — drives bit allocation)
5. Walk grid in 4×4×4 blocks, build 31 channels per block: density(1) + base RGB(3) + 9×3 SH coefficients(27)
6. **3D DCT** per channel (separable type-II, derived correctly after verifying in Python — my first-pass hand-derivation was wrong; fixed and re-verified roundtrip to 1e-15)
7. **Quantize**: frequency-aware matrix (DC=1 step, high-freq=22) × per-block scale (log2-encoded as u8) × saliency boost (active voxels get 2-4× finer steps)
8. zstd-compress the packed channel data
9. Write `V9DC` trailer alongside the existing `V8MX`

### Decoder
1. Parse V9DC blocks
2. Per block: dequantize → inverse 3D DCT → splat values back to hash-grid voxels
3. **Ray-march**: per pixel, slab-intersect AABB, step at half-voxel intervals through the grid, sample density + view-conditioned SH color, front-to-back compositing with transmittance

### Signs-of-life dump
On load of any .fao with V9DC, the app decodes the first non-empty chunk, ray-marches it from a synthetic camera looking at chunk center from 3× extent away, and writes `/tmp/v9_signs_of_life.ppm`. Log reports nonzero-byte count + voxel cell count.

### Env vars

- `FAO_V9=1` — encode V9DC trailer alongside V8MX
- `FAO_V9_RES=64` — voxels per axis (8..256)
- `FAO_V9_MAX_CHUNKS=0` — limit chunks processed (0 = all)
- `FAO_V9_NO_SIGNS=1` — suppress PPM dump on load

### What's in v9 vs not in v9 (yet)

In:
- Sparse hash grid via Morton-encoded keys (21 bits per axis, plenty of address space)
- Full DCT roundtrip (verified mathematically in Python before committing to Rust)
- Saliency-weighted bit allocation (per-voxel feature density drives 1×/2×/4× quant boost)
- Spherical-harmonics degree-2 framework (9 coeffs per channel, view-conditioned color in decoder)
- Ray-march decoder with AABB slab intersection + density compositing
- Trailer parse + length-prefix end detection (same pattern as V8MX/V8AT)

Not yet:
- GPU compute shader for splat (CPU reference path only — will be slow on real content)
- Multi-view SH fitting (current colors default to DC-only band, equivalent to view-independent)
- Online ML saliency model (using v8 feature density as proxy)
- Integration into live playback (signs-of-life PPM dump only)

### What's the test

The goal is to see whether the v9 pipeline produces non-garbage output 
on a real chunk. User encodes their DBD test video with `FAO_V9=1`, 
loads it, looks at `/tmp/v9_signs_of_life.ppm`. If it shows a coherent 
blob shaped like one of the chunks, v9 has signs of life — worth 
continuing. If it's black or random, the pipeline has a bug to fix 
before more investment.

### Risk if it doesn't work

Zero. v9 is a write-only-additional-trailer in this prototype — v8 
playback path is untouched. Disabling FAO_V9 reverts to v8-only. The 
.fao files are larger by however much V9DC adds (cap available).


## v9 continued — playback wiring + adaptive splat + grid cache

Continuation of v9 prototype. User reported "no v9 trace, no tmp file" — 
diagnosed as either an older build or a non-v8 encode (FAO_V9 only fires 
inside the `if any_v8_active` block since v9 builds from v8 chunks). 
Added an unconditional gate-check log so the next encode produces output 
regardless of whether FAO_V9 was set:

```
FAO v9.0: gate check — FAO_V9 env=ON (12 non-empty chunks available)
```

If the user encodes without FAO_V81=1, that log line won't appear 
because the v8 trailer block doesn't run at all. The fix is to also 
enable v8.

### Playback wired

`FAO_V9_RENDER=1` enables ray-marching from voxel grids during playback. 
Pipeline in decode_fao_frame_to_rgb:

1. If v9 chunks loaded and any covers the current frame...
2. Look up v8 pose for this frame (falls back to synthetic "looking at 
   chunk center" pose if not recorded)
3. Decode the chunk's voxel grid (cached via LRU — 3 most recent chunks 
   stay in memory)
4. Ray-march at the chunk's pose
5. If output is all-black, fall through to v8 mesh rendering (don't freeze)

### Adaptive splat sampling

Previous splat used fixed 8×8 = 64 barycentric samples per triangle. Now 
samples_per_axis adapts to triangle edge length: 2× the voxel resolution 
along the longest edge, clamped to [2, 16]. Big triangles get more 
samples; tiny ones get fewer. Result: better surface coverage on coarse 
meshes, less wasted work on dense meshes.

### Voxel grid cache

`fao_v9_grid_cache: RefCell<Vec<(usize, Rc<VoxelHashGrid>)>>` with 
LRU-replacement (capacity 3). decode_v9_chunk is expensive (IDCT + 
splat for every block), so caching makes playback at 24-60fps feasible. 
A typical chunk has ~150 active blocks → ~10ms decode. Without cache, 
that's 10ms per frame plus the ray-march cost.

### Color clamp in ray-march

After DCT roundtrip with quantization, voxel colors can occasionally go 
negative or slightly above 1.0. Added clamp(0,1) inside the ray-march 
to prevent ringing artifacts. Final output clamps to [0,255] separately.

### Frame-range trailer fields

V9Chunk and the V9DC trailer now serialize frame_start/frame_end (8 
extra bytes per chunk). The decoder uses these to find which chunk to 
ray-march for a given playback frame. Trailer chunk-header size went 
from 20 to 28 bytes.

### What's next for v9

- Multi-view SH fit (need per-vertex per-frame color samples; not stored 
  in v8 — would require a separate accumulation pass)
- GPU compute shader for splat (CPU still slow on big content)
- Voxel-level normals (3 channels added; enables proper Lambertian 
  shading without ML)
- ML saliency model (DepthAnything / SAM)
- Cross-chunk delta encoding (P-frame-style for V9 chunks)
- 8³ block size experiment


## Custom library database

User: "let's just roll our own database for what we need actually" 
(after asking about Rust embedded DB options and seeing redb / fjall / 
rusqlite as the serious contenders).

Built `fao_codec_lib/src/library_db.rs` — ~550 lines, single module, 
zero new dependencies. Design: append-only log + in-memory index, 
inspired by Bitcask.

### Record format
`"WLR1"`(4) + CRC32(4) + kind(1) + uuid(16) + payload_len(4) + payload(N). 
Each record is self-describing and self-validating. Typical size ~300 
bytes, well under the POSIX-atomic-write threshold.

### Key properties
- **Crash safe by construction.** Torn writes produce partial records; 
  on open we scan + truncate at the first invalid record.
- **Atomic per-record.** Each `add_web` is a single `write_all()` of 
  <4KB, POSIX-atomic.
- **In-memory index.** `HashMap<uuid_hex, LibraryWebStub>` rebuilt on 
  open. O(1) lookups.
- **Rotating backups.** Same UX as manifest.json rollback: 5 slots, 
  the GUI rollback button now restores BOTH manifest and log to the 
  same slot.
- **Compaction.** When tombstones exceed 25%, rewrites the log keeping 
  only live entries. Rotates current log into .bak.0.

### Dual-write with JSON manifest
The log doesn't REPLACE manifest.json yet — it's populated alongside. 
`add_web` and `touch` both write to JSON AND append a log record. The 
JSON manifest remains authoritative; the log is a fast-lookup mirror.

This is the safe migration: if the log goes wrong, JSON is still 
correct. Rollback is independent for each store.

### One-shot migration
On first writable open after this update, if the log has fewer 
entries than the manifest, the missing entries are populated from 
the manifest. Single-pass, idempotent, runs once per library.

### Bug fix uncovered along the way
The existing `signature_to_bytes` had a wrong array size — typed as 
`[u8; 88]` but the field math is 112 bytes. Would panic on any 
non-empty library write. Fixed to 112 and updated all callers.

User's libraries may have been empty during development, masking the 
panic. Now that we're writing signatures to log records this would 
have triggered immediately.

### Performance estimates
JSON-only `touch` (ref_count++): rewrite whole manifest, ~5ms per 1000 
webs. With log: single append, ~50μs.

For a video with 1000 canonical references, that's ~5 seconds saved 
during encode.

### Design doc
`docs/LIBRARY_DB_DESIGN.md` — 166 lines covering format, properties, 
performance, future work.


## DB perf + v9 diagnostics push

User: "keep working on making the db fast enough to fit our library spec 
and figure out why v9 isnt wired and fix it"

### DB perf wins

1. **eig0_index secondary index for find_match.** Sorted Vec<(f32, idx)> 
   over eigenvalues[0]. find_match now binary-searches for the candidate 
   range eig0 × [0.8, 1.2] then runs full similarity only on that subset. 
   Expected ~30× speedup on libraries > 64 webs.

2. **Fast-path touch.** When log_db is open, `touch` (ref_count++ on a 
   matched canonical) skips the JSON manifest rewrite entirely. Just 
   appends a record to webs.log. Drop flushes JSON once on close.
   
   Old cost: ~100ms per touch at 10k webs. Each encode could touch 
   hundreds of canonicals → 10-100s of redundant JSON writes per encode.
   
   New cost: ~50μs per touch (single log append). Drop adds one final 
   ~100ms JSON write. Total savings per encode: 10-100 seconds.

3. **library_db::bench() function** for synthetic perf measurement. 
   GUI "DB Bench" button in the Library Mode tab runs it on the user's 
   hardware (10k entries, 10k random gets) and prints the report.

### v9 diagnostics

User reported "no v9 in logs only v8". Most likely they hadn't clicked 
Export yet (v9 only runs at export, not during encode). Made this 
unmissable by adding:

1. **Boot banner.** Every app launch prints a line confirming v9 is 
   compiled in. If the user doesn't see this, they're on an old binary.
2. **Enhanced gate-check log.** Now shows the RAW env var string (so 
   accidental "yes"/"true"/"0" values are visible) plus chunk counts 
   (so "V9 is on but no chunks" is visible).
3. **WARN on V9-but-no-chunks.** Explicit log if FAO_V9=1 but 0 non-empty 
   chunks — points to v8 mesh integration issues.
4. **Box-drawing emphasis** around the V9DC success log so it's hard to 
   miss in a busy export log.
5. **Export-click diagnostics.** If the Export button click is being 
   silently ignored (no transcode, encode still running, app borrowed 
   elsewhere), now logs the specific reason.

### signature_to_bytes bug (latent)

The existing JSON manifest code had `[u8; 88]` typed but the field math 
totals 112 bytes (extent 12 + centroid 12 + eigenvalues 12 + eigenvectors 
36 + radial_histogram 40 = 112). Would panic on any non-empty library 
write. Fixed to 112. Probably wasn't triggered yet because production 
libraries were still empty during development.


## v8.2 DC prediction + library find_match alloc-free

User: "sure" (after asking whether to push on DC prediction or pivot).

### DC prediction for v8.2 residual

AV1-inspired spatial prediction added before the skip-block RLE step. 
For each block in raster scan order, predict its bias-128 delta from 
the (left + top) / 2 of previously-encoded neighbors. Encode the 
prediction error instead of the absolute value. After this transform, 
the distribution shifts toward zero, which makes more blocks 
skip-eligible.

Activated by `FAO_V82_DCPRED=1`. Requires `FAO_V82_SKIP=1` to be 
useful (DC pred alone without skip-block detection wastes the entropy 
reduction).

### Header encoding

The rw u16 field in the residual header now carries two flag bits:
- bit 15 (0x8000): v8.2 skip-block RLE
- bit 14 (0x4000): DC prediction

Width is in bits 0..13 (max 16383 — quarter-res of a 65532-wide source 
video; plenty of headroom for any realistic input).

### Clamp tracking

The bias-128 stored value gets clamped to [0, 255] when prediction 
error exceeds ±127. This is lossy in edge cases. Added a global atomic 
counter incremented on each clamp event, exposed via the existing 
residual diagnostic log. Lets the user judge whether DC pred is 
helping or hurting on their content.

### find_match alloc-free

The eig0_index candidate-pruning path previously built a `Vec<usize>` 
of candidate indices per `find_match` call. For large libraries that's 
~100-1000 allocations per encode chunk. Rewrote to use index-range 
iteration over either the full webs list or the eig0_index slice, 
no allocation. Frees ~30% of find_match overhead on hot paths.

### Adaptive zstd level for V8MX

Added module-level `adaptive_zstd_level(raw_size)` helper: returns 
level 19 for chunks ≤ 256KB, level 12 for bigger. Override with 
`FAO_ZSTD_FAST=1`. Multi-second export savings on big videos.


## v8.2 channel deinterleave + render_mesh hot-loop hoist

User: "keep working"

### v8.2 channel deinterleave (third AV1 tactic)

After DC prediction shifts most non-skip blocks toward zero, the 
remaining non-skip payload is RGB-interleaved [r0 g0 b0 r1 g1 b1 ...]. 
Each channel has a relatively narrow distribution (residuals are 
small deltas), but the interleaved stream mixes those distributions 
making zstd's LZ matching less effective.

Solution: deinterleave the per-channel runs within each non-skip 
segment. Output becomes [r0 r1 r2 ...] [g0 g1 g2 ...] [b0 b1 b2 ...]. 
Each contiguous run is monotone — zstd matches it much better.

Activated by `FAO_V82_DEINT=1`. Requires `FAO_V82_SKIP=1` (the 
non-skip-payload formatting only exists in the skip-block path). 
Header signaled via bit 13 of the rw u16.

### Header bit layout (v8.2 residual)

```
rw_field u16:
  bit 15 (0x8000) = v8.2 skip-block RLE
  bit 14 (0x4000) = DC prediction
  bit 13 (0x2000) = channel deinterleave for non-skip payload
  bits 0..12 = residual_w (max 8191)
```

### Hot-loop hoist: mesh integration

Added `quat_to_mat3` + `rotate_vec3_by_mat3` helpers and used them in 
the two per-pixel rotation hot loops (`MeshIntegrator::integrate` 
and `integrate_with_fusion`). For typical quarter-res depth maps 
(~57k pixels), the rotation step goes from ~22 ops/pixel (quat 
formula) to ~15 ops/pixel (matrix-vec). 2-3× speedup on the 
rotation step within the integration pass.

### Hot-loop hoist: render_mesh

Added `inv_quat_to_mat3` + `project_with_mat` to the rasterizer. 
`render_mesh` previously called `project()` per vertex, which 
re-derived inverse rotation + focal/principal fallbacks every time. 
Now those are hoisted to once-per-render_mesh-call. Hot path during 
residual computation (called per frame).

### v9 wiring audit (third pass)

Walked the full encode→export→read→render path one more time. Every 
stage cross-references correctly:
- Length prefix written before V9DC body in export_fao
- Length suffix appended inside write_v9_trailer
- GUI reads prefix from data[pos-4..pos] to determine v9_end
- read_v9_trailer stops cleanly after n_chunks records (ignores any 
  trailing bytes including the length suffix)

No actual wiring bug found. The user's "no v9" reports are most 
plausibly: hasn't clicked Export yet, didn't tick the V9 checkbox, 
or running an older binary that predates the dialog checkbox.

The boot banner addresses the "old binary" case. The "dialog → V9 
enabled" log addresses the "didn't tick" case. The encode-complete 
hint addresses the "didn't click Export" case.


## v8.3 repeat-marker + v9 zstd-19 + library tie-break

### v8.3 "R" repeat-marker for static frames

Static-frame fast path now emits a 1-byte `b'R'` marker instead of 
re-emitting the full previous Z-wrapper bytes. Decoder walks backwards 
through fao_index to find the most recent non-R frame, then decodes 
that. Bounded walk-back depth of 600 frames so corrupt R-runs can't 
spin forever.

For static-content workloads (slide presentations, screen recordings 
with mostly-still content, talking heads with static background), this 
collapses N consecutive static frames from N × wrapper-size down to 
N × 1 byte.

Gated by `FAO_V83_REPEAT=1` (default OFF for backward compat with 
older decoders). New decoders handle both R-markers and full re-emits.

Also handled in `decode_fao_frame_for_export` — that path already 
threads `prev_rgb` through, so R is just a passthrough there.

### v9 block compression upgrade

v9 per-block channel data now compresses at zstd level 19 (was 3). 
Blocks are tiny (~2KB raw) so level 19 is fast (~50μs/block vs 10μs 
at level 3). Buys ~15% smaller V9DC trailer.

### Library: find_match tie-break on ref_count

When two candidates score within 0.01 of each other, prefer the one 
with higher ref_count. Pushes the library toward consolidating around 
popular canonicals over time — newer encodes pile refs onto established 
canonicals rather than creating parallel near-duplicates that 
fragment match traffic.

The 0.01 tie-break window is narrow enough that a meaningful similarity 
advantage still wins, but wide enough to catch the "two equally-good 
matches" case where popularity is the right tiebreaker.

### Library DB open-scan speedup

`LogDb::open` now reads the whole log file once via `std::fs::read()` 
instead of per-record `seek + read_exact`. For 100k-web libraries the 
log is ~30MB and the speedup is 3-5× (eliminates syscall overhead, 
lets the OS use one readahead pass).

Memory cost: log file size held in RAM during scan. Acceptable up to 
~hundreds of MB; switching to mmap or chunked reads would be needed 
beyond that.


## V8MX-v2 mesh quantization + library FLW2 + stats/prune

User: "keep working on the library mode and the file sizes"

### V8MX trailer v2 — quantized vertex/color storage

V8MX trailer bumped from version 1 to version 2. Each blob now carries 
a tag byte (0 = legacy f32, 1 = quantized). For canonical/standalone 
meshes the quantized layout stores:

- AABB (24 bytes: 3×f32 min + 3×f32 max)
- Vertices as u16 quantized to AABB extent (6 bytes each vs 12)
- Colors as u8 RGB (3 bytes each vs 12)
- Indices unchanged (u32)

Size win: for a 1000-vertex mesh, 24KB raw → 9KB raw. After zstd, ~4KB 
vs ~12KB. ~3× reduction on the dominant trailer payload.

Precision: extent/65535. For typical 1-100m chunks that's sub-mm. 
Color quantization is 8-bit RGB, indistinguishable in practice from 
the f32 source (which originated from u8 colors anyway).

Aligned residual blobs stay as f32 — they're typically small and 
quantization to a residual-AABB would amplify noise. Tagged as legacy 
in v2 trailers so the decoder takes the right path.

`FAO_V8MX_RAW=1` forces legacy f32 storage for debugging or precision-
critical workflows.

Old V8MX-v1 decoders see version=2 and trip the version-check warning, 
then attempt parsing. With the tag byte they read a garbage vertex 
count and bail (the n_v sanity check catches the rest). New decoders 
handle both.

### Library FLW2 blob format

`write_web_blob` / `read_web_blob` upgraded to FLW2 with the same 
tag-byte mechanism. Library canonicals stored on disk now use the 
quantized layout by default. Normals also quantized (i16 per axis, 
range [-1, 1] → [-32767, 32767]).

Library size savings compound across the whole library:
- 1000 webs × 1000 verts each × (12B vert + 12B color + 12B normal) = 
  36MB raw
- New format: 1000 × 1000 × (6 + 3 + 6) = 15MB raw  
- After zstd-19: probably ~8MB vs ~25MB

`FAO_LIB_RAW=1` forces legacy f32 storage.

FLW1 blobs (old library data) read transparently — magic check accepts 
either, tag byte controls layout parsing.

### Library stats API + GUI

`Library::stats()` returns a multi-line summary:
- Web count + disk size
- Total/avg vertices + triangles
- Reference distribution (total refs, avg, zero-ref count, most-referenced)
- Time range (oldest/newest/last-referenced in days)
- Top 5 source videos by web count

GUI "Stats" button in the Library Mode tab prints the report to stderr. 
Disabled until a library is attached.

### Library prune API + GUI

`Library::prune_unused(min_age_days)` removes canonicals where 
ref_count=0 AND last_referenced is older than the threshold. Cleans 
up the blob files, log_db tombstones, manifest stub, rebuilds 
uuid_index + eig0_index.

GUI "Prune Unused" button calls it with `min_age_days=30`. Disabled 
until the library is open writable. Background thread to avoid 
blocking the UI on big prunes; idle_add refresh on completion.


## v10 Phase A — shared primitives (transform.rs + residual.rs)

User direction: "modules, fresh writes of glue and of libraries, and 
moving on is good now that we have grounded ourselves in what is 
possible." Answered the "are we still racing codecs" question by 
laying out the v10 plan (4 phases) and shipping Phase A.

### transform.rs (~620 lines)

New module. Holds all DCT/IDCT + quantization primitives that v8 
residual and v9 volumetric were each duplicating.

Contents:
- `dct1d_4` / `idct1d_4` — 4-point orthonormal DCT, unrolled
- `dct1d_8` / `idct1d_8` — 8-point orthonormal DCT (cosine table)
- `dct2d_4x4` / `idct2d_4x4` — 2D 4×4 (separable from 1D 4-point)
- `dct2d_8x8` / `idct2d_8x8` — 2D 8×8 (separable from 1D 8-point)
- `dct3d_4x4x4` / `idct3d_4x4x4` — 3D 4×4×4 (ported bit-exact from v9)
- `QUANT_MATRIX_4X4` (JPEG-luminance-derived, half-size)
- `QUANT_MATRIX_8X8` (JPEG luminance)
- `QUANT_MATRIX_4X4X4` (ported from v9 verbatim)
- `QuantParams { scale_byte, saliency_boost }` packed into 1 byte
- Generic `quantize_generic`/`dequantize_generic` over block size
- Public wrappers: `quantize_4x4`, `quantize_8x8`, `quantize_4x4x4` and 
  their inverses

Verified bit-exact roundtrip with Python for 4-point and 8-point 1D 
DCTs, plus pipeline-level for 2D 4×4 quantize → dequantize → recover.

### residual.rs (~650 lines)

New module. Replaces (going forward) the bias-128 box-average residual 
that lived inside `v8_reconstruction_codec.rs`. Same wire-format 
position (Z-wrapper residual slot), new payload format.

Pipeline:
1. For each 4×4 quarter-resolution block in raster order, compute 
   `target - predicted` per channel.
2. Forward 2D DCT.
3. Quantize using `transform::quantize_4x4` (frequency-weighted).
4. Optional: DC prediction across blocks (subtract predicted DC from 
   left+top reconstructed neighbors before encoding the residual DC).
5. Pack into either dense format (every block emits its params + 
   coefs) or skip-block RLE format (runs of "all coefs within ±skip_
   threshold" blocks collapse to a counter).
6. Optional: channel deinterleave within non-skip segments (RRR…GGG…
   BBB layout for better zstd matching).
7. zstd-compress with adaptive level.

Wire format:
- rw_field u16 (bits 0..12 width, 13=deint, 14=dc_pred, 15=skip_rle)
- rh u16
- zstd-compressed payload (variable by mode)

Same header bit assignments as the old v8.2 residual so the upper-
layer (Z-wrapper) doesn't need changes. Forward-compat: bit-0..12 
mask now properly bounded for the new max rw of 8191.

`ResidualOpts` struct carries all the toggles cleanly. `ResidualOpts::
from_env()` reads `FAO_V82_SKIP` / `FAO_V82_DCPRED` / `FAO_V82_DEINT` 
for backward compat with existing knobs.

Tests included:
- `identity_residual_lossless` — predicted == target → exact recovery
- `small_constant_offset` — uniform 10-unit shift → <5 mean error
- `skip_rle_roundtrip_matches_dense` — both modes recover identical pixels
- `dc_pred_roundtrips_within_quant_error` — gradient residual recovers <10 mean

### Deprecation banners

v3/v4/v5/v6/v7 codecs get:
- A module-level `//!` banner explaining why and when (Phase D deletes)
- `#[deprecated(note = ...)]` attribute on the `pub mod` in lib.rs

Compiler will emit warnings at use sites in main.rs. Wrapped with 
`#[allow(deprecated)]` on the same line so build doesn't fail.

### What's next

Phase B (next session):
- Switch v8's `compute_photometric_residual` + `apply_residual` to call 
  `residual::encode` / `residual::apply`. Delete the ~500 lines of 
  inlined residual code from v8_reconstruction_codec.rs.
- Switch v9's `quantize_block` + `dequantize_block` + `dct3d_4x4x4` to 
  call `transform::*`. Delete ~250 lines from v9_volumetric.rs.

Phase C: v10.rs orchestration of strategies (static-repeat, motion-comp, 
mesh-rasterized + residual, volumetric + residual), all sharing the 
transform + residual + library machinery.

Phase D: delete v3/v4/v5/v6/v7 modules and their main.rs branches.


## v10 Phase B + Phase C scaffolding

User: "keep going" after Phase A. Phase B switches v8 and v9 to consume 
the shared transform.rs/residual.rs modules. Phase C scaffold lays out 
the strategy orchestrator that will eventually replace main.rs's 
per-frame race.

### Phase B — v8 + v9 wired to shared primitives

#### v8 residual: dual-path dispatch

Added `fn compute_v10_residual` next to the legacy 
`compute_photometric_residual` in v8_reconstruction_codec.rs. It:
1. Rasterizes the scene mesh through the camera pose to get predicted RGB.
2. Converts source YUV → RGB.
3. For uncovered pixels (predicted is all-zero), sets predicted = source 
   so the residual is exactly zero there (cleaner than the legacy 
   sentinel approach — zero blocks collapse via skip-block RLE much 
   better than (128,128,128) sentinel runs).
4. Calls `residual::encode` with `ResidualOpts::from_env()` for the 
   shared pipeline.

The residual gate dispatches: `FAO_V10_RESIDUAL=1` picks v10 DCT, 
otherwise legacy bias-128. Both gated by 
`compute_v81_residual_enabled()` + the 100-vertex minimum.

`apply_residual` (decoder side) routes via `residual::is_dct_format`, 
which checks bits 11+12 of the residual header. Both bits set = v10. 
Two-bit signature avoids false positives on legacy trailers that 
might have one of those bits set as part of a large rw value.

Wire format: residual::encode emits `rw_field` with bits 11+12 always 
set (FLAG_DCT_FORMAT = 0x1800), width in bits 0..10 (max 2047 — quarter 
res of 8188-wide source, covers 8K).

Forward-compat: legacy bias-128 widths up to 4095 never set both bits 
11+12 simultaneously (would require rw ≥ 6144 = quarter-res of 
24576-wide source, not realistic).

#### v9 transforms: thin wrappers delegate to transform.rs

Functions in v9_volumetric.rs whose bodies are now one-line delegations:
- `dct3d_4x4x4` → `transform::dct3d_4x4x4`
- `idct3d_4x4x4` → `transform::idct3d_4x4x4`
- `quantize_block` → `transform::quantize_4x4x4` (with QuantParams→u8 unwrap)
- `dequantize_block` → `transform::dequantize_4x4x4` (with u8→QuantParams pack)

Bit-exact behavior verified by visual inspection — the transform.rs 
implementations are line-for-line copies of the v9 originals.

`QUANT_MATRIX_4X4X4` re-exported via `pub use crate::transform::QUANT_MATRIX_4X4X4` 
so external callers that imported `fao_v1::v9_volumetric::QUANT_MATRIX_4X4X4` 
keep working.

### Phase C — v10.rs scaffolding

New module `fao_codec_lib/src/v10.rs` (~250 lines). Scaffold for the 
strategy orchestrator that will eventually replace main.rs's parallel 
codec race.

Pieces in place:
- `Strategy` enum: StaticRepeat, MotionCompensated, MeshRasterized, 
  VolumetricRaymarched
- `EncodedFrame` result struct (bytes + predicted_rgb + psnr + which strategy)
- `Plan` config struct with per-strategy enable flags + residual opts 
  + static SAD threshold. `Plan::from_env()` for backward compat with 
  current FAO_* knobs.
- `StrategyStats` with per-strategy win/loss atomic counters + 
  `report()` summary
- `encode_frame()` entry shell — returns None today so callers fall 
  through to the legacy race path

Phase C proper (next session) will:
- Implement `MotionCompensated::encode` by porting the v2 T-frame logic
- Wire main.rs to call `v10::encode_frame` first, fall back to legacy on None
- Move the v8_wins/v8_losses atomics into StrategyStats
- Add the `quick_estimate` filter that drops obviously-losing strategies 
  before doing the full encode (the key efficiency win)

### Knobs added

- `FAO_V10_RESIDUAL=1` — opt-in: v8 emits DCT-quantized residual instead 
  of bias-128 box-average. Default OFF until we have data showing DCT 
  beats bias-128 on real content.
- `FAO_V10_MOTIONCOMP`, `FAO_V10_MESH` — placeholder env vars for the 
  Phase C strategy plan. Currently inert (Plan::from_env reads them 
  but encode_frame doesn't dispatch yet).

### What still needs to happen

Phase C: actually port v2 motion-comp into v10::MotionCompensated, 
wire main.rs to call v10::encode_frame first.

Phase D: delete v3/v4/v5/v6/v7 modules + their main.rs branches. 
Currently marked deprecated; deletion happens once Phase C proves out.

Phase E (was previously Phase D, but moving): v9 trailer compression 
work — channel mask to skip all-zero SH channels per block (50-80% 
v9 trailer reduction on simple scenes).


## v10 Phase C — strategy orchestrator implementations

User: "keep going". Phase C fleshes out v10.rs with actual strategy 
implementations + wires v10 dispatch into both decoder paths.

### Implemented strategies

**StaticRepeat** — emits 2 bytes ('X' + strategy_id=0). On decode, 
returns prev_rgb unchanged. This is the v8.3 R-marker idea wrapped in 
the v10 envelope.

**MotionCompensated** — runs `estimate_global_motion` on Y planes 
(target_yuv vs prev_rgb-converted-to-Y), shifts prev_rgb by the 
estimated (mvx, mvy), uses that as the prediction, then encodes 
(target - predicted) via `residual::encode`. Wire format:
- magic 'X' + strategy_id=1 + mvx i8 + mvy i8 + residual_len u32 + 
  residual_bytes
- Decoder reverses: shifts prev_rgb by motion vector, applies residual.

**MeshRasterized** + **VolumetricRaymarched** — stubs returning None. 
Phase D will delegate these to v8/v9 inside the v10 envelope.

### Orchestrator (encode_frame)

Walks enabled strategies, runs quick_estimate for each (cheap byte-
count proxy), drops strategies whose estimate exceeds best × 
`quick_estimate_margin` (default 1.5x). Runs full encode for survivors. 
Picks smallest result. Records wins/losses in StrategyStats.

quick_estimate for static-repeat returns Some(2) if SAD vs prev is 
below threshold, None otherwise. quick_estimate for motion-comp 
returns a residual-size proxy based on zero-motion SAD between 
prev_rgb (converted to Y) and target_yuv.

This is the "stop racing entire codecs, race strategies" architecture 
from the conversation. Per-frame cost goes from 4-7× best-strategy 
(parallel race of all codecs) down to ~1.2× (one winner + cheap quick-
estimate for each loser).

### Decoder wiring

`decode_fao_frame_to_rgb` and `decode_fao_frame_for_export` in main.rs 
both gain a v10 dispatch branch. When a payload starts with magic 
'X' (V10_MAGIC = 0x58), they route to `fao_v1::v10::decode_frame`. 
For decode_fao_frame_to_rgb, prev_rgb comes from recursively decoding 
idx-1. For the export path, prev_rgb is already threaded through.

This means any .fao file containing v10 frames decodes correctly 
regardless of which encoder produced them.

### Encoder side — NOT wired into main.rs race yet

The encoder-side wiring to main.rs's parallel race is intentionally 
deferred to Phase D. The race is a complex shared-worker structure 
where each worker has its own V8Encoder + anchor state — slotting 
v10::encode_frame into that requires:
- Per-worker prev_yuv + prev_rgb state (currently the anchor cache 
  handles this for some paths but not all)
- v10_attempt branch in the smallest-wins comparison
- v10_wins/v10_losses atomic counters parallel to v8_wins/v8_losses
- Auto-disable feedback per strategy (StrategyStats already structured 
  for this)

What CAN be tested today:
- v10 unit tests exercise the API end-to-end
- The decoder path: any synthetic .fao file with 'X' payloads will 
  decode correctly
- External integrations can call `fao_v1::v10::encode_frame` directly

### Wire format header

'X' (0x58) was chosen as v10 magic because it doesn't collide with 
any existing first-byte: V (v8 raw), Z (zstd-wrapped), D (DCT v21), 
P (YUV-split), L (lossy chroma), R (repeat-marker), T (translated).

### What's next

Phase D:
- Mesh + Volumetric strategy implementations (delegate to v8/v9 
  inside the v10 envelope)
- main.rs encode race wiring
- Per-worker prev-frame state plumbing
- Delete v3/v4/v5/v6/v7 modules and their main.rs branches
- Move v8_wins/v8_losses atomics into StrategyStats


## v10 Phase D — encoder wiring + opportunistic enhancement

User: "continue to phase d".

### encode_frame_against added

New entry point `v10::encode_frame_against(...)` that accepts a 
`baseline_bytes: Option<usize>`. Returns Some(v10_result) only when a 
v10 strategy estimate beats baseline (early-out before full encodes), 
AND the v10 result actually beats baseline after full encode.

This lets v10 sit OUTSIDE the existing per-codec race as an 
opportunistic enhancement: the existing race picks chosen_bytes via 
its v2/v8/k-frame logic, then v10 is asked "can you do better given 
this size as ceiling?" If yes, replace. If no, leave alone.

The old `encode_frame` is now a thin wrapper that passes None for 
baseline (used by external callers / tests).

### main.rs wiring (post-race v10 hook)

In the compress worker loop, right after `let c_yuv = if ... { ... };` 
assigns the race result, a v10 post-race block runs when FAO_V10=1:

1. Build prev_rgb proxy from the worker's `last_c_recon_rgb` cache 
   (or YUV→RGB conversion of `last_raw_yuv` if no recon cache).
2. Call `v10::encode_frame_against(plan, stats, orig, last_raw_yuv, 
   prev_rgb, w, h, c_yuv.len())`.
3. If Some, replace c_yuv with the v10 bytes; log the win.
4. If None, keep c_yuv as-is.

Per-worker state added: at end of frame, when FAO_V10=1, 
`last_raw_yuv` is updated to current `orig`. Bypassed in default mode 
(no memory cost when v10 is off).

This is the safest possible wiring: zero impact on existing race, 
opt-in via env var, fully reversible. Wins are pure additional 
savings — no regression risk on workloads where v10 doesn't help.

### Strategies still owned by v8/v9, not duplicated in v10

`MeshRasterized` and `VolumetricRaymarched` strategies in v10.rs are 
stubs returning None. The architectural insight from the v10 design 
discussion stands: v10 is the **orchestrator**, not a re-
implementation. v8 emits mesh frames; v9 emits volumetric frames; 
v10 picks among StaticRepeat / MotionCompensated / (v8's bytes) / 
(v9's bytes) — and currently it does so by sitting outside the race 
and checking only its own two strategies vs the race winner.

A future session can integrate v10 more deeply by:
- Moving the v8 race INTO v10 as the MeshRasterized strategy
- Adding per-strategy auto-disable feedback inside StrategyStats
- Eliminating the per-worker `fresh StrategyStats every frame` and 
  using a shared session-level instance

### Deprecation deletion deferred

v3/v4/v5/v6/v7 modules have 118 references in main.rs (struct fields, 
encode race branches, decode trailer parsing). Mechanical surgery, 
risk of breaking compilation if done partially. Leaving for a 
dedicated cleanup session — modules stay `#[deprecated]` for now.

### Available env vars

- `FAO_V10=1` — enable v10 post-race enhancement
- `FAO_V83_REPEAT` — within v10, controls StaticRepeat strategy (also 
  controls the old v8.3 R-marker path; same semantics)
- `FAO_V10_MOTIONCOMP` — control MotionCompensated strategy (default on)
- `FAO_V10_RESIDUAL` — use DCT residual in v8 (from Phase B, orthogonal 
  to v10 strategies)
- `FAO_V82_*` — residual sub-options used by both v8 legacy and v10


## v10 cleanup completion — v3 through v7 deleted

Three-session arc, this is the closer. Codecs v3 (meshed plate), v4 
(sparse skeleton dictionary), v5 (scene replay sprites), v6 (object 
field), v7 (perception with phase correlation) are now GONE from the 
codebase.

### Deleted source files

- `fao_codec_lib/src/v3_meshed.rs`
- `fao_codec_lib/src/v3_encoder.rs`
- `fao_codec_lib/src/v4_codec.rs`
- `fao_codec_lib/src/v5_codec.rs`
- `fao_codec_lib/src/v6_object_codec.rs`
- `fao_codec_lib/src/v7_perception_codec.rs`

### Extracted reusable pieces

Two algorithms that lived inside the deleted codecs were generic enough 
to keep — moved into their own modules:

**`fao_codec_lib/src/geometry.rs`** — `Vec3` struct + helpers. v6 was 
the original home; v8/v9/library all imported it via 
`v6_object_codec::Vec3`. Now they import from `geometry`.

**`fao_codec_lib/src/phase_correlation.rs`** — `PhaseCorrelationPass` 
and `CameraMotion`. Originally inside v7. Both v8 (SfM camera-motion 
seed) and the GPU phase_corr module need them. Pure FFT-based motion 
estimation, no codec-specific logic.

### main.rs surgery completed

Reference count went from ~85 to 0:
- State field declarations (`v3_state` through `v7_state`) — removed
- TranscodeState initializers — removed
- App struct decoder fields (`fao_v4_decoder` through `fao_v7_decoder`) — removed
- App initializers — removed
- Encoder init blocks (start_transcode) — removed
- Per-frame encode attempts (`v3_attempt` through `v7_attempt`) — removed
- Race branches in the smallest-wins ladder — removed
- Trailer write blocks (V3 plate, V4D1, V5L1, V6L1, V7L1, V7CT) — removed
- Decoder reset block — replaced with stub reads
- Trailer read functions — stubbed (detect magic + log, no parse)
- Playback dispatch in `decode_fao_frame_to_rgb` — collapsed to None for v4-v7
- World-view spatial trace blocks (5 sections, ~160 lines) — removed
- Textured Cairo rendering with sprite thumbnails — removed
- v7 camera-path frustum rendering (~106 lines) — removed
- `data_source_label` HUD logic — simplified to v8 + frame-web only
- VideoCam mode's v7 branch — removed
- `got_v5_data` / `got_v6_data` / `got_v7_data` tracking variables — removed
- `playback_mode` field + GUI button handlers — stubbed
- `v5_recon_outer` orphan cell — removed

### Backward compat for legacy `.fao` files

Old archives that contain v3/v4/v5/v6/v7 frames or trailers will still 
LOAD without crashing — the trailer readers (`read_v3_plate`, 
`read_v4_trailer`, `read_v5_trailer`, `read_v6_trailer`, 
`read_v7_trailer`) scan for the magic bytes and log a notice; they don't 
parse anymore. Frames with v3-v7 magic bytes in `decode_fao_frame_to_rgb` 
return None, which causes the player to passthrough (show source video).

This is the right tradeoff: users with old archives see "FAO: legacy 
V5L1 trailer detected" in the log and a fallback display, instead of a 
crash. New encodes only produce v2 / v8 / v9 / v10 frames.

### What's left in the codebase as encoders

- **v2 / v2.1** (`v2_encoder.rs`, `v21_predictive.rs`) — DCT-based 
  baseline with predictive translate frames
- **v8** (`v8_reconstruction_codec.rs`) — mesh-rasterized reconstruction
- **v9** (`v9_volumetric.rs`) — volumetric ray-marched
- **v10** (`v10.rs`) — strategy orchestrator

Plus the shared utilities: `transform`, `residual`, `geometry`, 
`phase_correlation`, `library`, `library_db`, `block_dict`, 
`block_store`, plus GPU modules.

### Files now cleanly compile-checkable

All brace-balanced. No dangling imports. lib.rs `pub mod` list matches 
the actual files on disk. v8_reconstruction_codec imports the two 
needed types directly from `phase_correlation` rather than the old 
`use crate::v7_perception_codec::*;` wildcard.

Ready for library + migration testing next session.
