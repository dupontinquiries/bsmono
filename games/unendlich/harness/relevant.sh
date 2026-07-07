#!/usr/bin/env bash
# relevant.sh — "where should I look?" relevancy tool for the unendlich codebase.
#
# Given topic keywords it returns:
#   (1) CURATED glossary hits  — concept -> the systems/docs/files that handle it
#       (handles synonyms the filenames don't: "test gpu" -> vk-harness).
#   (2) LIVE hits              — matching filenames + docs/source mentioning the terms
#       (self-updating: surfaces new files the glossary doesn't list yet).
#
# The point: call this BEFORE starting work on a topic, so existing infra (a
# design doc, a CPU reference, a harness like vk-harness) is found, not reinvented
# or missed. Cheap + deterministic; no build needed.
#
# Usage:  bash harness/relevant.sh <keywords...>
#   e.g.  bash harness/relevant.sh gpu shader test
#         bash harness/relevant.sh lod culling distance
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
GLO="$HERE/glossary.tsv"
[ $# -ge 1 ] || { echo "usage: relevant.sh <keywords...>"; exit 2; }
terms="$(printf '%s ' "$@" | tr 'A-Z' 'a-z' | tr -cs 'a-z0-9' ' ')"
alt="$(echo "$terms" | tr -s ' ' '\n' | grep -v '^$' | paste -sd'|')"

echo "== curated glossary (concept → where it lives) =="
if [ -f "$GLO" ]; then
  awk -F'\t' -v terms="$terms" '
    BEGIN{ n=split(terms,T," ") }
    NF>=3 {
      hay=tolower($1" "$2); s=0
      for(i=1;i<=n;i++) if(T[i]!="" && index(hay,T[i])) s++
      if(s>0) printf "%d\t%s\t%s\t%s\n", s,$1,$2,$3
    }' "$GLO" | sort -rn | head -8 | while IFS="$(printf '\t')" read -r sc kw sum loc; do
      printf "  • %s\n      %s\n      → %s\n" "$kw" "$sum" "$loc"
    done
  awk -F'\t' -v terms="$terms" 'BEGIN{n=split(terms,T," ")} NF>=3{hay=tolower($1" "$2);for(i=1;i<=n;i++)if(T[i]!=""&&index(hay,T[i])){print;next}}' "$GLO" | grep -q . || echo "  (no curated entry — see live hits below; consider adding one)"
else
  echo "  (no glossary.tsv)"
fi

echo
echo "== live hits: matching filenames =="
find "$ROOT/crates" "$ROOT/handoff" "$ROOT/docs" -type f 2>/dev/null \
  | grep -iE "/[^/]*(${alt})[^/]*\.(rs|comp|vert|frag|glsl|md|py|sh)$" \
  | sed "s#$ROOT/##" | sort -u | head -20 | sed 's/^/  /'

echo
echo "== live hits: docs/source mentioning the terms =="
grep -rilE "$alt" "$ROOT/handoff" "$ROOT/docs" 2>/dev/null \
  | sed "s#$ROOT/##" | sort -u | head -15 | sed 's/^/  /'
