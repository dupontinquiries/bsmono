# RC5+RC6 — v0.24.29 through v0.24.40 cumulative

Two new ships layered on RC4:
- **RC5 = v0.24.39-mid-lod** — GPU mesher accepts Mid LOD. Cells:
  16³ at stride 2 instead of 32³ at stride 1.
- **RC6 = v0.24.40-greedy-mid** — 1D greedy meshing at Mid (and
  higher) LODs. 2-10x vertex reduction at Mid distance.

Both gated behind `UNENDLICH_GPU_MESHER_MID=1` env var. Default
off so Near (mode 2 only) still works exactly as RC4.

## Full stack (RC4 + RC5 + RC6)

1. v0.24.29-shared-octree
2. v0.24.30-cross-chunk
3. v0.24.31-versioned-pool
4. v0.24.32-pool-prewarm
5. v0.24.33-climate-tint
6. v0.24.34-position-tint
7. v0.24.35-face-light
8. v0.24.36-bumpy-side-align
9. v0.24.37-vertex-ao
10. v0.24.38-bumpy-ao
11. v0.24.39-mid-lod  (RC5)
12. v0.24.40-greedy-mid (RC6)

## How to test

Standard Near (regression check — should look identical to RC4):
```bash
UNENDLICH_GPU_MESHER=2 \
  cargo run --release --bin unendlich-world --features render
```

Mid via GPU (RC5+RC6 new path):
```bash
UNENDLICH_GPU_MESHER=2 \
UNENDLICH_GPU_MESHER_MID=1 \
  cargo run --release --bin unendlich-world --features render
```

## What to look for

Near LOD (close-up): identical to RC4 — bumpy grass, AO at
inside corners, climate-tinted biomes, per-position color variance.

Mid LOD (medium distance):
- Chunks render with larger-but-fewer faces (greedy effect).
- Visible at chunk boundaries: flat shading, no per-block tint.
- No bumpy on Mid grass tops (foundation tradeoff).
- No AO darkening at Mid (foundation tradeoff).
- Climate tinting still works per-chunk.
- Face-light depth still works.

Performance:
- Vertex count at Mid: should be 2-10x lower than the same chunks
  meshed without greedy (compare to RC4 with `UNENDLICH_GPU_MESHER_MID=0`).
- Mesher per-chunk time at Mid: should be lower than RC4's
  CPU Mid mesher (~5-15 µs vs ~50-100 µs).

## Failure modes

- Mid chunks missing: shader hash mismatch, push constant size
  mismatch (24 B), or workgroup count math wrong.
- Greedy holes: wrong run-start detection. Visual: missing
  quads at column boundaries.
- Floating quads: `find_greedy_run` extending past the run.
- Z-fighting at Mid: double-emission. Both run-start AND
  non-start emitting.

If anything's badly broken at Mid, unset `UNENDLICH_GPU_MESHER_MID`
to fall back to CPU Mid mesher (RC4 behavior).
