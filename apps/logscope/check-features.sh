#!/usr/bin/env bash
# check-features.sh — build the feature combinations the features table advertises.
#
# WHY THIS EXISTS. `--no-default-features` is documented in logscope-egui's
# Cargo.toml as the lean build ("off lets logscope-cli build lean, no
# eframe/vello"), and it did not compile. Nothing built it, so nothing noticed:
# three std-only modules had drifted behind `#[cfg(feature = "gui")]`, and
# `hud_pack` — which speaks egui — had drifted out from behind it.
#
# A feature flag nobody builds is a feature flag that does not work. This is the
# cheapest thing that keeps the claim honest.
#
# It also matters for iterating in a small container: the lean build's target
# tree is ~1.2 GiB against ~2.9 GiB for the default, and it runs 483 of the 700
# tests in under a second. Most logic work does not need eframe.
#
#   ./check-features.sh          # compile every advertised combination
#   ./check-features.sh test     # and run the suites that can run
#
# Set CARGO_TARGET_DIR outside the repo on a tight disk, and clean between runs.
set -uo pipefail
cd "$(dirname "$0")"

MODE="${1:-build}"
FAIL=0

# ONE TARGET TREE AT A TIME. These combinations do not share artifacts — the
# default build pulls eframe, winit, wayland, vello and tree-sitter that the lean
# one never sees — so leaving all three in place needs roughly 6 GiB. Running
# this on a small disk without cleaning between them fails on the LAST
# combination with "No space left on device" inside a linker invocation, which
# reads like a code error and is not one. Cleaning between runs costs rebuild
# time and buys an answer you can trust.
#
# Set KEEP_TARGETS=1 when disk is plentiful and you would rather have the cache.
TARGET_ROOT="${CARGO_TARGET_DIR:-$PWD/target}"
reclaim() {
    [ -n "${KEEP_TARGETS:-}" ] && return 0
    rm -rf "$TARGET_ROOT"
}

# The heavy optional backends (voice, llamacpp, wgpu-engine) are deliberately NOT
# here. They pull whisper/cpal, GGML and gpu-gemm respectively, take far longer
# than everything else combined, and need hardware this check cannot assume. They
# are exercised on the machines that have that hardware; what this guards is that
# the combinations a developer picks on a laptop keep working.
run() {
    local label="$1"; shift
    printf '%-34s ' "$label"
    reclaim
    if out=$(cargo "$MODE" -p logscope-egui "$@" </dev/null 2>&1); then
        # Surface the count when testing, so a suite that silently stops running
        # is visible rather than just green.
        echo "ok $(echo "$out" | grep -oE 'test result: ok\. [0-9]+ passed' | head -1)"
    else
        echo "FAILED"
        echo "$out" | grep -E '^error' -A 4 | head -12 | sed 's/^/    /'
        FAIL=1
    fi
}

run "lean (no default features)"  --no-default-features
run "gui only (CPU scene path)"   --no-default-features --features gui
run "default (gui + gpu-scene)"
reclaim

exit $FAIL
