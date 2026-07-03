# v8+ Roadmap: Closing the Gap with AV1

> **v8.1 STATUS: SHIPPED** (behind FAO_V81 env var). See SESSION_LOG.md for details.

The current v8 reconstruction codec hits ~8x AV1 on test footage with 
varied scenes (e.g., a horror movie proxy). To close that gap requires 
fundamental quality and compression improvements, not just tuning.

## What's wrong today

### A. Vertex colors were placeholders (JUST FIXED)
The MeshIntegrator was painting vertices with [g*0.4, g*0.85, g] where 
g = depth confidence. So every mesh rendered in a teal-shaded 
confidence-visualization regardless of scene content. This is why the 
encoded video looked "blocky and bluish-green".

FIXED: integrate() now receives the source frame YUV and samples real 
RGB at each vertex's projected source-pixel location. Mesh now carries 
actual scene colors. Single biggest quality improvement to date.

### B. Vertex-only colored mesh has inherent limits
Even with correct vertex colors, ~10k-100k vertices per chunk means each 
~20x20-pixel triangle gets ONE color gradient between three vertices. 
Fine detail (textures, fabric, foliage) cannot be captured by a sparse 
mesh — it lives at sub-triangle frequencies.

AV1 captures this in 4x4-pixel transform blocks with full DCT coefficients. 
v8 effectively captures it in ~20x20-pixel barycentric gradients.

### C. Per-frame overhead is ~6KB
At 30fps that's 5.4Mbps for the per-frame bitstream alone, before any 
mesh data. AV1 at the same content quality is often <2Mbps total.

## v8+ proposed enhancements (compression-quality)

### v8.1 — Per-frame photometric residual track (HIGH IMPACT)
Encoder generates a small RGB residual at 1/4 or 1/8 source resolution 
capturing the delta between mesh-render and ground truth. Stored zstd'd 
in the frame payload. Decoder adds residual to mesh render after 
rasterization.

Expected impact:
- File size: +20-50% per frame (residual cost)
- Quality: dramatically better (captures sub-triangle detail)
- Net win: bytes/PSNR ratio improves by ~3x because the mesh becomes 
  a coarse low-freq prediction and the residual fills in highs

Format: new V8Frame variant 'r' (V8r) — magic + n_features + pose + 
features (existing) + residual_w + residual_h + residual_zstd_payload.

### v8.2 — Texture maps for canonical webs (BIGGEST IMPACT)
Each canonical web in the trailer carries a shared UV-mapped texture 
(e.g., 256x256 or 512x512). Per-vertex UV coords (4B) replace 
per-vertex RGB (12B). Decoder samples texture bilinearly during 
rasterization.

Encoder pipeline:
1. UV-unwrap the chunk mesh (parameterize 3D mesh onto 2D plane)
2. Reverse-project source frames onto the parameterization
3. Average / median-blend across frames in the chunk → texture
4. zstd-compress texture (raw or DXT5/BC3 — 4:1 if BC)

Expected impact:
- File size per chunk: shrinks ~30-50% (vertex colors gone, texture 
  is shared across all frames in chunk)
- Quality: massive jump — per-pixel detail through the entire chunk

### v8.3 — Pose & feature delta encoding (LOW EFFORT, MODEST WIN)
Pose changes slowly between frames (camera moves smoothly). Feature 
positions track motion. Delta from prev frame:
- Pose: 48B → ~12-15B (3 quantized position deltas + 4 quantized 
  rotation deltas)
- Features: 5B per feature → ~2B per feature (delta sx, sy)

Expected impact:
- Per-frame: 6KB → ~3KB. 50% reduction.
- Quality: identical (lossless delta).

Needs: a per-frame "is_keyframe" bit that switches between absolute 
and delta encoding (every Nth frame is absolute to allow seeking).

### v8.4 — Mesh simplification at chunk close (MODERATE WIN)
Apply Garland-Heckbert quadric error metric to drop redundant vertices 
that don't contribute to surface variation. Especially helpful for 
flat regions (walls, sky).

Expected impact:
- Mesh size per chunk: 30-50% reduction
- Quality: imperceptible loss at moderate decimation, useful for 
  dialing in size/quality tradeoff

### v8.5 — Spherical harmonics for lighting (LATER)
Store 9 SH coefficients per chunk representing ambient lighting. 
Vertices store base albedo + normal; render uses SH for shading. 
Supports relighting and simpler vertex data.

### v8.6 — Adaptive mesh density (LATER)
sample_stride varies by local depth variance + texture complexity. 
Flat regions get sparse vertices; high-detail regions get dense.

## Realistic targets

- v8.1 alone: 8x AV1 → 4-5x AV1 (quality jumps, modest size growth)
- v8.1 + v8.3: 8x AV1 → 3-4x AV1
- v8.1 + v8.2: 8x AV1 → 2-3x AV1 (texture is the key)
- v8.1 + v8.2 + v8.3 + v8.4: 8x AV1 → 1.5-2x AV1

To MATCH or BEAT AV1 on natural video, we'd need:
- All of the above
- Plus a proper inter-frame motion model (which is what AV1 does well)
- Plus adaptive quantization based on perceptual masking

Realistic plan: v8.1 next, v8.2 the session after, v8.3+ as polish.

## Pre-warmed GPU ops — NOT a compression win

The user is correct: pre-warming wgpu::Device + ComputePipelines saves 
~200ms of startup per encode. This affects ENCODE LATENCY, not 
COMPRESSION RATIO. File sizes don't change. Useful for batch jobs and 
realtime encoding, not for "8x AV1 → smaller".
