#!/bin/sh
# gen-all.sh — produce every /tmp/tiny-*.gguf the gpu-gemm test suite reads.
#
# WHY THIS EXISTS
#
# 28 test sites in gpu-gemm guard on a fixture file and `return` when it is
# absent. A test that returns early PASSES, so the suite reports green while
# doing nothing — which is how the loader went unexamined long enough for two
# documents to disagree about whether one of its modules existed.
#
# The generators were always here; nothing tied them to the names the tests
# read. Run this once and the guarded tests execute.
#
#   sh tools/tiny-variants/gen-all.sh
#   cargo test -p gpu-gemm --lib
#
# Names are the tests' own, not the generators' defaults. Grep them back with:
#   grep -rhoE '/tmp/tiny-[a-z0-9._-]+\.gguf' crates/gpu-gemm/src/*.rs | sort -u
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-/tmp}

gen() {  # gen <script> <outfile> [extra args...]
  script=$1; target=$2; shift 2
  if [ ! -f "$here/$script" ]; then
    echo "  SKIP  $target — no $script"; return 0
  fi
  if python3 "$here/$script" "$target" "$@" >/dev/null 2>&1; then
    echo "  ok    $target  ($(wc -c < "$target") B)"
  else
    echo "  FAIL  $target  (python3 $script $target $*)"
  fi
}

echo "generating fixtures into $out"
# THE SECOND ARGUMENT IS NOT OPTIONAL. Every gen3-family script defaults to the
# qwen2 shape and only emits the named architecture when argv[2] asks for it.
# Omitting it produces a file with the right NAME and the wrong ARCHITECTURE,
# which is worse than no fixture: the guarded tests then run against a qwen2
# model and fail with errors about the model, not about the fixture. Eight of
# them did exactly that before this line was corrected — the tell was
# tiny-llada2.gguf coming out byte-for-byte the same size as tiny-qwen2.gguf.
gen gen3.py       "$out/tiny-qwen3.gguf"          qwen3
gen gen2.py       "$out/tiny-qwen2.gguf"
gen gen3.py       "$out/tiny-llada2.gguf"         llada2
gen gen_shift.py  "$out/tiny-llada2-shift.gguf"   llada2
gen gen_wideexp.py "$out/tiny-llada2-64exp.gguf"  llada2
gen gen_glmdsa.py "$out/tiny-glmdsa.gguf"

# Not generated, and named so nobody hunts for a generator that does not exist:
#   tiny-qwen3-q8.gguf    a QUANTIZED build; the q8 path is exercised by the
#                         in-crate synthetic builders in main.rs instead.
#   tiny-llada2-q4file.gguf  likewise, a q4-in-file variant.
#   tiny-notok.gguf       deliberately tokenizer-less, for the borrow-a-vocab
#                         path in tokenizer.rs. Any weights-only GGUF works;
#                         gen2.py's output before the vocab was added did.
echo
echo "not generated (no generator in tree): tiny-qwen3-q8, tiny-llada2-q4file, tiny-notok"
