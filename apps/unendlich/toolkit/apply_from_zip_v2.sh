#!/usr/bin/env bash
# apply_from_zip_v2 — apply a unendlich "patch zip" onto your game repo.
#
# A patch zip contains:
#   patches/NNNN-*.patch   git format-patch series (in order)
#   MANIFEST.env           base=<sha>  branch=<name>  desc=<text>
#   TAGS                   one tag per line, same order as the patches
#   apply_from_zip_v2.sh   (this script, for convenience)
#
# It applies the series with `git am --3way` (so each commit keeps its message
# and authorship), then tags the new commits positionally from TAGS. Non-
# destructive preflight: refuses on a dirty tree or wrong base. Roll back with
# the printed `git reset --hard`.
#
# Usage:  apply_from_zip_v2.sh <patch-zip> [repo-dir]
#   repo-dir defaults to $UNENDLICH_REPO, else
#   ~/Code/unendlich-development/unendlich-game
set -euo pipefail

ZIP="${1:?usage: apply_from_zip_v2.sh <patch-zip> [repo-dir]}"
REPO="${2:-${UNENDLICH_REPO:-$HOME/Code/unendlich-development/unendlich-game}}"

log()  { printf '  %s\n' "$*"; }
die()  { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

[ -f "$ZIP" ] || die "patch zip not found: $ZIP"
ZIP="$(cd "$(dirname "$ZIP")" && pwd)/$(basename "$ZIP")"   # absolutize before cd
[ -d "$REPO/.git" ] || die "not a git repo: $REPO"
command -v git >/dev/null || die "git not on PATH"

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
unzip -q "$ZIP" -d "$TMP" || die "could not unzip $ZIP"

# locate the manifest (zip may or may not have a wrapping top dir)
ROOT="$TMP"
if [ ! -f "$ROOT/MANIFEST.env" ]; then
  found="$(find "$TMP" -name MANIFEST.env -print -quit 2>/dev/null || true)"
  [ -n "$found" ] || die "MANIFEST.env not found in zip"
  ROOT="$(dirname "$found")"
fi
# parse MANIFEST.env safely (do NOT source it — values may contain spaces/parens)
manifest_get() { grep -E "^$1=" "$ROOT/MANIFEST.env" 2>/dev/null | head -1 | cut -d= -f2-; }
base="$(manifest_get base)"
branch="$(manifest_get branch)"
desc="$(manifest_get desc)"

PATCHDIR="$ROOT/patches"
[ -d "$PATCHDIR" ] || die "patches/ directory not found in zip"
# shellcheck disable=SC2012
patches="$(ls "$PATCHDIR"/*.patch 2>/dev/null | sort || true)"
[ -n "$patches" ] || die "no .patch files under patches/"

cd "$REPO"

printf '== apply_from_zip_v2 ==\n'
log "repo:    $REPO"
log "zip:     $(basename "$ZIP")"
log "desc:    ${desc:-<none>}"
log "base:    ${base:-<unset>}"
log "branch:  ${branch:-<unset>}"

# ---- preflight: clean tree ----
if ! git diff --quiet || ! git diff --cached --quiet; then
  die "working tree is dirty — commit or stash before applying."
fi

cur_branch="$(git rev-parse --abbrev-ref HEAD)"
if [ -n "$branch" ] && [ "$cur_branch" != "$branch" ]; then
  log "WARNING: you are on '$cur_branch' but the patch was built for '$branch'."
fi

# ---- preflight: base ancestry ----
if [ -n "$base" ]; then
  if git cat-file -e "${base}^{commit}" 2>/dev/null; then
    git merge-base --is-ancestor "$base" HEAD \
      || die "base $base is not an ancestor of HEAD (wrong branch or checkout)."
  else
    log "WARNING: base $base not present locally — applying anyway."
  fi
fi

BEFORE="$(git rev-parse HEAD)"
npatch="$(printf '%s\n' "$patches" | grep -c . || true)"
log "applying $npatch patch(es) via git am --3way ..."

# shellcheck disable=SC2086
if ! git am --3way --keep-cr $patches; then
  git am --abort 2>/dev/null || true
  die "git am failed (the patch conflicts with your tree). Nothing was applied; see the failing patch above."
fi

AFTER="$(git rev-parse HEAD)"
new_commits=()
while IFS= read -r sha; do new_commits+=("$sha"); done < <(git rev-list --reverse "${BEFORE}..${AFTER}")
log "applied ${#new_commits[@]} commit(s): ${BEFORE:0:7}..${AFTER:0:7}"

# ---- tag positionally from TAGS ----
if [ -f "$ROOT/TAGS" ]; then
  tags=()
  while IFS= read -r t; do
    case "$t" in ''|'#'*) ;; *) tags+=("$t") ;; esac
  done < "$ROOT/TAGS"
  if [ "${#tags[@]}" -eq "${#new_commits[@]}" ]; then
    for i in "${!tags[@]}"; do
      git tag -f "${tags[$i]}" "${new_commits[$i]}" >/dev/null
      log "tagged ${tags[$i]} -> ${new_commits[$i]:0:7}"
    done
  else
    log "WARNING: TAGS lists ${#tags[@]} tag(s) but ${#new_commits[@]} commit(s) were applied — skipping tagging."
  fi
fi

printf '== done. HEAD is now %s ==\n' "$(git rev-parse --short HEAD)"
log "verify: cargo build   (or ./toolkit/apply_and_test.sh)"
log "undo:   git reset --hard $BEFORE   &&   git tag -d <any new tags>"
