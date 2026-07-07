# Block texturing — session plan

Today blocks render with a flat per-block colour (`BlockId::base_color`). The mesh
vertex is `[pos: vec3, color: vec3]` (24 bytes, `gpu_mesher_vk.rs`). Texturing swaps
the flat colour for an atlas sample. This needs the GPU/shader path, so it's a
hands-on session on the M2 (with a shader compiler) — but the pure data layer is
already in place and tested.

## Already done (pure, tested — `world/block_textures.rs`)
- `ATLAS_COLS = 16` → a 16×16 = 256-tile square atlas, row-major from top-left.
- `tile` module: named slot indices (the stable art↔code contract).
- `tiles(BlockId) -> BlockTiles { top, side, bottom }` — grass differs by face,
  most blocks uniform, unmapped → a `MISSING` checker so gaps are obvious.
- `tile_uv(tile) -> (u0,v0,u1,v1)` — the UV rect for a tile.

## Session steps (M2, in order)
1. **Atlas art.** Author a 16×16 tile PNG (each tile e.g. 16×16 px → 256×256 sheet)
   filling the slots in `block_textures::tile`. A magenta/black checker at index 255.
2. **Vertex format.** Extend the mesh vertex to carry a UV (and we keep `color` as a
   tint, so foliage/biome tinting still works): `[pos: vec3, color: vec3, uv: vec2]`.
   Update the vertex input attribute descriptions and the stride in
   `gpu_mesher_vk.rs` / the renderer's pipeline.
3. **Mesher UVs.** In the brick/GPU mesher, per visible face pick the tile via
   `tiles(block)` (top/side/bottom by face normal), get the rect from `tile_uv`, and
   write the four corner UVs — scaled by the quad footprint so a merged N×M quad
   tiles the texture N×M instead of stretching one tile.
4. **Atlas upload.** Load the PNG, create a Vulkan image + view + sampler
   (nearest filtering, clamp; mipmaps optional later), and add it to the world
   descriptor set (there are already shadow + froxel samplers to model on).
5. **Shaders.** Fragment shader samples the atlas at the interpolated UV and
   multiplies by the vertex `color` tint. Recompile the SPIR-V (this is why we need
   the M2 — no shader compiler in the dev sandbox).
6. **Test.** Confirm grass shows grass-top / dirt-side-with-grass-fringe / dirt
   bottom; stone/sand uniform; an unmapped block shows the checker.

## Notes / decisions to make together
- Tile size (16 vs 32 px) and whether to add mipmaps (avoids shimmer at distance,
  but needs the mip chain + trilinear sampling).
- Keep the `color` vertex attribute as a biome/foliage tint multiplier, or drop it
  once textures carry the look. (Recommend keep — cheap, and tinting is useful.)
- Greedy-meshed quads: UV tiling via `fract` in the shader, or bake repeated UVs.
  Tiling-in-shader is simpler and matches the merged-quad geometry.

## Decisions reached (design discussion)

**Anti-flicker is the migration priority.** Moving from flat colour to texture
introduces aliasing that flat colour never had. The stack that must be right:
full mip chain + trilinear (linear-mipmap-linear) + **anisotropic filtering**
(4–8x) — aniso is what kills the grazing-angle/distance sparkle that trilinear
alone leaves. Verify `samplerAnisotropy` in the Vulkan feature query (Honeykrisp/
Mesa) like ASTC.

**Format.** RGBA8 + mips + aniso now; ASTC later for higher-res. Note: you can
runtime-generate mips for RGBA8 (`vkCmdBlitImage` down-chain), but **not** for a
compressed format — ASTC mip chains are baked offline (build chain → compress each
level), so structure the loader for precomputed mips.

**Array, not atlas.** Use a 2D texture *array* (one layer per tile) rather than a
packed sheet: clean per-tile mips (no cross-tile bleed) and clean cross-blend/
wobble (no bleeding into neighbours). `block_textures::tiles()` maps to a layer
index just as well as an atlas UV.

**Material keying is layered, not one LUT.**
- Base optical material per block → dense `[SurfaceMaterial; N]` indexed by
  `BlockId` (`world/block_material.rs`: roughness, reflectivity/F0, relief height,
  emissive; 4 bytes each — tiny, no compression needed).
- Spatial variation per coordinate → *derived* in-shader from world-space noise +
  a sparse physics erosion/wetness field, via `SurfaceMaterial::with_weathering`
  (erosion roughens/pits/dulls; wetness smooths/brightens). Physics writes the
  field, shading reads it — optical roughness stays separate from physics friction.
- Per-texel maps (a real roughness/height image) → deferred; only for hero
  surfaces. Parallax depth is a per-texel height channel, out of the first pass.

## Procedural surface variation (one shared noise feeds all three)
Build one stable **world-space 3D noise** sampled in the fragment shader; it
drives: (1) tiling-break (variant/hex-tiling cross-blend over an x-px band — costs
2–4 samples), (2) wobble (UV domain-warp by 1–2px so identical-block seams aren't a
ruled grid — drive from world position so it doesn't swim), (3) roughness/erosion
variation. Procedural **solid texturing** (material as a function of position, no
image/atlas) is the direction to grow into for natural materials — it fits the
material-ID-grid + world-noise architecture and gives seam-free, infinite-res
surfaces; its anti-aliasing is analytic band-limiting (`fwidth`) rather than mips.

## Deformation-readiness (reserve now, build later)
To keep skin-warping creatures and elastic blocks (tree-trunk sway) open without
re-architecting texturing, carry **two coordinate spaces** per vertex:
- **render position** — rasterized after any deformation (skinning, bend, squash).
- **rest/material coordinate** — fixed at mesh-build; *all* appearance (solid
  texture, roughness, erosion) is evaluated here. Glues the texture to the surface
  so it warps with it — no swim, no seam.

Cost to reserve today is ~nothing: rest coord == local position and deformation ==
identity for rigid box parts / static blocks. Add the rest-coord vertex attribute,
evaluate material in rest space, and a per-vertex **flex-weight** slot (0 for now;
later: 0 at a trunk's anchored base rising toward the branches, times the wind
field → sway). When deformation arrives it's a vertex-shader change only. Reminders:
deformed geometry needs its **normals** transformed/recomputed (or lighting breaks),
and visual deformation should be **decoupled from collision** by default (hitbox
stays grid-aligned; leave a hook for future physics-coupled elastic response).
