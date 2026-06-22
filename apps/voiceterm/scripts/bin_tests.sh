#!/usr/bin/env bash
# v0.1.767: run the in-binary test suite (the ~2500 tests compiled into the
# web2qml bin, including the whole voice_intent suite). These were dormant for a
# long time because they need the FULL crate to build (Qt + audio deps), unlike
# the pure-module harness (scripts/cargo_test_pure.sh) which stages a
# dependency-free crate. Once the toolchain + Qt were restored and a couple of
# accumulated compile-drift errors were fixed (a missing CommandBlock field, a
# too-narrow test import), the suite runs again.
#
# Required environment (install once per fresh sandbox):
#   - Rust toolchain at /opt/rust (edition2024-capable; apt's 1.75 is too old)
#   - Qt6 + QtQuick:   apt-get install -y qt6-base-dev qmake6 qt6-declarative-dev
#   - ALSA:            apt-get install -y libasound2-dev
#   - libclang+cmake:  apt-get install -y libclang-dev cmake build-essential
#   - QMAKE=/usr/bin/qmake6  LIBCLANG_PATH=/usr/lib/llvm-18/lib
#
# Usage:  scripts/bin_tests.sh [test-name-filter]
set -u
export PATH="/opt/rust/bin:$HOME/.local/bin:$PATH"
export QMAKE="${QMAKE:-/usr/bin/qmake6}"
export LIBCLANG_PATH="${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found on PATH (expected /opt/rust/bin/cargo)."
  exit 1
fi

FILTER="${1:-}"
echo ">> cargo test --features 'voice claude' --bin web2qml ${FILTER}"
cargo test --features "voice claude" --bin web2qml ${FILTER} 2>&1 | grep -E \
  "test result|FAILED|error\[|could not compile"
