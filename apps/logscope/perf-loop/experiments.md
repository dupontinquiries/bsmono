# experiments — the queue, cheapest first

Each entry: hypothesis, how to run, what "worked" looks like, stop rule. Sources are the engine's own docs.

| # | tag | change | run | expect | stop rule |
|---|---|---|---|---|---|
| 0 | baseline | none; also `llama-bench` on the same GGUF | `bench baseline` | the two ratios: engine/ref and % of roofline | — |
| 1 | e1-matvec-sg | force the subgroup matvec (`V2WgSg`, native subgroupAdd, no threadgroup buffer). Field report: green on Metal, rel 1.2e-7 | `bench e1-matvec-sg LOGSCOPE_MATVEC_SG=1` | decode-vs-prefill.md says the m=1 path is latency-bound; this is the kernel built for that. Anything from 1.3x up | if the gate goes red on Metal, that's a bug report, not a discard |
| 2 | e2-flush-N | submit batching sweep | `bench e2-flush128 LOGSCOPE_FLUSH_EVERY=128`, then 256 | 64 already halved Qwen-14B (1.02→2.44); diminishing past the point where one submit's GPU time > ~2 ms | flat two steps in a row |
| 3 | e3-fuse-* | dispatch fusion toggles, one per tag: `LOGSCOPE_FUSED_QKV`, `LOGSCOPE_FUSE_GLU`, `LOGSCOPE_FUSE_DOWN`, `LOGSCOPE_PASS_FUSE` | four tags | ~210 dispatches/token today; each fusion 5–20% while sync_ms ≈ wall | any tag with a gate FAIL |
| 4 | e4-prefill-metric | **add** `--bench-prefill <gguf> <n>` to kernel-validate (prefill N synthetic tokens, print tok/s), then extend bench.sh to run it at 2048 | code, ~15 lines | none: this makes prefill visible. Without it E5–E7 can't be judged | — |
| 5 | e5-prefill-tiled | `LOGSCOPE_PREFILL_TILED=1` + `LOGSCOPE_PREFILL_CHUNK` 256/512/1024 | sweep tags | the 241→121 s Qwen-14B prefill came from batching; the tiled GEMM is the next lever named in FIELD_NOTES | flat across chunk sizes |
| 6 | e6-kquant-f16 | route k-quant prefill through dequant→f16 GEMM instead of inline dequant (the llama.cpp Vulkan crowd measured +8% on AGX for exactly this) | code in the prefill path; shader-only part via `shaders/` | prefill only | — |
| 7 | e7-q4-prefill-gemm | the q4 prefill GEMM `prefill-q4-gap.md` says is missing (prefill runs the q8 path) | code | prefill 2x+ on q4 models | — |
| 8 | e8-bindgroups | cache bind groups if `setup_ms` is > 15% of ms/token in the baseline JSON | code | setup_ms → ~0 | only if the baseline says setup matters |
| 9 | e9-moe-chunk | MoE prefill chunk raise + expert-kernel profile on the 35B-A3B (Scout: 188–350 s per 2–3K tokens on Metal) | `bench` with the A3B model after E4 exists | the 51→? tok/s prefill line | — |

Rule of thumb from the roofline: decode on a 3B-active MoE at Q4 (~2 GiB active) should approach
745/2 ≈ 370 tok/s in theory; llama.cpp Metal gets ~80–100 on this box. Passing 40 tok/s means the
engine is competitive for agents; passing llama.cpp's number means it's the fastest thing on the machine.

## E1 revised: the subgroup kernel is a LADDER, not a switch (2026-09-23)

E1 said "force the subgroup matvec with `LOGSCOPE_MATVEC_SG=1`". The gate's own comment makes that
reckless as written: on Asahi/Honeykrisp the kernel can be correct at small shapes and **hang the GPU**
at a vocabulary-sized projection (n=128256), which the firmware watchdog turns into a dead GPU. The
load-time self-check is a correctness check on a tiny shape, so it passes and the hang still comes.

The kernel is now gated per WIDTH, by a record this driver builds for itself:

```
kernel-validate --gate-sgladder     # probes ONE more width, then stops
```

Each run walks one rung of `[2048, 4096, 8192, 16384, 32768, 131072, 262144]`, and — the whole point —
writes `attempting=N` to `~/.cache/logscope/sgladder-<adapter>.txt` and **fsyncs it before the
dispatch**. Three outcomes:

* returns OK → `ok=N` appended, the next run probes the next rung;
* returns a wrong result → `failed=N`, the ladder stops there, everything below stays available;
* never returns (the hang) → the note is all that is left. The next engine start reads `attempting=N`
  with no result, concludes the GPU died there, and caps at the largest proven width **permanently**.
  You reboot, start logscope, and the fast kernel is on for every width that worked.

`Gpu::subgroup_ok_for_width(n)` is what the decode path asks now, so an FFN projection can use the
subgroup kernel on a driver where the output projection cannot. Drivers with no record behave exactly
as before, and one that has never run the ladder gets `Never` — proven-or-nothing.

Run order on the M2 Ultra under Asahi: `./loop.sh bench baseline`, then `--gate-sgladder` repeatedly
until it says finished or blocked, then `./loop.sh bench e1-matvec-sg` and compare. If a run kills the
GPU, that is the experiment succeeding: the number you wanted is now on disk.
