#!/usr/bin/env bash
# zips_to_history.sh — turn a directory of snapshot zips into ordered commits.
#
# The unendlich drops are ~86 zips named unendlich-r<rev>-<slug>.zip, each a
# snapshot of the tree at that point. Replayed in revision order into a git
# repo they become real commits with real diffs — history that was never
# recorded, recovered from the artifacts that survived.
#
# WHAT IT REFUSES TO GUESS
#
# It does not assume a zip is a FULL snapshot. If a drop contains only the
# files that changed, replaying it with deletion enabled would delete the rest
# of the tree and manufacture a diff that never happened. The script reports
# the file count of each drop against the previous one and leaves --delete off
# unless you pass it, because a wrong answer here corrupts every later diff and
# looks plausible while doing it.
#
# ORDERING TRAPS IN THE REAL FILENAMES, all handled:
#
#   unendlich-v0.99.0-r605-dragon-energy.zip   rev is not the first number
#   unendlich-r594-turning-ragdoll.zip         two drops share r594 —
#   unendlich-r594-wire-foliage-sway-tag.zip     sorted by name after rev
#   unendlich-r476-mesh-settings.zip           three share r476
#   unendlich-r785-...CHECKED (1).zip          a byte-identical duplicate
#   701.log.zip, err.zip, r476.log.zip         not drops; filtered out
#
# STATUS IS DATA. UNVERIFIED / CHECKED / VERIFIED / FIX in a filename is the
# author saying how much they trusted that drop. It goes in a commit trailer
# rather than being dropped, because "which of these were verified" is exactly
# the question a later reader asks.
#
# Usage:
#   ./zips_to_history.sh --src ~/post-fable-wiring --out /tmp/unendlich-history
#   ./zips_to_history.sh --src ... --out ... --apply          # actually commit
#   ./zips_to_history.sh --src ... --out ... --apply --delete # full snapshots
set -euo pipefail

SRC=""; OUT=""; APPLY=0; DELETE=0
# Top-level directories dropped from every drop before it is committed.
#
# art-targets is reference art — 14 PNGs, uncompressed in the original zips at
# dozens of MB and still 5.4 MB after compression. It is not source, it barely
# changes, and a binary that appears in most drops is the worst thing to put in
# a delta-compressed history: git stores each near-identical revision at close
# to full size. Carrying it would cost more than the entire replayed history of
# actual code.
EXCLUDE=("art-targets")
EXCLUDE_SET=0
while [ $# -gt 0 ]; do
  case "$1" in
    --src) SRC="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --apply) APPLY=1; shift ;;
    --delete) DELETE=1; shift ;;
    # Repeatable. The first --exclude replaces the default rather than adding
    # to it, so a caller who wants art-targets kept can say so.
    --exclude) if [ "$EXCLUDE_SET" -eq 0 ]; then EXCLUDE=(); EXCLUDE_SET=1; fi
               EXCLUDE+=("$2"); shift 2 ;;
    --no-exclude) EXCLUDE=(); EXCLUDE_SET=1; shift ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done
[ -n "$SRC" ] && [ -n "$OUT" ] || { echo "usage: $0 --src DIR --out DIR [--apply] [--delete]" >&2; exit 2; }
[ -d "$SRC" ] || { echo "no such dir: $SRC" >&2; exit 2; }
# Absolute, because the replay cd's into $OUT and every later reference to
# $SRC would then resolve against the wrong directory. Caught by running it:
# the dry run passed and --apply failed on the first unzip.
SRC="$(cd "$SRC" && pwd)"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"

command -v unzip >/dev/null || { echo "need unzip" >&2; exit 2; }
command -v git   >/dev/null || { echo "need git" >&2; exit 2; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# ── 1. select and parse ──────────────────────────────────────────────────────
# Only unendlich-*.zip. The log and err zips in the same directory are not
# drops, and a glob loose enough to catch them would silently interleave
# garbage commits into the history.
: > "$WORK/index"
shopt -s nullglob
for z in "$SRC"/unendlich-*.zip; do
  base="$(basename "$z")"
  # Rev = the LAST -r<digits>- in the name, so v0.99.0-r605 resolves to 605
  # rather than to the 0 in the version.
  rev="$(printf '%s' "$base" | grep -oE '\-r[0-9]+' | tail -1 | tr -dc '0-9')"
  [ -n "$rev" ] || { echo "skip (no revision): $base" >&2; continue; }
  status="$(printf '%s' "$base" | grep -oiE 'UNVERIFIED|VERIFIED[A-Z-]*|CHECKED|FIX' | head -1 || true)"
  sha="$(sha256sum "$z" | cut -c1-16)"
  printf '%s\t%s\t%s\t%s\n' "$rev" "$base" "${status:-none}" "$sha" >> "$WORK/index"
done
shopt -u nullglob

[ -s "$WORK/index" ] || { echo "no unendlich-*.zip found in $SRC" >&2; exit 1; }

# Numeric by revision, then by filename so the two r594 drops land in a stable,
# reproducible order rather than whatever the filesystem returned.
sort -t"$(printf '\t')" -k1,1n -k2,2 "$WORK/index" > "$WORK/sorted"

# Drop byte-identical duplicates — the "(1)" copy. Same bytes cannot be a
# different commit, and replaying it produces an empty one.
#
# Prefer the canonical name when choosing which copy to keep: a plain sort puts
# "...CHECKED (1).zip" BEFORE "...CHECKED.zip", because space sorts below dot,
# so the naive version recorded the duplicate's name as the provenance and the
# original's as the discard. The commit would have been right and the
# Source-Zip trailer subtly wrong.
awk -F'\t' '{ dup = ($2 ~ / \([0-9]+\)\.zip$/) ? 1 : 0; print dup "\t" $0 }' "$WORK/sorted" \
  | sort -t"$(printf '\t')" -k2,2n -k1,1n -k3,3 \
  | cut -f2- \
  | awk -F'\t' '!seen[$4]++' > "$WORK/plan"
dupes=$(( $(wc -l < "$WORK/sorted") - $(wc -l < "$WORK/plan") ))

if [ ${#EXCLUDE[@]} -gt 0 ]; then
  echo "excluding:  ${EXCLUDE[*]} (top level, every drop)"
fi
echo "drops:      $(wc -l < "$WORK/plan")"
echo "duplicates: $dupes (identical bytes, skipped)"
echo

# ── 2. report the plan ───────────────────────────────────────────────────────
prev_count=0
while IFS=$'\t' read -r rev base status sha; do
  # Counted with the exclusions applied, or a drop that is mostly art reads
  # as larger than it is and the PARTIAL heuristic below misfires.
  n="$(unzip -Z1 "$SRC/$base" 2>/dev/null | grep -v '/$' \
       | sed -E 's#^[^/]+/##' \
       | { if [ ${#EXCLUDE[@]} -gt 0 ]; then grep -vE "^($(IFS='|'; echo "${EXCLUDE[*]}"))/" || true; else cat; fi; } \
       | wc -l)"
  flag=""
  # A drop with far fewer files than the last one is the shape of a partial
  # delivery, and partial + --delete is what silently corrupts the history.
  if [ "$prev_count" -gt 0 ] && [ "$n" -lt $(( prev_count / 2 )) ]; then
    flag="  <-- ${n} files vs ${prev_count}: possibly PARTIAL, not a snapshot"
  fi
  printf 'r%-5s %-8s %5s files  %s%s\n' "$rev" "$status" "$n" "$base" "$flag"
  prev_count="$n"
done < "$WORK/plan"

if [ "$APPLY" -eq 0 ]; then
  echo
  echo "dry run. Check the ordering and the PARTIAL warnings above, then re-run"
  echo "with --apply. Add --delete only if every drop is a full snapshot —"
  echo "with partial drops it will delete files that were never removed."
  exit 0
fi

# ── 3. replay ────────────────────────────────────────────────────────────────
mkdir -p "$OUT"
cd "$OUT"
[ -d .git ] || { git init -q .; }
# A fresh repo on a machine with no global identity cannot commit at all, and
# the failure lands on the first drop after the script has already extracted
# it — confusing, and it happened the first time this was run. Set a local
# identity only when one is not already configured, so a real repo keeps its.
git config user.name  >/dev/null 2>&1 || git config user.name  "zip-replay"
git config user.email >/dev/null 2>&1 || git config user.email "zip-replay@localhost"
[ "$(git rev-list --count --all 2>/dev/null || echo 0)" -gt 0 ] || { git commit -q --allow-empty -m "unendlich: empty root

Replayed from snapshot zips by maintenance/tools/zips_to_history.sh. The
commits after this one are reconstructions: their trees are real, their
diffs are real, and their timestamps are the zip mtimes rather than when
the work happened."; }

: > "$OUT/../manifest.jsonl" 2>/dev/null || true
MANIFEST="$OUT/.git/replay-manifest.jsonl"
: > "$MANIFEST"

while IFS=$'\t' read -r rev base status sha; do
  rm -rf "$WORK/x"; mkdir -p "$WORK/x"
  unzip -qq -o "$SRC/$base" -d "$WORK/x"

  # Zips usually wrap everything in one top directory; strip it so the tree
  # does not gain a level per drop. Only when there is exactly one.
  root="$WORK/x"
  if [ "$(find "$WORK/x" -mindepth 1 -maxdepth 1 | wc -l)" -eq 1 ] \
     && [ -d "$(find "$WORK/x" -mindepth 1 -maxdepth 1)" ]; then
    root="$(find "$WORK/x" -mindepth 1 -maxdepth 1)"
  fi

  if [ "$DELETE" -eq 1 ]; then
    # Everything except .git, so a removed file shows up as a deletion.
    find "$OUT" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
  fi
  cp -a "$root/." "$OUT/"

  # Removed after the copy rather than filtered during it: unzip has no
  # exclude that survives the root-strip cleanly, and deleting a known
  # top-level path is easier to verify than a pattern that has to be right.
  for ex in ${EXCLUDE[@]+"${EXCLUDE[@]}"}; do
    [ -e "$OUT/$ex" ] && rm -rf "${OUT:?}/$ex"
  done

  git add -A
  if git diff --cached --quiet; then
    echo "r$rev: no change, skipped"
    continue
  fi

  mtime="$(date -u -r "$SRC/$base" +%Y-%m-%dT%H:%M:%S 2>/dev/null || date -u +%Y-%m-%dT%H:%M:%S)"
  slug="$(printf '%s' "$base" | sed -E 's/^unendlich-(v[0-9.]+-)?r[0-9]+-//; s/\.zip$//; s/ \([0-9]+\)$//')"

  # Trailers are machine-readable on purpose: commit_corpus.py and anything
  # else can recover rev, provenance and the author's own confidence without
  # parsing prose.
  GIT_AUTHOR_DATE="$mtime" GIT_COMMITTER_DATE="$mtime" \
  git commit -q -m "unendlich r$rev: $slug

Replayed from a snapshot zip; the diff is against the previous drop, so
it shows what changed between deliveries rather than what any single
edit did.

Rev: $rev
Source-Zip: $base
Zip-SHA256: $sha
Author-Status: $status
Reconstructed: true"

  printf '{"rev":%s,"zip":"%s","status":"%s","zip_sha":"%s","commit":"%s"}\n' \
    "$rev" "$base" "$status" "$sha" "$(git rev-parse HEAD)" >> "$MANIFEST"
  echo "r$rev committed  $(git rev-parse --short HEAD)  $slug"
done < "$WORK/plan"

echo
echo "commits: $(git rev-list --count HEAD)"
echo "manifest: $MANIFEST"
echo
echo "These are RECONSTRUCTIONS and say so in every message. The trees are"
echo "real and the diffs are real; the timestamps are zip mtimes, and each"
echo "diff spans a whole delivery rather than one edit."
