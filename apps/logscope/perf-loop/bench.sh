#!/usr/bin/env bash
# perf-loop/bench.sh — the headless runner. Runs ON the target (macOS/Metal or Asahi/Vulkan).
# One tag -> one JSON in $RESULTS_DIR plus raw logs. No ssh, no rsync, no opinions: it measures.
#
#   env: REPO, RESULTS_DIR, MODELS_DIR, BENCH_MODEL, BENCH_MODEL_SMALL, BENCH_ACTIVE_FRACTION,
#        BENCH_SHA, LLAMA_BENCH (optional), FORCE=1 (skip the busy check), LOGSCOPE_* knobs pass through.
set -uo pipefail
TAG="${1:?tag}"
REPO="${REPO:-$HOME/perf/repo}"; RESULTS_DIR="${RESULTS_DIR:-$HOME/perf/results}"; MODELS_DIR="${MODELS_DIR:-$HOME/models}"
REPO="${REPO/#\~/$HOME}"; RESULTS_DIR="${RESULTS_DIR/#\~/$HOME}"; MODELS_DIR="${MODELS_DIR/#\~/$HOME}"
BIN="$REPO/apps/logscope/crates/gpu-gemm/target/release/kernel-validate"
MODEL="${BENCH_MODEL:-}"; [ -n "$MODEL" ] && [ "${MODEL#/}" = "$MODEL" ] && MODEL="$MODELS_DIR/$MODEL"
ID="$(date +%Y%m%d-%H%M%S)-$TAG"; OUT="$RESULTS_DIR/$ID"; mkdir -p "$OUT/raw"

# 1. refuse to measure on a busy GPU (the number would be about the other process, not the change)
if [ "${FORCE:-0}" != "1" ] && pgrep -f 'llama-server|ollama serve|omlx|optiq serve|mlx_lm' >/dev/null; then
    echo "BUSY: local AI services are running; ./loop.sh quiet first (or FORCE=1)"; exit 2
fi
[ -x "$BIN" ] || { echo "no binary at $BIN — ./loop.sh build"; exit 3; }
# 1b. fresh autotune cache per run unless pinned: a stale packed DecodeConfig would hide a kernel change
export LOGSCOPE_CACHE_DIR="${LOGSCOPE_CACHE_DIR:-$OUT/cache}"; mkdir -p "$LOGSCOPE_CACHE_DIR"

run() { # run <name> <cmd...>  -> raw/<name>.log, timed
    local name="$1"; shift; local t0=$(date +%s)
    "$@" > "$OUT/raw/$name.log" 2>&1; local rc=$?
    printf '%s rc=%s %ss\n' "$name" "$rc" "$(( $(date +%s) - t0 ))"
}

run gate-perf   "$BIN" --gate-perf
run gate-matvec "$BIN" --gate-matvec        # correctness: tiled == untiled bit-exact
run gate-bytes  "$BIN" --gate-bytes         # correctness: lane-varying shifts (the Honeykrisp fault)
if [ -n "$MODEL" ] && [ -f "$MODEL" ]; then
    run bench-ctx8   "$BIN" --bench "$MODEL" 64 8
    run bench-ctx512 "$BIN" --bench "$MODEL" 64 512
    if [ -n "${LLAMA_BENCH:-}" ] && [ -x "$LLAMA_BENCH" ]; then
        run llama-bench "$LLAMA_BENCH" -m "$MODEL" -p 512 -n 128 -o json
    fi
else
    echo "no model (BENCH_MODEL='$MODEL'): kernel gates only"
fi

# 2. parse raw logs into one JSON (python3 ships with macOS CLT and every Linux)
MODEL="$MODEL" OUT="$OUT" ID="$ID" TAG="$TAG" python3 - << 'PY'
import json, os, re, platform, subprocess, time
out, ID, TAG, model = os.environ["OUT"], os.environ["ID"], os.environ["TAG"], os.environ["MODEL"]
raw = lambda n: open(f"{out}/raw/{n}.log").read() if os.path.exists(f"{out}/raw/{n}.log") else ""
r = {"id": ID, "tag": TAG, "time": time.strftime("%Y-%m-%dT%H:%M:%S"), "host": platform.node(),
     "os": platform.system(), "sha": os.environ.get("BENCH_SHA", "?"),
     "knobs": {k: v for k, v in os.environ.items() if k.startswith("LOGSCOPE_")},
     "model": os.path.basename(model) if model else None,
     "model_bytes": os.path.getsize(model) if model and os.path.exists(model) else None,
     "active_fraction": float(os.environ.get("BENCH_ACTIVE_FRACTION", "1.0"))}
# --gate-perf: "     <label> <ms> ms/call  <gibs> GiB/s"
r["matvec"] = {m[1].strip(): {"ms": float(m[2]), "gibs": float(m[3])}
               for m in re.finditer(r"^\s+(\S.*?)\s+([0-9.]+) ms/call\s+([0-9.]+) GiB/s", raw("gate-perf"), re.M)}
r["gates"] = {g: {"fail_lines": len(re.findall(r"FAIL", raw(g))), "ran": bool(raw(g))} for g in ("gate-matvec", "gate-bytes")}
def bench(name):
    t = raw(name)
    m = re.search(r"→ ([0-9.]+) tok/s \(([0-9.]+)ms/token\)", t)
    p = re.search(r"resource-setup ([0-9.]+)ms.*gpu-sync ([0-9.]+)ms.*other ([0-9.]+)ms", t)
    a = re.search(r"adapter = (.+)", t)
    if not m: return None
    return {"tok_s": float(m[1]), "ms_per_token": float(m[2]),
            "setup_ms": float(p[1]) if p else None, "sync_ms": float(p[2]) if p else None,
            "other_ms": float(p[3]) if p else None, "adapter": a[1].strip() if a else None}
r["decode_ctx8"], r["decode_ctx512"] = bench("bench-ctx8"), bench("bench-ctx512")
if r["model_bytes"] and r["decode_ctx512"]:
    active_gib = r["model_bytes"] * r["active_fraction"] / 2**30
    r["effective_gibs_ctx512"] = round(active_gib / (r["decode_ctx512"]["ms_per_token"] / 1000), 1)
    r["roofline_tok_s_745"] = round(745 / active_gib, 1)
lb = raw("llama-bench")
try:
    j = json.loads(lb[lb.index("["):]) if "[" in lb else []
    r["reference"] = {x["test"]: round(x["avg_ts"], 1) for x in j if "avg_ts" in x}
except Exception:
    r["reference"] = None
json.dump(r, open(f"{out}.json", "w"), indent=1)
print(f"wrote {out}.json")
d = r["decode_ctx512"]
print(f"  decode@512: {d['tok_s']} tok/s  sync {d['sync_ms']} ms/tok" if d else "  (no model run)")
PY
