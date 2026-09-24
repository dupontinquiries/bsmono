#!/usr/bin/env python3
"""perf-loop/compare.py RESULTS_DIR [base] [new]
Prints a delta table between two runs (by tag substring or file name; defaults: 'baseline' vs newest)
and a KEEP/DISCARD verdict. Pure reader: it never runs anything."""
import glob, json, os, sys

def load(results, key=None, exclude=None):
    files = sorted(glob.glob(os.path.join(results, "*.json")), key=os.path.getmtime)
    if key:
        files = [f for f in files if key in os.path.basename(f)]
    if exclude:
        files = [f for f in files if f != exclude]
    if not files:
        sys.exit(f"no result matching {key!r} in {results}")
    return files[-1], json.load(open(files[-1]))

results = sys.argv[1]
bf, b = load(results, sys.argv[2] if len(sys.argv) > 2 else "baseline")
nf, n = load(results, sys.argv[3] if len(sys.argv) > 3 else None, exclude=bf)

def g(d, *ks):
    for k in ks:
        d = d.get(k) if isinstance(d, dict) else None
    return d

def row(label, bv, nv, higher_is_better=True, unit=""):
    if bv is None or nv is None:
        print(f"  {label:28s} {str(bv):>10s} {str(nv):>10s}"); return None
    delta = (nv - bv) / bv * 100 if bv else 0.0
    sign = "+" if delta >= 0 else ""
    good = (delta > 0) == higher_is_better
    print(f"  {label:28s} {bv:10.2f} {nv:10.2f}  {sign}{delta:6.1f}% {'✓' if good else '✗'} {unit}")
    return delta if higher_is_better else -delta

print(f"base: {os.path.basename(bf)}  sha={b.get('sha')}  knobs={b.get('knobs')}")
print(f"new : {os.path.basename(nf)}  sha={n.get('sha')}  knobs={n.get('knobs')}")
print(f"model: {n.get('model')}  os={n.get('os')}  adapter={g(n,'decode_ctx512','adapter')}")
print(f"  {'metric':28s} {'base':>10s} {'new':>10s}")
key = row("decode tok/s @ctx512", g(b,"decode_ctx512","tok_s"), g(n,"decode_ctx512","tok_s"))
row("decode tok/s @ctx8", g(b,"decode_ctx8","tok_s"), g(n,"decode_ctx8","tok_s"))
row("gpu-sync ms/tok @512", g(b,"decode_ctx512","sync_ms"), g(n,"decode_ctx512","sync_ms"), False)
row("setup ms/tok @512", g(b,"decode_ctx512","setup_ms"), g(n,"decode_ctx512","setup_ms"), False)
row("effective GiB/s @512", b.get("effective_gibs_ctx512"), n.get("effective_gibs_ctx512"))
for lab in sorted(set(b.get("matvec", {})) | set(n.get("matvec", {}))):
    row(f"matvec {lab[:21]} GiB/s", g(b,"matvec",lab,"gibs"), g(n,"matvec",lab,"gibs"))
ref = n.get("reference") or {}
if ref:
    print(f"  reference llama-bench: {ref}   engine/ref tg = "
          f"{(g(n,'decode_ctx512','tok_s') or 0) / max(ref.get('tg128', 1e-9), 1e-9):.2f}x")
if n.get("roofline_tok_s_745"):
    print(f"  roofline @745 GiB/s: {n['roofline_tok_s_745']} tok/s  (engine at "
          f"{100 * (g(n,'decode_ctx512','tok_s') or 0) / n['roofline_tok_s_745']:.1f}% of it)")
fails = {k: v.get("fail_lines") for k, v in (n.get("gates") or {}).items()}
base_fails = {k: v.get("fail_lines") for k, v in (b.get("gates") or {}).items()}
new_fail = any((fails.get(k) or 0) > (base_fails.get(k) or 0) for k in fails)
verdict = "DISCARD (new gate failures)" if new_fail else \
          "KEEP" if key is not None and key >= 3.0 else \
          "NOISE (within ±3%)" if key is not None and abs(key) < 3.0 else \
          "DISCARD (slower)" if key is not None else "NO DECODE DATA"
print(f"verdict: {verdict}   gates: {fails}")
