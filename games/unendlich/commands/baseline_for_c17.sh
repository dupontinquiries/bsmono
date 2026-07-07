#!/usr/bin/env bash
#
# baseline_for_c17.sh — anchor the c.17 multiview-CSM baseline.
#
# Runs `rd8_warmup` against the current HEAD and prints the diag
# summary. This is the number every c.17-a/b/c/d ship is competing
# against; running this BEFORE starting a c.17 ship locks in the
# comparison baseline for the in-flight branch.
#
# Workflow:
#   1. ./commands/baseline_for_c17.sh         # establish baseline @HEAD
#   2. <work on c.17-X>
#   3. ./commands/baseline_for_c17.sh         # see new numbers vs baseline
#
# Or, more rigorously, use `unendlich-launcher compare`:
#   1. ./commands/baseline_for_c17.sh         # produces a run at @HEAD
#   2. git tag baseline-c17-pre
#   3. <work on c.17-a, commit>
#   4. unendlich-launcher compare \
#          tests/specs/rd8_warmup.unendtest \
#          --versions @baseline-c17-pre,@HEAD
#
# Usage:
#   ./commands/baseline_for_c17.sh                  # default behavior
#   ./commands/baseline_for_c17.sh --no-diag        # skip the diag print
#
# Exit codes match the launcher's:
#   0 — assertions passed (a valid baseline run)
#   1 — assertions failed (something regressed; investigate)
#   2 — launcher build/setup failure
#   3 — binary aborted

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

if ! git rev-parse --show-toplevel >/dev/null 2>&1; then
    echo "Error: not inside a git repository." >&2
    exit 2
fi
cd "$(git rev-parse --show-toplevel)"

SPEC="tests/specs/rd8_warmup.unendtest"
if [ ! -f "$SPEC" ]; then
    echo "Error: $SPEC not found." >&2
    echo "       (Did you upgrade past c.13-instrumentation-followup?)" >&2
    exit 2
fi

WITH_DIAG=1
for arg in "$@"; do
    case "$arg" in
        --no-diag) WITH_DIAG=0 ;;
        *) echo "Unknown argument: $arg" >&2; exit 2 ;;
    esac
done

echo "==> Building unendlich-launcher (release)..."
cargo build --release -p unendlich-launcher 2>&1 | tail -3
echo

echo "==> Running $SPEC against @HEAD..."
echo "    (~30s run + ~3s warmup; first invocation may also build"
echo "     unendlich-world if not cached.)"
echo
set +e
./target/release/unendlich-launcher run "$SPEC" --version @HEAD
RUN_EXIT=$?
set -e
echo

if [ "$WITH_DIAG" = "1" ]; then
    echo "==> Diag for the just-completed run:"
    echo
    ./target/release/unendlich-launcher diag
fi

echo
echo "==> Done. Latest result symlinked at:"
echo "    ~/.unendlich/latest-result.unendresult.json"
echo "    ~/.unendlich/latest-rd8_warmup.unendresult.json"

exit "$RUN_EXIT"
