#!/usr/bin/env bash
# snapshot_diff.sh <base.zip|dir> <new.zip|dir> [out.patch]
# Unified, git-apply-able diff between two snapshots (zip or dir). Use to compare
# two of Claude's drops, or to make a patch to drop onto unend-mono:
#   cd unend-mono && git apply --reject -p<level> out.patch
set -uo pipefail
prep(){ if [[ "$1" == *.zip ]]; then d="$(mktemp -d)"; unzip -q "$1" -d "$d";
          [ -d "$d/unendlich" ] && echo "$d/unendlich" || echo "$d";
        else [ -d "$1/unendlich" ] && echo "$1/unendlich" || echo "$1"; fi; }
b="$(prep "$1")"; n="$(prep "$2")"; out="${3:-changes.patch}"
git diff --no-index "$b" "$n" > "$out" 2>/dev/null || true
echo "wrote $out  ($(grep -c '^diff ' "$out" 2>/dev/null || echo 0) files changed)"
echo "review, or:  cd unend-mono && git apply --reject -p<level> $(realpath "$out")"
