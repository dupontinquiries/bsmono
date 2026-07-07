#!/usr/bin/env bash
# merge_bricks.sh — merge several parallel-session "bricks" into one branch.
#
# Each brick is an apply_from_zip_v2-style patch zip (patches/NNNN-*.patch +
# MANIFEST.env). This tool:
#   1. reads every brick's touched-file set and reports OVERLAPS between bricks
#      (and, if lanes.conf + python3 are present, the lanes each brick touches)
#      BEFORE applying anything — so you see collisions up front;
#   2. applies the bricks IN THE ORDER GIVEN onto <base> via `git am --3way`,
#      in a throwaway `git worktree` (your working tree is never touched);
#   3. stops on the first brick that conflicts, naming the brick + files, and
#      leaves the worktree at the last clean state for inspection.
#
# Usage:
#   merge_bricks.sh --base <ref> [--out <branch>] [--repo <dir>] \
#                   [--strict-lanes] [--snapshot] brickA.zip brickB.zip ...
#
#   --base <ref>     common ancestor to apply onto (required; e.g. a tag/sha).
#   --out <branch>   name for the merged branch (default: merge/<timestamp>).
#   --repo <dir>     repo (default: $UNENDLICH_REPO, else cwd).
#   --strict-lanes   treat any cross-lane / contested file as a hard error.
#   --snapshot       on full success, write <branch>.zip (git archive).
#
# Bricks apply in argument order, so list the most foundational session first.
set -uo pipefail

REPO="${UNENDLICH_REPO:-$PWD}"
BASE=""; OUT=""; STRICT=0; SNAP=0
BRICKS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --base) BASE="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    --repo) REPO="$2"; shift 2;;
    --strict-lanes) STRICT=1; shift;;
    --snapshot) SNAP=1; shift;;
    -h|--help) sed -n '2,30p' "$0"; exit 0;;
    -*) echo "unknown option: $1" >&2; exit 2;;
    *) BRICKS+=("$1"); shift;;
  esac
done

die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
log() { printf '  %s\n' "$*"; }
hr()  { printf -- '----------------------------------------------------------------\n'; }

[ -n "$BASE" ] || die "need --base <ref>"
[ "${#BRICKS[@]}" -ge 1 ] || die "need at least one brick zip"
[ -d "$REPO/.git" ] || die "not a git repo: $REPO"
command -v git >/dev/null || die "git not on PATH"
command -v unzip >/dev/null || die "unzip not on PATH"
REPO="$(cd "$REPO" && pwd)"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LANES="$HERE/lanes.conf"
[ -n "$OUT" ] || OUT="merge/$(date +%Y%m%d-%H%M%S)"

git -C "$REPO" cat-file -e "${BASE}^{commit}" 2>/dev/null \
  || die "base '$BASE' not found in $REPO (fetch/checkout it first)"

WORK="$(mktemp -d)"; EXTRACT="$WORK/extract"; mkdir -p "$EXTRACT"
WT="$WORK/worktree"
cleanup() {
  git -C "$REPO" worktree remove --force "$WT" 2>/dev/null || true
  rm -rf "$WORK"
}
# Keep the worktree on success (user wants the result); only scrub temp extract.
trap 'rm -rf "$EXTRACT"' EXIT

# ---- unpack each brick: patches dir + touched-file set --------------------
declare -a NAMES PATCHDIRS FILELISTS
i=0
for z in "${BRICKS[@]}"; do
  [ -f "$z" ] || die "brick not found: $z"
  d="$EXTRACT/$i"; mkdir -p "$d"
  unzip -q "$z" -d "$d" || die "could not unzip $z"
  pd="$(dirname "$(find "$d" -name MANIFEST.env -print -quit)")"
  [ -d "$pd/patches" ] || die "no patches/ in $(basename "$z")"
  fl="$d/files.txt"
  grep -hE '^\+\+\+ b/|^--- a/' "$pd"/patches/*.patch 2>/dev/null \
    | sed -E 's#^\+\+\+ b/##; s#^--- a/##' | sort -u | grep -v '^/dev/null$' > "$fl"
  NAMES+=("$(basename "$z")"); PATCHDIRS+=("$pd/patches"); FILELISTS+=("$fl")
  i=$((i+1))
done
N=${#NAMES[@]}

# ---- pre-merge report: files + lanes + overlaps ---------------------------
printf '== merge_bricks: %d brick(s) onto %s ==\n' "$N" "$BASE"
for j in $(seq 0 $((N-1))); do
  cnt=$(wc -l < "${FILELISTS[$j]}" | tr -d ' ')
  log "[$j] ${NAMES[$j]} — $cnt file(s)"
done
hr
if command -v python3 >/dev/null && [ -f "$LANES" ]; then
  echo "lanes touched per brick:"
  for j in $(seq 0 $((N-1))); do
    echo "  [$j] ${NAMES[$j]}"
    python3 "$HERE/lane_check.py" --repo "$REPO" --patches "${PATCHDIRS[$j]}"/*.patch \
      2>/dev/null | sed -n 's/^  \(\[.*\]\) \([0-9].*\)/      \1 \2/p'
  done
  hr
fi

echo "pairwise file OVERLAPS (likely conflict sites):"
overlap_found=0
for a in $(seq 0 $((N-1))); do
  for b in $(seq $((a+1)) $((N-1))); do
    [ "$b" -lt "$N" ] || continue
    common="$(comm -12 "${FILELISTS[$a]}" "${FILELISTS[$b]}")"
    if [ -n "$common" ]; then
      overlap_found=1
      echo "  [$a] ${NAMES[$a]}  <->  [$b] ${NAMES[$b]}:"
      echo "$common" | sed 's/^/      /'
    fi
  done
done
[ "$overlap_found" = 0 ] && echo "  (none — bricks touch disjoint files)"
if [ "$overlap_found" = 1 ] && [ "$STRICT" = 1 ]; then
  die "--strict-lanes: bricks overlap on files (see above). Resolve ordering/lane split first."
fi
hr

# ---- apply in order in a throwaway worktree -------------------------------
git -C "$REPO" worktree add -q -b "$OUT" "$WT" "$BASE" \
  || die "could not create worktree/branch '$OUT' (does it already exist? use --out)"
log "worktree: $WT"
log "branch:   $OUT (off $BASE)"
hr

fail=0
for j in $(seq 0 $((N-1))); do
  echo ">> applying [$j] ${NAMES[$j]}"
  # shellcheck disable=SC2046
  if git -C "$WT" am --3way --keep-cr $(ls "${PATCHDIRS[$j]}"/*.patch | sort) >/dev/null 2>"$WORK/amerr"; then
    log "OK"
  else
    echo "   !! CONFLICT applying ${NAMES[$j]}"
    git -C "$WT" diff --name-only --diff-filter=U 2>/dev/null | sed 's/^/      conflict: /'
    sed 's/^/      /' "$WORK/amerr" | tail -8
    git -C "$WT" am --abort 2>/dev/null || true
    fail=1
    break
  fi
done
hr

if [ "$fail" = 0 ]; then
  merged="$(git -C "$WT" rev-parse --short HEAD)"
  printf '== MERGED OK ==\n'
  log "branch '$OUT' is at $merged"
  log "inspect: git -C \"$REPO\" log --oneline $BASE..$OUT"
  if [ "$SNAP" = 1 ]; then
    snap="$REPO/${OUT//\//-}.zip"
    git -C "$WT" archive --format=zip --prefix=unendlich/ -o "$snap" HEAD
    log "snapshot: $snap"
  fi
  log "worktree kept at: $WT   (remove with: git -C \"$REPO\" worktree remove \"$WT\")"
  trap - EXIT; rm -rf "$EXTRACT"
  exit 0
else
  printf '== MERGE STOPPED ==\n'
  log "brick above conflicts onto the current stack. Options:"
  log "  * reorder bricks (list it earlier/later)"
  log "  * split the overlapping file into separate lanes"
  log "  * have one session rebase its brick onto the other's change"
  cleanup
  exit 1
fi
