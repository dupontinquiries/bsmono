# FAO v9 — Volumetric Architecture

User filed this as a major architectural pivot:

> "I think what we want is something more like warp and then build mesh
> and then GPU greedy the mesh into 3D DCT with filters and transforms
> and scales and compress that data as the output. This lets us drop
> triangles but keep world. I think we also want full cv object
> recognition and 3D-depth mapping like those AI research papers that
> do object saliency maps in grayscale."

This document is a roadmap for the redesign. It is intentionally
**non-binding** — every section is a research direction, not a
specification. Numbers and structure choices below are best-guesses
that should be confirmed (or rejected) before commitment.

## Core insight

v8 stores an **explicit triangle mesh** with per-vertex colors. Every
optimization since then (texture maps, mesh simplification, color
residual) tries to make the explicit representation smaller. But the
mesh is an *approximation* of a volumetric truth — a 3D scene with
density, color, and motion. Storing the approximation is wasteful when
we could store something closer to the truth.

v9's premise: **don't store mesh. Store the world.**

## Pipeline sketch

```
   frames N..N+T
        │
        ▼
   ┌──────────┐
   │  warp    │ ◄── what we already have (phase corr + LK)
   │ (motion- │     produces stabilized frame stack in a
   │  comp.)  │     common reference space
   └──────────┘
        │
        ▼
   ┌──────────┐
   │ build    │ ◄── triangulate or back-project per existing
   │ volume   │     v8 path, BUT THE OUTPUT IS A 3D GRID
   │          │     (occupancy + color + depth confidence)
   └──────────┘
        │
        ▼
   ┌──────────┐
   │ saliency │ ◄── object recognition / depth model
   │  + depth │     produces per-voxel importance weight
   │  (CV/ML) │     and refined depth estimate
   └──────────┘
        │
        ▼
   ┌──────────┐
   │   3D     │ ◄── chunk the volume, transform each chunk
   │  DCT     │     to frequency domain (or wavelet)
   │ greedy   │     greedy quantization: more bits to high-
   │ encode   │     saliency or high-magnitude coeffs
   └──────────┘
        │
        ▼
    .fao bytes
```

## What changes vs v8

| Concern              | v8                          | v9                                |
|----------------------|-----------------------------|-----------------------------------|
| Geometry primitive   | triangle mesh               | 3D voxel grid                     |
| Per-frame data       | pose + feature deltas       | pose + view-conditioned DCT delta |
| Chunk data           | mesh trailer (V8MX)         | volumetric trailer (V9DC)         |
| Color representation | per-vertex RGB              | per-voxel RGB + density           |
| Lighting             | baked at integration time   | view-conditioned (cosines of pose)|
| Detail allocation    | uniform mesh density        | saliency-weighted quantization    |
| Decode               | rasterize triangles         | ray-march or splat voxels         |

## Open design questions

These are the decisions that need answering before any code:

1. **Grid resolution and sparsity.** A naive 256³ voxel grid is
   16.7M voxels per chunk — 200MB raw at u32/voxel. Need a sparse
   representation: octree, hash grid (like Instant-NGP), or run-length.
   How sparse is typical content? What gain do we get from sparsity?

2. **DCT block size.** 2D codecs use 8×8. For 3D, 4×4×4 (64 coeffs)
   or 8×8×8 (512 coeffs)? Block size trades blocking artifacts vs
   compression efficiency.

3. **Quantization matrix.** JPEG has hand-tuned 8×8 quantization
   tables based on psychovisual studies. Does an equivalent exist for
   3D? Need to either find existing research or design our own —
   probably depth-axis (parallax sensitivity) differs from
   spatial axes.

4. **Wavelet vs DCT.** Wavelets compress smooth gradients better
   and degrade more gracefully. DCT is simpler and has more existing
   tooling. Either could work.

5. **What does "greedy" mean.** User said "GPU greedy the mesh
   into 3D DCT". Interpretations:
   - Greedy quantization: iteratively assign bits, highest-magnitude
     coefficient first, until budget exhausted.
   - Greedy mesh-to-voxel: walk the mesh, splat into the grid,
     accumulate.
   - Greedy chunking: pick chunk boundaries that minimize total
     coefficient entropy.
   All three could be relevant — they're separate decisions.

6. **Saliency model.** User mentioned "object saliency maps in
   grayscale" — what model? Some candidates:
   - SAM (Segment Anything) for object detection
   - DepthAnything or Midas for monocular depth refinement
   - Spectral residual saliency (classical, fast)
   - SalNet / DeepGaze (psychovisual saliency)
   Each has different latency / accuracy / dependency tradeoffs.
   Need to pick ONE and stick with it for v9 MVP.

7. **Object recognition: detection vs segmentation vs classification.**
   "Full CV object recognition" is broad. For our compression goal,
   semantic segmentation (per-pixel class) probably matters more than
   detection (bounding boxes) — it tells us which voxels share
   semantic identity and can be predicted from each other.

8. **Online vs offline.** Can we run ML models in-encode (slow but
   well-integrated) or as a separate preprocess pass that feeds
   metadata into the encoder?

9. **Backward compatibility.** v8 .fao files must keep playing. v9
   would be a new format tag — V9DC or similar. Player auto-detects.

## Why this is multi-session

Even the smallest viable v9 needs:
- A sparse 3D grid implementation (1-2 sessions)
- 3D DCT or wavelet encoder + decoder (1-2 sessions)
- Mesh-to-voxel splat (1 session)
- Saliency model integration (2-3 sessions, depends on model choice)
- Trailer format design + serialization (1 session)
- Decoder ray-march / splat (1-2 sessions)
- Quality + size benchmarking (1 session)

Total: ~8-12 sessions for an MVP. This is a v9 milestone, not a quick
patch.

## What we should do BEFORE v9

The current v8 still has issues to nail before pivoting:
- Pose stability is still occasionally off
- Anchor classification (Static/Dynamic/UiOverlay) is improving but
  not perfect
- Mesh coverage is 13-28% when working — fundamental limit until v8.2
  textures are added
- Compression is now competitive with AV1 thanks to the residual fixes

v8 is "good enough to compare against AV1 on real content". v9 is the
path to "substantially better than AV1 on real content because we
understand the scene". The migration plan is:
1. Stabilize v8 (this and next 2-3 sessions)
2. Pick ONE saliency model and integrate offline (proof of concept)
3. Pick ONE 3D representation (sparse grid vs hash vs octree) and
   prototype encoder + decoder
4. Compare on a single chunk against v8 for size and quality
5. If win, build the rest. If lose, document why and revisit.

## Reference reading

(User mentioned "AI research papers that do object saliency maps in
grayscale" — these are the right starting points.)

- DepthAnything v2 (monocular depth, 2024)
- SAM 2 (segmentation, 2024)
- Instant-NGP (hash-grid 3D representation, 2022)
- Plenoxels (sparse 3D grid, 2022)
- DeepGaze IIE (psychovisual saliency, 2018)
- VVC / AV2 (current state of the art video codecs — what we're
  trying to beat)

## Not in scope for v9

- Full neural rendering (NeRF / Gaussian splat) — too compute-heavy
  for the decoder
- ML-trained codec (like Google's NCC) — out of scope for an
  open-format target
