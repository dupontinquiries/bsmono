# Apply: v0.24.NEXT … NEXT+9 (ten stacked ships)

Adds the **GPU mesher overflow fix** (the missing-meshes root cause) on
top of the previous nine. Workflow: `docs/LAUNCHER.md` · recovery:
`docs/ROLLBACK.md` · open work: `docs/BACKLOG.md` · roadmap:
`docs/CLIMATE_PATH.md`.

## Headline this batch

**v0.24.NEXT+9 fixes the missing meshes.** `INDEX_CAPACITY` was sized
equal to `VERTEX_CAPACITY` (65_535), but indices run ~1.5–3× vertices, so
dense foliage chunks index-overflowed → empty mesh → hole. Raised to
131_072 + CPU-mesher fallback on any remaining overflow. **Always-on
behaviour change** (the cap bump) + a default-on flag
(`UNENDLICH_GPU_OVERFLOW_FALLBACK`).

## Ships

| # | Ship | Flag | Effect |
|---|------|------|--------|
| 1 | evict-chunk-lod | — | LOD-transition mesh-version leak fix |
| 2 | barrier-tighten | — | tighter copy barrier |
| 3 | gpu-decorate | `UNENDLICH_GPU_DECORATE` | grass/water/lava on GPU chunks |
| 4 | pool-early-unlock | `UNENDLICH_GPU_POOL_EARLY_UNLOCK` | burst-stutter mitigation |
| 5 | seasons | `UNENDLICH_SEASONS` (+`UNENDLICH_TICKS_PER_YEAR`) | seasonal tint |
| 6 | weather | `UNENDLICH_WEATHER` | rain/lightning atmosphere |
| 7 | water-field | — (inert lib) | water heightfield + settle |
| 8 | water-bridge | — (inert lib) | seed + ground heights |
| 9 | weather-snow | `UNENDLICH_WEATHER` | rain↔snow by temp, flicker, storm wind |
| 10 | **overflow-fix** | `UNENDLICH_GPU_OVERFLOW_FALLBACK` (default on) | **fixes missing meshes** |

## Workflow

```bash
cd ~/Code/unendlich-development/unendlich-game
git checkout -- assets/shaders/ 2>/dev/null
rm -f events.jsonl

./commands/update_from_claude_zip.sh \
  ~/Downloads/unendlich-v0_24_NEXT+9-overflow-fix.zip

git add -A
git commit -m "v0.24.86-overflow-fix: raise INDEX_CAPACITY + CPU fallback (missing meshes)"
./commands/tag_current_version.sh

# Build the game bin (skips the gpu_mesh_bench bin with the pre-existing
# duplicate-field bug; build that separately once it's fixed):
cargo build --release --bin unendlich-world --features render,gpu
cargo build --release -p unendlich-launcher -p unendlich-analyze --features render,gpu

# Tests (no sanity.sh in your tree — run the suite directly):
cargo test --release --lib -p unendlich-engine --features render,gpu
```

## Verify the fix

Run with the GPU mesher on (your usual mix). The
`octree mesher overflow … failed` **holes should be gone**. You'll see
far fewer `…overflowed GPU mesher (…); CPU-fallback meshing` lines for
only the densest chunks, and those chunks now render.

A/B the fix:

```bash
# holes return:
... <flag mix> ... UNENDLICH_GPU_OVERFLOW_FALLBACK=0 cargo run --release --bin unendlich-world --features render,gpu
# holes filled (default):
... <flag mix> ... cargo run --release --bin unendlich-world --features render,gpu
```

This should make the GPU-mesher world match the "filled" look of your
CPU-mesher (v0.24.63) diag run.

## Still-open visual bugs (diagnosed, not fixed — see readme + BACKLOG)

- **Magenta foliage** — GPU per-cell color path / climate tint. Confirm
  with `UNENDLICH_GPU_MESHER=0` (CPU) and `UNENDLICH_SEASONS=0`. Needs
  `chunk_mesh_octree.comp` to fix.
- **Missing top faces** — GPU top-boundary neighbor sampling.
- **LOD overlap** beyond the hole-reveal effect — LOD-band management.

## Rollback

`INDEX_CAPACITY` is a one-line revert; fallback is
`UNENDLICH_GPU_OVERFLOW_FALLBACK=0`. `git reset --hard <prev-tag>` for a
full revert. See `docs/ROLLBACK.md`.
