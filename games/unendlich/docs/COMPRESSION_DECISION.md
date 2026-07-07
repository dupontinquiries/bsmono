# Storage / wire compression — decision (validated on real generated chunks)

**Reproduced first-party on the r70 tree** via `examples/forest_codec_bench` (real `WorldGenerator`
+ real `OctreeArray`) + an lzma/gzip/bz2 race on the dumped voxel bytes:

```
chunk            dense   octree    gzip    lzma     bz2    lzma vs octree
bare_surface     32.0K   265.0K   0.49K   0.55K   0.54K        481x
forest           32.0K   203.8K   0.89K   0.86K   0.96K        237x   (wood=77, leaf=777)
underground      32.0K   145.4K   0.57K   0.58K   0.58K        250x
sky              32.0K     0.0K   0.06K   0.13K   0.04K          -
```

## The finding
- The **octree is not a compressor.** For solid terrain it is 4–8× *larger* than the dense 32 KB
  voxel array (145–265 KB; forest 204 KB / 5,217 nodes). It earns its keep as the in-memory
  **query / LOD-sampling** structure, not as a storage or wire format.
- **lzma on the dense voxel bytes is already sub-KB** (0.5–0.9 KB/chunk; 237–481× smaller than the
  octree). Foliage is full of byte-level redundancy, so the case where a fancier codec might win
  (lzma being mediocre on forests) is simply false.
- For **generated** content the structural floor is the **procedural seed**: regenerating a chunk
  from (coord, seed) is ~16 bytes and bit-exact (the generator is deterministic).
- **Neural compression: closed.** The budget is already sub-KB; a learned codec would pay a forward
  pass + resident weights + a determinism tax to shave fractions of a KB. No win. (This closes only
  neural *compression* — neural *rendering*, e.g. froxel denoise / shading, is a separate axis.)

## Decision — what to ship
1. **Persisted save / upload / MP wire:** never the octree. Send, per chunk, in priority order:
   - **generated & unmodified → the seed** (coord + world seed): ~16 B, regenerate on the receiver.
   - **modified → lzma'd dense voxels** (or an lzma'd delta vs the regenerated base): ~0.5–0.9 KB.
2. **In memory / runtime:** keep `OctreeArray` exactly as-is for queries/LOD. It is built locally
   from the voxels after receive/regenerate — it never crosses the wire.
3. Use stdlib **lzma preset 9** (xz). gzip/bz2 are within noise; lzma is marginally best and ubiquitous.

## Where this lands
- **MP world-join / client streaming** (`sim::session_mode` → `run_world(Client)`): the snapshot/
  chunk-stream wire format is seed-or-lzma-voxels per the above — 250–480× less bandwidth than
  streaming octrees, and near-zero for generated terrain (just the seed). See MULTIPLAYER_WORLD_JOIN.md.
- **Save format:** same rule — seed for pristine chunks, lzma'd voxel deltas for edited ones.

Reproduce on the M2: `cargo run --release -p unendlich-engine --features gpu --example forest_codec_bench`
then race the `/tmp/ue_*.bin` dumps (ratios are platform-independent, so they will match).
