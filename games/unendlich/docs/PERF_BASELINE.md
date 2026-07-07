# Perf baseline — CPU render-prep (r62, sandbox llvmpipe/CPU)

Tool: `perf-probe` (new, headless, no features) — `cargo run --release --bin perf-probe -- 1,2,3 --repeats 3`.
Emits CSV (radius,chunks,stage,total_ms,per_chunk_us,chunks_per_sec,extra). Stages: worldgen,
octree (from_chunk), roundtrip (to_chunk + correctness). Seed 0xCAFEF00D, 32^3 chunks.

Sandbox numbers (M2 will differ — diff the CSV):

| stage | per-chunk | rate | note |
|---|---|---|---|
| worldgen   | ~100,000 us | ~10 ch/s | **dominant — ~99.7% of CPU prep** |
| octree enc | ~310 us     | ~3,200 ch/s | encode is cheap |
| roundtrip  | ~400 us     | ~2,500 ch/s | decode + verify |

Findings that drive the perf candidates:
1. **Worldgen is the whole budget.** Any frame/stream-rate win comes from worldgen first.
2. **Octree currently costs memory, not time.** For mixed terrain chunks the encoded size is
   ~4x dense (u8) — many nodes, poor compression. Octree pays off only for homogeneous regions.
3. **Density is regional**: chunk_y>=2 is 0% solid (sky), chunk_y<=-2 is ~98% solid (deep stone)
   — fully-homogeneous chunks are common and can be detected + fast-pathed.

The benches:
- headless / sandbox-runnable: `perf-probe`, `world-bench`, `octree-bench` (no features), `mesh-bench`, `winding-check` (render, no window).
- device-only (window/GPU, run on the M2): `gpu-mesh-bench`, `unendlich-spin`, `unendlich-chunk`.
