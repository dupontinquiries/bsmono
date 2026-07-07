#!/usr/bin/env bash
#
# tag_current_version.sh — read Cargo.toml's version field and create
# a matching git tag on HEAD. Saves you from retyping the version
# every time you commit a new Claude drop.
#
# Usage:
#   ./commands/tag_current_version.sh
#
# Tag format: v<cargo_version>  (e.g. v0.24.2-c.11)
#
# Safety:
#   - Refuses if the tag already exists (won't clobber a different commit).
#   - Refuses if Cargo.toml's version doesn't parse cleanly.
#   - Doesn't push — that's your call.

set -euo pipefail

# Resolve to the repo root regardless of where the script was invoked from.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

if ! git rev-parse --show-toplevel >/dev/null 2>&1; then
    echo "Error: not inside a git repository (script is in $SCRIPT_DIR)." >&2
    exit 1
fi

cd "$(git rev-parse --show-toplevel)"

if [ ! -f "Cargo.toml" ]; then
    echo "Error: no Cargo.toml in repo root." >&2
    exit 1
fi

# Try to read version from root manifest first (single-crate).
# Fall back to engine crate (workspace layout).
read_version() {
    local v
    v=$(awk -F'"' '/^version = "/{print $2; exit}' Cargo.toml 2>/dev/null)
    if [ -n "$v" ]; then echo "$v"; return; fi
    for candidate in \
        crates/unendlich-engine/Cargo.toml \
        crates/unendlich-shared/Cargo.toml \
        crates/unendlich-launcher/Cargo.toml
    do
        if [ -f "$candidate" ]; then
            v=$(awk -F'"' '/^version = "/{print $2; exit}' "$candidate" 2>/dev/null)
            if [ -n "$v" ]; then echo "$v"; return; fi
        fi
    done
    echo ""
}

VERSION="$(read_version)"
if [ -z "$VERSION" ]; then
    echo "Error: couldn't find version in Cargo.toml or any crate manifest." >&2
    exit 1
fi

TAG="v$VERSION"

if git rev-parse "$TAG" >/dev/null 2>&1; then
    EXISTING="$(git rev-parse "$TAG")"
    HEAD_HASH="$(git rev-parse HEAD)"
    if [ "$EXISTING" = "$HEAD_HASH" ]; then
        echo "Tag $TAG already exists at HEAD ($HEAD_HASH). Nothing to do."
        exit 0
    else
        echo "Error: tag $TAG already exists at a different commit:" >&2
        echo "       $TAG → $EXISTING" >&2
        echo "       HEAD → $HEAD_HASH" >&2
        echo "       If you really want to move the tag, run:" >&2
        echo "         git tag -d $TAG && git tag $TAG" >&2
        exit 1
    fi
fi

git tag "$TAG"
echo "Tagged HEAD as $TAG"
echo "To push: git push origin $TAG"
