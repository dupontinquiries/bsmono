# ROADMAP — consolidated (2026-07-18)

The standing plan for the inference engine, recapped at the fusion kickoff. Kept verbatim from
the session note so the reasoning travels with the priorities; see FIELD_NOTES.md for the
dispatch-bench numbers it cites, LONG_CONTEXT.md for the KV/context ladder, and FLEET.md for
the fan-out status.

Good time to consolidate. The roadmap as we left it, recapped: (1) Speed on the M2 — kernel
fusion, because the dispatch bench proved the wall is the 0.51 ms serialization tax × ~700
chained dispatches/token (batching can't touch it; only fewer, fatter kernels can). (2) Long
context — the "1M" splits into near-term plumbing (int8/fp16 KV, chunked attention → real
128-256K) with true 1M waiting on the sparse-attention stage. (3) Bigger/other MoE —
DeepSeek-path (shared experts, then DSA), with GLM-5.2 as the decide-later endgame; Qwen3.5
now loads past its metadata wall too. Plus the parked product items: fan-out (proven
near-free), docs→small-model.

So: starting fusion now, with the highest-value safe target for the exact model you're
testing — the Qwen QK-Norm decode branch currently spends 4 chained dispatches (norm-q,
norm-k, rope-q, rope-k) that collapse naturally into one kernel: a workgroup per head computes
its RMS, normalizes with the right per-head weight (q vs k), and rotates its 64 pairs —
hd=128 means exactly 64 threads = 64 rotation pairs, a perfect fit.

Status ledger against this roadmap (update in place):
- fusion / QK-Norm 4→1: NOT STARTED here — first fusion target as scoped above.
- docs→small-model: LANDED in the UI layer as the chat file-summary toggle
  (crates/logscope-egui/src/filesummary.rs — structural digest instantly, small-model summary
  via the ollama sidecar, gitmind /tmp session files default to summary). The in-process
  pipeline route stays open through the same take_pending/fulfill queue.
