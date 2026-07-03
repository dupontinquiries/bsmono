#!/bin/bash
# Run the gpu-gemm correctness gates on lavapipe (software Vulkan) in-sandbox.
# Correctness/fidelity/structure ONLY — absolute tok/s + bandwidth still need real hardware.
set -e
. /home/claude/env.sh
[ -f /usr/share/vulkan/icd.d/lvp_icd.json ] || apt-get install -y mesa-vulkan-drivers vulkan-tools
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
cd "$(dirname "$0")/../crates/gpu-gemm"
RUSTFLAGS="-C prefer-dynamic" cargo build --bin kernel-validate
BIN=./target/debug/kernel-validate
GATES="${*:-matvec qkv q4resident}"
for g in $GATES; do
  echo "### --gate-$g ###"
  timeout 300 "$BIN" "--gate-$g" 2>&1 | grep -E '✓|✗|FAIL|adapter|cos=|GiB' || true
  echo
done
