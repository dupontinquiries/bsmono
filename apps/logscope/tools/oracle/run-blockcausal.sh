#!/bin/sh
# run-blockcausal.sh — verify the engine's BLOCK-CAUSAL attention against a reference that models
# the same mask. Until the reference learned the mask, both sides truncated and the oracle was
# structurally blind to this difference; this closes that.
# Covers ALIGNED and UNALIGNED prompts, since the alignment bug (blocks anchored at position 0
# instead of the fill start) only appeared when n_ctx % bcz != 0 — the common case on real prompts.
set -e
D=$(cd "$(dirname "$0")" && pwd)
M=${1:-/tmp/tiny-llada2.gguf}
python3 - "$D" "$M" <<'PY'
import importlib.util, numpy as np, subprocess, os, sys
D, M = sys.argv[1], sys.argv[2]
spec=importlib.util.spec_from_file_location('r', f'{D}/llada2_ref.py')
r=importlib.util.module_from_spec(spec); spec.loader.exec_module(r)
b,kv,tens,data = r.read_gguf(M)
mask = kv.get('tokenizer.ggml.mask_token_id')
if mask is None: print("SKIP: model has no mask id"); sys.exit(0)
gpu = f'{D}/../../crates/gpu-gemm/target/release/denoise-dump'
fail = 0
for (n_ctx, bcz, nfill) in [(8,4,8),(10,4,8),(12,4,12),(7,8,16),(13,8,16)]:
    prompt=[1+(i*13)%200 for i in range(n_ctx)]
    ids = prompt + [mask]*nfill
    lg = r.forward(b,kv,tens,data,ids,bcz,n_ctx)
    env=dict(os.environ); env['LOGSCOPE_IDS']=",".join(map(str,ids)); env['LOGSCOPE_BC']=f"{bcz},{n_ctx}"
    out = subprocess.run([gpu, M, str(len(ids))], capture_output=True, text=True, env=env).stdout.strip().splitlines()
    if len(out) != len(ids):
        print(f"FAIL n_ctx={n_ctx} bcz={bcz}: engine returned {len(out)} rows for {len(ids)} tokens"); fail=1; continue
    bad=0; worst=0.0
    for i,line in enumerate(out):
        g=line.split(); ga=int(g[0]); ra=int(np.argmax(lg[i]))
        z=lg[i]-lg[i].max(); p=np.exp(z); p/=p.sum()
        if ga!=ra: bad+=1
        worst=max(worst, abs(float(g[1])-p.max())/max(p.max(),1e-9))
    ok = bad==0 and worst < 1e-5
    aligned = "aligned" if n_ctx % bcz == 0 else "UNALIGNED"
    print(f"{'PASS' if ok else 'FAIL'} n_ctx={n_ctx} bcz={bcz} ({aligned}): {len(out)-bad}/{len(out)} argmax, p_top rel {worst:.2e}")
    if not ok: fail=1
sys.exit(fail)
PY
