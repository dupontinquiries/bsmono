#!/usr/bin/env bash
# survey-bundle.sh — "self-contained or incremental, and do I have the
# prerequisites?", answered WITHOUT fetching anything.
#
# A bundle's header is plain text up to the first empty line: a signature
# ("# v2 git bundle" / "# v3 git bundle"), optional @capability lines (v3),
# prerequisite lines ("-<sha> [comment]"), then "<sha> <refname>" lines.
# Everything after the empty line is pack data, so reading stops there and
# the 300+ MiB tail is never touched. That is the point of this script:
# maintenance/INCOMING_BUNDLE_REBASE.md records what it costs to discover a
# bundle's prerequisites AFTER history was rewritten, when the hash it names
# no longer exists anywhere.
#
# usage: survey-bundle.sh <bundle> [repo]
#   With a repo (default: the enclosing one, if any), each prerequisite is
#   checked for local presence — the "can I actually apply this" answer.
#
# Exit codes: 0 surveyed (even if not applyable — the survey succeeded),
#             1 usage / unreadable / not a bundle.
set -euo pipefail

BUNDLE="${1:?usage: survey-bundle.sh <bundle> [repo]}"
REPO="${2:-}"
[ -r "$BUNDLE" ] || { echo "cannot read: $BUNDLE" >&2; exit 1; }

sig=$(head -c 32 "$BUNDLE" | head -n 1 || true)
case "$sig" in
  "# v2 git bundle"|"# v3 git bundle") ;;
  *) echo "not a git bundle (signature: ${sig:-<empty>})" >&2; exit 1 ;;
esac

# Header only: stop at the first empty line. awk reads the binary tail's
# first buffer at most and exits before printing any of it.
header=$(awk 'NR==1 { next } /^$/ { exit } { print }' "$BUNDLE")

prereqs=$(printf '%s\n' "$header" | sed -n 's/^-\([0-9a-f]\{40,64\}\).*/\1/p')
refs=$(printf '%s\n' "$header" | grep -c '^[0-9a-f]\{40,64\} ' || true)
nprereq=0; [ -n "$prereqs" ] && nprereq=$(printf '%s\n' "$prereqs" | wc -l)

size=$(stat -c %s "$BUNDLE")
echo "bundle:   $BUNDLE"
echo "format:   ${sig#\# }"
echo "size:     $(( size / 1024 / 1024 )) MiB ($size bytes)"
echo "refs:     $refs"
printf '%s\n' "$header" | sed -n 's/^[0-9a-f]\{40,64\} \(.*\)/          \1/p'

# refs/notes/toolcalls is easy to lose (git bundle --all does not include it;
# export-bundle.sh appends it explicitly). A bundle without it looks, on the
# receiving side, exactly like a repo where capture was never enabled —
# failure mode #3 in SKILL_session_capture.md — so its absence is called out
# rather than left to be noticed in the ref list.
if printf '%s\n' "$header" | grep -q ' refs/notes/toolcalls$'; then
  echo "notes:    refs/notes/toolcalls INCLUDED"
else
  echo "notes:    refs/notes/toolcalls ABSENT — observed_calls will be null on the far side"
fi

if [ "$nprereq" -eq 0 ]; then
  echo "kind:     SELF-CONTAINED (no prerequisites)"
  exit 0
fi

echo "kind:     INCREMENTAL ($nprereq prerequisite(s))"

# Presence check, only meaningful against a repo. No repo argument and not
# inside one -> report the shas and say the check was not possible.
if [ -z "$REPO" ]; then
  REPO=$(git rev-parse --show-toplevel 2>/dev/null || true)
fi
if [ -z "$REPO" ]; then
  printf '%s\n' "$prereqs" | sed 's/^/          needs /'
  echo "verdict:  cannot check presence (no repo here; pass one as arg 2)"
  exit 0
fi

missing=0
while IFS= read -r sha; do
  if git -C "$REPO" cat-file -e "$sha^{commit}" 2>/dev/null; then
    echo "          have    $sha"
  else
    echo "          MISSING $sha"
    missing=$(( missing + 1 ))
  fi
done <<< "$prereqs"

if [ "$missing" -eq 0 ]; then
  echo "verdict:  applyable against $REPO"
else
  echo "verdict:  NOT applyable against $REPO ($missing prerequisite(s) missing)"
fi
