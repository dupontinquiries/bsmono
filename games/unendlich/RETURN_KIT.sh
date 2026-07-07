#!/usr/bin/env bash
# RETURN_KIT.sh — the worker runs this. Two steps:
#   bash RETURN_KIT.sh start    # ONCE, BEFORE you edit anything (snapshots the base)
#   bash RETURN_KIT.sh finish   # when done — verifies + produces the return artifact
#
# `finish` REQUIRES the sandbox harness to be GREEN, then writes a patch the
# coordinator can apply. Artifacts land in /mnt/user-data/outputs so you can
# present them. Keep your chat reply to just the present_files call + the
# printed MANIFEST. (Equivalent to the project's harness/snapshot_diff.sh.)
set -uo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"      # the unendlich/ tree root
BASE="/tmp/__unend_base"
MAN="$ROOT/MANIFEST.sha256"
WID="$(cat "$ROOT/WORKER_ID" 2>/dev/null || echo "")"   # e.g. T01 — set per worker zip
PATCH_NAME="CHANGES${WID:+-$WID}.patch"                  # -> CHANGES-T01.patch (or CHANGES.patch)
OUT="/mnt/user-data/outputs"; mkdir -p "$OUT" 2>/dev/null || OUT="$ROOT/.."
SKIP_RE='^(RETURN_KIT\.sh|MANIFEST\.sha256|WORKER_ID|WORKER_BRIEF.*\.md)$'

case "${1:-}" in
  start)
    rm -rf "$BASE"; cp -r "$ROOT" "$BASE"
    echo "[start] pristine snapshot saved to $BASE."
    echo "        Now build your tasks. Keep crates/.../sim or world or moisture PURE."
    echo "        When finished:  bash RETURN_KIT.sh finish"
    ;;
  finish)
    echo "== sandbox harness (must end in HARNESS GREEN) =="
    if ! ( cd "$ROOT" && bash harness/run.sh ); then   # run.sh needs CWD = repo root
      echo; echo "!! HARNESS NOT GREEN — fix before returning. Nothing written."; exit 1
    fi
    patch_out="$OUT/$PATCH_NAME"
    if [ -d "$BASE" ]; then
      # stage pristine (a/) vs current (b/), drop scaffolding, diff -> clean tree-relative paths (apply with -p1)
      stage="$(mktemp -d)"; cp -r "$BASE" "$stage/a"; cp -r "$ROOT" "$stage/b"
      for d in a b; do rm -f "$stage/$d/RETURN_KIT.sh" "$stage/$d/MANIFEST.sha256" "$stage/$d/WORKER_BRIEF.md" 2>/dev/null; done
      ( cd "$stage" && diff -ruN a b > "$patch_out" ); rm -rf "$stage"
      echo "[finish] wrote $patch_out  (apply: cd unend-mono && git apply -p1 CHANGES.patch)"
    else
      echo "[finish] WARNING: no 'start' snapshot found — patch skipped; sending CHANGES_files.zip instead."
      patch_out=""
    fi
    # New/changed file manifest (works even without a snapshot, via MANIFEST.sha256)
    echo "== MANIFEST  (+ = new file, ~ = changed file) =="
    bundle="/tmp/__changes"; rm -rf "$bundle"; mkdir -p "$bundle"
    while IFS= read -r f; do
      rel="${f#$ROOT/}"
      base_rel="$(basename "$rel")"
      [[ "$base_rel" =~ $SKIP_RE ]] && continue
      case "$rel" in harness/*) continue;; esac
      h="$(sha256sum "$f" | cut -d' ' -f1)"
      old="$(grep -F " $rel" "$MAN" 2>/dev/null | head -1 | cut -d' ' -f1)"
      if [ -z "$old" ]; then echo "+ $rel"; mkdir -p "$bundle/$(dirname "$rel")"; cp "$f" "$bundle/$rel";
      elif [ "$old" != "$h" ]; then echo "~ $rel"; mkdir -p "$bundle/$(dirname "$rel")"; cp "$f" "$bundle/$rel"; fi
    done < <(find "$ROOT" -type f | sort)
    ( cd "$bundle" && zip -rq "$OUT/CHANGES_files${WID:+-$WID}.zip" . 2>/dev/null ) && echo "[finish] wrote $OUT/CHANGES_files${WID:+-$WID}.zip (fallback)"
    echo
    echo "RETURN TO COORDINATOR: download $PATCH_NAME and give it to the coordinator chat."
    echo "Then your reply needs only:  present_files([\"$patch_out\"])  + the MANIFEST lines above."
    ;;
  *) echo "usage: bash RETURN_KIT.sh start|finish";;
esac
