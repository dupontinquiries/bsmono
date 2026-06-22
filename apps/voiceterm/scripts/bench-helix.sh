#!/usr/bin/env bash
# bench-helix.sh — measure REAL helix startup, decomposed.
#
# NOTE (v0.1.651): an earlier version used `script(1)` to open a TUI buffer
# and time it. That was WRONG — `script` adds a fixed ~2s teardown delay
# (measured: `script -qec true` alone ~2015ms), swamping the real startup
# (tens of ms). This version uses helix's clean-exit modes for reliable
# numbers + hyperfine if present.
#
# Measured reference (sandbox x86, NOT your M2 — proportions should hold):
#   hx --version      ~4.7 ms   (binary load + core init)
#   hx --health rust  ~15.9 ms  (+ one language's grammar + queries)
#   hx --health (all) ~31.8 ms  (+ scanning all ~250 languages)
#
# Usage:  ./bench-helix.sh [lang]    (default rust);  HX=hx N=20 ./bench-helix.sh
set -euo pipefail
HX="${HX:-hx}"; N="${N:-20}"; LANG_ID="${1:-rust}"
command -v "$HX" >/dev/null 2>&1 || { echo "helix ('$HX') not on PATH — set HX=..."; exit 1; }
echo "helix: $("$HX" --version 2>/dev/null | head -1)"; echo "trials: $N"; echo

timeloop() { local label="$1"; shift; local total=0 s e
  for _ in $(seq "$N"); do s=$(date +%s.%N); timeout 5 "$@" >/dev/null 2>&1 || true; e=$(date +%s.%N)
    total=$(awk "BEGIN{print $total + ($e - $s)}"); done
  awk "BEGIN{printf \"  %-26s mean %.1f ms\n\", \"$label\", ($total/$N)*1000}"; }

if command -v hyperfine >/dev/null 2>&1; then
  echo "## hyperfine (warms up; reports min/mean/max)"
  hyperfine -N -w 5 -r "$N" \
    -n "version (init)"            "$HX --version" \
    -n "health $LANG_ID (grammar)" "$HX --health $LANG_ID" \
    -n "health all (full scan)"    "$HX --health"
else
  echo "## time loop (install hyperfine for better numbers)"
  timeloop "version (init)"            "$HX" --version
  timeloop "health $LANG_ID (grammar)" "$HX" --health "$LANG_ID"
  timeloop "health all (full scan)"    "$HX" --health
fi

echo
echo "Interpretation:"
echo "  version                         → core binary+init floor."
echo "  health <lang> minus version     → that language's grammar+query load."
echo "  health all minus health <lang>  → all-languages scan overhead."
echo "  NOT captured: LSP spawn (only on a real buffer open). To measure on"
echo "  the M2: 'time hx file.rs' (quit :q!), then again with the language"
echo "  server disabled in languages.toml; the delta is the LSP cost — usually"
echo "  the dominant felt-latency term. Paste results back."
