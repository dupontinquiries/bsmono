#!/usr/bin/env bash
# apply_and_test.sh — extract every unendlich brick zip in dependency order,
# tag each one in git, run its tests, and log which tag applied and passed.
# Optionally syncs with the remote (pull before, push commits+tags after).
#
# The log is framed with "==== VERSION <tag> START/END ====" markers, the same
# format the telemetry dashboard segments on, so you can load it as a run and
# see per-tag results.
#
# Override anything via env vars, e.g.:
#   GIT_SYNC=0 ./apply_and_test.sh          # apply + test only, no git
#   FEATURES=render ./apply_and_test.sh     # different feature set
set -uo pipefail

ZIPS_DIR="${ZIPS_DIR:-$HOME/Code/unendlich-development/claude-zips}"
REPO="${REPO:-$HOME/Code/unendlich-development/unendlich-game}"
PKG="${PKG:-unendlich-engine}"
FEATURES="${FEATURES:-render,gpu}"
BRANCH="${BRANCH:-rc29-adaptive-uploads}"
GIT_SYNC="${GIT_SYNC:-1}"   # 1 = commit + tag each brick
GIT_PULL="${GIT_PULL:-1}"   # 1 = pull --rebase before starting (needs clean tree)
GIT_PUSH="${GIT_PUSH:-1}"   # 1 = push commits + tags at the end
LOG="${LOG:-$REPO/brick_apply_log.txt}"

# Dependency-ordered bricks:  zip-stem | cargo-test-filter | git-tag-suffix
# Each brick's dependencies were shipped earlier, so intermediate states compile.
BRICKS=(
  "unendlich-navfield-pathing-flat|sim::cognition::navigate|navfield-pathing"
  "unendlich-cognition-mind-flat|sim::cognition::mind|cognition-mind"
  "unendlich-mob-commanded-flat|sim::mob|mob-commanded"
  "unendlich-cognition-service-flat|sim::cognition::service|cognition-service"
  "unendlich-lumen-flat|world::lumen|lumen"
  "unendlich-stem-flat|world::stem|stem"
  "unendlich-cave-flora-flat|world::cave_flora|cave-flora"
  "unendlich-flora-sites-flat|world::flora_sites|flora-sites"
  "unendlich-chunk-flora-flat|world::chunk_flora|chunk-flora"
  "unendlich-cognitive-sim-flat|sim::cognition::cognitive_sim|cognitive-sim"
  "unendlich-gpu-bench-flat|sim::gpu_bench|gpu-bench"
  "unendlich-stem-gpu-flat|world::stem_gpu|stem-gpu"
  "unendlich-lumen-gpu-flat|world::lumen|lumen-gpu"
)

ts()  { date '+%Y-%m-%dT%H:%M:%S%z'; }
say() { printf '%s\n' "$*" | tee -a "$LOG"; }

[ -d "$REPO" ] || { echo "REPO not found: $REPO"; exit 1; }
cd "$REPO" || exit 1
: > "$LOG"
say "==== RUN START $(ts) ===="
say "repo=$REPO"
say "zips=$ZIPS_DIR  pkg=$PKG  features=$FEATURES  branch=$BRANCH"

if [ "$GIT_SYNC" = 1 ] && [ "$GIT_PULL" = 1 ]; then
  say "-- git pull --rebase origin $BRANCH"
  git pull --rebase origin "$BRANCH" 2>&1 | tee -a "$LOG"
fi

pass=0; fail=0; missing=0
for entry in "${BRICKS[@]}"; do
  IFS='|' read -r stem filter tag <<<"$entry"
  zip=$(ls -t "$ZIPS_DIR"/${stem}*.zip 2>/dev/null | head -n1)   # newest match; tolerates "(1)" suffixes
  say ""
  say "==== VERSION $tag START $(ts) ===="
  if [ -z "$zip" ]; then
    say "  [skip] no zip matching ${stem}*.zip"
    missing=$((missing+1)); say "==== VERSION $tag END $(ts) ===="; continue
  fi
  say "  apply: $(basename "$zip")"
  unzip -o "$zip" -d "$REPO" >>"$LOG" 2>&1

  if [ "$GIT_SYNC" = 1 ]; then
    git add -A
    if git diff --cached --quiet; then
      say "  git: no changes (already applied?)"
    else
      git commit -q -m "apply brick: $tag" && say "  git: committed"
      git tag -f "brick/$tag" >/dev/null 2>&1 && say "  git: tagged brick/$tag"
    fi
  fi

  say "  test: cargo test --release --lib -p $PKG --features $FEATURES $filter"
  if cargo test --release --lib -p "$PKG" --features "$FEATURES" "$filter" >>"$LOG" 2>&1; then
    say "  RESULT: PASS  ($tag)"; pass=$((pass+1))
  else
    say "  RESULT: FAIL  ($tag)  — search '$tag' in $LOG"; fail=$((fail+1))
  fi
  say "==== VERSION $tag END $(ts) ===="
done

say ""
say "==== RUN END $(ts) ====  pass=$pass fail=$fail missing=$missing"

if [ "$GIT_SYNC" = 1 ] && [ "$GIT_PUSH" = 1 ]; then
  say "-- git push origin $BRANCH (+ tags)"
  git push origin "$BRANCH"  2>&1 | tee -a "$LOG"
  git push origin --tags     2>&1 | tee -a "$LOG"
fi
say "done. full log: $LOG"
