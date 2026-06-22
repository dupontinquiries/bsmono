#!/usr/bin/env bash
#
# launcher_smoke.sh — quick end-to-end check that the launcher works
# against the current HEAD. Builds the launcher and runs the smallest
# spec (rd2_smoke) via it. Optionally runs the full suite.
#
# Usage:
#   ./commands/launcher_smoke.sh            # single rd2_smoke run
#   ./commands/launcher_smoke.sh --suite    # full tests/specs/ suite
#
# Exit codes match the launcher's:
#   0 — launcher worked, assertions passed
#   1 — launcher worked, assertions failed (still valid result file)
#   2 — launcher setup/build failed
#   3 — binary aborted

set -euo pipefail

# Resolve to the repo root regardless of where the script was invoked from.
# Without this, calling from a parent directory would fail with "not inside
# a git repository" because git rev-parse searches CWD upward, not the
# script's location.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

MODE="single"
if [ $# -ge 1 ] && [ "$1" = "--suite" ]; then
    MODE="suite"
fi

if ! git rev-parse --show-toplevel >/dev/null 2>&1; then
    echo "Error: not inside a git repository (script is in $SCRIPT_DIR)." >&2
    exit 1
fi
cd "$(git rev-parse --show-toplevel)"

if [ ! -d "tests/specs" ]; then
    echo "Error: tests/specs/ not found." >&2
    exit 1
fi

echo "==> Building unendlich-launcher..."
cargo build --release -p unendlich-launcher 2>&1 | tail -3

echo
if [ "$MODE" = "suite" ]; then
    echo "==> Running full suite against @HEAD..."
    echo "    (First invocation per spec builds unendlich-world from a fresh"
    echo "     clone; ~60-90s for the first one, fast thereafter.)"
    echo
    ./target/release/unendlich-launcher run-suite --version @HEAD
else
    echo "==> Running smoke test (rd2_smoke) against @HEAD..."
    echo "    (First invocation builds unendlich-world from a fresh clone:"
    echo "     expect ~60-90s of cargo build the first time, fast thereafter.)"
    echo
    ./target/release/unendlich-launcher run tests/specs/rd2_smoke.unendtest --version @HEAD
fi
