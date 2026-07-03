#!/usr/bin/env bash
# bench-tps.sh — tokens/sec collector for logscope-suite's gpu-gemm engine.
#
# Run this ON THE MACHINE WITH THE REAL GPU (M2 / AGX). lavapipe in-sandbox proves
# correctness but CANNOT give tok/s. Every command here is a real kernel-validate mode
# (crates/gpu-gemm/src/main.rs is the source of truth for arg shapes).
#
# Usage:
#   ./tools/bench-tps.sh /path/to/Llama-3.2-1B-Instruct-Q4_K_M.gguf
#   ./tools/bench-tps.sh <target.gguf> <draft.gguf>     # also runs speculative decode
#
# Always-on now (no flag): dims-uniform cache + output-buffer pool (per-dispatch upload
# churn). The toggles below are the levers still worth A/B-ing.
set -uo pipefail
MODEL="${1:?usage: $0 <model.gguf> [draft.gguf]}"; DRAFT="${2:-}"
SUITE_DIR="${SUITE_DIR:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
GEMM_DIR="$SUITE_DIR/crates/gpu-gemm"
LOGDIR="$SUITE_DIR/bench-logs/$(date +%Y%m%d-%H%M%S)"; mkdir -p "$LOGDIR"
[ -f "$MODEL" ] || { echo "model not found: $MODEL" >&2; exit 1; }
cd "$GEMM_DIR" || exit 1

echo ">>> building (release)"; cargo build --release --bin kernel-validate 2>&1 | tee "$LOGDIR/00-build.log"
BIN="$GEMM_DIR/target/release/kernel-validate"
[ -x "$BIN" ] || { echo "build failed" >&2; exit 1; }
{ echo "=== host ==="; uname -a; echo "=== gpu ==="; vulkaninfo --summary 2>/dev/null | grep -iE 'deviceName|driverName' || echo "(no vulkaninfo)"; echo "=== model ==="; ls -la "$MODEL"; } > "$LOGDIR/01-env.log" 2>&1

run() { local name="$1"; shift; echo ">>> $name :: $*"; env "$@" 2>&1 | tee "$LOGDIR/$name.log"; }

# 1) correctness first — a tok/s number is meaningless if the kernels drifted.
run 10-gates "$BIN" --gate-qkv-e2e; run 11-gates "$BIN" --gate-perf

# 2) single-session baseline (the rate to beat) — and the FUSED_QKV A/B (the 84% lever).
run 20-base                              "$BIN" --bench "$MODEL" 64 128
run 21-base-fusedqkv  LOGSCOPE_FUSED_QKV=1 "$BIN" --bench "$MODEL" 64 128
# windowed decode wants flush pinned high so a window stays one submit:
run 22-base-window    LOGSCOPE_FUSED_QKV=1 LOGSCOPE_FLUSH_EVERY=100000 "$BIN" --bench "$MODEL" 64 128

# 3) q4-resident A/B (half the weight bytes — bandwidth + the 70B-fit lever; coherence cost cos~0.91).
run 23-base-q4        LOGSCOPE_Q4_RESIDENT=1 "$BIN" --bench "$MODEL" 64 128

# 4) batched-decode worker sweep — multi-session curve; watch for the saturation knee.
run 30-workers                              "$BIN" --workers "$MODEL" 16 48 512
run 31-workers-fusedqkv LOGSCOPE_FUSED_QKV=1 "$BIN" --workers "$MODEL" 16 48 512

# 5) continuous-batching scheduler, f32 vs int8 KV (q8 should fit ~4x the sessions).
run 40-coord-f32 "$BIN" --coordinator "$MODEL" 32 2048 512 64
run 41-coord-q8  "$BIN" --coordinator "$MODEL" 32 2048 512 64 q8

# 6) speculative decode (single-session latency lever) — only if a draft model was given.
if [ -n "$DRAFT" ] && [ -f "$DRAFT" ]; then
  run 50-spec "$BIN" --spec "$MODEL" "$DRAFT" 64 4 "Explain the bug and the fix."
fi

echo ">>> summary"; grep -hiE "tok/s|speedup|agg|ms/token|dispatch|accepted" "$LOGDIR"/*.log | tee "$LOGDIR/99-summary.txt"
echo; echo "done — share $LOGDIR/99-summary.txt for analysis."
