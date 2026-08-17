#!/bin/sh
# gate-passfuse-diffusion.sh — does LOGSCOPE_PASS_FUSE change what the diffusion loop generates?
#
# It must be TWO PROCESSES: the flag is read once and cached, so setting it mid-process does
# nothing and an in-process A/B would report a reassuring false negative.
#
# c4-736 retracted pass fusion after it broke AR decode 6/6, and scoped the retraction to what had
# been measured. Meanwhile the profile line kept recommending the flag on runs where 97% of submits
# are forced by pass boundaries — i.e. to exactly the operator most likely to try it. This settles
# the diffusion half: greedy generation with tau=None is deterministic, so ANY difference is the
# fused path computing something else.
set -e
D=$(cd "$(dirname "$0")" && pwd)
BIN="$D/../crates/gpu-gemm/target/release/diffuse-dump"
differ=0; checked=0
for m in tiny-llada2 tiny-llada2-shift tiny-llada2-64exp; do
  [ -f "/tmp/$m.gguf" ] || continue
  for cfg in "32 32 32" "16 8 8"; do
    set -- $cfg
    a=$("$BIN" "/tmp/$m.gguf" "$1" "$2" "$3" 2>/dev/null || true)
    b=$(LOGSCOPE_PASS_FUSE=1 "$BIN" "/tmp/$m.gguf" "$1" "$2" "$3" 2>/dev/null || true)
    if [ -z "$a" ] || [ -z "$b" ]; then echo "SKIP $m $cfg (empty output)"; continue; fi
    checked=$((checked+1))
    if [ "$a" = "$b" ]; then echo "same   $m fill=$1 steps=$2 block=$3"
    else echo "DIFFER $m fill=$1 steps=$2 block=$3"; differ=$((differ+1)); fi
    set -- "$m"
  done
done
echo "pass-fusion probe: $differ of $checked configurations changed output"
if [ "$differ" -gt 0 ]; then
  echo "VERDICT: pass fusion is NOT safe on the diffusion path either — the c4-736 retraction holds here."
else
  echo "VERDICT: no difference on these fixtures. Re-price the flag before trusting that: the fixtures"
  echo "         are small and a fixture that stopped discriminating looks identical to a fix."
fi
