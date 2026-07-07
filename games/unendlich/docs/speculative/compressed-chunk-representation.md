# Speculative: compressed chunk representation for physics-aware upload culling

## What problem this is solving

User-flagged (paraphrased):

> The other big win we could do up front in a normal game engine is
> subsurface chunk culling, but here we need it in a compressed form
> for the physics... it is important in a physics-based engine to
> represent some chunks efficiently and be able to get them through
> upload queues in batches and work on the harder stuff locally on
> the GPU via a deterministic data expansion.

The constraint that distinguishes this from a normal-renderer
problem: chunks the player can't *see* still need to be
*queryable* for collision (a chunk 20m underground is invisible
but the player might fall into a pit and need to collide with
its blocks). So "cull" doesn't mean "drop from memory" — it means
"don't pay the full per-chunk upload cost; use a compressed form
that's still query-answering."

This dovetails with three architectural threads already in flight:

1. **GPU-resident world** (architecture-v1's destination):
   the world *lives* on the GPU; the CPU only sends edits.
   Compression is what makes that affordable.
2. **Stage 3 upload scheduler** (decoupling memo): once uploads
   move off the render thread, the per-upload bandwidth matters
   even more, because the upload thread can keep up with the
   meshing pool only if the per-upload payload is small.
3. **Procedural reconstitution** (already works, see
   `world::gpu_generator::VulkanChunkGenerator`): byte-identical
   chunks from `(coord, seed)`. The compression's strongest case
   is "I'm just (coord, seed) — regenerate me on the GPU."

## What already exists (significant infrastructure)

This is the encouraging part. The compression building blocks are
mostly written; what's missing is the routing layer.

- **`OctreeArray`** (`world::octree`): the canonical in-memory
  compressed form. Homogeneous 2³ subtrees collapse to a single
  leaf. Pure-air chunk = 1 node, ~32 bytes. Surface chunk =
  5K-10K nodes after collapse, ~200-400 KB. Already supports
  `sample`, `set_block`, `region_stable_since` — all the physics
  queries.
- **`Chunk::is_all_air`**: trivial uniform detector at the dense
  representation level (`chunk.rs:668`).
- **`Chunk::explicit_indices`** (`chunk.rs:94`): the sparse-edit
  pattern — a `BTreeSet<u16>` of cells that have diverged from
  the procedural seed. Pristine chunks have an empty set. This
  is THE compressed representation for "I'm a procedural chunk
  with N edits."
- **`world::cpu_generator::WorldGenerator`** and
  **`world::gpu_generator::VulkanChunkGenerator`**: deterministic
  `(coord, seed) → chunk`. The GPU version is the key for
  "upload (coord, seed) and let the GPU expand."
- **`world::surface::SurfaceRecord`** + **`extract_surface`**:
  pre-meshed sparse wire format. Used by the GPU mesher to skip
  CPU-side mesh storage entirely (Far/VeryFar virtual path).
- **`OctreeBlockSampler`** and `ChunkManager::octree_sampler()`:
  collision-query interface that works against the octree, not
  the dense `Chunk`. Physics is already partly compression-aware.

## What's missing — the routing layer

For each chunk in the world, decide the upload representation
based on its content shape:

| Chunk shape | Representation | Upload size | Physics query |
|---|---|---|---|
| All-air | 1 byte (BlockId) | 1 byte | Trivially "air" |
| All-stone (or any uniform solid) | 1 byte (BlockId) | 1 byte | Trivially the block |
| Procedural unedited | (coord, seed) | 16 bytes | Regen + sample, OR cache last-seen result |
| Procedural + few edits | (coord, seed) + sparse delta list | ~16 + 4·N bytes | Regen + apply deltas |
| Heavily edited / surface | Octree array | ~200-400 KB | Octree sample |
| Special (player-built structure) | Octree array | ~200-400 KB | Octree sample |

Decision logic at upload time:
```
if chunk.explicit_indices.is_empty():
    if chunk.octree.is_uniform():
        return Uniform(chunk.octree.root_block_id())
    else:
        return Procedural(chunk.coord, chunk.seed)
elif chunk.explicit_indices.len() < THRESHOLD:  # e.g., 64
    return ProceduralWithEdits(chunk.coord, chunk.seed, deltas)
else:
    return Octree(chunk.octree)
```

Subsurface culling falls out: a chunk that's "all stone, no
edits, surrounded by all-stone neighbors" gets the Uniform
representation, ships in 1 byte, and the GPU produces no mesh
(it has no visible faces). Physics still works (the GPU answers
"yes, solid stone at (x,y,z)" trivially).

## Why this is a "big win"

Concrete numbers using RD=6, ~925 loaded chunks:

| Scenario | Today | With compression |
|---|---|---|
| Boot a fresh world | 925 × full Chunk (~64 KB each) = ~60 MB CPU + meshing work + ~10 MB mesh uploads | ~30 MB CPU work for surface (1/3 of chunks), ~620 chunks × 1 byte uniform + ~80 chunks × ~16 byte procedural = ~2 KB |
| Walk into a cave system | Re-mesh exposed faces (real cost) | Same |
| Build a structure | New octree + mesh for that chunk | New octree for that chunk, neighbors still procedural |
| Look up at the sky | ~300 sky chunks loaded, each meshed (empty mesh, but worker still ran) | ~300 sky chunks loaded, each 1 byte, no worker invocation |

The "look up at the sky" case is the most striking. Each empty
mesh result still costs ~1ms of worker time (the greedy mesher
scans 32³ cells even if all are air). 300 chunks × 1ms = 300ms
of worker time per sky load. With Uniform detection, that drops
to 300 chunks × ~1µs (just an octree-is-uniform check).

The pure-stone (subsurface) case is similar in magnitude.

## Layering with the upload-scheduler arc

This work and the upload-scheduler (RC29-RC32 + Stage 3) are
**orthogonal but synergistic**:

- The scheduler decides *when* and *in what order* uploads happen.
- The compressed representation decides *how much data* each
  upload moves.

Together: Stage 3's off-thread upload pipeline handles N×
more uploads per frame, because the per-upload payload is 1 byte
for ~70% of chunks. The DEVICE_LOCAL backing matters less for
uniform chunks (the staging buffer holds the 1-byte payload for
microseconds; cache-coherency cost is negligible). The real win
of Stage 3 still applies to the surface chunks that ARE big.

## Sub-staging this work

Five independent sub-stages, each shippable.

### Sub-stage 0: Uniform-chunk skip in the mesh pipeline

**Smallest first step. Shippable as a small RC, but not as
trivial as it first appears.**

Add `OctreeArray::root_block() -> Option<BlockId>` (returns
`Some(b)` if uniform, `None` if internal). In the bin's submit
phase, before pushing a chunk to the mesh pool, check if its
octree is uniform-air. If yes, register the chunk as
"meshed-empty-at-version-V" and skip the worker pool.

**Subtle**: the existing `upload_prebuilt_mesh_at_lod` path for
empty meshes doesn't TRACK the version — it just removes any
slot. So a chunk that's been meshed-as-empty has
`renderer.mesh_version_for(coord) = None`, and the submit phase
keeps re-submitting it. To make sub-stage 0 actually save work,
the renderer needs an additional state: `empty_mesh_versions:
HashMap<MeshSlotKey, u64>` that tracks "this slot is meshed as
empty at version V". Then `mesh_version_for` returns Some(V) for
empty-meshed entries.

Saves: ~1ms × N_uniform_chunks_per_frame of worker time. N could
be ~300 for a player looking up at the sky.

Risk: low-medium. The version-tracking state needs cross-checking
with eviction, LOD transitions, and mesh-cache restore paths.

Effort: ~100-150 LoC (the original "50 LoC" estimate was wrong;
the renderer state work pushes it higher).

### Sub-stage 1: Upload representation enum + classifier

Add `pub enum UploadRepr { Uniform(BlockId), Procedural { coord,
seed }, ProceduralWithEdits(...), Dense(Vec<u8>), Octree(Arc<OctreeArray>) }`
and a `chunk.classify_for_upload() -> UploadRepr` that picks the
smallest applicable form.

This is just data shape work — no engine changes yet. Enables
sub-stages 2-4.

Effort: ~150 LoC + tests.

### Sub-stage 2: GPU-side procedural expansion

For `UploadRepr::Procedural`, the GPU receives `(coord, seed)`
and runs the existing `VulkanChunkGenerator` to expand to a
full chunk on the GPU. The expanded chunk lives in GPU memory
only; CPU never materializes it.

The existing `VulkanChunkGenerator` already produces byte-identical
output to `cpu_generator`. The wiring change is "compute shader
takes a list of (coord, seed) pairs, outputs N chunks worth of
block data to a GPU-side buffer."

Effort: ~250 LoC. Most of it is descriptor sets and shader
plumbing; the kernel itself exists.

**Depends on Stage 3 of the upload-scheduler arc**: this only
matters if uploads are off the render thread and use DEVICE_LOCAL
memory. Doing this before Stage 3 means the procedural expansion
happens but the result lives in HOST_COHERENT memory, defeating
the point.

### Sub-stage 3: GPU-side delta application

For `UploadRepr::ProceduralWithEdits`, GPU runs procedural expand,
then a tiny patch kernel that applies the delta list.

Effort: ~150 LoC. The delta list is just a `Vec<(local_index,
block_id)>`; the kernel iterates it.

### Sub-stage 4: Uniform-chunk physics path

Make physics queries against uniform chunks bypass the octree
entirely:
```rust
fn block_at(coord) -> BlockId {
    match self.chunks.get(&chunk_coord) {
        Some(ChunkRef::Uniform(b)) => *b,
        Some(ChunkRef::Octree(arr)) => arr.sample(local),
        None => BlockId::Air,
    }
}
```

Trades a DashMap lookup + branch for an octree descent. For
sky chunks that's ~10x faster.

Effort: ~100 LoC, lots of cross-checking with existing physics
code.

## The deterministic-expansion property the user emphasized

The user wrote:

> work on the harder stuff locally on the GPU via a deterministic
> data expansion

This is the unblocking property. Because `WorldGenerator` is
byte-identical for the same `(coord, seed)`, the CPU and GPU
agree on what an unedited chunk looks like. The CPU side stores
`(coord, seed)`; the GPU expands it; the result matches what the
CPU would have computed.

This is *already proven* in the codebase
(`docs/architecture-v1.md` notes this as a survives-the-rewrite
capability). What's missing is using it for upload culling.

The single subtle requirement: when the user edits a block, the
chunk *transitions* from `Procedural` to `ProceduralWithEdits`,
and the GPU's expanded copy must be patched accordingly. This is
exactly what sub-stage 3 (delta application) does.

## Open questions

1. **Where does the "seed" come from?** Currently the world has
   a single global seed. For multiplayer, do all peers share the
   seed? Probably yes (same world). The (coord, seed) pair is the
   canonical chunk identity.

2. **How big can `explicit_indices` get before we fall back to
   dense?** Threshold tuning. If a chunk has 1000 edits, the
   delta list is 4 KB — still smaller than the 32 KB octree
   serialization. Setting the threshold high (say, 5000) means
   most player-built structures stay in the delta form.

3. **Per-block instance state**: the engine has
   `BlockInstanceState` (decay, growth stages, etc.). This is
   per-block and isn't reproduced by the procedural generator.
   So a chunk with `instance_state.len() > 0` cannot be
   represented as pure `Procedural`. The delta list must include
   instance-state entries too.

4. **Plant indices / tree roots**: these are sparse side-tables
   alongside the block grid. They're regenerated by the cpu/gpu
   generator. So they don't need separate upload representation
   for `Procedural` chunks — the GPU expansion produces them.

5. **Eviction policy**: a chunk that's not in render distance is
   currently evicted from memory. Should it instead be replaced
   with its `UploadRepr` (saving the few bytes for "I exist as
   coord=X, seed=Y")? Probably yes, to support deterministic
   physics for falling-into-distant-chunks scenarios. But that
   touches the entire ChunkManager API.

## Recommendation

Ship **sub-stage 0 (uniform-chunk skip)** as a stepping-stone
RC in the next session. It's small, isolated, immediately
testable. The savings are real (skipped worker meshing for sky
and bedrock chunks).

Defer **sub-stages 1-4** until Stage 3 of the upload-scheduler
arc lands. Without Stage 3, the compressed representation lives
in CPU memory and gets memcpy'd into HOST_COHERENT memory anyway;
the compression doesn't save much. With Stage 3 + DEVICE_LOCAL,
the procedural expansion can happen GPU-side and the result lives
in GPU memory only.

The natural shipping order is therefore:
- RC33 (next): sub-stage 0 — uniform-chunk skip
- RC34: Stage 3 sub-stage A — DEVICE_LOCAL backing + staging ring
- RC35: Stage 3 sub-stage B — off-thread memcpy
- RC36: compressed-repr sub-stage 1 — UploadRepr enum + classifier
- RC37: compressed-repr sub-stage 2 — GPU procedural expansion
- RC38: compressed-repr sub-stage 3 — GPU delta application
- RC39: compressed-repr sub-stage 4 — uniform physics path

That's ~6 RCs of work; each independently testable; each builds
on the previous. The architecture-v1 destination ("GPU-resident
world plus procedural reconstitution upload") is reached at
roughly RC37.

## Cross-references

- `docs/architecture-v1.md` — strategic destination. This memo
  is the "how to get there" companion.
- `docs/architecture-v1-addendum.md` — refinements.
- `docs/speculative/upload-scheduler-design.md` — Stage 3 (the
  prerequisite for sub-stages 1-4 here).
- `docs/speculative/stage-3-implementation-guide.md` — exact
  next-session steps for the prerequisite.
- `crates/unendlich-engine/src/world/octree.rs` — the canonical
  in-memory compressed form (sub-stage 0 just needs an
  `is_uniform` predicate here).
- `crates/unendlich-engine/src/world/gpu_generator.rs` — the
  deterministic GPU-side regen. Already works.
- `crates/unendlich-engine/src/world/chunk.rs:94` —
  `explicit_indices` field (the existing sparse-edit pattern).
- `crates/unendlich-engine/src/world/chunk.rs:668` — `is_all_air`
  predicate.
