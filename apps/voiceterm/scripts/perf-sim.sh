#!/usr/bin/env bash
# perf-sim.sh — model the terminal publish→RichText-reparse decision and
# measure how many EXPENSIVE re-parses fire over realistic frame sequences,
# comparing dedup strategies. Built to diagnose the v670 report of idle
# re-parse churn (profile logs showed html oscillating 2540<->2536 every
# frame while 'idle'). PURE simulation — no GUI needed.
#
# Findings baked into the scenarios:
#  - pure cursor blink: v628 HTML-equality dedup ALREADY handles it (~1).
#  - statusline/content oscillation: equality dedup AND the cursor classifier
#    both fail (every frame genuinely differs) -> needs normalization (if
#    cosmetic) or idle-coalescing (if a real field animates).
# USAGE: scripts/perf-sim.sh
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
command -v rustc >/dev/null || { echo "rustc not found"; exit 0; }
for f in perf_sim_reparse perf_sim_dedup; do
  rustc -O --edition 2021 "$DIR/$f.rs" -o "/tmp/$f" 2>/dev/null && "/tmp/$f"; echo
done
echo "To diagnose YOUR idle churn: run the app with WEB2QML_PROFILE=1, grab two"
echo "consecutive 'republish' frames while idle, and diff their html. Cosmetic"
echo "delta -> normalized dedup; real animating field -> idle-republish coalesce."
