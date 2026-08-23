#!/usr/bin/env bash
# feature_sweep.sh — check every package ON ITS OWN, to find crates that only
# compile because a sibling enables a feature for them.
#
# THE BUG CLASS. A whole-workspace build UNIFIES features: if `snapclone-relay`
# turns on tokio's `sync`, then `snapclone-nat` compiles without declaring it
# and nobody notices until someone builds that crate alone — offline, on a clean
# clone, or in CI. It has been hit twice in this tree (`snapclone-nat`'s tokio
# `sync`, and `scene-kit` independently in c8parallel), which is the reason to
# assume two is not all of them.
#
# C9-2e's stale-lock checker could be cheap and static because absence is
# visible in a text file. This one cannot: it needs a real per-crate resolve,
# which is 30-60 s per package and hours for the tree. So the design goal here
# is not speed, it is being able to WALK AWAY and come back — and to trust the
# result when a run was cut short, rather than mistaking "not reached" for
# "passed".
#
#   maintenance/tools/feature_sweep.sh              # start or resume
#   maintenance/tools/feature_sweep.sh --list       # what would be checked
#   maintenance/tools/feature_sweep.sh --report     # summarise what ran
#   maintenance/tools/feature_sweep.sh --retry-fail # re-run only the failures
#
# RESUMABLE. Every result is appended to the report as it happens, and a rerun
# skips paths already recorded. Killing this at any point loses at most the
# crate in flight; nothing has to be redone.
#
# Exit: 0 all checked packages passed, 1 at least one FAILed, 2 nothing to do.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && cd .. && pwd)"
REPORT="${FEATURE_SWEEP_REPORT:-$ROOT/maintenance/.feature_sweep.tsv}"
TIMEOUT="${FEATURE_SWEEP_TIMEOUT:-180}"
AILITE="$ROOT/maintenance/tools/cargo-ailite"
BUILD_DIR="${FEATURE_SWEEP_BUILD_DIR:-/tmp/feature-sweep-bd}"

# Known native-dependency gates, from CLAUDE.md §3. These are NOT failures of
# the crate; they are this sandbox missing alsa/cmake/Qt/GTK. Without them the
# sweep reports noise, and a checker that reports noise gets ignored — which is
# how the last one would have failed.
extra_flags() {
  case "$1" in
    apps/beatspace|games/blackstar-rs) echo "--no-default-features" ;;
    apps/studio/*studio-media*)        echo "--no-default-features" ;;
    *) echo "" ;;
  esac
}
extra_env() {
  case "$1" in
    */snapclone-crypto) echo "SNAPCLONE_SKIP_CBINDGEN=1" ;;
    *) echo "" ;;
  esac
}

# Every directory holding a real [package]. wip/ and target/ are excluded: the
# first is staged reference material that is not ours to keep compiling, the
# second is build output.
packages() {
  find "$ROOT/apps" "$ROOT/shared" "$ROOT/system" "$ROOT/games" "$ROOT/tools" \
       -name Cargo.toml -not -path '*/target/*' -not -path '*/wip/*' 2>/dev/null \
  | while IFS= read -r f; do
      grep -q '^\[package\]' "$f" || continue          # skip virtual workspaces
      printf '%s\n' "${f%/Cargo.toml}" | sed "s|^$ROOT/||"
    done | sort -u
}

case "${1:-}" in
  --list) packages; exit 0 ;;
  --report)
    [ -f "$REPORT" ] || { echo "no report at $REPORT — nothing has run"; exit 2; }
    total=$(packages | wc -l)
    done_n=$(grep -c . "$REPORT")
    printf 'checked %s of %s package(s)\n' "$done_n" "$total"
    for st in FAIL TIMEOUT SKIP ok; do
      n=$(awk -F'\t' -v s="$st" '$2==s' "$REPORT" | wc -l)
      printf '  %-8s %s\n' "$st" "$n"
      [ "$st" = ok ] || awk -F'\t' -v s="$st" '$2==s {print "           " $1}' "$REPORT"
    done
    # The honest part: name what was never reached, so a partial run is not
    # read as a clean one.
    missing=$(comm -23 <(packages) <(cut -f1 "$REPORT" | sort -u))
    if [ -n "$missing" ]; then
      printf 'NOT REACHED (%s):\n' "$(printf '%s\n' "$missing" | wc -l)"
      printf '%s\n' "$missing" | sed 's/^/           /'
    fi
    exit 0 ;;
  --retry-fail)
    [ -f "$REPORT" ] || { echo "no report at $REPORT"; exit 2; }
    keep=$(awk -F'\t' '$2=="ok"' "$REPORT")
    # `printf '%s\n' ""` on an empty result would write a blank line, and a
    # blank first field becomes a package path that is skipped forever.
    if [ -n "$keep" ]; then printf '%s\n' "$keep" > "$REPORT"; else : > "$REPORT"; fi
    dropped=$(( $(grep -c . <<< "$(cat "$REPORT")" ) ))
    echo "kept $dropped ok row(s); rerun with no arguments to retry the rest"
    # MUST exit. Without it this fell through into the main sweep and started
    # re-checking all 257 remaining packages, which is a several-hour surprise
    # from a command whose whole job is to prepare a rerun.
    exit 0 ;;
  "") ;;
  *) echo "unknown option: $1" >&2; exit 2 ;;
esac

mkdir -p "$(dirname "$REPORT")"; touch "$REPORT"
todo=$(comm -23 <(packages) <(cut -f1 "$REPORT" | sort -u))
[ -n "$todo" ] || { echo "nothing left to check — $REPORT is complete"; exit 0; }

echo "sweep: $(printf '%s\n' "$todo" | wc -l) package(s) left, ${TIMEOUT}s each, report $REPORT"
fails=0
while IFS= read -r rel; do
  [ -n "$rel" ] || continue
  dir="$ROOT/$rel"
  flags=$(extra_flags "$rel"); envs=$(extra_env "$rel")
  printf '%-52s ' "$rel"
  start=$(date +%s)
  # Each crate gets its own build dir, deleted immediately: disk is what fails
  # in this sandbox, and a sweep is the longest-running way to discover that.
  rm -rf "$BUILD_DIR"
  if ! cd "$dir" 2>/dev/null; then
    printf '%s\t%s\t%s\n' "$rel" "SKIP" "0" >> "$REPORT"; echo "SKIP (no dir)"; continue
  fi
  # shellcheck disable=SC2086
  env $envs AILITE_BUILD_DIR="$BUILD_DIR" timeout "$TIMEOUT" "$AILITE" check $flags >/dev/null 2>&1
  rc=$?
  el=$(( $(date +%s) - start ))
  case $rc in
    0)   st=ok ;;
    124) st=TIMEOUT ;;
    *)   st=FAIL; fails=$(( fails + 1 )) ;;
  esac
  printf '%s\t%s\t%s\n' "$rel" "$st" "$el" >> "$REPORT"
  echo "$st (${el}s)"
  cd "$ROOT" || exit 1
done <<< "$todo"
rm -rf "$BUILD_DIR"

echo "--- $fails failure(s) this run; full picture: $0 --report"
[ "$fails" -eq 0 ]
