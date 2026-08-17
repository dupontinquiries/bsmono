#!/bin/sh
# check-all.sh — compile every crate the way the OPERATOR builds it. Written because c4-748 shipped
# a build break that my own verification could not see: I ran `cargo check -p logscope-egui` with
# NO FEATURES, while the whole wgpu path — including the diffusion code I was editing — sits behind
# `#[cfg(feature = "wgpu-engine")]`. A check that does not compile the code under test is vacuous,
# the same class as the empty-output oracle and the sub-ulp comparison.
set -e
cd "$(dirname "$0")/.."
echo "== gpu-gemm =="
(cd crates/gpu-gemm && cargo check -q)
echo "== logscope-egui (wgpu-engine) =="
cargo check -q -p logscope-egui --features wgpu-engine
echo "== logscope-egui (wgpu-engine,voice) — the operator's exact line =="
# `voice` pulls alsa-sys, which needs libasound2-dev. That is present on the operator's box and
# absent in the dev sandbox, so a failure HERE is reported and tolerated rather than masking the
# two stages above — but it is NOT silently skipped, because "the check did not run" is exactly the
# state that let the c4-748 break ship.
if cargo check -q -p logscope-egui --features wgpu-engine,voice 2>/tmp/voice-check.txt; then
  echo "   voice: OK"
elif grep -q 'alsa-sys' /tmp/voice-check.txt; then
  echo "   voice: SKIPPED — alsa-sys needs libasound2-dev, absent here. The wgpu-engine stage above"
  echo "          did compile the engine path; run this stage on a box with ALSA to cover the rest."
else
  echo "   voice: FAILED for a reason other than ALSA:"; tail -20 /tmp/voice-check.txt; exit 1
fi
echo "check-all: OK"
