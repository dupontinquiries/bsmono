#!/usr/bin/env bash
# gpu_check.sh — SANDBOX-TIER GPU shader gate (see harness/RULESET.md "GPU work").
#
# Compiles every engine shader STAGE (.comp/.vert/.frag/.geom/.tesc/.tese) to
# SPIR-V with glslangValidator (resolving #includes via -I) and validates the
# result with spirv-val. Catches the bug class — a shader that won't compile or
# emits invalid SPIR-V — WITHOUT a GPU or even a Vulkan loader, so it runs in the
# sandbox (only needs `apt-get install glslang-tools spirv-tools`).
#
# This is the GPU analogue of pure_test.sh for pure modules: it lets GPU/shader
# work be iterated here. It does NOT prove numeric behavior — that is
# INTEGRATION-TIER: vk-harness runs the real shader on lavapipe and diffs it vs a
# CPU reference (e.g. tests/mesh_parity.rs vs mesh_chunk_octree_cpu_reference).
# Per RULESET, every GPU compute kernel must ship such a CPU reference + parity test.
#
# Usage: bash harness/gpu_check.sh [shaders_dir]
#   default shaders_dir = crates/unendlich-engine/src/shaders
set -u
SH="${1:-crates/unendlich-engine/src/shaders}"
if ! command -v glslangValidator >/dev/null 2>&1; then
  echo "SKIP: glslangValidator absent (apt-get install glslang-tools) — gate not run"
  exit 0
fi
HAVE_VAL=0; command -v spirv-val >/dev/null 2>&1 && HAVE_VAL=1
[ "$HAVE_VAL" = 1 ] || echo "note: spirv-val absent (apt-get install spirv-tools) — compile-only"
inc="-I$SH"
tmp="$(mktemp -d)"; out="$tmp/s.spv"
rc=0; ok=0; n=0
shopt -s nullglob
for f in "$SH"/*.comp "$SH"/*.vert "$SH"/*.frag "$SH"/*.geom "$SH"/*.tesc "$SH"/*.tese; do
  n=$((n+1)); name=$(basename "$f")
  if ! err=$(glslangValidator -V "$f" "$inc" -o "$out" 2>&1); then
    echo "  FAIL  $name  (glslang)"
    printf '%s\n' "$err" | grep -viE "^($|$name\$|SPIR-V)" | sed 's/^/        /' | head -5
    rc=1; continue
  fi
  if [ "$HAVE_VAL" = 1 ] && ! verr=$(spirv-val "$out" 2>&1); then
    echo "  FAIL  $name  (spirv-val)"
    printf '%s\n' "$verr" | sed 's/^/        /' | head -5
    rc=1; continue
  fi
  echo "  OK    $name"; ok=$((ok+1))
done
rm -rf "$tmp"
echo
echo "shaders: $ok/$n compile + validate"
if [ $rc -eq 0 ]; then echo "GPU GATE GREEN"; else echo "GPU GATE RED"; fi
exit $rc
