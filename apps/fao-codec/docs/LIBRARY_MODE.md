# Library Mode

A FAO library is a persistent, append-only store of canonical webs 
shared across all videos encoded against it. When a new video is 
encoded with library mode, its chunks query the library; chunks that 
match an existing canonical replace their mesh with a tiny reference 
(uuid + alignment transform).

## Why this exists

A surveillance camera filming the same hallway 24/7 records the same 
geometry every day. AV1 compresses each day's footage in isolation. 
FAO with library mode references yesterday's mesh, paying only 
~100 bytes per chunk instead of ~100KB.

The win compounds: every new video contributes new canonicals to the 
library AND references old ones. After enough videos, the library 
becomes a near-complete model of the recurring scenes, and per-video 
overhead shrinks to motion + newly-visible geometry.

Plausible workloads:
- Security / surveillance (same physical space, many recordings)
- Sports broadcasts (same field/arena, many games)
- Esports / streaming (same game map, many sessions)
- News (same studio, daily program)
- Synthetic / generative video (same world, many cameras)

## Storage layout

```
~/.fao_library/
├── manifest.json         Web catalog
├── lock                  Sentinel file (writable handles only)
└── webs/<uuid>/
    ├── mesh.bin          zstd-compressed SceneMesh
    └── pose.bin          Compact pose track
```

Future additions per-web:
- `priors.bin` — depth histogram + occlusion mask + lighting baseline
- `texture.bin` — UV-mapped texture image (v8.2)

### Manifest format
```json
{
  "version": 1,
  "web_count": <u64>,
  "webs": [
    {
      "uuid": "32-hex-chars",
      "source_video": "/path/to/source.mkv",
      "source_chunk_start": 1080,
      "source_chunk_end": 1439,
      "vertex_count": 42351,
      "triangle_count": 84122,
      "signature": "<88 bytes hex>",
      "ref_count": 17,
      "created_at": 1715387654,
      "last_referenced": 1715820000
    },
    ...
  ]
}
```

The manifest is loaded fully into RAM at library open (~120B per web × 
100k webs = ~12MB). Mesh files stay on disk until matched and faulted.

## Matching

`Library::find_match(signature, threshold) -> Option<(stub, score)>`

Currently a linear scan with early-out on first match above threshold. 
WebSignature similarity returns f32 in [0, 1]; default match threshold 
is 0.85.

Signature is 88 bytes including:
- Bounding box extents (6 floats: min + max XYZ)
- Vertex count percentile bins (8 floats)
- Color histogram (16 bins × 3 channels)
- Mean normal direction
- Compact triangle aspect-ratio histogram

The signature is designed to be:
- Cheap to compute (one mesh pass)
- Insensitive to rigid transform (so a re-shot scene from a slightly 
  different angle still matches)
- Sensitive to topology changes (different scene shouldn't match)

For libraries above ~10k webs, the linear scan becomes the bottleneck. 
Filed as Phase 5: 3D spatial grid + extent-max sort for O(log N + K) 
lookup.

## Phase plan

### Phase 1 (✅ shipped)
- Disk spillover for chunk_store (caps RAM at policy budget)
- WebSignature::matches as a standalone fn
- Library::init / open / list

### Phase 2 (✅ shipped)
- CLI `fao-cli library {init|list|info|verify}`
- Library handle persisted across encode runs (read-only)
- WebSignature recomputation utility (`verify`)

### Phase 3 (✅ shipped)
- V8Encoder library integration at finalize() time:
  - Chunk close → compute signature → library.find_match
  - On match: drop mesh, write LibraryRef into chunk_store
  - On no match (writable lib): library.add_web → uuid issued
- V8LR trailer format + write/read
- V8Decoder.attach_library + resolve_library_ref
- V8Decoder.render_frame_rgb checks library_refs FIRST for each frame 
  (just shipped this session — was the missing piece)
- App-level GUI for Initialize / Attach / Detach / Writable toggle
- Env-var bootstrap: `FAO_LIBRARY` and `FAO_LIBRARY_RW`

### Phase 4 (pending)
- Per-canonical priors (depth histogram, occlusion mask, lighting 
  baseline) stored alongside mesh
- Pre-warmed singleton wgpu::Device + ComputePipelines
- Encoder consumes priors as regularizers during densification

### Phase 5 (pending)
- Spatial index: 3D grid keyed by signature centroid + extent_max sort 
  for range queries
- Bumps practical library size from ~10k to ~1M webs

### Phase 6 (pending)
- Garbage collection daemon (drop webs with ref_count == 0 after grace 
  period)
- Import / export commands (rsync-friendly bundles)
- Cross-machine replication
- SQLite migration (when manifest.json gets too big)

## Operational details

### Writable lock
Library::open_writable creates a sentinel `lock` file. Best-effort 
single-writer model; we don't use OS file locks. Concurrent writers 
must coordinate externally (e.g., the user only runs one encode at a 
time with FAO_LIBRARY_RW=1).

Read-only Library::open does not take the lock. Many readers + one 
writer is the expected pattern.

### Reference counting
Each web has a ref_count incremented when a new video references it. 
Used to drive garbage collection (Phase 6) — webs with ref_count=0 for 
N days are candidates for removal.

Not currently decremented when a referring .fao file is deleted. The 
honest model is "ref_count is an upper bound; GC sweeps look at all 
known .fao files and recompute". Filed.

### Two-way benefit at finalize
When a video is encoded against a writable library:
1. Chunks that MATCH library canonicals → uuid reference (saves video 
   space)
2. Chunks that DON'T match → INSERT into library (helps future videos 
   match against this one)

Net: the library grows with every video, and every video pays less 
overhead than its predecessors.

## Honest assessment

The library scaffold works. The match quality determines whether 
real-world compression actually improves; on test scenes it's working 
well for surveillance-style content (same room, different days) and 
poorly for natural video (a horror movie cuts to new scenes constantly, 
so library matches are rare).

For library mode to be a production feature, we need:
- Phase 4 priors (helps encode time on matches)
- Phase 5 spatial index (otherwise find_match dominates encode time)
- Phase 6 GC + sharing (otherwise the library bloats forever)
- Better signature design (current 88B descriptor is workable but 
  produces false matches on similar-looking scenes; want a learned 
  embedding longer-term)
