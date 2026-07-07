# TEXTURE PIPELINE AUDIT — why block textures don't show (r346)

## Verdict
The block-texture atlas is **CPU scaffolding that was never connected to the GPU**. Blocks render
with per-vertex RGB color + procedural fbm grain in `chunk.frag` — there is no bitmap-texture path
in the renderer at all. This is an **unfinished feature**, not a regression: nothing to "fix", four
stages to wire. The r339 foliage entries (and all of `block_texture`) are correct but unreachable.

## What already EXISTS (CPU side — ready to use)
- `texture_atlas::block_texture(BlockId) -> Option<BlockTexture>` (+ `_ext`, `_full`) — per-block
  face tiles + tint. Correct, tested.
- `BLOCK_ATLAS: AtlasLayout(16,4,3)` + `tile_uv(idx)` — tile→UV math.
- `block_texture_table() -> Vec<BlockTexGpu>` and `block_texture_table_bytes() -> Vec<u8>` —
  the per-block table (tiles[top,bottom,side,textured-flag] + tint) flattened to 32-byte entries,
  **upload-ready for an SSBO/UBO**. Currently called by NOTHING.
- The vertex seam: `PackedVertex::pack_color_id(color, id)` rides the block id in the alpha byte;
  `chunk.vert` is documented to recover it via `in_color_unorm.a * 255.0`.

## What is MISSING (every GPU stage) — in dependency order
1. **Atlas image upload + sampler.** `assets/textures/kpc_atlas.png` is referenced in comments but
   never loaded. No code creates a Vulkan image/sampler for it. (The only image upload in render is
   egui's font atlas.)
2. **Block-tex table upload.** `block_texture_table_bytes()` is never bound. `BlockTexGpu` is used
   only inside `texture_atlas.rs`.
3. **Descriptor bindings.** `ChunkPipeline` descriptor layout = binding 0 UBO, 1 shadow `sampler2DShadow`,
   2 `sampler3D froxel_volume`. **No atlas sampler, no table buffer.** (render/chunk_renderer.rs ~l.196–245.)
4. **Mesher emit.** The active mesher is the CPU `mesher::mesh_chunk`; `PackedVertex::from_world`
   (render/chunk_renderer.rs l.427) calls `pack_color(color)` which hardcodes id `0xFF`. The mesher
   never calls `block_texture`. `Vertex` has no `block` field, so the id isn't threaded through yet.
5. **Shader sampling.** `chunk.frag` has no atlas sampler and computes albedo procedurally; its own
   comments say texturing waits "once the texture atlas lands" / "when the overlay system exposes a
   material channel/sampler". `chunk.vert` forwards color but not a tile/block id to frag.

## Wiring plan (dependency order; reuse the existing scaffolding)
**These are GPU/ash/shader changes — they cannot be built or tested in the coordinator sandbox
(no GPU; Vulkan/ash code isn't gateable here; a wrong binding index = black or crashed chunk pass).
They are on-device (M2) work. Specs below are precise so they can be wired + tested there.**

- **S1 — Vertex carries the block id (pure CPU, gateable; SAFE prerequisite).**
  Add `block: u8` to `mesher::Vertex`; in `mesh_chunk`'s face-emit set it to the source `BlockId as u8`
  (it already knows the block per face). `PackedVertex::from_world` packs `pack_color_id(v.color, v.block)`
  instead of `pack_color`. Blocks with no `block_texture` entry → pass `0xFF` (untextured sentinel).
  Inert until S5 reads alpha — matches the existing seam comment. This is the only stage I can land +
  gate; flagged here rather than done blind because it's only meaningful WITH S2–S5 and changes the
  hot mesh path (best landed together with the shader on-device).
- **S2 — Upload the atlas image.** Load `assets/textures/kpc_atlas.png` → Vulkan image + a NEAREST
  sampler (crisp tiles, no bleed). Bind at `set 0, binding 3` of the chunk pipeline.
- **S3 — Upload the table.** `block_texture_table_bytes()` → SSBO; bind at `set 0, binding 4`.
- **S4 — Descriptor layout.** Extend `ChunkPipeline` DSL + `descriptor_pool` + `descriptor_sets`
  to add bindings 3 (combined image sampler) and 4 (storage buffer). render/chunk_renderer.rs ~196–245.
- **S5 — Shader.** `chunk.vert`: `out float v_block = in_color_unorm.a;` (the packed id). `chunk.frag`:
  `id = round(v_block * 255.0)`; if `id == 255` keep the current procedural albedo (untextured);
  else read `table[id]`, pick the tile by face normal (`n.y>0.5`→tiles[0] top, `n.y<-0.5`→tiles[1] bottom,
  else tiles[2] side), build atlas UV from `tile_uv` + the face's local UV, `texture(block_atlas, uv).rgb
  * tint`, use as albedo into the existing lighting. (chunk.frag/.vert are coordinator-owned; editing is
  allowed but must be validated with glslangValidator + tested on-device.)

## Notes
- Design is **table-driven** (emit block id, shader indexes `block_texture_table`), which matches the
  existing `BlockTexGpu`/`block_texture_table_bytes` scaffolding and keeps per-vertex data to 1 byte.
- The atlas PNG must actually contain the 12 `BLOCK_ATLAS` tiles in 4×3 @16px. Confirm the asset
  exists/populated; the foliage tiles currently borrow `T_DIRT` tinted (see HANDOFF_textures_foliage.md).
- Recommend wiring S1→S5 in ONE on-device session (they're interdependent) and bisecting with a debug
  toggle (force `id=0xFF`) to isolate regressions.
