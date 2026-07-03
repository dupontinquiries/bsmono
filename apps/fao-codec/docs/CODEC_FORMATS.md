# Codec Formats

The `.fao` container holds a sequence of frames followed by zero or more 
trailers. Frames have a 1-byte codec marker; trailers have a 4-byte magic.

## Frame markers

| Byte | Codec | Description |
|------|-------|-------------|
| `T`  | v2.1  | Translation residual frame (most common in steady-state) |
| `K`  | v2.1  | Keyframe — full DCT-coded |
| `V`  | v3-v5 | Various intermediate codec attempts (mostly retired) |
| `Z`  | v7/v8 | Reconstruction frame (camera pose + features) |
| `P`  | v7    | Perception frame (objects + bounding boxes) |
| `B`  | -     | Boundary marker |

## v8 frame layouts

The v8 codec emits Z-marker frames. Three serialization variants exist:

### V8 (legacy 20-byte features) — `to_bytes_v8_legacy`
```
'Z' + 0x00     2 bytes magic
n_features     u32 LE
camera_pose    48 bytes (position 12B + quaternion 16B + focal 8B + 
                         principal 8B + distortion 4B)
features:
  feature_id   u32 LE
  screen_x     f32 LE
  screen_y     f32 LE
  depth        f32 LE
  confidence   f32 LE       (20 bytes/feature)
```
Used only by tests now.

### V81 (compact 11-byte features) — `to_bytes_v81`
Same header. Features use compact encoding:
```
feature_id     u32 LE
screen_x_u16   u16 LE (scaled 0..frame_width)
screen_y_u16   u16 LE
depth_f16      2 bytes (half precision, max ~65500)
confidence_u8  1 byte (0..255 quantization)   (11 bytes/feature)
```

### V8p (planar 11-byte) — `to_bytes_v8p` (CURRENT DEFAULT)
Same data as V81, but features are stored column-major (planar):
```
magic + n_features + pose (header same as V81)
plane[feature_id]:  n × 4 bytes
plane[screen_x]:    n × 2 bytes
plane[screen_y]:    n × 2 bytes
plane[depth]:       n × 2 bytes
plane[confidence]:  n × 1 bytes
```
Planar layout compresses much better with zstd because consecutive 
feature_ids, screen positions, and depths are highly correlated within 
a plane but uncorrelated across planes.

## v8 frame outer wrapping

After to_bytes, the frame is optionally zstd-wrapped:
```
if raw_bytes.len() >= 600:
    compress at level 3
    if compressed.len() + 6 < raw_bytes.len():
        emit:  'Z' + version + u32_LE(raw_len) + compressed_bytes
    else:
        emit raw_bytes (no wrap)
else:
    emit raw_bytes (too small to benefit)
```

The `'Z'` outer marker is the SAME marker as inside; the wrap is 
detected by checking the version byte (which differs from V81/V8p 
inner versions).

## v8 trailers

### V8MX — Mesh eXchange trailer
Holds the chunk-aligned web list. Each web carries a SceneMesh + 
alignment transform + WebSignature.

```
"V8MX"          4 bytes magic
version         u8
n_webs          u32 LE
for each web:
  chunk_id            u32
  chunk_frame_start   u32
  chunk_frame_end     u32
  kind                u8   (0=aligned, 1=canonical, 2=standalone)
  canonical_id        u32  (if kind==0)
  transform           [f32; 16]  (64B, row-major 4×4)
  signature           88 bytes (extents, surface stats, descriptors)
  mesh_compressed_len u32 LE
  mesh_compressed     [u8]      (zstd of SceneMesh)
```

### V8LR — Library Reference trailer
Replaces per-chunk meshes when a chunk matches a library canonical.
The actual mesh lives in the external library; the trailer just 
points to it.

```
"V8LR"          4 bytes magic
version         u8
n_refs          u32 LE
for each ref:
  chunk_frame_start    u32 LE
  chunk_frame_end      u32 LE
  uuid_len             u8
  uuid_bytes           [u8; uuid_len]   (canonical 32 hex chars)
  alignment_transform  [f32; 16]        (64 bytes)
  similarity_score     f32              (0..1, diagnostic)
  residual_n_verts     u32 LE
  residual_compressed_len u32 LE
  residual_compressed  [u8; len]        (zstd of vertices+colors)
```

A consumer .fao file with V8LR refs is only fully decodable if the 
library is attached at decode time. Without the library, those chunks 
render as gray frames (gracefully degraded).

## v2 (reference codec)

### K-frame ('K' marker)
Full DCT-coded keyframe. Used at chunk start or periodic seek points.

```
'K'              1 byte marker
chunk_header     varint dimensions / dequant params
y_blocks         RLE-coded 8×8 DCT coefficients (Y plane)
u_blocks         (U plane, 4:2:0)
v_blocks         (V plane, 4:2:0)
```

### T-frame ('T' marker)
Translation + residual. Encodes (dx, dy) per block plus optional 
residual.

```
'T'                            1 byte marker
n_blocks                       varint
translations                   packed (dx, dy) pairs (golomb-coded)
residual                       optional, RLE 8×8 DCT
```

T-frames depend on the previous K-frame for prediction.

## Frame index

Each `.fao` file has an index of `FrameTag`s at fixed-size 16-byte 
entries. Allows random-access seek.

```
struct FrameTag {
  payload_offset_in_file: u64,
  payload_len: u32,
  ftype: u8,    // 0=K, 1=T, 2=Z, etc.
  tier: u8,     // 0=skip, 1=passthrough, 2=full, 3=anchor
  reserved: u16,
}
```

The tier byte distinguishes:
- **skip**: zero-payload frames (source was a duplicate; decoder shows 
  previous)
- **passthrough**: payload IS the original source frame bytes (codec 
  decided not to re-encode)
- **full**: codec-encoded payload
- **anchor**: scene-cut or seek anchor; K-frame guaranteed

## Looking ahead

The `V8_PLUS_ROADMAP.md` proposes:

- **V8r** (v8.1) — adds a per-frame photometric residual track (¼ res 
  RGB delta after mesh render).
- **V8t** (v8.2) — texture maps in V8MX; vertices store UV instead of 
  RGB. New per-web `texture_zstd_payload` field.
- **V8d** (v8.3) — pose & feature delta encoding. Per-frame `delta_flag` 
  switches between absolute and delta serialization.

Format versioning is handled by the inner version byte; old decoders 
won't try to read formats they don't understand, they'll fall through 
to the source-frame fallback.
