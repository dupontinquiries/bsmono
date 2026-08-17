#!/bin/sh
# Cross-engine forward oracle: llama.cpp (CPU reference) vs gpu-gemm (causal path) on the
# same f32 tiny model and the same fixed ids. Argmax agreement is the contract; each
# side's own margin conditions its ties (rows where the reference margin is under tau are
# reported but not failed — random tiny weights sit on real near-ties).
set -e
MODEL=${1:-/tmp/tiny-qwen2.gguf}; K=${2:-48}; V=${3:-256}; TAU=${4:-0.001}
D=$(dirname "$0")
"$D/oracle-ref" "$MODEL" "$K" "$V" > /tmp/oracle-ref.txt
"$D/../../crates/gpu-gemm/target/release/oracle-gpu" "$MODEL" "$K" 2>/dev/null > /tmp/oracle-gpu.txt
python3 - "$TAU" <<'PY'
import sys
tau = float(sys.argv[1])
ref = {int(l.split()[0]): (int(l.split()[1]), float(l.split()[2])) for l in open('/tmp/oracle-ref.txt')}
gpu = {int(l.split()[0]): (int(l.split()[1]), float(l.split()[2])) for l in open('/tmp/oracle-gpu.txt')}
hard = soft = agree = 0
for i in sorted(ref):
    (ra, rm), (ga, gm) = ref[i], gpu[i]
    if ra == ga: agree += 1
    elif rm > tau and gm > tau:
        hard += 1; print(f"  [x] pos {i}: ref {ra} (m={rm:.4f}) vs gpu {ga} (m={gm:.4f})")
    else: soft += 1
print(f"oracle: {agree}/{len(ref)} agree, {soft} near-tie disagreements (either margin <= {tau}), {hard} HARD disagreements")
print("oracle: " + ("PASS - the two engines' causal forwards agree" if hard == 0 else "FAIL - the forwards diverge on confident rows"))
sys.exit(0 if hard == 0 else 1)
PY
