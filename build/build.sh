#!/usr/bin/env bash
# build/build.sh — cross-target build orchestrator (SKELETON STUB)
#
# Status: NOT WIRED. This prints the build plan it WOULD execute, derived from
# manifest/projects.toml + manifest/targets.toml. Filling in the per-kind build
# bodies (cargo / cmake / hybrid) happens in the build-wiring stage, on hardware.
#
# Usage (intended):
#   build/build.sh                  # plan for all projects, all their targets
#   build/build.sh --target phone   # restrict to one target
#   build/build.sh --only snapclone # restrict to one project
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "ecosystem build orchestrator — PLAN ONLY (stub)"
echo "root: $ROOT"
echo
echo "Per-kind build bodies to implement in build-wiring stage:"
echo "  cargo  -> cargo build --release --target <triple>"
echo "  cmake  -> cmake -B build-<triple> ... && cmake --build"
echo "  hybrid -> build vendored/cmake deps, then cargo with linkage (snapclone, photosync)"
echo "  python+rust (rustcheck) -> packaged, not cross-built"
echo
echo "Targets (manifest/targets.toml): desktop, phone, alpine"
echo "Flatpak (desktop only, per manifest): see build/flatpak.sh (to be added)"
echo
echo "TODO(build-wiring):"
echo "  - repoint snapclone CMake at ../../vendor/opendht"
echo "  - resolve gitmind peek -> path dep on ../../tools/peek"
echo "  - emit per-project artifacts into dist/<triple>/"
echo "  - flatpak-cargo-generator over each flatpak=true project's Cargo.lock"
