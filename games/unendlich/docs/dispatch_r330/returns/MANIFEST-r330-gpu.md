# MANIFEST — worker r330-gpu (foliage wind-sway tag, §4b)

## Files touched
NEW   crates/unendlich-engine/src/sim/foliage_sway.rs      (pure core, self-contained)
EDIT  crates/unendlich-engine/src/sim/mod.rs               (+1: `pub mod foliage_sway;`)
EDIT  crates/unendlich-engine/src/render/mesher.rs         (+ `// r330-gpu` adapter block)
EDIT  crates/unendlich-engine/src/shaders/chunk.vert       (sway gated on 2-bit tag, not color)
REGEN assets/shaders/chunk.vert.spv                        (glslang -V --target-env vulkan1.2)
REGEN assets/shaders/chunk.vert.spv.hash                   (transitive_source_hash of new source)
NEW   HANDOFF_gpu_foliage_sway.md                          (on-device wiring proposal)
NEW   CHANGES-r330-gpu.patch                               (applies -p1; the 3 EDITs)
NEW   foliage_sway_proof.png                               (CPU replica of gate math, old vs new)

## API added (sim::foliage_sway — pure, crate-free)
SwayTag { None=0, Sway=1, Anchor=2 }  + bits()/from_bits()/is_eligible()
block_sway_eligible(is_leaf, is_plant, face) -> bool      // the bug fix
vertex_tag(eligible, is_base_vertex) -> SwayTag           // base anchoring
pack_tag_into_pos(pos_packed, tag) -> u32 / unpack_tag(pos_packed) -> SwayTag
tag_unorm(tag) -> f32 / tag_from_unorm(w) -> SwayTag       // A2 unorm channel
Wind { dir, mag, time } + sway_displacement(tag, world_pos, &Wind) -> [f32;3]
consts FACE_TOP..NORTH, POS_ALPHA_SHIFT=30, POS_ALPHA_MASK=0b11

## API added (render::mesher — `// r330-gpu` adapter, BlockId->core)
sway_flag(block: BlockId, face: usize, is_plant: bool) -> u8            // dispatch signature
vertex_sway_tag(block, face, is_plant, is_base_vertex) -> SwayTag
tag_packed_pos(pos_packed: u32, tag: SwayTag) -> u32
pub use crate::sim::foliage_sway::SwayTag;

## Vertex-format use
2-bit tag rides reserved alpha of PackedVertex::pos_packed (A2B10G10R10, bits 30..32).
Position bits 0..30 untouched. Color-alpha byte NOT used (reserved for tile-id / pack_color_id).
No struct layout change; 24-byte unpacked Vertex/GpuVertex unchanged.

## Verify commands run (+ results)
1) Pure core, standalone floor (shipped file):
   rustc --edition 2021 --test foliage_sway.rs  ->  15 passed / 0 failed, compile-clean
2) Shader compile (build.rs flags):
   glslangValidator -V --target-env vulkan1.2 chunk.vert  ->  OK (5356-byte SPIR-V)
3) SPIR-V validation:
   spirv-val chunk.vert.spv (committed)  ->  OK ; also --target-env vulkan1.2 OK
4) Full shader set:
   bash tools/validate_shaders.sh  ->  shaders OK: 35   compile-fail: 0   spirv-val-fail: 0
5) Hash regen via engine's exact transitive_source_hash (harness include!s
   shader_hash_shared.rs) == sha256sum cross-check:
   a6725e5bcb507ba196a2d3a1a8a20df252180f3484e68c68811f8e119bdfac93  (committed .hash matches)
6) Patch dry-run: patch -p1 --dry-run < CHANGES-r330-gpu.patch  ->  clean on all 3 files

## Toolchain (FS had reset; this round's uploads had no rust bundle)
Rebuilt from the allow-listed apt pool without root (dpkg-deb -x into a local prefix):
rustc 1.75.0, glslangValidator 11:15.1.0, spirv-val v2025.1.
NOTE: 1.75, not the SHARED_RULES 1.96 — acceptable for a dependency-free standalone
sim:: module on edition 2021 (no post-1.75 std APIs). Flagged for the coordinator.

## NOT done in-sandbox (coordinator on-device; see HANDOFF)
- Mesher per-vertex tag EMISSION + quantize-step tagging (Option A: ChunkMesh.sway_tags
  + tag_packed_pos at chunk_renderer.rs:427 / world_renderer.rs:1787). Touches shared
  emit loops + GPU readback parity, so proposed not wired (SHARED_RULES §1/§4).
- GPU-mesher/comp tag parity (chunk_mesh.comp). chunk.frag & froxel_fill.comp untouched (off-limits).
- Full `cargo build` (cold link overruns a turn; standalone gate is the documented substitute).

## Merge seam
chunk.vert sways ONLY tagged vertices; until emission is wired, foliage is quiet
(no bug regression). Merge the shader + mesher tagging together. gpu gates last per packet.

## Pre-existing failures untouched (per SHARED_RULES)
loot one_destroyed_eye_halves_the_amber, mob losing_both_eyes_blinds_it,
smoke_gpu pure_decay_monotonic_decrease.
