# HANDOFF — r336-textures (variation + broader coverage)

**WID:** `r336-textures` · **Status:** pure core VERIFIED in-sandbox (10/10 new,
31/31 whole file; rustc 1.96.0). De-tiling PNG shipped.
**Coordinator action:** (a) expand the atlas image with variant runs (asset step),
(b) call `variant_for` at **mesh time** and feed the resolved tile through the
existing tile→UV path. No shader edit needed (and `chunk.frag` is off-limits).

---

## What landed (verified, pure)
Extended `crates/unendlich-engine/src/texture_atlas.rs` (additive, `// r336-textures`
header — **no `mod.rs` change**, no new file).

API added:
```
const VARIANT_ATLAS: AtlasLayout                       // 16px 8×8 (64 slots) for variant runs
struct VariantSet { base_tile, count } + single/run/tile(variant)
variant_count_for(block) -> u8                         // per-block variant count (1 = no variation)
variant_for(block, pos:[i32;3]) -> u8                  // deterministic position-hashed pick in [0,count)
variant_uv(&AtlasLayout, &VariantSet, variant) -> Option<[f32;4]>
resolve_variant_uv(&AtlasLayout, block, &VariantSet, pos) -> Option<[f32;4]>
block_texture_ext(block) -> Option<BlockTexture>       // +20 blocks via tint, NO new pixels
block_texture_full(block) -> Option<BlockTexture>      // shipped block_texture(), else ext
```

Reuses `AtlasLayout`/`FaceTiles`/`BlockTexture` and the inset-UV math unchanged.

## Two deliverables

### 1. Per-block variation (the de-tiling win)
`variant_for(block, pos)` is a murmur3-fmix32 hash of `(block_id, x, y, z)` →
`0..variant_count_for(block)`. Deterministic (stable across frames/re-meshes),
well-mixed (see `texture_variation_proof.png` — Stone's 4 variants over 48×48 with
no visible repeat), single-variant blocks always `0`. A block's variant tiles are a
contiguous `VariantSet` run; `resolve_variant_uv` turns `(block,pos)` into the UV rect.

### 2. Broader coverage with no new pixels
`block_texture_ext` maps **20 more `BlockId`s** onto existing tiles via tint (the
established "one pattern, many blocks" approach): the extended stone family
(Limestone/Marble/Basalt/Tuff/Calcite/Schist/Quartz/Obsidian/Bedrock/StoneBricks),
sediment (Sandstone/RedSandstone/Gravel/Clay/Moss), and ores
(Gold/Iron/Coal/Copper off the speckled diamond-ore tile). All tile indices are
within the shipped 12-tile `BLOCK_ATLAS` (asserted) — they ship today, no art needed.

---

## Wiring (propose — do NOT wire)

### Asset step — variant runs
`VARIANT_ATLAS` documents where variant tiles live (8×8 = 64 slots). To make
`variant_count_for` real, the atlas **image** needs the variant runs authored
(e.g. 4 stone variants, 3 dirt/gravel/sand, 2 grass/moss) as contiguous tiles, and
each `VariantSet.base_tile` pointed at its run. Until then, `variant_for` still
returns a valid index; with `count==1` everywhere it's a no-op, so it's safe to land
the logic first and the art later.

### Mesh-time selection (NOT the shader)
Today the thin chunk vertex carries `block_id`; the fragment shader looks the block
up in `block_texture_table()` and picks the tile by normal. The shader has no world
position per fragment-of-a-quad and **`chunk.frag` is off-limits**, so variation
must be resolved at **mesh time**, where the block + world position are known:

1. In the greedy face emit (`render/mesher.rs`), for each face compute
   `let v = texture_atlas::variant_for(block, world_pos);` and select the tile via
   the block's `VariantSet`: `set.tile(v)` — then feed that tile index into the
   existing tile→UV path (or into the per-quad tile id the vertex/table already
   carries). One extra hash per emitted face; no new vertex attribute if the tile
   id is already per-face.
2. Extend `block_texture_table()` / `BlockTexGpu` only if you prefer the variant run
   to live GPU-side; simplest is mesh-time tile selection feeding the current table.

Because the emit path + GPU table are shared scaffolding (and the shader is
off-limits), this is proposed, not wired.

### block_texture_full
Fold the `block_texture_ext` arms into `block_texture` on merge (or just call
`block_texture_full` from `block_texture_table()`), so the +20 blocks populate the
GPU table. Tests pin that originals are unchanged.

## Verify on-device
- `cargo test -p unendlich-engine --lib texture_atlas` (mirrors the 31 standalone tests).
- After wiring: large stone/dirt/sand expanses no longer show an obvious tile repeat;
  the +20 blocks render with their tints instead of flat material colour.

## Honesty
- Pure/deterministic/panic-free: no IO/clock/RNG/`unwrap`/`expect`; integer hash, no float traps.
- New atlas *pixels* are an asset step — shipped here is the slot/UV/variation **math** and the
  pixel-free tint coverage, exactly per the brief's watch note.
- `render/mesher.rs` untouched (read-only); the PNG is the real `variant_for`, not a screenshot.
- Base = r330b (dispatch base r336); `texture_atlas`/`moisture_texture` present; `mathx` (r332) not
  in this base — not needed here (only integer hashing + existing AtlasLayout math).

## Pre-existing failures untouched (per SHARED_RULES)
loot one_destroyed_eye_halves_the_amber, mob losing_both_eyes_blinds_it,
smoke_gpu pure_decay_monotonic_decrease.
