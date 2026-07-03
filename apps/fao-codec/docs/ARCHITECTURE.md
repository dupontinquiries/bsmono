# FAO Architecture

## Crate layout

```
fao_codec_core/        Primitives shared by every codec version
├── bitio.rs           Bit reader/writer
├── bitpack.rs         Variable-length packers (zig-zag, golomb)
├── chroma.rs          YUV ↔ RGB conversions
├── color/             SIMD color converters (avx2/neon/sse2/scalar)
├── dct.rs + variants  DCT/IDCT (8×8, scalar + neon + sse)
├── f16.rs             Half-precision float for compact depth storage
├── frame_tag.rs       16-byte per-frame index tag (tier, fidx, ftype)
├── pixel_filters.rs   Box / bilinear / bilateral
├── quant.rs           Quantization tables
└── region_activity.rs Per-region change detection

fao_codec_lib/         Higher-level codecs + library mode
├── block_dict.rs      Dictionary-coded block reuse
├── block_store.rs     Disk-spillable chunk store
├── gpu_phase_corr.rs  WGSL phase correlation pipeline
├── gpu_pipeline.rs    wgpu device/queue setup
├── library.rs         Multi-video web library (Phase 1-3 complete)
├── phase_corr.wgsl    Stockham FFT in WGSL
├── v21_predictive.rs  v2.1 enhancements
├── v2_encoder.rs      v2.0 reference encoder (used as fallback)
├── v3_encoder.rs      v3 attempt (largely superseded)
├── v3_meshed.rs       v3 with mesh hints
├── v4_codec.rs        v4 attempt (unused)
├── v5_codec.rs        v5 attempt (unused)
├── v6_object_codec.rs v6 — object-segmentation prototype
├── v7_perception_codec.rs v7 — perception with bounding boxes
├── v8_rasterizer.rs   v8 decode-side mesh rasterizer
└── v8_reconstruction_codec.rs v8 encode + decode (main file, ~6700 lines)

fao_codec_v1/          v2.0/v2.1 reference encoder (legacy crate name)
fao_codec_v2/          Placeholder; currently empty
fao_app/               GTK4 desktop app (single 10k-line main.rs)
fao_cli/               Command-line tool (library init/list/info/verify)
```

## Encode pipeline (v8)

```
                              ┌─ ffmpeg readers (4 threads × 2 ffmpeg = 8 streams)
                              │
[input video] ──ffprobe──┐    ▼
                         │  ┌──── frame queue ────┐
                         │  │                     │
                         ▼  ▼                     ▼
                   [worker pool 0..N-1]  (Mutex<V8Encoder> shards)
                          │
                          │  each worker:
                          │   1. yuv→rgb conversion (SIMD)
                          │   2. v8 attempt: FeaturePass + DensifyPass
                          │                  + MeshIntegrator + V8Frame::to_bytes
                          │   3. v2 attempt: K-frame or T-frame + residual
                          │   4. choose smaller of (v8, v2) at policy threshold
                          │
                          ▼
                  [ordered disk writer]
                          │
                          │  writes:
                          │   - .fao frame index entries
                          │   - per-frame compressed payloads
                          │   - V8MX trailer (chunk meshes)
                          │   - V8LR trailer (library refs, if enabled)
                          │
                          ▼
                     [output.fao]
```

Key choices:
- **Sharded encoder state**: each worker has its own V8Encoder. This 
  avoids global Mutex contention on the per-frame hot path.
- **Repartition at merge time**: workers see SCATTERED ext_fidx 
  (round-robin backpressure), so chunks end up with overlapping frame 
  ranges. At trailer-write time we repartition into contiguous 
  non-overlapping spans. Required for correct decoder web lookup.
- **V8MergeCache**: caches the result of the merge step so a double-
  export call doesn't drain workers a second time and emit an empty 
  trailer. Was a critical bug; cache is now mandatory.

## Decode pipeline (v8)

```
[.fao file] ──read header──> [index loaded]
                 │
                 ├── read V8MX trailer → web list (meshes + transforms)
                 ├── read V8LR trailer → library refs (uuid + alignment)
                 │
                 ▼
       [V8Decoder ready]
                 │
   playback loop ▼
                load_fao_frame(idx):
                  read frame payload
                  V8Decoder::render_frame_rgb(payload, idx):
                    1. parse V8Frame for camera pose
                    2. check library_refs for a covering ref:
                       - resolve canonical mesh from library
                       - apply inverse alignment transform
                       - rasterize → return
                    3. else: find local web for this frame
                       - render web.mesh (kind=1/2) directly OR
                       - find canonical-in-file (kind=0) + transform
                    4. if all fail: caller falls back to source-fetch 
                       (if trailer has webs) or previous frame (if empty)
```

## Library mode (Phase 1-3 complete)

```
Library on disk:
~/.fao_library/
├── manifest.json         (web index + signatures + ref counts)
├── lock                   (sentinel for writable handle)
└── webs/<uuid>/
    ├── mesh.bin           (zstd SceneMesh)
    └── pose.bin           (compact pose track)

Encode-time integration:
  V8Encoder has Arc<RwLock<Library>>.
  At chunk close, signature is computed.
  Library.find_match(signature, threshold) → Option<(stub, score)>
  On match: chunk's mesh is DROPPED, a LibraryRef is recorded.
  On no match (and writable): chunk's mesh is INSERTED, uuid issued.

Decode-time integration (just shipped):
  V8Decoder has Optional Arc<RwLock<Library>>.
  render_frame_rgb checks library_refs FIRST for the frame's chunk.
  If a ref covers this frame, resolve_library_ref loads the canonical 
    mesh from the library, applies inverse alignment, rasterizes.
  Residual vertices in the LibraryRef are spliced into the canonical 
    mesh as additional points (currently as orphan vertices, not 
    triangulated; future enhancement).
```

## Where the time goes

Encode (v8, 2K 60fps):
- ~40% feature tracking (LK on N=300 tracks, hoisted in recent perf turn)
- ~20% mesh integration (depth → triangle assembly)
- ~15% serialization + zstd
- ~10% ffmpeg decode
- ~15% other (region activity, residual computation, scheduling)

Decode (v8, 2K 60fps):
- ~50% mesh rasterization (incremental edge-walk in recent perf turn)
- ~15% frame parse + zstd decompress
- ~5% pose interpolation
- ~30% source fallback when v8 fails for off-screen geometry (currently 
  blocks UI thread; queued fix is async worker)

## GPU acceleration

Optional; behind the `gpu` feature flag. Currently used for:
- Color conversion (production)
- Phase correlation pipeline (built but not yet wired)

Plans for further GPU work:
- Mesh rasterization on GPU (decoder)
- Feature tracking on GPU (encoder)
- See `V8_PLUS_ROADMAP.md`
