# HANDOFF — r330-gpu foliage wind-sway tag (§4b)

**WID:** `r330-gpu`  ·  **Status:** pure core + shader VERIFIED in-sandbox.
**Coordinator action:** wire the per-vertex tag emission (below) and gate on the
FP32 GPU path (`GPU_MESHER=2`). **Merge the shader and the mesher-tag wiring
together** — see "Merge seam" — they are correct individually but only produce
visible sway jointly.

---

## What landed (verified, no GPU needed)

| File | Change | Gate |
|---|---|---|
| `crates/unendlich-engine/src/sim/foliage_sway.rs` | **new** pure core: eligibility, per-vertex tag, pos-alpha pack/unpack, shader-gate replica | `rustc --edition 2021 --test foliage_sway.rs` → **15/15** |
| `crates/unendlich-engine/src/sim/mod.rs` | `+1` line: `pub mod foliage_sway;` | (declaration) |
| `crates/unendlich-engine/src/render/mesher.rs` | `// r330-gpu` adapter: `sway_flag` / `vertex_sway_tag` / `tag_packed_pos` mapping `BlockId`→core | gated on merge build |
| `crates/unendlich-engine/src/shaders/chunk.vert` | sway gated on 2-bit tag (`in_pos_unorm.w`), not green color | `glslangValidator --target-env vulkan1.2 -V` + `spirv-val` OK |
| `assets/shaders/chunk.vert.spv` (+`.hash`) | regenerated; hash via engine's `transitive_source_hash` | freshness test passes by construction |

## The mechanism

The tag rides the **2 reserved alpha bits** of `PackedVertex::pos_packed`
(`A2B10G10R10_UNORM_PACK32`, bits 30..32 — the slot the field doc already
earmarks as a "per-vertex flag"). I deliberately did **not** touch the color
alpha byte: `PackedVertex::pack_color_id` already reserves it for the future
per-block tile id (`world::texture_mapping`).

```
tag 0  None    solid geometry incl. green grass-block TOP   -> no sway   (the bug)
tag 1  Sway    leaf-cube vertex / plant tip                 -> full sway
tag 2  Anchor  planted plant base                           -> held still
tag 3  reserved
```

`chunk.vert` decodes `uint(in_pos_unorm.w * 3.0 + 0.5)` and sways only on `1`.
`sim::foliage_sway::sway_displacement` is a byte-for-byte Rust port of that gate,
so the behaviour is unit-tested (grass top & anchored base → `[0,0,0]`; tip moves,
bounded; deterministic; div-0-safe).

---

## Wiring to apply on-device (propose, not wired — SHARED_RULES §1/§4)

The unpacked `Vertex` / `GpuVertex` is `{ position:[f32;3], color:[f32;3] }`
(24 B, asserted by `gpu_mesher_vk::tests::vertex_layout_matches_cpu_mesher`).
It has **no flag field**, and the quantize step
(`PackedVertex::from_world`, called at `render/chunk_renderer.rs:427` and
`render/world_renderer.rs:1787`) sees only `position+color` — so the tag must be
carried from emit to pack. Two options:

### Option A (recommended, smallest blast radius) — parallel tag stream
Add `pub sway_tags: Vec<u8>` to `ChunkMesh` (one entry per vertex, default
`SwayTag::None`). At each emit site set it from `vertex_sway_tag(block, face,
is_plant, is_base_vertex)`:
- **greedy cube faces** (Grass/Dirt/Stone/…): `is_plant=false`, `is_leaf` derived
  inside the adapter → solid blocks get `None`; **leaves** (`is_subvoxel_only`)
  get `Sway` on every face.
- **plant pass** (`!block.spawns_grass_tufts()` branch, ~`mesher.rs:2123` —
  `stamp_template` / `emit_subvoxel_grid`): `is_plant=true`; mark the lowest
  vertex row of the plant cell `is_base_vertex=true` (→ `Anchor`), the rest
  `Sway`. Template stamping already knows cell-local Y.
- **grass tufts** (Case B): tuft geometry is foliage → `Sway` (no base anchor
  needed; the tuft host block keeps its solid `None` faces).

Then in the quantize loops, replace
`PackedVertex::from_world(v, origin)` with a tag-aware pack:
```rust
let pv = PackedVertex::from_world(v, origin);
let pv = PackedVertex { pos_packed: tag_packed_pos(pv.pos_packed, SwayTag::from_bits(tags[i])), ..pv };
```
`tag_packed_pos` only flips bits 30..31, so it composes with everything.

### Option B — widen the vertex
Add `flags: u8` to `Vertex`/`GpuVertex`, update the 24-byte assertion + the
`chunk_mesh.comp` output struct. More invasive (touches the GPU comp readback
layout) and changes upload bandwidth; only worth it if a parallel `Vec<u8>`
proves awkward. **Not recommended.**

### GPU mesher parity
`gpu_mesher_vk` emits the **same unpacked** `GpuVertex`; packing happens in the
same renderer quantize step. So Option A needs the GPU readback path to produce
the same per-vertex tag — cleanest is to have `chunk_mesh.comp` write a tag byte
alongside each vertex (or recompute it CPU-side from the block id during readback,
since the pool still has the block volume). Either way both meshers funnel through
`tag_packed_pos`, so `chunk.vert` stays single-source. `chunk_mesh.comp` is **not**
on the off-limits list; `chunk.frag` and `froxel_fill.comp` are and were untouched.

---

## Merge seam (important)

`chunk.vert` now sways **only** tagged vertices. Until Option A is wired, every
vertex packs tag `0`, so **foliage will not sway at all** (no regression to the
bug — it just goes quiet). Merge the shader + the mesher tagging in the **same**
step so sway returns the instant tags are emitted. Gate order per the dispatch
packet keeps gpu last; this is the final gate.

## Verify on-device
1. `cargo test -p unendlich-engine --lib foliage_sway` (and the shader freshness
   test in `shader_artifacts`).
2. `bash tools/validate_shaders.sh` → 35 OK.
3. Run with `GPU_MESHER=2`, look at a grass field under wind: solid block tops
   are still, tall grass/ferns/flowers sway with rooted bases, leaf canopies
   ripple. (A `examples/sim_collage.rs`-style headless PNG of a tagged vs
   untagged column is a nice-to-have once tags are emitted.)

## Honesty notes
- Toolchain this round was rebuilt from the apt pool offline: **rustc 1.75**
  (not the documented 1.96; fine for a dependency-free standalone `sim::` module
  on edition 2021 — no post-1.75 std APIs used), glslangValidator 15.1.0,
  spirv-val 2025.1.
- No full-workspace `cargo build` was run (cold link on the ~1031-module tree
  overruns a turn; standalone `rustc --test` is the documented substitute for a
  dependency-free `sim::` module). The `mesher.rs` adapter compiles against
  `crate::`, so it is gated on the coordinator's merge build, not in-sandbox.
- The proof PNG (`foliage_sway_proof.png`) is a CPU replica of the shader math,
  **not** an engine screenshot — clearly labelled as such.
