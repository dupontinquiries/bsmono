#!/usr/bin/env bash
# probe-bundle.sh — what would fetching this bundle ACTUALLY add to a repo?
#
# A bundle's file size is not its cost: a self-contained bundle re-ships the
# whole history, and a repo that already shares most of it pays only for the
# objects it lacks. In c9 this measurement turned a 369 MiB bundle into a
# measured 888 KiB and stopped two large ones from silently undoing the
# history rewrite (REPO_SIZE.md).
#
# Mechanism: a bare scratch repo whose objects/info/alternates points at the
# target repo's object store. Fetching from the bundle lands the bundle's
# pack in the scratch; `git repack -a -d -l` then keeps only objects NOT
# reachable through the alternate (-l = local). Whatever pack bytes remain
# are the true incremental cost. The scratch transiently holds the bundle's
# full pack, so free disk must cover the bundle size; checked up front.
#
# usage: probe-bundle.sh <bundle> [repo]   (repo defaults to the enclosing one)
# Exit codes: 0 measured, 1 usage/environment, 2 fetch failed (typically
#             missing prerequisites — survey-bundle.sh names them),
#             3 repack failed so nothing was measured (see the note below).
set -euo pipefail

BUNDLE="${1:?usage: probe-bundle.sh <bundle> [repo]}"
REPO="${2:-$(git rev-parse --show-toplevel 2>/dev/null || true)}"
[ -r "$BUNDLE" ] || { echo "cannot read: $BUNDLE" >&2; exit 1; }
[ -n "$REPO" ] || { echo "no repo: pass one as arg 2 or run inside one" >&2; exit 1; }

objects=$(git -C "$REPO" rev-parse --absolute-git-dir)/objects
[ -d "$objects" ] || { echo "no object store at $objects" >&2; exit 1; }

bsize=$(stat -c %s "$BUNDLE")
free=$(df --output=avail -B1 "${TMPDIR:-/tmp}" | tail -1)
if [ "$free" -lt $(( bsize * 2 )) ]; then
  echo "not enough free space in ${TMPDIR:-/tmp}: need ~2x bundle ($(( bsize / 1024 / 1024 )) MiB), have $(( free / 1024 / 1024 )) MiB" >&2
  exit 1
fi

scratchdir=$(mktemp -d)
trap 'rm -rf "$scratchdir"' EXIT
scratch="$scratchdir/probe.git"
git init -q --bare "$scratch"
printf '%s\n' "$objects" > "$scratch/objects/info/alternates"

# Refspecs are built from what the bundle actually lists, because a wildcard
# is a trap: a bundle recording only HEAD (e.g. `git bundle create x HEAD
# ^base`) matches nothing under +refs/*:refs/*, and the fetch then SUCCEEDS
# having fetched zero refs — the probe printed "0 bytes" for a bundle it
# never measured. Found by testing the failure path, not by reading docs.
refspecs=()
while read -r _sha ref; do
  case "$ref" in
    refs/*) refspecs+=("+$ref:$ref") ;;
    HEAD)   refspecs+=("+HEAD:refs/bundle-HEAD") ;;
  esac
done < <(git bundle list-heads "$BUNDLE")
[ "${#refspecs[@]}" -gt 0 ] || { echo "bundle lists no fetchable refs" >&2; exit 2; }

if ! git -C "$scratch" fetch -q --no-tags "$BUNDLE" "${refspecs[@]}" 2> "$scratchdir/fetch.err" < /dev/null; then
  echo "fetch from bundle FAILED — usually missing prerequisites; survey-bundle.sh names them:" >&2
  sed 's/^/  /' "$scratchdir/fetch.err" >&2
  exit 2
fi
fetched=$(git -C "$scratch" for-each-ref | wc -l)
if [ "$fetched" -eq 0 ]; then
  echo "fetch reported success but landed no refs — refusing to report a cost" >&2
  exit 2
fi

# STDIN CLOSED, the same hazard export-bundle.sh documents on `git gc`.
#
# KNOWN FAILURE, unresolved: on git 2.43.0 in this sandbox, repacking a scratch
# that received a LARGE amount of genuinely new content dies with
#   fatal: bad revision 'pack<TAB><sha>'
# from the `pack-objects` child (confirmed with GIT_TRACE), stdin closed or
# not, with or without `-l`, `-d`, alternates, or bitmaps. It does NOT fire
# when the bundle adds little or nothing, which is why this tool passed its
# tests and its first real bundle and only broke on the second. The cost
# measurement is the only casualty — survey-bundle.sh is unaffected, and
# fetching the bundle for real does not go through this path.
if ! git -C "$scratch" repack -a -d -l -q < /dev/null 2>"$scratchdir/repack.err"; then
  echo "repack FAILED, so the incremental cost was NOT measured:" >&2
  sed 's/^/  /' "$scratchdir/repack.err" >&2
  echo "known on git 2.43.0 here for bundles with a lot of new content; see the comment in $0" >&2
  exit 3
fi

cost=0
for p in "$scratch"/objects/pack/*.pack; do
  [ -f "$p" ] || continue
  cost=$(( cost + $(stat -c %s "$p") ))
done

echo "bundle:            $BUNDLE"
echo "bundle size:       $(( bsize / 1024 / 1024 )) MiB ($bsize bytes)"
echo "repo:              $REPO"
if [ "$cost" -lt 1048576 ]; then
  echo "incremental cost:  $(( cost / 1024 )) KiB ($cost bytes)"
else
  echo "incremental cost:  $(( cost / 1024 / 1024 )) MiB ($cost bytes)"
fi
