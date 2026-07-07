#!/usr/bin/env bash
#
# update_from_claude_zip.sh — replace the working tree with the contents
# of a Claude-produced unendlich-v*.zip, preserving .git and selected
# always-keep files (this commands/ directory included).
#
# Usage:
#   ./commands/update_from_claude_zip.sh <zip>
#
# <zip> may be:
#   - A full path: /path/to/unendlich-vX.zip
#   - A bare filename: unendlich-vX.zip
#       Resolved in order against:
#         1. ../claude-zips/   (the canonical layout's archive)
#         2. ~/Downloads/      (where browsers tend to land them)
#         3. Current directory
#
# Expected directory layout (recommended):
#
#   ~/unendlich-development/
#   ├── unendlich-game/         ← this repo, run script from here
#   │   ├── .git/
#   │   ├── Cargo.toml
#   │   ├── commands/
#   │   └── ...
#   └── claude-zips/            ← zip archive (sibling of repo)
#       ├── unendlich-v0.24.2c.10.zip
#       └── unendlich-v0.24.2c.11.zip
#
# What it does:
#   1. Refuses to run unless cwd is a git repo and has Cargo.toml at the
#      root with name = "unendlich" (sanity).
#   2. Resolves the zip arg (full path or bare filename, see above).
#   3. Extracts the zip to a temp dir.
#   4. Validates the zip wraps everything under a single top-level
#      `unendlich/` directory containing a Cargo.toml.
#   5. Wipes the working tree EXCEPT .git/ and entries in ALWAYS_KEEP.
#   6. Moves the contents of the zip's `unendlich/` directory into the
#      repo root.
#   7. Shows `git status` so you can review before committing.
#
# What it does NOT do:
#   - Commit anything (use git add -A && git commit manually).
#   - Tag anything (use ./commands/tag_current_version.sh).
#   - Touch ~/.unendlich/ (launcher state, independent).
#
# Safety rails:
#   - Bails on any sub-command failure (`set -euo pipefail`).
#   - Stages new content to /tmp first; only wipes the tree once the
#     new content is fully extracted and validated.
#   - Never recursively deletes anything outside the current repo root.

set -euo pipefail

# Files/dirs at the repo root to preserve across update. Add anything
# here you want kept across Claude zip updates.
ALWAYS_KEEP=(
    ".git"
    "commands"
)

# ---------------------------------------------------------------------
# Arg parsing
# ---------------------------------------------------------------------

if [ $# -ne 1 ]; then
    echo "Usage: $0 <zip>" >&2
    echo "" >&2
    echo "  <zip> may be a full path or just a filename." >&2
    echo "  Bare filenames are resolved against ../claude-zips/," >&2
    echo "  ~/Downloads/, then the current directory." >&2
    exit 1
fi

ZIP_ARG="$1"

# Resolve the zip path. If the arg is itself a valid file path, use
# it directly. Otherwise, search a few canonical locations.
resolve_zip() {
    local arg="$1"

    # Absolute or relative path that exists: use as-is.
    if [ -f "$arg" ]; then
        realpath "$arg"
        return 0
    fi

    # Bare filename (no slash): search canonical locations in order.
    if [[ "$arg" != */* ]]; then
        local candidates=(
            "../claude-zips/$arg"
            "$HOME/Downloads/$arg"
            "./$arg"
        )
        for cand in "${candidates[@]}"; do
            if [ -f "$cand" ]; then
                realpath "$cand"
                return 0
            fi
        done
    fi

    return 1
}

if ! ZIP_PATH="$(resolve_zip "$ZIP_ARG")"; then
    echo "Error: could not find zip: $ZIP_ARG" >&2
    echo "" >&2
    echo "  Searched:" >&2
    if [[ "$ZIP_ARG" != */* ]]; then
        echo "    ../claude-zips/$ZIP_ARG" >&2
        echo "    ~/Downloads/$ZIP_ARG" >&2
        echo "    ./$ZIP_ARG" >&2
    else
        echo "    $ZIP_ARG (as given)" >&2
    fi
    exit 1
fi

case "$ZIP_PATH" in
    *.zip) ;;
    *) echo "Error: $ZIP_PATH doesn't look like a zip file." >&2; exit 1 ;;
esac

echo "    Using zip: $ZIP_PATH"

# ---------------------------------------------------------------------
# Sanity: are we in an unendlich repo?
# ---------------------------------------------------------------------

if ! git rev-parse --show-toplevel >/dev/null 2>&1; then
    echo "Error: not inside a git repository." >&2
    echo "       Run this from inside the unendlich working tree." >&2
    exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

if [ ! -f "Cargo.toml" ]; then
    echo "Error: $REPO_ROOT has no Cargo.toml — refusing to wipe." >&2
    echo "       This script only runs against an unendlich repo (single-crate or workspace)." >&2
    exit 1
fi

# Sanity check: this Cargo.toml should be for unendlich. Accepts either:
#   - A package named "unendlich" (single-crate layout, c.11 and earlier).
#   - A package named "unendlich-*" (workspace member or future split).
#   - A workspace listing crates/unendlich-* in members (workspace root).
is_unendlich_manifest() {
    local toml="$1"
    grep -q '^name = "unendlich"' "$toml" 2>/dev/null && return 0
    grep -q '^name = "unendlich-' "$toml" 2>/dev/null && return 0
    grep -q 'crates/unendlich-' "$toml" 2>/dev/null && return 0
    return 1
}

if ! is_unendlich_manifest Cargo.toml; then
    echo "Error: $REPO_ROOT/Cargo.toml doesn't look like an unendlich manifest." >&2
    echo "       Refusing to wipe a non-unendlich repo." >&2
    exit 1
fi

# ---------------------------------------------------------------------
# Stage the zip's contents
# ---------------------------------------------------------------------

STAGE="$(mktemp -d -t unendlich-update.XXXXXXXX)"
trap 'rm -rf "$STAGE"' EXIT INT TERM

echo "==> Extracting $ZIP_PATH to staging area..."
unzip -q "$ZIP_PATH" -d "$STAGE"

# Validate the staged content has the expected structure.
if [ ! -d "$STAGE/unendlich" ]; then
    echo "Error: $ZIP_PATH doesn't contain a top-level 'unendlich/' directory." >&2
    exit 1
fi
if [ ! -f "$STAGE/unendlich/Cargo.toml" ]; then
    echo "Error: $ZIP_PATH/unendlich/Cargo.toml is missing." >&2
    exit 1
fi

# Cross-check: same package name or workspace? (Defensive against renamed projects.)
if ! is_unendlich_manifest "$STAGE/unendlich/Cargo.toml"; then
    echo "Error: Staged Cargo.toml doesn't look like an unendlich manifest." >&2
    echo "       The zip might be for a different project." >&2
    exit 1
fi

# What version is the zip claiming? (Informational.)
# Handle both single-crate (version at root Cargo.toml) and workspace
# (version in member crates) layouts.
read_version() {
    local repo_root="$1"
    local manifest="$repo_root/Cargo.toml"

    # Try the root manifest first (single-crate layout).
    local v
    v=$(awk -F'"' '/^version = "/{print $2; exit}' "$manifest" 2>/dev/null)
    if [ -n "$v" ]; then echo "$v"; return; fi

    # Workspace: try any member crate. Prefer the engine crate, fall
    # back to the first one we find.
    for candidate in \
        "$repo_root/crates/unendlich-engine/Cargo.toml" \
        "$repo_root/crates/unendlich-shared/Cargo.toml" \
        "$repo_root/crates/unendlich-launcher/Cargo.toml"
    do
        if [ -f "$candidate" ]; then
            v=$(awk -F'"' '/^version = "/{print $2; exit}' "$candidate" 2>/dev/null)
            if [ -n "$v" ]; then echo "$v"; return; fi
        fi
    done
    echo "unknown"
}

NEW_VERSION="$(read_version "$STAGE/unendlich")"
OLD_VERSION="$(read_version ".")"
echo "    Current version: $OLD_VERSION"
echo "    New version:     $NEW_VERSION"

if [ "$OLD_VERSION" = "$NEW_VERSION" ]; then
    echo "    (Same version — likely a re-extraction.)"
fi

# ---------------------------------------------------------------------
# Wipe and replace
# ---------------------------------------------------------------------

# Build the find expression that skips ALWAYS_KEEP entries.
echo "==> Wiping working tree (keeping: ${ALWAYS_KEEP[*]})..."
FIND_EXPR=()
for keep in "${ALWAYS_KEEP[@]}"; do
    FIND_EXPR+=(! -name "$keep")
done
find . -mindepth 1 -maxdepth 1 "${FIND_EXPR[@]}" -exec rm -rf {} +

echo "==> Moving new content into place..."
# Use `cp -r` rather than `mv` for two reasons:
#   (1) `mv` fails on inter-filesystem moves where the destination
#       directory exists and is non-empty (common: /tmp on tmpfs, repo
#       on a different filesystem). The error looks like
#         "inter-device move failed: ...; unable to remove target:
#          Directory not empty"
#       and silently drops files from the new content.
#   (2) For ALWAYS_KEEP directories (e.g. commands/), we want NEW files
#       from the zip to MERGE in alongside the preserved ones, not
#       replace the whole directory or fail. `cp -r` merges by default.
#
# The trade-off is slightly slower (extra copy from /tmp instead of
# rename), but the staging dir is small (~10MB) and the safety beats
# silent file loss.
#
# `cp` of a glob doesn't copy dotfiles by default, so handle them
# the same way as before.
shopt -s dotglob nullglob
cp -r "$STAGE/unendlich/"* .
shopt -u dotglob nullglob

# ---------------------------------------------------------------------
# Surface what changed
# ---------------------------------------------------------------------

echo
echo "==> Update complete. Review with git status:"
echo
git status

echo
echo "Next steps:"
echo "  1. Review the diff:        git diff --stat   (or 'gitui')"
echo "  2. Commit:                 git add -A && git commit -m 'Update to $NEW_VERSION'"
echo "  3. Tag:                    git tag v$NEW_VERSION"
echo "  4. (Optional) Push tag:    git push --tags"
