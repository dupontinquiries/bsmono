#!/usr/bin/env bash
# Build and run all test harnesses (unit + simulation/tuning), print a summary.
set -e
BUILD="${1:-build-default}"
cmake -B "$BUILD" -S . >/dev/null
cmake --build "$BUILD" -j4 >/dev/null
echo "=== ctest suite ==="
( cd "$BUILD" && ctest --output-on-failure ) | tail -5
echo
echo "=== movement tuning harness (metrics) ==="
"./$BUILD/test_movesim" || true
echo
echo "=== stepping/terrain harness ==="
"./$BUILD/harness_step" | tail -3 || true
