#!/bin/sh
# run-diffuse.sh — END-TO-END diffusion oracle: gpu-gemm's diffuse_generate vs a reference block
# loop over the (separately oracle-verified) NumPy forward. Compares generated ids exactly.
# This covers what run-bidir.sh cannot: block order, per-block step count, commits per step, which
# positions win, block-causal visibility, and shift_logits handling.
# REFUSES on empty output from either side — an empty-vs-empty "match" is how a broken invocation
# reports success, and this harness has been bitten by exactly that.
set -e
D=$(cd "$(dirname "$0")" && pwd)
GPU="$D/../../crates/gpu-gemm/target/release/diffuse-dump"
fail=0
for M in "$@"; do
  for cfg in "16 8 8" "20 4 8" "12 12 4"; do
    set -- $cfg
    g=$("$GPU" "$M" "$1" "$2" "$3" 2>/dev/null || true)
    r=$(python3 "$D/diffuse_ref.py" "$M" "$1" "$2" "$3" 2>/dev/null || true)
    if [ -z "$g" ] || [ -z "$r" ]; then
      echo "FAIL $M fill=$1 steps=$2 block=$3: empty output (gpu=${#g} ref=${#r}) — not a comparison"
      fail=1
    elif [ "$g" = "$r" ]; then
      echo "PASS $M fill=$1 steps=$2 block=$3"
    else
      echo "FAIL $M fill=$1 steps=$2 block=$3"
      echo "  gpu: $g"
      echo "  ref: $r"
      fail=1
    fi
    set -- "$M"
  done
done
exit $fail
