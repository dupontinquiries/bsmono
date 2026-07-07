#!/usr/bin/env bash
#
# unendlich test harness — orchestrator.
#
# Runs the full validation chain. The static layer (validate.py) runs always
# and is what the assistant runs in its sandbox. The real layer (glsl compile +
# cargo tests) runs whatever real tools are installed — that's the authoritative
# validation on a dev machine.
#
#   bash toolkit/harness/run.sh           # static + real GLSL compile + cargo tests
#   bash toolkit/harness/run.sh --quick   # static + real GLSL compile only (no cargo)
#   bash toolkit/harness/run.sh --static  # static layer only (what the assistant runs)
#
# Exit code: non-zero if any stage fails. Suitable for CI / pre-push hook.

set -uo pipefail   # pipefail: a failed `cargo` isn't hidden behind `| tail`
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
MODE="${1:-full}"
FAIL=0

step() { printf '\n\033[1m=== %s ===\033[0m\n' "$1"; }

# ── 1. static layer (always) ─────────────────────────────────────────────
step "static validator (validate.py)"
python3 "$HERE/validate.py" --repo "$REPO" --real || FAIL=1
[ "$MODE" = "--static" ] && { echo; [ $FAIL -eq 0 ] && echo "STATIC OK" || echo "STATIC FAILED"; exit $FAIL; }

# ── 2. real GLSL compile via the same path build.rs uses ──────────────────
# validate.py --real already shells out to glslc/glslangValidator if present;
# nothing more to do here. (Kept as a named step for clarity in logs.)

# ── 3. cargo tests — the authoritative suite ──────────────────────────────
# NOTE: `cargo test` accepts only ONE positional filter, so each integration
# test is selected by its file with `--test <name>`, and the engine lib tests
# run as one suite (covers render::mesher, render_parity, shader_contract, …).
if [ "$MODE" != "--quick" ]; then
  if command -v cargo >/dev/null 2>&1; then
    for t in shaders_compile mesh_parity octree_parity; do
      step "cargo: vk-harness --test $t"
      cargo test -p vk-harness --test "$t" 2>&1 | tail -25 || FAIL=1
    done

    step "cargo: engine lib unit tests --features render (winding, shader_contract, render_parity, …)"
    cargo test -p unendlich-engine --lib --features render 2>&1 | tail -30 || FAIL=1

    step "cargo: SSR cpu-reference tests (when present)"
    ssr_found=0
    for f in "$REPO"/crates/vk-harness/tests/ssr_*.rs; do
      [ -e "$f" ] || continue
      ssr_found=1
      t="$(basename "$f" .rs)"
      cargo test -p vk-harness --test "$t" 2>&1 | tail -20 || FAIL=1
    done
    [ "$ssr_found" = "0" ] && echo "  (no ssr_*.rs yet — added with the SSR increments)"
  else
    step "cargo (skipped: not installed)"
    echo "  Install Rust to run the authoritative suite:"
    echo "    cargo test -p vk-harness --test shaders_compile"
    echo "    cargo test -p vk-harness --test mesh_parity"
    echo "    cargo test -p vk-harness --test octree_parity"
    echo "    cargo test -p unendlich-engine --lib --features render"
  fi
fi

echo
if [ $FAIL -eq 0 ]; then echo -e "\033[32mHARNESS OK\033[0m"; else echo -e "\033[31mHARNESS FAILED\033[0m"; fi
exit $FAIL
