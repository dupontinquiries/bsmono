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
    out = {"tok_s": float(m[1]), "ms_per_token": float(m[2]),
           "setup_ms": float(p[1]) if p else None, "sync_ms": float(p[2]) if p else None,
           "other_ms": float(p[3]) if p else None, "adapter": a[1].strip() if a else None}
    # Counted per-token commands (the bench used to print a fixed "~210 dispatches" string).
    c = re.search(r"per token \(counted\): ([0-9.]+) dispatches \| ([0-9.]+) passes \| ([0-9.]+) submits \| ([0-9.]+) copies \| ([0-9.]+) polls", t)
    if c: out.update(dispatches=float(c[1]), passes=float(c[2]), submits=float(c[3]), copies=float(c[4]), polls=float(c[5]))
    # Bandwidth over the bytes the ENGINE reads (int8/q4 resident), not the GGUF file size.
    w = re.search(r"weights read per token: ([0-9.]+) (GB|MB) \((\w+)-resident.*?→ ([0-9.]+) GB/s effective over wall; ([0-9.]+) GB/s over gpu-sync", t)
    if w:
        out.update(weight_gb_per_token=float(w[1]) * (1 if w[2] == "GB" else 1e-3), resident_kind=w[3],
                   gbs_over_wall=float(w[4]), gbs_over_sync=float(w[5]))
    pf = re.search(r"prefill: (\d+) tokens in chunks of (\d+) → ([0-9.]+) tok/s \(([0-9.]+)ms/token\)", t)
    if pf: out.update(prefill_tokens=int(pf[1]), prefill_chunk=int(pf[2]), prefill_tok_s=float(pf[3]), prefill_ms_per_token=float(pf[4]))
    # Per-kernel GPU time table (LOGSCOPE_GPU_TIMESTAMPS=1): [(kernel, ms/tok, disp/tok, %gpu)]
    k = re.search(r"decode per-kernel: GPU time inside passes ([0-9.]+)ms per tok.*?\n(.*?)(?=\n  [^ ]|\Z)", t, re.S)
    if k:
        rows = re.findall(r"^\s+([0-9.]+)\s+([0-9.]+)\s+([0-9.]+)%\s+([0-9.]+)\s+(\S.*)$", k[2], re.M)
        out["gpu_ms_inside_passes"] = float(k[1])
        out["kernels"] = [{"kernel": r5.strip(), "ms_per_token": float(r1), "dispatches": float(r2), "pct": float(r3), "us_per_dispatch": float(r4)} for r1, r2, r3, r4, r5 in rows]
    return out
r["decode_ctx8"], r["decode_ctx512"] = bench("bench-ctx8"), bench("bench-ctx512")
# PEAK: this is an M2 MAX (~400 GB/s), not the M2 Ultra (~800) the 745 figure was written for —
# docs/agents/APPLE_M2_GPU_STACK.md. BENCH_PEAK_GBS overrides for another part.
peak = float(os.environ.get("BENCH_PEAK_GBS", "400"))
r["peak_gbs"] = peak
if r["model_bytes"] and r["decode_ctx512"]:
    active_gib = r["model_bytes"] * r["active_fraction"] / 2**30
    r["effective_gibs_ctx512_file"] = round(active_gib / (r["decode_ctx512"]["ms_per_token"] / 1000), 1)   # file-size denominator (legacy)
    r["roofline_tok_s_745"] = round(745 / active_gib, 1)   # legacy column, kept so old rows compare
    d = r["decode_ctx512"]
    if d.get("weight_gb_per_token"):
        r["roofline_tok_s_peak"] = round(peak / d["weight_gb_per_token"], 1)
        r["pct_of_peak"] = round(100 * d["gbs_over_wall"] / peak, 1)
lb = raw("llama-bench")
try:
    j = json.loads(lb[lb.index("["):]) if "[" in lb else []
    r["reference"] = {x["test"]: round(x["avg_ts"], 1) for x in j if "avg_ts" in x}
except Exception:
    r["reference"] = None
json.dump(r, open(f"{out}.json", "w"), indent=1)
print(f"wrote {out}.json")
d = r["decode_ctx512"]
if d:
    print(f"  decode@512: {d['tok_s']} tok/s  sync {d['sync_ms']} ms/tok  | {d.get('dispatches', '?')} dispatches/tok, {d.get('submits', '?')} submits/tok"
          + (f" | {d['gbs_over_wall']} GB/s over {d['weight_gb_per_token']} GB/tok ({r.get('pct_of_peak', '?')}% of {peak:.0f})" if d.get('weight_gb_per_token') else "")
          + (f" | prefill {d['prefill_tok_s']} tok/s" if d.get('prefill_tok_s') else ""))
    if d.get("kernels"):
        print("  top kernels (GPU ms/tok): " + ", ".join(f"{k['kernel']} {k['ms_per_token']}" for k in d["kernels"][:5]))
else:
    print("  (no model run)")
PY
