#!/usr/bin/env bash
# verify.sh — run the static "suspenders" over the whole tree (SKELETON STUB)
#
# Status: dispatcher scaffold. The three checks are located but not yet run —
# wiring them live is the NEXT stage (lightweight verification without compiling
# the heavy crates). Each check should emit a short pass/fail summary per project.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"

echo "suspenders over $ROOT  (stub — checks located, not yet executed)"
echo

# 1) peek wiring-audit: detect orphan modules (state+lifecycle, zero external refs)
echo "[peek]      tools/peek/wiring-audit.sh  -> run per Rust project, flag service-orphans"
#   next stage: for each apps/* system/* with Cargo, run wiring-audit, collect orphan counts

# 2) rustcheck: lint/structural checks
echo "[rustcheck] tools/rustcheck/rustcheck.py -> run per project, collect findings"
#   next stage: python3 tools/rustcheck/rustcheck.py <project>

# 3) drift: boundary-drift reconciliation (cross-boundary assumption checks)
echo "[drift]     tools/drift                  -> check declared vs actual boundary assumptions"
#   next stage: cargo run -p drift (cargo plugin) once buildable on hardware,
#               or its check mode over the FFI/daemon/kit seams

echo
echo "TODO(next stage): wire each check live, aggregate a single PASS/FAIL table,"
echo "and gate further structural work on it."
