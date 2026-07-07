#!/usr/bin/env bash
# run.sh — sandbox-tier harness. Run from repo root (unendlich/). Aggregates
# every check that does NOT need the full tree, per harness/RULESET.md.
# Integration-tier checks (cargo build/test, perf_check on a run log) are run by
# the user. ubo_parity is best-effort here: a skip (files not at these paths in
# your tree) is not a failure.
set -uo pipefail
H="$(cd "$(dirname "$0")" && pwd)"; HO="$H/../handoff"; SRC=crates/unendlich-engine/src
PURE=("$SRC/moisture_field.rs" "$SRC/debug_settings.rs")
CHANGED=("$@"); [ ${#CHANGED[@]} -eq 0 ] && CHANGED=("${PURE[@]}")
rc=0
echo "== R8 delimiter balance =="
python3 "$HO/check_delimiters.py" "${CHANGED[@]}" || rc=1
echo; echo "== R1 purity (no render/Vulkan imports) =="
python3 "$H/coupling_check.py" "${PURE[@]}" || rc=1
echo; echo "== R11/R21 hygiene (determinism, no hidden panics) =="
python3 "$H/hygiene_check.py" "${PURE[@]}" || rc=1
echo; echo "== R6/R14/R15 pure-module unit + property tests (rustc) =="
bash "$H/pure_test.sh" "${PURE[@]}" || rc=1
echo; echo "== R1b render-cfg typecheck (catches feature-gated breakage, e.g. a render-only helper miss) =="
for f in "${PURE[@]}"; do
  if rustc --edition 2021 --crate-type lib --cfg 'feature="render"' "$f" -o /tmp/_rcfg.out 2>/tmp/_rcfg.err; then
    echo "  RENDER-OK    $f"
  else
    echo "  RENDER-FAIL  $f"; sed 's/^/      /' /tmp/_rcfg.err | head -6; rc=1
  fi
done
rm -f /tmp/_rcfg.out /tmp/_rcfg.err

echo; echo "== R4 octree winding guard =="
python3 "$HO/check_octree_winding.py" >/dev/null 2>&1 && echo "  winding OK" || echo "  winding guard: see handoff/check_octree_winding.py"
echo; echo "== R16 CameraUbo Rust<->GLSL parity (best-effort) =="
python3 "$H/ubo_parity.py" "$SRC/render/world_renderer.rs" "$SRC/shaders/chunk.frag" CameraUbo
pr=$?; [ $pr -eq 1 ] && rc=1 || { [ $pr -eq 2 ] && echo "  (skipped: paths differ in your tree — run on the right files)"; }
echo; echo "== R23 GPU shader gate (sandbox-tier; best-effort) =="
gco=$(bash "$H/gpu_check.sh" 2>&1); echo "$gco" | tail -2 | sed 's/^/  /'
echo "$gco" | grep -q "GPU GATE RED" && rc=1
echo; echo "== R32 shader<->Rust descriptor-layout drift (best-effort) =="
python3 "$H/descriptor_gate.py" 2>&1 | tail -6 | sed 's/^/  /'
echo; [ $rc -eq 0 ] && echo "HARNESS GREEN" || echo "HARNESS RED"
exit $rc
