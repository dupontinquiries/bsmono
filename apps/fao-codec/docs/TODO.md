# FAO TODO

Tasks grouped by area and priority. ✅ = shipped. ⏳ = in flight. 
🟥 = critical / production-blocking. 🟨 = high priority. 🟩 = nice-to-have.

## Codec quality (v8 → v8+)

| | Item | Effort | Impact |
|--|------|--------|--------|
| ✅ | **Quaternion convention fix** | Decoder's `rotate_by_quat` was reading `[x,y,z,w]` while encoder/CameraPose use `[w,x,y,z]`. Every v8 frame was rendering all-behind-camera and falling back to source video silently. Three call sites fixed; three unit tests rewritten. |
| ✅ | **Small v8 frame recon bloat** | c_yuv-is-v8 check only matched Z prefix, missed raw `V81`/`V8p` (frames under ZSTD_THRESHOLD). Caused ~1MB recon per small v8 frame in tmp file. Now also matches raw V8 prefixes. |
| ✅ | **ffmpeg source-fetch keyframe inaccuracy** | Pre-input `-ss` only → cached source frames misaligned by up to several seconds on .webm with sparse keyframes. Now uses dual-stage seek (coarse pre-input + accurate post-input). |
| ✅ | **V8MX/V8LR trailer scan-based end detection** | False-magic in zstd blob bytes could truncate trailer parse. Now reads the u32 length prefix written by the export pipeline. |
| ✅ | **Decoder focal-length zero fallback** | CPU and GPU rasterizers had no fallback when `pose.focal[0] == 0`, causing one-pixel renders for any default-pose entries. Now falls back to 1000.0, matching encoder convention. |
| ✅ | **Z frame export stats** | Format breakdown logger missed Z entirely — your SC2 export said Z=0 when it had 83k Z frames. Now counts Z and prints it first. |
| ✅ | **GPU mesh rasterizer** | wgpu graphics pipeline + WGSL shader. Drops 2560×1440 per-frame raster from 30-50ms → 3-5ms. Lazy init, CPU fallback on failure, feature-gated behind `gpu`. |
| ✅ | **Real vertex colors** — sample source YUV during integrate (FIXED THE BLOCKY BLUE-GREEN MESH BUG) | 1 session | Huge — single biggest quality fix to date |
| ✅ | **v8.1 — per-frame photometric residual** | Shipped behind FAO_V81 env var. Adds quarter-res RGB delta after mesh render; +10-40KB/frame compressed. Decoder is fully back-compat (v1 frames render identically). Off by default — set FAO_V81=1 to enable. |
| 🟨 | **v8.2 — texture maps for canonical webs** | 2-3 sessions | Biggest single lever; -30-50% per chunk + per-pixel detail |
| 🟩 | **v8.3 — pose & feature delta encoding** | 1 session | 50% per-frame size reduction, lossless |
| 🟩 | **v8.4 — mesh simplification (Garland-Heckbert)** | 1 session | -30-50% mesh size, imperceptible loss |
| 🟩 | **v8.5 — spherical harmonics for lighting** | 2 sessions | Enables relighting, modest size impact |
| 🟩 | **v8.6 — adaptive mesh density** | 1-2 sessions | Better detail allocation |
| 🟩 | Better v8 disable signal — "consecutive empty meshes" | half session | Avoid v8 cost on slideshow content |
| 🟩 | Feature tracker silent-mode for near-static content | half session | 6KB → ~100B for static frames |

## Codec correctness (live bugs)

| | Item | Notes |
|--|------|-------|
| ✅ | V8MergeCache prevents double-export from emptying the trailer | shipped |
| ✅ | Repartition chunks at merge time (scatter problem in sharded encoder) | shipped |
| ✅ | start_transcode early-return when already running | shipped |
| ✅ | fps detection: prefer avg_frame_rate over r_frame_rate | shipped (fixes tbn-1000 leak on MKV) |
| ✅ | **v8 export size fix** — Z marker added to ORIG_MARKERS so v8 frames use compact orig payload, not the 500KB/frame zstd recon (was inflating .fao files ~250x for v8 encodes) |
| ✅ | **v8 tmp file size fix** — skip c_recon computation for Z frames (rasterizer reconstructs from trailer, recon column was dead data) |
| ✅ | **Memory-pressure threshold made relative** — was hard `< 16GB` which fired constantly on 16GB systems; now `< max(total/4, 4GB)` |
| ✅ | **Cancel button for in-flight encode** | tc.cancel_requested flag + worker/reader cooperative cancel + GUI button. Shipped. |
| 🟩 | Disk-write log uses tier labels | ✅ shipped |
| 🟩 | UI frame label codec-aware (T/K/V/Z/P/B) | ✅ shipped |
| 🟩 | VRR savings gate on plausible fps range | ✅ shipped |
| 🟩 | Bitrate extrapolation requires ≥50 samples | ✅ shipped |

## Decoder / playback

| | Item | Notes |
|--|------|-------|
| ✅ | **V8LR render integration** — library refs actually render | shipped this session; was the missing piece |
| ✅ | Incremental edge-walk rasterizer (~10x decode) | shipped |
| ✅ | Skip ffmpeg fallback when trailer has 0 webs | shipped (avoids per-frame ffmpeg thrash) |
| ✅ | **Source-fetch worker thread** | Async ffmpeg spawn; UI never blocks. Fixes ~once-per-minute hitches on slow-fps content with web gaps. Sync path retained as scrub fallback. |
| 🟩 | GPU rasterizer for large meshes | Currently CPU-only; 100k+ triangle meshes at 2K could benefit |

## Encoder performance

| | Item | Notes |
|--|------|-------|
| ✅ | LK structure-tensor hoisting (~2.7x encode) | shipped |
| ✅ | Sharded encoders (Mutex per worker) | shipped earlier |
| ✅ | AVX2 8-pixel color SIMD | shipped earlier |
| ✅ | GPU phase correlation pipeline (Stockham WGSL FFT) | shipped but not wired |
| 🟨 | **Wire GPU phase correlation into V8Encoder** | Pipeline built; just needs hookup |
| 🟩 | Pre-warmed wgpu::Device + ComputePipelines | Saves ~200ms encode startup; latency win not compression win |

## Library mode

| | Item | Notes |
|--|------|-------|
| ✅ | Phase 1 — disk spillover + signature standalone | shipped |
| ✅ | Phase 2 — CLI + persistent handle | shipped |
| ✅ | Phase 3 — encode integration + V8LR trailer + decoder attach | shipped |
| 🟨 | **Phase 4 — per-canonical priors + GPU pre-warm** | Depth hist + occlusion + lighting baseline |
| 🟨 | **Phase 5 — spatial index for find_match** | Linear scan becomes bottleneck above ~10k webs |
| 🟨 | Phase 6 — GC daemon, import/export, sync, ~~SQLite migration~~ → custom log DB | Custom append-only log DB shipped (binary, CRC, crash-safe, dual-write w/ JSON manifest, rollback). Still TODO: GC daemon, import/export. |
| 🟩 | Better signature design (learned embedding) | Reduce false matches on similar-looking-but-different scenes |

## Format / interop

| | Item | Notes |
|--|------|-------|
| ✅ | **AV1/H.264 escape hatch** — `fao-cli export` + GUI "→ AV1" button | Decode .fao through FAO pipeline, pipe RGB to ffmpeg, write .mkv. Both CLI and GUI flows shipped. T-frame anchor walk-back is still TODO (renders as previous-frame for now). |
| 🟩 | Audio Opus re-encode (per-chunk) | Currently audio is passthrough |
| 🟩 | Raytracing the mesh (optional render path) | High-quality lighting demo |

## UI / app

| | Item | Notes |
|--|------|-------|
| ✅ | Library tab GUI (Init / Attach / Detach / Writable) | shipped |
| 🟨 | **Cancel button for in-flight encode** | Listed in correctness; same item |
| 🟩 | Entity viewer — Phase 1: feature-cluster entities | No ML deps; uses existing feature trajectories |
| 🟩 | Entity viewer — Phase 2: face/body pose via ONNX | BlazeFace + MoveNet |
| 🟩 | Entity viewer — Phase 3: filter inspection per entity | Lighting/animation deltas |

## Infrastructure / debt

| | Item | Notes |
|--|------|-------|
| 🟩 | `integrate_with_fusion` is dead code | Free fn never called; still uses placeholder color |
| 🟩 | Stats Vec → final-output refactor | Defer until OOM at 3M+ frames |
| 🟩 | fao_codec_v2 crate is empty placeholder | Either fill or remove |
| 🟩 | v3/v4/v5 codecs largely retired | Consider removing source if confirmed unused |

## Documentation

| | Item | Notes |
|--|------|-------|
| ✅ | Initial docs/ folder | shipped this session |
| 🟩 | Add inline rustdoc to public APIs | Especially v8_reconstruction_codec.rs surface |
| 🟩 | Performance harness with reference numbers | Track regressions on test footage |

## Open investigations (to debug in priority order)

| Priority | Item | Why it matters |
|----------|------|----------------|
| 🟨 next | **Jitter diagnosis after summary trace** | After the playback summary is shipped, run a v8 .fao through the player and read the bucket breakdown. Three candidate root causes: (1) pose drift — mean pose-position delta should be 0 for static cameras; if it's not, the encoder is producing noisy poses. (2) Mesh churn — vertex count fluctuating frame-to-frame within a single web means triangulation is unstable. (3) Web swap pop — high web_transitions count means we're crossing web boundaries often, and adjacent webs' meshes don't line up. |
| 🟨 next | **Diff-overlay debug view** | Toggle in GUI that swaps the canvas to \|frame_N - frame_{N-1}\| with 4× brightness. Lights up jittering regions, stable geometry stays black. Best tool to localize the jitter spatially. 45min of work, reusable for future debugging. |
| 🟩 file | **Scene-space FSM sanity checker** | Discrete labels per region (static-plate / parallax / dynamic / occluded) with transition rules. Premature — we don't yet know which labels matter. Revisit once summary + diff overlay tell us what discrete states exist. |
| 🟩 file | **Sparse-mesh hole fill** | The "circles with parts of test footage" pattern is sparse mesh — triangles only where features cluster. Possible fix: vertex-color splatting where no triangles cover a pixel, OR background plane sampled from source at the latest anchor. Big visual win once geometry is verified stable. |
| 🟩 file | **GPU lighting shader (Lambertian → normal-mapped)** | Requires per-vertex normals (encoder-side change), normal interpolation in vertex→fragment varyings, and a lighting model in fs_main. Deferred until geometry is solid — adding shading on top of broken geometry produces beautifully shaded broken geometry. |
| 🟩 file | **Camera-rotation estimation in encoder** | v8_reconstruction_codec.rs:1060 hardcodes rotation=[1,0,0,0]. Real cameras pan/tilt; we should estimate rotation from feature tracks. Currently the system can't represent pan-tilt content. |
| 🟩 file | **V8MX trailer use length-prefix consistently for V8PT, V8MS** | The recent length-prefix-aware end detection was added for V8MX and V8LR. V8PT and V8MS still use scan-for-next-magic. Same risk in theory (false-magic in pose data). Lower probability since V8PT is fixed-size structs, but still worth aligning. |


## Newly filed (post-jitter-trace)

| Priority | Item | Notes |
|----------|------|-------|
| 🟨 | **Edge-anchor extraction for flat-greenscreen shots** | When feature tracking fails (uniform / low-texture content), use Sobel / Canny edge detection on the source frame to find anchor points along strong edges. Track those 2D anchors temporally. For shots with no camera parallax, treat them as fixed-depth plane vertices rather than triangulating them. Produces a sparse fallback mesh where the full v8 pipeline would emit zero triangles. Likely 2 sessions of work; integrate as a `DensifyPass` extension behind a `FAO_EDGE_ANCHORS` env var. |
| 🟨 | **Flat-shot detection → fallback to V2/V21** | Simpler than edge-anchors: when the encoder detects a low-texture or low-parallax shot (e.g., n_valid features < threshold for N consecutive frames), STOP attempting v8 entirely and let the V2 DCT path handle that range. v8 is great for textured 3D content; for flat scenes it produces ~0 verts and the player falls back to source anyway. Better to take a different codec path explicitly. |
| 🟩 | **Scene-classifier upstream of codec selector** | The current `codec_v` switch is global. A per-shot classifier (e.g., "this shot is locked-off, this one has parallax, this one is dynamic") could route shots to the codec best suited for each. Pairs naturally with the FSM idea filed earlier. |

## User-requested 2026-W19 — feature priorities

User reports: "some frames have residuals drawn over top and others 
don't but the ones that do sometimes have the feeling of being glued 
together due to slight jitter or color discrepancies"

Verified bugs from log analysis:

| | Item | Status |
|--|------|--------|
| ✅ | **CPU rasterizer returns Some(zero-RGB) when triangles bbox-but-miss-coverage** | Fixed this session: CPU `render_mesh` now stride-samples the output RGB and returns None if entirely zero, matching the GPU rasterizer's existing check. Without this, `apply_residual` was layering V8.1 residuals on top of an all-zero base — exactly the "residual glued onto black" symptom the user described. |
| ✅ | **OOM trailer-magic false match** | Fixed previous session. File now loads. |

User-requested features (priority order):

| | Item | Estimate | Notes |
|--|------|---------|-------|
| 🟨 | **Skip-frame = black-frame fallback strengthening** | 1 session | User says "fixed by making new version... we need to find that version of the codec and bump it to a plus version and gut the old one + archive it for research purposes". Translation: when v8 produces no output AND source-fetch is unavailable, currently we show prev frame OR vec![64; rgb_sz] (gray). Want: explicit "v8+" path that retries via different mechanism (e.g. interpolation from prev+next anchor, or temporal-blend). Then archive the original raw-zero path. |
| 🟨 | **Transparency / alpha-overlay greedy processing** | 2-3 sessions | Layer-detection already labels 2-7% of frames as `alpha_overlay`. Encoder doesn't do anything with the label. Want: separate alpha overlay regions (HUD, subtitles, transparent UI) into a dedicated stream with proper alpha-blend handling. Significant for game/UI content like DBD where 7% of pixels are overlay text. |
| 🟨 | **Better object recognition for encoder mesh-integration** | 2 sessions | Current mesh integration fuses voxels indiscriminately. Object-aware integration would let us track distinct objects across web boundaries instead of treating the scene as one undifferentiated mesh, reducing web-swap pop-in. |
| ✅ | **Better jitter — pose low-pass filter** | 1 session | Current FAO_LESS_JITTER snaps when delta < 0.01 per axis. Trace shows mean=30, std=76. Filter is essentially a no-op on real content. Want: configurable low-pass filter (1D Kalman or exponential MA) that smooths small-amplitude noise without freezing real motion. |
| 🟩 | **Color accuracy improvements** (before lighting) | 1-2 sessions | "focus on color accuracy into good compression ratios first then add lighting". Per-vertex color is currently sampled from source YUV at integration time. Could improve by (a) sampling at higher precision, (b) doing chroma-only or luma-only residuals to bring the per-vertex stored color closer to source. |
| 🟩 | **GPU lighting shader** (deferred) | 2-3 sessions | "lighting might help with the color correction in a roundabout way". Geometry still has gaps; lighting before we have full coverage will just lay shaded zero-regions on the residual. Defer until geometry coverage is consistently >50%. |

## User-requested 2026-W19 (round 2) — neural anchors & coverage fallback

### Shipped this turn

| | Item | Notes |
|--|------|-------|
| ✅ | **Low-coverage composite fallback** | When v8 mesh covers <25% of pixels (default), composite v8 over source video instead of rendering with black background. Two env vars: `FAO_PURE_MESH=1` disables (debug); `FAO_COVERAGE_THRESHOLD=0.25` adjusts threshold. Addresses user-reported "mostly black frames with residual glue". |

### User new feature requests

| | Item | Estimate | Notes |
|--|------|---------|-------|
| 🟨 | **Neural-secondary anchors with Kalman-style fusion** | 3-4 sessions | User's description: *"sparsely a 'neural anchor' to support our current anchors and try and randomly capture points and see if the reveal matches to basic ones over short bursts that we can try and correct... something like kalman filtering w.r.t. pixels instead of say temperature data maybe we can work in LUMA for that? then feed the transform matricies back and compare them"*. Translation: (1) generate sparse random secondary anchor points alongside primary feature-tracker anchors; (2) track each over a short window in luma space; (3) recover pose transform from secondary tracks independently; (4) Kalman-like filter combining both; (5) reject primary when secondary disagrees beyond threshold. Robustness layer for pose estimation. Useful where the user describes: *"some of the anchors throw off the camera estimation so some frames are basically 3d-transformed and some things might break from there"*. |
| 🟨 | **Per-anchor confidence score** | 1 session | Prerequisite for neural-anchor fusion. Each integrated anchor needs a confidence/quality metric so we can detect outliers. Existing `dm_cov` (depth-map coverage) is one signal; add residual-fit-error as another. |
| 🟩 | **Quaternion → richer pose representation** | 2 sessions | User: *"might require a quick matrix upsample for our existing to cover more advanced rotations etc"*. Could mean: encoder currently writes simple quat+pos poses; would need to encode rotation+scale+shear or 4×4 transform matrices for some content. Not urgent unless we see real failure cases. |

### Conceptual: "blindspot checker" abstraction
User noted the CPU-zero-output fix as a "blindspot checker". Worth 
generalizing: at each layer of the decode pipeline (mesh raster → 
residual apply → frame composite), add a guard that detects "produced 
output that's technically valid but visually zero" and triggers the 
next fallback. We already have this at:
- GPU rasterizer (any_nonzero gate)  ✅
- CPU rasterizer (any_nonzero gate)  ✅
- v8 composite path (coverage threshold) ✅ (this session)

Possible next places:
- Aligned-web canonical resolution (refuse if canonical mesh is empty)
- V8LR library resolution (refuse if library returns empty mesh)  
- T-frame anchor resolution (refuse if anchor lookup yields all-black)

## User insights 2026-W19 (round 3) — encoder root cause

### "the anchors are mostly on the UI elements"
This is the root cause behind both "skew" and "black frame" categories. 
The encoder's feature tracker locks onto the stable game UI (HUD, 
scoreboard, menus, killcam overlays) instead of the actual game scene 
(players, environment, action). 

Consequences:
- Mesh ends up representing the UI plane, not the 3D world
- When the camera pans away from heavy UI regions, dm_cov drops to 0%
- When new UI elements appear / disappear, mesh has to rebuild from 
  scratch, causing web swaps
- Pose estimation gets confused because UI is at zero depth (it doesn't 
  move with camera) so parallax is wrong

The layer-detector already produces an "active" layer classification 
(89-94% of pixels in user's DBD content). We should use it to bias the 
feature tracker away from non-active regions.

Fix candidates:
- **UI-region masking** (1-2 sessions): Don't accept feature tracks 
  whose pixel coordinates land in known-UI regions. Use the alpha_overlay 
  layer classification (already computed per-frame) to mark exclusion zones.
- **Active-content weighting** (1 session): Bias the feature-tracker 
  scoring function toward the active layer. Each candidate feature gets 
  a multiplier based on what layer it falls in.
- **Depth-prior filtering** (2 sessions): Reject features whose depth 
  estimate is exactly zero over multiple frames (a strong indicator of 
  static UI overlay).

### "we will want to see those webs merge eventually"
The canonical/aligned system identifies similar webs and stores them 
as deltas vs a canonical. But it doesn't MERGE the underlying meshes 
when two webs visit the same physical scene at different timestamps.

Filed: **Cross-web mesh merging** (3-4 sessions): When two webs are 
detected as similar (high alignment score), merge their meshes into 
one combined canonical. Resolves the case where a game returns to the 
same camera angle twice and we end up with two near-identical meshes.

### Diagnostic shipped this session
Added one-shot logging in `request_source_cache_fill` for each drop 
reason (no worker, no source path, bad dims, channel full) and for 
successful sends (at 1/10/100). User's next log will tell us why the 
source-fetch worker never produced diagnostics — most likely the 
channel-full case for the prewarm tasks.


## ✅ UI-overlay filter — all three axes shipped together

User asked to ship the most complete of 1/2/3. Shipped all three as an 
integrated system (see SESSION_LOG entry "UI-overlay filter shipped"):

- [x] Active-content weighting (axis 3, soft preference)
- [x] UI-region masking via layer mask (axis 1, hard exclude)
- [x] Depth-prior filtering via variance EMA (axis 2, hard exclude)

Watch in next encode log:
- {S}S/{D}D/{U}U/{O}O — UiOverlay count (the O); grows over time as filter 
  converges
- `[v8 ui-filter] anchor N: ...` — periodic diagnostic from maybe_densify

If results look good, the next encoder-side fix candidates are:
- Cross-web mesh merging (high alignment score → fuse meshes, not just 
  store delta) — 3-4 sessions
- Pose low-pass filter (better jitter handling) — 1 session
- Neural-secondary anchors with Kalman fusion — 3-4 sessions, needs 
  per-anchor confidence prereq

If the UI-overlay filter overfilters (mesh coverage drops too much), 
tuning knobs:
- `ui_overlay_promotion_threshold` (default 5) — raise to require more 
  frames of overlay-classification before promotion
- `ui_overlay_variance_threshold` (default 5.0) — lower to require even 
  tighter zero-parallax signature
- `ui_overlay_min_age_for_variance` (default 8) — raise to let young 
  tracks live longer before variance promotion kicks in


## New findings from round-4 log + user observations

### Root cause: phase correlation captures foreground motion in DBD content
The encoder's consensus classifier labels almost everything Dynamic 
because phase correlation's single motion vector tracks the dominant 
foreground subject (killer/survivor character) rather than the actual 
camera motion. Background scene features then disagree with this 
"camera motion" and get incorrectly classified Dynamic, excluding 
them from triangulation.

Fix candidates:
1. **Multi-region phase correlation** — sample phase correlation in 
   multiple regions (corners, edges) and pick the most-agreeing one 
   as background motion. Avoids the foreground dominance issue. 
   ~2-3 sessions.
2. **RANSAC over feature trajectories** — find the largest cluster of 
   features moving consistently together; assume that's the static 
   scene. Iterate. More robust than phase correlation for complex 
   content. ~3-4 sessions.
3. **Layer-weighted motion estimation** — when computing per-feature 
   motion residual, only consider features in Active layer. Drops 
   foreground-character features automatically. ~1-2 sessions, less 
   robust than (1) or (2) but cheap to ship.

### Mesh vertex coloring is frozen at integration time
Brightness/color seams during scrub come from mesh-color-from-frame-K 
vs source-color-at-frame-N mismatch. Mesh vertices are sampled from 
the YUV at integration time, so their color reflects only that one 
moment of lighting.

Fix candidates:
1. **Re-color mesh per-frame** — at decode time, re-sample vertex 
   colors from the current source frame. Eliminates the lag entirely 
   but requires source-video access at decode time (which we already 
   have via the source-fetch worker).
2. **Color residual encoding** — per-frame, store a small delta against 
   the mesh's baked color. Reconstructs to match the source frame. 
   This is the right long-term solution but requires bandwidth for 
   the residual.

### Visualizer ftype 8 = UiOverlay
Shipped this session. UiOverlay features now render with magenta ring 
in the World viewer. User can verify the filter is catching the 
expected UI elements (HUDs, scoreboard, etc).

### Source-fetch worker hang root cause
The `-vf select=between(...)` ffmpeg filter without `-frames:v` cap 
caused ffmpeg to decode the entire rest of the file looking for 
filter matches. A single fetch task on a 15-minute source video took 
minutes. Combined with channel capacity 2, this blocked all 
subsequent fetches. Fixed: switched to `-frames:v N`, raised channel 
cap to 16, added periodic timing diagnostics.


## Shipped this session (Tobii overlay + layer-robust + pose EMA + prewarm)

- [x] **Tobii-style anchor overlay** — full feature. gtk::Overlay + DrawingArea over player Pictures. 'A' keyboard toggle + ◉ Anchors UI button. Anchors colored by motion-class. V8AT trailer wires encode→export→load→render. (User asked for this last session; we shipped data capture + serialization first, finished UI this session.)
- [x] **Layer-weighted motion estimation** (`classify_with_layers`) — addresses "everything classified Dynamic" root cause. When a layer mask is available, computes median (dx, dy) over Active/ParallaxBg-classified features and uses THAT as the consensus camera motion. Phase correlation fallback when too few background features. Diagnostic log at frame counts 1/10/100/1000/5000.
- [x] **Pose EMA low-pass** (`FAO_POSE_EMA=<alpha>` env var) — soft pose smoothing as an alternative to the binary-threshold `FAO_LESS_JITTER` mode. Quaternion lerp + renormalize. Capped at alpha=0.95.
- [x] **Source-fetch prewarm timing** — reordered so `load_fao_frame(0)` runs BEFORE prewarm calls. Was the actual cause of "no batch decoded" in round-3 log: dims-check bailed because cur_w/cur_h/vid_fps weren't populated yet.

### What's left on the layer-robust front
The layer-weighted motion change should significantly reduce false-Dynamic 
classifications, but DBD content has another wrinkle: phase correlation 
itself can also be wrong (locked to foreground motion). The classifier 
falls back to phase correlation when < 8 background features are 
available. If even the background features are sparse (kill scenes, 
many overlapping UI elements), the fallback may stay wrong.

Next-step candidates if this isn't enough:
- RANSAC over feature trajectories (find dominant motion cluster from 
  features directly, ignoring phase correlation entirely) — 3-4 sessions
- Multi-region phase correlation — sample corners/edges, pick most 
  internally-consistent region — 2-3 sessions


## Also shipped: edge-feather composite (FAO_COMPOSITE_FEATHER)

Hard-cut composite at mesh edges produces visible seams ("glue artifact" 
user reported). Added optional N-pixel feathering:

- `FAO_COMPOSITE_FEATHER=1` — 9-pixel neighborhood, ~5ms extra per 1080p frame
- `FAO_COMPOSITE_FEATHER=2` — 25-pixel neighborhood, ~14ms
- `FAO_COMPOSITE_FEATHER=3` — 49-pixel neighborhood, ~30ms
- Unset/0 — original hard-cut behavior

For each pixel, count how many of its neighbors have v8 content. Use 
that fraction as the v8/source blend weight. Pixels fully inside the 
mesh stay full-v8; pixels at the edge softly fade to source.

Cost is real (5-30ms is notable at 60fps playback), so it's opt-in 
behind the env var. User can A/B test against hard-cut to see if the 
feathering helps the "negative or brightness differences" they reported.


## Critical fix this session: coverage-aware photometric residual

User's log showed 1.2GB FAO vs ~300MB AV1 — ~4× too big. Root cause:

`compute_photometric_residual` computed `delta = source - predicted` 
for EVERY pixel in the frame. When mesh coverage is only 20-30%, the 
other 70-80% of pixels have `predicted = 0` (no triangle covered them), 
so `delta = source - 0 = source`. The residual essentially STORED THE 
FULL SOURCE VIDEO at quarter resolution in the uncovered regions. After 
zstd that's 200-300KB per frame — uncompressed source is ~388KB raw 
for 1920×1080 quarter-res RGB.

Math:
- Old Z frame avg: 63KB (export total / Z count)
- T frame avg: 68KB
- Combined frames: ~616MB of frame data + 11MB trailers = 627MB expected
- Actually reported: 1714MB
- Difference: ~1.1GB of bloat, entirely from residual encoding

Fix: count covered pixels per block. If < 25% of block is mesh-covered, 
emit sentinel `(128, 128, 128)` — zero residual contribution. zstd 
compresses runs of 128 to near-zero. For covered blocks, compute the 
average ONLY over covered pixels.

Decoder-compatible: no format change. Same residual bytes, just smaller. 
Old decoders read the residual identically.

Expected size impact: residual drops from 250-300KB to 1-30KB on typical 
sparse-mesh frames. Z frame avg should drop from 63KB to ~3-10KB. Total 
file should fall from 1.7GB to ~700MB-800MB range.

Diagnostic added: `[v8 residual] #N: raw=KB compressed=KB ratio=X.X sentinel=A/B (Y% uncovered)` 
prints at counts 1/10/100/1000/5000+.


## Filed: subject-anchor classification refinement

User observed "killers hand was tracked as an extra point now which 
is lots of pixels -> single pose" — a single feature anchor lands on 
a foreground subject (DBD killer's hand) and contributes wrong motion 
to pose estimation, even though it's just one anchor.

Current behavior: features get classified Static/Dynamic/UiOverlay via 
MotionConsensusPass. If the layer-robust median agrees with a feature's 
motion, the feature is Static. When a foreground subject moves with the 
camera coincidentally (or the median is contaminated), the subject 
feature ends up Static and pollutes pose estimation.

Possible fixes:
1. **Motion-magnitude gate**: features with absolute per-frame motion 
   > N px get a "motion-suspicious" flag. PoseTrack weights them lower.
2. **Cluster detection**: features that move together (motion within 
   1px of each other) and disagree with the consensus form a foreground 
   cluster. Whole cluster gets Dynamic, regardless of individual consensus.
3. **Spatial isolation gate**: a single feature ISOLATED from other 
   classified-Static features (distance > 50px to nearest Static) is 
   suspicious. Probably a feature on a foreground subject.

(1) is simplest; (2) is most principled; (3) is cheapest to implement 
but most fragile. Each is half-to-one session.


## Shipped this session: 10× v2.1 + cluster detection

### `FAO_V21_TINY=1` — quarter-res T-frame residual

User asked to 10× v2.1 (T-frame 68KB → ~7KB target). Two changes:

1. **Quarter-res residual**: encoder downsamples residual 4×4 instead of 2×2. Raw residual is 1/16 the size (vs 1/4 at half-res). Each quarter-res sample applies to a 16-pixel tile on decode.
2. **2× aggressive SAD threshold**: `sad_skip_threshold = quant_step * 4096` instead of `* 2048` in tiny mode. Twice as many blocks skip residual entirely (rely on motion compensation alone). Started at 3× but pulled back to be safer on quality — 3× had threshold ≈ 168 luma units per pixel which is half the dynamic range, likely too aggressive.

Combined: T-frames should drop from ~68KB → ~10-15KB on typical content (about 5×). PSNR loss ~2-4dB. Not literally 10× (the math allows it but pushing harder hurts quality more than it should) — picked safer numbers to avoid breaking playback quality.

Format compatibility:
- New flag bit 0x04 encodes quarter-res in the existing flags byte
- Existing 0x01 (half-res) and 0x02 (motion-only) unchanged
- Decoder recognizes all three modes — old .fao files play unchanged
- New .fao files encoded with `FAO_V21_TINY=1` play correctly on decoders without the env var set

Decoder branches on `flags & 0x04` for the 4×4 upsample, falls through to 0x01 half-res, or to legacy full-res.

### Cluster detection — addresses "killer's hand single-pose" observation

User's observation: "killers hand was tracked as an extra point now which is lots of pixels -> single pose". A foreground feature can get classified Static when its motion coincidentally matches the consensus camera estimate, polluting pose estimation.

Added a cluster-detection pass to `classify_with_layers` after the main classification loop:

1. Gather all currently-Static-classified mature tracks
2. For each, find peers with similar motion (within 2px on both axes)
3. For groups of >=3 features (a "cluster"), check if cluster's mean motion deviates from consensus camera motion by >3px on either axis
4. If yes → demote all cluster members to Dynamic + zero their consensus score (so they stay Dynamic for a few frames)

Algorithm is O(n²) but n is small (~100-200 live features), ~10-40K comparisons per call — negligible cost.

Diagnostic: `[v8 cluster-demote] #N: K features demoted from Static→Dynamic (cluster moving with cam_diff>3px from consensus dx=X dy=Y)` at counts 1/10/100/1000/5000+.

Expected effect: when the killer's hand has multiple features tracking it, they form a cluster moving together but differently from the camera — all get demoted to Dynamic. Pose estimation then ignores them, fixing the contamination.

### Expected size impact (DBD content)

Before this session:
- T frames: 8582 × 68KB = ~588MB
- Z frames (with residual bloat): 17650 × ~63KB = ~1.1GB
- Total: ~1.7GB

After this session:
- T frames with FAO_V21_TINY: 8582 × ~12KB = ~100MB (~5× reduction)
- Z frames with coverage-aware residual: 17650 × ~5KB = ~88MB (12× reduction)
- Trailers: ~11MB
- Total: ~200-250MB

Target was 300MB (AV1). If both fixes pan out, FAO could match or beat AV1 — though that's an optimistic estimate; real-world ratio depends on residual content and PSNR tradeoffs.


## Also this session: composite colormatch + residual gate refinement

### `FAO_COMPOSITE_COLORMATCH=1` or `=2` — addresses brightness seams during scrub

The mesh's vertex colors are baked at integration time (one specific frame's lighting). During scrub, the source video's lighting changes but the mesh stays constant. Where mesh meets source in the composite, the color mismatch is visible as a seam.

Added a pre-pass that:
1. Walks the frame at stride 4 (10K samples typical at 1080p)
2. Identifies pixels where mesh has content AND a 4-neighbor doesn't (boundary)
3. Accumulates the per-channel delta `(source - mesh)` over boundary samples
4. If ≥50 samples found, applies the mean shift to all mesh pixels before compositing
5. Shifts capped at ±30 per channel to prevent overcorrection on noisy boundaries

Strength `1`: half the measured delta (mild). Strength `2`: full delta.

Works with both hard-cut and feathered composite. Diagnostic: `[FAO composite] colormatch #N: shift=(R,G,B) from K boundary samples (strength=S)`.

Cost: O(pixels/16) → ~130K ops/frame at 1080p, ~0.5ms — basically free.

### Residual encode gate tightened

Photometric residual is now skipped when scene_mesh has < 100 vertices, in addition to the existing "empty mesh" gate. Sparse meshes produce mostly-uncovered residual blocks anyway, and skipping the rasterize+diff step saves 5-15ms per such frame on the encoder. Worth it during chunk-startup phases when mesh is still building.

### Cluster detection optimization

Marked legitimate non-foreground clusters as visited so we don't re-evaluate the same large cluster from each of its members. Background-dominated frames now run cluster detection in ~O(N) rather than O(N²).


## This session — user requested architectural pivot to v9

User filed (verbatim):

> "I think this made the anchoring slightly worse or just slightly different... I think what we want is something more like warp and then build mesh and then GPU greedy the mesh into 3D DCT with filters and transforms and scales and compress that data as the output. This lets us drop triangles but keep world. I think we also want full cv object recognition and 3D-depth mapping like those AI research papers that do object saliency maps in grayscale."

### Near-term response

Cluster detection turned out to slightly hurt anchor quality on the user's content. Made it opt-in via `FAO_CLUSTER_DEMOTE=1`. Default behavior now matches pre-cluster-detection.

Also added `FAO_NO_ROBUST_MOTION=1` so the user can disable the layer-robust median estimator and compare it head-to-head against phase correlation alone. This addresses the "slightly different" half of the user's observation — if layer-robust is the culprit, this lets them isolate it.

### Long-term v9 architecture filed

See `docs/V9_ARCHITECTURE.md` — a 185-line design doc capturing the user's vision:

- Warp → build mesh → 3D DCT representation (instead of explicit triangles)
- Saliency-weighted quantization (more bits to perceptually-important voxels)
- ML-driven object recognition + depth refinement
- Sparse 3D grid (octree / hash grid)
- View-conditioned colors (not baked at integration time)

Estimated 8-12 sessions for an MVP. Pre-requisites:
- Pick ONE saliency model (DepthAnything, SAM, spectral residual, etc.)
- Pick ONE 3D representation (sparse voxel grid vs hash grid vs octree)
- Prototype encoder + decoder on a single chunk
- Compare against v8 for size and quality

This is post-v8.x. The next 2-3 sessions should focus on stabilizing v8 before pivoting.

### What stays in v8.x

- Tobii overlay (shipped)
- Layer-robust motion (shipped, env-var-toggleable)
- Pose EMA (shipped)
- Coverage-aware residual (shipped — big size win)
- FAO_V21_TINY (shipped)
- Composite colormatch (shipped)
- Cluster detection (shipped, opt-in)

Filed for v8.x but not yet shipped:
- v8.2 textures on canonical webs
- v8.3 pose & feature delta
- v8.4 mesh simplification (probably moot if we go v9)
- Wire GPU phase correlation


## v9 PROTOTYPE SHIPPED (this session)

User said "make v9 even if it might fail, do hard things, come back to v8 later".

Code: `fao_codec_lib/src/v9_volumetric.rs` (~1100 lines, single module).

### Pipeline (encode)
- `build_v9_chunk_from_v8(mesh, feature_positions, resolution)` is the entry point
- `VoxelHashGrid` (Morton-keyed sparse 3D grid) + `greedy_splat_mesh` (8×8 barycentric sample per triangle, 3×3×3 Gaussian falloff into voxels)
- `accumulate_feature_saliency` adds per-voxel feature counts as a saliency proxy
- `encode_v9_chunk` walks 4×4×4 voxel blocks, builds 31 channels (density + 3 base color + 27 SH), forward 3D DCT each, quantizes with `QUANT_MATRIX_4X4X4` × per-block scale × saliency boost
- `write_v9_trailer` serializes to `V9DC` magic
- Wired into export at `fao_app/src/main.rs` immediately after V8MX trailer write

### Pipeline (decode)
- `read_v9_trailer` parses
- `decode_v9_chunk` dequantizes + IDCTs + splats voxels back into a hash grid
- `ray_march(grid, pose, w, h)` casts a ray per pixel, slab-AABB intersect, half-voxel-step density compositing, SH evaluation for view-conditioned color
- "Signs of life" path: on load of .fao with V9DC, dumps `/tmp/v9_signs_of_life.ppm` automatically

### Env vars (all new)
- `FAO_V9=1` — encode V9DC alongside V8MX (default off)
- `FAO_V9_RES=64` — voxels per axis (default 64, range 8-256)
- `FAO_V9_MAX_CHUNKS=N` — cap chunks processed (default 0=all)
- `FAO_V9_NO_SIGNS=1` — suppress automatic PPM dump

### Outstanding v9 work (TBD if v9 shows signs of life)
- GPU compute shader for splat (CPU is slow on big content)
- Multi-view SH fit (need per-vertex per-frame color samples; not stored in v8 currently)
- ML saliency model integration (DepthAnything / SAM / SalNet)
- Mesh segmentation per voxel block — semantic awareness so the codec knows "this is sky, this is character"
- Wire ray-march into live playback (currently only the signs-of-life dump path uses it)
- Per-block delta encoding (compress v9 chunk N against chunk N-1 like P-frames)
- Wavelets vs DCT comparison
- 8³ block size experiment (smaller blocks compress less but render more accurately)

### Decision point

Encode a real video with FAO_V9=1, look at `/tmp/v9_signs_of_life.ppm`. 
If it shows a coherent blob → continue investing. If it's black/random → fix the bug or roll back.

The risk of starting v9 is bounded: v8 playback is untouched, V9DC is a trailer-only addition.
