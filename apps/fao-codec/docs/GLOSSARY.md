# Glossary

Terms used throughout the FAO codebase. Some are standard 3D / video 
terms; some are FAO-specific.

## Standard terms

**AV1** — AOMedia Video 1, a modern royalty-free video codec. Our 
reference for what natural-video compression looks like.

**BT.601** — color space conversion matrix for SD video. Used in YUV → 
RGB conversion in the mesh integrator (and almost everywhere else 
because most ffmpeg output is BT.601 limited-range).

**DCT** — Discrete Cosine Transform. Used by v2.1 for K-frame and 
T-frame block coding.

**FAST** — Features from Accelerated Segment Test. Corner detector 
used by the v8 feature pass.

**Lucas-Kanade (LK)** — iterative optical flow algorithm. Used to 
track features between consecutive frames in v8 encoding.

**PSNR** — Peak Signal-to-Noise Ratio. Standard quality metric for 
compressed video.

**SH** — Spherical Harmonics. Compact basis for low-frequency 
directional functions (lighting). Mentioned in v8.5 roadmap.

**YUV420** — chroma-subsampled color format. Y full resolution, U/V at 
half horizontal + half vertical.

## FAO-specific terms

**Anchor** — a frame chosen by DensifyPass for full depth-map 
densification. Other frames between anchors interpolate sparse 
features only.

**Canonical web** — a web (mesh chunk) that is the primary geometry 
source for its scene. kind=1. Aligned webs (kind=0) reference a 
canonical and store only transform + residuals.

**Chunk** — a contiguous frame range that an encoder worker processes 
together. Produces one web (after merge-time repartition). Typically 
60-360 frames.

**DensifyPass** — converts sparse triangulated features into a dense 
depth map at anchor frames. Bilinear interpolation between sparse 
points.

**FAO** — Frame-Aligned Output. The codec / container name.

**FeaturePass** — per-frame feature detection (FAST) + tracking (LK) 
+ triangulation. Produces 2D screen positions + 3D triangulated 
positions.

**K-frame** — full DCT keyframe in v2.1. Equivalent to an I-frame in 
H.264.

**Library** — persistent multi-video web store. See LIBRARY_MODE.md.

**LibraryRef** — pointer from a chunk in a .fao file to a canonical 
mesh stored in an external library. UUID + alignment transform + 
optional residual vertices.

**MeshIntegrator** — accumulates dense depth maps into the persistent 
SceneMesh, with cross-anchor vertex fusion.

**Pose** — camera position + rotation + intrinsics (focal, principal, 
distortion). 48 bytes per frame.

**Repartition** — at chunk merge time, the scattered ext_fidx ranges 
of non-empty chunks get reshaped into contiguous non-overlapping 
spans. Required because workers see backpressure-round-robin scattered 
frame indices.

**SceneMesh** — persistent 3D mesh accumulated across frames in a 
chunk. Vertices + colors + indices + normals + vertex_grid (for 
fusion) + vertex_hits (for blending).

**Signature** — 88-byte compact descriptor of a web's geometry. Used 
for library matching.

**T-frame** — translation residual frame in v2.1. Encodes (dx, dy) per 
block plus optional DCT residual. Equivalent to a P-frame in H.264.

**Tier** (in FrameTag) — encodes whether a frame is `skip` 
(duplicate), `passthrough` (verbatim source), `full` (re-encoded), or 
`anchor` (scene cut).

**Triangulation (in FeaturePass)** — recovering 3D position of a 
feature from its observations across multiple frames with known poses. 
Linear-triangulation with confidence weighting.

**V8MergeCache** — caches the result of the chunk-merge step so that a 
double-export call doesn't drain workers a second time and emit an 
empty trailer. Recent bugfix; cache is now mandatory.

**Web** — a chunk's mesh after alignment. The library mode primary 
unit. A web has kind=0 (aligned to a canonical), kind=1 (canonical 
itself), or kind=2 (standalone).

**WebSignature** — see Signature.

**WGSL** — WebGPU Shading Language. Used by the GPU phase correlation 
pipeline.

## Marker bytes (.fao frame header)

| Byte | Codec | Meaning |
|------|-------|---------|
| 'T'  | v2.1 | Translation residual |
| 'K'  | v2.1 | Keyframe |
| 'V'  | v3-v5 | Intermediate codecs (mostly retired) |
| 'Z'  | v7/v8 | Reconstruction (pose + features) |
| 'P'  | v7 | Perception (objects + bboxes) |
| 'B'  | -    | Boundary marker |

## Trailer magics (.fao trailer header)

| Magic | Meaning |
|-------|---------|
| "V8MX" | Mesh eXchange — chunk-aligned web list with meshes |
| "V8LR" | Library Reference — uuid + transform pointing to external library |

## File extensions

| Extension | Meaning |
|-----------|---------|
| `.fao` | FAO container |
| `.fao.idx` | Side-car index (rare; usually inlined) |
