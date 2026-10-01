#!/usr/bin/env bash
# gpu_hw_check.sh — REAL-HARDWARE-TIER GPU gate (see harness/RULESET.md "GPU work",
# and docs/agents/APPLE_M2_GPU_STACK.md at the repo root).
#
# The sandbox tier (gpu_check.sh) needs no GPU; the CPU-Vulkan tier runs the same
# probes on lavapipe/llvmpipe. This tier runs the headless probe on a REAL GPU and
# refuses to pass if the device it ended up on is a CPU device — on a machine that
# exposes both (Asahi: Honeykrisp AND llvmpipe) a "hardware" run that quietly fell
# back to software would otherwise read as a hardware result. The probe used to
# make that worse by labelling every run "(lavapipe)".
#
#   bash harness/gpu_hw_check.sh              build (if needed), run, gate
#   UNENDLICH_ALLOW_CPU=1 bash harness/gpu_hw_check.sh   accept a CPU device (CI)
#   SKIP_BUILD=1 ...                          use the existing release binary
#
# Exit: 0 green on a real GPU (or CPU with ALLOW_CPU), 1 probe failed,
#       2 ran on a CPU device without ALLOW_CPU, 3 could not run (no probe/binary).
set -u
cd "$(dirname "$0")/.."
BIN=target/release/gpu-headless-probe
if [ "${SKIP_BUILD:-0}" != 1 ]; then
  cargo build --release -p unendlich-engine --features headless --bin gpu-headless-probe >/dev/null 2>&1 \
    || { echo "BUILD FAILED: cargo build --release -p unendlich-engine --features headless --bin gpu-headless-probe"; exit 3; }
fi
[ -x "$BIN" ] || { echo "no binary at $BIN"; exit 3; }

log="$(mktemp)"; trap 'rm -f "$log"' EXIT
RUST_LOG=info timeout "${PROBE_TIMEOUT:-180}" "$BIN" >"$log" 2>&1
rc=$?

device=$(sed -n 's/.*Headless context: picked device \(.*\) (graphics qf.*/\1/p' "$log" | head -1)
driver=$(sed -n 's/.*driver_name: "\([^"]*\)".*/\1/p' "$log" | head -1)
avg=$(sed -n 's/^GPU mesh headless: .* avg \([0-9]*\) us\/chunk.*/\1/p' "$log" | head -1)
green=$(grep -c "HEADLESS GPU PROBE GREEN" "$log")

echo "device : ${device:-<not reported>}"
echo "driver : ${driver:-<not reported>}"
echo "mesh   : ${avg:-?} us/chunk"

if [ "$rc" -ne 0 ] || [ "$green" -ne 1 ]; then
  echo "PROBE FAILED (exit $rc, green lines: $green)"; tail -15 "$log"; exit 1
fi
# A probe that did not say which device it used cannot be trusted as a hardware run.
if [ -z "$device" ]; then
  echo "HW GATE RED — the probe did not report a device, so this cannot be called a hardware run"; exit 1
fi
if printf '%s %s' "$device" "$driver" | grep -qiE 'llvmpipe|lavapipe|softpipe|swrast|software|cpu'; then
  if [ "${UNENDLICH_ALLOW_CPU:-0}" = 1 ]; then
    echo "HW GATE GREEN (CPU device accepted: UNENDLICH_ALLOW_CPU=1)"; exit 0
  fi
  echo "HW GATE RED — ran on a CPU device ($device). Not a hardware result."
  echo "  A machine with both ICDs picks the GPU by default; if this fell back, check the driver (vulkaninfo --summary)."
  exit 2
fi
echo "HW GATE GREEN"
exit 0
