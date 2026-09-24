#!/usr/bin/env bash
# perf-loop/loop.sh — the driver. Runs on the laptop; drives the M2 Ultra over ssh/rsync.
# Every subcommand is one visible step; nothing here is clever. See PERF_LOOP.md for the protocol.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
LOGSCOPE_DIR="$(cd "$HERE/.." && pwd)"          # apps/logscope
MONO_DIR="$(cd "$LOGSCOPE_DIR/../.." && pwd)"   # monorepo root
[ -f "$HERE/hosts.env" ] || { echo "copy hosts.env.example to hosts.env and edit it"; exit 1; }
# shellcheck disable=SC1091
source "$HERE/hosts.env"
R() { ssh -o BatchMode=yes "$ULTRA_HOST" "$@"; }   # remote exec; ~ expands on the remote side

code_id() { # short sha + -dirty, computed on the laptop (the target has no .git)
    local sha; sha="$(git -C "$MONO_DIR" rev-parse --short HEAD 2>/dev/null || echo nogit)"
    git -C "$MONO_DIR" diff --quiet 2>/dev/null || sha="$sha-dirty"
    echo "$sha"
}

cmd_status() {
    R "uname -srm; sw_vers 2>/dev/null | head -2; uptime; echo '-- gpu users --'; pgrep -fl 'llama-server|ollama|omlx|optiq|mlx_lm|kernel-validate' || echo none"
}

cmd_sync() { # working tree only: no target/, no .git, no results
    R "mkdir -p $ULTRA_REPO $ULTRA_RESULTS $ULTRA_SHADERS"
    rsync -az --delete \
        --exclude 'target/' --exclude '.git/' --exclude 'perf-loop/results/' \
        "$MONO_DIR/" "$ULTRA_HOST:$ULTRA_REPO/"
    echo "synced $(code_id) -> $ULTRA_HOST:$ULTRA_REPO"
}

cmd_build() {
    R "cd $ULTRA_REPO/apps/logscope/crates/gpu-gemm && cargo build --release --bins --message-format=short 2>&1 | grep -v '^warning' | tail -20"
}

cmd_quiet() { R "bash $ULTRA_REPO/apps/logscope/perf-loop/services.sh stop"; }
cmd_loud()  { R "bash $ULTRA_REPO/apps/logscope/perf-loop/services.sh start"; }

# bench <tag> [KEY=VAL ...]   — KEY=VAL are LOGSCOPE_* knobs (or BENCH_MODEL=..., SHADER_DIR=1)
cmd_bench() {
    local tag="${1:?tag}"; shift
    local envs="BENCH_SHA=$(code_id) BENCH_MODEL=$BENCH_MODEL BENCH_MODEL_SMALL=$BENCH_MODEL_SMALL BENCH_ACTIVE_FRACTION=$BENCH_ACTIVE_FRACTION LLAMA_BENCH=${LLAMA_BENCH:-} RESULTS_DIR=$ULTRA_RESULTS MODELS_DIR=$ULTRA_MODELS REPO=$ULTRA_REPO"
    for kv in "$@"; do
        case "$kv" in
            SHADER_DIR=1) envs="$envs LOGSCOPE_SHADER_DIR=$ULTRA_SHADERS" ;;
            *) envs="$envs $kv" ;;
        esac
    done
    R "env $envs bash $ULTRA_REPO/apps/logscope/perf-loop/bench.sh $tag"
    rsync -az "$ULTRA_HOST:$ULTRA_RESULTS/" "$HERE/results/"
    echo "results pulled to $HERE/results/ (latest: $(ls -t "$HERE/results"/*.json | head -1))"
}

# shaders <tag> [KEY=VAL ...] — push perf-loop/shaders/*.wgsl and bench with the overrides, no rebuild
cmd_shaders() {
    local tag="${1:?tag}"; shift
    rsync -az --delete "$HERE/shaders/" "$ULTRA_HOST:$ULTRA_SHADERS/"
    cmd_bench "$tag" SHADER_DIR=1 "$@"
}

cmd_compare() { python3 "$HERE/compare.py" "$HERE/results" "$@"; }

cmd_smoke() { cmd_bench "${1:-smoke}" "BENCH_MODEL=$BENCH_MODEL_SMALL" "BENCH_ACTIVE_FRACTION=1.0"; }

cmd_watch() { # re-bench on every shader save; needs `brew install fswatch`
    command -v fswatch >/dev/null || { echo "brew install fswatch"; exit 1; }
    fswatch -o "$HERE/shaders" | while read -r _; do cmd_shaders "watch-$(date +%H%M%S)"; done
}

case "${1:-}" in
    status|sync|build|quiet|loud|bench|shaders|compare|smoke|watch) c="$1"; shift; "cmd_$c" "$@" ;;
    *) sed -n '2,3p' "$0"; echo "usage: loop.sh {status|sync|build|quiet|loud|smoke|bench <tag> [K=V..]|shaders <tag> [K=V..]|compare [base] [new]|watch}"; exit 1 ;;
esac
