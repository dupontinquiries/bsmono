#!/usr/bin/env bash
# xref.sh — token-cheap code/doc lookup for the unendlich coordinator.
# Prints minimal file:line + trimmed context (deduped, capped) so you spend
# tokens on the answer, not on scrolling grep dumps. Run from repo root.
#
#   tools/xref.sh sym  <Name>     definition site(s) of a fn/struct/impl/const/trait + use count
#   tools/xref.sh env  <FLAG>     where an UNENDLICH_* flag is read (or list all: `env -l`)
#   tools/xref.sh doc  <kw>       docs that mention kw (file + matching header/line, capped)
#   tools/xref.sh find <kw>       terse code grep (file:line:trimmed, capped)
#   tools/xref.sh file <frag>     resolve a path fragment to real file(s)
#
# Output is intentionally short. Re-run with a narrower query if truncated.
set -euo pipefail
SRC="crates/unendlich-engine/src"
CAP="${XREF_CAP:-25}"
G() { if command -v rg >/dev/null 2>&1; then rg -n --no-heading "$@"; else grep -rnE "$@"; fi; }
trim() { sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//'; }

cmd="${1:-}"; q="${2:-}"
case "$cmd" in
  sym)
    [ -z "$q" ] && { echo "usage: xref.sh sym <Name>"; exit 1; }
    echo "# definitions of '$q':"
    G "^[[:space:]]*(pub )?(async )?(unsafe )?(fn|struct|enum|trait|const|static|type|impl)[^A-Za-z0-9_]+${q}\b" "$SRC" \
      | sed -E "s#$SRC/##" | head -n "$CAP"
    n=$(G -c "\b${q}\b" "$SRC" 2>/dev/null | awk -F: '{s+=$2} END{print s+0}')
    echo "# ~$n total references in $SRC"
    ;;
  env)
    if [ "$q" = "-l" ] || [ "$q" = "" ]; then
      echo "# all UNENDLICH_* flags:"
      grep -rhoE 'UNENDLICH_[A-Z0-9_]+' "$SRC" --include=*.rs | sort -u
      exit 0
    fi
    echo "# reads of '$q':"
    G "\"${q}\"" "$SRC" --include=*.rs 2>/dev/null | sed -E "s#$SRC/##" \
      | sed -E 's/:[0-9]+:/&\t/' | cut -f1- | while IFS= read -r l; do echo "$l" | trim; done | head -n "$CAP"
    ;;
  doc)
    [ -z "$q" ] && { echo "usage: xref.sh doc <kw>"; exit 1; }
    echo "# docs mentioning '$q' (file: first matching line):"
    for d in docs handoff . ; do
      G -il "$q" "$d"/*.md 2>/dev/null || true
    done | sort -u | head -n "$CAP" | while IFS= read -r f; do
      line=$(grep -niE "$q" "$f" 2>/dev/null | head -1 | trim)
      echo "$f : $line"
    done
    ;;
  find)
    [ -z "$q" ] && { echo "usage: xref.sh find <kw>"; exit 1; }
    G "$q" "$SRC" --include=*.rs 2>/dev/null | sed -E "s#$SRC/##" \
      | while IFS= read -r l; do f=${l%%:*}; rest=${l#*:}; ln=${rest%%:*}; txt=$(echo "${rest#*:}" | trim);
          printf '%s:%s  %.100s\n' "$f" "$ln" "$txt"; done | head -n "$CAP"
    ;;
  file)
    [ -z "$q" ] && { echo "usage: xref.sh file <frag>"; exit 1; }
    find . -path ./target -prune -o -type f -name '*'"$q"'*' -print 2>/dev/null \
      | sed 's#^\./##' | grep -v '^target/' | head -n "$CAP"
    ;;
  *)
    sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
    ;;
esac
