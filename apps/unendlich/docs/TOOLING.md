# Tooling & verification guide

A guide to the verification tooling built up while migrating the block renderer to
textures, written so a later session (human or AI) can *use and extend* it without
rediscovering the environment's constraints. The tool-usage parts below are
explicit (exact commands, how to extend). For project state and design rationale,
this points at the other docs rather than duplicating them.

## The verify boundary (what can and can't be checked here)

The sandbox has the Rust toolchain but **no GPU, no window, and no shader
compiler**. That splits work into three tiers:

1. **Pure lib — fully verifiable.**
   ```
   cargo test -p unendlich-engine --no-default-features --lib <filter>
   ```
   Covers `world/`, `sim/`, and other non-render modules (~15–20s for the whole
   suite). New pure modules type-check and test here.

2. **`render/` modules — verifiable via the `mesh-ref` feature.**
   `render` is feature-gated (`#[cfg(feature = "render")]`), so it is NOT in the
   default `--no-default-features` build. But the `mesh-ref` feature pulls in
   `render` *without* the blocked egui/window/Vulkan stack:
   ```
   cargo test -p unendlich-engine --no-default-features --features mesh-ref --lib render::mesher::
   ```
   This is how the vertex pack path (`render/mesher.rs`) is tested here.

3. **NOT verifiable in-sandbox — the human's machine is the compiler/runtime.**
   - GLSL → SPIR-V: there is no `glslc`/`glslangValidator`. Shaders are
     write-only here; `build.rs` recompiles them on the machine that has a
     compiler.
   - The full render bin (`--features render`) fails on a blocked `egui_plot`
     download, so the live `bin/unendlich_world.rs` and all shaders cannot be
     compiled here.
   - Anything needing a GPU, window, or actually running a frame.

   For tier 3, the workflow is: write the code, hand over an exact
   compile/run/revert runbook, and **flag every unverified line**. Prefer small,
   diagnosable increments over one big unverified change to the hot render path.

## The shader contract checker (the main tool)

`crates/unendlich-engine/src/world/shader_contract.rs` reads the real shader
*source* as text and asserts it agrees with the Rust ruleset. It can't compile
GLSL, but it catches the bugs that actually bite: **Rust ↔ GLSL convention drift**
— a constant changed on one side, a vertex attribute grown on one side, or a
shader sampling a descriptor binding the pipeline never declared (an instant
device-lost on the machine).

Run it (identical in-sandbox and on the machine):
```
cargo test -p unendlich-engine --no-default-features --lib shader_contract
```

What it checks today (against `chunk.vert` / `chunk.frag`):
- `CHUNK_EXTENT` in the vertex shader matches the ruleset constant.
- Vertex inputs equal the declared packed-vertex layout (locations 0/1) — so a
  vertex grown on one side only is caught.
- No descriptor binding outside the declared set `{0,1,2,3}`.
- Face-UV reconstruction uses the same face-tangent axes as
  `texture_mapping::face_uv`.

**How to grow it** (do this whenever you add a shader convention):
- Add the expectation to the data tables (`EXPECTED_BINDINGS`,
  `EXPECTED_VERTEX_INPUTS`, `FACE_UV_EXPRS`) or add a new check inside
  `check_world_shaders()`.
- Add a test that the current shaders satisfy it **and** a synthetic-drift test
  proving the checker *detects* the violation (feed it a bogus string) — a check
  that only ever passes is worthless.
- If you add a new descriptor binding (e.g. a texture-array sampler), put it in
  `EXPECTED_BINDINGS` so the binding check stays meaningful.

## The texturing ruleset

`crates/unendlich-engine/src/world/texture_mapping.rs` is the CPU reference for
every convention the GLSL must transcribe: `snap_normal` (axis-snap, ported from
`chunk.frag`), packed-vertex round-trips (`A2B10G10R10` position + `R8G8B8A8`
color, including the proof that a tile id rides the reserved alpha byte
losslessly), `face_uv` (world-pos → in-face UV), array/atlas math, and
`mip_levels`/`mip_dim`. It is proven by property tests. **The GLSL is a
transcription of these formulas** — change a convention here first, then let the
contract checker enforce that the shader follows.

Related data layers: `world/block_textures.rs` (block_id → tile index, the
single source of truth for which tile a face shows) and `world/block_material.rs`
(per-block optical material + `with_weathering` for the physics-erosion seam).

## The production-packer pin

`render/mesher.rs::pack_color_id(color, id)` is the single seam both meshers
converge on (the CPU mesher and the octree GPU readback both reach
`PackedVertex::from_world → pack_color`). It rides a tile/material id in the
`R8G8B8A8` alpha byte; `pack_color` delegates with `id = 0xFF` so behaviour is
unchanged until a real id is emitted. Its tests include a **bit-for-bit
cross-check against `texture_mapping::pack_color_id`**, so the production packer
and the ruleset the contract is built on cannot drift. Verify under `mesh-ref`.

## Build / test / deliver loop

Per change: edit the canonical repo, copy the edited files into the build mirror
(`/home/claude/build`), run the relevant test filter, then the full lib suite;
commit with explicit paths (never `-A`, never stage `Cargo.lock`); regenerate the
patch set and the monorepo zip. The session summary carries the exact regen
commands and the known-failing tests (a pre-existing decay test and a SPIR-V
freshness test that only fails in a clean sandbox with no shader compiler).

## Where the texturing work stands / next step

- **Increment 1 (done, validated on the machine):** fragment-only per-face UV
  reconstruction — `chunk.frag` draws a `SHOW_UV_DEBUG` grid/checker from
  `v_world_pos` + the snapped normal. Proved UVs are block-aligned and 1m-tiled.
- **Seam (done):** `pack_color_id` makes the pack path id-capable.
- **Increment 2 (open fork):** carry a per-block tile id from the octree compute
  shader (`shaders/chunk_mesh_octree.comp`, the active near mesher — it already
  knows `block_id` where `color_for_block` runs) through the readback
  (`render/gpu_meshing_octree.rs`, which memcpy-reinterprets a `[f32;6]` fat
  vertex as `render::mesher::Vertex`) into the alpha byte, then decode in
  `chunk.vert` → flat varying → use in `chunk.frag`. Two decisions are open:
  (a) source the tile map by **mirroring `tiles()` as a comp function** (checker-
  pinned) vs **uploading a lookup buffer** (data-driven, new binding); and
  (b) **grow the fat vertex** by one slot (clear, but a stride change rippling
  through comp + `Vertex` + readback + buffer sizing) vs **repurpose the three
  color floats** during migration (no resize, but entangles fallback color).
  This is a coupled, hot-path, GPU-side-unverifiable change — write it as one
  coherent batch with a runbook and matching checker rules.

See `docs/TEXTURING.md` for the full design decisions (anti-flicker stack,
array-over-atlas, ASTC, procedural solid texturing, deformation-readiness) and
`MONOREPO_README.md` for the per-change log.
