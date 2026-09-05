#!/usr/bin/env bash
# export-bundle.sh — emit a self-contained c9-NNN-<summary>.bundle.
#
# Self-contained is the point: maintenance/INCOMING_BUNDLE_REBASE.md records
# what an incremental bundle costs once history is rewritten (the prerequisite
# hash stops existing and 81 deltas become unrecoverable). So this always uses
# --all with no implied base, and verifies the result says "complete history"
# before handing it over.
#
# usage:  ./export-bundle.sh <summary-slug> [repo]
#   e.g.  ./export-bundle.sh trap-merge
#         -> /mnt/user-data/outputs/c9-001-trap-merge.bundle
#         SERIES=c12unified ./export-bundle.sh trap-merge
#         -> /mnt/user-data/outputs/c12unified-001-trap-merge.bundle
#
# NNN auto-increments from the highest <SERIES>-NNN already in the output dir,
# so each series numbers independently. SERIES defaults to c9 for continuity
# with CLAUDE.md §9 and is an ENV VAR, not a hardcoded prefix — worth saying
# out loud because a c12 session read the default as a hardcoded one, exported
# a c9-NNN file and renamed it by hand.
set -euo pipefail

SUMMARY="${1:?usage: export-bundle.sh <summary-slug> [repo]}"
# Default to the repo THIS SCRIPT lives in. The old default was a hardcoded
# /home/claude/repo, which does not exist when the clone lands elsewhere —
# and the same trap made lock_check/orphan_check report success on scanning
# nothing (see C11_TODO §2). A tool inside the repo already knows where it is.
REPO="${2:-$(cd "$(dirname "$0")/../.." && pwd)}"
OUT="${OUT_DIR:-/mnt/user-data/outputs}"
SERIES="${SERIES:-c9}"
LIMIT_MIB="${LIMIT_MIB:-500}"

# slug hygiene: the filename is the changelog, so keep it readable and safe.
SUMMARY="$(printf '%s' "$SUMMARY" | tr '[:upper:] _' '[:lower:]--' | tr -cd 'a-z0-9-' | sed 's/--*/-/g; s/^-//; s/-$//')"
[ -n "$SUMMARY" ] || { echo "summary slug reduced to nothing" >&2; exit 1; }

mkdir -p "$OUT"
# The sequence comes from BOTH the output directory and a stamp file, and the
# stamp is why. Reading the directory alone means deleting an old bundle — to
# free disk, which this sandbox needs to do constantly — resets the counter: a
# session that had shipped 006 exported the next one as 001. The number is
# supposed to say which drop this is, and a second 001 makes two bundles
# indistinguishable by name.
STAMP="$OUT/.${SERIES}-seq"
last_dir=$(ls "$OUT" 2>/dev/null | sed -n "s/^${SERIES}-\([0-9]\{3\}\)-.*\.bundle$/\1/p" | sort -n | tail -1)
last_stamp=$(cat "$STAMP" 2>/dev/null || true); last_stamp=$(tr -dc '0-9' <<<"$last_stamp")
last=$(printf '%s\n%s\n' "${last_dir:-0}" "${last_stamp:-0}" | sort -n | tail -1)
next=$(printf '%03d' $(( 10#${last:-0} + 1 )))
printf '%s\n' "$next" > "$STAMP"
path="$OUT/${SERIES}-${next}-${SUMMARY}.bundle"

cd "$REPO"

# A dirty tree in a bundle is a lie: the bundle carries commits, so uncommitted
# work silently does not travel. Refuse rather than ship a partial export.
if [ -n "$(git status --porcelain)" ]; then
  echo "working tree is dirty — commit or stash first:" >&2
  git status --short >&2
  exit 1
fi

echo "consolidating packs (REPO_SIZE.md step 1)…"
# STDIN CLOSED, and the reason is narrower than an earlier version of this
# comment claimed.
#
# `git gc` intermittently died with `fatal: bad revision '<some previous
# command's output>'` followed by `failed to run repack`. That reads like a
# corrupt pack index and is not: `git fsck` is clean, `git repack -a -d -l`
# alone succeeds, and a traced `git gc` run succeeds too.
#
# It only failed when invoked inside a chained shell command, and the text it
# quoted was stale output from a command several steps earlier — including once
# when stdin was ALREADY redirected from /dev/null, which rules out the simple
# "repack ate the caller's stdin" story I first wrote here. The remaining
# suspect is `gc`'s cruft pass: `pack-objects --cruft` reads object names on
# stdin, so an inherited stream with anything in it is a hazard whatever put it
# there.
#
# `< /dev/null` is cheap and correct regardless, and this script's own
# invocation — a plain line in a script file — is the shape that has never
# failed. Verified after the fact rather than assumed.
git gc --quiet < /dev/null

# refs/notes/toolcalls is NOT included by --all. Omitting it is failure mode #3
# in SKILL_session_capture.md: the receiving clone shows every observed_calls
# as null and looks exactly like a repo where capture was never enabled.
# WRITE LOCALLY, THEN COPY. /mnt/user-data/outputs is a FUSE (rclone) mount, and
# `git bundle create` writing straight to it dies with
#   fatal: sha1 file '<stdout>' write error: Bad file descriptor
#   error: pack-objects died
# after having already created a zero-length file — so the failure looks like a
# corrupt bundle rather than an unsupported filesystem. Staging on local disk
# also means verify runs against the bytes git wrote, before anything crosses
# the mount.
stage="$(mktemp -d)/$(basename "$path")"
trap 'rm -rf "$(dirname "$stage")"' EXIT

echo "writing $stage"
git bundle create "$stage" --all refs/notes/toolcalls

echo "verifying…"
git bundle verify "$stage" | tail -4

size_mib=$(( $(stat -c %s "$stage") / 1024 / 1024 ))
echo "size: ${size_mib} MiB (budget ${LIMIT_MIB} MiB)"
if [ "$size_mib" -gt "$LIMIT_MIB" ]; then
  echo "OVER BUDGET — see maintenance/REPO_SIZE.md before sending" >&2
  exit 2
fi

echo "copying to $path"
cp "$stage" "$path"
# Confirm the copy survived the crossing: a truncated write on a network mount
# is silent, and the bundle would fail verify on the far side instead of here.
if [ "$(stat -c %s "$stage")" != "$(stat -c %s "$path")" ]; then
  echo "copy is short: $(stat -c %s "$path") of $(stat -c %s "$stage") bytes" >&2
  exit 3
fi

notes=$(git notes --ref=toolcalls list | wc -l)
echo "commits: $(git rev-list --count --all)   toolcall notes: $notes"
echo "$path"
