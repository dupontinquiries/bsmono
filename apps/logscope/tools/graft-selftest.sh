#!/bin/sh
# graft-selftest — reproduce the in-sandbox proof chain for gguf-shexp-graft.py on this machine
# before trusting it with a real checkpoint. Needs: python3+numpy, tools/make-tiny-gguf.py, and a
# built probe-bidir (pass its path as $1, default target/release under crates/gpu-gemm).
# Proof stages: (1) f32 graft is BIT-EXACT — a defective tiny grafted from a synthetic HF checkpoint
# must produce byte-identical engine output to a natively-complete tiny; (2) q4_0 graft loads with
# zero audit defects and a live forward; (3) the qkv verdict discriminates raw vs permuted layouts
# in BOTH directions. The Q4_0 quantizer itself was proven bit-identical to ggml's
# quantize_row_q4_0_ref (four trials incl. zero-block and signed-max) — that stage needs the ggml
# tree and is documented in the c4-731 commit rather than repeated here.
set -e
cd "$(dirname "$0")/.."
PROBE="${1:-crates/gpu-gemm/target/release/probe-bidir}"
[ -x "$PROBE" ] || { echo "probe-bidir not found at $PROBE"; exit 1; }
T=$(mktemp -d)
python3 tools/make-tiny-gguf.py "$T/native.gguf" llada2 >/dev/null
python3 tools/make-tiny-gguf.py "$T/defective.gguf" llada2 noshexp >/dev/null
python3 tools/make-tiny-gguf.py "$T/fusedqkv.gguf" llada2 alldense >/dev/null
python3 - "$T" <<'PY'
import sys, importlib.util, numpy as np, struct, json, os
T=sys.argv[1]
spec=importlib.util.spec_from_file_location("g","tools/gguf-shexp-graft.py")
g=importlib.util.module_from_spec(spec); spec.loader.exec_module(g)
src,_,tens,ds,_,_,kvs=g.read_gguf(f"{T}/native.gguf")
os.makedirs(f"{T}/hf",exist_ok=True); e={}; body=bytearray(); off=0
for t in tens:
    if 'shexp' not in t["name"]: continue
    raw=src[ds+t["offset"]:ds+t["offset"]+g.tensor_nbytes(t)]
    n=int(t["name"].split('.')[1]); part=t["name"].split('.')[2].replace('ffn_','').replace('_shexp','')
    e[f"model.layers.{n}.mlp.shared_experts.{part}_proj.weight"]={"dtype":"F32","shape":[t["ne"][1],t["ne"][0]],"data_offsets":[off,off+len(raw)]}
    body+=raw; off+=len(raw)
h=json.dumps(e).encode()
open(f"{T}/hf/model.safetensors","wb").write(struct.pack('<Q',len(h))+h+bytes(body))
# qkv variants from the fused tiny
src,_,tens,ds,_,_,kvs=g.read_gguf(f"{T}/fusedqkv.gguf")
t=next(t for t in tens if t["name"]=="blk.0.attn_qkv.weight")
w=np.frombuffer(src[ds+t["offset"]:ds+t["offset"]+g.tensor_nbytes(t)],'<f4').reshape(t["ne"][1],t["ne"][0]).copy()
nh=kvs["llada2.attention.head_count"]; nkvh=kvs.get("llada2.attention.head_count_kv",nh); d=kvs["llada2.embedding_length"]
def wf(dn,x):
    os.makedirs(dn,exist_ok=True)
    ee={"model.layers.0.attention.query_key_value.weight":{"dtype":"F32","shape":list(x.shape),"data_offsets":[0,x.size*4]}}
    hh=json.dumps(ee).encode(); open(dn+"/model.safetensors","wb").write(struct.pack('<Q',len(hh))+hh+x.astype('<f4').tobytes())
def inv(x,n):
    hd=x.shape[0]//n; return x.reshape(n,hd//2,2,x.shape[1]).swapaxes(1,2).reshape(x.shape)
wf(f"{T}/hfq-raw", w)
qr=d; kvr=(w.shape[0]-d)//2
wf(f"{T}/hfq-perm", np.concatenate([inv(w[:qr],nh),inv(w[qr:qr+kvr],nkvh),w[qr+kvr:]]))
PY
python3 tools/gguf-shexp-graft.py --gguf "$T/defective.gguf" --hf "$T/hf" --out "$T/grafted.gguf" --quant f32 >/dev/null
python3 tools/gguf-shexp-graft.py --gguf "$T/defective.gguf" --hf "$T/hf" --out "$T/grafted-q4.gguf" --quant q4_0 >/dev/null
"$PROBE" "$T/native.gguf"  2>/dev/null > "$T/a.txt"
"$PROBE" "$T/grafted.gguf" 2>/dev/null > "$T/b.txt"
diff -q "$T/a.txt" "$T/b.txt" >/dev/null && echo "STAGE 1: f32 graft engine-identical — PASS" || { echo "STAGE 1 FAIL"; exit 1; }
D=$(python3 tools/gguf-audit.py "$T/grafted-q4.gguf" 2>/dev/null | grep -ci defect || true)
L=$("$PROBE" "$T/grafted-q4.gguf" 2>/dev/null | grep -c ' ok' || true)
[ "$D" = "0" ] && [ "$L" -ge 5 ] && echo "STAGE 2: q4_0 graft defect-free and alive — PASS" || { echo "STAGE 2 FAIL (defects=$D live=$L)"; exit 1; }
R=$(python3 tools/gguf-shexp-graft.py --gguf "$T/fusedqkv.gguf" --hf "$T/hfq-raw"  --verify-qkv 2>&1)
P=$(python3 tools/gguf-shexp-graft.py --gguf "$T/fusedqkv.gguf" --hf "$T/hfq-perm" --verify-qkv 2>&1)
echo "$R" | grep -q 'RAW.*MATCH' && echo "$R" | grep -q 'PERMUTED.*-> no' && \
echo "$P" | grep -q 'PERMUTED.*MATCH' && echo "$P" | grep -q 'RAW.*-> no' && \
echo "STAGE 3: qkv verdict discriminates both directions — PASS" || { echo "STAGE 3 FAIL"; exit 1; }
echo "graft-selftest: ALL STAGES PASS"; rm -rf "$T"
