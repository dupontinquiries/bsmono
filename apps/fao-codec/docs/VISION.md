# FAO Vision

## What FAO is

FAO is a video codec where the output is a **streaming 3D scene** 
rather than a sequence of compressed pixels. Each chunk of input video 
gets analyzed for camera motion, feature trajectories, depth, and 
surface geometry. The result is a mesh (with vertex colors sampled 
from the source) plus a per-frame camera pose. To play back, the 
decoder rasterizes the mesh through the recorded pose for each frame.

The compressed form is not "I-frame + P-frame deltas" like H.264 or 
AV1. It is "scene mesh + camera trajectory + per-frame feature 
observations". A talking head with a static background only needs the 
background mesh ONCE (in the trailer); per-frame data shrinks to pose 
and motion tracks.

## What problems FAO is built to solve

### 1. Cross-video mesh sharing
A surveillance camera filming the same hallway 24/7 records the same 
geometry repeatedly. AV1 compresses each day's footage in isolation — 
the same hallway pixels get re-encoded every day. FAO's **library mode** 
lets a chunk in today's video reference a canonical mesh extracted from 
last week's video. The per-chunk overhead drops to a UUID + alignment 
transform (a few hundred bytes) instead of re-encoding all the geometry.

For repeating-scene workloads (security, sports venues, news studios, 
streaming game sessions of the same map), this compounds: every video 
in the library contributes to a shared mesh pool, and every new video 
references it. Asymptotically, each video stores only what's NEW.

### 2. Depth-aware playback
Because FAO encodes camera pose and 3D surface, decoders can:
- Adjust the view perspective slightly (re-render through a tilted 
  pose) for VR-lite passthrough
- Composite a 3D overlay onto the mesh
- Detect occluding objects and suppress UI that overlaps them
- Stabilize handheld footage post-hoc (re-render through a smoothed 
  pose track)

None of this is possible with a pixel-based codec.

### 3. Generative scene completion (longer-term)
Once the mesh exists, a decoder with a learned prior could PLAUSIBLY 
generate detail that wasn't captured — e.g., reconstruct the back side 
of an object the camera never saw, by referencing canonicals of similar 
objects in the library. This is speculative but tractable.

## Who FAO is for

- **Researchers** experimenting with hybrid 2D/3D video formats
- **Game devs** who need to capture playthroughs with consistent 
  reconstruction (e.g., automatic cinematics from gameplay)
- **Security & surveillance** operators with repetitive-scene footage
- **Streaming platforms** with shared assets across many videos (live 
  sports, news desks, esports)
- **Future**: AR/VR content producers, generative-completion pipelines

FAO is NOT for:
- General-purpose web video (use AV1/H.265)
- Mobile bandwidth-constrained playback (FAO's per-frame cost is 
  currently higher than AV1)
- Lossless archival (FAO is lossy by design)

## Where FAO is today

- v8 reconstruction codec works end-to-end (encode + library + decode)
- Per-vertex colors now sample real source frame data (just fixed)
- Currently 8x larger than AV1 on natural footage like horror movies
- Library mode shipped through Phase 3 (cross-encode sharing works)
- macOS + Linux runtime; Metal/Vulkan GPU acceleration optional

## Where FAO is going

See `V8_PLUS_ROADMAP.md`. Short version:
- **v8.1**: per-frame photometric residual (closes most of the AV1 gap)
- **v8.2**: texture maps for canonical webs (the big quality jump)
- **v8.3**: pose/feature delta encoding (50% per-frame savings)
- **v8.4**: mesh simplification (Garland-Heckbert quadric error)
- **v8.5**: spherical harmonics for lighting (enables relighting)

Plus library Phase 4-6 for production-quality library operations.

## Non-goals (explicit)

- We're not trying to beat AV1 on standard SDR 30fps natural video. 
  AV1 is the product of thousands of engineer-years; we won't catch 
  it on its home turf.
- We're not trying to be real-time on phones. Encoder needs serious 
  compute.
- We don't promise lossless reconstruction. Mesh-based representations 
  ARE lossy.
- Cross-platform decoder portability is a goal but not yet proven.
