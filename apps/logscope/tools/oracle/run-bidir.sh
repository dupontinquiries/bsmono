#!/bin/sh
# run-bidir.sh — BIDIRECTIONAL oracle: gpu-gemm's denoise_stats vs a NumPy transcription of
# inclusionAI's modeling_llada2_moe.py. The causal oracle (llama.cpp) cannot cover this path
# because llada2 has never landed upstream, so until now every diffusion result was compared only
# against itself. Argmax agreement is the contract; p_top/entropy are checked within tolerance and
# near-ties in the reference are reported, not failed (random tiny weights sit on real ties).
set -e
MODEL=${1:-/tmp/tiny-llada2.gguf}; T=${2:-16}
D=$(dirname "$0")
"$D/../../crates/gpu-gemm/target/release/denoise-dump" "$MODEL" "$T" 2>/dev/null > /tmp/bidir-gpu.txt
python3 "$D/llada2_ref.py" "$MODEL" "$T" > /tmp/bidir-ref.txt
python3 - "$MODEL" "$T" <<'PY'
import sys
model, t = sys.argv[1], int(sys.argv[2])
g = [l.split() for l in open('/tmp/bidir-gpu.txt') if l.strip()]
r = [l.split() for l in open('/tmp/bidir-ref.txt') if l.strip()]
n = min(len(g), len(r))
bad = ties = 0
worst_p = worst_h = 0.0
for i in range(n):
    gi, ri = g[i], r[i]
    mref = float(ri[2])
    if gi[0] != ri[0]:
        if mref < 1e-4: ties += 1          # a genuine near-tie: argmax is not meaningful here
        else:
            bad += 1
            if bad <= 3: print(f"  pos {i}: argmax gpu={gi[0]} ref={ri[0]} (ref logit margin {mref:.2e})")
    worst_p = max(worst_p, abs(float(gi[1]) - float(ri[1])) / max(float(ri[1]), 1e-9))
    worst_h = max(worst_h, abs(float(gi[3]) - float(ri[3])) / max(abs(float(ri[3])), 1e-9))
ok = bad == 0 and worst_p < 2e-3 and worst_h < 2e-3
print(f"{'PASS' if ok else 'FAIL'} {model} t={t}: {n-bad-ties}/{n} argmax agree ({ties} near-tie tolerated), "
      f"p_top rel {worst_p:.2e}, entropy rel {worst_h:.2e}")
sys.exit(0 if ok else 1)
PY
