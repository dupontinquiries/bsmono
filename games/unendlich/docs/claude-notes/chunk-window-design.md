# The chunk, rethought — memory structure for a window that moves (2026-09-02)

"The stutters mostly come when loading chunks." Below is what one chunk
costs to bring in and to throw out today, read from `world/chunk.rs`,
`world/chunk_manager.rs`, `render/octree_buffer.rs`, `render/mesh_buffer_pool.rs`,
and the memory shape that makes most of that cost disappear. Companion to
`gpu-cpu-residency-audit.md` (which is about bytes crossing the bus; this
is about bytes moving at all).

## What a loaded chunk is today

Per chunk, on the CPU:

| thing | where | size |
|---|---|---|
| `Chunk { blocks: Vec<BlockId>, explicit_indices: BTreeSet<u16>, plant_indices: BTreeSet<u16>, … }` | `DashMap<ChunkCoord, Arc<Chunk>>` | 32 KB + two trees + an `Arc` |
| `OctreeArray` (40 B nodes, 8–11 K typical, 40 960 cap) | a second `DashMap<ChunkCoord, Arc<OctreeArray>>` | **320–440 KB typical, 1.6 MB cap** |
| `ChunkFields` grid | `state.chunk_grids` (a third map) | per field × cells |
| mesh request snapshot | `Arc<Chunk>` clone | refcount only — fine |

Per chunk, on the GPU: a span in the octree pool (sub-allocated,
256 MB default → ~600 typical chunks before the OOM fallback fires), a
vertex span and an index span in the mesh pool (free-list sub-allocator;
"largest free span" in the warn log is the fragmentation it reports), an
indirect-metadata slot.

Note the ratio: **the octree is ~10× the dense chunk.** 32 768 cells at
1 byte is 32 KB; the octree of the same chunk is 8–11 K nodes × 40 B. The
octree exists so the raymarch probe can skip empty space, and it is what
the GPU mesher walks — but as the thing that gets *uploaded per chunk* it
is the worst of the three shapes available (dense, dense+bitmask,
octree), and the pool that holds it is the first thing to run out.

## What moving the window costs today

`ChunkManager::update` (line ~900) on the main thread, every frame:

1. **Drain completed loads**: per chunk, load edit records **from disk**
   (`storage::load_chunk_edits`), allocate a `ChunkFields` grid and
   insert it, rebuild the octree on the main thread if the chunk is
   dirty (`OctreeArray::from_chunk`, a 32³ walk), then two `DashMap`
   inserts + two `Arc` allocations.
2. **On a chunk-boundary crossing**: `evict_distant` — an O(loaded) scan
   over the `DashMap` collecting candidates, a sort, a `retain`, and per
   evicted chunk: octree map remove, field grid remove, three GPU pool
   frees (octree span, vertex span, index span) into free lists, an
   indirect slot free.
3. **`request_visible`**: a full shell walk `0..=rd` — O(rd³) coordinate
   visits (7 238 at rd 12, 33 401 at rd 20) with a loaded/wanted check
   each — and `should_rewalk` runs it again **whenever any chunk
   completes**, so during streaming it can run every frame.
4. Then, downstream, per arrived chunk: octree memcpy into the pool
   (320–440 KB), mesh pack + memcpy (audit doc), and on M2 the
   HOST_COHERENT write burst itself is a stall.

None of the bytes in (1)–(4) are required by the fact that the player
moved one chunk. They are required by the *containers*: hash maps keyed
by coordinate, free-list allocators, and a desired-set that is recomputed
instead of shifted.

## The structure: a toroidal window of fixed slots

The loaded world is a fixed box of `N × M × N` chunk slots centred on the
player's chunk. Chunk `(x, y, z)` lives at slot
`(x mod N, y mod M, z mod N)` — **it never moves**. When the player
crosses from chunk column `x` to `x+1`, the plane of chunks at `x − r`
leaves and the plane at `x + r + 1` enters, and because
`(x − r) ≡ (x + r + 1) (mod N)` when `N = 2r + 1`, every chunk that
enters lands in exactly the slot of the chunk that left. Nothing is
reallocated, nothing is rehashed, nothing is freed. (This is the clipmap /
toroidal addressing that terrain renderers have used since GPU Gems 2;
the octree pool's own pin ledger is already keyed by span, so it is a
smaller step than it sounds.)

What that buys, item by item against the list above:

- **Lookup** is index arithmetic plus a coordinate check (the slot holds
  the coord it currently represents). No `DashMap`.
- **The desired set is implicit** — it is the box. On a crossing the
  work is one plane: `N × M` slots, sorted by distance to the player,
  handed to the loader. Chunk completions never trigger a walk.
  `request_visible` and `evict_distant` become the same O(N²) step.
- **Every per-chunk thing lives at the same index in parallel arrays**:
  `blocks[slot]`, `masks[slot]`, `fields[slot]`, `octree_region[slot]`,
  `mesh_region[slot]`, `indirect_meta[slot]`. A chunk arriving is a write
  into a region that already exists; a chunk leaving is a generation bump.
  Results still in flight for the old occupant (a mesh, a generation) are
  dropped by generation mismatch — the `epoch`/`version` mechanism the
  pool already has, made structural.
- **GPU residency is a slot, not a span.** The octree pool and the mesh
  pool stop being sub-allocators. Uploads go to fixed offsets; the
  indirect metadata table *is* the slot table.

The cost is fixed capacity per slot on the GPU, and that is where the
chunk representation has to change too, because at
`N = 25, M = 9` the window is 5 625 slots (rd 12 sphere ≈ 7 238) and a
1.6 MB octree cap × 5 625 is 9 GB. Two things fix that:

1. **Most slots are uniform.** In a 25 × 9 × 25 box around a surface
   player, the chunks that are neither all-air nor all-solid are the one
   or two layers the surface passes through: ~600–1 300 of 5 625. A
   uniform slot stores one block id and a flag; it allocates nothing,
   uploads nothing, meshes nothing, and its neighbours treat its faces as
   one value. (Today `skip_air_chunks` skips the mesh; the `Chunk` still
   holds 32 KB and two `BTreeSet`s.)
2. **The GPU representation is dense + bitmask, not the octree.** Per
   non-uniform slot: 32 KB of block ids + 12 KB of occupancy bitmasks
   (three axis-major sets of 1 024 × u32 columns). 44 KB × 1 300 = 57 MB
   for the whole window's terrain, in fixed slots. Compare 256 MB of
   octree pool for ~600 chunks today. The raymarch probe, if it stays,
   builds its octree on the GPU from the dense slot (a bottom-up merge is
   a few dispatches) — the octree is then a *derived* GPU-resident view,
   never uploaded.

Meshes are the one thing that is not fixed-size per slot. Sizes are long-
tailed (empty → 30 K vertices). Options, in order of preference:

- **Page allocator, not span allocator.** Vertex and index arenas in
  fixed pages (e.g. 4 K vertices = 32 KB); a slot owns a short page list;
  alloc/free is a bitmap, O(1), no fragmentation, no "largest free span".
  The indirect draw stays one command per slot if pages are contiguous
  per slot — which a *buddy* page allocator gives cheaply (power-of-two
  runs), at ≤2× overallocation. This is the pragmatic answer.
- Two size classes (small/large) with fixed pools; overflow → the next
  coarser LOD. Simpler, wastes more.

## The chunk itself: columns of bits

The bitmask is not just a GPU upload format; it is what makes meshing
cheap enough that the thread pool stops mattering. With occupancy as one
`u32` per `(x, z)` column (bit `y` set = solid), the faces exposed in +Y
for the whole column are `solid & !(solid >> 1)` — 32 cells per
instruction — and −Y is `solid & !(solid << 1)`; the other four
directions are the same on the two transposed mask sets. Greedy merging
runs on the resulting face masks per slice with bit tricks
(trailing-zeros, mask-and-extend). This is the "binary greedy meshing"
family. **Measured here** (`bitchunk.rs`, one 2.1 GHz Xeon core, this
sandbox, `-O`): a terrain heightfield chunk — 15 974 exposed faces,
block-id key changing every 4 layers, 5 308 quads out — greedy-merges all
six faces in **72 µs**; a p=1/3 noise chunk (44 578 faces, 31 184 quads,
the pathological case) in 510 µs; building the three mask sets from a
predicate is 49 µs. The current CPU mesher is 18 ms/chunk/worker on the
M2 Ultra. Two-plus orders of magnitude, before SIMD, before the slice
transpose is hoisted. On a
phone core that is 10 000 chunks/s; on the GPU it is one thread per
column, no octree walk, no divergence — the Honeykrisp 8–15 ms/chunk
would not survive the rewrite either.

What the mesh loses in the first cut: per-face colour variation from the
climate/tint tables happens at emit from the block id at the quad's
origin (as now); greedy runs are broken on block-id change (as now); AO
and foliage sway are per-vertex tags computed from the masks (cheaper than
now — AO is three bit tests per vertex). Water/lava/decoration keep their
own passes.

## Order, and what is built

1. `world/chunk_window.rs` — the toroidal window as a pure, headless
   core: `coord ↔ slot`, generation counters, and `recentre(new_player)`
   returning exactly the slots that entered and left. No GPU, no maps.
   **Built and tested this session** (standalone `rustc --test`, 11 tests).
2. `world/bitchunk.rs` — the column-bitmask representation: build the
   three axis-major mask sets from a dense 32³ occupancy, compute the six
   face masks with neighbour columns, count exposed faces, and greedy-
   merge one slice into quads. **Built and tested this session**; the
   parity test checks face counts against a brute-force 6-neighbour scan
   over random chunks, and greedy area against the face count.
3. Slot-indexed `ChunkStore` replacing the two `DashMap`s behind the
   existing `ChunkManager` API (`get`, `install_chunk`, `evict_distant`,
   `request_visible`), uniform-slot elision. The `WorldHost` seam
   (`games/shared/worldhost`) is where blackstar asks its questions, so
   this does not touch the game.
4. Mesher on `bitchunk` (CPU first; the GPU shader is the same masks with
   one invocation per column). Parity against `mesh_chunk_greedy` on the
   face count and the covered area, not on vertex order.
5. Page allocator for the mesh pool; slot regions for the dense/bitmask
   upload; octree derived on the GPU or retired.
6. Then the residency audit's step B (mesh never leaves the GPU) is a
   `vkCmdCopyBuffer` between two slot tables.

Steps 1–2 have no hardware dependency and no build dependency on the
engine, which is why they were done first. Step 3 is a day of careful
API-preserving surgery; 4 is where the numbers change.
