#!/usr/bin/env bash
# build.sh — build the entire bo-suite workspace in release mode.
#
# Both bo-controller (the Qt/QML app) and bo-mux (the soundscape
# daemon) are members of the same Cargo workspace, so a single
# `cargo build` does everything. We pass `--release` so optimization
# is consistent with the install layout.
#
# Usage:
#   ./scripts/build.sh            # release build
#   ./scripts/build.sh --dev      # debug build (faster, for hacking)

set -euo pipefail

PROFILE="release"
PROFILE_FLAG="--release"
if [[ "${1:-}" == "--dev" || "${1:-}" == "-d" ]]; then
    PROFILE="debug"
    PROFILE_FLAG=""
fi

cd "$(dirname "$0")/.."

echo ">>> Building workspace in $PROFILE mode"
echo "    members: bo-controller, bo-mux"
echo

cargo build --workspace $PROFILE_FLAG

# ── QML lint pass ──────────────────────────────────────────────────
#
# Pre-flight check on the QML tree so runtime errors like
# "Cannot assign to non-existent property" surface in CI / dev-loop
# instead of at app launch (the AlbumDetailPage onTapped class of
# bug). qmllint ships with Qt 5.15+; if it's not installed we skip
# rather than fail — not every dev machine has it, and the cargo
# build is the primary product.
#
# We use --strict on the QML root so warnings are escalated to
# non-zero exit. Files lint independently — one broken file doesn't
# block reporting on the rest.
echo
echo ">>> Linting QML…"
QML_DIR="bo-controller/qml"
if command -v qmllint6 >/dev/null 2>&1; then
    QMLLINT=qmllint6
elif command -v qmllint >/dev/null 2>&1; then
    QMLLINT=qmllint
else
    QMLLINT=""
fi

if [[ -n "$QMLLINT" ]]; then
    LINT_FAIL=0
    for f in "$QML_DIR"/*.qml; do
        # Skip qmldir, the Bo singleton resolver is purely declarative.
        bn=$(basename "$f")
        if [[ "$bn" == "qmldir" ]]; then continue; fi
        # Capture stderr; if it emits anything starting with "Error" or
        # "Warning" we surface and count it. qmllint exits 0 on
        # warnings, which we treat as a failure here.
        out=$("$QMLLINT" -I "$QML_DIR" "$f" 2>&1 || true)
        if echo "$out" | grep -qE "^(Error|Warning|\\[)"; then
            echo "    [lint] $bn"
            echo "$out" | sed 's/^/        /'
            LINT_FAIL=1
        fi
    done
    if (( LINT_FAIL == 1 )); then
        echo
        echo "!!! QML lint surfaced issues above. Fix before installing,"
        echo "    or the binary will crash on the first navigation that"
        echo "    instantiates the broken page."
        echo "    To skip this gate (NOT RECOMMENDED): set SKIP_QML_LINT=1"
        if [[ "${SKIP_QML_LINT:-0}" != "1" ]]; then
            exit 2
        fi
    else
        echo "    OK"
    fi
else
    echo "    skipped — qmllint not installed (install qt5-tools or qt6-base-dev"
    echo "    to enable the pre-flight QML check)"
fi

echo
echo ">>> Built binaries:"
for bin in bo-controller bo-muxd; do
    path="target/$PROFILE/$bin"
    if [[ -x "$path" ]]; then
        size=$(du -h "$path" | cut -f1)
        echo "    $path  ($size)"
    fi
done

echo
echo ">>> Next: ./scripts/install.sh    (installs to ~/.local/bin + ~/.config)"
