# Platform validation — what is PROVEN where

The engine targets, in priority order: **aarch64 Asahi Linux (Mesa Honeykrisp Vulkan)** first,
**macOS (Metal)** second, with **llvmpipe/lavapipe** as the CI/sandbox validator. This file
records what has actually been *measured* on each, so "supported" stops meaning "guessed".
Primary sources: the 2026-07-27 field kernel-validation report from an Apple M2 Ultra on
**macOS Metal** (`kernel-validate --validate`, 164 rows), the Scout/GLM field logs from the same
machine, and this repo's own gate battery on Asahi and llvmpipe.

## Kernel suite

| Area | Asahi (Honeykrisp Vulkan, M2) | macOS (Metal, M2 Ultra) | llvmpipe (CI) |
|---|---|---|---|
| Core kernels (gemm, matvec, norms, rope, attention, dequant) | green (gate battery) | **163/164 green** — field report | green |
| Subgroup matvec | **OFF** — hangs the compiler/driver; preflight disables | **green, rel 1.2e-7** (field report — the lavapipe reds were the driver, as the registry guessed) | red rows = driver artifact, ignore |
| Flash split-K decode attention (4k–12k ctx) | green | green ≤3.4e-5 | green |
| Two-span (int8 warm + f32 hot) attention | green | green ≤1.4e-3 (tol 3e-2) | green |
| YaRN / freq-factors / llama4 attn-temp | green | green | green |
| `transformer block (resident)` row | green | **red, 4.082 — HARNESS artifact**, see below | green (0.000e0) |
| Speculative (n-gram) decode | green | green | green |

### The one Metal red row, explained
`transformer block (resident)` fails at 4.082 on Metal while, in the *same run*,
`model forward (load→resident→logits)` is exact at 7.153e-7 — and the failure is byte-identical
with `LOGSCOPE_FUSED_QKV=0`. The inference path every real model uses is exact; the isolated
harness disagrees with it only on Metal, i.e. the harness carries a platform-sensitive
layout/size assumption (it builds `BlockWeights` by hand rather than through the loader). The
row is deliberately left in place as a tripwire for whoever next has Metal hardware; treat it as
KNOWN-RED / not-inference until the harness is rebuilt on the loader path.

### Two gate rows that were red on Metal because *we* were wrong
* `gate-submit` "set_flush_every clamps + reports" — the clamp ceiling moved to 65536 after the
  row was written expecting 1024. Fixed: the row now asserts the real contract, honors the
  `LOGSCOPE_FLUSH_EVERY` pin, and restores state.
* `gate-qkv` end-to-end "bit-exact" — fused QKV is one reduction over the 3-projection width,
  the separate path is three; different reduction shapes legally round differently on real fp
  hardware. Metal refused the impossible contract honestly (and post-FFN-fuse, llvmpipe shows
  Δ=1.55e-6 too). The row now demands numerical equality (≤1e-4) and reports when it happens to
  be bit-exact.

## Submit economics (the number that decides tok/s)

| | Asahi Vulkan | macOS Metal |
|---|---|---|
| Cost per submit | cheap enough that the SubmitTuner's 8 ms *smoothness* budget binds first (settles ~16 passes/submit) | **~2.0 ms flat** — 1119 submits/tok on Scout was 2238 ms/tok of pure command-buffer overhead |
| Submit governor (in-flight cap) | works | **DISABLED at runtime** — completion callbacks slow/absent on this driver (field log) |
| Frame batching | worked for dense; **mid-decode readbacks used to kill it** (MoE router picks, windowed-sampler ids): `download()` took the frame and never reopened → one submit per dispatch after the first readback | same bug, catastrophically priced: ~23 stray submits/layer × 48 layers measured |

Fixed in this lineage: `download()`/`download_u32()` now **reopen the frame** (the cadence flush
always did), `end_frame()` skips empty frames, and the profile prints a
`submit causes — passes/download/end_frame/dims-wrap/frameless/other` breakdown so one run names
the mechanism. `frameless` should sit at ~0 during decode on every platform; if it doesn't,
that line is the bug report.

Also fixed: the autotune cache stores **packed DecodeConfigs** (`0x8000_0000 | kernel<<16 | tile`
— field values 2147549188/2147680260 decode to kernel 1/3, tile 4). A validation added earlier
mistook them for poison; packed values are now accepted and printed decoded, bare tiles accepted
in 1..=64, and only genuine garbage is discarded.

## Memory probes

| Probe | Linux/Asahi | macOS |
|---|---|---|
| System RAM (feeds the unified-memory GPU budget → q4 vs int8 residency) | `/proc/meminfo` | `sysctl hw.memsize` (new — a 192 GB Studio previously read as *unknown*, silently degrading residency choices) |
| Process RSS report | `smaps_rollup` (anon/file split) | `ps` total, split reported as unavailable (new — replaces the silent "unavailable on this platform") |

## Known voids (open, honest)

* **MoE prefill speed on Metal** — 188–350 s for 2–3 k tokens on Scout Q4_K even with the
  1024-token chunk raise. Unmeasured on Asahi for models this size. The frame fix does not
  address prefill; this is the next perf target for big MoE.
* **`transformer block (resident)` on Metal** — known-red harness row above; needs a Metal
  machine to rebuild the harness on the loader path and confirm green.
* **int8 MoE residency** — loader forces q4 for MoE (`LOGSCOPE_MOE_INT8=1` to override);
  unvalidated everywhere.
* **Governor on Metal** — with completion callbacks unreliable, in-flight throttling is off;
  submit *count* is the only lever there (hence the frame fix mattering ~20× on Scout).
* The field logs also showed a stale-UI **template mismatch** ("UI sent ChatML markers to a
  Llama4 model") from an older build; this lineage reads the live detected template, but the
  worker-side translate-and-warn stays as the belt to that suspender.
