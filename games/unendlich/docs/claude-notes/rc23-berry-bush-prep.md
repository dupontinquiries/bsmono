# RC23 prep: v0.24.57-berry-items (NOT YET APPLIED)

## Status

Built earlier in this session, packaged as
`unendlich-rc23-v0_24_57-berry-items.zip`. Has NOT been applied to
the user's machine — the user prioritized RC24-28 (entity scaffold)
and RC29 (adaptive uploads) first because RC23 is compile-risk
(adds a new BlockId requiring exhaustive matches in several spots).

This note records what's in RC23 and what to watch for when
applying it next session.

## What RC23 adds

A new block: `BlockId::BerryBush = 90`. A decorative item that
generates sparsely in surface biomes with appropriate moisture
and temperature, and renders with a wine-red ([0.55, 0.10, 0.20])
color.

## Files touched (4)

1. **crates/unendlich-engine/src/world/blocks.rs**
   - Adds the `BerryBush = 90` variant
   - Adds it to `is_decorative_item()` (skips collision)
   - Adds it to `from_id_str()` ("berry_bush" | "berries")
   - Adds it to `try_from(u8)` (90 → BerryBush)

2. **crates/unendlich-engine/src/render/mesher.rs**
   - **EXHAUSTIVE MATCH** in `block_color()`: adds the `[0.55, 0.10, 0.20]`
     (wine-red) arm for `BlockId::BerryBush`. This is the spot that
     a missing variant would fail to compile — the existing match
     uses `_ => ...` fallback so it actually wouldn't fail to compile,
     but the color would be the fallback (probably ugly pink) until
     this arm is added.

3. **crates/unendlich-engine/src/shaders/chunk_mesh_octree.comp**
   - Adds a color-table entry at index 90 matching the Rust side.
   - **This is the compile-risk part**: the SPIR-V shader binary
     gets recompiled by `build.rs`, but if the build.rs uses the
     pre-existing `.spv` and `.hash` files for cache invalidation
     and the .hash doesn't get refreshed, the runtime will load
     a stale shader. Watch for: visible black blocks where berry
     bushes should be (means the shader didn't recompile).

4. **crates/unendlich-engine/src/world/cpu_generator.rs**
   - Adds berry-bush placement during chunk generation. Density:
     `0.008` triggered via `tree_detail_hash` salt `0xBE221_0001`.
     Surface biomes only, requires moisture > some threshold AND
     suitable temperature.

## Pre-flight check before applying

```bash
# 1. Confirm the BerryBush variant doesn't already exist on the
#    user's tree (sanity — my sandbox got it applied somehow but
#    the user hasn't run RC23 yet):
grep -n "BerryBush" crates/unendlich-engine/src/world/blocks.rs

#    Expected: no matches. If matches exist, RC23 may have been
#    partially applied — investigate before proceeding.

# 2. Confirm cargo build succeeds BEFORE applying:
cargo build --release -p unendlich-engine --features render 2>&1 | tail -5

# 3. Apply the zip:
unzip -o ~/Code/unendlich-development/claude-zips/unendlich-rc23-v0_24_57-berry-items.zip

# 4. Rebuild — this is where the compile-risk surfaces. If the
#    `_ =>` fallback in mesher.rs was changed to exhaustive (which
#    my prep zip does NOT do, but a future RC might), the build
#    will fail with E0004 ("non-exhaustive patterns") and tell us
#    exactly where to add the new arm.
cargo build --release -p unendlich-engine --features render 2>&1 | tail -10

# 5. If build succeeds, run the tests:
cargo test --release -p unendlich-engine --features render --lib 2>&1 | tail -5

# 6. Run the game and look for berry bushes near water + suitable
#    temperature. They're rare (density 0.008 ~= 1 per 125 surface
#    cells) so it may take some walking.
```

## Rollback

If anything breaks:
```bash
git checkout -- crates/unendlich-engine/src/world/blocks.rs \
                crates/unendlich-engine/src/render/mesher.rs \
                crates/unendlich-engine/src/shaders/chunk_mesh_octree.comp \
                crates/unendlich-engine/src/world/cpu_generator.rs

# Also nuke the cached SPV if shader-side regenerated:
rm -f assets/shaders/chunk_mesh_octree.comp.spv*
cargo build --release -p unendlich-engine --features render
```

## Sandbox-state note

Claude's sandbox copy of the codebase has BerryBush ALREADY APPLIED
(probably from an earlier session). When I ship files from my
sandbox, I have to spot-check that I'm not accidentally including
BerryBush references in files that would otherwise be clean. I did
this for the RC30 omnibus — verified that none of the 8 patched
files reference BerryBush. But it's a recurring risk; if a future
ship of mine somehow breaks at compile time on `BlockId::BerryBush`,
the cause is "Claude's sandbox is one RC ahead of the user's tree".

The proper fix is to "roll back" my sandbox to match the user's
state by removing BerryBush. I haven't done this because it would
discard RC23's prepared changes and they'd have to be re-derived.
Future sessions should evaluate whether to:
  (a) Apply RC23 to the user's tree so my sandbox and theirs match.
  (b) Roll my sandbox back to pre-RC23 to match the user's state.
  (c) Manually de-BerryBush each file I ship (current approach;
      risky, requires careful spot-check every time).

## Open questions

- Is the shader cache key keyed on file content hash or on a
  separate version stamp? If content hash, the `.spv` will rebuild
  cleanly when the `.comp` source changes (good). If version stamp,
  the user has to bump it manually (likely a foot-gun).
- Are there other items in the pipeline (mushrooms, herbs, twigs)
  that should be batched into a single "decorative items" RC?
  Worth considering before applying RC23 to avoid 4 separate
  compile-risk RCs when a batch would have one risk window.
