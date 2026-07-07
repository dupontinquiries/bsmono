#!/usr/bin/env bash
# validate_shaders.sh — compile every GLSL shader to SPIR-V and validate it.
#
# Catches shader bugs without a GPU: glslangValidator compiles the GLSL (syntax
# + type checking against the Vulkan profile), then spirv-val checks the
# resulting SPIR-V against the spec (stricter — catches valid-GLSL-but-invalid-
# SPIR-V). Pure verification: it does NOT touch the committed assets/shaders/*.spv
# (those are version-managed by build.rs; recompiling with a different glslang
# version produces a byte-different-but-equivalent module, which is expected and
# is NOT staleness).
#
# Usage (from repo root):  bash tools/validate_shaders.sh
# Exit 0 = all shaders compile + validate; non-zero = at least one failed.
#
# Requires glslangValidator + spirv-val. On Debian/Ubuntu:
#   apt-get install -y glslang-tools   # provides both
set -u

SHADER_DIR="crates/unendlich-engine/src/shaders"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if ! command -v glslangValidator >/dev/null 2>&1; then
  echo "glslangValidator not found (apt-get install glslang-tools). Skipping." >&2
  exit 0   # graceful: the build ships precompiled SPIR-V, so absence isn't fatal
fi
HAVE_VAL=1
command -v spirv-val >/dev/null 2>&1 || HAVE_VAL=0

pass=0; fail=0; valfail=0
for f in "$SHADER_DIR"/*.vert "$SHADER_DIR"/*.frag "$SHADER_DIR"/*.comp; do
  [ -e "$f" ] || continue
  base="$(basename "$f")"
  if ! glslangValidator -V "$f" -o "$TMP/out.spv" >"$TMP/glslang.log" 2>&1; then
    fail=$((fail+1))
    echo "COMPILE FAIL: $base"
    grep -iE "error" "$TMP/glslang.log" | head -5 | sed 's/^/    /'
    continue
  fi
  if [ "$HAVE_VAL" -eq 1 ]; then
    if ! spirv-val "$TMP/out.spv" >"$TMP/val.log" 2>&1; then
      valfail=$((valfail+1))
      echo "SPIRV-VAL FAIL: $base"
      head -5 "$TMP/val.log" | sed 's/^/    /'
      continue
    fi
  fi
  pass=$((pass+1))
done

echo "----------------------------------------"
echo "shaders OK: $pass   compile-fail: $fail   spirv-val-fail: $valfail"
[ "$HAVE_VAL" -eq 1 ] || echo "(spirv-val absent — compile-only; install glslang-tools for full validation)"
if [ $((fail + valfail)) -gt 0 ]; then
  exit 1
fi
exit 0
