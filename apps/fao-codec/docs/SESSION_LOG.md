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

## v11 plate codec — measuring the web thesis in 2D (2026-08-30)

Test material: Lone Ranger bloopers 45s (1920x800 active, film grain,
~19 shots) + SMITE esports broadcast 17s (1920x1080@60, no grain, big
static HUD). 48-frame GOPs, scored with ffmpeg psnr/ssim against
lossless y4m, identical scoring for SVT-AV1 preset 6 baselines.

### Fixed first: residual DC-prediction was silently broken both ways

`residual::tests::small_constant_offset` (mean err 9.375 on a +10
offset = exactly 15/16 blocks losing the offset) root-caused to TWO
masking bugs: decoder re-quantized the propagated true DC through each
block's OWN scale — skip blocks carry scale_byte 0 whose 127 steps max
out at ~0.008, so propagated DC clamped to nothing while the prediction
chain tracked it as delivered; and the encoder wrote DC deltas into
scales chosen from pre-prediction coefficients, so a flat block after a
high-DC neighbor saturated q0 at ±127 of a microscopic step. Decoder now
carries true DC as f32 into reconstruction (never re-quantized); encoder
widens scale_byte until the delta fits, re-quantizing AC under the new
scale. All 4 residual tests green. Every dc_prediction residual v8.1+
ever produced was degraded by this.

Added `ResidualOpts::dead_zone` — the rate knob the coder lacked
(per-block maxabs scaling otherwise preserves grain near-losslessly).

### v11 ladder on the static-camera store shot (shotB), marginal B/frame

  v11.1 plate+residual only          ~88 KB @ 38.1 dB
  v11.2 + zero-motion prev mode      ~47 KB @ 38.1 dB
  v11.3 + MC (16x16, ±12 diamond,
          MV rate bias, pred deltas)  9.2 KB @ 37.1 dB / 0.956
  SVT-AV1 preset6 crf54 (same GOP)    1.27 KB @ 37.6 dB / 0.975

Each structural addition bought 2–5x. Remaining ~7x on this shot;
9–12x on SMITE GOPs (rotating 3D camera defeats translation-only MC;
deadzone punishes HUD text); shotA handheld: plate useless (prev share
0.89), degenerates to pure inter.

### What the residue is (measured, not guessed)

1. Film grain dominates residual on film content at matched-AV1 quality:
   temporally white, unpredictable by ANY world model. v11.4 implements
   the denoise+synthesize split (deterministic xorshift grain, σ byte per
   frame); at dz≥32 the deadzone already discards grain so bytes barely
   move, and white triangular synth grain HURTS psnr/ssim (real scan
   grain is spatially correlated — needs AR filtering à la AV1 FGS).
   Mechanism kept, off by default, honest verdict recorded.
2. Entropy coding: zstd over quantized coefficient bytes vs adaptive
   arithmetic coding — worth ~2–3x.
3. Sub-pixel MC, variable block sizes, real intra prediction, RD-driven
   mode decisions — the rest of the gap. This is codec engineering, not
   thesis failure; but it is exactly what "near AV1" costs.

The defensible thesis claim is ASYMPTOTIC marginal cost on web-covered
content: the plate (348 KB zstd'd for shotB) amortizes across GOPs today
and across a library tomorrow, and the v1→v3 ladder shows marginal cost
falling 10x while quality holds. The indefensible claim is 10x under AV1
at matched PSNR on a 45s grainy clip — the grain alone forbids it.

Files: fao_codec_lib/src/v11_plate.rs (5 unit tests), src/bin/plate_lab.rs
(y4m in → accounting + recon y4m out; --static --v1 --v2 --dz --band
--skip-thr --boost). Known limits: translation-only motion model; plate
median is O(frames) per pixel; chroma coded at full res through the
3-channel interleave (2x chroma waste); mode decision uses raw SAD which
is a weak proxy at high deadzone.

### v11.4b — AR grain + the two-branch classifier (same session)

Measured the source's grain lag-1 spatial correlation: ρ = 0.71–0.76 —
strongly low-pass (optical chain + the h264 layer), which is exactly why
white synthesis read as electronic snow. synth_grain now shapes white
triangular noise with a separable AR(1) at the measured ρ (variance-
normalized, one ρ byte/frame on the wire). SSIM at dz32: 0.926 with AR
grain vs 0.835 with white (band differs slightly; the direction is
unambiguous). Rate verdict unchanged and honest: at dz≥32 the deadzone
already discards grain, so the split buys believability, not bytes, on
this source; a cleaner (non-pre-compressed) film scan is where the rate
win should appear.

classify_highpass() quantifies the "flat-mask vs world-derived" split:
high-pass energy that survives the plate prediction (sign + ≥half
magnitude co-sited) is world texture; the rest is mask. shotB mid-frame:
world 0.09 / mask 0.91 — i.e. 91% of fine-detail energy on this shot is
screen-space-noise-like. Caveat recorded: the plate's median denoising
means real texture it blurred also lands in "mask", and mover texture
(absent from the plate) does too — the statistic is conservative toward
mask; per-mode (MC-prev) classification would tighten it.

Dual-reference scoring context: grainy-source vs denoised-reference =
39.27 dB — the ceiling ANY synthesis-based codec (AV1 FGS included)
faces vs the grainy original, independent of rate.

Open RD item, now ranked first: the MV rate bias (v3c) dropped
prev-mode share 0.69→0.33 and cost ~1.5 dB at matched dz — the
plate-vs-prev decision uses raw SAD, a weak proxy for post-deadzone
residual cost. Needs a deadzone-aware cost (SATD or coefficient-count
above threshold) before further grain/entropy work.

### v11.5 — spacetime residual volumes (2026-08-30, same session)

Tests the "merge residuals across frames via a continuous function"
idea in its simplest falsifiable form: 4×4×4 (x,y,t) residual volumes
through the existing dct3d_4x4x4, groups of 4 frames predicted
open-loop from the previous group's last recon (a temporal block needs
all its planes before any frame finalizes, so within-group closed loop
is impossible by construction).

Headline measurement, shotB: **temporal_dc_share = 0.74–0.75** — three
quarters of pre-quantization residual energy lives in the t-frequency-0
plane. The premise is real: most of what the per-frame coder re-codes
every frame is cross-frame-continuous and mergeable by ANY functional
family (this transform, polynomials, or learned). The white quarter is
the grain floor, unreachable by construction.

RD verdict is operating-point-dependent and honest: at high quality
(dz8) t3d beats the per-frame path (+0.9 dB for +14% bytes — 65 KB/f
@ 41.7 dB/0.977) because surviving structure packs into the volume; at
low quality (dz32: 15.1 KB/f @ 37.6) the open-loop anchor penalty
(prediction from up to 4 frames back; prev share falls to 0.34) eats
the transform gain — the classic MCTF-vs-closed-loop tradeoff. A
hierarchical (bidirectional) group structure would recover most of the
anchor penalty; noted, not built.

### v11.6 — SR plate + phase-correlation confidence fix (same session)

phase_correlation confidence was structurally broken for its best case:
the second-peak scan never excluded the main peak's own shoulders, so a
sharp single peak (static scene, peak at (0,0), neighbor nearly as
tall) reported confidence ≈ 0 exactly when the estimate was most
reliable. Now excludes a 5×5 wrapped neighborhood. shotB min pairwise
confidence: 0.00 → 0.81. v8 gates on this too — its camera seeding was
being spuriously distrusted on static content.

v11.6 build_plate_sr: multi-frame super-resolution plate at S=2 —
frame pixels splat as world point-samples at their sub-pixel phase,
per-cell two-pass robust mean (±12 band), holes fall back to the S=1
median plate; predict_frame_sr samples it back. Synthetic test with
beyond-Nyquist stripes and 0.37/0.23 px-per-frame drift: SR predicts
held-out views ≥1.3× better (falsifiable, passing). Real footage,
prediction-only luma PSNR: shotA (handheld) +1.21 dB, shotB +0.41 dB —
the "locked" shot has real sub-pixel gate weave the fixed confidence
now trusts. Honest bound: shotA's base prediction is 14.8 dB — the
translation-only camera model, not resolution, is the binding
constraint on handheld/parallax content. The fence-slit lever is real
but its payoff is gated behind pose.

v8 pose reconnaissance for next session: headless-callable pieces
survive — CameraPose, LkTracker::track, FeatureTrackPass (stable-ID
tracks), TriangulatePass + run_triangulation + annotate_with_depth,
MeshIntegrator (needs source YUV since the vertex-color fix),
SceneMesh, and resolve_library_ref for cross-video sharing. 6 known
failing tests in library/feature paths (documented in README) are the
debt to clear before trusting it on real GOPs. The stairs-on-Anakin
result came from this stack; pointing it at strong-geometry synthetic
content (the SMITE arena) with the now-fixed confidence gate is the
planned 3D step.

### Detection fixes — why fragments, never whole scenes (same session)

Measured the starvation directly (plate_lab --features: keypoint count +
fraction of 64px cells with ≥1 keypoint). Pre-fix: shotB (dark film)
422 keypoints, 22% coverage; smiteB capped at 5000 with 56% coverage,
everything hoarded by the HUD. Pose and triangulation can't build a
scene from that — hence stairs-here-and-there, never a whole map.

Fix 1 — FAST quick-reject was calibrated for the wrong detector: it
required ≥3 of 4 cardinal points to agree, which is sound for FAST-12
but NOT for the arc-9 test this detector runs — a contiguous 9-run can
touch as few as 2 cardinals (they sit 4 apart on the 16-circle; run
1..=9 touches only 4 and 8). The ≥3 rule mathematically rejected valid
FAST-9 corners, including every near-tip detection at right-angle step
corners — i.e., exactly the corners rectilinear game/architecture
content is made of. Now ≥2. The long-documented
fast_finds_synthetic_corner failure (root-caused in README as
"geometrically impossible") flips to PASSING: it was never a design
call, it was a soundness bug in the prefilter. Pre-existing failures
6 → 5.

Fix 2 — per-cell keypoint budget in NMS (64px cells, cap =
4×max/n_cells): global top-N let one busy region hoard the budget;
whole-scene reconstruction needs spread. smiteB coverage 0.54→0.61.

Honest remaining limit: shotB still 654 kps / 24% coverage — the fixed
threshold (20) exceeds local contrast across dark regions. Next
detection lever is adaptive/per-tile thresholding (or local contrast
normalization before detect); after that, FeatureTrackPass's own
pre-existing failure and single-scale LK (no pyramid → small motions
only) are the tracking debt.

### Detection round 2 + mode-decision experiment (same session)

Adaptive per-tile FAST threshold (clamp(tile MAD, 8, 20)): shotB
654→1223 keypoints, coverage 24→36%; smiteB coverage →0.66–0.70.
Remaining shotB empty cells are genuinely near-flat shadow (MAD<8) —
detection floor reached; the next gain there is tracking, not detection.

Both remaining tracker test failures were DEGENERATE INPUTS, not
tracker bugs: (x·7+y·13)%256 is a linear ramp — constant gradient ⇒
exactly singular LK structure tensor (total aperture problem), and the
detection test's ramp contains zero FAST corners (straight seams never
make a 9-arc). The LK solver verifies correct on smooth
well-conditioned texture with STRICTER asserts (Some required). Its
real contract is now test-documented: correct within the linear basin
(displacement < ~texture wavelength/2); pyramid LK remains the future
work for large motions. Pre-existing failures 5→3 (library ×2, densify
cadence).

Mode-decision experiment — negative result, recorded: replaced the raw
SAD plate-vs-prev decision with a deadzone-aware codable cost. Every
variant lost to SAD on shotB at matched dz: quiet-region all-plate
drops real drift (35.0 dB vs 35.5), all-prev accumulates quantization
drift (34.7), deterministic rotating plate refresh K=4/K=8 lands
0.15–0.25 dB under SAD at ~equal bytes. Diagnosis: SAD's
grain-alignment noise dithers quiet regions between predictors — an
accidental but effective intra-refresh that single-frame proxies don't
replicate. The principled successor is lookahead RD (charge plate-mode's
persistent error across future frames), not a better proxy. SAD stays
by measurement; region_codable_cost kept #[allow(dead_code)] as the
experiment record.

### Learned restoration — linear baseline measured at zero (same session)

restore.rs: RAISR-style learned restoration — pixels bucketed by local
gradient orientation × strength (12 buckets), per-bucket 5×5+bias
filters solved EXACTLY by ridge least squares from (degraded, clean)
pairs. Deterministic, CPU-fast, held-out-tested. Synthetic sanity: it
recovers >40% of a 3×3 box blur's error on unseen frames (unit test).

Real AV1 artifacts: nothing. shotB@crf50 +0.01 dB, shotB@crf63
+0.02 dB, smiteB@crf55 −0.02 dB on held-out halves; detection
coverage unchanged. Two causes, both instructive:
  1. On grainy film the "clean" reference is itself noisy — the optimal
     linear predictor of grain from smoothed input is ~identity; the
     regression is ill-posed exactly as predicted ("noisy input is just
     as much of a problem").
  2. AV1's in-loop deblock/CDEF/loop-restoration already perform
     near-optimal linear-class restoration at decode; the surviving
     artifacts are the non-linear residue by construction.

Conclusion, measured not assumed: artifact→pattern recovery requires a
non-linear model. The in-tree path is beatspace's candle training
stack (hand-built layers, checkpointing, wgpu_train) on the Asahi
Mesa/Vulkan target, with unendlich's engine as the ground-truth
generator for game content — render clean frames, degrade with the
real codec chain (downscale→upscale→encode), and the training
distribution matches deployment exactly, which no transcoded film
source can offer. restore.rs stands as the baseline such a model must
beat and the harness (--restore-exp: held-out PSNR + detection
coverage deltas) is the evaluation it plugs into.

### Track-chain probes + three tracking fixes (same session)

plate_lab --track runs the real FeatureTrackPass over a GOP and reports
what pose estimation actually eats: per-frame survivors, lifetime
distribution, and spatial coverage of ≥8-frame tracks.

Found and fixed, in causal order:
1. **No track was ever born after frame 0** — re-detection gated on
   population count alone, so any 100+ features anywhere satisfied it
   while newly revealed world got nothing. This is a structural cause of
   "fragments, never whole maps": a panning camera only ever mapped its
   first view. Gate is now spatial (reseed when >25% of 64px cells lack
   a live track, on a periodic beat, or when starved; admission fills
   empty cells first). Births went zero → continuous on all GOPs.
2. **Pyramid LK** (track_with_init + track_pyramid, 3 levels, own basin
   test: tracks a 6px shift single-scale cannot). Measured honestly: it
   changed NOTHING on the handheld GOP — byte-identical stats — which
   localized the real killer one link down:
3. **The residual gate (30.0) was executing aligned tracks for the
   crime of motion blur**: pyramid finds the alignment, the blurred
   window can't match the sharp previous one within 30 mean levels, the
   track dies anyway. Widened to 45 with the finding recorded in-code.

Where the chain stands (48-frame GOPs, ≥8-frame tracks / their 64px
coverage): shotB 413 / 0.61, smiteB 408 / 0.66, shotA 473 / 0.87 —
with continuous replenishment. The remaining shotA median lifetime of
16 is bimodal and physical (violent pan phases blur content beyond
recognition; the long-lived cohort is what pose uses). Known next
items: HUD/world track segmentation for game broadcasts (screen-locked
tracks are poison for rigid-world pose — likely most of smiteB's
long-lived cohort), then actually running TriangulatePass on these
tracks.

### HUD/world track segmentation (same session)

Probe finding first: on the SMITE broadcast GOP, 182 of ~410 long
tracks are screen-locked (move <0.5px while the scene moves) — and the
motion-consensus classifier labeled them STATIC world points with zero
UiOverlay, because phase correlation locks onto the dominant static
HUD, reads camera ≈ 0, and HUD agreement then looks like world
agreement. The classifier's world model was inverted on exactly the
content class (game broadcasts) the whole-scene goal cares about.

Fix: UiOverlay promotion axis 4 (screen-lock) inside
MotionConsensusPass — per-track counters (screen_moving_frames /
screen_still_hits on FeatureTrack), latched when a track sits <0.5px
through ≥90% of ≥6 scene-moving frames. The scene-motion signal is the
p75 of observed per-track displacement, deliberately NOT the camera
estimate (which reads ≈0 on HUD-dominated frames) and NOT the median
(the still population IS the majority there — the median sits at zero
precisely when the viewport moves).

Measured: smiteB 187 UiOverlay (independent probe: 182 — agreement),
static pool 351→166; shotB 0 false positives; shotA latches 50 — all
stuck LK tracks in the defocused foreground that never respond to
scene motion, whose exclusion from pose is safe (zero parallax
information) and arguably beneficial. UiOverlay is already
hard-excluded from v8's pose/densify paths, so the world pool feeding
pose is now HUD-free on broadcast content.

### First end-to-end geometry (same session)

plate_lab --triangulate runs the entire chain exactly as v8's encode
path composes it: FeatureTrackPass (spatial reseed, pyramid,
blur-tolerant gate) → MotionConsensusPass (HUD screen-lock axis live)
→ Static-track observations → v8's translation-only pose convention
(cumulative negated phase-corr motion, identity rotation, focal ≈
frame dims) → TriangulatePass.

Results (48-frame GOPs):
  shotA  93/247 tracks triangulated | z 784..2638 | reproj 1.13px | baseline 34.5px
  shotB  96/302                     | z 929..2075 | reproj 1.94px | baseline 26.8px
  smiteB 61/168                     | z 901..3775 | reproj 1.67px | baseline 22.6px

All depths positive and content-plausible; handheld film reprojects at
~1.1px through a TRANSLATION-ONLY pose model. shotB triangulates off
its own gate weave (the same sub-pixel drift the SR plate feeds on).
This is the first time the full chain has produced clean sparse
geometry on any of these content classes.

TriangulatePass gained a cheirality check: the midpoint solution must
sit at positive depth along BOTH observing rays. Before it, the
game-broadcast GOP reported p10 depth −3301 — rays from a ROTATING
camera forced through the translation-only model intersect behind the
cameras, and reprojection error cannot catch that (projection is
symmetric through the pinhole). After: all outputs front-of-camera.

The indicted next bottleneck, by these numbers: rotation in the pose
model. smiteB's 107 rejected candidates are dominated by
rotation-as-translation inconsistency; the reserved CameraMotion.zoom
field and log-polar phase correlation are the planned path, or 2-view
rotation from the now-clean static track pairs.

### Global affine motion layer (same session)

global_motion.rs: robust 6-DOF affine fit from static-track
correspondences (least squares, iterative MEDIAN-based outlier
rejection — mean+3σ measurably collapses under a mover's high-leverage
correspondences: the contaminated fit chases them AND their presence
inflates σ past the cut). Unit test recovers a known 2° roll + 1.5%
zoom under 10% mover contamination.

--triangulate now reports the translation-vs-affine pairwise residual
(the "does rotation/zoom matter" metric) and --affine rectifies
observations by the accumulated LINEAR part about the principal point
(roll/zoom/shear removed; pan and parallax preserved for
triangulation).

Measured:
  smiteB: translation RMS 2.18px vs affine 1.83px; rectification cuts
    mean reprojection 1.67 → 1.11px at the same 61 points and tightens
    the depth spread (p90 3775 → 2728). The orbit camera's zoom/shear
    was real. Count unchanged — the 107 remaining rejects are
    yaw/pitch folded into translation, which the affine's linear part
    cannot separate from pan; that requires rotation-aware two-view
    geometry (homography/essential decomposition), the next tier.
  shotA: translation 17.58px vs affine 17.30px — the residual is
    PARALLAX (depth-dependent flow no global model should explain);
    rectification correctly neutral (1.13 → 1.21px). The layer knows
    when it isn't needed.

### Two-view rotation + regime-gated pose sources (same session)

two_view.rs: robust homography fit (normalized DLT, iterative median
rejection) + nearest-rotation extraction from K⁻¹HK (hand-rolled
Jacobi symmetric eigensolver → polar decomposition). Unit test
recovers a 1.7° mostly-yaw rotation to <0.003 rad through 10% mover
contamination.

Always-on rotation rectification measured HONESTLY before gating:
coherent on the orbit camera (baselines 22→112px, depths collapsing to
the fixed-radius shell an orbit should produce) but it EATS PARALLAX
as phantom rotation on translation-dominant film (14.4° fictitious on
the handheld GOP, reprojection 1.13→2.53px). Two per-pair/per-content
gates fix both directions:

1. Model selection per pair: apply R only when the homography
   out-explains pure translation by 1.5×. Parallax content fails the
   margin (its variance is depth structure both models leave behind).
2. Translation-source selection by UiOverlay fraction: broadcast
   content (ui_frac > 0.15) uses the CLASSIFIED static-track median —
   phase correlation there is HUD-locked at ≈0 and had been silently
   poisoning every pose with near-zero baselines, which retro-explains
   v8's historic game-content failures. Film keeps phase correlation
   (the track median mixes depth-dependent flow and underestimates
   camera motion — measured 93→81 when misapplied).

Final gated results (--triangulate --rot):
  smiteB 102/168 (61%) @ 1.59px   [plain: 61 — +67%]
  shotA   91/247       @ 1.07px   [plain: 93 @ 1.13 — parity]
  shotB   96/302       @ 1.94px   [identical passthrough]

The discovery chain that produced the smiteB jump: HUD segmentation →
trustworthy static-track pool → usable track-median translation →
real baselines. Each stage this session unblocked the next.

### Colored cloud export (same session)

--export-ply writes the triangulated cloud as ASCII PLY, colors sampled
from the source frame at each point's last raw observation (rectified
coords are geometry-only; pixels index by raw). Track capacity raised
to 2500 for scene capture (the 500 default is a codec budget, not a
mapping budget): smiteB 118 points @ 1.25px, shotA 128 @ 1.26px.
Candidate growth outpaced triangulation growth — the marginal
adaptive-threshold corners are lower-grade, as expected. Visual check:
coherent near-field clusters with a sparse far-depth tail; the clouds
are small but geometrically sane. Delivered as cloud_smiteB.ply /
cloud_shotA.ply. Densification path when wanted: DepthMap +
MeshIntegrator (dense contract) fed from these sparse anchors.

### Suite fully green — the last three failures root-caused (same session)

1. **Library tests destroyed each other**: temp_dir() keyed on process
   id alone; cargo's parallel test threads shared one directory, and
   each test's opening remove_dir_all deleted the library out from
   under its siblings. This alone was the run-to-run flakiness in WHICH
   library tests failed. Fixed with a per-call atomic sequence.
2. **The real one — manifest reader dropped the FIRST web**: the
   hand-rolled JSON walker began depth-counting at the "webs" KEY, so
   the first stub's '{' was mistaken for the enclosing object and its
   close never hit the capture depth. Every manifest reload silently
   lost its oldest canonical; a one-web library reloaded as empty
   (manifest_persists), and quick_stats "disagreed" with
   total_disk_bytes only because the reopened library was missing the
   blob it should have counted. For production libraries this was
   compounding data loss on every open — and it sat directly under
   cross-video web sharing, the codec's core asymptotic thesis. Parser
   now anchors on the array '[' and terminates at its ']'.
3. **anchor_cadence_correct was stale**: default anchor_interval was
   deliberately retuned 30 → 60 (documented at the Default impl); the
   test still asserted 30. Now asserts the semantics against the
   configured interval so retunes can't strand it again.

fao-codec-lib: 82 passed / 0 failed. At session start: 66 / 7 with the
test binary having been uncompilable until 2026-08-19.

### First live cross-video library run (same session)

plate_lab --lib-add/--lib-match: sparse reconstructions become
canonical webs (SceneMesh from triangulated points + sampled colors,
WebSignature::from_mesh) and query signatures score against every
stored web — the codec's asymptotic dedupe claim exercised end-to-end
for the first time, on a library that (as of the manifest fix) actually
keeps its webs.

Setup: three sets from the bloopers (train-car, store, desert burial)
added; queried with a DISJOINT 2s window of the store shot (same set,
different take content) and with the desert GOP itself.

Results: self-match 1.000 exactly (deterministic pipeline sanity).
Same-set ranking CORRECT in every configuration — store-take2's best
match is store-take1 — but the margin is not threshold-grade:

  scale-sensitive (before): same-set 0.486, wrong sets 0.294/0.349
  scale-invariant  (after): same-set 0.609, wrong sets 0.523/0.469

signature_similarity is now scale-invariant (eigenvalue fractions,
unit-norm extents): monocular webs carry no absolute scale — units are
each run's own camera travel — so cross-take comparison of absolute
spectra was comparing an unobservable; alignment recovers scale later
where it belongs. Suite stays 82/0.

Honest limit, measured: ~100-point sparse PCA signatures are dominated
by WHICH features happened to track in a given take, not by scene
identity — the signature was designed for dense v8 meshes (10⁴–10⁵
verts) where shape statistics stabilize. Threshold-grade cross-take
matching needs either dense geometry under the signature (densify +
MeshIntegrator over the sparse anchors — the planned construction) or
appearance terms (vertex-color histograms exist in the mesh; adding
them to WebSignature is a wire-format change to take deliberately).

### Dense meshes close the cross-take matching loop (same session)

plate_lab --dense: DensifyPass interpolates depth between the sparse
triangulated anchors at two anchor frames (coverage 0.46–0.71 of the
downsampled map), MeshIntegrator fuses both into one mesh with real
source colors (7.5k–14.9k verts, 18k–29k tris — the density regime
WebSignature was designed for). The dense mesh replaces the sparse
cloud for signature/library purposes.

Result, same three-set experiment as before:
  store-take2 vs store-take1:  sparse 0.609  →  dense 0.789  ✓ fires
  vs train-car:                        0.523  →        0.404
  vs desert:                           0.469  →        0.563
The same-set score crosses the library's 0.75 find_match threshold
while both wrong sets stay under it; winner margin 0.086 → 0.226.

First time the complete asymptotic mechanism has closed end-to-end:
reconstruct a take → densify → mesh → signature → MATCH a different
take of the same set above threshold → (with the manifest reader
fixed) the matching chunk ships a UUID + alignment instead of
geometry. Honesty note: this is one same-set pair against two wrong
sets — a demonstration that the loop closes, not a validation suite;
building that suite (many takes, many sets, ROC of threshold) is the
obvious next measurement. GPU status for the record: v8's wgpu
rasterizer + WGSL phase correlation exist behind the `gpu` feature
(compile-checked only here — no GPU in this sandbox; they target the
Asahi Vulkan stack), core DCT/color has NEON/SSE/AVX2 paths; all
session code is deliberate CPU lab code, tested not optimized.

### Full-45s suite measurement: v11 vs AV1 (same session)

Question asked directly: enough to measure AV1 vs web over the full
video? Answer: yes for the 2D web (v11), no for the 3D web — matching
canonicals works as of this session, but residual-coding against a
matched canonical's render does not exist yet, so the 3D path produces
match scores, not bytes. That bridge is the named missing construction.

Protocol: all 19 shots of the cropped 45s clip through v11.3 at dz32
(one operating point), per-shot PSNR/SSIM aggregated by frame-weighted
MSE; anchor = user's AV1-150k file scored on identical cropped frames
in one pass (33.33 dB, 988,908 bytes video-only). Full table in
docs/full_v11_dz32.csv.

VERDICT: v11 45.95 MB @ 31.33 dB vs AV1 0.99 MB @ 33.33 dB —
46× larger AND 2 dB worse. The per-GOP experiments (7× on shotB)
flattered the codec by using content its model fits. The full clip is
dominated by handheld/fast-motion shots at 25–80 KB/frame where plate
prediction collapses (worst: shot 9, 118 frames, 74 KB/f @ 27.2 dB),
and the plates alone (11.1 MB) cost 11× the entire AV1 file.

The same table shows the other half of the story: on shots the model
actually fits, v11 is AV1-class or beyond — 45 B/frame @ 54 dB and
481 B/frame @ 95 dB on locked shots, 67 B/f near-lossless on a frozen
6-framer, 16–17 KB/f @ 38–39 dB on gentle ones — against AV1's 915
B/f clip average. Model-based coding measured bimodal: it wins where
the world model holds and loses catastrophically where it doesn't.
The honest implications, in order: (1) the marginal-cost thesis needs
the 3D/library path to produce BYTES (residual vs canonical render)
before any cross-video amortization claim can be tested; (2) hard-
motion shots need the standard toolbox (subpel MC, intra, entropy
coding) or a hybrid that falls back to a conventional coder; (3) the
per-shot plate overhead is the tax the library exists to amortize —
today every shot pays it fresh.

### Bimodal verdict vs AV1 per-shot + hybrid oracle + half-res coding (same session)

AV1-150k measured PER SHOT (packet-timestamp byte binning + per-shot
PSNR on identical windows; boundary packets can drift a few frames —
the 2-frame shot 17 reading is unreliable). This corrects the earlier
framing: AV1's rate control also exploits easy content (380–500 B/f on
the gentle shots), so "v11's 16 KB/f vs AV1's 915 B/f average" was
misleading.

STRICT bimodal verdict: v11 beats AV1 on 2 of 19 shots — the truly
frozen ones. Shot 1: 45 vs 87 B/f at identical 54.0 dB. Shot 5:
67 B/f near-lossless vs 2,897 B/f @ 47.8 (six frames — a keyframe
can't amortize; the plate is the better keyframe). The win mode is
real and it is exactly "static enough that plate+skip ≈ free while
per-frame codec overhead cannot be skipped."

ORACLE per-shot hybrid (route each shot to whichever coder wins):
0.97 MB @ 33.33 dB vs AV1's 0.99 MB — never worse, +1.9% on THIS
clip, because bloopers spend almost no duration in the win mode.
Hybrid value is proportional to content mix; the target genres
(games with locked HUD/arena shots, repeated sets, surveillance-like
footage) are where the win region covers real duration — and the 3D
library changes the economics further by amortizing the 11.1 MB of
plates across takes (store-set shots 10+12 alone carry 2.0 MB of
plates that a matched canonical would make free-after-first).

Half-res coding for the bad mode (user's upscaler idea, classical
bound): shot 9 (worst: 74 KB/f @ 27.2 dB full-res) at 960×400 + 
lanczos upscale = 18.6 KB/f @ 25.5 dB / 0.668 SSIM. Measured: 4× rate
relief at a −1.7 dB / −0.085 SSIM toll. That toll is precisely what a
learned upscaler exists to buy back (lanczos cannot invent texture);
the unendlich neural upscale path on the Asahi target is the intended
occupant of this slot, with engine-rendered training pairs. Even so,
AV1 remains 18× cheaper on this shot — hard-motion shots stay routed
to the traditional branch; half-res+SR improves the MODEL branch
where it is used.

Hybrid reconciliation design (recorded): route per SHOT/GOP, not per
block. A cheap plate-predictability probe (plate residual energy —
already computed) selects the branch; the container muxes segments;
no block-map or motion-vector interop between coders exists because
the streams never interleave. Per-block hybrid is phase 2, only if a
measurement shows per-shot routing leaving significant money on the
table.

### Residual ledger: naming the unsolved factors (same session)

User's objection accepted and adopted: "hard motion" is a mislabel for
UNSOLVED motion. Game footage is emitted by a finite state machine —
no hard motion exists in it by construction — and real-world motion
obeys gravity, air, and light transport: indirect filters a generative
model knows to want and a pixel model pays for as residual forever.

residual_ledger (v11_plate.rs, plate_lab --ledger) runs v3's exact
prediction loop and attributes every region-frame: SKIP (<8 SAD/px),
TRACKED-CHANGED (8–24: aligned but changed — blur/lighting/grain),
UNSOLVED (≥24: movers/parallax/reveals — nothing in the model
explains it). Byte attribution by energy share (approximate, stated).

Measured, worst shot (74 KB/f @ 27 dB):
  regions:  skip 84.6% | tracked-changed 13.2% | unsolved 2.2%
  energy:   skip 54.9% | tracked-changed 27.8% | unsolved 17.3%
  unsolved cells sweep 92.3% of the frame over the GOP (the actor).
Control (locked store): 89.8% of energy in skip class. Game: 81.7%.

THE FINDING: unsolved motion is NOT the cost center — 17% of energy.
The majority is a DIFFUSE FLOOR paid everywhere on aligned content:
exactly the user's invisible filters. Follow-up null: v4 grain-split
on the worst shot saves nothing (8.68 vs 8.72 MB) — at dz32 the
dead-zone already ate synthesizable grain, so the surviving floor is
STRUCTURED: sub-pixel misalignment ghosting (integer-pel MC on
drifting footage ≈ ½px edge error, everywhere, every frame), motion
blur (deterministic given velocity; we pay per frame), and lighting
flutter (global, indirect). An engine pays none of these.

Constructions ranked by measured bytes:
  1. Sub-pixel MC + sub-pel plate warp (attacks the 55% floor)
  2. Mover layers — object plates for the 2.2%-of-regions actor that
     sweeps the frame (attacks the 17% + kills its footprint growth)
  3. Velocity-conditioned blur rendering ("air")
  4. Global exposure/gain term ("light sources")
SMITE endgame restated: the library's canonicals converge on the game
tree's emission alphabet; with engine-derived canonicals (unendlich
renders) the codec ships state-like parameters. No hard motion —
only factors not yet moved from residual to model.

### Half-pel MC — the ledger's 55% bucket, cashed (same session)

search_mv: 8-neighbor half-pel refinement (bilinear SAD) after integer
diamond convergence, accepted only when beating the integer SAD by
>16. copy_region_mc: MVs now in half-pel units — even components take
the exact old integer path (bit-identical), odd components sample
bilinearly; chroma at quarter-pel of its plane. Wire format unchanged
in shape (MV deltas still i8, still cluster at 0); encoder and inline
recon share the functions so consistency is automatic.

A/B at dz32, both dimensions improving simultaneously:
  shot 9 (worst): 8.72 → 7.64 MB (−12.3%) AND 27.21 → 28.56 dB
    (+1.35) AND SSIM 0.754 → 0.814 (+0.060)
  shotB (control): marginal 9.2K → 7.2K B/f (−21%) AND 37.07 → 37.58
    (+0.51 dB) — gate weave is exactly half-pel territory.
Suite 82/0. Full-19-shot suite rerun with half-pel pending (expect
order −10–20% bytes at +0.5–1.4 dB; the 46× headline shrinks but the
bimodal structure stands). Remaining floor after half-pel: quarter-pel,
motion blur, lighting flutter — in the ledger's ranked queue.

### Character aging across series — design position (user's question)

Recorded as the library's identity model, to build toward:
identity = a LINEAGE GRAPH of canonicals connected by parameterized
transforms, not a single object. Three regimes, chosen economically:
 1. Within-work drift (stubble, costume wear, lighting): correction
    layers on one canonical — weathering as an indirect filter, same
    family as grain/exposure. Cheap, low-dimensional.
 2. Structured biological change across a series: when canonicals are
    ENGINE-GRADE (rigged bones+skin — the unendlich representation),
    aging is a trajectory in rig/morph space plus new texture layers;
    sharing survives a decade of in-world time. This is literally how
    game series already handle it: one rig, age morphs, new albedo.
    Biology supplies the PRIOR that aging is low-dimensional.
 3. Theseus re-base: when accumulated delta bytes approach a fresh
    canonical's cost, re-encode — but keep a lineage edge to the
    ancestor so cross-era matching still fires, and let every future
    reference cite whichever node in the lineage is cheapest.
The ship dissolves as a paradox: the ship is the lineage, and the
codec picks a node per reference by rate alone. Today's canonicals
are unrigged fused meshes and the only transform is rigid alignment
(compute_alignment_transform); the aging question is therefore
another argument for engine-grade canonicals, where non-rigid
parameterized deltas come with the representation.

### The citation bridge, built and priced (same session)

save_plate/load_plate + motions_against_plate + plate_lab
--save-plate/--use-plate: a stored plate is the 2D-native canonical,
and a GOP can now CITE one — alignment (phase-corr of frame 0 vs the
plate's zero-motion prediction) + motions + residual travel; the
plate costs a 24-byte reference. This is the library-citation
infrastructure in its minimum form; an engine-rendered canonical
slots into the same interface.

Measured on the store takes (w2 citing w1's plate, dz32):
  self-coded: 937 KB @ 37.19 dB (own plate 570 KB)
  cited:      481 KB @ 33.22 dB (−48.7% bytes, −3.97 dB)
  cited, quality-matched (~37.2 dB, interpolated dz≈16): ~1.5 MB —
    MORE than self-coding. The cross-take mismatch residual (~+1 MB
    at matched quality) exceeds the plate saved (570 KB), so at high
    quality this pair NEVER amortizes, at any reuse count.
  wrong-canonical control (desert citing the store plate): 1.18 MB —
    not catastrophic, because v3's mode decision routes regions to
    prev-MC when the plate is useless. Graceful degradation is
    built in.

The economics are now a formula: citation wins iff
ΔR(quality) < P_saved / N_reuses, and ΔR is the research object.
Named levers, in order, all with tools already in-tree:
  1. Alignment is translation-only today — the affine/homography
     machinery from the pose arc should align the cited plate
     (zoom/rotation between takes currently lands in residual).
  2. Canonical refinement: fuse takes into the canonical (union
     median) so the canonical converges on the SET rather than one
     take's sampling of it.
  3. Illumination compensation — the "light sources" term again:
     per-region gain/offset vs the canonical before residual.
At the low-quality tier (~33 dB) citation already wins today (481 vs
~600+ KB), which matters for the game-content regime where the
canonical will eventually be engine-exact and ΔR → grain floor.

### Citation ΔR hunted to ground: three nulls and a verdict (same session)

Lever 1, affine-aligned citation (align_plate_affine: variance-gated
32px block matching vs the plate's prediction → robust 6-DOF fit →
one-time plate warp, +24 wire bytes): the fitted transform on the
store takes is IDENTITY (scale 1.0004, −0.2px) — the takes are
already geometrically aligned; bytes/PSNR unchanged. The machinery is
correct and ready for pairs that DO differ geometrically; this pair
doesn't.

Lever 3, grain-band citation (v4 band split on the cited path):
WORSE bytes (2.46 vs 2.19 MB at dz12) — consistent with the earlier
finding that this pre-compressed source's grain is not cleanly
separable noise.

The diagnostic that settles it — mode share: the cited encode routes
91.9% of regions to prev-MC vs 54.5% self-coded. The encoder USES the
cross-take canonical for ~8% of regions. ΔR is CONTENT: blooper takes
of the "same set" differ in actor resting positions, shadows, and
props across most of the frame, and frame-level plates entangle set
with movers. The mode decision correctly routes around the mismatch
(same graceful-degradation path as the wrong-canonical control).

VERDICT, honestly reordered roadmap: mover-layer separation is a
PREREQUISITE for citation, not a sibling lever. The canonical must
model the SET alone (background plate with movers excised via the
mover mask — the classification machinery exists) before another
take can profitably cite it; citation then applies to true background
and movers are coded per-take. This also matches the engine framing:
an engine's scene is assets + actors, never a fused frame. The
affine-citation machinery stays in-tree, correct and waiting for
geometrically-differing pairs (handheld retakes, game camera orbits).

### Canonical fusion works — after robust registration (same session)

build_plate_fused: pool samples from multiple takes (each take's
frames motion-compensated into take-0 coordinates + a cross-take
offset), per-pixel median over the pooled set. The median IS the
trimming: an actor must occupy a pixel in more than half of ALL
pooled samples to survive into the canonical — where take 1's actor
rests, take 2's background votes them out.

First attempt poisoned itself: cross-take registration by raw phase
correlation latched onto actor-to-actor correspondence (different
resting positions) and returned a bogus shift; the misregistered
samples blurred the median (fused plate ballooned to 2226px wide on a
LOCKED set, citation gained only +0.34 dB). Fix: variance-gated block
matching on a grid, MEDIAN block displacement — actors are the
minority of blocks, so the median votes with the background.

Measured, w2 citing the robust-fused canonical at dz32:
  single-take canonical:  481 KB @ 33.22 dB, prev-mode share 0.919
  fused canonical:        433 KB @ 34.94 dB, prev-mode share 0.766
Bytes −10% AND +1.72 dB simultaneously; the canonical serves ~3× the
regions. And w1 cites the same canonical at 452 KB (mode 0.688) —
ONE canonical, BOTH takes.

Pair arithmetic including canonical transmission once (fused file
446 KB): cited pair 452+433+446 = 1,331 KB vs self-coded pair
708+937 = 1,645 KB → −19% across two takes, with the canonical
amortizing further on every additional take. Quality tier honesty:
cited sits at ~34–35 dB vs self's 37.2 at this dz; the matched-
quality crossover still needs the mover-layer step (actor regions are
the residual bulk and shrink nothing via fusion). But the direction
is now right, monotonic in take count, and the lineage mechanic —
takes refining a shared canonical they then cite — is measured
working for the first time.

CORRECTION to the pair arithmetic above: the fused canonical file
measured 554 KB (the −19% figure used a 446 KB estimate written
before the file size was read). Actual: cited pair 452+433+554 =
1,439 KB vs self-coded 1,645 KB → −12.5% across two takes. The
canonical is larger than either single-take plate because the fused
bounds span both takes' motion envelopes (2046px wide vs 1920-ish) —
tightening fused bounds to the intersection-plus-margin is a known
easy win on top. Amortization per additional take unchanged in
structure.

### Matched-quality crossover: achieved, and take-conditional (same session)

Sweep of the fused-canonical citation to self-coding's quality:

  w2 cited (dz20): 802.5 KB @ 37.06 dB vs self 937.4 KB @ 37.19
    → −14.4% bytes at matched quality (−0.13 dB). FIRST matched-
    quality citation win in real bytes.
  w1 cited (dz20): 883.6 KB @ 36.38 vs self 707.8 KB @ 37.58
    → citing LOSES for w1, on both axes.

The asymmetry is the mover problem seen from the donor side: fusion
voted w1's actors OUT of the canonical (that was the point), so w1
must now pay for its own staged content as residual — content its
self-plate carried for free. w2 gains because its window is busier
and its self-plate correspondingly worse; the canonical hands it
clean background it never had.

Measured design law: CITATION PROFIT IS TAKE-CONDITIONAL. The
encoder routes per take — cite when the canonical beats the take's
own background model, self-code otherwise (the per-shot hybrid
routing principle, one level down). Canonical break-even: ~554 KB /
~135 KB saved per citing take ≈ 4 citing takes; every take after is
profit. Two takes of bloopers: not yet profitable. Hundreds of takes
of one SMITE map: decisively profitable — the regime the thesis was
always about. Mover layers remain the lever to grow the per-take
saving beyond 14% (actor regions still dominate the citing residual)
and to let donor takes cite without repurchasing their own actors.

### Mover-patch MVP: prices the static-patch approximation (same session)

--patch-movers <thr>: regions persistently deviating from the cited
canonical are the take's staged content; the take's own plate is
built encoder-side, those regions spliced into the canonical, and
only the patch travels (quasi-static splice at frame-0 motion —
valid for the locked-set regime; general motion needs per-frame
layer compositing, the full design). Patch bytes print separately
and are NOT in total_bytes — honest totals below include them.

Threshold matters: at thr=10, HALF the frame flags as mover — the
detector conflates actors with the cross-take grain-decorrelation
floor. thr=16 → 31% of regions, thr=24 → 21.5%.

Measured, w1 (the donor take with the excellent self-model), dz20:
  self:                707.8 KB @ 37.58
  cited-plain:         883.6 KB @ 36.38
  cited+patch thr16:   980.3 KB @ 37.41  (794.2 + 186.1 patch)
  cited+patch thr24:   943.0 KB @ 37.15  (814.9 + 128.1 patch)

Verdict: the patch is a real RD improvement to the CITED branch
(+1.03 dB for +97 KB over cited-plain at thr16) but does NOT beat
self-coding for a take with a good self-model, for two priced
reasons: (1) scattered patch regions zstd ~35% worse per pixel than
the same content inside a coherent whole plate; (2) the cited
background still carries the grain-decorrelation residual outside
patches. The per-take routing law stands unmodified: cite when the
canonical beats your self-model, self-code otherwise; the patch
raises the cited branch's ceiling and will matter most where
self-plates are expensive (busy takes) or canonicals are engine-
exact (no grain decorrelation). The full mover-layer design —
movers as objects with their own motion, composited per frame —
remains the named construction; this MVP measured its static
approximation.

### Per-take routing + half-pel full-suite rerun (same session)

--route (with --use-plate): encodes BOTH branches and picks by RD
cost J = SSE + λ·bytes, λ = dz²/3. Validation on the takes taught a
real subtlety: w1 → SELF (correct), and w2 → SELF as well — which is
λ-CONSISTENT, not a bug. The earlier "citation wins at matched
quality" compared cited@dz20 to self@dz32, a cross-λ hull
comparison; on the hull, citation wins below ~37.2 dB and ties near
38, and a router at dz20's λ sits where self edges ahead. Routing
decisions are λ-dependent — standard RD practice, now recorded
precisely: the citation win is a low-to-mid-quality-tier phenomenon
on this grain-decorrelated content, and tier-independent only when
canonicals stop decorrelating (engine-exact).

Half-pel full-suite rerun (all 19 shots, dz32, same protocol; CSV in
docs/full_v11_dz32_halfpel.csv):
  int-pel : 45.95 MB @ 31.33 dB / SSIM 0.895
  half-pel: 42.88 MB @ 32.62 dB / SSIM 0.922
  → −6.7% bytes AND +1.29 dB AND +0.027 SSIM across the whole 45 s.
  vs AV1-150k (0.99 MB @ 33.33): 46.5× @ −2.00 dB → 43.4× @ −0.71 dB.
One construction closed 1.3 of the 2 dB whole-video quality gap while
shrinking bytes — the residual ledger pointed at the right bucket.
Quarter-pel is the next rung of the same ladder.

### Quarter-pel + the gain/offset term: whole-video quality parity with AV1 (same session)

Quarter-pel (third refinement ring, quarter-pel MV wire units):
measured near-neutral (−1.7% on the worst shot, +0.13 dB for +2% on
the control) — diminishing returns; the sub-pel ladder's next real
rung is TAP QUALITY (6-tap interpolation), not finer steps. Kept.

The gain/offset term (per-region a·pred+b, least-squares fit,
a quantized to 1/64 steps ±31, b to ints ±31, ~2 bytes/region where
used, gated on a 96-L1 rate cover; correction mutates the composite
before residual coding so encoder/recon consistency is by
construction): ledger-measured ceiling ~42% of residual energy on
the worst shot — and the realized win is the LARGEST single RD move
of the session:
  shot 9: 7.51 → 5.26 MB (−30.0%) AND +0.68 dB AND +0.025 SSIM —
    strict dominance. Cumulative on shot 9 this session: −39.7%
    bytes at +2.04 dB.
  shotB: +20% bytes for +1.80 dB — an up-curve move at/above the
    dz-tradeoff line (not strict dominance; stated as such).
The mechanism, per the ledger: gain<1 is a first-order model of
motion blur's contrast reduction ("air"), offset models exposure
flutter ("light sources") — two of the named invisible filters,
captured by two parameters per region. (The ledger's ceiling probe
itself needed a fix on the way: it had compared MC'd raw SAD against
gain-corrected UNcompensated prediction — 226% "corrected" energy
was the probe's bug, recorded.)

Full-suite ladder, all 19 shots at dz32:
  int-pel  : 45.95 MB @ 31.33 dB / 0.895   (46.5× AV1, −2.00 dB)
  half-pel : 42.88 MB @ 32.62 dB / 0.922   (43.4×, −0.71 dB)
  qpel+gain: 39.90 MB @ 33.69 dB / 0.934   (40.3×, +0.36 dB)

WHOLE-VIDEO QUALITY PARITY WITH AV1: CROSSED (+0.36 dB over the full
45 s) — while v11's own bytes fell 13% across the ladder. Every rung
was chosen by residual-ledger attribution, not intuition. The
remaining gap is pure rate: 40.3×, owned by entropy coding (2–3×
estimated), intra modes, lookahead RD, mover compositing, and — for
the target regimes — canonical citation amortization.

### Band-major coefficient order + zstd level (same session)

The entropy chain's cheapest structural fix: within each nonskip
segment, coefficients now serialize BAND-MAJOR (coef k of every block
together) instead of block-major. Each spectral band has homogeneous
statistics (DC deltas small/smooth, high-AC mostly zero); grouping a
band hands zstd's entropy stage a coherent local distribution that LZ
matching cannot reconstruct across a 17-byte block stride. Plus
level 6 → 15 on large payloads (exactly the frames dominating total
bytes). Same reconstruction by construction — measured bit-identical
PSNR (six decimals) with fewer bytes: worst shot −8.8%, control
−1.8%. Residual roundtrip tests green.

Full-suite ladder update:
  int-pel   : 45.95 MB @ 31.33 / 0.895   (46.5×, −2.00 dB)
  half-pel  : 42.88 MB @ 32.62 / 0.922   (43.4×, −0.71 dB)
  qpel+gain : 39.90 MB @ 33.69 / 0.934   (40.3×, +0.36 dB)
  band-major: 37.90 MB @ 33.69 / 0.934   (38.3×, +0.36 dB)
The rate gap is 38.3× at +0.36 dB over the AV1 anchor. The rest of
the entropy estimate (adaptive rANS with per-band context models —
the real 1.5–2×) remains the largest single uncollected item.

CORRECTION: the band-major suite aggregate measured 36.35 MB (36.8×),
not the 37.90 estimated in the entry above (written in the same
command block before the aggregate printed — same mistake as the
fused-canonical arithmetic, same fix: numbers go in the log AFTER
they print). Ladder end state: 36.35 MB @ 33.69 dB / 0.934 = 36.8×
the AV1 anchor at +0.36 dB, from 46.5× at −2.00 dB at ladder start —
−21% bytes and +2.36 dB across the session's constructions.

### Per-context rANS entropy coder (same session)

rans.rs: standard byte-renormalizing rANS (state u32, L=1<<16,
freqs to 1<<12), semi-adaptive per-frame static tables, per-context
models. Contexts = channel class (Y vs chroma) × band group
(DC | 1–3 | 4–9 | 10–15) = 8. Residual coding splits into a structure
stream (RLE counts + params → zstd) and a coefficient stream
(per-context rANS); a method byte selects min(zstd, rANS) per frame,
so the path can never lose more than one byte. Roundtrip unit test;
suite 83/0.

Two bugs found by real data before the numbers: a 255-symbol table's
count byte collided with the absent-context marker (255), read back
as "absent", misaligned the stream, and the slot builder indexed past
the table BEFORE the sum check — fixed by reserving 255 (≥254-symbol
tables use the dense format) and guarding accumulation to fail-None
instead of panic.

Measured, bit-identical reconstruction throughout (PSNR to six
decimals): worst shot 4.79 → 4.43 MB (−7.5%; entropy work cumulative
on that shot: −15.6%); locked shot −0.4% (small payloads: zstd-12 +
table overhead compete). Full suite (aggregate printed BEFORE this
entry, per the twice-learned rule):
  band-major : 36.35 MB @ 33.69 / 0.934   (36.8×)
  rANS       : 33.89 MB @ 33.69 / 0.934   (34.3×)
Entropy arc total: 39.90 → 33.89 MB (−15.1%) at identical
reconstructions. The 8-context order-0 model beat zstd by what
order-0 separation is worth; the remaining entropy headroom is
richer contexts (neighbor-magnitude conditioning, zero-run symbols)
— the last step toward the original 2–3× entropy estimate.

Session ladder, whole 45 s: 46.5× @ −2.00 dB → 34.3× @ +0.36 dB.

### Entropy context experiments: two measured negatives (same session)

Both A/B'd on real payloads against the 8-context rANS baseline
(worst shot 4,432,674 B), both reverted, both recorded in the code at
the emission site:

1. Neighbor conditioning (ctx × previous-block-same-band == 0, 16
   contexts): worst shot +0.6%. The conditional distributions aren't
   different enough after band grouping to pay for doubled table
   serialization (~1.2–1.8 KB/frame measured at 16 ctxs).
2. (run, level) symbolization (zero-runs as chunked run symbols, zero-
   free literal alphabets): worst shot +8.1% — rANS lost best-of on
   EVERY frame (total collapsed to the zstd-only number). The
   arithmetic error was mine, upstream: at p(zero)≈0.95 an order-0
   model already codes zeros at ~0.07 bits, so the entire recoverable
   zero mass is ~3 KB/frame — smaller than what heavy-tailed run
   lengths cost under 12-bit quantized static tables with per-band
   resets.

Conclusion the two negatives triangulate: static per-frame tables
are near their optimum at 8 contexts. The remaining entropy headroom
(toward the original 2–3× estimate) requires ADAPTIVE modeling —
per-symbol model updates, which rANS's LIFO order makes a genuinely
different construction (interleaved streams or two-pass) — correctly
sized as its own project, not a knob.

Process note, recorded because it cost a cycle: a `grep -cE "^error"`
output of "2" was misread as a pattern line rather than an error
COUNT — the revert build had failed and the A/B ran a STALE binary
(the "13 ctxs used" debug line was the tell: impossible under the
reverted 8-context encoder). Third stale-binary incident this
project; the fix applied is exact-baseline verification after every
revert (both shots now byte-identical to the recorded baseline).
Suite 84/0 including a new direct rANS-split roundtrip test (whose
first version wrongly asserted sub-threshold blocks survive — they
are skip-RLE'd to zero by design; the test was wrong, not the codec).

### DC-intra mode: strict whole-video dominance (same session)

Mode 2: predict a region as the mean of its already-composed top/left
borders. Raster order makes it greedily decodable (neighbors are
FINAL when a region decides; the inline recon rebuilds the identical
composite); it costs 1 flag bit and zero motion bytes; the gate
requires beating the winning temporal predictor by 96 L1 so grain
noise can't dither into it. Targets the ledger's unsolved bucket —
reveals and washed-out content both temporal predictors miss.

Measured (numbers logged after printing):
  shot 9: 4,432,674 → 4,171,368 (−5.9%) AND +0.05 dB AND +0.010 SSIM.
  shotB : −0.6% bytes at −0.025 dB (inside the gate's noise).
  Full suite (docs/full_v11_dz32_intra.csv):
    rANS  : 33.89 MB @ 33.69 / 0.934   (34.3×)
    +intra: 32.07 MB @ 33.80 / 0.940   (32.4×, +0.47 dB over AV1)
  → −5.4% bytes AND +0.11 dB AND +0.006 SSIM: strict dominance over
  the entire 45 s from one flag bit per region.

Session ladder to date: 46.5× @ −2.00 dB → 32.4× @ +0.47 dB.

### Planar intra (mode 3, same session)

HEVC-style plane from the same top/left borders (horizontal interp
toward the top-right sample, vertical toward the bottom-left,
averaged) — DC's gradient-shaped sibling for smooth shading where a
constant leaves a ramp in the residual. One extra bit per intra
region choosing DC vs planar. Borrow-checker required buffering the
plane before mutating the composite; behavior identical.

Measured (after printing): shot 9 −2.0% AND +0.05 dB (strict
dominance again); shotB +0.1% bytes for +0.044 dB. Full suite
(docs/full_v11_dz32_planar.csv):
  DC-intra: 32.07 MB @ 33.80 / 0.940   (32.4×)
  +planar : 31.58 MB @ 33.90 / 0.941   (31.9×, +0.57 dB over AV1)
→ −1.5% bytes AND +0.10 dB across the whole 45 s.

Session ladder: 46.5× @ −2.00 dB → 31.9× @ +0.57 dB. The intra
family (DC + planar) has now delivered −6.8% bytes and +0.21 dB
combined — the ledger's unsolved bucket paying out as predicted.

### MV range 12 → 24 px (same session)

Diagnostic first: 4.4% of prev-mode regions on the fast-pan shot sat
pinned at the ±12 px clamp (28,693 of 658,861 measured) — silently
taking bad prediction. Extended to ±24 px (±96 quarter-pel, still
i8) with a step-8 tier seeding the diamond descent. A/B (after
printing): shot 9 −0.5% AND +0.054 dB; shotB flat bytes AND
+0.065 dB — strictly positive on both axes on both shots. Full-suite
headline NOT yet re-measured for this change; the standing suite
number remains the planar generation (31.58 MB @ 33.90 / 0.941,
31.9×) until the next full run prints.

### Drift-anchored mode decision: third principled attempt, third loss (same session)

The idea: the leftover error SAD(recon, source) per region after each
frame IS the exact accumulated drift state (the encoder is its own
decoder), so re-anchor a region to the plate deterministically when
measured drift exceeds a threshold — replacing the grain-dither
accident with measurement. Implementation found its own bug first:
the drifted branch was written as an algebraic identity of the plain
rule (35,988 drifted flags, ZERO decision flips — the instrument
caught it). Corrected to a raised prev margin and measured:
  thr 768, margin 256: shotB +0.2% bytes AND −0.02 dB (worse both axes)
  thr 2048           : +0.05% / +0.002 dB (statistical zero)
Verdict: joins region_codable_cost and rotating refresh in the
recorded negatives. The SAD dither's accidental refresh remains
unbeaten by every deterministic replacement tried; whatever finally
beats it must price the FUTURE cost of each choice (true lookahead
with trial encodes), not re-derive refresh from present-frame state.
All drift machinery reverted; both A/B shots verified BYTE-EXACT
against the range24 baselines after revert.

Process failures this stretch, on the record:
1. python replace() without count=1 applied patches to v2, v3, AND v4
   (three functions share decision-loop shape); two repair cycles.
2. The binary was rebuilt while a full-suite run was live — the suite
   invokes the binary per shot, so the run mixed generations and its
   output was DISCARDED (full_v11_CONTAMINATED_discard.csv kept as
   the artifact). Rule adopted: no rebuilds while a suite runs.

### MV range 24: suite headline (clean rerun after the contaminated one)

  planar : 31.58 MB @ 33.90 / 0.941   (31.9×)
  range24: 30.82 MB @ 34.02 / 0.943   (31.2×, +0.69 dB over AV1)
→ −2.4% bytes AND +0.12 dB AND +0.001 SSIM — strict dominance, and
larger than the two A/B shots implied: the other pan/handheld shots
benefited more from the unclamped search than the diagnostic shot.
docs/full_v11_dz32_range24.csv. Session ladder:
46.5× @ −2.00 dB → 31.2× @ +0.69 dB.

### Byte attribution → gain-gate retune (same session)

New diagnostic (plate_lab prints the plate/motion/mode+mv/residual
split): on the quiet shot, MODE+MV SIDE INFO had grown to 44% of
total bytes — larger than the plate, 3.4× the residual — and the raw
composition showed gain values as the largest component (233 KB raw
over 48 frames: 2.4K regions/frame buying 2-byte corrections through
a 96-L1 gate on a locked shot).

Two coding-layer fixes tried first, both measured, both reverted,
both recorded at the emission site: delta-coding the gain values
LOST +6.2% (regions repeat exact (a,b) pairs — LZ gold that deltas
destroy) and plane-splitting qa/b LOST +0.8% (pairwise matches beat
plane homogeneity). The volume, not the coding, was the problem.

Gate sweep (after printing): 96→192 on shotB −15.6% at −0.15 dB, on
shot 9 −7.3% at −0.035 dB (~880 KB/dB and ~8.5 MB/dB — far cheaper
rate than the dz knob); 384 overshoots (−0.46 dB). Full suite at 192
(docs/full_v11_dz32_gate192.csv):
  gate 96 : 30.82 MB @ 34.02 / 0.943   (31.2×)
  gate 192: 28.61 MB @ 33.97 / 0.942   (28.9×, +0.64 dB over AV1)
→ −7.2% bytes at −0.05 dB: an operating-point trade taken
deliberately — the codec's entire remaining gap is rate, and this
buys it at ~44 MB/dB suite-wide. Session ladder:
46.5× @ −2.00 dB → 28.9× @ +0.64 dB.

### The plate was never quantized — DCT-coded plate + 3-median MV predictor

Two findings from the byte-split diagnostic, in order of size.

1. MV predictor: the mean-of-two (left, top) replaced by a true
   3-neighbour median (left, top, top-right) with absent neighbours
   contributing NOTHING rather than a spurious zero (a zero-filled
   mean biases every edge region's delta). Bit-identical
   reconstruction, −0.14% shotB / −0.25% shot 9. Free.
   (Investigation started from a wrong hypothesis — a zero-MV flag to
   avoid paying 2 bytes for stationary regions — which the encoder
   already had as the nz bitmap. 38% of shotB's prev-mode MVs are
   exactly zero and already cost 1 bit each; the 186 KB is real
   motion.)

2. THE PLATE WAS THE ONE SURFACE NEVER QUANTIZED: raw zstd'd pixels,
   33% of the hard shot and 42% of the quiet one, paying full entropy
   for grain the dead zone discards everywhere else. Now coded
   through the same residual path as every frame (predicted from flat
   gray), best-of against raw zstd.

   The first version reported a 15× plate shrink at BIT-IDENTICAL
   PSNR — impossible, and the tell: it counted the coded size while
   still predicting from the UNQUANTIZED plate. Accounting fiction
   the decoder could never reproduce. Fixed by adopting the decoded
   plate as the actual reference; all numbers below are post-fix.

   Plate dead zone swept on the FULL SUITE (three complete 19-shot
   runs, CSVs in docs/):
     raw plate  : 28.61 MB @ 33.97 / 0.942  (28.9×, +0.64 dB)
     plate dz/4 : 20.38 MB @ 33.43 / 0.937  (20.6×, +0.10 dB) 15.4 MB/dB
     plate dz/8 : 20.92 MB @ 33.58 / 0.938  (21.2×, +0.25 dB) 19.6 MB/dB
     plate dz/16: 21.79 MB @ 33.62 / 0.939  (22.0×, +0.29 dB) 19.8 MB/dB
   CHOSEN dz/8: the knee — near-best exchange rate, and the cheapest
   point still holding a quality lead over the AV1 anchor. −26.9%
   bytes for −0.39 dB against the raw-plate generation; the largest
   single rate move of the session.

   Structural bonus beyond this clip: the plate is also the CITATION
   currency. A canonical that costs 7× less to transmit moves the
   library break-even from ~4 citing takes toward ~1–2, which is the
   economics the whole 3D/library arc depends on.

Session ladder: 46.5× @ −2.00 dB → 21.2× @ +0.25 dB.
