# LEDGER

| tag | change | tok/s@512 base→new | Δ% | engine/ref | verdict |
|---|---|---|---|---|---|
| baseline | first run on Asahi M2 Max (G14C B1) Vulkan, Llama-3.2-3B Q4_K_M, 1112566931+dirty, ollama stopped | 12.94 @ctx544 (16.00 @ctx40) | — | n/a (no llama-bench) | BASELINE; gate-matvec/bytes pass; decode ~8% of 400 GB/s spec; results/20260930-230627-baseline.json in ~/perf |
| e2-flush128/256 | LOGSCOPE_FLUSH_EVERY=128/256 | 12.94→12.44/12.42 | -4% | n/a | NOISE/MOOT: decode is already 1 submit/token |
| e3-qkv / e3-norm / e4-bgcache | =1 on levers that default ON (qkv, norm) or whose setup is ~1ms (bgcache) | 12.94→12.82/12.91/12.94 | ≈0 | n/a | INVALID as written for qkv/norm (no-op); bgcache NOISE |
| e3-glu / e3-down | MoE fusions on a dense model | 12.94→12.95/12.87 | ≈0 | n/a | N/A dense |
| e3-qkv-off | LOGSCOPE_FUSED_QKV=0 (ablation) | 12.94→9.64 | -25% | n/a | fusion pays; keep ON |
| e3-norm-off | LOGSCOPE_FUSE_NORM=0 (ablation) | 12.94→8.86 | -31% | n/a | fusion pays; keep ON |
| e6-tile1/2/8 | LOGSCOPE_DECODE_TILE pinned | 12.94→12.57/12.83/12.95 | -3%..0 | n/a | NOISE: tile is not a lever |
| ornith9b-baseline | Ornith-1.5-9B Q4_K_M (hybrid Gated DeltaNet, 33 layers, d=4096), ollama stopped, after fixing `--bench` to build its cache with `new_cache_for` | 6.91 @ctx8, 5.08 @ctx544 | — | n/a | BASELINE; gpu-sync 190 of 197 ms/token = GPU-bound; ~5.5 GB of weights at 5.08 tok/s is ~28 GB/s effective (~7% of 400 GB/s), the same ~8% the 3B showed, so the limit is the kernels not the model; the `~210 dispatches/token` line is the same constant as the 3B run so it is not a measurement; first bench attempt panicked (raw `new_cache()` on a hybrid), see the gpu-gemm bench commit |
