# perf-loop — the protocol

Two machines, two roles, one seam. The **M4 Max laptop** edits code and drives (`loop.sh`, Claude Code
lives here). The **M2 Ultra** is a headless target that only ever runs `bench.sh` and returns JSON.
The target never needs a human at it; the driver never needs a GPU.

```
laptop: edit → ./loop.sh sync → ./loop.sh build → ./loop.sh bench <tag> [K=V] → ./loop.sh compare
        edit .wgsl in perf-loop/shaders/ → ./loop.sh shaders <tag>            (no rebuild: LOGSCOPE_SHADER_DIR)
target: bench.sh <tag> → results/<ts>-<tag>.json + raw/*.log                   (refuses if a local AI server is up)
```

## Session start (every time)
1. `./loop.sh status` — what's on the Ultra right now. If llama-server/ollama/omlx/optiq are up: `./loop.sh quiet`.
   (`./loop.sh loud` puts them back when you're done. bench.sh will refuse to measure a busy GPU.)
2. `./loop.sh sync && ./loop.sh build`
3. `./loop.sh smoke` (1B model, ~20 s) to prove the pipe, then `./loop.sh bench baseline` on the real model.
   The first `baseline` after any engine change is the reference every later compare uses.

## One experiment = one tag
- Tag format `eN-short-name` (e.g. `e1-matvec-sg`). One variable per run; sweeps are several tags.
- Knobs go on the command line and are recorded in the JSON: `./loop.sh bench e2-flush128 LOGSCOPE_FLUSH_EVERY=128`.
- Shader-only changes: copy `crates/gpu-gemm/src/kernels/<k>.wgsl` into `perf-loop/shaders/`, edit, `./loop.sh shaders eN`.
  The engine announces every override it applies; if the announcement is missing, you measured the old kernel.
- Rust changes: `sync && build && bench`. Build output is on the laptop; if it fails, nothing was measured.

## Reading a result
`./loop.sh compare baseline eN` prints deltas and a verdict:
- **KEEP**: decode tok/s @ctx512 ≥ +3% and no new FAIL lines in `--gate-matvec` / `--gate-bytes`.
- **NOISE**: within ±3%. Re-run once; two NOISE results = discard the hypothesis, not the run.
- **DISCARD**: slower, or a gate regressed. Correctness beats speed every time; a faster wrong kernel is a bug.
The JSON also carries `sync_ms` vs `setup_ms` (GPU-bound vs CPU-bound), per-kernel GiB/s from `--gate-perf`,
the llama-bench reference on the same GGUF (`engine/ref tg`), and `roofline_tok_s_745` (active bytes at 745 GiB/s —
the M2 ULTRA figure; this box is an M2 Max at ~400, see `docs/agents/APPLE_M2_GPU_STACK.md`, so prefer the next
paragraph's numbers and keep that column only to compare old rows).
The two numbers that matter long-term are **engine/ref** (how far from Metal-llama.cpp) and **% of roofline**.

Since 2026-10-04 `--bench` COUNTS rather than quotes, and the JSON gains (all under `decode_ctx512` /
`decode_ctx8`): `dispatches`, `passes`, `submits`, `copies`, `polls` per token (the old line said
"1 submit/token; ~210 dispatches/token" as a constant string — the code says ~8 and 202 for a 32-layer
hybrid); `weight_gb_per_token` and `gbs_over_wall` / `gbs_over_sync` over the bytes the ENGINE reads
(int8-resident is 9 bits/weight: the ledger's "28 GB/s effective" for Ornith-9B divided a Q4_K_M file
size by the token time and was really ~45); `prefill_tok_s` for the `ctx` tokens through the app's chunked
path (the first prompt-processing rate any tool here reports; needs `--bench <m> 64 512`, not ctx 8);
and `pct_of_peak` / `roofline_tok_s_peak` against `BENCH_PEAK_GBS` (default 400). With
`LOGSCOPE_GPU_TIMESTAMPS=1` in the env the bench adds a per-kernel GPU-time table from timestamp queries
(`kernels` in the JSON: ms/token, dispatches, %, µs/dispatch per shader) and the split between time inside
passes and time between them — which kernel, not just that the GPU was busy. Timestamps are requested from
the device only under that flag, so the default device is the one every earlier row measured.
`LOGSCOPE_FUSED_V2=0` is the A/B for the word-per-thread rewrite of the three fused decode kernels
(`--gate-fused-v2` is its correctness gate); `bench e7-fusedv1` with it set against the baseline is the
first experiment to run.

## Keep / commit / ledger
On KEEP: commit with the tag in the message, append one line to `LEDGER.md`
(`tag | change | tok/s@512 base→new | Δ% | engine/ref | KEEP`), and re-tag that run as the new `baseline`
(`./loop.sh bench baseline`). On DISCARD: revert, one line in the ledger, next experiment.

## Rules
- Never bench with local AI services running (the number is about them, not the change).
- Don't change two knobs at once unless the tag says `sweep-`. Don't tune on the 1B; smoke only.
- The autotune cache is fresh per run by default (`LOGSCOPE_CACHE_DIR` = the run dir). Pin it only when the
  experiment is "does the cache help", not when the experiment is a kernel.
- Read `raw/*.log` before believing a surprising JSON. The parser is regexes over stdout, not an oracle.
- Nothing in the loop writes outside `~/perf/` on the target. Models are read-only.
- Sandbox lane (optional): the offline toolchain + lavapipe run the correctness gates without a GPU
  (`run-all-gates.sh`); lavapipe timings are meaningless, its FAIL lines are not.

## Handing the loop to a local model

`crates/gpu-gemm/LOCAL-AI-PROBING.md` is the runbook for a model driving this unattended: the queue in
order, a stop rule per experiment, the record format, and the three hazards (the subgroup kernel can
hang the GPU, raising the memory budget can OOM the box, long sweeps are a thermal load). Point the
model at that file and nothing else; it is written to be the whole brief.

A test in `gpu-gemm` (`sgladder::runbook_tests`) fails if that document names a gate or an engine env
knob the source does not have — a renamed flag is worse than a missing one, because the model would run
the command, silently get the default, and report the number as a measurement.

## Where the time goes (why this loop exists)
Measured on the M2 Ultra: DeepSeek-8B Q4 decode 182 ms/token against a 10 ms roofline; Qwen3-30B-A3B
prefill 51 tok/s against ~700–1,000 for llama.cpp Metal on the same box. Ornith-1.5-35B-A3B is a
Qwen3.5-35B-A3B fine-tune (same qwen35moe graph), so every win on the Qwen GGUF is a win on Ornith — set
`BENCH_MODEL` to the Ornith GGUF directly if that is the workload you care about.
